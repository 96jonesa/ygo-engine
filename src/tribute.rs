//! Tributes and forced zones: what a summon may spend, and where it may go.
//!
//! The primitives underneath `SummonRule` and `MonsterSet`. Nothing here is a
//! processor unit; these are the questions those machines ask before they
//! offer a summon at all.
//!
//! ## `get_forced_zones` closes a stub the port has carried
//!
//! `get_tofield_count` has been computing its seat mask against a constant
//! `NO_FORCED_ZONES = 0xff`, with a comment saying it must become a real call
//! when the forcing effects land. This is that call.
//!
//! ## The two-row packing, once more, with a twist
//!
//! `EFFECT_MUST_USE_MZONE`'s value carries both players' seats — low half
//! for the effect's own side, high half for the opponent's. `get_forced_zones`
//! ends by **selecting one half**:
//!
//! ```text
//! if uplayer == playerid or uplayer > 1:  res &= 0xff          // own seats
//! else:                                   res = (res >> 16) & 0xff
//! ```
//!
//! The `uplayer > 1` case is not a guard against bad input — `PLAYER_NONE`
//! reaching here means "nobody in particular is asking", and the reference
//! treats that as the owning player's view.
//!
//! ## `release_param` lives on the card, not in the return
//!
//! `get_summon_release_list` writes how many tributes each card counts as
//! onto the card itself, and returns only the total. That is deliberate: the
//! caller that later *spends* the tributes needs the per-card value, and it
//! is not the same caller.

use crate::board::{location, position};
use crate::effect::{flag, Ctx};
use crate::event::{code, CardId, EffectId, PLAYER_NONE};
use crate::field::Field;
use std::collections::BTreeSet;

/// What `get_summon_release_list` found, split the three ways its callers
/// need.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReleaseLists {
    /// Cards that may be tributed normally.
    pub release: BTreeSet<CardId>,
    /// The opponent's cards that `EFFECT_EXTRA_RELEASE` makes freely
    /// tributable — counted separately because `check_tribute` subtracts
    /// them from the maximum rather than counting them toward it.
    pub extra: BTreeSet<CardId>,
    /// Cards that `EFFECT_EXTRA_RELEASE_SUM` allows **one of**. Only the
    /// largest contributes to the total, which is what "one of" means when
    /// they are worth different amounts.
    pub extra_one_of: BTreeSet<CardId>,
}

impl Field {
    /// `field::save_lp_cost` / `restore_lp_cost`.
    pub fn save_lp_cost(&mut self) {
        for p in 0..2usize {
            let c = &mut self.core.lp_cost[p];
            if c.count < 8 {
                c.stack[c.count as usize] = c.amount;
            }
            c.count += 1;
        }
    }

    /// The counterpart. Note the counter decrements even when it is past the
    /// stack's end, so nesting deeper than eight restores a stale value
    /// rather than the wrong one — the reference's choice.
    pub fn restore_lp_cost(&mut self) {
        for p in 0..2usize {
            let c = &mut self.core.lp_cost[p];
            c.count = c.count.saturating_sub(1);
            if c.count < 8 {
                c.amount = c.stack[c.count as usize];
            }
        }
    }

    /// `field::get_forced_zones` — which Monster Zone seats an effect
    /// *insists* a summon use.
    ///
    /// Returns a mask of **permitted** seats, so `0xff` (every seat) means
    /// nothing is forcing anything. Only the Monster Zone can be forced;
    /// every other location returns `0xff` immediately.
    ///
    /// The accumulation is an **AND**, not an OR: two effects each insisting
    /// on a set of seats leave only the seats both allow, and two
    /// incompatible ones leave none. That is the opposite of how the
    /// `disabled_location` masks combine, and it follows from these being
    /// requirements rather than prohibitions.
    ///
    /// An effect with an `operation` uses it as a **condition** — the
    /// reference calls `check_condition(peff->operation, 4)` — which is the
    /// reuse the port models with `operation_filter`.
    pub fn get_forced_zones(
        &mut self,
        card: Option<CardId>,
        playerid: u8,
        loc: u8,
        uplayer: u8,
        reason: u32,
    ) -> u32 {
        if loc != location::MZONE {
            return 0xff;
        }
        let mut effects: Vec<EffectId> = Vec::new();
        if uplayer < 2 {
            effects.extend(self.filter_player_effect(uplayer, code::MUST_USE_MZONE));
        }
        if let Some(c) = card {
            effects.extend(self.filter_effect(c, code::MUST_USE_MZONE));
        }

        let mut res = 0xff7f_ff7fu32;
        for e in effects {
            let spent = self
                .effects
                .get(e)
                .is_some_and(|x| x.is_flag(crate::effect::flag::COUNT_LIMIT) && x.count_limit == 0);
            if spent {
                continue;
            }
            // `operation` read as a condition.
            let declined = self
                .effects
                .get(e)
                .and_then(|x| x.operation_filter)
                .is_some_and(|f| !f(self, e, card, 0, playerid));
            if declined {
                continue;
            }
            let value = self.forced_zone_value(e, card, playerid, uplayer, reason) as u32;
            res &= value;
        }
        if uplayer == playerid || uplayer > 1 {
            res & 0xff
        } else {
            (res >> 16) & 0xff
        }
    }

    fn forced_zone_value(
        &self,
        effect: EffectId,
        card: Option<CardId>,
        playerid: u8,
        uplayer: u8,
        reason: u32,
    ) -> i64 {
        let Some(e) = self.effects.get(effect) else {
            return 0xff7f_ff7f;
        };
        let ev = crate::event::Event::new(0);
        let ctx = crate::effect::Ctx {
            reason_effect: effect,
            player: playerid,
            event: &ev,
            // A player-target effect is asked without a card; otherwise the
            // card is pushed ahead of the three integers.
            card: if e.is_flag(crate::effect::flag::PLAYER_TARGET) {
                None
            } else {
                card
            },
            args: &[i64::from(playerid), i64::from(uplayer), i64::from(reason)],
        };
        e.get_value(self, &ctx)
    }

    /// `field::get_summon_release_list` — what this summon could tribute,
    /// and how much it is worth.
    ///
    /// Three passes, and they are not the same question.
    ///
    /// **The summoner's own Monster Zone.** Anything releasable and allowed
    /// by the `releasable` mask. Each card's `release_param` is set to 3, 2
    /// or 1 by `TRIPLE_TRIBUTE` / `DOUBLE_TRIBUTE`, and the total accumulates.
    ///
    /// **The opponent's Monster Zone**, whose seats are addressed by the
    /// `releasable` mask **shifted up 16** — the same two-row packing as
    /// everywhere else. A card there lands in one of three places:
    /// `EXTRA_RELEASE` makes it freely tributable and it goes to `extra`;
    /// otherwise `ex` (a summon onto the opponent's side) or an
    /// `ADD_EXTRA_TRIBUTE` naming it puts it in the ordinary list; otherwise
    /// `EXTRA_RELEASE_SUM` puts it in `extra_one_of`, where **only the
    /// largest counts**.
    ///
    /// **The `ADD_EXTRA_TRIBUTE` cards that are not in a Monster Zone**, which
    /// is how a card in the hand or graveyard becomes tributable. The two
    /// earlier passes skipped them precisely because they are not on the
    /// field.
    /// `field::get_release_list` — the pool for a release that is **not**
    /// a tribute summon (`field.cpp:1790`).
    ///
    /// The sibling of [`Field::get_summon_release_list`], and a much
    /// plainer walk: the controller's Monster Zone, its hand when
    /// `use_hand`, and then the opponent's row.
    ///
    /// ## The opponent's row is the whole of the difference
    ///
    /// With `use_oppo` the opponent's monsters join the ordinary list
    /// outright — that is a card saying "release monsters on the field",
    /// either side. Without it they are only reachable through
    /// `EFFECT_EXTRA_RELEASE` (the `extra` list) or
    /// `EFFECT_EXTRA_RELEASE_NONSUM` (the **one-of** list, and only when
    /// its count limit is unspent and its value condition holds); a
    /// monster with neither is skipped entirely. That is what stops an
    /// ordinary cost tributing the opponent's board.
    ///
    /// **No card in this pool has either effect** — checked by
    /// `tools/check_constants.py`'s `ABSENT_FROM_POOL` scan — so for this
    /// pool the `else` arm contributes nothing at all. It is transcribed
    /// because leaving it out would make the walk read as though the
    /// opponent's row were simply not scanned.
    ///
    /// The face-up test on the opponent's row is `is_position(POS_FACEUP)
    /// || !fun`: with **no** filter a face-down monster is in the pool,
    /// and with one it is not. Not a typo — an unfiltered ask is about
    /// count, and a filtered one would have to look at a card it may not
    /// see.
    ///
    /// Returns the count the reference returns: the ordinary matches plus
    /// **one** for the one-of list however many are in it.
    #[allow(clippy::too_many_arguments)]
    pub fn get_release_list(
        &mut self,
        playerid: u8,
        filter: Option<crate::script_api::Filter>,
        use_hand: bool,
        use_oppo: bool,
        exc: Option<CardId>,
        exg: Option<&BTreeSet<CardId>>,
        reason: u32,
    ) -> (u32, ReleaseLists) {
        let mut lists = ReleaseLists::default();
        let mut rcount = 0u32;
        let excluded = |c: CardId| Some(c) == exc || exg.is_some_and(|g| g.contains(&c));

        let mut own: Vec<CardId> = self.players[playerid as usize]
            .mzone
            .iter()
            .flatten()
            .copied()
            .collect();
        if use_hand {
            own.extend(self.players[playerid as usize].hand.iter().copied());
        }
        for c in own {
            if excluded(c)
                || !self.is_releasable_by_nonsummon(c, playerid, reason)
                || !filter.is_none_or(|f| f(self, c))
            {
                continue;
            }
            lists.release.insert(c);
            self.cards[c].release_param = 1;
            rcount += 1;
        }

        let mut ex_oneof_max = 0u32;
        let theirs: Vec<CardId> = self.players[1 - playerid as usize]
            .mzone
            .iter()
            .flatten()
            .copied()
            .collect();
        for c in theirs {
            if excluded(c)
                || !(filter.is_none() || self.cards[c].current.position & position::FACEUP != 0)
                || !self.is_releasable_by_nonsummon(c, playerid, reason)
                || !filter.is_none_or(|f| f(self, c))
            {
                continue;
            }
            self.cards[c].release_param = 1;
            if use_oppo {
                lists.release.insert(c);
                rcount += 1;
                continue;
            }
            if self.is_affected_by_effect(c, code::EXTRA_RELEASE).is_some() {
                lists.extra.insert(c);
                rcount += 1;
                continue;
            }
            let Some(e) = self.is_affected_by_effect(c, code::EXTRA_RELEASE_NONSUM) else {
                continue;
            };
            let spent = self
                .effects
                .get(e)
                .is_some_and(|x| x.is_flag(flag::COUNT_LIMIT) && x.count_limit == 0);
            if spent {
                continue;
            }
            // `add_param(reason_effect); add_param(reason);
            // add_param(reason_player); check_value_condition(3)` — the
            // three the reference pushes before asking.
            let args = [
                self.core.reason_effect.map_or(-1, |x| x as i64),
                i64::from(reason),
                i64::from(self.core.reason_player),
            ];
            let ev = crate::event::Event::new(0);
            let ctx = Ctx {
                reason_effect: e,
                player: self.core.reason_player,
                event: &ev,
                card: Some(c),
                args: &args,
            };
            let holds = self
                .effects
                .get(e)
                .is_some_and(|x| x.check_value_condition(self, &ctx));
            if !holds {
                continue;
            }
            lists.extra_one_of.insert(c);
            ex_oneof_max = 1;
        }
        (rcount + ex_oneof_max, lists)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn get_summon_release_list(
        &mut self,
        target: CardId,
        group: Option<&BTreeSet<CardId>>,
        ex: bool,
        releasable: u32,
        pos: u32,
    ) -> (u32, ReleaseLists) {
        let p = self.cards[target].current.controller;
        let mut lists = ReleaseLists::default();

        // Cards an effect adds to the tribute pool, if its value covers the
        // position being summoned into.
        let mut ex_tribute: BTreeSet<CardId> = BTreeSet::new();
        for e in self.filter_effect(target, code::ADD_EXTRA_TRIBUTE) {
            if self.effect_value_for_card_pub(e, target) as u32 & pos != 0 {
                for c in self.filter_inrange_cards(e) {
                    ex_tribute.insert(c);
                }
            }
        }

        let mut rcount = 0u32;
        let own: Vec<CardId> = self.players[p as usize]
            .mzone
            .iter()
            .flatten()
            .copied()
            .collect();
        for card in own {
            let seq = self.cards[card].current.sequence;
            if (releasable >> seq) & 1 == 0 || !self.is_releasable_by_summon(card, p, target) {
                continue;
            }
            if group.is_some_and(|g| !g.contains(&card)) {
                continue;
            }
            lists.release.insert(card);
            rcount += self.set_release_param(card, target);
        }

        let mut ex_oneof_max = 0u32;
        let theirs: Vec<CardId> = self.players[1 - p as usize]
            .mzone
            .iter()
            .flatten()
            .copied()
            .collect();
        for card in theirs {
            let seq = self.cards[card].current.sequence;
            // The opponent's seats live 16 bits up.
            if (releasable >> (seq + 16)) & 1 == 0 || !self.is_releasable_by_summon(card, p, target)
            {
                continue;
            }
            if group.is_some_and(|g| !g.contains(&card)) {
                continue;
            }
            let param = self.set_release_param(card, target);
            if self
                .is_affected_by_effect(card, code::EXTRA_RELEASE)
                .is_some()
            {
                lists.extra.insert(card);
                rcount += param;
            } else if ex || ex_tribute.contains(&card) {
                lists.release.insert(card);
                rcount += param;
            } else {
                let allows = self.is_affected_by_effect(card, code::EXTRA_RELEASE_SUM);
                let usable = allows.is_some_and(|e| {
                    !self.effects.get(e).is_some_and(|x| {
                        x.is_flag(crate::effect::flag::COUNT_LIMIT) && x.count_limit == 0
                    })
                });
                if !usable {
                    continue;
                }
                lists.extra_one_of.insert(card);
                // Only the largest of these counts — "one of" them.
                ex_oneof_max = ex_oneof_max.max(param);
            }
        }

        for card in ex_tribute {
            if self.cards[card].current.location == location::MZONE
                || !self.is_releasable_by_summon(card, p, target)
            {
                continue;
            }
            lists.release.insert(card);
            rcount += self.set_release_param(card, target);
        }
        (rcount + ex_oneof_max, lists)
    }

    /// Write how many tributes a card counts as, and return it.
    fn set_release_param(&mut self, card: CardId, target: CardId) -> u32 {
        let param = if self
            .is_affected_by_effect_against(card, code::TRIPLE_TRIBUTE, target)
            .is_some()
        {
            3
        } else if self
            .is_affected_by_effect_against(card, code::DOUBLE_TRIBUTE, target)
            .is_some()
        {
            2
        } else {
            1
        };
        self.cards[card].release_param = param;
        param
    }

    /// `field::filter_inrange_cards` — every card in an effect's own range.
    ///
    /// Returns nothing for a player-target or special-summon-parameter
    /// effect: neither addresses cards by range, and asking would give the
    /// wrong set rather than an empty one.
    pub fn filter_inrange_cards(&self, effect: EffectId) -> Vec<CardId> {
        let Some(e) = self.effects.get(effect) else {
            return Vec::new();
        };
        if e.is_flag(crate::effect::flag::PLAYER_TARGET | crate::effect::flag::SPSUM_PARAM) {
            return Vec::new();
        }
        let self_player = e.get_handler_player(&self.cards);
        if self_player == PLAYER_NONE {
            return Vec::new();
        }
        let range = e.s_range;
        let mut out = Vec::new();
        for p in [self_player as usize, 1 - self_player as usize] {
            let z = &self.players[p];
            for (bit, cards) in [
                (location::MZONE, None),
                (location::SZONE, None),
                (location::GRAVE, Some(&z.grave)),
                (location::REMOVED, Some(&z.removed)),
                (location::HAND, Some(&z.hand)),
                (location::DECK, Some(&z.main)),
                (location::EXTRA, Some(&z.extra)),
            ] {
                if range & u16::from(bit) == 0 {
                    continue;
                }
                match cards {
                    Some(pile) => out.extend(pile.iter().copied()),
                    None if bit == location::MZONE => out.extend(z.mzone.iter().flatten().copied()),
                    None => out.extend(z.szone.iter().flatten().copied()),
                }
            }
        }
        out
    }
}

impl Field {
    /// `field::check_tribute` — could this summon actually be paid for?
    ///
    /// Asked before a summon is offered, so its job is to answer "is there
    /// *some* legal set of tributes", not to pick one.
    ///
    /// **`ex` is derived, not passed**: summoning onto the opponent's side
    /// makes their monsters ordinarily tributable, and that is decided here
    /// by comparing `toplayer` with the summoning card's controller.
    ///
    /// The seat count is the subtle part. A tribute in a Monster Zone
    /// **frees the seat it was in**, so each one that sits on the destination
    /// player's side and in an allowed zone adds to the available count. That
    /// is why a monster needing two tributes can be summoned onto a full
    /// field: the tributes make the room.
    ///
    /// `ex_list` is **subtracted from the maximum** rather than counted
    /// toward it — those are the opponent's monsters taken by
    /// `EFFECT_EXTRA_RELEASE`, which do not satisfy a tribute requirement.
    ///
    /// The two `-fcount + 1` tests are the Monster Zone limit expressed as a
    /// negative: `get_mzone_limit` can return a negative number when the
    /// player is *over* the limit, and the summon then needs at least that
    /// many tributes before it is even possible.
    #[allow(clippy::too_many_arguments)]
    pub fn check_tribute(
        &mut self,
        card: CardId,
        min: i32,
        max: i32,
        group: Option<&BTreeSet<CardId>>,
        toplayer: u8,
        zone: u32,
        releasable: u32,
        pos: u32,
    ) -> bool {
        let sumplayer = self.cards[card].current.controller;
        let ex = toplayer == 1 - sumplayer;
        let (m, lists) = self.get_summon_release_list(card, group, ex, releasable, pos);

        let mut max = max.min(m as i32);
        if min > max {
            return false;
        }
        let forced = self.get_forced_zones(
            Some(card),
            toplayer,
            location::MZONE,
            sumplayer,
            Field::LOCATION_REASON_TOFIELD,
        );
        let zone = zone & 0x1f & forced;

        let mut s = 0i32;
        let mut ct = self.get_tofield_count(
            Some(card),
            toplayer,
            location::MZONE,
            sumplayer,
            Field::LOCATION_REASON_TOFIELD,
            zone,
        );
        if ct <= 0 && max <= 0 {
            return false;
        }
        // The list to count seats from: the extra-release cards if there are
        // enough of them to satisfy the minimum on their own, otherwise the
        // ordinary one. Not a union — the reference picks one.
        let to_check = if !lists.extra.is_empty() && lists.extra.len() as i32 >= min {
            &lists.extra
        } else {
            &lists.release
        };
        for &c in to_check {
            if self.cards[c].current.location == location::MZONE
                && self.cards[c].current.controller == toplayer
            {
                s += 1;
                // Tributing frees the seat, so it becomes available.
                if (zone >> self.cards[c].current.sequence) & 1 != 0 {
                    ct += 1;
                }
            }
        }
        if ct <= 0 {
            return false;
        }
        max -= lists.extra.len() as i32;
        let fcount = self.get_mzone_limit(toplayer, sumplayer, Field::LOCATION_REASON_TOFIELD);
        if s < -fcount + 1 {
            return false;
        }
        let max = max.max(0);
        // `max >= -fcount + 1`, as the reference writes it. Clippy prefers
        // `max > -fcount`; kept in the reference's form because the `+ 1`
        // is the same "one more than the overage" the test above uses, and
        // the two reading alike is the point.
        #[allow(clippy::int_plus_one)]
        {
            max >= -fcount + 1
        }
    }
}

/// What a `filter_*_procedure` found.
///
/// Not a `Vec` and a flag, because the reference's three-way answer does not
/// decompose into those: `-2` and an empty list mean different things.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Procedures {
    /// A `LIMIT_*_PROC` effect applies and **replaces** the ordinary summon
    /// entirely. The listed procedures are the only ones available.
    Limited(Vec<EffectId>),
    /// A `LIMIT_*_PROC` effect applies and none of its procedures are
    /// usable. The monster cannot be summoned at all — which is *not* the
    /// same as an empty ordinary list, where the monster may still be
    /// summoned normally.
    Forbidden,
    /// The ordinary summon is available (`ordinary`), alongside any
    /// `SUMMON_PROC` effects that apply.
    Ordinary {
        procedures: Vec<EffectId>,
        ordinary: bool,
    },
}

impl Field {
    /// `field::get_summon_count_limit`.
    pub fn get_summon_count_limit(&mut self, playerid: u8) -> i32 {
        if self.is_flag(crate::duel::flags::UNLIMITED_SUMMONS) {
            return i32::MAX - 100;
        }
        let mut count = 1;
        for e in self.filter_player_effect(playerid, code::SET_SUMMON_COUNT_LIMIT) {
            let c = self.effect_plain_value(e) as i32;
            if c > count {
                count = c;
            }
        }
        count
    }

    /// `card::get_summon_tribute_count` — how many tributes this monster
    /// needs, as `min | max << 16`.
    ///
    /// The base is by level: under 5 needs none, 5-6 needs one, 7+ needs two.
    ///
    /// `EFFECT_DECREASE_TRIBUTE` then reduces it, and the two kinds of
    /// reduction **do not combine the same way**:
    ///
    /// - **Without** `COUNT_LIMIT` the reductions are *not* cumulative — the
    ///   largest wins (`minul`/`maxul` take a maximum), because two effects
    ///   each saying "needs one fewer tribute" do not stack.
    /// - **With** `COUNT_LIMIT`, and only while unspent, they subtract
    ///   directly and therefore *do* stack.
    ///
    /// Writing both as subtraction is the obvious simplification and makes
    /// two independent "one fewer tribute" effects remove two.
    pub fn get_summon_tribute_count(&mut self, card: CardId) -> (i32, i32) {
        let level = self.get_level(card) as i32;
        let (mut min, mut max) = match level {
            l if l < 5 => return (0, 0),
            l if l < 7 => (1, 1),
            _ => (2, 2),
        };
        let (mut minul, mut maxul) = (0i32, 0i32);
        for e in self.filter_effect(card, code::DECREASE_TRIBUTE) {
            let dec = self.effect_value_for_card_pub(e, card) as i32;
            let counted = self
                .effects
                .get(e)
                .is_some_and(|x| x.is_flag(crate::effect::flag::COUNT_LIMIT));
            if !counted {
                // Not cumulative: the largest reduction wins.
                minul = minul.max(dec & 0xffff);
                maxul = maxul.max(dec >> 16);
            } else if self.effects.get(e).is_some_and(|x| x.count_limit > 0) {
                min -= dec & 0xffff;
                max -= dec >> 16;
            }
        }
        min -= minul;
        max -= maxul;
        let min = min.max(0);
        (min, max.max(min))
    }

    /// `card::get_set_tribute_count` — the same, for setting.
    ///
    /// Simpler in a way worth noticing: `EFFECT_DECREASE_TRIBUTE_SET` takes
    /// only the **last** effect (`eset.back()`), not a combination and not
    /// the largest. Where the summon form carefully distinguishes cumulative
    /// from non-cumulative reductions, this one just takes whichever was
    /// registered most recently.
    pub fn get_set_tribute_count(&mut self, card: CardId) -> (i32, i32) {
        let level = self.get_level(card) as i32;
        let (mut min, mut max) = match level {
            l if l < 5 => return (0, 0),
            l if l < 7 => (1, 1),
            _ => (2, 2),
        };
        if let Some(&e) = self.filter_effect(card, code::DECREASE_TRIBUTE_SET).last() {
            let dec = self.effect_value_for_card_pub(e, card) as i32;
            min -= dec & 0xffff;
            max -= dec >> 16;
        }
        let min = min.max(0);
        (min, max.max(min))
    }

    /// `field::is_player_can_summon` — may this player summon at all?
    ///
    /// **`SUMMON_TYPE_NORMAL` is ORed into whatever it is asked about**,
    /// because `ADVANCE` is `NORMAL` plus a bit rather than a sibling: a
    /// tribute summon is a normal summon, and a prohibition on normal
    /// summoning must catch it.
    ///
    /// A prohibition with **no target forbids unconditionally** — the same
    /// shape as every other `is_player_can_*`.
    ///
    /// The position half is separate and subtractive:
    /// `EFFECT_FORCE_NORMAL_SUMMON_POSITION` **narrows** the allowed
    /// positions by ANDing its value, and a narrowing to zero forbids the
    /// summon outright. The starting set is face-up attack, widened to all
    /// face-up positions by the duel option or by either of two effects.
    pub fn is_player_can_summon(
        &mut self,
        sumtype: u32,
        playerid: u8,
        card: Option<CardId>,
        toplayer: u8,
    ) -> bool {
        let sumtype = sumtype | crate::card::summon_type::NORMAL;
        for e in self.filter_player_effect(playerid, code::CANNOT_SUMMON) {
            let Some(target) = self.effects.get(e).and_then(|x| x.target_filter) else {
                // No target: unconditional.
                return false;
            };
            let args = [
                i64::from(playerid),
                i64::from(sumtype),
                i64::from(crate::board::position::FACEUP),
                i64::from(toplayer),
            ];
            if target(self, e, card, &args) {
                return false;
            }
        }

        let mut sumpos = crate::board::position::FACEUP_ATTACK;
        if self.is_flag(crate::duel::flags::NORMAL_SUMMON_FACEUP_DEF)
            || self
                .is_player_affected_by_effect(playerid, code::NORMAL_SUMMON_FACEUP_DEFENSE)
                .is_some()
            || self
                .is_player_affected_by_effect(playerid, code::DEVINE_LIGHT)
                .is_some()
        {
            sumpos = crate::board::position::FACEUP;
        }
        for e in self.filter_player_effect(playerid, code::FORCE_NORMAL_SUMMON_POSITION) {
            if let Some(target) = self.effects.get(e).and_then(|x| x.target_filter) {
                // The same four, with the *current* position rather than a
                // constant: the effect is being asked about the set it is
                // narrowing, not about the default.
                let args = [
                    i64::from(playerid),
                    i64::from(sumtype),
                    i64::from(sumpos),
                    i64::from(toplayer),
                ];
                if !target(self, e, card, &args) {
                    continue;
                }
            }
            sumpos &= self.effect_plain_value(e) as u8;
            if sumpos == 0 {
                return false;
            }
        }
        true
    }

    /// `field::is_player_can_mset` — the same question for setting.
    pub fn is_player_can_mset(
        &mut self,
        sumtype: u32,
        playerid: u8,
        card: Option<CardId>,
        _toplayer: u8,
    ) -> bool {
        let sumtype = sumtype | crate::card::summon_type::NORMAL;
        for e in self.filter_player_effect(playerid, code::CANNOT_MSET) {
            let Some(target) = self.effects.get(e).and_then(|x| x.target_filter) else {
                return false;
            };
            let args = [
                i64::from(playerid),
                i64::from(sumtype),
                i64::from(crate::board::position::FACEDOWN_DEFENSE),
                i64::from(_toplayer),
            ];
            if target(self, e, card, &args) {
                return false;
            }
        }
        true
    }

    /// `card::is_summonable` — can this summon procedure actually be used?
    ///
    /// Runs the procedure effect's **condition**, with the reason temporarily
    /// pointed at it and the life-point cost stack saved. Both are restored
    /// afterwards: asking whether a summon is possible must leave no trace,
    /// and a condition that pays a cost to find out would otherwise charge
    /// for the question.
    pub fn is_summonable(
        &mut self,
        card: CardId,
        procedure: EffectId,
        min_tribute: u8,
        zone: u32,
        releasable: u32,
    ) -> bool {
        let (old_effect, old_player) = (self.core.reason_effect, self.core.reason_player);
        self.core.reason_effect = Some(procedure);
        self.core.reason_player = self.cards[card].current.controller;
        self.save_lp_cost();

        let ev = crate::event::Event::new(0);
        let ctx = crate::effect::Ctx {
            reason_effect: procedure,
            player: self.cards[card].current.controller,
            event: &ev,
            card: Some(card),
            args: &[
                i64::from(min_tribute),
                i64::from(zone),
                i64::from(releasable),
            ],
        };
        let condition = self.effects.get(procedure).and_then(|e| e.condition);
        let result = condition.is_some_and(|f| f(self, &ctx));

        self.restore_lp_cost();
        self.core.reason_effect = old_effect;
        self.core.reason_player = old_player;
        result
    }
}

impl Field {
    /// The `EXTRA_*_COUNT` retry that `check_summon_procedure` and
    /// `check_set_procedure` share.
    ///
    /// A player who has used their normal summon may still summon if some
    /// effect grants an extra one. Each such effect returns a small tuple
    /// and the summon is retried under it.
    ///
    /// The defaults when the card does not supply a member are **not**
    /// zero, and differ between the two callers — `0x1f` where the summon is
    /// onto one's own side, `0x1f001f` where a destination player is in
    /// play. Reading "absent" as zero would offer no zone at all.
    ///
    /// `releasable`'s third member is signed for a reason: a **negative**
    /// value is *subtracted from* the default `0xff00ff` rather than
    /// replacing it, which is how an effect says "the usual, minus these".
    pub(crate) fn extra_count_params_pub(
        &mut self,
        card: CardId,
        effect: EffectId,
        default_zone: u32,
    ) -> (u8, u32, u32) {
        let vals = {
            let ev = crate::event::Event::new(0);
            let ctx = crate::effect::Ctx {
                reason_effect: effect,
                player: self.cards[card].current.controller,
                event: &ev,
                card: Some(card),
                args: &[],
            };
            match self.effects.get(effect) {
                Some(e) => e.get_value_list(self, &ctx),
                None => Vec::new(),
            }
        };
        let min = vals.first().copied().unwrap_or(0) as u8;
        let zone = vals.get(1).copied().map_or(default_zone, |z| z as u32);
        let releasable = match vals.get(2).copied() {
            // Negative: a reduction of the default, not a replacement.
            Some(r) if r < 0 => (0xff00ffu32).wrapping_add(r as u32),
            Some(r) => r as u32,
            None => 0xff00ff,
        };
        (min, zone, releasable)
    }

    /// Whose side a procedure summons onto.
    ///
    /// `EFFECT_FLAG_SPSUM_PARAM` marks a procedure that names a destination,
    /// and `o_range` being set means "the opponent's". Without the flag the
    /// summon is onto the summoning player's own side whatever the ranges
    /// say — the ranges mean something else entirely for an ordinary effect.
    fn procedure_toplayer(&self, effect: EffectId, playerid: u8) -> u8 {
        let spsum = self
            .effects
            .get(effect)
            .is_some_and(|e| e.is_flag(crate::effect::flag::SPSUM_PARAM));
        let opponent = self.effects.get(effect).is_some_and(|e| e.o_range != 0);
        if spsum && opponent {
            1 - playerid
        } else {
            playerid
        }
    }

    /// `card::check_summon_procedure` — can this monster be summoned by that
    /// procedure?
    pub fn check_summon_procedure(
        &mut self,
        card: CardId,
        procedure: EffectId,
        playerid: u8,
        ignore_count: bool,
        min_tribute: u8,
        zone: u32,
    ) -> bool {
        if !self.check_count_limit(procedure, playerid) {
            return false;
        }
        let toplayer = self.procedure_toplayer(procedure, playerid);
        let sumtype = self.effect_value_for_card_pub(procedure, card) as u32;
        if !self.is_player_can_summon(sumtype, playerid, Some(card), toplayer) {
            return false;
        }
        if self
            .check_unique_onfield(card, toplayer, u16::from(location::MZONE), None)
            .is_some()
        {
            return false;
        }
        // The script checks min_tribute itself, via Duel.CheckTribute.
        if self.needs_an_extra_summon(playerid, ignore_count) {
            for e in self.filter_effect(card, code::EXTRA_SUMMON_COUNT) {
                let (mut new_min, mut new_zone, releasable) =
                    self.extra_count_params_pub(card, e, 0x1f001f);
                if new_min < min_tribute {
                    new_min = min_tribute;
                }
                new_zone = self.flip_zone_for_opponent_pub(procedure, new_zone);
                new_zone &= zone;
                if self.is_summonable(card, procedure, new_min, new_zone, releasable) {
                    return true;
                }
            }
            false
        } else {
            self.is_summonable(card, procedure, min_tribute, zone, 0xff00ff)
        }
    }

    /// `card::check_set_procedure`.
    ///
    /// The same shape with two differences: it asks `is_player_can_mset`
    /// rather than `is_player_can_summon`, and it does **not** check
    /// `check_unique_onfield` — a set monster is face-down, and the
    /// uniqueness rule is about what is face-up on the field.
    pub fn check_set_procedure(
        &mut self,
        card: CardId,
        procedure: EffectId,
        playerid: u8,
        ignore_count: bool,
        min_tribute: u8,
        zone: u32,
    ) -> bool {
        if !self.check_count_limit(procedure, playerid) {
            return false;
        }
        let toplayer = self.procedure_toplayer(procedure, playerid);
        let sumtype = self.effect_value_for_card_pub(procedure, card) as u32;
        if !self.is_player_can_mset(sumtype, playerid, Some(card), toplayer) {
            return false;
        }
        if self.needs_an_extra_summon(playerid, ignore_count) {
            for e in self.filter_effect(card, code::EXTRA_SET_COUNT) {
                let (mut new_min, mut new_zone, releasable) =
                    self.extra_count_params_pub(card, e, 0x1f001f);
                new_zone = self.flip_zone_for_opponent_pub(procedure, new_zone);
                if new_min < min_tribute {
                    new_min = min_tribute;
                }
                new_zone &= zone;
                if self.is_summonable(card, procedure, new_min, new_zone, releasable) {
                    return true;
                }
            }
            false
        } else {
            self.is_summonable(card, procedure, min_tribute, zone, 0xff00ff)
        }
    }

    /// Has this player run out of normal summons, with no extra spent yet?
    ///
    /// The three conditions are separate: `ignore_count` skips the question
    /// entirely, `extra_summon` is a one-shot permission rather than a
    /// higher limit, and the count is compared against the limit.
    fn needs_an_extra_summon(&mut self, playerid: u8, ignore_count: bool) -> bool {
        if ignore_count || self.core.extra_summon[playerid as usize] {
            return false;
        }
        self.core.summon_count[playerid as usize] >= self.get_summon_count_limit(playerid) as u32
    }

    /// Swap the two halves of a zone mask when the procedure summons onto
    /// the opponent's side.
    ///
    /// `(z >> 16) | (z & 0xffff << 16)` in the reference — note the
    /// precedence: `<<` binds tighter than `&` in C, so it is
    /// `z & (0xffff << 16)`, i.e. keep the high half *in place* and copy the
    /// high half down. Written the way it parses rather than the way it
    /// reads, because the way it reads is a swap and it is not one.
    pub(crate) fn flip_zone_for_opponent_pub(&self, procedure: EffectId, zone: u32) -> u32 {
        let spsum = self
            .effects
            .get(procedure)
            .is_some_and(|e| e.is_flag(crate::effect::flag::SPSUM_PARAM));
        let opponent = self.effects.get(procedure).is_some_and(|e| e.o_range != 0);
        if spsum && opponent {
            (zone >> 16) | (zone & (0xffffu32 << 16))
        } else {
            zone
        }
    }

    /// `card::filter_summon_procedure` — which procedures could summon this
    /// monster, and is the ordinary summon among them?
    ///
    /// `EFFECT_LIMIT_SUMMON_PROC` **replaces** the ordinary summon rather
    /// than adding to it: if any is present, only its procedures are
    /// available, and if none of them is usable the monster cannot be
    /// summoned at all. That is why the answer is three-valued —
    /// "limited to these", "limited to nothing", and "the ordinary summon
    /// plus these" are genuinely different, and an empty list means
    /// different things in the first and third.
    pub fn filter_summon_procedure(
        &mut self,
        card: CardId,
        playerid: u8,
        ignore_count: bool,
        min_tribute: u8,
        zone: u32,
    ) -> Procedures {
        let limits = self.filter_effect(card, code::LIMIT_SUMMON_PROC);
        if !limits.is_empty() {
            let usable: Vec<EffectId> = limits
                .into_iter()
                .filter(|&e| {
                    self.check_summon_procedure(card, e, playerid, ignore_count, min_tribute, zone)
                })
                .collect();
            return if usable.is_empty() {
                Procedures::Forbidden
            } else {
                Procedures::Limited(usable)
            };
        }

        let procedures: Vec<EffectId> = self
            .filter_effect(card, code::SUMMON_PROC)
            .into_iter()
            .filter(|&e| {
                self.check_summon_procedure(card, e, playerid, ignore_count, min_tribute, zone)
            })
            .collect();

        let ordinary =
            self.ordinary_summon_available(card, playerid, ignore_count, min_tribute, zone);
        Procedures::Ordinary {
            procedures,
            ordinary,
        }
    }

    /// Whether the plain tribute summon is available, which is the bulk of
    /// `filter_summon_procedure`'s tail.
    fn ordinary_summon_available(
        &mut self,
        card: CardId,
        playerid: u8,
        ignore_count: bool,
        min_tribute: u8,
        zone: u32,
    ) -> bool {
        if !self.is_player_can_summon(
            crate::card::summon_type::NORMAL,
            playerid,
            Some(card),
            playerid,
        ) {
            return false;
        }
        if self
            .check_unique_onfield(card, playerid, u16::from(location::MZONE), None)
            .is_some()
        {
            return false;
        }
        let (mut min, mut max) = self.get_summon_tribute_count(card);
        // A tribute summon being forbidden caps the maximum at zero rather
        // than forbidding the summon: a monster needing no tributes is
        // unaffected.
        if !self.is_player_can_summon(
            crate::card::summon_type::ADVANCE,
            playerid,
            Some(card),
            playerid,
        ) {
            max = 0;
        }
        if min < i32::from(min_tribute) {
            min = i32::from(min_tribute);
        }
        if max < min {
            return false;
        }
        let controller = self.cards[card].current.controller;
        if self.needs_an_extra_summon(playerid, ignore_count) {
            for e in self.filter_effect(card, code::EXTRA_SUMMON_COUNT) {
                let (new_min, mut new_zone, releasable) =
                    self.extra_count_params_pub(card, e, 0x1f);
                let new_min = i32::from(new_min).max(min);
                new_zone &= zone;
                if self.check_tribute(
                    card,
                    new_min,
                    max,
                    None,
                    controller,
                    new_zone,
                    releasable,
                    u32::from(crate::board::position::FACEUP),
                ) {
                    return true;
                }
            }
            false
        } else {
            self.check_tribute(
                card,
                min,
                max,
                None,
                controller,
                zone,
                0xff00ff,
                u32::from(crate::board::position::FACEUP),
            )
        }
    }

    /// `card::filter_set_procedure` — the same, for setting.
    ///
    /// Two differences beyond the obvious code swap: the ordinary path asks
    /// `is_player_can_mset` and **omits the uniqueness check** entirely, and
    /// the tribute check is made for a face-down defence position rather
    /// than face-up.
    pub fn filter_set_procedure(
        &mut self,
        card: CardId,
        playerid: u8,
        ignore_count: bool,
        min_tribute: u8,
        zone: u32,
    ) -> Procedures {
        let limits = self.filter_effect(card, code::LIMIT_SET_PROC);
        if !limits.is_empty() {
            let usable: Vec<EffectId> = limits
                .into_iter()
                .filter(|&e| {
                    self.check_set_procedure(card, e, playerid, ignore_count, min_tribute, zone)
                })
                .collect();
            return if usable.is_empty() {
                Procedures::Forbidden
            } else {
                Procedures::Limited(usable)
            };
        }

        let procedures: Vec<EffectId> = self
            .filter_effect(card, code::SET_PROC)
            .into_iter()
            .filter(|&e| {
                self.check_set_procedure(card, e, playerid, ignore_count, min_tribute, zone)
            })
            .collect();

        let ordinary = self.ordinary_set_available(card, playerid, ignore_count, min_tribute, zone);
        Procedures::Ordinary {
            procedures,
            ordinary,
        }
    }

    fn ordinary_set_available(
        &mut self,
        card: CardId,
        playerid: u8,
        ignore_count: bool,
        min_tribute: u8,
        zone: u32,
    ) -> bool {
        if !self.is_player_can_mset(
            crate::card::summon_type::NORMAL,
            playerid,
            Some(card),
            playerid,
        ) {
            return false;
        }
        let (mut min, mut max) = self.get_set_tribute_count(card);
        if !self.is_player_can_mset(
            crate::card::summon_type::ADVANCE,
            playerid,
            Some(card),
            playerid,
        ) {
            max = 0;
        }
        if min < i32::from(min_tribute) {
            min = i32::from(min_tribute);
        }
        if max < min {
            return false;
        }
        let controller = self.cards[card].current.controller;
        let pos = u32::from(crate::board::position::FACEDOWN_DEFENSE);
        if self.needs_an_extra_summon(playerid, ignore_count) {
            for e in self.filter_effect(card, code::EXTRA_SET_COUNT) {
                let (new_min, mut new_zone, releasable) =
                    self.extra_count_params_pub(card, e, 0x1f);
                let new_min = i32::from(new_min).max(min);
                new_zone &= zone;
                if self.check_tribute(
                    card, new_min, max, None, controller, new_zone, releasable, pos,
                ) {
                    return true;
                }
            }
            false
        } else {
            self.check_tribute(card, min, max, None, controller, zone, 0xff00ff, pos)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::position;
    use crate::card::{card_type, status, Card, CardData};
    use crate::effect::{effect_type, flag, Effect};

    fn monster(f: &mut Field, player: u8, seat: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 18036057,
                type_: card_type::MONSTER,
                ..Default::default()
            },
            player,
        );
        c.current.controller = player;
        c.current.position = position::FACEUP_ATTACK;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(player, id, location::MZONE, seat, false);
        id
    }

    fn in_hand(f: &mut Field, player: u8) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 72989439,
                type_: card_type::MONSTER,
                ..Default::default()
            },
            player,
        );
        c.current.controller = player;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(player, id, location::HAND, 0, false);
        id
    }

    /// An effect printed on `card` with a constant value.
    fn printed(f: &mut Field, card: CardId, code_: u32, value: i64) -> EffectId {
        let mut e = Effect::new(effect_type::SINGLE, code_);
        e.owner = Some(card);
        e.handler = Some(card);
        e.value = value;
        let id = f.new_effect(e);
        f.cards[card].single_effect.insert(code_, id);
        f.cards[card].indexer.insert(id);
        id
    }

    mod lp_cost {
        use super::*;

        #[test]
        fn the_stack_saves_and_restores() {
            let mut f = Field::new(8000);
            f.core.lp_cost[0].amount = 500;
            f.save_lp_cost();
            f.core.lp_cost[0].amount = 1200;
            f.restore_lp_cost();
            assert_eq!(f.core.lp_cost[0].amount, 500);
        }

        /// The counter keeps counting past the stack's eight slots while the
        /// stack stops recording, so deep nesting restores a stale value
        /// rather than a wrong one. The reference accepts this.
        #[test]
        fn nesting_past_eight_restores_a_stale_value_not_a_wrong_one() {
            let mut f = Field::new(8000);
            for i in 0..12 {
                f.core.lp_cost[0].amount = i;
                f.save_lp_cost();
            }
            assert_eq!(f.core.lp_cost[0].count, 12, "the counter kept counting");
            // Unwind back into the recorded region.
            for _ in 0..5 {
                f.restore_lp_cost();
            }
            assert_eq!(f.core.lp_cost[0].count, 7);
            assert_eq!(f.core.lp_cost[0].amount, 7, "slot 7, as recorded");
        }
    }

    mod get_forced_zones {
        use super::*;

        /// Only the Monster Zone can be forced.
        #[test]
        fn nothing_but_the_monster_zone_is_forced() {
            let mut f = Field::new(8000);
            assert_eq!(
                f.get_forced_zones(None, 0, location::SZONE, 0, Field::LOCATION_REASON_TOFIELD),
                0xff
            );
        }

        /// No forcing effect means every seat is permitted — and "every
        /// seat" is `0x7f`, not `0xff`.
        ///
        /// The accumulator starts at `0xff7fff7f`, whose low byte already
        /// has **bit 7 clear**: that is the seat which does not exist, the
        /// same gap `refresh_location_info_instant` masks out. So the
        /// not-a-Monster-Zone early return (`0xff`) and the
        /// nothing-is-forcing answer (`0x7f`) deliberately differ. Only the
        /// low five bits reach `get_tofield_count`, where the two agree.
        #[test]
        fn no_effect_permits_every_real_seat() {
            let mut f = Field::new(8000);
            assert_eq!(
                f.get_forced_zones(None, 0, location::MZONE, 0, Field::LOCATION_REASON_TOFIELD),
                0x7f,
                "bit 7 is not a seat"
            );
            assert_eq!(
                f.get_forced_zones(None, 0, location::SZONE, 0, Field::LOCATION_REASON_TOFIELD),
                0xff,
                "but the early return is a plain 0xff"
            );
        }

        /// The mask is a **permission**, and two effects AND together — two
        /// incompatible requirements leave no seat at all. This is the
        /// opposite of how the disabled masks combine.
        #[test]
        fn two_requirements_intersect_rather_than_union() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f, 0);
            printed(&mut f, c, code::MUST_USE_MZONE, 0x0003);
            printed(&mut f, c, code::MUST_USE_MZONE, 0x0006);
            let got = f.get_forced_zones(
                Some(c),
                0,
                location::MZONE,
                0,
                Field::LOCATION_REASON_TOFIELD,
            );
            assert_eq!(got, 0x2, "only the seat both allow");

            let mut g = Field::new(8000);
            let d = in_hand(&mut g, 0);
            printed(&mut g, d, code::MUST_USE_MZONE, 0x0001);
            printed(&mut g, d, code::MUST_USE_MZONE, 0x0002);
            let got = g.get_forced_zones(
                Some(d),
                0,
                location::MZONE,
                0,
                Field::LOCATION_REASON_TOFIELD,
            );
            assert_eq!(got, 0, "incompatible requirements leave nothing");
        }

        /// Which half of the value is read depends on whose view is asked
        /// for — and `PLAYER_NONE` reads as the owning player's view rather
        /// than being rejected.
        #[test]
        fn the_half_read_depends_on_who_is_asking() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f, 0);
            // low half 0x3, high half 0x18
            printed(&mut f, c, code::MUST_USE_MZONE, 0x0018_0003);

            let own = f.get_forced_zones(
                Some(c),
                0,
                location::MZONE,
                0,
                Field::LOCATION_REASON_TOFIELD,
            );
            assert_eq!(own, 0x3, "the asking player's own seats");

            let theirs = f.get_forced_zones(
                Some(c),
                0,
                location::MZONE,
                1,
                Field::LOCATION_REASON_TOFIELD,
            );
            assert_eq!(theirs, 0x18, "the other half");

            let nobody = f.get_forced_zones(
                Some(c),
                0,
                location::MZONE,
                PLAYER_NONE,
                Field::LOCATION_REASON_TOFIELD,
            );
            assert_eq!(nobody, 0x3, "PLAYER_NONE reads as the owning player's view");
        }

        /// A spent count-limited effect forces nothing.
        #[test]
        fn a_spent_effect_forces_nothing() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f, 0);
            let e = printed(&mut f, c, code::MUST_USE_MZONE, 0x1);
            if let Some(x) = f.effects.get_mut(e) {
                x.flag[0] |= flag::COUNT_LIMIT;
                x.count_limit = 0;
            }
            assert_eq!(
                f.get_forced_zones(
                    Some(c),
                    0,
                    location::MZONE,
                    0,
                    Field::LOCATION_REASON_TOFIELD
                ),
                0x7f,
                "as though the effect were not there"
            );
        }
    }

    mod get_summon_release_list {
        use super::*;

        /// Each own-side monster counts as one by default.
        #[test]
        fn own_monsters_count_one_each() {
            let mut f = Field::new(8000);
            let summoning = in_hand(&mut f, 0);
            monster(&mut f, 0, 0);
            monster(&mut f, 0, 1);
            let (total, lists) = f.get_summon_release_list(summoning, None, false, 0xff00ff, 0x1f);
            assert_eq!(total, 2);
            assert_eq!(lists.release.len(), 2);
        }

        /// `DOUBLE_TRIBUTE` and `TRIPLE_TRIBUTE` are written onto the card,
        /// not merely counted — the caller that spends the tributes needs
        /// the per-card value.
        #[test]
        fn a_double_tribute_is_recorded_on_the_card() {
            let mut f = Field::new(8000);
            let summoning = in_hand(&mut f, 0);
            let big = monster(&mut f, 0, 0);
            printed(&mut f, big, code::DOUBLE_TRIBUTE, 1);
            let (total, _) = f.get_summon_release_list(summoning, None, false, 0xff00ff, 0x1f);
            assert_eq!(total, 2, "worth two");
            assert_eq!(f.cards[big].release_param, 2, "and it says so on the card");
        }

        /// The opponent's seats are addressed 16 bits up in the `releasable`
        /// mask — the same two-row packing as everywhere else.
        #[test]
        fn the_opponents_seats_live_sixteen_bits_up() {
            let mut f = Field::new(8000);
            let summoning = in_hand(&mut f, 0);
            let theirs = monster(&mut f, 1, 0);
            printed(&mut f, theirs, code::EXTRA_RELEASE, 1);

            // low half only: their seat is not addressed
            let (total, _) = f.get_summon_release_list(summoning, None, false, 0x0000ff, 0x1f);
            assert_eq!(total, 0, "their seat is not in the low half");

            let (total, lists) = f.get_summon_release_list(summoning, None, false, 0xff00ff, 0x1f);
            assert_eq!(total, 1);
            assert_eq!(lists.extra.len(), 1, "extra-release, not ordinary");
        }

        /// `EXTRA_RELEASE_SUM` allows **one of** a set, so only the largest
        /// contributes — not their sum.
        #[test]
        fn only_the_largest_one_of_counts() {
            let mut f = Field::new(8000);
            let summoning = in_hand(&mut f, 0);
            let a = monster(&mut f, 1, 0);
            let b = monster(&mut f, 1, 1);
            printed(&mut f, a, code::EXTRA_RELEASE_SUM, 1);
            printed(&mut f, b, code::EXTRA_RELEASE_SUM, 1);
            printed(&mut f, b, code::DOUBLE_TRIBUTE, 1);

            let (total, lists) = f.get_summon_release_list(summoning, None, false, 0xff00ff, 0x1f);
            assert_eq!(lists.extra_one_of.len(), 2, "both are candidates");
            assert_eq!(total, 2, "but only the larger counts, not 1 + 2");
        }
    }

    mod tribute_counts {
        use super::*;

        fn leveled(f: &mut Field, level: u32) -> CardId {
            let mut c = Card::with_data(
                CardData {
                    code: 18036057,
                    type_: card_type::MONSTER,
                    level,
                    ..Default::default()
                },
                0,
            );
            c.current.controller = 0;
            c.current.position = position::FACEUP_ATTACK;
            c.set_status(status::EFFECT_ENABLED, true);
            let id = f.new_card(c);
            f.add_card(0, id, location::MZONE, 0, false);
            id
        }

        #[test]
        fn the_base_is_by_level() {
            for (level, want) in [
                (4u32, (0, 0)),
                (5, (1, 1)),
                (6, (1, 1)),
                (7, (2, 2)),
                (12, (2, 2)),
            ] {
                let mut f = Field::new(8000);
                let c = leveled(&mut f, level);
                assert_eq!(f.get_summon_tribute_count(c), want, "level {level}");
            }
        }

        /// Uncounted reductions are **not cumulative** — the largest wins.
        /// Two independent "one fewer tribute" effects remove one, not two.
        #[test]
        fn uncounted_reductions_do_not_stack() {
            let mut f = Field::new(8000);
            let c = leveled(&mut f, 8); // needs two
            printed(&mut f, c, code::DECREASE_TRIBUTE, 0x0001_0001);
            printed(&mut f, c, code::DECREASE_TRIBUTE, 0x0001_0001);
            assert_eq!(
                f.get_summon_tribute_count(c),
                (1, 1),
                "the largest reduction wins; they do not add"
            );
        }

        /// Count-limited reductions **do** stack, and only while unspent.
        #[test]
        fn counted_reductions_stack_while_unspent() {
            let mut f = Field::new(8000);
            let c = leveled(&mut f, 8);
            for _ in 0..2 {
                let e = printed(&mut f, c, code::DECREASE_TRIBUTE, 0x0001_0001);
                if let Some(x) = f.effects.get_mut(e) {
                    x.flag[0] |= flag::COUNT_LIMIT;
                    x.count_limit = 1;
                }
            }
            assert_eq!(f.get_summon_tribute_count(c), (0, 0), "both applied");

            let mut g = Field::new(8000);
            let d = leveled(&mut g, 8);
            for limit in [1u8, 0] {
                let e = printed(&mut g, d, code::DECREASE_TRIBUTE, 0x0001_0001);
                if let Some(x) = g.effects.get_mut(e) {
                    x.flag[0] |= flag::COUNT_LIMIT;
                    x.count_limit = limit;
                }
            }
            assert_eq!(
                g.get_summon_tribute_count(d),
                (1, 1),
                "the spent one does not"
            );
        }

        /// The set form takes only the **last** effect — not the largest and
        /// not a combination.
        #[test]
        fn the_set_form_takes_only_the_last_reduction() {
            let mut f = Field::new(8000);
            let c = leveled(&mut f, 8);
            printed(&mut f, c, code::DECREASE_TRIBUTE_SET, 0x0002_0002);
            printed(&mut f, c, code::DECREASE_TRIBUTE_SET, 0x0001_0001);
            assert_eq!(
                f.get_set_tribute_count(c),
                (1, 1),
                "the last registered, even though the first reduces more"
            );
        }
    }

    mod summon_permission {
        use super::*;

        /// A player-wide effect.
        ///
        /// `s_range` must be non-zero: `is_target_player` reads the two
        /// ranges as "applies to my side" and "applies to theirs", and an
        /// effect with neither targets nobody — so `filter_player_effect`
        /// silently returns nothing and a test of it passes vacuously.
        fn player_effect(f: &mut Field, player: u8, code_: u32, value: i64) -> EffectId {
            let mut e = Effect::new(effect_type::FIELD, code_);
            e.effect_owner = player;
            e.flag[0] |= flag::PLAYER_TARGET | flag::FIELD_ONLY;
            e.s_range = 1;
            e.value = value;
            let id = f.new_effect(e);
            f.field_effects.aura.insert(code_, id);
            f.field_effects.indexer.insert(id);
            id
        }

        #[test]
        fn nothing_forbidding_permits_the_summon() {
            let mut f = Field::new(8000);
            assert!(f.is_player_can_summon(crate::card::summon_type::NORMAL, 0, None, 0));
        }

        /// A prohibition with no target forbids unconditionally.
        #[test]
        fn a_prohibition_without_a_target_forbids() {
            let mut f = Field::new(8000);
            player_effect(&mut f, 0, code::CANNOT_SUMMON, 0);
            assert!(!f.is_player_can_summon(crate::card::summon_type::NORMAL, 0, None, 0));
        }

        /// `ADVANCE` is `NORMAL` plus a bit, so a prohibition on normal
        /// summoning catches a tribute summon too.
        #[test]
        fn advance_is_a_normal_summon() {
            assert_eq!(
                crate::card::summon_type::ADVANCE & crate::card::summon_type::NORMAL,
                crate::card::summon_type::NORMAL,
                "ADVANCE contains NORMAL"
            );
            let mut f = Field::new(8000);
            player_effect(&mut f, 0, code::CANNOT_SUMMON, 0);
            assert!(!f.is_player_can_summon(crate::card::summon_type::ADVANCE, 0, None, 0));
        }

        /// The arguments reach the prohibition's condition.
        ///
        /// This is what the port was silently dropping: a prohibition that
        /// asks about the **summon type** — "you may not Tribute Summon" —
        /// cannot be answered without them, and a `target_filter` that never
        /// received them made every such prohibition unconditional.
        #[test]
        fn the_summon_type_reaches_the_prohibition() {
            fn only_advance(_: &Field, _: EffectId, _: Option<CardId>, args: &[i64]) -> bool {
                // args = [playerid, sumtype, position, toplayer]
                args.get(1).copied().unwrap_or(0) as u32 == crate::card::summon_type::ADVANCE
            }
            let mut f = Field::new(8000);
            let e = player_effect(&mut f, 0, code::CANNOT_SUMMON, 0);
            f.effects.get_mut(e).unwrap().target_filter = Some(only_advance);

            assert!(
                f.is_player_can_summon(crate::card::summon_type::NORMAL, 0, None, 0),
                "an ordinary summon is not what this forbids"
            );
            assert!(
                !f.is_player_can_summon(crate::card::summon_type::ADVANCE, 0, None, 0),
                "but a tribute summon is"
            );
        }

        /// `SUMMON_TYPE_NORMAL` is ORed in before the condition sees it.
        ///
        /// Every *named* summon type that reaches here already carries the
        /// bit — `ADVANCE` is `0x11000000` and `GEMINI` is `0x12000000` —
        /// so the OR only does anything for the one caller that reads a
        /// **card's own** value: a `SUMMON_PROC` effect naming its summon
        /// type. This test supplies such a foreign type directly, because
        /// asserting with `ADVANCE` passes whether or not the OR is there.
        #[test]
        fn the_normal_bit_is_ored_in_before_the_condition_sees_it() {
            fn any_normal(_: &Field, _: EffectId, _: Option<CardId>, args: &[i64]) -> bool {
                args.get(1).copied().unwrap_or(0) as u32 & crate::card::summon_type::NORMAL != 0
            }
            let mut f = Field::new(8000);
            let e = player_effect(&mut f, 0, code::CANNOT_SUMMON, 0);
            f.effects.get_mut(e).unwrap().target_filter = Some(any_normal);

            // A card-supplied type with the NORMAL bit absent.
            const FOREIGN: u32 = 0x0200_0000;
            assert_eq!(FOREIGN & crate::card::summon_type::NORMAL, 0);
            assert!(
                !f.is_player_can_summon(FOREIGN, 0, None, 0),
                "the OR puts the NORMAL bit back before the condition is asked"
            );
        }

        /// The position rule is subtractive: forcing narrows the allowed
        /// set, and narrowing to nothing forbids the summon.
        #[test]
        fn forcing_a_position_narrows_and_can_forbid() {
            let mut f = Field::new(8000);
            // face-up attack is the default; force face-up defence only
            player_effect(
                &mut f,
                0,
                code::FORCE_NORMAL_SUMMON_POSITION,
                i64::from(position::FACEUP_DEFENSE),
            );
            assert!(
                !f.is_player_can_summon(crate::card::summon_type::NORMAL, 0, None, 0),
                "nothing left once narrowed"
            );

            // with the duel option widening the default, it survives
            let mut g = Field::new(8000);
            g.flags |= crate::duel::flags::NORMAL_SUMMON_FACEUP_DEF;
            player_effect(
                &mut g,
                0,
                code::FORCE_NORMAL_SUMMON_POSITION,
                i64::from(position::FACEUP_DEFENSE),
            );
            assert!(g.is_player_can_summon(crate::card::summon_type::NORMAL, 0, None, 0));
        }
    }

    mod procedures {
        use super::*;

        fn summonable(f: &mut Field, level: u32) -> CardId {
            let mut c = Card::with_data(
                CardData {
                    code: 18036057,
                    type_: card_type::MONSTER,
                    level,
                    ..Default::default()
                },
                0,
            );
            c.current.controller = 0;
            c.set_status(status::EFFECT_ENABLED, true);
            let id = f.new_card(c);
            f.add_card(0, id, location::HAND, 0, false);
            id
        }

        /// A procedure whose condition always agrees.
        fn procedure(f: &mut Field, card: CardId, code_: u32, ok: bool) -> EffectId {
            fn yes(_: &mut Field, _: &crate::effect::Ctx) -> bool {
                true
            }
            fn no(_: &mut Field, _: &crate::effect::Ctx) -> bool {
                false
            }
            let mut e = Effect::new(effect_type::SINGLE, code_);
            e.owner = Some(card);
            e.handler = Some(card);
            e.condition = Some(if ok { yes } else { no });
            let id = f.new_effect(e);
            f.cards[card].single_effect.insert(code_, id);
            f.cards[card].indexer.insert(id);
            id
        }

        /// With no procedure effects, a low-level monster can be summoned
        /// ordinarily.
        #[test]
        fn an_ordinary_summon_is_available_by_default() {
            let mut f = Field::new(8000);
            let c = summonable(&mut f, 4);
            match f.filter_summon_procedure(c, 0, false, 0, 0x1f) {
                Procedures::Ordinary {
                    procedures,
                    ordinary,
                } => {
                    assert!(ordinary, "no tributes needed, room on the field");
                    assert!(procedures.is_empty());
                }
                other => panic!("expected Ordinary, got {other:?}"),
            }
        }

        /// A `SUMMON_PROC` **adds** to the ordinary summon.
        #[test]
        fn a_summon_proc_adds_to_the_ordinary_summon() {
            let mut f = Field::new(8000);
            let c = summonable(&mut f, 4);
            procedure(&mut f, c, code::SUMMON_PROC, true);
            match f.filter_summon_procedure(c, 0, false, 0, 0x1f) {
                Procedures::Ordinary {
                    procedures,
                    ordinary,
                } => {
                    assert_eq!(procedures.len(), 1);
                    assert!(ordinary, "and the ordinary summon is still there");
                }
                other => panic!("expected Ordinary, got {other:?}"),
            }
        }

        /// A `LIMIT_SUMMON_PROC` **replaces** it.
        #[test]
        fn a_limit_proc_replaces_the_ordinary_summon() {
            let mut f = Field::new(8000);
            let c = summonable(&mut f, 4);
            procedure(&mut f, c, code::LIMIT_SUMMON_PROC, true);
            match f.filter_summon_procedure(c, 0, false, 0, 0x1f) {
                Procedures::Limited(ps) => assert_eq!(ps.len(), 1),
                other => panic!("expected Limited, got {other:?}"),
            }
        }

        /// And a limit whose procedures are all unusable forbids the summon
        /// outright — which is **not** the same as an empty ordinary list.
        #[test]
        fn an_unusable_limit_forbids_rather_than_falling_back() {
            let mut f = Field::new(8000);
            let c = summonable(&mut f, 4);
            procedure(&mut f, c, code::LIMIT_SUMMON_PROC, false);
            assert_eq!(
                f.filter_summon_procedure(c, 0, false, 0, 0x1f),
                Procedures::Forbidden,
                "no fallback to the ordinary summon"
            );

            // Contrast: an unusable SUMMON_PROC leaves the ordinary summon.
            let mut g = Field::new(8000);
            let d = summonable(&mut g, 4);
            procedure(&mut g, d, code::SUMMON_PROC, false);
            match g.filter_summon_procedure(d, 0, false, 0, 0x1f) {
                Procedures::Ordinary {
                    procedures,
                    ordinary,
                } => {
                    assert!(procedures.is_empty());
                    assert!(ordinary, "the ordinary summon survives");
                }
                other => panic!("expected Ordinary, got {other:?}"),
            }
        }

        /// A level-7 monster needs two tributes, and with an empty field
        /// there are none — so no ordinary summon.
        #[test]
        fn a_high_level_monster_needs_tributes_it_does_not_have() {
            let mut f = Field::new(8000);
            let c = summonable(&mut f, 7);
            match f.filter_summon_procedure(c, 0, false, 0, 0x1f) {
                Procedures::Ordinary { ordinary, .. } => {
                    assert!(!ordinary, "two tributes needed, none available")
                }
                other => panic!("expected Ordinary, got {other:?}"),
            }
        }

        /// The set form omits the uniqueness check entirely — a set monster
        /// is face-down, and uniqueness is about what is face-up.
        ///
        /// The blocker has to be a **different** card: `check_unique_onfield`
        /// skips the card being asked about, so registering the card itself
        /// proves nothing and the test would pass with the check present.
        #[test]
        fn the_set_form_ignores_uniqueness() {
            fn same_code(_: &Field, _: EffectId, _: Option<CardId>, _: &[i64]) -> bool {
                true
            }
            let mut f = Field::new(8000);
            let c = summonable(&mut f, 4);

            // A different, already-present card claiming uniqueness.
            let blocker = monster(&mut f, 0, 0);
            // Everything `check_unique_onfield` requires of the blocker:
            // face-up, enabled, not disabled or forbidden, a non-zero
            // `unique_fieldid`, a matching code, and a location the rule
            // covers.
            f.cards[blocker].unique_code = 18036057;
            f.cards[blocker].unique_pos = [1, 1];
            f.cards[blocker].unique_fieldid = 1;
            f.cards[blocker].unique_location = u16::from(location::MZONE);
            let mut e = Effect::new(effect_type::SINGLE, code::UNIQUE_CHECK);
            e.owner = Some(blocker);
            e.handler = Some(blocker);
            e.target_filter = Some(same_code);
            let e = f.new_effect(e);
            f.cards[blocker].unique_effect = Some(e);
            f.core.unique_cards[0].push(blocker);

            // The summon form is blocked by it...
            match f.filter_summon_procedure(c, 0, false, 0, 0x1f) {
                Procedures::Ordinary { ordinary, .. } => {
                    assert!(!ordinary, "uniqueness blocks the summon")
                }
                other => panic!("expected Ordinary, got {other:?}"),
            }
            // ...and the set form is not.
            match f.filter_set_procedure(c, 0, false, 0, 0x1f) {
                Procedures::Ordinary { ordinary, .. } => {
                    assert!(ordinary, "setting is not blocked by uniqueness")
                }
                other => panic!("expected Ordinary, got {other:?}"),
            }
        }

        /// Forbidding the **tribute** summon caps the maximum at zero rather
        /// than forbidding the summon: a monster needing no tributes is
        /// unaffected, and one that needs them can no longer be summoned.
        #[test]
        fn forbidding_the_tribute_summon_only_caps_the_maximum() {
            fn advance_only(_: &Field, _: EffectId, _: Option<CardId>, args: &[i64]) -> bool {
                args.get(1).copied().unwrap_or(0) as u32 == crate::card::summon_type::ADVANCE
            }
            // A level-4 monster needs none, so it is unaffected.
            let mut f = Field::new(8000);
            let low = summonable(&mut f, 4);
            let mut e = Effect::new(effect_type::FIELD, code::CANNOT_SUMMON);
            e.effect_owner = 0;
            e.flag[0] |= flag::PLAYER_TARGET | flag::FIELD_ONLY;
            e.s_range = 1;
            e.target_filter = Some(advance_only);
            let e = f.new_effect(e);
            f.field_effects.aura.insert(code::CANNOT_SUMMON, e);
            f.field_effects.indexer.insert(e);

            match f.filter_summon_procedure(low, 0, false, 0, 0x1f) {
                Procedures::Ordinary { ordinary, .. } => {
                    assert!(ordinary, "no tributes needed, so nothing is capped away")
                }
                other => panic!("expected Ordinary, got {other:?}"),
            }
        }
    }

    mod value_list {
        use super::*;

        /// A constant value yields a **one-element** list, which is how a
        /// caller tells "the card did not say" from "the card said zero".
        #[test]
        fn a_constant_yields_one_element() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f, 0);
            let e = printed(&mut f, c, code::EXTRA_SUMMON_COUNT, 0);
            let ev = crate::event::Event::new(0);
            let ctx = crate::effect::Ctx {
                reason_effect: e,
                player: 0,
                event: &ev,
                card: Some(c),
                args: &[],
            };
            let vals = f.effects.get(e).unwrap().get_value_list(&f, &ctx);
            assert_eq!(vals, vec![0]);
            assert_eq!(vals.len(), 1, "one element, not none");
        }

        /// A missing second member falls back to the caller's default, not
        /// to zero — reading it as zero would offer no zone at all.
        #[test]
        fn a_missing_member_takes_the_callers_default() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f, 0);
            let e = printed(&mut f, c, code::EXTRA_SUMMON_COUNT, 0);
            let (min, zone, releasable) = f.extra_count_params_pub(c, e, 0x1f);
            assert_eq!(min, 0);
            assert_eq!(zone, 0x1f, "the default, not 0");
            assert_eq!(releasable, 0xff00ff, "likewise");
        }

        /// A negative third member is **subtracted from** the default rather
        /// than replacing it — "the usual, minus these".
        #[test]
        fn a_negative_releasable_reduces_the_default() {
            fn three(_: &Field, _: &crate::effect::Ctx) -> Vec<i64> {
                vec![0, 0x1f, -1]
            }
            let mut f = Field::new(8000);
            let c = in_hand(&mut f, 0);
            let e = printed(&mut f, c, code::EXTRA_SUMMON_COUNT, 0);
            if let Some(x) = f.effects.get_mut(e) {
                x.flag[0] |= flag::FUNC_VALUE;
                x.value_list_fn = Some(three);
            }
            let (_, _, releasable) = f.extra_count_params_pub(c, e, 0x1f);
            assert_eq!(releasable, 0xff00ff - 1, "reduced, not replaced");
        }
    }
}

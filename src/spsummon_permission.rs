//! May this be Special Summoned? — the permissions, the counters, and the
//! once-per-turn ledger.
//!
//! The layer under `SpSummonStep` and `SpSummonRule`, in the same relation
//! `tribute.rs`'s [`Field::is_player_can_summon`] stands in to `SummonRule`.
//! Nothing here moves a card; every function answers a question, and the
//! machines that act on the answers come next.
//!
//! ## Three different limits, easy to conflate
//!
//! - **`EFFECT_CANNOT_SPECIAL_SUMMON`** — a prohibition. Read twice, once
//!   against the card and once against the player, with opposite polarity on
//!   the missing-target case (see [`Field::is_player_can_spsummon`]).
//! - **`EFFECT_SPSUMMON_COUNT_LIMIT`** — a *ceiling* on how many Special
//!   Summons a player may make, counted per effect that imposes one. Each
//!   such effect carries its own tally on its handler card.
//! - **`spsummon_code`** — the once-per-turn name limit, a ledger keyed by a
//!   shared name rather than by card. Several cards can share one code, which
//!   is how "you can only Special Summon <name> once per turn" covers a
//!   family rather than a printing.
//!
//! ## The `_rst` halves, and why they are here at all
//!
//! Both counters carry a second tally recording how much a chain could still
//! take back. They are only ever written under
//! `DUEL_CANNOT_SUMMON_OATH_OLD`, which this project does not run with, so
//! every `_rst` value is zero here. They are ported because the branch that
//! reads them is reachable the moment the option changes, and a port that
//! silently drops a duel option's behaviour is worse than one that carries
//! dead-but-correct code.

use crate::board::{location, position};
use crate::card::{card_type, status, summon_type};
use crate::event::{code, CardId, EffectId};
use crate::field::Field;

impl Field {
    /// `field::check_spsummon_once` — has this name been Special Summoned
    /// already this turn?
    ///
    /// `spsummon_code == 0` is the common case and shortcuts: most cards are
    /// not limited this way at all.
    pub fn check_spsummon_once(&self, card: CardId, playerid: u8) -> bool {
        let spcode = self.cards[card].spsummon_code;
        if spcode == 0 {
            return true;
        }
        self.core.spsummon_once_map[playerid as usize]
            .get(&spcode)
            .is_none_or(|&n| n == 0)
    }

    /// `field::set_spsummon_counter` — record a Special Summon against every
    /// effect that limits them.
    ///
    /// The `add` and `chain` arguments only mean anything under
    /// `DUEL_CANNOT_SUMMON_OATH_OLD`: without it a Special Summon is counted
    /// once and never given back, and both arguments are ignored. With it,
    /// the counts made *during a chain* are recorded twice — once in the
    /// tally and once in the give-back half — so that negating the chain can
    /// undo exactly those.
    ///
    /// The per-effect tallies live on the **handler card**, not on the
    /// effect, and are only touched for effects that are currently available
    /// and whose range covers the summoning player.
    pub fn set_spsummon_counter(&mut self, playerid: u8, add: bool, chain: bool) {
        let old_oath = self.is_flag(crate::duel::flags::CANNOT_SUMMON_OATH_OLD);
        let p = playerid as usize;
        if old_oath {
            if add {
                self.core.spsummon_state_count[p] += 1;
                if chain {
                    self.core.spsummon_state_count_rst[p] += 1;
                }
            } else if chain {
                self.core.spsummon_state_count[p] -= self.core.spsummon_state_count_rst[p];
                self.core.spsummon_state_count_rst[p] = 0;
            } else {
                self.core.spsummon_state_count[p] -= 1;
            }
        } else {
            self.core.spsummon_state_count[p] += 1;
        }

        for effect in self.field_effects.spsummon_count_eff.clone() {
            let Some(handler) = self
                .effects
                .get(effect)
                .and_then(|e| e.get_handler(&self.cards))
            else {
                continue;
            };
            if old_oath && !add {
                let back = self.cards[handler].spsummon_counter_rst[p];
                self.cards[handler].spsummon_counter[p] -= back;
                self.cards[handler].spsummon_counter_rst[p] = 0;
                continue;
            }
            if !self.is_available(effect) {
                continue;
            }
            if !self.counter_covers(effect, handler, playerid) {
                continue;
            }
            self.cards[handler].spsummon_counter[p] += 1;
            if old_oath && chain {
                self.cards[handler].spsummon_counter_rst[p] += 1;
            }
        }
    }

    /// Whether a count-limit effect's range covers this player: `s_range`
    /// for its controller's own summons, `o_range` for the opponent's.
    fn counter_covers(&self, effect: EffectId, handler: CardId, playerid: u8) -> bool {
        let Some(e) = self.effects.get(effect) else {
            return false;
        };
        let controller = self.cards[handler].current.controller;
        if playerid == controller {
            e.s_range != 0
        } else {
            e.o_range != 0
        }
    }

    /// `field::check_spsummon_counter` — would `ct` more Special Summons
    /// exceed anybody's ceiling?
    ///
    /// The comparison is `>` rather than `>=`: the value is the number
    /// *allowed*, so reaching it exactly is still legal.
    pub fn check_spsummon_counter(&self, playerid: u8, ct: u16) -> bool {
        for &effect in &self.field_effects.spsummon_count_eff {
            let Some(e) = self.effects.get(effect) else {
                continue;
            };
            let Some(handler) = e.get_handler(&self.cards) else {
                continue;
            };
            if !self.is_available(effect) {
                continue;
            }
            let allowed = e.value as u16;
            if self.cards[handler].spsummon_counter[playerid as usize] + ct > allowed {
                return false;
            }
        }
        true
    }

    /// `field::is_player_can_spsummon_count` — has the player room for
    /// `count` more?
    ///
    /// `EFFECT_LEFT_SPSUMMON_COUNT` answers with a number rather than a
    /// prohibition, and **every** such effect must allow the count — the
    /// smallest wins, expressed as "any one refusing is a refusal".
    pub fn is_player_can_spsummon_count(&mut self, playerid: u8, count: u32) -> bool {
        for e in self.filter_player_effect(playerid, code::LEFT_SPSUMMON_COUNT) {
            let left = self.effect_plain_value(e);
            if left < i64::from(count) {
                return false;
            }
        }
        self.check_spsummon_counter(playerid, count as u16)
    }

    /// `field::is_player_can_spsummon(playerid)` — the bare question, with no
    /// card in hand to ask it about.
    ///
    /// **A prohibition with no target refuses outright here**, which is the
    /// opposite of how the same effect reads in the card-aware form below.
    /// The asymmetry is the reference's and is deliberate: without a card
    /// there is nothing for a conditional prohibition to test, so only the
    /// unconditional ones can answer.
    pub fn is_player_can_spsummon_player(&mut self, playerid: u8) -> bool {
        for e in self.filter_player_effect(playerid, code::CANNOT_SPECIAL_SUMMON) {
            if self
                .effects
                .get(e)
                .is_some_and(|x| x.target_filter.is_none())
            {
                return false;
            }
        }
        self.is_player_can_spsummon_count(playerid, 1)
    }

    /// `field::is_player_can_spsummon_monster` (`field.cpp:2898`) — the
    /// same question about a monster that is **not on the board**: a
    /// token about to be made, or a card about to be summoned from
    /// nowhere.
    ///
    /// The reference lends its `temp_card` the described data, asks, and
    /// blanks it again. Doing the same here keeps the question honest:
    /// every prohibition that reads the card — a type, a level, an
    /// attribute — sees the shape being asked about rather than a
    /// stand-in.
    pub fn is_player_can_spsummon_monster(
        &mut self,
        playerid: u8,
        toplayer: u8,
        sumpos: u8,
        sumtype: u32,
        data: crate::card::CardData,
    ) -> bool {
        let scratch = match self.core.temp_card {
            Some(c) => c,
            None => {
                let c = self.new_card(crate::card::Card::with_data(
                    crate::card::CardData::default(),
                    playerid,
                ));
                self.core.temp_card = Some(c);
                c
            }
        };
        self.cards[scratch].data = data;
        self.cards[scratch].owner = playerid;
        self.cards[scratch].current.controller = playerid;
        let effect = self.core.reason_effect;
        let result =
            self.is_player_can_spsummon(effect, sumtype, sumpos, playerid, toplayer, scratch, None);
        self.cards[scratch].data = crate::card::CardData::default();
        result
    }

    #[allow(clippy::too_many_arguments)]
    /// `field::is_player_can_spsummon(effect*, sumtype, sumpos, playerid,
    /// toplayer, card*, proc_effect)` — the full question.
    ///
    /// Ten refusals in order, and three of them are the kind that look like
    /// noise until they are not:
    ///
    /// - **A Link Monster is narrowed to face-up attack** before the
    ///   emptiness test, so a Link asked for in defence is refused rather
    ///   than quietly summoned sideways.
    /// - **`EFFECT_DEVINE_LIGHT` folds face-down into face-up** by shifting
    ///   the face-down bits down one, which turns *defence* into *defence*
    ///   and *attack* into *attack* — the positions pair up that way in the
    ///   bit layout, and a mask would not.
    /// - **The cost condition is asked with the LP cost saved and restored**
    ///   around it. Asking whether a cost *could* be paid must not leave the
    ///   duel believing it was.
    pub fn is_player_can_spsummon(
        &mut self,
        peffect: Option<EffectId>,
        sumtype: u32,
        sumpos: u8,
        playerid: u8,
        toplayer: u8,
        card: CardId,
        proc_effect: Option<EffectId>,
    ) -> bool {
        if self
            .is_affected_by_effect(card, code::CANNOT_SPECIAL_SUMMON)
            .is_some()
        {
            return false;
        }
        if self.cards[card].is_status(status::FORBIDDEN) {
            return false;
        }
        let ty = self.cards[card].data.type_;
        if ty & card_type::TOKEN != 0
            && self.cards[card]
                .current
                .is_location(u16::from(location::ONFIELD))
        {
            return false;
        }
        let mut sumpos = sumpos;
        if ty & card_type::LINK != 0 && ty & card_type::MONSTER != 0 {
            sumpos &= position::FACEUP_ATTACK;
        }
        if sumpos == 0 {
            return false;
        }
        let sumtype = sumtype | summon_type::SPECIAL;

        self.save_lp_cost();
        let affordable =
            self.check_cost_condition_typed(card, code::SPSUMMON_COST, playerid, sumtype);
        self.restore_lp_cost();
        if !affordable {
            return false;
        }

        if sumpos & position::FACEDOWN != 0
            && self
                .is_player_affected_by_effect(playerid, code::DEVINE_LIGHT)
                .is_some()
        {
            sumpos = (sumpos & position::FACEUP) | ((sumpos & position::FACEDOWN) >> 1);
        }

        // The card-aware reading: a prohibition with **no** target is
        // unconditional and refuses; one with a target refuses only if the
        // target agrees. Note that this is `if condition` rather than
        // `if !condition` — the filter says "this is forbidden", not "this
        // is allowed".
        for e in self.filter_player_effect(playerid, code::CANNOT_SPECIAL_SUMMON) {
            let Some(check) = self.effects.get(e).and_then(|x| x.target_filter) else {
                return false;
            };
            let args = [
                i64::from(playerid),
                i64::from(sumtype),
                i64::from(sumpos),
                i64::from(toplayer),
                peffect.map_or(-1, |x| x as i64),
                proc_effect.map_or(-1, |x| x as i64),
            ];
            if check(self, e, Some(card), &args) {
                return false;
            }
        }

        for e in self.filter_player_effect(playerid, code::FORCE_SPSUMMON_POSITION) {
            if let Some(check) = self.effects.get(e).and_then(|x| x.target_filter) {
                let args = [
                    i64::from(playerid),
                    i64::from(sumtype),
                    i64::from(sumpos),
                    i64::from(toplayer),
                    peffect.map_or(-1, |x| x as i64),
                ];
                if !check(self, e, Some(card), &args) {
                    continue;
                }
            }
            sumpos &= self.effect_plain_value(e) as u8;
            if sumpos == 0 {
                return false;
            }
        }

        if !self.check_spsummon_once(card, playerid) {
            return false;
        }
        self.check_spsummon_counter(playerid, 1)
    }

    /// `field::is_player_can_flipsummon`.
    ///
    /// The card-aware polarity again: no target means unconditional.
    /// `FlipSummon` itself does not call this — the command layer that
    /// offers the flip does, which is why it lands here rather than in that
    /// machine.
    pub fn is_player_can_flipsummon(&mut self, playerid: u8, card: CardId) -> bool {
        for e in self.filter_player_effect(playerid, code::CANNOT_FLIP_SUMMON) {
            let Some(check) = self.effects.get(e).and_then(|x| x.target_filter) else {
                return false;
            };
            let args = [i64::from(playerid)];
            if check(self, e, Some(card), &args) {
                return false;
            }
        }
        true
    }

    /// `card::check_cost_condition(ecode, playerid, sumtype)` — can every
    /// cost of this kind be paid?
    ///
    /// The effects come from **two** places: the player's field-wide ones and
    /// the card's own. The field sweep is the *unsorted* form of
    /// `filter_player_effect` in the reference; the port's is sorted either
    /// way, which is a difference only when two costs both refuse and
    /// neither refusal is observable.
    ///
    /// It is the effect's **`cost`** slot that is asked, not its condition.
    /// The reference calls the same Lua ref at a shorter arity to do it —
    /// `check_condition(peffect->cost, 4)` rather than running it as a cost —
    /// and this port spells that as `chk = false`, which is its existing
    /// name for "ask, do not pay". The extra parameters ride in `args`.
    pub fn check_cost_condition_typed(
        &mut self,
        card: CardId,
        ecode: u32,
        playerid: u8,
        sumtype: u32,
    ) -> bool {
        let mut candidates = self.filter_player_effect(playerid, ecode);
        candidates.extend(self.filter_effect(card, ecode));
        for e in candidates {
            let Some(cost) = self.effects.get(e).and_then(|x| x.cost) else {
                // No cost function: nothing to refuse with.
                continue;
            };
            let args = [i64::from(playerid), i64::from(sumtype)];
            let asked = cost(
                self,
                &crate::effect::Ctx {
                    reason_effect: e,
                    player: playerid,
                    event: &crate::event::Event::new(0),
                    card: Some(card),
                    args: &args,
                },
                false,
            );
            if !asked {
                return false;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{Card, CardData};
    use crate::effect::{effect_type, flag, Effect};

    fn on_field(f: &mut Field, player: u8, seat: u32, type_: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 18036057,
                type_,
                ..Default::default()
            },
            player,
        );
        c.current.controller = player;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(player, id, location::MZONE, seat, false);
        f.cards[id].current.position = position::FACEUP_ATTACK;
        id
    }

    fn monster(f: &mut Field, player: u8, seat: u32) -> CardId {
        on_field(f, player, seat, card_type::MONSTER)
    }

    /// A field-wide effect applying to one player.
    fn player_effect(f: &mut Field, card: CardId, code_: u32, value: i64) -> EffectId {
        let mut e = Effect::new(effect_type::FIELD, code_);
        e.owner = Some(card);
        e.handler = Some(card);
        e.value = value;
        e.flag[0] = flag::PLAYER_TARGET | flag::ABSOLUTE_TARGET;
        e.range = u16::from(location::MZONE);
        e.s_range = 1;
        let id = f.new_effect(e);
        f.add_effect(id, 0);
        id
    }

    /// An `EFFECT_SPSUMMON_COUNT_LIMIT`: a ceiling of `value`, covering the
    /// controller's own summons when `s` is set and the opponent's when `o`
    /// is.
    fn count_limit(f: &mut Field, card: CardId, value: i64, s: u16, o: u16) -> EffectId {
        let mut e = Effect::new(effect_type::FIELD, code::SPSUMMON_COUNT_LIMIT);
        e.owner = Some(card);
        e.handler = Some(card);
        e.value = value;
        e.range = u16::from(location::MZONE);
        e.s_range = s;
        e.o_range = o;
        let id = f.new_effect(e);
        f.add_effect(id, 0);
        id
    }

    mod check_spsummon_once {
        use super::*;

        /// Most cards are not name-limited at all, and the zero shortcuts.
        #[test]
        fn a_card_with_no_code_is_never_limited() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            f.core.spsummon_once_map[0].insert(0, 5);
            assert!(f.check_spsummon_once(c, 0), "zero is not a name");
        }

        #[test]
        fn a_name_already_summoned_is_refused() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            f.cards[c].spsummon_code = 777;
            assert!(f.check_spsummon_once(c, 0));
            f.core.spsummon_once_map[0].insert(777, 1);
            assert!(!f.check_spsummon_once(c, 0));
        }

        /// The ledger is per player: the opponent summoning it does not use
        /// up your own allowance.
        #[test]
        fn the_ledger_is_per_player() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            f.cards[c].spsummon_code = 777;
            f.core.spsummon_once_map[1].insert(777, 1);
            assert!(f.check_spsummon_once(c, 0), "the other player's ledger");
            assert!(!f.check_spsummon_once(c, 1));
        }

        /// A zeroed entry reads as absent — which is what makes the
        /// give-back work without deleting keys.
        #[test]
        fn a_zeroed_entry_is_as_good_as_absent() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            f.cards[c].spsummon_code = 777;
            f.core.spsummon_once_map[0].insert(777, 0);
            assert!(f.check_spsummon_once(c, 0));
        }
    }

    mod counters {
        use super::*;

        #[test]
        fn a_special_summon_is_counted_once() {
            let mut f = Field::new(8000);
            f.set_spsummon_counter(0, true, false);
            assert_eq!(f.core.spsummon_state_count[0], 1);
            assert_eq!(f.core.spsummon_state_count[1], 0);
            assert_eq!(
                f.core.spsummon_state_count_rst[0], 0,
                "nothing to give back without the old oath option"
            );
        }

        /// **The per-effect tally lives on the handler card**, not on the
        /// effect. Several effects on one card share the card's counter, and
        /// that is the reference's arrangement rather than a simplification.
        #[test]
        fn the_tally_lands_on_the_handler_card() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            count_limit(&mut f, c, 1, 1, 0);
            f.set_spsummon_counter(0, true, false);
            assert_eq!(f.cards[c].spsummon_counter[0], 1);
            assert_eq!(f.cards[c].spsummon_counter[1], 0);
        }

        /// An effect that is not available does not count.
        #[test]
        fn an_unavailable_effect_is_not_counted() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            count_limit(&mut f, c, 1, 1, 0);
            // Face-down: the effect is out of range of itself.
            f.cards[c].current.position = position::FACEDOWN_DEFENSE;
            f.set_spsummon_counter(0, true, false);
            assert_eq!(f.cards[c].spsummon_counter[0], 0);
        }

        /// `s_range` covers the controller's summons and `o_range` the
        /// opponent's. An effect with neither counts nobody's.
        #[test]
        fn the_range_decides_whose_summons_count() {
            let mut f = Field::new(8000);
            let own = monster(&mut f, 0, 0);
            let opp = monster(&mut f, 0, 1);
            let neither = monster(&mut f, 0, 2);
            count_limit(&mut f, own, 9, 1, 0);
            count_limit(&mut f, opp, 9, 0, 1);
            count_limit(&mut f, neither, 9, 0, 0);

            f.set_spsummon_counter(0, true, false);
            assert_eq!(f.cards[own].spsummon_counter[0], 1, "its controller's");
            assert_eq!(f.cards[opp].spsummon_counter[0], 0);
            assert_eq!(f.cards[neither].spsummon_counter[0], 0);

            f.set_spsummon_counter(1, true, false);
            assert_eq!(f.cards[own].spsummon_counter[1], 0);
            assert_eq!(f.cards[opp].spsummon_counter[1], 1, "the opponent's");
        }

        /// Under the old oath option a summon made during a chain is
        /// recorded twice — once as the tally and once as what a negation
        /// could take back — and the give-back removes exactly that.
        #[test]
        fn the_old_oath_option_records_and_returns_a_give_back() {
            let mut f = Field::with_flags(
                8000,
                crate::duel::REFERENCE_CONFIGURATION | crate::duel::flags::CANNOT_SUMMON_OATH_OLD,
            );
            let c = monster(&mut f, 0, 0);
            count_limit(&mut f, c, 9, 1, 0);

            f.set_spsummon_counter(0, true, false);
            f.set_spsummon_counter(0, true, true);
            assert_eq!(f.core.spsummon_state_count[0], 2);
            assert_eq!(f.core.spsummon_state_count_rst[0], 1, "only the chain's");
            assert_eq!(f.cards[c].spsummon_counter[0], 2);
            assert_eq!(f.cards[c].spsummon_counter_rst[0], 1);

            f.set_spsummon_counter(0, false, true);
            assert_eq!(f.core.spsummon_state_count[0], 1, "the chain's is returned");
            assert_eq!(f.core.spsummon_state_count_rst[0], 0);
            assert_eq!(f.cards[c].spsummon_counter[0], 1);
            assert_eq!(f.cards[c].spsummon_counter_rst[0], 0);
        }

        /// Without a chain, the give-back is a plain decrement.
        #[test]
        fn the_old_oath_option_can_also_take_one_back_outright() {
            let mut f = Field::with_flags(
                8000,
                crate::duel::REFERENCE_CONFIGURATION | crate::duel::flags::CANNOT_SUMMON_OATH_OLD,
            );
            f.set_spsummon_counter(0, true, false);
            f.set_spsummon_counter(0, true, false);
            f.set_spsummon_counter(0, false, false);
            assert_eq!(f.core.spsummon_state_count[0], 1);
        }

        /// **The comparison is `>`, not `>=`.** The value is the number
        /// allowed, so reaching it exactly is still legal.
        #[test]
        fn reaching_the_ceiling_exactly_is_allowed() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            count_limit(&mut f, c, 2, 1, 0);
            f.cards[c].spsummon_counter[0] = 1;
            assert!(f.check_spsummon_counter(0, 1), "1 + 1 > 2 is false");
            f.cards[c].spsummon_counter[0] = 2;
            assert!(!f.check_spsummon_counter(0, 1), "2 + 1 > 2 is true");
        }

        /// A batch is tested as a whole, not one at a time: three summons
        /// against a ceiling of two is refused even from zero.
        #[test]
        fn a_batch_is_tested_as_a_whole() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            count_limit(&mut f, c, 2, 1, 0);
            assert!(f.check_spsummon_counter(0, 2));
            assert!(!f.check_spsummon_counter(0, 3));
        }

        /// A ceiling only binds while its effect is available — a
        /// face-down card imposes nothing.
        ///
        /// The counterpart of the availability test in `set_spsummon_counter`
        /// above, and a separate one: the two functions each make the check
        /// for themselves.
        #[test]
        fn an_unavailable_ceiling_does_not_bind() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            count_limit(&mut f, c, 0, 1, 0);
            assert!(!f.check_spsummon_counter(0, 1), "face-up, it binds");
            f.cards[c].current.position = position::FACEDOWN_DEFENSE;
            assert!(f.check_spsummon_counter(0, 1), "face-down, it does not");
        }

        /// A ceiling on the opponent's summons does not bind your own.
        #[test]
        fn the_ceiling_is_read_against_the_asking_player() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            count_limit(&mut f, c, 0, 0, 1);
            f.cards[c].spsummon_counter[1] = 0;
            assert!(!f.check_spsummon_counter(1, 1), "0 + 1 > 0");
            assert!(!f.check_spsummon_counter(0, 1), "the ceiling is shared");
        }
    }

    mod player_permission {
        use super::*;

        /// `EFFECT_LEFT_SPSUMMON_COUNT` answers with a number.
        #[test]
        fn the_left_count_answers_with_a_number() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            player_effect(&mut f, c, code::LEFT_SPSUMMON_COUNT, 2);
            assert!(f.is_player_can_spsummon_count(0, 2));
            assert!(!f.is_player_can_spsummon_count(0, 3));
        }

        /// Every such effect must allow it — the smallest wins.
        #[test]
        fn every_left_count_must_allow_it() {
            let mut f = Field::new(8000);
            let a = monster(&mut f, 0, 0);
            let b = monster(&mut f, 0, 1);
            player_effect(&mut f, a, code::LEFT_SPSUMMON_COUNT, 5);
            player_effect(&mut f, b, code::LEFT_SPSUMMON_COUNT, 1);
            assert!(f.is_player_can_spsummon_count(0, 1));
            assert!(!f.is_player_can_spsummon_count(0, 2), "the smaller binds");
        }

        /// **With no card to ask about, only an unconditional prohibition can
        /// answer** — the opposite of the card-aware form.
        #[test]
        fn only_an_unconditional_prohibition_answers_the_bare_form() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            let e = player_effect(&mut f, c, code::CANNOT_SPECIAL_SUMMON, 0);
            assert!(!f.is_player_can_spsummon_player(0), "no target: refuses");

            if let Some(x) = f.effects.get_mut(e) {
                x.target_filter = Some(|_, _, _, _| true);
            }
            assert!(
                f.is_player_can_spsummon_player(0),
                "a conditional one has nothing to test here, so it is passed over"
            );
        }

        /// And the ceiling is still consulted.
        #[test]
        fn the_bare_form_still_reads_the_ceiling() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            count_limit(&mut f, c, 0, 1, 0);
            assert!(!f.is_player_can_spsummon_player(0));
        }
    }

    mod can_spsummon {
        use super::*;

        fn ask(f: &mut Field, card: CardId, pos: u8) -> bool {
            f.is_player_can_spsummon(None, 0, pos, 0, 0, card, None)
        }

        #[test]
        fn an_ordinary_monster_may_be_summoned() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            assert!(ask(&mut f, c, position::FACEUP));
        }

        #[test]
        fn a_forbidden_card_is_refused() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            f.cards[c].set_status(status::FORBIDDEN, true);
            assert!(!ask(&mut f, c, position::FACEUP));
        }

        /// A Token already on the field cannot be Special Summoned again;
        /// one that is not on the field can.
        #[test]
        fn a_token_already_on_the_field_is_refused() {
            let mut f = Field::new(8000);
            let c = on_field(&mut f, 0, 0, card_type::MONSTER | card_type::TOKEN);
            assert!(!ask(&mut f, c, position::FACEUP));
            f.remove_card(c);
            f.add_card(0, c, location::GRAVE, 0, false);
            assert!(ask(&mut f, c, position::FACEUP));
        }

        /// **A Link Monster is narrowed to face-up attack**, so one asked for
        /// in defence is refused rather than summoned sideways.
        #[test]
        fn a_link_monster_is_narrowed_to_face_up_attack() {
            let mut f = Field::new(8000);
            let c = on_field(&mut f, 0, 0, card_type::MONSTER | card_type::LINK);
            assert!(ask(&mut f, c, position::FACEUP));
            assert!(
                !ask(&mut f, c, position::FACEUP_DEFENSE),
                "nothing is left of the mask"
            );
            let plain = monster(&mut f, 0, 1);
            assert!(
                ask(&mut f, plain, position::FACEUP_DEFENSE),
                "and an ordinary monster is not narrowed"
            );
        }

        #[test]
        fn an_empty_position_mask_is_refused() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            assert!(!ask(&mut f, c, 0));
        }

        /// **`EFFECT_DEVINE_LIGHT` shifts the face-down bits down one.**
        ///
        /// The four position bits pair up as attack, attack, defence,
        /// defence, so `>> 1` turns face-down defence into face-up defence
        /// and face-down attack into face-up attack. A mask would have lost
        /// which of the two it was.
        #[test]
        fn devine_light_folds_face_down_into_face_up() {
            let mut f = Field::new(8000);
            let anchor = monster(&mut f, 0, 0);
            player_effect(&mut f, anchor, code::DEVINE_LIGHT, 0);
            let c = monster(&mut f, 0, 1);
            // Face-down only: without the fold there would be nothing left
            // once `FORCE_SPSUMMON_POSITION` narrowed it to face-up.
            let forcing = monster(&mut f, 0, 2);
            player_effect(
                &mut f,
                forcing,
                code::FORCE_SPSUMMON_POSITION,
                i64::from(position::FACEUP_DEFENSE),
            );
            assert!(
                ask(&mut f, c, position::FACEDOWN_DEFENSE),
                "folded to face-up defence, which the forcing effect allows"
            );
            assert!(
                !ask(&mut f, c, position::FACEDOWN_ATTACK),
                "folded to face-up attack, which it does not"
            );
        }

        /// Forcing a position can narrow the mask to nothing, and does so
        /// **inside** the loop — the emptiness is tested after each effect,
        /// not once at the end.
        #[test]
        fn forcing_a_position_can_narrow_it_to_nothing() {
            let mut f = Field::new(8000);
            let anchor = monster(&mut f, 0, 0);
            player_effect(
                &mut f,
                anchor,
                code::FORCE_SPSUMMON_POSITION,
                i64::from(position::FACEUP_DEFENSE),
            );
            let c = monster(&mut f, 0, 1);
            assert!(ask(&mut f, c, position::FACEUP));
            assert!(!ask(&mut f, c, position::FACEUP_ATTACK));
        }

        /// **The polarity of the card-aware prohibition is inverted.** The
        /// target says "this is forbidden", so a target that *agrees*
        /// refuses the summon.
        #[test]
        fn a_prohibition_refuses_when_its_target_agrees() {
            let mut f = Field::new(8000);
            let anchor = monster(&mut f, 0, 0);
            let e = player_effect(&mut f, anchor, code::CANNOT_SPECIAL_SUMMON, 0);
            let c = monster(&mut f, 0, 1);
            assert!(
                !ask(&mut f, c, position::FACEUP),
                "no target: unconditional"
            );

            if let Some(x) = f.effects.get_mut(e) {
                x.target_filter = Some(|_, _, _, _| false);
            }
            assert!(ask(&mut f, c, position::FACEUP), "the target declined");

            if let Some(x) = f.effects.get_mut(e) {
                x.target_filter = Some(|_, _, _, _| true);
            }
            assert!(!ask(&mut f, c, position::FACEUP), "and agreeing refuses");
        }

        /// A prohibition on the card itself refuses before anything else is
        /// asked.
        #[test]
        fn a_prohibition_on_the_card_refuses() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            let mut e = Effect::new(effect_type::SINGLE, code::CANNOT_SPECIAL_SUMMON);
            e.owner = Some(c);
            e.handler = Some(c);
            let id = f.new_effect(e);
            f.cards[c]
                .single_effect
                .insert(code::CANNOT_SPECIAL_SUMMON, id);
            f.cards[c].indexer.insert(id);
            assert!(!ask(&mut f, c, position::FACEUP));
        }

        /// The once-per-turn ledger and the ceiling are both consulted last.
        #[test]
        fn the_name_limit_and_the_ceiling_are_consulted() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            f.cards[c].spsummon_code = 777;
            f.core.spsummon_once_map[0].insert(777, 1);
            assert!(!ask(&mut f, c, position::FACEUP));

            f.core.spsummon_once_map[0].insert(777, 0);
            assert!(ask(&mut f, c, position::FACEUP));

            let limiter = monster(&mut f, 0, 1);
            count_limit(&mut f, limiter, 0, 1, 0);
            assert!(!ask(&mut f, c, position::FACEUP));
        }

        /// **The cost is asked, not paid**, and the LP cost stack is put back
        /// the way it was around the question.
        #[test]
        fn the_cost_condition_is_asked_and_the_stack_restored() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            let mut e = Effect::new(effect_type::SINGLE, code::SPSUMMON_COST);
            e.owner = Some(c);
            e.handler = Some(c);
            e.cost = Some(|_, _, chk| {
                // Refuse, and assert on the way past that nothing is being
                // paid: `chk` false is the question form.
                assert!(!chk, "the cost must be asked rather than paid");
                false
            });
            let id = f.new_effect(e);
            f.cards[c].single_effect.insert(code::SPSUMMON_COST, id);
            f.cards[c].indexer.insert(id);

            let before = f.core.lp_cost[0].count;
            assert!(!ask(&mut f, c, position::FACEUP), "the cost refused");
            assert_eq!(f.core.lp_cost[0].count, before, "and the stack is level");
        }

        /// A cost with no function refuses nothing.
        #[test]
        fn a_cost_with_no_function_refuses_nothing() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            let mut e = Effect::new(effect_type::SINGLE, code::SPSUMMON_COST);
            e.owner = Some(c);
            e.handler = Some(c);
            let id = f.new_effect(e);
            f.cards[c].single_effect.insert(code::SPSUMMON_COST, id);
            f.cards[c].indexer.insert(id);
            assert!(ask(&mut f, c, position::FACEUP));
        }
    }

    mod flip_summon_permission {
        use super::*;

        #[test]
        fn nothing_forbidding_permits_it() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            assert!(f.is_player_can_flipsummon(0, c));
        }

        #[test]
        fn a_prohibition_refuses_unconditionally_or_when_its_target_agrees() {
            let mut f = Field::new(8000);
            let anchor = monster(&mut f, 0, 0);
            let e = player_effect(&mut f, anchor, code::CANNOT_FLIP_SUMMON, 0);
            let c = monster(&mut f, 0, 1);
            assert!(!f.is_player_can_flipsummon(0, c), "no target");

            if let Some(x) = f.effects.get_mut(e) {
                x.target_filter = Some(|_, _, _, _| false);
            }
            assert!(f.is_player_can_flipsummon(0, c), "the target declined");

            if let Some(x) = f.effects.get_mut(e) {
                x.target_filter = Some(|_, _, _, _| true);
            }
            assert!(!f.is_player_can_flipsummon(0, c));
        }
    }

    /// The registration branch this port had been missing.
    mod registration {
        use super::*;

        #[test]
        fn a_count_limit_effect_is_indexed_and_removed() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            let id = count_limit(&mut f, c, 1, 1, 0);
            assert!(f.field_effects.spsummon_count_eff.contains(&id));
            f.remove_effect(id);
            assert!(!f.field_effects.spsummon_count_eff.contains(&id));
        }

        /// **The three global flags.** Each gates a pass that, until this
        /// branch existed, could never run in a real duel.
        #[test]
        fn the_three_global_flags_are_raised_on_registration() {
            for (code_, flag_) in [
                (code::SELF_TOGRAVE, crate::field::global_flag::SELF_TOGRAVE),
                (
                    code::REMOVE_BRAINWASHING,
                    crate::field::global_flag::BRAINWASHING_CHECK,
                ),
                (
                    code::REVERSE_DECK,
                    crate::field::global_flag::DECK_REVERSE_CHECK,
                ),
            ] {
                let mut f = Field::new(8000);
                let c = monster(&mut f, 0, 0);
                assert_eq!(f.core.global_flag & flag_, 0, "code {code_}");
                player_effect(&mut f, c, code_, 0);
                assert_ne!(f.core.global_flag & flag_, 0, "code {code_}");
            }
        }

        /// **They are one-way.** Removing the effect leaves the flag set, so
        /// the pass runs and finds nothing rather than being switched off.
        #[test]
        fn the_flags_are_never_lowered() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            let id = player_effect(&mut f, c, code::SELF_TOGRAVE, 0);
            f.remove_effect(id);
            assert_ne!(
                f.core.global_flag & crate::field::global_flag::SELF_TOGRAVE,
                0
            );
        }

        /// An effect with an action type takes the other branch entirely.
        #[test]
        fn an_action_effect_is_not_given_the_treatment() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            let mut e = Effect::new(
                effect_type::ACTIONS | effect_type::TRIGGER_O | effect_type::FIELD,
                code::SELF_TOGRAVE,
            );
            e.owner = Some(c);
            e.handler = Some(c);
            e.range = u16::from(location::MZONE);
            let id = f.new_effect(e);
            f.add_effect(id, 0);
            assert_eq!(
                f.core.global_flag & crate::field::global_flag::SELF_TOGRAVE,
                0
            );
        }
    }
}

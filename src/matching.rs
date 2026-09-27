//! `field::filter_matching_card` — the scan behind `Duel.GetMatchingGroup`,
//! `IsExistingMatchingCard`, `GetFirstMatchingCard` and their `Target`
//! variants (`field.cpp`), with the two helpers it reads:
//! `get_pzone_index` and `card::is_capable_be_effect_target`.
//!
//! The reference takes a Lua filter, an owner, two location masks (the
//! owner's and the opponent's), an optional group to fill, an exception
//! (one card or a group), an optional out-parameter for the first match,
//! a match count to stop at, and the `is_target` flag. The one thing not
//! carried across is `extraargs` — the Lua varargs forwarded to the filter
//! — because a Rust filter captures what it needs.
//!
//! ## The three list tests
//!
//! Each zone list has its own extra test wrapped around the caller's
//! filter, and they use **different** status readers: the Monster Zone
//! excludes any of `SUMMONING | SUMMON_DISABLED | SPSUMMON_STEP`
//! (`get_status`, any-bit), the Spell row and pendulum zones exclude
//! `ACTIVATE_DISABLED` (`is_status`), and the pendulum test also wants the
//! card to actually be a pendulum in its zone. The piles (deck, extra,
//! hand, grave, banished) get no extra test.
//!
//! ## Three-column fields
//!
//! Under `DUEL_3_COLUMNS_FIELD` the extra-monster range shrinks from
//! `[5, 7)` to `[6, 6)` — empty — and the spell-row range from `[0, 5)` to
//! `[1, 4)`. Transcribed as the reference has it, empty range and all.

use crate::board::location;
use crate::card::{card_type, status};
use crate::duel::flags;
use crate::effect::Ctx;
use crate::event::{code, CardId, EffectId, Event};
use crate::field::Field;
use crate::script_api::Filter;

/// The zone-dependent test wrapped around the filter (`mzonechk`,
/// `szonechk`, `pzonechk`, or `checkc` bare).
#[derive(Clone, Copy)]
enum Extra {
    Plain,
    Mzone,
    Szone,
    Pzone,
}

/// The reference's `checkc` closure and the state it captures.
struct Scan<'a> {
    filter: Option<Filter<'a>>,
    exception: Option<CardId>,
    exgroup: Option<&'a [CardId]>,
    group: Option<&'a mut Vec<CardId>>,
    first: Option<&'a mut Option<CardId>>,
    fcount: i32,
    is_target: bool,
    count: i32,
}

impl Scan<'_> {
    /// `checkc(pcard, extrafil)` — `true` means the scan stops here.
    fn check(&mut self, f: &mut Field, card: Option<CardId>, extra: Extra) -> bool {
        let Some(c) = card else {
            return false;
        };
        let cd = &f.cards[c];
        let extra_ok = match extra {
            Extra::Plain => true,
            Extra::Mzone => {
                !cd.get_status(status::SUMMONING | status::SUMMON_DISABLED | status::SPSUMMON_STEP)
            }
            Extra::Szone => !cd.is_status(status::ACTIVATE_DISABLED),
            Extra::Pzone => cd.current.pzone && !cd.is_status(status::ACTIVATE_DISABLED),
        };
        if !extra_ok || self.exception == Some(c) || self.exgroup.is_some_and(|g| g.contains(&c)) {
            return false;
        }
        if let Some(filter) = self.filter {
            if !filter(f, c) {
                return false;
            }
        }
        // The reference dereferences `core.reason_effect` for the handler
        // check, so a target scan with no reason effect cannot happen in a
        // sane duel; with none, nothing is targetable.
        if self.is_target
            && !f
                .core
                .reason_effect
                .is_some_and(|re| f.is_capable_be_effect_target(c, re, f.core.reason_player))
        {
            return false;
        }
        if let Some(first) = self.first.as_deref_mut() {
            *first = Some(c);
            return true;
        }
        self.count += 1;
        if self.fcount != 0 && self.count >= self.fcount {
            return true;
        }
        if let Some(group) = self.group.as_deref_mut() {
            group.push(c);
        }
        false
    }

    /// `check_list` / `std::find_if` over seats `begin..end` of a zone
    /// row — `true` if any entry stopped the scan. Read by index each
    /// time, because the filter takes the field mutably and may not be
    /// holding a borrow of the row.
    fn any_seat(
        &mut self,
        f: &mut Field,
        me: u8,
        row: Row,
        begin: usize,
        end: usize,
        extra: Extra,
    ) -> bool {
        for i in begin..end {
            let card = row.seat(f, me, i);
            if self.check(f, card, extra) {
                return true;
            }
        }
        false
    }

    /// `check_list` over a whole pile.
    fn any_pile(&mut self, f: &mut Field, me: u8, pile: Pile) -> bool {
        let mut i = 0;
        while let Some(card) = pile.at(f, me, i) {
            if self.check(f, Some(card), Extra::Plain) {
                return true;
            }
            i += 1;
        }
        false
    }
}

/// The two zone rows.
#[derive(Clone, Copy)]
enum Row {
    Monster,
    Spell,
}

impl Row {
    fn seat(self, f: &Field, me: u8, i: usize) -> Option<CardId> {
        let pz = &f.players[usize::from(me)];
        match self {
            Row::Monster => pz.mzone.get(i).copied().flatten(),
            Row::Spell => pz.szone.get(i).copied().flatten(),
        }
    }
}

/// The five piles, in the order the reference scans them.
#[derive(Clone, Copy)]
enum Pile {
    Deck,
    Extra,
    Hand,
    Grave,
    Removed,
}

impl Pile {
    const SCAN_ORDER: [(u8, Pile); 5] = [
        (location::DECK, Pile::Deck),
        (location::EXTRA, Pile::Extra),
        (location::HAND, Pile::Hand),
        (location::GRAVE, Pile::Grave),
        (location::REMOVED, Pile::Removed),
    ];

    fn at(self, f: &Field, me: u8, i: usize) -> Option<CardId> {
        let pz = &f.players[usize::from(me)];
        let list = match self {
            Pile::Deck => &pz.main,
            Pile::Extra => &pz.extra,
            Pile::Hand => &pz.hand,
            Pile::Grave => &pz.grave,
            Pile::Removed => &pz.removed,
        };
        list.get(i).copied()
    }
}

impl Field {
    /// `field::filter_matching_card`.
    ///
    /// Returns `true` as soon as the scan stops — at the first match when
    /// `first` is given, at the `fcount`-th match when `fcount` is
    /// non-zero — and `false` after a full scan otherwise, having pushed
    /// every match onto `group` if one was given. A caller collecting a
    /// group therefore passes `fcount == 0`.
    ///
    /// Takes the field mutably because the filter does: the reference's is
    /// a Lua function that may call anything, and `Card.IsType` alone
    /// writes the card's scratch type while it evaluates.
    #[allow(clippy::too_many_arguments)]
    pub fn filter_matching_card(
        &mut self,
        filter: Option<Filter>,
        self_: u8,
        location1: u32,
        location2: u32,
        group: Option<&mut Vec<CardId>>,
        exception: Option<CardId>,
        exgroup: Option<&[CardId]>,
        first: Option<&mut Option<CardId>>,
        fcount: i32,
        is_target: bool,
    ) -> bool {
        if self_ != 0 && self_ != 1 {
            return false;
        }
        let mut scan = Scan {
            filter,
            exception,
            exgroup,
            group,
            first,
            fcount,
            is_target,
            count: 0,
        };
        let three_columns = self.is_flag(flags::THREE_COLUMNS_FIELD);
        let mut me = self_;
        let mut location = location1;
        for p in 0..2 {
            if p == 1 {
                location = location2;
                me = 1 - me;
            }
            if location & u32::from(location::MZONE) != 0 {
                if scan.any_seat(self, me, Row::Monster, 0, 7, Extra::Mzone) {
                    return true;
                }
            } else {
                if location & u32::from(location::MMZONE) != 0
                    && scan.any_seat(self, me, Row::Monster, 0, 5, Extra::Mzone)
                {
                    return true;
                }
                if location & u32::from(location::EMZONE) != 0 {
                    let (mut begin, mut end) = (5, 7);
                    if three_columns {
                        begin += 1;
                        end -= 1;
                    }
                    if scan.any_seat(self, me, Row::Monster, begin, end, Extra::Mzone) {
                        return true;
                    }
                }
            }
            if location & u32::from(location::SZONE) != 0 {
                if scan.any_seat(self, me, Row::Spell, 0, 8, Extra::Szone) {
                    return true;
                }
            } else {
                if location & u32::from(location::STZONE) != 0 {
                    let (mut begin, mut end) = (0, 5);
                    if three_columns {
                        begin += 1;
                        end -= 1;
                    }
                    if scan.any_seat(self, me, Row::Spell, begin, end, Extra::Szone) {
                        return true;
                    }
                }
                if location & u32::from(location::FZONE) != 0
                    && scan.any_seat(self, me, Row::Spell, 5, 6, Extra::Szone)
                {
                    return true;
                }
                if location & u32::from(location::PZONE) != 0 {
                    let (left, right) = (self.get_pzone_index(0, me), self.get_pzone_index(1, me));
                    if scan.any_seat(self, me, Row::Spell, left, left + 1, Extra::Pzone)
                        || scan.any_seat(self, me, Row::Spell, right, right + 1, Extra::Pzone)
                    {
                        return true;
                    }
                }
            }
            for (loc, pile) in Pile::SCAN_ORDER {
                if location & u32::from(loc) != 0 && scan.any_pile(self, me, pile) {
                    return true;
                }
            }
        }
        false
    }

    /// `field::filter_field_card` — every card in the named zones, with
    /// no filter and no exception.
    ///
    /// A **second** scan, and deliberately not `filter_matching_card`
    /// with a null filter. Three differences, each of which would be
    /// "tidied" away by anyone who assumed the two were the same walk:
    ///
    /// | | `filter_matching_card` | here |
    /// |---|---|---|
    /// | the whole Monster Zone | skips `SUMMONING`, `SUMMON_DISABLED`, `SPSUMMON_STEP` | skips `SUMMONING`, `SPSUMMON_STEP` — **not** `SUMMON_DISABLED` |
    /// | the Monster Zone *sub*-masks | the same skip | **no skip at all** |
    /// | the Spell row | skips `ACTIVATE_DISABLED` | no skip |
    /// | `EMZONE` on three columns | shrinks to an empty range | **does not shrink** |
    ///
    /// The pendulum seats still want `current.pzone`, and the piles are
    /// taken whole. The reference inserts some piles back-to-front, which
    /// does not matter: the group is a set ordered by creation id, so the
    /// result is sorted either way — and it is sorted here for the same
    /// reason.
    ///
    /// Returns how many were found, whether or not a group was given.
    pub fn filter_field_card(
        &self,
        self_: u8,
        location1: u32,
        location2: u32,
        mut group: Option<&mut Vec<CardId>>,
    ) -> usize {
        if self_ != 0 && self_ != 1 {
            return 0;
        }
        let three_columns = self.is_flag(flags::THREE_COLUMNS_FIELD);
        let mut count = 0;
        let mut me = self_;
        let mut location = location1;
        for p in 0..2 {
            if p == 1 {
                location = location2;
                me = 1 - me;
            }
            let pz = &self.players[usize::from(me)];
            let mut take = |c: CardId, count: &mut usize| {
                if let Some(g) = group.as_deref_mut() {
                    g.push(c);
                }
                *count += 1;
            };
            if location & u32::from(location::MZONE) != 0 {
                for c in pz.mzone.iter().flatten().copied().collect::<Vec<_>>() {
                    if !self.cards[c].get_status(status::SUMMONING | status::SPSUMMON_STEP) {
                        take(c, &mut count);
                    }
                }
            } else {
                if location & u32::from(location::MMZONE) != 0 {
                    let (mut begin, mut end) = (0, 5);
                    if three_columns {
                        begin += 1;
                        end -= 1;
                    }
                    for c in pz.mzone[begin..end]
                        .iter()
                        .flatten()
                        .copied()
                        .collect::<Vec<_>>()
                    {
                        take(c, &mut count);
                    }
                }
                if location & u32::from(location::EMZONE) != 0 {
                    // No three-column shrink here, unlike the matching
                    // scan. Transcribed as the reference has it.
                    for c in pz.mzone[5..7].iter().flatten().copied().collect::<Vec<_>>() {
                        take(c, &mut count);
                    }
                }
            }
            if location & u32::from(location::SZONE) != 0 {
                for c in pz.szone.iter().flatten().copied().collect::<Vec<_>>() {
                    take(c, &mut count);
                }
            } else {
                if location & u32::from(location::STZONE) != 0 {
                    let (mut begin, mut end) = (0, 5);
                    if three_columns {
                        begin += 1;
                        end -= 1;
                    }
                    for c in pz.szone[begin..end]
                        .iter()
                        .flatten()
                        .copied()
                        .collect::<Vec<_>>()
                    {
                        take(c, &mut count);
                    }
                }
                if location & u32::from(location::FZONE) != 0 {
                    if let Some(c) = pz.szone[5] {
                        take(c, &mut count);
                    }
                }
                if location & u32::from(location::PZONE) != 0 {
                    for i in 0..2u8 {
                        let seat = self.get_pzone_index(i, me);
                        if let Some(c) = self.players[usize::from(me)].szone[seat] {
                            if self.cards[c].current.pzone {
                                take(c, &mut count);
                            }
                        }
                    }
                }
            }
            let pz = &self.players[usize::from(me)];
            for (loc, pile) in [
                (location::HAND, &pz.hand),
                (location::DECK, &pz.main),
                (location::EXTRA, &pz.extra),
                (location::GRAVE, &pz.grave),
                (location::REMOVED, &pz.removed),
            ] {
                if location & u32::from(loc) != 0 {
                    for c in pile.clone() {
                        take(c, &mut count);
                    }
                }
            }
        }
        if let Some(g) = group {
            g.sort_unstable();
            g.dedup();
        }
        count
    }

    /// `field::has_separate_pzone` — the player is unused in the
    /// reference too.
    pub fn has_separate_pzone(&self, _p: u8) -> bool {
        self.is_flag(flags::SEPARATE_PZONE)
    }

    /// `field::get_pzone_index` — which Spell-row seat holds pendulum
    /// scale `seq` (0 = left, 1 = right): seats 6 and 7 with separate
    /// pendulum zones, 1 and 3 on a three-column field, 0 and 4 otherwise.
    /// Anything but scale 0 or 1 is seat 0.
    pub fn get_pzone_index(&self, seq: u8, p: u8) -> usize {
        if seq > 1 {
            return 0;
        }
        if self.has_separate_pzone(p) {
            return usize::from(seq) + 6;
        }
        if self.is_flag(flags::THREE_COLUMNS_FIELD) {
            return usize::from(seq) * 2 + 1;
        }
        usize::from(seq) * 4
    }

    /// `card::is_capable_be_effect_target(peffect, playerid)` — whether
    /// `peffect` may target the card: not while it is being summoned or
    /// after battle destroyed it, never in the deck, extra or hand, a
    /// token only on the field, and no `CANNOT_BE_EFFECT_TARGET` on the
    /// card nor `CANNOT_SELECT_EFFECT_TARGET` on the effect's handler says
    /// otherwise.
    ///
    /// The first value read pushes the player, the second the card — as
    /// `Ctx::args` and `Ctx::card` respectively.
    pub fn is_capable_be_effect_target(
        &self,
        card: CardId,
        peffect: EffectId,
        playerid: u8,
    ) -> bool {
        let c = &self.cards[card];
        if c.is_status(status::SUMMONING) || c.is_status(status::BATTLE_DESTROYED) {
            return false;
        }
        if c.current.location & (location::DECK | location::EXTRA | location::HAND) != 0 {
            return false;
        }
        if c.data.type_ & card_type::TOKEN != 0 && c.current.location & location::ONFIELD == 0 {
            return false;
        }
        let ev = Event::new(0);
        for e in self.filter_effect(card, code::CANNOT_BE_EFFECT_TARGET) {
            let Some(eff) = self.effects.get(e) else {
                continue;
            };
            let ctx = Ctx {
                reason_effect: peffect,
                player: playerid,
                event: &ev,
                card: None,
                args: &[i64::from(playerid)],
            };
            if eff.get_value(self, &ctx) != 0 {
                return false;
            }
        }
        let Some(handler) = self.effects.get(peffect).and_then(|e| e.handler) else {
            return true;
        };
        for e in self.filter_effect(handler, code::CANNOT_SELECT_EFFECT_TARGET) {
            let Some(eff) = self.effects.get(e) else {
                continue;
            };
            let ctx = Ctx {
                reason_effect: peffect,
                player: playerid,
                event: &ev,
                card: Some(card),
                args: &[],
            };
            if eff.get_value(self, &ctx) != 0 {
                return false;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{card_type, Card, CardData};
    use crate::script_api as api;

    fn card(f: &mut Field, owner: u8, code: u32, type_: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code,
                type_,
                ..Default::default()
            },
            owner,
        );
        c.current.controller = owner;
        f.new_card(c)
    }

    fn monster(f: &mut Field, owner: u8, seat: u32) -> CardId {
        let id = card(f, owner, 1000 + seat, card_type::MONSTER);
        f.add_card(owner, id, location::MZONE, seat, false);
        id
    }

    fn spell(f: &mut Field, owner: u8, seat: u32) -> CardId {
        let id = card(f, owner, 2000 + seat, card_type::SPELL);
        f.add_card(owner, id, location::SZONE, seat, false);
        id
    }

    fn pile(f: &mut Field, owner: u8, loc: u8, code: u32) -> CardId {
        // `add_card` sends a monster that is not an extra-deck type back
        // to the main deck, so the extra-deck card is a Fusion.
        let type_ = if loc == location::EXTRA {
            card_type::MONSTER | card_type::FUSION
        } else {
            card_type::MONSTER
        };
        let id = card(f, owner, code, type_);
        let seq = match loc {
            location::DECK => f.players[usize::from(owner)].main.len(),
            location::HAND => f.players[usize::from(owner)].hand.len(),
            location::GRAVE => f.players[usize::from(owner)].grave.len(),
            location::REMOVED => f.players[usize::from(owner)].removed.len(),
            location::EXTRA => f.players[usize::from(owner)].extra.len(),
            _ => unreachable!(),
        };
        f.add_card(owner, id, loc, seq as u32, false);
        id
    }

    const MZ: u32 = location::MZONE as u32;
    const SZ: u32 = location::SZONE as u32;

    /// Every match, in scan order — `filter_matching_card` with a group
    /// and no early stop.
    fn scan(f: &mut Field, self_: u8, l1: u32, l2: u32) -> Vec<CardId> {
        let mut g = Vec::new();
        let stopped = f.filter_matching_card(
            None,
            self_,
            l1,
            l2,
            Some(&mut g),
            None,
            None,
            None,
            0,
            false,
        );
        assert!(!stopped, "a full scan reports no early stop");
        g
    }

    /// **The two masks are the asking player's and the opponent's.** The
    /// same call from the other player swaps the sides.
    #[test]
    fn the_first_mask_is_the_askers_side_and_the_second_the_opponents() {
        let mut f = Field::new(8000);
        let mine = monster(&mut f, 0, 0);
        let theirs = monster(&mut f, 1, 2);
        assert_eq!(scan(&mut f, 0, MZ, 0), vec![mine]);
        assert_eq!(scan(&mut f, 0, 0, MZ), vec![theirs]);
        assert_eq!(scan(&mut f, 0, MZ, MZ), vec![mine, theirs]);
        assert_eq!(
            scan(&mut f, 1, MZ, 0),
            vec![theirs],
            "asked from the other seat"
        );
        assert_eq!(
            scan(&mut f, 1, MZ, MZ),
            vec![theirs, mine],
            "the asker's side first"
        );
    }

    /// **The Monster Zone skips a card with any of the summoning marks**
    /// (`get_status`, any-bit — one of the three bits is enough), while
    /// the same mark on a card in a pile is not looked at.
    #[test]
    fn a_monster_mid_summon_is_skipped_but_only_in_the_monster_zone() {
        let mut f = Field::new(8000);
        let a = monster(&mut f, 0, 0);
        let b = monster(&mut f, 0, 1);
        let c = monster(&mut f, 0, 2);
        let g = pile(&mut f, 0, location::GRAVE, 77);
        f.cards[a].status |= status::SUMMONING;
        f.cards[b].status |= status::SUMMON_DISABLED;
        f.cards[c].status |= status::SPSUMMON_STEP;
        f.cards[g].status |= status::SUMMONING;
        assert_eq!(
            scan(&mut f, 0, MZ, 0),
            Vec::<CardId>::new(),
            "one bit each is enough"
        );
        assert_eq!(
            scan(&mut f, 0, location::GRAVE as u32, 0),
            vec![g],
            "the pile has no extra test"
        );
        f.cards[a].status = 0;
        assert_eq!(
            scan(&mut f, 0, MZ, 0),
            vec![a],
            "and clearing it lets the card through"
        );
    }

    /// **The Spell row skips an activation-disabled card**, whichever of
    /// the row masks asked.
    #[test]
    fn an_activate_disabled_spell_is_skipped_in_the_row() {
        let mut f = Field::new(8000);
        let ok = spell(&mut f, 0, 0);
        let dead = spell(&mut f, 0, 1);
        f.cards[dead].status |= status::ACTIVATE_DISABLED;
        assert_eq!(scan(&mut f, 0, SZ, 0), vec![ok]);
        assert_eq!(scan(&mut f, 0, location::STZONE as u32, 0), vec![ok]);
        f.cards[dead].status = 0;
        assert_eq!(scan(&mut f, 0, SZ, 0), vec![ok, dead]);
    }

    /// **`LOCATION_SZONE` is the whole row; the sub-masks split it.**
    /// The field spell seat is 5, the pendulum seats are 0 and 4 by
    /// default and want the card to be a pendulum in its zone.
    #[test]
    fn the_row_sub_masks_split_the_eight_seats() {
        let mut f = Field::new(8000);
        let s0 = spell(&mut f, 0, 0);
        let s4 = spell(&mut f, 0, 4);
        let fs = spell(&mut f, 0, 5);
        assert_eq!(scan(&mut f, 0, SZ, 0), vec![s0, s4, fs]);
        assert_eq!(scan(&mut f, 0, location::STZONE as u32, 0), vec![s0, s4]);
        assert_eq!(scan(&mut f, 0, location::FZONE as u32, 0), vec![fs]);
        assert_eq!(
            scan(&mut f, 0, location::PZONE as u32, 0),
            Vec::<CardId>::new(),
            "a Spell in the pendulum seat is not a pendulum"
        );
        f.cards[s0].current.pzone = true;
        f.cards[s4].current.pzone = true;
        assert_eq!(scan(&mut f, 0, location::PZONE as u32, 0), vec![s0, s4]);
        f.cards[s4].status |= status::ACTIVATE_DISABLED;
        assert_eq!(
            scan(&mut f, 0, location::PZONE as u32, 0),
            vec![s0],
            "pendulum seats also skip a disabled card"
        );
        f.cards[s4].status = 0;
        assert_eq!(
            scan(
                &mut f,
                0,
                (location::STZONE | location::FZONE | location::PZONE) as u32,
                0
            ),
            vec![s0, s4, fs, s0, s4],
            "the sub-masks are scanned one after another, so a seat can match twice"
        );
    }

    /// **`MMZONE` is seats 0–4 and `EMZONE` seats 5–6**; on a
    /// three-column field the extra range is empty and the row shrinks to
    /// seats 1–3, exactly as the reference's iterator arithmetic has it.
    #[test]
    fn the_monster_sub_masks_and_the_three_column_shrink() {
        let mut f = Field::new(8000);
        let m4 = monster(&mut f, 0, 4);
        let m5 = monster(&mut f, 0, 5);
        let s0 = spell(&mut f, 0, 0);
        let s1 = spell(&mut f, 0, 1);
        assert_eq!(scan(&mut f, 0, location::MMZONE as u32, 0), vec![m4]);
        assert_eq!(scan(&mut f, 0, location::EMZONE as u32, 0), vec![m5]);
        assert_eq!(
            scan(&mut f, 0, MZ, 0),
            vec![m4, m5],
            "the full mask ignores the split"
        );
        assert_eq!(scan(&mut f, 0, location::STZONE as u32, 0), vec![s0, s1]);
        let mut three = Field::with_flags(8000, flags::THREE_COLUMNS_FIELD);
        let m4 = monster(&mut three, 0, 4);
        let m5 = monster(&mut three, 0, 5);
        let _s0 = spell(&mut three, 0, 0);
        let s1 = spell(&mut three, 0, 1);
        assert_eq!(scan(&mut three, 0, location::MMZONE as u32, 0), vec![m4]);
        assert_eq!(
            scan(&mut three, 0, location::EMZONE as u32, 0),
            Vec::<CardId>::new(),
            "[6, 6) is empty"
        );
        assert_eq!(scan(&mut three, 0, MZ, 0), vec![m4, m5]);
        assert_eq!(
            scan(&mut three, 0, location::STZONE as u32, 0),
            vec![s1],
            "seat 0 is outside [1, 4)"
        );
    }

    /// **The pendulum seat table**: 0 and 4 by default, 1 and 3 on a
    /// three-column field, 6 and 7 with separate pendulum zones — and
    /// separate zones win over three columns. Any other scale is seat 0.
    #[test]
    fn the_pendulum_seat_table() {
        let plain = Field::new(8000);
        assert_eq!(
            (plain.get_pzone_index(0, 0), plain.get_pzone_index(1, 1)),
            (0, 4)
        );
        assert_eq!(plain.get_pzone_index(2, 0), 0, "not a scale");
        let three = Field::with_flags(8000, flags::THREE_COLUMNS_FIELD);
        assert_eq!(
            (three.get_pzone_index(0, 0), three.get_pzone_index(1, 0)),
            (1, 3)
        );
        let sep = Field::with_flags(8000, flags::SEPARATE_PZONE | flags::THREE_COLUMNS_FIELD);
        assert_eq!(
            (sep.get_pzone_index(0, 0), sep.get_pzone_index(1, 0)),
            (6, 7)
        );
        assert!(sep.has_separate_pzone(0) && sep.has_separate_pzone(1));
        assert!(!plain.has_separate_pzone(0));
    }

    /// **The exception is a card or a group**, never both matched.
    #[test]
    fn the_exception_card_or_group_is_left_out() {
        let mut f = Field::new(8000);
        let a = monster(&mut f, 0, 0);
        let b = monster(&mut f, 0, 1);
        let c = monster(&mut f, 0, 2);
        let mut g = Vec::new();
        f.filter_matching_card(None, 0, MZ, 0, Some(&mut g), Some(b), None, None, 0, false);
        assert_eq!(g, vec![a, c]);
        let mut g = Vec::new();
        f.filter_matching_card(
            None,
            0,
            MZ,
            0,
            Some(&mut g),
            None,
            Some(&[a, c]),
            None,
            0,
            false,
        );
        assert_eq!(g, vec![b]);
    }

    /// **`fcount` stops the scan at that many matches and reports it**;
    /// the group only receives the matches before the stop.
    #[test]
    fn the_count_stops_the_scan_early() {
        let mut f = Field::new(8000);
        let a = monster(&mut f, 0, 0);
        let _b = monster(&mut f, 0, 1);
        let _c = monster(&mut f, 0, 2);
        let mut g = Vec::new();
        assert!(f.filter_matching_card(None, 0, MZ, 0, Some(&mut g), None, None, None, 2, false));
        assert_eq!(g, vec![a], "the stopping match itself is not pushed");
        assert!(
            !f.filter_matching_card(None, 0, MZ, 0, None, None, None, None, 4, false),
            "not enough"
        );
        assert!(
            f.filter_matching_card(None, 0, MZ, 0, None, None, None, None, 3, false),
            "exactly enough"
        );
    }

    /// **`first` mode returns the first match and stops**, filling no
    /// group.
    #[test]
    fn the_first_match_mode_stops_at_one() {
        let mut f = Field::new(8000);
        let a = monster(&mut f, 0, 3);
        let _b = monster(&mut f, 0, 5);
        let mut first = None;
        let mut g = Vec::new();
        assert!(f.filter_matching_card(
            None,
            0,
            MZ,
            0,
            Some(&mut g),
            None,
            None,
            Some(&mut first),
            0,
            false
        ));
        assert_eq!(first, Some(a));
        assert!(g.is_empty(), "the group is not filled in first mode");
        let mut first = None;
        assert!(!f.filter_matching_card(
            None,
            0,
            0,
            MZ,
            None,
            None,
            None,
            Some(&mut first),
            0,
            false
        ));
        assert_eq!(first, None, "no match leaves it untouched");
    }

    /// **The filter is asked about each card**, and `None` passes all.
    #[test]
    fn the_filter_is_applied_per_card() {
        let mut f = Field::new(8000);
        let _a = monster(&mut f, 0, 0);
        let b = monster(&mut f, 0, 1);
        let want = f.cards[b].data.code;
        let by_code = move |f: &mut Field, c: CardId| f.cards[c].data.code == want;
        let mut g = Vec::new();
        f.filter_matching_card(
            Some(&by_code),
            0,
            MZ,
            0,
            Some(&mut g),
            None,
            None,
            None,
            0,
            false,
        );
        assert_eq!(g, vec![b]);
        assert_eq!(scan(&mut f, 0, MZ, 0).len(), 2);
    }

    /// **The piles are scanned deck, extra, hand, grave, banished** — in
    /// that order, after the zones — so the group's order is the scan's,
    /// not the cards' ages.
    #[test]
    fn the_piles_are_scanned_in_the_references_order() {
        let mut f = Field::new(8000);
        let r = pile(&mut f, 0, location::REMOVED, 1);
        let g = pile(&mut f, 0, location::GRAVE, 2);
        let h = pile(&mut f, 0, location::HAND, 3);
        let x = pile(&mut f, 0, location::EXTRA, 4);
        let d = pile(&mut f, 0, location::DECK, 5);
        let m = monster(&mut f, 0, 0);
        let all = (location::DECK
            | location::EXTRA
            | location::HAND
            | location::GRAVE
            | location::REMOVED
            | location::MZONE) as u32;
        assert_eq!(scan(&mut f, 0, all, 0), vec![m, d, x, h, g, r]);
        assert_eq!(scan(&mut f, 0, location::HAND as u32, 0), vec![h]);
        assert_eq!(scan(&mut f, 0, location::EXTRA as u32, 0), vec![x]);
    }

    /// **A pile is scanned to its end**, not just its first card.
    #[test]
    fn a_pile_is_scanned_to_its_end() {
        let mut f = Field::new(8000);
        let h1 = pile(&mut f, 0, location::HAND, 11);
        let h2 = pile(&mut f, 0, location::HAND, 12);
        let g1 = pile(&mut f, 0, location::GRAVE, 13);
        let g2 = pile(&mut f, 0, location::GRAVE, 14);
        let g3 = pile(&mut f, 0, location::GRAVE, 15);
        assert_eq!(
            scan(&mut f, 0, (location::HAND | location::GRAVE) as u32, 0),
            vec![h1, h2, g1, g2, g3]
        );
        assert!(
            f.filter_matching_card(
                None,
                0,
                location::GRAVE as u32,
                0,
                None,
                None,
                None,
                None,
                3,
                false
            ),
            "three in the graveyard"
        );
    }

    /// **The asking player must be a player.**
    #[test]
    fn a_non_player_finds_nothing() {
        let mut f = Field::new(8000);
        let _a = monster(&mut f, 0, 0);
        let mut g = Vec::new();
        assert!(!f.filter_matching_card(None, 2, MZ, MZ, Some(&mut g), None, None, None, 0, false));
        assert!(g.is_empty());
    }

    mod is_capable_be_effect_target {
        use super::*;
        use crate::effect::effect_type;
        use crate::event::code;

        /// A registered effect on `owner`'s card to be the reason effect.
        fn reason(f: &mut Field, owner: u8) -> EffectId {
            let h = card(f, owner, 999, card_type::SPELL);
            f.add_card(owner, h, location::HAND, 0, false);
            let e = api::create_effect(f, h);
            api::set_type(f, e, effect_type::SINGLE);
            api::set_code(f, e, code::CANNOT_SELECT_EFFECT_TARGET);
            // Registering is what sets the handler.
            api::register_effect(f, h, e, false);
            e
        }

        /// **Location and summoning state come first**: the hand, deck
        /// and extra deck are never targetable, nor a card mid-summon or
        /// battle-destroyed.
        #[test]
        fn location_and_summoning_state() {
            let mut f = Field::new(8000);
            let re = reason(&mut f, 1);
            let m = monster(&mut f, 0, 0);
            assert!(f.is_capable_be_effect_target(m, re, 1));
            for loc in [location::HAND, location::DECK, location::EXTRA] {
                let c = pile(&mut f, 0, loc, 50);
                assert!(!f.is_capable_be_effect_target(c, re, 1), "loc {loc:#x}");
            }
            let g = pile(&mut f, 0, location::GRAVE, 51);
            assert!(
                f.is_capable_be_effect_target(g, re, 1),
                "the graveyard is fine"
            );
            f.cards[m].status |= status::SUMMONING;
            assert!(!f.is_capable_be_effect_target(m, re, 1));
            f.cards[m].status = status::BATTLE_DESTROYED;
            assert!(!f.is_capable_be_effect_target(m, re, 1));
            f.cards[m].status = 0;
            assert!(f.is_capable_be_effect_target(m, re, 1));
        }

        /// **A token is only targetable on the field.**
        #[test]
        fn a_token_off_the_field() {
            let mut f = Field::new(8000);
            let re = reason(&mut f, 1);
            let t = pile(&mut f, 0, location::GRAVE, 60);
            f.cards[t].data.type_ |= card_type::TOKEN;
            assert!(!f.is_capable_be_effect_target(t, re, 1));
            let on = monster(&mut f, 0, 0);
            f.cards[on].data.type_ |= card_type::TOKEN;
            assert!(f.is_capable_be_effect_target(on, re, 1));
        }

        /// **`CANNOT_BE_EFFECT_TARGET` on the card refuses when its value
        /// says so**, asked with the player as the pushed argument; a
        /// value of zero does not refuse.
        #[test]
        fn cannot_be_effect_target_on_the_card() {
            let mut f = Field::new(8000);
            let re = reason(&mut f, 1);
            let m = monster(&mut f, 0, 0);
            let e = api::create_effect(&mut f, m);
            api::set_type(&mut f, e, effect_type::SINGLE);
            api::set_code(&mut f, e, code::CANNOT_BE_EFFECT_TARGET);
            api::set_value_fn(&mut f, e, |_e, _f, ctx| i64::from(ctx.args == [1]));
            api::register_effect(&mut f, m, e, false);
            assert!(
                !f.is_capable_be_effect_target(m, re, 1),
                "refused for player 1"
            );
            assert!(
                f.is_capable_be_effect_target(m, re, 0),
                "the value fn saw player 0 and said no"
            );
        }

        /// **`CANNOT_SELECT_EFFECT_TARGET` on the reason effect's handler
        /// refuses when its value says so**, asked with the card.
        #[test]
        fn cannot_select_effect_target_on_the_handler() {
            let mut f = Field::new(8000);
            let re = reason(&mut f, 1);
            let handler = f.effects.get(re).unwrap().handler.unwrap();
            let a = monster(&mut f, 0, 0);
            let b = monster(&mut f, 0, 1);
            let e = api::create_effect(&mut f, handler);
            api::set_type(&mut f, e, effect_type::SINGLE);
            api::set_code(&mut f, e, code::CANNOT_SELECT_EFFECT_TARGET);
            api::set_value_fn(&mut f, e, |_e, f, ctx| {
                i64::from(ctx.card.is_some_and(|c| f.cards[c].current.sequence == 0))
            });
            api::register_effect(&mut f, handler, e, false);
            assert!(!f.is_capable_be_effect_target(a, re, 1));
            assert!(f.is_capable_be_effect_target(b, re, 1));
        }

        /// **`is_target` mode routes through it**, reading the reason
        /// effect and player off `core`; with no reason effect nothing is
        /// targetable.
        #[test]
        fn the_target_mode_of_the_scan() {
            let mut f = Field::new(8000);
            let re = reason(&mut f, 1);
            let m = monster(&mut f, 0, 0);
            let summoning = monster(&mut f, 0, 1);
            f.cards[summoning].status |= status::SUMMONING;
            let h = pile(&mut f, 0, location::HAND, 70);
            f.core.reason_effect = Some(re);
            f.core.reason_player = 1;
            let hand_and_field = (location::HAND | location::MZONE) as u32;
            let mut g = Vec::new();
            f.filter_matching_card(
                None,
                0,
                hand_and_field,
                0,
                Some(&mut g),
                None,
                None,
                None,
                0,
                true,
            );
            assert_eq!(g, vec![m], "the hand card is not targetable");
            let mut g = Vec::new();
            f.filter_matching_card(
                None,
                0,
                hand_and_field,
                0,
                Some(&mut g),
                None,
                None,
                None,
                0,
                false,
            );
            assert_eq!(g, vec![m, h], "without the flag the hand card matches");
            f.core.reason_effect = None;
            let mut g = Vec::new();
            f.filter_matching_card(
                None,
                0,
                hand_and_field,
                0,
                Some(&mut g),
                None,
                None,
                None,
                0,
                true,
            );
            assert!(g.is_empty(), "no reason effect, nothing targetable");
        }
    }
}

#[cfg(test)]
mod filter_field_card_tests {
    use super::*;
    use crate::card::{card_type, Card, CardData};

    fn card_at(f: &mut Field, owner: u8, loc: u8, seq: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 300 + seq,
                type_: card_type::MONSTER,
                ..Default::default()
            },
            owner,
        );
        c.current.controller = owner;
        let id = f.new_card(c);
        f.add_card(owner, id, loc, seq, false);
        id
    }

    fn scan(f: &Field, self_: u8, l1: u32, l2: u32) -> Vec<CardId> {
        let mut g = Vec::new();
        let n = f.filter_field_card(self_, l1, l2, Some(&mut g));
        assert_eq!(n, g.len(), "the count and the group agree");
        g
    }

    const MZ: u32 = location::MZONE as u32;

    /// **Both masks, as in the matching scan**: the asker's side and the
    /// opponent's.
    #[test]
    fn the_two_masks_are_the_two_sides() {
        let mut f = Field::new(8000);
        let mine = card_at(&mut f, 0, location::MZONE, 0);
        let theirs = card_at(&mut f, 1, location::MZONE, 0);
        assert_eq!(scan(&f, 0, MZ, 0), vec![mine]);
        assert_eq!(scan(&f, 0, 0, MZ), vec![theirs]);
        assert_eq!(scan(&f, 0, MZ, MZ), vec![mine, theirs]);
    }

    /// **The whole Monster Zone skips two marks, not three.** The
    /// matching scan also skips `SUMMON_DISABLED`; this one does **not**,
    /// and the difference is the reference's.
    #[test]
    fn the_monster_zone_skips_summoning_but_not_summon_disabled() {
        let mut f = Field::new(8000);
        let a = card_at(&mut f, 0, location::MZONE, 0);
        let b = card_at(&mut f, 0, location::MZONE, 1);
        let c = card_at(&mut f, 0, location::MZONE, 2);
        f.cards[a].status |= status::SUMMONING;
        f.cards[b].status |= status::SPSUMMON_STEP;
        f.cards[c].status |= status::SUMMON_DISABLED;
        assert_eq!(
            scan(&f, 0, MZ, 0),
            vec![c],
            "a summon-disabled card is still a card on the field here"
        );
        // Where the matching scan drops all three.
        let mut g = Vec::new();
        let mut f2 = f;
        f2.filter_matching_card(None, 0, MZ, 0, Some(&mut g), None, None, None, 0, false);
        assert!(g.is_empty(), "and the matching scan drops it");
    }

    /// **The sub-masks skip nothing at all**, where the full mask skips
    /// two — so a card mid-summon is invisible to `LOCATION_MZONE` and
    /// visible to `LOCATION_MMZONE`.
    #[test]
    fn the_sub_masks_skip_nothing() {
        let mut f = Field::new(8000);
        let a = card_at(&mut f, 0, location::MZONE, 0);
        f.cards[a].status |= status::SUMMONING;
        assert!(scan(&f, 0, MZ, 0).is_empty(), "the full mask skips it");
        assert_eq!(
            scan(&f, 0, location::MMZONE as u32, 0),
            vec![a],
            "the sub-mask does not"
        );
    }

    /// **`EMZONE` does not shrink on a three-column field**, though the
    /// Spell row does — and though the matching scan shrinks both.
    #[test]
    fn the_extra_zone_does_not_shrink() {
        let mut three = Field::with_flags(8000, flags::THREE_COLUMNS_FIELD);
        let extra = card_at(&mut three, 0, location::MZONE, 5);
        let row0 = card_at(&mut three, 0, location::SZONE, 0);
        let row1 = card_at(&mut three, 0, location::SZONE, 1);
        assert_eq!(
            scan(&three, 0, location::EMZONE as u32, 0),
            vec![extra],
            "still [5, 7) here, where the matching scan empties it"
        );
        assert_eq!(
            scan(&three, 0, location::STZONE as u32, 0),
            vec![row1],
            "but the Spell row shrinks, so seat 0 is out"
        );
        let _ = row0;
    }

    /// **The Spell row keeps an activation-disabled card**, where the
    /// matching scan drops it.
    #[test]
    fn the_spell_row_keeps_an_activate_disabled_card() {
        let mut f = Field::new(8000);
        let s = card_at(&mut f, 0, location::SZONE, 0);
        f.cards[s].status |= status::ACTIVATE_DISABLED;
        assert_eq!(scan(&f, 0, location::SZONE as u32, 0), vec![s]);
    }

    /// **The piles are taken whole**, and a count with no group is the
    /// same number.
    #[test]
    fn the_piles_are_taken_whole() {
        let mut f = Field::new(8000);
        let h: Vec<_> = (0..3)
            .map(|i| card_at(&mut f, 0, location::HAND, i))
            .collect();
        let d: Vec<_> = (0..2)
            .map(|i| card_at(&mut f, 0, location::DECK, i))
            .collect();
        let hand_and_deck = (location::HAND | location::DECK) as u32;
        let mut want = h.clone();
        want.extend(&d);
        want.sort_unstable();
        assert_eq!(scan(&f, 0, hand_and_deck, 0), want);
        assert_eq!(
            f.filter_field_card(0, hand_and_deck, 0, None),
            5,
            "counting without collecting gives the same answer"
        );
    }

    /// **A player who is not a player finds nothing.**
    #[test]
    fn a_non_player_finds_nothing() {
        let mut f = Field::new(8000);
        card_at(&mut f, 0, location::MZONE, 0);
        assert_eq!(f.filter_field_card(2, MZ, MZ, None), 0);
    }
}

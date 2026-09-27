//! What a card *was*: the statistics half of `card_state`.
//!
//! When a card leaves the field, a great many rules ask what it had been —
//! a destroyed monster's level, a banished card's name, the type of
//! something that is now in the graveyard. The reference answers those from
//! `previous`, which it snapshots on the way out.
//!
//! This module is that snapshot, plus the ordering rule that decides which
//! card in a batch is processed first.
//!
//! ## The statistics belong to `previous` alone
//!
//! ocgcore declares **one** `card_state` and gives each card two of them,
//! `current` and `previous`, so on paper both carry the statistics. In
//! practice only one does. Across the whole of ocgcore:
//!
//! - `current.code`, `current.type`, `current.level`, `current.attack` — and
//!   every other statistic — are **never read**, and
//! - they are **never written** either.
//!
//! The statistics half of `current` is dead weight in the declaration. So
//! this port puts them where they are actually used: `Card::previous_stats`,
//! leaving [`crate::board::Loc`] purely positional. Two reasons, and the
//! second is the one that would have bitten:
//!
//! **Cost.** `Card` is cloned at the rate a solver searches, and carrying
//! fifteen dead fields on every card — including a `BTreeSet` — is a real
//! per-clone cost for data nothing reads.
//!
//! **Safety.** This port assigns `previous = current` **wholesale** in four
//! places, where the reference assigns the five positional fields one by
//! one. Those are equivalent only while `Loc` is positional. Put the
//! statistics inside it and the wholesale copy silently overwrites the
//! snapshot that was just taken — `SendTo` records what the card was at its
//! step 2, then calls `move_card` at step 6, which would wipe it. The card
//! would report its printed statistics instead of what it actually was, and
//! nothing would fail visibly. Keeping the statistics out of `Loc` makes
//! that mistake unrepresentable rather than merely avoided.

use crate::board::location;
use crate::event::code;
use crate::event::CardId;
use crate::field::Field;
use std::collections::BTreeSet;

/// The statistics a card had when it last left where it was.
///
/// The reference's `card_state` minus its positional fields — see the module
/// note for why the two halves are split here and not there. Field names and
/// widths are the reference's.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CardStats {
    pub code: u32,
    /// The *second* name of a double-named card, or one granted by
    /// `EFFECT_ADD_CODE`. Zero when the card has only one name.
    pub code2: u32,
    pub setcodes: BTreeSet<u16>,
    pub type_: u32,
    pub level: u32,
    pub rank: u32,
    pub attribute: u32,
    pub race: u64,
    pub attack: i32,
    pub defense: i32,
}

impl Field {
    /// Record what this card is, before it stops being it.
    ///
    /// `SendTo` step 2 runs this over every card that is leaving the field.
    /// The reference writes it inline; it is a function here because it is
    /// one idea, it is the only writer of these fields, and `Destroy` and
    /// `Release` reach the same code through `SendTo`.
    ///
    /// ## Face-up and face-down are answered from different sources
    ///
    /// A face-up card is snapshotted from its **resolved** values — what it
    /// counted as, with every continuous effect applied. A face-down one is
    /// snapshotted from its **printed** data, because nothing on the board
    /// was modifying a card nobody could see.
    ///
    /// That split is the whole subtlety, and it is why a set monster
    /// destroyed while an effect was pumping the field reports its printed
    /// attack rather than the pumped one.
    ///
    /// ## Three narrower traps inside the face-down branch
    ///
    /// - **`alias` stands in for `code`, but only when no `EFFECT_ADD_CODE`
    ///   applies.** An alias is a reprint pointing at the original; a card
    ///   granted an extra name is not one, and the reference checks the
    ///   effect set is empty before using it.
    /// - **`rank` is set from `data.level`.** The printed data has no
    ///   separate rank field, and the reference assigns the level to both.
    /// - **`code2` comes from the *last* `EFFECT_ADD_CODE`**, not the first
    ///   and not a combination.
    ///
    /// ## The level/rank group is monster-zone only
    ///
    /// In the face-up branch the six monster statistics are snapshotted only
    /// for a card in the Monster Zone. A face-up Spell leaving the field
    /// records its name and type and nothing else — asking a resolved level
    /// of it would be asking a question it has no answer to.
    ///
    /// `setcodes` is taken last, from `EFFECT_ADD_SETCODE`, for **both**
    /// branches — and it is cleared first, so a card that had setcodes
    /// granted and then lost them records none rather than keeping stale
    /// ones.
    pub fn snapshot_previous_stats(&mut self, card: CardId) {
        if !self.cards[card]
            .current
            .is_location(u16::from(location::ONFIELD))
        {
            return;
        }
        let controller = self.cards[card].current.controller;
        let mut stats = CardStats::default();

        if self.cards[card].current.is_faceup() {
            stats.code = self.get_code(card);
            stats.code2 = self.get_another_code(card);
            stats.type_ = self.get_type(card, None, 0, controller);
            if self.cards[card].current.location & location::MZONE != 0 {
                stats.level = self.get_level(card);
                stats.rank = self.get_rank(card);
                stats.attribute = self.get_attribute(card, None, 0, controller);
                stats.race = self.get_race(card, None, 0, controller);
                stats.attack = self.get_attack(card);
                stats.defense = self.get_defense(card);
            }
        } else {
            let added = self.filter_effect(card, code::ADD_CODE);
            let data = self.cards[card].data.clone();
            stats.code = if data.alias != 0 && added.is_empty() {
                data.alias
            } else {
                data.code
            };
            // The *last* one, as the reference takes it.
            stats.code2 = match added.last() {
                Some(&id) => self.effect_value_for_card_pub(id, card) as u32,
                None => 0,
            };
            stats.type_ = data.type_;
            stats.level = data.level;
            // No separate printed rank: the reference assigns the level here.
            stats.rank = data.level;
            stats.attribute = data.attribute;
            stats.race = data.race;
            stats.attack = data.attack;
            stats.defense = data.defense;
        }

        for id in self.filter_effect(card, code::ADD_SETCODE) {
            stats
                .setcodes
                .insert(self.effect_value_for_card_pub(id, card) as u16);
        }
        self.cards[card].previous_stats = stats;
    }

    /// `card::card_operation_sort` — which card in a batch is handled first.
    ///
    /// Returns true when `a` sorts before `b`. This is not cosmetic: it
    /// decides the order cards are moved, and therefore the order their
    /// events are raised and the sequence each lands at.
    ///
    /// Four rules, in order:
    ///
    /// 1. **Different controllers sort by the turn player.** A card owned by
    ///    nobody sorts by raw comparison; otherwise the turn player's cards
    ///    come first, which is expressed as "ascending when the turn player
    ///    is 0, descending when it is 1". An Xyz material is judged by the
    ///    controller of the monster it is *under*, not its own.
    /// 2. Then by location, ascending.
    /// 3. Within the overlay location, by the monster's sequence, and then by
    ///    the material's own.
    /// 4. Otherwise by sequence — **descending** for the four pile locations
    ///    (deck, extra, graveyard, banished) and ascending for the rest.
    ///
    /// Rule 4's inversion is the one to get right. A pile's top is its
    /// highest sequence, so taking several cards from a graveyard processes
    /// the topmost first; the field's zones read the other way.
    pub fn card_operation_sort(&self, a: CardId, b: CardId) -> bool {
        let ctrl = |c: CardId| match self.cards[c].overlay_target {
            Some(t) => self.cards[t].current.controller,
            None => self.cards[c].current.controller,
        };
        let (ca, cb) = (ctrl(a), ctrl(b));
        if ca != cb {
            if ca == crate::event::PLAYER_NONE || cb == crate::event::PLAYER_NONE {
                return ca < cb;
            }
            return if self.infos.turn_player == 0 {
                ca < cb
            } else {
                ca > cb
            };
        }
        let (la, lb) = (
            self.cards[a].current.location,
            self.cards[b].current.location,
        );
        if la != lb {
            return la < lb;
        }
        if la & location::OVERLAY != 0 {
            let seq = |c: CardId| {
                self.cards[c]
                    .overlay_target
                    .map_or(0, |t| self.cards[t].current.sequence)
            };
            let (sa, sb) = (seq(a), seq(b));
            if sa != sb {
                return sa < sb;
            }
            return self.cards[a].current.sequence < self.cards[b].current.sequence;
        }
        let piles = location::DECK | location::EXTRA | location::GRAVE | location::REMOVED;
        if la & piles != 0 {
            self.cards[a].current.sequence > self.cards[b].current.sequence
        } else {
            self.cards[a].current.sequence < self.cards[b].current.sequence
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::position;
    use crate::card::{card_type, status, Card, CardData};
    use crate::effect::{effect_type, Effect};
    use crate::event::EffectId;

    fn card_at(f: &mut Field, loc: u8, pos: u8, data: CardData) -> CardId {
        let mut c = Card::with_data(data, 0);
        c.current.controller = 0;
        c.current.position = pos;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        let seat = f.cards.len() as u32 - 1;
        f.add_card(0, id, loc, seat, false);
        id
    }

    fn monster_data() -> CardData {
        CardData {
            code: 18036057,
            type_: card_type::MONSTER | card_type::EFFECT,
            level: 5,
            attribute: 0x10,
            race: 0x2,
            attack: 1900,
            defense: 1400,
            ..Default::default()
        }
    }

    /// An effect on the card whose `value` is a constant.
    fn valued(f: &mut Field, card: CardId, code_: u32, value: i64) -> EffectId {
        let mut e = Effect::new(effect_type::SINGLE, code_);
        e.owner = Some(card);
        e.handler = Some(card);
        e.value = value;
        let id = f.new_effect(e);
        f.cards[card].single_effect.insert(code_, id);
        f.cards[card].indexer.insert(id);
        id
    }

    mod snapshot_previous_stats {
        use super::*;

        #[test]
        fn a_card_not_on_the_field_is_not_snapshotted() {
            let mut f = Field::new(8000);
            let c = card_at(
                &mut f,
                location::GRAVE,
                position::FACEUP_ATTACK,
                monster_data(),
            );
            f.snapshot_previous_stats(c);
            assert_eq!(
                f.cards[c].previous_stats,
                CardStats::default(),
                "only cards leaving the field record what they were"
            );
        }

        #[test]
        fn a_faceup_monster_records_its_resolved_statistics() {
            let mut f = Field::new(8000);
            let c = card_at(
                &mut f,
                location::MZONE,
                position::FACEUP_ATTACK,
                monster_data(),
            );
            // A continuous pump, which the resolved read must include.
            valued(&mut f, c, code::UPDATE_ATTACK, 500);

            f.snapshot_previous_stats(c);
            let s = &f.cards[c].previous_stats;
            assert_eq!(s.code, 18036057);
            assert_eq!(s.level, 5);
            assert_eq!(
                s.attack, 2400,
                "resolved, not printed: 1900 + 500 from the pump"
            );
        }

        /// The core split: a set card is read from its *printed* data, so a
        /// pump that was applying to the board does not appear.
        #[test]
        fn a_facedown_monster_records_its_printed_statistics() {
            let mut f = Field::new(8000);
            let c = card_at(
                &mut f,
                location::MZONE,
                position::FACEDOWN_DEFENSE,
                monster_data(),
            );
            valued(&mut f, c, code::UPDATE_ATTACK, 500);

            f.snapshot_previous_stats(c);
            assert_eq!(
                f.cards[c].previous_stats.attack, 1900,
                "printed, not resolved — nothing was modifying a card nobody could see"
            );
        }

        /// A printed card has no separate rank; the reference assigns the
        /// level to both fields.
        #[test]
        fn a_facedown_card_takes_its_rank_from_its_level() {
            let mut f = Field::new(8000);
            let c = card_at(
                &mut f,
                location::MZONE,
                position::FACEDOWN_DEFENSE,
                monster_data(),
            );
            f.snapshot_previous_stats(c);
            let s = &f.cards[c].previous_stats;
            assert_eq!((s.level, s.rank), (5, 5));
        }

        /// An alias stands in for the code — but only with no `ADD_CODE`.
        #[test]
        fn an_alias_names_a_facedown_card() {
            let mut f = Field::new(8000);
            let mut d = monster_data();
            d.alias = 11111111;
            let c = card_at(&mut f, location::MZONE, position::FACEDOWN_DEFENSE, d);
            f.snapshot_previous_stats(c);
            assert_eq!(f.cards[c].previous_stats.code, 11111111);
        }

        #[test]
        fn an_added_code_beats_the_alias() {
            let mut f = Field::new(8000);
            let mut d = monster_data();
            d.alias = 11111111;
            let c = card_at(&mut f, location::MZONE, position::FACEDOWN_DEFENSE, d);
            valued(&mut f, c, code::ADD_CODE, 22222222);

            f.snapshot_previous_stats(c);
            let s = &f.cards[c].previous_stats;
            assert_eq!(
                s.code, 18036057,
                "a card granted a name is not a reprint, so the alias is not used"
            );
            assert_eq!(s.code2, 22222222, "the granted name lands in code2");
        }

        /// `code2` is the *last* `ADD_CODE`, not the first.
        #[test]
        fn the_last_added_code_wins() {
            let mut f = Field::new(8000);
            let c = card_at(
                &mut f,
                location::MZONE,
                position::FACEDOWN_DEFENSE,
                monster_data(),
            );
            valued(&mut f, c, code::ADD_CODE, 111);
            valued(&mut f, c, code::ADD_CODE, 222);

            f.snapshot_previous_stats(c);
            assert_eq!(f.cards[c].previous_stats.code2, 222);
        }

        /// The monster group is Monster-Zone-only: a Spell leaving the field
        /// records its name and type and nothing else.
        #[test]
        fn a_spell_records_no_monster_statistics() {
            let mut f = Field::new(8000);
            let c = card_at(
                &mut f,
                location::SZONE,
                position::FACEUP_ATTACK,
                CardData {
                    code: 55144522,
                    type_: card_type::SPELL,
                    ..Default::default()
                },
            );
            f.snapshot_previous_stats(c);
            let s = &f.cards[c].previous_stats;
            assert_eq!(s.code, 55144522);
            assert_eq!(s.type_, card_type::SPELL);
            assert_eq!(
                (s.level, s.attack, s.defense),
                (0, 0, 0),
                "asking a Spell for a resolved level is a question it cannot answer"
            );
        }

        /// Setcodes are taken for both branches, and cleared first, so stale
        /// ones cannot survive.
        #[test]
        fn setcodes_are_collected_and_never_stale() {
            let mut f = Field::new(8000);
            let c = card_at(
                &mut f,
                location::MZONE,
                position::FACEUP_ATTACK,
                monster_data(),
            );
            valued(&mut f, c, code::ADD_SETCODE, 0x54);
            valued(&mut f, c, code::ADD_SETCODE, 0x99);

            f.snapshot_previous_stats(c);
            assert_eq!(
                f.cards[c].previous_stats.setcodes,
                BTreeSet::from([0x54, 0x99])
            );

            // Lose them, snapshot again: none remain.
            let stale: Vec<EffectId> = f.cards[c]
                .single_effect
                .equal_range(code::ADD_SETCODE)
                .to_vec();
            for id in stale {
                f.cards[c].single_effect.remove(code::ADD_SETCODE, id);
            }
            f.snapshot_previous_stats(c);
            assert!(
                f.cards[c].previous_stats.setcodes.is_empty(),
                "cleared first, so a card that lost its setcodes records none"
            );
        }

        /// The reason the statistics are not inside `Loc`: this port copies
        /// `previous = current` wholesale, and that must not disturb them.
        #[test]
        fn a_wholesale_positional_copy_leaves_the_snapshot_alone() {
            let mut f = Field::new(8000);
            let c = card_at(
                &mut f,
                location::MZONE,
                position::FACEUP_ATTACK,
                monster_data(),
            );
            f.snapshot_previous_stats(c);
            assert_eq!(f.cards[c].previous_stats.attack, 1900);

            f.cards[c].previous = f.cards[c].current;
            assert_eq!(
                f.cards[c].previous_stats.attack, 1900,
                "positional copy, statistics untouched — the trap this layout removes"
            );
        }
    }

    mod card_operation_sort {
        use super::*;

        fn at(f: &mut Field, controller: u8, loc: u8, seq: u32) -> CardId {
            let mut c = Card::with_data(monster_data(), controller);
            c.current.controller = controller;
            c.current.location = loc;
            c.current.sequence = seq;
            c.current.position = position::FACEUP_ATTACK;
            f.new_card(c)
        }

        /// The turn player's cards go first, and which comparison expresses
        /// that flips with the turn player.
        #[test]
        fn the_turn_player_sorts_first_either_way() {
            let mut f = Field::new(8000);
            let p0 = at(&mut f, 0, location::MZONE, 0);
            let p1 = at(&mut f, 1, location::MZONE, 0);

            f.infos.turn_player = 0;
            assert!(f.card_operation_sort(p0, p1));
            assert!(!f.card_operation_sort(p1, p0));

            f.infos.turn_player = 1;
            assert!(
                f.card_operation_sort(p1, p0),
                "player 1's cards come first on player 1's turn"
            );
        }

        /// An Xyz material is judged by the controller of the monster it is
        /// under, not by its own.
        #[test]
        fn a_material_is_judged_by_the_monster_above_it() {
            let mut f = Field::new(8000);
            let xyz = at(&mut f, 1, location::MZONE, 0);
            let mat = at(&mut f, 0, location::OVERLAY, 0);
            f.cards[mat].overlay_target = Some(xyz);
            let other = at(&mut f, 0, location::OVERLAY, 0);

            f.infos.turn_player = 0;
            assert!(
                f.card_operation_sort(other, mat),
                "`other` is player 0's; `mat` counts as player 1's via its monster"
            );
        }

        #[test]
        fn location_orders_before_sequence() {
            let mut f = Field::new(8000);
            let deck = at(&mut f, 0, location::DECK, 0);
            let hand = at(&mut f, 0, location::HAND, 99);
            assert!(f.card_operation_sort(deck, hand), "DECK 0x01 < HAND 0x02");
        }

        /// The inversion that is easy to miss: the four pile locations sort
        /// by sequence *descending*, because a pile's top is its highest.
        #[test]
        fn piles_sort_by_sequence_descending() {
            let mut f = Field::new(8000);
            for loc in [
                location::DECK,
                location::EXTRA,
                location::GRAVE,
                location::REMOVED,
            ] {
                let lo = at(&mut f, 0, loc, 0);
                let hi = at(&mut f, 0, loc, 5);
                assert!(
                    f.card_operation_sort(hi, lo),
                    "in a pile the topmost (highest sequence) is handled first"
                );
            }
        }

        #[test]
        fn field_zones_sort_by_sequence_ascending() {
            let mut f = Field::new(8000);
            let lo = at(&mut f, 0, location::MZONE, 0);
            let hi = at(&mut f, 0, location::MZONE, 3);
            assert!(f.card_operation_sort(lo, hi), "zones read the other way");
        }

        /// Within the overlay location the monster's sequence decides, and
        /// only then the material's own.
        #[test]
        fn materials_sort_by_their_monster_then_themselves() {
            let mut f = Field::new(8000);
            let x0 = at(&mut f, 0, location::MZONE, 0);
            let x1 = at(&mut f, 0, location::MZONE, 1);
            let under_x1 = at(&mut f, 0, location::OVERLAY, 0);
            let under_x0 = at(&mut f, 0, location::OVERLAY, 9);
            f.cards[under_x1].overlay_target = Some(x1);
            f.cards[under_x0].overlay_target = Some(x0);
            assert!(
                f.card_operation_sort(under_x0, under_x1),
                "the monster's sequence wins even though the material's is higher"
            );

            let a = at(&mut f, 0, location::OVERLAY, 0);
            let b = at(&mut f, 0, location::OVERLAY, 1);
            f.cards[a].overlay_target = Some(x0);
            f.cards[b].overlay_target = Some(x0);
            assert!(f.card_operation_sort(a, b), "same monster: own sequence");
        }
    }
}

//! A card's current level, rank, attribute and race.
//!
//! The resolved counterparts of `data.level`, `data.attribute` and
//! `data.race`, in the same sense that `get_type` is the resolved
//! counterpart of `data.type_`. Each runs the effects that adjust it, under
//! the same recursion guard, and each is asked by a great many cards.
//!
//! ## Two shapes, not one
//!
//! `get_attribute` and `get_race` follow `get_type` exactly: ADD, REMOVE and
//! CHANGE, with a separate accumulator for summon-context effects and a
//! `changed` flag that is set **only** in that branch.
//!
//! `get_level` and `get_rank` are different. There is no summon context;
//! instead there are three kinds of adjustment, and they interact:
//!
//! | effect | does |
//! |---|---|
//! | `UPDATE_*` | adds, into one of *two* accumulators |
//! | `CHANGE_*` | replaces the base, and discards `up` |
//! | `CHANGE_*_FINAL` | replaces the base, and discards **both** |
//!
//! The two accumulators are the part worth reading twice. A plain single
//! effect without `SINGLE_RANGE` adds into `up`; everything else adds into
//! `upc`. A `CHANGE` discards `up` and keeps `upc` — so an effect from
//! elsewhere survives a level change that an effect printed on the card does
//! not. Only `CHANGE_FINAL` clears both.
//!
//! ## Level and rank can swap
//!
//! `EFFECT_RANK_LEVEL_S` and `EFFECT_LEVEL_RANK_S` make a card's level and
//! rank the same quantity, and when either is present **both** accessors
//! gather **both** sets of effects. That is why `get_level` reads
//! `EFFECT_UPDATE_RANK` and `get_rank` reads `EFFECT_UPDATE_LEVEL` — not a
//! copy-paste slip.
//!
//! The non-`_S` forms (`EFFECT_RANK_LEVEL`, `EFFECT_LEVEL_RANK`) do
//! something different again: they decide whether a card *has* a rank or a
//! level at all, in the guards at the top.

use crate::board::location;
use crate::card::{assume, card_type, status};
use crate::effect::{effect_type, flag};
use crate::event::{code, CardId, PLAYER_NONE};
use crate::field::Field;

impl Field {
    /// `card::get_level`.
    ///
    /// The guard at the top is four questions in one: an Xyz monster has a
    /// rank rather than a level unless an effect says otherwise, a Link
    /// monster has neither, `STATUS_NO_LEVEL` suppresses it, and a card that
    /// is not a monster at all has no level to report.
    pub fn get_level(&mut self, card: CardId) -> u32 {
        let printed = self.cards[card].data.type_;
        let xyz_without_level = printed & card_type::XYZ != 0
            && self.is_affected_by_effect(card, code::RANK_LEVEL).is_none()
            && self
                .is_affected_by_effect(card, code::RANK_LEVEL_S)
                .is_none();
        if xyz_without_level
            || printed & card_type::LINK != 0
            || self.cards[card].is_status(status::NO_LEVEL)
            || !self.counts_as_monster(card)
        {
            return 0;
        }
        if let Some(&assumed) = self.cards[card].assume.get(&assume::LEVEL) {
            return assumed as u32;
        }
        if let Some(partial) = self.cards[card].temp.level {
            return partial;
        }
        let base = self.cards[card].data.level;
        self.cards[card].temp.level = Some(base);
        let value = self.resolve_level_like(card, base, Which::Level);
        self.cards[card].temp.level = None;
        value
    }

    /// `card::get_rank`.
    ///
    /// Two differences from `get_level` beyond the guard. A card that is not
    /// in a monster zone reports its **printed level** as its rank without
    /// consulting any effect — an Xyz monster in the graveyard has the rank
    /// it was printed with. And the base is `data.level`, not a separate
    /// printed rank: the reference stores one number for both.
    pub fn get_rank(&mut self, card: CardId) -> u32 {
        let printed = self.cards[card].data.type_;
        let not_xyz = printed & card_type::XYZ == 0 || self.cards[card].is_status(status::NO_LEVEL);
        let borrowed = self.is_affected_by_effect(card, code::LEVEL_RANK).is_some()
            || self
                .is_affected_by_effect(card, code::LEVEL_RANK_S)
                .is_some();
        if (not_xyz && !borrowed) || printed & card_type::LINK != 0 {
            return 0;
        }
        if let Some(&assumed) = self.cards[card].assume.get(&assume::RANK) {
            return assumed as u32;
        }
        if !self.cards[card]
            .current
            .is_location(u16::from(location::MZONE))
        {
            return self.cards[card].data.level;
        }
        if let Some(partial) = self.cards[card].temp.rank {
            return partial;
        }
        let base = self.cards[card].data.level;
        self.cards[card].temp.rank = Some(base);
        let value = self.resolve_level_like(card, base, Which::Rank);
        self.cards[card].temp.rank = None;
        value
    }

    /// The body `get_level` and `get_rank` share.
    fn resolve_level_like(&mut self, card: CardId, base: u32, which: Which) -> u32 {
        // When level and rank are the same quantity, both accessors gather
        // both sets of effects.
        let swapped = self
            .is_affected_by_effect(card, code::RANK_LEVEL_S)
            .is_some()
            || self
                .is_affected_by_effect(card, code::LEVEL_RANK_S)
                .is_some();
        let codes: &[u32] = match (swapped, which) {
            (true, _) => &[
                code::UPDATE_RANK,
                code::UPDATE_LEVEL,
                code::CHANGE_RANK,
                code::CHANGE_LEVEL,
                code::CHANGE_RANK_FINAL,
                code::CHANGE_LEVEL_FINAL,
            ],
            (false, Which::Level) => &[
                code::UPDATE_LEVEL,
                code::CHANGE_LEVEL,
                code::CHANGE_LEVEL_FINAL,
            ],
            (false, Which::Rank) => &[
                code::UPDATE_RANK,
                code::CHANGE_RANK,
                code::CHANGE_RANK_FINAL,
            ],
        };
        let mut gathered = Vec::new();
        for &c in codes {
            gathered.extend(self.filter_effect(card, c));
        }
        // Only the last `filter_effect` call sorts, and it sorts everything
        // gathered so far — so they apply in registration order.
        self.sort_by_effect_id(&mut gathered);

        let mut value = base as i64;
        let mut up = 0i64;
        let mut upc = 0i64;
        for id in gathered {
            let Some(e) = self.effects.get(id) else {
                continue;
            };
            let e_code = e.code;
            // A plain single effect adds into `up`, which a CHANGE discards;
            // anything else adds into `upc`, which survives one.
            let plain_single = e.is_type(effect_type::SINGLE) && !e.is_flag(flag::SINGLE_RANGE);
            let amount = self.effect_value_for_card_pub(id, card);
            match e_code {
                c if c == code::UPDATE_RANK || c == code::UPDATE_LEVEL => {
                    if plain_single {
                        up += amount;
                    } else {
                        upc += amount;
                    }
                }
                c if c == code::CHANGE_RANK || c == code::CHANGE_LEVEL => {
                    value = amount;
                    up = 0;
                }
                _ => {
                    value = amount;
                    up = 0;
                    upc = 0;
                }
            }
            let partial = (value + up + upc).max(0) as u32;
            match which {
                Which::Level => self.cards[card].temp.level = Some(partial),
                Which::Rank => self.cards[card].temp.rank = Some(partial),
            }
        }

        let mut total = value + up + upc;
        // A monster's level floors at 1 unless something explicitly allows a
        // negative one. A non-monster is left alone, which is how a card
        // that has lost its monsterhood keeps a level of 0.
        if total < 1
            && self.get_type(card, None, 0, PLAYER_NONE) & card_type::MONSTER != 0
            && self
                .is_affected_by_effect(card, code::ALLOW_NEGATIVE)
                .is_none()
        {
            total = 1;
        }
        total.max(0) as u32
    }

    /// `card::get_attribute`.
    pub fn get_attribute(
        &mut self,
        card: CardId,
        scard: Option<CardId>,
        sumtype: u64,
        playerid: u8,
    ) -> u32 {
        if let Some(&assumed) = self.cards[card].assume.get(&assume::ATTRIBUTE) {
            return assumed as u32;
        }
        if !self.counts_as_monster(card) {
            return 0;
        }
        if let Some(partial) = self.cards[card].temp.attribute {
            return partial;
        }
        let base = self.cards[card].data.attribute;
        self.cards[card].temp.attribute = Some(base);
        let value = self.resolve_masked(
            card,
            u64::from(base),
            scard,
            sumtype,
            playerid,
            [
                code::ADD_ATTRIBUTE,
                code::REMOVE_ATTRIBUTE,
                code::CHANGE_ATTRIBUTE,
            ],
            Masked::Attribute,
        );
        self.cards[card].temp.attribute = None;
        value as u32
    }

    /// `card::get_race`. The same shape, in 64 bits — the race table
    /// outgrew 32.
    pub fn get_race(
        &mut self,
        card: CardId,
        scard: Option<CardId>,
        sumtype: u64,
        playerid: u8,
    ) -> u64 {
        if let Some(&assumed) = self.cards[card].assume.get(&assume::RACE) {
            return assumed;
        }
        if !self.counts_as_monster(card) {
            return 0;
        }
        if let Some(partial) = self.cards[card].temp.race {
            return partial;
        }
        let base = self.cards[card].data.race;
        self.cards[card].temp.race = Some(base);
        let value = self.resolve_masked(
            card,
            base,
            scard,
            sumtype,
            playerid,
            [code::ADD_RACE, code::REMOVE_RACE, code::CHANGE_RACE],
            Masked::Race,
        );
        self.cards[card].temp.race = None;
        value
    }

    /// The body `get_attribute` and `get_race` share, which is `get_type`'s.
    #[allow(clippy::too_many_arguments)]
    fn resolve_masked(
        &mut self,
        card: CardId,
        base: u64,
        scard: Option<CardId>,
        sumtype: u64,
        playerid: u8,
        codes: [u32; 3],
        which: Masked,
    ) -> u64 {
        let mut value = base;
        let mut alt = 0u64;
        let mut changed = false;

        let mut gathered = self.filter_effect(card, codes[0]);
        gathered.extend(self.filter_effect(card, codes[1]));
        gathered.extend(self.filter_effect(card, codes[2]));
        self.sort_by_effect_id(&mut gathered);

        for id in gathered {
            let Some(e) = self.effects.get(id) else {
                continue;
            };
            let (e_code, filter) = (e.code, e.operation_filter);
            let has_operation = e.operation.is_some() || filter.is_some();

            if has_operation {
                if sumtype == 0 {
                    continue;
                }
                if let Some(filter) = filter {
                    if !filter(self, id, scard, sumtype, playerid) {
                        continue;
                    }
                }
                let amount = self.effect_value_for_card_pub(id, card) as u64;
                if e_code == codes[0] {
                    alt |= amount;
                } else if e_code == codes[1] {
                    alt &= !amount;
                } else {
                    alt = amount;
                    changed = true;
                }
            } else {
                let amount = self.effect_value_for_card_pub(id, card) as u64;
                if e_code == codes[0] {
                    value |= amount;
                } else if e_code == codes[1] {
                    value &= !amount;
                } else {
                    value = amount;
                }
                match which {
                    Masked::Attribute => self.cards[card].temp.attribute = Some(value as u32),
                    Masked::Race => self.cards[card].temp.race = Some(value),
                }
            }
        }
        if changed {
            alt
        } else {
            value | alt
        }
    }

    /// The test every stat accessor opens with: printed a monster, counting
    /// as one now, or treated as one by `EFFECT_PRE_MONSTER`.
    ///
    /// The third is what lets a Spell or Trap that is about to become a
    /// monster report a level while it still is not one.
    fn counts_as_monster(&mut self, card: CardId) -> bool {
        self.cards[card].data.is_type(card_type::MONSTER)
            || self.get_type(card, None, 0, PLAYER_NONE) & card_type::MONSTER != 0
            || self
                .is_affected_by_effect(card, code::PRE_MONSTER)
                .is_some()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Which {
    Level,
    Rank,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Masked {
    Attribute,
    Race,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::position;
    use crate::card::{Card, CardData};
    use crate::effect::Effect;
    use crate::event::EffectId;

    fn monster(f: &mut Field, level: u32, type_: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 18036057,
                type_: card_type::MONSTER | type_,
                level,
                attribute: crate::card::attribute::LIGHT,
                race: crate::card::race::FAIRY,
                ..Default::default()
            },
            0,
        );
        c.current.controller = 0;
        c.current.position = position::FACEUP_ATTACK;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        // The next free seat, not seat 0: `add_card` refuses a taken seat
        // silently, and a card left in nowhere reports printed values.
        let seat = f.players[0]
            .mzone
            .iter()
            .position(Option::is_none)
            .expect("a free monster seat") as u32;
        f.add_card(0, id, location::MZONE, seat, false);
        id
    }

    /// An effect on the card itself. `plain_single` decides whether it is
    /// the kind a CHANGE discards.
    fn stat_effect(
        f: &mut Field,
        card: CardId,
        code_: u32,
        value: i64,
        plain_single: bool,
    ) -> crate::event::EffectId {
        let ty = if plain_single {
            effect_type::SINGLE
        } else {
            effect_type::SINGLE | effect_type::FIELD
        };
        let mut e = Effect::new(ty, code_);
        e.owner = Some(card);
        e.handler = Some(card);
        e.value = value;
        if !plain_single {
            e.flag[0] |= flag::SINGLE_RANGE;
            e.range = u16::from(location::MZONE);
        }
        let id = f.new_effect(e);
        f.cards[card].single_effect.insert(code_, id);
        f.cards[card].indexer.insert(id);
        id
    }

    mod level {
        use super::*;

        #[test]
        fn a_plain_monster_reports_its_printed_level() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 4, card_type::EFFECT);
            assert_eq!(f.get_level(c), 4);
        }

        #[test]
        fn updates_add_and_changes_replace() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 4, card_type::EFFECT);
            stat_effect(&mut f, c, code::UPDATE_LEVEL, 2, true);
            assert_eq!(f.get_level(c), 6);

            let c2 = monster(&mut f, 4, card_type::EFFECT);
            stat_effect(&mut f, c2, code::CHANGE_LEVEL, 8, true);
            assert_eq!(f.get_level(c2), 8);
        }

        /// The two accumulators. A `CHANGE` discards the additions from
        /// plain single effects and **keeps** the rest, so an effect from
        /// elsewhere survives a level change that one printed on the card
        /// does not.
        #[test]
        fn a_change_discards_only_the_cards_own_additions() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 4, card_type::EFFECT);
            stat_effect(&mut f, c, code::UPDATE_LEVEL, 2, true); // into `up`
            stat_effect(&mut f, c, code::UPDATE_LEVEL, 3, false); // into `upc`
            stat_effect(&mut f, c, code::CHANGE_LEVEL, 10, true);

            assert_eq!(
                f.get_level(c),
                13,
                "the change discarded the +2 and kept the +3"
            );
        }

        /// `CHANGE_FINAL` discards both.
        #[test]
        fn a_final_change_discards_everything() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 4, card_type::EFFECT);
            stat_effect(&mut f, c, code::UPDATE_LEVEL, 2, true);
            stat_effect(&mut f, c, code::UPDATE_LEVEL, 3, false);
            stat_effect(&mut f, c, code::CHANGE_LEVEL_FINAL, 10, true);
            assert_eq!(f.get_level(c), 10);
        }

        /// A monster's level floors at 1 unless something allows a negative.
        #[test]
        fn a_monsters_level_floors_at_one() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 4, card_type::EFFECT);
            stat_effect(&mut f, c, code::UPDATE_LEVEL, -10, true);
            assert_eq!(f.get_level(c), 1);

            stat_effect(&mut f, c, code::ALLOW_NEGATIVE, 1, true);
            assert_eq!(f.get_level(c), 0, "with the allowance, no floor");
        }

        /// An Xyz monster has a rank, not a level — unless an effect lends
        /// it one.
        #[test]
        fn an_xyz_monster_has_no_level_by_default() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 4, card_type::XYZ);
            assert_eq!(f.get_level(c), 0);

            stat_effect(&mut f, c, code::RANK_LEVEL, 1, true);
            assert_eq!(f.get_level(c), 4, "lent a level");
        }

        #[test]
        fn a_link_monster_and_a_no_level_card_have_none() {
            let mut f = Field::new(8000);
            let link = monster(&mut f, 4, card_type::LINK);
            assert_eq!(f.get_level(link), 0);

            let suppressed = monster(&mut f, 4, card_type::EFFECT);
            f.cards[suppressed].set_status(status::NO_LEVEL, true);
            assert_eq!(f.get_level(suppressed), 0);
        }

        /// The guard is cleared afterwards, so a second call agrees.
        #[test]
        fn the_recursion_guard_is_cleared() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 4, card_type::EFFECT);
            stat_effect(&mut f, c, code::UPDATE_LEVEL, 2, true);
            assert_eq!(f.get_level(c), 6);
            assert_eq!(f.cards[c].temp.level, None);
            assert_eq!(f.get_level(c), 6);
        }
    }

    mod rank {
        use super::*;

        #[test]
        fn only_an_xyz_monster_has_a_rank() {
            let mut f = Field::new(8000);
            let ordinary = monster(&mut f, 4, card_type::EFFECT);
            assert_eq!(f.get_rank(ordinary), 0);

            let xyz = monster(&mut f, 4, card_type::XYZ);
            assert_eq!(f.get_rank(xyz), 4);
        }

        /// Off the monster zone a card reports its **printed** level as its
        /// rank, consulting no effect at all.
        #[test]
        fn off_the_field_the_printed_level_is_the_rank() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 4, card_type::XYZ);
            stat_effect(&mut f, c, code::UPDATE_RANK, 3, true);
            assert_eq!(f.get_rank(c), 7, "in the monster zone, effects apply");

            f.move_card(0, c, location::GRAVE, 0, false);
            assert_eq!(
                f.get_rank(c),
                4,
                "in the graveyard, the printed level and nothing else"
            );
        }

        /// With the swap effect, both accessors gather both sets — so a
        /// level effect moves a rank.
        #[test]
        fn the_swap_makes_level_effects_move_a_rank() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 4, card_type::XYZ);
            stat_effect(&mut f, c, code::UPDATE_LEVEL, 3, true);
            assert_eq!(f.get_rank(c), 4, "a level effect does not move a rank");

            stat_effect(&mut f, c, code::RANK_LEVEL_S, 1, true);
            assert_eq!(f.get_rank(c), 7, "until the two are the same quantity");
        }
    }

    mod attack_and_defence {
        use super::*;

        /// The next free monster seat. `add_card` refuses a taken one
        /// *silently*, so a helper that always used seat 0 would put every
        /// card after the first in nowhere — where the stat accessors
        /// report printed values and every test passes for the wrong
        /// reason.
        fn free_seat(f: &Field) -> u32 {
            f.players[0]
                .mzone
                .iter()
                .position(Option::is_none)
                .expect("a free monster seat") as u32
        }

        fn with_stats(f: &mut Field, atk: i32, def: i32) -> CardId {
            let mut c = Card::with_data(
                CardData {
                    code: 18036057,
                    type_: card_type::MONSTER | card_type::EFFECT,
                    level: 4,
                    attack: atk,
                    defense: def,
                    ..Default::default()
                },
                0,
            );
            c.current.controller = 0;
            c.current.position = position::FACEUP_ATTACK;
            c.set_status(status::EFFECT_ENABLED, true);
            let id = f.new_card(c);
            let seat = free_seat(f);
            f.add_card(0, id, location::MZONE, seat, false);
            id
        }

        #[test]
        fn a_plain_monster_reports_its_printed_stats() {
            let mut f = Field::new(8000);
            let c = with_stats(&mut f, 1900, 1400);
            assert_eq!(f.get_attack(c), 1900);
            assert_eq!(f.get_defense(c), 1400);
        }

        /// Off the monster zone, or mid-summon, the printed value stands —
        /// nothing has had a chance to modify it.
        #[test]
        fn off_the_field_the_printed_value_stands() {
            let mut f = Field::new(8000);
            let c = with_stats(&mut f, 1900, 1400);
            stat_effect(&mut f, c, code::UPDATE_ATTACK, 500, true);
            assert_eq!(f.get_attack(c), 2400);

            f.move_card(0, c, location::GRAVE, 0, false);
            assert_eq!(f.get_attack(c), 1900, "in the graveyard, printed only");
        }

        #[test]
        fn updates_add_and_sets_replace() {
            let mut f = Field::new(8000);
            let c = with_stats(&mut f, 1900, 1400);
            stat_effect(&mut f, c, code::UPDATE_ATTACK, 300, true);
            assert_eq!(f.get_attack(c), 2200);

            let c2 = with_stats(&mut f, 1900, 1400);
            stat_effect(&mut f, c2, code::SET_ATTACK, 100, true);
            assert_eq!(f.get_attack(c2), 100);
        }

        /// A pure field effect — not a single effect with a range, which is
        /// a different thing that the `SET_ATTACK` test below distinguishes.
        fn field_effect_on(f: &mut Field, card: CardId, code_: u32, value: i64) -> EffectId {
            let mut e = Effect::new(effect_type::FIELD, code_);
            e.owner = Some(card);
            e.handler = Some(card);
            e.value = value;
            e.range = u16::from(location::MZONE);
            e.s_range = u16::from(location::MZONE);
            let id = f.new_effect(e);
            f.cards[card].single_effect.insert(code_, id);
            f.cards[card].indexer.insert(id);
            id
        }

        /// Three categories, not two — and the two tests use *different*
        /// ones, which is the trap.
        ///
        /// The `up`/`upc` split for an UPDATE asks `SINGLE && !SINGLE_RANGE`.
        /// The "does a SET discard the additions" test asks only `!SINGLE`.
        /// So a **ranged single** effect accumulates like a foreign effect
        /// but sets like the card's own:
        ///
        /// | effect | UPDATE goes to | SET clears `up`? |
        /// |---|---|---|
        /// | single, no range | `up` | no |
        /// | single, ranged | `upc` | no |
        /// | not single (field) | `upc` | **yes** |
        #[test]
        fn where_a_set_comes_from_decides_what_it_discards() {
            let mut f = Field::new(8000);
            let own = with_stats(&mut f, 1900, 1400);
            stat_effect(&mut f, own, code::UPDATE_ATTACK, 300, true);
            stat_effect(&mut f, own, code::SET_ATTACK, 1000, true);
            assert_eq!(f.get_attack(own), 1300, "its own set keeps the +300");

            // A *ranged single* effect is still a single effect, so it also
            // keeps the additions.
            let ranged = with_stats(&mut f, 1900, 1400);
            stat_effect(&mut f, ranged, code::UPDATE_ATTACK, 300, true);
            stat_effect(&mut f, ranged, code::SET_ATTACK, 1000, false);
            assert_eq!(
                f.get_attack(ranged),
                1300,
                "a ranged single effect is still single"
            );

            // Only a genuinely non-single effect discards them.
            let foreign = with_stats(&mut f, 1900, 1400);
            stat_effect(&mut f, foreign, code::UPDATE_ATTACK, 300, true);
            field_effect_on(&mut f, foreign, code::SET_ATTACK, 1000);
            assert_eq!(f.get_attack(foreign), 1000, "a field effect does");
        }

        /// `SET_BASE_ATTACK` in the main loop puts the card back on its
        /// base — which is a different thing from setting it to a value.
        #[test]
        fn setting_the_base_returns_to_the_base() {
            let mut f = Field::new(8000);
            let c = with_stats(&mut f, 1900, 1400);
            stat_effect(&mut f, c, code::SET_ATTACK, 100, true);
            stat_effect(&mut f, c, code::SET_BASE_ATTACK, 2500, true);
            assert_eq!(
                f.get_attack(c),
                2500,
                "the base was set and the earlier SET_ATTACK discarded"
            );
        }

        /// `EFFECT_REVERSE_UPDATE` flips the sign of every adjustment.
        #[test]
        fn a_reversed_monster_is_lowered_by_a_raise() {
            let mut f = Field::new(8000);
            let c = with_stats(&mut f, 1900, 1400);
            stat_effect(&mut f, c, code::UPDATE_ATTACK, 500, true);
            assert_eq!(f.get_attack(c), 2400);

            stat_effect(&mut f, c, code::REVERSE_UPDATE, 1, true);
            assert_eq!(f.get_attack(c), 1400, "the same effect now subtracts");
        }

        /// A negative printed value is clamped to zero before anything
        /// else — which is how a "?" attack behaves as 0 rather than as a
        /// negative number.
        #[test]
        fn a_negative_printed_value_is_clamped() {
            let mut f = Field::new(8000);
            let c = with_stats(&mut f, -2, -2);
            assert_eq!(f.get_attack(c), 0);
            assert_eq!(f.get_defense(c), 0);
        }

        /// And the result never goes below zero.
        #[test]
        fn the_result_floors_at_zero() {
            let mut f = Field::new(8000);
            let c = with_stats(&mut f, 1000, 1000);
            stat_effect(&mut f, c, code::UPDATE_ATTACK, -5000, true);
            assert_eq!(f.get_attack(c), 0);
        }

        /// A Link monster has no defence at all.
        #[test]
        fn a_link_monster_has_no_defence() {
            let mut f = Field::new(8000);
            let mut c = Card::with_data(
                CardData {
                    code: 1,
                    type_: card_type::MONSTER | card_type::LINK,
                    attack: 2000,
                    defense: 1000,
                    ..Default::default()
                },
                0,
            );
            c.current.position = position::FACEUP_ATTACK;
            let c = f.new_card(c);
            f.add_card(0, c, location::MZONE, 0, false);
            assert_eq!(f.get_attack(c), 2000);
            assert_eq!(f.get_defense(c), 0);
        }

        /// `SWAP_BASE_AD` exchanges the two bases.
        #[test]
        fn swapping_the_bases_exchanges_them() {
            let mut f = Field::new(8000);
            let c = with_stats(&mut f, 1900, 1400);
            stat_effect(&mut f, c, code::SWAP_BASE_AD, 0, true);
            assert_eq!(f.get_attack(c), 1400);
            assert_eq!(f.get_defense(c), 1900);
        }

        /// `SWAP_AD` makes each side report the other — and the mutual
        /// recursion terminates, which is the point of the guard.
        #[test]
        fn swapping_terminates() {
            let mut f = Field::new(8000);
            let c = with_stats(&mut f, 1900, 1400);
            stat_effect(&mut f, c, code::SWAP_AD, 0, true);
            // Whatever the values, the call must return rather than recurse.
            let atk = f.get_attack(c);
            let def = f.get_defense(c);
            assert!(atk >= 0 && def >= 0);
            assert_eq!(f.cards[c].temp.attack, None, "and the guards are clear");
            assert_eq!(f.cards[c].temp.defense, None);
        }

        /// The guard is cleared afterwards, so repeated calls agree.
        #[test]
        fn repeated_calls_agree() {
            let mut f = Field::new(8000);
            let c = with_stats(&mut f, 1900, 1400);
            stat_effect(&mut f, c, code::UPDATE_ATTACK, 400, true);
            assert_eq!(f.get_attack(c), 2300);
            assert_eq!(f.get_attack(c), 2300);
            assert_eq!(f.cards[c].temp.attack, None);
        }
    }

    mod attribute_and_race {
        use super::*;

        #[test]
        fn add_remove_and_change_behave_as_for_type() {
            use crate::card::{attribute, race};
            let mut f = Field::new(8000);

            let c = monster(&mut f, 4, card_type::EFFECT);
            assert_eq!(f.get_attribute(c, None, 0, PLAYER_NONE), attribute::LIGHT);
            stat_effect(
                &mut f,
                c,
                code::ADD_ATTRIBUTE,
                i64::from(attribute::DARK),
                true,
            );
            assert_eq!(
                f.get_attribute(c, None, 0, PLAYER_NONE),
                attribute::LIGHT | attribute::DARK
            );

            let c2 = monster(&mut f, 4, card_type::EFFECT);
            stat_effect(
                &mut f,
                c2,
                code::CHANGE_ATTRIBUTE,
                i64::from(attribute::WATER),
                true,
            );
            assert_eq!(f.get_attribute(c2, None, 0, PLAYER_NONE), attribute::WATER);

            let c3 = monster(&mut f, 4, card_type::EFFECT);
            assert_eq!(f.get_race(c3, None, 0, PLAYER_NONE), race::FAIRY);
            stat_effect(&mut f, c3, code::CHANGE_RACE, race::DRAGON as i64, true);
            assert_eq!(f.get_race(c3, None, 0, PLAYER_NONE), race::DRAGON);
        }

        /// A card that is not a monster has neither, which is how a Spell
        /// reports nothing rather than its database defaults.
        #[test]
        fn a_non_monster_has_neither() {
            let mut f = Field::new(8000);
            let mut c = Card::with_data(
                CardData {
                    code: 1,
                    type_: card_type::SPELL,
                    attribute: crate::card::attribute::LIGHT,
                    race: crate::card::race::FAIRY,
                    ..Default::default()
                },
                0,
            );
            c.current.position = position::FACEUP_ATTACK;
            let c = f.new_card(c);
            f.add_card(0, c, location::SZONE, 0, false);

            assert_eq!(f.get_attribute(c, None, 0, PLAYER_NONE), 0);
            assert_eq!(f.get_race(c, None, 0, PLAYER_NONE), 0);
            assert_eq!(f.get_level(c), 0);

            // Unless something says to treat it as one.
            stat_effect(&mut f, c, code::PRE_MONSTER, 1, true);
            assert_eq!(
                f.get_attribute(c, None, 0, PLAYER_NONE),
                crate::card::attribute::LIGHT,
                "PRE_MONSTER makes the stats readable early"
            );
        }
    }
}

// Attack and defence.
//
// These are the two the reference singles out — *"Atk and def are special
// cases since text atk/def ? are involved"* — and they are the most
// intricate accessors in the engine. Four things make them so.
//
// **Base-setting effects are extracted in a first pass.** `SET_BASE_ATTACK`
// and `SET_BASE_DEFENSE` from anything other than a plain single effect are
// applied *and removed from the list* before the main loop runs, so they
// establish the base that everything else then adjusts. A single pass would
// apply them in sequence order and get a different number.
//
// **`-1` means "unset", not a value.** `atk` starts at -1 and the running
// total reads `(atk < 0) ? batk : atk`. A `SET_BASE_ATTACK` in the main loop
// resets `atk` to -1 — putting the card back on its base — which is a
// different thing from setting it to zero.
//
// **`EFFECT_REVERSE_UPDATE` flips the sign of every adjustment**, so the
// same `UPDATE_ATTACK` that raises a normal monster lowers a reversed one.
//
// **Attack and defence can each be defined in terms of the other**, through
// `SWAP_AD` and the `_FINAL` effects. The recursion is bounded by the temp
// guard: the tail that consults the other value runs **only when the other
// is not currently being computed**. That test — `!has_valid_property_val
// (temp.defense)` — is the whole termination argument, and it is one `if`.

impl Field {
    /// `card::get_attack`.
    pub fn get_attack(&mut self, card: CardId) -> i32 {
        self.get_attack_or_defense(card, Side::Attack)
    }

    /// `card::get_defense`. The mirror of `get_attack`, code for code.
    pub fn get_defense(&mut self, card: CardId) -> i32 {
        self.get_attack_or_defense(card, Side::Defense)
    }

    fn get_attack_or_defense(&mut self, card: CardId, side: Side) -> i32 {
        let assume_key = match side {
            Side::Attack => assume::ATTACK,
            Side::Defense => assume::DEFENSE,
        };
        if let Some(&assumed) = self.cards[card].assume.get(&assume_key) {
            return assumed as i32;
        }
        if !self.counts_as_monster(card) {
            return 0;
        }
        // A Link monster has no defence at all.
        if side == Side::Defense && self.cards[card].data.is_type(card_type::LINK) {
            return 0;
        }
        // Off the monster zone, or mid-summon, the printed value stands:
        // nothing has had a chance to modify it.
        let c = &self.cards[card];
        if c.current.location != location::MZONE
            || c.get_status(status::SUMMONING | status::SPSUMMON_STEP)
        {
            return match side {
                Side::Attack => c.data.attack,
                Side::Defense => c.data.defense,
            };
        }
        if let Some(partial) = self.temp_of(card, side) {
            return partial;
        }
        let value = self.resolve_attack_or_defense(card, side);
        self.set_temp(card, side, None);
        self.set_temp_base(card, side, None);
        value.max(0)
    }

    fn resolve_attack_or_defense(&mut self, card: CardId, side: Side) -> i32 {
        let codes = Codes::for_side(side);
        // Negative printed values are clamped to zero before anything else,
        // which is how a "?" attack behaves as 0 rather than as a negative.
        let mut base = self.cards[card].data.attack.max(0);
        let mut other_base = self.cards[card].data.defense.max(0);
        if side == Side::Defense {
            std::mem::swap(&mut base, &mut other_base);
        }
        self.set_temp_base(card, side, Some(base));
        self.set_temp(card, side, Some(base));

        let is_link = self.cards[card].data.is_type(card_type::LINK);
        let mut gathered = Vec::new();
        for &c in &[
            codes.update,
            codes.set,
            codes.set_final,
            codes.swap_final,
            codes.set_base,
        ] {
            gathered.extend(self.filter_effect(card, c));
        }
        if !is_link {
            for &c in &[code::SWAP_AD, code::SWAP_BASE_AD, codes.set_base_other] {
                gathered.extend(self.filter_effect(card, c));
            }
        }
        self.sort_by_effect_id(&mut gathered);

        // First pass: base-setting effects that are not plain single effects
        // establish the base, and are taken out of the list.
        let mut rest = Vec::with_capacity(gathered.len());
        for id in gathered {
            let Some(e) = self.effects.get(id) else {
                continue;
            };
            let plain_single = e.is_type(effect_type::SINGLE) && !e.is_flag(flag::SINGLE_RANGE);
            let e_code = e.code;
            if !plain_single && (e_code == codes.set_base || e_code == codes.set_base_other) {
                let v = self.effect_value_for_card_pub(id, card) as i32;
                if e_code == codes.set_base {
                    base = v.max(0);
                    self.set_temp_base(card, side, Some(base));
                } else {
                    other_base = v.max(0);
                }
                continue;
            }
            rest.push(id);
        }
        self.set_temp(card, side, Some(base));

        let reversed = self
            .is_affected_by_effect(card, code::REVERSE_UPDATE)
            .is_some();
        // -1 is "unset": the running total falls back to the base.
        let mut set = -1i32;
        let mut up = 0i32;
        let mut upc = 0i32;
        let mut swap_final = false;
        let mut finals = Vec::new();
        let mut delayed_finals = Vec::new();

        for id in rest {
            let Some(e) = self.effects.get(id) else {
                continue;
            };
            let plain_single = e.is_type(effect_type::SINGLE) && !e.is_flag(flag::SINGLE_RANGE);
            let single = e.is_type(effect_type::SINGLE);
            let delayed = e.is_flag(flag::DELAY);
            let e_code = e.code;
            let v = self.effect_value_for_card_pub(id, card) as i32;

            if e_code == codes.update {
                if plain_single {
                    up += v;
                } else {
                    upc += v;
                }
            } else if e_code == codes.set {
                set = v;
                // A `SET_ATTACK` from elsewhere discards the card's own
                // additions; one printed on the card does not.
                if !single {
                    up = 0;
                }
            } else if e_code == codes.set_final {
                if plain_single {
                    set = v;
                    up = 0;
                    upc = 0;
                } else if delayed {
                    delayed_finals.push(id);
                } else {
                    finals.push(id);
                }
            } else if e_code == codes.set_base {
                base = v.max(0);
                // Back to the base, which is not the same as setting zero.
                set = -1;
            } else if e_code == codes.swap_final {
                set = v;
                up = 0;
                upc = 0;
            } else if e_code == codes.set_base_other {
                other_base = v.max(0);
            } else if e_code == code::SWAP_AD {
                swap_final = !swap_final;
            } else if e_code == code::SWAP_BASE_AD {
                std::mem::swap(&mut base, &mut other_base);
            }

            self.set_temp_base(card, side, Some(base));
            let running = if set < 0 { base } else { set };
            let total = if reversed {
                running - up - upc
            } else {
                running + up + upc
            };
            self.set_temp(card, side, Some(total.max(0)));
        }

        for id in finals {
            let v = self.effect_value_for_card_pub(id, card) as i32;
            self.set_temp(card, side, Some(v));
        }

        // The tail that may consult the *other* value runs only when the
        // other is not itself mid-computation. That single test is what
        // bounds the mutual recursion.
        if self.temp_of(card, side.other()).is_none() {
            if swap_final {
                let other = match side {
                    Side::Attack => self.get_defense(card),
                    Side::Defense => self.get_attack(card),
                };
                self.set_temp(card, side, Some(other));
            }
            for id in delayed_finals {
                let v = self.effect_value_for_card_pub(id, card) as i32;
                self.set_temp(card, side, Some(v));
                // `EFFECT_FLAG_REPEAT` asks for the value twice — the second
                // reading may differ, because the first changed the state it
                // is computed from.
                if self
                    .effects
                    .get(id)
                    .is_some_and(|e| e.is_flag(flag::REPEAT))
                {
                    let again = self.effect_value_for_card_pub(id, card) as i32;
                    self.set_temp(card, side, Some(again));
                }
            }
        }
        self.temp_of(card, side).unwrap_or(0)
    }

    fn temp_of(&self, card: CardId, side: Side) -> Option<i32> {
        match side {
            Side::Attack => self.cards[card].temp.attack,
            Side::Defense => self.cards[card].temp.defense,
        }
    }

    fn set_temp(&mut self, card: CardId, side: Side, v: Option<i32>) {
        match side {
            Side::Attack => self.cards[card].temp.attack = v,
            Side::Defense => self.cards[card].temp.defense = v,
        }
    }

    fn set_temp_base(&mut self, card: CardId, side: Side, v: Option<i32>) {
        match side {
            Side::Attack => self.cards[card].temp.base_attack = v,
            Side::Defense => self.cards[card].temp.base_defense = v,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Side {
    Attack,
    Defense,
}

impl Side {
    fn other(self) -> Self {
        match self {
            Side::Attack => Side::Defense,
            Side::Defense => Side::Attack,
        }
    }
}

/// The five effect codes that differ between the attack and defence
/// versions, plus the one that names the *other* side's base.
struct Codes {
    update: u32,
    set: u32,
    set_final: u32,
    set_base: u32,
    swap_final: u32,
    set_base_other: u32,
}

impl Codes {
    fn for_side(side: Side) -> Self {
        match side {
            Side::Attack => Codes {
                update: code::UPDATE_ATTACK,
                set: code::SET_ATTACK,
                set_final: code::SET_ATTACK_FINAL,
                set_base: code::SET_BASE_ATTACK,
                swap_final: code::SWAP_ATTACK_FINAL,
                set_base_other: code::SET_BASE_DEFENSE,
            },
            Side::Defense => Codes {
                update: code::UPDATE_DEFENSE,
                set: code::SET_DEFENSE,
                set_final: code::SET_DEFENSE_FINAL,
                set_base: code::SET_BASE_DEFENSE,
                swap_final: code::SWAP_DEFENSE_FINAL,
                set_base_other: code::SET_BASE_ATTACK,
            },
        }
    }
}

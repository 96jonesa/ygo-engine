//! `RefreshLoc` — which seats are unusable, recomputed from scratch.
//!
//! `Adjust` runs this after every board change. It throws both players'
//! `disabled_location` away and rebuilds them from the effects in force, then
//! tells the host **only if the answer changed**.
//!
//! ## Three loops, written as pairs of cases
//!
//! The machine is one gather (case 0) followed by three drain loops, each a
//! pair: a case that takes the next effect and asks something, and a case
//! that folds the answer in and jumps back. They chain — the disable-field
//! loop falls into the extra-Monster-Zone loop, which falls into the
//! extra-Spell-Zone loop — and the jumps read backwards unless the increment
//! rule is held in mind: `arg.step = 2` means *case 3*.
//!
//! ```text
//!   0 → 1 ⇄ 2 → 3 ⇄ 4 → 5 ⇄ 6 → 7
//!       └ disable ┘   └ mzone ┘  (then szone, then the message)
//! ```
//!
//! ## An effect with no operation is given a value rather than asked
//!
//! `EFFECT_DISABLE_FIELD` normally carries an operation that computes which
//! seats it blocks. One without gets `value = 0x80` written **onto the
//! effect** — the machine mutating an effect it is reading, which is the
//! reference's, and is how the answer is cached for the queries that read the
//! value later.
//!
//! ## The two Extra Monster Zones are mirrored between players
//!
//! Case 7 is four lines of bit-shuffling that say one thing: seat 5 of one
//! player is seat 6 of the other. Disabling one disables its mirror, because
//! they are the same physical zone.

use crate::event::{code, EffectId};
use crate::field::{Field, Message};
use crate::processor::Kind;

/// The state `RefreshLoc` carries between its steps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RefreshLocState {
    /// How many seats the current extra-zone effect is allowed to name.
    pub dis_count: u8,
    /// What the answer was before this pass, so the message can be skipped
    /// when nothing moved. Both rows packed: player 0 low, player 1 high.
    pub previously_disabled: u32,
    /// The effect whose answer the next case will fold in.
    pub current: Option<EffectId>,
}

impl Field {
    /// One step of `RefreshLoc`.
    pub(crate) fn refresh_loc_step(&mut self, step: u16, state: &mut RefreshLocState) -> bool {
        match step {
            0 => self.rl_gather(state),
            1 => self.rl_next_disable_field(state),
            2 => self.rl_take_disable_field(state),
            3 => self.rl_next_extra_zone(state, true),
            4 => self.rl_take_extra_zone(state, true),
            5 => self.rl_next_extra_zone(state, false),
            6 => self.rl_take_extra_zone(state, false),
            7 => self.rl_finish(state),
            _ => true,
        }
    }

    /// Step 0: throw the answer away and gather what will rebuild it.
    ///
    /// An `EFFECT_DISABLE_FIELD` with a **non-zero value and no `REPEAT`
    /// flag** is folded in immediately — its answer is already known.
    /// Everything else goes on a list to be asked, and `REPEAT` is what makes
    /// an effect re-answer every pass rather than keep its cached value.
    fn rl_gather(&mut self, state: &mut RefreshLocState) -> bool {
        if self.is_flag(crate::duel::flags::THREE_COLUMNS_FIELD) {
            for p in 0..2 {
                self.players[p].used_location |= 0x1111;
            }
        }
        state.previously_disabled = (self.players[0].disabled_location & 0xffff)
            | (self.players[1].disabled_location << 16);
        self.players[0].disabled_location = 0;
        self.players[1].disabled_location = 0;
        self.core.disfield_effects.clear();
        self.core.extra_mzone_effects.clear();
        self.core.extra_szone_effects.clear();

        for e in self.filter_field_effect(code::DISABLE_FIELD) {
            let value = self.effect_plain_value(e) as u32;
            let repeat = self
                .effects
                .get(e)
                .is_some_and(|x| x.is_flag(crate::effect::flag::REPEAT));
            if value != 0 && !repeat {
                self.players[0].disabled_location |= value & 0xff7f;
                self.players[1].disabled_location |= (value >> 16) & 0xff7f;
            } else {
                self.core.disfield_effects.push(e);
            }
        }

        for (extra_code, is_mzone) in [
            (code::USE_EXTRA_MZONE, true),
            (code::USE_EXTRA_SZONE, false),
        ] {
            for e in self.filter_field_effect(extra_code) {
                let p = self
                    .effects
                    .get(e)
                    .map_or(0, |x| x.get_handler_player(&self.cards))
                    as usize;
                let value = self.effect_plain_value(e) as u32;
                // The Monster Zone shift is 16 and the Spell Zone one is 8,
                // because the seats they name sit in different bytes of
                // `disabled_location`. Same value layout, different landing.
                if is_mzone {
                    self.players[p].disabled_location |= (value >> 16) & 0x1f;
                } else {
                    self.players[p].disabled_location |= (value >> 8) & 0x1f00;
                }
                if Self::field_used_count((value >> 16) & 0x1f) < (value & 0xffff) {
                    if is_mzone {
                        self.core.extra_mzone_effects.push(e);
                    } else {
                        self.core.extra_szone_effects.push(e);
                    }
                }
            }
        }
        false
    }

    /// Step 1: take the next disable-field effect and run its operation.
    fn rl_next_disable_field(&mut self, state: &mut RefreshLocState) -> bool {
        if self.core.disfield_effects.is_empty() {
            self.set_step(2);
            return false;
        }
        let e = self.core.disfield_effects.remove(0);
        state.current = Some(e);
        if self.effects.get(e).is_none_or(|x| x.operation.is_none()) {
            // No operation: the answer is "the field zone only", written
            // onto the effect so the queries that read its value find it.
            if let Some(x) = self.effects.get_mut(e) {
                x.value = 0x80;
            }
            self.set_step(0);
            return false;
        }
        let player = self
            .effects
            .get(e)
            .map_or(0, |x| x.get_handler_player(&self.cards));
        self.core
            .sub_solving_event
            .push_back(crate::event::Event::new(0));
        self.emplace(Kind::ExecuteOperation {
            resume: None,
            effect: e,
            player,
            subject: None,
            args: Vec::new(),
            was_disabled: false,
        });
        false
    }

    /// Step 2: fold the answer in, and **swap the rows when the effect
    /// belongs to player 1**.
    ///
    /// The value is always written from its owner's point of view, so an
    /// effect on the second player's side has its two halves exchanged before
    /// being stored. A zero answer becomes `0x80` — the field zone — rather
    /// than nothing, so that "this effect disables nothing" is still a cached
    /// answer and not an empty one to be recomputed.
    fn rl_take_disable_field(&mut self, state: &mut RefreshLocState) -> bool {
        let mut disabled = self.core.returns.get() as u32;
        disabled &= 0xff7f_ff7f;
        if disabled == 0 {
            disabled = 0x80;
        }
        let Some(e) = state.current else {
            self.set_step(0);
            return false;
        };
        let owner = self
            .effects
            .get(e)
            .map_or(0, |x| x.get_handler_player(&self.cards));
        if owner == 0 {
            if let Some(x) = self.effects.get_mut(e) {
                x.value = i64::from(disabled);
            }
            self.players[0].disabled_location |= disabled & 0xff7f;
            self.players[1].disabled_location |= (disabled >> 16) & 0xff7f;
        } else {
            if let Some(x) = self.effects.get_mut(e) {
                // `(v << 16) | (v >> 16)` as the reference writes it, which
                // on a `u32` is exactly a rotation.
                x.value = i64::from(disabled.rotate_right(16));
            }
            self.players[1].disabled_location |= disabled & 0xff7f;
            self.players[0].disabled_location |= (disabled >> 16) & 0xff7f;
        }
        self.core.returns.set(disabled as i32);
        self.set_step(0);
        false
    }

    /// Steps 3 and 5: take the next extra-zone effect and ask the player
    /// which seats to give up.
    ///
    /// **A full row ends the loop rather than asking**, and it ends the
    /// *whole* loop rather than skipping the one effect — the reference
    /// jumps past, not back. `dis_count` is how many more seats the effect
    /// allows, capped by how many are actually free.
    fn rl_next_extra_zone(&mut self, state: &mut RefreshLocState, mzone: bool) -> bool {
        let list = if mzone {
            &mut self.core.extra_mzone_effects
        } else {
            &mut self.core.extra_szone_effects
        };
        if list.is_empty() {
            self.set_step(if mzone { 4 } else { 6 });
            return false;
        }
        let e = list.remove(0);
        state.current = Some(e);
        let p = self
            .effects
            .get(e)
            .map_or(0, |x| x.get_handler_player(&self.cards)) as usize;
        let occupied = self.players[p].disabled_location | self.players[p].used_location;
        let row = if mzone {
            occupied & 0x1f
        } else {
            (occupied >> 8) & 0x1f
        };
        if row == 0x1f {
            self.set_step(if mzone { 4 } else { 6 });
            return false;
        }
        let val = self.effect_plain_value(e) as u32;
        let allowed = (val & 0xffff) as i32 - Self::field_used_count((val >> 16) & 0x1f) as i32;
        let empty = 5 - Self::field_used_count(row) as i32;
        let flag = if mzone {
            row | 0xffff_ffe0
        } else {
            (row << 8) | 0xffff_e0ff
        };
        state.dis_count = allowed.min(empty).max(0) as u8;
        self.emplace(Kind::SelectPlace {
            player: p as u8,
            flag,
            count: state.dis_count,
            disable_field: true,
        });
        false
    }

    /// Steps 4 and 6: fold the chosen seats in.
    ///
    /// The answer is a run of three bytes per seat — player, location, seat —
    /// and only the third of each is read. The chosen seats go into
    /// `disabled_location` at the row's own offset, but onto the **effect's
    /// value** shifted by 16 either way: the effect records what it gave up
    /// in the high half regardless of which row it was.
    fn rl_take_extra_zone(&mut self, state: &mut RefreshLocState, mzone: bool) -> bool {
        let mut chosen = 0u32;
        for i in 0..u32::from(state.dis_count) {
            let seat = self.core.returns.at_i8((i * 3 + 2) as usize);
            chosen |= 1u32 << seat;
        }
        if let Some(e) = state.current {
            let p = self
                .effects
                .get(e)
                .map_or(0, |x| x.get_handler_player(&self.cards)) as usize;
            self.players[p].disabled_location |= if mzone { chosen } else { chosen << 8 };
            if let Some(x) = self.effects.get_mut(e) {
                x.value |= i64::from(chosen << 16);
            }
        }
        self.set_step(if mzone { 2 } else { 4 });
        false
    }

    /// Step 7: mirror the Extra Monster Zones, and tell the host if anything
    /// changed.
    ///
    /// Seat 5 of one player is seat 6 of the other — the same physical zone
    /// seen from two sides — so disabling either disables both. The four
    /// lines that say it are pure bit-shuffling and are easy to mistype in a
    /// way nothing notices.
    fn rl_finish(&mut self, state: &mut RefreshLocState) -> bool {
        let (a, b) = (
            self.players[0].disabled_location,
            self.players[1].disabled_location,
        );
        self.players[0].disabled_location |= (((b >> 5) & 1) << 6) | (((b >> 6) & 1) << 5);
        self.players[1].disabled_location |= (((a >> 5) & 1) << 6) | (((a >> 6) & 1) << 5);
        let now = self.players[0].disabled_location | (self.players[1].disabled_location << 16);
        if now != state.previously_disabled {
            self.messages
                .push(Message::FieldDisabled { locations: now });
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::{location, position};
    use crate::card::{card_type, status, Card, CardData};
    use crate::effect::{effect_type, flag, Effect};
    use crate::event::CardId;
    use crate::processor::Status;

    fn anchor(f: &mut Field, player: u8, seat: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 18036057,
                type_: card_type::MONSTER,
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

    /// A field-wide effect of `code_` owned by `player`.
    fn aura(f: &mut Field, card: CardId, code_: u32, value: i64) -> crate::event::EffectId {
        let mut e = Effect::new(effect_type::FIELD, code_);
        e.owner = Some(card);
        e.handler = Some(card);
        e.value = value;
        e.range = u16::from(location::MZONE);
        let id = f.new_effect(e);
        let owner = f.cards[card].current.controller;
        f.add_effect(id, owner);
        id
    }

    fn run(f: &mut Field, seats: &[i8]) -> Status {
        let mut given = 0usize;
        for _ in 0..2048 {
            match f.process() {
                Status::Continue => continue,
                Status::Awaiting => match f.messages.last() {
                    Some(Message::SelectPlace { .. } | Message::Retry) => {
                        for (i, &s) in seats.iter().enumerate() {
                            f.core.returns.set_i8(i * 3, 0);
                            f.core.returns.set_i8(i * 3 + 1, location::MZONE as i8);
                            f.core.returns.set_i8(i * 3 + 2, s);
                        }
                        given += 1;
                        assert!(given < 64, "answered too many times");
                    }
                    _ => return Status::Awaiting,
                },
                other => return other,
            }
        }
        panic!("did not settle");
    }

    fn refresh(f: &mut Field) {
        f.emplace(Kind::RefreshLoc {
            state: Default::default(),
        });
    }

    /// With nothing in force, nothing is disabled — and the host is told
    /// nothing, because nothing changed.
    #[test]
    fn an_empty_board_disables_nothing_and_says_nothing() {
        let mut f = Field::new(8000);
        refresh(&mut f);
        assert_eq!(run(&mut f, &[]), Status::End);
        assert_eq!(f.players[0].disabled_location, 0);
        assert_eq!(f.players[1].disabled_location, 0);
        assert!(
            !f.messages
                .iter()
                .any(|m| matches!(m, Message::FieldDisabled { .. })),
            "unchanged, so unreported"
        );
    }

    /// **The old answer is thrown away first.** A stale value left over from
    /// a previous pass does not survive one where nothing disables anything.
    #[test]
    fn the_previous_answer_is_discarded() {
        let mut f = Field::new(8000);
        f.players[0].disabled_location = 0x1f;
        refresh(&mut f);
        run(&mut f, &[]);
        assert_eq!(f.players[0].disabled_location, 0, "rebuilt from nothing");
        assert!(
            f.messages
                .iter()
                .any(|m| matches!(m, Message::FieldDisabled { locations: 0 })),
            "and the change is reported"
        );
    }

    mod disable_field {
        use super::*;

        /// An effect with a **cached value and no `REPEAT`** is folded in
        /// without being asked: the low half is this player's row and the
        /// high half the opponent's.
        #[test]
        fn a_cached_value_is_folded_in_without_asking() {
            let mut f = Field::new(8000);
            let a = anchor(&mut f, 0, 0);
            aura(&mut f, a, code::DISABLE_FIELD, 0x0003_0005);
            refresh(&mut f);
            assert_eq!(run(&mut f, &[]), Status::End);
            assert_eq!(f.players[0].disabled_location, 0x5);
            assert_eq!(f.players[1].disabled_location, 0x3);
        }

        /// `EFFECT_FLAG_REPEAT` is what makes an effect re-answer rather than
        /// keep its cached value — so a repeating effect with no operation
        /// falls through to the `0x80` default instead.
        #[test]
        fn a_repeating_effect_is_asked_again() {
            let mut f = Field::new(8000);
            let a = anchor(&mut f, 0, 0);
            let e = aura(&mut f, a, code::DISABLE_FIELD, 0x0003_0005);
            if let Some(x) = f.effects.get_mut(e) {
                x.flag[0] |= flag::REPEAT;
            }
            refresh(&mut f);
            run(&mut f, &[]);
            assert_eq!(
                f.effects.get(e).map(|x| x.value),
                Some(0x80),
                "re-answered, and an effect with no operation answers the field zone"
            );
            // **And it blocks nothing this pass.** The default is written
            // onto the effect for the *next* gather to read — which, for a
            // repeating effect, never happens. An effect that repeats and
            // cannot answer contributes nothing, which is the rule rather
            // than an oversight.
            assert_eq!(f.players[0].disabled_location, 0);
        }

        /// An effect **with** an operation is run, and its answer is cached
        /// onto the effect.
        #[test]
        fn an_operation_is_run_and_its_answer_cached() {
            fn block_two(_: &mut Field, _: &crate::effect::Ctx) -> crate::effect::Yield {
                crate::effect::Yield::Done(0x3)
            }
            let mut f = Field::new(8000);
            let a = anchor(&mut f, 0, 0);
            let e = aura(&mut f, a, code::DISABLE_FIELD, 0);
            if let Some(x) = f.effects.get_mut(e) {
                x.operation = Some(block_two);
            }
            refresh(&mut f);
            run(&mut f, &[]);
            assert_eq!(f.players[0].disabled_location, 0x3);
            assert_eq!(f.effects.get(e).map(|x| x.value), Some(0x3), "cached");
        }

        /// **An effect on the second player's side has its two halves
        /// swapped** before being stored, because the value is always written
        /// from its owner's point of view.
        #[test]
        fn the_second_players_effect_has_its_rows_swapped() {
            fn block(_: &mut Field, _: &crate::effect::Ctx) -> crate::effect::Yield {
                crate::effect::Yield::Done(0x3)
            }
            let mut f = Field::new(8000);
            let a = anchor(&mut f, 1, 0);
            let e = aura(&mut f, a, code::DISABLE_FIELD, 0);
            if let Some(x) = f.effects.get_mut(e) {
                x.operation = Some(block);
            }
            refresh(&mut f);
            run(&mut f, &[]);
            assert_eq!(
                f.players[1].disabled_location, 0x3,
                "the owner's row is the low half"
            );
            assert_eq!(f.players[0].disabled_location, 0);
            assert_eq!(
                f.effects.get(e).map(|x| x.value),
                Some(0x0003_0000),
                "and the stored value is swapped"
            );
        }

        /// **The answer is masked before it is stored**, and the bit the
        /// mask strips is the field zone's own — `0xff7f` in each half. An
        /// effect cannot claim the field zone by answering with it; that is
        /// what the zero default is for.
        #[test]
        fn the_answer_is_masked() {
            fn everything(_: &mut Field, _: &crate::effect::Ctx) -> crate::effect::Yield {
                crate::effect::Yield::Done(0xff)
            }
            let mut f = Field::new(8000);
            let a = anchor(&mut f, 0, 0);
            let e = aura(&mut f, a, code::DISABLE_FIELD, 0);
            if let Some(x) = f.effects.get_mut(e) {
                x.operation = Some(everything);
            }
            refresh(&mut f);
            run(&mut f, &[]);
            assert_eq!(
                f.effects.get(e).map(|x| x.value),
                Some(0x7f),
                "the field-zone bit is struck out"
            );
            assert_eq!(f.players[0].disabled_location, 0x7f);
        }

        /// An answer of zero becomes the field zone rather than nothing, so
        /// that "this disables nothing" is still a cached answer.
        #[test]
        fn an_answer_of_zero_becomes_the_field_zone() {
            fn nothing(_: &mut Field, _: &crate::effect::Ctx) -> crate::effect::Yield {
                crate::effect::Yield::Done(0)
            }
            let mut f = Field::new(8000);
            let a = anchor(&mut f, 0, 0);
            let e = aura(&mut f, a, code::DISABLE_FIELD, 0);
            if let Some(x) = f.effects.get_mut(e) {
                x.operation = Some(nothing);
            }
            refresh(&mut f);
            run(&mut f, &[]);
            assert_eq!(f.effects.get(e).map(|x| x.value), Some(0x80));
        }

        /// **Every effect in the list is drained**, not only the first — the
        /// loop is the point of the two-case shape.
        #[test]
        fn every_effect_is_asked() {
            let mut f = Field::new(8000);
            let a = anchor(&mut f, 0, 0);
            let b = anchor(&mut f, 0, 1);
            let x = aura(&mut f, a, code::DISABLE_FIELD, 0);
            let y = aura(&mut f, b, code::DISABLE_FIELD, 0);
            for (e, v) in [(x, 0x1), (y, 0x2)] {
                if let Some(eff) = f.effects.get_mut(e) {
                    eff.value = v;
                    eff.flag[0] |= flag::REPEAT;
                }
            }
            // Both repeat and neither has an operation, so both land on the
            // default — which is only observable as *both* being rewritten.
            refresh(&mut f);
            run(&mut f, &[]);
            assert_eq!(f.effects.get(x).map(|e| e.value), Some(0x80));
            assert_eq!(f.effects.get(y).map(|e| e.value), Some(0x80));
        }
    }

    mod extra_zones {
        use super::*;

        /// An extra-zone effect blocks the seats its value names in the high
        /// half, and asks for more when it allows more than it has taken.
        #[test]
        fn the_named_seats_are_blocked_without_asking() {
            let mut f = Field::new(8000);
            let a = anchor(&mut f, 0, 0);
            // Allows one, has already taken seat 1: nothing more to ask.
            aura(&mut f, a, code::USE_EXTRA_MZONE, 0x0002_0001);
            refresh(&mut f);
            assert_eq!(run(&mut f, &[]), Status::End);
            assert_ne!(
                f.players[0].disabled_location & 0x2,
                0,
                "the named seat is blocked"
            );
        }

        /// **A row that is already full ends the loop rather than asking.**
        #[test]
        fn a_full_row_is_not_asked_about() {
            let mut f = Field::new(8000);
            let a = anchor(&mut f, 0, 0);
            aura(&mut f, a, code::USE_EXTRA_MZONE, 0x0000_0002);
            f.players[0].used_location |= 0x1f;
            refresh(&mut f);
            assert_eq!(run(&mut f, &[]), Status::End);
            // A `SelectPlace` for zero seats answers itself with a bare
            // prompt, so "no question" has to be asserted on the message and
            // not on the machine settling.
            assert!(
                !f.messages.iter().any(|m| matches!(
                    m,
                    Message::Hint {
                        kind: crate::host_question::hint::SELECTMSG,
                        ..
                    }
                )),
                "the loop ended rather than asking for nothing"
            );
        }

        /// The Spell Zone half lands eight bits up, where the Monster Zone
        /// half lands at zero — same value layout, different row.
        #[test]
        fn the_spell_zone_half_lands_in_its_own_row() {
            let mut f = Field::new(8000);
            let a = anchor(&mut f, 0, 0);
            aura(&mut f, a, code::USE_EXTRA_SZONE, 0x0002_0001);
            refresh(&mut f);
            run(&mut f, &[]);
            assert_ne!(f.players[0].disabled_location & 0x0200, 0);
            assert_eq!(f.players[0].disabled_location & 0x1f, 0, "not the monsters");
        }
    }

    /// **Seat 5 of one player is seat 6 of the other**: the same physical
    /// Extra Monster Zone, so disabling either disables both.
    #[test]
    fn the_extra_monster_zones_are_mirrored() {
        let mut f = Field::new(8000);
        let a = anchor(&mut f, 0, 0);
        aura(&mut f, a, code::DISABLE_FIELD, 0x0000_0020);
        refresh(&mut f);
        run(&mut f, &[]);
        assert_ne!(f.players[0].disabled_location & 0x20, 0, "seat 5 of one");
        assert_ne!(
            f.players[1].disabled_location & 0x40,
            0,
            "is seat 6 of the other"
        );

        // And the same crossing the other way, which is a separate line of
        // the same four and fails independently.
        let mut g = Field::new(8000);
        let a = anchor(&mut g, 0, 0);
        aura(&mut g, a, code::DISABLE_FIELD, 0x0020_0000);
        refresh(&mut g);
        run(&mut g, &[]);
        assert_ne!(g.players[1].disabled_location & 0x20, 0);
        assert_ne!(
            g.players[0].disabled_location & 0x40,
            0,
            "the mirror works from either side"
        );
    }

    /// The duel option that blocks the outer columns writes them into
    /// `used_location`, not into the disabled rows.
    #[test]
    fn the_three_column_option_marks_the_outer_seats_used() {
        let mut f = Field::with_flags(
            8000,
            crate::duel::REFERENCE_CONFIGURATION | crate::duel::flags::THREE_COLUMNS_FIELD,
        );
        refresh(&mut f);
        run(&mut f, &[]);
        for p in 0..2 {
            assert_eq!(
                f.players[p].used_location & 0x1111,
                0x1111,
                "the outer columns are spoken for"
            );
        }
    }
}

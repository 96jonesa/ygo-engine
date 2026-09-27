//! Setting a Spell or Trap face-down: `SpellSet`.
//!
//! Three steps, and the first is entirely **refusals**: five reasons the set
//! does not happen, each returning `TRUE` — finished, not failed. A caller
//! that emplaces this unit and expects the card to be face-down afterwards
//! has to look at the board to find out; the unit reports nothing.
//!
//! That is the same shape `MonsterSet` has, but the refusals are different
//! questions, and one of them is not obvious:
//!
//! ## Setting a *monster* into a Spell/Trap Zone
//!
//! Case 0's second test reads `EFFECT_MONSTER_SSET`, and case 2 reads it
//! again. That is the trap-monster mechanism working backwards: a monster
//! may be Set as a Spell/Trap only if something has said it may, and when it
//! is, the effect's **value is the card type it takes on while it sits
//! there**, applied as an `EFFECT_CHANGE_TYPE` on the card itself.
//!
//! So `SpellSet` writes an effect onto the card as part of setting it, and
//! the reset mask is `RESET_EVENT + 0x1fe0000` — the type reverts the moment
//! the card leaves, in any of the eight ways a card can leave.
//!
//! ## The Field Spell's zone is named, and everything else's is not
//!
//! `move_to_field` is called with `0x1 << 5` for a Field Spell and `0xff`
//! for everything else. The first is *the* Field Zone; the second is "any of
//! them", which lets the placement machinery ask the player. Case 0's count
//! check is skipped for a Field Spell for the same reason: a Field Spell
//! displaces whatever is already there rather than needing a free seat.

use crate::board::{location, position};
use crate::card::{card_type, status};
use crate::event::{code, CardId, EffectId};
use crate::field::{reset, timing, Field, Message};
use crate::processor::Kind;

impl Field {
    /// The type a monster borrows to sit in the Spell & Trap row: the
    /// `EFFECT_MONSTER_SSET` value applied as an `EFFECT_CHANGE_TYPE` — the
    /// Trap Monster mechanism backwards.
    ///
    /// **Shared between `SpellSet` and `SpellSetGroup`, where the
    /// reference writes it out twice.** That is a deliberate departure: the
    /// two copies are the same operation down to the reset literal, and two
    /// transcriptions of a constant-laden block are two chances to get it
    /// wrong independently.
    ///
    /// Unconditional on the permission existing: both callers have already
    /// refused a monster without `MONSTER_SSET`.
    pub(crate) fn grant_set_monster_type(&mut self, target: CardId) {
        let granting = self
            .is_affected_by_effect(target, code::MONSTER_SSET)
            .expect("the caller refuses a monster without MONSTER_SSET");
        let type_val = self.effect_plain_value(granting) as i32;
        let mut e =
            crate::effect::Effect::new(crate::effect::effect_type::SINGLE, code::CHANGE_TYPE);
        e.owner = Some(target);
        e.handler = Some(target);
        e.value = i64::from(type_val);
        // `RESET_EVENT + 0x1fe0000` — every way the card can leave, LEAVE
        // included. One bit wider than the equip borrow's `0x17e0000`.
        e.reset_flag = reset::EVENT
            + reset::TURN_SET
            + reset::TOGRAVE
            + reset::REMOVE
            + reset::TEMP_REMOVE
            + reset::TOHAND
            + reset::TODECK
            + reset::LEAVE
            + reset::TOFIELD;
        debug_assert_eq!(
            e.reset_flag - reset::EVENT,
            0x1fe_0000,
            "the decomposition must equal the reference's literal"
        );
        let id = self.new_effect(e);
        self.cards[target]
            .single_effect
            .insert(code::CHANGE_TYPE, id);
        self.cards[target].indexer.insert(id);
    }

    /// `field::process(Processors::SpellSet&)`.
    pub(crate) fn spell_set_step(
        &mut self,
        step: u16,
        setplayer: u8,
        toplayer: u8,
        target: CardId,
        reason_effect: Option<EffectId>,
    ) -> bool {
        match step {
            0 => self.spell_set_step_0(setplayer, toplayer, target),
            1 => self.spell_set_step_1(setplayer, toplayer, target),
            _ => self.spell_set_step_2(setplayer, target, reason_effect),
        }
    }

    /// Step 0: the five refusals, then the cost.
    ///
    /// The order is the reference's and is load-bearing in one place: the
    /// zone count is asked **first**, before anything about permissions, so
    /// a set with nowhere to go is refused without ever asking the player's
    /// prohibitions — which matters because asking them can run card code.
    ///
    /// `current.location == LOCATION_SZONE` refuses a card that is *already*
    /// there. Not a redundancy with the count: the zone it occupies is one
    /// of the free ones as far as the count is concerned, so without this a
    /// card could be "set" onto itself.
    fn spell_set_step_0(&mut self, setplayer: u8, toplayer: u8, target: CardId) -> bool {
        let ty = self.cards[target].data.type_;
        if ty & card_type::FIELD == 0
            && self.get_useable_count(
                Some(target),
                toplayer,
                location::SZONE,
                setplayer,
                Self::LOCATION_REASON_TOFIELD,
                0xff,
            ) <= 0
        {
            return true;
        }
        if ty & card_type::MONSTER != 0
            && self
                .is_affected_by_effect(target, code::MONSTER_SSET)
                .is_none()
        {
            return true;
        }
        if self.cards[target].current.location == location::SZONE {
            return true;
        }
        if !self.is_player_can_sset(setplayer, target) {
            return true;
        }
        if self
            .is_affected_by_effect(target, code::CANNOT_SSET)
            .is_some()
        {
            return true;
        }
        // The costs are *run*, not merely asked: `is_setable_szone` already
        // checked they could be paid. Each is a separate `ExecuteOperation`,
        // so they resolve in reverse order of emplacement — the reference's
        // stack, kept.
        for e in self.filter_effect(target, code::SSET_COST) {
            if self.effects.get(e).is_some_and(|x| x.operation.is_some()) {
                self.core
                    .sub_solving_event
                    .push_back(crate::event::Event::new(0));
                self.emplace(Kind::ExecuteOperation {
                    resume: None,
                    effect: e,
                    player: setplayer,
                    subject: None,
                    args: Vec::new(),
                    was_disabled: false,
                });
            }
        }
        false
    }

    /// Step 1: put it down, face-down, with its field effect switched off.
    ///
    /// `enable_field_effect(false)` before the move, not after: the card is
    /// about to occupy a zone from which its continuous effect would
    /// otherwise apply, and a set card's effect does not apply.
    fn spell_set_step_1(&mut self, setplayer: u8, toplayer: u8, target: CardId) -> bool {
        let zone = if self.cards[target].data.type_ & card_type::FIELD != 0 {
            0x1 << 5
        } else {
            0xff
        };
        self.enable_field_effect(target, false);
        self.move_to_field(
            target,
            setplayer,
            toplayer,
            u16::from(location::SZONE),
            position::FACEDOWN,
            false,
            0,
            zone,
            false,
            0,
            // `confirm`: the reference's default, and it is `true`.
            true,
        );
        false
    }

    /// Step 2: record it, and grant the trap monster its borrowed type.
    ///
    /// `STATUS_SET_TURN` is what makes "you may not activate a Trap the turn
    /// you set it" decidable later; it is set here and nowhere else on this
    /// path.
    ///
    /// The `PointEvent` at the end is emplaced **only outside a chain**, the
    /// same rule `MonsterSet` follows: a set during a chain's resolution
    /// does not open a window of its own.
    fn spell_set_step_2(
        &mut self,
        setplayer: u8,
        target: CardId,
        reason_effect: Option<EffectId>,
    ) -> bool {
        self.core.phase_action = true;
        self.cards[target].set_status(status::SET_TURN, true);
        if self.cards[target].data.type_ & card_type::MONSTER != 0 {
            self.grant_set_monster_type(target);
        }

        let info = self.get_info_location(target);
        let code_ = self.cards[target].data.code;
        self.messages.push(Message::Set {
            code: code_,
            controller: info.controller,
            location: info.location,
            sequence: info.sequence,
            position: info.position,
        });
        self.adjust_instant();
        self.raise_event(
            Some(target),
            code::SSET,
            reason_effect,
            0,
            setplayer,
            setplayer,
            0,
        );
        self.process_instant_event();
        if self.core.current_chain.is_empty() {
            self.adjust_all();
            self.core.hint_timing[setplayer as usize] |= timing::SSET;
            self.emplace(Kind::PointEvent {
                skip: crate::point_event::PointEventSkip::default(),
            });
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{Card, CardData};
    use crate::effect::{effect_type, Effect};
    use crate::processor::Status;

    fn spell_in_hand(f: &mut Field, type_: u32) -> CardId {
        let seat = f.players[0].hand.len() as u32;
        let mut c = Card::with_data(
            CardData {
                code: 5318639 + seat,
                type_,
                ..Default::default()
            },
            0,
        );
        c.current.controller = 0;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(0, id, location::HAND, seat, false);
        id
    }

    /// Run, answering the seat question `MoveToField` asks and declining
    /// the response window the last step opens.
    fn run(f: &mut Field, seat: i8) -> Status {
        for _ in 0..512 {
            match f.process() {
                Status::Continue => continue,
                Status::Awaiting => match f.messages.last() {
                    Some(Message::SelectPlace { .. }) => {
                        f.core.returns.set_i8(0, 0);
                        f.core.returns.set_i8(1, location::SZONE as i8);
                        f.core.returns.set_i8(2, seat);
                    }
                    Some(Message::SelectChain { .. }) => f.core.returns.set(-1),
                    other => panic!("unexpected question: {other:?}"),
                },
                other => return other,
            }
        }
        panic!("did not settle");
    }

    fn asked_for_a_seat(f: &Field) -> bool {
        f.messages
            .iter()
            .any(|m| matches!(m, Message::SelectPlace { .. }))
    }

    fn set(f: &mut Field, target: CardId) {
        f.emplace(Kind::SpellSet {
            setplayer: 0,
            toplayer: 0,
            target,
            reason_effect: None,
        });
    }

    /// The ordinary case: the card lands face-down in a Spell/Trap Zone,
    /// marked as set this turn, and `EVENT_SSET` is raised.
    #[test]
    fn a_spell_lands_face_down_and_is_marked() {
        let mut f = Field::new(8000);
        let c = spell_in_hand(&mut f, card_type::SPELL);
        set(&mut f, c);
        run(&mut f, 0);

        assert_eq!(f.cards[c].current.location, location::SZONE);
        assert_eq!(f.cards[c].current.position, position::FACEDOWN);
        assert!(f.cards[c].is_status(status::SET_TURN));
        assert!(
            !f.cards[c].is_status(status::EFFECT_ENABLED),
            "a set card's effect does not apply"
        );
        assert!(f.core.phase_action);
        // The timing hint is *spent* by the window it opens, so it is gone
        // by the time the run settles. What it is for is telling the
        // responder why they are being asked, so that is what is asserted.
        let offered_timing = f.messages.iter().find_map(|m| match m {
            Message::SelectChain { hint_timing, .. } => Some(*hint_timing),
            _ => None,
        });
        assert_eq!(
            offered_timing.map(|x| x & timing::SSET),
            Some(timing::SSET),
            "the response window was opened on the set"
        );
        assert!(
            f.messages.iter().any(|m| matches!(m, Message::Set { .. })),
            "MSG_SET"
        );
    }

    /// **A card already in a Spell/Trap Zone is refused** — which is not
    /// implied by the seat count, because the seat it sits in is one of the
    /// free ones as far as the count is concerned.
    #[test]
    fn a_card_already_in_the_zone_is_refused() {
        let mut f = Field::new(8000);
        let mut c = Card::with_data(
            CardData {
                code: 5318639,
                type_: card_type::SPELL,
                ..Default::default()
            },
            0,
        );
        c.current.controller = 0;
        let c = f.new_card(c);
        f.add_card(0, c, location::SZONE, 0, false);
        f.cards[c].current.position = position::FACEUP;

        set(&mut f, c);
        run(&mut f, 1);
        assert_eq!(
            f.cards[c].current.position,
            position::FACEUP,
            "untouched — the unit finished without doing anything"
        );
        assert!(!f.cards[c].is_status(status::SET_TURN));
    }

    /// **A monster needs `EFFECT_MONSTER_SSET`**, and when it has one the
    /// effect's value becomes an `EFFECT_CHANGE_TYPE` on the card — the
    /// trap-monster mechanism, working backwards.
    #[test]
    fn a_monster_needs_permission_and_gets_a_borrowed_type() {
        let mut f = Field::new(8000);
        let c = spell_in_hand(&mut f, card_type::MONSTER);
        set(&mut f, c);
        run(&mut f, 0);
        assert_eq!(
            f.cards[c].current.location,
            location::HAND,
            "refused without permission"
        );

        let mut f = Field::new(8000);
        let c = spell_in_hand(&mut f, card_type::MONSTER);
        let mut e = Effect::new(effect_type::SINGLE, code::MONSTER_SSET);
        e.owner = Some(c);
        e.handler = Some(c);
        e.value = i64::from(card_type::TRAP | card_type::MONSTER);
        let id = f.new_effect(e);
        f.cards[c].single_effect.insert(code::MONSTER_SSET, id);
        f.cards[c].indexer.insert(id);

        set(&mut f, c);
        run(&mut f, 0);
        assert_eq!(f.cards[c].current.location, location::SZONE);
        let granted = f.cards[c]
            .single_effect
            .equal_range(code::CHANGE_TYPE)
            .to_vec();
        assert_eq!(granted.len(), 1, "one EFFECT_CHANGE_TYPE was added");
        assert_eq!(
            f.effects.get(granted[0]).map(|x| x.value),
            Some(i64::from(card_type::TRAP | card_type::MONSTER)),
            "carrying the permission's value"
        );
        assert_eq!(
            f.effects.get(granted[0]).map(|x| x.reset_flag),
            Some(reset::EVENT + 0x1fe_0000),
            "and reset by every way the card can leave"
        );
    }

    /// **A Field Spell is placed in the Field Zone by name, and needs no
    /// free seat** — the count check is skipped for it entirely.
    #[test]
    fn a_field_spell_is_named_its_zone_and_needs_no_seat() {
        let mut f = Field::new(8000);
        // Fill every ordinary Spell/Trap Zone.
        for seat in 0..5 {
            let mut c = Card::with_data(
                CardData {
                    code: 1 + seat,
                    type_: card_type::SPELL,
                    ..Default::default()
                },
                0,
            );
            c.current.controller = 0;
            let id = f.new_card(c);
            f.add_card(0, id, location::SZONE, seat, false);
        }
        let c = spell_in_hand(&mut f, card_type::SPELL | card_type::FIELD);
        set(&mut f, c);
        run(&mut f, 0);
        assert!(
            !asked_for_a_seat(&f),
            "the zone is named, so nothing is asked"
        );
        assert_eq!(f.cards[c].current.location, location::SZONE);
        assert_eq!(f.cards[c].current.sequence, 5, "the Field Zone");
        assert_eq!(f.cards[c].current.position, position::FACEDOWN);
    }

    /// **No free seat refuses**, and the refusal is the count's, not the
    /// placement machinery's — nothing is asked and nothing moves.
    #[test]
    fn a_full_row_refuses_an_ordinary_spell() {
        let mut f = Field::new(8000);
        for seat in 0..5 {
            let mut c = Card::with_data(
                CardData {
                    code: 1 + seat,
                    type_: card_type::SPELL,
                    ..Default::default()
                },
                0,
            );
            c.current.controller = 0;
            let id = f.new_card(c);
            f.add_card(0, id, location::SZONE, seat, false);
        }
        let c = spell_in_hand(&mut f, card_type::SPELL);
        set(&mut f, c);
        run(&mut f, 0);
        assert!(!asked_for_a_seat(&f), "refused by the count, never asked");
        assert_eq!(f.cards[c].current.location, location::HAND);
    }

    /// **`EFFECT_CANNOT_SSET` on the card refuses**, after the seat count
    /// and before anything moves.
    #[test]
    fn a_prohibition_on_the_card_refuses() {
        let mut f = Field::new(8000);
        let c = spell_in_hand(&mut f, card_type::SPELL);
        let mut e = Effect::new(effect_type::SINGLE, code::CANNOT_SSET);
        e.owner = Some(c);
        e.handler = Some(c);
        let id = f.new_effect(e);
        f.cards[c].single_effect.insert(code::CANNOT_SSET, id);
        f.cards[c].indexer.insert(id);

        set(&mut f, c);
        run(&mut f, 0);
        assert_eq!(f.cards[c].current.location, location::HAND);
    }

    /// **The `EFFECT_SSET_COST` operation is run**, and only when the
    /// effect actually has one.
    #[test]
    fn the_set_cost_operation_is_run() {
        use std::sync::atomic::{AtomicU32, Ordering};
        static PAID: AtomicU32 = AtomicU32::new(0);

        let mut f = Field::new(8000);
        let c = spell_in_hand(&mut f, card_type::SPELL);
        let mut e = Effect::new(effect_type::SINGLE, code::SSET_COST);
        e.owner = Some(c);
        e.handler = Some(c);
        e.operation = Some(|_, _| {
            PAID.fetch_add(1, Ordering::SeqCst);
            crate::effect::Yield::Done(0)
        });
        let id = f.new_effect(e);
        f.cards[c].single_effect.insert(code::SSET_COST, id);
        f.cards[c].indexer.insert(id);

        PAID.store(0, Ordering::SeqCst);
        set(&mut f, c);
        run(&mut f, 0);
        assert_eq!(PAID.load(Ordering::SeqCst), 1, "paid exactly once");
        assert_eq!(f.cards[c].current.location, location::SZONE);
    }

    /// **The response window opens only outside a chain.** Inside one, the
    /// set happens but no `PointEvent` is emplaced.
    #[test]
    fn the_window_opens_only_outside_a_chain() {
        let mut f = Field::new(8000);
        let c = spell_in_hand(&mut f, card_type::SPELL);
        let anchor = spell_in_hand(&mut f, card_type::SPELL);
        let mut e = Effect::new(effect_type::ACTIVATE, 0);
        e.owner = Some(anchor);
        e.handler = Some(anchor);
        let id = f.new_effect(e);
        f.core
            .current_chain
            .push(crate::chain::Chain::new(id, crate::event::Event::new(0)));

        set(&mut f, c);
        run(&mut f, 0);
        assert_eq!(f.cards[c].current.location, location::SZONE, "it still set");
        assert!(
            !f.messages
                .iter()
                .any(|m| matches!(m, Message::SelectChain { .. })),
            "but nobody was offered a response"
        );
        assert_eq!(
            f.core.hint_timing[0] & timing::SSET,
            0,
            "and no timing hint was written"
        );
    }
    /// **The player-level prohibition refuses too.** `is_player_can_sset`
    /// is a separate question from the card's own `EFFECT_CANNOT_SSET`, and
    /// only this one reads an effect that names no card.
    #[test]
    fn the_player_prohibition_refuses() {
        let mut f = Field::new(8000);
        let c = spell_in_hand(&mut f, card_type::SPELL);
        let anchor = spell_in_hand(&mut f, card_type::SPELL);
        let mut e = Effect::new(effect_type::FIELD, code::CANNOT_SSET);
        e.owner = Some(anchor);
        e.handler = Some(anchor);
        e.flag[0] = crate::effect::flag::PLAYER_TARGET | crate::effect::flag::ABSOLUTE_TARGET;
        e.range = u16::from(location::HAND);
        e.s_range = 1;
        let id = f.new_effect(e);
        f.add_effect(id, 0);

        set(&mut f, c);
        run(&mut f, 0);
        assert_eq!(f.cards[c].current.location, location::HAND);
    }

    /// **`EVENT_SSET` is raised**, and something listening for it fires.
    #[test]
    fn the_set_event_is_raised() {
        let mut f = Field::new(8000);
        let c = spell_in_hand(&mut f, card_type::SPELL);
        let watcher = spell_in_hand(&mut f, card_type::TRAP);
        let watcher_code = f.cards[watcher].data.code;
        let mut e = Effect::new(
            effect_type::FIELD | effect_type::ACTIONS | effect_type::TRIGGER_O,
            code::SSET,
        );
        e.owner = Some(watcher);
        e.handler = Some(watcher);
        e.range = u16::from(location::HAND);
        let id = f.new_effect(e);
        f.add_effect(id, 0);

        set(&mut f, c);
        run(&mut f, 0);
        // A card listening for `EVENT_SSET` is offered the response window
        // the set opens. Read from the message rather than from `core`,
        // which is drained by the time the run settles.
        let offered = f.messages.iter().any(|m| match m {
            Message::SelectChain { chains, .. } => chains.iter().any(|ch| ch.code == watcher_code),
            _ => false,
        });
        assert!(offered, "the EVENT_SSET listener was offered");
    }

    /// **The cost is emplaced only when the effect has an operation.** An
    /// `EFFECT_SSET_COST` with no operation is skipped outright rather than
    /// run and found empty — the reference's guard, which is invisible from
    /// the board because an executor with nothing to run is a no-op.
    #[test]
    fn a_cost_without_an_operation_is_not_emplaced() {
        let mut f = Field::new(8000);
        let c = spell_in_hand(&mut f, card_type::SPELL);
        let mut e = Effect::new(effect_type::SINGLE, code::SSET_COST);
        e.owner = Some(c);
        e.handler = Some(c);
        let id = f.new_effect(e);
        f.cards[c].single_effect.insert(code::SSET_COST, id);
        f.cards[c].indexer.insert(id);

        f.emplace(Kind::SpellSet {
            setplayer: 0,
            toplayer: 0,
            target: c,
            reason_effect: None,
        });
        // One step, which is case 0 and nothing more.
        f.process();
        assert!(
            !f.core
                .subunits
                .iter()
                .any(|u| matches!(u.kind, Kind::ExecuteOperation { .. })),
            "nothing to run, so nothing emplaced"
        );
        assert!(
            f.core.sub_solving_event.is_empty(),
            "and no event pushed for it to consume"
        );
    }
}

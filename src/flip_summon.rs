//! The flip summon: `flip_summon` and `FlipSummon`.
//!
//! Five steps, and a machine worth reading next to [`crate::summon_rule`]
//! rather than on its own: it is the same shape with almost everything taken
//! out. No procedures, no tributes, no extra-summon permission, no
//! position choice, and **no normal-summon count** — a flip summon spends
//! the player's summon for the turn at the command layer that offers it, not
//! here.
//!
//! What survives is the part that made `SummonRule` long: the card is marked
//! `STATUS_SUMMONING`, a window is opened, and step 2 reads the status as the
//! verdict — **still set means nothing negated it**. Same polarity, same
//! trap.
//!
//! ## One condition the two machines do not share
//!
//! `SummonRule` opens its window only **outside a chain** — a summon during a
//! chain's resolution gets no window of its own, because the chain already
//! has one. `FlipSummon` has no such check: step 1 asks only whether
//! `EFFECT_CANNOT_DISABLE_FLIP_SUMMON` applies. A flip summon mid-chain is
//! negatable.
//!
//! Two machines built to the same shape and differing in one condition is
//! exactly the kind of thing a port "tidies up" by symmetry, so it is pinned
//! by a test.
//!
//! ## The card turns face-up before anyone may respond
//!
//! Step 1 flips it and *then* opens the window. A flip summon that is
//! negated has already been seen, and the card is sent to the graveyard from
//! the Monster Zone rather than being turned back down. That is the rule the
//! order encodes.
//!
//! ## Three events at the end, not one
//!
//! Step 4 raises `EVENT_FLIP`, `EVENT_FLIP_SUMMON_SUCCESS` and
//! `EVENT_CHANGE_POS` — each both as a single event and as a field one. A
//! flip summon is a flip, a summon, and a position change at once, and a card
//! that watches any of the three must see it.

use crate::board::{location, position};
use crate::card::{reason, status};
use crate::event::{code, CardId, EffectId};
use crate::field::{reset, timing, Field, Message};
use crate::processor::Kind;

/// The three events a successful flip summon raises — **once as single
/// events and again as field ones**, which is why they are a list rather
/// than six literals. The reference writes each code twice; keeping one list
/// makes it impossible for the two loops to drift apart, which is the only
/// way that repetition can go wrong.
const SUCCESS_EVENTS: [u32; 3] = [code::FLIP, code::FLIP_SUMMON_SUCCESS, code::CHANGE_POS];

/// The state `FlipSummon` carries: the costs it paid, kept so their oaths can
/// be released whether the summon succeeds or is negated.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FlipSummonState {
    pub cost_effects: Vec<EffectId>,
}

impl Field {
    /// Queue a flip summon.
    ///
    /// The reference has no `field::flip_summon`: both callers emplace the
    /// unit inline, from the idle and battle command layers. This is that
    /// emplacement given a name, so the machine can be reached before those
    /// layers exist.
    pub fn flip_summon(&mut self, sumplayer: u8, target: CardId) {
        self.emplace(Kind::FlipSummon {
            sumplayer,
            target,
            state: FlipSummonState::default(),
        });
    }

    /// One step of `FlipSummon`.
    pub(crate) fn flip_summon_step(
        &mut self,
        step: u16,
        sumplayer: u8,
        target: CardId,
        state: &mut FlipSummonState,
    ) -> bool {
        match step {
            0 => self.flip_step_0(sumplayer, target, state),
            1 => self.flip_step_1(sumplayer, target),
            2 => self.flip_step_2(sumplayer, target, state),
            3 => self.flip_step_3(target, state),
            4 => self.flip_step_4(sumplayer, target),
            _ => true,
        }
    }

    /// Step 0: may this be flip summoned, and pay for it.
    ///
    /// Three refusals, and they are the whole of the eligibility check —
    /// there is no `is_summonable_card` here, because a card that is
    /// face-down in a Monster Zone got there by being summonable in the
    /// first place.
    fn flip_step_0(&mut self, sumplayer: u8, target: CardId, state: &mut FlipSummonState) -> bool {
        if self.cards[target].current.location != location::MZONE {
            return true;
        }
        if !self.cards[target].current.is_position(position::FACEDOWN) {
            return true;
        }
        if self
            .check_unique_onfield(target, sumplayer, u16::from(location::MZONE), None)
            .is_some()
        {
            return true;
        }
        state.cost_effects = self.filter_effect(target, code::FLIPSUMMON_COST);
        for &e in &state.cost_effects.clone() {
            if self.effects.get(e).is_some_and(|x| x.operation.is_some()) {
                self.core
                    .sub_solving_event
                    .push_back(crate::event::Event::new(0));
                self.emplace(Kind::ExecuteOperation {
                    resume: None,
                    effect: e,
                    player: sumplayer,
                    subject: None,
                    args: Vec::new(),
                    was_disabled: false,
                });
            }
        }
        false
    }

    /// Step 1: turn it face-up, announce it, and open the window.
    ///
    /// The **new `fieldid`** is the part that is easy to leave out and hard
    /// to notice missing. It is a creation counter, and every ordering of
    /// simultaneous effects reads it — so a card that flips face-up sorts
    /// *after* everything that was already face-up, from now on.
    ///
    /// `EFFECT_CANNOT_DISABLE_FLIP_SUMMON` sends the machine to step 3, past
    /// the window and past the step that reads its verdict. It is the
    /// **only** thing that does: unlike `SummonRule`, a chain in progress
    /// does not close the window here.
    ///
    /// Nothing touches `unique_fieldid`, where `ChangePos` does when it turns
    /// a card face-up. The uniqueness question was asked at step 0 instead,
    /// and asked as a refusal rather than as a cleanup.
    fn flip_step_1(&mut self, sumplayer: u8, target: CardId) -> bool {
        self.cards[target].previous.position = self.cards[target].current.position;
        self.cards[target].current.position = position::FACEUP_ATTACK;
        self.cards[target].summon.player = sumplayer;
        self.cards[target].fieldid = self.next_field_id_raw();
        self.core.phase_action = true;
        if self.is_flag(crate::duel::flags::CANNOT_SUMMON_OATH_OLD) {
            self.bump_flipsummon_counter(sumplayer, target);
        }

        let info = self.get_info_location(target);
        let code_ = self.cards[target].data.code;
        self.messages.push(Message::FlipSummoning {
            code: code_,
            controller: info.controller,
            location: info.location,
            sequence: info.sequence,
            position: info.position,
        });

        if self
            .is_affected_by_effect(target, code::CANNOT_DISABLE_FLIP_SUMMON)
            .is_some()
        {
            self.set_step(2);
            return false;
        }
        self.cards[target].set_status(status::SUMMONING, true);
        self.cards[target].set_status(status::SUMMON_DISABLED, false);
        self.raise_event(
            Some(target),
            code::FLIP_SUMMON,
            None,
            0,
            sumplayer,
            sumplayer,
            0,
        );
        self.process_instant_event();
        // **The negation window skips everything** — `PointEvent(true, true,
        // true)` in the reference (`FlipSummon` case 1, `operations.cpp`): no triggers, no free chains, no new
        // chains. Only an effect that answers the summon event itself (a
        // negation) may act here; a Quick-Play or Trap the player could
        // activate "anyway" waits for the success window. The port's
        // default here offered Enemy Controller before Man-Eater Bug's flip
        // trigger existed (fuzz seed 17) and Ring of Destruction before
        // Morphing Jar's (seed 21).
        self.emplace(Kind::PointEvent {
            skip: crate::point_event::PointEventSkip {
                trigger: true,
                freechain: true,
                new: true,
            },
        });
        false
    }

    fn bump_flipsummon_counter(&mut self, sumplayer: u8, target: CardId) {
        self.core.flipsummon_state_count[sumplayer as usize] += 1;
        self.check_card_counter(
            target,
            crate::summon_support::activity::FLIPSUMMON,
            sumplayer,
        );
    }

    /// Step 2: was it negated?
    ///
    /// `STATUS_SUMMONING` **still set** means nothing took the summon away,
    /// and the machine goes on to step 3. Falling past that test means it was
    /// negated: the oaths are released and the card goes to the graveyard
    /// face-up, from the Monster Zone it is already in.
    fn flip_step_2(&mut self, sumplayer: u8, target: CardId, state: &mut FlipSummonState) -> bool {
        if self.cards[target].is_status(status::SUMMONING) {
            return false;
        }
        for &e in &state.cost_effects.clone() {
            self.remove_oath_effect(e);
        }
        if self.cards[target].current.location == location::MZONE {
            self.send_to_card(
                target,
                None,
                reason::RULE,
                sumplayer,
                sumplayer,
                u16::from(location::GRAVE),
                0,
                0,
                false,
            );
        }
        self.emplace(Kind::PointEvent {
            skip: crate::point_event::PointEventSkip::default(),
        });
        true
    }

    /// Step 3: it stuck. Release the oaths and let the card work.
    ///
    /// **`adjust_instant` is not called here**, where `SummonRule`'s
    /// equivalent step is followed by one that does. That is not an omission
    /// to correct: step 4 calls it, after `MSG_FLIPSUMMONED`.
    fn flip_step_3(&mut self, target: CardId, state: &mut FlipSummonState) -> bool {
        for &e in &state.cost_effects.clone() {
            self.release_oath_relation(e);
        }
        self.cards[target].set_status(status::SUMMONING, false);
        self.enable_field_effect(target, true);
        if self.cards[target].is_status(status::DISABLED) {
            self.reset_card(target, reset::DISABLE, reset::EVENT);
        }
        self.cards[target].set_status(status::FLIP_SUMMON_TURN, true);
        false
    }

    /// Step 4: the three events.
    ///
    /// `adjust_all` runs **unconditionally**, where the `PointEvent` and the
    /// timing are charged only outside a chain. `SummonRule` puts the
    /// `adjust_all` inside that condition; this one does not, and the
    /// difference is the reference's.
    fn flip_step_4(&mut self, sumplayer: u8, target: CardId) -> bool {
        self.messages.push(Message::FlipSummoned);
        if !self.is_flag(crate::duel::flags::CANNOT_SUMMON_OATH_OLD) {
            self.bump_flipsummon_counter(sumplayer, target);
        }
        self.adjust_instant();
        for event in SUCCESS_EVENTS {
            self.raise_single_event(target, vec![], event, None, 0, sumplayer, sumplayer, 0);
        }
        self.process_single_event();
        for event in SUCCESS_EVENTS {
            self.raise_event(Some(target), event, None, 0, sumplayer, sumplayer, 0);
        }
        self.process_instant_event();
        self.adjust_all();
        if self.core.current_chain.is_empty() {
            self.core.hint_timing[sumplayer as usize] |= timing::FLIPSUMMON;
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

    /// **The negation window skips everything** — `FlipSummon` case 1 in
    /// the reference emplaces `PointEvent(true, true, true)`: only an
    /// effect answering `EVENT_FLIP_SUMMON` itself may act, and a
    /// Quick-Play the player could activate "anyway" waits for the
    /// success window. Fuzz seed 17 found Enemy Controller offered here,
    /// before Man-Eater Bug's flip trigger existed.
    #[test]
    fn the_negation_window_skips_triggers_free_chains_and_new_chains() {
        let mut f = Field::new(8000);
        f.infos.turn_player = 0;
        f.infos.phase = crate::duel::phases::MAIN1;
        let target = face_down(&mut f, 0);
        f.flip_step_1(0, target);
        let skip = queued_point_event_skip(&f);
        assert!(skip.trigger && skip.freechain && skip.new, "{skip:?}");
    }

    fn queued_point_event_skip(f: &Field) -> crate::point_event::PointEventSkip {
        f.core
            .subunits
            .iter()
            .find_map(|u| match u.kind {
                crate::processor::Kind::PointEvent { skip } => Some(skip),
                _ => None,
            })
            .expect("a PointEvent was queued")
    }
    use crate::card::{card_type, Card, CardData};
    use crate::effect::{effect_type, Effect};
    use crate::processor::Status;

    /// A face-down monster in a Monster Zone.
    fn face_down(f: &mut Field, seat: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 31560081,
                type_: card_type::MONSTER,
                level: 1,
                ..Default::default()
            },
            0,
        );
        c.current.controller = 0;
        let id = f.new_card(c);
        f.add_card(0, id, location::MZONE, seat, false);
        f.cards[id].current.position = position::FACEDOWN_DEFENSE;
        assert_eq!(f.cards[id].current.location, location::MZONE);
        id
    }

    fn single(f: &mut Field, card: CardId, code_: u32) -> EffectId {
        let mut e = Effect::new(effect_type::SINGLE, code_);
        e.owner = Some(card);
        e.handler = Some(card);
        let id = f.new_effect(e);
        f.cards[card].single_effect.insert(code_, id);
        f.cards[card].indexer.insert(id);
        id
    }

    /// Step the machine, declining the response windows, and show each state
    /// to `watch` before it is stepped.
    fn drive(f: &mut Field, mut watch: impl FnMut(&Field)) -> Status {
        for _ in 0..4096 {
            watch(f);
            match f.process() {
                Status::Continue => continue,
                Status::Awaiting => match f.messages.last() {
                    Some(Message::SelectChain { .. }) => f.core.returns.set(-1),
                    // One offer with nothing on the chain is asked as a
                    // yes/no about the card rather than as a list.
                    Some(Message::SelectEffectYesNo { .. }) => f.core.returns.set(0),
                    _ => return Status::Awaiting,
                },
                other => return other,
            }
        }
        panic!("did not settle");
    }

    fn run(f: &mut Field) -> Status {
        drive(f, |_| {})
    }

    /// Run, negating the flip summon at the step that reads the verdict.
    ///
    /// Clearing `STATUS_SUMMONING` is what a negating effect does; waiting
    /// for `subunits` to be empty puts it after the response window rather
    /// than before it.
    fn run_negating(f: &mut Field, card: CardId) -> Status {
        for _ in 0..4096 {
            let at_verdict = f
                .queue()
                .next()
                .is_some_and(|u| matches!(u.kind, Kind::FlipSummon { .. }) && u.step == 2)
                && f.core.subunits.is_empty();
            if at_verdict {
                f.cards[card].set_status(status::SUMMONING, false);
                f.cards[card].set_status(status::SUMMON_DISABLED, true);
            }
            match f.process() {
                Status::Continue => continue,
                Status::Awaiting => match f.messages.last() {
                    Some(Message::SelectChain { .. }) => f.core.returns.set(-1),
                    _ => return Status::Awaiting,
                },
                other => return other,
            }
        }
        panic!("did not settle");
    }

    #[test]
    fn a_face_down_monster_is_turned_face_up() {
        let mut f = Field::new(8000);
        let c = face_down(&mut f, 0);
        // A stale value from an earlier summon, so that recording the new
        // one is a change rather than a default.
        f.cards[c].summon.player = 1;
        assert!(
            !f.cards[c].is_status(status::EFFECT_ENABLED),
            "a face-down card applies nothing"
        );
        f.flip_summon(0, c);
        assert_eq!(run(&mut f), Status::End);
        assert!(
            f.cards[c].is_status(status::EFFECT_ENABLED),
            "and a face-up one does"
        );
        assert_eq!(f.cards[c].current.position, position::FACEUP_ATTACK);
        assert_eq!(
            f.cards[c].previous.position,
            position::FACEDOWN_DEFENSE,
            "where it came from is recorded"
        );
        assert!(f.cards[c].is_status(status::FLIP_SUMMON_TURN));
        assert_eq!(f.cards[c].summon.player, 0);
        assert!(f.core.phase_action);
    }

    /// **The card gets a new `fieldid`.** It is a creation counter and every
    /// ordering of simultaneous effects reads it, so a card that flips
    /// face-up sorts after everything already face-up.
    #[test]
    fn turning_face_up_renews_the_field_id() {
        let mut f = Field::new(8000);
        let c = face_down(&mut f, 0);
        let other = face_down(&mut f, 1);
        f.cards[other].current.position = position::FACEUP_ATTACK;
        f.cards[other].fieldid = f.next_field_id_raw();
        let before = f.cards[c].fieldid;

        f.flip_summon(0, c);
        run(&mut f);
        assert_ne!(f.cards[c].fieldid, before);
        assert!(
            f.cards[c].fieldid > f.cards[other].fieldid,
            "it sorts after the card that was already face-up"
        );
    }

    /// The list both loops read. A flip summon is a flip, a summon and a
    /// position change at once, and the *field* half of that is asserted
    /// below; there is no way for the single half to say anything different,
    /// because it is the same list.
    #[test]
    fn the_three_events_are_the_three_a_flip_summon_is() {
        assert_eq!(
            SUCCESS_EVENTS,
            [code::FLIP, code::FLIP_SUMMON_SUCCESS, code::CHANGE_POS]
        );
    }

    /// A flip summon is a flip, a summon and a position change at once.
    #[test]
    fn all_three_events_are_raised() {
        let mut f = Field::new(8000);
        let c = face_down(&mut f, 0);
        f.flip_summon(0, c);
        run(&mut f);
        for want in [code::FLIP, code::FLIP_SUMMON_SUCCESS, code::CHANGE_POS] {
            assert!(
                f.core
                    .instant_event
                    .iter()
                    .chain(f.core.used_event.iter())
                    .any(|e| e.event_code == want),
                "event {want} was raised"
            );
        }
    }

    #[test]
    fn the_messages_bracket_the_summon() {
        let mut f = Field::new(8000);
        let c = face_down(&mut f, 0);
        f.flip_summon(0, c);
        run(&mut f);
        let announced = f
            .messages
            .iter()
            .position(|m| matches!(m, Message::FlipSummoning { .. }));
        let confirmed = f
            .messages
            .iter()
            .position(|m| matches!(m, Message::FlipSummoned));
        assert!(announced.is_some() && confirmed.is_some());
        assert!(announced < confirmed, "announced before it is confirmed");
    }

    /// The timing is charged at step 4 and spent by the window it then
    /// opens, so it is only visible from inside the run.
    #[test]
    fn the_flip_summon_timing_is_charged() {
        let mut f = Field::new(8000);
        let c = face_down(&mut f, 0);
        f.flip_summon(0, c);
        let mut charged = false;
        drive(&mut f, |f| {
            charged |= f.core.hint_timing[0] & timing::FLIPSUMMON != 0;
        });
        assert!(charged);
    }

    /// **A flip summon is not an `ACTIVITY_SUMMON`.** It bumps its own tally
    /// and no other — the distinction between "was anything summoned" and
    /// "was a monster put onto the field".
    #[test]
    fn only_the_flip_summon_tally_is_bumped() {
        let mut f = Field::new(8000);
        let c = face_down(&mut f, 0);
        f.flip_summon(0, c);
        run(&mut f);
        assert_eq!(f.core.flipsummon_state_count[0], 1);
        assert_eq!(f.core.flipsummon_state_count[1], 0);
        assert_eq!(f.core.summon_state_count[0], 0);
        assert_eq!(f.core.normalsummon_state_count[0], 0);
        assert_eq!(
            f.core.summon_count[0], 0,
            "and the normal summon is spent by the command layer, not here"
        );
    }

    /// The old duel option moves the tally from success to attempt, and does
    /// not add a second bump.
    #[test]
    fn the_old_oath_option_bumps_the_tally_once() {
        let mut f = Field::with_flags(
            8000,
            crate::duel::REFERENCE_CONFIGURATION | crate::duel::flags::CANNOT_SUMMON_OATH_OLD,
        );
        let c = face_down(&mut f, 0);
        f.flip_summon(0, c);
        run(&mut f);
        assert_eq!(f.cards[c].current.position, position::FACEUP_ATTACK);
        assert_eq!(f.core.flipsummon_state_count[0], 1);
    }

    /// **The single events reach the card's own effects.**
    ///
    /// The field half of step 4 is visible in the event lists; the single
    /// half is not — a single event with nothing listening is dropped without
    /// trace. So this one puts an optional trigger on the flipped card keyed
    /// to `EVENT_FLIP_SUMMON_SUCCESS` and looks for the offer it becomes.
    ///
    /// A `SINGLE | TRIGGER_O` effect is registered on the card rather than in
    /// the field's trigger index, so only `process_single_event` can find it
    /// — which is what makes this a test of the single half and not of both.
    #[test]
    fn the_single_events_reach_the_cards_own_effects() {
        let mut f = Field::new(8000);
        let c = face_down(&mut f, 0);
        let mut e = Effect::new(
            effect_type::SINGLE | effect_type::ACTIONS | effect_type::TRIGGER_O,
            code::FLIP_SUMMON_SUCCESS,
        );
        e.owner = Some(c);
        e.handler = Some(c);
        e.range = u16::from(location::MZONE);
        let id = f.new_effect(e);
        f.cards[c]
            .single_effect
            .insert(code::FLIP_SUMMON_SUCCESS, id);
        f.cards[c].indexer.insert(id);

        f.flip_summon(0, c);
        run(&mut f);
        let offered = f.messages.iter().any(|m| match m {
            Message::SelectEffectYesNo { code, .. } => *code == 31560081,
            Message::SelectChain { chains, .. } => !chains.is_empty(),
            _ => false,
        });
        assert!(offered, "the trigger was offered");
    }

    mod refusals {
        use super::*;

        /// Only from a Monster Zone.
        #[test]
        fn a_card_elsewhere_is_refused() {
            let mut f = Field::new(8000);
            let c = face_down(&mut f, 0);
            f.remove_card(c);
            f.add_card(0, c, location::GRAVE, 0, false);
            f.flip_summon(0, c);
            assert_eq!(run(&mut f), Status::End);
            assert_eq!(f.core.flipsummon_state_count[0], 0);
            assert!(!f.cards[c].is_status(status::FLIP_SUMMON_TURN));
        }

        /// And only face-down — a face-up monster has nothing to flip.
        #[test]
        fn a_face_up_monster_is_refused() {
            let mut f = Field::new(8000);
            let c = face_down(&mut f, 0);
            f.cards[c].current.position = position::FACEUP_DEFENSE;
            f.flip_summon(0, c);
            run(&mut f);
            assert_eq!(f.core.flipsummon_state_count[0], 0);
            assert_eq!(
                f.cards[c].current.position,
                position::FACEUP_DEFENSE,
                "and it is not turned to attack"
            );
        }
    }

    mod negation {
        use super::*;

        /// **The card is already face-up when it is negated.** It is sent to
        /// the graveyard rather than turned back down — the order of step 1
        /// is what decides that.
        #[test]
        fn a_negated_flip_summon_sends_the_card_to_the_graveyard() {
            let mut f = Field::new(8000);
            let c = face_down(&mut f, 0);
            f.flip_summon(0, c);
            run_negating(&mut f, c);
            assert_eq!(f.cards[c].current.location, location::GRAVE);
            assert!(!f.cards[c].is_status(status::FLIP_SUMMON_TURN));
            assert_eq!(
                f.core.flipsummon_state_count[0], 0,
                "the tally counts successes"
            );
        }

        /// `EFFECT_CANNOT_DISABLE_FLIP_SUMMON` skips the window entirely.
        #[test]
        fn a_flip_summon_that_cannot_be_disabled_opens_no_window() {
            let mut f = Field::new(8000);
            let c = face_down(&mut f, 0);
            single(&mut f, c, code::CANNOT_DISABLE_FLIP_SUMMON);
            f.flip_summon(0, c);
            run(&mut f);
            assert_eq!(f.cards[c].current.position, position::FACEUP_ATTACK);
            assert!(!f.cards[c].is_status(status::SUMMONING));
            assert!(
                !f.core
                    .instant_event
                    .iter()
                    .chain(f.core.used_event.iter())
                    .any(|e| e.event_code == code::FLIP_SUMMON),
                "the window's event was never raised"
            );
            assert_eq!(f.core.flipsummon_state_count[0], 1, "it still succeeded");
        }

        /// The contrast: an ordinary flip summon does open one.
        #[test]
        fn an_ordinary_flip_summon_opens_the_window() {
            let mut f = Field::new(8000);
            let c = face_down(&mut f, 0);
            f.flip_summon(0, c);
            run(&mut f);
            assert!(f
                .core
                .instant_event
                .iter()
                .chain(f.core.used_event.iter())
                .any(|e| e.event_code == code::FLIP_SUMMON));
        }

        /// **A flip summon during a chain's resolution still opens its
        /// window**, where a normal summon does not.
        ///
        /// `SummonRule`'s step 13 checks `current_chain` and skips the
        /// window when a chain is resolving; `FlipSummon`'s step 1 checks
        /// only `EFFECT_CANNOT_DISABLE_FLIP_SUMMON`. Two machines built to
        /// the same shape, differing in one condition — and this is the
        /// direction that is easy to "fix" by symmetry.
        ///
        /// The run stops at `SolveChain`, which the fabricated chain link
        /// reaches and which is not fully ported; everything asserted here
        /// happens at step 1, long before.
        #[test]
        fn a_flip_summon_inside_a_chain_still_opens_its_window() {
            let mut f = Field::new(8000);
            let c = face_down(&mut f, 0);
            let anchor = face_down(&mut f, 1);
            f.cards[anchor].current.position = position::FACEUP_ATTACK;
            let e = single(&mut f, anchor, code::UPDATE_ATTACK);
            let mut chain = crate::chain::Chain::new(e, crate::event::Event::new(0));
            chain.triggering_player = 0;
            f.core.current_chain.push(chain);

            f.flip_summon(0, c);
            for _ in 0..64 {
                if f.queue()
                    .next()
                    .is_some_and(|u| matches!(u.kind, Kind::SolveChain { .. }))
                {
                    break;
                }
                if f.process() != Status::Continue {
                    break;
                }
            }
            assert_eq!(f.cards[c].current.position, position::FACEUP_ATTACK);
            assert!(
                f.core
                    .instant_event
                    .iter()
                    .chain(f.core.point_event.iter())
                    .chain(f.core.used_event.iter())
                    .any(|e| e.event_code == code::FLIP_SUMMON),
                "the window was opened even though a chain is resolving"
            );
        }

        /// **Step 4's timing is charged only outside a chain**, even though
        /// step 1's window is not conditioned that way.
        ///
        /// Reaching step 4 with a chain in progress needs the window skipped
        /// — otherwise the `PointEvent` at step 1 goes on to resolve the
        /// chain, which is not ported — so the card is made undisableable.
        /// That is the one combination in which the guard is observable.
        #[test]
        fn a_flip_summon_inside_a_chain_charges_no_timing() {
            let mut f = Field::new(8000);
            let c = face_down(&mut f, 0);
            single(&mut f, c, code::CANNOT_DISABLE_FLIP_SUMMON);
            let anchor = face_down(&mut f, 1);
            f.cards[anchor].current.position = position::FACEUP_ATTACK;
            let e = single(&mut f, anchor, code::UPDATE_ATTACK);
            let mut chain = crate::chain::Chain::new(e, crate::event::Event::new(0));
            chain.triggering_player = 0;
            f.core.current_chain.push(chain);

            f.flip_summon(0, c);
            let mut charged = false;
            drive(&mut f, |f| {
                charged |= f.core.hint_timing[0] & timing::FLIPSUMMON != 0;
            });
            assert_eq!(f.cards[c].current.position, position::FACEUP_ATTACK);
            assert_eq!(f.core.flipsummon_state_count[0], 1, "it reached step 4");
            assert!(!charged, "and charged no timing there");
        }
    }

    mod costs {
        use super::*;

        /// `EFFECT_FLIPSUMMON_COST`'s operation runs before anything else
        /// happens.
        #[test]
        fn the_cost_is_paid_first() {
            let mut f = Field::new(8000);
            let c = face_down(&mut f, 0);
            let cost = single(&mut f, c, code::FLIPSUMMON_COST);
            if let Some(e) = f.effects.get_mut(cost) {
                e.operation = Some(|f, _| {
                    // Something observable, done from inside the cost.
                    f.players[0].lp -= 500;
                    crate::effect::Yield::Done(0)
                });
            }
            f.flip_summon(0, c);
            run(&mut f);
            assert_eq!(f.players[0].lp, 7500, "the cost was paid");
            assert_eq!(f.cards[c].current.position, position::FACEUP_ATTACK);
        }
    }
}

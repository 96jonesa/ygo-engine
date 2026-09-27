//! The spirit procedure — `proc_spirit.lua`.
//!
//! A spirit monster goes back to its owner's hand during the End Phase of
//! the turn it arrived. `Spirit.AddProcedure` is what makes that happen,
//! and like `proc_equip` it lives under `cards/` because it is script: the
//! reference implements it in Lua on top of the exports a card uses.
//!
//! ## It is a flag, not a memory
//!
//! The procedure does not remember when the monster arrived. Each event a
//! card names — a summon, a flip — registers a **flag effect**, and the
//! End Phase trigger fires only if the flag is there:
//!
//! ```lua
//! fe1:SetOperation(function(e) e:GetHandler():RegisterFlagEffect(
//!     FLAG_SPIRIT_RETURN, RESETS_STANDARD_PHASE_END, 0, 1) end)
//! ```
//!
//! `RESETS_STANDARD_PHASE_END` is the library composite — every standard
//! reset, plus the end of the phase. So the flag clears itself if the
//! monster leaves, *and* at the end of the turn whether or not the trigger
//! ran. A monster that arrives and survives goes back; one that was
//! already there does not, because nothing set its flag.
//!
//! ## Two triggers for one return
//!
//! `e1` is **mandatory** and `e2` is the same effect cloned into an
//! **optional** one, and which fires is decided by a condition rather than
//! by the card:
//!
//! - both want the flag, and neither fires under `EFFECT_SPIRIT_DONOT_RETURN`;
//! - `e1` additionally wants *no* `EFFECT_SPIRIT_MAYNOT_RETURN`;
//! - `e2` wants exactly that effect to be present.
//!
//! So the pair is exhaustive and exclusive: with the flag and no
//! prohibition, precisely one of them applies. Nothing in this pool
//! registers either code, so `e2` is the branch that cannot fire here —
//! and it is transcribed rather than dropped, because the pair is the
//! mechanism.
//!
//! The optional one also asks `IsAbleToHand` where the mandatory one does
//! not, which reads backwards until you see what it is for: a mandatory
//! return that cannot happen still *tries*, and the attempt is what the
//! rules want; an optional one is not offered when it would do nothing.
//!
//! ## The target resets the flag, not the operation
//!
//! `ResetFlagEffect` is called in the **target**, before the return is
//! announced — so a chain that never resolves has still spent the flag.
//! That is the reference's ordering and it is load-bearing: the flag is a
//! "this turn" marker, not a promise that the return happened.

use crate::board::location;
use crate::card::reason;
use crate::effect::{effect_type, flag, Ctx, Yield};
use crate::event::{category, code, CardId};
use crate::field::{resets, Field};
use crate::script_api as api;

/// `FLAG_SPIRIT_RETURN` — the marker's id, as `proc_spirit.lua` names it.
pub const FLAG_SPIRIT_RETURN: u32 = 2;

/// The description the End Phase trigger carries — `e1:SetDescription(1105)`.
pub const RETURN_DESCRIPTION: u64 = 1105;

/// `Spirit.AddProcedure(c, ...)` — the End Phase return, plus one
/// flag-setting effect per event the card names.
pub fn add_procedure(f: &mut Field, c: CardId, events: &[u32]) {
    // Return this card to the hand during the End Phase
    let e1 = api::create_effect(f, c);
    api::set_description(f, e1, RETURN_DESCRIPTION);
    api::set_category(f, e1, category::TOHAND);
    api::set_type(f, e1, effect_type::FIELD | effect_type::TRIGGER_F);
    api::set_code(f, e1, code::PHASE | u32::from(crate::duel::phases::END));
    api::set_range(f, e1, u16::from(location::MZONE));
    api::set_condition(f, e1, mandatory_return_condition);
    api::set_target(f, e1, mandatory_return_target);
    api::set_operation(f, e1, return_operation);
    api::register_effect(f, c, e1, false);
    // Optional return in case of "SPIRIT_MAYNOT_RETURN" effects
    let e2 = api::clone_effect(f, e1);
    api::set_type(f, e2, effect_type::FIELD | effect_type::TRIGGER_O);
    api::set_condition(f, e2, optional_return_condition);
    api::set_target(f, e2, optional_return_target);
    api::register_effect(f, c, e2, false);
    // Effects that register the flags
    for &event in events {
        let fe = api::create_effect(f, c);
        api::set_type(f, fe, effect_type::SINGLE | effect_type::CONTINUOUS);
        api::set_property(f, fe, flag::CANNOT_DISABLE, 0);
        api::set_code(f, fe, event);
        api::set_operation(f, fe, set_return_flag);
        api::register_effect(f, c, fe, false);
    }
}

/// The per-event operation: mark the card as having arrived this turn.
fn set_return_flag(f: &mut Field, ctx: &Ctx) -> Yield {
    let Some(c) = api::get_handler(f, ctx.reason_effect) else {
        return api::done();
    };
    api::register_flag_effect(
        f,
        c,
        FLAG_SPIRIT_RETURN,
        resets::STANDARD_PHASE_END,
        0,
        1,
        0,
    );
    api::done()
}

/// `Spirit.CommonCondition` — the flag is there and nothing forbids the
/// return outright.
fn common_condition(f: &mut Field, c: CardId) -> bool {
    api::has_flag_effect(f, c, FLAG_SPIRIT_RETURN)
        && !api::is_has_effect(f, c, code::SPIRIT_DONOT_RETURN)
}

fn mandatory_return_condition(f: &mut Field, ctx: &Ctx) -> bool {
    let Some(c) = api::get_handler(f, ctx.reason_effect) else {
        return false;
    };
    common_condition(f, c) && !api::is_has_effect(f, c, code::SPIRIT_MAYNOT_RETURN)
}

fn optional_return_condition(f: &mut Field, ctx: &Ctx) -> bool {
    let Some(c) = api::get_handler(f, ctx.reason_effect) else {
        return false;
    };
    common_condition(f, c) && api::is_has_effect(f, c, code::SPIRIT_MAYNOT_RETURN)
}

fn mandatory_return_target(f: &mut Field, ctx: &Ctx, chk: bool, _chkc: Option<CardId>) -> Yield {
    if !chk {
        // Mandatory: it tries whether or not it can succeed.
        return api::yes(true);
    }
    announce_return(f, ctx);
    api::yes(true)
}

fn optional_return_target(f: &mut Field, ctx: &Ctx, chk: bool, _chkc: Option<CardId>) -> Yield {
    let Some(c) = api::get_handler(f, ctx.reason_effect) else {
        return api::yes(false);
    };
    if !chk {
        // Optional: not offered when it would do nothing.
        return api::yes(api::is_able_to_hand(f, c, None));
    }
    announce_return(f, ctx);
    api::yes(true)
}

/// The half both targets share: spend the flag, then announce.
fn announce_return(f: &mut Field, ctx: &Ctx) {
    let Some(c) = api::get_handler(f, ctx.reason_effect) else {
        return;
    };
    api::reset_flag_effect(f, c, FLAG_SPIRIT_RETURN);
    api::set_operation_info(f, 0, category::TOHAND, Some(vec![c]), 1, 0, 0);
}

fn return_operation(f: &mut Field, ctx: &Ctx) -> Yield {
    let e = ctx.reason_effect;
    let Some(c) = api::get_handler(f, e) else {
        return api::done();
    };
    if api::is_relate_to_effect(f, c, e) {
        api::send_to_hand(f, vec![c], None, reason::EFFECT);
        // `Duel.SendtoHand` **yields**: the operation is not over until
        // the card has arrived, and that is when the hand-shuffle flag
        // the arrival raises is consumed — at the operation's end, with
        // `check_level` back at zero. Returning at once consumed nothing
        // and the returned Spirit's hand was never shuffled (fuzz seed
        // 13).
        return api::suspend(|_f, _ctx| api::done());
    }
    api::done()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::position;
    use crate::card::{card_type, status, Card, CardData};
    use crate::chain::Chain;
    use crate::effect::Effect;
    use crate::event::{EffectId, Event};
    use crate::processor::Status;

    /// A synthetic spirit built by the procedure alone, so the tests are
    /// about the procedure and not about any card's extras.
    fn spirit(events: &[u32]) -> (Field, CardId) {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        f.infos.turn_player = 0;
        f.infos.phase = crate::duel::phases::END;
        let mut d = Card::with_data(
            CardData {
                code: 999_100,
                type_: card_type::MONSTER | card_type::EFFECT | card_type::SPIRIT,
                level: 4,
                attack: 1000,
                defense: 1000,
                ..Default::default()
            },
            0,
        );
        d.current.controller = 0;
        d.set_status(status::EFFECT_ENABLED, true);
        let c = f.new_card(d);
        f.add_card(0, c, location::MZONE, 0, false);
        f.cards[c].current.position = position::FACEUP_ATTACK;
        f.cards[c].set_status(status::INITIALIZING, true);
        add_procedure(&mut f, c, events);
        f.cards[c].set_status(status::INITIALIZING, false);
        f.apply_field_effect(c);
        (f, c)
    }

    const END: u32 = code::PHASE | (crate::duel::phases::END as u32);

    fn returns(f: &Field, c: CardId) -> (EffectId, EffectId) {
        let ids = f.cards[c].field_effect.equal_range(END);
        assert_eq!(ids.len(), 2, "a mandatory return and its optional twin");
        let mandatory = ids
            .iter()
            .copied()
            .find(|&e| f.effects.get(e).unwrap().is_type(effect_type::TRIGGER_F))
            .expect("a forced trigger");
        let optional = ids
            .iter()
            .copied()
            .find(|&e| f.effects.get(e).unwrap().is_type(effect_type::TRIGGER_O))
            .expect("an optional trigger");
        (mandatory, optional)
    }

    fn ctx_for<'a>(e: EffectId, ev: &'a Event, tp: u8) -> Ctx<'a> {
        Ctx {
            reason_effect: e,
            player: tp,
            event: ev,
            card: None,
            args: &[],
        }
    }

    /// Run the flag-setter for `event`.
    fn arrive(f: &mut Field, c: CardId, event: u32) {
        let e = f.cards[c].single_effect.equal_range(event)[0];
        let ev = Event::new(event);
        set_return_flag(f, &ctx_for(e, &ev, 0));
    }

    /// Give the card a single effect with `code_`, to stand in for a
    /// `SPIRIT_*_RETURN` prohibition.
    fn under(f: &mut Field, c: CardId, code_: u32) {
        let mut e = Effect::new(effect_type::SINGLE, code_);
        e.owner = Some(c);
        e.handler = Some(c);
        let id = f.new_effect(e);
        f.cards[c].single_effect.insert(code_, id);
        f.cards[c].indexer.insert(id);
    }

    /// **The procedure registers a mandatory return, an optional twin,
    /// and one flag-setter per event named.**
    #[test]
    fn it_registers_two_returns_and_a_setter_for_each_event() {
        let (f, c) = spirit(&[code::SUMMON_SUCCESS, code::FLIP]);
        let (mandatory, optional) = returns(&f, c);
        let m = f.effects.get(mandatory).unwrap();
        assert!(m.is_type(effect_type::FIELD));
        assert_eq!(m.category, category::TOHAND);
        assert_eq!(m.range, u16::from(location::MZONE));
        assert_eq!(m.description, RETURN_DESCRIPTION);
        // The twin is a *clone*, so it carries the description and the
        // category across without either being set again.
        let o = f.effects.get(optional).unwrap();
        assert_eq!(o.description, RETURN_DESCRIPTION);
        assert_eq!(o.category, category::TOHAND);
        assert_eq!(o.range, u16::from(location::MZONE));

        for event in [code::SUMMON_SUCCESS, code::FLIP] {
            let ids = f.cards[c].single_effect.equal_range(event);
            assert_eq!(ids.len(), 1, "one setter for {event}");
            let fe = f.effects.get(ids[0]).unwrap();
            assert!(fe.is_type(effect_type::CONTINUOUS));
            assert!(fe.is_flag(flag::CANNOT_DISABLE));
        }

        // No events named, no setters — and the returns are still there.
        let (f, c) = spirit(&[]);
        assert_eq!(f.cards[c].field_effect.equal_range(END).len(), 2);
        assert!(f.cards[c]
            .single_effect
            .equal_range(code::SUMMON_SUCCESS)
            .is_empty());
    }

    /// **The flag is what the return reads**, and it is set by the event
    /// rather than remembered by the procedure.
    #[test]
    fn the_return_wants_the_flag() {
        let (mut f, c) = spirit(&[code::SUMMON_SUCCESS]);
        let (mandatory, _) = returns(&f, c);
        let ev = Event::new(END);
        assert!(
            !mandatory_return_condition(&mut f, &ctx_for(mandatory, &ev, 0)),
            "a spirit that did not arrive this turn stays"
        );
        arrive(&mut f, c, code::SUMMON_SUCCESS);
        assert!(mandatory_return_condition(
            &mut f,
            &ctx_for(mandatory, &ev, 0)
        ));
    }

    /// **The flag clears itself at the end of the phase and when the card
    /// leaves** — `RESETS_STANDARD_PHASE_END`, which is both.
    #[test]
    fn the_flag_is_scoped_to_the_turn_and_to_the_field() {
        // The id as a literal: `proc_spirit.lua` says `FLAG_SPIRIT_RETURN=2`,
        // and an assertion written against the constant agrees with
        // whatever the constant is.
        assert_eq!(FLAG_SPIRIT_RETURN, 2);
        let (mut f, c) = spirit(&[code::SUMMON_SUCCESS]);
        arrive(&mut f, c, code::SUMMON_SUCCESS);
        let id = f.cards[c]
            .single_effect
            .equal_range((FLAG_SPIRIT_RETURN & 0x0fff_ffff) | api::FLAG_EFFECT)[0];
        let e = f.effects.get(id).unwrap();
        assert_eq!(
            e.reset_count, 1,
            "one end phase is all it lasts — a count of two would carry \
             the flag into the next turn"
        );
        let reset = e.reset_flag;
        assert_ne!(reset & resets::STANDARD, 0, "every standard reset");
        assert_ne!(reset & crate::field::reset::PHASE, 0);
        assert_ne!(
            reset & u32::from(crate::duel::phases::END),
            0,
            "and the end of the phase"
        );
        // Leaving the field takes it.
        f.move_card(0, c, location::GRAVE, 0, false);
        f.reset_card(c, crate::field::reset::TOGRAVE, crate::field::reset::EVENT);
        assert!(!api::has_flag_effect(&f, c, FLAG_SPIRIT_RETURN));
    }

    /// **The two branches are exhaustive and exclusive.** With the flag
    /// and no prohibition exactly one applies, `DONOT_RETURN` silences
    /// both, and `MAYNOT_RETURN` swaps which.
    #[test]
    fn exactly_one_branch_applies() {
        for (donot, maynot, want_mandatory, want_optional) in [
            (false, false, true, false),
            (false, true, false, true),
            (true, false, false, false),
            (true, true, false, false),
        ] {
            let (mut f, c) = spirit(&[code::SUMMON_SUCCESS]);
            arrive(&mut f, c, code::SUMMON_SUCCESS);
            if donot {
                under(&mut f, c, code::SPIRIT_DONOT_RETURN);
            }
            if maynot {
                under(&mut f, c, code::SPIRIT_MAYNOT_RETURN);
            }
            let (mandatory, optional) = returns(&f, c);
            let ev = Event::new(END);
            // Through the **registered** conditions, not the functions
            // directly: which condition each trigger carries is part of
            // what the procedure does, and calling the functions by name
            // would pass however they were wired up.
            let m_cond = f
                .effects
                .get(mandatory)
                .unwrap()
                .condition
                .expect("a condition");
            let o_cond = f
                .effects
                .get(optional)
                .unwrap()
                .condition
                .expect("a condition");
            assert_eq!(
                m_cond(&mut f, &ctx_for(mandatory, &ev, 0)),
                want_mandatory,
                "donot {donot} maynot {maynot}"
            );
            assert_eq!(
                o_cond(&mut f, &ctx_for(optional, &ev, 0)),
                want_optional,
                "donot {donot} maynot {maynot}"
            );
        }
    }

    /// **The mandatory return tries whatever happens; the optional one is
    /// not offered when it would do nothing.**
    ///
    /// The asymmetry reads backwards until you see what it is for, so it
    /// gets a board where the two disagree: a card that cannot go to a
    /// hand at all.
    #[test]
    fn only_the_optional_return_asks_whether_it_can() {
        let (mut f, c) = spirit(&[code::SUMMON_SUCCESS]);
        arrive(&mut f, c, code::SUMMON_SUCCESS);
        let (mandatory, optional) = returns(&f, c);
        let ev = Event::new(END);
        assert!(
            mandatory_return_target(&mut f, &ctx_for(mandatory, &ev, 0), false, None)
                .finished()
                .unwrap_or(0)
                != 0
        );
        assert!(
            optional_return_target(&mut f, &ctx_for(optional, &ev, 0), false, None)
                .finished()
                .unwrap_or(0)
                != 0,
            "the positive sibling"
        );

        // Now make the hand unreachable.
        under(&mut f, c, code::CANNOT_TO_HAND);
        assert!(
            mandatory_return_target(&mut f, &ctx_for(mandatory, &ev, 0), false, None)
                .finished()
                .unwrap_or(0)
                != 0,
            "mandatory still tries"
        );
        assert!(
            optional_return_target(&mut f, &ctx_for(optional, &ev, 0), false, None)
                .finished()
                .unwrap_or(0)
                == 0,
            "optional is not offered"
        );
    }

    /// **The flag is spent in the target, before the return is
    /// announced** — so a chain that never resolves has still used it up.
    #[test]
    fn the_target_spends_the_flag_and_announces() {
        let (mut f, c) = spirit(&[code::SUMMON_SUCCESS]);
        arrive(&mut f, c, code::SUMMON_SUCCESS);
        let (mandatory, _) = returns(&f, c);
        let mut ch = Chain::new(mandatory, Event::new(END));
        ch.chain_count = 1;
        ch.triggering_player = 0;
        f.core.current_chain.push(ch);
        let ev = Event::new(END);
        mandatory_return_target(&mut f, &ctx_for(mandatory, &ev, 0), true, None);
        assert!(
            !api::has_flag_effect(&f, c, FLAG_SPIRIT_RETURN),
            "spent, whether or not the return happens"
        );
        let info = f.core.current_chain[0]
            .opinfos
            .get(&category::TOHAND)
            .expect("the to-hand category");
        assert_eq!(info.cards.as_deref(), Some(&[c][..]));
        assert_eq!(info.count, 1);
    }

    /// **The return sends the card to its owner's hand, and only while it
    /// is still related.**
    #[test]
    fn the_return_sends_it_home() {
        // Related: it goes.
        let (mut f, c) = spirit(&[code::SUMMON_SUCCESS]);
        let (mandatory, _) = returns(&f, c);
        f.cards[c].create_chain_relation(mandatory, 11);
        let mut ch = Chain::new(mandatory, Event::new(END));
        ch.chain_count = 1;
        ch.chain_id = 11;
        f.core.current_chain.push(ch);
        let ev = Event::new(END);
        return_operation(&mut f, &ctx_for(mandatory, &ev, 0));
        drain(&mut f);
        assert_eq!(f.cards[c].current.location, location::HAND);
        assert_eq!(f.cards[c].current.controller, 0, "its owner's hand");

        // The **owner's** hand, which is not the controller's. A spirit
        // whose control has changed still goes home, and naming a player
        // instead of passing `nil` would send it to the wrong one.
        let (mut f, c) = spirit(&[code::SUMMON_SUCCESS]);
        f.cards[c].owner = 1;
        let (mandatory, _) = returns(&f, c);
        f.cards[c].create_chain_relation(mandatory, 11);
        let mut ch = Chain::new(mandatory, Event::new(END));
        ch.chain_count = 1;
        ch.chain_id = 11;
        f.core.current_chain.push(ch);
        return_operation(&mut f, &ctx_for(mandatory, &ev, 0));
        drain(&mut f);
        assert_eq!(f.cards[c].current.location, location::HAND);
        assert_eq!(
            f.cards[c].current.controller, 1,
            "its owner's hand, though player 0 controlled it"
        );

        // Not related: it stays.
        let (mut f, c) = spirit(&[code::SUMMON_SUCCESS]);
        let (mandatory, _) = returns(&f, c);
        let ev = Event::new(END);
        return_operation(&mut f, &ctx_for(mandatory, &ev, 0));
        assert!(f.core.subunits.is_empty());
        assert_eq!(f.cards[c].current.location, location::MZONE);
    }

    /// **The returned card's hand is shuffled once it arrives.**
    /// `Duel.SendtoHand` yields, so the operation ends after the card is
    /// in the hand — and that end, with `check_level` back at zero, is
    /// where the hand-shuffle flag the arrival raised is consumed. An
    /// operation that returned before the card moved consumed nothing,
    /// and the hand was never shuffled (fuzz seed 13).
    #[test]
    fn the_return_shuffles_the_hand_it_arrives_in() {
        let (mut f, c) = spirit(&[code::SUMMON_SUCCESS]);
        // Another, face-down card already in the hand, so the "all
        // face-up" skip does not apply.
        let mut d = Card::with_data(
            CardData {
                code: 5_053_103,
                type_: card_type::MONSTER,
                ..Default::default()
            },
            0,
        );
        d.current.controller = 0;
        let other = f.new_card(d);
        f.add_card(0, other, location::HAND, 0, false);
        f.cards[other].current.position = position::FACEDOWN;
        let (mandatory, _) = returns(&f, c);
        f.cards[c].create_chain_relation(mandatory, 11);
        let mut ch = Chain::new(mandatory, Event::new(END));
        ch.chain_count = 1;
        ch.chain_id = 11;
        f.core.current_chain.push(ch);
        f.core.chain_solving = true;
        f.core.sub_solving_event.push_back(Event::new(END));
        f.emplace(crate::processor::Kind::ExecuteOperation {
            resume: None,
            effect: mandatory,
            player: 0,
            subject: None,
            args: Vec::new(),
            was_disabled: false,
        });
        drain(&mut f);
        assert_eq!(f.cards[c].current.location, location::HAND);
        assert!(
            f.messages
                .iter()
                .any(|m| matches!(m, crate::field::Message::ShuffleHand { player: 0, .. })),
            "the hand was shuffled after the arrival"
        );
    }

    fn drain(f: &mut Field) {
        for _ in 0..4096 {
            match f.process() {
                Status::Continue => {
                    if f.core.units.is_empty() && f.core.subunits.is_empty() {
                        return;
                    }
                }
                Status::Awaiting => panic!("unexpected question: {:?}", f.messages.last()),
                other => panic!("stopped with {other:?}"),
            }
        }
        panic!("did not settle");
    }
}

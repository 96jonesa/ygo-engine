//! Mystical Space Typhoon — `c5318639.lua`.
//!
//! The first card that **asks a player to choose**. Trap Hole and
//! Sakuretsu Armor target without a selection, reading their victim off
//! the event; this one hands the decision to a person, which is the thing
//! the seam could not do before it learned to suspend.
//!
//! ## `SelectTarget` is two halves
//!
//! The export queues a selection and yields; the block after the yield
//! folds the answer into the chain link. So `target(chk = 1)` reads
//!
//! ```text
//! hint · select_target · suspend · selected_targets · record
//! ```
//!
//! which is `Duel.Hint`, `Duel.SelectTarget` and the `yieldk` block,
//! spelled out. This is the first card whose **target** suspends rather
//! than its operation.
//!
//! ## Three filters, not one
//!
//! `chk == 0` asks `IsExistingTarget` — a match that may also be
//! *targeted*, which is not the same as a match. `chk == 1` selects with
//! the same filter. And `chkc` answers about one named card with a
//! hand-written test rather than the filter: on the field, a Spell or
//! Trap, and **not itself**. The exception argument carries that last
//! clause for the other two.
//!
//! Every call here is to `script_api`, by the rule in `cards/mod.rs`.

use crate::board::location;
use crate::card::reason;
use crate::effect::{effect_type, flag, Ctx, Yield};
use crate::event::{category, code, CardId};
use crate::field::{timing, Field};
use crate::host_question::{hint, hintmsg};
use crate::script_api as api;

pub const CODE: u32 = 5_318_639;

/// `TIMINGS_CHECK_MONSTER_E` (`constant.lua:902`).
const TIMINGS_CHECK_MONSTER_E: u32 = 0x1e0;

pub fn initial_effect(f: &mut Field, c: CardId) {
    // Destroy 1 Spell/Trap on the field
    let e1 = api::create_effect(f, c);
    api::set_description(f, e1, api::stringid(CODE, 0));
    api::set_category(f, e1, category::DESTROY);
    api::set_type(f, e1, effect_type::ACTIVATE);
    api::set_property(f, e1, flag::CARD_TARGET, 0);
    api::set_code(f, e1, code::FREE_CHAIN);
    api::set_hint_timing(
        f,
        e1,
        0,
        timing::STANDBY_PHASE | timing::MAIN_END | TIMINGS_CHECK_MONSTER_E,
    );
    api::set_target(f, e1, target);
    api::set_operation(f, e1, activate);
    api::register_effect(f, c, e1, false);
}

const ONFIELD: u32 = location::ONFIELD as u32;

fn filter(f: &mut Field, c: CardId) -> bool {
    api::is_spell_trap(f, c)
}

fn target(f: &mut Field, ctx: &Ctx, chk: bool, chkc: Option<CardId>) -> Yield {
    let tp = ctx.player;
    let handler = api::get_handler(f, ctx.reason_effect);
    if let Some(chkc) = chkc {
        return api::yes(
            api::is_on_field(f, chkc) && api::is_spell_trap(f, chkc) && Some(chkc) != handler,
        );
    }
    let except = handler.map_or(api::Except::None, api::Except::Card);
    if !chk {
        return api::yes(api::is_existing_target(
            f,
            Some(&filter),
            tp,
            ONFIELD,
            ONFIELD,
            1,
            except,
        ));
    }
    api::hint(f, hint::SELECTMSG, tp, hintmsg::DESTROY);
    api::select_target(f, tp, Some(&filter), tp, ONFIELD, ONFIELD, 1, 1, except);
    api::suspend(move |f, _| {
        // `local g = Duel.SelectTarget(...)`, then the operation info.
        // Cancelling gives nothing back — it cannot happen here, since
        // the selection is not cancelable and wants exactly one, but the
        // Lua would carry a nil forward rather than invent a group.
        if let Some(g) = api::selected_targets(f) {
            let n = g.len() as u8;
            api::set_operation_info(f, 0, category::DESTROY, Some(g), n, tp, 0);
        }
        api::yes(true)
    })
}

fn activate(f: &mut Field, ctx: &Ctx) -> Yield {
    let Some(tc) = api::get_first_target(f) else {
        return api::done();
    };
    if api::is_relate_to_effect(f, tc, ctx.reason_effect) {
        api::destroy(f, vec![tc], reason::EFFECT);
    }
    api::done()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{card_type, status, Card, CardData};
    use crate::chain::Chain;
    use crate::duel::phases;
    use crate::event::Event;
    use crate::field::Message;
    use crate::position;
    use crate::processor::{Kind, Status};

    fn put(f: &mut Field, owner: u8, code_: u32, type_: u32, loc: u8, seq: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: code_,
                type_,
                ..Default::default()
            },
            owner,
        );
        c.current.controller = owner;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(owner, id, loc, seq, false);
        f.cards[id].current.position = position::FACEUP;
        let fid = f.next_field_id_raw();
        f.cards[id].fieldid = fid;
        f.cards[id].fieldid_r = fid;
        id
    }

    /// Player 0 in Main Phase 1 with the typhoon face-up in its own row —
    /// where it sits once activated — and `others` Spell/Traps of player
    /// 1's to aim at.
    fn field(others: u32) -> (Field, CardId, Vec<CardId>) {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        f.infos.turn_player = 0;
        f.infos.phase = phases::MAIN1;
        let mst = put(
            &mut f,
            0,
            CODE,
            card_type::SPELL | card_type::QUICKPLAY,
            location::SZONE,
            0,
        );
        f.initialize_card(mst);
        let targets = (0..others)
            .map(|i| put(&mut f, 1, 8_000 + i, card_type::TRAP, location::SZONE, i))
            .collect();
        (f, mst, targets)
    }

    fn ctx_of(f: &Field, mst: CardId) -> crate::event::EffectId {
        f.cards[mst].field_effect.equal_range(code::FREE_CHAIN)[0]
    }

    /// `target`, asked as a plain yes/no.
    ///
    /// The executor sets `core.reason_effect` around every call to a
    /// card's function, and the targeting scan reads it to decide what
    /// may be targeted *by this effect* — so a direct call has to set it
    /// too, or nothing is targetable and every answer is no.
    fn asks(f: &mut Field, ctx: &Ctx, chk: bool, chkc: Option<CardId>) -> bool {
        f.core.reason_effect = Some(ctx.reason_effect);
        f.core.reason_player = ctx.player;
        let answer = target(f, ctx, chk, chkc).finished().unwrap_or(0) != 0;
        f.core.reason_effect = None;
        answer
    }

    /// **One printed activate effect that targets**, with the timings a
    /// Quick-Play is offered at.
    #[test]
    fn the_script_registers_a_targeting_activate_effect() {
        let (f, mst, _) = field(0);
        let ids = f.cards[mst].field_effect.equal_range(code::FREE_CHAIN);
        assert_eq!(ids.len(), 1);
        let e = f.effects.get(ids[0]).unwrap();
        assert!(e.is_type(effect_type::ACTIVATE));
        assert!(e.is_flag(flag::CARD_TARGET));
        assert!(e.is_flag(crate::effect::flag::INITIAL));
        assert_eq!(e.category, category::DESTROY);
        assert_eq!(e.description, api::stringid(CODE, 0));
        assert_eq!(
            e.hint_timing,
            [
                0,
                timing::STANDBY_PHASE | timing::MAIN_END | TIMINGS_CHECK_MONSTER_E
            ],
            "offered to the opponent at the timings a Quick-Play gets"
        );
    }

    /// **The third question: on the field, a Spell or Trap, and not
    /// itself.** All three clauses, each refused on its own.
    #[test]
    fn the_third_question_refuses_itself_and_monsters() {
        let (mut f, mst, targets) = field(1);
        let monster = put(&mut f, 1, 9_000, card_type::MONSTER, location::MZONE, 0);
        let e = ctx_of(&f, mst);
        let ev = Event::new(code::FREE_CHAIN);
        let ctx = Ctx {
            reason_effect: e,
            player: 0,
            event: &ev,
            card: None,
            args: &[],
        };
        assert!(asks(&mut f, &ctx, false, Some(targets[0])), "another Trap");
        assert!(!asks(&mut f, &ctx, false, Some(mst)), "never itself");
        assert!(!asks(&mut f, &ctx, false, Some(monster)), "not a monster");
        // And not one that has left the field.
        f.cards[targets[0]].current.location = location::GRAVE;
        assert!(
            !asks(&mut f, &ctx, false, Some(targets[0])),
            "not off the field"
        );
    }

    /// **It is offered only when something else is there to hit.** Its
    /// own presence in the row does not count, which is what the
    /// exception argument is for.
    #[test]
    fn it_is_not_offered_with_only_itself_on_the_field() {
        let (mut f, mst, _) = field(0);
        let e = ctx_of(&f, mst);
        let ev = Event::new(code::FREE_CHAIN);
        let ctx = Ctx {
            reason_effect: e,
            player: 0,
            event: &ev,
            card: None,
            args: &[],
        };
        assert!(!asks(&mut f, &ctx, false, None), "nothing but itself");
        let (mut f, mst, _) = field(1);
        let e = ctx_of(&f, mst);
        let ctx = Ctx {
            reason_effect: e,
            player: 0,
            event: &ev,
            card: None,
            args: &[],
        };
        assert!(asks(&mut f, &ctx, false, None), "something to aim at");
    }

    /// **Choosing a target records it and destroys it**, end to end
    /// through the target's suspension: the chosen card is named, related
    /// to the link, announced, and gone.
    #[test]
    fn it_targets_what_was_chosen_and_destroys_it() {
        let (mut f, mst, targets) = field(2);
        let e = ctx_of(&f, mst);
        let mut ch = Chain::new(e, Event::new(code::FREE_CHAIN));
        ch.triggering_player = 0;
        ch.chain_count = 1;
        ch.chain_id = 11;
        f.core.current_chain.push(ch);
        f.core
            .sub_solving_event
            .push_back(Event::new(code::FREE_CHAIN));
        f.emplace(Kind::ExecuteTarget {
            resume: None,
            effect: e,
            player: 0,
            subject: None,
            args: Vec::new(),
            was_disabled: false,
        });
        let mut offered = 0;
        for _ in 0..4096 {
            match f.process() {
                Status::Continue => {
                    if f.core.units.is_empty() {
                        break;
                    }
                }
                Status::Awaiting => match f.messages.last() {
                    Some(Message::SelectCard { cards, .. }) => {
                        offered = cards.len();
                        f.core.return_cards.clear();
                        f.core.returns.set_i32(0, 0);
                        f.core.returns.set_i32(1, 1);
                        f.core.returns.set_i32(2, 1); // the *second* offer
                    }
                    other => panic!("unexpected question: {other:?}"),
                },
                other => panic!("stopped with {other:?}"),
            }
        }
        assert_eq!(offered, 2, "both of the opponent's, and not itself");
        let chosen = targets[1];
        assert_eq!(
            f.core.current_chain[0].target_cards,
            vec![chosen],
            "the one that was picked, not the first"
        );
        assert!(f.cards[chosen].has_chain_relation(e, 11));
        assert!(
            f.messages
                .iter()
                .any(|m| matches!(m, Message::BecomeTarget { cards } if cards == &vec![chosen])),
            "and it was announced"
        );
        let recorded = f.core.current_chain[0].opinfos.get(&category::DESTROY);
        assert_eq!(recorded.and_then(|t| t.cards.clone()), Some(vec![chosen]));

        // Now resolve.
        f.core.subunits.clear();
        let ev = Event::new(code::FREE_CHAIN);
        let ctx = Ctx {
            reason_effect: e,
            player: 0,
            event: &ev,
            card: None,
            args: &[],
        };
        activate(&mut f, &ctx);
        assert_eq!(f.core.subunits.len(), 1, "the destroy is queued");
        assert!(matches!(f.core.subunits[0].kind, Kind::Destroy { .. }));
    }

    /// **A target that is no longer the card that was targeted is left
    /// alone.** The re-read at resolution, as every targeting card does.
    #[test]
    fn a_target_that_lost_its_relation_is_spared() {
        let (mut f, mst, targets) = field(1);
        let e = ctx_of(&f, mst);
        let mut ch = Chain::new(e, Event::new(code::FREE_CHAIN));
        ch.triggering_player = 0;
        ch.chain_count = 1;
        ch.target_cards = vec![targets[0]];
        f.core.current_chain.push(ch);
        let ev = Event::new(code::FREE_CHAIN);
        let ctx = Ctx {
            reason_effect: e,
            player: 0,
            event: &ev,
            card: None,
            args: &[],
        };
        f.core.subunits.clear();
        activate(&mut f, &ctx);
        assert!(
            f.core.subunits.is_empty(),
            "no relation, so nothing is destroyed"
        );
        f.cards[targets[0]].relate_effect_insert_for_test(e);
        activate(&mut f, &ctx);
        assert_eq!(f.core.subunits.len(), 1, "and with one, it is");
    }
}

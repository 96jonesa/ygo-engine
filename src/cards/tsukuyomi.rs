//! Tsukuyomi — `c34853266.lua`.
//!
//! The thirty-seventh card, and Asura Priest's twin: the same spirit
//! procedure and the same "cannot be Special Summoned", with a different
//! second half. This one turns a face-up monster face-down when it
//! arrives — and it arrives, goes home, and comes back, which is the loop
//! the card is known for.
//!
//! ## The trigger is registered twice, not once for two events
//!
//! ```lua
//! e2:SetCode(EVENT_SUMMON_SUCCESS)  ...  local e3=e2:Clone()  e3:SetCode(EVENT_FLIP)
//! ```
//!
//! A clone, so the description, category, property, target and operation
//! all come across and only the event differs. The same two events the
//! spirit procedure watches, and for the same reason: those are the two
//! ways this card can arrive face-up.
//!
//! ## It may flip **itself**
//!
//! `s.posfilter` is `IsFaceup() and IsCanTurnSet()` with no exception for
//! the handler, and the scan is two-sided. So Tsukuyomi flipping itself
//! face-down is a legal choice — which is what lets it be flipped up again
//! next turn for another use. Reading the filter as "an *opponent's*
//! monster" would quietly remove the card's most-used line.
//!
//! ## The trigger fires whether or not there is anything to flip
//!
//! ```lua
//! if chk==0 then return true end
//! ```
//!
//! Unconditionally true, where most targeting cards answer a scan. The
//! trigger is **forced**, so it goes on the chain when Tsukuyomi arrives
//! whatever the board looks like, and the selection is simply asked over
//! whatever there is. The resolution re-checks instead.
//!
//! Every call here is to `script_api`, by the rule in `cards/mod.rs`.

use crate::board::{location, position};
use crate::effect::{effect_type, flag, Ctx, Yield};
use crate::event::{category, code, CardId};
use crate::field::Field;
use crate::host_question::{hint, hintmsg};
use crate::script_api as api;

use super::proc_spirit;

pub const CODE: u32 = 34_853_266;

pub fn initial_effect(f: &mut Field, c: CardId) {
    proc_spirit::add_procedure(f, c, &[code::SUMMON_SUCCESS, code::FLIP]);
    // Cannot be Special Summoned
    let e1 = api::create_effect(f, c);
    api::set_type(f, e1, effect_type::SINGLE);
    api::set_property(f, e1, flag::CANNOT_DISABLE | flag::UNCOPYABLE, 0);
    api::set_code(f, e1, code::SPSUMMON_CONDITION);
    api::register_effect(f, c, e1, false);
    // Flip 1 monster face-down
    let e2 = api::create_effect(f, c);
    api::set_description(f, e2, api::stringid(CODE, 0));
    api::set_category(f, e2, category::POSITION | category::SET);
    api::set_type(f, e2, effect_type::SINGLE | effect_type::TRIGGER_F);
    api::set_property(f, e2, flag::CARD_TARGET, 0);
    api::set_code(f, e2, code::SUMMON_SUCCESS);
    api::set_target(f, e2, postg);
    api::set_operation(f, e2, posop);
    api::register_effect(f, c, e2, false);
    let e3 = api::clone_effect(f, e2);
    api::set_code(f, e3, code::FLIP);
    api::register_effect(f, c, e3, false);
}

const MZONE: u32 = location::MZONE as u32;

/// `s.posfilter` — face-up and able to be turned face-down. **No
/// exception for the handler**, which is what lets Tsukuyomi flip itself.
fn posfilter(f: &mut Field, c: CardId) -> bool {
    api::is_faceup(f, c) && api::is_can_turn_set(f, c)
}

fn postg(f: &mut Field, ctx: &Ctx, chk: bool, chkc: Option<CardId>) -> Yield {
    let tp = ctx.player;
    if let Some(c) = chkc {
        return api::yes(api::is_location(f, c, u16::from(location::MZONE)) && posfilter(f, c));
    }
    if !chk {
        // A forced trigger: it goes on the chain whatever the board is.
        return api::yes(true);
    }
    api::hint(f, hint::SELECTMSG, tp, hintmsg::FACEUP);
    api::select_target(
        f,
        tp,
        Some(&posfilter),
        tp,
        MZONE,
        MZONE,
        1,
        1,
        api::Except::None,
    );
    api::suspend(move |f, _ctx| {
        let Some(g) = api::selected_targets(f) else {
            return api::done();
        };
        // `#g` — the size of what was chosen, not a literal one, and the
        // parameter slot carries the **position** it will be put into.
        let n = g.len() as u8;
        api::set_operation_info(
            f,
            0,
            category::POSITION,
            Some(g),
            n,
            tp,
            i32::from(position::FACEDOWN_DEFENSE),
        );
        api::done()
    })
}

fn posop(f: &mut Field, ctx: &Ctx) -> Yield {
    let e = ctx.reason_effect;
    let Some(tc) = api::get_first_target(f) else {
        return api::done();
    };
    // Still related, and **still face-up** — a monster already turned
    // down by something else is left alone.
    if api::is_relate_to_effect(f, tc, e) && api::is_faceup(f, tc) {
        api::change_position(f, vec![tc], position::FACEDOWN_DEFENSE);
    }
    api::done()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{card_type, status, Card, CardData};
    use crate::chain::Chain;
    use crate::event::{EffectId, Event};
    use crate::field::Message;
    use crate::processor::{Kind, Status};

    fn monster(f: &mut Field, player: u8, code_: u32, seq: u32, faceup: bool) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: code_,
                type_: card_type::MONSTER | card_type::NORMAL,
                level: 4,
                attack: 1000,
                defense: 1000,
                ..Default::default()
            },
            player,
        );
        c.current.controller = player;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(player, id, location::MZONE, seq, false);
        f.cards[id].current.position = if faceup {
            position::FACEUP_ATTACK
        } else {
            position::FACEDOWN_DEFENSE
        };
        id
    }

    /// Tsukuyomi face-up in `tp`'s first Monster Zone.
    fn field_as(tp: u8) -> (Field, CardId) {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        f.infos.turn_player = tp;
        f.infos.phase = crate::duel::phases::MAIN1;
        let mut d = Card::with_data(crate::cards::card_data(CODE).expect("printed data"), tp);
        d.current.controller = tp;
        d.set_status(status::EFFECT_ENABLED, true);
        let t = f.new_card(d);
        f.add_card(tp, t, location::MZONE, 0, false);
        f.cards[t].current.position = position::FACEUP_ATTACK;
        f.initialize_card(t);
        f.apply_field_effect(t);
        (f, t)
    }

    fn field() -> (Field, CardId) {
        field_as(0)
    }

    fn triggers(f: &Field, t: CardId) -> (EffectId, EffectId) {
        let on_summon = f.cards[t].single_effect.equal_range(code::SUMMON_SUCCESS);
        let on_flip = f.cards[t].single_effect.equal_range(code::FLIP);
        // Each event carries the spirit procedure's flag-setter as well as
        // the flip trigger, so pick the one that targets.
        let pick = |ids: &[EffectId]| {
            ids.iter()
                .copied()
                .find(|&e| f.effects.get(e).unwrap().target.is_some())
                .expect("a targeting trigger")
        };
        (pick(on_summon), pick(on_flip))
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

    /// **The flip trigger is registered twice, once per event, and the
    /// second is a clone** — so the description, category and property
    /// come across without being set again.
    #[test]
    fn the_flip_trigger_is_registered_for_both_arrival_events() {
        let (f, t) = field();
        let (on_summon, on_flip) = triggers(&f, t);
        for e in [on_summon, on_flip] {
            let x = f.effects.get(e).unwrap();
            assert!(x.is_type(effect_type::SINGLE), "its own arrival only");
            assert!(x.is_type(effect_type::TRIGGER_F), "forced");
            assert_eq!(x.category, category::POSITION | category::SET);
            assert!(x.is_flag(flag::CARD_TARGET));
            assert_eq!(x.description, api::stringid(CODE, 0));
        }
        assert_eq!(f.effects.get(on_summon).unwrap().code, code::SUMMON_SUCCESS);
        assert_eq!(f.effects.get(on_flip).unwrap().code, code::FLIP);

        // And the spirit procedure's own effects are there too: two
        // returns, and a flag-setter for **each** named event — so each
        // event carries two singles, the setter and the flip trigger.
        let end = code::PHASE | u32::from(crate::duel::phases::END);
        assert_eq!(f.cards[t].field_effect.equal_range(end).len(), 2);
        for event in [code::SUMMON_SUCCESS, code::FLIP] {
            assert_eq!(
                f.cards[t].single_effect.equal_range(event).len(),
                2,
                "a spirit flag-setter and a flip trigger for {event}"
            );
        }
        // Cannot be Special Summoned, and the restriction cannot be
        // switched off with the card or copied off it.
        let ban = f.cards[t]
            .single_effect
            .equal_range(code::SPSUMMON_CONDITION);
        assert_eq!(ban.len(), 1);
        let e1 = f.effects.get(ban[0]).unwrap();
        assert!(e1.is_flag(flag::CANNOT_DISABLE));
        assert!(e1.is_flag(flag::UNCOPYABLE));
    }

    /// **The filter wants a face-up monster that can be turned down —
    /// and makes no exception for Tsukuyomi itself.**
    ///
    /// That last clause is the card: flipping itself face-down is how it
    /// comes back next turn for another use, and a filter that excluded
    /// the handler would quietly remove it.
    #[test]
    fn it_may_choose_itself_and_wants_a_face_up_monster() {
        let (mut f, t) = field();
        let up = monster(&mut f, 1, 9_001, 0, true);
        let down = monster(&mut f, 1, 9_002, 1, false);
        assert!(posfilter(&mut f, t), "itself, face-up");
        assert!(posfilter(&mut f, up), "the opponent's, face-up");
        assert!(!posfilter(&mut f, down), "not a face-down one");

        // And one that cannot be turned face-down.
        let mut lock = crate::effect::Effect::new(effect_type::SINGLE, code::CANNOT_TURN_SET);
        lock.owner = Some(up);
        lock.handler = Some(up);
        let id = f.new_effect(lock);
        f.cards[up].single_effect.insert(code::CANNOT_TURN_SET, id);
        f.cards[up].indexer.insert(id);
        assert!(!posfilter(&mut f, up), "it may not be turned down");
    }

    /// **`chkc` also wants a Monster Zone**, which the filter does not
    /// ask — a face-up monster in a graveyard passes the filter and fails
    /// the location clause.
    #[test]
    fn the_target_check_wants_a_monster_zone() {
        let (mut f, t) = field();
        let up = monster(&mut f, 1, 9_001, 0, true);
        let (on_summon, _) = triggers(&f, t);
        let ev = Event::new(code::SUMMON_SUCCESS);
        let ask = |f: &mut Field, c: CardId| {
            f.core.reason_effect = Some(on_summon);
            postg(f, &ctx_for(on_summon, &ev, 0), false, Some(c))
                .finished()
                .unwrap_or(0)
                != 0
        };
        assert!(ask(&mut f, up), "the positive sibling");

        // In the Monster Zone but refused by the filter: a face-down
        // monster. Only the filter can be saying no here.
        let down = monster(&mut f, 1, 9_002, 1, false);
        assert!(!posfilter(&mut f, down));
        assert!(!ask(&mut f, down), "the filter refuses it");

        // Passes the filter but is not in a Monster Zone.
        f.move_card(1, up, location::GRAVE, 0, false);
        f.cards[up].current.position = position::FACEUP_ATTACK;
        assert!(posfilter(&mut f, up), "still passes the filter");
        assert!(!ask(&mut f, up), "but not the location clause");
    }

    /// **The trigger fires whether or not there is anything to flip.**
    /// It is forced, so it goes on the chain when Tsukuyomi arrives
    /// whatever the board looks like.
    #[test]
    fn the_trigger_does_not_ask_whether_there_is_a_target() {
        let (mut f, t) = field();
        // Take Tsukuyomi off the field so that even it is not a target.
        f.move_card(0, t, location::GRAVE, 0, false);
        let (on_summon, _) = triggers(&f, t);
        f.core.reason_effect = Some(on_summon);
        let ev = Event::new(code::SUMMON_SUCCESS);
        assert!(
            postg(&mut f, &ctx_for(on_summon, &ev, 0), false, None)
                .finished()
                .unwrap_or(0)
                != 0,
            "an empty board still fires it"
        );
    }

    /// Drive the target then the operation, answering the one question.
    fn resolve(f: &mut Field, t: CardId, e: EffectId, tp: u8, pick: usize) -> Vec<CardId> {
        let mut ch = Chain::new(e, Event::new(code::SUMMON_SUCCESS));
        ch.triggering_player = tp;
        ch.chain_count = 1;
        ch.chain_id = 11;
        f.core.current_chain.push(ch);
        f.core.chain_solving = true;
        f.cards[t].create_chain_relation(e, 11);
        f.core
            .sub_solving_event
            .push_back(Event::new(code::SUMMON_SUCCESS));
        f.emplace(Kind::ExecuteTarget {
            resume: None,
            effect: e,
            player: tp,
            subject: None,
            args: Vec::new(),
            was_disabled: false,
        });
        let mut offered = Vec::new();
        let mut operated = false;
        for _ in 0..8192 {
            match f.process() {
                Status::Continue => {
                    if f.core.units.is_empty() && f.core.subunits.is_empty() {
                        if operated {
                            break;
                        }
                        operated = true;
                        f.core
                            .sub_solving_event
                            .push_back(Event::new(code::SUMMON_SUCCESS));
                        f.emplace(Kind::ExecuteOperation {
                            resume: None,
                            effect: e,
                            player: tp,
                            subject: None,
                            args: Vec::new(),
                            was_disabled: false,
                        });
                    }
                }
                Status::Awaiting => match f.messages.last() {
                    Some(Message::SelectCard { min, cards, .. }) => {
                        offered = cards.clone();
                        let min = usize::from(*min);
                        f.core.return_cards.clear();
                        f.core.returns.set_i32(0, 0);
                        f.core.returns.set_i32(1, min as i32);
                        for i in 0..min {
                            f.core.returns.set_i32(i + 2, (pick + i) as i32);
                        }
                    }
                    other => panic!("unexpected question: {other:?}"),
                },
                other => panic!("stopped with {other:?}"),
            }
        }
        offered
    }

    /// **It turns the chosen monster face-down in defence**, from either
    /// side, and the announcement names the position it will take.
    #[test]
    fn it_turns_the_chosen_monster_face_down() {
        let (mut f, t) = field();
        let theirs = monster(&mut f, 1, 9_001, 0, true);
        let (on_summon, _) = triggers(&f, t);
        let offered = resolve(&mut f, t, on_summon, 0, 0);
        let mut want = vec![t, theirs];
        want.sort_unstable();
        let mut got = offered.clone();
        got.sort_unstable();
        assert_eq!(got, want, "both sides, itself included");
        let chosen = offered[0];
        assert_eq!(
            f.cards[chosen].current.position,
            position::FACEDOWN_DEFENSE,
            "face-down, and in defence"
        );
        assert!(
            f.messages
                .iter()
                .any(|m| matches!(m, Message::Hint { kind, player, value }
                if *kind == hint::SELECTMSG && *player == 0 && *value == hintmsg::FACEUP)),
            "prefaced by the face-up prompt"
        );
        let info = f.core.current_chain[0]
            .opinfos
            .get(&category::POSITION)
            .expect("the position category");
        assert_eq!(info.cards.as_deref(), Some(&[chosen][..]));
        assert_eq!(info.count, 1, "the size of what was chosen");
        assert_eq!(info.player, 0);
        assert_eq!(
            info.param,
            i32::from(position::FACEDOWN_DEFENSE),
            "the position it will be put into"
        );
    }

    /// **It can turn itself face-down**, which is the line the card is
    /// played for.
    #[test]
    fn it_can_turn_itself_face_down() {
        let (mut f, t) = field();
        let (on_summon, _) = triggers(&f, t);
        // Its own row and nothing else: the only target is itself.
        let offered = resolve(&mut f, t, on_summon, 0, 0);
        assert_eq!(offered, vec![t]);
        assert_eq!(f.cards[t].current.position, position::FACEDOWN_DEFENSE);
    }

    /// **The same, as player 1** — the selection follows `tp`.
    #[test]
    fn the_question_follows_the_activating_player() {
        let (mut f, t) = field_as(1);
        monster(&mut f, 0, 9_001, 0, true);
        let (on_summon, _) = triggers(&f, t);
        let mut asked = u8::MAX;
        let mut ch = Chain::new(on_summon, Event::new(code::SUMMON_SUCCESS));
        ch.triggering_player = 1;
        ch.chain_count = 1;
        f.core.current_chain.push(ch);
        f.core.reason_effect = Some(on_summon);
        let ev = Event::new(code::SUMMON_SUCCESS);
        postg(&mut f, &ctx_for(on_summon, &ev, 1), true, None);
        for _ in 0..64 {
            if let Status::Awaiting = f.process() {
                if let Some(Message::SelectCard { player, .. }) = f.messages.last() {
                    asked = *player;
                }
                break;
            }
        }
        assert_eq!(asked, 1, "player 1 activated, so player 1 chooses");
    }

    /// **The resolution re-checks that the target is still related and
    /// still face-up**, and leaves it alone otherwise.
    #[test]
    fn the_resolution_re_checks_the_target() {
        for (related, faceup, expect) in [
            (true, true, position::FACEDOWN_DEFENSE),
            (false, true, position::FACEUP_ATTACK),
            (true, false, position::FACEDOWN_ATTACK),
        ] {
            let (mut f, t) = field();
            let tc = monster(&mut f, 1, 9_001, 0, true);
            if !faceup {
                f.cards[tc].current.position = position::FACEDOWN_ATTACK;
            }
            let (on_summon, _) = triggers(&f, t);
            let mut ch = Chain::new(on_summon, Event::new(code::SUMMON_SUCCESS));
            ch.chain_count = 1;
            ch.chain_id = 11;
            ch.target_cards = vec![tc];
            f.core.current_chain.push(ch);
            f.core.chain_solving = true;
            if related {
                f.cards[tc].create_chain_relation(on_summon, 11);
            }
            let ev = Event::new(code::SUMMON_SUCCESS);
            posop(&mut f, &ctx_for(on_summon, &ev, 0));
            for _ in 0..1024 {
                // `End` is an ordinary outcome here: with nothing queued
                // the processor has nothing left to do.
                match f.process() {
                    Status::Continue => {
                        if f.core.units.is_empty() && f.core.subunits.is_empty() {
                            break;
                        }
                    }
                    Status::End => break,
                    other => panic!("stopped with {other:?}"),
                }
            }
            assert_eq!(
                f.cards[tc].current.position, expect,
                "related {related} faceup {faceup}"
            );
        }
    }
}

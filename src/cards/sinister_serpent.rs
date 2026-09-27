//! Sinister Serpent — `c8131171.lua`.
//!
//! The twenty-eighth card, and the first to act **from a graveyard**.
//! It returns itself to the hand every Standby Phase, and leaves behind
//! an effect that banishes a copy of itself during the opponent's next
//! End Phase — the card's own brake on itself.
//!
//! ## A trigger ranged to the graveyard
//!
//! `EFFECT_TYPE_FIELD + EFFECT_TYPE_TRIGGER_O`, `SetRange(LOCATION_GRAVE)`,
//! on `EVENT_PHASE | PHASE_STANDBY`. Three things at once that are new:
//! the effect is a **field** type rather than a single one, its range is
//! a graveyard rather than the field, and its event is a **phase**.
//!
//! The condition is `Duel.IsTurnPlayer(tp)` — its controller's own
//! Standby Phase, not either.
//!
//! ## It declares a certainty and a possibility
//!
//! ```lua
//! Duel.SetOperationInfo(0,CATEGORY_TOHAND,c,1,tp,0)
//! Duel.SetPossibleOperationInfo(0,CATEGORY_REMOVE,nil,1,tp,LOCATION_GRAVE)
//! ```
//!
//! The to-hand *will* happen; the banish only *might*, and later, and it
//! goes in a different map on the chain link. Declaring both as
//! certainties would tell an opponent a banish is coming when it may
//! never be.
//!
//! ## The brake is built at resolution
//!
//! The operation registers a continuous field effect on
//! `EVENT_PHASE + PHASE_END`, once, conditional on it being the
//! **opponent's** turn, and resetting at the end of the opponent's turn.
//! So the serpent comes back on your Standby Phase and one copy is
//! banished at the end of the opponent's next turn.
//!
//! ## `aux.SpElimFilter` is a check for a card that is not in this pool
//!
//! It asks whether the controller is affected by **Spirit Elimination**
//! (card `69832741`), and answers "graveyard only" when they are not.
//! The card is not in the Goat pool, so the branch is unreachable here —
//! it is ported anyway, because the alternative is to write the
//! simplified form and have it be quietly wrong the day the pool grows.
//!
//! Every call here is to `script_api`, by the rule in `cards/mod.rs`.

use crate::board::{location, position};
use crate::card::reason;
use crate::effect::{effect_type, Ctx, Yield};
use crate::event::{category, code, CardId};
use crate::field::{reset, Field};
use crate::host_question::{hint, hintmsg};
use crate::script_api as api;

pub const CODE: u32 = 8_131_171;

/// `Spirit Elimination`, which `aux.SpElimFilter` asks after by number.
const SPIRIT_ELIMINATION: u32 = 69_832_741;

pub fn initial_effect(f: &mut Field, c: CardId) {
    // Add this card to the hand
    let e1 = api::create_effect(f, c);
    api::set_description(f, e1, api::stringid(CODE, 0));
    api::set_category(f, e1, category::TOHAND | category::REMOVE);
    api::set_type(f, e1, effect_type::FIELD | effect_type::TRIGGER_O);
    api::set_code(f, e1, code::PHASE | u32::from(crate::duel::phases::STANDBY));
    api::set_range(f, e1, u16::from(location::GRAVE));
    api::set_count_limit(f, e1, 1, CODE, 0);
    api::set_condition(f, e1, condition);
    api::set_target(f, e1, target);
    api::set_operation(f, e1, operation);
    api::register_effect(f, c, e1, false);
}

fn condition(f: &mut Field, ctx: &Ctx) -> bool {
    api::is_turn_player(f, ctx.player)
}

fn target(f: &mut Field, ctx: &Ctx, chk: bool, _chkc: Option<CardId>) -> Yield {
    let tp = ctx.player;
    let Some(handler) = api::get_handler(f, ctx.reason_effect) else {
        return api::yes(false);
    };
    if !chk {
        return api::yes(api::is_able_to_hand(f, handler, None));
    }
    // A certainty, and a possibility, in two different maps.
    api::set_operation_info(f, 0, category::TOHAND, Some(vec![handler]), 1, tp, 0);
    api::set_possible_operation_info(
        f,
        0,
        category::REMOVE,
        None,
        1,
        tp,
        i32::from(location::GRAVE),
    );
    api::yes(true)
}

/// `aux.SpElimFilter(c, true)` — the Spirit Elimination check, with
/// `mustbefaceup` set and `includemzone` left off.
fn sp_elim_filter(f: &mut Field, c: CardId) -> bool {
    if api::is_monster(f, c) {
        // Unreachable in this pool, and kept: with nobody affected by
        // Spirit Elimination the branch below already demands a
        // graveyard, which a card in the monster row fails whichever way
        // up it is. The guard only does work for a player who *is*
        // affected, where the branch answers `IsLocation(MZONE)` and this
        // is the clause that refuses a face-down one.
        if api::is_location(f, c, u16::from(location::MZONE)) && api::is_facedown(f, c) {
            return false;
        }
        let controller = api::get_controler(f, c);
        if api::is_player_affected_by_effect(f, controller, SPIRIT_ELIMINATION) {
            api::is_location(f, c, u16::from(location::MZONE))
        } else {
            api::is_location(f, c, u16::from(location::GRAVE))
        }
    } else {
        // `includemzone or c:IsLocation(LOCATION_GRAVE)`, and this caller
        // leaves `includemzone` off.
        api::is_location(f, c, u16::from(location::GRAVE))
    }
}

fn rm_filter(f: &mut Field, c: CardId) -> bool {
    api::is_code(f, c, CODE)
        && api::is_able_to_remove(f, c, None, None, None)
        && sp_elim_filter(f, c)
}

const MZONE_OR_GRAVE: u32 = (location::MZONE | location::GRAVE) as u32;

fn rm_condition(f: &mut Field, ctx: &Ctx) -> bool {
    let tp = ctx.player;
    api::is_turn_player(f, 1 - tp)
        && api::is_existing_matching_card(
            f,
            Some(&rm_filter),
            tp,
            MZONE_OR_GRAVE,
            0,
            1,
            api::Except::None,
        )
}

fn rm_operation(f: &mut Field, ctx: &Ctx) -> Yield {
    let tp = ctx.player;
    api::hint(f, hint::SELECTMSG, tp, hintmsg::REMOVE);
    api::select_matching_card(
        f,
        tp,
        Some(&rm_filter),
        tp,
        MZONE_OR_GRAVE,
        0,
        1,
        1,
        api::Except::None,
    );
    api::suspend(move |f, _ctx| {
        let g = api::group_selected(f);
        if g.is_empty() {
            return api::done();
        }
        api::remove(f, g, position::FACEUP, reason::EFFECT);
        api::done()
    })
}

fn operation(f: &mut Field, ctx: &Ctx) -> Yield {
    let tp = ctx.player;
    let Some(handler) = api::get_handler(f, ctx.reason_effect) else {
        return api::done();
    };
    if api::is_relate_to_effect(f, handler, ctx.reason_effect) {
        api::send_to_hand(f, vec![handler], None, reason::EFFECT);
        // `Duel.SendtoHand` **yields** — the brake below is registered
        // once the card has actually returned, and the operation's end
        // (where the hand-shuffle flag is consumed) comes after that.
        return api::suspend(move |f, _ctx| {
            // The brake: banish one copy at the end of the opponent's next turn.
            let e1 = api::create_effect(f, handler);
            api::set_type(f, e1, effect_type::FIELD | effect_type::CONTINUOUS);
            api::set_code(f, e1, code::PHASE | u32::from(crate::duel::phases::END));
            api::set_count_limit(f, e1, 1, 0, 0);
            api::set_condition(f, e1, rm_condition);
            api::set_operation(f, e1, rm_operation);
            api::set_reset(
                f,
                e1,
                reset::PHASE | u32::from(crate::duel::phases::END) | reset::OPPO_TURN,
                0,
            );
            api::duel_register_effect(f, e1, tp);
            api::suspend(|_, _| api::done())
        });
    }
    // The brake: banish one copy at the end of the opponent's next turn.
    let e1 = api::create_effect(f, handler);
    api::set_type(f, e1, effect_type::FIELD | effect_type::CONTINUOUS);
    api::set_code(f, e1, code::PHASE | u32::from(crate::duel::phases::END));
    api::set_count_limit(f, e1, 1, 0, 0);
    api::set_condition(f, e1, rm_condition);
    api::set_operation(f, e1, rm_operation);
    api::set_reset(
        f,
        e1,
        reset::PHASE | u32::from(crate::duel::phases::END) | reset::OPPO_TURN,
        0,
    );
    api::duel_register_effect(f, e1, tp);
    api::suspend(|_, _| api::done())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{card_type, status, Card, CardData};
    use crate::chain::Chain;
    use crate::duel::phases;
    use crate::event::{EffectId, Event};
    use crate::field::Message;
    use crate::processor::{Kind, Status};

    fn serpent(f: &mut Field, owner: u8, loc: u8, seq: u32) -> CardId {
        let mut c = Card::with_data(crate::cards::card_data(CODE).expect("printed data"), owner);
        c.current.controller = owner;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(owner, id, loc, seq, false);
        f.cards[id].current.position = if loc == location::MZONE {
            position::FACEUP_ATTACK
        } else {
            position::FACEUP
        };
        f.initialize_card(id);
        id
    }

    /// `tp`'s Standby Phase with a serpent in `tp`'s graveyard.
    fn field_as(tp: u8) -> (Field, CardId) {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        f.infos.turn_player = tp;
        f.infos.phase = phases::STANDBY;
        let ss = serpent(&mut f, tp, location::GRAVE, 0);
        (f, ss)
    }

    fn field() -> (Field, CardId) {
        field_as(0)
    }

    fn effect_of(f: &Field, ss: CardId) -> EffectId {
        let want = code::PHASE | u32::from(phases::STANDBY);
        f.cards[ss].field_effect.equal_range(want)[0]
    }

    fn ctx_for(e: EffectId, ev: &Event, tp: u8) -> Ctx<'_> {
        Ctx {
            reason_effect: e,
            player: tp,
            event: ev,
            card: None,
            args: &[],
        }
    }

    fn drive(f: &mut Field, tp: u8, e: EffectId, which: Kind) {
        f.core.sub_solving_event.push_back(Event::new(0));
        let _ = tp;
        f.emplace(which);
        for _ in 0..8192 {
            match f.process() {
                Status::Continue => {
                    if f.core.units.is_empty() && f.core.subunits.is_empty() {
                        break;
                    }
                }
                Status::Awaiting => match f.messages.last() {
                    Some(Message::SelectCard { min, .. }) => {
                        let min = usize::from(*min);
                        f.core.return_cards.clear();
                        f.core.returns.set_i32(0, 0);
                        f.core.returns.set_i32(1, min as i32);
                        for i in 0..min {
                            f.core.returns.set_i32(i + 2, i as i32);
                        }
                    }
                    other => panic!("unexpected question: {other:?}"),
                },
                other => panic!("stopped with {other:?}"),
            }
        }
        let _ = e;
    }

    /// Run the card's own target and operation with a chain link.
    fn resolve_as(f: &mut Field, tp: u8, e: EffectId) {
        let mut ch = Chain::new(e, Event::new(0));
        ch.triggering_player = tp;
        ch.chain_count = 1;
        ch.chain_id = 11;
        f.core.current_chain.push(ch);
        f.core.chain_solving = true;
        // Activating a card entangles it with its own chain link; the
        // operation's `IsRelateToEffect` reads exactly that, and a driven
        // test has to create it the way an activation would.
        if let Some(handler) = api::get_handler(f, e) {
            f.cards[handler].create_chain_relation(e, 11);
        }
        for kind in [
            Kind::ExecuteTarget {
                resume: None,
                effect: e,
                player: tp,
                subject: None,
                args: Vec::new(),
                was_disabled: false,
            },
            Kind::ExecuteOperation {
                resume: None,
                effect: e,
                player: tp,
                subject: None,
                args: Vec::new(),
                was_disabled: false,
            },
        ] {
            drive(f, tp, e, kind);
        }
    }

    /// **One printed optional trigger, ranged to the graveyard**, on its
    /// controller's Standby Phase, once per turn per name.
    #[test]
    fn the_script_registers_a_graveyard_trigger() {
        let (f, ss) = field();
        let want = code::PHASE | u32::from(phases::STANDBY);
        let ids = f.cards[ss].field_effect.equal_range(want);
        assert_eq!(ids.len(), 1, "one effect on the Standby Phase");
        let e = f.effects.get(ids[0]).unwrap();
        assert!(
            e.is_type(effect_type::FIELD),
            "a field effect, not a single"
        );
        assert!(e.is_type(effect_type::TRIGGER_O), "optional");
        assert!(!e.is_type(effect_type::TRIGGER_F));
        assert!(e.is_flag(crate::effect::flag::INITIAL));
        assert_eq!(e.range, u16::from(location::GRAVE), "it acts from a grave");
        assert!(e.is_flag(crate::effect::flag::COUNT_LIMIT));
        assert_eq!(e.count_code, CODE, "per name");
        assert_eq!(e.category, category::TOHAND | category::REMOVE);
        assert_eq!(e.description, api::stringid(CODE, 0));
    }

    /// **The condition is its controller's own Standby Phase.**
    #[test]
    fn the_condition_wants_its_own_turn() {
        let (mut f, ss) = field();
        let e = effect_of(&f, ss);
        let ev = Event::new(0);
        let ctx = ctx_for(e, &ev, 0);
        assert!(condition(&mut f, &ctx), "player 0's turn");
        f.infos.turn_player = 1;
        assert!(!condition(&mut f, &ctx), "not on the opponent's");
    }

    /// **The registered effect carries the condition.** Dropping it
    /// would make the trigger fire on either player's Standby Phase.
    #[test]
    fn the_registered_effect_has_a_condition() {
        let (f, ss) = field();
        let e = f.effects.get(effect_of(&f, ss)).unwrap();
        assert!(e.condition.is_some(), "the script sets one");
    }

    /// **The target refuses when the card cannot come back**, which is
    /// what stops it being offered from a graveyard it may not leave.
    #[test]
    fn the_target_refuses_a_card_that_cannot_return() {
        let (mut f, ss) = field();
        let e = effect_of(&f, ss);
        let ev = Event::new(0);
        let ctx = ctx_for(e, &ev, 0);
        assert!(
            target(&mut f, &ctx, false, None).finished().unwrap_or(0) != 0,
            "an ordinary serpent may come back"
        );
        let mut ban = crate::effect::Effect::new(effect_type::SINGLE, code::CANNOT_TO_HAND);
        ban.owner = Some(ss);
        ban.handler = Some(ss);
        let ban = f.new_effect(ban);
        f.cards[ss].single_effect.insert(code::CANNOT_TO_HAND, ban);
        assert!(
            target(&mut f, &ctx, false, None).finished().unwrap_or(0) == 0,
            "and one that may not is not offered"
        );
    }

    /// **A serpent that lost its relation stays put**, and the brake is
    /// still registered — the two halves of the operation are
    /// independent.
    #[test]
    fn a_serpent_that_lost_its_relation_stays_put() {
        let (mut f, ss) = field();
        let e = effect_of(&f, ss);
        let mut ch = Chain::new(e, Event::new(0));
        ch.triggering_player = 0;
        ch.chain_count = 1;
        ch.chain_id = 11;
        f.core.current_chain.push(ch);
        // No relation created.
        drive(
            &mut f,
            0,
            e,
            Kind::ExecuteOperation {
                resume: None,
                effect: e,
                player: 0,
                subject: None,
                args: Vec::new(),
                was_disabled: false,
            },
        );
        assert_eq!(
            f.cards[ss].current.location,
            location::GRAVE,
            "it did not come back"
        );
        assert!(
            !f.field_effects
                .continuous
                .equal_range(code::PHASE | u32::from(phases::END))
                .is_empty(),
            "but the brake is registered either way"
        );
    }

    /// **It goes to its *owner's* hand.** A serpent owned by player 1
    /// sitting in player 0's graveyard goes home, not to the player
    /// whose graveyard it was in.
    #[test]
    fn it_returns_to_its_owners_hand() {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        f.infos.turn_player = 0;
        f.infos.phase = phases::STANDBY;
        let mut owned_by_one =
            Card::with_data(crate::cards::card_data(CODE).expect("printed data"), 1);
        owned_by_one.current.controller = 0;
        owned_by_one.set_status(status::EFFECT_ENABLED, true);
        let ss = f.new_card(owned_by_one);
        f.add_card(0, ss, location::GRAVE, 0, false);
        f.cards[ss].current.position = position::FACEUP;
        f.initialize_card(ss);
        let e = effect_of(&f, ss);
        resolve_as(&mut f, 0, e);
        assert_eq!(f.cards[ss].current.location, location::HAND);
        assert_eq!(
            f.cards[ss].current.controller, 1,
            "its owner's hand, not the graveyard's owner's"
        );
    }

    /// **The banish filter wants a card that may actually be banished.**
    #[test]
    fn the_banish_filter_wants_a_removable_card() {
        let (mut f, ss) = field();
        assert!(rm_filter(&mut f, ss));
        let mut ban = crate::effect::Effect::new(effect_type::SINGLE, code::CANNOT_REMOVE);
        ban.owner = Some(ss);
        ban.handler = Some(ss);
        let ban = f.new_effect(ban);
        f.cards[ss].single_effect.insert(code::CANNOT_REMOVE, ban);
        assert!(
            !rm_filter(&mut f, ss),
            "a serpent that cannot be banished is not a candidate"
        );
    }

    /// **The brake looks only at its own side.** A copy in the
    /// opponent's graveyard is not a candidate — the `o` mask is zero.
    #[test]
    fn the_brake_does_not_reach_the_opponents_graveyard() {
        let (mut f, ss) = field();
        let e = effect_of(&f, ss);
        resolve_as(&mut f, 0, e);
        let theirs = serpent(&mut f, 1, location::GRAVE, 0);
        f.infos.turn_player = 1;
        let brake = f
            .field_effects
            .continuous
            .equal_range(code::PHASE | u32::from(phases::END))[0];
        let ev = Event::new(0);
        let ctx = ctx_for(brake, &ev, 0);
        assert!(
            !rm_condition(&mut f, &ctx),
            "the only copy is the opponent's, and it is out of reach"
        );
        assert_eq!(f.cards[theirs].current.location, location::GRAVE);
    }

    /// **It declares the to-hand as certain and the banish as possible.**
    ///
    /// Two different maps on the chain link: the return *will* happen,
    /// the banish only *might*, and later.
    #[test]
    fn the_two_declarations_go_to_different_maps() {
        let (mut f, ss) = field();
        let e = effect_of(&f, ss);
        resolve_as(&mut f, 0, e);
        let link = &f.core.current_chain[0];
        let certain = link
            .opinfos
            .get(&category::TOHAND)
            .cloned()
            .expect("the to-hand is certain");
        assert_eq!(certain.cards, Some(vec![ss]), "and it names itself");
        assert_eq!(certain.player, 0);
        assert!(
            !link.opinfos.contains_key(&category::REMOVE),
            "the banish is not a certainty"
        );
        let possible = link
            .possible_opinfos
            .get(&category::REMOVE)
            .cloned()
            .expect("the banish is a possibility");
        assert_eq!(possible.cards, None, "no card is named for it yet");
        assert_eq!(possible.param, i32::from(location::GRAVE));
    }

    /// **It returns itself to the hand and leaves the brake behind.**
    #[test]
    fn it_returns_itself_and_registers_the_brake() {
        let (mut f, ss) = field();
        let e = effect_of(&f, ss);
        resolve_as(&mut f, 0, e);
        assert_eq!(f.cards[ss].current.location, location::HAND);
        assert_eq!(
            f.cards[ss].current.controller, 0,
            "its own hand — `nil` means the owner's, and it owns itself here"
        );
        assert!(f.cards[ss].reason & reason::EFFECT != 0);
        let brake = f
            .field_effects
            .continuous
            .equal_range(code::PHASE | u32::from(phases::END))
            .first()
            .copied()
            .expect("a brake was registered");
        let b = f.effects.get(brake).unwrap();
        assert!(b.is_type(effect_type::FIELD));
        assert!(b.is_type(effect_type::CONTINUOUS));
        assert_eq!(b.effect_owner, 0, "registered for its controller");
        assert!(b.is_flag(crate::effect::flag::COUNT_LIMIT), "once");
        assert!(
            b.reset_flag & reset::OPPO_TURN != 0,
            "it expires at the end of the opponent's turn"
        );
    }

    /// **The brake waits for the opponent's turn and something to
    /// banish.** Both halves of its condition, each refused on its own.
    #[test]
    fn the_brake_wants_the_opponents_turn_and_a_copy() {
        let (mut f, ss) = field();
        let e = effect_of(&f, ss);
        resolve_as(&mut f, 0, e);
        let brake = f
            .field_effects
            .continuous
            .equal_range(code::PHASE | u32::from(phases::END))[0];
        let ev = Event::new(0);
        let ctx = ctx_for(brake, &ev, 0);

        // Its own turn, and the serpent is in the hand now, not a grave.
        f.infos.turn_player = 0;
        assert!(!rm_condition(&mut f, &ctx), "not on its own turn");
        f.infos.turn_player = 1;
        assert!(
            !rm_condition(&mut f, &ctx),
            "the opponent's turn, but nothing in a graveyard to banish"
        );
        // A second copy in the graveyard makes it fire.
        let spare = serpent(&mut f, 0, location::GRAVE, 1);
        assert!(rm_condition(&mut f, &ctx), "now there is one");
        assert_eq!(f.cards[spare].current.location, location::GRAVE);
    }

    /// **The brake banishes a copy face-up from the graveyard.**
    #[test]
    fn the_brake_banishes_a_copy() {
        let (mut f, ss) = field();
        let e = effect_of(&f, ss);
        resolve_as(&mut f, 0, e);
        let spare = serpent(&mut f, 0, location::GRAVE, 1);
        f.infos.turn_player = 1;
        f.infos.phase = phases::END;
        let brake = f
            .field_effects
            .continuous
            .equal_range(code::PHASE | u32::from(phases::END))[0];
        f.core.current_chain.clear();
        drive(
            &mut f,
            0,
            brake,
            Kind::ExecuteOperation {
                resume: None,
                effect: brake,
                player: 0,
                subject: None,
                args: Vec::new(),
                was_disabled: false,
            },
        );
        assert_eq!(f.cards[spare].current.location, location::REMOVED);
        assert_eq!(
            f.cards[spare].current.position,
            position::FACEUP,
            "banished face-up"
        );
        assert!(
            f.messages.iter().any(|m| matches!(
                m,
                Message::Hint { kind, player: 0, value }
                    if *kind == hint::SELECTMSG && *value == hintmsg::REMOVE
            )),
            "prompted with the banish message"
        );
    }

    /// **The elimination filter wants a monster in a graveyard.**
    ///
    /// `aux.SpElimFilter(c, true)` with nobody affected by Spirit
    /// Elimination reduces to "in the graveyard" — a copy sitting
    /// face-up on the field is not a legal choice for the brake.
    #[test]
    fn the_elimination_filter_wants_the_graveyard() {
        let (mut f, ss) = field();
        let on_field = serpent(&mut f, 0, location::MZONE, 0);
        assert!(sp_elim_filter(&mut f, ss), "the one in the graveyard");
        assert!(
            !sp_elim_filter(&mut f, on_field),
            "and not the one on the field"
        );
        // Face-down in the monster row is refused by the *first* clause,
        // not by the graveyard test — which matters because the two arms
        // answer the same way here and only differ once someone is
        // affected by Spirit Elimination. Asserting on the controller's
        // side of the question is what separates them.
        f.cards[on_field].current.position = position::FACEDOWN_DEFENSE;
        assert!(!sp_elim_filter(&mut f, on_field));
        assert_eq!(
            api::get_controler(&f, on_field),
            0,
            "the filter reads the *controller* to ask about elimination, \
             and a card in a player's row is controlled by them"
        );
        assert!(!rm_filter(&mut f, on_field));
        assert!(
            rm_filter(&mut f, ss),
            "a graveyard copy passes the whole filter"
        );
    }

    /// **The banish filter wants this card's name.** Another monster in
    /// the graveyard is not a candidate.
    #[test]
    fn the_banish_filter_wants_its_own_name() {
        let (mut f, ss) = field();
        let mut other = Card::with_data(
            CardData {
                code: 9_999,
                type_: card_type::MONSTER | card_type::NORMAL,
                ..Default::default()
            },
            0,
        );
        other.current.controller = 0;
        other.set_status(status::EFFECT_ENABLED, true);
        let other = f.new_card(other);
        f.add_card(0, other, location::GRAVE, 1, false);
        assert!(rm_filter(&mut f, ss));
        assert!(!rm_filter(&mut f, other), "a different card is not a copy");
    }

    /// **The trigger is keyed to the Standby Phase**, pinned against
    /// `constant.lua` so a renamed constant cannot silently move it.
    #[test]
    fn the_trigger_is_keyed_to_the_standby_phase() {
        assert_eq!(phases::STANDBY, 0x02, "PHASE_STANDBY");
        let (f, ss) = field();
        assert!(
            !f.cards[ss]
                .field_effect
                .equal_range(code::PHASE | u32::from(phases::STANDBY))
                .is_empty(),
            "registered under PHASE | PHASE_STANDBY"
        );
        assert!(
            f.cards[ss]
                .field_effect
                .equal_range(code::PHASE | u32::from(phases::END))
                .is_empty(),
            "and not under any other phase"
        );
    }
}

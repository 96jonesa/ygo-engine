//! Scapegoat (`73915051`) — `cardscripts/c73915051.lua`.
//!
//! ```text
//! Special Summon 4 "Sheep Tokens" (Beast-Type/EARTH/Level 1/ATK 0/DEF 0)
//! in Defense Position. You cannot Normal Summon or Special Summon the
//! turn you activate this card. These Tokens cannot be Tributed for a
//! Tribute Summon.
//! ```
//!
//! The pool's first **tokens**, and the first card whose cost is entirely
//! a set of prohibitions on its own controller.
//!
//! ## The cost is four effects, and one of them has a hole in it
//!
//! `s.cost` registers `EFFECT_CANNOT_SUMMON`, `EFFECT_CANNOT_FLIP_SUMMON`
//! and `EFFECT_CANNOT_SPECIAL_SUMMON` on the player, plus a fourth,
//! code-less effect that exists only to put the reminder text on the
//! client. All four are `EFFECT_FLAG_OATH`, so they lapse if the
//! activation is negated rather than sticking to a card that did nothing.
//!
//! The Special Summon prohibition carries a **target**, `s.sumlimit`,
//! and that target is what lets the card summon its own tokens through a
//! prohibition it just registered on itself:
//!
//! ```text
//! function s.sumlimit(e,c,sump,sumtype,sumpos,targetp,se)
//!     return e:GetLabelObject()~=se
//! end
//! ```
//!
//! `e:SetLabelObject(e)` points the prohibition at **the activation
//! itself**, and the filter refuses every summon except one whose
//! summoning effect is that one. A port that dropped the target would
//! make the prohibition unconditional and the card would summon nothing.
//!
//! ## Two questions asked twice
//!
//! `s.target` and `s.activate` ask the same three things — not Spirit's
//! Invitation, four free seats, and the token shape being summonable —
//! because between the target and the resolution anything may have
//! happened. The `> 3` is not `>= 4` in the reference and is not here
//! either; they are the same number and the code says what the script
//! says.

use crate::board::{location, position};
use crate::card::{attribute, card_type, race, CARD_BLUEEYES_SPIRIT};
use crate::effect::{effect_type, flag, Ctx, LabelObject, Yield};
use crate::event::{category, code, CardId};
use crate::field::{reset, timing, Field};
use crate::script_api as api;

pub const CODE: u32 = 73915051;

/// The Sheep Tokens are `id + 1` through `id + 4`.
pub const TOKEN_CODES: [u32; 4] = [CODE + 1, CODE + 2, CODE + 3, CODE + 4];

/// `TYPES_TOKEN` (`constant.lua`) — what the script describes the token
/// as when asking whether it could be summoned.
const TYPES_TOKEN: u32 = card_type::MONSTER | card_type::NORMAL | card_type::TOKEN;

/// The "cannot be Tributed for a Tribute Summon" reminder.
const UNRELEASABLE_HINT: u64 = 3304;

pub fn initial_effect(f: &mut Field, c: CardId) {
    let e1 = api::create_effect(f, c);
    api::set_category(f, e1, category::SPECIAL_SUMMON | category::TOKEN);
    api::set_type(f, e1, effect_type::ACTIVATE);
    api::set_code(f, e1, code::FREE_CHAIN);
    api::set_hint_timing(f, e1, 0, timing::END_PHASE);
    api::set_cost(f, e1, cost);
    api::set_target(f, e1, target);
    api::set_operation(f, e1, activate);
    api::register_effect(f, c, e1, false);
}

/// `s.sumlimit` — every Special Summon is forbidden **except** one made
/// by the effect this prohibition was registered for.
///
/// `args[4]` is the summoning effect, which the port's
/// `is_player_can_spsummon` pushes in the reference's `se` slot.
fn sumlimit(f: &Field, e: crate::event::EffectId, _c: Option<CardId>, args: &[i64]) -> bool {
    let se = args.get(4).copied().unwrap_or(-1);
    let ours = api::get_label_object_effect(f, e).map_or(-1, |x| x as i64);
    ours != se
}

/// `s.cost` — the tally check, then four oaths on the controller.
fn cost(f: &mut Field, ctx: &Ctx, chk: bool) -> bool {
    let tp = ctx.player;
    if !chk {
        return api::get_activity_count(f, tp, api::activity::SUMMON) == 0
            && api::get_activity_count(f, tp, api::activity::FLIPSUMMON) == 0
            && api::get_activity_count(f, tp, api::activity::SPSUMMON) == 0;
    }
    let Some(handler) = api::get_handler(f, ctx.reason_effect) else {
        return false;
    };
    let phase_end = reset::PHASE | u32::from(crate::duel::phases::END);

    // Cannot Special Summon — except by this very activation.
    let e1 = api::create_effect(f, handler);
    api::set_type(f, e1, effect_type::FIELD);
    api::set_property(f, e1, flag::PLAYER_TARGET | flag::OATH, 0);
    api::set_code(f, e1, code::CANNOT_SPECIAL_SUMMON);
    api::set_reset(f, e1, phase_end, 1);
    api::set_target_range(f, e1, 1, 0);
    api::set_label_object(f, e1, Some(LabelObject::Effect(ctx.reason_effect)));
    api::set_target_filter(f, e1, sumlimit);
    api::duel_register_effect(f, e1, tp);

    // Cannot Normal Summon, and cannot Flip Summon.
    for code_ in [code::CANNOT_SUMMON, code::CANNOT_FLIP_SUMMON] {
        let e = api::create_effect(f, handler);
        api::set_type(f, e, effect_type::FIELD);
        api::set_property(f, e, flag::PLAYER_TARGET | flag::OATH, 0);
        api::set_code(f, e, code_);
        api::set_reset(f, e, phase_end, 1);
        api::set_target_range(f, e, 1, 0);
        api::duel_register_effect(f, e, tp);
    }

    // **A fourth effect with no code at all**: it carries the reminder
    // text to the client and nothing else. Registered because the
    // reference registers it; a port that dropped it would produce a duel
    // that plays the same and reads differently.
    let e4 = api::create_effect(f, handler);
    api::set_property(
        f,
        e4,
        flag::PLAYER_TARGET | flag::CLIENT_HINT | flag::OATH,
        0,
    );
    api::set_description(f, e4, api::stringid(CODE, 1));
    api::set_reset(f, e4, phase_end, 1);
    api::set_target_range(f, e4, 1, 0);
    api::duel_register_effect(f, e4, tp);
    true
}

/// The three conditions `s.target` and `s.activate` both ask.
///
/// **`> 3`, not `>= 4`.** The same number, and the script's way of saying
/// it.
fn there_is_room_for_four(f: &mut Field, tp: u8) -> bool {
    !api::is_player_affected_by_effect(f, tp, CARD_BLUEEYES_SPIRIT)
        && api::get_location_count(f, tp, location::MZONE) > 3
        && api::is_player_can_special_summon_monster(
            f,
            tp,
            CODE + 1,
            TYPES_TOKEN,
            0,
            0,
            1,
            race::BEAST,
            attribute::EARTH,
        )
}

fn target(f: &mut Field, ctx: &Ctx, chk: bool, _chkc: Option<CardId>) -> Yield {
    let tp = ctx.player;
    if !chk {
        return api::yes(there_is_room_for_four(f, tp));
    }
    // Four of each, and the token category names no cards because they do
    // not exist yet.
    api::set_operation_info(f, 0, category::TOKEN, None, 4, 0, 0);
    api::set_operation_info(f, 0, category::SPECIAL_SUMMON, None, 4, tp, 0);
    api::yes(true)
}

/// `s.activate` — four tokens, **one at a time**.
///
/// `Duel.SpecialSummonStep` yields, and that is load-bearing: the token
/// is not on the field until the step has run, and the
/// `EFFECT_UNRELEASABLE_SUM` that follows is registered on a card that is
/// already there. Queueing all four and registering afterwards summons
/// nothing — the port did exactly that first, and the differential caught
/// it at the very first token.
///
/// So this is a loop of suspensions, like the incremental selections, and
/// the same shape: a named `fn` that re-enters itself with the index
/// carried in the continuation.
fn activate(f: &mut Field, ctx: &Ctx) -> Yield {
    let tp = ctx.player;
    if !there_is_room_for_four(f, tp) {
        return api::done();
    }
    summon_token(f, ctx, 0)
}

/// One pass of the reference's `for i=1,4 do ... end`.
fn summon_token(f: &mut Field, ctx: &Ctx, i: usize) -> Yield {
    let tp = ctx.player;
    let Some(&token_code) = TOKEN_CODES.get(i) else {
        api::special_summon_complete(f);
        return api::done();
    };
    let Some(token) = api::create_token(f, tp, token_code) else {
        return summon_token(f, ctx, i + 1);
    };
    api::special_summon_step(f, token, 0, tp, tp, false, false, position::FACEUP_DEFENSE);
    api::suspend(move |f, ctx| {
        let Some(handler) = api::get_handler(f, ctx.reason_effect) else {
            return api::done();
        };
        // Cannot be tributed for a Tribute Summon
        let e1 = api::create_effect(f, handler);
        api::set_description(f, e1, UNRELEASABLE_HINT);
        api::set_type(f, e1, effect_type::SINGLE);
        api::set_code(f, e1, code::UNRELEASABLE_SUM);
        api::set_property(f, e1, flag::CANNOT_DISABLE | flag::CLIENT_HINT, 0);
        api::set_value(f, e1, 1);
        api::set_reset(f, e1, reset::EVENT | crate::field::resets::STANDARD, 1);
        // `token:RegisterEffect(e1, true)` — **forced**, because the token
        // is mid-summon and registration would otherwise be refused.
        api::register_effect(f, token, e1, true);
        summon_token(f, ctx, i + 1)
    })
}

/// The printed line the tokens are made with — `TYPES_TOKEN`, Level 1,
/// 0/0, EARTH Beast. Kept here rather than guessed at the call site.
pub fn token_data(code_: u32) -> Option<crate::card::CardData> {
    TOKEN_CODES.contains(&code_).then(|| crate::card::CardData {
        code: code_,
        type_: TYPES_TOKEN,
        level: 1,
        attack: 0,
        defense: 0,
        attribute: attribute::EARTH,
        race: race::BEAST,
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{status, Card, CardData};
    use crate::chain::Chain;
    use crate::duel::phases;
    use crate::event::{EffectId, Event};
    use crate::field::Message;
    use crate::processor::{Kind, Status};

    fn vanilla(f: &mut Field, player: u8, seq: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 4400 + seq,
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
        f.cards[id].current.position = position::FACEUP_ATTACK;
        id
    }

    /// Scapegoat face-up in `tp`'s Spell row, with `mine` vanillas beside it.
    fn board(tp: u8, mine: u32) -> (Field, CardId) {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        f.infos.turn_player = tp;
        f.infos.phase = phases::MAIN1;
        let mut d = Card::with_data(crate::cards::card_data(CODE).expect("printed data"), tp);
        d.current.controller = tp;
        d.set_status(status::EFFECT_ENABLED, true);
        let sg = f.new_card(d);
        f.add_card(tp, sg, location::SZONE, 0, false);
        f.cards[sg].current.position = position::FACEUP;
        f.initialize_card(sg);
        for i in 0..mine {
            vanilla(&mut f, tp, i);
        }
        (f, sg)
    }

    fn effect_of(f: &Field, sg: CardId) -> EffectId {
        f.cards[sg].field_effect.equal_range(code::FREE_CHAIN)[0]
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

    fn as_reason(f: &mut Field, e: EffectId, tp: u8) {
        f.core.reason_effect = Some(e);
        f.core.reason_player = tp;
    }

    fn row(f: &Field, player: u8) -> Vec<CardId> {
        f.players[player as usize]
            .mzone
            .iter()
            .flatten()
            .copied()
            .collect()
    }

    /// Drive the cost, target and operation the way an activation does,
    /// answering every seat question with the lowest free one. Returns
    /// how many seat questions were put.
    fn resolve(f: &mut Field, e: EffectId, tp: u8) -> usize {
        let mut ch = Chain::new(e, Event::new(code::FREE_CHAIN));
        ch.triggering_player = tp;
        ch.chain_count = 1;
        ch.chain_id = 11;
        f.core.current_chain.push(ch);
        f.core.chain_solving = true;
        let ev = Event::new(code::FREE_CHAIN);
        as_reason(f, e, tp);
        cost(f, &ctx_for(e, &ev, tp), true);
        f.core
            .sub_solving_event
            .push_back(Event::new(code::FREE_CHAIN));
        f.emplace(Kind::ExecuteTarget {
            resume: None,
            effect: e,
            player: tp,
            subject: None,
            args: Vec::new(),
            was_disabled: false,
        });
        let mut operated = false;
        let mut seats = 0usize;
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
                            .push_back(Event::new(code::FREE_CHAIN));
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
                    Some(Message::SelectPlace { player, flag, .. }) => {
                        seats += 1;
                        let (pl, flag) = (*player, *flag);
                        let seq = (0..5u32)
                            .find(|s| flag & (1 << s) == 0)
                            .expect("a free monster seat");
                        f.core.returns.set_i8(0, pl as i8);
                        f.core.returns.set_i8(1, location::MZONE as i8);
                        f.core.returns.set_i8(2, seq as i8);
                    }
                    other => panic!("unexpected question: {other:?}"),
                },
                Status::End => break,
            }
        }
        seats
    }

    mod registration {
        use super::*;

        #[test]
        fn one_activation_in_two_categories() {
            let (f, sg) = board(0, 0);
            let e = f.effects.get(effect_of(&f, sg)).expect("the effect");
            assert!(e.is_type(effect_type::ACTIVATE));
            assert_eq!(e.code, code::FREE_CHAIN);
            assert_eq!(e.category, category::SPECIAL_SUMMON | category::TOKEN);
            assert_eq!(e.hint_timing, [0, timing::END_PHASE]);
            assert_eq!(
                crate::cards::card_data(CODE).expect("printed data").type_,
                card_type::SPELL | card_type::QUICKPLAY
            );
        }

        /// **The Sheep Tokens have printed lines of their own**, read out
        /// of the database like any other card rather than stated by the
        /// script.
        #[test]
        fn the_four_tokens_have_printed_lines() {
            for code_ in TOKEN_CODES {
                let d = crate::cards::card_data(code_).expect("a printed line");
                assert_eq!(d.type_, TYPES_TOKEN, "a normal monster token");
                assert_eq!((d.level, d.attack, d.defense), (1, 0, 0));
                assert_eq!(d.attribute, attribute::EARTH);
                assert_eq!(d.race, race::BEAST);
            }
            assert_eq!(TOKEN_CODES, [CODE + 1, CODE + 2, CODE + 3, CODE + 4]);
        }
    }

    mod the_cost {
        use super::*;

        fn payable(f: &mut Field, sg: CardId) -> bool {
            let e = effect_of(f, sg);
            let ev = Event::new(code::FREE_CHAIN);
            cost(f, &ctx_for(e, &ev, 0), false)
        }

        /// **All three tallies, and each on its own.**
        #[test]
        fn any_summon_this_turn_refuses_it() {
            let (mut f, sg) = board(0, 0);
            assert!(payable(&mut f, sg), "the positive sibling");
            for which in [0usize, 1, 2] {
                let (mut f, sg) = board(0, 0);
                match which {
                    0 => f.core.summon_state_count[0] = 1,
                    1 => f.core.flipsummon_state_count[0] = 1,
                    _ => f.core.spsummon_state_count[0] = 1,
                }
                assert!(!payable(&mut f, sg), "tally {which}");
            }
        }

        /// It is **this** player's tally that matters.
        #[test]
        fn the_opponents_summons_do_not_matter() {
            let (mut f, sg) = board(0, 0);
            f.core.summon_state_count[1] = 1;
            f.core.spsummon_state_count[1] = 1;
            assert!(payable(&mut f, sg));
        }

        /// **Four effects, and the fourth carries only text.**
        #[test]
        fn paying_registers_four_oaths_on_the_controller() {
            let (mut f, sg) = board(0, 0);
            let e = effect_of(&f, sg);
            as_reason(&mut f, e, 0);
            let ev = Event::new(code::FREE_CHAIN);
            assert!(cost(&mut f, &ctx_for(e, &ev, 0), true));

            let registered: Vec<EffectId> = f
                .field_effects
                .aura
                .iter()
                .map(|&(_, id)| id)
                .filter(|&id| f.effects.get(id).is_some_and(|x| x.is_flag(flag::OATH)))
                .collect();
            assert_eq!(registered.len(), 4, "three prohibitions and the reminder");

            let codes: std::collections::BTreeSet<u32> = registered
                .iter()
                .filter_map(|&id| f.effects.get(id).map(|x| x.code))
                .collect();
            assert!(codes.contains(&code::CANNOT_SPECIAL_SUMMON));
            assert!(codes.contains(&code::CANNOT_SUMMON));
            assert!(codes.contains(&code::CANNOT_FLIP_SUMMON));
            assert!(codes.contains(&0), "the reminder has no code at all");

            for id in &registered {
                let x = f.effects.get(*id).expect("the effect");
                assert!(x.is_flag(flag::PLAYER_TARGET), "aimed at a player");
                assert_eq!((x.s_range, x.o_range), (1, 0), "at its own controller");
                assert_ne!(x.reset_flag, 0, "and it expires");
            }
        }

        /// **The hole in the prohibition**: it refuses every Special
        /// Summon except one made by the activation that registered it.
        #[test]
        fn the_special_summon_prohibition_lets_its_own_activation_through() {
            let (mut f, sg) = board(0, 0);
            let e = effect_of(&f, sg);
            as_reason(&mut f, e, 0);
            let ev = Event::new(code::FREE_CHAIN);
            cost(&mut f, &ctx_for(e, &ev, 0), true);

            let limit = f
                .field_effects
                .aura
                .equal_range(code::CANNOT_SPECIAL_SUMMON)[0];
            let ours = e as i64;
            // args: player, sumtype, sumpos, toplayer, summoning effect.
            assert!(
                !sumlimit(&f, limit, None, &[0, 0, 0, 0, ours]),
                "its own activation is allowed through"
            );
            assert!(
                sumlimit(&f, limit, None, &[0, 0, 0, 0, ours + 1]),
                "and everything else is forbidden"
            );
        }
    }

    mod the_room_check {
        use super::*;

        fn offered(f: &mut Field, sg: CardId) -> bool {
            let e = effect_of(f, sg);
            as_reason(f, e, 0);
            let ev = Event::new(code::FREE_CHAIN);
            target(f, &ctx_for(e, &ev, 0), false, None).finished() == Some(1)
        }

        /// **Four free seats, not three.**
        #[test]
        fn it_wants_four_free_seats() {
            let (mut f, sg) = board(0, 1);
            assert!(offered(&mut f, sg), "four seats left");
            let (mut f, sg) = board(0, 2);
            assert!(!offered(&mut f, sg), "only three");
        }

        #[test]
        fn spirits_invitation_stops_it() {
            let (mut f, sg) = board(0, 0);
            assert!(offered(&mut f, sg), "the positive sibling");

            let (mut f, sg) = board(0, 0);
            let holder = vanilla(&mut f, 0, 4);
            let e = api::create_effect(&mut f, holder);
            api::set_type(&mut f, e, effect_type::FIELD);
            api::set_code(&mut f, e, CARD_BLUEEYES_SPIRIT);
            api::set_range(&mut f, e, u16::from(location::MZONE));
            api::set_property(&mut f, e, flag::PLAYER_TARGET, 0);
            api::set_target_range(&mut f, e, 1, 0);
            api::register_effect(&mut f, holder, e, false);
            assert!(!offered(&mut f, sg));
        }

        /// **A prohibition that reads the card sees a Beast** — which
        /// only works because the question is asked with the token's
        /// shape rather than a blank stand-in.
        #[test]
        fn a_prohibition_that_reads_the_shape_sees_a_beast() {
            fn refuses_beasts(f: &Field, _e: EffectId, c: Option<CardId>, _a: &[i64]) -> bool {
                c.is_some_and(|c| f.cards[c].data.race == race::BEAST)
            }
            fn refuses_dragons(f: &Field, _e: EffectId, c: Option<CardId>, _a: &[i64]) -> bool {
                c.is_some_and(|c| f.cards[c].data.race == race::DRAGON)
            }

            for (filter, want) in [
                (refuses_beasts as crate::effect::TargetFilter, false),
                (refuses_dragons as crate::effect::TargetFilter, true),
            ] {
                let (mut f, sg) = board(0, 0);
                let holder = vanilla(&mut f, 0, 4);
                let e = api::create_effect(&mut f, holder);
                api::set_type(&mut f, e, effect_type::FIELD);
                api::set_code(&mut f, e, code::CANNOT_SPECIAL_SUMMON);
                api::set_range(&mut f, e, u16::from(location::MZONE));
                api::set_property(&mut f, e, flag::PLAYER_TARGET, 0);
                api::set_target_range(&mut f, e, 1, 0);
                api::set_target_filter(&mut f, e, filter);
                api::register_effect(&mut f, holder, e, false);
                assert_eq!(offered(&mut f, sg), want, "a Beast token, filtered by race");
            }
        }

        /// The target records four of each, against the activating player.
        #[test]
        fn the_target_records_four_tokens_and_four_summons() {
            let (mut f, sg) = board(1, 0);
            let e = effect_of(&f, sg);
            let mut ch = Chain::new(e, Event::new(code::FREE_CHAIN));
            ch.triggering_player = 1;
            ch.chain_count = 1;
            f.core.current_chain.push(ch);
            as_reason(&mut f, e, 1);
            let ev = Event::new(code::FREE_CHAIN);
            target(&mut f, &ctx_for(e, &ev, 1), true, None);
            let ops = &f.core.current_chain[0].opinfos;
            assert_eq!(ops.get(&category::TOKEN).expect("tokens").count, 4);
            let sp = ops.get(&category::SPECIAL_SUMMON).expect("summons");
            assert_eq!(sp.count, 4);
            assert_eq!(sp.player, 1, "the activating player");
        }
    }

    mod resolving {
        use super::*;

        /// **Four tokens, face-up defence, in the controller's row** —
        /// each with the tribute prohibition on it.
        #[test]
        fn it_summons_four_sheep() {
            let (mut f, sg) = board(0, 0);
            let e = effect_of(&f, sg);
            let seats = resolve(&mut f, e, 0);
            assert_eq!(seats, 4, "a seat asked for each");
            assert!(
                f.core.special_summoning.is_empty(),
                "and the batch was completed rather than left open"
            );
            let mine = row(&f, 0);
            assert_eq!(mine.len(), 4, "four of them");
            assert!(row(&f, 1).is_empty(), "and none for the opponent");
            let mut codes: Vec<u32> = mine.iter().map(|&c| f.cards[c].data.code).collect();
            codes.sort_unstable();
            assert_eq!(codes, TOKEN_CODES.to_vec(), "one of each");
            for &t in &mine {
                assert_eq!(f.cards[t].current.position, position::FACEUP_DEFENSE);
                assert_eq!(
                    f.cards[t]
                        .single_effect
                        .equal_range(code::UNRELEASABLE_SUM)
                        .len(),
                    1,
                    "cannot be tributed for a Tribute Summon"
                );
            }
        }

        /// **A board that changed since the target is checked again.**
        #[test]
        fn a_row_filled_after_the_target_summons_nothing() {
            let (mut f, sg) = board(0, 0);
            let e = effect_of(&f, sg);
            for i in 0..5 {
                vanilla(&mut f, 0, i);
            }
            let seats = resolve(&mut f, e, 0);
            assert_eq!(row(&f, 0).len(), 5, "the vanillas, and no tokens");
            assert_eq!(seats, 0, "and no seat was ever asked for");
        }

        /// The tokens go to the **activating** player, whoever that is.
        #[test]
        fn the_tokens_belong_to_the_activating_player() {
            let (mut f, sg) = board(1, 0);
            let e = effect_of(&f, sg);
            resolve(&mut f, e, 1);
            assert_eq!(row(&f, 1).len(), 4);
            assert!(row(&f, 0).is_empty());
        }
    }
}

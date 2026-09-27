//! Enemy Controller (`98045062`) — `cardscripts/c98045062.lua`.
//!
//! ```text
//! Activate 1 of these effects.
//! • Target 1 face-up monster your opponent controls; change that
//!   target's battle position.
//! • Tribute 1 monster, then target 1 face-up monster your opponent
//!   controls; take control of that target until the End Phase.
//! ```
//!
//! The pool's first card with **two effects behind one activation**, and
//! the first to release a card as a cost — through
//! [`super::aux_release_cost`], which is the second incremental selection
//! in the library and shares `Group.SelectUnselect` with the first.
//!
//! ## The label is a state machine, not a note
//!
//! `e:SetLabel` carries which branch is in play, and it is written from
//! three places:
//!
//! | written | by | meaning |
//! |---|---|---|
//! | `9` | `s.cost`, on **both** the asking and the paying call | the tribute is available |
//! | `0` | `s.target` at `chk == 0`, after reading it | nothing chosen yet |
//! | `1` or `2` | `s.target` at `chk == 1` | the branch the player picked |
//!
//! The nine is the interesting one. `s.cost` has no `chk` guard, so it
//! runs before either kind of `s.target` call — and `b2` reads it to
//! decide **which question it is answering**: with the cost in hand,
//! "could I tribute something and take a monster"; without it, "is there
//! already room to take one". Those are different boards. Reading the
//! label as a plain scratch value, or resetting it in the wrong place,
//! silently swaps the two.
//!
//! `chkc` and `s.activate` then both read it to tell the position branch
//! from the control branch, which is why it is written *last* in the
//! target.
//!
//! ## `cfilter` asks about the seat it is about to free
//!
//! The monster being tributed is the one making room, so the filter asks
//! `Duel.GetMZoneCount(tp, c, tp, LOCATION_REASON_CONTROL)` — counting
//! **without** that card — and pairs it with
//! `IsControlerCanBeChanged(..., true)`, the `ignore_mzone` form. Both
//! halves are about the same seat, from opposite sides.

use super::aux_release_cost as rel;
use crate::board::{location, position};
use crate::card::reason;
use crate::duel::phases;
use crate::effect::{effect_type, flag, Ctx, Yield};
use crate::event::{category, code, CardId};
use crate::field::{timing, Field};
use crate::host_question::{hint, hintmsg};
use crate::script_api as api;

pub const CODE: u32 = 98045062;

const MZONE: u32 = location::MZONE as u32;

/// The label `s.cost` writes, meaning "the tribute is available".
const COST_PAID: i64 = 9;
/// The label for the battle-position branch.
const POSITION_BRANCH: i64 = 1;

pub fn initial_effect(f: &mut Field, c: CardId) {
    let e1 = api::create_effect(f, c);
    api::set_description(f, e1, api::stringid(CODE, 0));
    api::set_type(f, e1, effect_type::ACTIVATE);
    api::set_property(f, e1, flag::CARD_TARGET, 0);
    api::set_code(f, e1, code::FREE_CHAIN);
    api::set_hint_timing(
        f,
        e1,
        timing::BATTLE_PHASE | timing::STANDBY_PHASE,
        timing::BATTLE_PHASE,
    );
    api::set_cost(f, e1, cost);
    api::set_target(f, e1, target);
    api::set_operation(f, e1, activate);
    api::register_effect(f, c, e1, false);
}

/// `s.cost` — **no `chk` guard**: it marks the label on the asking call
/// as well as the paying one, and that is deliberate. See the module
/// note.
fn cost(f: &mut Field, ctx: &Ctx, _chk: bool) -> bool {
    api::set_label(f, ctx.reason_effect, vec![COST_PAID]);
    true
}

/// A face-up monster whose controller could be changed.
fn faceup_controler_can_be_changed(f: &mut Field, c: CardId) -> bool {
    api::is_faceup(f, c) && api::is_controler_can_be_changed(f, c, false)
}

/// The same, asked **ignoring the Monster Zone** — for a card that is
/// about to free one.
fn faceup_controler_can_be_changed_ignoring_zone(f: &mut Field, c: CardId) -> bool {
    api::is_faceup(f, c) && api::is_controler_can_be_changed(f, c, true)
}

fn faceup_can_change_position(f: &mut Field, c: CardId) -> bool {
    api::is_faceup(f, c) && api::is_can_change_position(f, c)
}

/// `s.cfilter` — could **this** card be the tribute?
///
/// Both halves are about the seat it would free: enough room once it is
/// gone, and an opponent's monster that could come over into that room.
fn cfilter(f: &mut Field, c: CardId, tp: u8) -> bool {
    api::get_mzone_count(
        f,
        tp,
        api::Except::Card(c),
        Some(tp),
        Some(api::LOCATION_REASON_CONTROL),
    ) > 0
        && api::is_existing_target(
            f,
            Some(&faceup_controler_can_be_changed_ignoring_zone),
            tp,
            0,
            MZONE,
            1,
            api::Except::Card(c),
        )
}

/// The tribute pool: the release group, matched by `s.cfilter`.
fn tribute_pool(f: &mut Field, tp: u8) -> Vec<CardId> {
    let group = api::get_release_group(f, tp, false, false, None);
    group
        .into_iter()
        .filter(|&c| cfilter(f, c, tp))
        .collect::<Vec<_>>()
}

/// `s.target`.
fn target(f: &mut Field, ctx: &Ctx, chk: bool, chkc: Option<CardId>) -> Yield {
    let (tp, e) = (ctx.player, ctx.reason_effect);
    if let Some(chkc) = chkc {
        if !(api::is_location(f, chkc, u16::from(location::MZONE))
            && api::is_faceup(f, chkc)
            && api::is_controler(f, chkc, 1 - tp))
        {
            return api::yes(false);
        }
        return api::yes(if api::get_label_of(f, e) == POSITION_BRANCH {
            api::is_can_change_position(f, chkc)
        } else {
            api::is_controler_can_be_changed(f, chkc, false)
        });
    }

    let b1 = api::is_existing_target(
        f,
        Some(&faceup_can_change_position),
        tp,
        0,
        MZONE,
        1,
        api::Except::None,
    );
    let b2 = if api::get_label_of(f, e) == COST_PAID {
        let mg = tribute_pool(f, tp);
        rel::check_release_group_cost(f, &mg, tp, rel::Cost::of(1, 1).borrowed())
    } else {
        api::get_location_count_for(f, tp, location::MZONE, tp, api::LOCATION_REASON_CONTROL) > 0
            && api::is_existing_target(
                f,
                Some(&faceup_controler_can_be_changed),
                tp,
                0,
                MZONE,
                1,
                api::Except::None,
            )
    };

    if !chk {
        api::set_label(f, e, vec![0]);
        return api::yes(b1 || b2);
    }

    let Some(sel) = api::select_effect(
        f,
        tp,
        &[(b1, api::stringid(CODE, 1)), (b2, api::stringid(CODE, 2))],
    ) else {
        return api::yes(false);
    };
    api::suspend(move |f, ctx| {
        let (tp, e) = (ctx.player, ctx.reason_effect);
        let answer = api::selected_option(f, tp, true);
        let op = sel[answer.max(0) as usize] as i64;
        if op == POSITION_BRANCH {
            api::set_category(f, e, category::POSITION);
            api::hint(f, hint::SELECTMSG, tp, hintmsg::POSCHANGE);
            api::select_target(
                f,
                tp,
                Some(&faceup_can_change_position),
                tp,
                0,
                MZONE,
                1,
                1,
                api::Except::None,
            );
            return api::suspend(move |f, ctx| {
                let g = api::selected_targets(f);
                api::set_operation_info(f, 0, category::POSITION, g, 1, 0, 0);
                api::set_label(f, ctx.reason_effect, vec![op]);
                api::yes(true)
            });
        }
        api::set_category(f, e, category::CONTROL);
        if api::get_label_of(f, e) == COST_PAID {
            let mg = tribute_pool(f, tp);
            return rel::select_release_group_cost(f, ctx, mg, tp, rel::Cost::of(1, 1), released);
        }
        take_control_target(f, ctx)
    })
}

/// The continuation after the tribute is chosen: pay it, then name the
/// monster to take.
/// `Duel.Release(rg,REASON_COST)` **yields** in the reference: the Lua
/// coroutine does not continue to `SelectTarget` until the tribute has
/// actually left the field. That order is load-bearing — the target
/// filter asks whether the opponent's monster *can change control*, and
/// that includes whether this player has a seat for it, which the tribute
/// is about to free. Continuing synchronously asked the question with the
/// tribute still seated, found no legal target on a full board, and
/// skipped the selection ocgcore makes (fuzz seeds 2, 6, 11).
fn released(f: &mut Field, _ctx: &Ctx, rg: Vec<CardId>) -> Yield {
    api::release(f, rg, reason::COST, None);
    api::suspend(take_control_target)
}

/// The control branch's tail, shared by the paying and non-paying paths.
fn take_control_target(f: &mut Field, ctx: &Ctx) -> Yield {
    let tp = ctx.player;
    api::hint(f, hint::SELECTMSG, tp, hintmsg::CONTROL);
    api::select_target(
        f,
        tp,
        Some(&faceup_controler_can_be_changed),
        tp,
        0,
        MZONE,
        1,
        1,
        api::Except::None,
    );
    api::suspend(move |f, ctx| {
        let g = api::selected_targets(f);
        api::set_operation_info(f, 0, category::CONTROL, g, 1, 0, 0);
        // Always branch two here: the position branch returned earlier.
        api::set_label(f, ctx.reason_effect, vec![2]);
        api::yes(true)
    })
}

/// `s.activate` — the same three tests whichever branch was chosen, and
/// then the label decides which one runs.
fn activate(f: &mut Field, ctx: &Ctx) -> Yield {
    let (tp, e) = (ctx.player, ctx.reason_effect);
    let Some(tc) = api::get_first_target(f) else {
        return api::done();
    };
    if !(api::is_relate_to_effect(f, tc, e)
        && api::is_faceup(f, tc)
        && api::is_controler(f, tc, 1 - tp))
    {
        return api::done();
    }
    if api::get_label_of(f, e) == POSITION_BRANCH {
        // Face-up attack turns to defence and face-up defence to attack;
        // the two face-down slots are zero, meaning "leave it alone".
        api::change_position_each(
            f,
            vec![tc],
            position::FACEUP_DEFENSE,
            0,
            position::FACEUP_ATTACK,
            0,
        );
    } else {
        api::get_control(f, vec![tc], tp, phases::END, 1);
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

    fn monster(f: &mut Field, player: u8, seq: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 5500 + seq + u32::from(player) * 50,
                type_: card_type::MONSTER | card_type::NORMAL,
                level: 4,
                attack: 1200,
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

    /// Enemy Controller face-up in `tp`'s Spell row, with monsters on
    /// both sides.
    fn board(tp: u8, mine: u32, theirs: u32) -> (Field, CardId, Vec<CardId>, Vec<CardId>) {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        f.infos.turn_player = tp;
        f.infos.phase = crate::duel::phases::MAIN1;
        let mut d = Card::with_data(crate::cards::card_data(CODE).expect("printed data"), tp);
        d.current.controller = tp;
        d.set_status(status::EFFECT_ENABLED, true);
        let ec = f.new_card(d);
        f.add_card(tp, ec, location::SZONE, 0, false);
        f.cards[ec].current.position = position::FACEUP;
        f.initialize_card(ec);
        let ours = (0..mine).map(|i| monster(&mut f, tp, i)).collect();
        let theirs = (0..theirs).map(|i| monster(&mut f, 1 - tp, i)).collect();
        (f, ec, ours, theirs)
    }

    fn effect_of(f: &Field, ec: CardId) -> EffectId {
        f.cards[ec].field_effect.equal_range(code::FREE_CHAIN)[0]
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

    #[derive(Default)]
    struct Run {
        /// Each `SelectOption`: who, and the descriptions.
        options: Vec<(u8, Vec<u64>)>,
        /// Each incremental release question: offered, taken.
        releases: Vec<(Vec<CardId>, Vec<CardId>)>,
        /// Each `SelectCard`: who, and what they were offered.
        card_questions: Vec<(u8, Vec<CardId>)>,
        hints: Vec<(u8, u8, u64)>,
        accepted: bool,
    }

    /// Drive the target and then the operation, answering with the given
    /// option index and card choices.
    fn resolve(f: &mut Field, e: EffectId, tp: u8, option: usize, picks: &[CardId]) -> Run {
        let mut ch = Chain::new(e, Event::new(code::FREE_CHAIN));
        ch.triggering_player = tp;
        ch.chain_count = 1;
        ch.chain_id = 11;
        f.core.current_chain.push(ch);
        f.core.chain_solving = true;
        // The cost runs before the target, as it does in play.
        let ev = Event::new(code::FREE_CHAIN);
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
        let mut run = Run::default();
        let (mut seen, mut next) = (0usize, 0usize);
        let mut operated = false;
        for _ in 0..8192 {
            while seen < f.messages.len() {
                if let Message::Hint {
                    kind,
                    player,
                    value,
                } = &f.messages[seen]
                {
                    run.hints.push((*kind, *player, *value));
                }
                seen += 1;
            }
            match f.process() {
                Status::Continue => {
                    if f.core.units.is_empty() && f.core.subunits.is_empty() {
                        if operated {
                            break;
                        }
                        operated = true;
                        run.accepted = f.core.returns.get() != 0;
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
                    Some(Message::SelectOption { player, options }) => {
                        run.options.push((*player, options.clone()));
                        f.core.returns.set(option.min(options.len() - 1) as i32);
                    }
                    Some(Message::SelectUnselectCard {
                        select, unselect, ..
                    }) => {
                        run.releases.push((select.clone(), unselect.clone()));
                        f.core.return_cards.clear();
                        f.core.returns.set_i32(0, 1);
                        f.core.returns.set_i32(1, 0);
                    }
                    Some(Message::SelectCard {
                        player, min, cards, ..
                    }) => {
                        run.card_questions.push((*player, cards.clone()));
                        let min = usize::from(*min);
                        let pick = picks
                            .get(next)
                            .and_then(|c| cards.iter().position(|x| x == c))
                            .unwrap_or(0);
                        next += 1;
                        f.core.return_cards.clear();
                        f.core.returns.set_i32(0, 0);
                        f.core.returns.set_i32(1, min as i32);
                        for i in 0..min {
                            f.core.returns.set_i32(i + 2, (pick + i) as i32);
                        }
                    }
                    // Taking control needs a seat on the new side.
                    Some(Message::SelectPlace { player, flag, .. }) => {
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
        run
    }

    mod registration {
        use super::*;

        /// **One activate effect that targets**, with the hint timings
        /// that let it be played in the Battle Phase.
        #[test]
        fn one_targeting_activation_with_battle_phase_timings() {
            let (f, ec, _, _) = board(0, 0, 0);
            let e = f.effects.get(effect_of(&f, ec)).expect("the effect");
            assert!(e.is_type(effect_type::ACTIVATE));
            assert_eq!(e.code, code::FREE_CHAIN);
            assert!(e.is_flag(flag::CARD_TARGET));
            assert_eq!(e.description, api::stringid(CODE, 0));
            assert_eq!(
                e.hint_timing,
                [
                    timing::BATTLE_PHASE | timing::STANDBY_PHASE,
                    timing::BATTLE_PHASE
                ],
                "own turn and the opponent's"
            );
            assert_eq!(
                crate::cards::card_data(CODE).expect("printed data").type_,
                card_type::SPELL | card_type::QUICKPLAY,
                "a Quick-Play, which is what lets it be played in a battle"
            );
        }
    }

    mod the_two_branches {
        use super::*;

        /// **The cost marks the label on the asking call too**, and that
        /// is what makes `b2` the "could I tribute" question rather than
        /// the "is there already room" one.
        #[test]
        fn the_cost_marks_the_label_whether_paying_or_asking() {
            let (mut f, ec, _, _) = board(0, 1, 1);
            let e = effect_of(&f, ec);
            let ev = Event::new(code::FREE_CHAIN);
            assert!(cost(&mut f, &ctx_for(e, &ev, 0), false));
            assert_eq!(api::get_label_of(&f, e), COST_PAID, "on the asking call");
            api::set_label(&mut f, e, vec![0]);
            assert!(cost(&mut f, &ctx_for(e, &ev, 0), true));
            assert_eq!(api::get_label_of(&f, e), COST_PAID, "and on the paying one");
        }

        /// **And the asking target puts it back to zero**, so a later
        /// `chkc` does not read a stale nine as a branch.
        #[test]
        fn asking_the_target_clears_the_label() {
            let (mut f, ec, _, _) = board(0, 1, 1);
            let e = effect_of(&f, ec);
            as_reason(&mut f, e, 0);
            let ev = Event::new(code::FREE_CHAIN);
            cost(&mut f, &ctx_for(e, &ev, 0), false);
            target(&mut f, &ctx_for(e, &ev, 0), false, None);
            assert_eq!(api::get_label_of(&f, e), 0);
        }

        /// A face-up opponent's monster is enough for the position
        /// branch, with no tribute anywhere.
        #[test]
        fn the_position_branch_needs_only_an_opponents_monster() {
            let (mut f, ec, _, _) = board(0, 0, 1);
            let e = effect_of(&f, ec);
            as_reason(&mut f, e, 0);
            let ev = Event::new(code::FREE_CHAIN);
            assert_eq!(
                target(&mut f, &ctx_for(e, &ev, 0), false, None).finished(),
                Some(1)
            );
        }

        /// **Nothing on the opponent's side means neither branch.**
        #[test]
        fn an_empty_opposing_row_refuses_both() {
            let (mut f, ec, _, _) = board(0, 2, 0);
            let e = effect_of(&f, ec);
            as_reason(&mut f, e, 0);
            let ev = Event::new(code::FREE_CHAIN);
            cost(&mut f, &ctx_for(e, &ev, 0), false);
            assert_eq!(
                target(&mut f, &ctx_for(e, &ev, 0), false, None).finished(),
                Some(0)
            );
        }

        /// **The `chkc` arm reads the label to know which branch it is
        /// validating.** The two questions are different, and a target
        /// legal for one need not be legal for the other.
        #[test]
        fn the_card_target_check_follows_the_branch() {
            let (mut f, ec, _, theirs) = board(0, 1, 1);
            let e = effect_of(&f, ec);
            as_reason(&mut f, e, 0);
            let ev = Event::new(code::FREE_CHAIN);
            let tc = theirs[0];

            api::set_label(&mut f, e, vec![POSITION_BRANCH]);
            assert_eq!(
                target(&mut f, &ctx_for(e, &ev, 0), false, Some(tc)).finished(),
                Some(1)
            );
            api::set_label(&mut f, e, vec![2]);
            assert_eq!(
                target(&mut f, &ctx_for(e, &ev, 0), false, Some(tc)).finished(),
                Some(1),
                "the positive sibling for the control branch"
            );

            // A monster summoned this turn: an effect may still turn it
            // over, so the position branch accepts it.
            f.cards[tc].set_status(status::SUMMON_TURN, true);
            api::set_label(&mut f, e, vec![POSITION_BRANCH]);
            assert_eq!(
                target(&mut f, &ctx_for(e, &ev, 0), false, Some(tc)).finished(),
                Some(1),
                "the effect question, not the rule one"
            );
        }

        /// Our own monster is never a legal target, whichever branch.
        #[test]
        fn our_own_monsters_are_not_targets() {
            let (mut f, ec, ours, _) = board(0, 1, 1);
            let e = effect_of(&f, ec);
            as_reason(&mut f, e, 0);
            let ev = Event::new(code::FREE_CHAIN);
            for label in [POSITION_BRANCH, 2] {
                api::set_label(&mut f, e, vec![label]);
                assert_eq!(
                    target(&mut f, &ctx_for(e, &ev, 0), false, Some(ours[0])).finished(),
                    Some(0),
                    "label {label}"
                );
            }
        }

        /// **The two branches ask different questions of the same
        /// card.** Filling our own row leaves nowhere for a monster to
        /// come to, so the control branch refuses a target the position
        /// branch accepts.
        #[test]
        fn the_two_branches_disagree_when_there_is_no_room() {
            let (mut f, ec, _, theirs) = board(0, 5, 1);
            let e = effect_of(&f, ec);
            as_reason(&mut f, e, 0);
            let ev = Event::new(code::FREE_CHAIN);
            let tc = theirs[0];
            api::set_label(&mut f, e, vec![POSITION_BRANCH]);
            assert_eq!(
                target(&mut f, &ctx_for(e, &ev, 0), false, Some(tc)).finished(),
                Some(1),
                "its position can still be changed"
            );
            api::set_label(&mut f, e, vec![2]);
            assert_eq!(
                target(&mut f, &ctx_for(e, &ev, 0), false, Some(tc)).finished(),
                Some(0),
                "but there is nowhere for it to go"
            );
        }

        /// **Without the cost in hand, `b2` is the other question**, and
        /// the two answer differently on a full row: with a tribute
        /// available the control branch is offered, and without one it is
        /// not. Visible in the branch list the player is shown.
        #[test]
        fn the_label_decides_which_question_b2_asks() {
            // A full row of our own, one monster to take. A tribute frees
            // a seat, so the control branch is live — *if* the label says
            // the cost is in hand.
            let (mut f, ec, _, _) = board(0, 5, 1);
            let e = effect_of(&f, ec);
            let with_cost = branches_offered(&mut f, e, true);
            assert_eq!(
                with_cost.len(),
                2,
                "the tribute would make room, so both branches"
            );

            let (mut f, ec, _, _) = board(0, 5, 1);
            let e = effect_of(&f, ec);
            let without = branches_offered(&mut f, e, false);
            assert_eq!(
                without,
                vec![api::stringid(CODE, 1)],
                "no cost in hand, and the row is full, so only the position branch"
            );
        }

        /// Drive the target to the point of the branch question and
        /// report the descriptions offered, optionally skipping the cost.
        fn branches_offered(f: &mut Field, e: EffectId, pay: bool) -> Vec<u64> {
            let mut ch = Chain::new(e, Event::new(code::FREE_CHAIN));
            ch.triggering_player = 0;
            ch.chain_count = 1;
            ch.chain_id = 11;
            f.core.current_chain.push(ch);
            f.core.chain_solving = true;
            let ev = Event::new(code::FREE_CHAIN);
            if pay {
                cost(f, &ctx_for(e, &ev, 0), true);
            } else {
                api::set_label(f, e, vec![0]);
            }
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
            for _ in 0..4096 {
                match f.process() {
                    Status::Continue => {
                        if f.core.units.is_empty() && f.core.subunits.is_empty() {
                            break;
                        }
                    }
                    Status::Awaiting => {
                        if let Some(Message::SelectOption { options, .. }) = f.messages.last() {
                            return options.clone();
                        }
                        panic!("expected the branch question first");
                    }
                    Status::End => break,
                }
            }
            Vec::new()
        }

        /// A face-down monster is not a target either.
        #[test]
        fn a_face_down_monster_is_not_a_target() {
            let (mut f, ec, _, theirs) = board(0, 1, 1);
            f.cards[theirs[0]].current.position = position::FACEDOWN_DEFENSE;
            let e = effect_of(&f, ec);
            as_reason(&mut f, e, 0);
            let ev = Event::new(code::FREE_CHAIN);
            assert_eq!(
                target(&mut f, &ctx_for(e, &ev, 0), false, Some(theirs[0])).finished(),
                Some(0)
            );
        }
    }

    mod the_tribute_filter {
        use super::*;

        /// **The seat is counted without the tribute and for a *control*
        /// change.** Both are what make a full row still workable: the
        /// monster leaving is the room, and taking a monster over is a
        /// different count from putting one down.
        #[test]
        fn it_counts_the_seat_the_tribute_would_free() {
            let (mut f, ec, ours, _) = board(0, 5, 1);
            let e = effect_of(&f, ec);
            as_reason(&mut f, e, 0);
            assert_eq!(
                api::get_location_count_for(
                    &mut f,
                    0,
                    location::MZONE,
                    0,
                    api::LOCATION_REASON_CONTROL
                ),
                0,
                "the row is full as it stands"
            );
            assert!(
                cfilter(&mut f, ours[0], 0),
                "but it is not full once this one goes"
            );
        }

        /// A cap on the Monster Zone that bites **only** when the count
        /// is asked for a control change, so the two reasons give
        /// different answers on the same board.
        fn only_for_control(_e: &crate::effect::Effect, _f: &Field, ctx: &Ctx) -> i64 {
            if ctx.args.get(2) == Some(&i64::from(api::LOCATION_REASON_CONTROL)) {
                0
            } else {
                5
            }
        }

        /// **The reason is `LOCATION_REASON_CONTROL`, not the default.**
        /// Counting seats "to put a card down" and "to take one over" are
        /// different counts, and only a cap keyed on the reason can tell
        /// the two calls apart.
        #[test]
        fn the_seat_is_counted_for_taking_a_monster_over() {
            let (mut f, ec, ours, _) = board(0, 1, 1);
            let e = effect_of(&f, ec);
            as_reason(&mut f, e, 0);
            assert!(cfilter(&mut f, ours[0], 0), "the positive sibling");

            let mut cap = crate::effect::Effect::new(effect_type::FIELD, code::MAX_MZONE);
            // The handler has to sit in the effect's own range, so the
            // cap rides on a monster rather than on the Spell.
            cap.owner = Some(ours[0]);
            cap.handler = Some(ours[0]);
            cap.effect_owner = 0;
            cap.flag[0] |= flag::PLAYER_TARGET | flag::FUNC_VALUE;
            cap.range = u16::from(location::MZONE);
            cap.s_range = u16::from(location::MZONE);
            cap.value_fn = Some(only_for_control);
            let id = f.new_effect(cap);
            f.field_effects.aura.insert(code::MAX_MZONE, id);
            f.field_effects.indexer.insert(id);

            assert!(
                !cfilter(&mut f, ours[0], 0),
                "the cap reaches the count only through the control reason"
            );
        }

        /// **The monster being tributed is not a target of itself.** It
        /// is ours; the scan is of the opponent's row with this card
        /// excepted, and an empty opposing row refuses.
        #[test]
        fn it_wants_an_opposing_monster_to_take() {
            let (mut f, ec, ours, _) = board(0, 2, 0);
            let e = effect_of(&f, ec);
            as_reason(&mut f, e, 0);
            assert!(
                !cfilter(&mut f, ours[0], 0),
                "nothing on the other side to take"
            );
        }

        /// **It asks ignoring the zone it is about to free.** With our
        /// row full, the plain question refuses and the `ignore_mzone`
        /// one does not — and the filter uses the latter.
        #[test]
        fn it_ignores_the_zone_it_is_about_to_free() {
            let (mut f, ec, ours, theirs) = board(0, 5, 1);
            let e = effect_of(&f, ec);
            as_reason(&mut f, e, 0);
            assert!(
                !faceup_controler_can_be_changed(&mut f, theirs[0]),
                "the plain question sees a full row"
            );
            assert!(
                faceup_controler_can_be_changed_ignoring_zone(&mut f, theirs[0]),
                "the ignoring one does not"
            );
            assert!(cfilter(&mut f, ours[0], 0), "and the filter uses that one");
        }
    }

    mod resolving {
        use super::*;

        /// **On a full board the target is asked after the tribute has
        /// left.** `Duel.Release` yields in the reference, and the target
        /// filter asks whether the opponent's monster can change control
        /// — which needs a seat on this side, the seat the tribute frees.
        /// Asked synchronously, with the tribute still seated, there was
        /// no legal target and the port skipped the question ocgcore asks
        /// (fuzz seeds 2, 6, 11).
        #[test]
        fn with_a_full_board_the_target_is_asked_after_the_tribute_has_left() {
            let (mut f, ec, mine, theirs) = board(0, 5, 1);
            assert_eq!(mine.len(), 5, "no free seat until something leaves");
            let e = effect_of(&f, ec);
            let run = resolve(&mut f, e, 0, 1, &[theirs[0]]);
            assert_eq!(run.releases.len(), 1, "the tribute was chosen");
            assert_eq!(
                run.card_questions.len(),
                1,
                "and then, with the seat freed, the control target was asked"
            );
            assert_eq!(run.card_questions[0].1, vec![theirs[0]]);
            assert!(run.accepted);
        }

        /// **Branch one turns the monster over** and takes no tribute.
        #[test]
        fn the_position_branch_turns_the_target_over() {
            let (mut f, ec, ours, theirs) = board(0, 1, 1);
            let e = effect_of(&f, ec);
            let run = resolve(&mut f, e, 0, 0, &[theirs[0]]);
            assert!(run.accepted);
            assert_eq!(run.options.len(), 1, "one branch question");
            assert!(run.releases.is_empty(), "and no tribute");
            assert_eq!(
                f.cards[theirs[0]].current.position,
                position::FACEUP_DEFENSE,
                "attack position turned to defence"
            );
            assert_eq!(
                f.cards[theirs[0]].current.controller, 1,
                "and it stays theirs"
            );
            assert_eq!(f.cards[ours[0]].current.location, location::MZONE, "kept");
            // **The script sets the category at run time**, one per
            // branch, and the chain link records the matching operation.
            assert_eq!(
                f.effects.get(e).expect("the effect").category,
                category::POSITION
            );
            let ops = &f.core.current_chain[0].opinfos;
            assert!(ops.contains_key(&category::POSITION));
            assert!(!ops.contains_key(&category::CONTROL));
        }

        /// **Branch two pays a tribute and takes the monster** — and the
        /// control change is temporary, filed for the End Phase.
        #[test]
        fn the_control_branch_tributes_and_takes_the_monster() {
            let (mut f, ec, ours, theirs) = board(0, 1, 1);
            let e = effect_of(&f, ec);
            let run = resolve(&mut f, e, 0, 1, &[theirs[0]]);
            assert!(run.accepted);
            assert_eq!(run.releases.len(), 1, "one tribute question");
            assert_eq!(run.releases[0].0, vec![ours[0]], "our own monster");
            assert_eq!(
                f.cards[ours[0]].current.location,
                location::GRAVE,
                "the tribute is paid"
            );
            assert!(
                f.cards[ours[0]].reason & reason::COST != 0,
                "as a cost, not an effect"
            );
            assert_eq!(
                f.cards[theirs[0]].current.controller, 0,
                "and the monster changed hands"
            );
            let ids = f.cards[theirs[0]]
                .single_effect
                .equal_range(code::SET_CONTROL);
            assert_eq!(ids.len(), 1);
            assert!(
                f.field_effects.pheff.contains(&ids[0]),
                "filed for the End Phase, so it goes home"
            );
            let x = f.effects.get(ids[0]).expect("the control effect");
            assert_ne!(
                x.reset_flag & u32::from(phases::END),
                0,
                "and it is the End Phase it is waiting for"
            );
            assert_eq!(x.reset_count, 1, "once");
            assert_eq!(
                api::get_label_of(&f, e),
                2,
                "the branch is remembered for the resolution"
            );
            let ops = &f.core.current_chain[0].opinfos;
            assert!(ops.contains_key(&category::CONTROL));
            assert_eq!(
                f.effects.get(e).expect("the effect").category,
                category::CONTROL
            );
        }

        /// **Without a tribute to pay, only the position branch is
        /// offered.**
        #[test]
        fn with_nothing_to_tribute_only_one_branch_is_offered() {
            let (mut f, ec, _, _) = board(0, 0, 1);
            let e = effect_of(&f, ec);
            let run = resolve(&mut f, e, 0, 0, &[]);
            assert_eq!(run.options.len(), 1);
            assert_eq!(
                run.options[0].1,
                vec![api::stringid(CODE, 1)],
                "the position branch alone"
            );
        }

        /// Both prompts go out in order: the branch, then the tribute,
        /// then the target.
        #[test]
        fn the_control_branch_asks_for_a_tribute_then_a_target() {
            let (mut f, ec, _, theirs) = board(0, 1, 1);
            let e = effect_of(&f, ec);
            let run = resolve(&mut f, e, 0, 1, &[theirs[0]]);
            let msgs: Vec<u64> = run
                .hints
                .iter()
                .filter(|h| h.0 == hint::SELECTMSG)
                .map(|h| h.2)
                .collect();
            // The seat question that follows names the card, so only the
            // first two are the card's own prompts.
            assert_eq!(
                &msgs[..2],
                &[hintmsg::RELEASE, hintmsg::CONTROL],
                "tribute first, then the monster to take"
            );
        }

        /// The position branch's prompt is the position one.
        #[test]
        fn the_position_branch_asks_with_the_position_prompt() {
            let (mut f, ec, _, theirs) = board(0, 0, 1);
            let e = effect_of(&f, ec);
            let run = resolve(&mut f, e, 0, 0, &[theirs[0]]);
            assert!(run
                .hints
                .contains(&(hint::SELECTMSG, 0, hintmsg::POSCHANGE)));
        }

        /// **The position change goes both ways.** A face-up defence
        /// monster turns to attack, which the `du` slot is for — a port
        /// that filled only `au` would turn attackers over and leave
        /// defenders alone.
        #[test]
        fn a_defending_monster_is_turned_to_attack() {
            let (mut f, ec, _, theirs) = board(0, 0, 1);
            f.cards[theirs[0]].current.position = position::FACEUP_DEFENSE;
            let e = effect_of(&f, ec);
            let run = resolve(&mut f, e, 0, 0, &[theirs[0]]);
            assert!(run.accepted);
            assert_eq!(
                f.cards[theirs[0]].current.position,
                position::FACEUP_ATTACK,
                "defence turned to attack"
            );
        }

        /// A monster that left before the effect resolved is not
        /// touched — the relate check.
        #[test]
        fn a_target_that_lost_its_relation_is_not_touched() {
            let (mut f, ec, _, theirs) = board(0, 0, 1);
            let e = effect_of(&f, ec);
            let mut ch = Chain::new(e, Event::new(code::FREE_CHAIN));
            ch.triggering_player = 0;
            ch.chain_count = 1;
            ch.target_cards = vec![theirs[0]];
            f.core.current_chain.push(ch);
            as_reason(&mut f, e, 0);
            api::set_label(&mut f, e, vec![POSITION_BRANCH]);
            let ev = Event::new(code::FREE_CHAIN);
            activate(&mut f, &ctx_for(e, &ev, 0));
            for _ in 0..512 {
                if f.core.units.is_empty() && f.core.subunits.is_empty() {
                    break;
                }
                f.process();
            }
            assert_eq!(
                f.cards[theirs[0]].current.position,
                position::FACEUP_ATTACK,
                "no relation, so nothing happened"
            );
        }
    }
}

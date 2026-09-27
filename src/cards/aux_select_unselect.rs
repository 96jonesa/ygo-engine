//! `aux.SelectUnselectGroup` and `aux.SelectUnselectLoop`
//! (`utility.lua:2032` and `:2012`) — an **incremental** selection.
//!
//! The library's answer to a choice whose legal options depend on what has
//! already been chosen. Chaos Sorcerer banishes one LIGHT and one DARK: a
//! plain `SelectCard` with `min = max = 2` would offer two LIGHTs, because
//! it settles the whole selection in one question. This offers one card at
//! a time and re-derives the legal set after every answer, so a card that
//! cannot lead to a legal finish is never shown.
//!
//! ## One reference function, two Rust ones
//!
//! The reference splits on `chk`: zero is a pure feasibility test that
//! never yields, one is the interactive loop. Those are different shapes
//! here — [`can_select_unselect`] returns a `bool`, and
//! [`select_unselect_group`] returns a [`Yield`] — so they are two
//! functions that must be kept in step. A board the first accepts and the
//! second then finds nothing on is the bug this split invites.
//!
//! ## The loop's state lives in the continuation
//!
//! In the reference `sg` is a Lua local, held across every yield by the
//! coroutine's own stack. There is no coroutine here, so the accumulating
//! selection rides in the boxed closure instead: [`su_step`] queues one
//! question, suspends with `sg` captured, and the resumption calls
//! [`su_step`] again with the answer folded in. The recursion terminates
//! at the type level because it goes through `Box<dyn FnMut>` — one
//! concrete closure type, monomorphised once — and the processor re-parks
//! every pass in the *same* suspension slot (`execute.rs:157`), so a
//! ten-card selection costs no more state than a one-card one.
//!
//! The consequence for the caller is continuation-passing: the loop cannot
//! *return* its group to the function that started it, so that function
//! hands over a [`Then`] to be called with the finished selection.
//!
//! ## `mg` names two different groups
//!
//! Worth reading twice in the reference. Inside the loop `mg` is the
//! **filtered** candidate list, and that is what `breakcon` receives. But
//! `g:Filter(SelectUnselectLoop, sg, sg, g, ...)` passes the **whole**
//! group as the recursive helper's `mg`, so `rescon` and the feasibility
//! recursion see all of `g`. `finishcon` gets `g` too. Handing the
//! narrowed set to `rescon` would quietly shrink what the search can
//! reach, and nothing about the result would look wrong.

use crate::effect::{Ctx, Yield};
use crate::event::{CardId, EffectId};
use crate::field::Field;
use crate::host_question::hint;
use crate::script_api as api;

/// `rescon(sg, e, tp, mg, c)` — is `sg` a legal selection, and should the
/// search stop descending?
///
/// Two return values, as the reference's is: `(res, stop)`. `stop` prunes
/// the branch — no card in this pool's callers ever sets it, but it is
/// what the recursion checks before descending, not an afterthought.
pub type ResCon =
    fn(&mut Field, sg: &[CardId], e: EffectId, tp: u8, mg: &[CardId], c: CardId) -> (bool, bool);

/// `finishcon(sg, e, tp, g)` — may the player stop here? Gates whether
/// `-1` is accepted as "I have chosen enough".
pub type FinishCon = fn(&mut Field, sg: &[CardId], e: EffectId, tp: u8, g: &[CardId]) -> bool;

/// `breakcon(sg, e, tp, mg)` — should the loop stop asking? Note this one
/// receives the **filtered** candidates.
pub type BreakCon = fn(&mut Field, sg: &[CardId], e: EffectId, tp: u8, mg: &[CardId]) -> bool;

/// What to do with the finished selection — the port's stand-in for the
/// reference's `return sg`.
pub type Then = fn(&mut Field, &Ctx, Vec<CardId>) -> Yield;

/// The arguments both halves share, in the reference's order.
#[derive(Clone, Copy)]
pub struct Args {
    /// `minc or 1`.
    pub minc: usize,
    /// `maxc or #g` — the caller resolves the default, because it needs
    /// the group to do so.
    pub maxc: usize,
    pub rescon: Option<ResCon>,
    /// Who is asked. Not necessarily the effect's controller.
    pub seltp: u8,
    /// `hintmsg or 0`.
    pub hintmsg: u64,
    pub finishcon: Option<FinishCon>,
    pub breakcon: Option<BreakCon>,
    pub cancelable: bool,
}

/// `Auxiliary.SelectUnselectLoop` (`utility.lua:2012`) — can `c` extend
/// `sg` into a selection that eventually satisfies `rescon`?
///
/// The card is added, asked about, and removed again, so `sg` comes back
/// exactly as it went in. With no `rescon` at all the answer is a flat
/// `true`: `local res = not rescon`.
///
/// The two recursion arms are not one arm with a compound test. Below
/// `minc` it descends **whatever** `rescon` just said, because a selection
/// that is too short cannot be accepted yet however legal it looks; at or
/// above `minc` it descends only when `rescon` said no, to look for a
/// longer selection that works.
fn select_unselect_loop(
    f: &mut Field,
    c: CardId,
    sg: &mut Vec<CardId>,
    mg: &[CardId],
    e: EffectId,
    tp: u8,
    a: Args,
) -> bool {
    // `sg:AddCard` is a set insert, so the reference cannot double up. The
    // library's callers pass `sg` as the scan's exception group, which is
    // what keeps that true here.
    debug_assert!(
        !sg.contains(&c),
        "the candidate scan excludes the selection"
    );
    let mut res = a.rescon.is_none();
    if sg.len() >= a.maxc {
        return false;
    }
    sg.push(c);
    if let Some(rescon) = a.rescon {
        let (r, stop) = rescon(f, sg, e, tp, mg, c);
        res = r;
        if stop {
            sg.pop();
            return false;
        }
    }
    if sg.len() < a.minc || (sg.len() < a.maxc && !res) {
        // `mg:IsExists(SelectUnselectLoop, 1, sg, ...)` — the exception is
        // the running selection. Snapshotted before descending, because
        // `sg` is handed on mutably.
        let rest: Vec<CardId> = mg.iter().copied().filter(|x| !sg.contains(x)).collect();
        res = rest
            .into_iter()
            .any(|x| select_unselect_loop(f, x, sg, mg, e, tp, a));
    }
    sg.pop();
    res
}

/// `aux.SelectUnselectGroup(..., chk = 0)` — is there any legal selection
/// at all?
///
/// The exception group shrinks as the scan advances (`eg:RemoveCard(c)`
/// after each candidate), so a pair is considered once rather than once in
/// each order. And the recursion's `mg` is that shrinking `eg`, **not**
/// the full group — the one place where the two halves differ on more than
/// shape.
pub fn can_select_unselect(f: &mut Field, g: &[CardId], e: EffectId, tp: u8, a: Args) -> bool {
    if g.len() < a.minc {
        return false;
    }
    let mut eg: Vec<CardId> = g.to_vec();
    for &c in g {
        let mut sg = Vec::new();
        if select_unselect_loop(f, c, &mut sg, &eg, e, tp, a) {
            return true;
        }
        eg.retain(|&x| x != c);
    }
    false
}

/// `aux.SelectUnselectGroup(..., chk = 1)` — run the interactive loop and
/// hand the finished selection to `then`.
pub fn select_unselect_group(
    f: &mut Field,
    ctx: &Ctx,
    g: Vec<CardId>,
    a: Args,
    then: Then,
) -> Yield {
    su_step(f, ctx, g, Vec::new(), a, then)
}

/// One pass of the reference's `while true do ... end`.
fn su_step(
    f: &mut Field,
    ctx: &Ctx,
    mut g: Vec<CardId>,
    mut sg: Vec<CardId>,
    a: Args,
    then: Then,
) -> Yield {
    let e = ctx.reason_effect;
    let tp = ctx.player;
    let finishable = sg.len() >= a.minc
        && a.finishcon
            .is_none_or(|finishcon| finishcon(f, &sg, e, tp, &g));
    // `g:Filter(SelectUnselectLoop, sg, sg, g, ...)`: the exception is the
    // running selection, and the helper's `mg` is the **whole** group.
    let candidates: Vec<CardId> = g.iter().copied().filter(|c| !sg.contains(c)).collect();
    let mut mg: Vec<CardId> = Vec::new();
    for c in candidates {
        if select_unselect_loop(f, c, &mut sg, &g, e, tp, a) {
            mg.push(c);
        }
    }
    let broke = a
        .breakcon
        .is_some_and(|breakcon| breakcon(f, &sg, e, tp, &mg));
    if broke || mg.is_empty() || sg.len() >= a.maxc {
        return then(f, ctx, sg);
    }
    api::hint(f, hint::SELECTMSG, a.seltp, a.hintmsg);
    // `finishable or (cancelable and #sg==0)`. With `minc == maxc` the
    // loop breaks on `maxc` before `#sg >= minc` can hold, so `finishable`
    // is false throughout and only the *first* question may be backed out
    // of — a `-1` on any later one is a retry, not a cancel.
    let cancelable = finishable || (a.cancelable && sg.is_empty());
    let asked = api::select_unselect(
        f,
        &mg,
        &sg,
        a.seltp,
        finishable,
        cancelable,
        a.minc as u8,
        a.maxc as u8,
    );
    if !asked {
        return then(f, ctx, sg);
    }
    api::suspend(move |f, ctx| {
        // The captures may not be given away: `Rest` is `FnMut`, and a
        // closure that moved out of one could not be called twice even
        // though this one never is.
        let g = std::mem::take(&mut g);
        let mut sg = std::mem::take(&mut sg);
        let Some(tc) = api::selected_one(f) else {
            // `if not tc then break end`
            return then(f, ctx, sg);
        };
        match sg.iter().position(|&x| x == tc) {
            Some(i) => {
                sg.remove(i);
            }
            None => sg.push(tc),
        }
        su_step(f, ctx, g, sg, a, then)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::{location, position};
    use crate::card::{attribute, card_type, status, Card, CardData};
    use crate::event::Event;
    use crate::field::Message;
    use crate::processor::{Kind, Status};
    use std::cell::RefCell;

    thread_local! {
        /// The arguments the probe effect's target runs with. A `Target`
        /// is a plain `fn`, so a test varies the arguments here rather
        /// than by writing one target per case.
        static ARGS: RefCell<Option<Args>> = const { RefCell::new(None) };
        /// What the loop finished with.
        static RESULT: RefCell<Option<Vec<CardId>>> = const { RefCell::new(None) };
        /// The group the probe hands the loop.
        static GROUP: RefCell<Vec<CardId>> = const { RefCell::new(Vec::new()) };
        /// Every `(sg, mg)` pair `rescon` was asked about.
        static RESCON_SAW: RefCell<Vec<(Vec<CardId>, Vec<CardId>)>> =
            const { RefCell::new(Vec::new()) };
    }

    /// A monster of a given attribute in `player`'s Monster Zone.
    fn monster(f: &mut Field, player: u8, seq: u32, attr: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 90_000 + seq,
                type_: card_type::MONSTER | card_type::NORMAL,
                level: 4,
                attack: 1000,
                defense: 1000,
                attribute: attr,
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

    /// One LIGHT and one DARK — Chaos Sorcerer's rule, without the seat
    /// count, so these tests are about the loop and not about the board.
    fn light_and_dark(
        f: &mut Field,
        sg: &[CardId],
        _e: EffectId,
        _tp: u8,
        mg: &[CardId],
        _c: CardId,
    ) -> (bool, bool) {
        RESCON_SAW.with(|r| r.borrow_mut().push((sg.to_vec(), mg.to_vec())));
        let light = sg
            .iter()
            .any(|&x| api::is_attribute(f, x, attribute::LIGHT));
        let dark = sg.iter().any(|&x| api::is_attribute(f, x, attribute::DARK));
        (light && dark, false)
    }

    fn base_args() -> Args {
        Args {
            minc: 2,
            maxc: 2,
            rescon: Some(light_and_dark),
            seltp: 0,
            hintmsg: crate::host_question::hintmsg::REMOVE,
            finishcon: None,
            breakcon: None,
            cancelable: true,
        }
    }

    fn probe_target(f: &mut Field, ctx: &Ctx, _chk: bool, _chkc: Option<CardId>) -> Yield {
        let a = ARGS.with(|x| x.borrow().expect("args set"));
        let g = GROUP.with(|x| x.borrow().clone());
        select_unselect_group(f, ctx, g, a, probe_done)
    }

    fn probe_done(_f: &mut Field, _ctx: &Ctx, sg: Vec<CardId>) -> Yield {
        RESULT.with(|x| *x.borrow_mut() = Some(sg.clone()));
        api::yes(!sg.is_empty())
    }

    /// An effect whose target is the probe, on a fresh card.
    fn probe_effect(f: &mut Field, tp: u8) -> EffectId {
        let c = Card::with_data(
            CardData {
                code: 99_999,
                type_: card_type::SPELL,
                ..Default::default()
            },
            tp,
        );
        let holder = f.new_card(c);
        f.add_card(tp, holder, location::SZONE, 0, false);
        let e = api::create_effect(f, holder);
        api::set_type(f, e, crate::effect::effect_type::FIELD);
        api::set_code(f, e, crate::event::code::SPSUMMON_PROC);
        api::set_target(f, e, probe_target);
        api::register_effect(f, holder, e, false);
        e
    }

    /// What one driven run of the loop said and was asked.
    /// One question: who was asked, what was offered, what was already
    /// taken, and the two permissions (`finishable`, `cancelable`).
    type Ask = (u8, Vec<CardId>, Vec<CardId>, bool, bool);

    #[derive(Default)]
    struct Run {
        asks: Vec<Ask>,
        /// Each `(kind, player, value)` hint.
        hints: Vec<(u8, u8, u64)>,
        /// Every `Retry` the host was sent.
        retries: usize,
        /// What `probe_done` received.
        finished: Option<Vec<CardId>>,
    }

    /// Drive the probe's target, answering with `answers` in order.
    ///
    /// An answer is `Some(index into select ++ unselect)` or `None` for
    /// `-1`. Running out of answers sends `-1`, which is how a test that
    /// only cares about the questions ends.
    fn drive(f: &mut Field, e: EffectId, tp: u8, answers: &[Option<usize>]) -> Run {
        RESULT.with(|x| *x.borrow_mut() = None);
        f.core.sub_solving_event.push_back(Event::new(0));
        f.emplace(Kind::ExecuteTarget {
            resume: None,
            effect: e,
            player: tp,
            subject: None,
            args: Vec::new(),
            was_disabled: false,
        });
        let mut run = Run::default();
        let mut seen = 0usize;
        let mut next = 0usize;
        for _ in 0..8192 {
            while seen < f.messages.len() {
                match &f.messages[seen] {
                    Message::Hint {
                        kind,
                        player,
                        value,
                    } => run.hints.push((*kind, *player, *value)),
                    Message::Retry => run.retries += 1,
                    _ => {}
                }
                seen += 1;
            }
            match f.process() {
                Status::Continue => {
                    if f.core.units.is_empty() && f.core.subunits.is_empty() {
                        break;
                    }
                }
                Status::Awaiting => match f.messages.last() {
                    Some(Message::SelectUnselectCard {
                        player,
                        finishable,
                        cancelable,
                        select,
                        unselect,
                        ..
                    }) => {
                        run.asks.push((
                            *player,
                            select.clone(),
                            unselect.clone(),
                            *finishable,
                            *cancelable,
                        ));
                        let answer = answers.get(next).copied().unwrap_or(None);
                        next += 1;
                        f.core.return_cards.clear();
                        match answer {
                            Some(i) => {
                                f.core.returns.set_i32(0, 1);
                                f.core.returns.set_i32(1, i as i32);
                            }
                            None => f.core.returns.set_i32(0, -1),
                        }
                    }
                    // A refused answer is re-asked, and the host has to
                    // answer again. Not recorded as a question: it is the
                    // same one.
                    Some(Message::Retry) => {
                        let answer = answers.get(next).copied().unwrap_or(None);
                        next += 1;
                        f.core.return_cards.clear();
                        match answer {
                            Some(i) => {
                                f.core.returns.set_i32(0, 1);
                                f.core.returns.set_i32(1, i as i32);
                            }
                            None => f.core.returns.set_i32(0, -1),
                        }
                    }
                    other => panic!("unexpected question: {other:?}"),
                },
                Status::End => break,
            }
        }
        run.finished = RESULT.with(|x| x.borrow().clone());
        run
    }

    /// A board of four monsters — two LIGHT, two DARK — and the probe
    /// effect over all of them.
    fn board(a: Args) -> (Field, EffectId, Vec<CardId>) {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        let cards = vec![
            monster(&mut f, 0, 0, attribute::LIGHT),
            monster(&mut f, 0, 1, attribute::DARK),
            monster(&mut f, 0, 2, attribute::LIGHT),
            monster(&mut f, 0, 3, attribute::DARK),
        ];
        let e = probe_effect(&mut f, 0);
        ARGS.with(|x| *x.borrow_mut() = Some(a));
        GROUP.with(|x| *x.borrow_mut() = cards.clone());
        RESCON_SAW.with(|x| x.borrow_mut().clear());
        (f, e, cards)
    }

    // ---------- extra conditions the cases below need ----------

    fn always(
        _f: &mut Field,
        _sg: &[CardId],
        _e: EffectId,
        _tp: u8,
        _mg: &[CardId],
        _c: CardId,
    ) -> (bool, bool) {
        (true, false)
    }

    /// True only while the selection is a single LIGHT — so a *longer*
    /// selection is worse than a shorter one, which is what separates the
    /// two recursion arms.
    fn exactly_one_light(
        f: &mut Field,
        sg: &[CardId],
        _e: EffectId,
        _tp: u8,
        _mg: &[CardId],
        _c: CardId,
    ) -> (bool, bool) {
        (
            sg.len() == 1 && api::is_attribute(f, sg[0], attribute::LIGHT),
            false,
        )
    }

    /// Legal, but refuses to be extended past two.
    fn legal_but_stop_at_two(
        _f: &mut Field,
        sg: &[CardId],
        _e: EffectId,
        _tp: u8,
        _mg: &[CardId],
        _c: CardId,
    ) -> (bool, bool) {
        (true, sg.len() >= 2)
    }

    fn never_finished(
        _f: &mut Field,
        _sg: &[CardId],
        _e: EffectId,
        _tp: u8,
        _g: &[CardId],
    ) -> bool {
        false
    }

    /// Reads the group it was handed, so a caller that passes the running
    /// selection instead of the whole group is visible.
    fn finish_when_given_four(
        _f: &mut Field,
        _sg: &[CardId],
        _e: EffectId,
        _tp: u8,
        g: &[CardId],
    ) -> bool {
        g.len() == 4
    }

    fn break_at_once(
        _f: &mut Field,
        _sg: &[CardId],
        _e: EffectId,
        _tp: u8,
        _mg: &[CardId],
    ) -> bool {
        true
    }

    /// True when it was handed **four** candidates — the filtered list is
    /// three on the board these tests use, the whole group is four.
    fn break_when_given_four(
        _f: &mut Field,
        _sg: &[CardId],
        _e: EffectId,
        _tp: u8,
        mg: &[CardId],
    ) -> bool {
        mg.len() == 4
    }

    /// A board where one card can never be part of a legal pair, so the
    /// filtered candidate list and the whole group differ.
    fn board_with_a_dead_card(a: Args) -> (Field, EffectId, Vec<CardId>) {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        let cards = vec![
            monster(&mut f, 0, 0, attribute::LIGHT),
            monster(&mut f, 0, 1, attribute::DARK),
            monster(&mut f, 0, 2, attribute::LIGHT),
            monster(&mut f, 0, 3, attribute::EARTH),
        ];
        let e = probe_effect(&mut f, 0);
        ARGS.with(|x| *x.borrow_mut() = Some(a));
        GROUP.with(|x| *x.borrow_mut() = cards.clone());
        RESCON_SAW.with(|x| x.borrow_mut().clear());
        (f, e, cards)
    }

    /// Just a field and a group, for the `chk == 0` half — which never
    /// asks anything and so needs no driving.
    fn attrs(list: &[u32]) -> (Field, EffectId, Vec<CardId>) {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        let cards = list
            .iter()
            .enumerate()
            .map(|(i, &a)| monster(&mut f, 0, i as u32, a))
            .collect::<Vec<_>>();
        let e = probe_effect(&mut f, 0);
        RESCON_SAW.with(|x| x.borrow_mut().clear());
        (f, e, cards)
    }

    mod can_select_unselect {
        use super::*;

        #[test]
        fn a_legal_pair_is_found() {
            let (mut f, e, g) = attrs(&[attribute::LIGHT, attribute::DARK]);
            assert!(super::super::can_select_unselect(
                &mut f,
                &g,
                e,
                0,
                base_args()
            ));
        }

        #[test]
        fn no_legal_pair_is_refused() {
            let (mut f, e, g) = attrs(&[attribute::LIGHT, attribute::LIGHT]);
            assert!(!super::super::can_select_unselect(
                &mut f,
                &g,
                e,
                0,
                base_args()
            ));
        }

        #[test]
        fn a_group_smaller_than_minc_is_refused() {
            let (mut f, e, g) = attrs(&[attribute::LIGHT]);
            assert!(!super::super::can_select_unselect(
                &mut f,
                &g,
                e,
                0,
                base_args()
            ));
        }

        /// **With no `rescon` the answer is a flat yes** — `local res =
        /// not rescon`, not `false`.
        #[test]
        fn no_rescon_accepts_any_selection() {
            let (mut f, e, g) = attrs(&[attribute::LIGHT, attribute::LIGHT]);
            let a = Args {
                rescon: None,
                ..base_args()
            };
            assert!(super::super::can_select_unselect(&mut f, &g, e, 0, a));
        }

        /// The `maxc` guard only shows itself when `minc` is the larger of
        /// the two: the below-`minc` arm keeps descending, and this is
        /// what stops it overrunning.
        #[test]
        fn the_search_never_grows_past_maxc() {
            let (mut f, e, g) = attrs(&[attribute::LIGHT; 4]);
            let a = Args {
                minc: 3,
                maxc: 2,
                rescon: Some(always),
                ..base_args()
            };
            assert!(!super::super::can_select_unselect(&mut f, &g, e, 0, a));
        }

        /// `rescon`'s second return value prunes the branch, and a pruned
        /// branch is not legal however legal `res` said it was.
        #[test]
        fn a_stopped_branch_is_refused_even_when_legal() {
            let (mut f, e, g) = attrs(&[attribute::LIGHT, attribute::DARK, attribute::LIGHT]);
            let a = Args {
                minc: 2,
                maxc: 3,
                rescon: Some(legal_but_stop_at_two),
                ..base_args()
            };
            assert!(!super::super::can_select_unselect(&mut f, &g, e, 0, a));
        }

        /// Below `minc` the search descends **whatever** `rescon` said:
        /// a selection that is too short cannot be accepted, however legal
        /// it looks on its own.
        #[test]
        fn a_short_legal_selection_does_not_count() {
            let (mut f, e, g) = attrs(&[attribute::LIGHT, attribute::DARK]);
            let a = Args {
                minc: 2,
                maxc: 2,
                rescon: Some(exactly_one_light),
                ..base_args()
            };
            assert!(!super::super::can_select_unselect(&mut f, &g, e, 0, a));
        }

        /// And at or above `minc` it descends only when `rescon` said no.
        /// A legal selection is not thrown away to look for a longer one.
        #[test]
        fn a_legal_selection_is_not_extended_looking_for_a_better_one() {
            let (mut f, e, g) = attrs(&[attribute::LIGHT, attribute::DARK]);
            let a = Args {
                minc: 1,
                maxc: 2,
                rescon: Some(exactly_one_light),
                ..base_args()
            };
            assert!(super::super::can_select_unselect(&mut f, &g, e, 0, a));
        }

        /// The other side of the same line: at `minc` with `rescon`
        /// refusing, it *does* descend.
        #[test]
        fn a_refused_selection_is_extended() {
            let (mut f, e, g) = attrs(&[attribute::LIGHT, attribute::DARK]);
            let a = Args {
                minc: 1,
                maxc: 2,
                ..base_args()
            };
            assert!(super::super::can_select_unselect(&mut f, &g, e, 0, a));
        }

        /// One completion is enough — `IsExists(.., 1, ..)`, not "every
        /// candidate works".
        #[test]
        fn one_working_completion_is_enough() {
            let (mut f, e, g) = attrs(&[attribute::LIGHT, attribute::DARK, attribute::LIGHT]);
            assert!(super::super::can_select_unselect(
                &mut f,
                &g,
                e,
                0,
                base_args()
            ));
        }

        /// **The exception group shrinks as the scan advances**, and the
        /// recursion is given that shrinking group rather than the whole
        /// one. Visible only through what `rescon` was handed.
        #[test]
        fn the_scan_narrows_what_the_recursion_may_use() {
            let (mut f, e, g) = attrs(&[attribute::LIGHT; 3]);
            assert!(!super::super::can_select_unselect(
                &mut f,
                &g,
                e,
                0,
                base_args()
            ));
            let sizes: Vec<usize> = RESCON_SAW.with(|r| {
                let mut seen: Vec<usize> = r.borrow().iter().map(|(_, mg)| mg.len()).collect();
                seen.dedup();
                seen
            });
            assert_eq!(
                sizes,
                vec![3, 2, 1],
                "each candidate is offered only the cards from it onward"
            );
        }
    }

    mod select_unselect_group {
        use super::*;
        use crate::host_question::{hint, hintmsg};

        /// The narrowing is the whole point: once a LIGHT is taken, the
        /// other LIGHT is no longer a legal answer and is not offered.
        #[test]
        fn the_offer_narrows_after_every_answer() {
            let (mut f, e, g) = board(base_args());
            let run = drive(&mut f, e, 0, &[Some(0), Some(0)]);
            assert_eq!(run.asks.len(), 2, "one question per pick");
            assert_eq!(run.asks[0].1, g, "everything is legal to start with");
            assert_eq!(
                run.asks[1].1,
                vec![g[1], g[3]],
                "with a LIGHT taken, only the DARKs remain"
            );
            assert_eq!(run.asks[1].2, vec![g[0]], "and the LIGHT may be given back");
            assert_eq!(run.finished, Some(vec![g[0], g[1]]));
        }

        /// A card already chosen comes back off the selection rather than
        /// being added twice.
        #[test]
        fn choosing_a_chosen_card_unselects_it() {
            let (mut f, e, g) = board(base_args());
            // take g[0], give it back, take g[2] instead, then a DARK
            let run = drive(&mut f, e, 0, &[Some(0), Some(2), Some(1), Some(1)]);
            assert_eq!(run.asks[1].2, vec![g[0]]);
            assert_eq!(
                run.asks[2].1, g,
                "everything is legal again once the LIGHT is released"
            );
            assert_eq!(run.finished, Some(vec![g[1], g[2]]));
        }

        /// An unselect takes **one** card off, not the lot.
        #[test]
        fn an_unselect_removes_only_that_card() {
            let a = Args {
                minc: 1,
                maxc: 3,
                rescon: None,
                ..base_args()
            };
            let (mut f, e, g) = board(a);
            // take g[0] and g[1], then give g[0] back, then finish
            let run = drive(&mut f, e, 0, &[Some(0), Some(0), Some(2), None]);
            assert_eq!(run.finished, Some(vec![g[1]]), "g[1] survives");
        }

        /// **The `-1` on a second question is a retry, not a cancel.**
        /// With `minc == maxc` the loop breaks on `maxc` before
        /// `#sg >= minc` can hold, so `finishable` is false throughout and
        /// only the first question carries `cancelable`.
        #[test]
        fn only_the_first_question_may_be_backed_out_of() {
            let (mut f, e, g) = board(base_args());
            let run = drive(&mut f, e, 0, &[Some(0), None, Some(0)]);
            assert!(!run.asks[0].3, "never finishable at minc == maxc");
            assert!(run.asks[0].4, "but the first pick may be abandoned");
            assert!(!run.asks[1].3);
            assert!(!run.asks[1].4, "the second may not");
            assert_eq!(run.retries, 1, "so the -1 was refused");
            assert_eq!(run.finished, Some(vec![g[0], g[1]]));
        }

        /// Cancelling the first question finishes with nothing.
        #[test]
        fn cancelling_the_first_question_chooses_nothing() {
            let (mut f, e, _g) = board(base_args());
            let run = drive(&mut f, e, 0, &[None]);
            assert_eq!(run.asks.len(), 1);
            assert_eq!(run.finished, Some(Vec::new()));
        }

        /// Finishing early keeps what was chosen rather than discarding
        /// it — `break` leaves `sg` alone.
        #[test]
        fn finishing_early_keeps_the_selection() {
            let a = Args {
                minc: 1,
                maxc: 3,
                rescon: None,
                ..base_args()
            };
            let (mut f, e, g) = board(a);
            let run = drive(&mut f, e, 0, &[Some(0), None]);
            assert!(run.asks[1].3, "one card is enough, so it is finishable");
            // The host is told it may cancel as well. The unit would take
            // the `-1` on `finishable` alone, so this only shows up in the
            // message — which is what a client draws its buttons from.
            assert!(run.asks[1].4, "and the host is told so");
            assert_eq!(run.finished, Some(vec![g[0]]));
        }

        /// `finishable` needs `minc` cards **and** `finishcon`.
        #[test]
        fn finishcon_can_refuse_a_long_enough_selection() {
            let a = Args {
                minc: 1,
                maxc: 3,
                rescon: None,
                finishcon: Some(never_finished),
                ..base_args()
            };
            let (mut f, e, _g) = board(a);
            let run = drive(&mut f, e, 0, &[Some(0), Some(0), Some(0)]);
            assert!(
                run.asks.iter().all(|q| !q.3),
                "finishcon refuses every stopping point"
            );
        }

        /// And `finishcon` is handed the **whole** group, not the running
        /// selection.
        #[test]
        fn finishcon_reads_the_whole_group() {
            let a = Args {
                minc: 1,
                maxc: 3,
                rescon: None,
                finishcon: Some(finish_when_given_four),
                ..base_args()
            };
            let (mut f, e, _g) = board(a);
            let run = drive(&mut f, e, 0, &[Some(0), None]);
            assert!(!run.asks[0].3, "nothing chosen yet, so not finishable");
            assert!(run.asks[1].3, "four cards were handed over, so finishable");
        }

        /// `breakcon` ends the loop before anything is asked.
        #[test]
        fn breakcon_stops_the_loop() {
            let a = Args {
                breakcon: Some(break_at_once),
                ..base_args()
            };
            let (mut f, e, _g) = board(a);
            let run = drive(&mut f, e, 0, &[Some(0)]);
            assert!(run.asks.is_empty(), "no question was ever put");
            assert_eq!(run.finished, Some(Vec::new()));
        }

        /// **`breakcon` is handed the narrowed candidates**, where
        /// `rescon` and `finishcon` get the whole group. Three names, two
        /// different values.
        #[test]
        fn breakcon_reads_the_narrowed_candidates() {
            let a = Args {
                breakcon: Some(break_when_given_four),
                ..base_args()
            };
            let (mut f, e, _g) = board_with_a_dead_card(a);
            let run = drive(&mut f, e, 0, &[Some(0), Some(0)]);
            assert!(
                !run.asks.is_empty(),
                "the filtered list is three, so breakcon does not fire"
            );
        }

        /// `rescon`, by contrast, always sees every card in the group.
        #[test]
        fn rescon_reads_the_whole_group() {
            let (mut f, e, g) = board_with_a_dead_card(base_args());
            drive(&mut f, e, 0, &[Some(0), Some(0)]);
            RESCON_SAW.with(|r| {
                assert!(!r.borrow().is_empty(), "rescon ran");
                for (sg, mg) in r.borrow().iter() {
                    assert_eq!(mg, &g, "asked about {sg:?} with a narrowed group");
                }
            });
        }

        /// Nothing legal left means the loop ends without asking — and
        /// without sending a hint for a question it is not going to put.
        #[test]
        fn an_impossible_board_asks_nothing() {
            let a = base_args();
            let mut f = Field::new(8000);
            f.infos.turn_id = 3;
            let cards = vec![
                monster(&mut f, 0, 0, attribute::LIGHT),
                monster(&mut f, 0, 1, attribute::LIGHT),
            ];
            let e = probe_effect(&mut f, 0);
            ARGS.with(|x| *x.borrow_mut() = Some(a));
            GROUP.with(|x| *x.borrow_mut() = cards);
            let run = drive(&mut f, e, 0, &[]);
            assert!(run.asks.is_empty());
            assert!(
                !run.hints.iter().any(|h| h.0 == hint::SELECTMSG),
                "no hint for a question that was never asked"
            );
            assert_eq!(run.finished, Some(Vec::new()));
        }

        /// The hint goes out before each question, to the player being
        /// asked — who is `seltp`, not the effect's controller.
        #[test]
        fn the_hint_and_the_question_go_to_seltp() {
            let a = Args {
                seltp: 1,
                ..base_args()
            };
            let (mut f, e, _g) = board(a);
            let run = drive(&mut f, e, 0, &[Some(0), Some(0)]);
            assert_eq!(
                run.hints
                    .iter()
                    .filter(|h| h.0 == hint::SELECTMSG)
                    .copied()
                    .collect::<Vec<_>>(),
                vec![
                    (hint::SELECTMSG, 1, hintmsg::REMOVE),
                    (hint::SELECTMSG, 1, hintmsg::REMOVE)
                ],
                "one hint per question, both to the opponent"
            );
            assert!(run.asks.iter().all(|q| q.0 == 1), "and so is the question");
        }
    }
}

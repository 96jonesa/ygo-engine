//! `Duel.CheckReleaseGroupCost` and `Duel.SelectReleaseGroupCost`
//! (`proc_workaround.lua:620` and `:634`) — releasing cards **as a cost**.
//!
//! These are library functions, not core exports: the core offers
//! `CheckReleaseGroup`/`SelectReleaseGroup`, and the library wraps them
//! with the bookkeeping that "as a cost" needs. The wrapper is a second
//! incremental selection, built on the same `Group.SelectUnselect` that
//! [`super::aux_select_unselect`] loops around.
//!
//! ## What this pool collapses, and what licenses the collapse
//!
//! The reference splits the release pool in two with
//! `Auxiliary.ReleaseCostFilter`, keeping back the opponent's cards that
//! `EFFECT_EXTRA_RELEASE` or `EFFECT_EXTRA_RELEASE_NONSUM` make available
//! — `exg` — and separately requiring the `EXTRA_RELEASE` ones to be
//! included at all — `mustg`. It then tracks how many of `exg` a
//! selection holds, because **at most one** may be used, and spends the
//! enabling effect's count limit afterwards.
//!
//! **No card in this pool has either effect**, checked by
//! `tools/check_constants.py`'s `ABSENT_FROM_POOL` scan rather than
//! asserted here. So for this pool `exg` and `mustg` are always empty,
//! `ReleaseCheckSingleUse` always answers `(true, false)`, and
//! `Field::get_release_list` never puts an opponent's monster in the pool
//! at all.
//!
//! Both groups are still **parameters** here, and
//! [`release_check_single_use`] is still real code, for the same reason
//! `finishcon` survived in the other loop: leaving them out would not
//! shrink these functions, it would write different ones. What is
//! deliberately **not** ported is the tail of `SelectReleaseGroupCost`
//! that spends `EFFECT_EXTRA_RELEASE_NONSUM`'s count limit and announces
//! it with `HINT_CARD` — that runs only when the chosen set meets `exg`,
//! which cannot happen while the scan cannot produce one.
//!
//! ## `ct` is carried, not measured
//!
//! `RelCheckRecursive` takes a count and increments it rather than
//! reading `#sg`. With `mustg` empty the two agree, but they are not the
//! same number in general — `sg` starts out holding `mustg`, which does
//! not count toward the minimum the player is choosing.

use crate::effect::{Ctx, Yield};
use crate::event::CardId;
use crate::field::Field;
use crate::host_question::{hint, hintmsg};
use crate::script_api as api;

/// The `check` a caller may supply — `(res, stop)`, as the reference's
/// is. `stop` prunes the branch rather than merely failing it.
pub type SpecialCheck = fn(&mut Field, sg: &[CardId], tp: u8, exg: &[CardId]) -> (bool, bool);

/// What to do with the finished selection.
pub type Then = fn(&mut Field, &Ctx, Vec<CardId>) -> Yield;

/// The arguments both halves share.
#[derive(Clone, Copy)]
pub struct Args<'a> {
    pub minc: usize,
    pub maxc: usize,
    /// The cards `EFFECT_EXTRA_RELEASE*` made available. Empty in this
    /// pool; see the module note.
    pub exg: &'a [CardId],
    /// The cards that **must** be included if anything is. Empty in this
    /// pool.
    pub mustg: &'a [CardId],
    pub check: Option<SpecialCheck>,
}

/// `Auxiliary.ReleaseCheckSingleUse` (`proc_workaround.lua:578`) — at
/// most **one** of the specially-enabled cards may be taken.
///
/// `ct` is `#sg - #(sg - exg)`, which is the size of the intersection.
/// The second return value is not the negation of the first: it says
/// *stop descending*, because adding more cards can never bring an
/// over-full intersection back under one.
pub fn release_check_single_use(sg: &[CardId], exg: &[CardId]) -> (bool, bool) {
    let ct = sg.iter().filter(|c| exg.contains(c)).count();
    (ct <= 1, ct > 1)
}

/// `Auxiliary.MakeSpecialCheck` — the caller's check **and** the
/// single-use rule, or just the single-use rule when there is no caller's
/// check.
fn special_check(f: &mut Field, sg: &[CardId], tp: u8, a: Args) -> (bool, bool) {
    let (res2, stop2) = release_check_single_use(sg, a.exg);
    let Some(check) = a.check else {
        return (res2, stop2);
    };
    let (res, stop) = check(f, sg, tp, a.exg);
    (res && res2, stop || stop2)
}

/// `Auxiliary.RelCheckGoal` (`:598`) — is `sg` a finished selection?
///
/// The size test comes first and its `stop` is `ct > maxc`: too few is
/// worth descending from, too many is not.
pub fn rel_check_goal(f: &mut Field, sg: &[CardId], tp: u8, ct: usize, a: Args) -> (bool, bool) {
    if ct < a.minc || ct > a.maxc {
        return (false, ct > a.maxc);
    }
    let (res, stop) = special_check(f, sg, tp, a);
    (res && a.mustg.iter().all(|m| sg.contains(m)), stop)
}

/// `Auxiliary.RelCheckRecursive` (`:588`) — can `c` extend `sg` into a
/// finished selection?
pub fn rel_check_recursive(
    f: &mut Field,
    c: CardId,
    sg: &mut Vec<CardId>,
    mg: &[CardId],
    tp: u8,
    ct: usize,
    a: Args,
) -> bool {
    sg.push(c);
    let ct = ct + 1;
    let (mut res, stop) = rel_check_goal(f, sg, tp, ct, a);
    if !res && !stop {
        // `mg:IsExists(RelCheckRecursive, 1, sg, ...)` — the exception is
        // the running selection.
        let rest: Vec<CardId> = mg.iter().copied().filter(|x| !sg.contains(x)).collect();
        res = ct < a.maxc
            && rest
                .into_iter()
                .any(|x| rel_check_recursive(f, x, sg, mg, tp, ct, a));
    }
    sg.pop();
    res
}

/// `Duel.CheckReleaseGroupCost` — is there any legal selection at all?
///
/// `mg` is the already-matched release group; the caller builds it with
/// `Duel.GetReleaseGroup` and its own filter, as the reference does
/// before splitting.
pub fn check_release_group_cost(f: &mut Field, mg: &[CardId], tp: u8, a: Args) -> bool {
    // `mg:Includes(mustg)` — the must-include cards have to be available.
    if !a.mustg.iter().all(|m| mg.contains(m)) {
        return false;
    }
    let mut sg = Vec::new();
    let candidates = mg.to_vec();
    candidates
        .into_iter()
        .any(|c| rel_check_recursive(f, c, &mut sg, mg, tp, 0, a))
}

/// The owned state the loop carries across a suspension — the borrowed
/// [`Args`] cannot survive one.
#[derive(Clone)]
pub struct Cost {
    pub minc: usize,
    pub maxc: usize,
    /// Empty in this pool; see the module note.
    pub exg: Vec<CardId>,
    /// Empty in this pool.
    pub mustg: Vec<CardId>,
    pub check: Option<SpecialCheck>,
}

impl Cost {
    /// The plain case: `minc`/`maxc` and nothing special.
    pub fn of(minc: usize, maxc: usize) -> Self {
        Cost {
            minc,
            maxc,
            exg: Vec::new(),
            mustg: Vec::new(),
            check: None,
        }
    }

    /// The borrowed view the check functions take.
    pub fn borrowed(&self) -> Args<'_> {
        self.args()
    }

    fn args(&self) -> Args<'_> {
        Args {
            minc: self.minc,
            maxc: self.maxc,
            exg: &self.exg,
            mustg: &self.mustg,
            check: self.check,
        }
    }
}

/// `Duel.SelectReleaseGroupCost` — run the loop and hand the chosen set
/// to `then`.
///
/// The selection **starts holding `mustg`**, and a `mustg` card chosen
/// again is ignored rather than removed — it may not be given back.
pub fn select_release_group_cost(
    f: &mut Field,
    ctx: &Ctx,
    mg: Vec<CardId>,
    tp: u8,
    cost: Cost,
    then: Then,
) -> Yield {
    let sg = cost.mustg.clone();
    rel_step(f, ctx, mg, sg, tp, cost, then)
}

fn rel_step(
    f: &mut Field,
    ctx: &Ctx,
    mut mg: Vec<CardId>,
    mut sg: Vec<CardId>,
    tp: u8,
    cost: Cost,
    then: Then,
) -> Yield {
    let a = cost.args();
    if sg.len() >= a.maxc {
        return then(f, ctx, sg);
    }
    let ct = sg.len();
    let candidates: Vec<CardId> = mg.iter().copied().filter(|c| !sg.contains(c)).collect();
    let mut cg: Vec<CardId> = Vec::new();
    for c in candidates {
        if rel_check_recursive(f, c, &mut sg, &mg, tp, ct, cost.args()) {
            cg.push(c);
        }
    }
    if cg.is_empty() {
        return then(f, ctx, sg);
    }
    // `cancel` is both permissions here — the reference passes the same
    // value for `finishable` and `cancelable`, so "I have chosen enough"
    // and "I am backing out" are the same answer.
    let (cancel, _) = rel_check_goal(f, &sg, tp, ct, cost.args());
    api::hint(f, hint::SELECTMSG, tp, hintmsg::RELEASE);
    if !api::select_unselect(f, &cg, &sg, tp, cancel, cancel, 1, 1) {
        return then(f, ctx, sg);
    }
    api::suspend(move |f, ctx| {
        let mg = std::mem::take(&mut mg);
        let mut sg = std::mem::take(&mut sg);
        let cost = cost.clone();
        let Some(tc) = api::selected_one(f) else {
            return then(f, ctx, sg);
        };
        // A must-include card is not removable: the reference skips the
        // toggle entirely for one.
        if !cost.mustg.contains(&tc) {
            match sg.iter().position(|&x| x == tc) {
                Some(i) => {
                    sg.remove(i);
                }
                None => sg.push(tc),
            }
        }
        rel_step(f, ctx, mg, sg, tp, cost, then)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::{location, position};
    use crate::card::{card_type, status, Card, CardData};
    use crate::event::Event;
    use crate::field::Message;
    use crate::processor::{Kind, Status};
    use std::cell::RefCell;

    thread_local! {
        static COST: RefCell<Option<Cost>> = const { RefCell::new(None) };
        static POOL: RefCell<Vec<CardId>> = const { RefCell::new(Vec::new()) };
        static RESULT: RefCell<Option<Vec<CardId>>> = const { RefCell::new(None) };
    }

    fn monster(f: &mut Field, player: u8, seq: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 95_000 + seq,
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

    /// Never satisfied, and prunes once the selection holds two.
    fn stop_at_two(_f: &mut Field, sg: &[CardId], _tp: u8, _exg: &[CardId]) -> (bool, bool) {
        (true, sg.len() >= 2)
    }

    fn never(_f: &mut Field, _sg: &[CardId], _tp: u8, _exg: &[CardId]) -> (bool, bool) {
        (false, false)
    }

    /// Satisfied only at three, but refuses to be grown past two — so the
    /// three-card selection is reachable **only** by ignoring the stop.
    fn wants_three_but_stops_at_two(
        _f: &mut Field,
        sg: &[CardId],
        _tp: u8,
        _exg: &[CardId],
    ) -> (bool, bool) {
        (sg.len() == 3, sg.len() >= 2)
    }

    fn probe_target(f: &mut Field, ctx: &Ctx, _chk: bool, _chkc: Option<CardId>) -> Yield {
        let cost = COST.with(|x| x.borrow().clone().expect("cost set"));
        let mg = POOL.with(|x| x.borrow().clone());
        select_release_group_cost(f, ctx, mg, ctx.player, cost, probe_done)
    }

    fn probe_done(_f: &mut Field, _ctx: &Ctx, sg: Vec<CardId>) -> Yield {
        RESULT.with(|x| *x.borrow_mut() = Some(sg.clone()));
        api::yes(!sg.is_empty())
    }

    fn probe_effect(f: &mut Field, tp: u8) -> crate::event::EffectId {
        let c = Card::with_data(
            CardData {
                code: 99_998,
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

    /// One question: who, offered, taken, finishable, cancelable.
    type Ask = (u8, Vec<CardId>, Vec<CardId>, bool, bool);

    #[derive(Default)]
    struct Run {
        asks: Vec<Ask>,
        hints: Vec<(u8, u8, u64)>,
        finished: Option<Vec<CardId>>,
    }

    fn drive(f: &mut Field, e: crate::event::EffectId, tp: u8, answers: &[Option<usize>]) -> Run {
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
        let (mut seen, mut next) = (0usize, 0usize);
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
                    other => panic!("unexpected question: {other:?}"),
                },
                Status::End => break,
            }
        }
        run.finished = RESULT.with(|x| x.borrow().clone());
        run
    }

    fn board(n: u32, cost: Cost) -> (Field, crate::event::EffectId, Vec<CardId>) {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        let cards: Vec<CardId> = (0..n).map(|i| monster(&mut f, 0, i)).collect();
        let e = probe_effect(&mut f, 0);
        COST.with(|x| *x.borrow_mut() = Some(cost));
        POOL.with(|x| *x.borrow_mut() = cards.clone());
        (f, e, cards)
    }

    mod single_use {
        use super::*;

        /// **At most one of the specially-enabled cards**, and the second
        /// return value is a *stop*, not a negation: an over-full
        /// intersection can never be repaired by choosing more.
        #[test]
        fn one_is_allowed_and_two_stops_the_search() {
            assert_eq!(release_check_single_use(&[], &[]), (true, false));
            assert_eq!(release_check_single_use(&[1, 2], &[]), (true, false));
            assert_eq!(release_check_single_use(&[1, 2], &[2]), (true, false));
            assert_eq!(
                release_check_single_use(&[1, 2], &[1, 2]),
                (false, true),
                "two of them: refused, and stop"
            );
        }
    }

    mod rel_check {
        use super::*;

        fn args<'a>(minc: usize, maxc: usize, exg: &'a [CardId], mustg: &'a [CardId]) -> Args<'a> {
            Args {
                minc,
                maxc,
                exg,
                mustg,
                check: None,
            }
        }

        /// **The size test's stop is one-sided.** Too few is worth
        /// descending from; too many is not.
        #[test]
        fn the_goal_stops_only_when_the_selection_is_too_big() {
            let mut f = Field::new(8000);
            assert_eq!(
                rel_check_goal(&mut f, &[], 0, 0, args(1, 2, &[], &[])),
                (false, false),
                "too few, keep looking"
            );
            assert_eq!(
                rel_check_goal(&mut f, &[1, 2, 3], 0, 3, args(1, 2, &[], &[])),
                (false, true),
                "too many, stop"
            );
            assert_eq!(
                rel_check_goal(&mut f, &[1], 0, 1, args(1, 2, &[], &[])),
                (true, false)
            );
        }

        /// A selection missing a must-include card is not finished.
        #[test]
        fn the_goal_wants_the_must_include_group() {
            let mut f = Field::new(8000);
            let (res, _) = rel_check_goal(&mut f, &[1], 0, 1, args(1, 2, &[], &[9]));
            assert!(!res, "9 is not in the selection");
            let (res, _) = rel_check_goal(&mut f, &[9], 0, 1, args(1, 2, &[], &[9]));
            assert!(res);
        }

        fn always(_f: &mut Field, _sg: &[CardId], _tp: u8, _exg: &[CardId]) -> (bool, bool) {
            (true, false)
        }

        /// The caller's check and the single-use rule are **both**
        /// required, and either may stop the search.
        #[test]
        fn the_two_checks_are_combined() {
            let mut f = Field::new(8000);
            let a = Args {
                check: Some(never),
                ..args(1, 2, &[], &[])
            };
            assert_eq!(rel_check_goal(&mut f, &[1], 0, 1, a), (false, false));

            // **And the single-use rule still applies when a caller's
            // check is supplied.** With a check that always passes, the
            // only thing that can refuse is the intersection with `exg`.
            let exg = [1, 2];
            let a = Args {
                check: Some(always),
                ..args(1, 2, &exg, &[])
            };
            assert_eq!(
                rel_check_goal(&mut f, &[1], 0, 1, a),
                (true, false),
                "one of them is allowed"
            );
            assert_eq!(
                rel_check_goal(&mut f, &[1, 2], 0, 2, a),
                (false, true),
                "two is not, whatever the caller's check said"
            );
            let a = Args {
                check: Some(stop_at_two),
                ..args(1, 3, &[], &[])
            };
            assert_eq!(
                rel_check_goal(&mut f, &[1, 2], 0, 2, a),
                (true, true),
                "legal, but refusing to grow"
            );
        }

        /// `check_release_group_cost` is the search: one card is enough
        /// at `minc = 1`, and none at all is not.
        #[test]
        fn the_search_finds_a_selection_or_refuses() {
            let mut f = Field::new(8000);
            assert!(check_release_group_cost(
                &mut f,
                &[1, 2],
                0,
                args(1, 1, &[], &[])
            ));
            assert!(
                !check_release_group_cost(&mut f, &[], 0, args(1, 1, &[], &[])),
                "nothing to release"
            );
            assert!(
                !check_release_group_cost(&mut f, &[1], 0, args(2, 2, &[], &[])),
                "one card cannot make two"
            );
        }

        /// **A must-include card that is not in the pool refuses
        /// outright**, before the search runs.
        #[test]
        fn a_missing_must_include_card_refuses() {
            let mut f = Field::new(8000);
            assert!(!check_release_group_cost(
                &mut f,
                &[1, 2],
                0,
                args(1, 2, &[], &[9])
            ));
        }

        /// A stopped branch is not descended from, so a check that both
        /// permits and stops still refuses a longer selection.
        #[test]
        fn a_stopped_branch_is_not_extended() {
            let mut f = Field::new(8000);
            let a = Args {
                check: Some(wants_three_but_stops_at_two),
                ..args(1, 3, &[], &[])
            };
            assert!(
                !check_release_group_cost(&mut f, &[1, 2, 3], 0, a),
                "the only satisfying selection is past the stop"
            );
        }
    }

    mod the_loop {
        use super::*;
        use crate::host_question::{hint, hintmsg};

        /// One card, one question, and the release prompt with it.
        #[test]
        fn it_asks_once_for_one_card() {
            let (mut f, e, cards) = board(3, Cost::of(1, 1));
            let run = drive(&mut f, e, 0, &[Some(0)]);
            assert_eq!(run.asks.len(), 1);
            assert_eq!(run.asks[0].1, cards, "everything is offerable");
            assert!(!run.asks[0].3, "nothing chosen yet, so not finishable");
            assert_eq!(run.finished, Some(vec![cards[0]]));
            assert!(run.hints.contains(&(hint::SELECTMSG, 0, hintmsg::RELEASE)));
        }

        /// **`finishable` and `cancelable` are the same answer here** —
        /// the reference passes one value for both — so once the minimum
        /// is met the player may stop.
        #[test]
        fn once_the_minimum_is_met_the_question_may_be_finished() {
            let (mut f, e, cards) = board(3, Cost::of(1, 2));
            let run = drive(&mut f, e, 0, &[Some(0), None]);
            assert!(!run.asks[0].3);
            assert!(run.asks[1].3, "one is enough");
            assert_eq!(run.asks[1].4, run.asks[1].3, "and the two agree");
            assert_eq!(run.finished, Some(vec![cards[0]]));
        }

        /// A card already chosen comes back off.
        #[test]
        fn choosing_a_chosen_card_unselects_it() {
            let (mut f, e, cards) = board(3, Cost::of(1, 2));
            // take the first, give it back, take the second, then stop
            let run = drive(&mut f, e, 0, &[Some(0), Some(2), Some(1), None]);
            assert_eq!(run.asks[1].2, vec![cards[0]], "it may be given back");
            assert_eq!(run.finished, Some(vec![cards[1]]));
        }

        /// **A must-include card is in the selection from the start and
        /// cannot be given back.**
        #[test]
        fn a_must_include_card_is_kept() {
            let (mut f, e, cards) = board(3, Cost::of(1, 2));
            let cost = Cost {
                mustg: vec![cards[0]],
                ..Cost::of(1, 2)
            };
            COST.with(|x| *x.borrow_mut() = Some(cost));
            let run = drive(&mut f, e, 0, &[Some(1), None]);
            assert_eq!(
                run.asks[0].2,
                vec![cards[0]],
                "already taken before anything was asked"
            );
            let chosen = run.finished.expect("finished");
            assert!(chosen.contains(&cards[0]), "and still there");
        }

        /// Nothing releasable means no question at all — and no prompt
        /// for one either.
        #[test]
        fn an_empty_pool_asks_nothing() {
            let (mut f, e, _) = board(0, Cost::of(1, 1));
            let run = drive(&mut f, e, 0, &[]);
            assert!(run.asks.is_empty());
            assert!(
                !run.hints.iter().any(|h| h.0 == hint::SELECTMSG),
                "no prompt for a question that was never put"
            );
            assert_eq!(run.finished, Some(Vec::new()));
        }

        /// **A must-include card cannot be given back.** Choosing it
        /// again is ignored, where any other card would come off.
        #[test]
        fn a_must_include_card_cannot_be_unselected() {
            let (mut f, e, cards) = board(3, Cost::of(1, 2));
            let cost = Cost {
                mustg: vec![cards[0]],
                ..Cost::of(1, 2)
            };
            COST.with(|x| *x.borrow_mut() = Some(cost));
            // The first question offers the other two and shows cards[0]
            // as already taken; answering with it would normally remove it.
            let run = drive(&mut f, e, 0, &[Some(2), None]);
            assert_eq!(run.asks[0].2, vec![cards[0]]);
            let chosen = run.finished.expect("finished");
            assert!(
                chosen.contains(&cards[0]),
                "it is not removable, so it is still there: {chosen:?}"
            );
        }
    }
}

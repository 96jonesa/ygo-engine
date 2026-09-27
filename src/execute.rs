//! Running a card's own code: `ExecuteCost`, `ExecuteTarget`,
//! `ExecuteOperation`.
//!
//! Three near-identical units, and they are the boundary between the engine
//! and a card. Everything the chain machinery does converges here: a chain
//! link pays its cost through one, declares its targets through the second,
//! and resolves through the third.
//!
//! ## The one place the "no Lua" decision is visible in the machinery
//!
//! In ocgcore these three call a Lua **coroutine**, and the unit exists
//! because that coroutine can *yield* — pause in the middle of a cost to ask
//! the player something, and resume where it left off. The `step` the unit
//! carries is the coroutine's resume point, and `result != COROUTINE_YIELD`
//! is how it learns the function finished.
//!
//! This port has no Lua, so an **operation** carries the coroutine itself:
//! it returns [`Yield`], which is either `Done` or a `Suspended` holding
//! the rest of the work as a boxed closure. `ExecuteOperation` then runs
//! the same protocol the reference does — setup at step 0, a call on every
//! step, `false` on a yield so the unit is re-entered with whatever the
//! card queued spliced in front, and teardown only when the card is
//! actually finished.
//!
//! The parked closure lives in `core.suspensions`, addressed by a token in
//! the unit, because a unit's `Kind` is cloned on every step and must stay
//! `Clone + Debug + Eq` — which a boxed closure is not. ocgcore has the
//! same split for the same reason: the coroutine lives in the Lua registry
//! and the unit holds a reference.
//!
//! **`Target` suspends too**, because `Duel.SelectTarget` does: a card
//! that asks a player to choose cannot answer until they have. The
//! executor is the same one, with the same token.
//!
//! The **legality** call is different, and deliberately so. `chk = false`
//! is asked by `is_activate_ready` and by `check_chain_target` as a
//! *plain* call, not through a coroutine — the reference uses
//! `check_condition` there, not `call_coroutine` — so a target that
//! suspended in that path would have nothing to resume it. Those callers
//! read a suspension as a refusal rather than ignoring it, and no script
//! selects on `chk == 0` anyway.
//!
//! **`Cost` still cannot yield.** No pool card has a suspending cost, and
//! the shape to copy when one appears is directly above.
//!
//! ## `chk` is the leading `1`
//!
//! `ExecuteCost` and `ExecuteTarget` push a literal `1` ahead of the event
//! arguments; `ExecuteOperation` does not. That `1` is `chk`: "actually pay"
//! rather than "could this be paid". The port models it as the `chk: bool`
//! parameter [`crate::effect::Cost`] and [`crate::effect::Target`] already
//! take, and it is always `true` here — the legality checks that pass
//! `false` call the function directly rather than through a unit.
//!
//! ## The event being solved is a stack
//!
//! Each unit begins by splicing `sub_solving_event` onto the **front** of
//! `solving_event` and ends by popping that front. So the event a card's
//! code sees is the innermost one, and nesting restores the outer event
//! automatically. Forgetting the pop leaves an inner event visible to the
//! outer effect — which looks like the right event, because it is *an*
//! event, and is the wrong one.
//!
//! ## The shuffles are deferred to the outermost executor
//!
//! Disturbing a hand or deck sets a flag rather than shuffling. `check_level`
//! counts the nesting, and only the executor that brings it back to zero
//! performs the four shuffles. An effect whose cost disturbs a hand, called
//! inside another effect's operation, therefore shuffles **once**, when the
//! outer one finishes.
//!
//! `shuffle_check_disabled` is saved and restored around each executor
//! rather than simply cleared, so a caller that had suppressed the checks
//! gets its suppression back.

use crate::board::location;
use crate::effect::{Suspended, Yield};
use crate::event::{EffectId, PLAYER_NONE};
use crate::field::Field;

/// Which of a card's three functions an executor runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Executing {
    Cost,
    Target,
    Operation,
}

impl Field {
    /// Park a suspended operation and return the token the unit carries.
    ///
    /// Slots are reused, so a long duel does not grow the slab by one per
    /// suspension.
    fn park_suspension(&mut self, s: Suspended) -> usize {
        if let Some(i) = self.core.suspensions.iter().position(Option::is_none) {
            self.core.suspensions[i] = Some(s);
            return i;
        }
        self.core.suspensions.push(Some(s));
        self.core.suspensions.len() - 1
    }

    /// The teardown every executor shares, run once the card's function
    /// has actually finished — not when it merely suspends.
    fn finish_executor(&mut self, was_disabled: &mut bool) {
        self.core.reason_effect = None;
        self.core.reason_player = PLAYER_NONE;
        self.core.check_level -= 1;
        if self.core.check_level == 0 {
            for p in 0..2usize {
                if self.core.shuffle_hand_check[p] {
                    self.shuffle(p as u8, location::HAND);
                }
            }
            for p in 0..2usize {
                if self.core.shuffle_deck_check[p] {
                    self.shuffle(p as u8, location::DECK);
                }
            }
        }
        self.core.shuffle_check_disabled = *was_disabled;
        self.core.solving_event.pop_front();
    }

    /// Re-enter a suspended operation. Mirrors the reference's resumed
    /// `call_coroutine`: no setup, and the same finished/yielded decision.
    fn execute_resume(
        &mut self,
        effect: EffectId,
        triggering_player: u8,
        subject: Option<crate::event::CardId>,
        args: &[i64],
        was_disabled: &mut bool,
        resume: &mut Option<usize>,
    ) -> bool {
        let Some(token) = *resume else {
            // Nothing parked: the unit was re-entered without having
            // suspended, which the loop does not do. Finish rather than
            // run the operation from the top a second time.
            return true;
        };
        let Some(mut parked) = self.core.suspensions[token].take() else {
            return true;
        };
        self.core.reason_effect = Some(effect);
        self.core.reason_player = triggering_player;
        let event = self.core.solving_event.front().cloned().unwrap_or_default();
        let ctx = crate::effect::Ctx {
            reason_effect: effect,
            player: triggering_player,
            event: &event,
            card: subject,
            args,
        };
        let outcome = (parked.0)(self, &ctx);
        match outcome {
            Yield::Suspended(again) => {
                self.core.suspensions[token] = Some(again);
                self.core.returns.set(0);
                false
            }
            Yield::Done(v) => {
                *resume = None;
                self.core.returns.set(v);
                self.finish_executor(was_disabled);
                true
            }
        }
    }

    /// The body all three units share.
    ///
    /// Returns true when the unit is finished, which — see the module note —
    /// is always, because a Rust `fn` cannot yield.
    pub(crate) fn execute_step(
        &mut self,
        what: Executing,
        effect: EffectId,
        triggering_player: u8,
        subject: Option<crate::event::CardId>,
        args: &[i64],
        was_disabled: &mut bool,
    ) -> bool {
        self.execute_step_resumable(
            what,
            0,
            effect,
            triggering_player,
            subject,
            args,
            was_disabled,
            &mut None,
        )
    }

    /// The same, for the one executor whose function may suspend.
    ///
    /// `step` and `resume` are the reference's coroutine handling: it calls
    /// `call_coroutine(..., step)` on **every** step, does the setup only
    /// at step 0, and returns `FALSE` on a yield so the unit is re-entered
    /// with whatever the yielding export queued spliced in front.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn execute_step_resumable(
        &mut self,
        what: Executing,
        step: u16,
        effect: EffectId,
        triggering_player: u8,
        subject: Option<crate::event::CardId>,
        args: &[i64],
        was_disabled: &mut bool,
        resume: &mut Option<usize>,
    ) -> bool {
        // A resumed call does none of the setup: the event is already on the
        // stack, `check_level` is already raised, and re-splicing would push
        // a second copy of an event nobody pops.
        if step > 0 {
            return self.execute_resume(
                effect,
                triggering_player,
                subject,
                args,
                was_disabled,
                resume,
            );
        }
        // Splice first, and unconditionally: even an effect with no function
        // of this kind must move the pending event onto the stack and pop
        // it, or the event outlives the executor that owned it.
        let pending: Vec<_> = self.core.sub_solving_event.drain(..).collect();
        for ev in pending.into_iter().rev() {
            self.core.solving_event.push_front(ev);
        }

        let has_function = self.effects.get(effect).is_some_and(|e| match what {
            Executing::Cost => e.cost.is_some(),
            Executing::Target => e.target.is_some(),
            Executing::Operation => e.operation.is_some(),
        });
        if !has_function {
            self.core.solving_event.pop_front();
            return true;
        }

        if self.core.check_level == 0 {
            self.core.shuffle_deck_check = [false, false];
            self.core.shuffle_hand_check = [false, false];
        }
        *was_disabled = self.core.shuffle_check_disabled;
        self.core.shuffle_check_disabled = false;
        self.core.check_level += 1;

        self.core.reason_effect = Some(effect);
        self.core.reason_player = triggering_player;

        let Some(event) = self.core.solving_event.front().cloned() else {
            // Nothing to solve: unwind exactly as the tail below would.
            self.core.check_level -= 1;
            self.core.shuffle_check_disabled = *was_disabled;
            return true;
        };
        let ctx = crate::effect::Ctx {
            reason_effect: effect,
            player: triggering_player,
            event: &event,
            card: subject,
            args,
        };
        let f = self.effects.get(effect).and_then(|e| match what {
            Executing::Cost => e.cost.map(Runnable::Cost),
            Executing::Target => e.target.map(Runnable::Target),
            Executing::Operation => e.operation.map(Runnable::Operation),
        });
        let outcome = match f {
            // `chk = true`: this is the paying call, not the asking one.
            Some(Runnable::Cost(f)) => Yield::Done(i32::from(f(self, &ctx, true))),
            Some(Runnable::Target(f)) => f(self, &ctx, true, None),
            Some(Runnable::Operation(f)) => f(self, &ctx),
            None => Yield::Done(0),
        };
        if let Yield::Suspended(s) = outcome {
            // The reference writes the yielded value into `returns[0]`
            // here too; a yield carries none, and whatever the queued
            // processors produce overwrites it before the resume reads it.
            self.core.returns.set(0);
            *resume = Some(self.park_suspension(s));
            return false;
        }
        let Yield::Done(yielded) = outcome else {
            unreachable!("handled above")
        };
        self.core.returns.set(yielded);

        self.finish_executor(was_disabled);
        true
    }
}

/// The three function shapes, so the match above can pick one without
/// borrowing `self.effects` across the call.
enum Runnable {
    Cost(crate::effect::Cost),
    Target(crate::effect::Target),
    Operation(crate::effect::Operation),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{card_type, status, Card, CardData};
    use crate::effect::{effect_type, Ctx, Effect};
    use crate::event::{code, CardId, Event};
    use crate::processor::{Kind, Status};

    fn field_with_card() -> (Field, CardId) {
        let mut f = Field::new(8000);
        let mut c = Card::with_data(
            CardData {
                code: 18036057,
                type_: card_type::MONSTER,
                ..Default::default()
            },
            0,
        );
        c.current.controller = 0;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(0, id, location::MZONE, 0, false);
        (f, id)
    }

    fn effect_on(f: &mut Field, card: CardId) -> EffectId {
        let mut e = Effect::new(effect_type::SINGLE, code::UPDATE_ATTACK);
        e.owner = Some(card);
        e.handler = Some(card);
        let id = f.new_effect(e);
        f.cards[card].single_effect.insert(code::UPDATE_ATTACK, id);
        f.cards[card].indexer.insert(id);
        id
    }

    fn run(f: &mut Field) -> Status {
        for _ in 0..64 {
            match f.process() {
                Status::Continue => continue,
                other => return other,
            }
        }
        panic!("did not settle");
    }

    /// An effect with no function of the kind being executed still moves the
    /// pending event onto the stack and pops it. Skipping the splice would
    /// leave the event for the next executor to find.
    #[test]
    fn an_effect_with_no_function_still_balances_the_event_stack() {
        let (mut f, c) = field_with_card();
        let e = effect_on(&mut f, c);
        f.core
            .sub_solving_event
            .push_back(Event::new(code::CHAINING));
        f.emplace(Kind::ExecuteOperation {
            resume: None,
            effect: e,
            player: 0,
            subject: None,
            args: Vec::new(),
            was_disabled: false,
        });
        run(&mut f);
        assert!(f.core.sub_solving_event.is_empty(), "spliced");
        assert!(f.core.solving_event.is_empty(), "and popped");
    }

    /// The event a card's code sees is the innermost one, and the stack is
    /// restored on the way out.
    #[test]
    fn the_innermost_event_is_the_one_solved() {
        fn records(f: &mut Field, ctx: &Ctx) -> Yield {
            // stash the code of the event this operation was given
            f.core.hint_timing[0] = ctx.event.event_code;
            Yield::Done(0)
        }
        let (mut f, c) = field_with_card();
        let e = effect_on(&mut f, c);
        f.effects.get_mut(e).unwrap().operation = Some(records);

        // an outer event already being solved
        f.core.solving_event.push_back(Event::new(code::MOVE));
        // and the inner one this executor owns
        f.core
            .sub_solving_event
            .push_back(Event::new(code::TO_GRAVE));

        f.emplace(Kind::ExecuteOperation {
            resume: None,
            effect: e,
            player: 0,
            subject: None,
            args: Vec::new(),
            was_disabled: false,
        });
        run(&mut f);
        assert_eq!(f.core.hint_timing[0], code::TO_GRAVE, "the inner event");
        assert_eq!(
            f.core.solving_event.len(),
            1,
            "and the outer one is still there"
        );
        assert_eq!(f.core.solving_event[0].event_code, code::MOVE);
    }

    /// `chk` is the leading `1`: this is the paying call, not the asking
    /// one.
    #[test]
    fn a_cost_is_run_with_chk_true() {
        fn pays(f: &mut Field, _: &Ctx, chk: bool) -> bool {
            f.core.hint_timing[1] = u32::from(chk);
            chk
        }
        let (mut f, c) = field_with_card();
        let e = effect_on(&mut f, c);
        f.effects.get_mut(e).unwrap().cost = Some(pays);
        f.core
            .sub_solving_event
            .push_back(Event::new(code::CHAINING));
        f.emplace(Kind::ExecuteCost {
            effect: e,
            player: 0,
            subject: None,
            args: Vec::new(),
            was_disabled: false,
        });
        run(&mut f);
        assert_eq!(f.core.hint_timing[1], 1, "chk was true");
        assert_eq!(f.core.returns.get(), 1, "and its answer is the return");
    }

    /// The reason is set for the duration and cleared afterwards, so
    /// nothing outside the executor sees a stale one.
    #[test]
    fn the_reason_is_set_during_and_cleared_after() {
        fn checks(f: &mut Field, _: &Ctx) -> Yield {
            f.core.hint_timing[0] = u32::from(f.core.reason_effect.is_some());
            Yield::Done(0)
        }
        let (mut f, c) = field_with_card();
        let e = effect_on(&mut f, c);
        f.effects.get_mut(e).unwrap().operation = Some(checks);
        f.core
            .sub_solving_event
            .push_back(Event::new(code::CHAINING));
        f.emplace(Kind::ExecuteOperation {
            resume: None,
            effect: e,
            player: 0,
            subject: None,
            args: Vec::new(),
            was_disabled: false,
        });
        run(&mut f);
        assert_eq!(f.core.hint_timing[0], 1, "set while running");
        assert!(f.core.reason_effect.is_none(), "and cleared after");
        assert_eq!(f.core.reason_player, PLAYER_NONE);
    }

    mod deferred_shuffles {
        use super::*;

        fn disturbs(f: &mut Field, _: &Ctx) -> Yield {
            f.core.shuffle_hand_check[0] = true;
            Yield::Done(0)
        }

        /// A hand with something hidden in it, so that `shuffle` actually
        /// runs — it returns early on an empty hand, and on one that is
        /// entirely face-up. Without this the shuffle is unobservable and a
        /// test of it passes whether or not it happened.
        fn hand_worth_shuffling(f: &mut Field) {
            for i in 0..3u32 {
                let mut c = Card::with_data(
                    CardData {
                        code: 1000 + i,
                        type_: card_type::MONSTER,
                        ..Default::default()
                    },
                    0,
                );
                c.current.controller = 0;
                c.current.position = crate::board::position::FACEDOWN_DEFENSE;
                let id = f.new_card(c);
                f.add_card(0, id, location::HAND, 0, false);
            }
        }

        /// A disturbed hand is shuffled by the executor that brings the
        /// nesting back to zero — and `shuffle` **clears the flag**, which
        /// is how "it ran" is observable at all.
        #[test]
        fn the_outermost_executor_performs_the_shuffle() {
            let (mut f, c) = field_with_card();
            hand_worth_shuffling(&mut f);
            let e = effect_on(&mut f, c);
            f.effects.get_mut(e).unwrap().operation = Some(disturbs);
            f.core
                .sub_solving_event
                .push_back(Event::new(code::CHAINING));
            f.emplace(Kind::ExecuteOperation {
                resume: None,
                effect: e,
                player: 0,
                subject: None,
                args: Vec::new(),
                was_disabled: false,
            });
            run(&mut f);
            assert_eq!(f.core.check_level, 0, "unwound");
            assert!(
                !f.core.shuffle_hand_check[0],
                "the shuffle ran, and clears the flag as it does"
            );
        }

        /// Nesting: an inner executor must not shuffle, because it does not
        /// bring the level to zero. The flag is left set for the outer one.
        #[test]
        fn an_inner_executor_does_not_shuffle() {
            let (mut f, c) = field_with_card();
            hand_worth_shuffling(&mut f);
            let e = effect_on(&mut f, c);
            f.effects.get_mut(e).unwrap().operation = Some(disturbs);

            // Pretend an outer executor is already running.
            f.core.check_level = 1;
            f.core
                .sub_solving_event
                .push_back(Event::new(code::CHAINING));
            let mut was = false;
            f.execute_step(Executing::Operation, e, 0, None, &[], &mut was);
            assert_eq!(f.core.check_level, 1, "still inside the outer one");
            assert!(
                f.core.shuffle_hand_check[0],
                "left set for the outer executor to act on"
            );
        }

        /// `shuffle_check_disabled` is saved and restored rather than
        /// cleared, so a caller's suppression survives.
        ///
        /// The effect needs an operation: without one the executor returns
        /// before the save/restore, and the test passes without reaching the
        /// code it names.
        #[test]
        fn a_callers_suppression_is_restored() {
            fn does_nothing(_: &mut Field, _: &Ctx) -> crate::effect::Yield {
                crate::effect::Yield::Done(0)
            }
            let (mut f, c) = field_with_card();
            let e = effect_on(&mut f, c);
            f.effects.get_mut(e).unwrap().operation = Some(does_nothing);
            f.core.shuffle_check_disabled = true;
            f.core
                .sub_solving_event
                .push_back(Event::new(code::CHAINING));
            f.emplace(Kind::ExecuteOperation {
                resume: None,
                effect: e,
                player: 0,
                subject: None,
                args: Vec::new(),
                was_disabled: false,
            });
            run(&mut f);
            assert!(f.core.shuffle_check_disabled, "restored, not left cleared");
        }
    }
}

#[cfg(test)]
mod suspension_tests {
    use super::*;
    use crate::board::location;
    use crate::card::{card_type, Card, CardData};
    use crate::effect::{effect_type, Ctx, Effect};
    use crate::event::{code, Event};
    use crate::processor::{Kind, Status};
    use crate::script_api as api;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // One set of counters per test: these run in parallel threads, and a
    // shared counter makes each test see the others' work.
    static PLAIN_ENTRIES: AtomicUsize = AtomicUsize::new(0);
    static SUSP_ENTRIES: AtomicUsize = AtomicUsize::new(0);
    static SUSP_RESUMES: AtomicUsize = AtomicUsize::new(0);
    static TWICE_RESUMES: AtomicUsize = AtomicUsize::new(0);
    /// What the continuation saw in `returns` when it woke up.
    static TWICE_SAW: AtomicUsize = AtomicUsize::new(0);
    static SAW_REASON: AtomicUsize = AtomicUsize::new(0);
    static NESTED: AtomicUsize = AtomicUsize::new(0);
    static TARGET_SAW: AtomicUsize = AtomicUsize::new(0);

    fn field_with_effect(op: crate::effect::Operation) -> (Field, EffectId) {
        let mut f = Field::new(8000);
        for p in 0..2u8 {
            for i in 0..5u32 {
                let mut c = Card::with_data(
                    CardData {
                        code: 5_053_103,
                        type_: card_type::MONSTER,
                        ..Default::default()
                    },
                    p,
                );
                c.current.controller = p;
                let id = f.new_card(c);
                f.add_card(p, id, location::DECK, i, false);
            }
        }
        let mut c = Card::with_data(
            CardData {
                code: 1234,
                type_: card_type::SPELL,
                ..Default::default()
            },
            0,
        );
        c.current.controller = 0;
        let card = f.new_card(c);
        f.add_card(0, card, location::SZONE, 0, false);
        let mut e = Effect::new(effect_type::ACTIVATE, code::FREE_CHAIN);
        e.owner = Some(card);
        e.handler = Some(card);
        e.operation = Some(op);
        let e = f.new_effect(e);
        (f, e)
    }

    /// Run an `ExecuteOperation` unit to completion, reporting how many
    /// times the processor loop turned.
    fn run(f: &mut Field, e: EffectId) -> usize {
        f.core
            .sub_solving_event
            .push_back(Event::new(code::FREE_CHAIN));
        f.emplace(Kind::ExecuteOperation {
            resume: None,
            effect: e,
            player: 0,
            subject: None,
            args: Vec::new(),
            was_disabled: false,
        });
        for turn in 1..512 {
            match f.process() {
                Status::Continue => {
                    if f.core.units.is_empty() {
                        return turn;
                    }
                }
                other => panic!("stopped with {other:?}"),
            }
        }
        panic!("did not settle")
    }

    /// **An operation that does not suspend still finishes at the first
    /// step**, which is every card written so far.
    #[test]
    fn a_plain_operation_finishes_at_once() {
        fn plain(f: &mut Field, _ctx: &Ctx) -> Yield {
            PLAIN_ENTRIES.fetch_add(1, Ordering::SeqCst);
            f.players[0].lp -= 100;
            api::done()
        }
        let (mut f, e) = field_with_effect(plain);
        run(&mut f, e);
        assert_eq!(PLAIN_ENTRIES.load(Ordering::SeqCst), 1, "entered once");
        assert_eq!(f.players[0].lp, 7900, "and it ran");
        assert_eq!(f.core.check_level, 0, "the executor tore down");
        assert!(f.core.solving_event.is_empty(), "and popped its event");
    }

    /// **A suspended operation runs its rest later, and only once.** The
    /// work it queued — a draw — happens in between, and the executor does
    /// **not** tear down until the rest finishes: `check_level` stays
    /// raised across the suspension, which is what stops the shuffles
    /// firing halfway through a card.
    #[test]
    fn a_suspended_operation_resumes_after_its_queued_work() {
        fn suspends(f: &mut Field, ctx: &Ctx) -> Yield {
            SUSP_ENTRIES.fetch_add(1, Ordering::SeqCst);
            let player = ctx.player;
            api::draw(f, player, 2, crate::card::reason::EFFECT);
            api::suspend(move |f, _ctx| {
                SUSP_RESUMES.fetch_add(1, Ordering::SeqCst);
                // Something observable, and something that proves the
                // captured local came across.
                f.players[player as usize].lp -= 500;
                api::done()
            })
        }
        let (mut f, e) = field_with_effect(suspends);
        let deck_before = f.players[0].main.len();
        run(&mut f, e);
        assert_eq!(
            SUSP_ENTRIES.load(Ordering::SeqCst),
            1,
            "the operation ran once"
        );
        assert_eq!(
            SUSP_RESUMES.load(Ordering::SeqCst),
            1,
            "and its rest ran once"
        );
        assert_eq!(
            f.players[0].main.len(),
            deck_before - 2,
            "the queued draw happened in between"
        );
        assert_eq!(f.players[0].lp, 7500, "the captured player came across");
        assert_eq!(f.core.check_level, 0, "and only then did it tear down");
        assert!(f.core.solving_event.is_empty());
        assert!(
            f.core.suspensions.iter().all(Option::is_none),
            "the parking slot was released"
        );
    }

    /// **A target may suspend too, and its answer still arrives.**
    ///
    /// The reference drives all three executors through
    /// `call_coroutine`, and a target that selects — `Duel.SelectTarget`
    /// — suspends exactly as an operation does. What must survive the
    /// round trip is the **answer**: a target reports yes or no through
    /// `returns`, and a suspension must not lose it.
    #[test]
    fn a_target_may_suspend_and_still_answer() {
        fn picks(f: &mut Field, _ctx: &Ctx) -> Yield {
            api::draw(f, 0, 1, crate::card::reason::EFFECT);
            api::suspend(|f, _| {
                TARGET_SAW.store(api::resumed_value(f) as usize, Ordering::SeqCst);
                api::yes(true)
            })
        }
        fn as_target(
            f: &mut Field,
            ctx: &Ctx,
            _chk: bool,
            _chkc: Option<crate::event::CardId>,
        ) -> Yield {
            picks(f, ctx)
        }
        let (mut f, e) = field_with_effect(|_, _| api::done());
        f.effects.get_mut(e).unwrap().target = Some(as_target);
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
        for _ in 0..512 {
            match f.process() {
                Status::Continue => {
                    if f.core.units.is_empty() {
                        break;
                    }
                }
                other => panic!("stopped with {other:?}"),
            }
        }
        assert_eq!(
            TARGET_SAW.load(Ordering::SeqCst),
            1,
            "the queued draw ran before the target finished"
        );
        assert_eq!(
            f.core.returns.at_i32(0),
            1,
            "and the target's answer survived the suspension"
        );
        assert_eq!(f.core.check_level, 0, "the executor tore down once");
    }

    /// **A resumed operation is given the reason effect back.** Anything
    /// it queues attributes itself to `core.reason_effect`, so a
    /// continuation that woke up without it would queue work belonging to
    /// nobody — a draw with no reason, a send with no source.
    #[test]
    fn a_resumed_operation_sees_the_reason_effect() {
        fn checks(f: &mut Field, ctx: &Ctx) -> Yield {
            let mine = ctx.reason_effect;
            // Queue a *nested executor*, not a draw. Its teardown clears
            // `reason_effect`, which is what makes the restore on resume
            // observable — after a draw it is merely still set.
            let other = NESTED.load(Ordering::SeqCst);
            f.core
                .sub_solving_event
                .push_back(Event::new(code::FREE_CHAIN));
            f.emplace(Kind::ExecuteOperation {
                resume: None,
                effect: other,
                player: 0,
                subject: None,
                args: Vec::new(),
                was_disabled: false,
            });
            api::suspend(move |f, _| {
                SAW_REASON.store(
                    usize::from(f.core.reason_effect == Some(mine)),
                    Ordering::SeqCst,
                );
                api::done()
            })
        }
        fn inner(_f: &mut Field, _ctx: &Ctx) -> Yield {
            api::done()
        }
        let (mut f, e) = field_with_effect(checks);
        // A second effect for the nested executor to run.
        let mut second = Effect::new(effect_type::ACTIVATE, code::FREE_CHAIN);
        second.owner = f.effects.get(e).and_then(|x| x.owner);
        second.handler = f.effects.get(e).and_then(|x| x.handler);
        second.operation = Some(inner);
        let second = f.new_effect(second);
        NESTED.store(second, Ordering::SeqCst);
        run(&mut f, e);
        assert_eq!(
            SAW_REASON.load(Ordering::SeqCst),
            1,
            "the continuation was given the reason effect back"
        );
    }

    /// **An operation may suspend more than once**, and each resumption
    /// sees the result of the work it queued — which is the whole point:
    /// a card branching on a draw's result needs the value back, not just
    /// the turn back.
    #[test]
    fn an_operation_may_suspend_twice_and_reads_each_result() {
        fn twice(f: &mut Field, ctx: &Ctx) -> Yield {
            let player = ctx.player;
            api::draw(f, player, 2, crate::card::reason::EFFECT);
            api::suspend(move |f, _| {
                TWICE_SAW.store(api::resumed_value(f) as usize, Ordering::SeqCst);
                api::draw(f, player, 1, crate::card::reason::EFFECT);
                api::suspend(move |f, _| {
                    TWICE_RESUMES.fetch_add(1, Ordering::SeqCst);
                    f.players[player as usize].lp -= api::resumed_value(f);
                    api::done()
                })
            })
        }
        let (mut f, e) = field_with_effect(twice);
        let deck_before = f.players[0].main.len();
        run(&mut f, e);
        assert_eq!(TWICE_RESUMES.load(Ordering::SeqCst), 1);
        assert_eq!(
            f.players[0].main.len(),
            deck_before - 3,
            "both draws happened"
        );
        assert_eq!(
            TWICE_SAW.load(Ordering::SeqCst),
            2,
            "the first draw's count came back"
        );
        assert_eq!(f.players[0].lp, 8000 - 1, "and the second's");
        assert_eq!(f.core.check_level, 0);
    }
}

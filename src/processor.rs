//! The processor loop: a cooperative step machine over a queue of units.
//!
//! A translation of `field::process()` and the unit queue it drives. See
//! `docs/processor-loop.md` for the reading this was written from.
//!
//! Every ordering subtlety in the rules is expressed as the relative
//! position of units in this queue, so the machine is translated rather
//! than abstracted away.

use crate::execute::Executing;
use crate::field::Field;

/// What one call to [`Field::process`] achieved.
///
/// `field::process()` returns `OCG_DuelStatus`; the three values the loop
/// itself produces are these. The engine never blocks waiting for a player
/// — it yields [`Status::Awaiting`] and expects a response before the next
/// call.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    /// A step ran. Call again.
    Continue,
    /// The unit at the front wants a player's answer first.
    Awaiting,
    /// The queue is empty: the duel is over.
    End,
}

/// Assigning this to a unit's step means "run me again from the top": the
/// loop's `+= 1` wraps it to zero.
///
/// ocgcore spells it `static_cast<uint16_t>(~(uint16_t()))` and relies on
/// the same overflow. Rust panics on overflow in debug builds, so the loop
/// uses `wrapping_add` — the wrap is the mechanism, not an accident.
pub const RESTART: u16 = u16::MAX;

/// A resumable procedure. Its handler is a match on `step`, and it stays at
/// the front of the queue until its handler reports that it is finished.
#[derive(Clone, Debug)]
pub struct Unit {
    pub kind: Kind,
    pub step: u16,
}

impl Unit {
    /// A unit that will begin at step 0.
    pub fn new(kind: Kind) -> Self {
        Self { kind, step: 0 }
    }

    /// A unit entered at a given step, for the callers that resume one
    /// part-way (`emplace_process<T>(Step{ 30 }, ...)`).
    pub fn at(kind: Kind, step: u16) -> Self {
        Self { kind, step }
    }

    /// Whether this unit yields to the host for a player's answer.
    fn needs_answer(&self) -> bool {
        self.kind.needs_answer()
    }
}

/// The unit types.
///
/// ocgcore holds these in a `std::variant` and dispatches with `std::visit`;
/// here they are an enum and a `match`. Variants arrive as the machinery
/// they need is ported — the loop below is complete without them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A placeholder so the loop can be exercised before any real unit
    /// exists: it runs for `steps` steps and retires.
    Nop {
        steps: u16,
    },
    /// Exercises the restart wrap the way a real handler does — by
    /// assigning [`RESTART`] to its own step and reporting "not finished".
    Loop {
        laps: u16,
    },
    /// Build the next chain in `new_chains` onto the chain stack.
    ///
    /// Carries one piece of state across its steps, exactly as the
    /// reference's unit struct does: whether the subroutine at steps 10-11
    /// decided this is an activated effect.
    AddChain {
        is_activated_effect: bool,
    },
    /// Run an effect's `cost`. `ExecuteCost` in the reference.
    ExecuteCost {
        effect: usize,
        player: u8,
        /// The card and integers the reference pushes before the call.
        /// Empty for the chain-resolution callers, which push only the
        /// event; the summon machines push a card and four numbers.
        subject: Option<usize>,
        args: Vec<i64>,
        was_disabled: bool,
    },
    /// Run an effect's `target`.
    ExecuteTarget {
        /// Where the suspended rest of the target is parked. A target
        /// that selects suspends exactly as an operation does.
        resume: Option<usize>,
        effect: usize,
        player: u8,
        /// The card and integers the reference pushes before the call.
        /// Empty for the chain-resolution callers, which push only the
        /// event; the summon machines push a card and four numbers.
        subject: Option<usize>,
        args: Vec<i64>,
        was_disabled: bool,
    },
    /// Run an effect's `operation`.
    ExecuteOperation {
        effect: usize,
        player: u8,
        /// Where the suspended rest of the operation is parked, if it
        /// suspended. `None` on the first step and once it finishes.
        resume: Option<usize>,
        /// The card and integers the reference pushes before the call.
        /// Empty for the chain-resolution callers, which push only the
        /// event; the summon machines push a card and four numbers.
        subject: Option<usize>,
        args: Vec<i64>,
        /// `shuffle_check_was_disabled` — saved on the way in and restored
        /// on the way out, so a caller that had suppressed the pile-shuffle
        /// checks gets its suppression back.
        was_disabled: bool,
    },
    /// Ask a player a yes/no question about a card.
    SelectEffectYesNo {
        player: u8,
        description: u64,
        card: usize,
    },
    /// Ask the turn player what to do in the Main Phase. Reads the six
    /// lists `IdleCommand` built into `core`; carries only who is asked.
    SelectIdleCmd {
        player: u8,
    },
    /// Ask a player to choose among `select_options`.
    SelectOption {
        player: u8,
    },
    /// Choose tributes one card at a time, re-offering as the requirement
    /// is met. A loop around `SelectUnselectCard`, not a question itself.
    SelectTribute {
        target: usize,
        player: u8,
        cancelable: bool,
        min: u8,
        max: u8,
        toplayer: u8,
        zone: u32,
        state: Box<crate::select_tribute::SelectTributeState>,
    },
    /// Ask a player to choose tributes, counted by worth rather than by
    /// card.
    SelectTributeP {
        player: u8,
        cancelable: bool,
        min: u8,
        max: u8,
    },
    /// Ask a player for **one** card, from the offered list or from what is
    /// already chosen.
    SelectUnselectCard {
        player: u8,
        cancelable: bool,
        min: u8,
        max: u8,
        finishable: bool,
    },
    /// Ask a player to choose cards from `select_cards`.
    SelectCard {
        player: u8,
        cancelable: bool,
        /// Narrowed to what is actually on offer by step 0, and **written
        /// back**, so step 1 validates against the same bounds the question
        /// was asked with. The reference stores them on the unit for this
        /// reason; re-deriving them in step 1 would read whatever
        /// `select_cards` holds by then.
        min: u8,
        max: u8,
    },
    /// Ask a player a bare yes/no.
    SelectYesNo {
        player: u8,
        description: u64,
    },
    /// Solver mode: the coin question ([`Message::SelectCoin`]).
    SelectCoin {
        player: u8,
        count: u8,
    },
    /// Solver mode: the random-selection question ([`Message::SelectRandom`]).
    SelectRandom {
        player: u8,
        count: u8,
        cards: Vec<crate::event::CardId>,
    },
    /// Solver mode: the deck-top question ([`Message::SelectDeckTop`]).
    SelectDeckTop {
        player: u8,
        count: u32,
    },
    /// Ask a player which way up to put a card.
    SelectPosition {
        player: u8,
        code: u32,
        positions: u8,
    },
    /// Ask a player to choose one or more zones. `flag` masks the zones that
    /// are **not** available.
    SelectPlace {
        player: u8,
        flag: u32,
        count: u8,
        disable_field: bool,
    },
    /// Resolve the chain, link by link.
    SolveChain {
        skip: crate::solve_chain::SolveChainSkip,
    },
    /// Open a timing window for the triggers gathered so far.
    PointEvent {
        skip: crate::point_event::PointEventSkip,
    },
    /// Destroy a card that is over its own face-up limit.
    SelfDestroyUnique {
        card: usize,
        player: u8,
    },
    /// Destroy the cards whose effects say they destroy themselves.
    SelfDestroy,
    /// Send to the graveyard the cards whose effects say so.
    SelfToGrave,
    /// Draw cards.
    Draw {
        playerid: u8,
        count: u32,
        why: u32,
        by: Option<usize>,
        reason_player: u8,
        drawn_set: Vec<usize>,
        /// Solver mode: the deck-top question has been asked, so step 0 is
        /// being re-entered to draw.
        chance_asked: bool,
    },
    /// Set several Spells or Traps at once. Two backward loops: seats are
    /// all chosen before any card moves.
    SpellSetGroup {
        setplayer: u8,
        toplayer: u8,
        targets: Vec<usize>,
        confirm: bool,
        reason_effect: Option<usize>,
        set_cards: Vec<usize>,
    },
    /// Attach an equip card to a monster.
    Equip {
        equip_player: u8,
        equip_card: usize,
        target: usize,
        faceup: bool,
        is_step: bool,
    },
    /// Choose what to release. A fast path and a one-card-at-a-time loop.
    SelectRelease {
        playerid: u8,
        cancelable: bool,
        min: u16,
        max: u16,
        check_field: bool,
        to_check: Option<usize>,
        toplayer: u8,
        zone: u32,
        state: Box<crate::select_release::SelectReleaseState>,
    },
    /// Take counters off cards. Cases 0-4; the named-card and replacing
    /// paths jump past case 3 and so raise no event.
    RemoveCounter {
        reason: u32,
        pcard: Option<usize>,
        rplayer: u8,
        self_side: bool,
        oppo_side: bool,
        counter_type: u16,
        count: u16,
    },
    /// Split a counter removal across the cards that hold them.
    SelectCounter {
        playerid: u8,
        counter_type: u16,
        count: u16,
        self_side: bool,
        oppo_side: bool,
    },
    /// Toss coins. Cases 0, 1 and 3 — there is no case 2.
    TossCoin {
        reason_effect: Option<usize>,
        reason_player: u8,
        playerid: u8,
        count: u8,
    },
    /// Declare monster Types. The only unit here that asks a question.
    AnnounceRace {
        playerid: u8,
        count: u8,
        available: u64,
    },
    /// Discard from the hand: ask which cards, then send them.
    DiscardHand {
        playerid: u8,
        min: u8,
        max: u8,
        reason: u32,
    },
    /// Mill from the top of the deck. Asks nothing.
    DiscardDeck {
        playerid: u8,
        count: u16,
        reason: u32,
        /// What actually moved — `core.discarded_set` in the reference,
        /// carried on the unit here because only this unit reads it.
        discarded: Vec<usize>,
    },
    /// Destroy a batch of cards.
    ///
    /// Two entry points: step 0 for an ordinary destruction, step 10 for the
    /// battle path, which resolves protection and then stops rather than
    /// sending anything.
    Destroy {
        targets: usize,
        reason_effect: Option<usize>,
        reason: u32,
        reason_player: u8,
    },
    /// Offer one card's own replacement effects a chance at its destruction.
    DestroyReplace {
        targets: usize,
        target: usize,
        battle: bool,
    },
    /// Release a batch of cards.
    Release {
        targets: usize,
        reason_effect: Option<usize>,
        reason: u32,
        reason_player: u8,
    },
    /// Offer one card's own replacement effects a chance at its release.
    ReleaseReplace {
        targets: usize,
        target: usize,
    },
    /// Change the position of a batch of cards.
    ///
    /// The target position is on each card's `position_param`, not here: one
    /// call can move different cards to different positions.
    ChangePos {
        targets: usize,
        reason_effect: Option<usize>,
        reason_player: u8,
        enable: bool,
        state: Box<crate::change_position::ChangePosState>,
    },
    /// Normal summon a monster.
    SummonRule {
        sumplayer: u8,
        target: usize,
        ignore_count: bool,
        min_tribute: u8,
        zone: u32,
        state: Box<crate::summon_rule::SummonRuleState>,
    },
    /// A monster Special Summoning **itself** by its own printed procedure.
    SpSummonRule {
        sumplayer: u8,
        target: usize,
        summon_type: u32,
        state: Box<crate::spsummon_rule::SpSummonRuleState>,
    },
    /// Special Summon a batch of cards: one `SpSummonStep` each, then
    /// everything that happens once.
    SpSummon {
        reason_effect: Option<usize>,
        reason_player: u8,
        targets: usize,
        zone: u32,
    },
    /// Special Summon **one** card. `targets` is the batch it belongs to, or
    /// `None` when it was summoned on its own.
    SpSummonStep {
        targets: Option<usize>,
        target: usize,
        zone: u32,
        state: crate::spsummon::SpSummonStepState,
    },
    /// Turn a face-down monster face-up as a summon.
    FlipSummon {
        sumplayer: u8,
        target: usize,
        state: crate::flip_summon::FlipSummonState,
    },
    /// Set a Spell or Trap face-down.
    ///
    /// `setplayer` is who is doing it and `toplayer` is whose field it lands
    /// on; they differ only when an effect sets a card onto the opponent's
    /// side.
    SpellSet {
        setplayer: u8,
        toplayer: u8,
        target: usize,
        reason_effect: Option<usize>,
    },
    /// Set a monster face-down in defence.
    MonsterSet {
        setplayer: u8,
        target: usize,
        ignore_count: bool,
        min_tribute: u8,
        zone: u32,
        state: Box<crate::monster_set::MonsterSetState>,
    },
    /// Put one card onto the field.
    ///
    /// `location` and the two players live packed on the card's
    /// `to_field_param`, as in the reference; the unit carries the rest.
    MoveToField {
        target: usize,
        enable: bool,
        ret: u8,
        pzone: bool,
        zone: u32,
        rule: bool,
        location_reason: u8,
        confirm: bool,
    },
    /// Send a batch of cards somewhere that is not the field.
    ///
    /// The destination is **not** here: it is on each card's `sendto_param`,
    /// because redirects rewrite it per card and one batch can scatter.
    SendTo {
        targets: usize,
        reason_effect: Option<usize>,
        reason: u32,
        reason_player: u8,
        state: Box<crate::send_to::SendToState>,
    },
    /// Offer one card's own replacement effects a chance at its move.
    SendToReplace {
        targets: usize,
        target: usize,
    },
    /// Offer one replacement effect a chance at a move.
    OperationReplace {
        replace_effect: usize,
        targets: usize,
        target: Option<usize>,
        is_destroy: bool,
    },
    /// Carry out an attack a card forced.
    /// The Main Phase: offer the turn player their menu, act on the
    /// answer, and offer it again. Loops until they leave the phase.
    IdleCommand {
        state: crate::idle_command::IdleCommandState,
    },
    /// Deal the opening hands and start the first turn. Runs once.
    Startup,
    /// Hand the deck on in a relay duel. Its two steps are the two
    /// players, not two stages.
    RefreshRelay,
    /// One whole turn, from the Draw Phase to the End Phase, then again
    /// with the players swapped. The outermost loop.
    Turn {
        turn_player: u8,
        state: crate::turn::TurnState,
    },
    /// The Battle Phase: offer the menu, carry out an attack, run the
    /// damage step, and go round again. The largest unit in the engine.
    BattleCommand {
        state: Box<crate::battle_command::BattleCommandState>,
    },
    /// One attack's damage step.
    DamageStep {
        state: Box<crate::damage_step::DamageStepState>,
    },
    /// Take life points, gain them, or pay a cost.
    Damage {
        arg: Box<crate::life_points::LpChange>,
    },
    Recover {
        arg: Box<crate::life_points::LpChange>,
    },
    PayLPCost {
        playerid: u8,
        cost: u32,
    },
    /// Ask a player to put `core.select_cards` in an order.
    SortCard {
        player: u8,
        is_chain: bool,
    },
    /// Ask a player to order their own chain links. Reads the turn player
    /// to decide which of the two chain lists it is sorting.
    SortChain {
        player: u8,
    },
    /// Ask the turn player what to do in the Battle Phase.
    SelectBattleCmd {
        player: u8,
    },
    /// Negate the attack in progress. Answers through `returns`: 0 for
    /// "nothing to negate", 1 for "negated".
    AttackDisable,
    ForcedBattle {
        state: crate::battle::ForcedBattleState,
    },
    /// Offer one player their quick effects.
    ///
    /// `is_opponent` is unit state, as it is in the reference: steps 0-1
    /// sweep the turn player's mandatory quick effects and then set it to
    /// sweep the opponent's. It only ever goes one way, which is what makes
    /// the sweep terminate.
    QuickEffect {
        skip_freechain: bool,
        player: u8,
        is_opponent: bool,
    },
    /// Ask a player to choose a chain from `select_chains`.
    SelectChain {
        player: u8,
        spe_count: u8,
        forced: bool,
    },
    /// Move monsters to the other player's side.
    GetControl {
        reason_effect: Option<usize>,
        chose_player: u8,
        targets: usize,
        playerid: u8,
        reset_phase: u16,
        reset_count: u8,
        zone: u32,
        state: Box<crate::get_control::GetControlState>,
    },
    /// Exchange control of two monsters, seat for seat.
    SwapControl {
        reason_effect: Option<usize>,
        reason_player: u8,
        targets1: usize,
        targets2: usize,
        reset_phase: u16,
        reset_count: u8,
        state: Box<crate::swap_control::SwapControlState>,
    },
    /// Put trap monsters that have stopped being monsters back where they
    /// belong.
    TrapMonsterAdjust {
        state: Box<crate::trap_monster_adjust::TrapMonsterAdjustState>,
    },
    /// The window a phase opens when it begins or ends.
    PhaseEvent {
        phase: u16,
        state: crate::phase_event::PhaseEventState,
    },
    /// Recompute which seats are unusable.
    RefreshLoc {
        state: crate::refresh_loc::RefreshLocState,
    },
    /// Re-evaluate the continuous effects on the field.
    Adjust,
    /// Resolve the continuous effects queued in `sub_solving_continuous`.
    ///
    /// The first real unit. Its handler arrives with the chain machinery;
    /// the gather needs to *emplace* it, which is what a unit variant is
    /// for, and emplacing something whose handler does not exist yet is
    /// honest only because the loop cannot reach it — nothing starts a duel
    /// yet.
    SolveContinuous {
        state: crate::solve_continuous::SolveContinuousState,
    },
}

impl Kind {
    /// The unit types that yield to the host mid-way. ocgcore declares this
    /// per type as `static constexpr auto needs_answer`.
    fn needs_answer(&self) -> bool {
        match self {
            // The three `Select*` units yield to the host for an answer.
            // ocgcore declares this per unit type as
            // `static constexpr auto needs_answer`.
            Kind::SelectEffectYesNo { .. }
            | Kind::SelectIdleCmd { .. }
            | Kind::SelectBattleCmd { .. }
            | Kind::SortCard { .. }
            | Kind::SelectOption { .. }
            | Kind::SelectYesNo { .. }
            | Kind::SelectCoin { .. }
            | Kind::SelectRandom { .. }
            | Kind::SelectDeckTop { .. }
            | Kind::SelectPosition { .. }
            | Kind::SelectPlace { .. }
            | Kind::SelectCard { .. }
            | Kind::SelectTributeP { .. }
            | Kind::SelectUnselectCard { .. }
            | Kind::SelectChain { .. }
            | Kind::AnnounceRace { .. }
            | Kind::SelectCounter { .. } => true,
            Kind::Nop { .. }
            | Kind::Loop { .. }
            | Kind::SolveContinuous { .. }
            | Kind::AddChain { .. }
            | Kind::ExecuteCost { .. }
            | Kind::ExecuteTarget { .. }
            | Kind::ExecuteOperation { .. }
            | Kind::SolveChain { .. }
            | Kind::PointEvent { .. }
            | Kind::QuickEffect { .. }
            | Kind::Draw { .. }
            | Kind::DiscardHand { .. }
            | Kind::DiscardDeck { .. }
            | Kind::TossCoin { .. }
            | Kind::RemoveCounter { .. }
            | Kind::SelectRelease { .. }
            | Kind::Equip { .. }
            | Kind::SpellSetGroup { .. }
            | Kind::IdleCommand { .. }
            | Kind::Startup
            | Kind::RefreshRelay
            | Kind::Turn { .. }
            | Kind::SortChain { .. }
            | Kind::BattleCommand { .. }
            | Kind::DamageStep { .. }
            | Kind::Damage { .. }
            | Kind::Recover { .. }
            | Kind::PayLPCost { .. }
            | Kind::AttackDisable
            | Kind::ForcedBattle { .. }
            | Kind::SelfDestroyUnique { .. }
            | Kind::SelfDestroy
            | Kind::SelfToGrave
            | Kind::OperationReplace { .. }
            // `Process<false>` in the reference: a loop around the two
            // questions below, not a question itself. Marking it as needing
            // an answer makes it yield before it has asked anything.
            | Kind::SelectTribute { .. }
            | Kind::SendTo { .. }
            | Kind::SendToReplace { .. }
            | Kind::MoveToField { .. }
            | Kind::SpellSet { .. }
            | Kind::MonsterSet { .. }
            | Kind::SummonRule { .. }
            | Kind::FlipSummon { .. }
            | Kind::SpSummon { .. }
            | Kind::SpSummonRule { .. }
            | Kind::RefreshLoc { .. }
            | Kind::PhaseEvent { .. }
            | Kind::TrapMonsterAdjust { .. }
            | Kind::GetControl { .. }
            | Kind::SwapControl { .. }
            | Kind::SpSummonStep { .. }
            | Kind::ChangePos { .. }
            | Kind::Destroy { .. }
            | Kind::DestroyReplace { .. }
            | Kind::Release { .. }
            | Kind::ReleaseReplace { .. }
            | Kind::Adjust => false,
        }
    }
}

impl Field {
    /// Queue a unit to run *before* the one that queued it resumes.
    ///
    /// This is `emplace_process<T>()`. Note that it does not run anything:
    /// the splice happens at the top of the next `process` call.
    pub fn emplace(&mut self, kind: Kind) {
        self.core.subunits.push(Unit::new(kind));
    }

    /// `emplace_process<T>(Step{ n }, ...)`: queue a unit entered at `n`.
    pub fn emplace_at(&mut self, kind: Kind, step: u16) {
        self.core.subunits.push(Unit::at(kind, step));
    }

    /// Put a unit at the back of the queue — the entry point for starting a
    /// duel, rather than something a running unit does.
    pub fn push_back(&mut self, kind: Kind) {
        self.core.units.push_back(Unit::new(kind));
    }

    /// Units currently queued, front first. For tests and debugging.
    pub fn queue(&self) -> impl Iterator<Item = &Unit> {
        self.core.units.iter()
    }

    /// Change the step of the unit at the front of the queue.
    ///
    /// `arg.step = n` in a handler. Assigning [`RESTART`] here is how a unit
    /// loops; assigning any other value is how it jumps, and the loop's
    /// increment means the *next* case to run is `n + 1`.
    pub fn set_step(&mut self, step: u16) {
        if let Some(unit) = self.core.units.front_mut() {
            unit.step = step;
        }
    }

    /// One step of the machine.
    ///
    /// The literal translation would take `&mut self` and a reference to the
    /// front unit at once, which Rust forbids. Popping the unit, running it,
    /// and pushing it back when unfinished is observationally identical:
    /// subunits emplaced during the handler are spliced ahead of it on the
    /// next call either way.
    pub fn process(&mut self) -> Status {
        // `core.units.splice(core.units.begin(), core.subunits)` — emplaced
        // work jumps the queue, keeping the order it was emplaced in. Popped
        // from the back and pushed to the front: no intermediate collection.
        while let Some(unit) = self.core.subunits.pop() {
            self.core.units.push_front(unit);
        }

        // The unit stays at the front while its handler runs, so the handler
        // can assign its own step — `arg.step = 9` and friends — and so that
        // anything it emplaces is spliced ahead of it next call. The
        // reference keeps the unit in place throughout and mutates it; here
        // its kind is *taken* for the dispatch and put back afterwards, in
        // place of the clone this used to make every step. `Kind::Adjust`
        // is the stand-in: nothing reads the front's kind during a dispatch.
        let mut unit = {
            let Some(front) = self.core.units.front_mut() else {
                return Status::End;
            };
            Unit {
                kind: std::mem::replace(&mut front.kind, Kind::Adjust),
                step: front.step,
            }
        };
        // `PORT_UNITS=1`: every unit as it runs, with the message count, so
        // a trace divergence can be tied to the processor step that wrote it.
        // Read once: an environment lookup per step is a third of a game.
        static UNITS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if *UNITS.get_or_init(|| std::env::var_os("PORT_UNITS").is_some()) {
            let kind = format!("{:?}", unit.kind);
            let name = kind
                .split(|c: char| !c.is_alphanumeric())
                .next()
                .unwrap_or("");
            eprintln!("UNIT m{} {name} step {}", self.messages.len(), unit.step);
        }
        let done = self.dispatch(&mut unit);
        let Some(front) = self.core.units.front_mut() else {
            return Status::Continue;
        };
        // A handler that jumped assigned its own step; otherwise carry the
        // one the dispatch left.
        if front.step == unit.step {
            front.kind = unit.kind;
        } else {
            unit.step = front.step;
            *front = unit;
        }

        if done {
            self.core.units.pop_front();
            Status::Continue
        } else {
            let front = self.core.units.front_mut().unwrap();
            // Not finished: advance and resume it next call. The wrap is
            // how RESTART means "from the top".
            front.step = front.step.wrapping_add(1);
            if front.needs_answer() {
                Status::Awaiting
            } else {
                Status::Continue
            }
        }
    }

    /// `std::visit` over the unit types. Returns true when the unit is done.
    fn dispatch(&mut self, unit: &mut Unit) -> bool {
        match unit.kind {
            // `saturating_add`: a handler is never entered at RESTART -
            // ocgcore assigns it and the loop's increment wraps it to 0
            // before the next dispatch - but arithmetic on `step` must not
            // panic if one ever is.
            Kind::Nop { steps } => unit.step.saturating_add(1) >= steps,
            Kind::AddChain {
                ref mut is_activated_effect,
            } => {
                let mut state = *is_activated_effect;
                let done = self.add_chain_step(unit.step, &mut state);
                *is_activated_effect = state;
                done
            }
            Kind::SolveChain { skip } => self.solve_chain_step(unit.step, skip),
            Kind::PointEvent { skip } => self.point_event_step(unit.step, skip),
            Kind::Draw {
                playerid,
                ref mut count,
                why,
                by,
                reason_player,
                ref mut drawn_set,
                ref mut chance_asked,
            } => {
                let (mut c, mut d) = (*count, std::mem::take(drawn_set));
                let done = self.draw_step(
                    unit.step,
                    playerid,
                    &mut c,
                    why,
                    by,
                    reason_player,
                    &mut d,
                    chance_asked,
                );
                *count = c;
                *drawn_set = d;
                done
            }
            Kind::SpellSetGroup {
                setplayer,
                toplayer,
                ref targets,
                confirm,
                reason_effect,
                ref mut set_cards,
            } => {
                let (tg, mut sc) = (targets.clone(), std::mem::take(set_cards));
                let done = self.spell_set_group_step(
                    unit.step,
                    setplayer,
                    toplayer,
                    &tg,
                    confirm,
                    reason_effect,
                    &mut sc,
                );
                *set_cards = sc;
                done
            }
            Kind::Equip {
                equip_player,
                equip_card,
                target,
                faceup,
                is_step,
            } => self.equip_step(unit.step, equip_player, equip_card, target, faceup, is_step),
            Kind::SelectRelease {
                playerid,
                cancelable,
                min,
                max,
                check_field,
                to_check,
                toplayer,
                zone,
                ref mut state,
            } => {
                let mut s = std::mem::take(state);
                let done = self.select_release_step(
                    unit.step,
                    playerid,
                    cancelable,
                    min,
                    max,
                    check_field,
                    to_check,
                    toplayer,
                    zone,
                    &mut s,
                );
                *state = s;
                done
            }
            Kind::RemoveCounter {
                reason,
                pcard,
                rplayer,
                self_side,
                oppo_side,
                counter_type,
                count,
            } => self.remove_counter_step(
                unit.step,
                reason,
                pcard,
                rplayer,
                self_side,
                oppo_side,
                counter_type,
                count,
            ),
            Kind::SelectCounter {
                playerid,
                counter_type,
                count,
                self_side,
                oppo_side,
            } => self.select_counter_step(
                unit.step,
                playerid,
                counter_type,
                count,
                self_side,
                oppo_side,
            ),
            Kind::TossCoin {
                reason_effect,
                reason_player,
                playerid,
                count,
            } => self.toss_coin_step(unit.step, reason_effect, reason_player, playerid, count),
            Kind::AnnounceRace {
                playerid,
                count,
                available,
            } => self.announce_race_step(unit.step, playerid, count, available),
            Kind::DiscardHand {
                playerid,
                min,
                max,
                reason,
            } => self.discard_hand_step(unit.step, playerid, min, max, reason),
            Kind::DiscardDeck {
                playerid,
                count,
                reason,
                ref mut discarded,
            } => {
                let mut d = std::mem::take(discarded);
                let done = self.discard_deck_step(unit.step, playerid, count, reason, &mut d);
                *discarded = d;
                done
            }
            Kind::SendTo {
                targets,
                reason_effect,
                reason,
                reason_player,
                ref mut state,
            } => {
                let mut s = std::mem::take(state);
                let done = self.send_to_step(
                    unit.step,
                    targets,
                    reason_effect,
                    reason,
                    reason_player,
                    &mut s,
                );
                *state = s;
                done
            }
            Kind::SendToReplace { targets, target } => self.send_to_replace_step(targets, target),
            Kind::SummonRule {
                sumplayer,
                target,
                ignore_count,
                ref mut min_tribute,
                ref mut zone,
                ref mut state,
            } => {
                let (mut mt, mut z) = (*min_tribute, *zone);
                let mut st = std::mem::take(state);
                let done = self.summon_rule_step(
                    unit.step,
                    sumplayer,
                    target,
                    ignore_count,
                    &mut mt,
                    &mut z,
                    &mut st,
                );
                *min_tribute = mt;
                *zone = z;
                *state = st;
                done
            }
            Kind::SpSummonRule {
                sumplayer,
                target,
                summon_type,
                ref mut state,
            } => {
                let mut st = std::mem::take(state);
                let done =
                    self.sp_summon_rule_step(unit.step, sumplayer, target, summon_type, &mut st);
                *state = st;
                done
            }
            Kind::SpSummon {
                reason_effect,
                reason_player,
                targets,
                zone,
            } => self.sp_summon_step(unit.step, reason_effect, reason_player, targets, zone),
            Kind::SpSummonStep {
                targets,
                target,
                zone,
                ref mut state,
            } => {
                let mut st = std::mem::take(state);
                let done = self.sp_summon_step_step(unit.step, targets, target, zone, &mut st);
                *state = st;
                done
            }
            Kind::FlipSummon {
                sumplayer,
                target,
                ref mut state,
            } => {
                let mut st = std::mem::take(state);
                let done = self.flip_summon_step(unit.step, sumplayer, target, &mut st);
                *state = st;
                done
            }
            Kind::SpellSet {
                setplayer,
                toplayer,
                target,
                reason_effect,
            } => self.spell_set_step(unit.step, setplayer, toplayer, target, reason_effect),
            Kind::MonsterSet {
                setplayer,
                target,
                ignore_count,
                ref mut min_tribute,
                ref mut zone,
                ref mut state,
            } => {
                let (mut mt, mut z) = (*min_tribute, *zone);
                let mut st = std::mem::take(state);
                let done = self.monster_set_step(
                    unit.step,
                    setplayer,
                    target,
                    ignore_count,
                    &mut mt,
                    &mut z,
                    &mut st,
                );
                *min_tribute = mt;
                *zone = z;
                *state = st;
                done
            }
            Kind::ChangePos {
                targets,
                reason_effect,
                reason_player,
                enable,
                ref mut state,
            } => {
                let mut st = std::mem::take(state);
                let done = self.change_pos_step(
                    unit.step,
                    targets,
                    reason_effect,
                    reason_player,
                    enable,
                    &mut st,
                );
                *state = st;
                done
            }
            Kind::MoveToField {
                target,
                enable,
                ret,
                pzone,
                zone,
                rule,
                location_reason,
                confirm,
            } => self.move_to_field_step(
                unit.step,
                target,
                enable,
                ret,
                pzone,
                zone,
                rule,
                location_reason,
                confirm,
            ),
            Kind::Destroy {
                targets,
                reason_effect,
                reason,
                reason_player,
            } => self.destroy_step(unit.step, targets, reason_effect, reason, reason_player),
            Kind::DestroyReplace {
                targets,
                target,
                battle,
            } => self.destroy_replace_step(targets, target, battle),
            Kind::Release {
                targets,
                reason_effect,
                reason,
                reason_player,
            } => self.release_step(unit.step, targets, reason_effect, reason, reason_player),
            Kind::ReleaseReplace { targets, target } => self.release_replace_step(targets, target),
            Kind::OperationReplace {
                replace_effect,
                targets,
                target,
                is_destroy,
            } => {
                self.operation_replace_step(unit.step, replace_effect, targets, target, is_destroy)
            }
            Kind::QuickEffect {
                skip_freechain,
                player,
                ref mut is_opponent,
            } => {
                let mut state = *is_opponent;
                let done = self.quick_effect_step(unit.step, skip_freechain, player, &mut state);
                *is_opponent = state;
                done
            }
            Kind::SelectYesNo {
                player,
                description,
            } => self.select_yes_no_step(unit.step, player, description),
            Kind::SelectCoin { player, count } => self.select_coin_step(unit.step, player, count),
            Kind::SelectDeckTop { player, count } => {
                self.select_deck_top_step(unit.step, player, count)
            }
            Kind::SelectRandom {
                player,
                count,
                ref cards,
            } => {
                let cards = cards.clone();
                self.select_random_step(unit.step, player, count, cards)
            }
            Kind::SelectEffectYesNo {
                player,
                description,
                card,
            } => self.select_effect_yes_no_step(unit.step, player, card, description),
            Kind::SelectIdleCmd { player } => self.select_idle_cmd_step(unit.step, player),
            Kind::SelectOption { player } => self.select_option_step(unit.step, player),
            Kind::SelectTribute {
                target,
                player,
                cancelable,
                ref mut min,
                ref mut max,
                toplayer,
                zone,
                ref mut state,
            } => {
                let (mut lo, mut hi) = (*min, *max);
                let mut st = std::mem::take(state);
                let done = self.select_tribute_step(
                    unit.step, target, player, cancelable, &mut lo, &mut hi, toplayer, zone,
                    &mut st,
                );
                *min = lo;
                *max = hi;
                *state = st;
                done
            }
            Kind::SelectTributeP {
                player,
                cancelable,
                ref mut min,
                ref mut max,
            } => {
                let (mut lo, mut hi) = (*min, *max);
                let done =
                    self.select_tribute_p_step(unit.step, player, cancelable, &mut lo, &mut hi);
                *min = lo;
                *max = hi;
                done
            }
            Kind::SelectUnselectCard {
                player,
                cancelable,
                min,
                max,
                finishable,
            } => {
                self.select_unselect_card_step(unit.step, player, cancelable, min, max, finishable)
            }
            Kind::SelectCard {
                player,
                cancelable,
                ref mut min,
                ref mut max,
            } => {
                let (mut lo, mut hi) = (*min, *max);
                let done = self.select_card_step(unit.step, player, cancelable, &mut lo, &mut hi);
                *min = lo;
                *max = hi;
                done
            }
            Kind::SelectPosition {
                player,
                code,
                positions,
            } => self.select_position_step(unit.step, player, code, positions),
            Kind::SelectPlace {
                player,
                flag,
                count,
                disable_field,
            } => self.select_place_step(unit.step, player, flag, count, disable_field),
            Kind::ExecuteCost {
                effect,
                player,
                subject,
                ref args,
                ref mut was_disabled,
            } => {
                let args = args.clone();
                self.execute_step(
                    Executing::Cost,
                    effect,
                    player,
                    subject,
                    &args,
                    was_disabled,
                )
            }
            Kind::ExecuteTarget {
                ref mut resume,
                effect,
                player,
                subject,
                ref args,
                ref mut was_disabled,
            } => {
                let args = args.clone();
                self.execute_step_resumable(
                    Executing::Target,
                    unit.step,
                    effect,
                    player,
                    subject,
                    &args,
                    was_disabled,
                    resume,
                )
            }
            Kind::ExecuteOperation {
                ref mut resume,
                effect,
                player,
                subject,
                ref args,
                ref mut was_disabled,
            } => {
                let args = args.clone();
                // The only executor whose function may suspend, so the only
                // one that needs its step and a place to park the rest.
                self.execute_step_resumable(
                    Executing::Operation,
                    unit.step,
                    effect,
                    player,
                    subject,
                    &args,
                    was_disabled,
                    resume,
                )
            }
            Kind::SelectChain {
                player,
                spe_count,
                forced,
            } => self.select_chain_step(unit.step, player, spe_count, forced),
            Kind::GetControl {
                reason_effect,
                chose_player,
                targets,
                playerid,
                reset_phase,
                reset_count,
                zone,
                ref mut state,
            } => {
                let mut st = std::mem::take(state);
                let done = self.get_control_step(
                    unit.step,
                    reason_effect,
                    chose_player,
                    targets,
                    playerid,
                    reset_phase,
                    reset_count,
                    zone,
                    &mut st,
                );
                *state = st;
                done
            }
            Kind::SwapControl {
                reason_effect,
                reason_player,
                targets1,
                targets2,
                reset_phase,
                reset_count,
                ref mut state,
            } => {
                let mut st = std::mem::take(state);
                let done = self.swap_control_step(
                    unit.step,
                    reason_effect,
                    reason_player,
                    targets1,
                    targets2,
                    reset_phase,
                    reset_count,
                    &mut st,
                );
                *state = st;
                done
            }
            Kind::TrapMonsterAdjust { ref mut state } => {
                let mut st = std::mem::take(state);
                let done = self.trap_monster_adjust_step(unit.step, &mut st);
                *state = st;
                done
            }
            Kind::PhaseEvent {
                phase,
                ref mut state,
            } => {
                let mut st = *state;
                let done = self.phase_event_step(unit.step, phase, &mut st);
                *state = st;
                done
            }
            Kind::RefreshLoc { ref mut state } => {
                let mut st = *state;
                let done = self.refresh_loc_step(unit.step, &mut st);
                *state = st;
                done
            }
            Kind::SelfDestroyUnique { card, player } => {
                self.self_destroy_unique_step(unit.step, card, player)
            }
            Kind::SelfDestroy => self.self_destroy_step(unit.step),
            Kind::SelfToGrave => self.self_to_grave_step(unit.step),
            Kind::Adjust => self.adjust_step(unit.step),
            Kind::SolveContinuous { ref mut state } => {
                let mut st = *state;
                let done = self.solve_continuous_step(unit.step, &mut st);
                *state = st;
                done
            }
            Kind::IdleCommand { ref mut state } => {
                let mut st = std::mem::take(state);
                let done = self.idle_command_step(unit.step, &mut st);
                *state = st;
                done
            }
            Kind::Startup => self.startup_step(unit.step),
            Kind::RefreshRelay => self.refresh_relay_step(unit.step),
            Kind::Turn {
                ref mut turn_player,
                ref mut state,
            } => {
                let mut tp = *turn_player;
                let mut st = std::mem::take(state);
                let done = self.turn_step(unit.step, &mut tp, &mut st);
                *turn_player = tp;
                *state = st;
                done
            }
            Kind::BattleCommand { ref mut state } => {
                let mut st = std::mem::take(state);
                let done = self.battle_command_step(unit.step, &mut st);
                *state = st;
                done
            }
            Kind::DamageStep { ref mut state } => {
                let mut st = std::mem::take(state);
                let done = self.damage_step_step(unit.step, &mut st);
                *state = st;
                done
            }
            Kind::Damage { ref mut arg } => {
                let mut a = (**arg).clone();
                let done = self.damage_step_unit(unit.step, &mut a);
                **arg = a;
                done
            }
            Kind::Recover { ref mut arg } => {
                let mut a = (**arg).clone();
                let done = self.recover_step(unit.step, &mut a);
                **arg = a;
                done
            }
            Kind::PayLPCost {
                playerid,
                ref mut cost,
            } => {
                let mut c = *cost;
                let done = self.pay_lp_cost_step(unit.step, playerid, &mut c);
                *cost = c;
                done
            }
            Kind::SortCard { player, is_chain } => self.sort_card_step(unit.step, player, is_chain),
            Kind::SortChain { player } => self.sort_chain_step(unit.step, player),
            Kind::SelectBattleCmd { player } => self.select_battle_cmd_step(unit.step, player),
            Kind::AttackDisable => self.attack_disable_step(unit.step),
            Kind::ForcedBattle { ref mut state } => {
                let mut st = std::mem::take(state);
                let done = self.forced_battle_step(unit.step, &mut st);
                *state = st;
                done
            }
            Kind::Loop { ref mut laps } => {
                if *laps == 0 {
                    true
                } else {
                    *laps -= 1;
                    unit.step = RESTART;
                    false
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unit runs once per `process` call until its handler retires it.
    #[test]
    fn a_unit_runs_one_step_per_call() {
        let mut f = Field::new(8000);
        f.push_back(Kind::Nop { steps: 3 });
        assert_eq!(f.process(), Status::Continue);
        assert_eq!(f.queue().next().map(|u| u.step), Some(1));
        assert_eq!(f.process(), Status::Continue);
        assert_eq!(f.queue().next().map(|u| u.step), Some(2));
        // The third call retires it, and the queue empties.
        assert_eq!(f.process(), Status::Continue);
        assert_eq!(f.process(), Status::End);
    }

    /// An empty queue is the end of the duel, not an error.
    #[test]
    fn an_empty_queue_ends() {
        assert_eq!(Field::new(8000).process(), Status::End);
    }

    /// Emplaced work jumps ahead of the unit that emplaced it, and keeps
    /// the order it was emplaced in. This is the mechanism behind every
    /// "do this, then continue" in the engine.
    #[test]
    fn emplaced_units_run_before_the_one_that_queued_them() {
        let mut f = Field::new(8000);
        f.push_back(Kind::Nop { steps: 9 });
        // Two emplaced units, neither of which retires on its first step.
        f.emplace(Kind::Nop { steps: 2 });
        f.emplace(Kind::Nop { steps: 3 });
        f.process();
        let steps: Vec<u16> = f
            .queue()
            .map(|u| match u.kind {
                Kind::Nop { steps } => steps,
                Kind::Loop { laps } => laps,
                _ => 0,
            })
            .collect();
        assert_eq!(
            steps,
            vec![2, 3, 9],
            "emplaced first, in the order emplaced, then the unit that queued them"
        );
        assert_eq!(
            f.queue().next().map(|u| u.step),
            Some(1),
            "the first one ran"
        );
    }

    /// `emplace_process<T>(Step{ n })`: a unit can be entered part-way.
    #[test]
    fn a_unit_can_be_entered_at_a_step() {
        let mut f = Field::new(8000);
        f.push_back(Kind::Nop { steps: 9 });
        // 40 steps, entered at 30: it must not retire on this dispatch.
        f.emplace_at(Kind::Nop { steps: 40 }, 30);
        f.process();
        assert_eq!(
            f.queue().next().map(|u| u.step),
            Some(31),
            "ran step 30, advanced"
        );
    }

    /// RESTART wraps to zero, which is how a unit loops inside a queue that
    /// has no loops.
    #[test]
    fn restart_wraps_to_the_top() {
        assert_eq!(RESTART.wrapping_add(1), 0);
        let mut f = Field::new(8000);
        f.push_back(Kind::Nop { steps: 9 });
        f.emplace_at(Kind::Nop { steps: 9 }, RESTART);
        f.process();
        assert_eq!(
            f.queue().next().map(|u| u.step),
            Some(0),
            "restart means step 0 next"
        );
    }
}

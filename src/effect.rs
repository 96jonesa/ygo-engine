//! Effects: what a card can do, and when.
//!
//! A translation of ocgcore's `effect`, with one deliberate and large
//! departure — described below, because it is the place where this port
//! stops being a literal translation and the reason is worth stating once.
//!
//! ## No Lua
//!
//! In ocgcore an effect's four behaviours — `condition`, `cost`, `target`,
//! `operation` — are integers: references into the Lua registry, resolved
//! by calling into the interpreter. This port has no interpreter, because a
//! solver cloning duel state millions of times cannot carry a script VM per
//! state.
//!
//! So those four become Rust function pointers. The effect *descriptor* —
//! its type, code, range, flags, counts — is translated field for field,
//! since that is what the machinery reads when deciding whether an effect
//! may activate. What changes is only how the behaviour is reached.
//!
//! The consequence to keep in view: each card's four functions are now
//! translations of that card's Lua, and translating them is where per-card
//! defects will enter. The machinery around them is a structural port; the
//! functions are not.

use crate::board::location;
use crate::card::Card;
use crate::event::{code, CardId, EffectId, Event, PLAYER_NONE};
use crate::field::Field;

/// What kind of effect this is. A mask, because an effect is often several
/// — a trigger effect that is also a field effect, say.
pub mod effect_type {
    pub const SINGLE: u16 = 0x0001;
    pub const FIELD: u16 = 0x0002;
    pub const EQUIP: u16 = 0x0004;
    pub const ACTIONS: u16 = 0x0008;
    pub const ACTIVATE: u16 = 0x0010;
    pub const FLIP: u16 = 0x0020;
    pub const IGNITION: u16 = 0x0040;
    /// Optional trigger — "you can".
    pub const TRIGGER_O: u16 = 0x0080;
    pub const QUICK_O: u16 = 0x0100;
    /// Mandatory trigger — the one a player may not decline, and so may not
    /// agree to end a phase while it is owed.
    pub const TRIGGER_F: u16 = 0x0200;
    pub const QUICK_F: u16 = 0x0400;
    pub const CONTINUOUS: u16 = 0x0800;
    pub const XMATERIAL: u16 = 0x1000;
    pub const GRANT: u16 = 0x2000;
    /// An effect one card applies to another specific card. Distinct from
    /// `FIELD`, which applies to whatever is in range.
    pub const TARGET: u16 = 0x4000;
}

/// `EFFECT_FLAG_*` — the first flag word. Transcribed whole, for the reason
/// recorded in `docs/processor-loop.md`.
///
/// The reference keeps two flag words and overloads `is_flag` on the enum
/// type to pick between them, which Rust will not do. They are two modules
/// here instead, and the two `is_flag` methods say which word they read.
pub mod flag {
    pub const INITIAL: u32 = 0x0001;
    pub const FUNC_VALUE: u32 = 0x0002;
    pub const COUNT_LIMIT: u32 = 0x0004;
    pub const FIELD_ONLY: u32 = 0x0008;
    pub const CARD_TARGET: u32 = 0x0010;
    pub const IGNORE_RANGE: u32 = 0x0020;
    pub const ABSOLUTE_TARGET: u32 = 0x0040;
    pub const IGNORE_IMMUNE: u32 = 0x0080;
    pub const SET_AVAILABLE: u32 = 0x0100;
    pub const CANNOT_NEGATE: u32 = 0x0200;
    pub const CANNOT_DISABLE: u32 = 0x0400;
    pub const PLAYER_TARGET: u32 = 0x0800;
    pub const BOTH_SIDE: u32 = 0x1000;
    pub const COPY_INHERIT: u32 = 0x2000;
    pub const DAMAGE_STEP: u32 = 0x4000;
    pub const DAMAGE_CAL: u32 = 0x8000;
    pub const DELAY: u32 = 0x10000;
    pub const SINGLE_RANGE: u32 = 0x20000;
    pub const UNCOPYABLE: u32 = 0x40000;
    pub const OATH: u32 = 0x80000;
    pub const SPSUM_PARAM: u32 = 0x100000;
    pub const REPEAT: u32 = 0x200000;
    pub const NO_TURN_RESET: u32 = 0x400000;
    /// The event's player activates the effect, rather than the handler's
    /// controller. The gather reads this at every site that builds a chain.
    pub const EVENT_PLAYER: u32 = 0x800000;
    pub const OWNER_RELATE: u32 = 0x1000000;
    pub const CANNOT_INACTIVATE: u32 = 0x2000000;
    pub const CLIENT_HINT: u32 = 0x4000000;
    pub const CONTINUOUS_TARGET: u32 = 0x8000000;
    pub const LIMIT_ZONE: u32 = 0x10000000;
    pub const IMMEDIATELY_APPLY: u32 = 0x80000000;
}

/// `EFFECT_FLAG2_*` — the second flag word.
pub mod flag2 {
    pub const CONTINUOUS_EQUIP: u32 = 0x0001;
    pub const COF: u32 = 0x0002;
    /// Gates a trigger on whether its handler left the field in the same
    /// batch as the event it is responding to.
    pub const CHECK_SIMULTANEOUS: u32 = 0x0004;
    pub const FORCE_ACTIVATE_LOCATION: u32 = 0x40000000;
    pub const MAJESTIC_MUST_COPY: u32 = 0x80000000;
}

/// What a card's behaviour is told about the question being asked.
///
/// The reference passes these as the first arguments to every Lua callback:
/// the effect asking, the player it is asked on behalf of, and the event
/// that prompted it. Keeping them in one struct rather than as loose
/// parameters is the one liberty taken, and it keeps the four signatures
/// aligned with each other.
pub struct Ctx<'a> {
    /// `reason_effect` — the effect the question is asked *for*. Usually the
    /// effect itself, but `is_activate_ready` takes a separate one, and
    /// several cards distinguish them.
    pub reason_effect: EffectId,
    /// The player the question is being asked on behalf of.
    pub player: u8,
    pub event: &'a Event,
    /// The card the question is about, when there is one.
    ///
    /// This is the slot the reference's `get_value` overloads differ by —
    /// `get_value(card*, ...)` pushes a card onto the Lua stack before the
    /// call. With one Rust signature there is no stack, so the extra
    /// argument becomes a field.
    pub card: Option<CardId>,
    /// The plain integers the reference pushes before a value call, in the
    /// order it pushes them.
    ///
    /// `get_value(3)` means "three arguments are already on the stack", and
    /// which three depends entirely on the call site — `get_mzone_limit`
    /// pushes `(playerid, uplayer, reason)`, `get_forced_zones` the same
    /// three, an activation's zone mask pushes seven fields of the event.
    /// Dropping them is invisible: the value function simply sees fewer
    /// arguments and a card that reads one of them silently misbehaves.
    pub args: &'a [i64],
}

/// May this effect activate at all, given the event?
/// A condition takes the field **mutably**.
///
/// Not because a condition should change anything — it should not — but
/// because almost every question worth asking about a board goes through
/// a scan, and the port's scans take `&mut Field` for their own
/// bookkeeping (memoised stats, re-entrancy guards). A `&Field`
/// condition can only ask the handful of questions that happen to be
/// immutable, which is an arbitrary line: Sinister Serpent's condition
/// is `IsTurnPlayer(1-tp) and IsExistingMatchingCard(...)`, and the
/// second half is unaskable without this.
pub type Condition = fn(&mut Field, &Ctx) -> bool;

/// Pay the activation cost.
///
/// `chk` is the reference's trailing argument, and it is not decoration:
/// `chk == false` asks "could this be paid?" without paying, and is what
/// the activation-legality checks call. `chk == true` pays. A port that
/// dropped it would charge the cost every time the engine merely asked
/// whether a card was activatable.
pub type Cost = fn(&mut Field, &Ctx, chk: bool) -> bool;

/// Choose targets and declare what the effect will do. `chk` as for [`Cost`].
pub type Target = fn(&mut Field, &Ctx, chk: bool, chkc: Option<CardId>) -> Yield;

/// Resolve, and **yield a value**.
///
/// The return is not decoration: ocgcore's operation is a Lua function whose
/// return value lands in `returns`, and `RefreshLoc` is the first machine in
/// this port to read one — it asks an `EFFECT_DISABLE_FIELD` effect which
/// seats it blocks and takes the answer from there. Most operations answer
/// nothing and return zero, which is what the interpreter leaves behind for a
/// Lua function with no return.
pub type Operation = fn(&mut Field, &Ctx) -> Yield;

/// What a card's operation reports back — the port's stand-in for
/// `call_coroutine`'s `COROUTINE_YIELD`.
///
/// In ocgcore an operation is a Lua coroutine, and `ExecuteOperation`
/// calls it once per step: a yield returns `FALSE` so the unit is
/// re-entered, and anything the yielding export queued runs in between.
/// This is the same protocol with the coroutine replaced by a closure.
#[derive(Debug)]
pub enum Yield {
    /// Finished. The value lands in `returns[0]`, as a Lua function's
    /// return does.
    Done(i32),
    /// Suspended. Whatever the operation queued runs, and then this is
    /// called again.
    Suspended(Suspended),
}

impl Yield {
    /// The value a finished operation reported, or `None` if it suspended.
    /// Tests assert on this rather than on the enum, which cannot be
    /// compared because a boxed closure cannot.
    pub fn finished(&self) -> Option<i32> {
        match self {
            Yield::Done(v) => Some(*v),
            Yield::Suspended(_) => None,
        }
    }
}

/// A suspended operation: the rest of the card's work, waiting to be run
/// once the processors it queued have finished.
///
/// A boxed `FnMut` rather than a function pointer plus a state blob,
/// because the closure captures the card's locals **by type**. Forgetting
/// to carry one does not compile, where a positional state blob would
/// have moved the Lua's locals into an array of integers and reintroduced
/// exactly the off-by-one hazard the step machines carry.
///
/// The field is taken as an argument rather than captured, so nothing
/// holds a borrow across the suspension.
pub struct Suspended(pub Box<dyn Rest>);

/// The rest of a suspended operation: a closure over what it captured. It
/// must be **cloneable**, so a `Field` mid-operation can be copied — the
/// captures are card ids, players and small values, which clone freely; a
/// closure capturing anything else fails to compile at `api::suspend`,
/// which is the point. And **`Send`**, so a `Field` can be handed to
/// another thread: a search's playouts are independent, and the captures
/// that clone freely cross threads freely too. (`Sync` is not asked: the
/// effect ids are `Cell`s, and a thread owns its field outright.)
pub trait Rest: for<'a> FnMut(&mut Field, &Ctx<'a>) -> Yield + Send {
    fn clone_box(&self) -> Box<dyn Rest>;
}

impl<F> Rest for F
where
    F: for<'a> FnMut(&mut Field, &Ctx<'a>) -> Yield + Clone + Send + 'static,
{
    fn clone_box(&self) -> Box<dyn Rest> {
        Box::new(self.clone())
    }
}

impl Clone for Suspended {
    fn clone(&self) -> Self {
        Suspended(self.0.clone_box())
    }
}

impl std::fmt::Debug for Suspended {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Suspended(..)")
    }
}

/// The *condition* use of `operation`.
///
/// A third distinct use of a callback slot, alongside `target`/`target_filter`.
/// `card::get_type` runs an effect's `operation` reference through
/// `check_condition`, passing the summoning card, the summon type and the
/// player — asking "does this type change apply to this summon?" rather than
/// performing anything. `Operation` returns nothing and cannot answer that,
/// so it is a separate slot, for the same reason `target_filter` is.
pub type OperationFilter =
    fn(&Field, EffectId, scard: Option<CardId>, sumtype: u64, playerid: u8) -> bool;

/// The filter use of `target`: does this effect apply to that card?
///
/// `args` carries the integers the reference pushes after the effect and the
/// card, and it is **not** decoration. `is_player_can_summon` pushes the
/// player, the summon type, the position and the destination player;
/// `is_player_can_remove` pushes a reason. A prohibition whose condition
/// asks about the summon *type* — "you may not Special Summon" — cannot be
/// answered without them, and a port that dropped them would silently make
/// every such prohibition unconditional.
///
/// The card is optional because the reference passes a null for the
/// player-wide questions that are not about a particular card.
pub type TargetFilter = fn(&Field, EffectId, Option<CardId>, args: &[i64]) -> bool;

/// An effect's `value`, when it is computed rather than constant.
/// What an effect's `label_object` slot holds.
///
/// Deliberately *not* a card id that an effect id could be mistaken for:
/// both are `usize` indices into different arenas, so an untyped slot
/// would accept either and read back the wrong arena without complaint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LabelObject {
    Card(crate::event::CardId),
    Effect(crate::event::EffectId),
    /// A **group**, for a script that works out a set in one function and
    /// reads it back in another — Chaos Sorcerer's `sptg` chooses the two
    /// monsters to banish and its `spop` banishes them.
    ///
    /// The reference stores a Lua `group*`, which the script keeps alive
    /// with `KeepAlive` and releases with `DeleteGroup`. Here it is a
    /// handle into `Field::groups`, which is append-only and never freed,
    /// so both of those calls have nothing to do.
    Group(crate::field::GroupId),
}

impl LabelObject {
    /// The card, when that is what is stored.
    pub fn card(self) -> Option<crate::event::CardId> {
        match self {
            LabelObject::Card(c) => Some(c),
            LabelObject::Effect(_) | LabelObject::Group(_) => None,
        }
    }

    /// The effect, when that is what is stored.
    pub fn effect(self) -> Option<crate::event::EffectId> {
        match self {
            LabelObject::Effect(e) => Some(e),
            LabelObject::Card(_) | LabelObject::Group(_) => None,
        }
    }

    /// The group, when that is what is stored.
    pub fn group(self) -> Option<crate::field::GroupId> {
        match self {
            LabelObject::Group(g) => Some(g),
            LabelObject::Card(_) | LabelObject::Effect(_) => None,
        }
    }
}

/// A **procedure's** arguments, kept on the effect it built.
///
/// In the reference a procedure like `aux.AddEquipProcedure` returns Lua
/// **closures** over its arguments and installs those as the effect's
/// target and operation. The port's seams are plain function pointers
/// with nothing to close over, so the arguments live here instead —
/// which is the same place, since a Lua closure is reachable only
/// through the effect too.
///
/// Per-`Field` by construction, because effects are. A side table keyed
/// by effect id would collide between two duels running at once.
/// A procedure's `f`, with the two extra arguments the reference passes
/// alongside the card.
pub type AuxFilter = fn(&mut Field, crate::event::CardId, crate::event::EffectId, u8) -> bool;

/// A procedure's `tg`, run once a target is chosen.
pub type AuxAfterTarget = fn(&mut Field, &Ctx, crate::event::CardId);

// No `PartialEq`: the fields are function pointers, whose addresses
// carry no meaning to compare.
#[derive(Clone, Copy, Debug, Default)]
pub struct AuxArgs {
    /// Whose cards the procedure concerns, as the reference's `p`:
    /// `0`, `1`, or `PLAYER_ALL`.
    pub side: u8,
    /// Which cards the procedure accepts.
    pub filter: Option<AuxFilter>,
    /// What runs once a target is chosen.
    pub after_target: Option<AuxAfterTarget>,
}

pub type ValueFn = fn(&Effect, &Field, &Ctx) -> i64;

/// An effect's `value` when it returns **several** numbers.
///
/// The reference's `get_value(card*, extraargs, vector&)` overload: a Lua
/// function may return more than one value, and a handful of effects use
/// that to carry a small tuple. `EFFECT_EXTRA_SUMMON_COUNT` returns
/// `(min_tribute, zone, releasable)`.
///
/// A separate slot for the same reason `value_fn` is separate from `value`
/// and `target_filter` from `target`: one Lua reference is read at several
/// arities, and Rust function pointers are not.
///
/// **A constant value yields a one-element list**, which is why the callers
/// can read "absent" and "zero" apart — `retval.size() > 1` is a real test
/// in the reference and means "did the card bother to say".
pub type ValueListFn = fn(&Field, &Ctx) -> Vec<i64>;

/// An effect as the machinery sees it.
///
/// The descriptor half is translated field for field from the reference,
/// because these are exactly the fields the chain machinery reads when it
/// decides whether an effect may be activated, by whom, and from where.
#[derive(Clone)]
pub struct Effect {
    /// An `effect_type::*` mask.
    pub effect_type: u16,
    /// What this effect *is* — `EFFECT_UPDATE_ATTACK` and the like — or, for
    /// an activated effect, the event it responds to.
    pub code: u32,
    /// The locations the effect works from.
    pub range: u16,
    /// The locations it may target, split by side.
    pub s_range: u16,
    pub o_range: u16,
    /// `EFFECT_FLAG_*` masks. Two words in the reference, and kept as two.
    pub flag: [u32; 2],
    /// The card this effect belongs to.
    pub owner: Option<CardId>,
    /// The card it is currently *on*, which differs from the owner for an
    /// equip or a granted effect.
    pub handler: Option<CardId>,
    /// Set while the effect is on a chain, to the card the handler is an
    /// Xyz material of. `get_real_handler` prefers it over both `owner` and
    /// `handler`, which is how an Xyz material's effect keeps pointing at
    /// the monster it is under while it resolves.
    pub active_handler: Option<CardId>,
    /// Registration order, and the key every effect sort uses.
    ///
    /// `id` changes when an effect is re-registered; `initial_id` is stamped
    /// once and never moves. The reference sorts by `id` almost everywhere
    /// and by `initial_id` only for granted effects, and the two orders
    /// differ exactly when something has been re-registered — so they are
    /// two fields here as well.
    ///
    /// A `Cell`, because the reference renumbers an effect from inside
    /// `effect::is_available` — a query, called from every `&self` effect
    /// walk — when its condition turns true (`effect.cpp`, the
    /// `EFFECT_STATUS_AVAILABLE` lines). See [`Effect::available`].
    pub id: std::cell::Cell<u32>,
    pub initial_id: u32,
    /// The reference's `EFFECT_STATUS_AVAILABLE` bit of `effect::status`,
    /// kept apart from [`Effect::status`] because it is written from a
    /// `&self` query: set when the condition passes, cleared when it fails,
    /// and **untouched** by the gates before the condition (out of range,
    /// disabled, not yet enabled), which return early. On a false → true
    /// transition the effect takes a fresh id, so it sorts after
    /// everything that was in force before it.
    pub available: std::cell::Cell<bool>,
    /// What the handling card counted as when this effect was put on a
    /// chain. Snapshotted because the card may change type mid-chain.
    pub card_type: u32,
    /// The message id the client shows for this effect.
    pub description: u64,
    /// `category` — a `CATEGORY_*` mask the card declares. Read by the
    /// client and by `SetOperationInfo`'s callers; nothing in the core
    /// branches on it, but every activation names one.
    pub category: u64,
    /// Non-zero when this effect was *copied* onto its card by
    /// `copy_effect` rather than printed on it. `card::remove_effect` reads
    /// it together with `EFFECT_FLAG_INITIAL` and `STATUS_EFFECT_REPLACED`
    /// to decide whether removing the effect means the card's printed
    /// effects have to be re-registered.
    pub copy_id: u16,
    /// When this effect is taken away: a `reset::*` mask.
    pub reset_flag: u32,
    /// An `effect_status::*` mask. Distinct from the card's status.
    pub status: u32,
    /// The timings at which a client should highlight this effect, as
    /// `[own, opponent's]`. A hint, not a rule — but the reference's hint.
    pub hint_timing: [u32; 2],
    /// Whose effect it counts as.
    pub effect_owner: u8,
    /// Activation counts, for once-per-turn and its relatives.
    ///
    /// `count_limit` is what remains; `count_limit_max` is the ceiling the
    /// duel-wide tally is compared against. `count_code`, `count_flag` and
    /// `count_hopt_index` name *which* tally — a "hard once per turn" is
    /// shared between every copy of a card, so the count cannot live on the
    /// effect.
    pub count_limit: u8,
    pub count_limit_max: u8,
    /// How many phase resets this effect survives. `RESET_PHASE` says *when*
    /// it resets; this says how many of those it takes.
    pub reset_count: u8,
    pub count_code: u32,
    /// An `effect_count::*` mask, choosing which tally and at what scope.
    pub count_flag: u8,
    pub count_hopt_index: u8,

    /// Where the handler was when this effect was activated. Snapshotted by
    /// `set_activate_location`, because the handler moves.
    pub active_location: u8,
    pub active_sequence: u32,
    /// What the handler counted as when activated; 0 until set.
    pub active_type: u32,

    /// A constant, or — when `EFFECT_FLAG_FUNC_VALUE` is set — ignored in
    /// favour of `value_fn`.
    /// `SetLabel` — scratch integers an effect carries for its own
    /// functions to read back, and nothing else looks at.
    ///
    /// A **vector**, as the reference's is: `SetLabel` takes a table or a
    /// stack of values and `GetLabel` pushes them all back. Most cards
    /// store one, and `GetLabel` on an empty label reads **zero** rather
    /// than failing, which is the arm a card relies on when it never set
    /// one.
    /// `effect::is_available`'s condition — the reference's **same**
    /// `condition` field, in its other calling convention.
    ///
    /// ocgcore stores one function and calls it two ways: an activation
    /// condition gets the eight event parameters
    /// (`field::is_condition_check`), while a *continuous* effect's is
    /// called with exactly one — the effect itself — at the end of
    /// `effect::is_available` (`effect.cpp`, `check_condition(condition,
    /// 1)`). The questions differ: one asks "may this be activated now",
    /// the other "is this in force at all", asked at arbitrary moments
    /// with no event in hand.
    ///
    /// The port splits them because the two call sites disagree about
    /// borrowing. An activation condition runs from a `&mut Field` and
    /// several pool cards need that. `is_available` is `&self` and is
    /// called from inside predicates that already hold a borrow — Sangan's
    /// `ValueFn` among them — so making it mutable is the crate-wide
    /// widening `ValueFn` and `TargetFilter` are already waiting on, not
    /// a change a card may make in passing.
    ///
    /// **One field in the reference, two here**, and this is the
    /// read-only one. See `docs/processor-loop.md`, "Reaper on the
    /// Nightmare".
    pub avail_condition: Option<fn(&crate::field::Field, crate::event::EffectId) -> bool>,
    pub label: Vec<i64>,
    /// `label_object` — something an effect wants to remember.
    ///
    /// In the reference this is a Lua registry reference and can hold any
    /// object. This pool stores a **card** (Premature Burial's equip
    /// limit, naming what it revived) or an **effect** (Call of the
    /// Haunted's `e3`, naming the `e2` whose label it reads). A later
    /// pool that stores a group widens this again rather than reaching
    /// for a registry.
    pub label_object: Option<LabelObject>,
    pub value: i64,
    pub value_fn: Option<ValueFn>,
    /// What a procedure closed over; see [`AuxArgs`].
    pub aux: Option<AuxArgs>,
    pub value_list_fn: Option<ValueListFn>,

    /// The behaviour. `None` means "always", which is how the reference
    /// treats a null Lua reference.
    pub condition: Option<Condition>,
    /// The *filter* use of `target`: "does this effect apply to that card?"
    ///
    /// A separate slot from `target`, because the reference calls the same
    /// Lua reference with two different arities — nine arguments when
    /// declaring what an activation will do, two when filtering a card. One
    /// Rust signature cannot be both, and collapsing them would silently
    /// pick one meaning for both uses.
    pub target_filter: Option<TargetFilter>,
    /// The condition use of `operation` — see [`OperationFilter`].
    pub operation_filter: Option<OperationFilter>,
    pub cost: Option<Cost>,
    pub target: Option<Target>,
    pub operation: Option<Operation>,
}

impl Default for Effect {
    fn default() -> Self {
        Self {
            effect_type: 0,
            code: 0,
            range: 0,
            s_range: 0,
            o_range: 0,
            flag: [0, 0],
            id: std::cell::Cell::new(0),
            initial_id: 0,
            available: std::cell::Cell::new(false),
            card_type: 0,
            description: 0,
            category: 0,
            copy_id: 0,
            reset_flag: 0,
            label_object: None,
            status: 0,
            hint_timing: [0, 0],
            owner: None,
            handler: None,
            active_handler: None,
            effect_owner: PLAYER_NONE,
            count_limit: 0,
            avail_condition: None,
            label: Vec::new(),
            count_limit_max: 0,
            reset_count: 0,
            count_code: 0,
            count_flag: 0,
            count_hopt_index: 0,
            active_location: 0,
            active_sequence: 0,
            active_type: 0,
            value: 0,
            value_fn: None,
            aux: None,
            value_list_fn: None,
            condition: None,
            target_filter: None,
            operation_filter: None,
            cost: None,
            target: None,
            operation: None,
        }
    }
}

impl Effect {
    pub fn new(effect_type: u16, code: u32) -> Self {
        Self {
            effect_type,
            code,
            ..Default::default()
        }
    }

    pub fn is_type(&self, mask: u16) -> bool {
        self.effect_type & mask != 0
    }

    /// `is_flag(effect_flag)` — the first flag word.
    pub fn is_flag(&self, mask: u32) -> bool {
        self.flag[0] & mask != 0
    }

    /// `is_flag(effect_flag2)` — the second.
    pub fn is_flag2(&self, mask: u32) -> bool {
        self.flag[1] & mask != 0
    }

    /// A trigger the controller may not decline. The chain machinery asks
    /// this to decide whether a player may agree to end the phase.
    pub fn is_mandatory_trigger(&self) -> bool {
        self.is_type(effect_type::TRIGGER_F)
    }

    /// `has_count_limit`.
    pub fn has_count_limit(&self) -> bool {
        self.is_flag(flag::COUNT_LIMIT)
    }

    /// `has_function_value` — is `value` computed rather than constant?
    pub fn has_function_value(&self) -> bool {
        self.is_flag(flag::FUNC_VALUE)
    }

    /// `get_value` — the constant, or the function's result.
    ///
    /// The reference has six overloads differing only in which extra
    /// arguments they push onto the Lua stack before the call. With Rust
    /// functions there is no stack to push onto, so there is one.
    pub fn get_value(&self, field: &Field, ctx: &Ctx) -> i64 {
        match (self.has_function_value(), self.value_fn) {
            (true, Some(f)) => f(self, field, ctx),
            _ => self.value,
        }
    }

    /// `get_value(card, extraargs, vector&)` — the value as a list.
    ///
    /// A constant yields one element, matching the reference, so a caller
    /// testing `len() > 1` learns whether the card actually supplied a
    /// second number rather than whether it happened to be zero.
    pub fn get_value_list(&self, field: &Field, ctx: &Ctx) -> Vec<i64> {
        match (self.has_function_value(), self.value_list_fn) {
            (true, Some(f)) => f(field, ctx),
            // A function with only the single-value slot filled still
            // answers, with one element.
            (true, None) => vec![self.get_value(field, ctx)],
            _ => vec![self.value],
        }
    }

    /// `check_value_condition` — read `value` as a yes/no rather than a
    /// number.
    ///
    /// The same field, asked a different question: a constant is truthy when
    /// non-zero, and a function value is its result. The reference has this
    /// as a separate method from `get_value` for the same reason it is one
    /// here — the call sites mean different things by it.
    pub fn check_value_condition(&self, field: &Field, ctx: &Ctx) -> bool {
        self.get_value(field, ctx) != 0
    }

    /// `set_activate_location` — snapshot where the handler was.
    pub fn set_activate_location(&mut self, snap: &crate::card::CardState) {
        self.active_location = snap.loc.location;
        self.active_sequence = snap.loc.sequence;
    }

    /// `get_real_handler` — the card this effect should be read as being
    /// on, when that is not simply `handler`.
    ///
    /// `active_handler` wins; failing that, an Xyz material's effect points
    /// at the monster it is under. `pre_overlay_target` is the fallback for
    /// a material that has just been detached, so an effect resolving in
    /// that instant still knows where it came from.
    pub fn real_handler(&self, cards: &[Card]) -> Option<CardId> {
        if self.active_handler.is_some() {
            return self.active_handler;
        }
        if self.is_type(effect_type::XMATERIAL) {
            let handler = self.handler?;
            return cards[handler]
                .overlay_target
                .or(cards[handler].pre_overlay_target);
        }
        None
    }

    /// `get_handler` — the card the effect acts from.
    pub fn get_handler(&self, cards: &[Card]) -> Option<CardId> {
        self.real_handler(cards).or(self.handler)
    }

    /// `get_owner` — the card the effect belongs to.
    pub fn get_owner(&self, cards: &[Card]) -> Option<CardId> {
        self.real_handler(cards).or(self.owner)
    }

    /// `get_handler_player` — whose effect this is for range purposes.
    ///
    /// A field-only effect has no handler on the board, so it falls back to
    /// `effect_owner` rather than dereferencing one.
    pub fn get_handler_player(&self, cards: &[Card]) -> u8 {
        if self.is_flag(flag::FIELD_ONLY) {
            return self.effect_owner;
        }
        self.get_handler(cards)
            .map_or(PLAYER_NONE, |h| cards[h].current.controller)
    }

    /// `get_owner_player`.
    pub fn get_owner_player(&self, cards: &[Card]) -> u8 {
        if self.effect_owner != PLAYER_NONE {
            return self.effect_owner;
        }
        self.get_owner(cards)
            .map_or(PLAYER_NONE, |o| cards[o].current.controller)
    }

    /// `is_can_be_forbidden` — may this effect be shut off by a card being
    /// forbidden? A few kinds never can.
    pub fn is_can_be_forbidden(&self) -> bool {
        if self.is_flag(flag::CANNOT_DISABLE) && !self.is_flag(flag::CANNOT_NEGATE) {
            return false;
        }
        let counter = self.code & 0xf0000;
        !(self.code == code::CHANGE_CODE
            || counter == code::COUNTER_PERMIT
            || counter == code::COUNTER_LIMIT)
    }

    /// `is_target_player` — does this effect apply to this player?
    ///
    /// Only ever true for a player-target effect; an effect that targets
    /// cards answers no, whatever the ranges say.
    pub fn is_target_player(&self, cards: &[Card], playerid: u8) -> bool {
        if !self.is_flag(flag::PLAYER_TARGET) {
            return false;
        }
        if self.is_flag(flag::ABSOLUTE_TARGET) {
            (self.s_range != 0 && playerid == 0) || (self.o_range != 0 && playerid == 1)
        } else {
            let self_player = self.get_handler_player(cards);
            (self.s_range != 0 && self_player == playerid)
                || (self.o_range != 0 && self_player != playerid)
        }
    }

    /// `in_range` — is the card somewhere this effect works from?
    ///
    /// The Main and Extra Monster Zone bits in `range` are collapsed back to
    /// plain `MZONE` first. That looks redundant next to
    /// [`Self::is_in_range_of_symbolic_mzone`] and is not: this asks the
    /// broad question, and that one asks the narrow one. An effect ranged to
    /// the Main Monster Zones is in range of *a* monster zone here, and is
    /// separately rejected there if the card sits in an Extra one.
    ///
    /// An Xyz material's effect ignores `range` entirely: it is in range
    /// exactly when its handler is under something.
    pub fn in_range(&self, cards: &[Card], card: &Card) -> bool {
        if self.is_type(effect_type::XMATERIAL) {
            return self
                .handler
                .is_some_and(|h| cards[h].overlay_target.is_some());
        }
        let mut real_range = self.range;
        if self.range & (location::MMZONE | location::EMZONE) != 0 {
            real_range &= !(location::MMZONE | location::EMZONE);
            real_range |= u16::from(location::MZONE);
        }
        card.current.is_location(real_range)
    }

    /// `is_in_range_of_symbolic_mzone` — when an effect names a *specific*
    /// kind of monster zone, is the card in that kind?
    ///
    /// Vacuously true for an effect that names neither, which is most of
    /// them.
    pub fn is_in_range_of_symbolic_mzone(&self, card: &Card) -> bool {
        if self.range & (location::MMZONE | location::EMZONE) == 0 {
            return true;
        }
        !card.current.is_location(u16::from(location::MZONE))
            || card.current.is_location(self.range)
    }
}

/// `EFFECT_COUNT_CODE_*` — which tally an activation count is kept in, and
/// at what scope.
pub mod effect_count {
    /// The count is refunded if the activation is negated.
    pub const OATH: u8 = 0x1;
    /// Once per duel rather than once per turn.
    pub const DUEL: u8 = 0x2;
    /// Counted per *card* rather than per card name — which is the
    /// difference between "once per turn" and "hard once per turn".
    pub const SINGLE: u8 = 0x4;
    /// Once per chain.
    pub const CHAIN: u8 = 0x8;
}

/// The effect pool: every effect the duel has created, addressed by id.
///
/// This is the arena `EffectId` indexes into — the analogue of the
/// reference's `pduel->new_effect()` pool, not of its `field::effects`.
/// Those are two different things and conflating them would be a mistake:
/// the pool holds every effect, while `field::effects` is a set of
/// *registration indices* — eight `std::multimap<uint32_t, effect*>`s, one
/// per effect type, keyed by event code — that the gather searches.
///
/// Those indices arrive with the machinery that searches them, which is
/// what says how they must be shaped.
#[derive(Clone, Default)]
pub struct Effects {
    entries: Vec<Effect>,
}

impl Effects {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, effect: Effect) -> EffectId {
        self.entries.push(effect);
        self.entries.len() - 1
    }

    pub fn get(&self, id: EffectId) -> Option<&Effect> {
        self.entries.get(id)
    }

    pub fn get_mut(&mut self, id: EffectId) -> Option<&mut Effect> {
        self.entries.get_mut(id)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Every effect in the pool with this code and this effect owner.
    ///
    /// A plain scan of the pool, and deliberately not named after the
    /// reference's `filter_player_effect` — that one searches the
    /// `aura_effect` registration index and filters on `is_target_player`
    /// and `is_available`, neither of which has its substrate yet. Giving
    /// this the reference's name would claim a fidelity it does not have.
    pub fn with_code_owned_by(
        &self,
        player: u8,
        code: u32,
    ) -> impl Iterator<Item = (EffectId, &Effect)> {
        self.entries
            .iter()
            .enumerate()
            .filter(move |(_, e)| e.code == code && e.effect_owner == player)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two flag words are separate, and a mask from one must not be
    /// read against the other. The reference gets this from overloading on
    /// two enum types; here it is two methods, so the test is worth having.
    #[test]
    fn the_two_flag_words_are_read_separately() {
        let mut e = Effect::new(effect_type::TRIGGER_O, 0);
        e.flag[0] = flag::EVENT_PLAYER;
        e.flag[1] = flag2::CHECK_SIMULTANEOUS;
        assert!(e.is_flag(flag::EVENT_PLAYER));
        assert!(e.is_flag2(flag2::CHECK_SIMULTANEOUS));
        assert!(!e.is_flag(flag2::CHECK_SIMULTANEOUS), "a flag2 mask");
        assert!(!e.is_flag2(flag::EVENT_PLAYER), "a flag mask");
    }

    /// Pinned literals, so a value that drifts from the reference fails here
    /// rather than as an effect that mysteriously never triggers.
    #[test]
    fn flag_values_are_the_references() {
        assert_eq!(flag::FIELD_ONLY, 0x0008);
        assert_eq!(flag::DELAY, 0x10000);
        assert_eq!(flag::EVENT_PLAYER, 0x800000);
        assert_eq!(flag::IMMEDIATELY_APPLY, 0x80000000);
        assert_eq!(flag2::CHECK_SIMULTANEOUS, 0x0004);
    }

    #[test]
    fn effect_types_are_the_references_flags() {
        assert_eq!(effect_type::ACTIVATE, 0x0010);
        assert_eq!(effect_type::TRIGGER_O, 0x0080);
        assert_eq!(effect_type::TRIGGER_F, 0x0200);
        assert_eq!(effect_type::CONTINUOUS, 0x0800);
    }

    /// An effect is often several types at once, so the test is a mask test.
    #[test]
    fn an_effect_can_be_several_types() {
        let e = Effect::new(effect_type::FIELD | effect_type::TRIGGER_F, 0);
        assert!(e.is_type(effect_type::FIELD));
        assert!(e.is_type(effect_type::TRIGGER_F));
        assert!(!e.is_type(effect_type::ACTIVATE));
        assert!(e.is_mandatory_trigger(), "TRIGGER_F may not be declined");
    }

    mod range {
        use super::*;
        use crate::board::location;
        use crate::card::Card;

        fn card_at(loc: u8, sequence: u32) -> Card {
            let mut c = Card::new(18036057, 0);
            c.current.location = loc;
            c.current.sequence = sequence;
            c
        }

        /// `in_range` asks the broad question: the Main and Extra Monster
        /// Zone bits collapse back to plain MZONE first, so an effect ranged
        /// to either is in range of any monster zone.
        #[test]
        fn the_broad_range_collapses_the_monster_zone_kinds() {
            let mut e = Effect::new(effect_type::FIELD, 0);
            e.range = location::MMZONE;
            assert!(e.in_range(&[], &card_at(location::MZONE, 1)));
            assert!(
                e.in_range(&[], &card_at(location::MZONE, 6)),
                "an Extra Monster Zone is still a monster zone, broadly"
            );
            assert!(!e.in_range(&[], &card_at(location::GRAVE, 0)));
        }

        /// An ordinary range needs no collapsing.
        #[test]
        fn an_ordinary_range_is_a_plain_mask_test() {
            let mut e = Effect::new(effect_type::FIELD, 0);
            e.range = u16::from(location::GRAVE | location::HAND);
            assert!(e.in_range(&[], &card_at(location::GRAVE, 0)));
            assert!(e.in_range(&[], &card_at(location::HAND, 0)));
            assert!(!e.in_range(&[], &card_at(location::MZONE, 0)));
        }

        /// `is_in_range_of_symbolic_mzone` asks the narrow question, and is
        /// what separates the two. An effect ranged to the Main Monster
        /// Zones is *not* in narrow range of a card in an Extra one.
        #[test]
        fn the_narrow_range_distinguishes_the_monster_zone_kinds() {
            let mut e = Effect::new(effect_type::FIELD, 0);
            e.range = location::MMZONE;
            assert!(e.is_in_range_of_symbolic_mzone(&card_at(location::MZONE, 1)));
            assert!(!e.is_in_range_of_symbolic_mzone(&card_at(location::MZONE, 6)));
        }

        /// Vacuously true for an effect that names neither kind, which is
        /// most of them — so the narrow test never rejects on its own.
        #[test]
        fn the_narrow_range_is_vacuous_without_a_symbolic_range() {
            let mut e = Effect::new(effect_type::FIELD, 0);
            e.range = u16::from(location::MZONE);
            assert!(e.is_in_range_of_symbolic_mzone(&card_at(location::MZONE, 6)));
            assert!(e.is_in_range_of_symbolic_mzone(&card_at(location::GRAVE, 0)));
        }

        /// A card that is not in a monster zone at all passes the narrow
        /// test, whatever the effect names — it is about *which* monster
        /// zone, not about being in one.
        #[test]
        fn a_card_outside_the_monster_zones_passes_the_narrow_test() {
            let mut e = Effect::new(effect_type::FIELD, 0);
            e.range = location::EMZONE;
            assert!(e.is_in_range_of_symbolic_mzone(&card_at(location::GRAVE, 0)));
        }

        /// `set_activate_location` snapshots, because the handler moves.
        #[test]
        fn the_activation_location_is_snapshotted() {
            let mut e = Effect::new(effect_type::ACTIVATE, 0);
            let mut c = card_at(location::SZONE, 3);
            e.set_activate_location(&c.state());
            c.current.location = location::GRAVE;
            c.current.sequence = 0;
            assert_eq!(e.active_location, location::SZONE);
            assert_eq!(e.active_sequence, 3);
        }
    }

    mod value {
        use super::*;
        use crate::event::code;
        use crate::field::Field;

        /// Without `EFFECT_FLAG_FUNC_VALUE` the constant is the value, and
        /// a function that happens to be set is ignored — the flag, not the
        /// presence of the function, is what the reference branches on.
        #[test]
        fn the_flag_decides_whether_the_function_is_used() {
            fn forty_two(_: &Effect, _: &Field, _: &Ctx) -> i64 {
                42
            }
            let f = Field::new(8000);
            let ev = Event::new(code::CHAINING);
            let ctx = Ctx {
                reason_effect: 0,
                player: 0,
                event: &ev,
                card: None,
                args: &[],
            };

            let mut e = Effect::new(effect_type::FIELD, 0);
            e.value = 7;
            e.value_fn = Some(forty_two);
            assert_eq!(e.get_value(&f, &ctx), 7, "the flag is not set");

            e.flag[0] |= flag::FUNC_VALUE;
            assert_eq!(e.get_value(&f, &ctx), 42);
        }

        /// The flag set with no function falls back to the constant rather
        /// than panicking.
        #[test]
        fn a_missing_function_falls_back_to_the_constant() {
            let f = Field::new(8000);
            let ev = Event::new(code::CHAINING);
            let ctx = Ctx {
                reason_effect: 0,
                player: 0,
                event: &ev,
                card: None,
                args: &[],
            };
            let mut e = Effect::new(effect_type::FIELD, 0);
            e.flag[0] |= flag::FUNC_VALUE;
            e.value = 5;
            assert_eq!(e.get_value(&f, &ctx), 5);
        }
    }

    /// The pool scan: by code, and by whose effect it is.
    #[test]
    fn effects_are_searchable_by_player_and_code() {
        let mut fx = Effects::new();
        let mut a = Effect::new(effect_type::FIELD, 42);
        a.effect_owner = 0;
        let mut b = Effect::new(effect_type::FIELD, 42);
        b.effect_owner = 1;
        let mut c = Effect::new(effect_type::FIELD, 99);
        c.effect_owner = 0;
        fx.insert(a);
        fx.insert(b);
        fx.insert(c);

        assert_eq!(fx.with_code_owned_by(0, 42).count(), 1);
        assert_eq!(fx.with_code_owned_by(1, 42).count(), 1);
        assert_eq!(fx.with_code_owned_by(0, 99).count(), 1);
        assert_eq!(fx.with_code_owned_by(1, 99).count(), 0);
        assert_eq!(fx.len(), 3);
    }
}

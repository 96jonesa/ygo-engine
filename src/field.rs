//! The duel: the state everything else is a view of.
//!
//! A translation of ocgcore's `field`, which holds three things the rest of
//! the engine reaches through: `core` (the processor's own working state —
//! the unit queue, the event lists, the chain lists), `infos` (the duel's
//! counters), and the players' zones.
//!
//! The pointer-to-index decision recorded in `event.rs` is what makes this
//! possible to express in Rust at all: cards and effects live in arenas on
//! this struct, and everything that the reference would hold a `card*` for
//! holds a `CardId` instead.
//!
//! The `raise_*` functions and `is_activate_ready` carry
//! `clippy::too_many_arguments` allowances.
//! Their parameter lists are the reference's, in the reference's order, and
//! that is worth more here than the lint: a call site in this crate should
//! be readable against the corresponding call site in `processor.cpp`
//! without an argument-bundling struct to translate in between.

use crate::board::{location, position, PlayerZones};
use crate::card::{assume, card_type, reason, second_code, status, Card, CardData};
use crate::chain::{chain_flag, Chain, ChainArray, ChainList};
use crate::duel::{flags, phases, REFERENCE_CONFIGURATION};
use crate::effect::{effect_count, effect_type, flag, flag2, Ctx, Effect, Effects};
use crate::event::{code, CardId, EffectId, Event, PLAYER_NONE};
use crate::processor::{Kind, Unit};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::mem;

/// Index into the duel's group arena. `group*` in the reference.
pub type GroupId = usize;

/// A list of events, in the reference's `std::list<tevent>`.
pub type EventList = VecDeque<Event>;

/// The duel's counters. `field_info` in the reference, including the two
/// values that start at 1 rather than 0 — ids, where 0 means "none".
#[derive(Clone, Debug)]
pub struct Infos {
    /// Stamped onto every event raised, and carried onto the chains built
    /// from it. Bumped when the engine decides one batch of events is over.
    pub event_id: u32,
    /// Allocates card field ids, effect registration ids, and chain ids —
    /// all three from this one sequence, so that "which happened first" is
    /// answerable *across* kinds and not only within one.
    /// A `Cell`: `is_available` draws from it through a shared reference.
    pub field_id: std::cell::Cell<u32>,
    pub copy_id: u16,
    pub turn_id: i16,
    pub turn_id_by_player: [i16; 2],
    pub card_id: u32,
    pub phase: u16,
    pub turn_player: u8,
    pub priorities: [u8; 2],
    pub can_shuffle: bool,
}

impl Default for Infos {
    fn default() -> Self {
        Self {
            event_id: 1,
            field_id: std::cell::Cell::new(1),
            copy_id: 1,
            turn_id: 0,
            turn_id_by_player: [0, 0],
            card_id: 1,
            phase: 0,
            turn_player: 0,
            priorities: [0, 0],
            can_shuffle: true,
        }
    }
}

/// The registration indices the gather searches.
///
/// `field_effect` in the reference: eight `std::multimap<uint32_t, effect*>`,
/// one per effect type, keyed by the event code the effect responds to. An
/// effect is registered into the index for its type when it becomes active
/// and removed when it stops being.
///
/// The container choice is not incidental. `std::multimap` is *ordered* by
/// key and preserves insertion order among equal keys, and `equal_range`
/// walks them in that order — which is the order chains are built in, and
/// therefore observable. `BTreeMap<u32, Vec<EffectId>>` has both properties;
/// a `HashMap` would have neither.
/// The disable-check list: cards whose `DISABLED`/`FORBIDDEN` status may
/// have changed, to be refreshed at the next adjust.
///
/// The reference keeps a `std::set<card*>`, which the port had mirrored
/// with a `BTreeSet` — a node allocation per insert and a tree walk per
/// drain, on every adjust of every card on the field. A list that is
/// sorted and deduplicated when drained visits the same cards in the same
/// (card-id) order and allocates nothing after the first adjust.
#[derive(Clone, Debug, Default)]
pub struct CheckList {
    items: Vec<CardId>,
}

impl CheckList {
    pub fn insert(&mut self, card: CardId) {
        self.items.push(card);
    }
    pub fn contains(&self, card: &CardId) -> bool {
        self.items.contains(card)
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    pub fn len(&self) -> usize {
        let mut v = self.items.clone();
        v.sort_unstable();
        v.dedup();
        v.len()
    }
    pub fn clear(&mut self) {
        self.items.clear();
    }
    pub fn into_items(self) -> Vec<CardId> {
        self.items
    }
    /// The cards, each once, in ascending id order — the drained list's
    /// contract — leaving the list empty with its capacity kept.
    pub fn drain_sorted(&mut self) -> std::vec::Drain<'_, CardId> {
        self.items.sort_unstable();
        self.items.dedup();
        self.items.drain(..)
    }
}

#[derive(Clone, Default)]
pub struct FieldEffects {
    pub aura: EffectIndex,
    pub ignition: EffectIndex,
    pub activate: EffectIndex,
    pub trigger_o: EffectIndex,
    pub trigger_f: EffectIndex,
    pub quick_o: EffectIndex,
    pub quick_f: EffectIndex,
    pub continuous: EffectIndex,
    /// Which effects are registered at all. The reference keeps an
    /// `effect*` to iterator map so it can erase without re-deriving the
    /// container; an index-based port needs only the membership.
    pub indexer: BTreeSet<EffectId>,
    /// The `EFFECT_SPSUMMON_COUNT_LIMIT` effects, kept as their own set.
    ///
    /// They are in `aura` as well; this is the reference's side index, and it
    /// exists because both counter functions iterate *all* of them on every
    /// Special Summon rather than looking one up by code.
    pub spsummon_count_eff: BTreeSet<EffectId>,
    /// Field-only effects that reset at a phase, at a chain's end, or that
    /// carry a count limit to be recharged.
    pub pheff: BTreeSet<EffectId>,
    pub cheff: BTreeSet<EffectId>,
    pub rechargeable: BTreeSet<EffectId>,
    /// Cards whose disabled state may have changed and needs re-asking.
    pub disable_check_set: CheckList,
    /// Oath effects, each mapped to the effect whose activation created it.
    ///
    /// An "oath" is a promise made as a cost — *you cannot do X this turn* —
    /// and it has to be taken back if the activation that made it is
    /// negated. The value is that activation; `None` means it resolved, so
    /// the promise stands whatever happens next.
    pub oath: BTreeMap<EffectId, Option<EffectId>>,
}

/// Where a gathered continuous effect's chain goes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ContinuousDest {
    Tp,
    Ntp,
    DelayedTp,
    DelayedNtp,
}

/// `NO_FLIP_EFFECT` — set in an event's value to say a flip happened but
/// must not trigger flip effects. The reference keeps it in the high half
/// and shifts it down to compare, so the constant is the unshifted one.
pub const NO_FLIP_EFFECT: u32 = 0x10000;

/// One `std::multimap<uint32_t, effect*>`: effects keyed by event code, in
/// registration order within a code.
/// One activity counter's state: the effect's check and a per-player tally.
///
/// `PartialEq` is deliberately *not* derived: the check is a function
/// pointer, and comparing those is meaningless — two distinct `fn` items may
/// share an address and one may have several. Nothing compares these.
#[derive(Clone, Debug, Default)]
pub struct ActionCount {
    pub check: Option<crate::effect::TargetFilter>,
    pub player_amount: [u16; 2],
}

/// One player's life-point cost stack. `cost[playerid]` in the reference.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LpCost {
    pub amount: i32,
    pub count: u32,
    pub stack: [i32; 8],
}

/// `return_card` — the cards a player chose, and whether they declined.
///
/// `canceled` is not "chose nothing": a selection with a minimum of zero can
/// legitimately return an empty list, and a *cancelled* one means the player
/// declined to make the choice at all. Callers act on the two differently.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReturnCards {
    pub canceled: bool,
    pub list: Vec<CardId>,
}

impl ReturnCards {
    pub fn clear(&mut self) {
        self.canceled = false;
        self.list.clear();
    }
}

/// `ProgressiveBuffer` — the buffer a sub-process writes its answer into.
///
/// A byte vector read and written through *typed* accessors, and the
/// indexing is the detail to get right: `at::<T>(pos)` reads from byte
/// `pos * size_of::<T>()`, **not** from byte `pos`. So the three `i8` slots
/// `SelectPlace` writes occupy bytes 0, 1 and 2, while an `i32` written at
/// slot 0 occupies bytes 0-3 — the two **alias**, deliberately. `SendTo`
/// reads `i8` slot 2 of an answer other units read as an `i32`.
///
/// Two behaviours that look like accidents and are not:
///
/// - **Reading past the end yields zero**, not an error and not garbage. A
///   unit may read a slot nothing has written, and the reference returns a
///   default-constructed value.
/// - **Writing past the end grows the buffer**, zero-filling the gap.
///
/// Modelled as the reference's byte buffer rather than as a single integer
/// because both properties are load-bearing, and because storing only the
/// leading `i32` returns the wrong byte the moment a unit reads a narrower
/// slot — which `SendTo` step 8 already does.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Returns {
    data: Vec<u8>,
}

impl Returns {
    pub fn clear(&mut self) {
        self.data.clear();
    }

    /// `at<int32_t>(pos)`.
    pub fn at_i32(&self, pos: usize) -> i32 {
        let (start, end) = (pos * 4, pos * 4 + 4);
        if self.data.len() < end {
            return 0;
        }
        i32::from_le_bytes(self.data[start..end].try_into().unwrap())
    }

    /// `set<int32_t>(pos, val)`.
    pub fn set_i32(&mut self, pos: usize, val: i32) {
        let (start, end) = (pos * 4, pos * 4 + 4);
        if self.data.len() < end {
            self.data.resize(end, 0);
        }
        self.data[start..end].copy_from_slice(&val.to_le_bytes());
    }

    /// `at<int8_t>(pos)`.
    pub fn at_i8(&self, pos: usize) -> i8 {
        self.data.get(pos).map_or(0, |&b| b as i8)
    }

    /// `at<uint32_t>(pos)`.
    pub fn at_u32(&self, pos: usize) -> u32 {
        self.at_i32(pos) as u32
    }

    /// `at<uint16_t>(pos)` — bytes `pos * 2` and `pos * 2 + 1`.
    pub fn at_u16(&self, pos: usize) -> u16 {
        let (start, end) = (pos * 2, pos * 2 + 2);
        if self.data.len() < end {
            return 0;
        }
        u16::from_le_bytes(self.data[start..end].try_into().unwrap())
    }

    /// `at<int16_t>(pos)` — bytes `pos * 2` and `pos * 2 + 1`, signed.
    ///
    /// `SelectCounter`'s answer is one `int16_t` per offered card, and the
    /// validation compares it against the card's counter total. Reading it
    /// unsigned would turn a negative answer into a huge positive one and
    /// the `<` check would pass it.
    pub fn at_i16(&self, pos: usize) -> i16 {
        self.at_u16(pos) as i16
    }

    /// `set<int16_t>(pos, val)`.
    pub fn set_i16(&mut self, pos: usize, val: i16) {
        let (start, end) = (pos * 2, pos * 2 + 2);
        if self.data.len() < end {
            self.data.resize(end, 0);
        }
        self.data[start..end].copy_from_slice(&val.to_le_bytes());
    }

    /// `at<uint64_t>(pos)`.
    ///
    /// `AnnounceRace`'s answer is a 64-bit mask: the reference widened the
    /// race table past 32 bits, so reading this as a `u32` silently drops
    /// every race above the thirty-second.
    pub fn at_u64(&self, pos: usize) -> u64 {
        let (start, end) = (pos * 8, pos * 8 + 8);
        if self.data.len() < end {
            return 0;
        }
        u64::from_le_bytes(self.data[start..end].try_into().unwrap())
    }

    /// `set<uint64_t>(pos, val)`.
    pub fn set_u64(&mut self, pos: usize, val: u64) {
        let (start, end) = (pos * 8, pos * 8 + 8);
        if self.data.len() < end {
            self.data.resize(end, 0);
        }
        self.data[start..end].copy_from_slice(&val.to_le_bytes());
    }

    /// `bitGet(pos)` — bit `pos % 8` of byte `pos / 8`.
    ///
    /// Out of range reads false, like the typed accessors.
    pub fn bit_get(&self, pos: usize) -> bool {
        self.data
            .get(pos / 8)
            .is_some_and(|&b| b & (1 << (pos % 8)) != 0)
    }

    /// `at<uint8_t>(pos)`.
    ///
    /// The same byte as [`Returns::at_i8`], read unsigned. `SelectPlace`
    /// *writes* its answer with `set<int8_t>` and *validates* it with
    /// `at<uint8_t>`, so both views are needed — and the difference is not
    /// cosmetic: a player id of 255 must fail the `> 1` test rather than
    /// read back as -1 and pass it.
    pub fn at_u8(&self, pos: usize) -> u8 {
        self.data.get(pos).copied().unwrap_or(0)
    }

    /// `set<int8_t>(pos, val)`.
    pub fn set_i8(&mut self, pos: usize, val: i8) {
        if self.data.len() < pos + 1 {
            self.data.resize(pos + 1, 0);
        }
        self.data[pos] = val as u8;
    }

    /// The leading `int32_t` — what most units mean by "the answer".
    pub fn get(&self) -> i32 {
        self.at_i32(0)
    }

    /// The common case for writing.
    pub fn set(&mut self, val: i32) {
        self.set_i32(0, val);
    }
}

#[derive(Clone, Debug, Default)]
pub struct EffectIndex {
    /// Per code, in insertion order. A hash map, not a tree: this is the
    /// lookup every effect walk makes, millions of times a game, and its
    /// iteration order is never used (`pairs` carries the ordered view).
    entries: crate::fxhash::FxHashMap<u32, Vec<EffectId>>,
    /// The same contents flattened, kept so the whole index can be walked in
    /// the reference's iteration order without rebuilding it each time.
    pairs: Vec<(u32, EffectId)>,
}

impl EffectIndex {
    pub fn insert(&mut self, code: u32, effect: EffectId) {
        self.entries.entry(code).or_default().push(effect);
        let at = self.pairs.partition_point(|&(c, _)| c <= code);
        self.pairs.insert(at, (code, effect));
    }

    /// `equal_range(code)` — every effect registered under this code, in the
    /// order they were registered.
    pub fn equal_range(&self, code: u32) -> &[EffectId] {
        self.entries.get(&code).map_or(&[], Vec::as_slice)
    }

    pub fn remove(&mut self, code: u32, effect: EffectId) {
        if let Some(v) = self.entries.get_mut(&code) {
            if let Some(i) = v.iter().position(|&e| e == effect) {
                v.remove(i);
            }
            if v.is_empty() {
                self.entries.remove(&code);
            }
        }
        if let Some(i) = self.pairs.iter().position(|&p| p == (code, effect)) {
            self.pairs.remove(i);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Every `(code, effect)` pair, in code order then registration order —
    /// what iterating the reference's `std::multimap` gives.
    pub fn iter(&self) -> impl Iterator<Item = &(u32, EffectId)> {
        self.pairs.iter()
    }
}

impl FieldEffects {
    /// The index an effect of this type registers into, in the reference's
    /// order — which is `field::add_effect`'s chain of `else if`s, not a set
    /// of independent tests.
    ///
    /// Three things here are easy to get wrong, and were:
    ///
    /// - The **aura** case is "not an action effect", not "a field effect".
    ///   A continuous field effect is an aura; a *triggered* one is not.
    /// - `IGNITION` is tested **first** among the action types, before
    ///   `ACTIVATE`. An effect that is both registers as an ignition effect.
    /// - `TRIGGER_O` and `TRIGGER_F` additionally require `FIELD`. A
    ///   triggered effect that is not a field effect falls past them — and
    ///   in the reference lands on an *uninitialised* iterator unless it is
    ///   also quick, activate or continuous. Such effects reach a card's own
    ///   container rather than this one, so the case does not arise; it is
    ///   noted because the code looks like it handles them.
    pub fn index_for_mut(&mut self, ty: u16) -> Option<&mut EffectIndex> {
        if ty & effect_type::ACTIONS == 0 {
            return Some(&mut self.aura);
        }
        match ty {
            t if t & effect_type::IGNITION != 0 => Some(&mut self.ignition),
            t if t & effect_type::TRIGGER_O != 0 && t & effect_type::FIELD != 0 => {
                Some(&mut self.trigger_o)
            }
            t if t & effect_type::TRIGGER_F != 0 && t & effect_type::FIELD != 0 => {
                Some(&mut self.trigger_f)
            }
            t if t & effect_type::QUICK_O != 0 => Some(&mut self.quick_o),
            t if t & effect_type::QUICK_F != 0 => Some(&mut self.quick_f),
            t if t & effect_type::ACTIVATE != 0 => Some(&mut self.activate),
            t if t & effect_type::CONTINUOUS != 0 => Some(&mut self.continuous),
            _ => None,
        }
    }
}

/// The processor's working state: `processor core` in the reference.
///
/// Almost every member is a queue or a list, and *which* list a thing is on
/// is how the rules' timing is expressed. They are named after the
/// reference's members rather than after what they mean, because the
/// meanings only become legible alongside the code that moves between them.
#[derive(Clone, Default)]
// `Core` is `Default` for the bulk of its fields, but **not every field's
// zero value is its initial value** — the reference initialises some
// members in the declaration. Those are set in [`Core::new`], which is
// what `Field` uses; deriving `Default` alone gives a `Core` that is
// subtly wrong and looks fine.
pub struct Core {
    /// The unit queue the processor loop runs. `core.units`.
    pub units: VecDeque<Unit>,
    /// Work emplaced by the running unit, spliced to the front of `units`
    /// at the top of the next step. `core.subunits`.
    pub subunits: Vec<Unit>,

    /// Events raised but not yet gathered from. `raise_event` appends here,
    /// and the gather drains it.
    pub queue_event: EventList,
    /// Events the gather has consumed, still answerable by `check_event`
    /// for the rest of this window.
    pub instant_event: EventList,
    /// The events a timing window is built around.
    pub point_event: EventList,
    /// Events aimed at one card, gathered against that card's own effects
    /// rather than the field's. `raise_single_event` appends here.
    pub single_event: EventList,
    /// Events that are over.
    pub used_event: EventList,
    pub delayed_activate_event: EventList,

    /// Chains gathered and waiting to be built into the chain stack.
    pub new_chains: ChainList,
    /// The chain stack itself, innermost last.
    pub current_chain: ChainArray,
    /// What a player is being offered.
    pub select_chains: ChainList,

    /// Mandatory triggers gathered this batch, and optional ones.
    pub new_fchain: ChainList,
    pub new_ochain: ChainList,
    /// The same two, held back while a flip is still resolving.
    pub new_fchain_b: ChainList,
    pub new_ochain_b: ChainList,

    /// Continuous effects waiting to be solved, split by whose they are.
    /// A `DELAY` effect raised while a chain is solving goes here instead of
    /// being solved at once.
    pub delayed_continuous_tp: ChainList,
    pub delayed_continuous_ntp: ChainList,
    /// Continuous effects being solved now, and the ones queued to join
    /// them at the front.
    pub solving_continuous: ChainList,
    pub sub_solving_continuous: ChainList,
    pub conti_solving: bool,
    /// Set while a flip is still resolving, which diverts the trigger
    /// gather into the `_b` lists.
    pub flip_delayed: bool,

    /// Mandatory quick effects, one chain per effect.
    ///
    /// `std::map<effect*, chain>` in the reference — ordered by *pointer
    /// address*. That order is observable, since the map is iterated to
    /// build what a player is offered. Keying by `EffectId` instead gives
    /// creation order, which is what pointer order amounts to in practice
    /// (effects come from a pool allocated in order) and is reproducible,
    /// which pointer order is not.
    pub quick_f_chain: BTreeMap<EffectId, Chain>,
    /// Optional quick effects with `EFFECT_FLAG_DELAY`, held for the next
    /// window. Keyed the same way and for the same reason.
    ///
    /// The **whole event** is part of the key, not its id: the reference
    /// later erases `(effect, chain->evt)` by value, so the event has to be
    /// recoverable from the set rather than merely identified.
    pub delayed_quick_tmp: BTreeSet<(EffectId, Event)>,

    /// The events a running subunit is solving for.
    pub sub_solving_event: EventList,
    /// What a player is being offered, alongside `select_chains`.
    pub select_options: Vec<u64>,
    /// The cards a `Select*` unit is offering. Filled by whatever asks the
    /// question, read by the unit, and — for `SelectCard` — sorted into
    /// `card_operation_sort` order before being shown, so the indices the
    /// host answers with refer to that order and not to insertion order.
    pub select_cards: Vec<CardId>,
    /// The cards already chosen, offered back so a selection can be undone.
    /// `SelectUnselectCard` shows both lists as one numbered sequence, with
    /// these after `select_cards`.
    pub unselect_cards: Vec<CardId>,
    /// What a summon may tribute, in the three groups
    /// `get_summon_release_list` separates. Filled before a tribute
    /// selection and read by it.
    pub release_cards: Vec<CardId>,
    pub release_cards_ex: Vec<CardId>,
    pub release_cards_ex_oneof: Vec<CardId>,
    /// Whether a tribute selection may be abandoned. Set by whatever offers
    /// the summon; read by `SelectTribute` when it asks.
    pub summon_cancelable: bool,
    /// How deeply nested the summon machines are.
    ///
    /// A summon that happens *during* another summon — a procedure that
    /// summons something as part of itself — must not pay twice or open its
    /// own negation window. Only the outermost one does either, and this is
    /// the counter that decides which that is.
    pub summon_depth: u32,
    /// The activity counters. `action_counter_t` in the reference — one map
    /// per kind of action, keyed by the effect that is watching, holding
    /// that effect's check function and a per-player tally.
    ///
    /// The tally is *not* a count of actions: it counts how many times the
    /// watching effect's check has **refused**, and once it has refused at
    /// all it is never asked again this turn. See `check_card_counter`.
    pub normalsummon_counter: std::collections::HashMap<u32, ActionCount>,
    /// The same, for `ACTIVITY_SUMMON`. A normal summon bumps **both** —
    /// `ACTIVITY_SUMMON` counts summons of every kind, `ACTIVITY_NORMALSUMMON`
    /// only the normal ones — so a set bumps only the latter.
    pub summon_counter: std::collections::HashMap<u32, ActionCount>,
    /// Bumped when a normal summon or set happens; a separate tally from
    /// the counters above, which are per-effect.
    pub normalsummon_state_count: [u32; 2],
    /// Bumped by a summon of any kind. Paired with `summon_counter` exactly
    /// as `normalsummon_state_count` is with `normalsummon_counter`.
    pub summon_state_count: [u32; 2],
    /// The flip-summon pair. A flip summon bumps **only** these — it is not
    /// an `ACTIVITY_SUMMON`, which is what separates "was anything summoned"
    /// from "was a monster put onto the field".
    pub flipsummon_counter: std::collections::HashMap<u32, ActionCount>,
    /// The field-wide special-summon tally, paired with
    /// `spsummon_state_count` as the others are with theirs. Distinct from
    /// the **per-card** `spsummon_counter` on `Card`, which counts the same
    /// thing for one card under an `EFFECT_SPSUMMON_COUNT_LIMIT`; `Turn`
    /// clears both, by different routes.
    pub spsummon_counter: std::collections::HashMap<u32, ActionCount>,
    /// The battle system's two tallies, cleared with the rest at the top of
    /// every turn. Nothing writes them yet — `BattleCommand` is not ported
    /// — but `Turn` clears all six counters together and splitting the
    /// clear is how one gets forgotten.
    pub attack_counter: std::collections::HashMap<u32, ActionCount>,
    pub chain_counter: std::collections::HashMap<u32, ActionCount>,
    pub flipsummon_state_count: [u32; 2],
    /// Set while `EFFECT_REMOVE_BRAINWASHING` is being honoured, and read by
    /// the control-status queries.
    pub remove_brainwashing: bool,
    pub last_control_changed_id: u32,
    /// What the host chose. `return_cards` in the reference.
    pub return_cards: ReturnCards,
    pub select_effects: Vec<Option<EffectId>>,
    /// Chain-activation restrictions in force. Cleared when a chain starts.
    pub chain_limit: Vec<EffectId>,
    /// The answer from the last sub-process that produced one.
    pub returns: Returns,
    /// Cards confirmed to be leaving the field at the end of the chain.
    /// Operations that have suspended, addressed by the token their
    /// `ExecuteOperation` unit carries.
    ///
    /// They live here rather than in the unit because a unit's `Kind` is
    /// cloned on every step and must stay `Clone + Debug + Eq`, which a
    /// boxed closure is not. ocgcore has the same split: the coroutine
    /// lives in the Lua registry and the unit holds only a reference to
    /// it.
    pub suspensions: Vec<Option<crate::effect::Suspended>>,
    pub leave_confirmed: Vec<CardId>,
    /// How many chains have really been built, as distinct from how many
    /// links the stack shows.
    pub real_chain_count: i32,
    /// Set when something happened this phase that a player chose to do.
    pub phase_action: bool,

    /// Cards sent to the graveyard in the batch now being gathered from.
    /// Read by the simultaneity check that decides whether a trigger sees
    /// its own handler's departure.
    pub just_sent_cards: BTreeSet<CardId>,
    /// `core.set_group_seq` — which seat each card of a group set was
    /// placed in, indexed by the order they were chosen.
    ///
    /// Seven entries because that is the Spell & Trap row; a Field Spell
    /// takes none of them.
    pub set_group_seq: [u32; 7],
    /// Chosen, but not yet moved.
    pub set_group_pre_set: Vec<CardId>,
    /// Moved and set.
    pub set_group_set: Vec<CardId>,
    /// A bitmask of the seats this group set has already spoken for, so
    /// the next card in the batch is not offered one of them.
    pub set_group_used_zones: u32,
    /// `core.copy_reset` / `copy_reset_count` — the reset a copied effect
    /// inherits while `STATUS_COPYING_EFFECT` is on its card.
    pub copy_reset: u32,
    pub copy_reset_count: u8,
    /// `core.coin_results` — what the last `TossCoin` produced, in order.
    ///
    /// `true` is `COIN_HEADS`. A replacing or choosing effect writes this
    /// *instead of* the rolls, which is why it is field state rather than
    /// a local: the effect and the unit that reads it back are different
    /// pieces of machinery.
    pub coin_results: Vec<bool>,

    /// Cards on the field that impose a "only one face-up" limit, per
    /// controller. A cache: `check_unique_onfield` walks it rather than the
    /// whole board.
    pub unique_cards: [Vec<CardId>; 2],

    /// The working copies of the trigger lists the offer loops consume,
    /// and the hand-trigger subset the non-public SEGOC rules keep.
    pub new_fchain_s: ChainList,
    pub new_ochain_s: ChainList,
    pub new_ochain_h: ChainList,
    /// Every event this window is about, kept for the delayed activations.
    pub full_event: EventList,
    /// The delayed quick effects promoted from `delayed_quick_tmp` when the
    /// window opened.
    pub delayed_quick: BTreeSet<(EffectId, Event)>,
    /// Ignition effects that keep priority under the obsolete-ignition
    /// rulings.
    pub ignition_priority_chains: ChainList,
    /// Which player the offer loops are currently asking.
    pub current_player: u8,
    /// How many free-chain continuous effects each player was offered.
    pub spe_effect: [u8; 2],
    /// Set once a player has been made to discard to the hand limit.
    pub hand_adjusted: bool,
    /// An attack a card forced, waiting for a window to close before it is
    /// carried out.
    pub set_forced_attack: bool,
    /// The five things the Main Phase offers, rebuilt from scratch at the
    /// top of every `IdleCommand` and read by `SelectIdleCmd` — which is
    /// why they live in `core` rather than in either unit's state: the
    /// builder and the asker are different processor units, and the answer
    /// indexes back into the same lists.
    ///
    /// The order within each list **is** the answer's meaning: the host
    /// replies with an index, so a list rebuilt in a different order is a
    /// different answer to the same question.
    pub summonable_cards: Vec<CardId>,
    pub spsummonable_cards: Vec<CardId>,
    pub repositionable_cards: Vec<CardId>,
    pub msetable_cards: Vec<CardId>,
    pub ssetable_cards: Vec<CardId>,
    /// Whether the Main Phase may be left for the Battle Phase and for the
    /// End Phase. Both are recomputed at the top of every `IdleCommand`,
    /// and both are also *read back* by `SelectIdleCmd`'s validation — an
    /// answer of "go to battle" is rejected when `to_bp` says there is no
    /// battle to go to.
    pub to_bp: bool,
    pub to_ep: bool,
    /// Set when Main Phase 2 is to be skipped without asking. Unlike
    /// `EFFECT_SKIP_M2`, this is a one-shot: `IdleCommand` consumes it,
    /// clearing the flag as it reads it.
    pub skip_m2: bool,
    /// Piles that have been disturbed and should be shuffled before anyone
    /// sees them again, and the switch that suppresses the bookkeeping.
    pub shuffle_deck_check: [bool; 2],
    pub shuffle_hand_check: [bool; 2],
    pub shuffle_check_disabled: bool,
    /// **Solver mode.** When set, every event the duel would decide from its
    /// own generator — a coin toss, a random selection, which cards are on
    /// top of a deck about to be read — is instead put to the host as a
    /// question, so a solver can treat it as a chance node with known
    /// outcomes. Off (the default) is the faithful mode the differential
    /// harness validates: the generator decides, and no such question is
    /// ever asked. The two modes must agree exactly when the host answers
    /// with the generator's own values; `driver::chance_tests` checks it.
    pub chance_mode: bool,
    /// The last random selection's picks, for the operation that asked
    /// (`api::random_selected`).
    pub random_selected: Vec<CardId>,
    /// Solver mode's knowledge channel: every card shown to a player by a
    /// confirm or reveal since the host last looked, as `(player, card)`.
    /// The reference's messages carry codes, not identities, so the
    /// showing sites record the identity here for
    /// [`crate::observation::Knowledge`]; the `Game` drains it every step.
    pub revealed: Vec<(u8, CardId)>,
    /// How deeply nested the cost/target/operation executors currently are.
    ///
    /// The pile shuffles that hide information are deferred to the
    /// **outermost** executor: an effect whose cost disturbs a hand, inside
    /// another effect's operation, shuffles once when the outer one
    /// finishes, not twice. `check_level` is the depth counter that makes
    /// "outermost" decidable.
    pub check_level: u32,
    /// The life-point cost stack, per player. `save_lp_cost` pushes the
    /// current amount and `restore_lp_cost` pops it, so a cost evaluated
    /// inside another cost does not corrupt the outer one.
    ///
    /// The depth counter is **not** bounded by the stack: it keeps counting
    /// past 8 while the stack stops recording. Deeper than eight and the
    /// restore brings back whatever was at the top — the reference accepts
    /// that rather than growing, and a port that bounded the counter instead
    /// would restore the *wrong* entry rather than a stale one.
    pub lp_cost: [LpCost; 2],
    /// How many normal summons each player has used this turn, and whether
    /// they have spent their one *extra* summon. The two are separate
    /// because an extra summon is not a higher limit — it is a one-shot
    /// permission that the count limit does not govern.
    pub summon_count: [u32; 2],
    pub extra_summon: [bool; 2],
    /// Effects taken out of play this step, kept so anything holding one can
    /// notice it is gone.
    pub reseted_effects: BTreeSet<EffectId>,
    /// Replacement effects currently resolving, the events they are
    /// resolving for, and the ones whose operations are deferred.
    pub continuous_chain: ChainList,
    pub solving_event: EventList,
    pub desrep_chain: ChainList,
    /// Cards whose destruction a replacement effect cancelled.
    pub destroy_canceled: BTreeSet<CardId>,
    /// Cards that stood in for a battle destruction — the substitutes an
    /// `EFFECT_DESTROY_SUBSTITUTE` named on the battle path. Kept separate
    /// from the non-battle path's `extra`, which is a local, because the
    /// battle machinery reads this one afterwards.
    pub battle_destroy_rep: BTreeSet<CardId>,
    /// Indestructible-count effects that have been *used* but not yet
    /// charged. `Release` defers `dec_count` to its last step so that a
    /// release which turns out not to happen does not spend a count.
    pub dec_count_reserve: Vec<EffectId>,
    /// Cards queued to destroy themselves, and the switch that suspends the
    /// whole self-destroy pass.
    pub unique_destroy_set: BTreeSet<CardId>,
    pub self_destroy_set: BTreeSet<CardId>,
    pub self_tograve_set: BTreeSet<CardId>,
    /// A player who tried to draw from an empty deck this step.
    pub overdraw: [bool; 2],
    /// What the last completed operation acted on, for the caller to read.
    pub operated_set: Vec<CardId>,
    /// Whether the decks are currently face-up.
    pub deck_reversed: bool,
    pub selfdes_disabled: bool,
    /// `GLOBALFLAG_*` — cheap "is any card in this duel doing X" answers,
    /// set when such a card's effect registers so the engine can skip whole
    /// passes when none is.
    pub global_flag: u32,

    /// Which optional-trigger timings are still live, per player.
    pub hint_timing: [u32; 2],
    /// Cards already re-evaluated in the current adjust pass.
    pub readjust_map: crate::fxhash::FxHashMap<CardId, u32>,
    /// An effect's own operation, parked while a replacement runs.
    pub backed_up_operation: Option<crate::effect::Operation>,
    /// Cards being special summoned, and cards being equipped, during one
    /// chain link's operation.
    /// A **set**, as the reference's `card_set` is: `special_summon_complete`
    /// turns it into a group, and the order the batch is then processed in
    /// is observable.
    pub special_summoning: BTreeSet<CardId>,
    /// Materials a summon procedure is being *made* to use, and the only
    /// ones it may use. Both are set by the caller before a rule summon and
    /// read by the procedure's own condition; `SpSummonRule` saves and
    /// restores them around its gather, because the gather runs the
    /// conditions and a condition may clobber them.
    pub must_use_mats: Option<GroupId>,
    pub only_use_mats: Option<GroupId>,
    /// A material count the caller is forcing. Zero means "not forced", and
    /// that is the *only* thing that distinguishes forced from unforced —
    /// the two extra arguments are pushed to the procedure only when `minc`
    /// is non-zero.
    pub forced_summon_minc: u32,
    pub forced_summon_maxc: u32,
    /// Set when a pass changed something, so `Adjust` runs again from the
    /// top. The loop terminates because each pass that sets it has also
    /// *done* something, and the board is finite.
    pub re_adjust: bool,
    /// A win already decided by an effect, waiting to be announced, and why.
    /// `5` is "nobody", which is the reference's sentinel and not a player.
    ///
    /// **Initialised to 5, not 0.** `uint8_t win_player{ 5 }` in the
    /// reference's declaration — and 0 is a real player, so a derived
    /// `Default` makes the very first `Adjust` announce a win for player 0.
    /// See [`Core::new`].
    pub win_player: u8,
    pub win_reason: u8,
    /// Set while the turn is being ended by force, which suspends the win
    /// check — the duel is not over, it is being wound up.
    pub force_turn_end: bool,
    /// The battle system's per-turn state counts, and the flag that says an
    /// attack is being chained to. Cleared by `Turn`; written by
    /// `BattleCommand`, which is not ported.
    /// The cards the Battle Phase is currently offering as attackers,
    /// rebuilt by `BattleCommand` and read by `SelectBattleCmd` — the same
    /// split `IdleCommand` and `SelectIdleCmd` have.
    pub attackable_cards: Vec<CardId>,
    /// Whether the Battle Phase may be left for Main Phase 2, alongside
    /// `to_ep`, which it shares with the Main Phase menu.
    pub to_m2: bool,
    /// The attack in progress: who is attacking, what they declared against
    /// (`None` is a direct attack), and the bookkeeping around it.
    pub attack_target: Option<CardId>,
    pub attack_cancelable: bool,
    pub attack_cost_paid: bool,
    /// A chain attack's attacker, by field id rather than by card — the
    /// card may have left and come back, and that is a different card for
    /// this purpose.
    pub chain_attacker_id: u32,
    pub chain_attack_target: Option<CardId>,
    /// Set when an effect is attacking the player directly rather than a
    /// monster doing it.
    pub attack_player: bool,
    /// The attack an effect is forcing, waiting for `ForcedBattle`.
    pub forced_attacker: Option<CardId>,
    pub forced_attack_target: Option<CardId>,
    /// Nonzero while damage calculation is running under an effect rather
    /// than under the battle — `AttackDisable` refuses during it.
    pub effect_damage_step: u8,
    /// The field ids of the attacker and the attack target as they were
    /// when the attack was declared. `AttackDisable` compares against these
    /// so that a card which left and returned is not the same attacker.
    pub pre_field: [u32; 2],
    /// Damage and recovery computed but not yet applied. The damage step
    /// works out every change a battle produces and then applies the batch,
    /// so that two simultaneous changes see the same life totals.
    pub recover_damage_reserve: Vec<crate::life_points::ReservedLpChange>,
    /// The damage each player takes from the battle being calculated.
    /// Written by `calculate_battle_damage` and read by `BattleCommand`,
    /// which is why it lives here rather than being returned.
    pub battle_damage: [u32; 2],
    /// Set when an attack has been undone and has to be re-declared, which
    /// is what makes an "attack replay" a replay rather than a new attack.
    pub attack_rollback: bool,
    /// The field ids of the opposing monsters as they were when the attack
    /// was declared. A replay is offered when this set has changed — the
    /// attack was declared against a board that no longer exists.
    pub opp_mzone: std::collections::BTreeSet<u32>,
    /// Set once damage calculation has happened, so an attack interrupted
    /// afterwards is not re-calculated.
    pub damage_calculated: bool,
    /// The two players' chains, held while their order is being chosen.
    /// `SortChain` sorts one of them; which is decided by comparing the
    /// player against the turn player.
    pub tpchain: ChainList,
    pub ntpchain: ChainList,
    /// A unit that has stepped aside. `DamageStep`'s case 1 moves itself
    /// here and reports finished; `SolveChain` puts it back on the subunit
    /// queue when the chain it made way for ends. So two units interleave
    /// without either sitting on the queue at once.
    pub reserved: Option<crate::processor::Unit>,
    /// `temp_card` — a scratch card the field lends to questions asked
    /// about a **card that does not exist**: "could I Special Summon a
    /// monster shaped like this". The reference keeps one for the life of
    /// the duel and blanks its data afterwards; this is created on first
    /// use and blanked the same way. It sits in the arena and in no zone,
    /// which nothing scans.
    pub temp_card: Option<CardId>,
    /// `Effect.GetChainData` (`chain.lua:75`) — a scratch the **library**
    /// keeps per chain link and effect, not engine state at all. The
    /// reference's is an arbitrary Lua table; this pool stores one number
    /// in it (Metamorphosis remembers the level it tributed), so this is
    /// narrowed to that.
    pub chain_data: std::collections::BTreeMap<(u32, EffectId), i64>,
    pub attack_state_count: [u16; 2],
    pub battle_phase_count: [u16; 2],
    pub battled_count: [u16; 2],
    pub chain_attack: bool,
    /// The monster currently attacking, if the battle system is running.
    /// **The battle system is not ported**; this exists so `Adjust`'s attack
    /// check can take its own first branch, which is "there is no attacker".
    pub attacker: Option<CardId>,
    /// Cards whose controller no longer matches what the effects say, one
    /// set per *side they are sitting on* — which is why `Adjust` crosses
    /// them when it hands them over.
    pub control_adjust_set: [BTreeSet<CardId>; 2],
    /// Trap monsters whose "this is a monster" effect has been switched
    /// off and which therefore have to go back to a Spell/Trap Zone.
    /// Gathered by `Adjust`, acted on by `TrapMonsterAdjust`.
    pub trap_monster_adjust_set: [BTreeSet<CardId>; 2],
    /// The `EFFECT_DISABLE_FIELD` effects `RefreshLoc` still has to ask,
    /// and the two extra-zone lists, each drained one at a time by its own
    /// loop. They are `core` members rather than unit state because the
    /// operation that answers them is a separate processor unit, and the
    /// answer comes back through `returns`.
    pub disfield_effects: Vec<EffectId>,
    pub extra_mzone_effects: Vec<EffectId>,
    pub extra_szone_effects: Vec<EffectId>,
    /// Cards that were about to be Special Summoned and had nowhere to go.
    /// `SpSummon` empties it to the graveyard at its step 1.
    pub ss_tograve_set: BTreeSet<CardId>,
    pub equiping_cards: Vec<CardId>,
    /// Field effects held back until the chain finishes resolving.
    ///
    /// A **set**, as in the reference: a card flipped face-up twice within
    /// one chain must be enabled once, not twice. This was a `Vec` until
    /// `ChangePos` became its first writer.
    pub delayed_enable_set: BTreeSet<CardId>,
    pub spsummon_state_count: [u32; 2],
    pub spsummon_state_count_tmp: [u32; 2],
    /// The half of `spsummon_state_count` a negated chain gives back. Only
    /// ever non-zero under `DUEL_CANNOT_SUMMON_OATH_OLD`.
    pub spsummon_state_count_rst: [u32; 2],
    /// How many times each `spsummon_code` has been Special Summoned this
    /// turn, per player — the "once per turn" ledger — and the same
    /// give-back half.
    pub spsummon_once_map: [std::collections::HashMap<u32, u32>; 2],
    pub spsummon_once_map_rst: [std::collections::HashMap<u32, u32>; 2],
    pub spsummon_rst: bool,

    /// Whose turn it is to be offered a window, or `PLAYER_NONE`.
    pub conti_player: u8,
    pub chain_solving: bool,

    /// The effect currently being asked about, and on whose behalf. Saved
    /// and restored around every callback, because a card's condition may
    /// ask what the *reason* for the question was.
    pub reason_effect: Option<EffectId>,
    pub reason_player: u8,

    /// Activation tallies. Three maps rather than one, because the scopes
    /// reset at different times: the per-turn map at end of turn, the chain
    /// map at chain end, the duel map never.
    /// Scratch buffers for the effect walks and event windows: a walk that
    /// must copy an index range or an event list (because the predicate it
    /// calls takes `&mut self`) takes a buffer here and gives it back, so
    /// after the first window nothing in those paths allocates. Nesting is
    /// fine — each taker gets its own buffer.
    pub scratch_effects: Vec<Vec<EffectId>>,
    pub scratch_events: Vec<Vec<Event>>,
    pub scratch_delayed: Vec<Vec<(EffectId, Event)>>,
    pub effect_count_code: crate::fxhash::FxHashMap<u64, u32>,
    pub effect_count_code_duel: crate::fxhash::FxHashMap<u64, u32>,
    pub effect_count_code_chain: crate::fxhash::FxHashMap<u64, u32>,
}

/// The duel.
#[derive(Clone, Default)]
pub struct Field {
    /// The printed data for every card name the duel may need to look up,
    /// including names no card in the duel has. `read_card` in the
    /// reference, which is a host callback.
    pub database: crate::fxhash::FxHashMap<u32, CardData>,
    pub core: Core,
    pub infos: Infos,
    /// The duel options in force. The machinery branches on these
    /// constantly, so they are part of the specification rather than a
    /// settings bag — see `duel.rs`.
    pub flags: u64,
    /// Every card in the duel. `CardId` indexes this.
    pub cards: Vec<Card>,
    /// Every effect the duel has created. `EffectId` indexes this.
    pub effects: Effects,
    /// The registration indices the gather searches.
    pub field_effects: FieldEffects,
    pub players: Vec<PlayerZones>,
    /// Card groups. `group*` in the reference — shared, mutable handles
    /// that a unit erases from and its caller reads afterwards to see what
    /// survived. A `Vec` passed by value would give each replacement effect
    /// its own private copy to cancel moves in.
    pub groups: Vec<BTreeSet<CardId>>,
    /// The duel's random source. Part of the specification rather than the
    /// harness — see `rng.rs`.
    pub rng: crate::rng::Xoshiro256StarStar,
    /// `duel::uncopy` — effects refused registration because the card was
    /// mid-copy and the effect is `UNCOPYABLE`.
    pub uncopy: BTreeSet<EffectId>,
    /// Messages for the host, in the order they were written.
    pub messages: Vec<Message>,
}

impl Core {
    /// A `Core` with the reference's **declaration initialisers** applied.
    ///
    /// Most of the struct is zero-initialised and `Default` is right for
    /// it. A handful of members are not, because `field.h` gives them a
    /// value in the declaration, and for those the zero is a *legal* value
    /// that means something else. `win_player{ 5 }` is the one that has
    /// bitten: 5 is "nobody" and 0 is a player, so a derived `Default`
    /// makes the first `Adjust` announce a win.
    ///
    /// Anything added here should be checked against `field.h`'s
    /// declaration, not against what looks sensible.
    pub fn new() -> Self {
        Self {
            // `uint8_t win_player{ 5 }`
            win_player: 5,
            // `uint8_t conti_player{ PLAYER_NONE }`
            conti_player: crate::event::PLAYER_NONE,
            ..Default::default()
        }
    }
}

impl Field {
    /// A duel under this project's reference configuration.
    pub fn new(starting_lp: i32) -> Self {
        Self::with_flags(starting_lp, REFERENCE_CONFIGURATION)
    }

    pub fn with_flags(starting_lp: i32, flags: u64) -> Self {
        Self::with_flags_and_seed(starting_lp, flags, Self::default_seed())
    }

    /// The seed a duel gets when nobody names one.
    ///
    /// **Not the same as the differential harness's oracle**, which seeds
    /// `[1, 2, 3, 4]`. That costs nothing while no unit draws — under
    /// `DUEL_PSEUDO_SHUFFLE` the piles are ordered by insertion — but
    /// `TossCoin` does draw, so any comparison has to name the seed on
    /// both sides rather than trust these to coincide.
    pub fn default_seed() -> [u64; 4] {
        [0x2545_F491_4F6C_DD1D, 1, 2, 3]
    }

    /// A duel with a chosen random state. The seed is an argument rather
    /// than a global, because a solver replaying a game needs to replay its
    /// rolls too.
    pub fn with_flags_and_seed(starting_lp: i32, flags: u64, seed: [u64; 4]) -> Self {
        Self {
            flags,
            players: vec![PlayerZones::new(starting_lp), PlayerZones::new(starting_lp)],
            rng: crate::rng::Xoshiro256StarStar::new(seed),
            core: Core::new(),
            ..Default::default()
        }
    }

    /// `field::is_flag` — is this duel option in force?
    pub fn is_flag(&self, flag: u64) -> bool {
        self.flags & flag != 0
    }

    /// `get_pzone_index` — which spell/trap seat is a player's nth Pendulum
    /// Zone. Three different answers depending on the duel options, which is
    /// exactly why it is a function rather than a constant.
    pub fn pzone_index(&self, seq: u8) -> u8 {
        if seq > 1 {
            return 0;
        }
        if self.is_flag(flags::SEPARATE_PZONE) {
            seq + 6 // its own seats, 6 and 7
        } else if self.is_flag(flags::THREE_COLUMNS_FIELD) {
            seq * 2 + 1 // 1 and 3
        } else {
            seq * 4 // 0 and 4 — the outermost spell/trap zones
        }
    }

    /// `is_location_useable` — is this particular seat free to be used?
    ///
    /// A faithful translation, including the seat arithmetic. The
    /// symbolic locations are *rewritten* into a real location and an
    /// adjusted sequence before the bitfield is consulted, which is the
    /// whole trick: `EMZONE` seat 0 is monster seat 5, and a three-column
    /// field shifts the main seats along by one.
    ///
    /// The one case that reaches across the table: an Extra Monster Zone is
    /// shared, so seat `n` is unusable when the opponent occupies the seat
    /// that faces it — `11 - n`.
    pub fn is_location_useable(&self, playerid: usize, loc: u16, sequence: u32) -> bool {
        let blocked = self.players[playerid].blocked();
        let three_columns = u32::from(self.is_flag(flags::THREE_COLUMNS_FIELD));

        let (loc, sequence) = match loc {
            location::EMZONE => (u16::from(location::MZONE), sequence + 5),
            location::MMZONE => (u16::from(location::MZONE), sequence + three_columns),
            location::STZONE => (u16::from(location::SZONE), sequence + three_columns),
            other => (other, sequence),
        };

        if loc == u16::from(location::MZONE) {
            if blocked & (1u32 << sequence) != 0 {
                return false;
            }
            if sequence >= 5 {
                let oppo = self.players[1 - playerid].blocked();
                if oppo & (1u32 << (11 - sequence)) != 0 {
                    return false;
                }
            }
        } else if loc == u16::from(location::SZONE) {
            if blocked & (0x100u32 << sequence) != 0 {
                return false;
            }
        } else if loc == location::FZONE {
            if blocked & (0x100u32 << (5 + sequence)) != 0 {
                return false;
            }
        } else if loc == location::PZONE {
            if !self.is_flag(flags::PZONE) {
                return false;
            }
            let index = self.pzone_index(sequence as u8);
            if blocked & (0x100u32 << index) != 0 {
                return false;
            }
        }
        true
    }

    /// `effect::is_available` — is a single/field/equip effect in force?
    ///
    /// A long predicate, translated in the reference's order because the
    /// order is load-bearing: a later test may dereference something an
    /// earlier one has already rejected.
    ///
    /// Note the first line. An `EFFECT_TYPE_ACTIONS` effect is *never*
    /// "available" in this sense — availability is about continuous effects
    /// that are simply in force, whereas an action effect has to be
    /// activated, and asks `is_activateable` instead.
    pub fn is_available(&self, id: EffectId) -> bool {
        let Some(e) = self.effects.get(id) else {
            return false;
        };
        if e.is_type(effect_type::ACTIONS) {
            return false;
        }
        let cards = &self.cards[..];
        let handler = e.get_handler(cards);
        let owner = e.get_owner(cards);

        let single_or_xmaterial = e.is_type(effect_type::SINGLE | effect_type::XMATERIAL)
            && !e.is_type(effect_type::FIELD);
        if single_or_xmaterial {
            let (Some(h), Some(o)) = (handler, owner) else {
                return false;
            };
            let (hc, oc) = (&self.cards[h], &self.cards[o]);
            // A card with no controller is nowhere; three codes are exempt
            // because they describe the card itself rather than its state.
            if hc.current.controller == PLAYER_NONE
                && e.code != code::ADD_SETCODE
                && e.code != code::ADD_CODE
                && e.code != code::REVIVE_LIMIT
            {
                return false;
            }
            if !e.is_in_range_of_symbolic_mzone(hc) {
                return false;
            }
            if e.is_flag(flag::SINGLE_RANGE) {
                if !e.in_range(cards, hc) {
                    return false;
                }
                if !hc.is_status(status::EFFECT_ENABLED) && !e.is_flag(flag::IMMEDIATELY_APPLY) {
                    return false;
                }
                if hc.current.is_location(u16::from(location::ONFIELD)) && !hc.current.is_faceup() {
                    return false;
                }
            }
            if !Self::passes_owner_gates(e, hc, oc, h, o) {
                return false;
            }
        }

        if e.is_type(effect_type::EQUIP) {
            let (Some(h), Some(o)) = (e.handler, e.owner) else {
                return false;
            };
            let (hc, oc) = (&self.cards[h], &self.cards[o]);
            if hc.current.controller == PLAYER_NONE {
                return false;
            }
            if !Self::passes_owner_gates(e, hc, oc, h, o) {
                return false;
            }
            if !e.is_flag(flag::SET_AVAILABLE) {
                if !hc.is_status(status::EFFECT_ENABLED) {
                    return false;
                }
                if !hc.current.is_faceup() {
                    return false;
                }
            }
        }

        if e.is_type(effect_type::FIELD | effect_type::TARGET) && !e.is_flag(flag::FIELD_ONLY) {
            let (Some(h), Some(o)) = (handler, owner) else {
                return false;
            };
            let (hc, oc) = (&self.cards[h], &self.cards[o]);
            if hc.current.controller == PLAYER_NONE {
                return false;
            }
            if !e.is_in_range_of_symbolic_mzone(hc) || !e.in_range(cards, hc) {
                return false;
            }
            if !hc.is_status(status::EFFECT_ENABLED) && !e.is_flag(flag::IMMEDIATELY_APPLY) {
                return false;
            }
            if hc.current.is_location(u16::from(location::ONFIELD)) && !hc.current.is_faceup() {
                return false;
            }
            if !Self::passes_owner_gates(e, hc, oc, h, o) {
                return false;
            }
        }

        // **And then the condition** — the reference's last three lines
        // (`effect.cpp`: `if (!condition) return TRUE;` then
        // `check_condition(condition, 1)`), which is why its
        // `is_available` is not const.
        //
        // Leaving this out makes every conditional continuous effect
        // unconditional, and that is invisible until a card has one:
        // Reaper on the Nightmare is this pool's first, and without this
        // it self-destroys the moment it is summoned. See
        // `Effect::avail_condition` for why the port keeps this
        // read-only where the reference has one field for both callers.
        let Some(condition) = e.avail_condition else {
            return true;
        };
        let res = condition(self, id);
        // The reference's bookkeeping, verbatim: a condition that passes
        // marks the effect available, and if it was not available before,
        // renumbers it — a fresh id, so it now sorts after everything that
        // was already in force. A condition that fails clears the mark.
        // The gates above return without touching it.
        if res {
            if !e.available.get() {
                e.id.set(self.bump_field_id());
            }
            e.available.set(true);
        } else {
            e.available.set(false);
        }
        res
    }

    /// The four forbidden/disabled tests the reference repeats verbatim in
    /// each branch of `is_available`.
    ///
    /// Factored out because they are identical every time, and because
    /// getting one of the four wrong in one branch only is exactly the sort
    /// of thing that survives review. The pairing is deliberate: an
    /// `OWNER_RELATE` effect is shut off by its *owner's* state, and an
    /// effect whose owner is its handler by that card's own.
    fn passes_owner_gates(
        e: &Effect,
        handler: &Card,
        owner: &Card,
        handler_id: CardId,
        owner_id: CardId,
    ) -> bool {
        let same = handler_id == owner_id;
        if e.is_flag(flag::OWNER_RELATE)
            && e.is_can_be_forbidden()
            && owner.is_status(status::FORBIDDEN)
        {
            return false;
        }
        if same && e.is_can_be_forbidden() && handler.is_status(status::FORBIDDEN) {
            return false;
        }
        if e.is_flag(flag::OWNER_RELATE)
            && !e.is_flag(flag::CANNOT_DISABLE)
            && owner.is_status(status::DISABLED)
        {
            return false;
        }
        if same && !e.is_flag(flag::CANNOT_DISABLE) && handler.is_status(status::DISABLED) {
            return false;
        }
        true
    }

    /// `effect::is_immuned` — does this card shrug the effect off?
    ///
    /// The reference gates on `peffect->value`, which is truthy both for a
    /// constant and for a Lua reference — an immunity effect with no value
    /// at all grants nothing. Here that is "a constant or a function".
    pub fn is_immuned(&self, id: EffectId, card: CardId) -> bool {
        self.cards[card].immune_effect.iter().any(|&immune| {
            self.effects
                .get(immune)
                .is_some_and(|i| i.value != 0 || i.value_fn.is_some())
                && self.is_available(immune)
                && self.check_immune_value(immune, id, card)
        })
    }

    fn check_immune_value(&self, immune: EffectId, against: EffectId, card: CardId) -> bool {
        let Some(i) = self.effects.get(immune) else {
            return false;
        };
        let ev = Event::new(0);
        let ctx = Ctx {
            reason_effect: against,
            player: self.cards[card].current.controller,
            event: &ev,
            card: None,
            args: &[],
        };
        i.get_value(self, &ctx) != 0
    }

    /// `card::is_affect_by_effect` — may this effect touch this card?
    ///
    /// A card in the middle of being summoned is untouchable by every effect
    /// except the two that exist to interrupt a summon. The reference writes
    /// that guard as `is_status(SUMMONING) && (peffect && ...)`, and the
    /// `peffect &&` is not defensive programming: a **null** effect is a
    /// rule-driven action with no card behind it, and those still reach a
    /// summoning card. Dropping the null check would make a summon
    /// untouchable by the rules themselves.
    pub fn is_affect_by_effect(&self, card: CardId, effect: Option<EffectId>) -> bool {
        if self.cards[card].is_status(status::SUMMONING) {
            let blocked = effect.and_then(|e| self.effects.get(e)).is_some_and(|e| {
                e.code != code::CANNOT_DISABLE_SUMMON && e.code != code::CANNOT_DISABLE_SPSUMMON
            });
            if blocked {
                return false;
            }
        }
        let Some(id) = effect else {
            return true;
        };
        if self
            .effects
            .get(id)
            .is_some_and(|e| e.is_flag(flag::IGNORE_IMMUNE))
        {
            return true;
        }
        !self.is_immuned(id, card)
    }

    /// `effect::is_target` — does this effect apply to this card?
    pub fn is_target(&self, id: EffectId, card: CardId) -> bool {
        let Some(e) = self.effects.get(id) else {
            return false;
        };
        if e.is_type(effect_type::ACTIONS) {
            return false;
        }
        let single_ish = effect_type::SINGLE | effect_type::EQUIP | effect_type::XMATERIAL;
        if e.is_type(single_ish) && !e.is_type(effect_type::FIELD) {
            return true;
        }
        if e.is_type(effect_type::TARGET) && !e.is_type(effect_type::FIELD) {
            return self.is_fit_target_function(id, card);
        }

        let c = &self.cards[card];
        if !e.is_flag(flag::SET_AVAILABLE)
            && c.current.is_location(u16::from(location::ONFIELD))
            && !c.current.is_faceup()
        {
            return false;
        }
        if !e.is_flag(flag::IGNORE_RANGE) {
            if c.is_status(
                status::SUMMONING
                    | status::SUMMON_DISABLED
                    | status::ACTIVATE_DISABLED
                    | status::SPSUMMON_STEP,
            ) {
                return false;
            }
            if e.is_flag(flag::SPSUM_PARAM) {
                return false;
            }
            // Which side's range applies: the card's own controller number
            // for an absolute-target effect, otherwise whether it is on the
            // same side as the effect's handler.
            let own_side = if e.is_flag(flag::ABSOLUTE_TARGET) {
                c.current.controller == 0
            } else {
                c.current.controller == e.get_handler_player(&self.cards)
            };
            let range = if own_side { e.s_range } else { e.o_range };
            if !c.current.is_location(range) {
                return false;
            }
        }
        self.is_fit_target_function(id, card)
    }

    /// `effect::is_fit_target_function` — the card's own `target` callback
    /// used as a filter rather than as a declaration.
    ///
    /// Note the arity: two arguments in the reference, not nine. This is the
    /// *filter* use of `target`, distinct from the activation use that
    /// `is_activate_ready` makes with `chk = false`.
    pub fn is_fit_target_function(&self, id: EffectId, card: CardId) -> bool {
        let Some(e) = self.effects.get(id) else {
            return false;
        };
        let Some(filter) = e.target_filter else {
            return true;
        };
        filter(self, id, Some(card), &[])
    }

    /// `effect_sort_id` — the order effect sets are put in before use.
    ///
    /// By `id`, which is registration order. It matters wherever effects
    /// accumulate and then apply in sequence: `get_type` gathers ADD,
    /// REMOVE and CHANGE separately and sorts the lot, so a REMOVE
    /// registered before an ADD applies before it, whatever order the
    /// gathering happened in.
    pub fn sort_by_effect_id(&self, effects: &mut [EffectId]) {
        effects.sort_by_key(|&e| self.effects.get(e).map_or(0, |x| x.id.get()));
    }

    /// `card::filter_effect` — every effect with this code that applies to
    /// this card, from all five places one can come from.
    ///
    /// The reference writes this traversal twice: once collecting into a set
    /// and once, as `is_affected_by_effect`, returning at the first match.
    /// Here it is written once and both are built on it, which is
    /// observationally identical and removes a place for the two copies to
    /// drift apart.
    ///
    /// **The result is sorted by effect id**, which is the reference's
    /// default (`card.cpp`, `if(sort) std::sort(eset, effect_sort_id)`;
    /// every caller in this port corresponds to a `sort = TRUE` call).
    /// That is not tidiness: readers that take the *last* entry —
    /// `refresh_control_status`, the tribute decrease — mean the *newest*,
    /// and the walk's source order (own effects, then equips, then the
    /// rest) is not that. Before this line an Equip Spell's continuous
    /// `EFFECT_SET_CONTROL` came after a later control change's single
    /// effect and won, so Creature Swap could not give a Snatch-Stolen
    /// monster back. `is_affected_by_effect` keeps the walk
    /// order: the reference's first-match reader does not sort either.
    pub fn filter_effect(&self, card: CardId, code: u32) -> Vec<EffectId> {
        let mut out = Vec::new();
        self.walk_applicable_effects(card, code, |e| {
            out.push(e);
            false
        });
        self.sort_by_effect_id(&mut out);
        out
    }

    /// `card::filter_single_continuous_effect` — the effects *attached to*
    /// this card, with no filtering at all.
    ///
    /// Same four sources as [`Field::filter_effect`] and deliberately not the
    /// same function, because it differs in three ways that all matter:
    ///
    /// 1. **No field aura.** `filter_effect` ends by sweeping the field's
    ///    registered effects; this one stops at the card. That is the whole
    ///    point of the name — `SendTo` uses it to offer a card its *own*
    ///    replacement effects, having already offered everyone else's
    ///    through the batch-wide `operation_replace`. Adding the aura here
    ///    would offer every field replacement a second time.
    /// 2. **No `is_available` and no `is_affect_by_effect`.** `filter_effect`
    ///    gates every source on both. This one takes each matching effect as
    ///    it finds it. A replacement effect on a card that is *already
    ///    leaving* would be filtered out by the availability test, which is
    ///    exactly when it needs to run.
    /// 3. **Source 3 tests the wrong card, and that is the reference's.**
    ///    For effects another card aims at this one, `filter_effect` asks
    ///    `is_target(this)` — does it target *me*. This one asks
    ///    `is_target(pcard)` — does it target *its own owner*. Reproduced
    ///    verbatim: it looks like an upstream slip, but a faithful port
    ///    mirrors the behaviour rather than the intent, and correcting it
    ///    here would silently diverge from every duel ocgcore plays.
    ///
    /// The Xyz-material source still skips `EFFECT_TYPE_FIELD` effects, as
    /// in `filter_effect`: a material does not project a field effect.
    pub fn filter_single_continuous_effect(&self, card: CardId, code: u32) -> Vec<EffectId> {
        let mut out = Vec::new();
        let c = &self.cards[card];
        for &e in c.single_effect.equal_range(code) {
            out.push(e);
        }
        for &equipper in &c.equiping_cards {
            for &e in self.cards[equipper].equip_effect.equal_range(code) {
                out.push(e);
            }
        }
        for &owner in &c.effect_target_owner {
            for &e in self.cards[owner].target_effect.equal_range(code) {
                // `is_target(owner)`, not `is_target(card)` — see point 3.
                if self.is_target(e, owner) {
                    out.push(e);
                }
            }
        }
        for &material in &c.xyz_materials {
            for &e in self.cards[material].xmaterial_effect.equal_range(code) {
                if self
                    .effects
                    .get(e)
                    .is_some_and(|x| x.is_type(effect_type::FIELD))
                {
                    continue;
                }
                out.push(e);
            }
        }
        out
    }

    /// `card::is_affected_by_effect` — the first such effect, if any.
    pub fn is_affected_by_effect(&self, card: CardId, code: u32) -> Option<EffectId> {
        let mut found = None;
        self.walk_applicable_effects(card, code, |e| {
            found = Some(e);
            true
        });
        found
    }

    /// `card::is_affected_by_effect(code, target)` — the reference's second
    /// overload.
    ///
    /// The same traversal, with one extra condition: the effect's **value is
    /// asked about `target`** and must come back non-zero. That is what turns
    /// a blanket "cannot be tributed" into "cannot be tributed *for that
    /// monster*", and dropping the target would make every such restriction
    /// unconditional.
    pub fn is_affected_by_effect_against(
        &self,
        card: CardId,
        code: u32,
        target: CardId,
    ) -> Option<EffectId> {
        let mut found = None;
        self.walk_applicable_effects(card, code, |e| {
            if self.effect_value_for_card_pub(e, target) != 0 {
                found = Some(e);
                return true;
            }
            false
        });
        found
    }

    /// The traversal the two share. `stop` returning true ends it early.
    fn walk_applicable_effects(
        &self,
        card: CardId,
        code: u32,
        mut stop: impl FnMut(EffectId) -> bool,
    ) {
        let c = &self.cards[card];

        // 1. The card's own effects. A single-range effect must additionally
        //    be one the card can be affected by.
        for &e in c.single_effect.equal_range(code) {
            let ok = self.is_available(e)
                && (!self
                    .effects
                    .get(e)
                    .is_some_and(|x| x.is_flag(flag::SINGLE_RANGE))
                    || self.is_affect_by_effect(card, Some(e)));
            if ok && stop(e) {
                return;
            }
        }
        // 2. Effects from cards equipped to it.
        for &equipper in &c.equiping_cards {
            for &e in self.cards[equipper].equip_effect.equal_range(code) {
                if self.is_available(e) && self.is_affect_by_effect(card, Some(e)) && stop(e) {
                    return;
                }
            }
        }
        // 3. Effects aimed at it by another card.
        for &owner in &c.effect_target_owner {
            for &e in self.cards[owner].target_effect.equal_range(code) {
                if self.is_available(e)
                    && self.is_target(e, card)
                    && self.is_affect_by_effect(card, Some(e))
                    && stop(e)
                {
                    return;
                }
            }
        }
        // 4. Effects from its Xyz materials — but not field effects, which
        //    a material does not project.
        for &material in &c.xyz_materials {
            for &e in self.cards[material].xmaterial_effect.equal_range(code) {
                if self
                    .effects
                    .get(e)
                    .is_some_and(|x| x.is_type(effect_type::FIELD))
                {
                    continue;
                }
                if self.is_available(e) && self.is_affect_by_effect(card, Some(e)) && stop(e) {
                    return;
                }
            }
        }
        // 5. Field-wide effects, excluding those aimed at players.
        for &e in self.field_effects.aura.equal_range(code) {
            let is_player_target = self
                .effects
                .get(e)
                .is_some_and(|x| x.is_flag(flag::PLAYER_TARGET));
            if !is_player_target
                && self.is_available(e)
                && self.is_target(e, card)
                && self.is_affect_by_effect(card, Some(e))
                && stop(e)
            {
                return;
            }
        }
    }

    /// `field::filter_player_effect` — the field-wide effects with this code
    /// that apply to a *player* rather than to a card.
    ///
    /// Now that the `aura` index exists, this is the reference's function
    /// rather than the pool scan that stood in for it.
    pub fn filter_player_effect(&self, playerid: u8, code: u32) -> Vec<EffectId> {
        self.field_effects
            .aura
            .equal_range(code)
            .iter()
            .copied()
            .filter(|&e| {
                self.effects
                    .get(e)
                    .is_some_and(|x| x.is_target_player(&self.cards, playerid))
                    && self.is_available(e)
            })
            .collect()
    }

    /// `duel::read_card` — the printed data for a card name.
    ///
    /// A host callback in the reference. Here it is a map the duel is built
    /// with; the only caller so far is alias resolution in `get_code`, which
    /// needs the data of a card that may not be in the duel at all.
    pub fn read_card(&self, code: u32) -> Option<&CardData> {
        self.database.get(&code)
    }

    /// `card::get_code` — the card's current name.
    ///
    /// `EFFECT_CHANGE_CODE` replaces the name outright and the *last* such
    /// effect wins. Only if nothing changed it does the alias apply, and only
    /// when no `EFFECT_ADD_CODE` without an operation is present — a card
    /// that has gained a second name keeps its printed one.
    pub fn get_code(&mut self, card: CardId) -> u32 {
        if let Some(&assumed) = self.cards[card].assume.get(&assume::CODE) {
            return assumed as u32;
        }
        if let Some(partial) = self.cards[card].temp.code {
            return partial;
        }
        let printed = self.cards[card].data.code;
        self.cards[card].temp.code = Some(printed);

        let mut changers = self.filter_effect(card, code::CHANGE_CODE);
        self.sort_by_effect_id(&mut changers);
        let mut result = printed;
        if let Some(&last) = changers.last() {
            result = self.effect_value_for_card(last, card) as u32;
        }
        self.cards[card].temp.code = None;

        if result == printed {
            let mut adders = self.filter_effect(card, code::ADD_CODE);
            self.sort_by_effect_id(&mut adders);
            let has_plain_add_code = adders
                .into_iter()
                .rev()
                .any(|e| self.effects.get(e).is_some_and(|x| x.operation.is_none()));
            let alias = self.cards[card].data.alias;
            if alias != 0 && !has_plain_add_code {
                result = alias;
            }
        } else if let Some(data) = self.read_card(result) {
            let alias = data.alias;
            if alias != 0 && second_code(result) == 0 {
                result = alias;
            }
        }
        result
    }

    /// `card::get_another_code` — the second name, for a card that has one.
    pub fn get_another_code(&mut self, card: CardId) -> u32 {
        let code = self.get_code(card);
        if code != self.cards[card].data.code {
            return second_code(code);
        }
        let mut adders = self.filter_effect(card, code::ADD_CODE);
        self.sort_by_effect_id(&mut adders);
        let add = adders
            .into_iter()
            .rev()
            .find(|&e| self.effects.get(e).is_some_and(|x| x.operation.is_none()));
        let Some(add) = add else {
            return 0;
        };
        let other = self.effect_value_for_card(add, card) as u32;
        // The reference has two guards here, one of them dead: `get_code()
        // != otcode` followed immediately by `code != otcode`, with `code`
        // already holding `get_code()`. Only the second can fail, so only
        // the second is translated.
        if code != other {
            other
        } else {
            0
        }
    }

    /// `card::get_type` — what the card counts as *now*.
    ///
    /// Distinct from `data.type_`, which is what is printed. The reference's
    /// parameters are kept: `scard` and `sumtype` describe a summon the
    /// question is being asked in the context of, and are `None`/`0` for the
    /// ordinary "what is this card" question.
    ///
    /// Three kinds of effect take part, and they do not compose the way one
    /// might guess. Effects **with** an `operation` are summon-context ones:
    /// they are skipped entirely when `sumtype` is 0, and otherwise
    /// accumulate into a *separate* value. Effects **without** one adjust the
    /// ordinary value.
    ///
    /// The two accumulators then combine asymmetrically, and this is the
    /// part worth reading twice:
    ///
    /// ```text
    /// type |= alttype;
    /// if (changed) type = alttype;
    /// ```
    ///
    /// `changed` is set **only** in the operation branch, by a `CHANGE_TYPE`
    /// there. So a plain `CHANGE_TYPE` replaces the ordinary value but does
    /// not discard the summon-context one, while a summon-context
    /// `CHANGE_TYPE` discards everything. Setting the flag in both branches —
    /// which is the natural reading of "a change type replaces" — makes a
    /// plain `CHANGE_TYPE` throw away the summon accumulator.
    pub fn get_type(
        &mut self,
        card: CardId,
        scard: Option<CardId>,
        sumtype: u64,
        playerid: u8,
    ) -> u32 {
        if let Some(&assumed) = self.cards[card].assume.get(&assume::TYPE) {
            return assumed as u32;
        }
        let c = &self.cards[card];
        // Nowhere an effect could have reached it.
        if !c.current.is_location(u16::from(
            location::ONFIELD | location::HAND | location::GRAVE,
        )) {
            return c.data.type_;
        }
        // A card in a Pendulum Zone is a Spell, whatever it is printed as —
        // but not when a summon is being asked about.
        if c.current.is_location(location::PZONE) && sumtype == 0 {
            return card_type::PENDULUM + card_type::SPELL;
        }
        if let Some(partial) = c.temp.type_ {
            return partial;
        }

        let printed = c.data.type_;
        let mut resolved = printed;
        // `alttype`. Seeded with the printed type when a summon is being
        // asked about and the card is in a spell/trap zone — a trap monster
        // being summoned, which is the case this seeding exists for.
        let mut alt = if sumtype != 0 && c.current.location == location::SZONE {
            printed
        } else {
            0
        };
        let mut changed = false;
        self.cards[card].temp.type_ = Some(printed);

        // The reference gathers the three codes into one set and sorts it
        // by effect id — the first two calls pass `sort = FALSE`, the third
        // takes the default `TRUE`, which sorts everything accumulated so
        // far. Gathering without the sort would apply them in code order
        // instead of registration order.
        let mut effects = self.filter_effect(card, code::ADD_TYPE);
        effects.extend(self.filter_effect(card, code::REMOVE_TYPE));
        effects.extend(self.filter_effect(card, code::CHANGE_TYPE));
        self.sort_by_effect_id(&mut effects);

        for id in effects {
            let Some(e) = self.effects.get(id) else {
                continue;
            };
            let (e_code, filter) = (e.code, e.operation_filter);
            let has_operation = e.operation.is_some() || filter.is_some();

            if has_operation {
                if sumtype == 0 {
                    continue;
                }
                if let Some(filter) = filter {
                    if !filter(self, id, scard, sumtype, playerid) {
                        continue;
                    }
                }
                let value = self.effect_value_for_card(id, card) as u32;
                match e_code {
                    c if c == code::ADD_TYPE => alt |= value,
                    c if c == code::REMOVE_TYPE => alt &= !value,
                    _ => {
                        alt = value;
                        changed = true;
                    }
                }
            } else {
                let value = self.effect_value_for_card(id, card) as u32;
                match e_code {
                    c if c == code::ADD_TYPE => resolved |= value,
                    c if c == code::REMOVE_TYPE => resolved &= !value,
                    _ => resolved = value,
                }
                self.cards[card].temp.type_ = Some(resolved);
            }
        }

        self.cards[card].temp.type_ = None;
        if changed {
            alt
        } else {
            resolved | alt
        }
    }

    /// `effect::get_active_type` — what the handler counted as when this
    /// effect was activated.
    pub fn get_active_type(&mut self, id: EffectId) -> u32 {
        let Some(e) = self.effects.get(id) else {
            return 0;
        };
        // 0x7f0: the activated/triggered/quick/continuous types. An effect
        // outside that set reports its *owner's* type instead of its
        // handler's, which differ for a granted effect.
        if e.effect_type & 0x7f0 == 0 {
            return match e.get_owner(&self.cards) {
                Some(o) => self.get_type(o, None, 0, PLAYER_NONE),
                None => 0,
            };
        }
        if e.active_type != 0 {
            return e.active_type;
        }
        let is_activate = e.is_type(effect_type::ACTIVATE);
        let Some(handler) = e.get_handler(&self.cards) else {
            return 0;
        };
        if is_activate && self.cards[handler].data.is_type(card_type::PENDULUM) {
            return card_type::PENDULUM + card_type::SPELL;
        }
        self.get_type(handler, None, 0, PLAYER_NONE)
    }

    /// `get_value(card*, ...)` — an effect's value, asked about a card.
    pub(crate) fn effect_value_for_card_pub(&self, id: EffectId, card: CardId) -> i64 {
        self.effect_value_for_card(id, card)
    }

    fn effect_value_for_card(&self, id: EffectId, card: CardId) -> i64 {
        let Some(e) = self.effects.get(id) else {
            return 0;
        };
        let ev = Event::new(0);
        let ctx = Ctx {
            reason_effect: id,
            player: self.cards[card].current.controller,
            event: &ev,
            card: Some(card),
            args: &[],
        };
        e.get_value(self, &ctx)
    }

    /// `effect::is_activateable` — may this effect be put onto a chain?
    ///
    /// The reference's structure is kept exactly: an outer `FIELD_ONLY`
    /// test, then within it three mutually exclusive cases — an activate
    /// effect, a continuous one (which falls through *both* inner branches
    /// and is tested by neither), and everything else. The branches share no
    /// tests, and each would be wrong with another's.
    ///
    /// The `neglect_*` parameters are the reference's, in its order. They
    /// are how the engine asks narrower versions of the same question at
    /// different points, and they keep their names so a call site here reads
    /// against a call site there.
    #[allow(clippy::too_many_arguments)]
    pub fn is_activateable(
        &mut self,
        id: EffectId,
        playerid: u8,
        event: &Event,
        neglect_cond: bool,
        neglect_cost: bool,
        neglect_target: bool,
        neglect_loc: bool,
        neglect_faceup: bool,
    ) -> bool {
        let Some(e) = self.effects.get(id) else {
            return false;
        };
        if !e.is_type(effect_type::ACTIONS) {
            return false;
        }
        if !self.check_count_limit(id, playerid) {
            return false;
        }

        let e = self.effects.get(id).unwrap();
        let is_field_only = e.is_flag(flag::FIELD_ONLY);
        let is_activate = e.is_type(effect_type::ACTIVATE);
        let is_continuous = e.is_type(effect_type::CONTINUOUS);

        if is_field_only {
            let e = self.effects.get(id).unwrap();
            if e.get_owner_player(&self.cards) != playerid && !e.is_flag(flag::BOTH_SIDE) {
                return false;
            }
        } else if is_activate {
            if !self.activate_branch(id, playerid, event, neglect_loc) {
                return false;
            }
        } else if !is_continuous {
            if !self.trigger_branch(id, playerid, neglect_faceup) {
                return false;
            }
        } else if !self.continuous_branch(id) {
            return false;
        }

        // A continuous effect has no action check: it is not an activation a
        // player chooses to make, so there is nothing to forbid.
        if !is_continuous && !self.is_action_check(id, playerid) {
            return false;
        }
        self.is_activate_ready(
            id,
            id,
            playerid,
            event,
            neglect_cond,
            neglect_cost,
            neglect_target,
        )
    }

    /// The **continuous** branch of `is_activateable` — the third arm of
    /// the reference's `if/else if/else` (`effect.cpp:268`).
    ///
    /// This was missing entirely, and nothing noticed until Reaper on the
    /// Nightmare: it has three `EFFECT_TYPE_FIELD + EFFECT_TYPE_CONTINUOUS`
    /// effects whose handler can be **flipped face-down** while they are
    /// registered. Without the face-up test the port keeps running them,
    /// so a Reaper that Book of Moon turned face-down still destroys
    /// itself for having been targeted — and the harness says so, at line
    /// 860 of that shape.
    ///
    /// The other three tests here have had no card to bite on yet. They
    /// are transcribed anyway, for the reason the port always gives:
    /// getting one of a group of sibling checks wrong in the one branch
    /// nothing exercises is exactly what survives review.
    fn continuous_branch(&mut self, id: EffectId) -> bool {
        let Some(e) = self.effects.get(id) else {
            return false;
        };
        let (Some(h), Some(o)) = (e.get_handler(&self.cards), e.get_owner(&self.cards)) else {
            return false;
        };
        let is_field = e.is_type(effect_type::FIELD);
        let single_range = e.is_type(effect_type::SINGLE) && e.is_flag(flag::SINGLE_RANGE);
        let hc = &self.cards[h];

        if is_field && hc.is_status(status::BATTLE_DESTROYED) {
            return false;
        }
        // **A face-down card projects nothing.** `EFFECT_ENABLED` is the
        // same question asked the other way — `enable_field_effect(false)`
        // clears it rather than unregistering the effect, so the index
        // still holds an effect that must not fire.
        if (is_field || single_range)
            && hc.current.is_location(u16::from(location::ONFIELD))
            && (!hc.current.is_faceup() || !hc.is_status(status::EFFECT_ENABLED))
        {
            return false;
        }
        if single_range && !e.in_range(&self.cards, hc) {
            return false;
        }
        let oc = &self.cards[o];
        Self::passes_owner_gates(e, hc, oc, h, o)
    }

    /// The `EFFECT_TYPE_ACTIVATE` branch of `is_activateable`.
    fn activate_branch(
        &mut self,
        id: EffectId,
        playerid: u8,
        event: &Event,
        neglect_loc: bool,
    ) -> bool {
        let e = self.effects.get(id).unwrap();
        let Some(handler) = e.handler else {
            return false;
        };
        let both_side = e.is_flag(flag::BOTH_SIDE);
        let limit_zone = e.is_flag(flag::LIMIT_ZONE);
        let e_code = e.code;
        let damage_step_ok = e.is_flag(flag::DAMAGE_STEP);
        let damage_cal_ok = e.is_flag(flag::DAMAGE_CAL);
        let value = e.value;

        if self.cards[handler].current.controller != playerid && !both_side {
            return false;
        }
        if self
            .check_unique_onfield(handler, playerid, u16::from(location::SZONE), None)
            .is_some()
        {
            return false;
        }

        // Printed type throughout this branch, never the resolved one: a
        // type-changing effect must not make a trap activatable from the
        // hand, nor a non-counter card usable during the Damage Step.
        let printed = self.cards[handler].data.type_;

        if printed & card_type::COUNTER == 0 {
            // Counter traps aside, the Damage Step is closed. The code
            // ranges are the handful of effects that are exempt by number;
            // `get_cteffect` is the other way through, for a continuous trap
            // whose activation does nothing on its own.
            let phase = self.infos.phase;
            if !(1132..=1149).contains(&e_code)
                && phase == phases::DAMAGE
                && !damage_step_ok
                && !self.get_cteffect(id, playerid)
            {
                return false;
            }
            if !(1134..=1136).contains(&e_code)
                && phase == phases::DAMAGE_CAL
                && !damage_cal_ok
                && !self.get_cteffect(id, playerid)
            {
                return false;
            }
        }

        // The zone mask an effect may restrict itself to, computed from its
        // value with the event's seven fields as arguments.
        let mut zone = 0xffu32;
        if printed & (card_type::FIELD | card_type::PENDULUM) == 0 && limit_zone {
            zone = self.effect_zone_value(id, playerid, event) as u32;
            if zone == 0 {
                return false;
            }
        }

        let c = &self.cards[handler];
        if c.current.location == location::SZONE {
            if c.current.is_faceup() {
                return false;
            }
            if c.equiping_target.is_some() {
                return false;
            }
            if printed & (card_type::FIELD | card_type::PENDULUM) == 0
                && limit_zone
                && zone & (1u32 << c.current.sequence) == 0
            {
                return false;
            }
        } else {
            // Activating from anywhere else needs a seat, unless the card is
            // a field spell, or its value names the field zone or the hand.
            let value_loc = value as u32;
            let needs_no_seat = (printed & card_type::FIELD != 0 && !limit_zone && value <= 0)
                || (!limit_zone && value_loc & u32::from(location::FZONE) != 0)
                || (!limit_zone && value_loc & u32::from(location::HAND) != 0);
            if !needs_no_seat {
                if !limit_zone && value_loc & u32::from(location::MZONE) != 0 {
                    if self.get_useable_count(
                        Some(handler),
                        playerid,
                        location::MZONE,
                        playerid,
                        Self::LOCATION_REASON_TOFIELD,
                        0xff,
                    ) <= 0
                    {
                        return false;
                    }
                } else if printed & card_type::PENDULUM != 0
                    || (!limit_zone && value_loc & u32::from(location::PZONE) != 0)
                {
                    if !self.is_location_useable(playerid as usize, location::PZONE, 0)
                        && !self.is_location_useable(playerid as usize, location::PZONE, 1)
                    {
                        return false;
                    }
                } else if self.get_useable_count(
                    Some(handler),
                    playerid,
                    location::SZONE,
                    playerid,
                    Self::LOCATION_REASON_TOFIELD,
                    zone,
                ) <= 0
                {
                    return false;
                }
            }
        }

        if !self.activation_from_here_is_allowed(handler, playerid, neglect_loc) {
            return false;
        }
        if self.cards[handler].is_status(status::FORBIDDEN) {
            return false;
        }
        self.is_affected_by_effect(handler, code::CANNOT_TRIGGER)
            .is_none()
    }

    /// The "may it be activated from *there*" question: a trap in the hand,
    /// a quick-play on the opponent's turn, or either in the turn it was
    /// set, each needs a permitting effect that still has a use left.
    ///
    /// One case is a flat no rather than a permission question: an ordinary
    /// Spell in the hand on the opponent's turn. There is no effect that
    /// permits it, so the reference returns rather than looking for one.
    ///
    /// `neglect_loc` guards **only** the hand case. The reference spells it
    /// `if(location == LOCATION_HAND && !neglect_loc) ... else if(location
    /// == LOCATION_SZONE)`, so neglecting the location skips the hand
    /// permission and leaves the set-turn permission in force. Hoisting the
    /// flag to cover both — which is what it looks like it should do — lets
    /// a card activate in the turn it was set without permission.
    fn activation_from_here_is_allowed(
        &self,
        handler: CardId,
        playerid: u8,
        neglect_loc: bool,
    ) -> bool {
        let c = &self.cards[handler];
        let printed = c.data.type_;
        let quickplay = printed & card_type::QUICKPLAY != 0
            || self
                .is_affected_by_effect(handler, code::BECOME_QUICK)
                .is_some();

        let permit = if c.current.location == location::HAND && !neglect_loc {
            if printed & card_type::TRAP != 0 {
                Some(code::TRAP_ACT_IN_HAND)
            } else if printed & card_type::SPELL != 0 && self.infos.turn_player != playerid {
                if quickplay {
                    Some(code::QP_ACT_IN_NTPHAND)
                } else {
                    return false;
                }
            } else {
                None
            }
        } else if c.current.location == location::SZONE && c.is_status(status::SET_TURN) {
            // Not `else if`: the reference tests both, so a card that is
            // somehow both keeps the later answer.
            let mut permit = None;
            if printed & card_type::TRAP != 0 {
                permit = Some(code::TRAP_ACT_IN_SET_TURN);
            }
            if printed & card_type::SPELL != 0 && quickplay {
                permit = Some(code::QP_ACT_IN_SET_TURN);
            }
            permit
        } else {
            None
        };

        let Some(permit) = permit else {
            return true;
        };
        self.filter_effect(handler, permit)
            .into_iter()
            .any(|p| self.check_count_limit(p, playerid))
    }

    /// The branch for a triggered or quick effect — everything that is
    /// neither an activate effect nor a continuous one.
    fn trigger_branch(&mut self, id: EffectId, playerid: u8, neglect_faceup: bool) -> bool {
        let e = self.effects.get(id).unwrap();
        let Some(handler) = e.get_handler(&self.cards) else {
            return false;
        };
        let e_type = e.effect_type;
        let e_code = e.code;
        let continuous_equip = e.is_flag2(flag2::CONTINUOUS_EQUIP);
        let set_available = e.is_flag(flag::SET_AVAILABLE);
        let damage_step_ok = e.is_flag(flag::DAMAGE_STEP);
        let damage_cal_ok = e.is_flag(flag::DAMAGE_CAL);
        let both_or_event = e.is_flag(flag::BOTH_SIDE | flag::EVENT_PLAYER);
        let range = e.range;
        let effect_owner = e.effect_owner;

        // A monster effect on a card that is no longer a monster. Under
        // `TRIGGER_WHEN_PRIVATE_KNOWLEDGE` this test is off; that flag is
        // not in this project's configuration, so the test applies.
        if !self.is_flag(flags::TRIGGER_WHEN_PRIVATE_KNOWLEDGE) {
            let now = self.get_type(handler, None, 0, PLAYER_NONE);
            let active = self.get_active_type(id);
            if now & card_type::MONSTER == 0 && active & card_type::MONSTER != 0 {
                return false;
            }
        }

        // A continuous *card* that is also an equip is not a trigger
        // source. The reference asks `get_type()` here — the *resolved*
        // type — so a card made continuous by an effect counts. Contrast the
        // activate branch, which asks the printed type throughout; the two
        // branches genuinely differ and neither is a slip.
        if !continuous_equip {
            let now = self.get_type(handler, None, 0, PLAYER_NONE);
            if now & card_type::CONTINUOUS != 0 && now & card_type::EQUIP != 0 {
                return false;
            }
        }
        let c = &self.cards[handler];
        if !neglect_faceup
            && c.current
                .is_location(u16::from(location::ONFIELD | location::REMOVED))
        {
            if !c.current.is_faceup() && !set_available {
                return false;
            }
            if c.current.is_faceup() && !c.is_status(status::EFFECT_ENABLED) {
                return false;
            }
        }

        // Flip and mandatory-trigger effects are exempt from the Damage Step
        // closure, as is a single-card optional trigger.
        let exempt = e_type & (effect_type::FLIP | effect_type::TRIGGER_F) != 0
            || (e_type & effect_type::TRIGGER_O != 0 && e_type & effect_type::SINGLE != 0);
        if !exempt {
            let phase = self.infos.phase;
            if !(1132..=1149).contains(&e_code) && phase == phases::DAMAGE && !damage_step_ok {
                return false;
            }
            if !(1134..=1136).contains(&e_code) && phase == phases::DAMAGE_CAL && !damage_cal_ok {
                return false;
            }
        }

        let c = &self.cards[handler];
        if c.current.location == location::OVERLAY {
            return false;
        }
        if e_type & effect_type::FIELD != 0 && c.current.controller != playerid && !both_or_event {
            return false;
        }
        // A card in the deck, or face-down in the extra deck, cannot trigger
        // — unless the duel says otherwise, or the effect is ranged there.
        if !self.is_flag(flags::TRIGGER_WHEN_PRIVATE_KNOWLEDGE)
            && !self.is_flag(flags::RETURN_TO_DECK_TRIGGERS)
        {
            let c = &self.cards[handler];
            let hidden = c.current.location == location::DECK
                || (c.current.location == location::EXTRA
                    && c.current.position & position::FACEDOWN != 0);
            if hidden {
                if e_type & effect_type::SINGLE != 0 && e_code != code::TO_DECK {
                    return false;
                }
                if e_type & effect_type::FIELD != 0
                    && range & u16::from(location::DECK | location::EXTRA) == 0
                {
                    return false;
                }
            }
        }
        let _ = effect_owner;
        if self.cards[handler].is_status(status::FORBIDDEN) {
            return false;
        }
        self.is_affected_by_effect(handler, code::CANNOT_TRIGGER)
            .is_none()
    }

    /// An effect's `value` read as a zone mask, with the event's seven
    /// fields as the extra arguments the reference pushes.
    fn effect_zone_value(&self, id: EffectId, playerid: u8, event: &Event) -> i64 {
        let Some(e) = self.effects.get(id) else {
            return 0;
        };
        // `get_value(7)`: the reference pushes seven values before the
        // call — the player and six fields of the event. Two of those are
        // handles rather than integers (the event's card group and its
        // reason effect) and have no integer form here; they are passed as 0
        // and noted, because a value function that reads one would need a
        // richer Ctx than any card in this pool asks for.
        let args = [
            i64::from(playerid),
            0, // event_cards: a group
            i64::from(event.event_player),
            i64::from(event.event_value),
            0, // reason_effect: a handle
            i64::from(event.reason),
            i64::from(event.reason_player),
        ];
        let ctx = Ctx {
            reason_effect: id,
            player: playerid,
            event,
            card: None,
            args: &args,
        };
        e.get_value(self, &ctx)
    }

    /// `field::get_cteffect` — does this continuous trap have a triggered
    /// effect it could be chained for instead?
    ///
    /// A narrow question with a narrow subject. It applies only to a card
    /// printed exactly `TYPE_TRAP | TYPE_CONTINUOUS`, whose activate effect
    /// is a bare `EVENT_FREE_CHAIN` with no cost, target or operation —
    /// i.e. the activation does nothing on its own and exists only to put
    /// the card on the field. The question is then whether one of the
    /// card's *field* effects could be activated right now.
    ///
    /// It is why `is_activateable` lets such a card through during the
    /// Damage Step, which is otherwise closed to it.
    ///
    /// `store` collects the chains into `select_chains` rather than
    /// answering; this is the asking form, and the storing form arrives with
    /// the machinery that offers the choice.
    ///
    /// Note the mutual recursion with `is_activateable`, which is bounded:
    /// the effects examined here are TRIGGER/QUICK ones, and
    /// `is_activateable` only reaches back into `get_cteffect` from its
    /// ACTIVATE branch, so it cannot descend twice.
    pub fn get_cteffect(&mut self, id: EffectId, playerid: u8) -> bool {
        let phase_code = code::PHASE + u32::from(self.infos.phase);
        let Some(candidates) = self.cteffect_candidates(id) else {
            return false;
        };

        for (event_code, fid) in candidates {
            if event_code == code::FREE_CHAIN || event_code == phase_code {
                // The reference reuses its `nil_event`, setting only the
                // code — so every other field is whatever it was left as.
                // A fresh event is the same thing said honestly, since the
                // reference clears it on each duel step.
                let mut nil = Event::new(event_code);
                nil.event_player = PLAYER_NONE;
                if self.cteffect_evt(fid, playerid, &nil) {
                    return true;
                }
            } else {
                let events: Vec<Event> = self
                    .core
                    .point_event
                    .iter()
                    .chain(self.core.instant_event.iter())
                    .filter(|ev| ev.event_code == event_code)
                    .cloned()
                    .collect();
                for ev in events {
                    if self.cteffect_evt(fid, playerid, &ev) {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// The subject test and candidate scan both forms of `get_cteffect`
    /// share. `None` means this effect is not the narrow kind the question
    /// is about at all.
    pub(crate) fn cteffect_candidates_pub(&self, id: EffectId) -> Option<Vec<(u32, EffectId)>> {
        self.cteffect_candidates(id)
    }

    fn cteffect_candidates(&self, id: EffectId) -> Option<Vec<(u32, EffectId)>> {
        let e = self.effects.get(id)?;
        let handler = e.get_handler(&self.cards)?;
        // Printed exactly a continuous trap — equality, not a mask test.
        if self.cards[handler].data.type_ != (card_type::TRAP | card_type::CONTINUOUS) {
            return None;
        }
        if !e.is_type(effect_type::ACTIVATE) || e.code != code::FREE_CHAIN {
            return None;
        }
        if e.cost.is_some() || e.target.is_some() || e.operation.is_some() {
            return None;
        }
        Some(
            self.cards[handler]
                .field_effect
                .iter()
                .filter(|&&(_, fid)| {
                    self.effects.get(fid).is_some_and(|f| {
                        f.is_type(
                            effect_type::TRIGGER_F | effect_type::TRIGGER_O | effect_type::QUICK_O,
                        ) && f.in_range(&self.cards, &self.cards[handler])
                    })
                })
                .copied()
                .collect(),
        )
    }

    /// `field::get_cteffect_evt`, asking rather than storing.
    ///
    /// `neglect_faceup` is true: the card is face-down, since it is being
    /// activated from the set position, and the question is what it *would*
    /// be able to do once face-up.
    fn cteffect_evt(&mut self, id: EffectId, playerid: u8, event: &Event) -> bool {
        self.is_activateable(id, playerid, event, false, false, false, false, true)
    }

    /// `effect::dec_count` — spend one activation.
    ///
    /// Two independent things happen, and the conditions differ. The
    /// per-effect counter falls only for an effect with no *shared* tally,
    /// or one flagged `NO_TURN_RESET`; the shared tally is bumped only for
    /// an effect that has one. An effect can therefore do both, either, or
    /// neither.
    pub fn dec_count(&mut self, id: EffectId, playerid: u8) {
        let Some(e) = self.effects.get(id) else {
            return;
        };
        if !e.has_count_limit() || e.count_limit == 0 {
            return;
        }
        let (count_code, count_flag, hopt) = (e.count_code, e.count_flag, e.count_hopt_index);
        let no_turn_reset = e.is_flag(flag::NO_TURN_RESET);
        let handler = e.handler;

        if (count_code == 0 && count_flag == 0) || no_turn_reset {
            if let Some(e) = self.effects.get_mut(id) {
                e.count_limit -= 1;
            }
        }
        if count_code != 0 || count_flag != 0 {
            if count_flag & effect_count::SINGLE != 0 {
                let fieldid = handler.map_or(0, |h| self.cards[h].fieldid);
                self.add_effect_code(fieldid, count_flag, hopt, PLAYER_NONE);
            } else {
                self.add_effect_code(count_code, count_flag, hopt, playerid);
            }
        }
    }

    /// `effect::set_active_type` — remember what the handler counted as.
    /// A trap monster stops counting as a trap.
    pub fn set_active_type(&mut self, id: EffectId) {
        let Some(handler) = self
            .effects
            .get(id)
            .and_then(|e| e.get_handler(&self.cards))
        else {
            return;
        };
        let mut t = self.get_type(handler, None, 0, PLAYER_NONE);
        if t & card_type::TRAPMONSTER != 0 {
            t &= !card_type::TRAP;
        }
        if let Some(e) = self.effects.get_mut(id) {
            e.active_type = t;
        }
    }

    /// Which permission an activation from this card's location needs, if
    /// any. The shape of `is_activateable`'s `ecode` block, reused by
    /// `AddChain`'s subroutine.
    pub fn permission_code_for(&self, handler: CardId) -> Option<u32> {
        let c = &self.cards[handler];
        let printed = c.data.type_;
        let quickplay = printed & card_type::QUICKPLAY != 0
            || self
                .is_affected_by_effect(handler, code::BECOME_QUICK)
                .is_some();
        if c.current.location == location::HAND {
            if printed & card_type::TRAP != 0 {
                return Some(code::TRAP_ACT_IN_HAND);
            }
            if printed & card_type::SPELL != 0
                && quickplay
                && self.infos.turn_player != c.current.controller
            {
                return Some(code::QP_ACT_IN_NTPHAND);
            }
            return None;
        }
        if c.current.location == location::SZONE && c.is_status(status::SET_TURN) {
            let mut permit = None;
            if printed & card_type::TRAP != 0 {
                permit = Some(code::TRAP_ACT_IN_SET_TURN);
            }
            if printed & card_type::SPELL != 0 && quickplay {
                permit = Some(code::QP_ACT_IN_SET_TURN);
            }
            return permit;
        }
        None
    }

    /// The permitting effect's value is read for its side effect — the
    /// reference calls `eff->get_value(phandler, 1)` after pushing the
    /// activating effect, and discards the result.
    pub fn consume_permission(&mut self, permit: EffectId, activating: EffectId, handler: CardId) {
        let Some(e) = self.effects.get(permit) else {
            return;
        };
        let ev = Event::new(0);
        let ctx = Ctx {
            reason_effect: activating,
            player: self.cards[handler].current.controller,
            event: &ev,
            card: Some(handler),
            args: &[],
        };
        let _ = e.get_value(self, &ctx);
    }

    /// `EFFECT_DISABLE_EFFECT`, applied to a chain before its cost is paid.
    ///
    /// A card disabled *now* must not pay for an effect that will be negated
    /// anyway, which is why the reference does this at step 2 and says so.
    pub fn apply_disable_chain(&mut self, effect: EffectId, handler: CardId, chain_id: u16) {
        let field_only = self
            .effects
            .get(effect)
            .is_some_and(|e| e.is_flag(flag::FIELD_ONLY));
        if field_only || !self.cards[handler].has_chain_relation(effect, chain_id) {
            return;
        }
        let Some(disabler) = self.is_affected_by_effect(handler, code::DISABLE_EFFECT) else {
            return;
        };
        let owner = self.effects.get(disabler).and_then(|e| e.owner);
        let mut negate = Effect::new(effect_type::SINGLE, code::DISABLE_CHAIN);
        negate.owner = owner;
        negate.value = i64::from(chain_id);
        let negate = self.new_effect(negate);
        self.cards[handler]
            .single_effect
            .insert(code::DISABLE_CHAIN, negate);
    }

    /// A spell or trap that stays on the field once activated, and is
    /// still there and still related to this chain.
    pub fn is_continuous_card_on_field(&mut self, effect: EffectId, handler: CardId) -> bool {
        let t = self.get_type(handler, None, 0, PLAYER_NONE);
        if t & (card_type::SPELL | card_type::TRAP) == 0 {
            return false;
        }
        if t & (card_type::CONTINUOUS
            | card_type::FIELD
            | card_type::EQUIP
            | card_type::PENDULUM
            | card_type::LINK)
            == 0
        {
            return false;
        }
        let field_only = self
            .effects
            .get(effect)
            .is_some_and(|e| e.is_flag(flag::FIELD_ONLY));
        !field_only
            && self.cards[handler].current.location == location::SZONE
            && self.cards[handler].has_effect_relation(effect)
    }

    /// `field::get_cteffect(..., store = TRUE)` — the storing form.
    ///
    /// Where the asking form answers yes/no, this one fills `select_chains`
    /// and `select_options` with every choice and answers "was there any".
    /// It does **not** stop at the first: `AddChain` needs the whole list to
    /// offer.
    pub fn get_cteffect_storing(&mut self, id: EffectId, playerid: u8) -> bool {
        self.core.select_chains.clear();
        self.core.select_options.clear();
        let Some(candidates) = self.cteffect_candidates(id) else {
            return false;
        };
        let phase_code = code::PHASE + u32::from(self.infos.phase);
        for (event_code, fid) in candidates {
            let events: Vec<Event> = if event_code == code::FREE_CHAIN || event_code == phase_code {
                vec![Event::new(event_code)]
            } else {
                self.core
                    .point_event
                    .iter()
                    .chain(self.core.instant_event.iter())
                    .filter(|ev| ev.event_code == event_code)
                    .cloned()
                    .collect()
            };
            for ev in events {
                if self.is_activateable(fid, playerid, &ev, false, false, false, false, true) {
                    let description = self.effects.get(fid).map_or(0, |e| e.description);
                    let mut chain = Chain::new(fid, ev);
                    chain.triggering_player = playerid;
                    self.core.select_chains.push_back(chain);
                    self.core.select_options.push(description);
                }
            }
        }
        !self.core.select_chains.is_empty()
    }

    /// `field::solve_continuous` — queue a continuous effect and ask the
    /// processor to resolve it.
    pub fn solve_continuous(&mut self, playerid: u8, effect: EffectId, event: Event) {
        let mut chain = Chain::new(effect, event);
        chain.chain_id = 0;
        chain.chain_count = 0;
        chain.triggering_player = playerid;
        self.core.sub_solving_continuous.push_back(chain);
        self.emplace(Kind::SolveContinuous {
            state: Default::default(),
        });
    }

    /// `check_simul` — did this effect's handler leave the field in the very
    /// batch it is responding to?
    ///
    /// Only effects flagged `CHECK_SIMULTANEOUS` care. Those that do, and
    /// whose handler was just sent, are skipped — unless the effect is
    /// ranged to the hand, which is the one place a just-sent card can still
    /// act from.
    fn check_simul(&self, effect: EffectId, handler: CardId) -> bool {
        self.effects
            .get(effect)
            .is_some_and(|e| e.is_flag2(flag2::CHECK_SIMULTANEOUS))
            && self.core.just_sent_cards.contains(&handler)
    }

    /// Which list a continuous effect's chain goes on, given whose it is.
    ///
    /// A `DELAY` effect raised while a chain is already solving is held back
    /// rather than solved at once — that is the whole mechanism behind
    /// "after this resolves".
    fn continuous_destination(&self, effect: EffectId, owner: u8) -> ContinuousDest {
        let delayed = self
            .effects
            .get(effect)
            .is_some_and(|e| e.is_flag(flag::DELAY));
        if delayed && (self.core.chain_solving || self.core.conti_solving) {
            if owner == self.infos.turn_player {
                ContinuousDest::DelayedTp
            } else {
                ContinuousDest::DelayedNtp
            }
        } else if owner == self.infos.turn_player {
            ContinuousDest::Tp
        } else {
            ContinuousDest::Ntp
        }
    }

    /// `EFFECT_FLAG_EVENT_PLAYER`, applied: whose effect this counts as.
    fn acting_player(&self, effect: EffectId, event: &Event, default: u8) -> u8 {
        let by_event = self
            .effects
            .get(effect)
            .is_some_and(|e| e.is_flag(flag::EVENT_PLAYER));
        if by_event && (event.event_player == 0 || event.event_player == 1) {
            event.event_player
        } else {
            default
        }
    }

    /// `field::process_instant_event` — build chains from everything that
    /// has been raised, then hand the events on.
    ///
    /// The single most important thing about it: it runs over the **whole
    /// queue**, not over one event. Effects gathered from different events
    /// in the same batch end up on the same lists, which is what makes them
    /// simultaneous. Gathering at each `raise_event` instead would be
    /// simpler, would pass any test that raises one event, and would get
    /// SEGOC wrong everywhere.
    ///
    /// The queue is taken out at the top rather than iterated in place.
    /// Rust would not allow iterating it while pushing chains, and the
    /// reference ends by splicing it onto `instant_event` and leaving it
    /// empty — so taking it is what the function does anyway, said earlier.
    /// A cleared effect-id buffer from the pool (or a new one).
    pub(crate) fn take_effects(&mut self) -> Vec<EffectId> {
        self.core.scratch_effects.pop().unwrap_or_default()
    }
    pub(crate) fn give_effects(&mut self, mut v: Vec<EffectId>) {
        v.clear();
        self.core.scratch_effects.push(v);
    }
    pub(crate) fn take_events(&mut self) -> Vec<Event> {
        self.core.scratch_events.pop().unwrap_or_default()
    }
    pub(crate) fn give_events(&mut self, mut v: Vec<Event>) {
        v.clear();
        self.core.scratch_events.push(v);
    }
    pub(crate) fn take_delayed(&mut self) -> Vec<(EffectId, Event)> {
        self.core.scratch_delayed.pop().unwrap_or_default()
    }
    pub(crate) fn give_delayed(&mut self, mut v: Vec<(EffectId, Event)>) {
        v.clear();
        self.core.scratch_delayed.push(v);
    }

    pub fn process_instant_event(&mut self) {
        if self.core.queue_event.is_empty() {
            return;
        }
        // The queue is taken whole, as the deque it is: what the loop raises
        // goes into the fresh queue and waits for the next call, as before.
        let events = mem::take(&mut self.core.queue_event);
        let mut tp: ChainList = ChainList::new();
        let mut ntp: ChainList = ChainList::new();

        for ev in &events {
            let mut ids = self.take_effects();
            ids.extend_from_slice(self.field_effects.continuous.equal_range(ev.event_code));
            for effect in ids.iter().copied() {
                let default = self
                    .effects
                    .get(effect)
                    .map_or(PLAYER_NONE, |e| e.get_handler_player(&self.cards));
                let owner = self.acting_player(effect, ev, default);
                if !self.is_activateable(effect, owner, ev, false, false, false, false, false) {
                    continue;
                }
                let mut chain = Chain::new(effect, ev.clone());
                chain.chain_id = 0;
                chain.chain_count = 0;
                chain.triggering_player = owner;
                match self.continuous_destination(effect, owner) {
                    ContinuousDest::DelayedTp => self.core.delayed_continuous_tp.push_back(chain),
                    ContinuousDest::DelayedNtp => self.core.delayed_continuous_ntp.push_back(chain),
                    ContinuousDest::Tp => tp.push_back(chain),
                    ContinuousDest::Ntp => ntp.push_back(chain),
                }
            }
            self.give_effects(ids);

            // Three event kinds raise continuous effects but no triggers.
            if ev.event_code == code::ADJUST
                || ev.event_code == code::BREAK_EFFECT
                || ev.event_code & code::PHASE_MASK == code::PHASE_START
            {
                continue;
            }

            self.gather_triggers(ev, true);
            self.gather_triggers(ev, false);
            self.gather_quick_f(ev);

            self.core.delayed_activate_event.push_back(ev.clone());

            let mut ids = self.take_effects();
            ids.extend_from_slice(self.field_effects.quick_o.equal_range(ev.event_code));
            for effect in ids.iter().copied() {
                let Some(handler) = self
                    .effects
                    .get(effect)
                    .and_then(|e| e.get_handler(&self.cards))
                else {
                    continue;
                };
                let delayed = self
                    .effects
                    .get(effect)
                    .is_some_and(|e| e.is_flag(flag::DELAY));
                let controller = self.cards[handler].current.controller;
                if delayed && self.is_condition_check(effect, controller, ev) {
                    self.core.delayed_quick_tmp.insert((effect, ev.clone()));
                }
            }
            self.give_effects(ids);
        }

        // Turn player's continuous effects first, then the opponent's, each
        // emplaced as its own unit so they resolve one at a time.
        while let Some(chain) = tp.pop_front() {
            self.core.sub_solving_continuous.push_back(chain);
            self.emplace(Kind::SolveContinuous {
                state: Default::default(),
            });
        }
        while let Some(chain) = ntp.pop_front() {
            self.core.sub_solving_continuous.push_back(chain);
            self.emplace(Kind::SolveContinuous {
                state: Default::default(),
            });
        }

        self.core.instant_event.extend(events);
    }

    /// The mandatory and optional trigger halves of the gather.
    ///
    /// They are nearly the same and differ in three ways, each of which
    /// matters: the optional one gathers a chain even when the condition
    /// *fails*, provided the effect is ranged to the hand; it therefore has
    /// to decide separately whether to create the card relation; and it uses
    /// a different list.
    ///
    /// Every site reads `get_handler()` rather than `handler`, so that an
    /// Xyz material's effect reports the monster it is under.
    fn gather_triggers(&mut self, ev: &Event, mandatory: bool) {
        let index = if mandatory {
            &self.field_effects.trigger_f
        } else {
            &self.field_effects.trigger_o
        };
        for effect in index.equal_range(ev.event_code).to_vec() {
            let Some(handler) = self
                .effects
                .get(effect)
                .and_then(|e| e.get_handler(&self.cards))
            else {
                continue;
            };
            let controller = self.cards[handler].current.controller;
            let enabled = self.cards[handler].is_status(status::EFFECT_ENABLED);
            let condition_ok = enabled && self.is_condition_check(effect, controller, ev);
            let ranged_to_hand = self
                .effects
                .get(effect)
                .is_some_and(|e| e.range & u16::from(location::HAND) != 0);

            let was_just_sent = self.check_simul(effect, handler);
            if was_just_sent && !ranged_to_hand {
                continue;
            }
            if mandatory {
                if !condition_ok {
                    continue;
                }
            } else if !ranged_to_hand && !condition_ok {
                // An optional trigger ranged to the hand is gathered even
                // when its condition fails — the card may be revealed from
                // the hand and the offer made anyway. Anything else needs
                // its condition to hold.
                continue;
            }

            let card = self.cards[handler].state();
            if let Some(e) = self.effects.get_mut(effect) {
                e.set_activate_location(&card);
            }
            let mut chain = self.build_chain(effect, handler, ev.clone());
            chain.was_just_sent = was_just_sent;
            chain.event_id = ev.global_id;

            let relate = if mandatory {
                true
            } else {
                // The optional branch relates the handler only when the
                // effect is field-only, or is not a hand effect, or is a
                // hand effect that is actually in range and whose condition
                // held. A hand trigger gathered on a failed condition is
                // deliberately left unrelated.
                let e = self.effects.get(effect).unwrap();
                let in_range_now = e.range & u16::from(self.cards[handler].current.location) != 0;
                e.is_flag(flag::FIELD_ONLY) || !ranged_to_hand || (in_range_now && condition_ok)
            };
            if relate {
                let chain_id = chain.chain_id;
                self.cards[handler].create_chain_relation(effect, chain_id);
            }

            // Always the main lists here. The `_b` diversion belongs to
            // the single-event path alone — `process_instant_event` has no
            // `flip_delayed` test, and adding one would hold back field
            // triggers that the reference offers immediately.
            let list = if mandatory {
                &mut self.core.new_fchain
            } else {
                &mut self.core.new_ochain
            };
            list.push_back(chain);
        }
    }

    /// Mandatory quick effects. One chain per effect, replacing any earlier
    /// one for the same effect — the reference indexes by effect, so a
    /// second event in the same batch overwrites rather than adds.
    fn gather_quick_f(&mut self, ev: &Event) {
        for effect in self
            .field_effects
            .quick_f
            .equal_range(ev.event_code)
            .to_vec()
        {
            let Some(handler) = self
                .effects
                .get(effect)
                .and_then(|e| e.get_handler(&self.cards))
            else {
                continue;
            };
            let card = self.cards[handler].state();
            if let Some(e) = self.effects.get_mut(effect) {
                e.set_activate_location(&card);
            }
            let controller = self.cards[handler].current.controller;
            if !self.is_activateable(effect, controller, ev, false, false, false, false, false) {
                continue;
            }
            let chain = self.build_chain(effect, handler, ev.clone());
            let chain_id = chain.chain_id;
            self.cards[handler].create_chain_relation(effect, chain_id);
            self.core.quick_f_chain.insert(effect, chain);
        }
    }

    /// `field::process_single_event` — events aimed at one card, gathered
    /// against *that card's* effects.
    ///
    /// A separate path rather than a stage of the same one: a flip effect
    /// belongs to the card that flipped, and asking the field about it would
    /// find every card's flip effect.
    pub fn process_single_event(&mut self) {
        if self.core.single_event.is_empty() {
            return;
        }
        let events: Vec<Event> = mem::take(&mut self.core.single_event).into();
        let mut tp: ChainList = ChainList::new();
        let mut ntp: ChainList = ChainList::new();

        for ev in &events {
            let Some(target) = ev.trigger_card else {
                continue;
            };
            for effect in self.cards[target]
                .single_effect
                .equal_range(ev.event_code)
                .to_vec()
            {
                self.single_event_for(effect, ev, &mut tp, &mut ntp);
            }
            // And from its Xyz materials, excluding field effects.
            for material in self.cards[target].xyz_materials.clone() {
                for effect in self.cards[material]
                    .xmaterial_effect
                    .equal_range(ev.event_code)
                    .to_vec()
                {
                    if self
                        .effects
                        .get(effect)
                        .is_some_and(|e| e.is_type(effect_type::FIELD))
                    {
                        continue;
                    }
                    self.single_event_for(effect, ev, &mut tp, &mut ntp);
                }
            }
        }

        while let Some(chain) = tp.pop_front() {
            self.core.sub_solving_continuous.push_back(chain);
            self.emplace(Kind::SolveContinuous {
                state: Default::default(),
            });
        }
        while let Some(chain) = ntp.pop_front() {
            self.core.sub_solving_continuous.push_back(chain);
            self.emplace(Kind::SolveContinuous {
                state: Default::default(),
            });
        }
        // Note: `single_event` is cleared rather than spliced onto
        // `instant_event`. A single event is not answerable by
        // `check_event` afterwards, which is a real difference from the
        // field path and not an oversight in the reference.
    }

    /// One single-event effect. `field::process_single_event(effect*, ...)`.
    fn single_event_for(
        &mut self,
        effect: EffectId,
        ev: &Event,
        tp: &mut ChainList,
        ntp: &mut ChainList,
    ) {
        let Some(e) = self.effects.get(effect) else {
            return;
        };
        if !e.is_type(effect_type::ACTIONS) {
            return;
        }
        // A flip effect is skipped when the event says the flip was not to
        // be treated as one. `NO_FLIP_EFFECT` lives in the high half of the
        // value, which is why the reference shifts it down.
        if e.is_type(effect_type::FLIP) && ev.event_value & (NO_FLIP_EFFECT >> 16) != 0 {
            return;
        }

        if e.is_type(effect_type::CONTINUOUS) {
            let default = e.get_handler_player(&self.cards);
            let owner = self.acting_player(effect, ev, default);
            if !self.is_activateable(effect, owner, ev, false, false, false, false, false) {
                return;
            }
            let mut chain = Chain::new(effect, ev.clone());
            chain.chain_id = 0;
            chain.chain_count = 0;
            chain.triggering_player = owner;
            match self.continuous_destination(effect, owner) {
                ContinuousDest::DelayedTp => self.core.delayed_continuous_tp.push_back(chain),
                ContinuousDest::DelayedNtp => self.core.delayed_continuous_ntp.push_back(chain),
                ContinuousDest::Tp => tp.push_back(chain),
                ContinuousDest::Ntp => ntp.push_back(chain),
            }
            return;
        }

        let Some(handler) = e.get_handler(&self.cards) else {
            return;
        };
        let controller = self.cards[handler].current.controller;
        if !self.is_condition_check(effect, controller, ev) {
            return;
        }
        let card = self.cards[handler].state();
        if let Some(e) = self.effects.get_mut(effect) {
            e.set_activate_location(&card);
        }

        let mut chain = self.build_chain(effect, handler, ev.clone());
        chain.event_id = ev.global_id;
        // A single event's triggering player is *not* chosen the same way as
        // a field event's. Without `EVENT_PLAYER` it is the snapshotted
        // controller — or, for a card that left the field only temporarily,
        // the controller it had *before* it left. A card returning from a
        // temporary absence is asked by the player who owned it.
        let by_event = self
            .effects
            .get(effect)
            .is_some_and(|x| x.is_flag(flag::EVENT_PLAYER));
        if !(by_event && (ev.event_player == 0 || ev.event_player == 1)) {
            chain.triggering_player = if self.cards[handler].reason & reason::TEMPORARY != 0 {
                self.cards[handler].previous.controller
            } else {
                chain.triggering_controler
            };
        }

        let chain_id = chain.chain_id;
        self.cards[handler].create_chain_relation(effect, chain_id);

        let is_optional = self
            .effects
            .get(effect)
            .is_some_and(|x| x.is_type(effect_type::TRIGGER_O));
        let list = if self.core.flip_delayed && ev.event_code == code::FLIP {
            if is_optional {
                &mut self.core.new_ochain_b
            } else {
                &mut self.core.new_fchain_b
            }
        } else if is_optional {
            &mut self.core.new_ochain
        } else {
            &mut self.core.new_fchain
        };
        list.push_back(chain);
    }

    /// `effect::is_action_check` — is the player forbidden from activating
    /// this, or unable to pay an activation cost the field imposes?
    ///
    /// Two field-wide questions, not one. `EFFECT_CANNOT_ACTIVATE` forbids
    /// outright; `EFFECT_ACTIVATE_COST` imposes an extra cost, and it is
    /// only a refusal when the cost's *target* admits the activation but its
    /// *cost* cannot be paid — an activation cost whose target says no
    /// simply does not apply here.
    pub fn is_action_check(&mut self, effect: EffectId, playerid: u8) -> bool {
        let ev = Event::new(0);
        for id in self.filter_player_effect(playerid, code::CANNOT_ACTIVATE) {
            let ctx = Ctx {
                reason_effect: effect,
                player: playerid,
                event: &ev,
                card: None,
                args: &[],
            };
            if self
                .effects
                .get(id)
                .is_some_and(|e| e.check_value_condition(self, &ctx))
            {
                return false;
            }
        }
        for id in self.filter_player_effect(playerid, code::ACTIVATE_COST) {
            let Some(e) = self.effects.get(id) else {
                continue;
            };
            let (target, cost) = (e.target_filter, e.cost);
            if let Some(target) = target {
                if !target(self, id, None, &[]) {
                    continue;
                }
            }
            if let Some(cost) = cost {
                let ctx = Ctx {
                    reason_effect: effect,
                    player: playerid,
                    event: &ev,
                    card: None,
                    args: &[],
                };
                if !self.with_reason_mut(id, playerid, |f| cost(f, &ctx, false)) {
                    return false;
                }
            }
        }
        true
    }

    /// `check_unique_onfield` — would putting this card here break a
    /// "only one face-up" limit? Returns the effect that forbids it.
    ///
    /// Two halves, and they ask different questions. The first asks whether
    /// some *other* card already on the field imposes a limit that covers
    /// this one. The second asks whether *this* card imposes a limit that its
    /// own arrival would break — which is why the count there is `>= 2` and
    /// includes the card itself.
    pub fn check_unique_onfield(
        &self,
        card: CardId,
        controller: u8,
        loc: u16,
        ignore: Option<CardId>,
    ) -> Option<EffectId> {
        for &unique in &self.core.unique_cards[controller as usize] {
            if unique == card || Some(unique) == ignore {
                continue;
            }
            let u = &self.cards[unique];
            if u.current.is_faceup()
                && u.is_status(status::EFFECT_ENABLED)
                && !u.get_status(status::DISABLED | status::FORBIDDEN)
                && u.unique_fieldid != 0
                && u.check_unique_code(self, card)
                && u.unique_location & loc != 0
            {
                return u.unique_effect;
            }
        }

        let c = &self.cards[card];
        if c.unique_code == 0
            || c.unique_location & loc == 0
            || c.get_status(status::DISABLED | status::FORBIDDEN)
        {
            return None;
        }
        let mut targets = self.unique_targets(card, controller, ignore);
        if c.check_unique_code(self, card) {
            targets.insert(card);
        }
        if targets.len() >= 2 {
            c.unique_effect
        } else {
            None
        }
    }

    /// `card::get_unique_target` — the face-up cards this card's uniqueness
    /// limit already covers.
    ///
    /// `unique_pos` is indexed by `controller ^ p`, so it says *which sides*
    /// the limit looks at rather than which player: a limit may be "one on
    /// your field" or "one anywhere".
    pub(crate) fn unique_targets(
        &self,
        card: CardId,
        controller: u8,
        ignore: Option<CardId>,
    ) -> BTreeSet<CardId> {
        let c = &self.cards[card];
        let mut found = BTreeSet::new();
        for p in 0u8..2 {
            if c.unique_pos[p as usize] == 0 {
                continue;
            }
            let zones = &self.players[(controller ^ p) as usize];
            if c.unique_location & u16::from(location::MZONE) != 0 {
                for slot in zones.mzone.iter().flatten() {
                    let other = &self.cards[*slot];
                    if Some(*slot) != ignore
                        && other.current.is_faceup()
                        && !other.is_status(status::SPSUMMON_STEP)
                        && c.check_unique_code(self, *slot)
                    {
                        found.insert(*slot);
                    }
                }
            }
            if c.unique_location & u16::from(location::SZONE) != 0 {
                for slot in zones.szone.iter().flatten() {
                    let other = &self.cards[*slot];
                    if Some(*slot) != ignore
                        && other.current.is_faceup()
                        && c.check_unique_code(self, *slot)
                    {
                        found.insert(*slot);
                    }
                }
            }
        }
        found
    }

    /// `get_mzone_limit` / `get_szone_limit` — how many more cards a player
    /// may have in a zone, after the effects that cap it have their say.
    ///
    /// `uplayer >= 2` means "ignore the capping effects", which is how the
    /// engine asks about the board itself rather than about a particular
    /// player's attempt to use it.
    pub fn get_mzone_limit(&self, playerid: u8, uplayer: u8, reason: u32) -> i32 {
        let used_flag = self.players[playerid as usize].used_location & 0x1f;
        let mut max = 5i32;
        let mut used = Self::field_used_count(used_flag) as i32;
        if self.is_flag(flags::EMZONE) {
            max = 7;
            for seat in 5..7 {
                if self.players[playerid as usize].mzone[seat].is_some() {
                    used += 1;
                }
            }
        }
        if uplayer < 2 {
            max = self.capped_at(max, code::MAX_MZONE, playerid, uplayer, reason);
        }
        max - used
    }

    pub fn get_szone_limit(&self, playerid: u8, uplayer: u8, reason: u32) -> i32 {
        let used_flag = (self.players[playerid as usize].used_location >> 8) & 0x1f;
        let mut max = 5i32;
        if uplayer < 2 {
            max = self.capped_at(max, code::MAX_SZONE, playerid, uplayer, reason);
        }
        max - Self::field_used_count(used_flag) as i32
    }

    /// The shared tail of the two limits: every capping effect lowers the
    /// maximum, and the lowest wins.
    fn capped_at(&self, mut max: i32, code: u32, playerid: u8, uplayer: u8, reason: u32) -> i32 {
        let ev = Event::new(0);
        for id in self.filter_player_effect(playerid, code) {
            let Some(e) = self.effects.get(id) else {
                continue;
            };
            let ctx = Ctx {
                reason_effect: id,
                player: uplayer,
                event: &ev,
                card: None,
                // `get_value(3)`: the reference pushes these three, in this
                // order, before calling.
                args: &[i64::from(playerid), i64::from(uplayer), i64::from(reason)],
            };
            let v = e.get_value(self, &ctx) as i32;
            if max > v {
                max = v;
            }
        }
        max
    }

    /// `LOCATION_REASON_TOFIELD` — the reason code the placement queries
    /// pass, and which reaches a capping effect's value function.
    pub const LOCATION_REASON_TOFIELD: u32 = 0x1;

    /// `LOCATION_REASON_CONTROL` — the reason code used when the move is a
    /// *change of controller* rather than an arrival. `MoveToField` picks
    /// between the two by where the card already is: a card leaving a
    /// Monster Zone is changing control, anything else is coming to the
    /// field.
    pub const LOCATION_REASON_CONTROL: u32 = 0x2;

    /// `get_tofield_count` — how many of the five main seats are free, after
    /// the caller's `zone` mask and any forced-zone effects.
    ///
    /// ## Three arguments this port had been dropping
    ///
    /// The reference takes `pcard`, `uplayer` and `reason` and hands all
    /// three to `get_forced_zones`. This port took none of them: it passed
    /// `None`, `playerid` and `LOCATION_REASON_TOFIELD` instead, which is
    /// right only when the caller happened to be asking about a card with no
    /// forcing effects of its own, on its own behalf, for an arrival.
    ///
    /// Each of the three changes the answer:
    ///
    /// - **`pcard`** brings the card's *own* `EFFECT_MUST_USE_MZONE` effects
    ///   into the mask; without it only the player's were read.
    /// - **`uplayer`** picks which half of the two-row packing
    ///   `get_forced_zones` returns — so a placement made on the opponent's
    ///   behalf was reading the wrong row.
    /// - **`reason`** reaches the forcing effect's own value function, which
    ///   is how an effect distinguishes an arrival from a change of control.
    ///
    /// `MoveToField` is the caller that had all three wrong at once.
    pub fn get_tofield_count(
        &mut self,
        card: Option<CardId>,
        playerid: u8,
        loc: u8,
        uplayer: u8,
        reason: u32,
        zone: u32,
    ) -> i32 {
        self.get_tofield_count_with_flag(card, playerid, loc, uplayer, reason, zone)
            .0
    }

    /// `get_tofield_count` with the reference's `list` out-parameter: the
    /// count *and* the mask of seats that are not available.
    ///
    /// Two details the count-only form can discard and this one cannot:
    ///
    /// - The mask is computed **before** the count and returned after, so it
    ///   is the same value the count was derived from.
    /// - For a Monster Zone the reference then sets bits 5 and 6 — the two
    ///   Extra Monster Zones — marking them unavailable *after* counting.
    ///   They are never a seat a card is placed into by this path, and
    ///   leaving them clear would offer a zone selection two seats that do
    ///   not exist for it.
    pub fn get_tofield_count_with_flag(
        &mut self,
        card: Option<CardId>,
        playerid: u8,
        loc: u8,
        uplayer: u8,
        reason: u32,
        zone: u32,
    ) -> (i32, u32) {
        if loc != location::MZONE && loc != location::SZONE {
            return (0, 0);
        }
        let blocked = self.players[playerid as usize].blocked();
        let mut flag = if loc == location::MZONE {
            let forced = self.get_forced_zones(card, playerid, loc, uplayer, reason);
            (blocked | !forced | !zone) & 0x1f
        } else {
            ((blocked >> 8) | !zone) & 0x1f
        };
        let count = 5 - Self::field_used_count(flag) as i32;
        if loc == location::MZONE {
            flag |= (1 << 5) | (1 << 6);
        }
        (count, flag)
    }

    /// `get_useable_count_other` — the free-seat count capped by the zone
    /// limit, which is the whole of `get_useable_count` for a card that is
    /// not in the Extra Deck.
    fn get_useable_count_other(
        &mut self,
        card: Option<CardId>,
        playerid: u8,
        loc: u8,
        uplayer: u8,
        reason: u32,
        zone: u32,
    ) -> (i32, u32) {
        let (count, mask) =
            self.get_tofield_count_with_flag(card, playerid, loc, uplayer, reason, zone);
        let limit = if loc == location::MZONE {
            self.get_mzone_limit(playerid, uplayer, reason)
        } else {
            self.get_szone_limit(playerid, uplayer, reason)
        };
        (count.min(limit), mask)
    }

    /// `get_useable_count_fromex` — the same question for a card **in the
    /// Extra Deck**, which is a different question once the Extra Monster
    /// Zone exists.
    ///
    /// Without `DUEL_EMZONE` — which this project masks off — it reduces to
    /// the ordinary count with the location and reason forced to
    /// `LOCATION_MZONE` and `LOCATION_REASON_TOFIELD`. That forcing is the
    /// only observable difference in this configuration, and it is a real
    /// one: a caller asking with `LOCATION_REASON_CONTROL` about an
    /// Extra-Deck card gets the arrival limit rather than the control one.
    ///
    /// The reference substitutes a scratch card when asked with none. Here
    /// `None` is passed straight through, which is equivalent: a scratch
    /// card carries no effects, so `get_forced_zones` finds the same nothing
    /// either way.
    pub fn get_useable_count_fromex(
        &mut self,
        card: Option<CardId>,
        playerid: u8,
        uplayer: u8,
        zone: u32,
    ) -> (i32, u32) {
        if self.is_flag(crate::duel::flags::EMZONE) {
            return self.useable_count_fromex_rule4();
        }
        self.get_useable_count_other(
            card,
            playerid,
            location::MZONE,
            uplayer,
            Self::LOCATION_REASON_TOFIELD,
            zone,
        )
    }

    /// Not ported: the Extra Monster Zone arm, which needs `check_extra_link`
    /// and `get_rule_zone_fromex`.
    fn useable_count_fromex_rule4(&mut self) -> (i32, u32) {
        unimplemented!("get_useable_count_fromex_rule4 needs the Extra Monster Zone")
    }

    /// `get_useable_count` with the seat mask alongside the count.
    #[allow(clippy::too_many_arguments)]
    /// `Duel.GetMZoneCount(playerid, without, uplayer, reason, zone)` —
    /// how many Monster Zone seats the player could use **if the named
    /// card were not there**.
    ///
    /// The reference does it by temporarily lying to the board: it swaps
    /// out both players' `used_location` and monster rows for copies with
    /// the card removed, asks the ordinary seat count, and swaps them
    /// back. Nothing else can produce the answer, because the count reads
    /// the row rather than taking a list.
    ///
    /// The lie is two-sided because an Extra Monster Zone seat is shared:
    /// removing a card from one row can free a seat in the other.
    ///
    /// The spell/trap half of `used_location` (`0xff00`) is carried across
    /// untouched — only the monster seats are being pretended away.
    pub fn get_mzone_count(
        &mut self,
        playerid: u8,
        without: Option<&[CardId]>,
        uplayer: u8,
        reason: u32,
        zone: u32,
    ) -> i32 {
        if playerid > 1 {
            return 0;
        }
        // `lua_get_card_or_group<true>`: nothing passed at all is not the
        // same as an **empty** group. Nothing passed skips the swap
        // entirely; an empty group still rebuilds `used_location` from the
        // rows, which can differ from what is recorded there.
        let Some(without) = without else {
            return self.get_useable_count(None, playerid, location::MZONE, uplayer, reason, zone);
        };
        self.with_rows_without(without, |f| {
            f.get_useable_count(None, playerid, location::MZONE, uplayer, reason, zone)
        })
    }

    /// Run `body` with both Monster Zone rows rebuilt as if `without`
    /// were not on the board, and put them back afterwards.
    ///
    /// The swap the reference performs by hand in `GetMZoneCount` and
    /// `GetLocationCountFromEx` alike: `used_location` is rebuilt from the
    /// remaining cards, the high half kept, and the rows exchanged for
    /// copies with the excluded cards removed.
    pub fn with_rows_without<T>(
        &mut self,
        without: &[CardId],
        body: impl FnOnce(&mut Self) -> T,
    ) -> T {
        let default_loc = 0x1111 * u32::from(self.is_flag(flags::THREE_COLUMNS_FIELD));
        let mut saved_used = [0u32; 2];
        let mut saved_rows: Vec<Vec<Option<CardId>>> = Vec::with_capacity(2);
        for (p, saved) in saved_used.iter_mut().enumerate() {
            let mut used = default_loc;
            let mut row: Vec<Option<CardId>> = Vec::with_capacity(self.players[p].mzone.len());
            for (i, slot) in self.players[p].mzone.iter().enumerate() {
                match *slot {
                    Some(c) if !without.contains(&c) => {
                        used |= 1 << i;
                        row.push(Some(c));
                    }
                    _ => row.push(None),
                }
            }
            used |= self.players[p].used_location & 0xff00;
            *saved = self.players[p].used_location;
            self.players[p].used_location = used;
            saved_rows.push(std::mem::replace(&mut self.players[p].mzone, row));
        }
        let out = body(self);
        for (p, row) in saved_rows.into_iter().enumerate() {
            self.players[p].used_location = saved_used[p];
            self.players[p].mzone = row;
        }
        out
    }

    #[allow(clippy::too_many_arguments)]
    pub fn get_useable_count_with_flag(
        &mut self,
        card: Option<CardId>,
        playerid: u8,
        loc: u8,
        uplayer: u8,
        reason: u32,
        zone: u32,
        flag: &mut u32,
    ) -> i32 {
        let (count, mask) = self.useable_count(card, playerid, loc, uplayer, reason, zone);
        *flag = mask;
        count
    }

    /// `get_useable_count` — how many more cards of this kind the player can
    /// place.
    ///
    /// **A card in the Extra Deck going to a Monster Zone takes its own
    /// branch.** The fork is on the card's current location, so it is only
    /// reachable now that the card is an argument at all.
    pub fn get_useable_count(
        &mut self,
        card: Option<CardId>,
        playerid: u8,
        loc: u8,
        uplayer: u8,
        reason: u32,
        zone: u32,
    ) -> i32 {
        self.useable_count(card, playerid, loc, uplayer, reason, zone)
            .0
    }

    fn useable_count(
        &mut self,
        card: Option<CardId>,
        playerid: u8,
        loc: u8,
        uplayer: u8,
        reason: u32,
        zone: u32,
    ) -> (i32, u32) {
        let from_extra = card.is_some_and(|c| self.cards[c].current.location == location::EXTRA);
        if loc == location::MZONE && from_extra {
            self.get_useable_count_fromex(card, playerid, uplayer, zone)
        } else {
            self.get_useable_count_other(card, playerid, loc, uplayer, reason, zone)
        }
    }

    /// `get_count_map` — which tally a count flag names.
    fn count_map(&mut self, flag: u8) -> &mut crate::fxhash::FxHashMap<u64, u32> {
        if flag & effect_count::DUEL != 0 {
            &mut self.core.effect_count_code_duel
        } else if flag & effect_count::CHAIN != 0 {
            &mut self.core.effect_count_code_chain
        } else {
            &mut self.core.effect_count_code
        }
    }

    fn count_map_ref(&self, flag: u8) -> &crate::fxhash::FxHashMap<u64, u32> {
        if flag & effect_count::DUEL != 0 {
            &self.core.effect_count_code_duel
        } else if flag & effect_count::CHAIN != 0 {
            &self.core.effect_count_code_chain
        } else {
            &self.core.effect_count_code
        }
    }

    /// `generate_count_map_key` — pack the four parts of a tally's identity
    /// into one key.
    ///
    /// Worth a note, because it reads like a bug and is not one. The
    /// reference's function is declared `(code, flag, hopt_index, playerid)`
    /// but its body packs `code << 32 | hopt_index << 16 | flag << 8 |
    /// playerid`, while every call site passes `(code, hopt_index, flag,
    /// playerid)`. The names are crossed against the callers, so the layout
    /// that actually results is the one below. All three call sites agree,
    /// so the behaviour is self-consistent; only the naming is not.
    fn count_map_key(code: u32, flag: u8, hopt_index: u8, playerid: u8) -> u64 {
        (u64::from(code) << 32)
            | (u64::from(flag) << 16)
            | (u64::from(hopt_index) << 8)
            | u64::from(playerid)
    }

    /// `add_effect_code` — record one use.
    pub fn add_effect_code(&mut self, code: u32, flag: u8, hopt_index: u8, playerid: u8) {
        let key = Self::count_map_key(code, flag, hopt_index, playerid);
        *self.count_map(flag).entry(key).or_insert(0) += 1;
    }

    /// `get_effect_code` — how many times it has been used.
    pub fn get_effect_code(&self, code: u32, flag: u8, hopt_index: u8, playerid: u8) -> u32 {
        let key = Self::count_map_key(code, flag, hopt_index, playerid);
        self.count_map_ref(flag).get(&key).copied().unwrap_or(0)
    }

    /// `dec_effect_code` — give one back, as an oath effect does when its
    /// activation is negated. Saturating, as the reference's guard is.
    pub fn dec_effect_code(&mut self, code: u32, flag: u8, hopt_index: u8, playerid: u8) {
        let key = Self::count_map_key(code, flag, hopt_index, playerid);
        if let Some(n) = self.count_map(flag).get_mut(&key) {
            *n = n.saturating_sub(1);
        }
    }

    /// `effect::check_count_limit` — has this effect any activations left?
    ///
    /// The two branches are the difference between "once per turn" and
    /// "hard once per turn". `EFFECT_COUNT_CODE_SINGLE` tallies against the
    /// *card's* field id, so each copy has its own count; otherwise the
    /// tally is against `count_code`, which every copy of a card name
    /// shares.
    pub fn check_count_limit(&self, effect: EffectId, playerid: u8) -> bool {
        let Some(e) = self.effects.get(effect) else {
            return false;
        };
        if !e.has_count_limit() {
            return true;
        }
        if e.count_limit == 0 {
            return false;
        }
        if e.count_code == 0 && e.count_flag == 0 {
            return true;
        }
        let max = u32::from(e.count_limit_max);
        if e.count_flag & effect_count::SINGLE != 0 {
            let Some(handler) = e.handler else {
                return false;
            };
            let fieldid = self.cards[handler].fieldid;
            self.get_effect_code(fieldid, e.count_flag, e.count_hopt_index, PLAYER_NONE) < max
        } else {
            self.get_effect_code(e.count_code, e.count_flag, e.count_hopt_index, playerid) < max
        }
    }

    /// `effect::is_condition_check` — the gate the trigger gather applies
    /// before offering a triggered effect.
    ///
    /// Three tests, and the order matters. A non-activate effect on a card
    /// that is face-down on the field or banished face-down cannot see the
    /// event at all. Then the symbolic-zone range. Only then is the card's
    /// own condition consulted, and a card with no condition passes.
    ///
    /// The reference brackets the callback with `save_lp_cost()` /
    /// `restore_lp_cost()`. Those are **empty inline no-ops** in this
    /// version — the bodies in `field.cpp` are inside a comment block — so
    /// they are deliberately not translated. Implementing what the commented
    /// bodies say would add behaviour the reference does not have.
    pub fn is_condition_check(&mut self, effect: EffectId, playerid: u8, event: &Event) -> bool {
        let Some(e) = self.effects.get(effect) else {
            return false;
        };
        let Some(handler) = e.handler else {
            return false;
        };
        let (is_activate, condition) = (e.is_type(effect_type::ACTIVATE), e.condition);
        let in_symbolic_range = e.is_in_range_of_symbolic_mzone(&self.cards[handler]);

        let card = &self.cards[handler];
        if !is_activate
            && card
                .current
                .is_location(u16::from(location::ONFIELD | location::REMOVED))
            && !card.current.is_faceup()
        {
            return false;
        }
        if !in_symbolic_range {
            return false;
        }
        let Some(condition) = condition else {
            return true;
        };

        self.with_reason_mut(effect, playerid, |f| {
            condition(
                f,
                &Ctx {
                    reason_effect: effect,
                    player: playerid,
                    event,
                    card: None,
                    args: &[],
                },
            )
        })
    }

    /// `effect::is_activate_ready` — condition, then cost, then target, each
    /// asked rather than performed.
    ///
    /// `chk = false` on cost and target is the whole point: this is the
    /// question "could this be activated?", and a port that passed `true`
    /// would pay costs merely for asking.
    ///
    /// Continuous effects skip the cost check, as the reference does.
    #[allow(clippy::too_many_arguments)]
    pub fn is_activate_ready(
        &mut self,
        effect: EffectId,
        reason_effect: EffectId,
        playerid: u8,
        event: &Event,
        neglect_cond: bool,
        neglect_cost: bool,
        neglect_target: bool,
    ) -> bool {
        let Some(e) = self.effects.get(effect) else {
            return false;
        };
        let (condition, cost, target) = (e.condition, e.cost, e.target);
        let is_continuous = e.is_type(effect_type::CONTINUOUS);

        // The range test is against the *reason* effect's handler, not this
        // effect's. The reference is explicit about it and the two differ
        // whenever one effect asks on another's behalf.
        let Some(reason_handler) = self.effects.get(reason_effect).and_then(|r| r.handler) else {
            return false;
        };
        if !self
            .effects
            .get(effect)
            .is_some_and(|e| e.is_in_range_of_symbolic_mzone(&self.cards[reason_handler]))
        {
            return false;
        }

        let ctx = Ctx {
            reason_effect,
            player: playerid,
            event,
            card: None,
            args: &[],
        };

        if !neglect_cond {
            if let Some(condition) = condition {
                if !self.with_reason_mut(effect, playerid, |f| condition(f, &ctx)) {
                    return false;
                }
            }
        }
        if !neglect_cost && !is_continuous {
            if let Some(cost) = cost {
                if !self.with_reason_mut(effect, playerid, |f| cost(f, &ctx, false)) {
                    return false;
                }
            }
        }
        if !neglect_target {
            if let Some(target) = target {
                // The legality question, asked as a plain call: the
                // reference asks it with `check_condition` rather than
                // through the coroutine, so a target that suspended here
                // would have nothing to resume it. A suspension is read
                // as a refusal rather than silently ignored.
                let answer = self.with_reason_mut(effect, playerid, |f| {
                    target(f, &ctx, false, None).finished().unwrap_or(0)
                });
                if answer == 0 {
                    return false;
                }
            }
        }
        true
    }

    /// Run a callback with `core.reason_effect` / `core.reason_player` set,
    /// restoring whatever they were. The reference does this by hand around
    /// every call; doing it in one place keeps the pairing honest.
    fn with_reason_mut<R>(
        &mut self,
        effect: EffectId,
        playerid: u8,
        f: impl FnOnce(&mut Field) -> R,
    ) -> R {
        let saved = (self.core.reason_effect, self.core.reason_player);
        self.core.reason_effect = Some(effect);
        self.core.reason_player = playerid;
        let out = f(self);
        (self.core.reason_effect, self.core.reason_player) = saved;
        out
    }

    /// `field_used_count[flag]` — how many of the five main seats a mask
    /// marks as taken.
    ///
    /// The reference builds a 32-entry lookup table at compile time whose
    /// every entry is `popcnt(i)`. That is a popcount, so this is
    /// `count_ones` on the same five bits. The table is the optimisation,
    /// not the behaviour.
    pub fn field_used_count(flag: u32) -> u32 {
        (flag & 0x1f).count_ones()
    }

    pub fn card(&self, id: CardId) -> &Card {
        &self.cards[id]
    }

    pub fn card_mut(&mut self, id: CardId) -> &mut Card {
        &mut self.cards[id]
    }

    /// Put a card into the duel and hand back its id.
    pub fn new_card(&mut self, card: Card) -> CardId {
        self.cards.push(card);
        self.cards.len() - 1
    }

    /// Put an effect into the pool and hand back its index. `new_effect()`.
    ///
    /// The pool index and the effect's `id` are different things: the index
    /// addresses the arena, while `id` is registration order and is what
    /// every effect sort keys on. They stop agreeing as soon as anything is
    /// re-registered, which `enable_field_effect` does.
    ///
    /// `id` comes from `infos.field_id`, the same counter as card field ids
    /// and chain ids — not from a counter of its own. That interleaving is
    /// deliberate: it makes an effect's registration comparable against a
    /// card's arrival.
    pub fn new_effect(&mut self, mut effect: Effect) -> EffectId {
        let id = self.next_field_id_raw();
        effect.id.set(id);
        effect.initial_id = id;
        self.effects.insert(effect)
    }

    /// `field::add_effect` — put an effect into play.
    ///
    /// An effect with no handler is a *field* effect: one belonging to the
    /// duel rather than to a card. The reference marks it `FIELD_ONLY`,
    /// points its handler at its owner and gives it an id here, which is the
    /// only place that happens — so "has no handler" is how a caller asks
    /// for one.
    pub fn add_effect(&mut self, id: EffectId, owner_player: u8) {
        let Some(effect) = self.effects.get(id) else {
            return;
        };
        if effect.handler.is_none() {
            let new_id = self.next_field_id_raw();
            let owner = self.effects.get(id).and_then(|e| e.owner);
            if let Some(e) = self.effects.get_mut(id) {
                e.flag[0] |= flag::FIELD_ONLY;
                e.handler = owner;
                e.effect_owner = owner_player;
                e.id.set(new_id);
                e.initial_id = new_id;
            }
        }
        let owner_type = self
            .effects
            .get(id)
            .and_then(|e| e.owner)
            .map_or(0, |o| self.cards[o].data.type_);
        if let Some(e) = self.effects.get_mut(id) {
            e.card_type = owner_type;
        }

        let Some(effect) = self.effects.get(id) else {
            return;
        };
        let (ty, code_, field_only, reset_flag, count_limited) = (
            effect.effect_type,
            effect.code,
            effect.is_flag(flag::FIELD_ONLY),
            effect.reset_flag,
            effect.is_flag(flag::COUNT_LIMIT),
        );
        if let Some(index) = self.field_effects.index_for_mut(ty) {
            index.insert(code_, id);
        }
        self.field_effects.indexer.insert(id);
        // The reference's `if (!(type & ACTIONS))` branch does four more
        // things, and until now this port did none of them: three global
        // flags and the `EFFECT_SPSUMMON_COUNT_LIMIT` side index. The flags
        // gate passes that were therefore unreachable — see the findings doc.
        if ty & effect_type::ACTIONS == 0 {
            match code_ {
                code::SELF_TOGRAVE => self.core.global_flag |= global_flag::SELF_TOGRAVE,
                code::SPSUMMON_COUNT_LIMIT => {
                    self.field_effects.spsummon_count_eff.insert(id);
                }
                code::REMOVE_BRAINWASHING => {
                    self.core.global_flag |= global_flag::BRAINWASHING_CHECK;
                }
                code::REVERSE_DECK => self.core.global_flag |= global_flag::DECK_REVERSE_CHECK,
                _ => {}
            }
        }

        if field_only {
            if reset_flag & reset::PHASE != 0 {
                self.field_effects.pheff.insert(id);
            }
            if reset_flag & reset::CHAIN != 0 {
                self.field_effects.cheff.insert(id);
            }
            if count_limited {
                self.field_effects.rechargeable.insert(id);
            }
        }
    }

    /// `field::remove_effect` — take an effect out of play.
    ///
    /// The reference erases by the *iterator* it stored when adding, so it
    /// never has to work out which container the effect went into — and its
    /// own dispatch here is subtly laxer than `add_effect`'s (no `FIELD`
    /// requirement on the trigger cases). An index-based port has to derive
    /// the container, and derives it with **`add_effect`'s** dispatch, since
    /// that is the container the effect is actually in.
    pub fn remove_effect(&mut self, id: EffectId) {
        if !self.field_effects.indexer.remove(&id) {
            return;
        }
        let Some(effect) = self.effects.get(id) else {
            return;
        };
        let (ty, code_, field_only, reset_flag, is_oath, is_count_limited) = (
            effect.effect_type,
            effect.code,
            effect.is_flag(flag::FIELD_ONLY),
            effect.reset_flag,
            effect.is_flag(flag::OATH),
            effect.is_flag(flag::COUNT_LIMIT),
        );
        if let Some(index) = self.field_effects.index_for_mut(ty) {
            index.remove(code_, id);
        }
        // The reference erases from the side index here and **leaves the
        // three global flags set**. They are one-way: a flag says "this pass
        // may have work to do", and the pass then finds nothing.
        if ty & effect_type::ACTIONS == 0 && code_ == code::SPSUMMON_COUNT_LIMIT {
            self.field_effects.spsummon_count_eff.remove(&id);
        }
        if field_only {
            if self.is_disable_related(id) {
                self.update_disable_check_list(id);
            }
            if is_oath {
                self.field_effects.oath.remove(&id);
            }
            if reset_flag & reset::PHASE != 0 {
                self.field_effects.pheff.remove(&id);
            }
            if reset_flag & reset::CHAIN != 0 {
                self.field_effects.cheff.remove(&id);
            }
            if is_count_limited {
                self.field_effects.rechargeable.remove(&id);
            }
            self.core.reseted_effects.insert(id);
        }
    }

    /// Register an effect into the index for its type. A thin alias kept for
    /// the call sites that mean only the indexing.
    pub fn register_effect(&mut self, id: EffectId) {
        let Some(effect) = self.effects.get(id) else {
            return;
        };
        let (ty, code_) = (effect.effect_type, effect.code);
        if let Some(index) = self.field_effects.index_for_mut(ty) {
            index.insert(code_, id);
        }
        self.field_effects.indexer.insert(id);
    }

    /// Make a card group and hand back its handle.
    pub fn new_group(&mut self, cards: impl IntoIterator<Item = CardId>) -> GroupId {
        self.groups.push(cards.into_iter().collect());
        self.groups.len() - 1
    }

    pub fn group(&self, id: GroupId) -> &BTreeSet<CardId> {
        &self.groups[id]
    }

    pub fn group_mut(&mut self, id: GroupId) -> &mut BTreeSet<CardId> {
        &mut self.groups[id]
    }

    /// Allocate the next value of `infos.field_id`.
    ///
    /// One counter feeds three different things in the reference: a card's
    /// `fieldid` when it is placed, an effect's `id` when it is
    /// re-registered, and a chain's `chain_id`. They are drawn from the same
    /// sequence, so the three are interleaved and comparable — which is what
    /// makes "which of these happened first" answerable across kinds.
    pub fn next_field_id_raw(&mut self) -> u32 {
        self.bump_field_id()
    }

    /// The same counter through a shared reference: `is_available`
    /// renumbers an effect from inside a query.
    fn bump_field_id(&self) -> u32 {
        let id = self.infos.field_id.get();
        self.infos.field_id.set(id + 1);
        id
    }

    /// The same counter, truncated — a chain id is a `uint16_t` in the
    /// reference while a card's field id is a `uint32_t`. The truncation is
    /// the reference's and is kept, rather than widening `chain_id` to make
    /// the types agree.
    pub fn next_field_id(&mut self) -> u16 {
        self.next_field_id_raw() as u16
    }

    /// `raise_event(card*, ...)` — something happened, to one card or none.
    ///
    /// Note what this does *not* do: it gathers nothing and offers nothing.
    /// It appends to `queue_event`, and the gather runs later, over the
    /// whole batch at once. That is what makes simultaneous events
    /// simultaneous, and it is why `global_id` is stamped here.
    #[allow(clippy::too_many_arguments)]
    pub fn raise_event(
        &mut self,
        event_card: Option<CardId>,
        event_code: u32,
        reason_effect: Option<EffectId>,
        reason: u32,
        reason_player: u8,
        event_player: u8,
        event_value: u32,
    ) {
        let mut e = Event::new(event_code);
        e.event_cards = event_card.into_iter().collect();
        e.reason_effect = reason_effect;
        e.reason = reason;
        e.reason_player = reason_player;
        e.event_player = event_player;
        e.event_value = event_value;
        e.global_id = self.infos.event_id;
        self.core.queue_event.push_back(e);
    }

    /// `raise_event(card_set, ...)` — one event over many cards.
    ///
    /// A board wipe is one event, not one per card. Effects that count what
    /// was destroyed read `event_cards`, and would count wrong if this
    /// raised several.
    #[allow(clippy::too_many_arguments)]
    pub fn raise_event_over(
        &mut self,
        event_cards: Vec<CardId>,
        event_code: u32,
        reason_effect: Option<EffectId>,
        reason: u32,
        reason_player: u8,
        event_player: u8,
        event_value: u32,
    ) {
        let mut e = Event::new(event_code);
        e.event_cards = event_cards;
        e.reason_effect = reason_effect;
        e.reason = reason;
        e.reason_player = reason_player;
        e.event_player = event_player;
        e.event_value = event_value;
        e.global_id = self.infos.event_id;
        self.core.queue_event.push_back(e);
    }

    /// `raise_single_event` — an event aimed at one card, to be gathered
    /// against *that card's own* effects rather than the field's.
    ///
    /// It goes on a different list from `raise_event`, which is the whole
    /// distinction: a flip effect is on the card that flipped, and asking
    /// the field about it would find every card's flip effect instead.
    #[allow(clippy::too_many_arguments)]
    pub fn raise_single_event(
        &mut self,
        trigger_card: CardId,
        event_cards: Vec<CardId>,
        event_code: u32,
        reason_effect: Option<EffectId>,
        reason: u32,
        reason_player: u8,
        event_player: u8,
        event_value: u32,
    ) {
        let mut e = Event::new(event_code);
        e.trigger_card = Some(trigger_card);
        e.event_cards = event_cards;
        e.reason_effect = reason_effect;
        e.reason = reason;
        e.reason_player = reason_player;
        e.event_player = event_player;
        e.event_value = event_value;
        e.global_id = self.infos.event_id;
        self.core.single_event.push_back(e);
    }

    /// `check_event` — has an event of this code happened in this window?
    ///
    /// `point_event` first, then `instant_event`, and the first match wins.
    /// `queue_event` is deliberately not searched: an event that has been
    /// raised but not yet gathered from has not happened yet as far as this
    /// question is concerned.
    pub fn check_event(&self, code: u32) -> Option<&Event> {
        self.core
            .point_event
            .iter()
            .chain(self.core.instant_event.iter())
            .find(|e| e.event_code == code)
    }

    /// Build a chain for an effect responding to an event, allocating its
    /// id and snapshotting its handler — the shape every gather site
    /// repeats.
    ///
    /// The triggering player is the handler's controller, *except* when the
    /// effect carries `EFFECT_FLAG_EVENT_PLAYER` and the event names a real
    /// player, in which case the event's player activates it. Several cards
    /// read wrong without that.
    pub fn build_chain(&mut self, effect: EffectId, handler: CardId, evt: Event) -> Chain {
        let chain_id = self.next_field_id();
        let mut chain = Chain::new(effect, evt);
        chain.chain_id = chain_id;
        chain.set_triggering_state(&self.cards[handler].state());

        let by_event_player = self
            .effects
            .get(effect)
            .is_some_and(|e| e.is_flag(crate::effect::flag::EVENT_PLAYER));
        let event_player = chain.evt.event_player;
        chain.triggering_player = if by_event_player && event_player != PLAYER_NONE {
            event_player
        } else {
            chain.triggering_controler
        };
        chain
    }
}

#[cfg(test)]
mod tests {

    /// The script library's composite reset masks, pinned as literals.
    ///
    /// `tools/check_constants.py` evaluates the Lua and compares, which
    /// catches a drift in either table — but only when someone runs it.
    /// These are the numbers themselves, so a wrong constituent fails a
    /// build. A wrong constant is invisible otherwise: the surrounding
    /// code stays internally consistent with whatever it was given.
    #[test]
    fn the_composite_reset_masks_are_what_the_library_says() {
        use super::resets;
        assert_eq!(resets::STANDARD, 0x01fe_0000);
        assert_eq!(resets::STANDARD_DISABLE, 0x01ff_0000);
        assert_eq!(resets::STANDARD_PHASE_END, 0x41fe_1200);
        assert_eq!(resets::STANDARD_DISABLE_PHASE_END, 0x41ff_1200);
        assert_eq!(resets::REDIRECT, 0x0c7e_0000);
        assert_eq!(resets::CANNOT_ACT, 0x017e_0000);
        assert_eq!(resets::STANDARD_EXC_GRAVE, 0x017a_0000);
    }

    /// The library's composite **timing** masks, likewise.
    ///
    /// `CHECK_MONSTER_E` is the reason this test exists: the `_E` reads
    /// like "extended, so also a Set", and it is the End Phase. Written
    /// from the name it comes out `0x3c0` instead of `0x1e0`, and nothing
    /// downstream would look wrong — the hint timing would simply never
    /// fire where it should.
    #[test]
    fn the_composite_timing_masks_are_what_the_library_says() {
        use super::timings;
        assert_eq!(timings::CHECK_MONSTER, 0x1c0);
        assert_eq!(timings::CHECK_MONSTER_E, 0x1e0);
    }

    mod disable_chain {
        use super::super::*;
        use crate::card::{card_type, status, Card, CardData};
        use crate::chain::{chain_flag, Chain};
        use crate::effect::{effect_type, Effect};

        /// A card in `player`'s Spell & Trap row with an activate effect,
        /// and a chain link naming it from `from`.
        fn one_link(from: u16) -> (Field, CardId, EffectId) {
            let mut f = Field::new(8000);
            f.infos.turn_id = 3;
            let mut c = Card::with_data(
                CardData {
                    code: 41_100,
                    type_: card_type::TRAP,
                    ..Default::default()
                },
                0,
            );
            c.current.controller = 0;
            c.set_status(status::EFFECT_ENABLED, true);
            let card = f.new_card(c);
            f.add_card(0, card, location::SZONE, 0, false);
            f.cards[card].current.position = crate::board::position::FACEUP;
            let mut e = Effect::new(effect_type::ACTIVATE, code::FREE_CHAIN);
            e.owner = Some(card);
            e.handler = Some(card);
            let eid = f.new_effect(e);
            let mut ch = Chain::new(eid, crate::event::Event::new(code::FREE_CHAIN));
            ch.chain_count = 1;
            ch.chain_id = 11;
            ch.triggering_player = 0;
            ch.triggering_location = from;
            f.cards[card].create_chain_relation(eid, 11);
            f.core.current_chain.push(ch);
            (f, card, eid)
        }

        /// **It negates, announces, and records who did it.**
        #[test]
        fn it_negates_and_says_so() {
            let (mut f, _, _) = one_link(u16::from(location::SZONE));
            f.core.reason_player = 1;
            f.messages.clear();
            assert!(f.disable_chain(1));
            assert_ne!(f.core.current_chain[0].flag & chain_flag::DISABLE_EFFECT, 0);
            assert_eq!(f.core.current_chain[0].disable_player, 1, "who negated it");
            assert!(matches!(
                f.messages.as_slice(),
                [Message::ChainDisabled { chain_count: 1 }]
            ));
        }

        /// **Negating it twice does nothing the second time.**
        #[test]
        fn a_link_is_not_negated_twice() {
            let (mut f, _, _) = one_link(u16::from(location::SZONE));
            assert!(f.disable_chain(1));
            f.messages.clear();
            assert!(!f.disable_chain(1), "already negated");
            assert!(f.messages.is_empty(), "and not announced again");
        }

        /// **An effect flagged `CANNOT_DISABLE` is not negated**, which is
        /// `is_chain_disablable`'s job and is asked here rather than
        /// assumed.
        #[test]
        fn an_undisablable_effect_is_refused() {
            let (mut f, _, eid) = one_link(u16::from(location::SZONE));
            f.effects.get_mut(eid).unwrap().flag[0] |= flag::CANNOT_DISABLE;
            assert!(!f.disable_chain(1));
            assert_eq!(f.core.current_chain[0].flag & chain_flag::DISABLE_EFFECT, 0);
        }

        /// **A handler immune to the negating effect is refused.**
        #[test]
        fn an_immune_handler_is_refused() {
            let (mut f, card, _) = one_link(u16::from(location::SZONE));
            // The effect doing the negating.
            let mut by = Effect::new(effect_type::SINGLE, 0);
            by.owner = Some(card);
            by.handler = Some(card);
            let by = f.new_effect(by);
            f.core.reason_effect = Some(by);
            assert!(f.disable_chain(1), "the positive sibling");

            let (mut f, card, _) = one_link(u16::from(location::SZONE));
            let mut by = Effect::new(effect_type::SINGLE, 0);
            by.owner = Some(card);
            by.handler = Some(card);
            let by = f.new_effect(by);
            f.core.reason_effect = Some(by);
            let mut imm = Effect::new(effect_type::SINGLE, code::IMMUNE_EFFECT);
            imm.owner = Some(card);
            imm.handler = Some(card);
            imm.value = 1;
            let imm = f.new_effect(imm);
            f.cards[card].immune_effect.push(imm);
            f.cards[card].indexer.insert(imm);
            assert!(!f.disable_chain(1), "immune to what is negating it");
        }

        /// **An out-of-range count is clamped to the topmost link, not
        /// refused** — which is what a script passing a raw `ev` relies
        /// on.
        #[test]
        fn an_out_of_range_count_means_the_topmost_link() {
            for count in [0u8, 2, 200] {
                let (mut f, _, _) = one_link(u16::from(location::SZONE));
                assert!(f.disable_chain(count), "count {count}");
                assert_ne!(f.core.current_chain[0].flag & chain_flag::DISABLE_EFFECT, 0);
            }
            // With no chain at all there is nothing to clamp to.
            let mut f = Field::new(8000);
            assert!(!f.disable_chain(1));
        }

        /// **A link activated from the deck loses its relation**, so the
        /// card is not left tied to a chain that will not resolve — and a
        /// link from anywhere else keeps it.
        ///
        /// The extra deck case wants **face-down** as well: a face-up
        /// Pendulum in the extra deck is not the same thing.
        #[test]
        fn only_a_link_from_a_deck_releases_its_relation() {
            let facedown = crate::board::position::FACEDOWN_DEFENSE;
            let faceup = crate::board::position::FACEUP_ATTACK;
            for (from, pos, released) in [
                (u16::from(location::SZONE), faceup, false),
                (u16::from(location::HAND), faceup, false),
                (u16::from(location::DECK), faceup, true),
                (u16::from(location::EXTRA), facedown, true),
                (u16::from(location::EXTRA), faceup, false),
            ] {
                let (mut f, card, eid) = one_link(from);
                f.core.current_chain[0].triggering_position = pos;
                assert!(f.disable_chain(1), "from {from:#x}");
                assert_eq!(
                    !f.cards[card].has_chain_relation(eid, 11),
                    released,
                    "from {from:#x} position {pos:#x}"
                );
            }
        }
    }

    mod placing_an_activating_card {
        use super::*;
        use crate::card::{card_type, status, Card, CardData};
        use crate::effect::{effect_type, Effect};

        fn spell_in(f: &mut Field, loc: u8, type_: u32) -> CardId {
            let mut c = Card::with_data(
                CardData {
                    code: 41000,
                    type_,
                    ..Default::default()
                },
                0,
            );
            c.current.controller = 0;
            c.set_status(status::EFFECT_ENABLED, true);
            let id = f.new_card(c);
            f.add_card(0, id, loc, 0, false);
            id
        }

        fn activation(f: &mut Field, c: CardId) -> EffectId {
            let mut e = Effect::new(effect_type::ACTIVATE | effect_type::FIELD, code::FREE_CHAIN);
            e.owner = Some(c);
            e.handler = Some(c);
            let id = f.new_effect(e);
            // The block reads the chain being built for the zone-limit
            // value; give it one.
            f.core
                .new_chains
                .push_back(crate::chain::Chain::new(id, Event::new(0)));
            id
        }

        fn queued(f: &Field, pred: fn(&Kind) -> bool) -> bool {
            f.core
                .units
                .iter()
                .chain(f.core.subunits.iter())
                .any(|u| pred(&u.kind))
        }

        /// **`STATUS_ACT_FROM_HAND` is stamped from where the card is
        /// now** — set for a hand activation, cleared for one from the
        /// row.
        #[test]
        fn act_from_hand_is_stamped_from_the_current_location() {
            let mut f = Field::new(8000);
            let c = spell_in(&mut f, location::HAND, card_type::SPELL);
            let e = activation(&mut f, c);
            f.place_activating_card(e);
            assert!(f.cards[c].is_status(status::ACT_FROM_HAND));

            let mut g = Field::new(8000);
            let s = spell_in(&mut g, location::SZONE, card_type::SPELL);
            g.cards[s].set_status(status::ACT_FROM_HAND, true);
            let e2 = activation(&mut g, s);
            g.place_activating_card(e2);
            assert!(!g.cards[s].is_status(status::ACT_FROM_HAND));
        }

        /// **A Set card in the row is turned face-up in place**, not moved.
        #[test]
        fn a_set_card_in_the_row_is_turned_not_moved() {
            let mut f = Field::new(8000);
            let s = spell_in(&mut f, location::SZONE, card_type::SPELL);
            f.cards[s].current.position = position::FACEDOWN;
            let e = activation(&mut f, s);
            assert!(!f.place_activating_card(e));
            assert!(
                queued(&f, |k| matches!(k, Kind::ChangePos { .. })),
                "turned"
            );
            assert!(
                !queued(&f, |k| matches!(k, Kind::MoveToField { .. })),
                "not moved"
            );
        }

        /// **A Field Spell from the hand goes to the Field Zone**, an
        /// ordinary Spell to the row. `move_to_field` records the
        /// destination in `to_field_param` (bits 8-15) and normalises the
        /// Field Zone to the Spell row plus seat 5, which is what the
        /// queued unit carries.
        #[test]
        fn a_field_spell_from_the_hand_is_bound_for_the_field_zone() {
            fn queued_zone(f: &Field) -> Option<u32> {
                f.core
                    .units
                    .iter()
                    .chain(f.core.subunits.iter())
                    .find_map(|u| match u.kind {
                        Kind::MoveToField { zone, .. } => Some(zone),
                        _ => None,
                    })
            }
            let mut f = Field::new(8000);
            let fs = spell_in(&mut f, location::HAND, card_type::SPELL | card_type::FIELD);
            let e = activation(&mut f, fs);
            assert!(!f.place_activating_card(e));
            assert_eq!(
                (f.cards[fs].to_field_param >> 8) & 0xff,
                u32::from(location::SZONE)
            );
            assert_eq!(
                queued_zone(&f),
                Some(1 << 5),
                "the Field Zone is Spell seat 5"
            );

            let mut g = Field::new(8000);
            let s = spell_in(&mut g, location::HAND, card_type::SPELL);
            let e2 = activation(&mut g, s);
            g.place_activating_card(e2);
            assert_eq!(
                (g.cards[s].to_field_param >> 8) & 0xff,
                u32::from(location::SZONE)
            );
            assert_eq!(queued_zone(&g), Some(0xff), "any Spell seat");
        }

        /// **The card's field effects are switched off before it moves.**
        #[test]
        fn the_field_effect_is_disabled_before_the_move() {
            let mut f = Field::new(8000);
            let s = spell_in(&mut f, location::HAND, card_type::SPELL);
            let e = activation(&mut f, s);
            assert!(f.cards[s].is_status(status::EFFECT_ENABLED));
            f.place_activating_card(e);
            assert!(!f.cards[s].is_status(status::EFFECT_ENABLED));
        }

        /// **A chain counter whose check says false counts the link**, is
        /// recorded on it, and is given back on restore.
        #[test]
        fn a_chain_counter_is_applied_and_restored() {
            fn never(_: &Field, _: EffectId, _: Option<CardId>, _: &[i64]) -> bool {
                false
            }
            let mut f = Field::new(8000);
            let c = spell_in(&mut f, location::HAND, card_type::SPELL);
            let e = activation(&mut f, c);
            f.core.chain_counter.insert(
                77,
                crate::field::ActionCount {
                    check: Some(never),
                    player_amount: [0, 0],
                },
            );
            f.core
                .current_chain
                .push(crate::chain::Chain::new(e, Event::new(0)));
            f.check_chain_counter(e, 1, 5);
            assert_eq!(f.core.chain_counter[&77].player_amount, [0, 1]);
            assert_eq!(
                f.core.current_chain[0].applied_chain_counters,
                Some(vec![77])
            );
            f.restore_chain_counter(1, 0);
            assert_eq!(f.core.chain_counter[&77].player_amount, [0, 0]);
        }
    }
    use super::*;
    use crate::board::{location, position};
    use crate::effect::flag;
    use crate::event::code;

    fn field_with_card(controller: u8) -> (Field, CardId) {
        let mut f = Field::new(8000);
        let mut c = Card::new(44095762, controller);
        c.current.controller = controller;
        c.current.location = location::SZONE;
        let id = f.new_card(c);
        (f, id)
    }

    /// Ids start at 1, not 0, because 0 means "none".
    #[test]
    fn the_counters_start_where_the_reference_starts_them() {
        let i = Infos::default();
        assert_eq!(i.event_id, 1);
        assert_eq!(i.field_id.get(), 1);
        assert_eq!(i.card_id, 1);
        assert_eq!(i.turn_id, 0);
    }

    mod raising {
        use super::*;

        /// Raising gathers nothing. It appends to a queue, and the gather
        /// runs later over the whole batch — which is what makes
        /// simultaneous events simultaneous.
        #[test]
        fn raising_only_queues() {
            let (mut f, card) = field_with_card(0);
            f.raise_event(Some(card), code::CHAINING, None, 0, 0, 0, 0);
            assert_eq!(f.core.queue_event.len(), 1);
            assert!(f.core.instant_event.is_empty());
            assert!(f.core.new_fchain.is_empty(), "nothing was gathered");
        }

        /// Every event in one batch carries the same id. That is how two
        /// chains built from one batch are recognised as simultaneous.
        #[test]
        fn events_in_one_batch_share_an_id() {
            let (mut f, card) = field_with_card(0);
            f.raise_event(Some(card), code::CHAINING, None, 0, 0, 0, 0);
            f.raise_event(None, code::CHAIN_END, None, 0, 0, 0, 0);
            let ids: Vec<u32> = f.core.queue_event.iter().map(|e| e.global_id).collect();
            assert_eq!(ids, vec![1, 1]);

            f.infos.event_id += 1;
            f.raise_event(None, code::CHAIN_END, None, 0, 0, 0, 0);
            assert_eq!(f.core.queue_event.back().unwrap().global_id, 2);
        }

        /// A board wipe is one event over many cards, not many events.
        #[test]
        fn one_event_can_be_over_many_cards() {
            let mut f = Field::new(8000);
            f.raise_event_over(vec![0, 1, 2], code::CHAINING, None, 0, 0, 0, 0);
            assert_eq!(f.core.queue_event.len(), 1);
            assert_eq!(f.core.queue_event[0].event_cards.len(), 3);
        }

        /// Single events go on a different list, because they are gathered
        /// against the card's own effects rather than the field's.
        #[test]
        fn single_events_are_a_separate_list() {
            let (mut f, card) = field_with_card(0);
            f.raise_single_event(card, vec![], code::FLIP, None, 0, 0, 0, 0);
            assert!(f.core.queue_event.is_empty());
            assert_eq!(f.core.single_event.len(), 1);
            assert_eq!(f.core.single_event[0].trigger_card, Some(card));
        }
    }

    mod check_event {
        use super::*;

        /// A raised-but-not-gathered event has not happened yet, as far as
        /// this question goes.
        #[test]
        fn the_queue_is_not_searched() {
            let mut f = Field::new(8000);
            f.raise_event(None, code::CHAINING, None, 0, 0, 0, 0);
            assert!(f.check_event(code::CHAINING).is_none());
        }

        /// Point events are searched before instant ones, and the first
        /// match wins.
        #[test]
        fn point_events_are_searched_first() {
            let mut f = Field::new(8000);
            let mut point = Event::new(code::CHAINING);
            point.event_value = 111;
            let mut instant = Event::new(code::CHAINING);
            instant.event_value = 222;
            f.core.point_event.push_back(point);
            f.core.instant_event.push_back(instant);
            assert_eq!(f.check_event(code::CHAINING).unwrap().event_value, 111);
        }

        #[test]
        fn instant_events_are_searched_when_the_point_list_misses() {
            let mut f = Field::new(8000);
            f.core.point_event.push_back(Event::new(code::CHAIN_END));
            f.core.instant_event.push_back(Event::new(code::CHAINING));
            assert!(f.check_event(code::CHAINING).is_some());
            assert!(f.check_event(code::FLIP).is_none());
        }
    }

    mod gather {
        use super::*;

        /// A face-up monster carrying one trigger effect of the given type.
        fn trigger_on_field(
            f: &mut Field,
            controller: u8,
            ty: u16,
            event_code: u32,
        ) -> (CardId, EffectId) {
            let mut c = Card::with_data(
                CardData {
                    code: 18036057,
                    type_: card_type::MONSTER | card_type::EFFECT,
                    ..Default::default()
                },
                controller,
            );
            c.current.controller = controller;
            c.current.location = location::MZONE;
            c.current.position = position::FACEUP_ATTACK;
            c.set_status(status::EFFECT_ENABLED, true);
            let card = f.new_card(c);

            // `FIELD` as well as the trigger type: the field indices the
            // gather searches take only field effects. A trigger without it
            // is a *single* effect, reached through `raise_single_event`
            // rather than through these indices.
            let mut e = Effect::new(ty | effect_type::FIELD | effect_type::ACTIONS, event_code);
            e.owner = Some(card);
            e.handler = Some(card);
            e.effect_owner = controller;
            e.range = u16::from(location::MZONE);
            let id = f.new_effect(e);
            f.register_effect(id);
            (card, id)
        }

        /// The property the whole design rests on: the gather runs over the
        /// *whole queue*, so effects responding to different events raised
        /// in one batch end up on the same list — which is what makes them
        /// simultaneous.
        #[test]
        fn one_gather_covers_the_whole_queue() {
            let mut f = Field::new(8000);
            f.infos.phase = phases::MAIN1;
            let (a, _) = trigger_on_field(&mut f, 0, effect_type::TRIGGER_F, code::CHAINING);
            let (b, _) = trigger_on_field(&mut f, 0, effect_type::TRIGGER_F, code::CHAIN_END);

            f.raise_event(Some(a), code::CHAINING, None, 0, 0, 0, 0);
            f.raise_event(Some(b), code::CHAIN_END, None, 0, 0, 0, 0);
            assert_eq!(f.core.new_fchain.len(), 0, "nothing gathered by raising");

            f.process_instant_event();
            assert_eq!(
                f.core.new_fchain.len(),
                2,
                "both events gathered in one pass"
            );
            let ids: Vec<u32> = f.core.new_fchain.iter().map(|c| c.event_id).collect();
            assert_eq!(ids, vec![1, 1], "and they share an event id");
        }

        /// The queue is emptied onto `instant_event`, which is what makes
        /// the events answerable by `check_event` afterwards.
        #[test]
        fn the_queue_moves_on_to_the_instant_list() {
            let mut f = Field::new(8000);
            f.infos.phase = phases::MAIN1;
            f.raise_event(None, code::CHAINING, None, 0, 0, 0, 0);
            assert!(f.check_event(code::CHAINING).is_none());

            f.process_instant_event();
            assert!(f.core.queue_event.is_empty());
            assert_eq!(f.core.instant_event.len(), 1);
            assert!(
                f.check_event(code::CHAINING).is_some(),
                "now it has happened"
            );
        }

        /// Mandatory and optional triggers land on different lists.
        #[test]
        fn mandatory_and_optional_triggers_are_kept_apart() {
            let mut f = Field::new(8000);
            f.infos.phase = phases::MAIN1;
            trigger_on_field(&mut f, 0, effect_type::TRIGGER_F, code::CHAINING);
            trigger_on_field(&mut f, 0, effect_type::TRIGGER_O, code::CHAINING);

            f.raise_event(None, code::CHAINING, None, 0, 0, 0, 0);
            f.process_instant_event();
            assert_eq!(f.core.new_fchain.len(), 1);
            assert_eq!(f.core.new_ochain.len(), 1);
        }

        /// Three event kinds raise continuous effects but no triggers.
        #[test]
        fn three_event_kinds_raise_no_triggers() {
            for code_ in [code::ADJUST, code::BREAK_EFFECT, code::PHASE_START + 4] {
                let mut f = Field::new(8000);
                f.infos.phase = phases::MAIN1;
                trigger_on_field(&mut f, 0, effect_type::TRIGGER_F, code_);
                f.raise_event(None, code_, None, 0, 0, 0, 0);
                f.process_instant_event();
                assert!(
                    f.core.new_fchain.is_empty(),
                    "{code_:#x} should raise no triggers"
                );
            }
            // The same code as an ordinary event does gather.
            let mut f = Field::new(8000);
            f.infos.phase = phases::MAIN1;
            trigger_on_field(&mut f, 0, effect_type::TRIGGER_F, code::CHAINING);
            f.raise_event(None, code::CHAINING, None, 0, 0, 0, 0);
            f.process_instant_event();
            assert_eq!(f.core.new_fchain.len(), 1);
        }

        /// A disabled card's trigger is not gathered.
        #[test]
        fn a_disabled_card_does_not_trigger() {
            let mut f = Field::new(8000);
            f.infos.phase = phases::MAIN1;
            let (card, _) = trigger_on_field(&mut f, 0, effect_type::TRIGGER_F, code::CHAINING);
            f.cards[card].set_status(status::EFFECT_ENABLED, false);
            f.raise_event(None, code::CHAINING, None, 0, 0, 0, 0);
            f.process_instant_event();
            assert!(f.core.new_fchain.is_empty());
        }

        /// A trigger whose handler was sent away in the same batch is
        /// skipped — unless it is ranged to the hand, the one place a
        /// just-sent card can still act from.
        #[test]
        fn a_just_sent_handler_is_skipped_unless_it_acts_from_the_hand() {
            let mut f = Field::new(8000);
            f.infos.phase = phases::MAIN1;
            let (card, e) = trigger_on_field(&mut f, 0, effect_type::TRIGGER_F, code::CHAINING);
            f.effects.get_mut(e).unwrap().flag[1] |= flag2::CHECK_SIMULTANEOUS;
            f.core.just_sent_cards.insert(card);

            f.raise_event(None, code::CHAINING, None, 0, 0, 0, 0);
            f.process_instant_event();
            assert!(f.core.new_fchain.is_empty());

            let mut f = Field::new(8000);
            f.infos.phase = phases::MAIN1;
            let (card, e) = trigger_on_field(&mut f, 0, effect_type::TRIGGER_F, code::CHAINING);
            f.effects.get_mut(e).unwrap().flag[1] |= flag2::CHECK_SIMULTANEOUS;
            f.effects.get_mut(e).unwrap().range |= u16::from(location::HAND);
            f.core.just_sent_cards.insert(card);
            f.raise_event(None, code::CHAINING, None, 0, 0, 0, 0);
            f.process_instant_event();
            assert_eq!(f.core.new_fchain.len(), 1, "a hand effect still acts");
            assert!(f.core.new_fchain[0].was_just_sent);
        }

        /// An optional trigger ranged to the hand is gathered even when its
        /// condition fails — but is left unrelated to its handler. A
        /// mandatory one is not gathered at all.
        #[test]
        fn a_hand_trigger_is_offered_even_when_its_condition_fails() {
            fn never(_: &mut Field, _: &Ctx) -> bool {
                false
            }
            let mut f = Field::new(8000);
            f.infos.phase = phases::MAIN1;
            let (card, e) = trigger_on_field(&mut f, 0, effect_type::TRIGGER_O, code::CHAINING);
            f.effects.get_mut(e).unwrap().range = u16::from(location::HAND);
            f.effects.get_mut(e).unwrap().condition = Some(never);

            f.raise_event(None, code::CHAINING, None, 0, 0, 0, 0);
            f.process_instant_event();
            assert_eq!(f.core.new_ochain.len(), 1, "offered anyway");
            assert!(
                !f.cards[card].has_effect_relation(e),
                "but not related to its handler"
            );

            let mut f = Field::new(8000);
            f.infos.phase = phases::MAIN1;
            let (_, e) = trigger_on_field(&mut f, 0, effect_type::TRIGGER_F, code::CHAINING);
            f.effects.get_mut(e).unwrap().range = u16::from(location::HAND);
            f.effects.get_mut(e).unwrap().condition = Some(never);
            f.raise_event(None, code::CHAINING, None, 0, 0, 0, 0);
            f.process_instant_event();
            assert!(
                f.core.new_fchain.is_empty(),
                "a mandatory trigger still needs its condition"
            );
        }

        /// A gathered trigger is related to its handler, keyed by the chain.
        #[test]
        fn a_gathered_trigger_relates_its_handler() {
            let mut f = Field::new(8000);
            f.infos.phase = phases::MAIN1;
            let (card, e) = trigger_on_field(&mut f, 0, effect_type::TRIGGER_F, code::CHAINING);
            f.raise_event(None, code::CHAINING, None, 0, 0, 0, 0);
            f.process_instant_event();
            let chain_id = f.core.new_fchain[0].chain_id;
            assert!(f.cards[card].has_chain_relation(e, chain_id));
        }

        /// Continuous effects are emplaced as units, turn player first.
        #[test]
        fn continuous_effects_are_emplaced_turn_player_first() {
            let mut f = Field::new(8000);
            f.infos.phase = phases::MAIN1;
            f.infos.turn_player = 0;
            trigger_on_field(&mut f, 1, effect_type::CONTINUOUS, code::CHAINING);
            trigger_on_field(&mut f, 0, effect_type::CONTINUOUS, code::CHAINING);

            f.raise_event(None, code::CHAINING, None, 0, 0, 0, 0);
            f.process_instant_event();

            assert_eq!(f.core.sub_solving_continuous.len(), 2);
            assert_eq!(
                f.core.sub_solving_continuous[0].triggering_player, 0,
                "the turn player's first, whatever order they were gathered"
            );
            assert_eq!(f.core.sub_solving_continuous[1].triggering_player, 1);
        }

        /// A DELAY continuous effect raised while a chain is solving is held
        /// back rather than solved now.
        #[test]
        fn a_delayed_continuous_effect_is_held_back() {
            let mut f = Field::new(8000);
            f.infos.phase = phases::MAIN1;
            let (_, e) = trigger_on_field(&mut f, 0, effect_type::CONTINUOUS, code::CHAINING);
            f.effects.get_mut(e).unwrap().flag[0] |= flag::DELAY;
            f.core.chain_solving = true;

            f.raise_event(None, code::CHAINING, None, 0, 0, 0, 0);
            f.process_instant_event();
            assert!(f.core.sub_solving_continuous.is_empty());
            assert_eq!(f.core.delayed_continuous_tp.len(), 1);
        }

        /// Single events go to the card's own effects, not the field's.
        #[test]
        fn a_single_event_finds_only_the_cards_own_effect() {
            let mut f = Field::new(8000);
            f.infos.phase = phases::MAIN1;
            let (flipped, own) = trigger_on_field(
                &mut f,
                0,
                effect_type::TRIGGER_O | effect_type::SINGLE,
                code::FLIP,
            );
            let (_, elsewhere) = trigger_on_field(
                &mut f,
                0,
                effect_type::TRIGGER_O | effect_type::SINGLE,
                code::FLIP,
            );
            f.cards[flipped].single_effect.insert(code::FLIP, own);

            f.raise_single_event(flipped, vec![], code::FLIP, None, 0, 0, 0, 0);
            f.process_single_event();
            assert_eq!(f.core.new_ochain.len(), 1, "only the flipped card's");
            assert_eq!(f.core.new_ochain[0].triggering_effect, own);
            assert_ne!(f.core.new_ochain[0].triggering_effect, elsewhere);
            assert!(f.core.single_event.is_empty());
        }

        /// `NO_FLIP_EFFECT` in an event's value says the flip happened but
        /// must not trigger flip effects.
        #[test]
        fn no_flip_effect_suppresses_a_flip_effect() {
            let mut f = Field::new(8000);
            f.infos.phase = phases::MAIN1;
            let (flipped, own) = trigger_on_field(
                &mut f,
                0,
                effect_type::FLIP | effect_type::SINGLE,
                code::FLIP,
            );
            f.cards[flipped].single_effect.insert(code::FLIP, own);

            f.raise_single_event(
                flipped,
                vec![],
                code::FLIP,
                None,
                0,
                0,
                0,
                NO_FLIP_EFFECT >> 16,
            );
            f.process_single_event();
            assert!(f.core.new_fchain.is_empty());
            assert!(f.core.new_ochain.is_empty());
        }

        /// A card that left the field only temporarily is asked by the
        /// controller it had *before* it left.
        #[test]
        fn a_temporary_absence_is_asked_of_the_previous_controller() {
            let mut f = Field::new(8000);
            f.infos.phase = phases::MAIN1;
            let (card, own) = trigger_on_field(
                &mut f,
                1,
                effect_type::TRIGGER_O | effect_type::SINGLE,
                code::CHAINING,
            );
            f.cards[card].single_effect.insert(code::CHAINING, own);
            f.cards[card].previous.controller = 0;
            f.cards[card].reason = reason::TEMPORARY;

            f.raise_single_event(card, vec![], code::CHAINING, None, 0, 0, 0, 0);
            f.process_single_event();
            assert_eq!(f.core.new_ochain.len(), 1);
            assert_eq!(
                f.core.new_ochain[0].triggering_player, 0,
                "the controller it had before it left"
            );
        }
    }

    mod resolved_properties {
        use super::*;
        use crate::card::Temp;

        fn on_field(f: &mut Field, code: u32, type_: u32) -> CardId {
            let mut c = Card::with_data(
                CardData {
                    code,
                    type_,
                    ..Default::default()
                },
                0,
            );
            c.current.location = location::MZONE;
            c.current.position = position::FACEUP_ATTACK;
            c.set_status(status::EFFECT_ENABLED, true);
            f.new_card(c)
        }

        fn add_type_effect(f: &mut Field, card: CardId, code: u32, value: i64) -> EffectId {
            let mut e = Effect::new(effect_type::SINGLE, code);
            e.owner = Some(card);
            e.handler = Some(card);
            e.value = value;
            let id = f.new_effect(e);
            f.cards[card].single_effect.insert(code, id);
            id
        }

        /// Nowhere an effect could reach: the printed type, unresolved.
        #[test]
        fn a_card_out_of_play_reports_its_printed_type() {
            let mut f = Field::new(8000);
            let c = on_field(&mut f, 1, card_type::MONSTER);
            f.cards[c].current.location = location::DECK;
            add_type_effect(&mut f, c, code::ADD_TYPE, i64::from(card_type::SPELL));
            assert_eq!(
                f.get_type(c, None, 0, PLAYER_NONE),
                card_type::MONSTER,
                "in the deck, nothing has reached it"
            );
        }

        /// ADD and REMOVE adjust; CHANGE replaces.
        #[test]
        fn types_are_added_removed_and_replaced() {
            let mut f = Field::new(8000);
            let c = on_field(&mut f, 1, card_type::MONSTER | card_type::EFFECT);
            add_type_effect(&mut f, c, code::ADD_TYPE, i64::from(card_type::TUNER));
            assert_eq!(
                f.get_type(c, None, 0, PLAYER_NONE),
                card_type::MONSTER | card_type::EFFECT | card_type::TUNER
            );

            let c = on_field(&mut f, 2, card_type::MONSTER | card_type::EFFECT);
            add_type_effect(&mut f, c, code::REMOVE_TYPE, i64::from(card_type::EFFECT));
            assert_eq!(f.get_type(c, None, 0, PLAYER_NONE), card_type::MONSTER);

            let c = on_field(&mut f, 3, card_type::MONSTER | card_type::EFFECT);
            add_type_effect(&mut f, c, code::CHANGE_TYPE, i64::from(card_type::SPELL));
            assert_eq!(f.get_type(c, None, 0, PLAYER_NONE), card_type::SPELL);
        }

        /// They apply in registration order, not in the order the three
        /// codes were gathered. Registering REMOVE first and ADD second
        /// gives a different answer from the reverse.
        #[test]
        fn type_effects_apply_in_registration_order() {
            let mut f = Field::new(8000);
            let a = on_field(&mut f, 1, card_type::MONSTER);
            add_type_effect(&mut f, a, code::REMOVE_TYPE, i64::from(card_type::MONSTER));
            add_type_effect(&mut f, a, code::ADD_TYPE, i64::from(card_type::MONSTER));
            assert_eq!(
                f.get_type(a, None, 0, PLAYER_NONE),
                card_type::MONSTER,
                "removed, then added back"
            );

            let b = on_field(&mut f, 2, card_type::MONSTER);
            add_type_effect(&mut f, b, code::ADD_TYPE, i64::from(card_type::MONSTER));
            add_type_effect(&mut f, b, code::REMOVE_TYPE, i64::from(card_type::MONSTER));
            assert_eq!(
                f.get_type(b, None, 0, PLAYER_NONE),
                0,
                "added, then removed — the reverse order, the other answer"
            );
        }

        /// `temp` is a recursion guard, not a cache: it must be clear again
        /// afterwards, or every later call returns the stale partial value.
        #[test]
        fn the_recursion_guard_is_cleared_on_the_way_out() {
            let mut f = Field::new(8000);
            let c = on_field(&mut f, 1, card_type::MONSTER);
            add_type_effect(&mut f, c, code::ADD_TYPE, i64::from(card_type::TUNER));

            assert_eq!(f.cards[c].temp, Temp::default(), "clear before");
            let first = f.get_type(c, None, 0, PLAYER_NONE);
            assert_eq!(f.cards[c].temp.type_, None, "clear after");
            let second = f.get_type(c, None, 0, PLAYER_NONE);
            assert_eq!(first, second, "and the second call agrees with the first");
        }

        /// A value function that asks the card for its own type gets the
        /// partial value rather than recursing forever. This is the whole
        /// reason the guard exists.
        #[test]
        fn a_self_referential_type_effect_terminates() {
            fn asks_its_own_type(_: &Effect, f: &Field, ctx: &Ctx) -> i64 {
                let card = ctx.card.expect("asked about a card");
                // Reading the guard directly is what the reference's
                // `get_type` would return on re-entry.
                i64::from(f.cards[card].temp.type_.unwrap_or(0))
            }
            let mut f = Field::new(8000);
            let c = on_field(&mut f, 1, card_type::MONSTER);
            let e = add_type_effect(&mut f, c, code::ADD_TYPE, 0);
            f.effects.get_mut(e).unwrap().flag[0] |= flag::FUNC_VALUE;
            f.effects.get_mut(e).unwrap().value_fn = Some(asks_its_own_type);

            assert_eq!(f.get_type(c, None, 0, PLAYER_NONE), card_type::MONSTER);
            assert_eq!(f.cards[c].temp.type_, None, "and the guard is cleared");
        }

        /// A card in a Pendulum Zone is a Spell — except when a summon is
        /// what is being asked about.
        #[test]
        fn a_pendulum_zone_card_is_a_spell_outside_a_summon() {
            let mut f = Field::new(8000);
            let c = on_field(&mut f, 1, card_type::MONSTER | card_type::PENDULUM);
            f.cards[c].current.location = location::SZONE;
            f.cards[c].current.pzone = true;
            assert_eq!(
                f.get_type(c, None, 0, PLAYER_NONE),
                card_type::PENDULUM + card_type::SPELL
            );
            assert_ne!(
                f.get_type(c, None, 1, PLAYER_NONE),
                card_type::PENDULUM + card_type::SPELL,
                "but not when a summon is the question"
            );
        }

        /// `get_code` resolves the alias, and a name-changing effect wins.
        #[test]
        fn the_code_resolves_alias_and_change_effects() {
            let mut f = Field::new(8000);
            let c = on_field(&mut f, 111, card_type::MONSTER);
            assert_eq!(f.get_code(c), 111);

            f.cards[c].data.alias = 222;
            assert_eq!(f.get_code(c), 222, "the alias applies");

            add_type_effect(&mut f, c, code::CHANGE_CODE, 333);
            assert_eq!(f.get_code(c), 333, "a change effect wins over the alias");
        }
    }

    mod activateable {
        use super::*;

        fn set_trap(f: &mut Field, controller: u8) -> (CardId, EffectId) {
            let mut c = Card::with_data(
                CardData {
                    code: 44095762,
                    type_: card_type::TRAP,
                    ..Default::default()
                },
                controller,
            );
            c.current.controller = controller;
            c.current.location = location::SZONE;
            c.current.sequence = 0;
            c.current.position = position::FACEDOWN_DEFENSE;
            let card = f.new_card(c);
            f.players[controller as usize].szone[0] = Some(card);
            f.players[controller as usize].used_location |= 0x100;

            let mut e = Effect::new(
                effect_type::ACTIVATE | effect_type::ACTIONS,
                code::FREE_CHAIN,
            );
            e.owner = Some(card);
            e.handler = Some(card);
            let id = f.new_effect(e);
            (card, id)
        }

        /// A set trap in a spell/trap zone, face-down, in the Main Phase.
        #[test]
        fn a_set_trap_can_be_activated() {
            let mut f = Field::new(8000);
            f.infos.phase = phases::MAIN1;
            let (_, e) = set_trap(&mut f, 0);
            assert!(f.is_activateable(
                e,
                0,
                &Event::new(code::FREE_CHAIN),
                false,
                false,
                false,
                false,
                false
            ));
        }

        /// Not by the opponent, without `EFFECT_FLAG_BOTH_SIDE`.
        #[test]
        fn the_opponent_cannot_activate_it() {
            let mut f = Field::new(8000);
            f.infos.phase = phases::MAIN1;
            let (_, e) = set_trap(&mut f, 0);
            let ev = Event::new(code::FREE_CHAIN);
            assert!(!f.is_activateable(e, 1, &ev, false, false, false, false, false));
            f.effects.get_mut(e).unwrap().flag[0] |= flag::BOTH_SIDE;
            assert!(f.is_activateable(e, 1, &ev, false, false, false, false, false));
        }

        /// Face-up already, or forbidden: no.
        #[test]
        fn a_face_up_or_forbidden_card_cannot_be_activated() {
            let mut f = Field::new(8000);
            f.infos.phase = phases::MAIN1;
            let (card, e) = set_trap(&mut f, 0);
            let ev = Event::new(code::FREE_CHAIN);

            f.cards[card].current.position = position::FACEUP_ATTACK;
            assert!(!f.is_activateable(e, 0, &ev, false, false, false, false, false));
            f.cards[card].current.position = position::FACEDOWN_DEFENSE;

            f.cards[card].set_status(status::FORBIDDEN, true);
            assert!(!f.is_activateable(e, 0, &ev, false, false, false, false, false));
        }

        /// The Damage Step is closed to a non-counter card — this is the
        /// test the earlier, abandoned version silently dropped.
        #[test]
        fn the_damage_step_is_closed_to_an_ordinary_trap() {
            let mut f = Field::new(8000);
            let (card, e) = set_trap(&mut f, 0);
            let ev = Event::new(code::FREE_CHAIN);

            f.infos.phase = phases::DAMAGE;
            assert!(!f.is_activateable(e, 0, &ev, false, false, false, false, false));

            f.infos.phase = phases::DAMAGE_CAL;
            assert!(!f.is_activateable(e, 0, &ev, false, false, false, false, false));

            f.effects.get_mut(e).unwrap().flag[0] |= flag::DAMAGE_STEP | flag::DAMAGE_CAL;
            f.infos.phase = phases::DAMAGE;
            assert!(
                f.is_activateable(e, 0, &ev, false, false, false, false, false),
                "unless the effect says it may"
            );

            // A Counter Trap is exempt by card type rather than by flag.
            f.effects.get_mut(e).unwrap().flag[0] &= !(flag::DAMAGE_STEP | flag::DAMAGE_CAL);
            f.cards[card].data.type_ |= card_type::COUNTER;
            assert!(f.is_activateable(e, 0, &ev, false, false, false, false, false));
        }

        /// A continuous trap whose activation does nothing on its own gets
        /// through the Damage Step if one of its field effects could be
        /// activated — `get_cteffect`, the other omission.
        #[test]
        fn a_continuous_trap_with_a_live_field_effect_gets_through() {
            let mut f = Field::new(8000);
            f.infos.phase = phases::DAMAGE;
            let (card, e) = set_trap(&mut f, 0);
            f.cards[card].data.type_ = card_type::TRAP | card_type::CONTINUOUS;
            let ev = Event::new(code::FREE_CHAIN);

            assert!(
                !f.is_activateable(e, 0, &ev, false, false, false, false, false),
                "no field effect yet"
            );

            let mut fe = Effect::new(
                effect_type::TRIGGER_O | effect_type::ACTIONS | effect_type::FIELD,
                code::FREE_CHAIN,
            );
            fe.owner = Some(card);
            fe.handler = Some(card);
            fe.range = u16::from(location::SZONE);
            fe.flag[0] |= flag::DAMAGE_STEP;
            let fe = f.new_effect(fe);
            f.cards[card].field_effect.insert(code::FREE_CHAIN, fe);

            assert!(
                f.is_activateable(e, 0, &ev, false, false, false, false, false),
                "the field effect is live, so the activation is allowed"
            );
        }

        /// An ordinary Spell cannot be activated from the hand on the
        /// opponent's turn, and no permitting effect exists for it.
        #[test]
        fn an_ordinary_spell_cannot_be_activated_from_the_hand_off_turn() {
            let mut f = Field::new(8000);
            f.infos.phase = phases::MAIN1;
            f.infos.turn_player = 1;
            let (card, e) = set_trap(&mut f, 0);
            f.cards[card].data.type_ = card_type::SPELL;
            f.cards[card].current.location = location::HAND;
            let ev = Event::new(code::FREE_CHAIN);

            assert!(!f.is_activateable(e, 0, &ev, false, false, false, false, false));
            assert!(
                f.is_activateable(e, 0, &ev, false, false, false, true, false),
                "neglect_loc skips the hand question"
            );
        }

        /// `neglect_loc` guards only the hand case. A trap set this turn
        /// still needs its permitting effect.
        #[test]
        fn neglect_loc_does_not_excuse_the_set_turn() {
            let mut f = Field::new(8000);
            f.infos.phase = phases::MAIN1;
            let (card, e) = set_trap(&mut f, 0);
            f.cards[card].set_status(status::SET_TURN, true);
            let ev = Event::new(code::FREE_CHAIN);

            assert!(!f.is_activateable(e, 0, &ev, false, false, false, false, false));
            assert!(
                !f.is_activateable(e, 0, &ev, false, false, false, true, false),
                "neglecting the location must not excuse the set turn"
            );

            let mut permit = Effect::new(effect_type::SINGLE, code::TRAP_ACT_IN_SET_TURN);
            permit.owner = Some(card);
            permit.handler = Some(card);
            let permit = f.new_effect(permit);
            f.cards[card]
                .single_effect
                .insert(code::TRAP_ACT_IN_SET_TURN, permit);
            assert!(
                f.is_activateable(e, 0, &ev, false, false, false, false, false),
                "with permission, it may"
            );
        }

        /// An effect that is not an action effect is never activatable.
        #[test]
        fn a_non_action_effect_is_never_activatable() {
            let mut f = Field::new(8000);
            f.infos.phase = phases::MAIN1;
            let (_, e) = set_trap(&mut f, 0);
            f.effects.get_mut(e).unwrap().effect_type &= !effect_type::ACTIONS;
            assert!(!f.is_activateable(
                e,
                0,
                &Event::new(code::FREE_CHAIN),
                false,
                false,
                false,
                false,
                false
            ));
        }
    }

    mod zone_limits {
        use super::*;

        /// The limit is the cap minus what is already used, and the two
        /// halves of the bitfield are read differently: monster seats from
        /// bit 0, spell/trap seats from bit 8.
        #[test]
        fn the_limit_is_the_cap_less_what_is_used() {
            let mut f = Field::new(8000);
            assert_eq!(f.get_mzone_limit(0, 0, 0), 5);
            assert_eq!(f.get_szone_limit(0, 0, 0), 5);

            f.players[0].used_location = 0b101 | (0b11 << 8);
            assert_eq!(f.get_mzone_limit(0, 0, 0), 3);
            assert_eq!(f.get_szone_limit(0, 0, 0), 3);
            assert_eq!(f.get_mzone_limit(1, 0, 0), 5, "the other player's board");
        }

        /// An effect capping the zone lowers the maximum, and the lowest of
        /// several wins.
        #[test]
        fn a_capping_effect_lowers_the_maximum() {
            fn three(_: &Effect, _: &Field, _: &Ctx) -> i64 {
                3
            }
            fn two(_: &Effect, _: &Field, _: &Ctx) -> i64 {
                2
            }
            let mut f = Field::new(8000);
            let mut source = Card::new(18036057, 0);
            source.current.controller = 0;
            source.current.location = location::MZONE;
            source.current.position = position::FACEUP_ATTACK;
            source.set_status(status::EFFECT_ENABLED, true);
            let source = f.new_card(source);

            for value in [three, two] {
                let mut cap = Effect::new(effect_type::FIELD, code::MAX_MZONE);
                cap.owner = Some(source);
                cap.handler = Some(source);
                cap.effect_owner = 0;
                cap.flag[0] |= flag::PLAYER_TARGET | flag::FUNC_VALUE;
                cap.range = u16::from(location::MZONE);
                cap.s_range = u16::from(location::MZONE);
                cap.value_fn = Some(value);
                let cap = f.new_effect(cap);
                f.field_effects.aura.insert(code::MAX_MZONE, cap);
            }

            assert_eq!(f.get_mzone_limit(0, 0, 0), 2, "the lowest cap wins");
            assert_eq!(
                f.get_mzone_limit(0, 2, 0),
                5,
                "uplayer >= 2 asks about the board, ignoring the caps"
            );
            assert_eq!(f.get_szone_limit(0, 0, 0), 5, "a different zone");
        }

        /// `get_useable_count` is the free-seat count capped by the limit,
        /// so whichever is smaller decides.
        #[test]
        fn the_useable_count_is_the_smaller_of_seats_and_limit() {
            fn one(_: &Effect, _: &Field, _: &Ctx) -> i64 {
                1
            }
            let mut f = Field::new(8000);
            assert_eq!(f.get_useable_count(None, 0, location::MZONE, 0, 0, 0xff), 5);

            f.players[0].used_location = 0b111;
            assert_eq!(
                f.get_useable_count(None, 0, location::MZONE, 0, 0, 0xff),
                2,
                "seats are the binding constraint"
            );

            let mut source = Card::new(18036057, 0);
            source.current.location = location::MZONE;
            source.current.position = position::FACEUP_ATTACK;
            source.set_status(status::EFFECT_ENABLED, true);
            let source = f.new_card(source);
            let mut cap = Effect::new(effect_type::FIELD, code::MAX_MZONE);
            cap.owner = Some(source);
            cap.handler = Some(source);
            cap.effect_owner = 0;
            cap.flag[0] |= flag::PLAYER_TARGET | flag::FUNC_VALUE;
            cap.range = u16::from(location::MZONE);
            cap.s_range = u16::from(location::MZONE);
            cap.value_fn = Some(one);
            let cap = f.new_effect(cap);
            f.field_effects.aura.insert(code::MAX_MZONE, cap);
            assert_eq!(
                f.get_useable_count(None, 0, location::MZONE, 0, 0, 0xff),
                -2,
                "the cap binds, and the reference lets this go negative"
            );
        }

        /// **The card's own forcing effect is read.** Asked with no card
        /// only the player's effects are; asked with one, the card's join
        /// them.
        #[test]
        fn the_cards_own_forcing_effect_is_read() {
            let mut f = Field::new(8000);
            let mut c = Card::new(18036057, 0);
            c.current.location = location::MZONE;
            c.current.position = position::FACEUP_ATTACK;
            c.set_status(status::EFFECT_ENABLED, true);
            let c = f.new_card(c);
            let mut forcing = Effect::new(effect_type::SINGLE, code::MUST_USE_MZONE);
            forcing.owner = Some(c);
            forcing.handler = Some(c);
            // The low half is this player's row: one seat only.
            forcing.value = 0x0000_0001;
            let id = f.new_effect(forcing);
            f.cards[c].single_effect.insert(code::MUST_USE_MZONE, id);
            f.cards[c].indexer.insert(id);

            assert_eq!(
                f.get_tofield_count(
                    None,
                    0,
                    location::MZONE,
                    0,
                    Field::LOCATION_REASON_TOFIELD,
                    0xff
                ),
                5,
                "with no card, only the player's effects are read"
            );
            assert_eq!(
                f.get_tofield_count(
                    Some(c),
                    0,
                    location::MZONE,
                    0,
                    Field::LOCATION_REASON_TOFIELD,
                    0xff
                ),
                1,
                "the card forces itself into one seat"
            );
        }

        /// **`uplayer` selects which row of the packed value is read.**
        ///
        /// The effect's value carries both players' seats — low half for the
        /// asking player's own side, high half for the opponent's — so a
        /// placement made on the opponent's behalf reads a different mask
        /// entirely.
        #[test]
        fn the_asking_player_selects_the_row() {
            let mut f = Field::new(8000);
            let mut c = Card::new(18036057, 0);
            c.current.location = location::MZONE;
            c.current.position = position::FACEUP_ATTACK;
            c.set_status(status::EFFECT_ENABLED, true);
            let c = f.new_card(c);
            let mut forcing = Effect::new(effect_type::SINGLE, code::MUST_USE_MZONE);
            forcing.owner = Some(c);
            forcing.handler = Some(c);
            // One seat in the low row, three in the high one.
            forcing.value = 0x0007_0001;
            let id = f.new_effect(forcing);
            f.cards[c].single_effect.insert(code::MUST_USE_MZONE, id);
            f.cards[c].indexer.insert(id);

            assert_eq!(
                f.get_tofield_count(
                    Some(c),
                    0,
                    location::MZONE,
                    0,
                    Field::LOCATION_REASON_TOFIELD,
                    0xff
                ),
                1,
                "asked on its own behalf: the low row"
            );
            assert_eq!(
                f.get_tofield_count(
                    Some(c),
                    0,
                    location::MZONE,
                    1,
                    Field::LOCATION_REASON_TOFIELD,
                    0xff
                ),
                3,
                "asked on the opponent's: the high row"
            );
        }

        /// **`reason` reaches the forcing effect's value function**, which is
        /// how an effect tells an arrival from a change of control.
        #[test]
        fn the_reason_reaches_the_forcing_effect() {
            fn by_reason(_: &Effect, _: &Field, ctx: &Ctx) -> i64 {
                // args are (playerid, uplayer, reason).
                if ctx.args[2] == Field::LOCATION_REASON_CONTROL as i64 {
                    0x0000_0003
                } else {
                    0x0000_0001
                }
            }
            let mut f = Field::new(8000);
            let mut c = Card::new(18036057, 0);
            c.current.location = location::MZONE;
            c.current.position = position::FACEUP_ATTACK;
            c.set_status(status::EFFECT_ENABLED, true);
            let c = f.new_card(c);
            let mut forcing = Effect::new(effect_type::SINGLE, code::MUST_USE_MZONE);
            forcing.owner = Some(c);
            forcing.handler = Some(c);
            forcing.flag[0] |= flag::FUNC_VALUE;
            forcing.value_fn = Some(by_reason);
            let id = f.new_effect(forcing);
            f.cards[c].single_effect.insert(code::MUST_USE_MZONE, id);
            f.cards[c].indexer.insert(id);

            assert_eq!(
                f.get_tofield_count(
                    Some(c),
                    0,
                    location::MZONE,
                    0,
                    Field::LOCATION_REASON_TOFIELD,
                    0xff
                ),
                1
            );
            assert_eq!(
                f.get_tofield_count(
                    Some(c),
                    0,
                    location::MZONE,
                    0,
                    Field::LOCATION_REASON_CONTROL,
                    0xff
                ),
                2,
                "a change of control is a different question"
            );
        }

        /// **A card in the Extra Deck takes the other branch**, which forces
        /// the reason to `LOCATION_REASON_TOFIELD` — so a caller asking with
        /// `LOCATION_REASON_CONTROL` gets the arrival limit instead.
        #[test]
        fn an_extra_deck_card_is_asked_about_with_the_arrival_reason() {
            fn by_reason(_: &Effect, _: &Field, ctx: &Ctx) -> i64 {
                if ctx.args[2] == Field::LOCATION_REASON_CONTROL as i64 {
                    1
                } else {
                    3
                }
            }
            let mut f = Field::new(8000);
            let mut source = Card::new(18036057, 0);
            source.current.location = location::MZONE;
            source.current.position = position::FACEUP_ATTACK;
            source.set_status(status::EFFECT_ENABLED, true);
            let source = f.new_card(source);
            let mut cap = Effect::new(effect_type::FIELD, code::MAX_MZONE);
            cap.owner = Some(source);
            cap.handler = Some(source);
            cap.effect_owner = 0;
            cap.flag[0] |= flag::PLAYER_TARGET | flag::FUNC_VALUE;
            cap.range = u16::from(location::MZONE);
            cap.s_range = u16::from(location::MZONE);
            cap.value_fn = Some(by_reason);
            let cap = f.new_effect(cap);
            f.field_effects.aura.insert(code::MAX_MZONE, cap);

            let mut extra = Card::new(63519819, 0);
            extra.current.location = location::EXTRA;
            let extra = f.new_card(extra);
            let mut hand = Card::new(63519819, 0);
            hand.current.location = location::HAND;
            let hand = f.new_card(hand);

            assert_eq!(
                f.get_useable_count(
                    Some(hand),
                    0,
                    location::MZONE,
                    0,
                    Field::LOCATION_REASON_CONTROL,
                    0xff
                ),
                1,
                "an ordinary card keeps the reason it was asked with"
            );
            assert_eq!(
                f.get_useable_count(
                    Some(extra),
                    0,
                    location::MZONE,
                    0,
                    Field::LOCATION_REASON_CONTROL,
                    0xff
                ),
                3,
                "an Extra Deck card is asked about as an arrival"
            );
        }

        /// **The two Extra Monster Zone bits are marked unavailable after
        /// the count**, so a zone selection is never offered two seats this
        /// path cannot place into. They are set for a Monster Zone question
        /// and not for a spell/trap one.
        #[test]
        fn the_extra_monster_zone_bits_are_marked_after_counting() {
            let mut f = Field::new(8000);
            let (count, mask) = f.get_tofield_count_with_flag(
                None,
                0,
                location::MZONE,
                0,
                Field::LOCATION_REASON_TOFIELD,
                0xff,
            );
            assert_eq!(count, 5, "all five main seats counted");
            assert_ne!(mask & ((1 << 5) | (1 << 6)), 0, "and the two extras marked");

            let (_, mask) = f.get_tofield_count_with_flag(
                None,
                0,
                location::SZONE,
                0,
                Field::LOCATION_REASON_TOFIELD,
                0xff,
            );
            assert_eq!(
                mask & ((1 << 5) | (1 << 6)),
                0,
                "a spell/trap row has no extras to mark"
            );
        }

        /// The caller's `zone` mask narrows which seats count.
        #[test]
        fn the_zone_mask_narrows_the_seats() {
            let mut f = Field::new(8000);
            assert_eq!(
                f.get_tofield_count(
                    None,
                    0,
                    location::SZONE,
                    0,
                    Field::LOCATION_REASON_TOFIELD,
                    0xff
                ),
                5
            );
            assert_eq!(
                f.get_tofield_count(
                    None,
                    0,
                    location::SZONE,
                    0,
                    Field::LOCATION_REASON_TOFIELD,
                    0b00011
                ),
                2
            );
            assert_eq!(
                f.get_tofield_count(
                    None,
                    0,
                    location::GRAVE,
                    0,
                    Field::LOCATION_REASON_TOFIELD,
                    0xff
                ),
                0,
                "not a field zone"
            );
        }
    }

    mod uniqueness {
        use super::*;

        fn unique_card(f: &mut Field, controller: u8, code: u32, seat: usize) -> CardId {
            let mut c = Card::new(code, controller);
            c.current.controller = controller;
            c.current.location = location::MZONE;
            c.current.sequence = seat as u32;
            c.current.position = position::FACEUP_ATTACK;
            c.set_status(status::EFFECT_ENABLED, true);
            c.unique_code = code;
            c.unique_location = u16::from(location::MZONE);
            c.unique_pos = [1, 0];
            c.unique_fieldid = 1;
            let id = f.new_card(c);
            f.players[controller as usize].mzone[seat] = Some(id);
            f.core.unique_cards[controller as usize].push(id);
            let mut e = Effect::new(effect_type::SINGLE, 0);
            e.owner = Some(id);
            e.handler = Some(id);
            let e = f.new_effect(e);
            f.cards[id].unique_effect = Some(e);
            id
        }

        /// A card already on the field blocks another of the same name.
        #[test]
        fn an_existing_unique_card_blocks_another() {
            let mut f = Field::new(8000);
            let first = unique_card(&mut f, 0, 12345, 0);
            let second = unique_card(&mut f, 0, 12345, 1);

            assert!(
                f.check_unique_onfield(second, 0, u16::from(location::MZONE), None)
                    .is_some(),
                "the first one forbids it"
            );
            assert!(
                f.check_unique_onfield(second, 0, u16::from(location::MZONE), Some(first))
                    .is_none(),
                "unless the first is the card being replaced"
            );
        }

        /// A disabled or face-down copy imposes nothing.
        #[test]
        fn a_disabled_unique_card_imposes_nothing() {
            let mut f = Field::new(8000);
            let first = unique_card(&mut f, 0, 12345, 0);
            let second = unique_card(&mut f, 0, 12345, 1);
            f.cards[first].set_status(status::DISABLED, true);
            // The second still counts itself plus the first via the target
            // scan, so clear that one's limit to isolate the first half.
            f.cards[second].unique_code = 0;
            assert!(f
                .check_unique_onfield(second, 0, u16::from(location::MZONE), None)
                .is_none());
        }

        /// The limit only covers the zones it names.
        #[test]
        fn the_limit_covers_only_the_zones_it_names() {
            let mut f = Field::new(8000);
            unique_card(&mut f, 0, 12345, 0);
            let second = unique_card(&mut f, 0, 12345, 1);
            assert!(f
                .check_unique_onfield(second, 0, u16::from(location::SZONE), None)
                .is_none());
        }

        /// A card with no limit of its own is not covered by its own scan.
        #[test]
        fn a_card_without_a_limit_imposes_none() {
            let mut f = Field::new(8000);
            let mut plain = Card::new(12345, 0);
            plain.current.location = location::MZONE;
            plain.current.position = position::FACEUP_ATTACK;
            let plain = f.new_card(plain);
            assert!(f
                .check_unique_onfield(plain, 0, u16::from(location::MZONE), None)
                .is_none());
        }
    }

    mod applicable_effects {
        use super::*;

        /// A face-up monster with a continuous single effect on it.
        fn monster(f: &mut Field, controller: u8) -> CardId {
            let mut c = Card::new(18036057, controller);
            c.current.controller = controller;
            c.current.location = location::MZONE;
            c.current.position = position::FACEUP_ATTACK;
            c.set_status(status::EFFECT_ENABLED, true);
            f.new_card(c)
        }

        fn single_effect_on(f: &mut Field, card: CardId, code: u32) -> EffectId {
            let mut e = Effect::new(effect_type::SINGLE, code);
            e.owner = Some(card);
            e.handler = Some(card);
            let id = f.new_effect(e);
            f.cards[card].single_effect.insert(code, id);
            id
        }

        /// An action effect is never "available": availability is about
        /// effects that are simply in force, and an action effect has to be
        /// activated instead.
        #[test]
        fn an_action_effect_is_never_available() {
            let mut f = Field::new(8000);
            let card = monster(&mut f, 0);
            let mut e = Effect::new(effect_type::SINGLE | effect_type::ACTIONS, 42);
            e.owner = Some(card);
            e.handler = Some(card);
            let id = f.new_effect(e);
            assert!(!f.is_available(id));
        }

        #[test]
        fn a_continuous_single_effect_on_a_face_up_monster_is_available() {
            let mut f = Field::new(8000);
            let card = monster(&mut f, 0);
            let e = single_effect_on(&mut f, card, 42);
            assert!(f.is_available(e));
        }

        /// **A continuous effect's condition is consulted**, which is
        /// the reference's last three lines of `is_available` and was
        /// missing here until Reaper on the Nightmare — the pool's first
        /// continuous effect with a condition — needed it.
        ///
        /// Without this every conditional continuous effect is
        /// unconditional: an `EFFECT_SELF_DESTROY` that should fire only
        /// when something targets its card fires the moment the card is
        /// summoned, and nothing else in the engine notices.
        #[test]
        fn a_continuous_effects_condition_decides_whether_it_is_in_force() {
            fn only_for_the_second_card(f: &Field, e: EffectId) -> bool {
                f.effects
                    .get(e)
                    .and_then(|x| x.handler)
                    .is_some_and(|h| h == 1)
            }
            let mut f = Field::new(8000);
            let first = monster(&mut f, 0);
            let second = monster(&mut f, 0);
            assert_eq!((first, second), (0, 1), "the ids the condition names");

            let a = single_effect_on(&mut f, first, 42);
            let b = single_effect_on(&mut f, second, 42);
            assert!(f.is_available(a), "no condition means in force");

            f.effects.get_mut(a).unwrap().avail_condition = Some(only_for_the_second_card);
            f.effects.get_mut(b).unwrap().avail_condition = Some(only_for_the_second_card);
            assert!(!f.is_available(a), "the condition said no");
            assert!(f.is_available(b), "and yes");
        }

        /// **A condition turning true renumbers the effect, once.** The
        /// reference's `EFFECT_STATUS_AVAILABLE` bookkeeping
        /// (`effect.cpp`): a passing condition marks the effect available
        /// and, if it was not before, gives it a fresh id — so it sorts
        /// after everything already in force, which is what decides the
        /// order of two Chaos monsters in the special-summon menu (three
        /// of a million pool games). A failing condition clears the
        /// mark; the gates before the condition leave it alone, so an
        /// effect shut off by its handler's state and let back in does
        /// **not** renumber.
        #[test]
        fn a_condition_turning_true_renumbers_the_effect_once() {
            fn from_turn_three(f: &Field, _e: EffectId) -> bool {
                f.infos.turn_id >= 3
            }
            let mut f = Field::new(8000);
            let card = monster(&mut f, 0);
            let e = single_effect_on(&mut f, card, 42);
            f.effects.get_mut(e).unwrap().avail_condition = Some(from_turn_three);
            let born = f.effects.get(e).unwrap().id.get();

            f.infos.turn_id = 1;
            assert!(!f.is_available(e));
            assert_eq!(
                f.effects.get(e).unwrap().id.get(),
                born,
                "failing: no renumber"
            );
            assert!(!f.effects.get(e).unwrap().available.get());

            f.infos.turn_id = 3;
            let counter = f.infos.field_id.get();
            assert!(f.is_available(e));
            let renumbered = f.effects.get(e).unwrap().id.get();
            assert_eq!(renumbered, counter, "the next id off the counter");
            assert!(renumbered > born, "and newer than it was");
            assert!(f.effects.get(e).unwrap().available.get());

            assert!(f.is_available(e));
            assert_eq!(
                f.effects.get(e).unwrap().id.get(),
                renumbered,
                "passing again: once"
            );

            f.infos.turn_id = 1;
            assert!(!f.is_available(e));
            assert!(
                !f.effects.get(e).unwrap().available.get(),
                "failing clears the mark"
            );
            f.infos.turn_id = 3;
            assert!(f.is_available(e));
            assert!(
                f.effects.get(e).unwrap().id.get() > renumbered,
                "and the next turn true renumbers again"
            );

            // A gate before the condition: the handler disabled. The
            // reference returns before the bookkeeping, so the mark stays
            // and the return to force does not renumber.
            let held = f.effects.get(e).unwrap().id.get();
            f.cards[card].set_status(status::DISABLED, true);
            assert!(!f.is_available(e), "shut off by the gate");
            assert!(
                f.effects.get(e).unwrap().available.get(),
                "the mark untouched"
            );
            f.cards[card].set_status(status::DISABLED, false);
            assert!(f.is_available(e));
            assert_eq!(
                f.effects.get(e).unwrap().id.get(),
                held,
                "no renumber on the way back"
            );
        }

        /// **The continuous branch of `is_activateable`, arm by arm.**
        ///
        /// Reaper on the Nightmare exercises the face-up test and nothing
        /// else here; the other three have no card in this pool to bite
        /// on, so they are driven directly rather than left as the only
        /// untested checks in a group of siblings.
        #[test]
        fn the_continuous_branch_refuses_four_kinds_of_handler() {
            fn field_continuous(f: &mut Field, card: CardId) -> EffectId {
                let mut e = Effect::new(
                    effect_type::FIELD | effect_type::CONTINUOUS | effect_type::ACTIONS,
                    42,
                );
                e.owner = Some(card);
                e.handler = Some(card);
                e.range = u16::from(location::MZONE);
                let id = f.new_effect(e);
                f.cards[card].field_effect.insert(42, id);
                id
            }
            let ev = Event::new(42);
            let ask = |f: &mut Field, e: EffectId| {
                f.is_activateable(e, 0, &ev, false, false, false, false, false)
            };

            let mut f = Field::new(8000);
            let card = monster(&mut f, 0);
            f.add_card(0, card, location::MZONE, 0, false);
            f.cards[card].current.position = position::FACEUP_ATTACK;
            f.cards[card].set_status(status::EFFECT_ENABLED, true);
            let e = field_continuous(&mut f, card);
            assert!(ask(&mut f, e), "the baseline");

            // **Battle-destroyed**, and still on the field: not live.
            f.cards[card].set_status(status::BATTLE_DESTROYED, true);
            assert!(!ask(&mut f, e));
            f.cards[card].set_status(status::BATTLE_DESTROYED, false);
            assert!(ask(&mut f, e));

            // **The location test is part of the face-up one.** A
            // face-down card *off* the field is not refused by it.
            f.cards[card].current.position = position::FACEDOWN_DEFENSE;
            assert!(!ask(&mut f, e), "face-down on the field");
            f.cards[card].current.location = location::GRAVE;
            assert!(ask(&mut f, e), "and face-down off it is not the test");
            f.cards[card].current.location = location::MZONE;
            f.cards[card].current.position = position::FACEUP_ATTACK;

            // **The owner gates.** Owner and handler are the same card,
            // so a disable on it shuts its own effect off.
            f.cards[card].set_status(status::DISABLED, true);
            assert!(!ask(&mut f, e));
            f.effects.get_mut(e).unwrap().flag[0] |= flag::CANNOT_DISABLE;
            assert!(ask(&mut f, e), "unless it says it cannot be disabled");
        }

        /// And the condition is consulted **last**: an effect its earlier
        /// gates have already rejected never runs one.
        #[test]
        fn the_condition_is_not_reached_by_an_effect_already_rejected() {
            fn boom(_: &Field, _: EffectId) -> bool {
                panic!("the condition must not be reached")
            }
            let mut f = Field::new(8000);
            let card = monster(&mut f, 0);
            let e = single_effect_on(&mut f, card, 42);
            f.effects.get_mut(e).unwrap().avail_condition = Some(boom);
            f.cards[card].set_status(status::FORBIDDEN, true);
            assert!(!f.is_available(e));
        }

        /// The forbidden and disabled gates. An effect whose owner is its
        /// handler is shut off by that card's own state.
        #[test]
        fn a_disabled_card_stops_its_own_effects() {
            let mut f = Field::new(8000);
            let card = monster(&mut f, 0);
            let e = single_effect_on(&mut f, card, 42);
            f.cards[card].set_status(status::DISABLED, true);
            assert!(!f.is_available(e));

            f.effects.get_mut(e).unwrap().flag[0] |= flag::CANNOT_DISABLE;
            assert!(f.is_available(e), "unless it says it cannot be disabled");
        }

        #[test]
        fn a_forbidden_card_stops_its_own_effects() {
            let mut f = Field::new(8000);
            let card = monster(&mut f, 0);
            let e = single_effect_on(&mut f, card, 42);
            f.cards[card].set_status(status::FORBIDDEN, true);
            assert!(!f.is_available(e));
        }

        /// `filter_effect` reaches all five sources. Here: the card's own,
        /// an equipped card's, and a field-wide aura.
        #[test]
        fn effects_are_gathered_from_every_source() {
            let mut f = Field::new(8000);
            let card = monster(&mut f, 0);
            let own = single_effect_on(&mut f, card, 42);

            let equipper = monster(&mut f, 0);
            let mut eq = Effect::new(effect_type::EQUIP, 42);
            eq.owner = Some(equipper);
            eq.handler = Some(equipper);
            let eq = f.new_effect(eq);
            f.cards[equipper].equip_effect.insert(42, eq);
            f.cards[card].equiping_cards.push(equipper);

            let source = monster(&mut f, 0);
            let mut aura = Effect::new(effect_type::FIELD, 42);
            aura.owner = Some(source);
            aura.handler = Some(source);
            aura.range = u16::from(location::MZONE);
            aura.s_range = u16::from(location::MZONE);
            aura.o_range = u16::from(location::MZONE);
            let aura = f.new_effect(aura);
            f.field_effects.aura.insert(42, aura);

            let got = f.filter_effect(card, 42);
            assert_eq!(got, vec![own, eq, aura], "own, then equips, then auras");
            assert_eq!(f.filter_effect(card, 43), Vec::<EffectId>::new());
        }

        /// `is_affected_by_effect` is the same traversal stopping at the
        /// first match — which is why they are written once here.
        #[test]
        fn the_first_match_is_the_same_traversal() {
            let mut f = Field::new(8000);
            let card = monster(&mut f, 0);
            let own = single_effect_on(&mut f, card, 42);
            single_effect_on(&mut f, card, 42);
            assert_eq!(f.is_affected_by_effect(card, 42), Some(own));
            assert_eq!(f.is_affected_by_effect(card, 43), None);
        }

        /// An aura aimed at players is not an effect on a card, and the
        /// card traversal must skip it.
        #[test]
        fn a_player_aura_is_not_gathered_onto_a_card() {
            let mut f = Field::new(8000);
            let card = monster(&mut f, 0);
            let source = monster(&mut f, 0);
            let mut aura = Effect::new(effect_type::FIELD, 42);
            aura.owner = Some(source);
            aura.handler = Some(source);
            aura.flag[0] |= flag::PLAYER_TARGET;
            aura.range = u16::from(location::MZONE);
            aura.s_range = u16::from(location::MZONE);
            let aura = f.new_effect(aura);
            f.field_effects.aura.insert(42, aura);

            assert!(f.filter_effect(card, 42).is_empty());
            assert_eq!(
                f.filter_player_effect(0, 42),
                vec![aura],
                "but it is a player effect"
            );
            assert!(f.filter_player_effect(1, 42).is_empty());
        }

        /// A card being summoned is untouchable — except by the two codes
        /// that exist to interrupt a summon.
        #[test]
        fn a_summoning_card_is_untouchable_with_two_exceptions() {
            let mut f = Field::new(8000);
            let card = monster(&mut f, 0);
            let ordinary = single_effect_on(&mut f, card, 42);
            let interrupt = single_effect_on(&mut f, card, code::CANNOT_DISABLE_SUMMON);
            f.cards[card].set_status(status::SUMMONING, true);

            assert!(!f.is_affect_by_effect(card, Some(ordinary)));
            assert!(f.is_affect_by_effect(card, Some(interrupt)));
        }

        /// A null effect is a rule-driven action with no card behind it, and
        /// those still reach a summoning card. The reference's `peffect &&`
        /// is what makes that so, and it is not defensive programming.
        #[test]
        fn the_rules_themselves_still_reach_a_summoning_card() {
            let mut f = Field::new(8000);
            let card = monster(&mut f, 0);
            f.cards[card].set_status(status::SUMMONING, true);
            assert!(f.is_affect_by_effect(card, None));
        }

        /// Immunity, and the flag that overrides it.
        #[test]
        fn an_immune_card_shrugs_an_effect_off() {
            fn immune_to_everything(_: &Effect, _: &Field, _: &Ctx) -> i64 {
                1
            }
            let mut f = Field::new(8000);
            let card = monster(&mut f, 0);
            let e = single_effect_on(&mut f, card, 42);

            let mut immunity = Effect::new(effect_type::SINGLE, 1);
            immunity.owner = Some(card);
            immunity.handler = Some(card);
            immunity.flag[0] |= flag::FUNC_VALUE;
            immunity.value_fn = Some(immune_to_everything);
            let immunity = f.new_effect(immunity);
            f.cards[card].immune_effect.push(immunity);

            assert!(!f.is_affect_by_effect(card, Some(e)));

            f.effects.get_mut(e).unwrap().flag[0] |= flag::IGNORE_IMMUNE;
            assert!(
                f.is_affect_by_effect(card, Some(e)),
                "IGNORE_IMMUNE goes through"
            );
        }

        /// An immunity effect with no value at all grants nothing.
        #[test]
        fn an_immunity_without_a_value_grants_nothing() {
            let mut f = Field::new(8000);
            let card = monster(&mut f, 0);
            let e = single_effect_on(&mut f, card, 42);
            let mut immunity = Effect::new(effect_type::SINGLE, 1);
            immunity.owner = Some(card);
            immunity.handler = Some(card);
            let immunity = f.new_effect(immunity);
            f.cards[card].immune_effect.push(immunity);
            assert!(f.is_affect_by_effect(card, Some(e)));
        }
    }

    mod activation_counts {
        use super::*;

        /// The three scopes are three maps, because they reset at different
        /// times. A count in one must not be visible in another.
        #[test]
        fn the_three_scopes_are_separate_tallies() {
            let mut f = Field::new(8000);
            f.add_effect_code(99, 0, 0, 0);
            f.add_effect_code(99, effect_count::DUEL, 0, 0);

            assert_eq!(f.get_effect_code(99, 0, 0, 0), 1);
            assert_eq!(f.get_effect_code(99, effect_count::DUEL, 0, 0), 1);
            assert_eq!(f.get_effect_code(99, effect_count::CHAIN, 0, 0), 0);
        }

        /// Each part of the key distinguishes a tally.
        #[test]
        fn the_key_distinguishes_code_player_and_index() {
            let mut f = Field::new(8000);
            f.add_effect_code(99, 0, 0, 0);
            assert_eq!(f.get_effect_code(99, 0, 0, 0), 1);
            assert_eq!(f.get_effect_code(98, 0, 0, 0), 0, "a different code");
            assert_eq!(f.get_effect_code(99, 0, 0, 1), 0, "a different player");
            assert_eq!(f.get_effect_code(99, 0, 1, 0), 0, "a different index");
        }

        /// Giving a count back — what an oath effect does when its
        /// activation is negated — and never below zero.
        #[test]
        fn a_count_can_be_given_back_and_does_not_underflow() {
            let mut f = Field::new(8000);
            f.add_effect_code(99, 0, 0, 0);
            f.dec_effect_code(99, 0, 0, 0);
            assert_eq!(f.get_effect_code(99, 0, 0, 0), 0);
            f.dec_effect_code(99, 0, 0, 0);
            assert_eq!(f.get_effect_code(99, 0, 0, 0), 0);
        }

        /// An effect with no count limit is always available.
        #[test]
        fn an_effect_without_a_count_limit_is_unlimited() {
            let mut f = Field::new(8000);
            let e = f.new_effect(Effect::new(effect_type::TRIGGER_O, code::CHAINING));
            assert!(f.check_count_limit(e, 0));
        }

        /// Once per turn: the tally is against `count_code`, which every
        /// copy of a card name shares.
        #[test]
        fn once_per_turn_is_shared_between_copies() {
            let mut f = Field::new(8000);
            let mut effect = Effect::new(effect_type::TRIGGER_O, code::CHAINING);
            effect.flag[0] |= flag::COUNT_LIMIT;
            effect.count_limit = 1;
            effect.count_limit_max = 1;
            effect.count_code = 12345;
            effect.count_flag = effect_count::OATH;
            let a = f.new_effect(effect.clone());
            let b = f.new_effect(effect);

            assert!(f.check_count_limit(a, 0));
            assert!(f.check_count_limit(b, 0));

            f.add_effect_code(12345, effect_count::OATH, 0, 0);
            assert!(!f.check_count_limit(a, 0));
            assert!(
                !f.check_count_limit(b, 0),
                "the other copy shares the tally"
            );
            assert!(f.check_count_limit(a, 1), "but the opponent has their own");
        }

        /// Hard once per turn: `SINGLE` tallies against the card's own field
        /// id, so each copy keeps its own count.
        #[test]
        fn a_hard_once_per_turn_is_tallied_per_card() {
            let mut f = Field::new(8000);
            let mut first = Card::new(31560081, 0);
            first.fieldid = 7;
            let mut second = Card::new(31560081, 0);
            second.fieldid = 8;
            let (c1, c2) = (f.new_card(first), f.new_card(second));

            let mut effect = Effect::new(effect_type::TRIGGER_O, code::CHAINING);
            effect.flag[0] |= flag::COUNT_LIMIT;
            effect.count_limit = 1;
            effect.count_limit_max = 1;
            effect.count_flag = effect_count::SINGLE;
            let mut a = effect.clone();
            a.handler = Some(c1);
            let mut b = effect;
            b.handler = Some(c2);
            let (a, b) = (f.new_effect(a), f.new_effect(b));

            f.add_effect_code(7, effect_count::SINGLE, 0, PLAYER_NONE);
            assert!(!f.check_count_limit(a, 0), "that copy has used it");
            assert!(f.check_count_limit(b, 0), "this one has not");
        }

        /// A count limit of zero is spent, whatever the tally says.
        #[test]
        fn a_spent_effect_is_unavailable() {
            let mut f = Field::new(8000);
            let mut effect = Effect::new(effect_type::TRIGGER_O, code::CHAINING);
            effect.flag[0] |= flag::COUNT_LIMIT;
            effect.count_limit = 0;
            let e = f.new_effect(effect);
            assert!(!f.check_count_limit(e, 0));
        }
    }

    mod condition_check {
        use super::*;

        fn effect_on(f: &mut Field, card: CardId, ty: u16) -> EffectId {
            let mut e = Effect::new(ty, code::CHAINING);
            e.handler = Some(card);
            f.new_effect(e)
        }

        /// An effect with no condition passes — a null Lua reference means
        /// "always" in the reference, and means the same here.
        #[test]
        fn a_missing_condition_admits_everything() {
            let (mut f, card) = field_with_card(0);
            f.cards[card].current.position = position::FACEUP_ATTACK;
            let e = effect_on(&mut f, card, effect_type::TRIGGER_O);
            assert!(f.is_condition_check(e, 0, &Event::new(code::CHAINING)));
        }

        #[test]
        fn a_condition_is_consulted_when_present() {
            fn only_player_one(_: &mut Field, ctx: &Ctx) -> bool {
                ctx.player == 1
            }
            let (mut f, card) = field_with_card(0);
            f.cards[card].current.position = position::FACEUP_ATTACK;
            let e = effect_on(&mut f, card, effect_type::TRIGGER_O);
            f.effects.get_mut(e).unwrap().condition = Some(only_player_one);

            let ev = Event::new(code::CHAINING);
            assert!(!f.is_condition_check(e, 0, &ev));
            assert!(f.is_condition_check(e, 1, &ev));
        }

        /// A face-down card on the field cannot see the event at all, and
        /// the test happens before the condition is ever consulted.
        #[test]
        fn a_face_down_card_on_the_field_sees_nothing() {
            fn always(_: &mut Field, _: &Ctx) -> bool {
                true
            }
            let (mut f, card) = field_with_card(0);
            f.cards[card].current.position = position::FACEDOWN_DEFENSE;
            let e = effect_on(&mut f, card, effect_type::TRIGGER_O);
            f.effects.get_mut(e).unwrap().condition = Some(always);
            assert!(!f.is_condition_check(e, 0, &Event::new(code::CHAINING)));
        }

        /// Except for an activate effect, which is how a set card activates
        /// in the first place.
        #[test]
        fn an_activate_effect_is_exempt_from_the_face_up_test() {
            let (mut f, card) = field_with_card(0);
            f.cards[card].current.position = position::FACEDOWN_DEFENSE;
            let e = effect_on(&mut f, card, effect_type::ACTIVATE);
            assert!(f.is_condition_check(e, 0, &Event::new(code::CHAINING)));
        }

        /// And the face-up test applies only on the field (and to banished
        /// cards); a face-down card in the graveyard is not a thing the
        /// test is about.
        #[test]
        fn the_face_up_test_applies_only_on_the_field_or_banished() {
            let (mut f, card) = field_with_card(0);
            f.cards[card].current.location = location::GRAVE;
            f.cards[card].current.position = position::FACEDOWN_DEFENSE;
            let e = effect_on(&mut f, card, effect_type::TRIGGER_O);
            assert!(f.is_condition_check(e, 0, &Event::new(code::CHAINING)));
        }

        /// The callback is told which effect and player the question is
        /// being asked for, and the field is restored afterwards.
        #[test]
        fn the_reason_is_set_during_the_callback_and_restored_after() {
            fn checks_reason(f: &mut Field, ctx: &Ctx) -> bool {
                f.core.reason_effect == Some(ctx.reason_effect)
                    && f.core.reason_player == ctx.player
            }
            let (mut f, card) = field_with_card(0);
            f.cards[card].current.position = position::FACEUP_ATTACK;
            let e = effect_on(&mut f, card, effect_type::TRIGGER_O);
            f.effects.get_mut(e).unwrap().condition = Some(checks_reason);

            f.core.reason_player = PLAYER_NONE;
            assert!(f.is_condition_check(e, 1, &Event::new(code::CHAINING)));
            assert_eq!(f.core.reason_effect, None, "restored");
            assert_eq!(f.core.reason_player, PLAYER_NONE, "restored");
        }
    }

    mod activate_ready {
        use super::*;

        fn ready_effect(f: &mut Field, card: CardId) -> EffectId {
            let mut e = Effect::new(effect_type::TRIGGER_O, code::CHAINING);
            e.handler = Some(card);
            f.new_effect(e)
        }

        /// Cost and target are *asked*, not performed. `chk == false` is
        /// what makes "could this be activated?" a question rather than a
        /// payment, and a port that passed `true` would charge for asking.
        #[test]
        fn cost_and_target_are_asked_rather_than_paid() {
            fn run(f: &mut Field, chk: bool) -> bool {
                assert!(!chk, "activation legality must not pay the cost");
                f.players[0].lp -= 1; // observable: proof it ran
                true
            }
            // A cost and a target have different signatures now that a
            // target also takes the check-this-card argument, so the one
            // body is reached through two thin adapters.
            fn refuses_when_asked(f: &mut Field, _: &Ctx, chk: bool) -> bool {
                run(f, chk)
            }
            fn refuses_when_asked_t(
                f: &mut Field,
                _: &Ctx,
                chk: bool,
                _chkc: Option<CardId>,
            ) -> crate::effect::Yield {
                crate::effect::Yield::Done(i32::from(run(f, chk)))
            }
            let (mut f, card) = field_with_card(0);
            let e = ready_effect(&mut f, card);
            f.effects.get_mut(e).unwrap().cost = Some(refuses_when_asked);
            f.effects.get_mut(e).unwrap().target = Some(refuses_when_asked_t);

            assert!(f.is_activate_ready(e, e, 0, &Event::new(code::CHAINING), false, false, false));
            assert_eq!(f.players[0].lp, 7998, "both ran, both were only asked");
        }

        /// Any of the three refusing is a refusal.
        #[test]
        fn a_refusal_from_any_of_the_three_is_a_refusal() {
            fn no(_: &mut Field, _: &Ctx) -> bool {
                false
            }
            fn no_mut(_: &mut Field, _: &Ctx, _: bool) -> bool {
                false
            }
            fn no_mut_t(
                _: &mut Field,
                _: &Ctx,
                _: bool,
                _: Option<CardId>,
            ) -> crate::effect::Yield {
                crate::effect::Yield::Done(0)
            }
            let ev = Event::new(code::CHAINING);

            let (mut f, card) = field_with_card(0);
            let e = ready_effect(&mut f, card);
            f.effects.get_mut(e).unwrap().condition = Some(no);
            assert!(!f.is_activate_ready(e, e, 0, &ev, false, false, false));
            assert!(
                f.is_activate_ready(e, e, 0, &ev, true, false, false),
                "unless the caller says to neglect it"
            );

            let (mut f, card) = field_with_card(0);
            let e = ready_effect(&mut f, card);
            f.effects.get_mut(e).unwrap().cost = Some(no_mut);
            assert!(!f.is_activate_ready(e, e, 0, &ev, false, false, false));
            assert!(f.is_activate_ready(e, e, 0, &ev, false, true, false));

            let (mut f, card) = field_with_card(0);
            let e = ready_effect(&mut f, card);
            f.effects.get_mut(e).unwrap().target = Some(no_mut_t);
            assert!(!f.is_activate_ready(e, e, 0, &ev, false, false, false));
            assert!(f.is_activate_ready(e, e, 0, &ev, false, false, true));
        }

        /// A continuous effect skips the cost check entirely — it has no
        /// activation cost to pay.
        #[test]
        fn a_continuous_effect_skips_the_cost() {
            fn no_mut(_: &mut Field, _: &Ctx, _: bool) -> bool {
                false
            }
            let (mut f, card) = field_with_card(0);
            let mut e = Effect::new(effect_type::CONTINUOUS, code::CHAINING);
            e.handler = Some(card);
            e.cost = Some(no_mut);
            let e = f.new_effect(e);
            assert!(
                f.is_activate_ready(e, e, 0, &Event::new(code::CHAINING), false, false, false),
                "the cost is not consulted for a continuous effect"
            );
        }
    }

    mod zone_usability {
        use super::*;

        /// The reference's 32-entry lookup table is `popcnt` of the index,
        /// so this is `count_ones` — and only over the five main seats.
        #[test]
        fn the_used_count_is_a_popcount_of_five_bits() {
            assert_eq!(Field::field_used_count(0b00000), 0);
            assert_eq!(Field::field_used_count(0b10101), 3);
            assert_eq!(Field::field_used_count(0b11111), 5);
            assert_eq!(
                Field::field_used_count(0b1111_11111),
                5,
                "bits above the five main seats are not counted"
            );
        }

        /// Monster seats are the low bits; spell/trap seats start at bit 8.
        /// Sharing a bitfield means the offsets are the behaviour.
        #[test]
        fn monster_and_spell_seats_use_different_halves_of_the_mask() {
            let mut f = Field::new(8000);
            f.players[0].used_location = 0b100; // monster seat 2

            assert!(!f.is_location_useable(0, u16::from(location::MZONE), 2));
            assert!(f.is_location_useable(0, u16::from(location::MZONE), 1));
            assert!(
                f.is_location_useable(0, u16::from(location::SZONE), 2),
                "a taken monster seat does not take the spell/trap seat"
            );

            f.players[0].used_location = 0x100 << 2; // spell/trap seat 2
            assert!(!f.is_location_useable(0, u16::from(location::SZONE), 2));
            assert!(f.is_location_useable(0, u16::from(location::MZONE), 2));
        }

        /// A disabled seat is as unusable as an occupied one, which is why
        /// the reference reads the two masks together.
        #[test]
        fn a_disabled_seat_is_unusable() {
            let mut f = Field::new(8000);
            f.players[0].disabled_location = 0b1;
            assert!(!f.is_location_useable(0, u16::from(location::MZONE), 0));
        }

        /// The symbolic locations are rewritten into a real location and an
        /// adjusted sequence before the mask is consulted. `EMZONE` seat 0
        /// is monster seat 5.
        #[test]
        fn the_extra_monster_zone_is_the_sixth_monster_seat() {
            let mut f = Field::new(8000);
            f.players[0].used_location = 1 << 5;
            assert!(!f.is_location_useable(0, location::EMZONE, 0));
            assert!(f.is_location_useable(0, location::EMZONE, 1));
        }

        /// The one case that reaches across the table: an Extra Monster Zone
        /// is shared, so a seat is unusable when the opponent holds the seat
        /// facing it.
        #[test]
        fn an_extra_monster_zone_is_blocked_by_the_opponent_opposite() {
            let mut f = Field::new(8000);
            f.players[1].used_location = 1 << 6; // 11 - 5
            assert!(
                !f.is_location_useable(0, location::EMZONE, 0),
                "the opponent occupies the facing seat"
            );
            assert!(
                f.is_location_useable(0, u16::from(location::MZONE), 1),
                "but an ordinary seat is unaffected by the opponent"
            );
        }

        /// The field zone is the sixth spell/trap seat in the mask.
        #[test]
        fn the_field_zone_is_the_sixth_spell_trap_seat() {
            let mut f = Field::new(8000);
            f.players[0].used_location = 0x100 << 5;
            assert!(!f.is_location_useable(0, location::FZONE, 0));
        }

        /// Pendulum Zones move depending on the duel options, which is why
        /// the reference computes the index rather than fixing it.
        #[test]
        fn pendulum_seats_depend_on_the_duel_options() {
            let default = Field::new(8000);
            assert_eq!(default.pzone_index(0), 0);
            assert_eq!(default.pzone_index(1), 4, "the outermost seats");

            let separate = Field::with_flags(8000, flags::PZONE | flags::SEPARATE_PZONE);
            assert_eq!(separate.pzone_index(0), 6);
            assert_eq!(separate.pzone_index(1), 7);

            let three = Field::with_flags(8000, flags::PZONE | flags::THREE_COLUMNS_FIELD);
            assert_eq!(three.pzone_index(0), 1);
            assert_eq!(three.pzone_index(1), 3);
        }

        /// Without the Pendulum Zone option the seat does not exist at all.
        #[test]
        fn there_are_no_pendulum_zones_without_the_option() {
            let with = Field::with_flags(8000, flags::PZONE);
            assert!(with.is_location_useable(0, location::PZONE, 0));

            let without = Field::with_flags(8000, 0);
            assert!(!without.is_location_useable(0, location::PZONE, 0));
        }

        /// A three-column field shifts the main seats along by one, and the
        /// shift is applied before the mask is read.
        #[test]
        fn a_three_column_field_shifts_the_main_seats() {
            let mut f = Field::with_flags(8000, flags::THREE_COLUMNS_FIELD);
            f.players[0].used_location = 1 << 1;
            assert!(
                !f.is_location_useable(0, location::MMZONE, 0),
                "shifted to 1"
            );
            assert!(f.is_location_useable(0, location::MMZONE, 1));

            let mut wide = Field::with_flags(8000, 0);
            wide.players[0].used_location = 1 << 1;
            assert!(
                wide.is_location_useable(0, location::MMZONE, 0),
                "not shifted"
            );
            assert!(!wide.is_location_useable(0, location::MMZONE, 1));
        }
    }

    mod effect_index {
        use super::*;

        /// The reference's container is an *ordered* multimap and the gather
        /// walks `equal_range` in registration order. That order is the
        /// order chains get built in, so it is observable and is kept.
        #[test]
        fn registration_order_is_preserved_within_a_code() {
            let mut ix = EffectIndex::default();
            ix.insert(code::CHAINING, 7);
            ix.insert(code::CHAINING, 3);
            ix.insert(code::CHAINING, 5);
            assert_eq!(ix.equal_range(code::CHAINING), &[7, 3, 5]);
        }

        #[test]
        fn a_code_with_nothing_registered_is_an_empty_range() {
            let ix = EffectIndex::default();
            assert!(ix.equal_range(code::CHAINING).is_empty());
        }

        #[test]
        fn removing_the_last_entry_drops_the_code() {
            let mut ix = EffectIndex::default();
            ix.insert(code::CHAINING, 1);
            ix.insert(code::CHAINING, 2);
            ix.remove(code::CHAINING, 1);
            assert_eq!(ix.equal_range(code::CHAINING), &[2]);
            ix.remove(code::CHAINING, 2);
            assert!(ix.is_empty());
        }

        /// An effect registers into the index for its type, and the gather
        /// for a different type must not find it.
        #[test]
        fn an_effect_registers_into_the_index_for_its_type() {
            let mut f = Field::new(8000);
            let mandatory = f.new_effect(Effect::new(
                effect_type::TRIGGER_F | effect_type::FIELD | effect_type::ACTIONS,
                code::CHAINING,
            ));
            let optional = f.new_effect(Effect::new(
                effect_type::TRIGGER_O | effect_type::FIELD | effect_type::ACTIONS,
                code::CHAINING,
            ));
            f.register_effect(mandatory);
            f.register_effect(optional);

            assert_eq!(
                f.field_effects.trigger_f.equal_range(code::CHAINING),
                &[mandatory]
            );
            assert_eq!(
                f.field_effects.trigger_o.equal_range(code::CHAINING),
                &[optional]
            );
            assert!(f.field_effects.continuous.is_empty());
        }
    }

    mod build_chain {
        use super::*;

        /// Chain ids come from `infos.field_id` — the same counter as card
        /// field ids and effect registration ids, so they are interleaved
        /// with those rather than forming a sequence of their own.
        ///
        /// The test therefore asserts the relationship rather than absolute
        /// values: consecutive *allocations* are consecutive, but a chain id
        /// is not "the nth chain".
        #[test]
        fn chain_ids_are_allocated_in_order() {
            let (mut f, card) = field_with_card(0);
            let e = f.new_effect(Effect::new(effect_type::TRIGGER_O, code::CHAINING));
            let first = f.build_chain(e, card, Event::new(code::CHAINING));
            let second = f.build_chain(e, card, Event::new(code::CHAINING));
            assert_eq!(second.chain_id, first.chain_id + 1);

            // Creating an effect draws from the same counter, so the next
            // chain id skips past it.
            let other = f.new_effect(Effect::new(effect_type::TRIGGER_O, code::CHAINING));
            let third = f.build_chain(e, card, Event::new(code::CHAINING));
            assert!(
                third.chain_id > second.chain_id + 1,
                "the effect took an id from the same sequence"
            );
            assert_eq!(
                f.effects.get(other).unwrap().id.get() as u16,
                second.chain_id + 1,
                "and it is the one in between"
            );
        }

        /// Normally the handler's controller activates the effect.
        #[test]
        fn the_controller_activates_by_default() {
            let (mut f, card) = field_with_card(1);
            let e = f.new_effect(Effect::new(effect_type::TRIGGER_O, code::CHAINING));
            let mut evt = Event::new(code::CHAINING);
            evt.event_player = 0;
            let chain = f.build_chain(e, card, evt);
            assert_eq!(chain.triggering_player, 1, "the controller, not the event");
        }

        /// Except with `EFFECT_FLAG_EVENT_PLAYER`, where the event's player
        /// activates it instead. Several cards read wrong without this.
        #[test]
        fn the_event_player_activates_when_the_flag_says_so() {
            let (mut f, card) = field_with_card(1);
            let mut effect = Effect::new(effect_type::TRIGGER_O, code::CHAINING);
            effect.flag[0] |= flag::EVENT_PLAYER;
            let e = f.new_effect(effect);

            let mut evt = Event::new(code::CHAINING);
            evt.event_player = 0;
            assert_eq!(f.build_chain(e, card, evt).triggering_player, 0);
        }

        /// The flag only redirects when the event names a real player.
        /// `PLAYER_NONE` falls back to the controller.
        #[test]
        fn the_flag_does_nothing_when_the_event_names_no_player() {
            let (mut f, card) = field_with_card(1);
            let mut effect = Effect::new(effect_type::TRIGGER_O, code::CHAINING);
            effect.flag[0] |= flag::EVENT_PLAYER;
            let e = f.new_effect(effect);
            // Event::new leaves event_player as PLAYER_NONE.
            let chain = f.build_chain(e, card, Event::new(code::CHAINING));
            assert_eq!(chain.triggering_player, 1);
        }
    }
}

/// One entry of a `MSG_SELECT_CHAIN` offer: the card the effect is on, and
/// how the host should present it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChainOffer {
    pub code: u32,
    pub info: crate::leave_field::LocInfo,
    pub description: u64,
    pub client_mode: u8,
}

/// One card offered in a `MSG_SELECT_IDLECMD` list.
///
/// The reference writes the sequence as a `uint32_t` everywhere except the
/// reposition list, where it writes a `uint8_t`. That is a **reference bug**
/// and it is invisible in play: every repositionable card is in a Monster
/// Zone, so its sequence is 0-6. Transcribed as one type here, with the
/// discrepancy recorded rather than reproduced — the wire format is the
/// host protocol's business and no rule depends on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IdleOffer {
    pub code: u32,
    pub controller: u8,
    pub location: u8,
    pub sequence: u32,
}

/// One card in a `SelectCounter` offer, with how many it currently holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CounterOffer {
    pub code: u32,
    pub controller: u8,
    pub location: u8,
    /// A `uint8_t` here, where an idle offer's sequence is a `uint32_t`.
    /// The reference's widths differ per message and are kept.
    pub sequence: u8,
    pub count: u16,
}

/// One card offered in a `MSG_SELECT_BATTLECMD` attack list.
///
/// `direct_attackable` travels with the offer because it is per-card and
/// momentary: `get_attack_target` set it while building the list, and the
/// host needs to know which of these can swing at the player.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AttackOffer {
    pub code: u32,
    pub controller: u8,
    pub location: u8,
    pub sequence: u32,
    pub direct_attackable: bool,
}

/// A message to the host, as the reference writes into its message buffer.
///
/// Only the messages the ported machinery emits are here. They are data
/// rather than a byte stream: the wire format is the host protocol's
/// business, and nothing in the rules depends on it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    /// `MSG_CHAINING` — an effect is being put onto the chain.
    Chaining {
        code: u32,
        controller: u8,
        location: u16,
        sequence: u32,
        description: u64,
        chain_count: u32,
    },
    /// `MSG_CHAINED` — it is on.
    Chained {
        chain_count: u8,
    },
    /// `MSG_MISSED_EFFECT` — a trigger that could have been used and was
    /// not, because the timing passed.
    MissedEffect {
        card: CardId,
        code: u32,
    },
    /// `MSG_CHAIN_SOLVING` — this link is about to resolve.
    ChainSolving {
        chain_count: u8,
    },
    /// `MSG_CHAIN_DISABLED` — its effect was negated.
    ChainDisabled {
        chain_count: u8,
    },
    /// `MSG_CHAIN_SOLVED` — it has resolved.
    ChainSolved {
        chain_count: u8,
    },
    /// `MSG_MOVE` — a card changed place without leaving.
    /// `MSG_MOVE`: the card, where it was, where it is, and why — the
    /// reference's full wire shape (`field.cpp`, `operations.cpp`), so the
    /// trace compares every movement's halves. A token leaving play has
    /// an all-zero `current`, as the reference writes `loc_info{}`.
    Move {
        code: u32,
        previous: crate::leave_field::LocInfo,
        current: crate::leave_field::LocInfo,
        reason: u32,
    },
    /// `MSG_SWAP` — two cards traded seats without either of them moving
    /// anywhere else. Written only when **both** kept their own sequence;
    /// a swap that also reseats a card writes two `Move`s instead.
    ///
    /// Carries the two codes. The reference also writes each card's
    /// location as it was *before* the exchange, which is the same
    /// simplification [`Message::Move`] already makes.
    Swap {
        first: u32,
        second: u32,
    },
    /// `MSG_SHUFFLE_HAND` / `_EXTRA` — the pile was disturbed, and here is
    /// what is in it now. The deck's message carries no codes, because the
    /// point of shuffling it is that nobody may see them.
    ShuffleHand {
        player: u8,
        codes: Vec<u32>,
    },
    ShuffleExtra {
        player: u8,
        codes: Vec<u32>,
    },
    ShuffleDeck {
        player: u8,
    },
    /// `MSG_DRAW`, and `MSG_CONFIRM_CARDS` when a reversed deck means the
    /// opponent saw what was taken.
    Draw {
        player: u8,
        codes: Vec<u32>,
    },
    ConfirmCards {
        player: u8,
        codes: Vec<u32>,
    },
    /// `MSG_ADD_COUNTER` / `MSG_REMOVE_COUNTER`.
    AddCounter {
        counter_type: u16,
        controller: u8,
        location: u8,
        sequence: u32,
        count: u16,
    },
    RemoveCounter {
        counter_type: u16,
        controller: u8,
        location: u8,
        sequence: u32,
        count: u16,
    },
    /// `MSG_SELECT_YESNO` / `MSG_SELECT_EFFECTYN` — a question for a player.
    SelectYesNo {
        player: u8,
        description: u64,
    },
    /// Solver mode only: `count` coins are about to be tossed for `player`;
    /// the host answers with a bitmask, bit `i` set meaning coin `i` is
    /// heads.
    SelectCoin {
        player: u8,
        count: u8,
    },
    /// Solver mode only: `count` of `cards` are about to be chosen at
    /// random for `player`; the host answers in the card-selection format
    /// (indices into `cards`).
    SelectRandom {
        player: u8,
        count: u8,
        cards: Vec<CardId>,
    },
    /// `MSG_RANDOM_SELECTED`: which cards a random selection picked.
    RandomSelected {
        player: u8,
        cards: Vec<crate::leave_field::LocInfo>,
    },
    /// Solver mode only: the top `count` cards of `player`'s deck are about
    /// to be seen — drawn, or dug through. The host settles the deck's
    /// order first, with [`Field::set_deck_order`], and then answers; the
    /// answer itself carries no value.
    SelectDeckTop {
        player: u8,
        count: u32,
    },
    SelectEffectYesNo {
        player: u8,
        code: u32,
        controller: u8,
        location: u8,
        sequence: u32,
        position: u8,
        description: u64,
    },
    /// `MSG_SELECT_OPTION`.
    SelectOption {
        player: u8,
        options: Vec<u64>,
    },
    /// `MSG_SUMMONING` — a summon has begun, and may still be negated.
    Summoning {
        code: u32,
        controller: u8,
        location: u8,
        sequence: u32,
        position: u8,
    },
    /// `MSG_SUMMONED` — it stuck.
    Summoned,
    /// `MSG_FLIPSUMMONING` — a face-down monster is being turned up, and may
    /// still be negated.
    FlipSummoning {
        code: u32,
        controller: u8,
        location: u8,
        sequence: u32,
        position: u8,
    },
    /// `MSG_FLIPSUMMONED` — it stuck.
    FlipSummoned,
    /// `MSG_SPSUMMONING` — a Special Summon is under way.
    ///
    /// A **face-down** summon writes a code of zero: the card is arriving
    /// where the opponent may not see it, so its name is not sent.
    SpSummoning {
        code: u32,
        controller: u8,
        location: u8,
        sequence: u32,
        position: u8,
    },
    /// `MSG_SPSUMMONED` — the batch is done.
    SpSummoned,
    /// `MSG_WIN` — the duel is over. `player` is `5` for a draw, which is
    /// the reference's sentinel rather than a player id.
    Win {
        player: u8,
        reason: u8,
    },
    /// `MSG_REVERSE_DECK` — both decks turned over.
    ReverseDeck,
    /// `MSG_CONFIRM_DECKTOP` — the top `count` cards of a deck, shown to
    /// everyone. Carries the codes in **top-first** order, which is the
    /// reverse of how the pile is stored.
    ConfirmDeckTop {
        player: u8,
        codes: Vec<u32>,
    },
    /// `MSG_DECK_TOP` — what is now on top of a deck, once it is public.
    DeckTop {
        player: u8,
        sequence: u32,
        code: u32,
        position: u8,
    },
    /// `MSG_TOSS_COIN` — the results, in order, `true` for heads.
    TossCoin {
        player: u8,
        results: Vec<bool>,
    },
    /// `MSG_ANNOUNCE_RACE` — declare `count` monster Types out of
    /// `available`. A 64-bit mask: the race table outgrew 32 bits.
    AnnounceRace {
        player: u8,
        count: u8,
        available: u64,
    },
    /// `MSG_SHUFFLE_SET_CARD` — the set cards were shuffled among their
    /// seats, and here is where each ended up.
    ShuffleSetCard {
        location: u8,
        count: u8,
    },
    /// `MSG_CARD_HINT` — a client hint attached to a card: a `chint::*`
    /// kind and a value (a description id, a turn count, a race mask).
    CardHint {
        controller: u8,
        location: u8,
        sequence: u32,
        kind: u8,
        value: u64,
    },
    /// `MSG_CARD_TARGET` — one card's effects now name another. Like
    /// [`Message::Equip`], both sides as locations.
    CardTarget {
        owner: crate::leave_field::LocInfo,
        target: crate::leave_field::LocInfo,
    },
    /// `MSG_EQUIP` — the equip card and what it is now attached to.
    ///
    /// Both sides as **locations**, which is what the reference writes:
    /// the message says which seat is now attached to which, and two
    /// copies of one card are told apart by where they are, not by their
    /// code.
    Equip {
        equip: crate::leave_field::LocInfo,
        target: crate::leave_field::LocInfo,
    },
    /// `MSG_SELECT_COUNTER` — take `count` counters of `counter_type` off
    /// the offered cards, however the player likes to split them.
    SelectCounter {
        player: u8,
        counter_type: u16,
        count: u16,
        cards: Vec<CounterOffer>,
    },
    /// `MSG_FIELD_DISABLED` — which seats are unusable, both players' rows
    /// packed into one word. Sent only when the answer **changed**.
    FieldDisabled {
        locations: u32,
    },
    /// `MSG_SET` — a card was set face-down.
    Set {
        code: u32,
        controller: u8,
        location: u8,
        sequence: u32,
        position: u8,
    },
    /// `MSG_SELECT_TRIBUTE` — like `SelectCard`, but each card carries how
    /// many tributes it is worth.
    /// `MSG_BECOME_TARGET` — the cards an activation has just named as
    /// its targets. Emitted only for an effect with
    /// `EFFECT_FLAG_CARD_TARGET`; a continuous effect never emits it.
    BecomeTarget {
        cards: Vec<CardId>,
    },
    SelectTribute {
        player: u8,
        cancelable: bool,
        min: u8,
        max: u8,
        cards: Vec<(CardId, u32)>,
    },
    /// `MSG_SELECT_UNSELECT_CARD` — choose **one** card, from the offered
    /// list or from what is already chosen, so a selection can be built up
    /// and undone one card at a time.
    SelectUnselectCard {
        player: u8,
        finishable: bool,
        cancelable: bool,
        min: u8,
        max: u8,
        select: Vec<CardId>,
        unselect: Vec<CardId>,
    },
    /// `MSG_SELECT_CARD` — choose cards from a list. The indices the host
    /// answers with refer to `cards` **as ordered here**.
    SelectCard {
        player: u8,
        cancelable: bool,
        min: u8,
        max: u8,
        cards: Vec<CardId>,
    },
    /// `MSG_SELECT_CHAIN` — which effect to chain, if any.
    ///
    /// Both players' hint timings ride along: a player deciding whether to
    /// respond needs to know what timing is open, and the reference sends
    /// its own first and the opponent's second.
    SelectChain {
        player: u8,
        spe_count: u8,
        forced: bool,
        hint_timing: u32,
        opponent_hint_timing: u32,
        chains: Vec<ChainOffer>,
    },
    /// `MSG_NEW_TURN` — whose turn it now is.
    NewTurn {
        player: u8,
    },
    /// `MSG_NEW_PHASE` — which phase has just begun.
    NewPhase {
        phase: u16,
    },
    /// `MSG_SORT_CARD` / `MSG_SORT_CHAIN` — put these in an order. One
    /// message for both, with a flag, because the two differ only in the
    /// opcode the host sees.
    Sort {
        player: u8,
        is_chain: bool,
        cards: Vec<IdleOffer>,
    },
    /// `MSG_ATTACK` — who is attacking what. The target is a default
    /// `LocInfo` for a direct attack, which is how the host tells one.
    Attack {
        attacker: crate::leave_field::LocInfo,
        target: crate::leave_field::LocInfo,
    },
    /// `MSG_DAMAGE_STEP_START` / `MSG_DAMAGE_STEP_END` — bare markers.
    DamageStepStart,
    DamageStepEnd,
    /// `MSG_ATTACK_DISABLED` — the attack in progress was negated.
    AttackDisabled,
    /// `MSG_CARD_SELECTED` — these cards are what the next question is
    /// about. A hint, not a question.
    CardSelected {
        cards: Vec<crate::leave_field::LocInfo>,
    },
    /// `MSG_BATTLE` — the two monsters and what happened to them.
    Battle {
        attacker: crate::leave_field::LocInfo,
        attacker_attack: i32,
        attacker_defense: i32,
        attacker_destroyed: bool,
        target: crate::leave_field::LocInfo,
        target_attack: i32,
        target_defense: i32,
        target_destroyed: bool,
    },
    /// `MSG_DAMAGE` / `MSG_RECOVER` — life points lost and gained.
    Damage {
        player: u8,
        amount: u32,
    },
    Recover {
        player: u8,
        amount: u32,
    },
    /// `MSG_PAY_LPCOST`.
    PayLpCost {
        player: u8,
        amount: u32,
    },
    /// `MSG_MATCH_KILL` — the card that won the match outright.
    MatchKill {
        code: u32,
    },
    /// `MSG_SELECT_BATTLECMD` — the Battle Phase's menu. Four choices:
    /// activate (0), attack (1), go to Main Phase 2 (2), go to the End
    /// Phase (3), packed the same way the idle menu's answer is.
    SelectBattleCmd {
        player: u8,
        activatable: Vec<ChainOffer>,
        attackable: Vec<AttackOffer>,
        to_m2: bool,
        to_ep: bool,
    },
    /// `MSG_SELECT_IDLECMD` — the Main Phase's whole menu, in one message.
    ///
    /// Six lists and three flags. The answer is a single packed integer:
    /// the low half names *which list* (0 summon, 1 special summon, 2
    /// reposition, 3 monster set, 4 spell set, 5 activate, 6 to battle,
    /// 7 to end, 8 shuffle hand) and the high half indexes into it.
    SelectIdleCmd {
        player: u8,
        summonable: Vec<IdleOffer>,
        spsummonable: Vec<IdleOffer>,
        repositionable: Vec<IdleOffer>,
        msetable: Vec<IdleOffer>,
        ssetable: Vec<IdleOffer>,
        activatable: Vec<ChainOffer>,
        to_bp: bool,
        to_ep: bool,
        can_shuffle: bool,
    },
    /// `MSG_SELECT_POSITION`.
    SelectPosition {
        player: u8,
        code: u32,
        positions: u8,
    },
    /// `MSG_SELECT_PLACE` / `MSG_SELECT_DISFIELD` — choose a zone. The flag
    /// is a mask of the zones that are **not** available.
    SelectPlace {
        player: u8,
        count: u8,
        flag: u32,
        disable_field: bool,
    },
    /// `MSG_HINT` — a prompt that is not itself a question.
    Hint {
        kind: u8,
        player: u8,
        value: u64,
    },
    /// `MSG_RETRY` — the last answer was not legal; the same question
    /// stands. Not an error: the unit does not advance, so the host is
    /// simply asked again.
    Retry,
    /// `MSG_CHAIN_END` — the whole chain is over.
    ChainEnd,
}

impl Field {
    /// `MSG_CHAINING`.
    pub fn write_chaining_message(&mut self, effect: EffectId, chain: &Chain) {
        let Some(handler) = self
            .effects
            .get(effect)
            .and_then(|e| e.get_handler(&self.cards))
        else {
            return;
        };
        let description = self.effects.get(effect).map_or(0, |e| e.description);
        let message = Message::Chaining {
            code: self.cards[handler].data.code,
            controller: chain.triggering_controler,
            location: chain.triggering_location,
            sequence: chain.triggering_sequence,
            description,
            chain_count: self.core.current_chain.len() as u32 + 1,
        };
        self.messages.push(message);
    }

    /// `MSG_CHAINED`.
    pub fn write_chained_message(&mut self, chain_count: u8) {
        self.messages.push(Message::Chained { chain_count });
    }

    /// The marker effect `AddChain` puts on a card whose continuous-trap
    /// effect was chosen, so the client can show which one.
    ///
    /// `EFFECT_FLAG_CANNOT_DISABLE | EFFECT_FLAG_CLIENT_HINT`, description
    /// 65, reset on chain end. It carries no rules meaning of its own.
    pub fn add_client_hint_effect(&mut self, handler: CardId) {
        let mut hint = Effect::new(effect_type::SINGLE, 0);
        hint.owner = Some(handler);
        hint.handler = Some(handler);
        hint.flag[0] = flag::CANNOT_DISABLE | flag::CLIENT_HINT;
        hint.description = 65;
        let hint = self.new_effect(hint);
        self.cards[handler].single_effect.insert(0, hint);
    }

    /// `field::break_effect` — the timing passed, so the optional triggers
    /// that were waiting for it are lost.
    ///
    /// This is where "you missed the timing" is implemented. An optional
    /// trigger without `EFFECT_FLAG_DELAY` is dropped from `new_ochain`
    /// here, and the client is told which — a `DELAY` effect survives,
    /// which is precisely the difference between "when" and "if" wording.
    pub fn break_effect(&mut self, clear_sent: bool) {
        if clear_sent {
            self.core.just_sent_cards.clear();
        }
        // Only the two damage-step timings survive a break.
        self.core.hint_timing[0] &= timing::DAMAGE_STEP | timing::DAMAGE_CAL;
        self.core.hint_timing[1] &= timing::DAMAGE_STEP | timing::DAMAGE_CAL;

        let mut kept = ChainList::new();
        let mut missed = Vec::new();
        while let Some(chain) = self.core.new_ochain.pop_front() {
            let effect = chain.triggering_effect;
            let Some(e) = self.effects.get(effect) else {
                continue;
            };
            if e.is_flag(flag::DELAY) {
                kept.push_back(chain);
                continue;
            }
            // Tell the client it was missed — but only if the effect could
            // actually have been used from where its card is.
            let reportable = e.is_flag(flag::FIELD_ONLY)
                || !e.is_type(effect_type::FIELD)
                || e.range & chain.triggering_location != 0;
            if reportable {
                if let Some(handler) = e.get_handler(&self.cards) {
                    missed.push((handler, self.cards[handler].data.code));
                }
            }
        }
        self.core.new_ochain = kept;
        for (card, code) in missed {
            self.messages.push(Message::MissedEffect { card, code });
        }

        let instant = std::mem::take(&mut self.core.instant_event);
        self.core.used_event.extend(instant);
        self.adjust_instant();
        // The reference ends here with the **win check** — the same block
        // `Adjust` step 1 runs — guarded by `!DUEL_RELAY && !force_turn_end`
        // (`processor.cpp` 4383–4434). So a chain that opens after a deck-out
        // announces the win before `MSG_CHAINED`, and a game decided by the
        // last draw is over before the next window's triggers fire. Without
        // it the port announced the win one unit later, after `chained`
        // (fuzz seeds 1001, 1005, …, seen once `MSG_WIN`'s neighbours were
        // in the trace).
        if !self.is_flag(crate::duel::flags::RELAY) {
            self.adjust_win_check();
        }
    }
}

/// `TIMING_*` — when an optional trigger may still be used. Transcribed
/// whole.
pub mod timing {
    pub const DRAW_PHASE: u32 = 0x1;
    pub const STANDBY_PHASE: u32 = 0x2;
    pub const MAIN_END: u32 = 0x4;
    pub const BATTLE_START: u32 = 0x8;
    pub const BATTLE_END: u32 = 0x10;
    pub const END_PHASE: u32 = 0x20;
    pub const SUMMON: u32 = 0x40;
    pub const SPSUMMON: u32 = 0x80;
    pub const FLIPSUMMON: u32 = 0x100;
    pub const MSET: u32 = 0x200;
    pub const SSET: u32 = 0x400;
    pub const POS_CHANGE: u32 = 0x800;
    pub const ATTACK: u32 = 0x1000;
    pub const DAMAGE_STEP: u32 = 0x2000;
    pub const DAMAGE_CAL: u32 = 0x4000;
    pub const CHAIN_END: u32 = 0x8000;
    pub const DRAW: u32 = 0x10000;
    pub const DAMAGE: u32 = 0x20000;
    pub const RECOVER: u32 = 0x40000;
    pub const DESTROY: u32 = 0x80000;
    pub const REMOVE: u32 = 0x100000;
    pub const TOHAND: u32 = 0x200000;
    pub const TODECK: u32 = 0x400000;
    pub const TOGRAVE: u32 = 0x800000;
    pub const BATTLE_PHASE: u32 = 0x1000000;
    pub const EQUIP: u32 = 0x2000000;
    pub const BATTLE_STEP_END: u32 = 0x4000000;
    pub const BATTLED: u32 = 0x8000000;
}

/// The composite timing masks the **script library** defines
/// (`constant.lua:901`), transcribed whole. Two entries today; both are
/// pinned against `constant.lua` by `tools/check_constants.py`.
pub mod timings {
    use super::timing;

    /// A monster arriving on the field by any of the three routes.
    pub const CHECK_MONSTER: u32 = timing::SUMMON | timing::SPSUMMON | timing::FLIPSUMMON;
    /// The same, plus the **End Phase** — not, as the name invites you to
    /// assume, plus a monster Set. `0x1e0`, and the extra bit is `0x20`.
    pub const CHECK_MONSTER_E: u32 = CHECK_MONSTER | timing::END_PHASE;
}

/// Subsystems `AddChain` calls that are not ported yet.
///
/// Each **panics** rather than doing nothing: a panic cannot be mistaken
/// for correct behaviour the way a silent no-op can. They are gathered
/// here so the list is one place rather than scattered `todo!()`s.
///
/// **These are no longer unreachable.** The original note here said
/// nothing could reach them because no duel started; duels start now, and
/// `AddChain` runs. What keeps them quiet is narrower: the only decks
/// these are exercised with are vanilla, so nothing is ever *activated*.
/// The first translated card changes that, and
/// [`Field::place_activating_card`] is the first one it will hit — its
/// own stated blocker, `change_position` and `move_to_field`, has since
/// been lifted.
impl Field {
    /// Put an activating card where activating it puts it — the tail of
    /// `AddChain` case 1. Returns **true if the activation is abandoned**
    /// (a zone-limiting effect named no zone, or a forced location of
    /// nothing), which the case turns into `return TRUE`.
    ///
    /// A Set card in the Spell & Trap row is turned face-up in place. A
    /// card anywhere else is moved: the destination is the Spell & Trap
    /// row, or the Pendulum or Field Zone by the card's type from the
    /// hand, or whatever `EFFECT_FLAG2_FORCE_ACTIVATE_LOCATION` says.
    /// `STATUS_ACT_FROM_HAND` is stamped first, from where the card is
    /// *now* — which is why it has to be read before the move.
    pub fn place_activating_card(&mut self, effect: EffectId) -> bool {
        let Some(handler) = self
            .effects
            .get(effect)
            .and_then(|e| e.get_handler(&self.cards))
        else {
            return false;
        };
        let from_hand = self.cards[handler].current.location == location::HAND;
        self.cards[handler].set_status(status::ACT_FROM_HAND, from_hand);
        let controller = self.cards[handler].current.controller;
        if self.cards[handler].current.location == location::SZONE {
            let up = position::FACEUP;
            self.change_position([handler], None, controller, up, up, up, up, 0, false);
            return false;
        }

        let Some(e) = self.effects.get(effect) else {
            return false;
        };
        let (limit_zone, force_loc, printed) = (
            e.is_flag(flag::LIMIT_ZONE),
            e.is_flag2(flag2::FORCE_ACTIVATE_LOCATION),
            self.cards[handler].data.type_,
        );
        let mut zone = 0xffu32;
        if printed & (card_type::FIELD | card_type::PENDULUM) == 0 && limit_zone {
            // `get_value(7)`: the triggering player and the six fields of
            // the event, in that order.
            let chain = &self.core.new_chains[0];
            let ev = chain.evt.clone();
            let args = [
                i64::from(chain.triggering_player),
                0, // the event's card group is a Lua object; a value function needs the fields below
                i64::from(ev.event_player),
                i64::from(ev.event_value),
                ev.reason_effect.map_or(0, |r| r as i64),
                i64::from(ev.reason),
                i64::from(ev.reason_player),
            ];
            let ctx = crate::effect::Ctx {
                reason_effect: effect,
                player: controller,
                event: &ev,
                card: None,
                args: &args,
            };
            zone = self
                .effects
                .get(effect)
                .map_or(0, |e| e.get_value(self, &ctx)) as u32;
            if zone == 0 {
                return true;
            }
        }
        let mut loc = u16::from(location::SZONE);
        if force_loc {
            loc = self.effect_plain_value(effect) as u16;
            if loc == 0 {
                return true;
            }
        } else if from_hand {
            if printed & card_type::PENDULUM != 0 {
                loc = location::PZONE;
            } else if printed & card_type::FIELD != 0 {
                loc = location::FZONE;
            }
        }
        self.enable_field_effect(handler, false);
        let pos = if loc == u16::from(location::MZONE) {
            position::FACEUP_ATTACK
        } else {
            position::FACEUP
        };
        self.move_to_field(
            handler, controller, controller, loc, pos, false, 0, zone, false, 0, true,
        );
        false
    }

    /// `field::next_player` — hand the deck to the next player of a relay
    /// team. Unreachable in this port's configuration: `DUEL_RELAY` is off,
    /// so `player[p].recharge` is never set.
    pub fn next_player(&mut self, _playerid: u8) {
        unimplemented!("next_player needs relay duels, which this configuration excludes")
    }

    /// `field::tag_swap` — exchange a tag player's deck and hand for their
    /// partner's.
    ///
    /// **A no-op outside a tag duel, by the reference's own first line:**
    /// `extra_lists_main` is empty in a two-player duel and the function
    /// returns immediately. `Turn` calls it on every turn after the first,
    /// so the guard is what makes that harmless — and transcribing the
    /// guard rather than the call site is what keeps the call where the
    /// reference has it.
    pub fn tag_swap(&mut self, _playerid: u8) {}

    /// `check_chain_counter` — the per-chain activity counters a card may
    /// register (`Duel.AddCustomActivityCounter` and kin).
    ///
    /// Every counter's check function is asked about the new link; the
    /// ones that answer **false** are the ones that count it, so their
    /// `player_amount` for the activating player goes up and their ids are
    /// recorded on the link for `restore_chain_counter` to undo if the
    /// activation is negated. The reference returns that list and the
    /// caller stores it on the chain; here the function stores it itself,
    /// on the link just pushed — the same effect, one call site fewer to
    /// get wrong.
    ///
    /// The pool registers no chain counters, so the map is empty and this
    /// is a loop over nothing — ported rather than stubbed so that the
    /// first card that does register one finds the machinery in place.
    pub fn check_chain_counter(&mut self, effect: EffectId, player: u8, chain_id: u16) {
        let ids: Vec<u32> = self.core.chain_counter.keys().copied().collect();
        let mut applied: Vec<u32> = Vec::new();
        for id in ids {
            let Some(check) = self.core.chain_counter.get(&id).and_then(|c| c.check) else {
                continue;
            };
            let args = [i64::from(player), i64::from(chain_id)];
            if !check(self, effect, None, &args) {
                if let Some(c) = self.core.chain_counter.get_mut(&id) {
                    c.player_amount[player as usize] += 1;
                }
                applied.push(id);
            }
        }
        if let Some(link) = self.core.current_chain.last_mut() {
            link.applied_chain_counters = if applied.is_empty() {
                None
            } else {
                Some(applied)
            };
        }
    }

    /// `adjust_all` — re-evaluate every continuous effect on the field.
    ///
    /// Ported in full: it is three lines, and the first of them is the
    /// important one. **Bumping `event_id` is what closes a batch of
    /// events.** Everything raised until the next bump shares an id and is
    /// therefore simultaneous, so where the bumps happen *is* the
    /// simultaneity rule. The `Adjust` unit that does the re-evaluation
    /// arrives later; emplacing it here is correct now.
    pub fn adjust_all(&mut self) {
        self.infos.event_id += 1;
        self.core.readjust_map.clear();
        self.emplace(Kind::Adjust);
    }

    /// `adjust_instant` — the cheaper re-evaluation, done inline rather
    /// than as a unit.
    ///
    /// The `event_id` bump is ported for the reason above. The two list
    /// adjustments it then makes are not, and panic rather than passing
    /// silently: `adjust_disable_check_list` needs `refresh_disable_status`
    /// and the effect-reset machinery, and `adjust_self_destroy_set` needs
    /// the `SelfDestroyUnique` unit.
    ///
    /// The consequence to know: `break_effect` is fully ported but cannot be
    /// *run* until these land.
    /// See [`Core::chance_mode`].
    pub fn set_chance_mode(&mut self, on: bool) {
        self.core.chance_mode = on;
    }

    /// Solver mode's deck tool: impose an order on `player`'s Main Deck,
    /// `order` listed bottom-first like the pile itself. It must be a
    /// permutation of the pile — the same cards, each once — or nothing
    /// changes and this returns false.
    ///
    /// This is how a [`Message::SelectDeckTop`] is answered: the deck's
    /// order is the one hidden variable a shuffle leaves, so a shuffle
    /// needs no chance node of its own; the node is the moment the top is
    /// looked at, and the host instantiates the top there.
    pub fn set_deck_order(&mut self, player: u8, order: &[CardId]) -> bool {
        let pile = &self.players[usize::from(player)].main;
        if order.len() != pile.len() {
            return false;
        }
        let mut have = pile.clone();
        have.sort_unstable();
        let mut given = order.to_vec();
        given.sort_unstable();
        if have != given || given.windows(2).any(|w| w[0] == w[1]) {
            return false;
        }
        self.players[usize::from(player)].main = order.to_vec();
        self.reset_sequence(player, crate::board::location::DECK);
        true
    }

    pub fn adjust_instant(&mut self) {
        self.infos.event_id += 1;
        self.adjust_disable_check_list();
        self.adjust_self_destroy_set();
    }

    /// `field::remove_oath_effect` — take back the promises an activation
    /// made, because the activation was negated.
    ///
    /// A "cannot do X this turn" cost is registered as an oath effect
    /// pointing at the activation that paid it. Negate the activation and
    /// the promise is unmade — which is the rule that a negated cost is
    /// refunded, expressed as a registry rather than as a special case in
    /// each card.
    pub fn remove_oath_effect(&mut self, reason: EffectId) {
        let doomed: Vec<EffectId> = self
            .oath_registry()
            .iter()
            .filter(|(_, &v)| v == Some(reason))
            .map(|(&k, _)| k)
            .collect();
        for effect in doomed {
            self.field_effects.oath.remove(&effect);
            self.remove_effect_from_wherever(effect);
        }
    }

    /// `field::release_oath_relation` — the activation resolved, so its
    /// promises stand whatever happens later.
    ///
    /// The entry is kept and its *reason* cleared, rather than the entry
    /// being removed. That matters: the effect is still an oath and still
    /// resets at end of turn; it has merely stopped being refundable.
    pub fn release_oath_relation(&mut self, reason: EffectId) {
        for value in self.field_effects.oath.values_mut() {
            if *value == Some(reason) {
                *value = None;
            }
        }
    }

    fn oath_registry(&self) -> &BTreeMap<EffectId, Option<EffectId>> {
        &self.field_effects.oath
    }

    /// Take an effect out of play, from the field or from whichever card
    /// holds it. The reference writes this choice out at every call site as
    /// `if(FIELD_ONLY) remove_effect(e) else e->handler->remove_effect(e)`.
    pub fn remove_effect_from_wherever(&mut self, effect: EffectId) {
        let Some(e) = self.effects.get(effect) else {
            return;
        };
        if e.is_flag(flag::FIELD_ONLY) {
            self.remove_effect(effect);
        } else if let Some(handler) = e.handler {
            self.remove_card_effect(handler, effect);
        }
    }

    /// Not ported: needs the chain-counter effects.
    pub fn restore_chain_counter(&mut self, player: u8, chain: usize) {
        let Some(applied) = self
            .core
            .current_chain
            .get(chain)
            .and_then(|c| c.applied_chain_counters.clone())
        else {
            return;
        };
        for id in applied {
            if let Some(c) = self.core.chain_counter.get_mut(&id) {
                c.player_amount[player as usize] =
                    c.player_amount[player as usize].saturating_sub(1);
            }
        }
    }

    /// `card::enable_field_effect` — a card becomes ready to use, or stops
    /// being.
    ///
    /// `STATUS_EFFECT_ENABLED` is set false before a card moves, is summoned
    /// or is chained, and true once it has settled. The early return means
    /// the body runs only on an actual transition, which matters because the
    /// body **renumbers effect ids**.
    ///
    /// That renumbering is the part worth knowing. Every effect the card
    /// brings back gets a fresh id from `infos.field_id`, and effect id is
    /// what every effect sort keys on — so a card that is re-enabled has its
    /// effects apply **last** among equals from then on. Two effects that
    /// would otherwise tie are ordered by which card settled most recently.
    ///
    /// The four containers are treated differently, and not by accident:
    /// single effects only if they are range-limited *and* in range, field
    /// and target effects if in range, and equip effects only while the card
    /// is in a spell/trap zone — with no range test at all, because an equip
    /// effect's range is its target rather than its own location.
    pub fn enable_field_effect(&mut self, card: CardId, enabled: bool) {
        if self.cards[card].current.location == 0 {
            return;
        }
        let already = self.cards[card].is_status(status::EFFECT_ENABLED);
        if enabled == already {
            return;
        }
        self.refresh_disable_status(card);

        if !enabled {
            self.cards[card].set_status(status::EFFECT_ENABLED, false);
        } else {
            self.cards[card].set_status(status::EFFECT_ENABLED, true);

            let mut renumber: Vec<EffectId> = Vec::new();
            for (_, effect) in self.cards[card]
                .single_effect
                .iter()
                .copied()
                .collect::<Vec<_>>()
            {
                let in_single_range = self.effects.get(effect).is_some_and(|e| {
                    e.is_flag(flag::SINGLE_RANGE) && e.in_range(&self.cards, &self.cards[card])
                });
                if in_single_range {
                    renumber.push(effect);
                }
            }
            for (_, effect) in self.cards[card]
                .field_effect
                .iter()
                .copied()
                .collect::<Vec<_>>()
            {
                if self
                    .effects
                    .get(effect)
                    .is_some_and(|e| e.in_range(&self.cards, &self.cards[card]))
                {
                    renumber.push(effect);
                }
            }
            if self.cards[card].current.location == location::SZONE {
                // No range test: an equip effect's range is its target.
                for (_, effect) in self.cards[card]
                    .equip_effect
                    .iter()
                    .copied()
                    .collect::<Vec<_>>()
                {
                    renumber.push(effect);
                }
            }
            for (_, effect) in self.cards[card]
                .target_effect
                .iter()
                .copied()
                .collect::<Vec<_>>()
            {
                if self
                    .effects
                    .get(effect)
                    .is_some_and(|e| e.in_range(&self.cards, &self.cards[card]))
                {
                    renumber.push(effect);
                }
            }
            for effect in renumber {
                let id = self.next_field_id_raw();
                if let Some(e) = self.effects.get_mut(effect) {
                    e.id.set(id);
                }
            }

            if self.cards[card].is_status(status::DISABLED) {
                self.reset_card(card, reset::DISABLE, reset::EVENT);
            }
        }

        // A card that is disabled or forbidden does not propagate: whatever
        // its own effects would have reached stays as it was.
        if self.cards[card].get_status(status::DISABLED | status::FORBIDDEN) {
            return;
        }
        self.filter_disable_related_cards(card);
    }

    /// `SolveChain`'s hand shuffle: a hand effect that stayed hidden is
    /// shuffled back so its position reveals nothing.
    pub fn shuffle_hand(&mut self, player: u8) {
        self.shuffle(player, location::HAND);
    }

    /// `SolveChain`'s confirmed-leaving set going to the graveyard.
    ///
    /// One `send_to` call, spelled out because its arguments are not the
    /// defaults: `REASON_RULE` (so nothing may refuse or replace it), no
    /// reason effect, `PLAYER_NONE` for both players so each card goes to
    /// its **owner's** graveyard, and face-up.
    pub fn send_to_grave(&mut self, cards: Vec<CardId>) {
        self.send_to(
            cards,
            None,
            crate::card::reason::RULE,
            PLAYER_NONE,
            PLAYER_NONE,
            u16::from(location::GRAVE),
            0,
            position::FACEUP,
            false,
        );
    }

    /// `field::reset_chain` — everything that lasts only for one chain
    /// stops lasting.
    ///
    /// Two things: the once-per-chain activation tallies, and every effect
    /// that registered itself as resetting on `RESET_CHAIN`. The second is
    /// why `cheff` is kept as a set at all — walking every effect in the
    /// duel at every chain's end would be the obvious alternative and is
    /// what the set exists to avoid.
    pub fn reset_chain(&mut self) {
        self.core.chain_limit.clear();
        self.core.effect_count_code_chain.clear();
        let chain_scoped: Vec<EffectId> = self.field_effects.cheff.iter().copied().collect();
        for effect in chain_scoped {
            self.remove_effect_from_wherever(effect);
        }
    }
}

/// The three questions asked of a chain link before it resolves.
///
/// Each takes a `chaincount` where **0 means "the innermost link"** rather
/// than "link zero" — the reference spells it `if(chaincount == 0) back()
/// else [chaincount - 1]`, so the parameter is one-based with 0 as a
/// shorthand. Treating it as an index is off by one for every caller that
/// passes a real link number.
impl Field {
    fn chain_at(&self, chaincount: u8) -> Option<&Chain> {
        if usize::from(chaincount) > self.core.current_chain.len() {
            return None;
        }
        if chaincount == 0 {
            self.core.current_chain.last()
        } else {
            self.core.current_chain.get(usize::from(chaincount) - 1)
        }
    }

    /// `filter_field_effect` — the available field-wide effects with a code.
    /// Unlike `filter_player_effect` it asks nothing about players.
    pub fn filter_field_effect(&self, code: u32) -> Vec<EffectId> {
        let mut out: Vec<EffectId> = self
            .field_effects
            .aura
            .equal_range(code)
            .iter()
            .copied()
            .filter(|&e| self.is_available(e))
            .collect();
        self.sort_by_effect_id(&mut out);
        out
    }

    /// A field effect read as a yes/no about a chain link.
    fn field_effect_vetoes(&self, code: u32, chaincount: u8) -> bool {
        let ev = Event::new(0);
        let args = [i64::from(chaincount)];
        self.filter_field_effect(code).into_iter().any(|id| {
            self.effects.get(id).is_some_and(|e| {
                e.check_value_condition(
                    self,
                    &Ctx {
                        reason_effect: id,
                        player: PLAYER_NONE,
                        event: &ev,
                        card: None,
                        args: &args,
                    },
                )
            })
        })
    }

    /// `is_chain_negatable` — may the *activation* be negated?
    pub fn is_chain_negatable(&self, chaincount: u8) -> bool {
        let Some(chain) = self.chain_at(chaincount) else {
            return false;
        };
        if self
            .effects
            .get(chain.triggering_effect)
            .is_some_and(|e| e.is_flag(flag::CANNOT_INACTIVATE))
        {
            return false;
        }
        !self.field_effect_vetoes(code::CANNOT_INACTIVATE, chaincount)
    }

    /// `field::disable_chain` — negate the effect of a chain link.
    ///
    /// `chaincount` is one-based and **clamped rather than refused**: zero
    /// or anything past the end means the topmost link, which is what a
    /// script passing `ev` from a chain-solving event relies on.
    ///
    /// Three conditions, and the third is the one a reader skips: the
    /// effect must not already be negated, the link must be *disablable*
    /// at all, and the effect's handler must not be immune to whatever is
    /// doing the negating.
    ///
    /// The tail is a rule about **where the card came from**. A link
    /// activated from the deck — or face-down from the extra deck — has
    /// its relation released when it is negated, so the card is no longer
    /// treated as related to a chain that will not resolve. A duel option
    /// turns that off.
    pub fn disable_chain(&mut self, chaincount: u8) -> bool {
        if self.core.current_chain.is_empty() {
            return false;
        }
        let len = self.core.current_chain.len() as u8;
        let chaincount = if chaincount > len || chaincount < 1 {
            len
        } else {
            chaincount
        };
        let idx = usize::from(chaincount) - 1;
        let (flag_set, effect, chain_id, triggering_location, triggering_position) = {
            let ch = &self.core.current_chain[idx];
            (
                ch.flag & chain_flag::DISABLE_EFFECT != 0,
                ch.triggering_effect,
                ch.chain_id,
                ch.triggering_location,
                ch.triggering_position,
            )
        };
        if flag_set {
            return false;
        }
        if !self.is_chain_disablable(chaincount) {
            return false;
        }
        let handler = self
            .effects
            .get(effect)
            .and_then(|e| e.get_handler(&self.cards));
        let by = self.core.reason_effect;
        let Some(handler) = handler else {
            return false;
        };
        if !self.is_affect_by_effect(handler, by) {
            return false;
        }
        let rp = self.core.reason_player;
        {
            let ch = &mut self.core.current_chain[idx];
            ch.flag |= chain_flag::DISABLE_EFFECT;
            ch.disable_reason = by;
            ch.disable_player = rp;
        }
        self.messages.push(Message::ChainDisabled {
            chain_count: chaincount,
        });
        let from_deck = triggering_location == u16::from(location::DECK)
            || (triggering_location == u16::from(location::EXTRA)
                && triggering_position & crate::board::position::FACEDOWN != 0);
        if !self.is_flag(flags::RETURN_TO_DECK_TRIGGERS) && from_deck {
            self.cards[handler].release_chain_relation(effect, chain_id);
        }
        true
    }

    /// `is_chain_disablable` — may the *effect* be negated?
    ///
    /// A forbidden card is disablable whatever it says: the two protections
    /// below are skipped entirely when its handler is forbidden, which is
    /// how a forbidden card loses its own "cannot be negated" clause.
    pub fn is_chain_disablable(&self, chaincount: u8) -> bool {
        let Some(chain) = self.chain_at(chaincount) else {
            return false;
        };
        let effect = chain.triggering_effect;
        let forbidden = self
            .effects
            .get(effect)
            .and_then(|e| e.get_handler(&self.cards))
            .is_some_and(|h| self.cards[h].is_status(status::FORBIDDEN));
        if !forbidden {
            if self
                .effects
                .get(effect)
                .is_some_and(|e| e.is_flag(flag::CANNOT_DISABLE))
            {
                return false;
            }
            if self.field_effect_vetoes(code::CANNOT_DISEFFECT, chaincount) {
                return false;
            }
        }
        true
    }

    /// `is_chain_disabled` — has it *been* negated?
    ///
    /// Takes `&mut self` where the reference's is non-const for the same
    /// reason: finding the negating effect stamps `RESET_CHAIN` on it, so
    /// asking the question has a side effect.
    pub fn is_chain_disabled(&mut self, chaincount: u8) -> bool {
        let Some(chain) = self.chain_at(chaincount) else {
            return false;
        };
        if chain.flag & chain_flag::DISABLE_EFFECT != 0 {
            return true;
        }
        let (chain_id, effect) = (chain.chain_id, chain.triggering_effect);
        let Some(handler) = self
            .effects
            .get(effect)
            .and_then(|e| e.get_handler(&self.cards))
        else {
            return false;
        };
        let candidates = self.cards[handler]
            .single_effect
            .equal_range(code::DISABLE_CHAIN)
            .to_vec();
        for id in candidates {
            let matches = self
                .effects
                .get(id)
                .is_some_and(|e| e.value == i64::from(chain_id));
            if matches {
                if let Some(e) = self.effects.get_mut(id) {
                    e.reset_flag |= reset::CHAIN;
                }
                return true;
            }
        }
        false
    }
}

/// `RESET_*` — when an effect is taken away again. Transcribed whole.
///
/// Two groups sharing one word: the high four bits are *when* (a turn, a
/// phase, a chain) and the middle bits are *what happened to the card*. A
/// reset mask is normally one of each.
pub mod reset {
    pub const SELF_TURN: u32 = 0x10000000;
    pub const OPPO_TURN: u32 = 0x20000000;
    pub const PHASE: u32 = 0x40000000;
    pub const CHAIN: u32 = 0x80000000;
    pub const EVENT: u32 = 0x1000;
    pub const CARD: u32 = 0x2000;
    pub const CODE: u32 = 0x4000;
    pub const COPY: u32 = 0x8000;

    pub const DISABLE: u32 = 0x00010000;
    pub const TURN_SET: u32 = 0x00020000;
    pub const TOGRAVE: u32 = 0x00040000;
    pub const REMOVE: u32 = 0x00080000;
    pub const TEMP_REMOVE: u32 = 0x00100000;
    pub const TOHAND: u32 = 0x00200000;
    pub const TODECK: u32 = 0x00400000;
    pub const LEAVE: u32 = 0x00800000;
    pub const TOFIELD: u32 = 0x01000000;
    pub const CONTROL: u32 = 0x02000000;
    pub const OVERLAY: u32 = 0x04000000;
    pub const MSCHANGE: u32 = 0x08000000;
}

#[cfg(test)]
mod mzone_count_tests {
    use super::*;
    use crate::card::{card_type, status, Card, CardData};
    use crate::effect::{effect_type, flag as eflag, Ctx, Effect};

    fn monster(f: &mut Field, player: u8, seat: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 40_000 + seat + u32::from(player) * 10,
                type_: card_type::MONSTER | card_type::NORMAL,
                ..Default::default()
            },
            player,
        );
        c.current.controller = player;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(player, id, location::MZONE, seat, false);
        f.cards[id].current.position = crate::board::position::FACEUP_ATTACK;
        id
    }

    const TOFIELD: u32 = Field::LOCATION_REASON_TOFIELD;
    const CONTROL: u32 = Field::LOCATION_REASON_CONTROL;

    /// **The excluded card's seat is counted as free**, and excluding
    /// nothing is the ordinary count.
    #[test]
    fn it_counts_the_excluded_cards_seat_as_free() {
        let mut f = Field::new(8000);
        let seats: Vec<CardId> = (0..5).map(|s| monster(&mut f, 0, s)).collect();
        assert_eq!(
            f.get_mzone_count(0, None, 0, TOFIELD, 0xff),
            0,
            "a full row"
        );
        assert_eq!(
            f.get_mzone_count(0, Some(&[seats[2]]), 0, TOFIELD, 0xff),
            1,
            "one seat freed by the exclusion"
        );
        let theirs = monster(&mut f, 1, 0);
        assert_eq!(
            f.get_mzone_count(0, Some(&[theirs]), 0, TOFIELD, 0xff),
            0,
            "a card in the other row frees nothing here"
        );
    }

    /// **The board is put back exactly as it was**, rows and masks alike
    /// — the count is a question, not a move.
    #[test]
    fn it_leaves_the_board_as_it_found_it() {
        let mut f = Field::new(8000);
        let seats: Vec<CardId> = (0..3).map(|s| monster(&mut f, 0, s)).collect();
        monster(&mut f, 1, 0);
        let before_rows: Vec<Vec<Option<CardId>>> =
            (0..2).map(|p| f.players[p].mzone.clone()).collect();
        let before_used: Vec<u32> = (0..2).map(|p| f.players[p].used_location).collect();
        f.get_mzone_count(0, Some(&[seats[1]]), 0, TOFIELD, 0xff);
        for p in 0..2 {
            assert_eq!(f.players[p].mzone, before_rows[p], "row {p}");
            assert_eq!(f.players[p].used_location, before_used[p], "mask {p}");
        }
    }

    /// **Both rows are rebuilt**, not just the one being counted for,
    /// because an Extra Monster Zone seat is shared.
    #[test]
    fn the_rebuild_covers_both_rows() {
        let mut f = Field::new(8000);
        let theirs = monster(&mut f, 1, 0);
        assert_eq!(
            f.get_mzone_count(1, Some(&[theirs]), 0, TOFIELD, 0xff),
            5,
            "their row is empty once their own card is excluded"
        );
    }

    /// **The Spell & Trap half of the mask is carried across**, and
    /// restored.
    #[test]
    fn the_spell_row_half_of_the_mask_is_kept() {
        let mut f = Field::new(8000);
        let m = monster(&mut f, 0, 0);
        f.players[0].used_location |= 0x1f00;
        f.get_mzone_count(0, Some(&[m]), 0, TOFIELD, 0xff);
        assert_eq!(f.players[0].used_location & 0xff00, 0x1f00, "restored");
        assert_eq!(
            f.get_szone_limit(0, 0, TOFIELD),
            0,
            "and the spell row is still full"
        );
    }

    /// **A player who does not exist counts zero**, and the *reason*
    /// reaches the seat count.
    #[test]
    fn it_refuses_a_third_player_and_passes_the_reason_through() {
        let mut f = Field::new(8000);
        let m = monster(&mut f, 0, 0);
        assert_eq!(f.get_mzone_count(2, Some(&[m]), 0, TOFIELD, 0xff), 0);

        let mut cap = Effect::new(effect_type::FIELD, code::MAX_MZONE);
        cap.owner = Some(m);
        cap.handler = Some(m);
        cap.effect_owner = 0;
        cap.flag[0] |= eflag::PLAYER_TARGET | eflag::FUNC_VALUE;
        cap.range = u16::from(location::MZONE);
        cap.s_range = u16::from(location::MZONE);
        cap.value_fn = Some(only_for_control);
        let id = f.new_effect(cap);
        f.field_effects.aura.insert(code::MAX_MZONE, id);
        f.field_effects.indexer.insert(id);

        assert_eq!(f.get_mzone_count(0, Some(&[m]), 0, TOFIELD, 0xff), 5);
        assert_eq!(
            f.get_mzone_count(0, Some(&[m]), 0, CONTROL, 0xff),
            1,
            "the reason reaches the cap"
        );
    }

    fn only_for_control(_e: &Effect, _f: &Field, ctx: &Ctx) -> i64 {
        if ctx.args.get(2) == Some(&i64::from(CONTROL)) {
            1
        } else {
            5
        }
    }
}

/// The composite reset masks the **script library** defines
/// (`constant.lua:298`), transcribed whole rather than one entry at a
/// time: a card names one of these, and the surrounding code stays
/// internally consistent with whatever number it was given.
///
/// Pinned against `constant.lua` by `tools/check_constants.py`, which
/// evaluates the Lua expressions rather than trusting these.
pub mod resets {
    use super::reset;
    use crate::duel::phases;

    pub const STANDARD: u32 = reset::TOFIELD
        | reset::LEAVE
        | reset::TODECK
        | reset::TOHAND
        | reset::TEMP_REMOVE
        | reset::REMOVE
        | reset::TOGRAVE
        | reset::TURN_SET;
    pub const STANDARD_DISABLE: u32 = STANDARD | reset::DISABLE;
    pub const STANDARD_PHASE_END: u32 = reset::EVENT | STANDARD | reset::PHASE | phases::END as u32;
    pub const STANDARD_DISABLE_PHASE_END: u32 = STANDARD_PHASE_END | reset::DISABLE;
    pub const REDIRECT: u32 =
        (STANDARD | reset::OVERLAY | reset::MSCHANGE) & !(reset::TOFIELD | reset::LEAVE);
    pub const CANNOT_ACT: u32 = STANDARD & !reset::LEAVE;
    pub const STANDARD_EXC_GRAVE: u32 = STANDARD & !(reset::LEAVE | reset::TOGRAVE);
}

/// `GLOBALFLAG_*` — "is any card in this duel doing X".
///
/// Set when a card's effect registers, so the engine can skip a whole pass
/// when nothing needs it. Transcribed whole; the reference has several
/// commented out and those are not here.
pub mod global_flag {
    pub const DECK_REVERSE_CHECK: u32 = 0x1;
    pub const BRAINWASHING_CHECK: u32 = 0x2;
    pub const DETACH_EVENT: u32 = 0x10;
    pub const SELF_TOGRAVE: u32 = 0x100;
    pub const SPSUMMON_ONCE: u32 = 0x200;
}

impl Field {
    /// `field::adjust_self_destroy_set` — find the cards that must destroy
    /// themselves, and queue the units that do it.
    ///
    /// Three passes, and the first is not like the other two.
    ///
    /// **Uniqueness.** A card limiting itself to one face-up copy reads its
    /// own count and *records* the answer rather than acting on it: none
    /// found clears `unique_fieldid`, exactly one records that card's id,
    /// and only two or more queue a destruction. The recorded id is how a
    /// later check knows which copy is the survivor.
    ///
    /// **Self-destroy and self-to-grave.** Both scan the same set of face-up
    /// cards, and the second is skipped entirely unless some card in the
    /// duel has a self-to-grave effect — which is what the global flag is
    /// for.
    ///
    /// The whole function is suspended while any of the three sets is
    /// non-empty: a pass already queued must finish before another is
    /// gathered, or the same card is queued twice.
    pub fn adjust_self_destroy_set(&mut self) {
        if self.core.selfdes_disabled
            || !self.core.unique_destroy_set.is_empty()
            || !self.core.self_destroy_set.is_empty()
            || !self.core.self_tograve_set.is_empty()
        {
            return;
        }

        // Turn player first, then the opponent.
        let mut p = self.infos.turn_player;
        for _ in 0..2 {
            let mut over_limit: Vec<CardId> = Vec::new();
            for unique in self.core.unique_cards[p as usize].clone() {
                let c = &self.cards[unique];
                if !(c.current.is_faceup()
                    && c.is_status(status::EFFECT_ENABLED)
                    && !c.get_status(status::DISABLED | status::FORBIDDEN))
                {
                    continue;
                }
                let targets = self.unique_targets(unique, p, None);
                match targets.len() {
                    0 => self.cards[unique].unique_fieldid = 0,
                    1 => {
                        let only = *targets.iter().next().unwrap();
                        self.cards[unique].unique_fieldid = self.cards[only].fieldid;
                    }
                    _ => over_limit.push(unique),
                }
            }
            // By field id, so the order is when the cards arrived rather
            // than where they sit.
            over_limit.sort_by_key(|&c| self.cards[c].fieldid);
            for card in over_limit {
                self.emplace(Kind::SelfDestroyUnique { card, player: p });
                self.core.unique_destroy_set.insert(card);
            }
            p = 1 - p;
        }

        // Every face-up card on the field, except a monster already destroyed
        // by battle — that one is on its way out regardless.
        // A stack buffer, not a set: the two sets it feeds are order-free,
        // and a `BTreeSet` (or a `Vec`) here was an allocation on every
        // adjust. Two players' monster and spell zones fit with room to spare.
        let mut buffer = [0 as CardId; 32];
        let mut n = 0usize;
        for p in 0..2 {
            for &card in self.players[p].mzone.iter().flatten() {
                if self.cards[card].current.is_faceup()
                    && !self.cards[card].is_status(status::BATTLE_DESTROYED)
                {
                    buffer[n] = card;
                    n += 1;
                }
            }
            for &card in self.players[p].szone.iter().flatten() {
                if self.cards[card].current.is_faceup() {
                    buffer[n] = card;
                    n += 1;
                }
            }
        }
        let candidates = &buffer[..n];

        for &card in candidates {
            if self
                .is_affected_by_effect(card, code::SELF_DESTROY)
                .is_some()
            {
                self.core.self_destroy_set.insert(card);
            }
        }
        if self.core.global_flag & global_flag::SELF_TOGRAVE != 0 {
            for &card in candidates {
                if self
                    .is_affected_by_effect(card, code::SELF_TOGRAVE)
                    .is_some()
                {
                    self.core.self_tograve_set.insert(card);
                }
            }
        }

        if !self.core.self_destroy_set.is_empty() {
            self.emplace(Kind::SelfDestroy);
        }
        if !self.core.self_tograve_set.is_empty() {
            self.emplace(Kind::SelfToGrave);
        }
    }
}

#[cfg(test)]
mod self_destroy_tests {
    use super::*;
    use crate::board::{location, position};
    use crate::card::{card_type, Card, CardData};
    use crate::effect::{effect_type, Effect};
    use crate::processor::Kind;

    fn face_up(f: &mut Field, controller: u8, seat: usize) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 18036057,
                type_: card_type::MONSTER | card_type::EFFECT,
                ..Default::default()
            },
            controller,
        );
        c.current.controller = controller;
        c.current.position = position::FACEUP_ATTACK;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(controller, id, location::MZONE, seat as u32, false);
        id
    }

    fn single_on(f: &mut Field, card: CardId, code_: u32) -> EffectId {
        let mut e = Effect::new(effect_type::SINGLE, code_);
        e.owner = Some(card);
        e.handler = Some(card);
        let id = f.new_effect(e);
        f.cards[card].single_effect.insert(code_, id);
        f.cards[card].indexer.insert(id);
        id
    }

    /// A card whose effect says to destroy itself is queued, and the unit
    /// that does it is emplaced once for the whole set.
    #[test]
    fn a_self_destroying_card_is_queued() {
        let mut f = Field::new(8000);
        let a = face_up(&mut f, 0, 0);
        face_up(&mut f, 0, 1);
        single_on(&mut f, a, code::SELF_DESTROY);

        f.adjust_self_destroy_set();
        assert_eq!(
            f.core.self_destroy_set.iter().copied().collect::<Vec<_>>(),
            vec![a]
        );
        assert_eq!(
            f.core
                .subunits
                .iter()
                .filter(|u| matches!(u.kind, Kind::SelfDestroy))
                .count(),
            1,
            "one unit for the whole set"
        );
    }

    /// A monster already destroyed by battle is left out — it is on its way
    /// off the field regardless.
    #[test]
    fn a_battle_destroyed_monster_is_left_out() {
        let mut f = Field::new(8000);
        let a = face_up(&mut f, 0, 0);
        single_on(&mut f, a, code::SELF_DESTROY);
        f.cards[a].set_status(status::BATTLE_DESTROYED, true);

        f.adjust_self_destroy_set();
        assert!(f.core.self_destroy_set.is_empty());
    }

    /// Self-to-grave is skipped entirely unless some card in the duel has
    /// such an effect — which is what the global flag is for.
    #[test]
    fn self_to_grave_needs_its_global_flag() {
        let mut f = Field::new(8000);
        let a = face_up(&mut f, 0, 0);
        single_on(&mut f, a, code::SELF_TOGRAVE);

        f.adjust_self_destroy_set();
        assert!(f.core.self_tograve_set.is_empty(), "flag not set");

        f.core.global_flag |= global_flag::SELF_TOGRAVE;
        f.adjust_self_destroy_set();
        assert_eq!(
            f.core.self_tograve_set.iter().copied().collect::<Vec<_>>(),
            vec![a]
        );
    }

    /// The whole pass is suspended while an earlier one is still queued, so
    /// a card cannot be queued twice.
    #[test]
    fn the_pass_is_suspended_while_one_is_outstanding() {
        let mut f = Field::new(8000);
        let a = face_up(&mut f, 0, 0);
        single_on(&mut f, a, code::SELF_DESTROY);

        f.core.self_tograve_set.insert(99);
        f.adjust_self_destroy_set();
        assert!(f.core.self_destroy_set.is_empty(), "suspended");

        f.core.self_tograve_set.clear();
        f.adjust_self_destroy_set();
        assert!(!f.core.self_destroy_set.is_empty());
    }

    /// A uniqueness limit *records* its answer rather than acting on it
    /// until there are two: none clears the recorded id, one records that
    /// card's, and only two or more queue a destruction.
    #[test]
    fn a_uniqueness_limit_records_before_it_destroys() {
        let mut f = Field::new(8000);
        let keeper = face_up(&mut f, 0, 0);
        f.cards[keeper].unique_code = 18036057;
        f.cards[keeper].unique_location = u16::from(location::MZONE);
        f.cards[keeper].unique_pos = [1, 0];
        f.core.unique_cards[0].push(keeper);

        // It sees only itself: exactly one, so its id is recorded.
        f.adjust_self_destroy_set();
        assert_eq!(f.cards[keeper].unique_fieldid, f.cards[keeper].fieldid);
        assert!(f.core.unique_destroy_set.is_empty(), "nothing to destroy");

        // A second copy makes two, which is over the limit.
        let mut f2 = Field::new(8000);
        let a = face_up(&mut f2, 0, 0);
        let b = face_up(&mut f2, 0, 1);
        for c in [a, b] {
            f2.cards[c].unique_code = 18036057;
            f2.cards[c].unique_location = u16::from(location::MZONE);
            f2.cards[c].unique_pos = [1, 0];
            f2.core.unique_cards[0].push(c);
        }
        f2.adjust_self_destroy_set();
        assert_eq!(f2.core.unique_destroy_set.len(), 2);
        assert_eq!(
            f2.core
                .subunits
                .iter()
                .filter(|u| matches!(u.kind, Kind::SelfDestroyUnique { .. }))
                .count(),
            2,
            "one unit per card, unlike the other two passes"
        );
    }

    /// A disabled or face-down copy imposes no limit, so it is not counted.
    #[test]
    fn a_disabled_unique_card_is_not_asked() {
        let mut f = Field::new(8000);
        let a = face_up(&mut f, 0, 0);
        f.cards[a].unique_code = 18036057;
        f.cards[a].unique_location = u16::from(location::MZONE);
        f.cards[a].unique_pos = [1, 0];
        f.cards[a].unique_fieldid = 12345;
        f.core.unique_cards[0].push(a);
        f.cards[a].set_status(status::DISABLED, true);

        f.adjust_self_destroy_set();
        assert_eq!(
            f.cards[a].unique_fieldid, 12345,
            "left alone rather than recomputed"
        );
    }

    /// The milestone: `break_effect` runs end to end now that
    /// `adjust_instant` has both of its halves.
    ///
    /// It is where "you missed the timing" lives — an optional trigger
    /// without `EFFECT_FLAG_DELAY` is given up here, and one with it
    /// survives.
    #[test]
    fn break_effect_runs_and_gives_up_the_missed_triggers() {
        use crate::chain::Chain;
        use crate::effect::flag;

        let mut f = Field::new(8000);
        let card = face_up(&mut f, 0, 0);

        let make = |f: &mut Field, delayed: bool| {
            let mut e = Effect::new(
                effect_type::TRIGGER_O | effect_type::FIELD | effect_type::ACTIONS,
                code::CHAINING,
            );
            e.owner = Some(card);
            e.handler = Some(card);
            e.range = u16::from(location::MZONE);
            if delayed {
                e.flag[0] |= flag::DELAY;
            }
            let id = f.new_effect(e);
            let mut chain = Chain::new(id, Event::new(code::CHAINING));
            chain.chain_id = f.next_field_id();
            chain.triggering_location = u16::from(location::MZONE);
            f.core.new_ochain.push_back(chain);
            id
        };
        let missed = make(&mut f, false);
        let survives = make(&mut f, true);

        f.core.instant_event.push_back(Event::new(code::CHAINING));
        f.core.hint_timing = [0xffff_ffff, 0xffff_ffff];
        let before = f.infos.event_id;

        f.break_effect(true);

        assert_eq!(f.core.new_ochain.len(), 1, "one was given up");
        assert_eq!(
            f.core.new_ochain[0].triggering_effect, survives,
            "the DELAY effect is the one that survives"
        );
        assert!(
            f.messages
                .iter()
                .any(|m| matches!(m, Message::MissedEffect { .. })),
            "and the client is told which was missed"
        );
        let _ = missed;

        assert!(f.core.instant_event.is_empty(), "the events are spent");
        assert_eq!(f.core.used_event.len(), 1);
        assert_eq!(
            f.core.hint_timing[0],
            timing::DAMAGE_STEP | timing::DAMAGE_CAL,
            "only the damage-step timings survive a break"
        );
        assert!(f.core.just_sent_cards.is_empty(), "cleared, as asked");
        assert!(
            f.infos.event_id > before,
            "adjust_instant closed the batch, which is what ends simultaneity"
        );
    }
}

#[cfg(test)]
mod returns_tests {
    use super::Returns;

    /// Reading a slot nothing has written gives zero, not garbage and not a
    /// panic. Units do this: a step reads an answer a previous step may not
    /// have produced.
    #[test]
    fn reading_past_the_end_yields_zero() {
        let r = Returns::default();
        assert_eq!(r.get(), 0);
        assert_eq!(r.at_i32(7), 0);
        assert_eq!(r.at_i8(3), 0);
    }

    /// Writing past the end grows the buffer and zero-fills the gap.
    #[test]
    fn writing_past_the_end_grows_and_zero_fills() {
        let mut r = Returns::default();
        r.set_i8(3, 9);
        assert_eq!(r.at_i8(3), 9);
        assert_eq!(r.at_i8(0), 0, "the gap is zero, not stale");
        assert_eq!(r.at_i8(1), 0);
    }

    /// The indexing is by *element*, scaled by the type's width — so `i8`
    /// slot 2 is byte 2, and an `i32` at slot 0 covers bytes 0-3.
    ///
    /// The consequence is sharper than "they alias": three `i8` writes leave
    /// the buffer **one byte short** of an `i32`, and the short-read rule
    /// then makes `at_i32(0)` return **zero** rather than the packed bytes.
    /// So a `SelectPlace` answer — three `i8` slots — reads as 0 to any unit
    /// that asks for the leading `i32`. It only aliases once a fourth byte
    /// exists.
    #[test]
    fn three_i8_writes_do_not_make_a_readable_i32() {
        let mut r = Returns::default();
        r.set_i8(0, 1);
        r.set_i8(1, 4);
        r.set_i8(2, 3);
        assert_eq!(r.at_i8(2), 3, "the sequence SendTo reads");
        assert_eq!(
            r.get(),
            0,
            "one byte short of an i32, so the short-read rule gives zero"
        );

        r.set_i8(3, 0);
        assert_eq!(
            r.get(),
            i32::from_le_bytes([1, 4, 3, 0]),
            "with a fourth byte the two views alias"
        );
    }

    /// And the other way round: an i32 written at slot 0 is readable as its
    /// individual bytes.
    #[test]
    fn an_i32_is_readable_as_its_bytes() {
        let mut r = Returns::default();
        r.set(0x0403_0201);
        assert_eq!(
            (r.at_i8(0), r.at_i8(1), r.at_i8(2), r.at_i8(3)),
            (1, 2, 3, 4)
        );
    }

    /// `i32` slot 1 is bytes 4-7, not byte 1.
    #[test]
    fn i32_slots_are_scaled_by_width() {
        let mut r = Returns::default();
        r.set_i32(1, 0x7f);
        assert_eq!(r.at_i32(1), 0x7f);
        assert_eq!(r.at_i32(0), 0, "slot 0 is a different four bytes");
        assert_eq!(r.at_i8(4), 0x7f, "which begins at byte 4");
    }

    /// Negative answers round-trip: `-1` is the "not answered yet" sentinel
    /// several units write.
    #[test]
    fn the_not_answered_sentinel_round_trips() {
        let mut r = Returns::default();
        r.set(-1);
        assert_eq!(r.get(), -1);
        r.set_i8(0, -1);
        assert_eq!(r.at_i8(0), -1);
    }

    #[test]
    fn clearing_returns_to_all_zeroes() {
        let mut r = Returns::default();
        r.set(42);
        r.clear();
        assert_eq!(r.get(), 0);
    }
}

/// `CHINT_*` (`ocgapi_constants.h:317-323`) — the kinds of `MSG_CARD_HINT`,
/// transcribed whole.
pub mod chint {
    pub const TURN: u8 = 1;
    pub const CARD: u8 = 2;
    pub const RACE: u8 = 3;
    pub const ATTRIBUTE: u8 = 4;
    pub const NUMBER: u8 = 5;
    pub const DESC_ADD: u8 = 6;
    pub const DESC_REMOVE: u8 = 7;
}

#[cfg(test)]
mod deck_order_tests {
    use super::*;
    use crate::board::location;
    use crate::card::{card_type, Card, CardData};

    fn deck_of(f: &mut Field, player: u8, n: u32) -> Vec<CardId> {
        (0..n)
            .map(|i| {
                let c = f.new_card_nowhere(Card::with_data(
                    CardData {
                        code: i + 1,
                        type_: card_type::MONSTER,
                        ..Default::default()
                    },
                    player,
                ));
                f.add_card(player, c, location::DECK, 0, false);
                c
            })
            .collect()
    }

    /// A permutation is imposed, and every card's sequence follows it.
    #[test]
    fn a_permutation_is_imposed_with_its_sequences() {
        let mut f = Field::new(8000);
        let deck = deck_of(&mut f, 0, 4);
        let order = vec![deck[2], deck[0], deck[3], deck[1]];
        assert!(f.set_deck_order(0, &order));
        assert_eq!(f.players[0].main, order);
        for (seq, &c) in order.iter().enumerate() {
            assert_eq!(f.cards[c].current.sequence, seq as u32, "sequence of {c}");
        }
    }

    /// Anything but a permutation of the pile is refused, and nothing moves.
    #[test]
    fn anything_but_a_permutation_is_refused() {
        let mut f = Field::new(8000);
        let deck = deck_of(&mut f, 0, 3);
        let other = deck_of(&mut f, 1, 1);
        let before = f.players[0].main.clone();
        assert!(!f.set_deck_order(0, &deck[..2]), "too short");
        assert!(
            !f.set_deck_order(0, &[deck[0], deck[1], deck[1]]),
            "a repeat"
        );
        assert!(
            !f.set_deck_order(0, &[deck[0], deck[1], other[0]]),
            "a stranger"
        );
        assert_eq!(f.players[0].main, before, "untouched");
        for (seq, &c) in before.iter().enumerate() {
            assert_eq!(f.cards[c].current.sequence, seq as u32);
        }
    }
}

//! Cards: identity, where they are, and what they are entangled with.
//!
//! A translation of the part of ocgcore's `card` that the rules machinery
//! reads. The statistics — attack, level, type, attribute — arrive with the
//! card pool; what the machinery needs first is identity, position, status,
//! and the relation sets.

use crate::board::Loc;
use crate::event::{CardId, EffectId};
use crate::field::{EffectIndex, Field};

/// `check_unique_code` when `unique_code == 1`: a function decides which
/// cards the uniqueness limit covers, rather than a card name.
pub type UniqueFilter = fn(&Field, CardId) -> bool;
use std::collections::{BTreeMap, BTreeSet, HashSet};

/// A card as printed: what the database says, before the duel touches it.
///
/// `card_data` in the reference, translated field for field. Every field is
/// here even where nothing yet reads it, because this is a table and a
/// partial table invites inventing the rest.
///
/// The distinction between this and a card's *current* properties is
/// load-bearing and easy to lose. `data.type` is what is printed on the
/// card; `card::get_type()` is what it counts as right now, after every
/// effect that adds, removes or changes a type has had its say. The
/// reference reads `data.type` directly in some places and `get_type()` in
/// others, and the choice is never incidental — `is_activateable`, for one,
/// asks `data.type` about quick-play and trap status precisely because a
/// type-changing effect must not make a trap activatable from the hand.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CardData {
    pub code: u32,
    /// The card this one is an alternate artwork of, if any. Rules that
    /// count copies count aliases together.
    pub alias: u32,
    /// Archetype membership. A set in the reference, and ordered, so it is
    /// ordered here.
    pub setcodes: BTreeSet<u16>,
    /// A `card_type::*` mask.
    pub type_: u32,
    pub level: u32,
    /// An `attribute::*` mask.
    pub attribute: u32,
    /// A `race::*` mask. 64 bits in the reference — the table outgrew 32.
    pub race: u64,
    pub attack: i32,
    pub defense: i32,
    pub lscale: u32,
    pub rscale: u32,
    pub link_marker: u32,
}

impl CardData {
    /// Does the printed type match this mask?
    ///
    /// Deliberately *not* called `get_type`. This reads what is printed;
    /// `get_type` is the resolved question and belongs on `Card`, where the
    /// effects that change a type can be consulted.
    pub fn is_type(&self, mask: u32) -> bool {
        self.type_ & mask != 0
    }
}

/// How a card came to be on the field.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SummonInfo {
    /// A `summon_type::*` mask. Note it accumulates: an advance summon is
    /// `NORMAL | ADVANCE`, not `ADVANCE` alone.
    pub type_: u32,
    pub player: u8,
    /// Where the card came **from**, which is the question most rules asking
    /// about a summon actually mean.
    pub location: u8,
    pub sequence: u8,
    pub pzone: bool,
}

/// Where a card is being sent. `sendto_param_t` in the reference.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SendToParam {
    pub playerid: u8,
    pub position: u8,
    pub location: u8,
    pub sequence: u32,
}

/// Partial results held while a resolved property is being computed.
///
/// This is the reference's `temp`, and it is worth being explicit about what
/// it is for, because it looks exactly like a memoisation cache and is not
/// one. It is a **recursion guard**.
///
/// `card::get_type` may run an effect's value function, and that function
/// may itself ask the card for its type. The reference handles it like this:
///
/// ```text
/// if (has_valid_property_val(temp.type)) return temp.type;  // already inside
/// temp.type = data.type;                                     // mark: computing
/// ... loop, assigning temp.type = type as each effect applies ...
/// set_max_property_val(temp.type);                           // clear on the way out
/// return type;
/// ```
///
/// The sentinel is all-ones, and `Option` says the same thing more plainly:
/// `None` means "not currently computing", `Some(v)` means "computing, and
/// this much is settled". Two ways to get this wrong, both quiet:
///
/// - Treat it as a cache and leave the value in place on the way out, and
///   every later call returns a stale answer.
/// - Omit it, and a card whose type depends on its own type recurses until
///   the stack goes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Temp {
    pub code: Option<u32>,
    pub type_: Option<u32>,
    pub level: Option<u32>,
    pub rank: Option<u32>,
    pub attribute: Option<u32>,
    pub race: Option<u64>,
    pub attack: Option<i32>,
    pub defense: Option<i32>,
    pub base_attack: Option<i32>,
    pub base_defense: Option<i32>,

    /// Why the card was *about* to move, parked while something decides
    /// whether it will. Unlike the fields above these are not a recursion
    /// guard: they are a saved copy, restored when a replacement effect
    /// cancels the move.
    pub reason: u32,
    pub reason_effect: Option<crate::event::EffectId>,
    pub reason_player: u8,
    pub reason_card: Option<crate::event::CardId>,
    /// The position a card had when a battle began, saved so that case 22
    /// can tell a target that *was* face-down from one that always was up.
    pub position: u8,
    /// The seat a `MoveToField` has chosen, parked between its steps.
    ///
    /// Not a recursion guard and not a saved copy: the unit yields to the
    /// host between choosing the seat and using it, so the choice has to
    /// survive somewhere the unit can read after resuming. The reference
    /// parks it on the card for the same reason.
    pub sequence: u32,
}

/// Keys for [`Card::assume`], as the reference numbers them.
pub mod assume {
    pub const CODE: u32 = 1;
    pub const TYPE: u32 = 2;
    pub const LEVEL: u32 = 3;
    pub const RANK: u32 = 4;
    pub const ATTRIBUTE: u32 = 5;
    pub const RACE: u32 = 6;
    pub const ATTACK: u32 = 7;
    pub const DEFENSE: u32 = 8;
}

/// The two cards that carry a second printed name. The reference hardcodes
/// the mapping and so does this.
/// Spirit's Invitation — "you cannot Special Summon", asked for by
/// **card number** rather than by an effect code, as several of the
/// library's summon procedures do. Pinned against
/// `card_counter_constants.lua` by `tools/check_constants.py`; the file
/// holds 157 of these and only the ones this port names are carried.
pub const CARD_BLUEEYES_SPIRIT: u32 = 59822133;

pub const CARD_MARINE_DOLPHIN: u32 = 78734254;
pub const CARD_TWINKLE_MOSS: u32 = 13857930;

/// `card::second_code` — the other name of a double-name card.
pub fn second_code(code: u32) -> u32 {
    match code {
        CARD_MARINE_DOLPHIN => 17955766,
        CARD_TWINKLE_MOSS => 17732278,
        _ => 0,
    }
}

/// A snapshot of a card at a moment.
///
/// `card_state` in the reference, where it serves three purposes: a card's
/// `current`, its `previous`, and a chain's `triggering_state`. The last is
/// why it has to be a *copy* rather than a look at the live card — an effect
/// that triggers on a card leaving the field must still be able to ask what
/// that card was, after it has stopped being it.
///
/// The reference's version also carries the statistics. Those are omitted
/// until there are values to put in them; the fields here are the ones the
/// machinery reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CardState {
    pub loc: Loc,
    /// A `status::*` mask, snapshotted.
    pub status: u32,
    /// Why the card came to be here: a `REASON_*` mask.
    pub reason: u32,
}

/// A card in the duel.
#[derive(Clone, Debug, Default)]
pub struct Card {
    /// What the database says about this card. `data` in the reference, and
    /// the thing to reach for when a rule asks what is *printed* rather than
    /// what the card currently counts as.
    pub data: CardData,
    /// The printed card number. The reference keeps this on `card` as well
    /// as in `data`, because a card's code can be changed by an effect while
    /// `data.code` stays what was printed.
    pub code: u32,
    /// Whose card it is. Control changes; ownership does not.
    pub owner: u8,
    /// Where it is now.
    pub current: Loc,
    /// Where it was. A great many rules ask where a card *was* — a trigger
    /// that fires on leaving the field has to know the field it left.
    pub previous: Loc,
    /// What the card *was*, statistically, when it last left the field.
    ///
    /// The reference keeps these inside `previous` — one `card_state` type
    /// serves both `current` and `previous`. They are a separate field here
    /// because ocgcore never reads or writes the statistics half of
    /// `current`, and because this port copies `previous = current`
    /// wholesale in four places: were they inside `Loc`, that copy would
    /// silently wipe the snapshot `SendTo` takes on the way out. See
    /// `leave_snapshot`.
    pub previous_stats: crate::leave_snapshot::CardStats,
    /// How a pending Special Summon was asked for, packed as the reference
    /// packs it: `playerid << 24 | nocheck << 16 | nolimit << 8 | positions`.
    ///
    /// Four arguments parked on the card because `SpSummonStep` is entered
    /// once per card from a batch and cannot carry per-card arguments on the
    /// unit. Kept packed rather than as four fields so that the shifts stay
    /// the reference's, and unpacked through named accessors.
    pub spsummon_param: u32,
    /// The "once per turn" name a Special Summon is counted against.
    ///
    /// Not the card's code: several printings and several *cards* can share
    /// one, which is how "you can only Special Summon <name> once per turn"
    /// covers a whole family. Zero means the card is not limited that way,
    /// and the checks all shortcut on it.
    pub spsummon_code: u32,
    /// How many Special Summons this card's `EFFECT_SPSUMMON_COUNT_LIMIT`
    /// has seen, per player, and how many of those a chain could still take
    /// back. The `_rst` half is only ever non-zero under
    /// `DUEL_CANNOT_SUMMON_OATH_OLD`.
    pub spsummon_counter: [u16; 2],
    pub spsummon_counter_rst: [u16; 2],
    /// The battle system's per-turn tallies, all cleared by `Turn`'s first
    /// case. **The battle system itself is not ported** — `BattleCommand`
    /// is a named panic — but the reset is, because a turn that does not
    /// clear them is wrong in a way nothing later can repair.
    ///
    /// `announce_count` and `attack_announce_count` are two different
    /// counts and the names do not say which is which: the first is how
    /// many attacks this card has *declared*, the second how many times an
    /// attack declaration has been *made with* it. They are bumped at
    /// different points and cleared together.
    pub announce_count: u16,
    pub attack_announce_count: u16,
    pub attacked_count: u16,
    /// The cards this one has declared an attack on, been made to attack,
    /// and actually fought. Three tallies, because "declared against" and
    /// "fought" come apart whenever an attack is replaced or negated.
    pub announced_cards: AttackerMap,
    pub attacked_cards: AttackerMap,
    pub battled_cards: AttackerMap,
    /// Whether an "attacks all" effect on this card still has every target
    /// available. Reset to `true` each turn, not to `false` — the default
    /// is permissive.
    pub attack_all_target: bool,
    /// Whether this card may attack directly **right now**. Not a
    /// permission and not stable: `get_attack_target` *writes* it as a side
    /// effect of being asked for the legal targets, so reading it without
    /// having just called that reads a stale answer.
    pub direct_attackable: bool,
    /// Who controlled this card when it declared its attack. Pinned then
    /// and compared later, so a monster whose control changed mid-battle
    /// is recognised as no longer the card that attacked.
    pub attack_controler: u8,

    /// A `status::*` mask.
    pub status: u32,
    /// Why the card came to be where it is: a `REASON_*` mask, and the
    /// effect and player behind it.
    pub reason: u32,
    pub reason_effect: Option<EffectId>,
    pub reason_player: u8,
    /// The *card* behind what is happening to this one, where there is one
    /// and it is not the same as the effect. `Release` reads it to ask
    /// "may I be tributed for **that** monster", which is a question about
    /// the summoning card rather than about any effect.
    pub reason_card: Option<CardId>,
    /// A duel-unique id, allocated from `infos.field_id` when the card
    /// enters the field. It is what a "hard once per turn" tallies against,
    /// so that each copy of a card keeps its own count.
    pub fieldid: u32,

    /// The card this one is an Xyz material of, and — for one that has
    /// just been detached — the one it was under a moment ago. An effect
    /// resolving in that instant still has to know where it came from.
    pub overlay_target: Option<CardId>,
    pub pre_overlay_target: Option<CardId>,
    /// The card this one is equipped to, and the cards equipped to it.
    pub equiping_target: Option<CardId>,
    pub equiping_cards: Vec<CardId>,
    /// What it *was* equipped to. `unequip` records this rather than simply
    /// clearing the live pointer, because an equip card that leaves the
    /// field still has effects that ask what it had been equipped to, and by
    /// the time they run the relationship is gone. The `pre_overlay_target`
    /// above is the same idea for Xyz materials.
    pub pre_equip_target: Option<CardId>,
    /// The cards whose `target_effect`s name this one.
    pub effect_target_owner: Vec<CardId>,
    /// The Xyz materials under this card.
    pub xyz_materials: Vec<CardId>,
    /// What this card was summoned *from* — the tributes or materials it
    /// consumed. Distinct from `xyz_materials`, which are the cards still
    /// sitting under it.
    ///
    /// A **set**, as the reference's `card_set` is: `SummonRule` raises
    /// `EVENT_BE_MATERIAL` over these one at a time, so the order is
    /// observable, and `card_set` orders by creation — which the arena index
    /// already is.
    pub material_cards: std::collections::BTreeSet<CardId>,
    /// How this card came to be on the field. `summon_info` in the
    /// reference, and it is *history* rather than state: it records the
    /// summon that put the card here, and a great many rules ask about it
    /// long afterwards — "was this Special Summoned from the graveyard".
    pub summon: SummonInfo,

    /// This card's own effect containers, keyed by code exactly as the
    /// field's registration indices are — `card.h` declares the same
    /// `std::multimap<uint32_t, effect*>` type for both.
    pub single_effect: EffectIndex,
    /// The card's *field* effects — the ones it projects onto the duel
    /// rather than onto itself. `get_cteffect` walks this one.
    pub field_effect: EffectIndex,
    pub equip_effect: EffectIndex,
    pub target_effect: EffectIndex,
    pub xmaterial_effect: EffectIndex,
    /// Every effect registered on this card, whichever container it went
    /// into. `indexer` in the reference, which keeps iterators; membership
    /// is all an index-based port needs.
    pub indexer: BTreeSet<EffectId>,
    /// Cards this card's target effects are aimed at.
    pub effect_target_cards: Vec<CardId>,
    /// Counters on the card, as `[permanent, temporary]`. A disable reset
    /// takes the temporary half and leaves the other.
    pub counters: BTreeMap<u16, [u16; 2]>,
    /// How many times each `EFFECT_INDESTRUCTABLE_COUNT` effect has already
    /// saved this card, keyed by effect id.
    ///
    /// This is the *uncounted* form of the protection: an effect without
    /// `EFFECT_FLAG_COUNT_LIMIT` says "this many times" through its value
    /// instead, and the tally has to live on the card because the effect may
    /// protect several cards independently.
    pub indestructable_effects: BTreeMap<u32, u32>,

    /// Effects granting this card immunity. Not keyed by code — the
    /// reference keeps a flat set, because every one of them is consulted
    /// for every question.
    pub immune_effect: Vec<EffectId>,

    /// Where the card is being sent, set by whatever is moving it and read
    /// by `add_card` when it arrives. `sendto_param` in the reference.
    ///
    /// Worth noting that `add_card` also *writes* it — a card arriving in
    /// the deck has its position forced face-down here, not by the caller —
    /// so it is a channel in both directions rather than an argument.
    pub sendto_param: SendToParam,
    /// Where this card is being moved *to the field*, packed as the
    /// reference packs it: `(move_player << 24) | (playerid << 16) |
    /// (location << 8) | positions`. One word rather than a struct because
    /// `move_to_field` writes it and the unit unpacks it, and the reference
    /// keeps the packing rather than passing four arguments.
    pub to_field_param: u32,
    /// The position a `ChangePos` is moving this card into, with its flags.
    pub position_param: u32,
    /// How many tributes this card counts as when released for a summon.
    /// Normally 1; `EFFECT_DOUBLE_TRIBUTE` and `EFFECT_TRIPLE_TRIBUTE` raise
    /// it. Written by `get_summon_release_list` and read by whoever spends
    /// the tributes, which is why it lives on the card rather than being
    /// returned.
    pub release_param: u32,
    /// A duel-unique id that survives a control change, where `fieldid` does
    /// not. Cards are ordered by it in several places.
    /// `mt.material` / `mt.material_count` — the codes a Fusion Monster
    /// is made of, recorded by `Fusion.AddProcMix`. Metatable fields in
    /// the reference, which is script-side storage; a field here, which
    /// is the nearest thing the port has.
    pub fusion_materials: Vec<u32>,
    pub fieldid_r: u32,
    /// The turn this card arrived where it is.
    pub turnid: i16,

    /// Partial results held *while* a resolved property is being computed.
    ///
    /// The reference calls this `temp` and it reads like a cache. It is not
    /// one — see [`Temp`].
    pub temp: Temp,
    /// Properties the engine is pretending this card has, for a hypothetical
    /// question. `assume` in the reference, keyed by `assume::*`.
    pub assume: crate::fxhash::FxHashMap<u32, u64>,

    /// The "only one of these face-up at a time" machinery. `unique_code` is
    /// the card name being limited (or 1, meaning a function decides);
    /// `unique_location` which zones the limit covers; `unique_pos` which
    /// sides of the table are looked at, indexed by `controller ^ p`.
    pub unique_code: u32,
    pub unique_location: u16,
    pub unique_pos: [u8; 2],
    pub unique_fieldid: u32,
    pub unique_effect: Option<EffectId>,
    /// Used when `unique_code == 1`: the name test is a function.
    pub unique_filter: Option<UniqueFilter>,

    /// Effects this card is entangled with, keyed by `(effect, chain_id)`.
    ///
    /// `relate_effect` in the reference, and the mechanism behind a great
    /// many "is it still the card the effect was aimed at?" questions. The
    /// chain id is part of the key rather than decoration: the same effect
    /// can be on two chains at once, and a relation from one must not answer
    /// for the other.
    relate_effect: HashSet<(EffectId, u16)>,
    /// Card-to-card relations, with the reset mask that clears them.
    relations: crate::fxhash::FxHashMap<CardId, u32>,
}

/// `card::attacker_map` — how many times this card has attacked each
/// target.
///
/// Two things about it are load-bearing and neither is visible from the
/// name:
///
/// - **The key is `fieldid_r`, not the card.** A card that leaves the
///   field and comes back has a new field id, so it counts as a *different*
///   target. An "attacks each monster once" effect therefore lets the
///   attacker hit the same physical card twice if it left in between.
/// - **Key 0 is the direct attack.** The reference keys `nullptr` to 0, so
///   attacking the player is tallied in the same map as attacking monsters,
///   under a key no real card can have.
///
/// It counts rather than records: `findcard` returns how many times, and
/// the "attacks all" check compares that count against the effect's value.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AttackerMap(BTreeMap<u32, (Option<CardId>, u32)>);

impl AttackerMap {
    /// `addcard` — one more attack against this target. `None` is the
    /// player.
    pub fn add(&mut self, target: Option<CardId>, fieldid_r: u32) {
        let key = if target.is_some() { fieldid_r } else { 0 };
        let entry = self.0.entry(key).or_insert((target, 0));
        entry.1 += 1;
    }

    /// `findcard` — how many times, or zero.
    pub fn count(&self, fieldid_r: u32, is_card: bool) -> u32 {
        let key = if is_card { fieldid_r } else { 0 };
        self.0.get(&key).map_or(0, |e| e.1)
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl Card {
    pub fn new(code: u32, owner: u8) -> Self {
        Self {
            data: CardData {
                code,
                ..Default::default()
            },
            code,
            owner,
            ..Default::default()
        }
    }

    /// A card with its printed properties.
    pub fn with_data(data: CardData, owner: u8) -> Self {
        Self {
            code: data.code,
            data,
            owner,
            ..Default::default()
        }
    }

    /// `card::is_status` (`card.h:249`) — **every** bit of `mask` is set.
    ///
    /// The reference has two readers and they are not interchangeable:
    /// this one requires all the bits, `get_status` any of them. Until
    /// 2026-09-14 this port had one function under this name with the
    /// any-bit meaning, which was right at sixteen multi-bit call sites
    /// (they mirror `get_status` or OR'd single-bit checks in the
    /// reference) and wrong at one — `adjust_disable_check_list`'s loop
    /// guard, which mirrors an all-bit `is_status`. Found by cross-checking
    /// the script-library note against its 2026-09-08 predecessor, which
    /// recorded the same trap on the Lua side (`Card.IsStatus` is any-bit).
    pub fn is_status(&self, mask: u32) -> bool {
        self.status & mask == mask
    }

    /// `card::get_status` (`card.h:245`) — **any** bit of `mask` is set.
    ///
    /// For a single-bit mask the two agree; the distinction only exists
    /// for a composed mask, and every composed call in this crate names
    /// the function the reference uses at that site.
    pub fn get_status(&self, mask: u32) -> bool {
        self.status & mask != 0
    }

    pub fn set_status(&mut self, mask: u32, on: bool) {
        if on {
            self.status |= mask;
        } else {
            self.status &= !mask;
        }
    }

    /// `check_unique_code` — is `other` one of the cards this card's
    /// uniqueness limit covers?
    ///
    /// `unique_code == 0` means the card imposes no limit at all, so it
    /// covers nothing — including itself. The reference returns FALSE there
    /// before looking at anything.
    pub fn check_unique_code(&self, field: &Field, other: CardId) -> bool {
        match self.unique_code {
            0 => false,
            1 => self.unique_filter.is_some_and(|f| f(field, other)),
            code => {
                let c = &field.cards[other];
                // `get_code` and `get_another_code` resolve name-changing
                // effects. Until those exist this is the printed name, which
                // is what they fall back to.
                c.code == code
            }
        }
    }

    /// `is_extra_deck_monster` — does this card belong in the Extra Deck?
    ///
    /// Takes the duel's list of extra-deck types, because
    /// `DUEL_EXTRA_DECK_RITUAL` adds Ritual monsters to it.
    pub fn is_extra_deck_monster(&self, extra_deck_types: u32) -> bool {
        self.data.is_type(card_type::MONSTER) && self.data.type_ & extra_deck_types != 0
    }

    /// A snapshot of this card as it is now.
    pub fn state(&self) -> CardState {
        CardState {
            loc: self.current,
            status: self.status,
            reason: self.reason,
        }
    }

    /// `create_relation(const chain&)` — entangle this card with one chain's
    /// effect. Takes the parts of the chain the relation is keyed by, so
    /// that a card need not borrow the chain it is being related to.
    pub fn create_chain_relation(&mut self, effect: EffectId, chain_id: u16) {
        self.relate_effect.insert((effect, chain_id));
    }

    pub fn has_chain_relation(&self, effect: EffectId, chain_id: u16) -> bool {
        self.relate_effect.contains(&(effect, chain_id))
    }

    pub fn release_chain_relation(&mut self, effect: EffectId, chain_id: u16) {
        self.relate_effect.remove(&(effect, chain_id));
    }

    /// `is_has_relation(effect*)` — any relation to this effect, whatever
    /// chain it is on. The reference scans for a matching first element, and
    /// so does this.
    pub fn has_effect_relation(&self, effect: EffectId) -> bool {
        self.relate_effect.iter().any(|&(e, _)| e == effect)
    }

    /// Insert a plain effect relation, for tests that need to stand a
    /// card in the state an activation would have left it.
    #[cfg(test)]
    pub fn relate_effect_insert_for_test(&mut self, effect: EffectId) {
        self.relate_effect.insert((effect, 0));
    }

    pub fn clear_relate_effect(&mut self) {
        self.relate_effect.clear();
    }

    pub fn create_relation(&mut self, target: CardId, reset: u32) {
        self.relations.insert(target, reset);
    }

    pub fn has_relation(&self, target: CardId) -> bool {
        self.relations.contains_key(&target)
    }

    pub fn release_relation(&mut self, target: CardId) {
        self.relations.remove(&target);
    }

    /// Keep only the card-to-card relations whose reset mask satisfies the
    /// predicate. `card::reset`'s first loop.
    pub fn retain_relations(&mut self, keep: impl Fn(u32) -> bool) {
        self.relations.retain(|_, &mut mask| keep(mask));
    }
}

/// `TYPE_*` — what a card is. A mask: a card is several of these at once
/// (a Flip Effect Monster is `MONSTER | EFFECT | FLIP`).
///
/// Named `card_type` rather than `type` because `type` is a Rust keyword.
pub mod card_type {
    pub const MONSTER: u32 = 0x1;
    pub const SPELL: u32 = 0x2;
    pub const TRAP: u32 = 0x4;
    pub const NORMAL: u32 = 0x10;
    pub const EFFECT: u32 = 0x20;
    pub const FUSION: u32 = 0x40;
    pub const RITUAL: u32 = 0x80;
    pub const TRAPMONSTER: u32 = 0x100;
    pub const SPIRIT: u32 = 0x200;
    pub const UNION: u32 = 0x400;
    pub const GEMINI: u32 = 0x800;
    pub const TUNER: u32 = 0x1000;
    pub const SYNCHRO: u32 = 0x2000;
    pub const TOKEN: u32 = 0x4000;
    pub const MAXIMUM: u32 = 0x8000;
    pub const QUICKPLAY: u32 = 0x10000;
    pub const CONTINUOUS: u32 = 0x20000;
    pub const EQUIP: u32 = 0x40000;
    pub const FIELD: u32 = 0x80000;
    pub const COUNTER: u32 = 0x100000;
    pub const FLIP: u32 = 0x200000;
    pub const TOON: u32 = 0x400000;
    pub const XYZ: u32 = 0x800000;
    pub const PENDULUM: u32 = 0x1000000;
    pub const SPSUMMON: u32 = 0x2000000;
    pub const LINK: u32 = 0x4000000;
}

/// `ATTRIBUTE_*`.
pub mod attribute {
    pub const EARTH: u32 = 0x01;
    pub const WATER: u32 = 0x02;
    pub const FIRE: u32 = 0x04;
    pub const WIND: u32 = 0x08;
    pub const LIGHT: u32 = 0x10;
    pub const DARK: u32 = 0x20;
    pub const DIVINE: u32 = 0x40;
    pub const ALL: u32 = DARK | DIVINE | EARTH | FIRE | LIGHT | WATER | WIND;
}

/// `RACE_*` — monster types. 64 bits, because the table outgrew 32 and the
/// reference widened it; `RACE_YOKAI` sits well above the 32-bit range.
pub mod race {
    pub const WARRIOR: u64 = 0x1;
    pub const SPELLCASTER: u64 = 0x2;
    pub const FAIRY: u64 = 0x4;
    pub const FIEND: u64 = 0x8;
    pub const ZOMBIE: u64 = 0x10;
    pub const MACHINE: u64 = 0x20;
    pub const AQUA: u64 = 0x40;
    pub const PYRO: u64 = 0x80;
    pub const ROCK: u64 = 0x100;
    pub const WINGEDBEAST: u64 = 0x200;
    pub const PLANT: u64 = 0x400;
    pub const INSECT: u64 = 0x800;
    pub const THUNDER: u64 = 0x1000;
    pub const DRAGON: u64 = 0x2000;
    pub const BEAST: u64 = 0x4000;
    pub const BEASTWARRIOR: u64 = 0x8000;
    pub const DINOSAUR: u64 = 0x10000;
    pub const FISH: u64 = 0x20000;
    pub const SEASERPENT: u64 = 0x40000;
    pub const REPTILE: u64 = 0x80000;
    pub const PSYCHIC: u64 = 0x100000;
    pub const DIVINE: u64 = 0x200000;
    pub const CREATORGOD: u64 = 0x400000;
    pub const WYRM: u64 = 0x800000;
    pub const CYBERSE: u64 = 0x1000000;
    pub const ILLUSION: u64 = 0x2000000;
    pub const CYBORG: u64 = 0x4000000;
    pub const MAGICALKNIGHT: u64 = 0x8000000;
    pub const HIGHDRAGON: u64 = 0x10000000;
    pub const OMEGAPSYCHIC: u64 = 0x20000000;
    pub const CELESTIALWARRIOR: u64 = 0x40000000;
    pub const GALAXY: u64 = 0x80000000;
    pub const YOKAI: u64 = 0x4000000000000000;
    pub const MAX: u64 = GALAXY;
    pub const ALL: u64 = ((MAX << 1) - 1) | YOKAI;
}

/// Per-card status bits, as the reference numbers them.
///
/// The whole table, not a subset. A partial one invites inventing the
/// missing entries, and a status bit with a plausible-looking wrong value is
/// not something a test would catch.
pub mod status {
    pub const DISABLED: u32 = 0x1;
    pub const TO_ENABLE: u32 = 0x2;
    pub const TO_DISABLE: u32 = 0x4;
    pub const PROC_COMPLETE: u32 = 0x8;
    pub const SET_TURN: u32 = 0x10;
    pub const NO_LEVEL: u32 = 0x20;
    pub const BATTLE_RESULT: u32 = 0x40;
    pub const SPSUMMON_STEP: u32 = 0x80;
    pub const FORM_CHANGED: u32 = 0x100;
    pub const SUMMONING: u32 = 0x200;
    /// The gate the trigger gather checks before offering a trigger effect.
    pub const EFFECT_ENABLED: u32 = 0x400;
    pub const SUMMON_TURN: u32 = 0x800;
    pub const DESTROY_CONFIRMED: u32 = 0x1000;
    pub const LEAVE_CONFIRMED: u32 = 0x2000;
    pub const BATTLE_DESTROYED: u32 = 0x4000;
    pub const COPYING_EFFECT: u32 = 0x8000;
    pub const CHAINING: u32 = 0x10000;
    pub const SUMMON_DISABLED: u32 = 0x20000;
    pub const ACTIVATE_DISABLED: u32 = 0x40000;
    pub const EFFECT_REPLACED: u32 = 0x80000;
    pub const FUTURE_FUSION: u32 = 0x100000;
    pub const ATTACK_CANCELED: u32 = 0x200000;
    pub const INITIALIZING: u32 = 0x400000;
    pub const JUST_POS: u32 = 0x1000000;
    pub const CONTINUOUS_POS: u32 = 0x2000000;
    pub const FORBIDDEN: u32 = 0x4000000;
    pub const ACT_FROM_HAND: u32 = 0x8000000;
    pub const OPPO_BATTLE: u32 = 0x10000000;
    pub const FLIP_SUMMON_TURN: u32 = 0x20000000;
    pub const SPSUMMON_TURN: u32 = 0x40000000;
}

/// Why something happened, as the reference numbers it. The whole table,
/// for the same reason `status` is whole.
/// How a monster came to be on the field. `SUMMON_TYPE_*`.
///
/// Note the shape: `ADVANCE` is `NORMAL` **plus a bit**, not a sibling of
/// it. A tribute summon is a normal summon, so `sumtype & SUMMON_TYPE_NORMAL`
/// is true for both — and `is_player_can_summon` relies on that, ORing
/// `SUMMON_TYPE_NORMAL` into whatever it is given before asking.
pub mod summon_type {
    pub const NORMAL: u32 = 0x1000_0000;
    pub const ADVANCE: u32 = 0x1100_0000;
    pub const GEMINI: u32 = 0x1200_0000;
    pub const FLIP: u32 = 0x2000_0000;
    pub const SPECIAL: u32 = 0x4000_0000;
    /// The Special Summon kinds. Each **contains** `SPECIAL`, so a test for
    /// "was this Special Summoned" answers true for all of them.
    pub const FUSION: u32 = 0x4300_0000;
    pub const RITUAL: u32 = 0x4500_0000;
    pub const SYNCHRO: u32 = 0x4600_0000;
    pub const XYZ: u32 = 0x4900_0000;
    pub const PENDULUM: u32 = 0x4a00_0000;
    pub const LINK: u32 = 0x4c00_0000;
}

pub mod reason {
    pub const DESTROY: u32 = 0x1;
    pub const RELEASE: u32 = 0x2;
    /// A card that left the field only for a moment. The trigger gather
    /// reads this one: a card returning has its *previous* controller ask
    /// the question.
    pub const TEMPORARY: u32 = 0x4;
    pub const MATERIAL: u32 = 0x8;
    pub const SUMMON: u32 = 0x10;
    pub const BATTLE: u32 = 0x20;
    pub const EFFECT: u32 = 0x40;
    pub const COST: u32 = 0x80;
    pub const ADJUST: u32 = 0x100;
    pub const LOST_TARGET: u32 = 0x200;
    pub const RULE: u32 = 0x400;
    pub const SPSUMMON: u32 = 0x800;
    pub const DISSUMMON: u32 = 0x1000;
    pub const FLIP: u32 = 0x2000;
    pub const DISCARD: u32 = 0x4000;
    pub const RDAMAGE: u32 = 0x8000;
    pub const RRECOVER: u32 = 0x10000;
    pub const RETURN: u32 = 0x20000;
    pub const FUSION: u32 = 0x40000;
    pub const SYNCHRO: u32 = 0x80000;
    pub const RITUAL: u32 = 0x100000;
    pub const XYZ: u32 = 0x200000;
    pub const REPLACE: u32 = 0x1000000;
    /// `REASON_EXCAVATE` (`constant.lua:150`) — **not defined by the
    /// core**, as the script library says in so many words. It is a
    /// script-side flag for "this left a deck because it was dug
    /// through", and `REASON_REVEAL` is an alias of the same bit.
    ///
    /// Kept here beside the core's own reasons because a card passes it
    /// in the same argument, and checked against `constant.lua` rather
    /// than against a C header.
    pub const EXCAVATE: u32 = 0x8000000;
    /// `REASON_REVEAL` (`constant.lua:152`) — the same bit as
    /// [`EXCAVATE`], under the name a revealing card uses.
    pub const REVEAL: u32 = EXCAVATE;
    pub const DRAW: u32 = 0x2000000;
    pub const REDIRECT: u32 = 0x4000000;
    pub const LINK: u32 = 0x10000000;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The two status readers differ on a composed mask, and only
    /// there.** `is_status` is the reference's all-bit test (`card.h:249`),
    /// `get_status` its any-bit test (`:245`). One bit of a two-bit mask
    /// satisfies the second and not the first; a single-bit mask satisfies
    /// both alike. Pinned because this crate had one function under the
    /// all-bit name with the any-bit meaning until 2026-09-14.
    #[test]
    fn is_status_needs_every_bit_and_get_status_any() {
        let mut c = Card::with_data(CardData::default(), 0);
        c.set_status(status::TO_DISABLE, true);
        let both = status::TO_ENABLE | status::TO_DISABLE;
        assert!(c.get_status(both), "any-bit: one of two is enough");
        assert!(!c.is_status(both), "all-bit: one of two is not");
        assert!(c.is_status(status::TO_DISABLE) && c.get_status(status::TO_DISABLE));
        c.set_status(status::TO_ENABLE, true);
        assert!(c.is_status(both), "all-bit: both is enough");
    }

    #[test]
    fn status_bits_set_and_clear() {
        let mut c = Card::new(46986414, 0);
        assert!(!c.is_status(status::EFFECT_ENABLED));
        c.set_status(status::EFFECT_ENABLED, true);
        c.set_status(status::SET_TURN, true);
        assert!(c.is_status(status::EFFECT_ENABLED));
        assert!(c.is_status(status::SET_TURN));
        c.set_status(status::EFFECT_ENABLED, false);
        assert!(!c.is_status(status::EFFECT_ENABLED));
        assert!(
            c.is_status(status::SET_TURN),
            "clearing one leaves the rest"
        );
    }

    /// The values are the reference's, not ours. Spot-checked at both ends
    /// of the table and at the one the trigger gather reads.
    #[test]
    fn status_values_are_the_references() {
        assert_eq!(status::DISABLED, 0x1);
        assert_eq!(status::EFFECT_ENABLED, 0x400);
        assert_eq!(status::FORBIDDEN, 0x4000000);
        assert_eq!(status::SPSUMMON_TURN, 0x40000000);
    }

    mod printed_data {
        use super::*;

        /// Pinned literals across each table, so a value that drifts from
        /// the reference fails here rather than as a rule that quietly stops
        /// matching a card.
        #[test]
        fn the_tables_are_the_references() {
            assert_eq!(card_type::MONSTER, 0x1);
            assert_eq!(card_type::QUICKPLAY, 0x10000);
            assert_eq!(card_type::FLIP, 0x200000);
            assert_eq!(card_type::LINK, 0x4000000);

            assert_eq!(attribute::EARTH, 0x01);
            assert_eq!(attribute::DIVINE, 0x40);
            assert_eq!(attribute::ALL, 0x7f);

            assert_eq!(race::WARRIOR, 0x1);
            assert_eq!(race::SPELLCASTER, 0x2);
            assert_eq!(race::GALAXY, 0x80000000);
            assert_eq!(
                race::YOKAI,
                0x4000000000000000,
                "the table outgrew 32 bits, which is why race is a u64"
            );
        }

        /// A card is several types at once, which is why every test is a
        /// mask test.
        #[test]
        fn a_printed_type_is_a_mask() {
            // Magician of Faith: a Flip Effect Monster.
            let d = CardData {
                code: 31560081,
                type_: card_type::MONSTER | card_type::EFFECT | card_type::FLIP,
                level: 1,
                attribute: attribute::LIGHT,
                race: race::SPELLCASTER,
                attack: 300,
                defense: 400,
                ..Default::default()
            };
            assert!(d.is_type(card_type::MONSTER));
            assert!(d.is_type(card_type::FLIP));
            assert!(!d.is_type(card_type::SPELL));
            assert!(
                d.is_type(card_type::SPELL | card_type::MONSTER),
                "a mask matches on any bit, as the reference's & does"
            );
        }

        /// A card carries its printed data, and `new` leaves the rest of it
        /// blank rather than guessing.
        #[test]
        fn a_card_carries_its_printed_data() {
            let d = CardData {
                code: 83555666,
                type_: card_type::TRAP,
                ..Default::default()
            };
            let c = Card::with_data(d, 1);
            assert_eq!(c.code, 83555666);
            assert!(c.data.is_type(card_type::TRAP));
            assert_eq!(c.owner, 1);

            let bare = Card::new(83555666, 0);
            assert_eq!(bare.data.code, 83555666);
            assert_eq!(bare.data.type_, 0, "unknown, not guessed");
        }

        /// Archetype membership is an ordered set in the reference, and
        /// ordering a set is only free if you keep it ordered.
        #[test]
        fn setcodes_are_an_ordered_set() {
            let mut d = CardData::default();
            d.setcodes.insert(0x54);
            d.setcodes.insert(0x11);
            d.setcodes.insert(0x54);
            let got: Vec<u16> = d.setcodes.iter().copied().collect();
            assert_eq!(got, vec![0x11, 0x54], "sorted, and no duplicate");
        }
    }

    /// A card remembers where it was, because the rules ask.
    #[test]
    fn a_card_carries_both_where_it_is_and_where_it_was() {
        use crate::board::location;
        let mut c = Card::new(46986414, 0);
        c.current.location = location::MZONE;
        c.previous = c.current;
        c.current.location = location::GRAVE;
        assert!(c.current.is_location(u16::from(location::GRAVE)));
        assert!(
            c.previous.is_location(u16::from(location::MZONE)),
            "it left the field"
        );
    }

    mod relations {
        use super::*;

        /// The chain id is part of the key. One effect on two chains at once
        /// is two relations, and releasing one leaves the other — which is
        /// the whole reason the reference keys on the pair.
        #[test]
        fn a_relation_is_to_an_effect_on_a_chain_not_to_an_effect() {
            let mut c = Card::new(44095762, 0);
            c.create_chain_relation(7, 1);
            c.create_chain_relation(7, 2);
            assert!(c.has_chain_relation(7, 1));
            assert!(c.has_chain_relation(7, 2));

            c.release_chain_relation(7, 1);
            assert!(!c.has_chain_relation(7, 1));
            assert!(c.has_chain_relation(7, 2), "the other chain is untouched");
            assert!(
                c.has_effect_relation(7),
                "but the effect is still related, on some chain"
            );

            c.release_chain_relation(7, 2);
            assert!(!c.has_effect_relation(7));
        }

        #[test]
        fn clearing_drops_every_chain_relation() {
            let mut c = Card::new(44095762, 0);
            c.create_chain_relation(1, 1);
            c.create_chain_relation(2, 1);
            c.clear_relate_effect();
            assert!(!c.has_effect_relation(1));
            assert!(!c.has_effect_relation(2));
        }

        /// Card-to-card relations are a separate set, carrying the reset
        /// mask that will clear them.
        #[test]
        fn card_relations_are_a_different_set_from_effect_relations() {
            let mut c = Card::new(44095762, 0);
            c.create_relation(3, reason::DESTROY);
            assert!(c.has_relation(3));
            assert!(!c.has_effect_relation(3), "a different set entirely");
            c.release_relation(3);
            assert!(!c.has_relation(3));
        }
    }

    /// A snapshot is a copy, not a look at the live card. That is the point
    /// of it: a chain holds one so it can still answer what a card *was*.
    #[test]
    fn a_snapshot_does_not_follow_the_card() {
        use crate::board::location;
        let mut c = Card::new(31560081, 0);
        c.current.location = location::MZONE;
        c.set_status(status::EFFECT_ENABLED, true);
        let snap = c.state();

        c.current.location = location::GRAVE;
        c.set_status(status::EFFECT_ENABLED, false);

        assert!(snap.loc.is_location(u16::from(location::MZONE)));
        assert_eq!(snap.status & status::EFFECT_ENABLED, status::EFFECT_ENABLED);
    }
}

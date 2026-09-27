//! The card-facing API: what a translated card script calls.
//!
//! In the reference a card script talks to the core through the Lua
//! tables `Card`, `Duel`, `Effect` and `Group` — and the shared script
//! library **replaces** some of those entries with its own versions before
//! any card loads. So a script's `c:IsRelateToEffect(e)` runs library
//! code that routes to a different core overload than the C++ itself uses.
//! `docs/script-library.md` enumerates every such replacement.
//!
//! This module is the port's answer to that layer, and it is a **split,
//! not a wrapper**: each function here *is* the script-facing semantics,
//! written directly. The core's own methods keep their own meaning and are
//! what the processor units call; a translated card calls only this
//! module. The two are kept apart by convention and by a grep over
//! `cards/`, the way the no-engine dependency is kept by `cargo tree`.
//!
//! ## What is faithful to which layer
//!
//! Every function names, in its doc, the C++ export it stands for and the
//! library site that replaced it, if any. Where the library's change is
//! unreachable for this pool it is still written down here, so the next
//! pool inherits the knowledge rather than the omission.
//!
//! ## `Ctx` is the argument list
//!
//! A Lua target or operation receives `(e, tp, eg, ep, ev, re, r, rp[, chk])`.
//! Here those are `ctx.reason_effect`, `ctx.player`, and `ctx.event`'s
//! fields; `chk` is the explicit `bool` on `Target` and `Cost`.

use crate::board::{location, position};
use crate::card::{card_type, reason};
use crate::chain::{Chain, OpTarget};
use crate::effect::{effect_type, flag, Ctx, Effect, LabelObject, Operation, Target, Yield};
use crate::event::{code, CardId, EffectId, PLAYER_NONE};
use crate::field::{reset, Field};

// ------------------------------------------------------------------------
// Effect construction — `Effect.CreateEffect` and the setters (libeffect.cpp)
// ------------------------------------------------------------------------

/// `Effect.CreateEffect(c)` — a blank effect owned by `c`, with
/// `effect_owner` taken from the current reason player.
pub fn create_effect(f: &mut Field, c: CardId) -> EffectId {
    let mut e = Effect::new(0, 0);
    e.effect_owner = f.core.reason_player;
    e.owner = Some(c);
    f.new_effect(e)
}

/// `Effect.SetType` — with the three folds the reference applies: an
/// action type (`0x0ff0`) gains `ACTIONS`; an activate/ignition/quick type
/// gains `FIELD`; an `ACTIVATE` sets the range to Spell row + Field Zone +
/// hand; a `FLIP` sets the code to `EVENT_FLIP` and defaults to `TRIGGER_F`.
pub fn set_type(f: &mut Field, e: EffectId, v: u16) {
    let mut v = v;
    if v & 0x0ff0 != 0 {
        v |= effect_type::ACTIONS;
    } else {
        v &= !effect_type::ACTIONS;
    }
    if v & (effect_type::ACTIVATE
        | effect_type::IGNITION
        | effect_type::QUICK_O
        | effect_type::QUICK_F)
        != 0
    {
        v |= effect_type::FIELD;
    }
    let Some(x) = f.effects.get_mut(e) else {
        return;
    };
    if v & effect_type::ACTIVATE != 0 {
        x.range = u16::from(location::SZONE) + location::FZONE + u16::from(location::HAND);
    }
    if v & effect_type::FLIP != 0 {
        x.code = code::FLIP;
        if v & effect_type::TRIGGER_O == 0 {
            v |= effect_type::TRIGGER_F;
        }
    }
    x.effect_type = v;
}

/// `Effect.SetProperty(v1, v2)` — the low `0x4f` bits of the first word
/// are the core's own and are preserved; everything else is replaced.
pub fn set_property(f: &mut Field, e: EffectId, v1: u32, v2: u32) {
    if let Some(x) = f.effects.get_mut(e) {
        x.flag[0] = (x.flag[0] & 0x4f) | (v1 & !0x4f);
        x.flag[1] = v2;
    }
}

pub fn set_code(f: &mut Field, e: EffectId, v: u32) {
    if let Some(x) = f.effects.get_mut(e) {
        x.code = v;
    }
}

pub fn set_category(f: &mut Field, e: EffectId, v: u64) {
    if let Some(x) = f.effects.get_mut(e) {
        x.category = v;
    }
}

pub fn set_description(f: &mut Field, e: EffectId, v: u64) {
    if let Some(x) = f.effects.get_mut(e) {
        x.description = v;
    }
}

pub fn set_target(f: &mut Field, e: EffectId, t: Target) {
    if let Some(x) = f.effects.get_mut(e) {
        x.target = Some(t);
    }
}

pub fn set_operation(f: &mut Field, e: EffectId, op: Operation) {
    if let Some(x) = f.effects.get_mut(e) {
        x.operation = Some(op);
    }
}

/// `Effect.SetCondition` on a **continuous** effect — one whose
/// condition `effect::is_available` calls rather than
/// `field::is_condition_check`.
///
/// The reference has one setter because it has one field; the port has
/// two because the two call sites disagree about borrowing. Which one a
/// card wants is decided by the effect's type, not by taste: an
/// `EFFECT_TYPE_ACTIONS` effect (activate, ignition, trigger, quick) is
/// activated and takes [`set_condition`]; anything else is simply in
/// force or not, and takes this. `Effect::avail_condition` records why.
pub fn set_avail_condition(f: &mut Field, e: EffectId, c: fn(&Field, EffectId) -> bool) {
    if let Some(x) = f.effects.get_mut(e) {
        x.avail_condition = Some(c);
    }
}

/// The availability answer of a procedure condition written with the
/// scripts' usual guard, `if c==nil then return true end`.
///
/// The reference stores one `condition` per effect and asks it two ways.
/// A summon procedure's `spcon(e,c)` is asked with the card when the
/// summon is attempted — that is the port's [`set_condition`] — and with
/// the effect **alone** by `effect::is_available`, where the guard makes
/// it *true*. That second answer is not idle: `is_available` marks the
/// effect available on a true and, the first time, renumbers it
/// (`effect.cpp`, `EFFECT_STATUS_AVAILABLE`), and the number is what
/// orders the special-summon menu. A procedure registered without this
/// keeps its enable-time number and can sort ahead of one the reference
/// renumbered — three of a million pool games.
pub fn spcon_nil_guard(_f: &Field, _e: EffectId) -> bool {
    true
}

pub fn set_condition(f: &mut Field, e: EffectId, c: crate::effect::Condition) {
    if let Some(x) = f.effects.get_mut(e) {
        x.condition = Some(c);
    }
}

pub fn set_cost(f: &mut Field, e: EffectId, c: crate::effect::Cost) {
    if let Some(x) = f.effects.get_mut(e) {
        x.cost = Some(c);
    }
}

/// `Effect.SetValue` with a plain integer. A function value takes
/// [`set_value_fn`], which also sets `FUNC_VALUE`, as the reference does.
pub fn set_value(f: &mut Field, e: EffectId, v: i64) {
    if let Some(x) = f.effects.get_mut(e) {
        x.flag[0] &= !flag::FUNC_VALUE;
        x.value = v;
        x.value_fn = None;
    }
}

/// A value function receives **its own effect** first, then the field and
/// the context — the reference pushes `e` as the Lua function's first
/// argument, so `s.aclimit(e, re, tp)` reads `e` for its label, `re` from
/// `ctx.reason_effect` and `tp` from `ctx.player`. Without the first
/// parameter a value function cannot read the label it was given, which
/// is the whole mechanism a card like Sangan uses to remember a name.
/// `Effect.SetTarget(f)` when `f` is a **filter** rather than an
/// activation target.
///
/// The reference has one setter and tells the two apart by how the effect
/// is used: a field effect's `target` is asked `(effect, card)` and
/// answers whether the card is in range, while an activation's is asked
/// nine arguments. The port has two seams for the two shapes, so a card
/// says which it means.
pub fn set_target_filter(f: &mut Field, e: EffectId, t: crate::effect::TargetFilter) {
    if let Some(x) = f.effects.get_mut(e) {
        x.target_filter = Some(t);
    }
}

pub fn set_value_fn(f: &mut Field, e: EffectId, v: crate::effect::ValueFn) {
    if let Some(x) = f.effects.get_mut(e) {
        x.flag[0] |= flag::FUNC_VALUE;
        x.value_fn = Some(v);
    }
}

/// `Effect.SetRange` — the symbolic-zone folds: both Monster halves fold
/// to `MZONE`, `MZONE` clears the halves; likewise the three Spell-row
/// parts and `SZONE`.
pub fn set_range(f: &mut Field, e: EffectId, v: u16) {
    let mut r = v;
    let mm = location::MMZONE | location::EMZONE;
    if r & mm == mm {
        r |= u16::from(location::MZONE);
    }
    if r & u16::from(location::MZONE) != 0 {
        r &= !mm;
    }
    let sz = location::FZONE | location::STZONE | location::PZONE;
    if r & sz == sz {
        r |= u16::from(location::SZONE);
    }
    if r & u16::from(location::SZONE) != 0 {
        r &= !sz;
    }
    if let Some(x) = f.effects.get_mut(e) {
        x.range = r;
    }
}

/// `Effect.SetTargetRange` — and it **clears** `ABSOLUTE_TARGET`.
pub fn set_target_range(f: &mut Field, e: EffectId, s: u16, o: u16) {
    if let Some(x) = f.effects.get_mut(e) {
        x.s_range = s;
        x.o_range = o;
        x.flag[0] &= !flag::ABSOLUTE_TARGET;
    }
}

/// `Effect.SetReset(v, c)` — a count of 0 means 1, and a phase reset with
/// neither turn named is both turns.
pub fn set_reset(f: &mut Field, e: EffectId, v: u32, c: u8) {
    let mut v = v;
    let c = if c == 0 { 1 } else { c };
    if v & reset::PHASE != 0 && v & (reset::SELF_TURN | reset::OPPO_TURN) == 0 {
        v |= reset::SELF_TURN | reset::OPPO_TURN;
    }
    if let Some(x) = f.effects.get_mut(e) {
        x.reset_flag = v;
        x.reset_count = c;
    }
}

// ------------------------------------------------------------------------
// Registration — `Card.RegisterEffect`, `Duel.RegisterEffect`
// ------------------------------------------------------------------------

/// `Card.RegisterEffect(c, e, forced)` as a script sees it.
///
/// The C++ export refuses an unforced registration while a reason effect
/// is resolving that the card is immune to (the effect is remembered in
/// `reseted_effects`), and otherwise defers to `card::add_effect` if the
/// effect has no handler yet. Returns the id, or 0.
///
/// Two library wrappers sit on top of it for a Goat duel, and both are
/// identity here, recorded rather than reproduced:
///
/// - `utility.lua:953` registers companion marker effects for extra
///   *varargs* codes. This signature has no varargs, so there are none.
/// - `proc_workaround.lua:100` rewrites the target of an
///   `EFFECT_CANNOT_SUMMON` / `_SPECIAL_SUMMON` effect to run a
///   summon-procedure "assume" hook first. The hook exists only when a
///   summon procedure's label object is a Lua table — the Link machinery,
///   nothing in this pool — so the rewritten target computes the base
///   target. Measured in the 2026-09-08 note as 557 rewrites, 0 changed
///   answers.
pub fn register_effect(f: &mut Field, c: CardId, e: EffectId, forced: bool) -> i32 {
    let Some(reason) = f.core.reason_effect else {
        return register_unchecked(f, c, e);
    };
    if !forced && !f.is_affect_by_effect(c, Some(reason)) {
        f.core.reseted_effects.insert(e);
        return 0;
    }
    register_unchecked(f, c, e)
}

fn register_unchecked(f: &mut Field, c: CardId, e: EffectId) -> i32 {
    let has_handler = f.effects.get(e).is_some_and(|x| x.handler.is_some());
    if has_handler {
        return -1;
    }
    f.add_card_effect(c, e)
}

/// `Duel.RegisterEffect(e, player)`.
///
/// Three library wrappers, outermost first (`proc_workaround.lua:381`,
/// `:144`, `chain.lua:574`), then the C++ `field::add_effect`:
///
/// - `:381` registers a `CARD_CARD_ADVANCE` flag effect when an
///   `EFFECT_EXTRA_SUMMON_COUNT` of value 1 is registered. No pool card
///   grants an extra Normal Summon, and the port has no flag-effect
///   registry yet; the case is refused loudly rather than dropped.
/// - `:144` is the same cannot-summon rewrite as the card form — identity.
/// - `chain.lua:574` snapshots the registering card's properties for the
///   `Chain.*` accessors. Nothing in the pool reads them; not kept.
pub fn duel_register_effect(f: &mut Field, e: EffectId, player: u8) {
    if player > 1 {
        return;
    }
    let extra_summon = f
        .effects
        .get(e)
        .is_some_and(|x| x.code == code::EXTRA_SUMMON_COUNT && x.value == 1);
    assert!(
        !extra_summon,
        "Duel.RegisterEffect: an EXTRA_SUMMON_COUNT of 1 also registers the CARD_ADVANCE flag \
         (proc_workaround.lua:381); no pool card does this and the flag registry is not ported"
    );
    f.add_effect(e, player);
}

// ------------------------------------------------------------------------
// Relation — `Card.IsRelateToEffect`, the one replacement that changes a
// Goat question
// ------------------------------------------------------------------------

/// `Effect.IsActivated` — `(type & 0x7f0) != 0`: any of the activated
/// kinds. A type test, not a status bit; the header's
/// `EFFECT_STATUS_ACTIVATED` is commented out.
pub fn is_activated(f: &Field, e: EffectId) -> bool {
    f.effects.get(e).is_some_and(|x| x.effect_type & 0x7f0 != 0)
}

/// `field::get_chain(count)` — `0` is the current link (the innermost
/// continuous chain if a continuous effect is the reason effect, else the
/// last of `current_chain`); otherwise the 1-based link, clamped to the
/// last.
pub fn get_chain(f: &Field, count: u8) -> Option<&Chain> {
    if count == 0 && !f.core.continuous_chain.is_empty() {
        let continuous = f
            .core
            .reason_effect
            .and_then(|r| f.effects.get(r))
            .is_some_and(|r| r.effect_type & effect_type::CONTINUOUS != 0);
        if continuous {
            return f.core.continuous_chain.back();
        }
    }
    let n = f.core.current_chain.len();
    let idx = if count == 0 || usize::from(count) > n {
        n
    } else {
        usize::from(count)
    };
    if idx == 0 {
        return None;
    }
    f.core.current_chain.get(idx - 1)
}

fn get_chain_mut(f: &mut Field, count: u8) -> Option<&mut Chain> {
    if count == 0 && !f.core.continuous_chain.is_empty() {
        let continuous = f
            .core
            .reason_effect
            .and_then(|r| f.effects.get(r))
            .is_some_and(|r| r.effect_type & effect_type::CONTINUOUS != 0);
        if continuous {
            return f.core.continuous_chain.back_mut();
        }
    }
    let n = f.core.current_chain.len();
    let idx = if count == 0 || usize::from(count) > n {
        n
    } else {
        usize::from(count)
    };
    if idx == 0 {
        return None;
    }
    f.core.current_chain.get_mut(idx - 1)
}

/// `Card.GetCardTarget()` (`libcard.cpp:1026`) — the cards **this card
/// is targeting**, which is the opposite direction from a chain link's
/// target list.
///
/// The two are kept apart in the reference by living on different
/// objects: `effect_target_cards` is what an Equip Spell (or any card
/// with a continuous target) points *at*, and `effect_target_owner` is
/// the back-reference. A chain link's `target_cards` is a different thing
/// again and is read with [`get_chain_target_cards`].
pub fn get_card_target(f: &Field, c: CardId) -> &[CardId] {
    &f.cards[c].effect_target_cards
}

/// `Card.GetCardTargetCount()` (`libcard.cpp:1037`).
pub fn get_card_target_count(f: &Field, c: CardId) -> usize {
    f.cards[c].effect_target_cards.len()
}

/// `Card.GetEquipGroup()` (`libcard.cpp:900`) — the cards equipped **to**
/// this one. `equiping_cards`, the inverse of [`get_equip_target`].
pub fn get_equip_group(f: &Field, c: CardId) -> &[CardId] {
    &f.cards[c].equiping_cards
}

/// `Card.GetOwnerTargetCount()` (`libcard.cpp:1058`) — the **other**
/// direction: how many cards are targeting *this* one.
///
/// `effect_target_owner`, not `effect_target_cards`. Ryu Senshi reads the
/// first to find a Spell aiming at it; Reaper on the Nightmare reads this
/// one to notice that anything at all is.
pub fn get_owner_target_count(f: &Field, c: CardId) -> usize {
    f.cards[c].effect_target_owner.len()
}

/// `Card.IsRelateToEffect(c, e)` — the **library's** version
/// (`proc_workaround.lua:21`), not the C++ export's.
///
/// The C++ `is_has_relation(effect*)` scans the card's relations for the
/// effect and ignores which chain the relation was made on. While a chain
/// is resolving and `e` is an activated effect, the library asks a
/// different question: is the card related to the **chain link** — the
/// `(effect, chain id)` pair — either the current one (when `e` is the
/// reason effect) or the link whose triggering effect is `e`. Only when no
/// link matches does it fall back to the effect-only scan.
///
/// The difference is the exhaustive lockstep's `no-semantics` arm at `card.cpp:2144` from
/// the other side, and for this pool it has never been observed to change
/// an answer (2,130 games). It is implemented faithfully anyway: the port
/// cannot know which future position makes it bite.
pub fn is_relate_to_effect(f: &Field, c: CardId, e: EffectId) -> bool {
    if !(f.core.chain_solving && is_activated(f, e)) {
        return f.cards[c].has_effect_relation(e);
    }
    if f.core.reason_effect == Some(e) {
        return is_relate_to_chain(f, c, 0);
    }
    // `Duel.GetCurrentChain() - 1` links, indexed 0..n-1, where index 0 is
    // `get_chain(0)` — the current link — and 1..n-1 are 1-based links.
    let n = f.core.current_chain.len();
    for link in 0..n {
        let triggering = get_chain(f, link as u8).map(|ch| ch.triggering_effect);
        if triggering == Some(e) {
            return is_relate_to_chain(f, c, link as u8);
        }
    }
    f.cards[c].has_effect_relation(e)
}

/// `Card.IsRelateToChain(c, count)` — the C++ export: `count` outside
/// `1..=len` means the last link; then `is_has_relation(const chain&)`,
/// the `(effect, chain id)` pair.
pub fn is_relate_to_chain(f: &Field, c: CardId, count: u8) -> bool {
    let n = f.core.current_chain.len();
    let idx = if usize::from(count) > n || count < 1 {
        n
    } else {
        usize::from(count)
    };
    if idx == 0 {
        return false;
    }
    let ch = &f.core.current_chain[idx - 1];
    f.cards[c].has_chain_relation(ch.triggering_effect, ch.chain_id)
}

// ------------------------------------------------------------------------
// Capability — the `IsAbleTo*` pair the pool reaches
// ------------------------------------------------------------------------

/// `Card.IsAbleToHand(c[, player])` — the library's version
/// (`proc_workaround.lua:59`): **false if the card is already in the
/// hand**, else the C++ `is_capable_send_to_hand`. The player defaults to
/// the reason player, as the export's does.
pub fn is_able_to_hand(f: &mut Field, c: CardId, player: Option<u8>) -> bool {
    if f.cards[c].current.location == location::HAND {
        return false;
    }
    let p = player.unwrap_or(f.core.reason_player);
    f.is_capable_send_to_hand(c, p)
}

/// `Card.IsAbleToRemove(c[, player, pos, reason])` — the library's
/// version (`:63`): false if already banished, else `is_removeable` with
/// the export's defaults (`POS_FACEUP`, `REASON_EFFECT`).
pub fn is_able_to_remove(
    f: &mut Field,
    c: CardId,
    player: Option<u8>,
    pos: Option<u8>,
    why: Option<u32>,
) -> bool {
    if f.cards[c].current.location == location::REMOVED {
        return false;
    }
    let p = player.unwrap_or(f.core.reason_player);
    f.is_removeable(
        c,
        p,
        pos.unwrap_or(position::FACEUP),
        why.unwrap_or(reason::EFFECT),
    )
}

// ------------------------------------------------------------------------
// Duel — the calls the first cards make (libduel.cpp)
// ------------------------------------------------------------------------

/// `Duel.IsPlayerCanDraw(p, count)` — the rule *and* the deck size.
pub fn is_player_can_draw(f: &mut Field, player: u8, count: usize) -> bool {
    if player > 1 {
        return false;
    }
    f.is_player_can_draw(player) && f.players[player as usize].main.len() >= count
}

/// `Duel.SetTargetPlayer` — on the current chain link.
pub fn set_target_player(f: &mut Field, player: u8) {
    if let Some(ch) = get_chain_mut(f, 0) {
        ch.target_player = player;
    }
}

/// `Duel.SetTargetParam` — on the current chain link.
pub fn set_target_param(f: &mut Field, param: i32) {
    if let Some(ch) = get_chain_mut(f, 0) {
        ch.target_param = param;
    }
}

/// `Duel.SetOperationInfo(ct, category, cards, count, player, param)`.
///
/// The reference refuses a `CATEGORY_SPECIAL_SUMMON` with `PLAYER_ALL`
/// whose group is not exactly two cards, because the core assumes that
/// shape when it counts summons. The assumption is kept as an assert.
pub fn set_operation_info(
    f: &mut Field,
    ct: u8,
    category: u64,
    cards: Option<Vec<CardId>>,
    count: u8,
    player: u8,
    param: i32,
) {
    if let Some(ref g) = cards {
        assert!(
            !(category == crate::event::category::SPECIAL_SUMMON
                && player == crate::event::PLAYER_ALL
                && g.len() != 2),
            "Duel.SetOperationInfo: CATEGORY_SPECIAL_SUMMON with PLAYER_ALL needs exactly two cards"
        );
    }
    if let Some(ch) = get_chain_mut(f, ct) {
        ch.opinfos.insert(
            category,
            OpTarget {
                cards,
                count,
                player,
                param,
            },
        );
    }
}

/// `Duel.GetChainInfo(ct, CHAININFO_TARGET_PLAYER, CHAININFO_TARGET_PARAM)`
/// — the pair Pot of Greed reads back. A missing link reads as
/// `(PLAYER_NONE, 0)`.
pub fn get_chain_target_player_param(f: &Field, ct: u8) -> (u8, i32) {
    get_chain(f, ct).map_or((PLAYER_NONE, 0), |ch| (ch.target_player, ch.target_param))
}

/// `Duel.Draw(p, count, reason)` — the core's `draw` with the reason
/// effect and reason player taken from `core`, as the export does.
pub fn draw(f: &mut Field, player: u8, count: u32, why: u32) {
    if player > 1 {
        return;
    }
    let (by, rp) = (f.core.reason_effect, f.core.reason_player);
    f.draw(player, count, why, by, rp);
}

/// A card filter, as the library's `f` argument: the reference passes a Lua
/// function and calls it per card through `check_matching`; here a card
/// module passes a Rust `fn`. `None` is the reference's `findex == 0` —
/// no filter, every card passes. The field is mutable because a Lua
/// filter may call anything — `Card.IsType` alone writes the card's
/// scratch type while it evaluates.
pub type Filter<'a> = &'a dyn Fn(&mut Field, CardId) -> bool;

/// `aux.TRUE` (`utility.lua:11`) — the filter that passes everything.
pub fn always(_f: &mut Field, _c: CardId) -> bool {
    true
}

/// `Duel.SetTargetCard(cards)` — name this activation's targets
/// (`libduel.cpp`).
///
/// Three things happen, and the third only sometimes. The cards go into
/// the last chain link's `target_cards`; each gains a **chain relation**
/// to the link, which is what `Card.IsRelateToEffect` later reads to tell
/// "still the card I targeted" from "something else in that seat"; and if
/// the effect carries `EFFECT_FLAG_CARD_TARGET`, a `MSG_BECOME_TARGET`
/// announces them.
///
/// A **continuous** effect takes neither the relation nor the message —
/// it replaces the set rather than adding to it. That asymmetry is the
/// reference's and is easy to miss: the two branches look alike.
pub fn set_target_card(f: &mut Field, cards: Vec<CardId>) {
    let Some(ch) = get_chain(f, 0) else {
        return;
    };
    let (effect, chain_id) = (ch.triggering_effect, ch.chain_id);
    let Some(e) = f.effects.get(effect) else {
        return;
    };
    let (continuous, card_target) = (
        e.is_type(effect_type::CONTINUOUS),
        e.is_flag(flag::CARD_TARGET),
    );
    let idx = match chain_index(f) {
        Some(i) => i,
        None => return,
    };
    if continuous {
        f.core.current_chain[idx].target_cards = cards;
        return;
    }
    for &c in &cards {
        if !f.core.current_chain[idx].target_cards.contains(&c) {
            f.core.current_chain[idx].target_cards.push(c);
        }
        f.cards[c].create_chain_relation(effect, chain_id);
    }
    if card_target {
        f.messages
            .push(crate::field::Message::BecomeTarget { cards });
    }
}

/// Which slot of `current_chain` `get_chain(0)` names, so a caller that
/// must *write* to the link can reach it.
fn chain_index(f: &Field) -> Option<usize> {
    let n = f.core.current_chain.len();
    (n > 0).then_some(n - 1)
}

/// `Duel.TossCoin(player, count)` — toss that many coins
/// (`libduel.cpp`). Two halves: this queues, [`tossed_coins`] reads.
pub fn toss_coin(f: &mut Field, player: u8, count: u8) -> bool {
    if player > 1 {
        return false;
    }
    let (by, rp) = (f.core.reason_effect, f.core.reason_player);
    f.toss_coin(by, rp, player, count);
    true
}

/// The results of the last [`toss_coin`], as `Duel.TossCoin` returns
/// them — one entry per coin, `1` for heads.
pub fn tossed_coins(f: &Field) -> Vec<bool> {
    f.core.coin_results.clone()
}

/// `Duel.CountHeads(...)` (`utility.lua`) — how many of them came up
/// heads.
pub fn count_heads(results: &[bool]) -> i32 {
    results.iter().filter(|&&heads| heads).count() as i32
}

/// `mt.material` and `mt.material_count` — what a Fusion Monster is
/// made of, as `Fusion.AddProcMix` records it.
///
/// The reference stores these on the card's **metatable**, which is
/// script-side storage a card reaches through its own type rather than
/// through the engine. The port has no metatables, so they live on the
/// card; nothing in this pool reads them.
pub fn set_fusion_materials(f: &mut Field, c: CardId, materials: &[u32]) {
    let mut unique: Vec<u32> = Vec::new();
    for &m in materials {
        if !unique.contains(&m) {
            unique.push(m);
        }
    }
    f.cards[c].fusion_materials = unique;
}

/// What [`set_fusion_materials`] recorded.
pub fn fusion_materials(f: &Field, c: CardId) -> &[u32] {
    &f.cards[c].fusion_materials
}

/// `Card.GetLevel()` — the level it has **now** (`libcard.cpp:147`),
/// which is not always the printed one.
pub fn get_level(f: &mut Field, c: CardId) -> u32 {
    f.get_level(c)
}

/// `Card.IsLevel(level)` — the same, compared.
pub fn is_level(f: &mut Field, c: CardId, level: u32) -> bool {
    f.get_level(c) == level
}

/// `Effect.IsActiveType(type)` — the type the effect is **acting as**
/// (`libeffect.cpp:392`), which for a granted effect is its owner's
/// rather than its handler's.
///
/// Not the same as asking the handler card its type: an effect a Flip
/// monster gave to something else still answers `TYPE_FLIP`.
pub fn is_active_type(f: &mut Field, e: EffectId, type_: u32) -> bool {
    f.get_active_type(e) & type_ != 0
}

/// `Effect.GetActiveType()` — the same value [`is_active_type`] masks,
/// handed over whole (`libeffect.cpp:392`).
///
/// **The difference is not cosmetic.** A script that writes
/// `re:GetActiveType()==TYPE_SPELL` is asking for a type that is
/// *exactly* Spell, and a Quick-Play answers `TYPE_SPELL|TYPE_QUICKPLAY`
/// — so the equality is false where the mask test would have been true.
/// Dark Balter is the pool's example, and the pool holds six Spells the
/// two readings disagree about.
pub fn get_active_type(f: &mut Field, e: EffectId) -> u32 {
    f.get_active_type(e)
}

/// `Effect.GetOwner()` (`libeffect.cpp:344`) — the card the effect was
/// created on, which for an effect registered on some *other* card is not
/// its handler. `Card.EquipByEffectLimit` reads it to ask "is the limit
/// on this monster mine?".
pub fn get_owner(f: &Field, e: EffectId) -> Option<CardId> {
    f.effects.get(e).and_then(|x| x.owner)
}

/// `Card.GetCardEffect([code])` (`libcard.cpp:1140` → `card.cpp:2910`) —
/// every effect **currently in force on** this card with that code, from
/// all four sources: its own single effects (available, and in range if
/// single-range), the field effects it registers (if they may touch it),
/// the equip effects of what is equipped to it, and the target effects
/// of what is aiming at it.
///
/// Read-only, which is what lets a `ValueFn` call it: every gate here is
/// a `&self` predicate. `code == 0` means every code, as in the reference.
pub fn get_card_effect(f: &Field, c: CardId, code_: u32) -> Vec<EffectId> {
    let mut out = Vec::new();
    let card = &f.cards[c];
    for &e in card.single_effect.iter().map(|(_, e)| e) {
        let Some(x) = f.effects.get(e) else { continue };
        if (code_ == 0 || x.code == code_)
            && f.is_available(e)
            && (!x.is_flag(flag::SINGLE_RANGE) || f.is_affect_by_effect(c, Some(e)))
        {
            out.push(e);
        }
    }
    for &e in card.field_effect.iter().map(|(_, e)| e) {
        let Some(x) = f.effects.get(e) else { continue };
        if (code_ == 0 || x.code == code_) && f.is_affect_by_effect(c, Some(e)) {
            out.push(e);
        }
    }
    for &equipper in &card.equiping_cards {
        for &e in f.cards[equipper].equip_effect.iter().map(|(_, e)| e) {
            let Some(x) = f.effects.get(e) else { continue };
            if (code_ == 0 || x.code == code_)
                && f.is_available(e)
                && f.is_affect_by_effect(c, Some(e))
            {
                out.push(e);
            }
        }
    }
    for &aimer in &card.effect_target_owner {
        for &e in f.cards[aimer].target_effect.equal_range(code_) {
            if f.is_available(e) && f.is_target(e, c) && f.is_affect_by_effect(c, Some(e)) {
                out.push(e);
            }
        }
    }
    out
}

/// `Effect.IsHasType(type)` — a mask test on the effect's **declared**
/// type (`libeffect.cpp:409`), not the active type above.
pub fn is_has_type(f: &Field, e: EffectId, type_: u16) -> bool {
    f.effects.get(e).is_some_and(|x| x.effect_type & type_ != 0)
}

/// `Effect.IsHasProperty(flag1[, flag2])` — one or both flag words
/// (`libeffect.cpp:397`).
///
/// **A zero matches.** The reference reads each word as "if you asked
/// about this one, it must be set", so asking about nothing is true —
/// which is why the two halves are `!flag || (flags & flag)` rather than
/// a plain mask.
pub fn is_has_property(f: &Field, e: EffectId, flag1: u32, flag2: u32) -> bool {
    let Some(x) = f.effects.get(e) else {
        return false;
    };
    (flag1 == 0 || x.flag[0] & flag1 != 0) && (flag2 == 0 || x.flag[1] & flag2 != 0)
}

/// `Duel.GetChainInfo(ct, CHAININFO_TARGET_CARDS)` — the cards that chain
/// link named as its targets.
pub fn get_chain_target_cards(f: &Field, ct: u8) -> Vec<CardId> {
    get_chain(f, ct).map_or_else(Vec::new, |ch| ch.target_cards.clone())
}

/// `aux.DoubleSnareValidity(c, range[, property])`
/// (`cards_specific_functions.lua:766`) — a marker effect carrying Double
/// Snare's own card number, which that card reads to know what it may
/// negate.
///
/// Double Snare is not in this pool, so nothing reads it. Registered
/// anyway, because a card that stopped carrying its marker would be
/// wrong in a pool that did have it, and the marker costs one effect.
pub fn double_snare_validity(f: &mut Field, c: CardId, range: u16, property: u32) {
    let e = create_effect(f, c);
    set_type(f, e, effect_type::SINGLE);
    set_property(
        f,
        e,
        flag::CANNOT_DISABLE | flag::SINGLE_RANGE | property,
        0,
    );
    set_range(f, e, range);
    set_code(f, e, CARD_DOUBLE_SNARE);
    register_effect(f, c, e, false);
}

/// Double Snare's card number, used as a marker code by
/// `aux.DoubleSnareValidity`. Pinned by `tools/check_constants.py`.
pub const CARD_DOUBLE_SNARE: u32 = 3_682_106;

/// `Card.IsFusionMonster()` — a monster with `TYPE_FUSION` on its
/// **printed** line (`libcard.cpp`).
pub fn is_fusion_monster(f: &Field, c: CardId) -> bool {
    let d = &f.cards[c].data;
    d.is_type(card_type::FUSION) && d.is_type(card_type::MONSTER)
}

/// `Duel.GetLocationCountFromEx(player[, uplayer, without, scard,
/// zone])` — seats for a card arriving **from the Extra Deck**
/// (`libduel.cpp:1739`).
///
/// A different question from [`get_location_count`] once the Extra
/// Monster Zone exists; under this project's configuration it is the
/// ordinary count with the location and reason forced. `without` is the
/// same card-or-group the Monster Zone count takes, and for the same
/// reason: the card about to be tributed is the seat being freed.
pub fn get_location_count_from_ex(
    f: &mut Field,
    player: u8,
    uplayer: Option<u8>,
    without: Except,
    scard: Option<CardId>,
) -> i32 {
    if player > 1 {
        return 0;
    }
    let uplayer = uplayer.unwrap_or(f.core.reason_player);
    let one;
    let without = match without {
        Except::None => None,
        Except::Card(c) => {
            one = [c];
            Some(&one[..])
        }
        Except::Group(g) => Some(g),
    };
    match without {
        Some(w) => f.with_rows_without(w, |f| {
            f.get_useable_count_fromex(scard, player, uplayer, 0xff).0
        }),
        None => f.get_useable_count_fromex(scard, player, uplayer, 0xff).0,
    }
}

/// `Effect.GetChainData(e).<key> = v` for the one key this pool uses.
///
/// The reference's is a Lua table the **library** keeps, keyed by chain
/// id and effect (`chain.lua:75`) — not engine state, which is why it
/// lives beside the other library pieces rather than on the chain link.
pub fn set_chain_data(f: &mut Field, e: EffectId, value: i64) {
    let Some(ch) = get_chain(f, 0) else {
        return;
    };
    let id = u32::from(ch.chain_id);
    f.core.chain_data.insert((id, e), value);
}

/// What [`set_chain_data`] stored for this chain link, if anything.
pub fn get_chain_data(f: &Field, e: EffectId) -> Option<i64> {
    let ch = get_chain(f, 0)?;
    f.core.chain_data.get(&(u32::from(ch.chain_id), e)).copied()
}

/// `ACTIVITY_*` (`constant.lua:967`) — what `Duel.GetActivityCount`
/// counts. Transcribed whole rather than the three this pool asks for.
pub mod activity {
    pub const SUMMON: u8 = 1;
    pub const NORMALSUMMON: u8 = 2;
    pub const SPSUMMON: u8 = 3;
    pub const FLIPSUMMON: u8 = 4;
    pub const ATTACK: u8 = 5;
    /// Not available in a custom counter.
    pub const BATTLE_PHASE: u8 = 6;
    /// Only available in a custom counter, and not a `*_state_count`.
    pub const CHAIN: u8 = 7;
}

/// `Duel.GetActivityCount(player, activity)` — how many of that thing the
/// player has done this turn (`libduel.cpp:3948`).
///
/// **It reads the `*_state_count` counters, not the `*_counter` maps.**
/// The maps are the per-effect "how many times has *this* been used"
/// bookkeeping; these are the plain per-player tallies a card means by
/// "if you have not summoned this turn". Asking the wrong one would
/// answer a different question entirely.
///
/// `ACTIVITY_CHAIN` has no `*_state_count` and the reference rejects it
/// here, so it is refused rather than answered with a zero.
pub fn get_activity_count(f: &Field, player: u8, what: u8) -> i32 {
    if player > 1 {
        return 0;
    }
    let p = player as usize;
    let n = match what {
        activity::SUMMON => f.core.summon_state_count[p],
        activity::NORMALSUMMON => f.core.normalsummon_state_count[p],
        activity::SPSUMMON => f.core.spsummon_state_count[p],
        activity::FLIPSUMMON => f.core.flipsummon_state_count[p],
        activity::ATTACK => u32::from(f.core.attack_state_count[p]),
        activity::BATTLE_PHASE => u32::from(f.core.battle_phase_count[p]),
        other => panic!("Duel.GetActivityCount: no such activity {other}"),
    };
    n as i32
}

/// `Duel.CreateToken(player, code)` — a card that was never in a deck
/// (`libduel.cpp:389`).
///
/// It is **not** put anywhere: the reference leaves `current.location` at
/// zero and the caller summons it. Its printed line comes from the card
/// database, the same reader every other card's does, so a token's stats
/// are data rather than something the script states.
pub fn create_token(f: &mut Field, player: u8, code_: u32) -> Option<CardId> {
    if player >= crate::event::PLAYER_NONE {
        return None;
    }
    let data = crate::cards::card_data(code_)?;
    let mut c = crate::card::Card::with_data(data, player);
    c.current.controller = player;
    c.current.location = 0;
    Some(f.new_card(c))
}

/// `Duel.IsPlayerCanSpecialSummonMonster(player, code, setcode, type,
/// atk, def, level, race, attribute[, pos, toplayer, sumtype])` — could a
/// monster **of this shape** be Special Summoned (`libduel.cpp:3728`)?
///
/// Asked before a token is made, because there is no card yet to ask
/// about. The described fields override the database's, which is how a
/// token with no database entry is still a well-formed question.
///
/// The export's trailing defaults, which this pool takes: `POS_FACEUP`,
/// the same player, and no summon type.
#[allow(clippy::too_many_arguments)]
pub fn is_player_can_special_summon_monster(
    f: &mut Field,
    player: u8,
    code_: u32,
    type_: u32,
    attack: i32,
    defense: i32,
    level: u32,
    race: u64,
    attribute: u32,
) -> bool {
    if player > 1 {
        return false;
    }
    let mut data = crate::cards::card_data(code_).unwrap_or_default();
    data.code = code_;
    data.alias = 0;
    data.type_ = type_;
    data.attack = attack;
    data.defense = defense;
    data.level = level;
    data.race = race;
    data.attribute = attribute;
    f.is_player_can_spsummon_monster(player, player, position::FACEUP, 0, data)
}

/// `Duel.GetReleaseGroup(player[, use_hand, use_oppo, reason])` — every
/// card that could be released right now (`libduel.cpp:2421`).
///
/// **The reference passes the same container three times**, so the
/// ordinary pool, the `EXTRA_RELEASE` pool and the one-of pool arrive as
/// one group. The distinction is kept inside
/// [`Field::get_release_list`] because the summon path needs it; here it
/// is flattened, exactly as the export does.
///
/// The export's defaults, which this pool takes: no hand, not the
/// opponent's side, `REASON_COST`.
pub fn get_release_group(
    f: &mut Field,
    player: u8,
    use_hand: bool,
    use_oppo: bool,
    reason: Option<u32>,
) -> Vec<CardId> {
    if player > 1 {
        return Vec::new();
    }
    let (_, lists) = f.get_release_list(
        player,
        None,
        use_hand,
        use_oppo,
        None,
        None,
        reason.unwrap_or(reason::COST),
    );
    let mut out: Vec<CardId> = lists
        .release
        .iter()
        .chain(lists.extra.iter())
        .chain(lists.extra_one_of.iter())
        .copied()
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// `Duel.Release(cards, reason[, reason_player])` — send them to their
/// owners' graveyards as a release (`libduel.cpp:646`).
pub fn release(f: &mut Field, cards: Vec<CardId>, why: u32, reason_player: Option<u8>) {
    let (effect, player) = (f.core.reason_effect, f.core.reason_player);
    f.release(cards, effect, why, reason_player.unwrap_or(player));
}

/// `Duel.SelectOption(player, ...)` — ask a player to pick one of a list
/// of descriptions (`libduel.cpp:3111`). Two halves; see
/// [`selected_option`].
pub fn select_option(f: &mut Field, player: u8, options: &[u64]) -> bool {
    if player > 1 {
        return false;
    }
    f.core.select_options = options.to_vec();
    f.emplace(crate::processor::Kind::SelectOption { player });
    true
}

/// The other half of [`select_option`]: the index chosen.
///
/// **It writes a message before answering.** `sel_hint` defaults to true,
/// and the reference announces the chosen description as `HINT_OPSELECTED`
/// to the chooser — not flipped, unlike a hint sent through `Duel.Hint`.
pub fn selected_option(f: &mut Field, player: u8, sel_hint: bool) -> i32 {
    let answer = f.core.returns.get();
    if sel_hint {
        if let Some(&value) = f.core.select_options.get(answer.max(0) as usize) {
            // Written **directly**, to the chooser: the flip to the other
            // player is `Duel.Hint`'s alone (`libduel.cpp` 3061), and the
            // reference's `SelectOption` continuation does not go through
            // it (3127, 3448, 3487).
            f.messages.push(crate::field::Message::Hint {
                kind: crate::host_question::hint::OPSELECTED,
                player,
                value,
            });
        }
    }
    answer
}

/// `Duel.SelectEffect(player, {enabled, description}, ...)`
/// (`utility.lua:2780`) — offer only the branches that are available, and
/// answer with the **original** option number.
///
/// Returns the mapping from the offered list back to those numbers, or
/// `None` when nothing is available and the reference returns `nil`. The
/// caller holds the mapping across the suspension and applies it to
/// [`selected_option`]'s answer, because the engine only ever sees the
/// shortened list.
pub fn select_effect(f: &mut Field, player: u8, options: &[(bool, u64)]) -> Option<Vec<usize>> {
    let mut descriptions = Vec::new();
    let mut sel = Vec::new();
    for (i, &(enabled, description)) in options.iter().enumerate() {
        if enabled {
            descriptions.push(description);
            sel.push(i + 1);
        }
    }
    if descriptions.is_empty() {
        return None;
    }
    select_option(f, player, &descriptions);
    Some(sel)
}

/// `Card.IsCanChangePosition()` — `is_capable_change_position_by_effect`
/// with the reason player (`libcard.cpp:1842`).
///
/// **The effect question, not the rule one.** See
/// [`Field::is_capable_change_position_by_effect`]: the rule version
/// refuses a monster summoned this turn, which would make Enemy
/// Controller refuse exactly the monster it is played against.
pub fn is_can_change_position(f: &Field, c: CardId) -> bool {
    let p = f.core.reason_player;
    f.is_capable_change_position_by_effect(c, p)
}

/// `Duel.GetFirstTarget()` — the first of this activation's named targets
/// (`libduel.cpp`), or nothing if it named none.
pub fn get_first_target(f: &Field) -> Option<CardId> {
    get_chain(f, 0).and_then(|ch| ch.target_cards.first().copied())
}

/// `Effect.Clone()` — `effect::clone` (`effect.cpp`): a copy of
/// everything **except the handler**, which is cleared, so the copy has
/// to be registered before it belongs to a card.
pub fn clone_effect(f: &mut Field, e: EffectId) -> EffectId {
    let Some(src) = f.effects.get(e).cloned() else {
        return e;
    };
    let mut copy = src;
    copy.handler = None;
    f.new_effect(copy)
}

/// `Card.IsFaceup`.
pub fn is_faceup(f: &Field, c: CardId) -> bool {
    f.cards[c].current.is_faceup()
}

/// `Card.GetAttack` — the *current* attack, with every modifier applied.
pub fn get_attack(f: &mut Field, c: CardId) -> i32 {
    f.get_attack(c)
}

/// `Card.GetDefense()` — the current defence, the mirror of
/// [`get_attack`] (`libcard.cpp`).
pub fn get_defense(f: &mut Field, c: CardId) -> i32 {
    f.get_defense(c)
}

/// `Card.IsOnField` — on the field **and** settled there: a card still
/// being summoned, or whose summon or activation was negated, is not
/// (`libcard.cpp`).
pub fn is_on_field(f: &Field, c: CardId) -> bool {
    use crate::card::status;
    let cd = &f.cards[c];
    cd.current.location & location::ONFIELD != 0
        && !cd.get_status(
            status::SUMMONING
                | status::SUMMON_DISABLED
                | status::ACTIVATE_DISABLED
                | status::SPSUMMON_STEP,
        )
}

/// `Card.IsCanBeEffectTarget(e)` — `card::is_capable_be_effect_target`,
/// asked for the effect's own player.
pub fn is_can_be_effect_target(f: &Field, c: CardId, e: EffectId) -> bool {
    f.is_capable_be_effect_target(c, e, f.core.reason_player)
}

/// `Card.CanAttack` — `card::is_capable_attack` (`libcard.cpp`).
pub fn can_attack(f: &mut Field, c: CardId) -> bool {
    f.is_capable_attack(c)
}

/// `Card.IsStatus(mask)` — the **any-bit** reader, `card::get_status`,
/// not the all-bit `is_status` the core uses internally. The two names
/// are transposed across the seam, which is the defect recorded in
/// `docs/script-library.md` §8.
pub fn is_status(f: &Field, c: CardId, mask: u32) -> bool {
    f.cards[c].get_status(mask)
}

/// `Duel.SelectYesNo(player, description)` — ask a player a bare
/// yes/no (`libduel.cpp`), refusing an out-of-range player outright.
///
/// Two halves as every asking export is: this queues the question, and
/// [`resumed_value`] is the answer. The reference reads it as a
/// *boolean* — `lua_pushboolean` of `returns[0]` — so a caller compares
/// against zero rather than expecting a count.
pub fn select_yes_no(f: &mut Field, player: u8, description: u64) -> bool {
    if player > 1 {
        return false;
    }
    f.emplace(crate::processor::Kind::SelectYesNo {
        player,
        description,
    });
    true
}

/// `Card.IsSSetable(ignore_field, to_player)` — may this card be Set in
/// a Spell/Trap Zone? (`libcard.cpp` → `card::is_setable_szone`.)
///
/// `to_player` defaults to **`core.reason_player`**, not to the card's
/// controller: whether a card can be Set depends on who is setting it,
/// the same asymmetry `Card.IsCanTurnSet` has.
pub fn is_ssetable(f: &mut Field, c: CardId, ignore_field: bool, to_player: Option<u8>) -> bool {
    let to = to_player.unwrap_or(f.core.reason_player);
    f.is_setable_szone(c, to, ignore_field)
}

/// `Duel.SSet(player, cards, to_player, confirm)` — Set Spell/Traps
/// from wherever they are (`libduel.cpp`).
///
/// An **empty group is not queued**: the reference returns the standing
/// `returns[0]` untouched rather than emplacing a process, so a caller
/// that suspends on an empty set would read a stale value. Nothing in
/// the pool does, because every caller guards first, but the shape is
/// the reference's and is kept.
///
/// `to_player` falls back to `player` when out of range, which the
/// reference does explicitly rather than refusing.
pub fn sset(
    f: &mut Field,
    player: u8,
    cards: Vec<CardId>,
    to_player: Option<u8>,
    confirm: bool,
) -> bool {
    if player > 1 {
        return false;
    }
    if cards.is_empty() {
        return false;
    }
    let to = match to_player {
        Some(p) if p <= 1 => p,
        _ => player,
    };
    let reason_effect = f.core.reason_effect;
    f.emplace(crate::processor::Kind::SpellSetGroup {
        setplayer: player,
        toplayer: to,
        targets: cards,
        confirm,
        reason_effect,
        set_cards: Vec::new(),
    });
    true
}

/// `Card.GetControler()` — whose side the card is on **now**
/// (`libcard.cpp`), which is not the same as who owns it.
pub fn get_controler(f: &Field, c: CardId) -> u8 {
    f.cards[c].current.controller
}

/// `Card.IsControler(player)` — whose side the card is on *now*
/// (`libcard.cpp`), which is not always whose card it is.
pub fn is_controler(f: &Field, c: CardId, player: u8) -> bool {
    f.cards[c].current.controller == player
}

/// A per-card value, as the library's `Card.GetAttack` and friends are
/// when passed to a group operation.
pub type Value<'a> = &'a dyn Fn(&mut Field, CardId) -> i64;

/// `Group.GetMaxGroup(f)` — the members whose value is the largest, all
/// of them (`libgroup.cpp`).
///
/// A **group**, not a card, because ties are kept: the reference clears
/// its accumulator only when it finds something strictly greater, and
/// adds to it on equality. A card that took the first maximum would be
/// right whenever the maximum is unique and silently wrong the rest of
/// the time — which is the case a card like Widespread Ruin exists to
/// ask a player about.
///
/// An empty group gives an empty one back; the reference pushes nothing
/// at all there, and every caller guards against it first.
pub fn get_max_group(f: &mut Field, group: &[CardId], value: Value) -> Vec<CardId> {
    let mut best: Option<i64> = None;
    let mut out: Vec<CardId> = Vec::new();
    for &c in group {
        let v = value(f, c);
        match best {
            None => {
                best = Some(v);
                out.push(c);
            }
            Some(m) if v == m => out.push(c),
            Some(m) if v > m => {
                best = Some(v);
                out.clear();
                out.push(c);
            }
            _ => {}
        }
    }
    out.sort_unstable();
    out
}

/// `Card.GetAttack` as a value function, for the group operations.
pub fn attack_value(f: &mut Field, c: CardId) -> i64 {
    i64::from(get_attack(f, c))
}

/// `Group.Select(player, min, max, exception)` — ask a player to choose
/// from a group the card is already holding (`libgroup.cpp`), rather
/// than from a scan of the field.
///
/// Two halves as `SelectTarget` is, and for the same reason: this queues
/// the question, [`group_selected`] is what runs when the answer is in.
/// Unlike `SelectTarget` it folds nothing into the chain — a plain
/// choice, with no targeting attached.
pub fn group_select(
    f: &mut Field,
    group: &[CardId],
    player: u8,
    min: u8,
    max: u8,
    exception: Except,
) -> bool {
    if player > 1 {
        return false;
    }
    let (exc, exg) = exception.split();
    let mut cards: Vec<CardId> = group
        .iter()
        .copied()
        .filter(|c| Some(*c) != exc && !exg.is_some_and(|g| g.contains(c)))
        .collect();
    cards.sort_unstable();
    f.core.select_cards = cards;
    f.emplace(crate::processor::Kind::SelectCard {
        player,
        cancelable: false,
        min,
        max,
    });
    true
}

/// The other half of [`group_select`]: what was chosen.
///
/// A cancelled selection gives an **empty group**, not nothing — the
/// reference pushes a fresh empty group unless the call was cancelable,
/// and these are not.
pub fn group_selected(f: &Field) -> Vec<CardId> {
    if f.core.return_cards.canceled {
        return Vec::new();
    }
    f.core.return_cards.list.clone()
}

/// `Group.SelectUnselect(unselect, player, finishable, cancelable, min,
/// max)` — ask for **one** card, from this group or from the ones
/// already chosen (`libgroup.cpp:262`).
///
/// The building block `aux.SelectUnselectGroup` loops around. Two halves
/// as the other selections are: this queues the question,
/// [`selected_one`] reads the answer.
///
/// ## It refuses when the two lists overlap
///
/// The reference walks both containers in `cardid` order and returns
/// **no value at all** — `nil` to the script — if a card is in both. A
/// card cannot be simultaneously on offer and already taken, and the
/// library's callers keep them disjoint by passing the chosen set as the
/// filter's exception group. Transcribed anyway: it is the export's
/// contract, and `false` here is what makes the loop break rather than
/// ask an impossible question.
///
/// Returns whether it queued anything, so a refusal reads as a break.
#[allow(clippy::too_many_arguments)]
pub fn select_unselect(
    f: &mut Field,
    select: &[CardId],
    unselect: &[CardId],
    player: u8,
    finishable: bool,
    cancelable: bool,
    min: u8,
    max: u8,
) -> bool {
    if player > 1 {
        return false;
    }
    if select.iter().any(|c| unselect.contains(c)) {
        return false;
    }
    // `if(min > max) min = max;` — the reference clamps, it does not
    // refuse.
    let min = min.min(max);
    f.core.select_cards = select.to_vec();
    f.core.unselect_cards = unselect.to_vec();
    f.emplace(crate::processor::Kind::SelectUnselectCard {
        player,
        cancelable,
        min,
        max,
        finishable,
    });
    true
}

/// The other half of [`select_unselect`]: the one card chosen, or `None`
/// when the player finished or cancelled.
///
/// Distinct from [`group_selected`], which cannot tell those apart — the
/// reference pushes `nil` here rather than an empty group, and the
/// difference is what ends the loop.
pub fn selected_one(f: &Field) -> Option<CardId> {
    if f.core.return_cards.canceled {
        return None;
    }
    f.core.return_cards.list.first().copied()
}

/// `Group.CreateGroup()` followed by the cards — a handle a script can
/// hand to `Effect.SetLabelObject` and read back later.
///
/// `Field::groups` is append-only: there is no `DeleteGroup` here
/// because there is nothing to delete. See [`crate::effect::LabelObject`].
pub fn new_group(f: &mut Field, cards: impl IntoIterator<Item = CardId>) -> crate::field::GroupId {
    f.new_group(cards)
}

/// What a group handle holds, in `cardid` order.
pub fn group_cards(f: &Field, g: crate::field::GroupId) -> Vec<CardId> {
    f.group(g).iter().copied().collect()
}

/// `Duel.GetFieldGroup(player, s, o)` — every card in those zones, with
/// no filter (`libduel.cpp`). See `Field::filter_field_card` for the ways
/// this scan differs from the matching one; they are not the same walk.
pub fn get_field_group(f: &Field, player: u8, s: u32, o: u32) -> Vec<CardId> {
    let mut g = Vec::new();
    f.filter_field_card(player, s, o, Some(&mut g));
    g
}

/// `Duel.SendtoGrave(cards, reason)` — the export's defaults: no named
/// player, the reason player from `core`, and face-up.
///
/// The position is passed because the export passes it, and it is
/// **discarded**: `send_to` forces `POS_FACEUP` for every destination
/// except `LOCATION_REMOVED` (`operations.cpp:327`, mirrored in
/// `send_to.rs`). So face-up here is the reference's literal argument
/// rather than a choice, and no test can tell it from face-down — a
/// mutation of it is an equivalent, not a gap.
pub fn send_to_grave(f: &mut Field, cards: Vec<CardId>, why: u32) {
    let (by, rp) = (f.core.reason_effect, f.core.reason_player);
    f.send_to(
        cards,
        by,
        why,
        rp,
        PLAYER_NONE,
        u16::from(location::GRAVE),
        0,
        position::FACEUP,
        false,
    );
}

/// `Card.IsCanTurnSet` — `card::is_capable_turn_set`, asked for the
/// **reason player** (`libcard.cpp`), not for the card's controller.
pub fn is_can_turn_set(f: &mut Field, c: CardId) -> bool {
    let by = f.core.reason_player;
    f.is_capable_turn_set(c, by)
}

/// `Card.IsMonster` — `aux.FilterBoolFunction(Card.IsType, TYPE_MONSTER)`
/// (`utility.lua`), so the **effective** type, as `IsSpellTrap` is.
pub fn is_monster(f: &mut Field, c: CardId) -> bool {
    is_type(f, c, card_type::MONSTER)
}

/// `Duel.ChangePosition(cards, au[, ad, du, dd, no_flip])` — turn cards
/// to a position, chosen by where each already is (`libduel.cpp`).
///
/// The export defaults the three later positions to the first, which is
/// what every single-argument caller means: "this position, whatever you
/// are now". The reason effect and player come from `core`, and `enable`
/// is the reference's literal `TRUE`.
pub fn change_position(f: &mut Field, cards: Vec<CardId>, pos: u8) {
    let (by, rp) = (f.core.reason_effect, f.core.reason_player);
    f.change_position(cards, by, rp, pos, pos, pos, pos, 0, true);
}

/// `Duel.ChangePosition(cards, au, ad, du, dd[, no_flip])` — the full
/// form, where each **current** position names the one to move to.
///
/// A zero means "leave it alone", which is how Enemy Controller turns a
/// face-up monster over without touching a face-down one: `au` is
/// face-up defence, `du` is face-up attack, and both face-down slots are
/// zero.
pub fn change_position_each(f: &mut Field, cards: Vec<CardId>, au: u8, ad: u8, du: u8, dd: u8) {
    let (by, rp) = (f.core.reason_effect, f.core.reason_player);
    f.change_position(cards, by, rp, au, ad, du, dd, 0, true);
}

/// `Duel.GetControl(cards, player[, reset_phase, reset_count, zone,
/// chose_player])` — hand them over (`libduel.cpp:1247`).
///
/// **`reset_phase` is masked to ten bits** by the export, and a reset
/// count of zero means the control change is permanent. Enemy Controller
/// passes the End Phase and a count of one, which is what "until the End
/// Phase" is made of.
pub fn get_control(
    f: &mut Field,
    cards: Vec<CardId>,
    player: u8,
    reset_phase: u16,
    reset_count: u8,
) -> bool {
    if player > 1 {
        return false;
    }
    let (by, chose) = (f.core.reason_effect, f.core.reason_player);
    f.get_control(
        cards,
        by,
        chose,
        player,
        reset_phase & 0x3ff,
        reset_count,
        0xff,
    );
    true
}

/// `Card.IsCode(code)` — whether the card *is* that card, by its
/// **effective** code (`libcard.cpp`).
///
/// Two codes are compared, not one: `get_code` and `get_another_code`,
/// because a card may be treated as a second name at once. The export
/// takes a list and matches any of them; every pool caller passes a
/// single code, so this does too.
pub fn is_code(f: &mut Field, c: CardId, code_: u32) -> bool {
    let first = f.get_code(c);
    let second = f.get_another_code(c);
    first == code_ || (second != 0 && second == code_)
}

/// `Card.IsCode(code)` for a **read-only** caller — a value function,
/// which the seam hands `&Field` rather than `&mut Field`.
///
/// [`is_code`] needs the mutable field only for `get_code`'s re-entrancy
/// guard (`temp.code`), not for any of the data: `filter_effect`,
/// `sort_by_effect_id` and `effect_value_for_card_pub` are all `&self`.
/// So the effective code is reachable immutably, and this resolves
/// `EFFECT_CHANGE_CODE` exactly as `get_code` does.
///
/// What it does **not** carry is that guard. A `CHANGE_CODE` value
/// function that asked for the same card's code while computing it would
/// recurse here where the mutable path would answer the printed code and
/// stop. Nothing in the reference's library does that, and nothing in
/// this pool could.
///
/// It also stops at the first name: [`is_code`] also consults
/// `get_another_code`, which needs the mutable path. No card in the pool
/// has a second name.
pub fn is_code_readonly(f: &Field, c: CardId, code_: u32) -> bool {
    if let Some(&assumed) = f.cards[c].assume.get(&crate::card::assume::CODE) {
        return assumed as u32 == code_;
    }
    let mut changers = f.filter_effect(c, code::CHANGE_CODE);
    f.sort_by_effect_id(&mut changers);
    let effective = match changers.last() {
        Some(&last) => f.effect_value_for_card_pub(last, c) as u32,
        None => f.cards[c].data.code,
    };
    effective == code_
}

/// `Card.GetCode()` — the effective code (`libcard.cpp`).
pub fn get_code(f: &mut Field, c: CardId) -> u32 {
    f.get_code(c)
}

/// `Duel.GetMatchingGroupCount(f, player, s, o, exception[, ...])` — how
/// many match, without building a group the caller keeps
/// (`libduel.cpp`). Note it is the plain scan, **not** the targeting one.
pub fn get_matching_group_count(
    f: &mut Field,
    filter: Option<Filter>,
    player: u8,
    s: u32,
    o: u32,
    exception: Except,
) -> usize {
    get_matching_group(f, filter, player, s, o, exception).len()
}

/// `Card.IsLocation(loc)` — `card::current.is_location`, which folds
/// the specific field zones in: a monster in the Monster Zone answers to
/// `LOCATION_MZONE` and to the zone masks that name part of it.
pub fn is_location(f: &Field, c: CardId, loc: u16) -> bool {
    f.cards[c].current.is_location(loc)
}

/// `Duel.Hint(kind, player, description)` — a message to a player, most
/// often "what am I choosing and why" ahead of a selection
/// (`libduel.cpp`).
///
/// `HINT_OPSELECTED` is addressed to the **other** player, which is the
/// one asymmetry in an otherwise plain message.
pub fn hint(f: &mut Field, kind: u8, player: u8, description: u64) {
    if player > 1 {
        return;
    }
    let player = if kind == crate::host_question::hint::OPSELECTED {
        1 - player
    } else {
        player
    };
    f.messages.push(crate::field::Message::Hint {
        kind,
        player,
        value: description,
    });
}

/// `Card.IsAbleToRemoveAsCost(c[, pos])` — `is_removeable_as_cost` with
/// the reason player and `POS_FACEUP` as the export's defaults
/// (`libcard.cpp:1514`).
pub fn is_able_to_remove_as_cost(f: &mut Field, c: CardId, pos: Option<u8>) -> bool {
    let p = f.core.reason_player;
    f.is_removeable_as_cost(c, p, pos.unwrap_or(position::FACEUP))
}

/// `aux.SpElimFilter(c, mustbefaceup, includemzone)`
/// (`cards_specific_functions.lua:45`) — the material filter every
/// "banish from your GY" summon procedure shares.
///
/// The card number is not a typo: **69832741 is Spirit Elimination**, and
/// `Duel.IsPlayerAffectedByEffect` is being asked "is that specific card
/// applying to this player", not about an `EFFECT_*`. While it is,
/// monsters in the graveyard are unavailable and the field is used
/// instead — which is the whole reason this filter exists rather than a
/// plain location test.
///
/// The reference notes it should only ever be asked about
/// `LOCATION_MZONE | LOCATION_GRAVE`.
pub fn sp_elim_filter(f: &mut Field, c: CardId, mustbefaceup: bool, includemzone: bool) -> bool {
    let in_mzone = f.cards[c].current.location == location::MZONE;
    let in_grave = f.cards[c].current.location == location::GRAVE;
    if !is_monster(f, c) {
        return includemzone || in_grave;
    }
    if mustbefaceup && in_mzone && !is_faceup(f, c) {
        return false;
    }
    let controller = f.cards[c].current.controller;
    if includemzone {
        return in_mzone || !is_player_affected_by_effect(f, controller, SPIRIT_ELIMINATION);
    }
    if is_player_affected_by_effect(f, controller, SPIRIT_ELIMINATION) {
        in_mzone
    } else {
        in_grave
    }
}

/// The card number `aux.SpElimFilter` asks after — Spirit Elimination.
const SPIRIT_ELIMINATION: u32 = 69832741;

/// `Duel.IsExistingTarget(f, player, s, o, count, exception[, ...])` —
/// `IsExistingMatchingCard` with the **targeting** test switched on
/// (`libduel.cpp`), so a card that could match but may not be targeted
/// does not count.
pub fn is_existing_target(
    f: &mut Field,
    filter: Option<Filter>,
    player: u8,
    s: u32,
    o: u32,
    count: i32,
    exception: Except,
) -> bool {
    let (exc, exg) = exception.split();
    f.filter_matching_card(filter, player, s, o, None, exc, exg, None, count, true)
}

/// `Duel.SelectTarget(chooser, f, player, s, o, min, max, exception)` —
/// ask a player to name this activation's targets (`libduel.cpp`).
///
/// **Two halves.** This queues the selection; [`selected_targets`] is the
/// `yieldk` block that runs when the answer is in. A card calls this,
/// suspends, and then calls that — the split is the reference's, not an
/// invention.
///
/// Returns whether it queued anything: with no chain link there is
/// nothing to target *for*, and the export returns early.
#[allow(clippy::too_many_arguments)]
pub fn select_target(
    f: &mut Field,
    chooser: u8,
    filter: Option<Filter>,
    player: u8,
    s: u32,
    o: u32,
    min: u8,
    max: u8,
    exception: Except,
) -> bool {
    if chooser > 1 || f.core.current_chain.is_empty() {
        return false;
    }
    let (exc, exg) = exception.split();
    let mut group = Vec::new();
    f.filter_matching_card(
        filter,
        player,
        s,
        o,
        Some(&mut group),
        exc,
        exg,
        None,
        0,
        true,
    );
    group.sort_unstable();
    f.core.select_cards = group;
    f.emplace(crate::processor::Kind::SelectCard {
        player: chooser,
        cancelable: false,
        min,
        max,
    });
    true
}

/// The other half of [`select_target`]: what the player chose, folded
/// into the chain link.
///
/// Cancelling gives `None`, as the export's `lua_pushnil`. Otherwise the
/// chosen cards join the link's `target_cards`, each gains a **chain
/// relation**, and `MSG_BECOME_TARGET` announces them if the effect
/// carries the flag — the same three steps `SetTargetCard` takes, and the
/// same exemption for a continuous effect, which adds no relation and
/// says nothing.
pub fn selected_targets(f: &mut Field) -> Option<Vec<CardId>> {
    if f.core.return_cards.canceled {
        return None;
    }
    let chosen: Vec<CardId> = f.core.return_cards.list.clone();
    let ch = get_chain(f, 0)?;
    let (effect, chain_id) = (ch.triggering_effect, ch.chain_id);
    let Some(e) = f.effects.get(effect) else {
        return Some(chosen);
    };
    let (continuous, card_target) = (
        e.is_type(effect_type::CONTINUOUS),
        e.is_flag(flag::CARD_TARGET),
    );
    let idx = f.core.current_chain.len() - 1;
    for &c in &chosen {
        if !f.core.current_chain[idx].target_cards.contains(&c) {
            f.core.current_chain[idx].target_cards.push(c);
        }
    }
    if continuous {
        // A continuous effect takes the whole accumulated set and neither
        // relates nor announces.
        return Some(f.core.current_chain[idx].target_cards.clone());
    }
    // One `MSG_BECOME_TARGET` **per card**, as the reference's continuation
    // writes it (`libduel.cpp` 2712) — and so none at all when nothing was
    // chosen, where `Duel.SetTargetCard` would write an empty one.
    for &c in &chosen {
        f.cards[c].create_chain_relation(effect, chain_id);
        if card_target {
            f.messages
                .push(crate::field::Message::BecomeTarget { cards: vec![c] });
        }
    }
    Some(chosen)
}

/// `Duel.ShuffleHand(player)` — `shuffle(player, LOCATION_HAND)`
/// (`libduel.cpp`). Immediate, so nothing needs waiting on.
pub fn shuffle_hand(f: &mut Field, player: u8) {
    if player > 1 {
        return;
    }
    f.shuffle(player, location::HAND);
}

/// `Duel.BreakEffect()` — end the current chain's timing window
/// (`libduel.cpp`): `break_effect()`, then raise `EVENT_BREAK_EFFECT` and
/// process it at once.
///
/// The export `yield()`s afterwards, but everything it does is synchronous
/// and it queues nothing, so there is nothing for a card to wait on.
pub fn break_effect(f: &mut Field) {
    f.break_effect(true);
    f.raise_event(
        None,
        code::BREAK_EFFECT,
        None,
        0,
        PLAYER_NONE,
        PLAYER_NONE,
        0,
    );
    f.process_instant_event();
}

/// `Duel.DiscardHand(player, f, min, max, reason[, exception])` — make a
/// player send cards from their hand to the graveyard (`libduel.cpp`).
///
/// The export does the filtering itself: it collects the matching hand
/// cards into `core.select_cards` and, **if none match, returns 0 without
/// queuing anything or suspending**. Returns whether it queued work, so a
/// card that must wait knows whether there is anything to wait for.
pub fn discard_hand(
    f: &mut Field,
    player: u8,
    filter: Option<Filter>,
    min: u8,
    max: u8,
    why: u32,
    exception: Except,
) -> bool {
    let (exc, exg) = exception.split();
    let mut hand = Vec::new();
    f.filter_matching_card(
        filter,
        player,
        u32::from(location::HAND),
        0,
        Some(&mut hand),
        exc,
        exg,
        None,
        0,
        false,
    );
    hand.sort_unstable();
    if hand.is_empty() {
        return false;
    }
    f.core.select_cards = hand;
    f.discard_hand(player, min, max, why);
    true
}

/// Finish a **target** with a yes or a no.
///
/// A target's answer reaches the engine the same way an operation's
/// does — through `returns[0]` — so "true" is a finished call carrying
/// one, exactly as a Lua `return true` would.
pub fn yes(answer: bool) -> Yield {
    Yield::Done(i32::from(answer))
}

/// Finish an operation, as a Lua function returning.
pub fn done() -> Yield {
    Yield::Done(0)
}

/// Suspend an operation until whatever it just queued has run, then
/// continue in `rest` — the port's `yieldk`.
///
/// The reference's exports end `return yieldk({ ... })`: they queue a
/// processor, suspend the coroutine, and the block runs on resumption
/// with the result in `returns`. A card here does the same by calling the
/// queueing function and then handing the rest of its work to this.
///
/// `rest` captures the card's locals **by type**, so a value the
/// continuation needs and the card forgot to carry does not compile. What
/// it must not capture is the field, which arrives as an argument — a
/// borrow held across the suspension would outlive the call.
///
/// The value the queued work produced is read back the way the reference
/// reads it, out of `returns`; see [`resumed_value`].
pub fn suspend(
    rest: impl for<'a> FnMut(&mut Field, &crate::effect::Ctx<'a>) -> Yield + Clone + Send + 'static,
) -> Yield {
    Yield::Suspended(crate::effect::Suspended(Box::new(rest)))
}

/// What the work a suspension waited on produced — `returns[0]`, which is
/// where the reference's `yieldk` blocks read their result from.
pub fn resumed_value(f: &Field) -> i32 {
    f.core.returns.at_i32(0)
}

/// The same, for the exports whose answer is a **mask** rather than a
/// count — `AnnounceRace` and its siblings write a `u64`, and reading
/// only the low half would lose every race above bit 31.
pub fn resumed_mask(f: &Field) -> u64 {
    f.core.returns.at_u64(0)
}

/// `Duel.CheckChainTarget(chaincount, card)` — whether that chain link's
/// effect would accept `card` as a target (`libduel.cpp`).
///
/// The only route to a target function's `chkc` argument. `chain.lua`
/// aliases it as `Chain.CanTarget`; no pool card calls either.
pub fn check_chain_target(f: &mut Field, chaincount: u8, card: CardId) -> bool {
    f.check_chain_target(chaincount, card)
}

/// `Duel.GetAttacker()` — `core.attacker` (`libduel.cpp`). The monster
/// that declared the attack, for as long as the battle is being resolved.
pub fn get_attacker(f: &Field) -> Option<CardId> {
    f.core.attacker
}

/// `Duel.GetAttackTarget()` — `core.attack_target`, which is `nil` for a
/// direct attack. Every card that reads it has to cope with that.
pub fn get_attack_target(f: &Field) -> Option<CardId> {
    f.core.attack_target
}

/// `Card.IsRelateToBattle` — `fieldid_r == core.pre_field[0] ||
/// fieldid_r == core.pre_field[1]` (`libcard.cpp`).
///
/// Not "was in this battle" but "**is still the same card** that was":
/// `pre_field` holds the two combatants' reset ids as they were when the
/// battle began, and `fieldid_r` changes whenever a card leaves and comes
/// back. A monster bounced and re-summoned mid-battle fails it, which is
/// what stops an effect reaching a card that is no longer the one it
/// fought.
pub fn is_relate_to_battle(f: &Field, c: CardId) -> bool {
    let id = f.cards[c].fieldid_r;
    id == f.core.pre_field[0] || id == f.core.pre_field[1]
}

/// `Duel.Remove(cards, pos, reason)` — `send_to` with `LOCATION_REMOVED`
/// as the destination. The export's defaults: no named player, the reason
/// player from `core`.
pub fn remove(f: &mut Field, cards: Vec<CardId>, pos: u8, why: u32) {
    let (by, rp) = (f.core.reason_effect, f.core.reason_player);
    f.send_to(
        cards,
        by,
        why,
        rp,
        PLAYER_NONE,
        u16::from(location::REMOVED),
        0,
        pos,
        false,
    );
}

/// `Effect.SetHintTiming(vs[, vo])` — when the client should *offer* an
/// optional activation, as two `TIMING_*` masks, one per player
/// (`libeffect.cpp`). The Lua export defaults `vo` to `vs`; both are
/// written here because every caller in the pool passes both.
///
/// It is a hint, not a legality: what may actually be activated is the
/// effect's condition and the window that is open. A wrong value shows up
/// as a prompt in the wrong place, not as an illegal play.
pub fn set_hint_timing(f: &mut Field, e: EffectId, vs: u32, vo: u32) {
    if let Some(x) = f.effects.get_mut(e) {
        x.hint_timing = [vs, vo];
    }
}

/// `Duel.GetCurrentPhase()` — `infos.phase` (`libduel.cpp`).
/// `Duel.IsDamageCalculated()` (`libduel.cpp:2121`) — has this damage
/// step already worked out its damage?
pub fn is_damage_calculated(f: &Field) -> bool {
    f.core.damage_calculated
}

pub fn get_current_phase(f: &Field) -> u16 {
    f.infos.phase
}

/// `Duel.IsTurnPlayer(player)` — `infos.turn_player == playerid`
/// (`libduel.cpp`). Note it compares against the *player id*, so a card
/// asking "is it the opponent's turn" passes `1 - tp`.
pub fn is_turn_player(f: &Field, playerid: u8) -> bool {
    f.infos.turn_player == playerid
}

/// `Card.IsAttackPos` — `is_position(POS_ATTACK)`, and `POS_ATTACK` is
/// `POS_FACEUP_ATTACK | POS_FACEDOWN_ATTACK`
/// (`ocgapi_constants.h:29`): the test is about the **orientation**, not
/// about being face-up. An ordinarily Set monster is face-down *defence*
/// and so fails it, but a card put face-down in attack position passes.
pub fn is_attack_pos(f: &Field, c: CardId) -> bool {
    f.cards[c].current.is_position(crate::position::ATTACK)
}

/// `Card.IsType(ttype)` in its plain form — no summon context: the
/// export defaults `scard` to none, `sumtype` to zero and the player to
/// `PLAYER_NONE` (the reason player only when `sumtype` is a Fusion
/// summon, which the plain form never is). Any bit of `ttype` suffices.
pub fn is_type(f: &mut Field, c: CardId, ttype: u32) -> bool {
    f.get_type(c, None, 0, PLAYER_NONE) & ttype != 0
}

/// `Card.IsSpellTrap` — `aux.FilterBoolFunction(Card.IsType,
/// TYPE_SPELL|TYPE_TRAP)` (`utility.lua:132`).
pub fn is_spell_trap(f: &mut Field, c: CardId) -> bool {
    is_type(f, c, card_type::SPELL | card_type::TRAP)
}

/// `Effect.SetLabel(n)` — scratch integers the effect carries for its own
/// functions (`libeffect.cpp`). The reference takes a list; a card that
/// stores one stores a list of one.
pub fn set_label(f: &mut Field, e: EffectId, label: Vec<i64>) {
    if let Some(x) = f.effects.get_mut(e) {
        x.label = label;
    }
}

/// `Effect.GetLabel()` — the first label, or **zero** when none was set
/// (`libeffect.cpp` pushes `0` for an empty label rather than failing).
pub fn get_label(e: &Effect) -> i64 {
    e.label.first().copied().unwrap_or(0)
}

/// The same, by id — for a card reading **another** effect's label.
pub fn get_label_of(f: &Field, e: EffectId) -> i64 {
    f.effects.get(e).map_or(0, get_label)
}

/// `Effect.GetLabelObject()` when what is stored is an **effect**.
///
/// Nothing, when the slot is empty or holds a card. The two are separate
/// accessors rather than one that answers `usize`, because a card id and
/// an effect id index different arenas.
pub fn get_label_object_effect(f: &Field, e: EffectId) -> Option<EffectId> {
    f.effects
        .get(e)
        .and_then(get_label_object)
        .and_then(LabelObject::effect)
}

/// `Effect.SetCountLimit(count, code, flag)` — how often this may be
/// used, and which tally it counts against (`libeffect.cpp`).
///
/// A count of zero is an error in the reference, not a silent "never";
/// the port refuses it the same way rather than registering an effect
/// that can never run. `code` names the tally — a "once per turn" shared
/// by every copy of a card counts against the card's own code, which is
/// what `SetCountLimit(1, id)` means.
pub fn set_count_limit(f: &mut Field, e: EffectId, count: u8, code_: u32, count_flag: u8) {
    assert!(
        count != 0,
        "a count limit of 0 is not a limit but a refusal"
    );
    let mut count_flag = count_flag;
    if count_flag & crate::effect::effect_count::CHAIN != 0 && code_ == 0 {
        count_flag |= crate::effect::effect_count::SINGLE;
    }
    if let Some(x) = f.effects.get_mut(e) {
        x.flag[0] |= flag::COUNT_LIMIT;
        x.count_limit = count;
        x.count_limit_max = count;
        x.count_code = code_;
        x.count_flag = count_flag;
    }
}

/// `Card.IsPreviousLocation(loc)` — where the card was *before* the move
/// being resolved (`libcard.cpp`).
///
/// The question a "when this is sent to the graveyard" trigger asks to
/// find out **where from**: a card that was already in a hand or a deck
/// did not leave the field, and Sangan only searches when it did.
pub fn is_previous_location(f: &Field, c: CardId, loc: u16) -> bool {
    f.cards[c].previous.is_location(loc)
}

/// `Card.IsAttackBelow(n)` — attack `n` or less (`libcard.cpp`).
///
/// Two guards, both the reference's. A card that is not a monster —
/// by printed type, by effective type, or by `EFFECT_PRE_MONSTER` —
/// answers **no** outright rather than comparing a zero attack. And a
/// negative attack answers no: `_atk >= 0 && _atk <= atk`, so a monster
/// whose attack has been driven below zero is not "attack 1500 or less".
pub fn is_attack_below(f: &mut Field, c: CardId, attack: i32) -> bool {
    let printed_monster = f.cards[c].data.type_ & card_type::MONSTER != 0;
    let effective_monster = f.get_type(c, None, 0, PLAYER_NONE) & card_type::MONSTER != 0;
    let pre_monster = f.is_affected_by_effect(c, code::PRE_MONSTER).is_some();
    if !printed_monster && !effective_monster && !pre_monster {
        return false;
    }
    let atk = get_attack(f, c);
    atk >= 0 && atk <= attack
}

/// `Card.IsFacedown()` — face-down, either way up (`libcard.cpp`).
pub fn is_facedown(f: &Field, c: CardId) -> bool {
    f.cards[c]
        .current
        .is_position(crate::board::position::FACEDOWN)
}

/// `Duel.ShuffleDeck(player)` — shuffle that player's deck
/// (`libduel.cpp`).
///
/// Under `DUEL_PSEUDO_SHUFFLE` this is the one shuffle that really does
/// nothing: `field::shuffle` skips the deck and only clears the pending
/// flag, where a **hand** shuffle happens regardless. So a card that
/// shuffles a deck is, in this configuration, announcing rather than
/// randomising — and the announcement still has to be made, because the
/// message is part of what the two engines compare.
pub fn shuffle_deck(f: &mut Field, player: u8) {
    if player > 1 {
        return;
    }
    f.shuffle(player, location::DECK);
}

/// `Duel.CheckLPCost(player, cost)` — could that player pay
/// (`libduel.cpp` → `field::check_lp_cost`)?
///
/// Not simply "has that much life": `EFFECT_LPCOST_CHANGE` effects
/// compose over the value first, and a cost reduced to zero or less is
/// payable by definition. The replacement path (`EFFECT_LPCOST_REPLACE`)
/// is what lets a player pay by some other means, and is why this can be
/// true for a player who could not afford the printed figure.
pub fn check_lp_cost(f: &mut Field, player: u8, cost: u32) -> bool {
    if player > 1 {
        return false;
    }
    f.check_lp_cost(player, cost)
}

/// `Duel.PayLPCost(player, cost)` — pay it (`libduel.cpp`).
///
/// Queues the unit; the reference yields on it, and no caller in this
/// pool reads a result back.
pub fn pay_lp_cost(f: &mut Field, player: u8, cost: u32) -> bool {
    if player > 1 {
        return false;
    }
    f.emplace(crate::processor::Kind::PayLPCost {
        playerid: player,
        cost,
    });
    true
}

/// `Duel.GetFieldGroupCount(player, s, o)` — how many are there, without
/// building a group the caller keeps (`libduel.cpp`).
pub fn get_field_group_count(f: &Field, player: u8, s: u32, o: u32) -> usize {
    get_field_group(f, player, s, o).len()
}

/// `Group.RandomSelect(player, count)` — take `count` at random
/// (`libgroup.cpp`).
///
/// **This rolls the duel's generator**, which makes it one of the few
/// places where two engines can disagree without either being wrong in
/// isolation. The reference's loop is kept exactly, including the part
/// that looks wasteful: it draws an index, inserts that card into a
/// `set`, and *keeps drawing until the set has grown*, so a duplicate
/// draw costs a roll and changes nothing. Picking without replacement
/// would consume a different number of values and every later roll in
/// the duel would diverge.
///
/// Two shortcuts before the loop, both the reference's: a count at or
/// above the group's size takes the whole group without rolling at all,
/// and a count of zero takes nothing.
/// `Group.RandomSelect(player, group, count)` — the request half. The picks
/// are read back with [`random_selected`] from a suspension, because in
/// solver mode the choice is the host's: a `SelectRandom` question is asked
/// and the operation must wait for it. In the faithful mode the generator
/// picks at once, exactly as before, and the suspension resumes without a
/// stop. Either way the picks are announced (`MSG_RANDOM_SELECTED`).
pub fn random_select_request(f: &mut Field, group: &[CardId], player: u8, count: usize) {
    f.core.random_selected.clear();
    if player > 1 {
        return;
    }
    let count = count.min(group.len());
    if count == 0 {
        f.finish_random_selection(player, Vec::new());
        return;
    }
    if count == group.len() {
        f.finish_random_selection(player, group.to_vec());
        return;
    }
    if f.core.chance_mode {
        f.emplace(crate::processor::Kind::SelectRandom {
            player,
            count: count as u8,
            cards: group.to_vec(),
        });
        return;
    }
    // Rolled as indices, announced as a set: ascending by card id, the
    // order the reference's `std::set<card*>` gives.
    let mut picks: Vec<CardId> = f
        .roll_random_indices(group.len(), count)
        .into_iter()
        .map(|i| group[i])
        .collect();
    picks.sort_unstable();
    f.finish_random_selection(player, picks);
}

/// The top `count` cards of `player`'s deck are about to be read — dug
/// through, revealed, counted from the top. In solver mode this asks the
/// host a `SelectDeckTop` question, so the hidden order is settled (with
/// [`Field::set_deck_order`]) before anything is looked at; the caller
/// **must** suspend after it and read the deck from the continuation. In
/// the faithful mode it does nothing: the deck is in the order the shuffle
/// gave it. A draw asks for itself; this is for the operations that read
/// the deck in place.
pub fn reveal_deck_request(f: &mut Field, player: u8, count: usize) {
    let count = count.min(f.players[usize::from(player)].main.len());
    if f.core.chance_mode && count > 0 {
        f.emplace(crate::processor::Kind::SelectDeckTop {
            player,
            count: count as u32,
        });
    }
}

/// The picks of the last [`random_select_request`], taken.
pub fn random_selected(f: &mut Field) -> Vec<CardId> {
    std::mem::take(&mut f.core.random_selected)
}

/// `SEQ_DECKSHUFFLE` (`constant.lua:30`) — "anywhere, then shuffle",
/// the sequence a card returned to a deck is usually given.
pub const SEQ_DECKSHUFFLE: u32 = 2;

/// `Duel.SendtoDeck(cards, player, sequence, reason)` — return them to a
/// deck (`libduel.cpp`).
///
/// `player` defaults to `PLAYER_NONE`, meaning **the owner's** deck.
/// `sequence` says where in it: `SEQ_DECKSHUFFLE` for "anywhere, then
/// shuffle", and the reference's one special value is `-2`, which sends
/// the card to **location zero** rather than to a deck at all — the
/// "return it to wherever it came from" case. Everything else is a deck.
///
/// Face-up, as the export always asks; `add_card` turns a card entering
/// a deck face-down on arrival.
pub fn send_to_deck(
    f: &mut Field,
    cards: Vec<CardId>,
    player: Option<u8>,
    sequence: i32,
    why: u32,
) -> bool {
    let to = player.unwrap_or(PLAYER_NONE);
    if to > PLAYER_NONE {
        return false;
    }
    let destination = if sequence == -2 {
        0
    } else {
        u16::from(location::DECK)
    };
    let (by, rp) = (f.core.reason_effect, f.core.reason_player);
    f.send_to(
        cards,
        by,
        why,
        rp,
        to,
        destination,
        sequence as u32,
        position::FACEUP,
        false,
    );
    true
}

/// `Card.GetSequence()` — where the card sits in its pile
/// (`libcard.cpp`).
///
/// In a deck this is an **index from the bottom**: the pile is stored
/// bottom-first, so the largest sequence is the card nearest the top.
/// A card looking for "the first Spell from the top" is looking for the
/// *highest* sequence, which reads backwards until you know that.
pub fn get_sequence(f: &Field, c: CardId) -> u32 {
    f.cards[c].current.sequence
}

/// `Duel.DisableShuffleCheck([disable])` — suppress the automatic
/// shuffle a pile would otherwise owe (`libduel.cpp`).
///
/// A card that is about to rearrange a deck deliberately, and then put
/// it back in a known order, turns this on so the engine does not shuffle
/// underneath it. The flag is cleared again by the executor around every
/// card function, so it does not leak past the card that set it.
pub fn disable_shuffle_check(f: &mut Field, disable: bool) {
    f.core.shuffle_check_disabled = disable;
}

/// `Duel.ConfirmDecktop(player, count)` — show the top `count` cards of
/// a deck to both players (`libduel.cpp`).
///
/// The count is capped at the deck's size. The message carries the codes
/// **top-first**, which is the reverse of how the pile is stored — the
/// reference walks `list_main.rbegin()` forward, and reading it the
/// other way round would show the bottom of the deck instead.
pub fn confirm_deck_top(f: &mut Field, player: u8, count: usize) -> bool {
    if player > 1 {
        return false;
    }
    let deck = &f.players[player as usize].main;
    let count = count.min(deck.len());
    let shown: Vec<CardId> = deck.iter().rev().take(count).copied().collect();
    let codes: Vec<u32> = shown.iter().map(|&c| f.cards[c].data.code).collect();
    f.messages
        .push(crate::field::Message::ConfirmDeckTop { player, codes });
    // Shown to both players.
    for p in 0..2u8 {
        f.core.revealed.extend(shown.iter().map(|&c| (p, c)));
    }
    true
}

/// `Duel.DiscardDeck(player, count, reason)` — send the top `count`
/// cards of a deck to the graveyard (`libduel.cpp`).
pub fn discard_deck(f: &mut Field, player: u8, count: u16, why: u32) -> bool {
    if player > 1 {
        return false;
    }
    f.emplace(crate::processor::Kind::DiscardDeck {
        playerid: player,
        count,
        reason: why,
        discarded: Vec::new(),
    });
    true
}

/// `Card.GetRace()` — the **effective** race (`libcard.cpp`), with the
/// same defaults [`is_race`] takes. A card folding several races
/// together reads this rather than the printed line.
pub fn get_race(f: &mut Field, c: CardId) -> u64 {
    f.get_race(c, None, 0, PLAYER_NONE)
}

/// `Card.IsDiscardable([reason, player])` — may this card be discarded
/// (`libcard.cpp`)?
///
/// Two clauses, and the first is easy to miss: a card affected by
/// `EFFECT_CANNOT_USE_AS_COST` is undiscardable **only when the reason is
/// a cost**. The same card discarded as an effect is fine. Both defaults
/// are the reference's — `REASON_COST`, and the reason player.
pub fn is_discardable(f: &mut Field, c: CardId, reason: Option<u32>, player: Option<u8>) -> bool {
    let reason = reason.unwrap_or(crate::card::reason::COST);
    let player = player.unwrap_or(f.core.reason_player);
    if reason == crate::card::reason::COST
        && f.is_affected_by_effect(c, code::CANNOT_USE_AS_COST)
            .is_some()
    {
        return false;
    }
    let by = f.core.reason_effect;
    f.is_player_can_discard_hand(player, c, by, reason)
}

/// `Duel.AnnounceRace(player, count, available)` — ask a player to name
/// `count` races out of `available` (`libduel.cpp`).
///
/// The count is capped at **how many races are actually on offer**, not
/// at what the caller asked for: `min(count, popcount(available))`. A
/// card offering a single race cannot demand two.
///
/// Two halves as every asking export is; [`resumed_value`] carries the
/// answer back as a mask.
pub fn announce_race(f: &mut Field, player: u8, count: u8, available: u64) -> bool {
    if player > 1 {
        return false;
    }
    let count = count.min(available.count_ones() as u8);
    f.emplace(crate::processor::Kind::AnnounceRace {
        playerid: player,
        count,
        available,
    });
    true
}

/// `Duel.SetPossibleOperationInfo(ct, category, cards, count, player,
/// param)` — what an activation *might* do (`libduel.cpp`).
///
/// The sibling of [`set_operation_info`], writing to a separate map on
/// the chain link. The difference is a promise against a possibility: a
/// card that will certainly take something to the hand declares that,
/// and a card that may *also* banish something later declares the banish
/// here. An opponent deciding whether to respond reads both, and folding
/// them into one would make every maybe look like a certainty.
pub fn set_possible_operation_info(
    f: &mut Field,
    ct: u8,
    category: u64,
    cards: Option<Vec<CardId>>,
    count: u8,
    player: u8,
    param: i32,
) {
    if let Some(ch) = get_chain_mut(f, ct) {
        ch.possible_opinfos.insert(
            category,
            crate::chain::OpTarget {
                cards,
                count,
                player,
                param,
            },
        );
    }
}

/// `Duel.IsPlayerAffectedByEffect(player, code)` — is any effect with
/// that code applying to the player (`libduel.cpp`)?
///
/// The `code` is usually an `EFFECT_*`, but the library also uses a
/// **card number** for "is this specific card applying" — which is how
/// `aux.SpElimFilter` asks after Spirit Elimination.
pub fn is_player_affected_by_effect(f: &mut Field, player: u8, code_: u32) -> bool {
    f.is_player_affected_by_effect(player, code_).is_some()
}

/// `Duel.GetLP(player)` — that player's life (`libduel.cpp`).
pub fn get_lp(f: &Field, player: u8) -> i32 {
    if player > 1 {
        return 0;
    }
    f.players[player as usize].lp
}

/// `Card.GetTextAttack()` — the attack **printed on the card**
/// (`libcard.cpp`), not the one it currently has.
///
/// `STATUS_NO_LEVEL` answers zero: a token or a card with no printed
/// line has no text attack to read. Everything else reads `data.attack`
/// and ignores every modifier on the board — which is the whole point,
/// for a card that wants to know what the monster *says* rather than
/// what it is worth right now.
pub fn get_text_attack(f: &Field, c: CardId) -> i32 {
    if f.cards[c].is_status(crate::card::status::NO_LEVEL) {
        return 0;
    }
    f.cards[c].data.attack
}

/// `Card.GetTextDefense()` (`libcard.cpp:376`) — the printed defence,
/// with the same `STATUS_NO_LEVEL` zero as [`get_text_attack`].
pub fn get_text_defense(f: &Field, c: CardId) -> i32 {
    if f.cards[c].is_status(crate::card::status::NO_LEVEL) {
        return 0;
    }
    f.cards[c].data.defense
}

/// `Card.GetOriginalType()` (`libcard.cpp:143`) — the printed type word,
/// untouched by any effect. What [`is_type_readonly`] masks, handed over
/// whole.
pub fn get_original_type(f: &Field, c: CardId) -> u32 {
    f.cards[c].data.type_
}

/// `Duel.Damage(player, amount, reason)` — deal damage (`libduel.cpp`).
///
/// A **negative or zero** amount becomes zero rather than healing: the
/// reference clamps with `if(amount > 0)` before it does anything. The
/// export yields a count of how much was actually dealt, which a card
/// reads to find out whether a replacement effect changed it.
pub fn damage(f: &mut Field, player: u8, amount: i64, why: u32) -> bool {
    if player > 1 {
        return false;
    }
    let actual = if amount > 0 { amount as u32 } else { 0 };
    let (by, rp) = (f.core.reason_effect, f.core.reason_player);
    f.damage(by, why, rp, None, player, actual, false);
    true
}

/// Park a procedure's arguments on the effect it built.
///
/// The port's stand-in for the Lua closure a procedure returns — see
/// [`crate::effect::AuxArgs`]. Only a procedure calls this; a card
/// translation has no use for it.
pub fn set_aux(f: &mut Field, e: EffectId, aux: crate::effect::AuxArgs) {
    if let Some(x) = f.effects.get_mut(e) {
        x.aux = Some(aux);
    }
}

/// What [`set_aux`] stored, if anything.
pub fn get_aux(f: &Field, e: EffectId) -> Option<crate::effect::AuxArgs> {
    f.effects.get(e).and_then(|x| x.aux)
}

/// `Card.EnableCounterPermit(counter_type[, range])` — let this card
/// hold a kind of counter (`libcard.cpp`).
///
/// Registers an `EFFECT_COUNTER_PERMIT | countertype` single effect whose
/// **value is the range** the permission applies in. The reference
/// defaults that range by printed type: a monster is permitted in the
/// Monster Zone, anything else in the Spell & Trap row and the Field
/// Zone. A counter placed under a permit goes in the card's *temporary*
/// half and is lost when the card is disabled.
pub fn enable_counter_permit(f: &mut Field, c: CardId, counter_type: u16, range: Option<u16>) {
    let range = range.unwrap_or_else(|| {
        if f.cards[c].data.is_type(card_type::MONSTER) {
            u16::from(location::MZONE)
        } else {
            u16::from(location::SZONE) | location::FZONE
        }
    });
    let e = create_effect(f, c);
    set_type(f, e, effect_type::SINGLE);
    set_code(f, e, code::COUNTER_PERMIT | u32::from(counter_type));
    set_value(f, e, i64::from(range));
    register_effect(f, c, e, false);
}

/// `Card.SetCounterLimit(counter_type, limit)` — how many of that
/// counter this card may hold (`libcard.cpp`).
pub fn set_counter_limit(f: &mut Field, c: CardId, counter_type: u16, limit: u32) {
    let e = create_effect(f, c);
    set_type(f, e, effect_type::SINGLE);
    set_code(f, e, code::COUNTER_LIMIT | u32::from(counter_type));
    set_value(f, e, i64::from(limit));
    register_effect(f, c, e, false);
}

/// `Card.AddCounter(counter_type, count[, singly])` — put counters on
/// (`libcard.cpp`).
///
/// **Guarded by immunity first.** A card the resolving effect cannot
/// touch gets nothing and the call answers false, which is checked before
/// the counter machinery is asked anything at all.
pub fn add_counter(f: &mut Field, c: CardId, counter_type: u16, count: u16, singly: bool) -> bool {
    let by = f.core.reason_effect;
    if !f.is_affect_by_effect(c, by) {
        return false;
    }
    let rp = f.core.reason_player;
    f.add_counter(c, rp, counter_type, count, singly)
}

/// `Card.GetCounter(counter_type)` — how many are on it, both halves
/// together (`libcard.cpp`).
///
/// A type of zero is an error in the reference rather than "all of
/// them"; there is a separate export for that, which nothing here uses.
pub fn get_counter(f: &Field, c: CardId, counter_type: u16) -> u16 {
    if counter_type == 0 {
        return 0;
    }
    f.get_counter(c, counter_type)
}

/// `Card.IsCanRemoveCounter(player, counter_type, count, reason)` — could
/// that many be taken off **this card** (`libcard.cpp`)?
///
/// Not simply "are there enough": a replacement effect registered on
/// `EFFECT_RCOUNTER_REPLACE + countertype` makes the removal payable
/// however few there are, the same shape [`check_lp_cost`] has.
pub fn is_can_remove_counter(
    f: &mut Field,
    c: CardId,
    player: u8,
    counter_type: u16,
    count: u16,
    reason: u32,
) -> bool {
    if player > 1 {
        return false;
    }
    f.is_player_can_remove_counter(player, Some(c), false, false, counter_type, count, reason)
}

/// `Card.RemoveCounter(player, counter_type, count, reason)` — take them
/// off **this card** (`libcard.cpp`).
///
/// Suspends, because a replacement effect may be offered instead; the
/// answer is read back with [`resumed_value`]. A type of zero is an
/// error in the reference.
pub fn remove_counter(
    f: &mut Field,
    c: CardId,
    player: u8,
    counter_type: u16,
    count: u16,
    reason: u32,
) -> bool {
    if counter_type == 0 {
        return false;
    }
    f.remove_counter_unit(reason, Some(c), player, false, false, counter_type, count);
    true
}

/// `Card.IsTrap()` — the **effective** type (`libcard.cpp`), the
/// single-type sibling of [`is_spell_trap`].
pub fn is_trap(f: &mut Field, c: CardId) -> bool {
    is_type(f, c, card_type::TRAP)
}

/// `Effect.IsTrapEffect()` — `utility.lua:794`, which is
/// `e:IsActiveType(TYPE_TRAP)`.
///
/// The **active** type, not the handler's printed one: an effect
/// activated from a Spell & Trap row carries the type it was activated
/// as, which is what a card asking "was that a Trap?" means.
pub fn is_trap_effect(f: &mut Field, e: EffectId) -> bool {
    f.get_active_type(e) & card_type::TRAP != 0
}

/// `Effect.IsSpellEffect()` — `utility.lua:790`, the Spell half of the
/// pair above and a **mask**, not an equality.
///
/// Worth saying out loud because Ryu Senshi uses both readings in one
/// card: its quick effect asks `re:GetActiveType()==TYPE_TRAP` and
/// answers only Normal Traps, while this helper answers any Spell at all.
/// The library wrote the loose one here and the script wrote the strict
/// one there, so the port does the same.
pub fn is_spell_effect(f: &mut Field, e: EffectId) -> bool {
    f.get_active_type(e) & card_type::SPELL != 0
}

/// `Duel.GetChainInfo(count, CHAININFO_TRIGGERING_LOCATION)` — where the
/// card that started that chain link was (`libduel.cpp`).
///
/// The symbolic zone bits are masked out, so a card in a Spell & Trap
/// Zone answers `LOCATION_SZONE` rather than `LOCATION_STZONE`. There is
/// a separate chain-info key for the symbolic form, which nothing here
/// asks for.
pub fn get_chain_triggering_location(f: &Field, count: u8) -> u16 {
    let Some(ch) = chain_at(f, count) else {
        return 0;
    };
    ch.triggering_location & !(location::STZONE | location::MMZONE | location::EMZONE)
}

/// `Duel.GetChainInfo(count, CHAININFO_TRIGGERING_EFFECT)`.
pub fn get_chain_triggering_effect(f: &Field, count: u8) -> Option<EffectId> {
    chain_at(f, count).map(|ch| ch.triggering_effect)
}

/// The chain link `count` names, one-based, with zero meaning the
/// topmost — the reference's own indexing.
fn chain_at(f: &Field, count: u8) -> Option<&Chain> {
    if usize::from(count) > f.core.current_chain.len() {
        return None;
    }
    if count == 0 {
        f.core.current_chain.last()
    } else {
        f.core.current_chain.get(usize::from(count) - 1)
    }
}

/// `Duel.NegateEffect(chaincount)` — negate a chain link's effect
/// (`libduel.cpp`).
pub fn negate_effect(f: &mut Field, chaincount: u8) -> bool {
    f.disable_chain(chaincount)
}

/// `Duel.IsChainDisablable(chaincount)` (`libduel.cpp:3916`).
///
/// **It answers `true` whenever the chain is not solving**, and only
/// consults the real predicate inside a resolution. That reads as a
/// stub, and is not: the question a negator asks *while the chain is
/// still being built* — Dark Balter's, on `EVENT_CHAINING` — is asked
/// before anything about the link could make it undisablable, so the
/// reference hands back a yes and lets `solve_chain` refuse later. The
/// guard is transcribed rather than dropped because `chain_solving` is
/// exactly what tells the two callers apart.
pub fn is_chain_disablable(f: &Field, chaincount: u8) -> bool {
    if f.core.chain_solving {
        return f.is_chain_disablable(chaincount);
    }
    true
}

/// `Card.GetBattleTarget()` (`libcard.cpp:2038`) — the other side of the
/// battle this card is in, or nothing if it is in none.
///
/// The reference reads the two `core` slots rather than anything on the
/// card, so a card that is not one of the pair answers nil even if a
/// battle is happening.
pub fn get_battle_target(f: &Field, c: CardId) -> Option<CardId> {
    if f.core.attacker == Some(c) {
        f.core.attack_target
    } else if f.core.attack_target == Some(c) {
        f.core.attacker
    } else {
        None
    }
}

/// `aux.TargetBoolFunction(Card.IsTrap)` (`utility.lua:80`) — a field
/// effect's target filter built from a plain card predicate.
///
/// The reference returns a closure taking `(effect, target)` and
/// discarding the effect; [`crate::effect::TargetFilter`] has the same
/// shape with two more slots, so this is the same discard spelled out.
///
/// **It reads the *printed* type, where `Card.IsTrap` reads the
/// effective one**, and that is a real difference rather than a
/// restatement: the seam hands out `&Field` and `get_type` needs it
/// mutably (it uses `temp.type_` as a recursion guard), so the effective
/// answer is not available here. Widening `TargetFilter` is the same
/// crate-wide change `ValueFn` would need — `is_fit_target_function` is
/// `&self` and is called from predicates that already hold a borrow.
///
/// The two answers differ only for a card whose type an effect has
/// changed. **No card in this pool has an `EFFECT_ADD_TYPE`,
/// `_REMOVE_TYPE` or `_CHANGE_TYPE`** — checked by
/// `tools/check_constants.py`'s `ABSENT_FROM_POOL` scan over every pool
/// script — and the one type change the rules make on their own, a trap
/// monster gaining `TYPE_MONSTER`, keeps `TYPE_TRAP`. So for this pool
/// the two agree exactly; for a pool that adds such a card, this is
/// where it breaks.
pub fn target_is_trap(f: &Field, _e: EffectId, target: Option<CardId>, _args: &[i64]) -> bool {
    target.is_some_and(|c| is_trap_readonly(f, c))
}

/// `Card.IsType` through an immutable seam — the **printed** type.
///
/// The general form of [`is_trap_readonly`], and it exists for the same
/// reason: `TargetFilter` and `ValueFn` hand out `&Field`, and the
/// effective-type reader needs it mutably. See [`target_is_trap`] for why
/// the two answers agree across this pool.
pub fn is_type_readonly(f: &Field, c: CardId, type_: u32) -> bool {
    f.cards[c].data.is_type(type_)
}

/// `Card.IsTrap` through an immutable seam — the **printed** type.
///
/// The sibling of [`is_code_readonly`], and it exists for the same
/// reason: two of the port's card-facing seams hand out `&Field`
/// (`TargetFilter` and `ValueFn`), and the effective-type reader needs it
/// mutably because it uses `temp.type_` as a recursion guard.
///
/// See [`target_is_trap`] for why the two answers agree across this pool.
pub fn is_trap_readonly(f: &Field, c: CardId) -> bool {
    f.cards[c].data.is_type(card_type::TRAP)
}

/// The handler of an effect that is trying to activate, as a Trap —
/// `s.aclimit`'s `re:GetHandler():IsTrap()`.
///
/// `EFFECT_CANNOT_ACTIVATE`'s value is asked with the **activating
/// effect** in `ctx.reason_effect`, which is why this takes an effect and
/// not a card.
pub fn handler_is_trap(f: &Field, e: EffectId) -> bool {
    get_handler(f, e).is_some_and(|c| is_trap_readonly(f, c))
}

/// The bit a **flag effect**'s code carries, so that a flag and a real
/// effect can share a number without colliding.
///
/// `RegisterFlagEffect` masks the script's id to 28 bits and sets this;
/// every reader masks the same way. It is why a flag effect is addressed
/// by its code alone — the code *is* the whole address.
pub const FLAG_EFFECT: u32 = 0x1000_0000;

pub fn flag_code(id: u32) -> u32 {
    (id & 0x0fff_ffff) | FLAG_EFFECT
}

/// `Card.IsHasEffect(code)` — is this card under an effect with that
/// code (`libcard.cpp`)?
///
/// The card-facing sibling of [`is_player_affected_by_effect`]. The
/// reference returns the effect itself; nothing in this pool needs more
/// than whether there is one.
pub fn is_has_effect(f: &Field, c: CardId, code_: u32) -> bool {
    f.is_affected_by_effect(c, code_).is_some()
}

/// `Card.RegisterFlagEffect(id, reset, flag, count[, label, desc])` — put
/// a marker on a card (`libcard.cpp`).
///
/// A flag effect is not a special kind of thing: it is an ordinary
/// `EFFECT_TYPE_SINGLE` effect with a masked code and no behaviour, whose
/// only purpose is to be counted later. What makes it a *flag* is that
/// nothing reads its value — only whether it is there.
///
/// Three details the reference fixes and a caller cannot change:
/// **`CANNOT_DISABLE` is always added** (a marker that a disable could
/// erase would be useless), a count of zero means one, and a phase reset
/// with neither turn named gets both — so "until the end phase" means the
/// next one, whoever's turn it is.
///
/// `handler` is left empty, which is what makes `add_effect` treat it as
/// the owner's own.
pub fn register_flag_effect(
    f: &mut Field,
    c: CardId,
    id: u32,
    reset_flag: u32,
    flag: u32,
    count: u8,
    label: i64,
) -> EffectId {
    let count = if count == 0 { 1 } else { count };
    let mut reset_flag = reset_flag;
    if reset_flag & reset::PHASE != 0 && reset_flag & (reset::SELF_TURN | reset::OPPO_TURN) == 0 {
        reset_flag |= reset::SELF_TURN | reset::OPPO_TURN;
    }
    let mut e = Effect::new(effect_type::SINGLE, flag_code(id));
    e.owner = Some(c);
    e.handler = None;
    e.reset_flag = reset_flag;
    e.flag[0] = flag | flag::CANNOT_DISABLE;
    e.reset_count = count;
    // `peffect->label = { lab }` — one entry, always, even when it is the
    // zero the reference defaults to. `GetFlagEffectLabel` reads
    // `label[0]` and falls back to zero for an empty vector, so the two
    // spellings agree; keeping the entry is the reference's shape.
    e.label = vec![label];
    let eid = f.new_effect(e);
    register_effect(f, c, eid, false);
    eid
}

/// `Card.GetFlagEffect(id)` — how many of that marker the card carries
/// (`libcard.cpp`).
pub fn get_flag_effect(f: &Field, c: CardId, id: u32) -> usize {
    f.cards[c].single_effect.equal_range(flag_code(id)).len()
}

/// `Card.GetFlagEffectLabel(id)` (`libcard.cpp:1229`) — the label each of
/// that marker carries, in registration order.
///
/// The reference returns **nil** rather than an empty list when the card
/// has none, and the one caller in this pool iterates the result either
/// way, so the empty vector is the same thing here.
pub fn get_flag_effect_label(f: &Field, c: CardId, id: u32) -> Vec<i64> {
    f.cards[c]
        .single_effect
        .equal_range(flag_code(id))
        .iter()
        .map(|&e| {
            f.effects
                .get(e)
                .and_then(|x| x.label.first().copied())
                .unwrap_or(0)
        })
        .collect()
}

/// `Card.HasFlagEffect(id[, ct])` (`utility.lua:704`) —
/// `GetFlagEffect(id) >= ct`, with `ct` defaulting to one.
pub fn has_flag_effect(f: &Field, c: CardId, id: u32) -> bool {
    get_flag_effect(f, c, id) >= 1
}

/// `Card.ResetFlagEffect(id)` — take the marker off (`libcard.cpp`).
///
/// `RESET_CODE`, which is the one reset kind addressed by a code rather
/// than by an event or a phase.
pub fn reset_flag_effect(f: &mut Field, c: CardId, id: u32) {
    f.reset_card(c, flag_code(id), reset::CODE);
}

/// `Duel.Recover(player, amount, reason)` — give a player life
/// (`libduel.cpp`).
///
/// [`damage`]'s mirror, and the same clamp: a negative figure becomes
/// zero rather than draining life. Suspends, like every life change, so
/// that a replacement effect gets its turn; the amount actually recovered
/// is read back with [`resumed_value`].
pub fn recover(f: &mut Field, player: u8, amount: i64, why: u32) -> bool {
    if player > 1 {
        return false;
    }
    let actual = if amount > 0 { amount as u32 } else { 0 };
    let (by, rp) = (f.core.reason_effect, f.core.reason_player);
    f.recover(by, why, rp, player, actual, false);
    true
}

/// `Effect.GetHandlerPlayer()` — the player the effect's handler belongs
/// to (`libeffect.cpp`).
///
/// **Not the same as the card's controller.** An equip spell that has
/// changed a monster's side still answers with its own controller, which
/// is exactly what Snatch Steal's control value is for: the effect names
/// the player who played it, whatever the monster's side now says.
/// `Effect.GetHandlerPlayer()` asked of an effect **object** rather than
/// an id — the form a value function needs when the context's
/// `reason_effect` is some *other* effect. `aux.tgoval(e,re,rp)` is the
/// case: `e` is the protection being evaluated, `re` the effect doing
/// the targeting, and the seam hands `e` over as `&Effect`.
pub fn get_handler_player_of(f: &Field, e: &Effect) -> u8 {
    e.get_handler_player(&f.cards)
}

pub fn get_handler_player(f: &Field, e: EffectId) -> u8 {
    f.effects
        .get(e)
        .map_or(PLAYER_NONE, |x| x.get_handler_player(&f.cards))
}

/// `Card.GetEquipTarget()` — what this card is attached to
/// (`libcard.cpp`), or nothing.
pub fn get_equip_target(f: &Field, c: CardId) -> Option<CardId> {
    f.cards[c].equiping_target
}

/// `Card.IsControlerCanBeChanged(ignore_mzone, zone)` — could this
/// monster change hands **right now** (`libcard.cpp`)?
///
/// The whole question, where [`is_able_to_change_controler`] asks only
/// about the lock: it also wants the card to be a face-up monster on the
/// field and for the *other* side to have room — a Monster Zone, and for
/// a trap monster a Spell & Trap seat as well.
///
/// **`ignore_mzone` is what a card asking on behalf of a *cost* passes.**
/// Enemy Controller releases a monster to make the seat, so it asks
/// whether control could change *ignoring* the seat it is about to free;
/// a card that is not paying anything asks with the default `false`.
/// `zone` defaults to every zone.
pub fn is_controler_can_be_changed(f: &mut Field, c: CardId, ignore_mzone: bool) -> bool {
    f.is_control_can_be_changed(c, ignore_mzone, 0xff)
}

/// `Duel.IsDuelType(flag)` — is this duel option in force
/// (`libduel.cpp`)?
pub fn is_duel_type(f: &Field, flag: u64) -> bool {
    f.is_flag(flag)
}

/// `Card.IsAttribute(attr)` — the **effective** attribute, as a mask
/// (`libcard.cpp`). Same shape as [`is_race`], and the same defaults:
/// no summoning card, summon type zero, `PLAYER_NONE`.
pub fn is_attribute(f: &mut Field, c: CardId, attribute: u32) -> bool {
    f.get_attribute(c, None, 0, PLAYER_NONE) & attribute != 0
}

/// `Duel.GetLocationCount(player, location)` — how many more cards that
/// player could put there (`libduel.cpp`).
///
/// The reference's defaults are spelled out: the **reason player** does
/// the putting (not the owner of the zone), the reason is
/// `LOCATION_REASON_TOFIELD`, and every zone is allowed. It also returns
/// a second value — the bitmask of which zones are free — that no card
/// in this pool reads, so this returns the count alone.
/// `LOCATION_REASON_TOFIELD` and `LOCATION_REASON_CONTROL`, as a script
/// names them. Counting seats "to put a card there" and "to take one
/// over" are different counts, and the constant is how a caller says
/// which.
pub const LOCATION_REASON_TOFIELD: u32 = Field::LOCATION_REASON_TOFIELD;
pub const LOCATION_REASON_CONTROL: u32 = Field::LOCATION_REASON_CONTROL;

/// `Duel.GetMZoneCount(player, without)` — Monster Zone seats, counting
/// as if `without` were not on the board (`libduel.cpp:1699`).
///
/// `without` is a card **or a group**: a procedure asking "will the
/// materials I am about to banish leave room" excludes all of them at
/// once.
///
/// A card asking "is there room for one more" while it is *itself* about
/// to leave has to exclude itself, or it counts its own seat as taken.
/// The reference's trailing defaults, which this pool takes: the reason
/// player does the putting, `LOCATION_REASON_TOFIELD`, every zone.
///
/// The reference also returns the free-seat mask as a second value, which
/// nothing here reads.
pub fn get_mzone_count(
    f: &mut Field,
    player: u8,
    without: Except,
    uplayer: Option<u8>,
    reason: Option<u32>,
) -> i32 {
    if player > 1 {
        return 0;
    }
    let uplayer = uplayer.unwrap_or(f.core.reason_player);
    // `lua_get_card_or_group<true>` — the same three shapes the matching
    // functions' exception argument takes, read by the same helper, which
    // is why this reuses `Except` rather than growing a twin of it.
    let one;
    let without = match without {
        Except::None => None,
        Except::Card(c) => {
            one = [c];
            Some(&one[..])
        }
        Except::Group(g) => Some(g),
    };
    f.get_mzone_count(
        player,
        without,
        uplayer,
        reason.unwrap_or(Field::LOCATION_REASON_TOFIELD),
        0xff,
    )
}

/// `Card.EnableReviveLimit()` — register the two effects that make a
/// monster "must be properly summoned before it can be revived"
/// (`libcard.cpp:1333`).
///
/// It is **two** effects, not one, and they are not interchangeable:
/// `EFFECT_UNSUMMONABLE_CARD` stops the normal summon,
/// `EFFECT_REVIVE_LIMIT` stops the revival of a copy that never landed
/// properly. Both are `CANNOT_DISABLE | UNCOPYABLE`, and the whole call
/// is skipped while the card is copying another's effects.
pub fn enable_revive_limit(f: &mut Field, c: CardId) {
    if f.cards[c].is_status(crate::card::status::COPYING_EFFECT) {
        return;
    }
    for code_ in [code::UNSUMMONABLE_CARD, code::REVIVE_LIMIT] {
        let e = create_effect(f, c);
        set_type(f, e, effect_type::SINGLE);
        set_code(f, e, code_);
        set_property(f, e, flag::CANNOT_DISABLE | flag::UNCOPYABLE, 0);
        register_effect(f, c, e, false);
    }
}

/// `Duel.ChainAttack([target])` — let the attacker declare again
/// (`libduel.cpp:2158`).
///
/// **It can refuse silently.** With no attacker, or with one the reason
/// effect may not affect, the reference returns without setting
/// anything — so a card that calls this is not guaranteed a second
/// attack, and the flag it sets is what the battle phase reads later.
///
/// `chain_attacker_id` is the attacker's `fieldid`, not its handle: the
/// battle phase compares identities, so a monster that leaves and comes
/// back is not the one that was granted the attack.
pub fn chain_attack(f: &mut Field, target: Option<CardId>) {
    let Some(attacker) = f.core.attacker else {
        return;
    };
    if !f.is_affect_by_effect(attacker, f.core.reason_effect) {
        return;
    }
    f.core.chain_attack = true;
    f.core.chain_attacker_id = f.cards[attacker].fieldid;
    if target.is_some() {
        f.core.chain_attack_target = target;
    }
}

/// `Card.CanChainAttack([ac, monsteronly])` — could this monster be
/// given another attack right now (`libcard.cpp:1548`)?
///
/// The export's defaults, which BLS takes: `ac = 2` and
/// `monsteronly = false`.
///
/// Six refusals before the target scan, and two are about **identity**
/// rather than about ability: the card must still be the `core.attacker`,
/// and its `fieldid_r` must still match `core.pre_field[0]` — a monster
/// that left the field and came back is a different monster as far as
/// this battle is concerned. `ac` is an attack **budget**: non-zero means
/// "refuse once the announce count reaches it", and the `ac == 2` case
/// additionally refuses a card already granted an extra attack, so the
/// two ways of getting a second attack do not stack.
///
/// The tail asks whether there is anything to attack: a scan with
/// `chain_attack` on, and an empty result is a refusal unless the card
/// can attack directly.
pub fn can_chain_attack(f: &mut Field, c: CardId, ac: Option<u8>, monsteronly: bool) -> bool {
    if f.core.attacker != Some(c) {
        return false;
    }
    let ac = ac.unwrap_or(2);
    let turn_player = f.infos.turn_player;
    if f.cards[c].is_status(crate::card::status::BATTLE_DESTROYED)
        || f.cards[c].current.controller != turn_player
        || f.cards[c].fieldid_r != f.core.pre_field[0]
        || !f.is_capable_attack_announce(c, turn_player)
        || (ac != 0 && f.cards[c].announce_count >= u16::from(ac))
        || (ac == 2 && f.is_affected_by_effect(c, code::EXTRA_ATTACK).is_some())
    {
        return false;
    }
    let mut targets = Vec::new();
    f.get_attack_target(c, &mut targets, true, true, None);
    !targets.is_empty() || (!monsteronly && f.cards[c].direct_attackable)
}

/// `Card.GetAttackAnnouncedCount()` — `attack_announce_count`
/// (`libcard.cpp:1012`): how many times this monster has declared an
/// attack while it has been on the field.
///
/// `reset_card` zeroes it when the card arrives or leaves, so "has not
/// attacked yet this turn" is really "has not attacked since it got
/// here".
pub fn get_attack_announced_count(f: &Field, c: CardId) -> i32 {
    i32::from(f.cards[c].attack_announce_count)
}

/// `Card.CompleteProcedure()` — mark a summon procedure as having been
/// carried out (`libcard.cpp`).
///
/// `STATUS_PROC_COMPLETE` is what `is_can_be_special_summoned` reads to
/// waive a `REVIVE_LIMIT`: a card properly summoned once may be revived
/// afterwards. A card special summoned *by ignoring* its conditions —
/// `nocheck`/`nolimit` true, as the LV monsters do — has to say so
/// explicitly, which is what this call is for.
pub fn complete_procedure(f: &mut Field, c: CardId) {
    f.cards[c].set_status(crate::card::status::PROC_COMPLETE, true);
}

/// `Card.IsAbleToGraveAsCost()` — could this card be sent to a graveyard
/// **as a cost** (`libcard.cpp`)?
pub fn is_able_to_grave_as_cost(f: &mut Field, c: CardId) -> bool {
    let rp = f.core.reason_player;
    f.is_capable_cost_to_grave(c, rp)
}

/// `Duel.GetLocationCount(player, location, uplayer, reason)` — the
/// form with the two trailing arguments spelled out.
///
/// The defaults [`get_location_count`] takes are the common case; a card
/// that is counting seats **for a control change** rather than for
/// putting one of its own cards there has to say so, because the two are
/// counted differently.
pub fn get_location_count_for(
    f: &mut Field,
    player: u8,
    location: u8,
    uplayer: u8,
    reason: u32,
) -> i32 {
    if player > 1 {
        return 0;
    }
    f.get_useable_count(None, player, location, uplayer, reason, 0xff)
}

pub fn get_location_count(f: &mut Field, player: u8, location: u8) -> i32 {
    if player > 1 {
        return 0;
    }
    let uplayer = f.core.reason_player;
    f.get_useable_count(
        None,
        player,
        location,
        uplayer,
        Field::LOCATION_REASON_TOFIELD,
        0xff,
    )
}

/// `Card.IsCanBeSpecialSummoned(e, sumtype, sumplayer, nocheck, nolimit)`
/// — could this card be special summoned *by this effect* right now
/// (`libcard.cpp`)?
///
/// The reference's trailing defaults, which every pool caller takes:
/// face-up, to the summoning player's own side, any zone.
pub fn is_can_be_special_summoned(
    f: &mut Field,
    c: CardId,
    e: EffectId,
    sumtype: u32,
    sumplayer: u8,
    nocheck: bool,
    nolimit: bool,
) -> bool {
    is_can_be_special_summoned_in(
        f,
        c,
        e,
        sumtype,
        sumplayer,
        nocheck,
        nolimit,
        crate::board::position::FACEUP,
    )
}

/// The same, with the **position** given rather than defaulted.
///
/// The reference's seventh argument, which a card that can only revive
/// into one position passes explicitly — Call of the Haunted asks about
/// `POS_FACEUP_ATTACK` alone, so a monster that may only arrive in
/// defence is no use to it.
#[allow(clippy::too_many_arguments)]
pub fn is_can_be_special_summoned_in(
    f: &mut Field,
    c: CardId,
    e: EffectId,
    sumtype: u32,
    sumplayer: u8,
    nocheck: bool,
    nolimit: bool,
    positions: u8,
) -> bool {
    f.is_can_be_special_summoned(
        c,
        Some(e),
        sumtype,
        positions,
        sumplayer,
        sumplayer,
        nocheck,
        nolimit,
        0xff,
    )
}

/// `Duel.SpecialSummon(cards, sumtype, sumplayer, playerid, nocheck,
/// nolimit, positions)` — summon them (`libduel.cpp`).
///
/// `sumplayer` is who is *doing* it and `playerid` whose side they land
/// on; they are the same for an ordinary self-summon and differ for a
/// card that puts something on the opponent's field. An out-of-range
/// `sumplayer` is refused, as the reference refuses it.
///
/// The zone mask defaults to every zone. Like `SendtoHand` this yields a
/// count in the reference; no pool caller reads it, so this queues and
/// returns.
#[allow(clippy::too_many_arguments)]
pub fn special_summon(
    f: &mut Field,
    cards: Vec<CardId>,
    sumtype: u32,
    sumplayer: u8,
    playerid: u8,
    nocheck: bool,
    nolimit: bool,
    positions: u8,
) -> bool {
    if sumplayer >= PLAYER_NONE {
        return false;
    }
    let set: std::collections::BTreeSet<CardId> = cards.into_iter().collect();
    f.special_summon(
        set, sumtype, sumplayer, playerid, nocheck, nolimit, positions, 0xff,
    );
    true
}

/// `Card.IsLevelBelow(n)` — level `n` or less (`libcard.cpp`).
///
/// **A level of zero is never below anything.** The reference guards
/// `plvl > 0 && plvl <= lvl`, so a Spell, a Trap, a Link monster or an
/// Xyz without a level answers no however small `n` is. Dropping the
/// guard would quietly make every Spell in a deck a legal search target
/// for a card looking for small monsters.
pub fn is_level_below(f: &mut Field, c: CardId, level: u32) -> bool {
    let lv = f.get_level(c);
    lv > 0 && lv <= level
}

/// `Card.IsRace(race)` — the **effective** race, as a mask
/// (`libcard.cpp`).
///
/// The export takes optional summon-context arguments that decide which
/// race a card counts as while being summoned a particular way; nothing
/// in this pool passes them, so the defaults are spelled out here —
/// no summoning card, summon type zero, and `PLAYER_NONE`.
/// `Card.IsRace(race)` read from the **printed** line, for the `&Field`
/// seams — a field effect's target filter, a value function.
///
/// `Card.IsRace` reads the effective race, which `get_race` computes
/// mutably. The two agree unless an `EFFECT_ADD_RACE`, `_REMOVE_RACE` or
/// `_CHANGE_RACE` is in play, and **no card in this pool has one** —
/// `tools/check_constants.py`'s `ABSENT_FROM_POOL` scan is the licence,
/// the same one [`target_is_trap`] and Fiend Skull Dragon's flip filter
/// hold for the type.
pub fn is_race_readonly(f: &Field, c: CardId, race: u64) -> bool {
    f.cards[c].data.race & race != 0
}

pub fn is_race(f: &mut Field, c: CardId, race: u64) -> bool {
    f.get_race(c, None, 0, PLAYER_NONE) & race != 0
}

/// `Duel.SelectMatchingCard(player, filter, s, loc1, loc2, min, max,
/// exception)` — scan, then ask a player to choose from what was found
/// (`libduel.cpp`).
///
/// The **non-targeting** sibling of `SelectTarget`: it runs the same scan
/// `GetMatchingGroup` does — `filter_matching_card`, which asks nothing
/// about targetability — and puts the result up for selection. A card
/// reaching into a deck uses this rather than `SelectTarget`, because a
/// card in a deck cannot be targeted at all.
///
/// Two halves as every asking export is. [`group_selected`] is the other.
#[allow(clippy::too_many_arguments)]
pub fn select_matching_card(
    f: &mut Field,
    player: u8,
    filter: Option<Filter>,
    s: u8,
    loc1: u32,
    loc2: u32,
    min: u8,
    max: u8,
    exception: Except,
) -> bool {
    if player > 1 {
        return false;
    }
    // Assigned in scan order and **not** pre-sorted: the `SelectCard`
    // processor applies `card_operation_sort` on its way out, which is
    // the reference's ordering. Sorting here as well would only decide
    // the ties that sort leaves alone, by a rule nothing in ocgcore has.
    f.core.select_cards = get_matching_group(f, filter, s, loc1, loc2, exception);
    f.emplace(crate::processor::Kind::SelectCard {
        player,
        cancelable: false,
        min,
        max,
    });
    true
}

/// `Card.IsSpell` — printed or effective Spell (`libcard.cpp`), the
/// single-type sibling of [`is_spell_trap`].
pub fn is_spell(f: &mut Field, c: CardId) -> bool {
    is_type(f, c, card_type::SPELL)
}

/// `Duel.SendtoHand(cards, player, reason)` — send to a hand
/// (`libduel.cpp`), face-up, sequence zero.
///
/// `player` is the hand to send to and defaults to `PLAYER_NONE`, which
/// means **the owner's**. That is not the same as the controller's: a card
/// taken back under someone else's control still goes home. The reference
/// refuses a player above `PLAYER_NONE` outright.
pub fn send_to_hand(f: &mut Field, cards: Vec<CardId>, player: Option<u8>, why: u32) -> bool {
    let to = player.unwrap_or(PLAYER_NONE);
    if to > PLAYER_NONE {
        return false;
    }
    let (by, rp) = (f.core.reason_effect, f.core.reason_player);
    f.send_to(
        cards,
        by,
        why,
        rp,
        to,
        u16::from(location::HAND),
        0,
        position::FACEUP,
        false,
    );
    true
}

/// `Duel.ConfirmCards(player, cards)` — reveal cards to a player
/// (`libduel.cpp`).
///
/// More than a message. Three things ride along, and all three are easy
/// to leave out because nothing in this pool reads them:
///
/// 1. **The reason depends on where we are.** `REASON_EFFECT` while a
///    chain is solving, `REASON_COST` otherwise — the reference reads
///    `core.chain_solving` to decide, so the same call means different
///    things in a cost and in a resolution.
/// 2. **The revealing player is the other one.** `1 - playerid`: the
///    argument names who *sees*, and the event names who *showed*.
/// 3. **`EVENT_TOHAND_CONFIRM` is conditional on two things at once** —
///    the card being in the hand now, and a `TO_HAND` event being in the
///    current set. Revealing a hand card outside a to-hand moment raises
///    only `EVENT_CONFIRM`.
///
/// The port's `Message::ConfirmCards` carries codes only, where the
/// reference also writes each card's controller, location and sequence.
/// That is a pre-existing simplification shared with `Draw` and
/// `SpellSetGroup`, not something this export introduces.
///
/// An empty group is refused, as is a player who does not exist.
pub fn confirm_cards(f: &mut Field, player: u8, cards: Vec<CardId>) -> bool {
    if player > 1 || cards.is_empty() {
        return false;
    }
    f.messages.push(crate::field::Message::ConfirmCards {
        player,
        codes: cards.iter().map(|&c| f.cards[c].data.code).collect(),
    });
    f.core.revealed.extend(cards.iter().map(|&c| (player, c)));

    let reason = if f.core.chain_solving {
        crate::card::reason::EFFECT
    } else {
        crate::card::reason::COST
    };
    let by = f.core.reason_effect;
    let rp = f.core.reason_player;
    let revealing = 1 - player;
    let during_to_hand = f.check_event(code::TO_HAND).is_some();

    let mut in_hand: Vec<CardId> = Vec::new();
    for &c in &cards {
        f.raise_single_event(c, Vec::new(), code::CONFIRM, by, reason, rp, revealing, 0);
        if during_to_hand && f.cards[c].current.location == location::HAND {
            f.raise_single_event(
                c,
                Vec::new(),
                code::TOHAND_CONFIRM,
                by,
                reason,
                rp,
                revealing,
                0,
            );
            in_hand.push(c);
        }
    }
    f.raise_event_over(cards, code::CONFIRM, by, reason, rp, revealing, 0);
    if !in_hand.is_empty() {
        f.raise_event_over(in_hand, code::TOHAND_CONFIRM, by, reason, rp, revealing, 0);
    }
    f.process_single_event();
    f.process_instant_event();
    true
}

/// `Effect.GetHandler` — `effect::get_handler`: the real handler when the
/// effect is carried by an overlay, else the handler.
pub fn get_handler(f: &Field, e: EffectId) -> Option<CardId> {
    f.effects.get(e).and_then(|x| x.get_handler(&f.cards))
}

/// The `exception` argument the matching functions take: the reference
/// reads it as a card first and a group second (`libduel.cpp`,
/// `IsExistingMatchingCard`), so `nil`, one card and a group are the three
/// shapes.
#[derive(Clone, Copy, Debug, Default)]
pub enum Except<'a> {
    #[default]
    None,
    Card(CardId),
    Group(&'a [CardId]),
}

impl<'a> Except<'a> {
    fn split(self) -> (Option<CardId>, Option<&'a [CardId]>) {
        match self {
            Except::None => (None, None),
            Except::Card(c) => (Some(c), None),
            Except::Group(g) => (None, Some(g)),
        }
    }
}

/// `Duel.IsExistingMatchingCard(f, player, s, o, count, exception[, ...])`
/// — `filter_matching_card` with no group and no `pret`, stopping at
/// `count` matches.
pub fn is_existing_matching_card(
    f: &mut Field,
    filter: Option<Filter>,
    player: u8,
    s: u32,
    o: u32,
    count: i32,
    exception: Except,
) -> bool {
    let (exc, exg) = exception.split();
    f.filter_matching_card(filter, player, s, o, None, exc, exg, None, count, false)
}

/// `Duel.GetMatchingGroup(f, player, s, o, exception[, ...])` — every
/// match, collected.
///
/// The reference collects into a `std::set` ordered by the cards' creation
/// ids, so a group's iteration order is creation order whatever order the
/// zones were scanned in. A card id here **is** the creation index (cards
/// are created in decklist order on both engines), so sorting gives the
/// same order.
pub fn get_matching_group(
    f: &mut Field,
    filter: Option<Filter>,
    player: u8,
    s: u32,
    o: u32,
    exception: Except,
) -> Vec<CardId> {
    let (exc, exg) = exception.split();
    let mut group = Vec::new();
    f.filter_matching_card(
        filter,
        player,
        s,
        o,
        Some(&mut group),
        exc,
        exg,
        None,
        0,
        false,
    );
    group.sort_unstable();
    group
}

/// `Duel.SpecialSummonStep(card, sumtype, sumplayer, playerid, nocheck,
/// nolimit, positions)` — put one card onto the field, without finishing
/// (`libduel.cpp`).
///
/// The first half of a two-part summon. `SpecialSummon` is this plus
/// [`special_summon_complete`]; splitting them lets a card do something
/// **between** the monster arriving and the summon being finished — which
/// is what Call of the Haunted needs, so that it is still on the field to
/// be given a card target.
///
/// Suspends; whether the card got onto the field is read back with
/// [`resumed_value`]. An out-of-range `sumplayer` is refused, as the
/// reference refuses it. The zone mask defaults to every zone.
#[allow(clippy::too_many_arguments)]
pub fn special_summon_step(
    f: &mut Field,
    c: CardId,
    sumtype: u32,
    sumplayer: u8,
    playerid: u8,
    nocheck: bool,
    nolimit: bool,
    positions: u8,
) -> bool {
    if sumplayer >= PLAYER_NONE {
        return false;
    }
    f.special_summon_step(
        c, sumtype, sumplayer, playerid, nocheck, nolimit, positions, 0xff,
    );
    true
}

/// `Duel.SpecialSummonComplete()` — finish what
/// [`special_summon_step`] started (`libduel.cpp`).
///
/// **Returns immediately when there is nothing pending**, which is the
/// reference's own shortcut: neither `special_summoning` nor
/// `ss_tograve_set` has anything in it, so the operated set is cleared
/// and the answer is zero without queueing anything. A card that calls
/// this unconditionally — as Call of the Haunted does, outside its own
/// `if` — relies on it.
///
/// Returns whether it queued anything; when it did, the count is read
/// back with [`resumed_value`].
pub fn special_summon_complete(f: &mut Field) -> bool {
    if f.core.special_summoning.is_empty() && f.core.ss_tograve_set.is_empty() {
        f.core.operated_set.clear();
        f.core.returns.set(0);
        return false;
    }
    let (by, rp) = (f.core.reason_effect, f.core.reason_player);
    f.special_summon_complete(by, rp);
    true
}

/// `Card.SetCardTarget(other)` — record that this card's effects name
/// `other` (`libcard.cpp`).
///
/// The explicit form of the relationship [`get_first_card_target`] reads.
/// An equip sets it as a side effect of attaching; a card that wants to
/// remember something it did **not** attach to says so here.
pub fn set_card_target(f: &mut Field, c: CardId, other: CardId) {
    f.add_card_target(c, other);
}

/// The cards an event is **about** — the reference's `eg`.
///
/// A board wipe raises one event over many cards, and a card that wants
/// to know whether *its* monster is among them reads this rather than
/// looking at the board, because by the time it runs the monster is
/// already gone.
pub fn event_cards<'a>(ctx: &'a Ctx<'a>) -> &'a [CardId] {
    &ctx.event.event_cards
}

/// `Card.IsDisabled()` — is this card's effect switched off
/// (`libcard.cpp`)?
///
/// `STATUS_DISABLED` alone, which is not the same as being unable to act:
/// a card can be disabled and still be on the field, still be targeted,
/// and still leave the field.
pub fn is_disabled(f: &Field, c: CardId) -> bool {
    f.cards[c].is_status(crate::card::status::DISABLED)
}

/// `Card.IsReason(mask)` — why is this card where it is (`libcard.cpp`)?
///
/// Reads `current.reason`, the mask written when the card was last put
/// somewhere. A card destroyed and then moved again carries the *latest*
/// reason, not the interesting one — which is why the cards that ask
/// this ask it from a trigger, while the reason is still theirs.
pub fn is_reason(f: &Field, c: CardId, mask: u32) -> bool {
    f.cards[c].reason & mask != 0
}

/// `Card.GetFirstCardTarget()` — the first card *this card's* effects
/// have named as a target (`libcard.cpp`), or nothing.
///
/// Not [`get_first_target`], which reads the **chain link**. This one is
/// kept on the card and outlives the chain: an equip card's target is
/// recorded here by `field::equip`, and is still there turns later when
/// the equip card leaves the field and wants to know what it was
/// attached to.
pub fn get_first_card_target(f: &Field, c: CardId) -> Option<CardId> {
    f.cards[c].effect_target_cards.first().copied()
}

/// `Effect.GetLabelObject()` when what is stored is a **group**.
///
/// The sibling of [`get_label_object_effect`], and card-facing for the
/// same reason: a card module cannot reach `Field::effects` itself.
pub fn get_label_object_group(f: &Field, e: EffectId) -> Option<crate::field::GroupId> {
    f.effects
        .get(e)
        .and_then(get_label_object)
        .and_then(LabelObject::group)
}

/// `Effect.SetLabelObject(object)` — remember something on the effect
/// (`libeffect.cpp`).
///
/// The reference stores a Lua registry reference and accepts any object;
/// this stores a card or another effect, which is what this pool needs.
/// Passing `None` clears it, as `SetLabelObject(nil)` does.
pub fn set_label_object(f: &mut Field, e: EffectId, what: Option<LabelObject>) {
    if let Some(x) = f.effects.get_mut(e) {
        x.label_object = what;
    }
}

/// `Effect.GetLabelObject()` — what [`set_label_object`] stored.
pub fn get_label_object(e: &Effect) -> Option<LabelObject> {
    e.label_object
}

/// `Duel.Equip(player, equip_card, target, faceup, is_step)` — attach one
/// card to another (`libduel.cpp`).
///
/// Suspends: the attachment may have to move the equip card into the
/// Spell & Trap row first, and it can fail — no seat, a face-down target,
/// a target that is not on the field — so whether it happened is read
/// back with [`resumed_value`].
///
/// The reference's trailing defaults, which this pool takes: **face-up**,
/// and not a step (`is_step` batches several equips under one
/// `EquipComplete`, which nothing here does).
pub fn equip(
    f: &mut Field,
    player: u8,
    equip_card: CardId,
    target: CardId,
    faceup: bool,
    is_step: bool,
) -> bool {
    if player > 1 {
        return false;
    }
    f.equip(player, equip_card, target, faceup, is_step);
    true
}

/// `Card.IsAbleToChangeControler` — could this card change hands at all
/// (`libcard.cpp`)?
///
/// One question only: is it under a `CANNOT_CHANGE_CONTROL` effect. It
/// says nothing about room, about being on the field, or about who
/// currently controls it — every one of those is checked separately by
/// whichever operation is about to move it, which is why a script that
/// calls this still needs its own seat test.
pub fn is_able_to_change_controler(f: &mut Field, c: CardId) -> bool {
    f.is_capable_change_control(c)
}

/// `Duel.HintSelection(group, selection)` — say which cards the thing
/// about to happen is about (`libduel.cpp`).
///
/// A message, nothing more. `selection` defaults to true and picks which
/// message: `MSG_CARD_SELECTED` for a choice that has been made,
/// `MSG_BECOME_TARGET` for one that is being announced as a target. Cards
/// a script has *chosen* rather than *targeted* have no other way of
/// reaching the other side of the table, which is what every pool caller
/// wants it for.
pub fn hint_selection(f: &mut Field, cards: &[CardId], selection: bool) {
    if selection {
        let infos = cards.iter().map(|&c| f.get_info_location(c)).collect();
        f.messages
            .push(crate::field::Message::CardSelected { cards: infos });
    } else {
        f.messages.push(crate::field::Message::BecomeTarget {
            cards: cards.to_vec(),
        });
    }
}

/// `Duel.SwapControl(card1, card2, reset_phase, reset_count)` — the two
/// monsters trade sides (`libduel.cpp`).
///
/// Suspends: the exchange asks each player where to seat what they are
/// gaining, so whether it happened at all is read back with
/// [`resumed_value`].
///
/// The reference masks `reset_phase` to ten bits and takes both reset
/// arguments as zero by default, which is what every pool caller passes:
/// a control change with no reset is permanent.
pub fn swap_control(
    f: &mut Field,
    card1: CardId,
    card2: CardId,
    reset_phase: u16,
    reset_count: u8,
) {
    let (by, rp) = (f.core.reason_effect, f.core.reason_player);
    f.swap_control(by, rp, [card1], [card2], reset_phase & 0x3ff, reset_count);
}

/// `Duel.Destroy(cards, reason[, dest, reason_player])` — the export's
/// defaults: destination the graveyard, the reason player from `core`,
/// and no named player.
pub fn destroy(f: &mut Field, cards: Vec<CardId>, why: u32) {
    destroy_to(f, cards, why, u16::from(location::GRAVE));
}

/// `Duel.Destroy(cards, reason, dest)` — destroyed, but sent somewhere
/// other than a graveyard (`libduel.cpp`).
///
/// The reference's third argument, which defaults to `LOCATION_GRAVE`
/// and is the whole difference between "destroy" and "destroy and
/// banish". `field::destroy` narrows anything that is not a hand, deck
/// or banished pile back to the graveyard, so a caller cannot use this
/// to put a card somewhere destruction never sends one.
pub fn destroy_to(f: &mut Field, cards: Vec<CardId>, why: u32, dest: u16) {
    let (by, rp) = (f.core.reason_effect, f.core.reason_player);
    f.destroy(cards, by, why, rp, PLAYER_NONE, dest, 0);
}

/// `aux.Stringid(code, id)` — `(id & 0xfffff) | code << 20`
/// (`utility.lua:845`).
pub fn stringid(code_: u32, id: u32) -> u64 {
    u64::from(id & 0xfffff) | (u64::from(code_) << 20)
}

/// Whether a card is printed as a Field Spell — used by callers deciding a
/// destination, kept here so a card never reads `data.type_` directly.
pub fn is_field_spell(f: &Field, c: CardId) -> bool {
    f.cards[c].data.type_ & card_type::FIELD != 0
}

#[cfg(test)]
mod matching_tests {
    use super::*;
    use crate::board::location;
    use crate::card::{card_type, Card, CardData};

    fn monster(f: &mut Field, owner: u8, seat: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 100 + seat,
                type_: card_type::MONSTER,
                ..Default::default()
            },
            owner,
        );
        c.current.controller = owner;
        let id = f.new_card(c);
        f.add_card(owner, id, location::MZONE, seat, false);
        id
    }

    const MZ: u32 = location::MZONE as u32;

    /// **`GetMatchingGroup` is in creation order, whatever the scan
    /// order** — the reference's group is a set ordered by card id.
    #[test]
    fn get_matching_group_is_in_creation_order() {
        let mut f = Field::new(8000);
        let later = monster(&mut f, 1, 0);
        let earlier = monster(&mut f, 0, 0);
        assert!(later < earlier, "created the other way round");
        assert_eq!(
            get_matching_group(&mut f, None, 0, MZ, MZ, Except::None),
            vec![later, earlier]
        );
        assert_eq!(
            get_matching_group(&mut f, Some(&always), 0, MZ, MZ, Except::None).len(),
            2
        );
        assert_eq!(
            get_matching_group(&mut f, None, 0, MZ, 0, Except::None),
            vec![earlier]
        );
    }

    /// **`GetHandler` is the card the effect was registered on**, not the
    /// one whose script created it — the two differ for a granted effect.
    #[test]
    fn get_handler_is_the_registering_card_not_the_owner() {
        let mut f = Field::new(8000);
        let a = monster(&mut f, 0, 0);
        let b = monster(&mut f, 0, 1);
        let e = create_effect(&mut f, a);
        set_type(&mut f, e, crate::effect::effect_type::SINGLE);
        set_code(&mut f, e, crate::event::code::CANNOT_BE_EFFECT_TARGET);
        register_effect(&mut f, b, e, false);
        assert_eq!(get_handler(&f, e), Some(b));
        assert_eq!(
            f.effects.get(e).unwrap().owner,
            Some(a),
            "created by a's script"
        );
        let unregistered = create_effect(&mut f, a);
        assert_eq!(
            get_handler(&f, unregistered),
            None,
            "no handler until registered"
        );
    }

    /// **`IsExistingMatchingCard` wants `count` matches**, and the
    /// exception is a card or a group.
    #[test]
    fn is_existing_matching_card_counts_and_excepts() {
        let mut f = Field::new(8000);
        let a = monster(&mut f, 0, 0);
        let b = monster(&mut f, 0, 1);
        assert!(is_existing_matching_card(
            &mut f,
            Some(&always),
            0,
            MZ,
            0,
            2,
            Except::None
        ));
        assert!(!is_existing_matching_card(
            &mut f,
            Some(&always),
            0,
            MZ,
            0,
            3,
            Except::None
        ));
        assert!(is_existing_matching_card(
            &mut f,
            None,
            0,
            MZ,
            0,
            1,
            Except::Card(a)
        ));
        assert!(!is_existing_matching_card(
            &mut f,
            None,
            0,
            MZ,
            0,
            2,
            Except::Card(a)
        ));
        assert!(!is_existing_matching_card(
            &mut f,
            None,
            0,
            MZ,
            0,
            1,
            Except::Group(&[a, b])
        ));
        assert!(
            !is_existing_matching_card(&mut f, None, 0, 0, MZ, 1, Except::None),
            "nothing on the other side"
        );
        let none = |_: &mut Field, _: CardId| false;
        assert!(!is_existing_matching_card(
            &mut f,
            Some(&none),
            0,
            MZ,
            0,
            1,
            Except::None
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{card_type, status, Card, CardData};
    use crate::chain::Chain;
    use crate::event::Event;
    use crate::processor::Kind;

    fn field() -> Field {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        f
    }

    fn card_in(f: &mut Field, loc: u8) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 30000,
                type_: card_type::MONSTER,
                level: 4,
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

    fn activate_effect(f: &mut Field, c: CardId) -> EffectId {
        let e = create_effect(f, c);
        set_type(f, e, effect_type::ACTIVATE);
        set_code(f, e, code::FREE_CHAIN);
        e
    }

    fn link(f: &mut Field, e: EffectId) -> u16 {
        let mut ch = Chain::new(e, Event::new(0));
        let id = f.next_field_id();
        ch.chain_id = id;
        f.core.current_chain.push(ch);
        id
    }

    /// **`SetProperty` keeps the core's own low bits** (`0x4f`) and
    /// replaces the rest.
    #[test]
    fn set_property_preserves_the_low_bits() {
        let mut f = field();
        let c = card_in(&mut f, location::MZONE);
        let e = create_effect(&mut f, c);
        f.effects.get_mut(e).unwrap().flag[0] = 0x4f | flag::CLIENT_HINT;
        set_property(&mut f, e, flag::PLAYER_TARGET, 7);
        let x = f.effects.get(e).unwrap();
        assert_eq!(x.flag[0] & 0x4f, 0x4f, "the core's bits survive");
        assert_ne!(x.flag[0] & flag::PLAYER_TARGET, 0, "the new bit is set");
        assert_eq!(x.flag[0] & flag::CLIENT_HINT, 0, "the old high bit is gone");
        assert_eq!(x.flag[1], 7);
    }

    /// **`SetReset` folds both turns into a phase reset that names
    /// neither, and a count of 0 means 1.**
    #[test]
    fn set_reset_folds_the_turns_in_and_defaults_the_count() {
        let mut f = field();
        let c = card_in(&mut f, location::MZONE);
        let e = create_effect(&mut f, c);
        set_reset(&mut f, e, reset::PHASE, 0);
        let x = f.effects.get(e).unwrap();
        assert_ne!(x.reset_flag & reset::SELF_TURN, 0);
        assert_ne!(x.reset_flag & reset::OPPO_TURN, 0);
        assert_eq!(x.reset_count, 1);
        // A phase reset that already names a turn is left alone.
        set_reset(&mut f, e, reset::PHASE | reset::SELF_TURN, 2);
        let x = f.effects.get(e).unwrap();
        assert_eq!(x.reset_flag & reset::OPPO_TURN, 0);
        assert_eq!(x.reset_count, 2);
    }

    /// **An unforced registration on a card immune to the resolving
    /// effect is refused** and remembered; a forced one goes through.
    #[test]
    fn register_effect_refuses_an_unforced_registration_on_an_immune_card() {
        let mut f = field();
        let c = card_in(&mut f, location::MZONE);
        // A summoning card is immune to everything but the two
        // summon-interrupting codes.
        f.cards[c].set_status(status::SUMMONING, true);
        let reason = create_effect(&mut f, c);
        f.core.reason_effect = Some(reason);

        let e = create_effect(&mut f, c);
        set_type(&mut f, e, effect_type::SINGLE);
        assert_eq!(register_effect(&mut f, c, e, false), 0, "refused");
        assert!(f.core.reseted_effects.contains(&e), "and remembered");
        assert!(!f.cards[c].indexer.contains(&e));

        let g = create_effect(&mut f, c);
        set_type(&mut f, g, effect_type::SINGLE);
        assert!(
            register_effect(&mut f, c, g, true) > 0,
            "forced: registered"
        );
    }

    /// **While a chain resolves, `IsRelateToEffect` asks about the chain
    /// link, not the effect.** A relation to the same effect under a
    /// different chain id satisfies the C++ scan and not the library's
    /// answer — and the other way round.
    #[test]
    fn is_relate_to_effect_routes_to_the_chain_link_while_resolving() {
        let mut f = field();
        let c = card_in(&mut f, location::MZONE);
        let e = activate_effect(&mut f, c);
        let current = link(&mut f, e);
        f.core.chain_solving = true;
        f.core.reason_effect = Some(e);

        // Related to `e` — but on an OLDER chain id.
        f.cards[c].create_chain_relation(e, current.wrapping_sub(50));
        assert!(f.cards[c].has_effect_relation(e), "the C++ scan says yes");
        assert!(
            !is_relate_to_effect(&f, c, e),
            "the library asks about the current link and says no"
        );

        // Now related on the current link too.
        f.cards[c].create_chain_relation(e, current);
        assert!(is_relate_to_effect(&f, c, e));

        // Outside resolution the effect-only scan is the answer.
        f.core.chain_solving = false;
        f.cards[c].release_chain_relation(e, current);
        assert!(
            is_relate_to_effect(&f, c, e),
            "any chain id will do when not resolving"
        );
    }

    /// And when the effect is not the reason effect, the link whose
    /// triggering effect it is gets asked — found by scanning the chain.
    #[test]
    fn is_relate_to_effect_finds_the_link_by_its_triggering_effect() {
        let mut f = field();
        let c = card_in(&mut f, location::MZONE);
        let e1 = activate_effect(&mut f, c);
        let e2 = activate_effect(&mut f, c);
        let id1 = link(&mut f, e1);
        let _id2 = link(&mut f, e2);
        f.core.chain_solving = true;
        f.core.reason_effect = Some(e2);
        // Related to e1 on e1's own link: found by the scan.
        f.cards[c].create_chain_relation(e1, id1);
        assert!(is_relate_to_effect(&f, c, e1));
        // Related to e1 only under a stray id: the scan finds e1's link,
        // asks the pair, and says no.
        f.cards[c].release_chain_relation(e1, id1);
        f.cards[c].create_chain_relation(e1, 9);
        assert!(!is_relate_to_effect(&f, c, e1));
    }

    /// **`IsRelateToChain` asks the (effect, chain id) pair**, and `0`
    /// means the last link.
    #[test]
    fn is_relate_to_chain_uses_the_pair_and_zero_is_the_last_link() {
        let mut f = field();
        let c = card_in(&mut f, location::MZONE);
        let e1 = activate_effect(&mut f, c);
        let e2 = activate_effect(&mut f, c);
        let id1 = link(&mut f, e1);
        let id2 = link(&mut f, e2);
        f.cards[c].create_chain_relation(e2, id2);
        assert!(is_relate_to_chain(&f, c, 0), "0 is the last link");
        assert!(is_relate_to_chain(&f, c, 2));
        assert!(!is_relate_to_chain(&f, c, 1), "not related to link 1");
        f.cards[c].create_chain_relation(e1, id1.wrapping_add(1));
        assert!(!is_relate_to_chain(&f, c, 1), "same effect, wrong chain id");
    }

    /// **`get_chain(0)` is the last link**, and a count past the end is
    /// clamped to it.
    #[test]
    fn get_chain_zero_and_overflow_mean_the_last_link() {
        let mut f = field();
        let c = card_in(&mut f, location::MZONE);
        let e1 = activate_effect(&mut f, c);
        let e2 = activate_effect(&mut f, c);
        assert!(get_chain(&f, 0).is_none(), "no chain, no link");
        link(&mut f, e1);
        link(&mut f, e2);
        assert_eq!(get_chain(&f, 0).map(|ch| ch.triggering_effect), Some(e2));
        assert_eq!(get_chain(&f, 1).map(|ch| ch.triggering_effect), Some(e1));
        assert_eq!(get_chain(&f, 2).map(|ch| ch.triggering_effect), Some(e2));
        assert_eq!(get_chain(&f, 9).map(|ch| ch.triggering_effect), Some(e2));
    }

    /// **A card already in the hand is not able to go there.**
    #[test]
    fn is_able_to_hand_refuses_a_card_already_in_hand() {
        let mut f = field();
        let on_field = card_in(&mut f, location::MZONE);
        let in_hand = card_in(&mut f, location::HAND);
        assert!(is_able_to_hand(&mut f, on_field, Some(0)));
        assert!(!is_able_to_hand(&mut f, in_hand, Some(0)));
    }

    /// **`IsPlayerCanDraw` counts the deck.**
    #[test]
    fn is_player_can_draw_needs_the_deck_to_have_that_many() {
        let mut f = field();
        card_in(&mut f, location::DECK);
        assert!(is_player_can_draw(&mut f, 0, 1));
        assert!(
            !is_player_can_draw(&mut f, 0, 2),
            "one card cannot give two"
        );
        assert!(!is_player_can_draw(&mut f, 2, 0), "not a player");
    }

    /// **`Draw` carries the resolving effect and player.**
    #[test]
    fn draw_passes_the_reason_effect_and_player_through() {
        let mut f = field();
        let c = card_in(&mut f, location::MZONE);
        let e = activate_effect(&mut f, c);
        f.core.reason_effect = Some(e);
        f.core.reason_player = 1;
        draw(&mut f, 0, 2, reason::EFFECT);
        let queued = f
            .core
            .units
            .iter()
            .chain(f.core.subunits.iter())
            .find_map(|u| match &u.kind {
                Kind::Draw {
                    by, reason_player, ..
                } => Some((*by, *reason_player)),
                _ => None,
            });
        assert_eq!(queued, Some((Some(e), 1)));
    }

    /// **`Stringid` packs the id low and the code high.** Pinned as a
    /// literal against `utility.lua:845`.
    #[test]
    fn stringid_packs_id_low_and_code_high() {
        assert_eq!(stringid(55_144_522, 3), (55_144_522u64 << 20) | 3);
        assert_eq!(
            stringid(1, 0xfffff + 1),
            1 << 20,
            "the id is masked to twenty bits"
        );
    }
}

#[cfg(test)]
mod turn_set_and_type_tests {
    use super::*;
    use crate::board::location;
    use crate::card::{card_type, Card, CardData};

    fn monster(f: &mut Field, owner: u8, type_: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 600,
                type_,
                ..Default::default()
            },
            owner,
        );
        c.current.controller = owner;
        let id = f.new_card(c);
        f.add_card(owner, id, location::MZONE, u32::from(owner), false);
        f.cards[id].current.position = crate::position::FACEUP_ATTACK;
        id
    }

    /// **`IsCanTurnSet` asks about the player doing it**, not the card's
    /// controller. With a prohibition on one player only, the two answers
    /// differ — and nothing in the Goat pool produces such a
    /// prohibition, so only a direct test can tell.
    #[test]
    fn turn_set_is_asked_for_the_reason_player() {
        let mut f = Field::new(8000);
        let theirs = monster(&mut f, 1, card_type::MONSTER);
        // Forbid player 1 from turning things face-down: registered to
        // player 0 with the range naming the opponent.
        let e = create_effect(&mut f, theirs);
        set_type(&mut f, e, effect_type::FIELD);
        set_code(&mut f, e, code::CANNOT_TURN_SET);
        set_property(&mut f, e, flag::PLAYER_TARGET, 0);
        set_target_range(&mut f, e, 0, 1);
        duel_register_effect(&mut f, e, 0);

        f.core.reason_player = 0;
        assert!(
            is_can_turn_set(&mut f, theirs),
            "player 0 may turn it, though its controller may not"
        );
        f.core.reason_player = 1;
        assert!(!is_can_turn_set(&mut f, theirs), "and player 1 may not");
    }

    /// **`IsMonster` is the effective type**, as `IsSpellTrap` is: a
    /// Spell treated as a monster answers yes.
    #[test]
    fn is_monster_reads_the_effective_type() {
        let mut f = Field::new(8000);
        let c = monster(&mut f, 0, card_type::SPELL);
        assert!(!is_monster(&mut f, c), "printed a Spell");
        f.cards[c]
            .assume
            .insert(crate::card::assume::TYPE, u64::from(card_type::MONSTER));
        assert!(is_monster(&mut f, c), "and assumed a monster");
    }
}

#[cfg(test)]
mod is_code_tests {
    use super::*;
    use crate::board::location;
    use crate::card::{card_type, Card, CardData};
    use crate::effect::Effect;

    fn monster(f: &mut Field, code_: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: code_,
                type_: card_type::MONSTER,
                ..Default::default()
            },
            0,
        );
        c.current.controller = 0;
        let id = f.new_card(c);
        f.add_card(0, id, location::MZONE, 0, false);
        f.cards[id].current.position = crate::position::FACEUP_ATTACK;
        id
    }

    /// **A card is its printed code.**
    #[test]
    fn a_card_is_its_printed_code() {
        let mut f = Field::new(8000);
        let c = monster(&mut f, 1234);
        assert!(is_code(&mut f, c, 1234));
        assert!(!is_code(&mut f, c, 5678));
    }

    /// **And its second name, when it has one.** `EFFECT_ADD_CODE` gives
    /// a card a name it also answers to, and `Card.IsCode` compares
    /// **both** — reading only the first is the mistake, and one no pool
    /// card could reveal, since none of them grants a second name.
    #[test]
    fn a_card_is_also_its_second_name() {
        let mut f = Field::new(8000);
        let c = monster(&mut f, 1234);
        let mut e = Effect::new(effect_type::SINGLE, code::ADD_CODE);
        e.owner = Some(c);
        e.handler = Some(c);
        e.value = 5678;
        let e = f.new_effect(e);
        f.cards[c].single_effect.insert(code::ADD_CODE, e);
        assert!(is_code(&mut f, c, 1234), "still the printed one");
        assert!(is_code(&mut f, c, 5678), "and the granted one");
        assert!(!is_code(&mut f, c, 9999), "and nothing else");
    }
}

#[cfg(test)]
mod select_target_tests {
    use super::*;
    use crate::board::location;
    use crate::card::{card_type, status, Card, CardData};
    use crate::chain::Chain;
    use crate::event::Event;
    use crate::field::Message;

    fn trap(f: &mut Field, owner: u8, seat: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 800 + seat,
                type_: card_type::TRAP,
                ..Default::default()
            },
            owner,
        );
        c.current.controller = owner;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(owner, id, location::SZONE, seat, false);
        f.cards[id].current.position = crate::position::FACEUP;
        id
    }

    /// A field with a chain link whose effect `tweak` shapes, and three
    /// Spell/Traps of the opponent's to aim at.
    fn with_link(tweak: impl FnOnce(&mut Effect)) -> (Field, Vec<CardId>, EffectId) {
        let mut f = Field::new(8000);
        let owner = trap(&mut f, 0, 0);
        let targets: Vec<_> = (0..3).map(|i| trap(&mut f, 1, i)).collect();
        let mut e = Effect::new(effect_type::ACTIVATE, code::FREE_CHAIN);
        e.owner = Some(owner);
        e.handler = Some(owner);
        e.flag[0] |= flag::CARD_TARGET;
        tweak(&mut e);
        let e = f.new_effect(e);
        let mut ch = Chain::new(e, Event::new(code::FREE_CHAIN));
        ch.triggering_player = 0;
        ch.chain_count = 1;
        ch.chain_id = 5;
        f.core.current_chain.push(ch);
        f.core.reason_effect = Some(e);
        f.core.reason_player = 0;
        (f, targets, e)
    }

    const SZ: u32 = location::SZONE as u32;

    /// **The selection is a *targeting* scan.** A card that matches the
    /// filter but may not be targeted is not offered — which is the whole
    /// difference between this and `GetMatchingGroup`.
    #[test]
    fn it_offers_only_what_may_be_targeted() {
        let (mut f, targets, _) = with_link(|_| {});
        assert!(select_target(&mut f, 0, None, 0, 0, SZ, 1, 1, Except::None));
        assert_eq!(f.core.select_cards.len(), 3, "all three to begin with");

        let (mut f, targets2, _) = with_link(|_| {});
        // Battle-destroyed: still on the field, no longer targetable.
        f.cards[targets2[1]].status |= status::BATTLE_DESTROYED;
        assert!(select_target(&mut f, 0, None, 0, 0, SZ, 1, 1, Except::None));
        assert_eq!(f.core.select_cards.len(), 2, "the untargetable one is gone");
        assert!(!f.core.select_cards.contains(&targets2[1]));
        let _ = targets;
    }

    /// **`IsExistingTarget` asks the same question**, and answers no when
    /// the only candidates cannot be targeted.
    #[test]
    fn existing_target_counts_only_the_targetable() {
        let (mut f, targets, _) = with_link(|_| {});
        assert!(is_existing_target(&mut f, None, 0, 0, SZ, 1, Except::None));
        for &t in &targets {
            f.cards[t].status |= status::BATTLE_DESTROYED;
        }
        assert!(
            !is_existing_target(&mut f, None, 0, 0, SZ, 1, Except::None),
            "matches, but none may be targeted"
        );
    }

    /// **With no chain link there is nothing to target for**, and the
    /// export returns early rather than queuing a question nobody asked.
    #[test]
    fn it_does_nothing_without_a_chain_link() {
        let (mut f, _, _) = with_link(|_| {});
        f.core.current_chain.clear();
        assert!(!select_target(
            &mut f,
            0,
            None,
            0,
            0,
            SZ,
            1,
            1,
            Except::None
        ));
        assert!(f.core.subunits.is_empty(), "nothing queued");
        // And a player that is not a player.
        let (mut f, _, _) = with_link(|_| {});
        assert!(!select_target(
            &mut f,
            2,
            None,
            0,
            0,
            SZ,
            1,
            1,
            Except::None
        ));
    }

    /// **A cancelled selection targets nothing**, and leaves the link
    /// untouched.
    #[test]
    fn a_cancelled_selection_targets_nothing() {
        let (mut f, _, _) = with_link(|_| {});
        f.core.return_cards.canceled = true;
        assert_eq!(selected_targets(&mut f), None);
        assert!(f.core.current_chain[0].target_cards.is_empty());
    }

    /// **What was chosen is recorded, related and announced.**
    #[test]
    fn the_chosen_are_recorded_related_and_announced() {
        let (mut f, targets, e) = with_link(|_| {});
        f.core.return_cards.list = vec![targets[2]];
        assert_eq!(selected_targets(&mut f), Some(vec![targets[2]]));
        assert_eq!(f.core.current_chain[0].target_cards, vec![targets[2]]);
        assert!(f.cards[targets[2]].has_chain_relation(e, 5));
        assert!(f
            .messages
            .iter()
            .any(|m| matches!(m, Message::BecomeTarget { .. })));
    }

    /// **A continuous effect relates nothing and says nothing**, and
    /// hands back the whole accumulated set rather than the new pick —
    /// the same exemption `SetTargetCard` makes.
    #[test]
    fn a_continuous_effect_neither_relates_nor_announces() {
        let (mut f, targets, e) = with_link(|x| x.effect_type |= effect_type::CONTINUOUS);
        f.core.return_cards.list = vec![targets[0]];
        let first = selected_targets(&mut f);
        assert_eq!(first, Some(vec![targets[0]]));
        f.core.return_cards.list = vec![targets[1]];
        assert_eq!(
            selected_targets(&mut f),
            Some(vec![targets[0], targets[1]]),
            "the whole set, not just the new pick"
        );
        assert!(!f.cards[targets[0]].has_chain_relation(e, 5), "no relation");
        assert!(
            !f.messages
                .iter()
                .any(|m| matches!(m, Message::BecomeTarget { .. })),
            "and silent"
        );
    }

    /// **`Duel.Hint` addresses `HINT_OPSELECTED` to the *other*
    /// player.** The one asymmetry in an otherwise plain message, and
    /// invisible in every other kind.
    #[test]
    fn an_op_selected_hint_goes_to_the_other_player() {
        let (mut f, _, _) = with_link(|_| {});
        hint(&mut f, crate::host_question::hint::SELECTMSG, 0, 1);
        hint(&mut f, crate::host_question::hint::OPSELECTED, 0, 2);
        let addressed: Vec<_> = f
            .messages
            .iter()
            .filter_map(|m| match m {
                Message::Hint { player, value, .. } => Some((*player, *value)),
                _ => None,
            })
            .collect();
        assert_eq!(
            addressed,
            vec![(0, 1), (1, 2)],
            "the select message to the chooser, the op-selected to the other"
        );
        // A player that is not a player is left alone.
        let before = f.messages.len();
        hint(&mut f, crate::host_question::hint::SELECTMSG, 2, 3);
        assert_eq!(f.messages.len(), before);
    }
}

#[cfg(test)]
mod waiting_tests {
    use super::*;
    use crate::board::location;
    use crate::card::{card_type, Card, CardData};
    use crate::field::Message;

    fn hand_of(n: u32) -> (Field, Vec<CardId>) {
        let mut f = Field::new(8000);
        let cards = (0..n)
            .map(|i| {
                let mut c = Card::with_data(
                    CardData {
                        code: 700 + i,
                        type_: card_type::MONSTER,
                        ..Default::default()
                    },
                    0,
                );
                c.current.controller = 0;
                let id = f.new_card(c);
                f.add_card(0, id, location::HAND, i, false);
                id
            })
            .collect();
        (f, cards)
    }

    /// **`BreakEffect` closes the timing window.** Everything but the two
    /// damage-step timings is cleared, which is what stops a trigger that
    /// was waiting for this window from still being offered.
    #[test]
    fn break_effect_closes_the_window() {
        let (mut f, _) = hand_of(1);
        f.core.hint_timing = [0xffff_ffff, 0xffff_ffff];
        break_effect(&mut f);
        let survivors = crate::field::timing::DAMAGE_STEP | crate::field::timing::DAMAGE_CAL;
        assert_eq!(f.core.hint_timing[0] & !survivors, 0, "the window is shut");
        assert_eq!(f.core.hint_timing[1] & !survivors, 0);
    }

    /// **`DiscardHand` filters first, and does nothing at all when
    /// nothing matches.** The export returns 0 without queuing, which is
    /// what lets a card skip waiting for work that will never happen.
    #[test]
    fn discard_hand_does_nothing_with_no_candidates() {
        let (mut f, _) = hand_of(0);
        let queued = discard_hand(
            &mut f,
            0,
            None,
            2,
            2,
            crate::card::reason::EFFECT,
            Except::None,
        );
        assert!(!queued, "nothing to discard");
        assert!(f.core.subunits.is_empty(), "and nothing queued");

        let none = |_: &mut Field, _: CardId| false;
        let (mut f, _) = hand_of(3);
        let queued = discard_hand(
            &mut f,
            0,
            Some(&none),
            2,
            2,
            crate::card::reason::EFFECT,
            Except::None,
        );
        assert!(!queued, "a hand that matches nothing is the same as none");
        assert!(f.core.subunits.is_empty());
    }

    /// **With candidates it publishes them and queues the unit.** The
    /// unit asks for a selection out of `core.select_cards`, so a caller
    /// that queued without filling it would ask about nothing.
    #[test]
    fn discard_hand_publishes_its_candidates() {
        let (mut f, cards) = hand_of(3);
        let queued = discard_hand(
            &mut f,
            0,
            None,
            2,
            2,
            crate::card::reason::EFFECT,
            Except::None,
        );
        assert!(queued);
        assert_eq!(f.core.select_cards, cards, "every hand card is a candidate");
        assert_eq!(f.core.subunits.len(), 1, "and the unit is queued");
    }

    /// **`ShuffleHand` shuffles, and says so.** It runs even under
    /// `DUEL_PSEUDO_SHUFFLE` — a hand is hidden from its owner too — and
    /// announces itself, which is how a card can be held to having done
    /// it without depending on the order that came out.
    #[test]
    fn shuffle_hand_announces_itself() {
        let (mut f, _) = hand_of(4);
        shuffle_hand(&mut f, 0);
        assert!(
            f.messages
                .iter()
                .any(|m| matches!(m, Message::ShuffleHand { .. })),
            "the shuffle was announced"
        );
        // And a player that does not exist is left alone.
        let before = f.messages.len();
        shuffle_hand(&mut f, 2);
        assert_eq!(f.messages.len(), before);
    }
}

#[cfg(test)]
mod target_card_tests {
    use super::*;
    use crate::board::location;
    use crate::card::{card_type, status, Card, CardData};
    use crate::chain::Chain;
    use crate::event::Event;
    use crate::field::Message;

    fn monster(f: &mut Field, owner: u8, seat: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 100 + seat,
                type_: card_type::MONSTER,
                ..Default::default()
            },
            owner,
        );
        c.current.controller = owner;
        let id = f.new_card(c);
        f.add_card(owner, id, location::MZONE, seat, false);
        id
    }

    /// A field with three monsters and one chain link, whose effect is
    /// shaped by `tweak`.
    fn with_link(tweak: impl FnOnce(&mut Effect)) -> (Field, Vec<CardId>, EffectId) {
        let mut f = Field::new(8000);
        let cards: Vec<_> = (0..3).map(|i| monster(&mut f, 0, i)).collect();
        let mut e = Effect::new(effect_type::ACTIVATE, code::FREE_CHAIN);
        e.owner = Some(cards[0]);
        e.handler = Some(cards[0]);
        e.flag[0] |= flag::CARD_TARGET;
        tweak(&mut e);
        let e = f.new_effect(e);
        let mut ch = Chain::new(e, Event::new(code::FREE_CHAIN));
        ch.triggering_player = 0;
        ch.chain_count = 1;
        ch.chain_id = 7;
        f.core.current_chain.push(ch);
        (f, cards, e)
    }

    fn announced(f: &Field) -> Vec<Vec<CardId>> {
        f.messages
            .iter()
            .filter_map(|m| match m {
                Message::BecomeTarget { cards } => Some(cards.clone()),
                _ => None,
            })
            .collect()
    }

    /// **The ordinary case: recorded, related, announced.**
    #[test]
    fn it_records_relates_and_announces() {
        let (mut f, cards, e) = with_link(|_| {});
        set_target_card(&mut f, vec![cards[1], cards[2]]);
        assert_eq!(
            f.core.current_chain[0].target_cards,
            vec![cards[1], cards[2]]
        );
        assert!(f.cards[cards[1]].has_chain_relation(e, 7));
        assert!(f.cards[cards[2]].has_chain_relation(e, 7));
        assert_eq!(announced(&f), vec![vec![cards[1], cards[2]]]);
    }

    /// **Without `EFFECT_FLAG_CARD_TARGET` nothing is announced** — but
    /// the cards are still recorded and still related. Only the message
    /// is conditional.
    #[test]
    fn without_the_flag_it_records_but_does_not_announce() {
        let (mut f, cards, e) = with_link(|x| x.flag[0] &= !flag::CARD_TARGET);
        set_target_card(&mut f, vec![cards[1]]);
        assert_eq!(f.core.current_chain[0].target_cards, vec![cards[1]]);
        assert!(f.cards[cards[1]].has_chain_relation(e, 7), "still related");
        assert!(announced(&f).is_empty(), "and silent");
    }

    /// **A continuous effect replaces the set and takes no relation**,
    /// and says nothing — all three halves of the asymmetry at once.
    #[test]
    fn a_continuous_effect_replaces_the_set_without_relating() {
        let (mut f, cards, e) = with_link(|x| x.effect_type |= effect_type::CONTINUOUS);
        set_target_card(&mut f, vec![cards[1]]);
        set_target_card(&mut f, vec![cards[2]]);
        assert_eq!(
            f.core.current_chain[0].target_cards,
            vec![cards[2]],
            "replaced, not appended"
        );
        assert!(!f.cards[cards[1]].has_chain_relation(e, 7), "no relation");
        assert!(!f.cards[cards[2]].has_chain_relation(e, 7));
        assert!(announced(&f).is_empty());
    }

    /// **`GetFirstTarget` is the first of them**, which only a set of
    /// more than one can show.
    #[test]
    fn the_first_target_is_the_first() {
        let (mut f, cards, _) = with_link(|_| {});
        assert_eq!(get_first_target(&f), None, "nothing named yet");
        set_target_card(&mut f, vec![cards[2], cards[1]]);
        assert_eq!(get_first_target(&f), Some(cards[2]), "the first named");
    }

    /// **`IsOnField` wants the card settled there.** Each of the four
    /// marks the reference lists refuses it on its own.
    #[test]
    fn on_field_refuses_a_card_still_arriving() {
        let (mut f, cards, _) = with_link(|_| {});
        let c = cards[1];
        assert!(is_on_field(&f, c));
        for mark in [
            status::SUMMONING,
            status::SUMMON_DISABLED,
            status::ACTIVATE_DISABLED,
            status::SPSUMMON_STEP,
        ] {
            f.cards[c].status = mark;
            assert!(!is_on_field(&f, c), "mark {mark:#x}");
        }
        f.cards[c].status = 0;
        // Battle-destroyed is *not* one of them: still on the field.
        f.cards[c].status = status::BATTLE_DESTROYED;
        assert!(is_on_field(&f, c));
    }

    /// **`Card.IsStatus` is the any-bit reader.** With one bit the two
    /// readers agree, which is why only a two-bit mask can tell them
    /// apart — and why every caller in the pool would have hidden a
    /// mistake here.
    #[test]
    fn is_status_is_any_bit_not_all_bit() {
        let (mut f, cards, _) = with_link(|_| {});
        let c = cards[1];
        f.cards[c].status = status::ATTACK_CANCELED;
        let both = status::ATTACK_CANCELED | status::BATTLE_DESTROYED;
        assert!(is_status(&f, c, status::ATTACK_CANCELED), "the bit it has");
        assert!(
            is_status(&f, c, both),
            "any bit of the mask is enough — the all-bit reader would say no"
        );
        assert!(
            !is_status(&f, c, status::BATTLE_DESTROYED),
            "a bit it lacks"
        );
    }

    /// **`Card.IsCanBeEffectTarget` and `IsOnField` are different
    /// questions.** A battle-destroyed monster is still on the field and
    /// is no longer targetable, which is the pair that separates them.
    #[test]
    fn on_field_and_targetable_are_different_questions() {
        let (mut f, cards, e) = with_link(|_| {});
        let c = cards[1];
        assert!(is_on_field(&f, c) && is_can_be_effect_target(&f, c, e));
        f.cards[c].status = status::BATTLE_DESTROYED;
        assert!(is_on_field(&f, c), "still on the field");
        assert!(!is_can_be_effect_target(&f, c, e), "but not targetable");
    }
}

#[cfg(test)]
mod group_tests {
    use super::*;
    use crate::board::location;
    use crate::card::{card_type, status, Card, CardData};
    use crate::field::Message;

    fn monster(f: &mut Field, owner: u8, seat: u32, atk: i32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 900 + seat + u32::from(owner) * 100,
                type_: card_type::MONSTER | card_type::NORMAL,
                level: 4,
                attack: atk,
                defense: 1000,
                ..Default::default()
            },
            owner,
        );
        c.current.controller = owner;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(owner, id, location::MZONE, seat, false);
        f.cards[id].current.position = crate::position::FACEUP_ATTACK;
        id
    }

    /// **`IsControler` reads the current controller**, not the owner.
    /// A card played under someone else's control answers for where it
    /// is now, which is the whole point of the export.
    #[test]
    fn is_controler_reads_the_current_controller() {
        let mut f = Field::new(8000);
        let c = monster(&mut f, 0, 0, 1700);
        assert!(is_controler(&f, c, 0));
        assert!(!is_controler(&f, c, 1));
        f.cards[c].current.controller = 1;
        assert!(is_controler(&f, c, 1), "control changed, the answer does");
        assert!(!is_controler(&f, c, 0));
        assert_eq!(f.cards[c].owner, 0, "the owner did not change");
    }

    /// **`GetMaxGroup` keeps every tie**, and a strictly greater value
    /// clears what came before it. Both halves matter: keeping only the
    /// first maximum, or accumulating on `>=` as well, would each pass a
    /// board where the maximum happens to be unique.
    #[test]
    fn get_max_group_keeps_every_tie_and_clears_on_a_greater_one() {
        let mut f = Field::new(8000);
        let a = monster(&mut f, 0, 0, 1700);
        let b = monster(&mut f, 0, 1, 1900);
        let c = monster(&mut f, 0, 2, 1900);
        let d = monster(&mut f, 0, 3, 1200);
        let g = vec![a, b, c, d];
        let mut want = vec![b, c];
        want.sort_unstable();
        assert_eq!(get_max_group(&mut f, &g, &attack_value), want);
        // The maximum arriving first, so the clearing branch never runs.
        let g2 = vec![b, c, a, d];
        assert_eq!(get_max_group(&mut f, &g2, &attack_value), want);
        // A unique maximum is a group of one.
        let mut f2 = Field::new(8000);
        let p = monster(&mut f2, 0, 0, 1700);
        let q = monster(&mut f2, 0, 1, 1900);
        let r = monster(&mut f2, 0, 2, 1500);
        assert_eq!(get_max_group(&mut f2, &[p, q, r], &attack_value), vec![q]);
    }

    /// **An empty group gives an empty one back.** The reference pushes
    /// nothing at all there; every caller guards first, and the port
    /// answers with a group so the guard is the caller's either way.
    #[test]
    fn get_max_group_of_nothing_is_nothing() {
        let mut f = Field::new(8000);
        assert!(get_max_group(&mut f, &[], &attack_value).is_empty());
    }

    /// **The maximum is seeded from the first member, not from zero.**
    /// The operation is any `int64` function — the reference calls it
    /// through `get_operation_value`, which can perfectly well return a
    /// difference or a negative — so a zero seed with the reference's
    /// strict `>` drops every member of an all-negative group and
    /// answers nothing.
    #[test]
    fn get_max_group_seeds_from_the_first_member() {
        let mut f = Field::new(8000);
        let a = monster(&mut f, 0, 0, 1500);
        let b = monster(&mut f, 0, 1, 1600);
        let c = monster(&mut f, 0, 2, 1600);
        // A value that is negative everywhere on this board.
        let below = |f: &mut Field, id: CardId| i64::from(get_attack(f, id)) - 1700;
        let mut want = vec![b, c];
        want.sort_unstable();
        assert_eq!(get_max_group(&mut f, &[a, b, c], &below), want);
    }

    /// **A group whose maximum is zero is still the whole group**, which
    /// is the same rule where the values happen to be attacks: a board of
    /// zero-attack monsters is an ordinary board.
    #[test]
    fn get_max_group_of_zero_attacks_is_the_whole_group() {
        let mut f = Field::new(8000);
        let a = monster(&mut f, 0, 0, 0);
        let b = monster(&mut f, 0, 1, 0);
        let c = monster(&mut f, 0, 2, 0);
        let mut want = vec![a, b, c];
        want.sort_unstable();
        assert_eq!(get_max_group(&mut f, &[a, b, c], &attack_value), want);
    }

    /// **`Group.Select` asks over the group it was given**, not over a
    /// scan of the field: a monster of the same shape that is not in the
    /// group is not offered.
    #[test]
    fn group_select_asks_over_the_group_it_was_given() {
        let mut f = Field::new(8000);
        let a = monster(&mut f, 0, 0, 1700);
        let b = monster(&mut f, 0, 1, 1700);
        let _absent = monster(&mut f, 0, 2, 1700);
        assert!(group_select(&mut f, &[a, b], 0, 1, 1, Except::None));
        let mut want = vec![a, b];
        want.sort_unstable();
        assert_eq!(f.core.select_cards, want);
        assert!(matches!(f.process(), crate::processor::Status::Awaiting));
        match f.messages.last() {
            Some(Message::SelectCard {
                player,
                cancelable,
                min,
                max,
                cards,
            }) => {
                assert_eq!(*player, 0);
                assert!(!cancelable, "the export's cancelable defaults to false");
                assert_eq!((*min, *max), (1, 1));
                assert_eq!(cards, &want);
            }
            other => panic!("expected a card question: {other:?}"),
        }
    }

    /// **The exception is removed from the group**, a card or a group of
    /// them, and an out-of-range player is refused outright.
    #[test]
    fn group_select_honours_the_exception_and_the_player() {
        let mut f = Field::new(8000);
        let a = monster(&mut f, 0, 0, 1700);
        let b = monster(&mut f, 0, 1, 1700);
        let c = monster(&mut f, 0, 2, 1700);
        assert!(group_select(&mut f, &[a, b, c], 0, 1, 1, Except::Card(b)));
        let mut want = vec![a, c];
        want.sort_unstable();
        assert_eq!(f.core.select_cards, want);
        let group = vec![a, c];
        assert!(group_select(
            &mut f,
            &[a, b, c],
            1,
            1,
            1,
            Except::Group(&group)
        ));
        assert_eq!(f.core.select_cards, vec![b]);
        f.core.select_cards.clear();
        assert!(
            !group_select(&mut f, &[a, b, c], 2, 1, 1, Except::None),
            "there is no player 2"
        );
        assert!(f.core.select_cards.is_empty(), "and nothing was queued");
    }

    /// **The answer comes back as a group**, and a cancelled selection
    /// is an *empty* group rather than nothing — the reference builds a
    /// fresh one unless the call was cancelable, and these are not.
    #[test]
    fn group_selected_reads_the_answer_and_an_empty_one_for_a_cancel() {
        let mut f = Field::new(8000);
        let a = monster(&mut f, 0, 0, 1700);
        let b = monster(&mut f, 0, 1, 1700);
        f.core.return_cards.clear();
        f.core.return_cards.list = vec![a, b];
        assert_eq!(group_selected(&f), vec![a, b]);
        f.core.return_cards.canceled = true;
        assert!(
            group_selected(&f).is_empty(),
            "a cancel is an empty group, not the list"
        );
    }
}

#[cfg(test)]
mod set_and_ask_tests {
    use super::*;
    use crate::board::location;
    use crate::card::{card_type, status, Card, CardData};

    fn card(f: &mut Field, owner: u8, type_: u32, loc: u8, seq: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 700 + seq + u32::from(owner) * 50,
                type_,
                ..Default::default()
            },
            owner,
        );
        c.current.controller = owner;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(owner, id, loc, seq, false);
        id
    }

    /// **`SelectYesNo` refuses a player who does not exist** rather than
    /// queueing a question nobody can answer.
    #[test]
    fn select_yes_no_refuses_an_out_of_range_player() {
        let mut f = Field::new(8000);
        assert!(select_yes_no(&mut f, 0, 42));
        assert_eq!(f.core.subunits.len(), 1, "queued for player 0");
        assert!(select_yes_no(&mut f, 1, 42));
        assert_eq!(f.core.subunits.len(), 2);
        assert!(!select_yes_no(&mut f, 2, 42), "there is no player 2");
        assert_eq!(f.core.subunits.len(), 2, "and nothing was queued");
    }

    /// **`IsSSetable` asks for the reason player by default**, not the
    /// card's controller. The two differ whenever one player's effect is
    /// deciding about another player's card, and the reference's default
    /// argument is `core.reason_player`.
    #[test]
    fn is_ssetable_defaults_to_the_reason_player() {
        let mut f = Field::new(8000);
        let c = card(&mut f, 0, card_type::TRAP, location::HAND, 0);
        // Fill player 1's Spell/Trap row so that asking *for player 1*
        // is a different question from asking for player 0.
        for i in 0..5 {
            card(&mut f, 1, card_type::TRAP, location::SZONE, i);
        }
        f.core.reason_player = 0;
        assert!(
            is_ssetable(&mut f, c, false, None),
            "room on player 0's row"
        );
        f.core.reason_player = 1;
        assert!(
            !is_ssetable(&mut f, c, false, None),
            "player 1 has nowhere to put it, and it is player 1 who is asked"
        );
        // The explicit argument overrides the default in both directions.
        assert!(is_ssetable(&mut f, c, false, Some(0)));
        f.core.reason_player = 0;
        assert!(!is_ssetable(&mut f, c, false, Some(1)));
        // And `ignore_field` skips the seat question entirely.
        assert!(is_ssetable(&mut f, c, true, Some(1)), "ignoring the field");
    }

    /// **`SSet` queues nothing for an empty group, refuses a player who
    /// does not exist, falls back to the setting player for an
    /// out-of-range destination, and carries the reason effect.**
    #[test]
    fn sset_guards_its_arguments_and_carries_the_reason_effect() {
        let mut f = Field::new(8000);
        let a = card(&mut f, 0, card_type::TRAP, location::HAND, 0);
        assert!(!sset(&mut f, 0, Vec::new(), None, false), "an empty group");
        assert!(f.core.subunits.is_empty(), "queues nothing");
        assert!(!sset(&mut f, 2, vec![a], None, false), "no player 2");
        assert!(f.core.subunits.is_empty());

        let e = f.new_effect(crate::effect::Effect::new(
            effect_type::ACTIVATE,
            code::FREE_CHAIN,
        ));
        f.core.reason_effect = Some(e);
        assert!(sset(&mut f, 0, vec![a], Some(9), false));
        match &f.core.subunits[0].kind {
            crate::processor::Kind::SpellSetGroup {
                setplayer,
                toplayer,
                targets,
                confirm,
                reason_effect,
                ..
            } => {
                assert_eq!(*setplayer, 0);
                assert_eq!(
                    *toplayer, 0,
                    "an out-of-range destination falls back to the setting player"
                );
                assert_eq!(targets, &vec![a]);
                assert!(!confirm);
                assert_eq!(*reason_effect, Some(e), "the reason effect rides along");
            }
            other => panic!("expected a SpellSetGroup: {other:?}"),
        }
        // And a destination that *is* in range is honoured.
        f.core.subunits.clear();
        assert!(sset(&mut f, 0, vec![a], Some(1), false));
        match &f.core.subunits[0].kind {
            crate::processor::Kind::SpellSetGroup { toplayer, .. } => assert_eq!(*toplayer, 1),
            other => panic!("expected a SpellSetGroup: {other:?}"),
        }
    }
}

#[cfg(test)]
mod reveal_tests {
    use super::*;
    use crate::board::location;
    use crate::card::{card_type, reason, status, Card, CardData};
    use crate::effect::Effect;
    use crate::field::Message;

    fn card(f: &mut Field, owner: u8, type_: u32, loc: u8, seq: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 400 + seq + u32::from(owner) * 50,
                type_,
                ..Default::default()
            },
            owner,
        );
        c.current.controller = owner;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(owner, id, loc, seq, false);
        f.cards[id].current.position = crate::position::FACEUP;
        id
    }

    /// A continuous single effect on `c` listening for `event`, so a test
    /// can see whether the event was actually raised at that card. A
    /// raised single event that finds a listener becomes a queued
    /// `SolveContinuous`; one that finds nothing leaves no trace at all,
    /// which is why the listener has to exist to observe it.
    fn listen(f: &mut Field, c: CardId, event: u32) {
        // `ACTIONS` is required: `single_event_for` refuses anything
        // without it before looking at the type at all.
        let mut e = Effect::new(
            effect_type::SINGLE | effect_type::ACTIONS | effect_type::CONTINUOUS,
            event,
        );
        e.owner = Some(c);
        e.handler = Some(c);
        let id = f.new_effect(e);
        f.cards[c].single_effect.insert(event, id);
    }

    fn solve_continuous_count(f: &Field) -> usize {
        f.core
            .subunits
            .iter()
            .filter(|u| matches!(u.kind, crate::processor::Kind::SolveContinuous { .. }))
            .count()
    }

    /// **`IsSpell` is Spells only**, where its sibling takes either.
    #[test]
    fn is_spell_does_not_take_traps() {
        let mut f = Field::new(8000);
        let spell = card(&mut f, 0, card_type::SPELL, location::GRAVE, 0);
        let trap = card(&mut f, 0, card_type::TRAP, location::GRAVE, 1);
        let monster = card(&mut f, 0, card_type::MONSTER, location::GRAVE, 2);
        assert!(is_spell(&mut f, spell));
        assert!(!is_spell(&mut f, trap), "a Trap is not a Spell");
        assert!(!is_spell(&mut f, monster));
        assert!(is_spell_trap(&mut f, trap), "but the pair takes it");
    }

    /// **`SendtoHand` goes to the owner's hand, and lands face-down
    /// there**, and refuses a player who cannot exist.
    ///
    /// The export asks for `POS_FACEUP`, and `add_card` overrules it: a
    /// card entering a hand is face-down unless it is public. The
    /// distinction is not cosmetic — a face-up card in a hand is what the
    /// Adjust step shuffles a hand *for*.
    #[test]
    fn send_to_hand_goes_face_up_to_the_owners_hand() {
        let mut f = Field::new(8000);
        // Owned by player 1, sitting in player 0's graveyard under player
        // 0's control — the only board on which "the owner's hand" and
        // "the controller's hand" are different places.
        let mut owned_by_one = Card::with_data(
            CardData {
                code: 4_242,
                type_: card_type::SPELL,
                ..Default::default()
            },
            1,
        );
        owned_by_one.current.controller = 0;
        owned_by_one.set_status(status::EFFECT_ENABLED, true);
        let c = f.new_card(owned_by_one);
        f.add_card(0, c, location::GRAVE, 0, false);
        f.cards[c].current.position = crate::position::FACEUP;
        assert!(send_to_hand(&mut f, vec![c], None, reason::EFFECT));
        for _ in 0..512 {
            if matches!(f.process(), crate::processor::Status::End) {
                break;
            }
        }
        assert_eq!(f.cards[c].current.location, location::HAND);
        assert_eq!(f.cards[c].current.controller, 1, "its owner's hand");
        assert_eq!(
            f.cards[c].current.position,
            crate::position::FACEDOWN,
            "face-down in the hand, whatever the export asked for"
        );

        let mut f = Field::new(8000);
        let c = card(&mut f, 0, card_type::SPELL, location::GRAVE, 0);
        assert!(
            !send_to_hand(&mut f, vec![c], Some(9), reason::EFFECT),
            "a player above PLAYER_NONE is refused"
        );
        assert!(f.core.subunits.is_empty(), "and nothing was queued");
    }

    /// **`ConfirmCards` refuses an empty group and an impossible
    /// player**, and otherwise announces to the player named.
    #[test]
    fn confirm_cards_guards_its_arguments() {
        let mut f = Field::new(8000);
        let c = card(&mut f, 0, card_type::SPELL, location::GRAVE, 0);
        assert!(!confirm_cards(&mut f, 0, Vec::new()), "an empty group");
        assert!(f.messages.is_empty());
        assert!(!confirm_cards(&mut f, 2, vec![c]), "there is no player 2");
        assert!(f.messages.is_empty());
        assert!(confirm_cards(&mut f, 1, vec![c]));
        let code_of = f.cards[c].data.code;
        assert!(f.messages.iter().any(|m| matches!(
            m,
            Message::ConfirmCards { player: 1, codes } if codes == &vec![code_of]
        )));
    }

    /// **The reason depends on whether a chain is solving** — `EFFECT`
    /// inside a resolution, `COST` outside it. The same call means two
    /// different things, and the event carries the difference.
    #[test]
    fn the_confirm_reason_follows_the_chain() {
        for (solving, want) in [(false, reason::COST), (true, reason::EFFECT)] {
            let mut f = Field::new(8000);
            let c = card(&mut f, 0, card_type::SPELL, location::GRAVE, 0);
            listen(&mut f, c, code::CONFIRM);
            f.core.chain_solving = solving;
            assert!(confirm_cards(&mut f, 1, vec![c]));
            let chain = f
                .core
                .sub_solving_continuous
                .front()
                .unwrap_or_else(|| panic!("the CONFIRM event reached the listener"));
            assert_eq!(chain.evt.reason, want, "chain_solving = {solving}");
            // And the revealing player is the *other* one: the argument
            // names who sees, the event names who showed.
            assert_eq!(chain.evt.event_player, 0);
        }
    }

    /// **`EVENT_TOHAND_CONFIRM` needs both halves**: the card in the hand
    /// *and* a `TO_HAND` event in the current window. Either alone raises
    /// only `EVENT_CONFIRM`.
    #[test]
    fn tohand_confirm_needs_the_hand_and_the_moment() {
        // Both: two events reach the card.
        let mut f = Field::new(8000);
        let c = card(&mut f, 0, card_type::SPELL, location::HAND, 0);
        listen(&mut f, c, code::CONFIRM);
        listen(&mut f, c, code::TOHAND_CONFIRM);
        f.core
            .point_event
            .push_back(crate::event::Event::new(code::TO_HAND));
        assert!(confirm_cards(&mut f, 1, vec![c]));
        assert_eq!(solve_continuous_count(&f), 2, "CONFIRM and TOHAND_CONFIRM");

        // In the hand, but no to-hand moment.
        let mut f = Field::new(8000);
        let c = card(&mut f, 0, card_type::SPELL, location::HAND, 0);
        listen(&mut f, c, code::CONFIRM);
        listen(&mut f, c, code::TOHAND_CONFIRM);
        assert!(confirm_cards(&mut f, 1, vec![c]));
        assert_eq!(solve_continuous_count(&f), 1, "only CONFIRM");

        // A to-hand moment, but the card is elsewhere.
        let mut f = Field::new(8000);
        let c = card(&mut f, 0, card_type::SPELL, location::GRAVE, 0);
        listen(&mut f, c, code::CONFIRM);
        listen(&mut f, c, code::TOHAND_CONFIRM);
        f.core
            .point_event
            .push_back(crate::event::Event::new(code::TO_HAND));
        assert!(confirm_cards(&mut f, 1, vec![c]));
        assert_eq!(solve_continuous_count(&f), 1, "only CONFIRM");
    }

    /// A registered **field** continuous effect listening for `event`.
    /// The field-wide half of `ConfirmCards` goes to `queue_event` and is
    /// matched against `field_effects.continuous`, not against the card's
    /// own single effects — so observing it needs a different listener
    /// from [`listen`].
    fn listen_field(f: &mut Field, source: CardId, event: u32) -> EffectId {
        let mut e = Effect::new(effect_type::CONTINUOUS | effect_type::ACTIONS, event);
        e.owner = Some(source);
        e.handler = Some(source);
        e.effect_owner = 0;
        e.range = u16::from(location::MZONE);
        let id = f.new_effect(e);
        f.field_effects.continuous.insert(event, id);
        f.field_effects.indexer.insert(id);
        id
    }

    /// **The field-wide `EVENT_CONFIRM` is raised as well as the
    /// single-card one.** Two different listeners see two different
    /// events, and dropping either leaves one of them silent.
    ///
    /// The earlier version of this test asserted only that the queues were
    /// drained, which is true whether or not anything was ever put on
    /// them — a reading of the outcome where the question was what
    /// mattered.
    #[test]
    fn confirm_raises_the_field_event_as_well_as_the_single_one() {
        let mut f = Field::new(8000);
        let source = card(&mut f, 0, card_type::MONSTER, location::MZONE, 0);
        let c = card(&mut f, 0, card_type::SPELL, location::GRAVE, 0);
        listen_field(&mut f, source, code::CONFIRM);
        assert!(confirm_cards(&mut f, 1, vec![c]));
        assert_eq!(
            solve_continuous_count(&f),
            1,
            "the field-wide CONFIRM reached a field effect"
        );
        assert!(f.core.single_event.is_empty(), "single queue drained");
        assert!(f.core.queue_event.is_empty(), "field queue drained");
    }

    /// **`SendtoHand`'s position argument is dead for the hand, in both
    /// engines.** `field::send_to` forces `POS_FACEUP` for any
    /// destination that is not `LOCATION_REMOVED` (unless `ignore`), so
    /// what the export asks for never reaches `sendto_param` — and then
    /// `add_card` turns it face-down again for the hand.
    ///
    /// Recorded as a test rather than a comment because it is the sort of
    /// argument a later reader would "fix" into something load-bearing.
    #[test]
    fn the_position_asked_for_never_reaches_a_hand_send() {
        for asked in [crate::position::FACEUP, crate::position::FACEDOWN] {
            let mut f = Field::new(8000);
            let c = card(&mut f, 0, card_type::SPELL, location::GRAVE, 0);
            f.send_to(
                vec![c],
                None,
                reason::EFFECT,
                0,
                PLAYER_NONE,
                u16::from(location::HAND),
                0,
                asked,
                false,
            );
            assert_eq!(
                f.cards[c].sendto_param.position,
                crate::position::FACEUP,
                "forced face-up before the move, whatever was asked"
            );
            for _ in 0..512 {
                if matches!(f.process(), crate::processor::Status::End) {
                    break;
                }
            }
            assert_eq!(
                f.cards[c].current.position,
                crate::position::FACEDOWN,
                "and face-down once it is in the hand"
            );
        }
    }
}

#[cfg(test)]
mod search_tests {
    use super::*;
    use crate::board::location;
    use crate::card::{assume, card_type, race, status, Card, CardData};
    use crate::field::Message;

    fn monster(f: &mut Field, owner: u8, level: u32, race_: u64, loc: u8) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 300 + level + u32::from(owner) * 50,
                type_: card_type::MONSTER | card_type::NORMAL,
                level,
                attack: 1000,
                defense: 1000,
                race: race_,
                ..Default::default()
            },
            owner,
        );
        c.current.controller = owner;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(owner, id, loc, 0, false);
        id
    }

    /// **`IsLevelBelow` is inclusive, and zero is never below anything.**
    ///
    /// The zero guard is the reference's (`plvl > 0 && plvl <= lvl`) and
    /// it is the clause that matters in a real deck: without it every
    /// Spell and Trap in there answers yes to "level 4 or less".
    #[test]
    fn is_level_below_is_inclusive_and_refuses_zero() {
        let mut f = Field::new(8000);
        let one = monster(&mut f, 0, 1, race::WARRIOR, location::DECK);
        let four = monster(&mut f, 0, 4, race::WARRIOR, location::DECK);
        let five = monster(&mut f, 0, 5, race::WARRIOR, location::DECK);
        assert!(is_level_below(&mut f, one, 4));
        assert!(is_level_below(&mut f, four, 4), "the bound is inclusive");
        assert!(!is_level_below(&mut f, five, 4));
        // A Spell has no level at all.
        let mut spell = Card::with_data(
            CardData {
                code: 999,
                type_: card_type::SPELL,
                ..Default::default()
            },
            0,
        );
        spell.current.controller = 0;
        let sid = f.new_card(spell);
        f.add_card(0, sid, location::DECK, 0, false);
        assert!(
            !is_level_below(&mut f, sid, 4),
            "level zero is not below four"
        );
        assert!(!is_level_below(&mut f, sid, 0), "nor below zero");
    }

    /// **`IsRace` is a mask test against the *effective* race**, not an
    /// equality against the printed one.
    ///
    /// A mask matters because a card can be several races at once; the
    /// effective race matters because effects change it. `assume` is the
    /// cheapest way to make the two disagree in a test.
    #[test]
    fn is_race_masks_the_effective_race() {
        let mut f = Field::new(8000);
        let c = monster(&mut f, 0, 4, race::WARRIOR, location::DECK);
        assert!(is_race(&mut f, c, race::WARRIOR));
        assert!(!is_race(&mut f, c, race::SPELLCASTER));
        assert!(
            is_race(&mut f, c, race::WARRIOR | race::SPELLCASTER),
            "a mask, so either bit is a match"
        );
        // Effective, not printed: the card still reads WARRIOR on its face.
        f.cards[c].assume.insert(assume::RACE, race::SPELLCASTER);
        assert!(is_race(&mut f, c, race::SPELLCASTER), "what it counts as");
        assert!(!is_race(&mut f, c, race::WARRIOR));
        assert_eq!(f.cards[c].data.race, race::WARRIOR, "the printed line");
    }

    /// **`SelectMatchingCard` asks over a non-targeting scan**, honours
    /// its exception, and refuses a player who does not exist.
    #[test]
    fn select_matching_card_asks_over_the_scan() {
        let mut f = Field::new(8000);
        let a = monster(&mut f, 0, 4, race::WARRIOR, location::DECK);
        let b = monster(&mut f, 0, 3, race::WARRIOR, location::DECK);
        let theirs = monster(&mut f, 1, 4, race::WARRIOR, location::DECK);
        let deck = u32::from(location::DECK);
        let warrior = |f: &mut Field, c: CardId| is_race(f, c, race::WARRIOR);

        assert!(select_matching_card(
            &mut f,
            0,
            Some(&warrior),
            0,
            deck,
            0,
            1,
            1,
            Except::None
        ));
        let mut got = f.core.select_cards.clone();
        got.sort_unstable();
        let mut want = vec![a, b];
        want.sort_unstable();
        assert_eq!(got, want, "my deck only, and only Warriors");
        assert!(!f.core.select_cards.contains(&theirs));

        // The exception is removed from the scan.
        assert!(select_matching_card(
            &mut f,
            0,
            Some(&warrior),
            0,
            deck,
            0,
            1,
            1,
            Except::Card(a)
        ));
        assert_eq!(f.core.select_cards, vec![b]);

        f.core.select_cards.clear();
        assert!(
            !select_matching_card(&mut f, 2, Some(&warrior), 0, deck, 0, 1, 1, Except::None),
            "there is no player 2"
        );
        assert!(f.core.select_cards.is_empty(), "and nothing was queued");
    }

    /// **The question it queues is not cancelable**, and asks the player
    /// it was told to.
    #[test]
    fn select_matching_card_queues_an_uncancelable_question() {
        let mut f = Field::new(8000);
        monster(&mut f, 1, 4, race::WARRIOR, location::DECK);
        let deck = u32::from(location::DECK);
        assert!(select_matching_card(
            &mut f,
            1,
            Some(&always),
            1,
            deck,
            0,
            1,
            1,
            Except::None
        ));
        assert!(matches!(f.process(), crate::processor::Status::Awaiting));
        match f.messages.last() {
            Some(Message::SelectCard {
                player,
                cancelable,
                min,
                max,
                ..
            }) => {
                assert_eq!(*player, 1);
                assert!(!cancelable, "the export's default");
                assert_eq!((*min, *max), (1, 1));
            }
            other => panic!("expected a card question: {other:?}"),
        }
    }
}

#[cfg(test)]
mod label_and_limit_tests {
    use super::*;
    use crate::board::location;
    use crate::card::{card_type, status, Card, CardData};
    use crate::effect::{effect_count, Effect};

    fn card(f: &mut Field, type_: u32, attack: i32, loc: u8) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 600,
                type_,
                level: 4,
                attack,
                defense: 0,
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

    fn effect(f: &mut Field) -> EffectId {
        f.new_effect(Effect::new(effect_type::FIELD, code::CANNOT_ACTIVATE))
    }

    /// **A label round-trips, and an unset one reads zero.** The zero is
    /// the reference's: `GetLabel` on an empty label pushes `0` rather
    /// than failing, and a card that never set one relies on it.
    #[test]
    fn a_label_round_trips_and_defaults_to_zero() {
        let mut f = Field::new(8000);
        let e = effect(&mut f);
        assert_eq!(get_label(f.effects.get(e).unwrap()), 0, "never set");
        set_label(&mut f, e, vec![4_242]);
        assert_eq!(get_label(f.effects.get(e).unwrap()), 4_242);
        // A list, as the reference's is; the first is what `GetLabel`
        // reads for a caller that stored one.
        set_label(&mut f, e, vec![7, 8, 9]);
        assert_eq!(get_label(f.effects.get(e).unwrap()), 7);
        set_label(&mut f, e, Vec::new());
        assert_eq!(get_label(f.effects.get(e).unwrap()), 0, "cleared");
    }

    /// **A count limit sets its flag, its ceiling, and which tally it
    /// counts against.** The code is what makes a limit shared between
    /// copies of a card rather than per-copy.
    #[test]
    fn a_count_limit_records_its_ceiling_and_its_tally() {
        let mut f = Field::new(8000);
        let e = effect(&mut f);
        set_count_limit(&mut f, e, 1, 26_202_165, 0);
        let x = f.effects.get(e).unwrap();
        assert!(x.is_flag(flag::COUNT_LIMIT));
        assert_eq!((x.count_limit, x.count_limit_max), (1, 1));
        assert_eq!(x.count_code, 26_202_165, "per name, not per card");

        // A chain-scoped limit with no code becomes a single-card one,
        // which is the reference's own repair of an under-specified call.
        let e2 = effect(&mut f);
        set_count_limit(&mut f, e2, 2, 0, effect_count::CHAIN);
        let x2 = f.effects.get(e2).unwrap();
        assert_eq!((x2.count_limit, x2.count_limit_max), (2, 2));
        assert!(x2.count_flag & effect_count::SINGLE != 0);
    }

    /// **A count of zero is refused**, as the reference refuses it: it
    /// would read as "no limit" where it means "never".
    #[test]
    #[should_panic(expected = "count limit of 0")]
    fn a_count_limit_of_zero_is_refused() {
        let mut f = Field::new(8000);
        let e = effect(&mut f);
        set_count_limit(&mut f, e, 0, 0, 0);
    }

    /// **`IsPreviousLocation` reads where the card was**, which is a
    /// different question from where it is.
    #[test]
    fn is_previous_location_reads_the_previous_one() {
        let mut f = Field::new(8000);
        let c = card(&mut f, card_type::MONSTER, 1000, location::GRAVE);
        f.cards[c].previous.location = location::MZONE;
        assert!(is_previous_location(&f, c, u16::from(location::MZONE)));
        assert!(
            is_previous_location(&f, c, u16::from(location::ONFIELD)),
            "the monster row is part of the field"
        );
        assert!(!is_previous_location(&f, c, u16::from(location::HAND)));
        assert!(
            !is_previous_location(&f, c, u16::from(location::GRAVE)),
            "where it is now is not where it was"
        );
    }

    /// **`IsAttackBelow` refuses a non-monster outright**, is inclusive,
    /// and refuses a negative attack.
    #[test]
    fn is_attack_below_guards_both_ends() {
        let mut f = Field::new(8000);
        let m = card(&mut f, card_type::MONSTER, 1500, location::DECK);
        assert!(is_attack_below(&mut f, m, 1500), "inclusive");
        assert!(!is_attack_below(&mut f, m, 1499));
        let spell = card(&mut f, card_type::SPELL, 0, location::DECK);
        assert!(
            !is_attack_below(&mut f, spell, 1500),
            "a Spell is not attack-zero, it is not a monster at all"
        );
        // A negative attack is not "below" anything.
        let neg = card(&mut f, card_type::MONSTER, -100, location::DECK);
        assert!(!is_attack_below(&mut f, neg, 1500));
        assert!(!is_attack_below(&mut f, neg, 0));
    }

    /// **The read-only code reader resolves `EFFECT_CHANGE_CODE`**, the
    /// same as the mutable one — the mutable field is needed for the
    /// re-entrancy guard, not for the data.
    #[test]
    fn the_read_only_code_reader_sees_a_changed_name() {
        let mut f = Field::new(8000);
        let c = card(&mut f, card_type::MONSTER, 1000, location::MZONE);
        let printed = f.cards[c].data.code;
        assert!(is_code_readonly(&f, c, printed));
        assert!(!is_code_readonly(&f, c, printed + 1));

        let mut changer = Effect::new(effect_type::SINGLE, code::CHANGE_CODE);
        changer.owner = Some(c);
        changer.handler = Some(c);
        changer.value = i64::from(printed + 1);
        let changer = f.new_effect(changer);
        f.cards[c].single_effect.insert(code::CHANGE_CODE, changer);
        assert!(
            is_code_readonly(&f, c, printed + 1),
            "the effective name, not the printed one"
        );
        assert!(!is_code_readonly(&f, c, printed));
        assert_eq!(f.cards[c].data.code, printed, "the printed line is intact");
        // And it agrees with the mutable reader.
        assert!(is_code(&mut f, c, printed + 1));
    }
}

#[cfg(test)]
mod summon_api_tests {
    use super::*;
    use crate::board::{location, position};
    use crate::card::{assume, attribute, card_type, status, Card, CardData};
    use crate::effect::Effect;

    fn monster(f: &mut Field, owner: u8, attr: u32, loc: u8, seat: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 800 + seat + u32::from(owner) * 50,
                type_: card_type::MONSTER | card_type::NORMAL,
                level: 4,
                attack: 1000,
                defense: 1000,
                attribute: attr,
                ..Default::default()
            },
            owner,
        );
        c.current.controller = owner;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(owner, id, loc, seat, false);
        if loc == location::MZONE {
            f.cards[id].current.position = position::FACEUP_ATTACK;
        }
        id
    }

    /// **`IsAttribute` is a mask against the *effective* attribute**, not
    /// an equality against the printed one. Both halves matter: a card
    /// can be two attributes at once, and effects change them.
    #[test]
    fn is_attribute_masks_the_effective_attribute() {
        let mut f = Field::new(8000);
        let c = monster(&mut f, 0, attribute::DARK, location::DECK, 0);
        assert!(is_attribute(&mut f, c, attribute::DARK));
        assert!(!is_attribute(&mut f, c, attribute::LIGHT));
        assert!(
            is_attribute(&mut f, c, attribute::DARK | attribute::LIGHT),
            "a mask, so either bit is a match"
        );
        f.cards[c]
            .assume
            .insert(assume::ATTRIBUTE, u64::from(attribute::LIGHT));
        assert!(
            is_attribute(&mut f, c, attribute::LIGHT),
            "what it counts as"
        );
        assert!(!is_attribute(&mut f, c, attribute::DARK));
        assert_eq!(f.cards[c].data.attribute, attribute::DARK, "printed line");
    }

    /// **`GetLocationCount` counts down as the row fills**, refuses a
    /// player who does not exist, and asks on behalf of the **reason
    /// player** rather than the owner of the zone.
    #[test]
    fn get_location_count_counts_the_free_zones() {
        let mut f = Field::new(8000);
        f.core.reason_player = 0;
        assert_eq!(get_location_count(&mut f, 0, location::MZONE), 5);
        monster(&mut f, 0, attribute::DARK, location::MZONE, 0);
        monster(&mut f, 0, attribute::DARK, location::MZONE, 1);
        assert_eq!(get_location_count(&mut f, 0, location::MZONE), 3);
        assert_eq!(
            get_location_count(&mut f, 1, location::MZONE),
            5,
            "the other player's row is its own"
        );
        assert_eq!(
            get_location_count(&mut f, 2, location::MZONE),
            0,
            "there is no player 2"
        );
    }

    /// **The count is asked for the reason player.** Which player is
    /// *doing* the putting decides what the answer may be, and it is not
    /// the same question as whose row it is — the reference passes
    /// `core.reason_player` as the default `uplayer`.
    #[test]
    fn get_location_count_asks_on_behalf_of_the_reason_player() {
        let mut f = Field::new(8000);
        // A prohibition on player 1 using player 0's row.
        fn none_at_all(_: &Effect, _: &Field, _: &crate::effect::Ctx) -> i64 {
            0
        }
        let source = monster(&mut f, 0, attribute::DARK, location::MZONE, 0);
        let mut cap = Effect::new(effect_type::FIELD, code::MAX_MZONE);
        cap.owner = Some(source);
        cap.handler = Some(source);
        cap.effect_owner = 0;
        cap.flag[0] |= flag::PLAYER_TARGET | flag::FUNC_VALUE;
        cap.range = u16::from(location::MZONE);
        cap.s_range = u16::from(location::MZONE);
        cap.value_fn = Some(none_at_all);
        let cap = f.new_effect(cap);
        f.field_effects.aura.insert(code::MAX_MZONE, cap);
        f.field_effects.indexer.insert(cap);

        f.core.reason_player = 0;
        let as_zero = get_location_count(&mut f, 0, location::MZONE);
        f.core.reason_player = 1;
        let as_one = get_location_count(&mut f, 1, location::MZONE);
        assert_ne!(
            (as_zero, as_one),
            (as_one, as_zero),
            "the two players do not see the same row"
        );
    }

    /// **`SpecialSummon` refuses a player who cannot exist**, and queues
    /// nothing when it does.
    #[test]
    fn special_summon_refuses_an_impossible_player() {
        let mut f = Field::new(8000);
        let c = monster(&mut f, 0, attribute::DARK, location::DECK, 0);
        assert!(
            !special_summon(
                &mut f,
                vec![c],
                0,
                PLAYER_NONE,
                0,
                false,
                false,
                position::FACEUP_ATTACK
            ),
            "PLAYER_NONE cannot summon"
        );
        assert!(f.core.subunits.is_empty(), "and nothing was queued");
        assert!(special_summon(
            &mut f,
            vec![c],
            0,
            0,
            0,
            false,
            false,
            position::FACEUP_ATTACK
        ));
        assert!(!f.core.subunits.is_empty(), "a real player queues one");
    }

    /// **`IsCanBeSpecialSummoned`'s unstated arguments are face-up, and
    /// the summoning player's own side.**
    ///
    /// Both defaults are the reference's trailing ones, and both are
    /// things a caller would otherwise have to guess: a face-down default
    /// would refuse cards that may only arrive face-up, and sending it to
    /// the other player's side asks an entirely different question.
    #[test]
    fn is_can_be_special_summoned_defaults_to_face_up_and_own_side() {
        let mut f = Field::new(8000);
        let c = monster(&mut f, 0, attribute::DARK, location::DECK, 0);
        let mut e = Effect::new(effect_type::SINGLE, code::FREE_CHAIN);
        e.owner = Some(c);
        e.handler = Some(c);
        let e = f.new_effect(e);
        assert!(is_can_be_special_summoned(&mut f, c, e, 0, 0, false, false));
        // The port's wrapper must ask the same question the reference's
        // defaults do; spelled out, that is face-up and `sumplayer` twice.
        let spelled_out =
            f.is_can_be_special_summoned(c, Some(e), 0, position::FACEUP, 0, 0, false, false, 0xff);
        assert!(spelled_out, "the same call, written out");
        // The predicate asks about the *card*, not the room — a full row
        // is `GetLocationCount`'s question, which is why the script asks
        // both separately.
    }

    /// **The two unstated arguments are face-up and the summoning
    /// player's own side**, and both are observable through a
    /// unique-on-field restriction — the one place the predicate reads
    /// `sumpos` and `toplayer` together:
    ///
    /// ```text
    /// if sumpos & POS_FACEUP && check_unique_onfield(card, toplayer, MZONE)
    ///     -> strip FACEUP; if nothing is left, refuse
    /// ```
    ///
    /// So a second copy of a unique card is refused **face-up** and
    /// allowed face-down, and the copy that blocks it is looked for on
    /// **`toplayer`'s** side. A wrapper that defaulted to face-down, or
    /// aimed at the other player, would answer yes to both.
    #[test]
    fn face_up_and_own_side_are_the_unstated_defaults() {
        fn unique_on_field(f: &mut Field, controller: u8, code_: u32, seat: usize) -> CardId {
            let mut c = Card::with_data(
                CardData {
                    code: code_,
                    type_: card_type::MONSTER | card_type::NORMAL,
                    level: 4,
                    attack: 1000,
                    defense: 1000,
                    ..Default::default()
                },
                controller,
            );
            c.current.controller = controller;
            c.current.location = location::MZONE;
            c.current.sequence = seat as u32;
            c.current.position = position::FACEUP_ATTACK;
            c.set_status(status::EFFECT_ENABLED, true);
            c.unique_code = code_;
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

        let mut f = Field::new(8000);
        // A second copy, waiting in player 0's deck.
        let mut twin = Card::with_data(
            CardData {
                code: 4_242,
                type_: card_type::MONSTER | card_type::NORMAL,
                level: 4,
                attack: 1000,
                defense: 1000,
                ..Default::default()
            },
            0,
        );
        twin.current.controller = 0;
        twin.set_status(status::EFFECT_ENABLED, true);
        // No `unique_*` of its own: what blocks it is the copy already on
        // the field, which is the first arm of `check_unique_onfield`.
        let twin = f.new_card(twin);
        f.add_card(0, twin, location::DECK, 0, false);
        let mut e = Effect::new(effect_type::SINGLE, code::FREE_CHAIN);
        e.owner = Some(twin);
        e.handler = Some(twin);
        let e = f.new_effect(e);

        assert!(
            is_can_be_special_summoned(&mut f, twin, e, 0, 0, false, false),
            "nothing is blocking it yet"
        );

        // The first copy arrives on player 0's field.
        unique_on_field(&mut f, 0, 4_242, 0);
        assert!(
            !is_can_be_special_summoned(&mut f, twin, e, 0, 0, false, false),
            "face-up, its own side: refused"
        );
        assert!(
            f.is_can_be_special_summoned(
                twin,
                Some(e),
                0,
                position::FACEDOWN,
                0,
                0,
                false,
                false,
                0xff
            ),
            "face-down never reaches the unique check, so a face-down \
             default would answer yes here"
        );
        assert!(
            f.is_can_be_special_summoned(
                twin,
                Some(e),
                0,
                position::FACEUP,
                0,
                1,
                false,
                false,
                0xff
            ),
            "and the blocker is looked for on `toplayer`'s side, which is \
             player 1's and empty"
        );
    }

    /// **`nocheck` is what lets a revive-limited card come out of a
    /// deck.** A monster carrying `EFFECT_REVIVE_LIMIT` that has never
    /// been properly summoned is refused from the deck or hand *unless*
    /// the caller passes `nocheck`. Passing `true` where the script
    /// passes `false` would let a Nomi monster be summoned straight out
    /// of a deck.
    #[test]
    fn nocheck_is_what_waives_the_revive_limit() {
        let mut f = Field::new(8000);
        let c = monster(&mut f, 0, attribute::DARK, location::DECK, 0);
        let mut e = Effect::new(effect_type::SINGLE, code::FREE_CHAIN);
        e.owner = Some(c);
        e.handler = Some(c);
        let e = f.new_effect(e);
        assert!(
            is_can_be_special_summoned(&mut f, c, e, 0, 0, false, false),
            "an ordinary monster comes out of a deck"
        );

        let mut limit = Effect::new(effect_type::SINGLE, code::REVIVE_LIMIT);
        limit.owner = Some(c);
        limit.handler = Some(c);
        let limit = f.new_effect(limit);
        f.cards[c].single_effect.insert(code::REVIVE_LIMIT, limit);
        assert!(
            !is_can_be_special_summoned(&mut f, c, e, 0, 0, false, false),
            "revive-limited, never properly summoned, and coming from a deck"
        );
        assert!(
            f.is_can_be_special_summoned(c, Some(e), 0, position::FACEUP, 0, 0, true, false, 0xff),
            "nocheck waives it for a deck or hand"
        );
        assert!(
            f.is_can_be_special_summoned(c, Some(e), 0, position::FACEUP, 0, 0, false, true, 0xff),
            "and nolimit waives it outright"
        );
    }

    /// **The two player arguments are asked separately.** `sumplayer` is
    /// who summons and `toplayer` whose side it lands on, and a player
    /// forbidden from special summoning refuses in the first slot.
    #[test]
    fn the_summon_players_are_two_different_questions() {
        let mut f = Field::new(8000);
        let c = monster(&mut f, 0, attribute::DARK, location::DECK, 0);
        let mut e = Effect::new(effect_type::SINGLE, code::FREE_CHAIN);
        e.owner = Some(c);
        e.handler = Some(c);
        let e = f.new_effect(e);
        assert!(is_can_be_special_summoned(&mut f, c, e, 0, 0, false, false));
        assert!(is_can_be_special_summoned(&mut f, c, e, 0, 1, false, false));

        // Player 1 may not special summon at all.
        let source = monster(&mut f, 1, attribute::DARK, location::MZONE, 0);
        let mut ban = Effect::new(effect_type::FIELD, code::CANNOT_SPECIAL_SUMMON);
        ban.owner = Some(source);
        ban.handler = Some(source);
        ban.effect_owner = 1;
        ban.flag[0] |= flag::PLAYER_TARGET;
        ban.range = u16::from(location::MZONE);
        ban.s_range = 1;
        ban.o_range = 0;
        let ban = f.new_effect(ban);
        f.field_effects
            .aura
            .insert(code::CANNOT_SPECIAL_SUMMON, ban);
        f.field_effects.indexer.insert(ban);

        assert!(
            is_can_be_special_summoned(&mut f, c, e, 0, 0, false, false),
            "player 0 still may"
        );
        assert!(
            !is_can_be_special_summoned(&mut f, c, e, 0, 1, false, false),
            "player 1 may not, and it is `sumplayer` that is asked"
        );
    }
}

#[cfg(test)]
mod banish_and_shuffle_tests {
    use super::*;
    use crate::board::location;
    use crate::card::{card_type, status, Card, CardData};
    use crate::field::Message;

    fn card_in(f: &mut Field, owner: u8, loc: u8, seq: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 900 + seq + u32::from(owner) * 50,
                type_: card_type::MONSTER | card_type::NORMAL,
                level: 4,
                attack: 1000,
                defense: 1000,
                ..Default::default()
            },
            owner,
        );
        c.current.controller = owner;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(owner, id, loc, seq, false);
        f.cards[id].current.position = crate::position::FACEUP_ATTACK;
        id
    }

    fn run(f: &mut Field) {
        for _ in 0..1024 {
            if matches!(f.process(), crate::processor::Status::End) {
                return;
            }
        }
    }

    /// **`Duel.Destroy`'s destination is the third argument**, and the
    /// plain call still means the graveyard.
    ///
    /// Destroyed *to* the banished pile is one action: it leaves as a
    /// destruction and never touches a graveyard on the way, which is a
    /// different thing from destroying it and then banishing it.
    #[test]
    fn destroy_can_name_somewhere_other_than_the_graveyard() {
        let mut f = Field::new(8000);
        let c = card_in(&mut f, 0, location::MZONE, 0);
        destroy(&mut f, vec![c], crate::card::reason::EFFECT);
        run(&mut f);
        assert_eq!(
            f.cards[c].current.location,
            location::GRAVE,
            "the plain call still means the graveyard"
        );

        let mut f = Field::new(8000);
        let c = card_in(&mut f, 0, location::MZONE, 0);
        destroy_to(
            &mut f,
            vec![c],
            crate::card::reason::EFFECT,
            u16::from(location::REMOVED),
        );
        run(&mut f);
        assert_eq!(f.cards[c].current.location, location::REMOVED);
        assert!(
            f.cards[c].reason & crate::card::reason::DESTROY != 0,
            "and it still left as a destruction"
        );
    }

    /// **`IsFacedown` is face-down either way up**, and is not merely the
    /// negation of one face-up position.
    #[test]
    fn is_facedown_covers_both_ways_up() {
        let mut f = Field::new(8000);
        let c = card_in(&mut f, 0, location::MZONE, 0);
        for (pos, want) in [
            (crate::position::FACEUP_ATTACK, false),
            (crate::position::FACEUP_DEFENSE, false),
            (crate::position::FACEDOWN_ATTACK, true),
            (crate::position::FACEDOWN_DEFENSE, true),
        ] {
            f.cards[c].current.position = pos;
            assert_eq!(is_facedown(&f, c), want, "position {pos:#x}");
        }
    }

    /// **`ShuffleDeck` shuffles a deck, refuses an impossible player, and
    /// announces itself.**
    ///
    /// Under `DUEL_PSEUDO_SHUFFLE` the deck is the one pile that is *not*
    /// really shuffled — the announcement is the whole observable effect,
    /// and it is what the two engines compare.
    #[test]
    fn shuffle_deck_announces_the_right_pile() {
        let mut f = Field::new(8000);
        card_in(&mut f, 0, location::DECK, 0);
        card_in(&mut f, 0, location::HAND, 0);
        shuffle_deck(&mut f, 0);
        assert!(
            f.messages
                .iter()
                .any(|m| matches!(m, Message::ShuffleDeck { player: 0 })),
            "the deck was announced"
        );
        assert!(
            !f.messages
                .iter()
                .any(|m| matches!(m, Message::ShuffleHand { .. })),
            "and the hand was left alone"
        );

        let before = f.messages.len();
        shuffle_deck(&mut f, 2);
        assert_eq!(f.messages.len(), before, "there is no player 2");
    }
}

#[cfg(test)]
mod cost_and_random_tests {
    use super::*;
    use crate::board::location;
    use crate::card::{card_type, status, Card, CardData};
    use crate::effect::Effect;

    fn hand_card(f: &mut Field, owner: u8, seq: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 1_000 + seq + u32::from(owner) * 50,
                type_: card_type::MONSTER | card_type::NORMAL,
                level: 4,
                attack: 1000,
                defense: 1000,
                ..Default::default()
            },
            owner,
        );
        c.current.controller = owner;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(owner, id, location::HAND, seq, false);
        id
    }

    /// **`CheckLPCost` is not a bare affordability test.**
    ///
    /// Three answers, and only the last is "do they have it": a cost
    /// reduced to zero or less by `EFFECT_LPCOST_CHANGE` is payable by
    /// definition, and a replacement that would apply makes it payable
    /// however little life is left.
    #[test]
    fn check_lp_cost_has_three_answers() {
        let mut f = Field::new(8000);
        assert!(check_lp_cost(&mut f, 0, 1000));
        f.players[0].lp = 1000;
        assert!(check_lp_cost(&mut f, 0, 1000), "exactly enough");
        f.players[0].lp = 999;
        assert!(!check_lp_cost(&mut f, 0, 1000), "one short");
        assert!(!check_lp_cost(&mut f, 2, 1000), "there is no player 2");

        // A cost-changing effect that reduces it to nothing.
        fn to_nothing(_: &Effect, _: &Field, _: &crate::effect::Ctx) -> i64 {
            0
        }
        let mut source = Card::with_data(
            CardData {
                code: 4_000,
                type_: card_type::MONSTER | card_type::NORMAL,
                ..Default::default()
            },
            0,
        );
        source.current.controller = 0;
        source.set_status(status::EFFECT_ENABLED, true);
        let source = f.new_card(source);
        f.add_card(0, source, location::MZONE, 0, false);
        f.cards[source].current.position = crate::position::FACEUP_ATTACK;
        let mut change = Effect::new(effect_type::FIELD, code::LPCOST_CHANGE);
        change.owner = Some(source);
        change.handler = Some(source);
        change.effect_owner = 0;
        change.flag[0] |= flag::PLAYER_TARGET | flag::FUNC_VALUE;
        change.range = u16::from(location::MZONE);
        change.s_range = 1;
        change.o_range = 0;
        change.value_fn = Some(to_nothing);
        let change = f.new_effect(change);
        f.field_effects.aura.insert(code::LPCOST_CHANGE, change);
        f.field_effects.indexer.insert(change);

        f.players[0].lp = 1;
        assert!(
            check_lp_cost(&mut f, 0, 1000),
            "reduced to nothing, so payable with one life left"
        );
    }

    /// **The composed figure is what is compared, not the printed one.**
    ///
    /// A cost halved to something the player *can* afford is payable,
    /// even though the number the card names is not. Comparing the
    /// original would refuse it — and the `val <= 0` shortcut cannot
    /// cover this case, because the reduced cost is still positive.
    #[test]
    fn check_lp_cost_compares_the_reduced_figure() {
        fn halved(_: &Effect, _: &Field, ctx: &crate::effect::Ctx) -> i64 {
            // `add_param(reason_effect); add_param(playerid); add_param(val)`
            // — the running figure is the last argument.
            ctx.args.last().copied().unwrap_or(0) / 2
        }
        let mut f = Field::new(8000);
        let mut source = Card::with_data(
            CardData {
                code: 4_001,
                type_: card_type::MONSTER | card_type::NORMAL,
                ..Default::default()
            },
            0,
        );
        source.current.controller = 0;
        source.set_status(status::EFFECT_ENABLED, true);
        let source = f.new_card(source);
        f.add_card(0, source, location::MZONE, 0, false);
        f.cards[source].current.position = crate::position::FACEUP_ATTACK;
        let mut change = Effect::new(effect_type::FIELD, code::LPCOST_CHANGE);
        change.owner = Some(source);
        change.handler = Some(source);
        change.effect_owner = 0;
        change.flag[0] |= flag::PLAYER_TARGET | flag::FUNC_VALUE;
        change.range = u16::from(location::MZONE);
        change.s_range = 1;
        change.o_range = 0;
        change.value_fn = Some(halved);
        let change = f.new_effect(change);
        f.field_effects.aura.insert(code::LPCOST_CHANGE, change);
        f.field_effects.indexer.insert(change);

        f.players[0].lp = 600;
        assert!(
            check_lp_cost(&mut f, 0, 1000),
            "1000 halved to 500, and 500 is affordable with 600 left"
        );
        f.players[0].lp = 400;
        assert!(
            !check_lp_cost(&mut f, 0, 1000),
            "but 500 is not affordable with 400"
        );
    }

    /// **`PayLPCost` queues the cost against the player named**, and
    /// refuses one who does not exist.
    #[test]
    fn pay_lp_cost_queues_against_the_named_player() {
        let mut f = Field::new(8000);
        assert!(pay_lp_cost(&mut f, 1, 1000));
        match &f.core.subunits[0].kind {
            crate::processor::Kind::PayLPCost { playerid, cost } => {
                assert_eq!(*playerid, 1);
                assert_eq!(*cost, 1000);
            }
            other => panic!("expected a PayLPCost: {other:?}"),
        }
        f.core.subunits.clear();
        assert!(!pay_lp_cost(&mut f, 2, 1000), "there is no player 2");
        assert!(f.core.subunits.is_empty());
    }

    /// **`RandomSelect`'s two shortcuts happen before any roll.**
    ///
    /// A count at or above the group's size takes the whole group and
    /// rolls **nothing**; a count of zero takes nothing. Both matter
    /// because a wasted roll advances the generator and every later roll
    /// in the duel diverges.
    #[test]
    fn random_select_shortcuts_without_rolling() {
        let mut f = Field::new(8000);
        let a = hand_card(&mut f, 0, 0);
        let b = hand_card(&mut f, 0, 1);
        let before = f.rng.clone();

        assert!({
            random_select_request(&mut f, &[a, b], 0, 0);
            random_selected(&mut f)
        }
        .is_empty());
        assert_eq!(f.rng, before, "a count of zero rolls nothing");

        let all = {
            random_select_request(&mut f, &[a, b], 0, 2);
            random_selected(&mut f)
        };
        assert_eq!(all.len(), 2, "the whole group");
        assert_eq!(f.rng, before, "and takes it without rolling");

        let capped = {
            random_select_request(&mut f, &[a, b], 0, 9);
            random_selected(&mut f)
        };
        assert_eq!(capped.len(), 2, "the count is capped at the group size");
        assert_eq!(f.rng, before, "still no roll");

        assert!(
            {
                random_select_request(&mut f, &[a, b], 2, 1);
                random_selected(&mut f)
            }
            .is_empty(),
            "there is no player 2"
        );
        assert_eq!(f.rng, before, "and a refused call rolls nothing");
    }

    /// **A duplicate draw costs a roll and changes nothing.**
    ///
    /// Picking 3 of 4 with the default seed draws a **repeat** on the
    /// way: four rolls for three cards. The reference loops until its
    /// result *set* has grown, so the wasted roll still advances the
    /// generator — and a picker that avoided the duplicate by taking the
    /// next card instead would use three rolls and leave the generator
    /// somewhere else entirely. Every later roll in the duel would then
    /// diverge.
    #[test]
    fn a_duplicate_draw_still_costs_a_roll() {
        let mut f = Field::new(8000);
        let cards: Vec<CardId> = (0..4).map(|i| hand_card(&mut f, 0, i)).collect();

        // What a picker that never repeats would leave behind: three
        // rolls, and no more.
        let mut without_repeats = f.rng.clone();
        for _ in 0..3 {
            without_repeats.next_integer(0, 3);
        }

        let picked = {
            random_select_request(&mut f, &cards, 0, 3);
            random_selected(&mut f)
        };
        assert_eq!(picked.len(), 3, "three distinct cards");
        assert_ne!(
            f.rng, without_repeats,
            "the duplicate cost a fourth roll, so the generator is further on"
        );

        // And it really is exactly one further: four rolls, not three.
        let mut with_one_repeat = Field::new(8000).rng;
        for _ in 0..4 {
            with_one_repeat.next_integer(0, 3);
        }
        assert_eq!(f.rng, with_one_repeat, "four rolls for three cards");
    }

    /// **A real pick rolls, and the draw is a function of the seed.**
    ///
    /// The reference draws an index, inserts into a **set**, and loops
    /// until the set has grown — so a duplicate draw costs a roll and
    /// changes nothing. Picking without replacement would consume a
    /// different number of values, and every later roll would diverge.
    #[test]
    fn random_select_rolls_and_repeats_on_a_duplicate() {
        let mut f = Field::new(8000);
        let cards: Vec<CardId> = (0..4).map(|i| hand_card(&mut f, 0, i)).collect();
        let before = f.rng.clone();
        let picked = {
            random_select_request(&mut f, &cards, 0, 1);
            random_selected(&mut f)
        };
        assert_eq!(picked.len(), 1);
        assert!(cards.contains(&picked[0]));
        assert_ne!(f.rng, before, "a real pick rolls");

        // The same seed picks the same card: the draw is a function of
        // the generator, not of anything else.
        let mut g = Field::new(8000);
        let same: Vec<CardId> = (0..4).map(|i| hand_card(&mut g, 0, i)).collect();
        assert_eq!(
            {
                random_select_request(&mut g, &same, 0, 1);
                random_selected(&mut g)
            },
            picked,
            "same seed, same card"
        );
    }

    /// **In solver mode the pick is the host's.**
    ///
    /// The request asks a `SelectRandom` question instead of rolling,
    /// the answer is in the card-selection format (indices into the
    /// group), a wrong-sized or repeated answer is refused with `Retry`,
    /// and the accepted picks are announced and read back exactly as the
    /// generator's would be. The generator is never advanced.
    #[test]
    fn random_select_in_chance_mode_asks_the_host() {
        use crate::field::Message;
        use crate::processor::Status;
        let mut f = Field::new(8000);
        f.set_chance_mode(true);
        let cards: Vec<CardId> = (0..4).map(|i| hand_card(&mut f, 0, i)).collect();
        let before = f.rng.clone();
        random_select_request(&mut f, &cards, 0, 1);
        assert!(f.core.random_selected.is_empty(), "nothing picked yet");
        let mut asked = None;
        for _ in 0..64 {
            match f.process() {
                Status::Awaiting => {
                    asked = f.messages.last().cloned();
                    break;
                }
                Status::Continue => {}
                Status::End => break,
            }
        }
        match asked {
            Some(Message::SelectRandom {
                player: 0,
                count: 1,
                cards: ref offered,
            }) => assert_eq!(offered, &cards, "the whole group is offered"),
            other => panic!("the random-pick question: {other:?}"),
        }

        // Two picks for a count of one: refused.
        f.core.returns.set_i32(0, 0);
        f.core.returns.set_i32(1, 2);
        f.core.returns.set_i32(2, 1);
        f.core.returns.set_i32(3, 2);
        assert_eq!(f.process(), Status::Awaiting, "asked again");
        assert!(
            matches!(f.messages.last(), Some(Message::Retry)),
            "with a retry: {:?}",
            f.messages.last()
        );

        // The third card, by index.
        f.core.returns.set_i32(0, 0);
        f.core.returns.set_i32(1, 1);
        f.core.returns.set_i32(2, 2);
        for _ in 0..64 {
            if !f.core.random_selected.is_empty() {
                break;
            }
            assert_ne!(f.process(), Status::Awaiting, "no further question");
        }
        assert_eq!(random_selected(&mut f), vec![cards[2]], "the host's pick");
        assert!(
            f.messages.iter().any(
                |m| matches!(m, Message::RandomSelected { player: 0, cards } if cards.len() == 1)
            ),
            "and it is announced"
        );
        assert_eq!(f.rng, before, "the generator is untouched");
    }
}

#[cfg(test)]
mod to_deck_tests {
    use super::*;
    use crate::board::location;
    use crate::card::{card_type, status, Card, CardData};

    fn card_in(f: &mut Field, owner: u8, loc: u8, seq: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 1_200 + seq + u32::from(owner) * 50,
                type_: card_type::MONSTER | card_type::NORMAL,
                level: 4,
                attack: 1000,
                defense: 1000,
                ..Default::default()
            },
            owner,
        );
        c.current.controller = owner;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(owner, id, loc, seq, false);
        id
    }

    fn run(f: &mut Field) {
        for _ in 0..1024 {
            if matches!(f.process(), crate::processor::Status::End) {
                return;
            }
        }
    }

    /// **The sequence is an instruction, not a position**, and
    /// `SEQ_DECKSHUFFLE` is the one that owes a shuffle.
    ///
    /// `add_card`'s deck arm reads 0 as the top, 1 as the bottom, and
    /// anything else as the top *with a shuffle owed* — so the choice
    /// between `0` and `SEQ_DECKSHUFFLE` decides whether the deck is
    /// announced as shuffled afterwards. A card that puts something back
    /// "and shuffle" says so with the sequence.
    #[test]
    fn the_deck_sequence_decides_whether_a_shuffle_is_owed() {
        for (sequence, expect_shuffle) in [(0i32, false), (SEQ_DECKSHUFFLE as i32, true)] {
            let mut f = Field::new(8000);
            card_in(&mut f, 0, location::DECK, 0);
            let c = card_in(&mut f, 0, location::HAND, 0);
            assert!(send_to_deck(
                &mut f,
                vec![c],
                None,
                sequence,
                crate::card::reason::EFFECT
            ));
            run(&mut f);
            assert_eq!(f.cards[c].current.location, location::DECK);
            // The flag, not the message: `shuffle_deck_check` is what the
            // sequence sets, and the executor is what turns it into an
            // announcement when `check_level` falls to zero. A bare call
            // like this one has no executor around it.
            assert_eq!(
                f.core.shuffle_deck_check[0], expect_shuffle,
                "sequence {sequence} and the shuffle it owes"
            );
        }
    }

    /// **It goes to the owner's deck by default**, and refuses a player
    /// who cannot exist.
    #[test]
    fn send_to_deck_defaults_to_the_owner_and_guards_its_player() {
        let mut f = Field::new(8000);
        // Owned by player 1, held by player 0.
        let mut owned_by_one = Card::with_data(
            CardData {
                code: 4_444,
                type_: card_type::MONSTER | card_type::NORMAL,
                ..Default::default()
            },
            1,
        );
        owned_by_one.current.controller = 0;
        owned_by_one.set_status(status::EFFECT_ENABLED, true);
        let c = f.new_card(owned_by_one);
        f.add_card(0, c, location::HAND, 0, false);
        assert!(send_to_deck(
            &mut f,
            vec![c],
            None,
            SEQ_DECKSHUFFLE as i32,
            crate::card::reason::EFFECT
        ));
        run(&mut f);
        assert_eq!(f.cards[c].current.location, location::DECK);
        assert_eq!(
            f.cards[c].current.controller, 1,
            "its owner's deck, not the holder's"
        );

        let mut f = Field::new(8000);
        let c = card_in(&mut f, 0, location::HAND, 0);
        assert!(
            !send_to_deck(
                &mut f,
                vec![c],
                Some(9),
                SEQ_DECKSHUFFLE as i32,
                crate::card::reason::EFFECT
            ),
            "a player above PLAYER_NONE is refused"
        );
        assert!(f.core.subunits.is_empty(), "and nothing was queued");
    }

    /// **A sequence of `-2` is not a deck at all.**
    ///
    /// The reference's one special value: `location = (sequence == -2) ?
    /// 0 : LOCATION_DECK`. Location zero is "nowhere named", the
    /// return-it-to-where-it-came-from case, and a port that always sent
    /// to a deck would quietly put cards in one.
    #[test]
    fn a_sequence_of_minus_two_is_not_a_deck() {
        let mut f = Field::new(8000);
        let c = card_in(&mut f, 0, location::HAND, 0);
        assert!(send_to_deck(
            &mut f,
            vec![c],
            None,
            -2,
            crate::card::reason::EFFECT
        ));
        assert_eq!(
            f.cards[c].sendto_param.location, 0,
            "location zero, not a deck"
        );

        // And any other sequence *is* a deck.
        let mut f = Field::new(8000);
        let c = card_in(&mut f, 0, location::HAND, 0);
        assert!(send_to_deck(
            &mut f,
            vec![c],
            None,
            SEQ_DECKSHUFFLE as i32,
            crate::card::reason::EFFECT
        ));
        assert_eq!(f.cards[c].sendto_param.location, location::DECK);
    }

    /// **A card entering a deck is face-down**, whatever the caller asks
    /// for — `send_to` forces face-up for any non-banish destination, and
    /// `add_card`'s deck arm then overrules that in turn.
    ///
    /// Recorded so the export's `POS_FACEUP` is not "corrected" into
    /// something load-bearing: it is dead, in both engines, for the same
    /// two reasons it is dead for `SendtoHand`.
    #[test]
    fn a_card_entering_a_deck_is_face_down_either_way() {
        for asked in [crate::position::FACEUP, crate::position::FACEDOWN] {
            let mut f = Field::new(8000);
            let c = card_in(&mut f, 0, location::HAND, 0);
            f.send_to(
                vec![c],
                None,
                crate::card::reason::EFFECT,
                0,
                PLAYER_NONE,
                u16::from(location::DECK),
                SEQ_DECKSHUFFLE,
                asked,
                false,
            );
            run(&mut f);
            assert_eq!(
                f.cards[c].current.position,
                crate::position::FACEDOWN,
                "asked for {asked:#x}, and a deck is face-down regardless"
            );
        }
    }
}

#[cfg(test)]
mod excavate_tests {
    use super::*;
    use crate::board::location;
    use crate::card::{card_type, status, Card, CardData};
    use crate::field::Message;

    fn on_deck(f: &mut Field, owner: u8, code_: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: code_,
                type_: card_type::MONSTER | card_type::NORMAL,
                ..Default::default()
            },
            owner,
        );
        c.current.controller = owner;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(owner, id, location::DECK, 0, false);
        id
    }

    fn deck_tops(f: &Field) -> Vec<(u8, Vec<u32>)> {
        f.messages
            .iter()
            .filter_map(|m| match m {
                Message::ConfirmDeckTop { player, codes } => Some((*player, codes.clone())),
                _ => None,
            })
            .collect()
    }

    /// **`ConfirmDecktop` shows the top, caps at the deck's size, and
    /// refuses a player who does not exist.**
    ///
    /// The pile is stored bottom-first, so "the top" is the *end* of it
    /// read backwards — the reference walks `list_main.rbegin()` forward.
    /// Reading it the other way round would show the bottom of the deck,
    /// which is the same length and looks entirely plausible.
    #[test]
    fn confirm_deck_top_shows_the_top_and_caps() {
        let mut f = Field::new(8000);
        let bottom = on_deck(&mut f, 0, 111);
        let middle = on_deck(&mut f, 0, 222);
        let top = on_deck(&mut f, 0, 333);
        let (bc, mc, tc) = (
            f.cards[bottom].data.code,
            f.cards[middle].data.code,
            f.cards[top].data.code,
        );

        assert!(confirm_deck_top(&mut f, 0, 2));
        assert_eq!(
            deck_tops(&f),
            vec![(0u8, vec![tc, mc])],
            "the top two, top first"
        );

        // Asking for more than there is shows the whole deck, not a
        // panic and not a short list padded out.
        f.messages.clear();
        assert!(confirm_deck_top(&mut f, 0, 99));
        assert_eq!(deck_tops(&f), vec![(0u8, vec![tc, mc, bc])]);

        f.messages.clear();
        assert!(!confirm_deck_top(&mut f, 2, 1), "there is no player 2");
        assert!(deck_tops(&f).is_empty(), "and nothing was announced");
    }

    /// **`DiscardDeck` queues against the player named**, and refuses an
    /// impossible one.
    #[test]
    fn discard_deck_guards_its_player() {
        let mut f = Field::new(8000);
        assert!(discard_deck(&mut f, 1, 3, crate::card::reason::EFFECT));
        match &f.core.subunits[0].kind {
            crate::processor::Kind::DiscardDeck {
                playerid,
                count,
                reason,
                ..
            } => {
                assert_eq!(*playerid, 1);
                assert_eq!(*count, 3);
                assert_eq!(*reason, crate::card::reason::EFFECT);
            }
            other => panic!("expected a DiscardDeck: {other:?}"),
        }
        f.core.subunits.clear();
        assert!(!discard_deck(&mut f, 2, 1, crate::card::reason::EFFECT));
        assert!(f.core.subunits.is_empty(), "there is no player 2");
    }
}

#[cfg(test)]
mod controller_tests {
    use super::*;
    use crate::board::location;
    use crate::card::{card_type, status, Card, CardData};

    /// **`GetControler` is whose side it is on now**, not who owns it.
    /// The two differ for a card taken from its owner, and every
    /// question about "your" anything reads the first.
    #[test]
    fn get_controler_is_not_the_owner() {
        let mut f = Field::new(8000);
        let mut owned_by_one = Card::with_data(
            CardData {
                code: 5_555,
                type_: card_type::MONSTER | card_type::NORMAL,
                ..Default::default()
            },
            1,
        );
        owned_by_one.current.controller = 0;
        owned_by_one.set_status(status::EFFECT_ENABLED, true);
        let c = f.new_card(owned_by_one);
        f.add_card(0, c, location::MZONE, 0, false);
        assert_eq!(get_controler(&f, c), 0, "player 0 has it");
        assert_eq!(f.cards[c].owner, 1, "player 1 owns it");
        f.cards[c].current.controller = 1;
        assert_eq!(get_controler(&f, c), 1, "control changed, the answer does");
    }
}

#[cfg(test)]
mod discard_and_announce_tests {
    use super::*;
    use crate::board::location;
    use crate::card::{card_type, race, status, Card, CardData};
    use crate::effect::Effect;
    use crate::field::Message;

    fn card_in(f: &mut Field, owner: u8, loc: u8, seq: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 1_300 + seq,
                type_: card_type::MONSTER | card_type::NORMAL,
                race: race::WARRIOR,
                ..Default::default()
            },
            owner,
        );
        c.current.controller = owner;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(owner, id, loc, seq, false);
        id
    }

    fn single_effect(f: &mut Field, c: CardId, code_: u32) {
        let mut e = Effect::new(effect_type::SINGLE, code_);
        e.owner = Some(c);
        e.handler = Some(c);
        let e = f.new_effect(e);
        f.cards[c].single_effect.insert(code_, e);
    }

    /// **`IsDiscardable`'s cost clause is cost-only.**
    ///
    /// `EFFECT_CANNOT_USE_AS_COST` refuses a discard made **as a cost**
    /// and says nothing about one made as an effect. Dropping the guard
    /// lets such a card pay; widening it to every reason stops the card
    /// being discarded at all.
    #[test]
    fn is_discardable_refuses_a_cost_card_only_as_a_cost() {
        let mut f = Field::new(8000);
        let c = card_in(&mut f, 0, location::HAND, 0);
        assert!(is_discardable(&mut f, c, None, Some(0)), "an ordinary card");
        single_effect(&mut f, c, code::CANNOT_USE_AS_COST);
        assert!(
            !is_discardable(&mut f, c, None, Some(0)),
            "the default reason is a cost, and this card may not be one"
        );
        assert!(
            is_discardable(&mut f, c, Some(crate::card::reason::EFFECT), Some(0)),
            "but discarding it as an *effect* is fine"
        );
    }

    /// **A card outside a hand is not discardable**, whatever else is
    /// true of it — the permission's first line.
    #[test]
    fn a_card_outside_a_hand_is_not_discardable() {
        let mut f = Field::new(8000);
        let held = card_in(&mut f, 0, location::HAND, 0);
        let on_field = card_in(&mut f, 0, location::MZONE, 0);
        let buried = card_in(&mut f, 0, location::GRAVE, 0);
        assert!(is_discardable(&mut f, held, None, Some(0)));
        assert!(!is_discardable(&mut f, on_field, None, Some(0)));
        assert!(!is_discardable(&mut f, buried, None, Some(0)));
    }

    /// **The reason reaches the prohibition's filter.**
    ///
    /// `EFFECT_CANNOT_DISCARD_HAND` is asked with `(effect, card,
    /// peffect, reason)`, and a prohibition that reads the reason can
    /// refuse one kind of discard and allow another. Passing a fixed
    /// value instead would make every such prohibition absolute.
    #[test]
    fn the_discard_prohibition_is_told_the_reason() {
        fn only_costs(_: &Field, _: EffectId, _: Option<CardId>, args: &[i64]) -> bool {
            // args are `[acting effect, reason]`.
            args.last().copied().unwrap_or(0) as u32 & crate::card::reason::COST != 0
        }
        let mut f = Field::new(8000);
        let c = card_in(&mut f, 0, location::HAND, 0);
        let source = card_in(&mut f, 0, location::MZONE, 1);
        f.cards[source].current.position = crate::position::FACEUP_ATTACK;
        let mut ban = Effect::new(effect_type::FIELD, code::CANNOT_DISCARD_HAND);
        ban.owner = Some(source);
        ban.handler = Some(source);
        ban.effect_owner = 0;
        ban.flag[0] |= flag::PLAYER_TARGET;
        ban.range = u16::from(location::MZONE);
        ban.s_range = 1;
        ban.o_range = 0;
        ban.target_filter = Some(only_costs);
        let ban = f.new_effect(ban);
        f.field_effects.aura.insert(code::CANNOT_DISCARD_HAND, ban);
        f.field_effects.indexer.insert(ban);

        assert!(
            !f.is_player_can_discard_hand(0, c, None, crate::card::reason::COST),
            "the prohibition applies to costs"
        );
        assert!(
            f.is_player_can_discard_hand(0, c, None, crate::card::reason::EFFECT),
            "and not to effects — so the reason must reach it"
        );
    }

    /// **`AnnounceRace` caps the count at what is on offer**, and
    /// refuses a player who does not exist.
    #[test]
    fn announce_race_caps_the_count_at_the_offer() {
        let mut f = Field::new(8000);
        assert!(announce_race(&mut f, 0, 3, race::WARRIOR | race::DRAGON));
        match &f.core.subunits[0].kind {
            crate::processor::Kind::AnnounceRace {
                playerid,
                count,
                available,
            } => {
                assert_eq!(*playerid, 0);
                assert_eq!(*count, 2, "asked for three, but only two are on offer");
                assert_eq!(*available, race::WARRIOR | race::DRAGON);
            }
            other => panic!("expected an AnnounceRace: {other:?}"),
        }
        f.core.subunits.clear();
        assert!(!announce_race(&mut f, 2, 1, race::WARRIOR), "no player 2");
        assert!(f.core.subunits.is_empty());
    }

    /// **`GetRace` reads the effective race**, not the printed line.
    #[test]
    fn get_race_reads_the_effective_race() {
        let mut f = Field::new(8000);
        let c = card_in(&mut f, 0, location::MZONE, 0);
        assert_eq!(get_race(&mut f, c), race::WARRIOR);
        f.cards[c]
            .assume
            .insert(crate::card::assume::RACE, race::DRAGON);
        assert_eq!(get_race(&mut f, c), race::DRAGON, "what it counts as");
        assert_eq!(f.cards[c].data.race, race::WARRIOR, "the printed line");
    }

    /// **A mask answer is read as a `u64`.**
    ///
    /// `AnnounceRace` writes its result as a 64-bit mask, and races
    /// reach well above bit 31 — reading the low half would silently
    /// lose every one of them.
    #[test]
    fn a_mask_answer_keeps_its_high_bits() {
        let mut f = Field::new(8000);
        let high = 1u64 << 40;
        f.core.returns.set_u64(0, high);
        assert_eq!(resumed_mask(&f), high, "the high bits survive");
        assert_eq!(
            resumed_value(&f),
            0,
            "and the 32-bit reader sees nothing of it, which is the trap"
        );
    }

    /// **`Message::AnnounceRace` carries the offer to the host.**
    #[test]
    fn the_announce_question_reaches_the_host() {
        let mut f = Field::new(8000);
        assert!(announce_race(&mut f, 1, 1, race::AQUA));
        assert!(matches!(f.process(), crate::processor::Status::Awaiting));
        match f.messages.last() {
            Some(Message::AnnounceRace {
                player,
                count,
                available,
            }) => {
                assert_eq!(*player, 1);
                assert_eq!(*count, 1);
                assert_eq!(*available, race::AQUA);
            }
            other => panic!("expected the question: {other:?}"),
        }
    }
}

#[cfg(test)]
mod life_and_burn_tests {
    use super::*;
    use crate::board::location;
    use crate::card::{card_type, status, Card, CardData};

    fn monster(f: &mut Field, owner: u8, attack: i32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 1_400,
                type_: card_type::MONSTER | card_type::NORMAL,
                level: 4,
                attack,
                defense: 0,
                ..Default::default()
            },
            owner,
        );
        c.current.controller = owner;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(owner, id, location::MZONE, 0, false);
        f.cards[id].current.position = crate::position::FACEUP_ATTACK;
        id
    }

    /// **`GetLP` reads a player's life, and refuses one who does not
    /// exist** — answering zero rather than indexing off the end.
    #[test]
    fn get_lp_reads_life_and_guards_its_player() {
        let mut f = Field::new(8000);
        assert_eq!(get_lp(&f, 0), 8000);
        f.players[1].lp = 1234;
        assert_eq!(get_lp(&f, 1), 1234);
        assert_eq!(get_lp(&f, 2), 0, "there is no player 2");
    }

    /// **`GetTextAttack` is the printed attack**, and zero for a card
    /// with no printed line at all.
    ///
    /// `STATUS_NO_LEVEL` is what a token carries: it has no text to
    /// read, and answering with `data.attack` would report whatever
    /// happened to be in the struct.
    #[test]
    fn get_text_attack_is_the_printed_line() {
        let mut f = Field::new(8000);
        let c = monster(&mut f, 0, 1700);
        assert_eq!(get_text_attack(&f, c), 1700);
        // An effect changes what it is worth, not what it says.
        f.cards[c].assume.insert(crate::card::assume::ATTACK, 2600);
        assert_eq!(get_attack(&mut f, c), 2600, "what it is worth");
        assert_eq!(get_text_attack(&f, c), 1700, "what it says");
        // A card with no printed line reads zero.
        f.cards[c].set_status(status::NO_LEVEL, true);
        assert_eq!(get_text_attack(&f, c), 0);
    }

    /// **`Damage` clamps a negative to zero and refuses an impossible
    /// player.** A negative amount is not healing.
    #[test]
    fn damage_clamps_and_guards() {
        let mut f = Field::new(8000);
        assert!(damage(&mut f, 0, 500, crate::card::reason::EFFECT));
        match &f.core.subunits[0].kind {
            crate::processor::Kind::Damage { arg } => {
                assert_eq!(arg.playerid, 0);
                assert_eq!(arg.amount, 500);
            }
            other => panic!("expected a Damage: {other:?}"),
        }
        f.core.subunits.clear();

        assert!(damage(&mut f, 0, -300, crate::card::reason::EFFECT));
        match &f.core.subunits[0].kind {
            crate::processor::Kind::Damage { arg } => {
                assert_eq!(arg.amount, 0, "a negative is nothing, not healing");
            }
            other => panic!("expected a Damage: {other:?}"),
        }
        f.core.subunits.clear();

        assert!(
            !damage(&mut f, 2, 500, crate::card::reason::EFFECT),
            "there is no player 2"
        );
        assert!(f.core.subunits.is_empty(), "and nothing was queued");
    }
}

#[cfg(test)]
mod equip_and_label_tests {
    use super::*;
    use crate::card::{card_type, status, Card, CardData};
    use crate::effect::{effect_type, Effect};

    fn card(f: &mut Field, owner: u8, code_: u32, loc: u8, seq: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: code_,
                type_: card_type::MONSTER | card_type::NORMAL,
                level: 4,
                attack: 1000,
                defense: 1000,
                ..Default::default()
            },
            owner,
        );
        c.current.controller = owner;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(owner, id, loc, seq, false);
        id
    }

    /// **`Duel.Equip` queues the attachment with the player it was
    /// given**, and refuses a player who does not exist.
    ///
    /// The player decides whose Spell & Trap row the equip card is moved
    /// into and who is counted as doing it — which matters for a card
    /// activated from somewhere other than that row, and is worth
    /// pinning here because no card in this pool is.
    #[test]
    fn equip_passes_the_player_through_and_refuses_a_third() {
        let mut f = Field::new(8000);
        let e = card(&mut f, 0, 101, location::SZONE, 0);
        let t = card(&mut f, 1, 102, location::MZONE, 0);
        assert!(equip(&mut f, 1, e, t, true, false));
        match &f.core.subunits[0].kind {
            crate::processor::Kind::Equip {
                equip_player,
                equip_card,
                target,
                faceup,
                is_step,
            } => {
                assert_eq!(*equip_player, 1);
                assert_eq!(*equip_card, e);
                assert_eq!(*target, t);
                assert!(*faceup);
                assert!(!*is_step);
            }
            other => panic!("expected an Equip: {other:?}"),
        }

        let mut f = Field::new(8000);
        let e = card(&mut f, 0, 101, location::SZONE, 0);
        let t = card(&mut f, 1, 102, location::MZONE, 0);
        assert!(!equip(&mut f, 2, e, t, true, false), "no such player");
        assert!(f.core.subunits.is_empty(), "and nothing queued");
    }

    /// **`Card.GetFirstCardTarget` takes the first**, which is the one
    /// the oldest relationship recorded.
    #[test]
    fn get_first_card_target_takes_the_first() {
        let mut f = Field::new(8000);
        let owner = card(&mut f, 0, 101, location::SZONE, 0);
        assert_eq!(get_first_card_target(&f, owner), None, "none yet");
        let a = card(&mut f, 0, 102, location::MZONE, 0);
        let b = card(&mut f, 0, 103, location::MZONE, 1);
        f.cards[owner].effect_target_cards.push(a);
        f.cards[owner].effect_target_cards.push(b);
        assert_eq!(get_first_card_target(&f, owner), Some(a));
    }

    /// **`Effect.SetLabelObject` holds a card or an effect, and `nil`
    /// clears.**
    ///
    /// The two are different arenas behind the same `usize`, so the slot
    /// is typed: a card put in must not read back as an effect.
    #[test]
    fn set_label_object_stores_a_card_or_an_effect_and_clears() {
        let mut f = Field::new(8000);
        let c = card(&mut f, 0, 101, location::MZONE, 0);
        let mut x = Effect::new(effect_type::SINGLE, 0);
        x.owner = Some(c);
        x.handler = Some(c);
        let e = f.new_effect(x);
        let mut y = Effect::new(effect_type::SINGLE, 0);
        y.owner = Some(c);
        y.handler = Some(c);
        let other = f.new_effect(y);

        assert_eq!(get_label_object(f.effects.get(e).unwrap()), None);
        set_label_object(&mut f, e, Some(LabelObject::Card(c)));
        let got = get_label_object(f.effects.get(e).unwrap()).unwrap();
        assert_eq!(got.card(), Some(c));
        assert_eq!(got.effect(), None, "a card does not read back as one");

        set_label_object(&mut f, e, Some(LabelObject::Effect(other)));
        let got = get_label_object(f.effects.get(e).unwrap()).unwrap();
        assert_eq!(got.effect(), Some(other));
        assert_eq!(got.card(), None);

        set_label_object(&mut f, e, None);
        assert_eq!(
            get_label_object(f.effects.get(e).unwrap()),
            None,
            "passing nothing clears it"
        );
    }

    /// **`Duel.SpecialSummonStep` refuses a player who does not
    /// exist**, as the reference refuses it — `>= PLAYER_NONE`, so both
    /// 2 and the sentinel itself.
    #[test]
    fn special_summon_step_refuses_a_player_who_does_not_exist() {
        let mut f = Field::new(8000);
        let c = card(&mut f, 0, 101, location::GRAVE, 0);
        let mut x = Effect::new(effect_type::SINGLE, 0);
        x.owner = Some(c);
        let e = f.new_effect(x);
        f.core.reason_effect = Some(e);
        assert!(!special_summon_step(
            &mut f,
            c,
            0,
            2,
            0,
            false,
            false,
            crate::board::position::FACEUP_ATTACK
        ));
        assert!(f.core.subunits.is_empty(), "nothing queued");
        assert!(special_summon_step(
            &mut f,
            c,
            0,
            0,
            0,
            false,
            false,
            crate::board::position::FACEUP_ATTACK
        ));
        assert!(!f.core.subunits.is_empty(), "the sibling does queue");
    }

    /// **`Duel.SpecialSummonComplete` watches both pending sets.**
    ///
    /// `special_summoning` holds the cards that reached the field;
    /// `ss_tograve_set` holds the ones that could not be given a seat and
    /// are owed a trip to the graveyard. Either alone is work to do, and
    /// reading only the first leaves those cards where they are.
    #[test]
    fn special_summon_complete_watches_both_pending_sets() {
        let mut f = Field::new(8000);
        assert!(!special_summon_complete(&mut f), "nothing pending");
        assert!(f.core.subunits.is_empty());

        let mut f = Field::new(8000);
        let c = card(&mut f, 0, 101, location::GRAVE, 0);
        f.core.special_summoning.insert(c);
        assert!(special_summon_complete(&mut f));

        let mut f = Field::new(8000);
        let c = card(&mut f, 0, 101, location::GRAVE, 0);
        f.core.ss_tograve_set.insert(c);
        assert!(
            special_summon_complete(&mut f),
            "cards owed a graveyard are pending too"
        );
    }

    /// **`Duel.Recover` clamps a negative figure to zero and refuses a
    /// player who does not exist** — [`damage`]'s mirror, and the same
    /// two guards.
    #[test]
    fn recover_clamps_and_refuses_a_third_player() {
        let mut f = Field::new(8000);
        assert!(recover(&mut f, 0, 1000, reason::EFFECT));
        match &f.core.subunits[0].kind {
            crate::processor::Kind::Recover { arg } => {
                assert_eq!(arg.playerid, 0);
                assert_eq!(arg.amount, 1000);
            }
            other => panic!("expected a Recover: {other:?}"),
        }

        let mut f = Field::new(8000);
        assert!(recover(&mut f, 0, -500, reason::EFFECT));
        match &f.core.subunits[0].kind {
            crate::processor::Kind::Recover { arg } => {
                assert_eq!(arg.amount, 0, "a negative recovery is not a drain");
            }
            other => panic!("expected a Recover: {other:?}"),
        }

        let mut f = Field::new(8000);
        assert!(!recover(&mut f, 2, 1000, reason::EFFECT));
        assert!(f.core.subunits.is_empty(), "and nothing queued");
    }

    /// **`Duel.GetLocationCount`'s reason changes the answer.**
    ///
    /// Counting seats "to put a card there" and "to take one over" are
    /// different counts — a `MAX_MZONE` cap that applies only to one
    /// reason is what makes them differ, and a caller that passes the
    /// wrong one gets a plausible number rather than an error.
    #[test]
    fn the_seat_count_depends_on_the_reason_given() {
        let mut f = Field::new(8000);
        let plain = get_location_count_for(&mut f, 0, location::MZONE, 0, LOCATION_REASON_TOFIELD);
        assert_eq!(plain, 5, "an empty row");
        assert_eq!(
            get_location_count_for(&mut f, 0, location::MZONE, 0, LOCATION_REASON_CONTROL),
            5
        );
        assert_eq!(
            get_location_count_for(&mut f, 2, location::MZONE, 0, LOCATION_REASON_TOFIELD),
            0,
            "no such player"
        );

        // A cap that applies only to a control change. The source sits
        // in the Spell & Trap row so that it does not occupy one of the
        // Monster Zones being counted.
        let mut source = Card::new(1, 0);
        source.current.controller = 0;
        source.current.location = location::SZONE;
        source.current.position = crate::board::position::FACEUP;
        source.set_status(status::EFFECT_ENABLED, true);
        let source = f.new_card(source);
        let mut cap = Effect::new(effect_type::FIELD, code::MAX_MZONE);
        cap.owner = Some(source);
        cap.handler = Some(source);
        cap.effect_owner = 0;
        // `FUNC_VALUE` as well as the function itself: without the flag
        // `get_value` reads the constant and the cap silently becomes
        // zero. `set_value_fn` sets both, which is why a card never has
        // to think about it.
        cap.flag[0] |= flag::PLAYER_TARGET | flag::FUNC_VALUE;
        cap.range = u16::from(location::SZONE);
        cap.s_range = u16::from(location::SZONE);
        cap.value_fn = Some(only_for_a_control_change);
        let id = f.new_effect(cap);
        f.field_effects.aura.insert(code::MAX_MZONE, id);
        f.field_effects.indexer.insert(id);

        assert_eq!(
            get_location_count_for(&mut f, 0, location::MZONE, 0, LOCATION_REASON_CONTROL),
            1,
            "the cap applies"
        );
        assert_eq!(
            get_location_count_for(&mut f, 0, location::MZONE, 0, LOCATION_REASON_TOFIELD),
            5,
            "and not to the other reason"
        );
    }

    /// A `MAX_MZONE` cap of one, but only when the reason is a control
    /// change. `args` is `(playerid, uplayer, reason)`.
    fn only_for_a_control_change(_e: &Effect, _f: &Field, ctx: &Ctx) -> i64 {
        if ctx.args.get(2) == Some(&i64::from(LOCATION_REASON_CONTROL)) {
            1
        } else {
            5
        }
    }

    /// **`Card.AddCounter` is guarded by immunity**, before the counter
    /// machinery is asked anything at all.
    #[test]
    fn add_counter_refuses_a_card_the_effect_cannot_touch() {
        for immune in [false, true] {
            let mut f = Field::new(8000);
            let c = card(&mut f, 0, 101, location::MZONE, 0);
            f.cards[c].current.position = crate::board::position::FACEUP_ATTACK;
            // Permitted to hold the counter, so only immunity decides.
            enable_counter_permit(&mut f, c, 0x1, None);

            let mut x = Effect::new(effect_type::SINGLE, 0);
            x.owner = Some(c);
            x.handler = Some(c);
            let by = f.new_effect(x);
            f.core.reason_effect = Some(by);

            if immune {
                let mut imm = Effect::new(effect_type::SINGLE, code::IMMUNE_EFFECT);
                imm.owner = Some(c);
                imm.handler = Some(c);
                imm.value = 1;
                let imm = f.new_effect(imm);
                f.cards[c].immune_effect.push(imm);
                f.cards[c].indexer.insert(imm);
            }

            let placed = add_counter(&mut f, c, 0x1, 1, false);
            assert_eq!(placed, !immune, "immune {immune}");
            assert_eq!(get_counter(&f, c, 0x1), u16::from(!immune));
        }
    }

    /// **A counter type of zero is an error in the reference**, not "all
    /// of them" — there are separate exports for that. Both the read and
    /// the removal refuse it.
    #[test]
    fn a_counter_type_of_zero_is_refused() {
        let mut f = Field::new(8000);
        let c = card(&mut f, 0, 101, location::MZONE, 0);
        f.cards[c].counters.insert(0x1, [3, 0]);
        assert_eq!(get_counter(&f, c, 0x1), 3, "the positive sibling");
        // Zero is **refused**, not looked up. Storing something under the
        // key is what tells the two apart: a lookup of a key that is not
        // there also answers zero, so an empty map proves nothing.
        f.cards[c].counters.insert(0, [7, 0]);
        assert_eq!(get_counter(&f, c, 0), 0, "refused, not read");
        assert!(!remove_counter(&mut f, c, 0, 0, 1, reason::COST));
        assert!(f.core.subunits.is_empty(), "and nothing queued");
        assert!(remove_counter(&mut f, c, 0, 0x1, 1, reason::COST));
        assert!(!f.core.subunits.is_empty());
    }

    /// **`Card.IsCanRemoveCounter` refuses a player who does not
    /// exist**, as the reference refuses it.
    #[test]
    fn is_can_remove_counter_refuses_a_third_player() {
        let mut f = Field::new(8000);
        let c = card(&mut f, 0, 101, location::MZONE, 0);
        f.cards[c].counters.insert(0x1, [3, 0]);
        assert!(is_can_remove_counter(&mut f, c, 0, 0x1, 1, reason::COST));
        assert!(!is_can_remove_counter(&mut f, c, 2, 0x1, 1, reason::COST));
    }

    /// **`Effect.IsTrapEffect` reads the *active* type**, not the
    /// handler's printed one.
    ///
    /// The two differ for a card whose effect was activated as something
    /// else — which is the whole point of there being an active type —
    /// and the port's `get_active_type` is what carries it.
    #[test]
    fn is_trap_effect_reads_the_active_type() {
        let mut f = Field::new(8000);
        // A card printed as a **Spell**, whose effect was activated as a
        // Trap. `active_type` is the answer when it is set; failing that
        // `get_active_type` falls back to the handler's type, which is
        // why the fallback and the override need separate boards.
        let c = card(&mut f, 0, 101, location::SZONE, 0);
        f.cards[c].data.type_ = crate::card::card_type::SPELL;
        let mut e = Effect::new(effect_type::ACTIVATE, code::FREE_CHAIN);
        e.owner = Some(c);
        e.handler = Some(c);
        e.active_type = crate::card::card_type::TRAP;
        let e = f.new_effect(e);
        assert!(is_trap_effect(&mut f, e), "activated as a Trap");
        assert!(
            !is_trap_readonly(&f, c),
            "though the card is printed as a Spell"
        );
    }

    /// **The triggering location drops the symbolic zone bits**, so a
    /// card in a Spell & Trap Zone answers `LOCATION_SZONE` rather than
    /// the symbolic `LOCATION_STZONE`.
    #[test]
    fn the_triggering_location_is_not_symbolic() {
        let mut f = Field::new(8000);
        let c = card(&mut f, 0, 101, location::SZONE, 0);
        let mut e = Effect::new(effect_type::ACTIVATE, code::FREE_CHAIN);
        e.owner = Some(c);
        e.handler = Some(c);
        let e = f.new_effect(e);
        let mut ch = Chain::new(e, crate::event::Event::new(code::FREE_CHAIN));
        ch.chain_count = 1;
        ch.triggering_location = u16::from(location::SZONE) | location::STZONE;
        f.core.current_chain.push(ch);
        assert_eq!(
            get_chain_triggering_location(&f, 1),
            u16::from(location::SZONE),
            "the real zone, not the symbolic one"
        );
        assert_eq!(get_chain_triggering_effect(&f, 1), Some(e));
        assert_eq!(get_chain_triggering_location(&f, 9), 0, "no such link");
    }

    /// **A flag effect is addressed by its code**, which carries a bit
    /// no ordinary effect has — so a flag with id 2 and a real effect
    /// with code 2 do not collide.
    #[test]
    fn a_flag_effect_is_addressed_by_a_masked_code() {
        let mut f = Field::new(8000);
        let c = card(&mut f, 0, 101, location::MZONE, 0);
        // A *real* single effect with the same bare number.
        let mut real = Effect::new(effect_type::SINGLE, 2);
        real.owner = Some(c);
        real.handler = Some(c);
        let real = f.new_effect(real);
        f.cards[c].single_effect.insert(2, real);
        f.cards[c].indexer.insert(real);

        assert_eq!(get_flag_effect(&f, c, 2), 0, "no flag yet");
        register_flag_effect(&mut f, c, 2, reset::EVENT | reset::TOGRAVE, 0, 1, 0);
        assert_eq!(get_flag_effect(&f, c, 2), 1, "and now one");
        assert!(has_flag_effect(&f, c, 2));
        assert_eq!(
            f.cards[c].single_effect.equal_range(2).len(),
            1,
            "the real effect is untouched"
        );

        // Resetting the flag leaves the real effect alone.
        reset_flag_effect(&mut f, c, 2);
        assert!(!has_flag_effect(&f, c, 2));
        assert_eq!(f.cards[c].single_effect.equal_range(2).len(), 1);
    }

    /// **The three things `RegisterFlagEffect` fixes for the caller**: it
    /// is always undisablable, a count of zero means one, and a phase
    /// reset that names neither turn gets both.
    #[test]
    fn register_flag_effect_fills_in_the_references_defaults() {
        let mut f = Field::new(8000);
        let c = card(&mut f, 0, 101, location::MZONE, 0);
        let e = register_flag_effect(
            &mut f,
            c,
            7,
            reset::PHASE | u32::from(crate::duel::phases::END),
            0,
            0,
            0,
        );
        let x = f.effects.get(e).unwrap();
        assert!(x.is_flag(flag::CANNOT_DISABLE), "always");
        assert_eq!(x.reset_count, 1, "a count of zero means one");
        assert_ne!(x.reset_flag & reset::SELF_TURN, 0, "both turns");
        assert_ne!(x.reset_flag & reset::OPPO_TURN, 0);
        assert!(x.is_type(effect_type::SINGLE));

        // A reset that is not a phase reset gets neither turn added.
        let e = register_flag_effect(&mut f, c, 8, reset::EVENT | reset::TOGRAVE, 0, 3, 0);
        let x = f.effects.get(e).unwrap();
        assert_eq!(x.reset_count, 3, "a count given is kept");
        assert_eq!(x.reset_flag & (reset::SELF_TURN | reset::OPPO_TURN), 0);

        // The count clamp, asked where it is the only thing doing it.
        // Registration bumps a **phase** effect's count from zero on its
        // own (`add_card_effect` step 8), so the phase case above cannot
        // tell the export's clamp from the registration's; a non-phase
        // reset can.
        let e = register_flag_effect(&mut f, c, 9, reset::EVENT | reset::TOGRAVE, 0, 0, 0);
        assert_eq!(
            f.effects.get(e).unwrap().reset_count,
            1,
            "zero means one, and nothing else here would have made it so"
        );

        // The id is masked to 28 bits before the flag bit goes on, so a
        // high bit in the id cannot masquerade as the flag bit.
        assert_eq!(super::flag_code(0xf000_0003), FLAG_EFFECT | 3);
        assert_eq!(super::flag_code(3), FLAG_EFFECT | 3, "and agree");
    }

    /// **`Duel.GetMZoneCount` counts for putting a card down**, which is
    /// the reference's default reason — not for a control change.
    ///
    /// The two differ only under a cap that applies to one and not the
    /// other, which is the only thing that makes the argument observable.
    #[test]
    fn get_mzone_count_counts_for_putting_a_card_down() {
        let mut f = Field::new(8000);
        let c = card(&mut f, 0, 101, location::MZONE, 0);
        f.cards[c].current.position = crate::board::position::FACEUP_ATTACK;
        let mut cap = Effect::new(effect_type::FIELD, code::MAX_MZONE);
        cap.owner = Some(c);
        cap.handler = Some(c);
        cap.effect_owner = 0;
        cap.flag[0] |= flag::PLAYER_TARGET | flag::FUNC_VALUE;
        cap.range = u16::from(location::MZONE);
        cap.s_range = u16::from(location::MZONE);
        cap.value_fn = Some(one_for_a_control_change);
        let id = f.new_effect(cap);
        f.field_effects.aura.insert(code::MAX_MZONE, id);
        f.field_effects.indexer.insert(id);

        assert_eq!(
            get_mzone_count(&mut f, 0, Except::Card(c), None, None),
            5,
            "the to-field reason, so the cap does not apply"
        );
        assert_eq!(
            get_mzone_count(&mut f, 2, Except::Card(c), None, None),
            0,
            "no such player"
        );
    }

    fn one_for_a_control_change(_e: &Effect, _f: &Field, ctx: &Ctx) -> i64 {
        if ctx.args.get(2) == Some(&i64::from(LOCATION_REASON_CONTROL)) {
            1
        } else {
            5
        }
    }

    /// **`Card.IsReason` is a mask test, not an equality test** — a card
    /// destroyed *by an effect* carries both bits, and asking about one
    /// of them must still answer yes.
    #[test]
    fn is_reason_tests_the_mask() {
        let mut f = Field::new(8000);
        let c = card(&mut f, 0, 101, location::GRAVE, 0);
        f.cards[c].reason = reason::DESTROY | reason::EFFECT;
        assert!(is_reason(&f, c, reason::DESTROY));
        assert!(is_reason(&f, c, reason::EFFECT));
        assert!(!is_reason(&f, c, reason::BATTLE));
        f.cards[c].reason = reason::EFFECT;
        assert!(!is_reason(&f, c, reason::DESTROY), "and only what is set");
    }
}

#[cfg(test)]
mod select_unselect_tests {
    use super::*;
    use crate::board::location;
    use crate::card::{attribute, card_type, status, Card, CardData};
    use crate::field::Message;

    fn monster(f: &mut Field, owner: u8, seat: u32, attr: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 7000 + seat,
                type_: card_type::MONSTER | card_type::NORMAL,
                level: 4,
                attack: 1000,
                defense: 1000,
                attribute: attr,
                ..Default::default()
            },
            owner,
        );
        c.current.controller = owner;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(owner, id, location::MZONE, seat, false);
        f.cards[id].current.position = crate::position::FACEUP_ATTACK;
        id
    }

    fn board() -> (Field, Vec<CardId>) {
        let mut f = Field::new(8000);
        let cards = (0..3)
            .map(|i| monster(&mut f, 0, i, attribute::LIGHT))
            .collect();
        (f, cards)
    }

    /// player, finishable, cancelable, min, max, on offer, already taken.
    type Question = (u8, bool, bool, u8, u8, Vec<CardId>, Vec<CardId>);

    fn last_question(f: &Field) -> Option<Question> {
        f.messages.iter().rev().find_map(|m| match m {
            Message::SelectUnselectCard {
                player,
                finishable,
                cancelable,
                min,
                max,
                select,
                unselect,
            } => Some((
                *player,
                *finishable,
                *cancelable,
                *min,
                *max,
                select.clone(),
                unselect.clone(),
            )),
            _ => None,
        })
    }

    #[test]
    fn the_two_lists_reach_the_host_as_they_were_given() {
        let (mut f, g) = board();
        assert!(select_unselect(
            &mut f,
            &g[1..],
            &g[..1],
            1,
            true,
            false,
            1,
            2
        ));
        f.process();
        let q = last_question(&f).expect("a question");
        assert_eq!(q.0, 1, "asked of the named player");
        assert!(q.1, "finishable");
        assert!(!q.2, "not cancelable");
        assert_eq!((q.3, q.4), (1, 2));
        assert_eq!(q.5, g[1..].to_vec(), "on offer");
        assert_eq!(q.6, g[..1].to_vec(), "already taken");
    }

    /// **The export refuses outright when the two lists overlap**, rather
    /// than asking a question in which one card means two things.
    #[test]
    fn overlapping_lists_are_refused() {
        let (mut f, g) = board();
        assert!(!select_unselect(&mut f, &g, &g[..1], 0, false, false, 1, 2));
        f.process();
        assert!(last_question(&f).is_none(), "nothing was asked");
    }

    /// `if(min > max) min = max;` — clamped, not refused.
    #[test]
    fn a_min_above_max_is_clamped() {
        let (mut f, g) = board();
        assert!(select_unselect(&mut f, &g, &[], 0, false, false, 3, 1));
        f.process();
        let q = last_question(&f).expect("a question");
        assert_eq!((q.3, q.4), (1, 1), "min came down to max");
    }

    #[test]
    fn there_is_no_third_player_to_ask() {
        let (mut f, g) = board();
        assert!(!select_unselect(&mut f, &g, &[], 2, false, false, 1, 1));
    }

    /// **A cancel is `nil`, not an empty group.** `group_selected` cannot
    /// tell the two apart; the loop's break depends on it.
    #[test]
    fn selected_one_reads_a_cancel_as_nothing() {
        let (mut f, g) = board();
        f.core.return_cards.list = vec![g[0]];
        f.core.return_cards.canceled = false;
        assert_eq!(selected_one(&f), Some(g[0]));
        f.core.return_cards.canceled = true;
        assert_eq!(selected_one(&f), None, "a cancel chose nothing");
    }

    #[test]
    fn a_group_handle_gives_its_cards_back() {
        let (mut f, g) = board();
        let handle = new_group(&mut f, [g[2], g[0]]);
        assert_eq!(group_cards(&f, handle), vec![g[0], g[2]], "in cardid order");
    }

    /// **Nothing passed is not the same as an empty group.** The
    /// reference only swaps the zone rows when a card or a group was
    /// actually given, and the swap rebuilds `used_location` from the
    /// rows.
    #[test]
    fn no_exclusion_and_an_empty_one_are_different_arguments() {
        let (mut f, g) = board();
        let all = get_mzone_count(&mut f, 0, Except::None, None, None);
        assert_eq!(all, 2, "three seats of five are taken");
        assert_eq!(
            get_mzone_count(&mut f, 0, Except::Group(&g), None, None),
            5,
            "taking all three out frees the row"
        );
        assert_eq!(
            get_mzone_count(&mut f, 0, Except::Card(g[0]), None, None),
            3,
            "or one of them"
        );
    }
}

#[cfg(test)]
mod spelim_and_revive_tests {
    use super::*;
    use crate::board::location;
    use crate::card::{card_type, status, Card, CardData};

    /// Spirit Elimination, which `aux.SpElimFilter` asks after by number.
    const SPIRIT_ELIMINATION_CODE: u32 = 69832741;

    fn card_in(f: &mut Field, owner: u8, loc: u8, seq: u32, type_: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 4000 + seq,
                type_,
                level: 4,
                attack: 1000,
                defense: 1000,
                ..Default::default()
            },
            owner,
        );
        c.current.controller = owner;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(owner, id, loc, seq, false);
        f.cards[id].current.position = if loc == location::MZONE {
            crate::position::FACEUP_ATTACK
        } else {
            crate::position::FACEUP
        };
        id
    }

    /// A player-wide effect carrying Spirit Elimination's card number,
    /// which is what the filter looks for.
    fn spirit_elimination_applies_to(f: &mut Field, player: u8) {
        let holder = card_in(f, player, location::SZONE, 0, card_type::SPELL);
        let e = create_effect(f, holder);
        set_type(f, e, effect_type::FIELD);
        set_code(f, e, SPIRIT_ELIMINATION_CODE);
        set_range(f, e, u16::from(location::SZONE));
        // `EFFECT_FLAG_PLAYER_TARGET` is what makes the ranges read as
        // *players* rather than as zones — without it `is_target_player`
        // refuses outright and the aura is never found.
        set_property(f, e, flag::PLAYER_TARGET, 0);
        set_target_range(f, e, 1, 0);
        register_effect(f, holder, e, false);
    }

    mod sp_elim_filter {
        use super::*;

        /// Without Spirit Elimination the graveyard is the pile that
        /// counts, and the field is not.
        #[test]
        fn the_graveyard_is_the_default_pile() {
            let mut f = Field::new(8000);
            let grave = card_in(&mut f, 0, location::GRAVE, 0, card_type::MONSTER);
            let field = card_in(&mut f, 0, location::MZONE, 0, card_type::MONSTER);
            assert!(sp_elim_filter(&mut f, grave, true, false));
            assert!(!sp_elim_filter(&mut f, field, true, false));
        }

        /// **With Spirit Elimination applying, the two piles swap.**
        #[test]
        fn spirit_elimination_swaps_the_piles() {
            let mut f = Field::new(8000);
            let grave = card_in(&mut f, 0, location::GRAVE, 0, card_type::MONSTER);
            let field = card_in(&mut f, 0, location::MZONE, 0, card_type::MONSTER);
            spirit_elimination_applies_to(&mut f, 0);
            assert!(!sp_elim_filter(&mut f, grave, true, false));
            assert!(sp_elim_filter(&mut f, field, true, false));
        }

        /// It is the **controller** who is asked about, so the opponent's
        /// graveyard is unaffected.
        #[test]
        fn only_the_affected_player_loses_the_graveyard() {
            let mut f = Field::new(8000);
            let mine = card_in(&mut f, 0, location::GRAVE, 0, card_type::MONSTER);
            let theirs = card_in(&mut f, 1, location::GRAVE, 0, card_type::MONSTER);
            spirit_elimination_applies_to(&mut f, 0);
            assert!(!sp_elim_filter(&mut f, mine, true, false));
            assert!(sp_elim_filter(&mut f, theirs, true, false));
        }

        /// `mustbefaceup` only bites on the field — a face-down monster
        /// in the graveyard is not a thing.
        #[test]
        fn mustbefaceup_refuses_a_face_down_monster_on_the_field() {
            let mut f = Field::new(8000);
            let field = card_in(&mut f, 0, location::MZONE, 0, card_type::MONSTER);
            f.cards[field].current.position = crate::position::FACEDOWN_DEFENSE;
            spirit_elimination_applies_to(&mut f, 0);
            assert!(!sp_elim_filter(&mut f, field, true, false));
            assert!(
                sp_elim_filter(&mut f, field, false, false),
                "and passes when the caller does not care"
            );
        }

        /// A non-monster takes the other arm entirely: the graveyard, or
        /// anywhere at all when the caller allows the field.
        #[test]
        fn a_non_monster_is_judged_by_location_alone() {
            let mut f = Field::new(8000);
            let spell_grave = card_in(&mut f, 0, location::GRAVE, 1, card_type::SPELL);
            let spell_field = card_in(&mut f, 0, location::SZONE, 1, card_type::SPELL);
            assert!(sp_elim_filter(&mut f, spell_grave, true, false));
            assert!(!sp_elim_filter(&mut f, spell_field, true, false));
            assert!(
                sp_elim_filter(&mut f, spell_field, true, true),
                "includemzone lets it through wherever it is"
            );
        }
    }

    mod enable_revive_limit {
        use super::*;

        #[test]
        fn it_registers_both_halves() {
            let mut f = Field::new(8000);
            let c = card_in(&mut f, 0, location::HAND, 0, card_type::MONSTER);
            enable_revive_limit(&mut f, c);
            for wanted in [code::UNSUMMONABLE_CARD, code::REVIVE_LIMIT] {
                let found = f.cards[c].single_effect.equal_range(wanted);
                assert_eq!(found.len(), 1, "one effect with code {wanted}");
                let e = f.effects.get(found[0]).expect("the effect");
                assert!(e.is_flag(flag::CANNOT_DISABLE), "cannot be disabled");
                assert!(e.is_flag(flag::UNCOPYABLE), "cannot be copied");
            }
        }

        /// A card copying another's effects registers neither.
        #[test]
        fn a_copying_card_registers_nothing() {
            let mut f = Field::new(8000);
            let c = card_in(&mut f, 0, location::HAND, 0, card_type::MONSTER);
            f.cards[c].set_status(crate::card::status::COPYING_EFFECT, true);
            enable_revive_limit(&mut f, c);
            assert!(f.cards[c]
                .single_effect
                .equal_range(code::REVIVE_LIMIT)
                .is_empty());
        }
    }

    #[test]
    fn the_attack_announced_count_is_read_off_the_card() {
        let mut f = Field::new(8000);
        let c = card_in(&mut f, 0, location::MZONE, 0, card_type::MONSTER);
        assert_eq!(get_attack_announced_count(&f, c), 0);
        f.cards[c].attack_announce_count = 2;
        assert_eq!(get_attack_announced_count(&f, c), 2);
    }
}

#[cfg(test)]
mod chain_attack_tests {
    use super::*;
    use crate::board::location;
    use crate::card::{card_type, status, Card, CardData};

    fn monster(f: &mut Field, owner: u8, seat: u32, atk: i32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 8000 + seat + u32::from(owner) * 100,
                type_: card_type::MONSTER | card_type::NORMAL,
                level: 4,
                attack: atk,
                defense: 1000,
                ..Default::default()
            },
            owner,
        );
        c.current.controller = owner;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(owner, id, location::MZONE, seat, false);
        f.cards[id].current.position = crate::position::FACEUP_ATTACK;
        id
    }

    /// Mid-battle: `attacker` has declared, the opponent has a monster,
    /// and the identity bookkeeping the check reads is consistent.
    fn mid_battle() -> (Field, CardId, CardId) {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        f.infos.turn_player = 0;
        let a = monster(&mut f, 0, 0, 2000);
        let d = monster(&mut f, 1, 0, 1000);
        f.core.attacker = Some(a);
        f.core.pre_field[0] = f.cards[a].fieldid_r;
        f.cards[a].announce_count = 1;
        (f, a, d)
    }

    mod can_chain_attack {
        use super::*;

        #[test]
        fn a_monster_that_attacked_once_may_attack_again() {
            let (mut f, a, _) = mid_battle();
            assert!(can_chain_attack(&mut f, a, None, false));
        }

        /// **Two identity checks, and neither is about ability.** The card
        /// must still be `core.attacker`, and its `fieldid_r` must still
        /// match `pre_field[0]` — a monster that left and came back is a
        /// different monster to this battle.
        #[test]
        fn it_must_still_be_the_monster_that_attacked() {
            let (mut f, a, d) = mid_battle();
            f.core.attacker = Some(d);
            assert!(
                !can_chain_attack(&mut f, a, None, false),
                "not the attacker"
            );

            let (mut f, a, _) = mid_battle();
            f.core.pre_field[0] = f.cards[a].fieldid_r + 1;
            assert!(
                !can_chain_attack(&mut f, a, None, false),
                "left and came back"
            );
        }

        #[test]
        fn a_destroyed_attacker_may_not() {
            let (mut f, a, _) = mid_battle();
            f.cards[a].set_status(status::BATTLE_DESTROYED, true);
            assert!(!can_chain_attack(&mut f, a, None, false));
        }

        #[test]
        fn it_must_be_its_controllers_turn() {
            let (mut f, a, _) = mid_battle();
            f.infos.turn_player = 1;
            assert!(!can_chain_attack(&mut f, a, None, false));
        }

        /// **`ac` is an attack budget, and the default is two.** Zero
        /// means "no budget at all", which is not the same as zero
        /// attacks — it switches the test off.
        #[test]
        fn the_budget_defaults_to_two_and_zero_switches_it_off() {
            let (mut f, a, _) = mid_battle();
            f.cards[a].announce_count = 2;
            assert!(
                !can_chain_attack(&mut f, a, None, false),
                "two announced already spends the default budget"
            );
            assert!(
                can_chain_attack(&mut f, a, Some(0), false),
                "a budget of zero is no budget"
            );
            assert!(
                can_chain_attack(&mut f, a, Some(3), false),
                "and a larger one still has room"
            );
        }

        /// The budget is spent **at** the count, not past it.
        #[test]
        fn the_budget_is_reached_not_exceeded() {
            let (mut f, a, _) = mid_battle();
            f.cards[a].announce_count = 1;
            assert!(can_chain_attack(&mut f, a, Some(2), false));
            f.cards[a].announce_count = 2;
            assert!(!can_chain_attack(&mut f, a, Some(2), false));
        }

        /// **An extra attack already granted blocks this one — but only
        /// at the default budget.** The two routes to a second attack do
        /// not stack, and a caller naming its own budget is saying it
        /// knows better.
        #[test]
        fn an_extra_attack_blocks_only_the_default_budget() {
            let (mut f, a, _) = mid_battle();
            let mut e = Effect::new(effect_type::SINGLE, code::EXTRA_ATTACK);
            e.owner = Some(a);
            e.handler = Some(a);
            let id = f.new_effect(e);
            f.cards[a].single_effect.insert(code::EXTRA_ATTACK, id);
            f.cards[a].indexer.insert(id);
            assert!(!can_chain_attack(&mut f, a, None, false), "at ac = 2");
            assert!(can_chain_attack(&mut f, a, Some(3), false), "but not at 3");
        }

        /// The battle system's own answer is asked too: a monster under a
        /// standing prohibition cannot announce an attack, chain or not.
        #[test]
        fn a_monster_that_may_not_announce_an_attack_is_refused() {
            let (mut f, a, _) = mid_battle();
            let mut e = Effect::new(effect_type::SINGLE, code::CANNOT_ATTACK);
            e.owner = Some(a);
            e.handler = Some(a);
            let id = f.new_effect(e);
            f.cards[a].single_effect.insert(code::CANNOT_ATTACK, id);
            f.cards[a].indexer.insert(id);
            assert!(!can_chain_attack(&mut f, a, None, false));
        }

        /// **Nothing to attack is a refusal** — unless the card can go
        /// straight for the player, which `monsteronly` overrides.
        #[test]
        fn an_empty_board_refuses_unless_it_can_attack_directly() {
            let (mut f, a, d) = mid_battle();
            f.remove_card(d);
            assert!(
                can_chain_attack(&mut f, a, None, false),
                "an empty opponent board means a direct attack"
            );
            assert!(
                !can_chain_attack(&mut f, a, None, true),
                "monsteronly wants a monster"
            );
        }
    }

    mod chain_attack {
        use super::*;

        #[test]
        fn it_records_the_attacker_by_field_id() {
            let (mut f, a, _) = mid_battle();
            chain_attack(&mut f, None);
            assert!(
                f.core.chain_attack,
                "the flag is what the battle phase reads"
            );
            assert_eq!(
                f.core.chain_attacker_id, f.cards[a].fieldid,
                "the identity, not the handle"
            );
            assert_eq!(f.core.chain_attack_target, None);
        }

        /// **A named target is kept; no target leaves whatever was there.**
        /// The reference only assigns when an argument was passed.
        #[test]
        fn a_named_target_is_kept_and_none_leaves_the_slot_alone() {
            let (mut f, _, d) = mid_battle();
            chain_attack(&mut f, Some(d));
            assert_eq!(f.core.chain_attack_target, Some(d));
            chain_attack(&mut f, None);
            assert_eq!(
                f.core.chain_attack_target,
                Some(d),
                "the slot is not cleared by a bare call"
            );
        }

        /// **And when the reason effect cannot touch the attacker.** A
        /// monster mid-summon is immune to everything but the two
        /// summon-negation codes, which is the cheapest way to say so.
        #[test]
        fn an_attacker_the_effect_cannot_affect_is_not_granted_one() {
            let (mut f, a, _) = mid_battle();
            let mut e = Effect::new(effect_type::SINGLE, code::CANNOT_ATTACK);
            e.owner = Some(a);
            e.handler = Some(a);
            let reason = f.new_effect(e);
            f.core.reason_effect = Some(reason);
            f.cards[a].set_status(crate::card::status::SUMMONING, true);
            chain_attack(&mut f, None);
            assert!(!f.core.chain_attack);

            f.cards[a].set_status(crate::card::status::SUMMONING, false);
            chain_attack(&mut f, None);
            assert!(f.core.chain_attack, "the positive sibling");
        }

        /// **It refuses silently** when there is no attacker, so a card
        /// calling it is not promised a second attack.
        #[test]
        fn with_no_attacker_it_does_nothing() {
            let mut f = Field::new(8000);
            chain_attack(&mut f, None);
            assert!(!f.core.chain_attack);
        }
    }
}

#[cfg(test)]
mod token_and_activity_tests {
    use super::*;
    use crate::board::location;
    use crate::card::{card_type, status, Card, CardData};

    fn monster(f: &mut Field, owner: u8, seq: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 2200 + seq,
                type_: card_type::MONSTER | card_type::NORMAL,
                level: 4,
                ..Default::default()
            },
            owner,
        );
        c.current.controller = owner;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(owner, id, location::MZONE, seq, false);
        f.cards[id].current.position = crate::position::FACEUP_ATTACK;
        id
    }

    /// **Six tallies, each its own counter.** `ACTIVITY_SUMMON` is not
    /// `ACTIVITY_NORMALSUMMON` and neither is `ACTIVITY_SPSUMMON`; a card
    /// asking "have you summoned this turn" means a different question by
    /// each.
    #[test]
    fn each_activity_reads_its_own_tally() {
        let mut f = Field::new(8000);
        f.core.summon_state_count = [1, 0];
        f.core.normalsummon_state_count = [2, 0];
        f.core.spsummon_state_count = [3, 0];
        f.core.flipsummon_state_count = [4, 0];
        f.core.attack_state_count = [5, 0];
        f.core.battle_phase_count = [6, 0];
        for (what, want) in [
            (activity::SUMMON, 1),
            (activity::NORMALSUMMON, 2),
            (activity::SPSUMMON, 3),
            (activity::FLIPSUMMON, 4),
            (activity::ATTACK, 5),
            (activity::BATTLE_PHASE, 6),
        ] {
            assert_eq!(get_activity_count(&f, 0, what), want, "activity {what}");
            assert_eq!(get_activity_count(&f, 1, what), 0, "the other player's");
        }
    }

    #[test]
    fn there_is_no_third_players_tally() {
        let mut f = Field::new(8000);
        f.core.summon_state_count = [1, 1];
        assert_eq!(get_activity_count(&f, 2, activity::SUMMON), 0);
    }

    mod create_token {
        use super::*;

        /// **A token is made for the player who asked, and put nowhere.**
        /// The caller summons it; until then it is in no zone at all.
        #[test]
        fn it_belongs_to_the_asking_player_and_is_in_no_zone() {
            let mut f = Field::new(8000);
            let t =
                create_token(&mut f, 1, crate::cards::scapegoat::TOKEN_CODES[0]).expect("a token");
            assert_eq!(f.cards[t].owner, 1);
            assert_eq!(f.cards[t].current.controller, 1, "theirs, not ours");
            assert_eq!(f.cards[t].current.location, 0, "and nowhere yet");
            assert_eq!(
                f.cards[t].data.race,
                crate::card::race::BEAST,
                "with the printed line the database holds"
            );
        }

        #[test]
        fn a_code_with_no_printed_line_makes_nothing() {
            let mut f = Field::new(8000);
            assert!(create_token(&mut f, 0, 1).is_none());
        }
    }

    mod monster_shape {
        use super::*;

        /// **The described shape overrides the database.** Asked about a
        /// Spell's code with a monster type, the question is about a
        /// monster — which is how a token with no entry of its own is
        /// still a well-formed question.
        #[test]
        fn the_description_wins_over_the_database() {
            fn only_monsters(f: &Field, _e: EffectId, c: Option<CardId>, _a: &[i64]) -> bool {
                c.is_some_and(|c| f.cards[c].data.is_type(card_type::MONSTER))
            }
            let mut f = Field::new(8000);
            let holder = monster(&mut f, 0, 0);
            let e = create_effect(&mut f, holder);
            set_type(&mut f, e, effect_type::FIELD);
            set_code(&mut f, e, code::CANNOT_SPECIAL_SUMMON);
            set_range(&mut f, e, u16::from(location::MZONE));
            set_property(&mut f, e, flag::PLAYER_TARGET, 0);
            set_target_range(&mut f, e, 1, 0);
            set_target_filter(&mut f, e, only_monsters);
            register_effect(&mut f, holder, e, false);

            // Pot of Greed's code, described as a monster: the
            // monster-refusing prohibition has to bite.
            assert!(
                !is_player_can_special_summon_monster(
                    &mut f,
                    0,
                    crate::cards::pot_of_greed::CODE,
                    card_type::MONSTER | card_type::NORMAL,
                    0,
                    0,
                    1,
                    crate::card::race::BEAST,
                    crate::card::attribute::EARTH,
                ),
                "described as a monster, so refused as one"
            );
            assert!(
                is_player_can_special_summon_monster(
                    &mut f,
                    0,
                    crate::cards::pot_of_greed::CODE,
                    card_type::SPELL,
                    0,
                    0,
                    0,
                    0,
                    0,
                ),
                "and described as a Spell, it slips past"
            );
        }

        /// **The question is asked as the named player**, so a
        /// prohibition that reads the controller sees the right side.
        #[test]
        fn the_shape_is_owned_by_the_player_asked_about() {
            fn refuses_player_one(f: &Field, _e: EffectId, c: Option<CardId>, _a: &[i64]) -> bool {
                c.is_some_and(|c| f.cards[c].current.controller == 1)
            }
            let mut f = Field::new(8000);
            for p in [0u8, 1u8] {
                let holder = monster(&mut f, p, u32::from(p));
                let e = create_effect(&mut f, holder);
                set_type(&mut f, e, effect_type::FIELD);
                set_code(&mut f, e, code::CANNOT_SPECIAL_SUMMON);
                set_range(&mut f, e, u16::from(location::MZONE));
                set_property(&mut f, e, flag::PLAYER_TARGET, 0);
                set_target_range(&mut f, e, 1, 0);
                set_target_filter(&mut f, e, refuses_player_one);
                register_effect(&mut f, holder, e, false);
            }
            let ask = |f: &mut Field, p: u8| {
                is_player_can_special_summon_monster(
                    f,
                    p,
                    crate::cards::scapegoat::TOKEN_CODES[0],
                    card_type::MONSTER | card_type::NORMAL | card_type::TOKEN,
                    0,
                    0,
                    1,
                    crate::card::race::BEAST,
                    crate::card::attribute::EARTH,
                )
            };
            assert!(ask(&mut f, 0), "player zero's shape is not refused");
            assert!(!ask(&mut f, 1), "player one's is");
        }

        /// **The scratch card is blanked afterwards**, so nothing later
        /// reads a shape that was only ever a question.
        #[test]
        fn the_scratch_card_is_given_back_empty() {
            let mut f = Field::new(8000);
            is_player_can_special_summon_monster(
                &mut f,
                0,
                crate::cards::scapegoat::TOKEN_CODES[0],
                card_type::MONSTER | card_type::NORMAL | card_type::TOKEN,
                0,
                0,
                1,
                crate::card::race::BEAST,
                crate::card::attribute::EARTH,
            );
            let scratch = f.core.temp_card.expect("a scratch card was used");
            assert_eq!(
                f.cards[scratch].data,
                crate::card::CardData::default(),
                "blanked, as the reference blanks it"
            );
        }
    }
}

#[cfg(test)]
mod release_and_option_tests {
    use super::*;
    use crate::board::location;
    use crate::card::{card_type, status, Card, CardData};
    use crate::field::Message;

    fn card_at(f: &mut Field, owner: u8, loc: u8, seq: u32, type_: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 3000 + seq + u32::from(owner) * 50,
                type_,
                level: 4,
                attack: 1000,
                defense: 1000,
                ..Default::default()
            },
            owner,
        );
        c.current.controller = owner;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(owner, id, loc, seq, false);
        f.cards[id].current.position = if loc == location::MZONE {
            crate::position::FACEUP_ATTACK
        } else {
            crate::position::FACEUP
        };
        id
    }

    mod get_release_group {
        use super::*;

        /// **Your own Monster Zone, and nothing of your opponent's.**
        /// Without `use_oppo` an opponent's monster is reachable only
        /// through `EFFECT_EXTRA_RELEASE*`, and a monster with neither is
        /// skipped — which is what stops an ordinary cost tributing the
        /// other side's board.
        #[test]
        fn it_is_your_own_row_unless_the_opponents_is_asked_for() {
            let mut f = Field::new(8000);
            let mine = card_at(&mut f, 0, location::MZONE, 0, card_type::MONSTER);
            let theirs = card_at(&mut f, 1, location::MZONE, 0, card_type::MONSTER);
            assert_eq!(get_release_group(&mut f, 0, false, false, None), vec![mine]);
            let both = get_release_group(&mut f, 0, false, true, None);
            assert!(both.contains(&mine) && both.contains(&theirs), "{both:?}");
        }

        /// The hand joins only when asked for.
        #[test]
        fn the_hand_joins_only_when_asked_for() {
            let mut f = Field::new(8000);
            let field = card_at(&mut f, 0, location::MZONE, 0, card_type::MONSTER);
            let hand = card_at(&mut f, 0, location::HAND, 0, card_type::MONSTER);
            assert_eq!(
                get_release_group(&mut f, 0, false, false, None),
                vec![field]
            );
            let with_hand = get_release_group(&mut f, 0, true, false, None);
            assert!(with_hand.contains(&hand), "{with_hand:?}");
        }

        /// A card under a release prohibition is not in the pool.
        #[test]
        fn a_card_that_cannot_be_released_is_left_out() {
            let mut f = Field::new(8000);
            let m = card_at(&mut f, 0, location::MZONE, 0, card_type::MONSTER);
            assert_eq!(get_release_group(&mut f, 0, false, false, None), vec![m]);
            let mut e = Effect::new(effect_type::SINGLE, code::UNRELEASABLE_NONSUM);
            e.owner = Some(m);
            e.handler = Some(m);
            e.value = 1;
            let id = f.new_effect(e);
            f.cards[m]
                .single_effect
                .insert(code::UNRELEASABLE_NONSUM, id);
            f.cards[m].indexer.insert(id);
            assert!(get_release_group(&mut f, 0, false, false, None).is_empty());
        }

        #[test]
        fn there_is_no_third_player() {
            let mut f = Field::new(8000);
            card_at(&mut f, 0, location::MZONE, 0, card_type::MONSTER);
            assert!(get_release_group(&mut f, 2, false, false, None).is_empty());
        }

        /// Give a card one of the two effects that put an opponent's
        /// monster in the release pool. Nothing in this pool has either —
        /// `ABSENT_FROM_POOL` checks that — so these boards are synthetic
        /// by necessity.
        fn grant(f: &mut Field, c: CardId, code_: u32, count_limit: Option<u8>) -> EffectId {
            let mut e = Effect::new(effect_type::SINGLE, code_);
            e.owner = Some(c);
            e.handler = Some(c);
            e.value = 1;
            if let Some(n) = count_limit {
                e.flag[0] |= flag::COUNT_LIMIT;
                e.count_limit = n;
            }
            let id = f.new_effect(e);
            f.cards[c].single_effect.insert(code_, id);
            f.cards[c].indexer.insert(id);
            id
        }

        /// **The special pools are part of the group the export hands
        /// back** — the reference passes one container three times, so a
        /// card made releasable by `EXTRA_RELEASE` arrives alongside the
        /// ordinary ones.
        #[test]
        fn the_special_pools_are_merged_into_the_group() {
            let mut f = Field::new(8000);
            let mine = card_at(&mut f, 0, location::MZONE, 0, card_type::MONSTER);
            let theirs = card_at(&mut f, 1, location::MZONE, 0, card_type::MONSTER);
            assert_eq!(get_release_group(&mut f, 0, false, false, None), vec![mine]);
            grant(&mut f, theirs, code::EXTRA_RELEASE, None);
            let g = get_release_group(&mut f, 0, false, false, None);
            assert!(g.contains(&theirs), "the extra pool joins the group: {g:?}");

            // And the **one-of** pool too, which is a separate list.
            let mut f = Field::new(8000);
            card_at(&mut f, 0, location::MZONE, 0, card_type::MONSTER);
            let oneof = card_at(&mut f, 1, location::MZONE, 0, card_type::MONSTER);
            grant(&mut f, oneof, code::EXTRA_RELEASE_NONSUM, None);
            let g = get_release_group(&mut f, 0, false, false, None);
            assert!(
                g.contains(&oneof),
                "the one-of pool joins it as well: {g:?}"
            );
        }

        /// **The one-of pool contributes one to the count however many
        /// are in it**, because only one of them may be used.
        #[test]
        fn the_one_of_pool_counts_once() {
            let mut f = Field::new(8000);
            card_at(&mut f, 0, location::MZONE, 0, card_type::MONSTER);
            let a = card_at(&mut f, 1, location::MZONE, 0, card_type::MONSTER);
            let b = card_at(&mut f, 1, location::MZONE, 1, card_type::MONSTER);
            grant(&mut f, a, code::EXTRA_RELEASE_NONSUM, None);
            grant(&mut f, b, code::EXTRA_RELEASE_NONSUM, None);
            let (count, lists) =
                f.get_release_list(0, None, false, false, None, None, reason::COST);
            assert_eq!(lists.extra_one_of.len(), 2, "both are in the pool");
            assert_eq!(count, 2, "one ordinary card plus one for the pool");
        }

        /// A `NONSUM` effect whose count limit is spent does not enable
        /// the release.
        #[test]
        fn a_spent_count_limit_does_not_enable_a_release() {
            let mut f = Field::new(8000);
            let theirs = card_at(&mut f, 1, location::MZONE, 0, card_type::MONSTER);
            grant(&mut f, theirs, code::EXTRA_RELEASE_NONSUM, Some(1));
            let (_, lists) = f.get_release_list(0, None, false, false, None, None, reason::COST);
            assert_eq!(lists.extra_one_of.len(), 1, "the positive sibling");

            let mut f = Field::new(8000);
            let theirs = card_at(&mut f, 1, location::MZONE, 0, card_type::MONSTER);
            grant(&mut f, theirs, code::EXTRA_RELEASE_NONSUM, Some(0));
            let (_, lists) = f.get_release_list(0, None, false, false, None, None, reason::COST);
            assert!(lists.extra_one_of.is_empty(), "spent");
        }

        /// **The exception is honoured**, which only `get_release_list`
        /// itself can be asked about — the export never passes one.
        #[test]
        fn the_named_exception_is_left_out() {
            let mut f = Field::new(8000);
            let a = card_at(&mut f, 0, location::MZONE, 0, card_type::MONSTER);
            let b = card_at(&mut f, 0, location::MZONE, 1, card_type::MONSTER);
            let (_, lists) = f.get_release_list(0, None, false, false, Some(a), None, reason::COST);
            assert_eq!(
                lists.release.iter().copied().collect::<Vec<_>>(),
                vec![b],
                "a was excluded by name"
            );
        }

        /// **The opponent's face-up test depends on there being a
        /// filter.** An unfiltered ask is about count and takes a
        /// face-down monster; a filtered one would have to look at a card
        /// it may not see, and does not.
        #[test]
        fn a_face_down_opponent_monster_is_counted_only_without_a_filter() {
            fn anything(_f: &mut Field, _c: CardId) -> bool {
                true
            }
            let mut f = Field::new(8000);
            let theirs = card_at(&mut f, 1, location::MZONE, 0, card_type::MONSTER);
            f.cards[theirs].current.position = crate::position::FACEDOWN_DEFENSE;
            let (_, lists) = f.get_release_list(0, None, false, true, None, None, reason::COST);
            assert!(lists.release.contains(&theirs), "no filter, so counted");
            let filter: Filter = &anything;
            let (_, lists) =
                f.get_release_list(0, Some(filter), false, true, None, None, reason::COST);
            assert!(
                !lists.release.contains(&theirs),
                "with a filter, a face-down monster is not offered"
            );
        }
    }

    /// **The release records the reason player**, which defaults to
    /// `core.reason_player` and is not always zero.
    #[test]
    fn a_release_is_recorded_against_the_reason_player() {
        let mut f = Field::new(8000);
        let m = card_at(&mut f, 1, location::MZONE, 0, card_type::MONSTER);
        f.core.reason_player = 1;
        release(&mut f, vec![m], reason::COST, None);
        assert_eq!(f.cards[m].reason_player, 1);
        assert_eq!(f.cards[m].reason & reason::COST, reason::COST);
    }

    mod options {
        use super::*;

        fn last_option_question(f: &Field) -> Option<(u8, Vec<u64>)> {
            f.messages.iter().rev().find_map(|m| match m {
                Message::SelectOption { player, options } => Some((*player, options.clone())),
                _ => None,
            })
        }

        #[test]
        fn the_descriptions_reach_the_host() {
            let mut f = Field::new(8000);
            assert!(select_option(&mut f, 1, &[11, 22]));
            f.process();
            assert_eq!(last_option_question(&f), Some((1, vec![11, 22])));
        }

        #[test]
        fn there_is_no_third_player_to_ask() {
            let mut f = Field::new(8000);
            assert!(!select_option(&mut f, 2, &[11]));
        }

        /// **The chosen description is announced.** `HINT_OPSELECTED`
        /// goes to the chooser: the reference's `SelectOption` continuation
        /// writes `HINT_OPSELECTED` directly (`libduel.cpp` 3127), and only
        /// `Duel.Hint` flips a hint to the other player.
        #[test]
        fn the_answer_is_announced_to_the_table() {
            let mut f = Field::new(8000);
            f.core.select_options = vec![11, 22];
            f.core.returns.set(1);
            assert_eq!(selected_option(&mut f, 0, true), 1);
            let announced: Vec<_> = f
                .messages
                .iter()
                .filter_map(|m| match m {
                    Message::Hint {
                        kind,
                        player,
                        value,
                    } if *kind == crate::host_question::hint::OPSELECTED => Some((*player, *value)),
                    _ => None,
                })
                .collect();
            assert_eq!(
                announced,
                vec![(0, 22)],
                "the chooser, the chosen text — written directly, not through Duel.Hint's flip"
            );

            let before = f.messages.len();
            assert_eq!(selected_option(&mut f, 0, false), 1);
            assert_eq!(f.messages.len(), before, "and silent when asked to be");
        }

        /// **`SelectEffect` offers only what is available**, and answers
        /// with the original branch number — so a card reading the answer
        /// does not have to know which branches were dropped.
        #[test]
        fn select_effect_offers_only_what_is_available() {
            let mut f = Field::new(8000);
            let sel = select_effect(&mut f, 0, &[(false, 11), (true, 22)]).expect("a choice");
            assert_eq!(sel, vec![2], "the second branch, numbered two");
            f.process();
            assert_eq!(
                last_option_question(&f),
                Some((0, vec![22])),
                "only the available one is shown"
            );
        }

        #[test]
        fn select_effect_numbers_both_branches_when_both_are_available() {
            let mut f = Field::new(8000);
            let sel = select_effect(&mut f, 0, &[(true, 11), (true, 22)]).expect("a choice");
            assert_eq!(sel, vec![1, 2]);
        }

        /// Nothing available is `nil`, not a question.
        #[test]
        fn select_effect_refuses_when_nothing_is_available() {
            let mut f = Field::new(8000);
            assert!(select_effect(&mut f, 0, &[(false, 11), (false, 22)]).is_none());
            f.process();
            assert!(last_option_question(&f).is_none());
        }
    }

    mod position_and_control {
        use super::*;

        /// **The effect question, not the rule one.** A monster summoned
        /// this turn may have its position changed *by an effect*, and
        /// the rule version refuses one — which is exactly the monster
        /// Enemy Controller is played against.
        #[test]
        fn a_monster_summoned_this_turn_may_still_be_turned_by_an_effect() {
            let mut f = Field::new(8000);
            let m = card_at(&mut f, 0, location::MZONE, 0, card_type::MONSTER);
            f.cards[m].set_status(status::SUMMON_TURN, true);
            assert!(
                !f.is_capable_change_position(m, 0),
                "the rule question refuses it"
            );
            assert!(
                is_can_change_position(&f, m),
                "but an effect may turn it over"
            );
        }

        /// **The four positions are kept apart**: each *current*
        /// position picks its own destination, and a zero leaves that
        /// case alone. `change_position` resolves the choice per card
        /// into `position_param`, so that is where the difference shows.
        #[test]
        fn each_current_position_names_its_own_destination() {
            let up = crate::position::FACEUP_ATTACK;
            let down = crate::position::FACEUP_DEFENSE;
            for (start, want) in [(up, down), (down, up)] {
                let mut f = Field::new(8000);
                let m = card_at(&mut f, 0, location::MZONE, 0, card_type::MONSTER);
                f.cards[m].current.position = start;
                change_position_each(&mut f, vec![m], down, 0, up, 0);
                assert_eq!(
                    f.cards[m].position_param,
                    u32::from(want),
                    "from {start:#x} it should turn to {want:#x}"
                );
            }

            // And a face-down monster is left alone: its slot is zero.
            let mut f = Field::new(8000);
            let m = card_at(&mut f, 0, location::MZONE, 0, card_type::MONSTER);
            f.cards[m].current.position = crate::position::FACEDOWN_DEFENSE;
            change_position_each(&mut f, vec![m], down, 0, up, 0);
            assert_eq!(f.cards[m].position_param, 0, "no destination named");
        }

        #[test]
        fn get_control_masks_the_phase_and_refuses_a_third_player() {
            let mut f = Field::new(8000);
            let m = card_at(&mut f, 1, location::MZONE, 0, card_type::MONSTER);
            assert!(!get_control(&mut f, vec![m], 2, 0x200, 1));
            assert!(get_control(&mut f, vec![m], 0, 0xfc00 | 0x200, 1));
            match &f.core.subunits[0].kind {
                crate::processor::Kind::GetControl {
                    playerid,
                    reset_phase,
                    reset_count,
                    ..
                } => {
                    assert_eq!(*playerid, 0);
                    assert_eq!(*reset_phase, 0x200, "masked to ten bits");
                    assert_eq!(*reset_count, 1);
                }
                other => panic!("expected a control change, got {other:?}"),
            }
        }
    }
}

#[cfg(test)]
mod fusion_and_coin_tests {
    use super::*;
    use crate::board::location;
    use crate::card::{card_type, status, Card, CardData};
    use crate::field::Message;

    fn card_at(f: &mut Field, owner: u8, loc: u8, seq: u32, type_: u32, level: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 1200 + seq + u32::from(owner) * 50,
                type_,
                level,
                ..Default::default()
            },
            owner,
        );
        c.current.controller = owner;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(owner, id, loc, seq, false);
        f.cards[id].current.position = crate::position::FACEUP_ATTACK;
        id
    }

    /// **A Fusion Monster is both bits, on the printed line.** A Fusion
    /// *Spell* is not one, and neither is an ordinary monster.
    #[test]
    fn a_fusion_monster_is_a_monster_with_the_fusion_bit() {
        let mut f = Field::new(8000);
        let fusion = card_at(
            &mut f,
            0,
            location::EXTRA,
            0,
            card_type::MONSTER | card_type::FUSION,
            4,
        );
        let plain = card_at(&mut f, 0, location::MZONE, 0, card_type::MONSTER, 4);
        let spell = card_at(
            &mut f,
            0,
            location::SZONE,
            0,
            card_type::SPELL | card_type::FUSION,
            0,
        );
        assert!(is_fusion_monster(&f, fusion));
        assert!(!is_fusion_monster(&f, plain), "no fusion bit");
        assert!(!is_fusion_monster(&f, spell), "fusion, but not a monster");
    }

    /// **It reads the printed line**, not the effective type: a monster
    /// granted `TYPE_FUSION` by an effect is not one.
    #[test]
    fn it_reads_the_printed_line() {
        let mut f = Field::new(8000);
        let c = card_at(&mut f, 0, location::MZONE, 0, card_type::MONSTER, 4);
        let mut e = Effect::new(effect_type::SINGLE, code::ADD_TYPE);
        e.owner = Some(c);
        e.handler = Some(c);
        e.value = i64::from(card_type::FUSION);
        let id = f.new_effect(e);
        f.cards[c].single_effect.insert(code::ADD_TYPE, id);
        f.cards[c].indexer.insert(id);
        assert!(
            is_type(&mut f, c, card_type::FUSION),
            "the effective type has it"
        );
        assert!(!is_fusion_monster(&f, c), "but the printed line does not");
    }

    /// A coin toss goes to the player it was asked for, and the count is
    /// the count.
    #[test]
    fn a_coin_toss_names_its_player_and_count() {
        let mut f = Field::new(8000);
        assert!(toss_coin(&mut f, 1, 3));
        for _ in 0..16 {
            f.process();
            if f.messages
                .iter()
                .any(|m| matches!(m, Message::TossCoin { .. }))
            {
                break;
            }
        }
        let got = f.messages.iter().find_map(|m| match m {
            Message::TossCoin { player, results } => Some((*player, results.len())),
            _ => None,
        });
        assert_eq!(got, Some((1, 3)));
        assert!(!toss_coin(&mut f, 2, 1), "and there is no third player");
    }

    /// **Heads is `true`.** Counting the tails instead is the mistake
    /// this pins.
    #[test]
    fn heads_are_counted_not_tails() {
        assert_eq!(count_heads(&[true, false, true]), 2);
        assert_eq!(count_heads(&[false, false, false]), 0);
        assert_eq!(count_heads(&[true, true, true]), 3);
    }

    /// **The material list drops duplicates**, as the reference's
    /// `addmat` walk does.
    #[test]
    fn the_material_list_is_deduplicated() {
        let mut f = Field::new(8000);
        let c = card_at(&mut f, 0, location::EXTRA, 0, card_type::MONSTER, 4);
        set_fusion_materials(&mut f, c, &[11, 22, 11, 33]);
        assert_eq!(fusion_materials(&f, c), &[11, 22, 33]);
    }

    mod extra_deck_count {
        use super::*;

        /// The seat count for a card arriving from the Extra Deck, with
        /// and without the card that is about to leave.
        #[test]
        fn the_exclusion_frees_the_seat_it_names() {
            let mut f = Field::new(8000);
            let fusion = card_at(
                &mut f,
                0,
                location::EXTRA,
                0,
                card_type::MONSTER | card_type::FUSION,
                4,
            );
            for i in 0..5 {
                card_at(&mut f, 0, location::MZONE, i, card_type::MONSTER, 4);
            }
            let leaving = f.players[0].mzone[0].expect("a monster");
            assert_eq!(
                get_location_count_from_ex(&mut f, 0, Some(0), Except::None, Some(fusion)),
                0,
                "the row is full"
            );
            assert_eq!(
                get_location_count_from_ex(&mut f, 0, Some(0), Except::Card(leaving), Some(fusion)),
                1,
                "but not once that one is gone"
            );
        }

        #[test]
        fn there_is_no_third_player() {
            let mut f = Field::new(8000);
            assert_eq!(
                get_location_count_from_ex(&mut f, 2, None, Except::None, None),
                0
            );
        }

        /// **The card being summoned is named**, so a forced-zone effect
        /// on it is consulted. Nothing in this pool has one, so the
        /// difference shows only in what is passed through.
        #[test]
        fn the_summoned_card_is_passed_through() {
            let mut f = Field::new(8000);
            let fusion = card_at(
                &mut f,
                0,
                location::EXTRA,
                0,
                card_type::MONSTER | card_type::FUSION,
                4,
            );
            let mut e = Effect::new(effect_type::SINGLE, code::SPSUMMON_CONDITION);
            e.owner = Some(fusion);
            e.handler = Some(fusion);
            let id = f.new_effect(e);
            f.cards[fusion]
                .single_effect
                .insert(code::SPSUMMON_CONDITION, id);
            f.cards[fusion].indexer.insert(id);
            // Both answers are five here; the point of the test is that
            // the call reaches the card at all rather than a null.
            assert_eq!(
                get_location_count_from_ex(&mut f, 0, Some(0), Except::None, Some(fusion)),
                5
            );
        }
    }

    mod chain_data {
        use super::*;

        fn with_chain(chain_id: u16) -> Field {
            let mut f = Field::new(8000);
            let mut ch = crate::chain::Chain::new(0, crate::event::Event::new(0));
            ch.chain_id = chain_id;
            ch.chain_count = 1;
            f.core.current_chain.push(ch);
            f
        }

        /// **Keyed by chain link and effect**, as the library's table is:
        /// two effects on one link do not share, and one effect on two
        /// links does not either.
        #[test]
        fn it_is_keyed_by_both_the_link_and_the_effect() {
            let mut f = with_chain(7);
            set_chain_data(&mut f, 1, 42);
            assert_eq!(get_chain_data(&f, 1), Some(42));
            assert_eq!(get_chain_data(&f, 2), None, "a different effect");

            f.core.current_chain.clear();
            let mut ch = crate::chain::Chain::new(0, crate::event::Event::new(0));
            ch.chain_id = 8;
            ch.chain_count = 1;
            f.core.current_chain.push(ch);
            assert_eq!(get_chain_data(&f, 1), None, "a different link");
        }

        #[test]
        fn with_no_chain_there_is_nowhere_to_put_it() {
            let mut f = Field::new(8000);
            set_chain_data(&mut f, 1, 42);
            assert_eq!(get_chain_data(&f, 1), None);
        }
    }
}

#[cfg(test)]
mod chain_and_battle_tests {
    use super::*;
    use crate::board::{location, position};
    use crate::card::{card_type, status, Card, CardData};
    use crate::chain::Chain;
    use crate::effect::effect_type;
    use crate::event::Event;

    fn card_on_field(f: &mut Field, player: u8, seq: u32, type_: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 9_000 + seq + u32::from(player) * 50,
                type_,
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

    /// One chain link over a fresh effect, and its id.
    fn link(f: &mut Field, holder: CardId, property: u32) -> EffectId {
        let e = create_effect(f, holder);
        set_type(f, e, effect_type::ACTIVATE);
        set_property(f, e, property, 0);
        register_effect(f, holder, e, false);
        let mut ch = Chain::new(e, Event::new(0));
        ch.chain_count = 1;
        ch.chain_id = 31;
        f.core.current_chain.push(ch);
        e
    }

    /// **`IsChainDisablable` is a yes outside a resolution.**
    ///
    /// The effect below may not be negated — `EFFECT_FLAG_CANNOT_DISABLE`
    /// — and the export still answers `true`, because
    /// `core.chain_solving` is false. That is the reference's shape
    /// (`libduel.cpp:3916`) and not a stub: a negator asking on
    /// `EVENT_CHAINING` asks before the chain solves, and `solve_chain`
    /// does the refusing later.
    #[test]
    fn is_chain_disablable_only_consults_the_predicate_while_solving() {
        let mut f = Field::new(8000);
        let holder = card_on_field(&mut f, 0, 0, card_type::SPELL);
        link(&mut f, holder, flag::CANNOT_DISABLE);

        assert!(
            !f.core.chain_solving,
            "the chain is being built, not solved"
        );
        assert!(
            is_chain_disablable(&f, 1),
            "outside a resolution every link answers yes"
        );

        f.core.chain_solving = true;
        assert!(
            !is_chain_disablable(&f, 1),
            "and inside one the real predicate is consulted"
        );
    }

    /// And a link that *can* be negated answers yes either way.
    #[test]
    fn a_disablable_link_answers_yes_in_both_states() {
        let mut f = Field::new(8000);
        let holder = card_on_field(&mut f, 0, 0, card_type::SPELL);
        link(&mut f, holder, 0);
        assert!(is_chain_disablable(&f, 1));
        f.core.chain_solving = true;
        assert!(is_chain_disablable(&f, 1));
    }

    /// **`GetActiveType` hands back the whole word, and the equality a
    /// script writes against it is not the mask `is_active_type` applies.**
    ///
    /// A Quick-Play answers `TYPE_SPELL|TYPE_QUICKPLAY`: the mask says
    /// "yes, a Spell", the equality says "no, not *just* a Spell". Dark
    /// Balter's condition is the equality.
    #[test]
    fn get_active_type_is_the_whole_word_not_a_mask() {
        let mut f = Field::new(8000);
        let plain = card_on_field(&mut f, 0, 0, card_type::SPELL);
        let quick = card_on_field(&mut f, 0, 1, card_type::SPELL | card_type::QUICKPLAY);
        let e1 = link(&mut f, plain, 0);
        let e2 = link(&mut f, quick, 0);

        assert_eq!(get_active_type(&mut f, e1), card_type::SPELL);
        assert_eq!(
            get_active_type(&mut f, e2),
            card_type::SPELL | card_type::QUICKPLAY
        );
        assert!(
            is_active_type(&mut f, e2, card_type::SPELL),
            "the mask says yes"
        );
        assert_ne!(
            get_active_type(&mut f, e2),
            card_type::SPELL,
            "and the equality says no"
        );
    }

    /// **`IsHasType` reads the *declared* type**, which is a different
    /// question from the active one: here the effect is an activation
    /// living on a Spell, and the two answers have no bits in common.
    #[test]
    fn is_has_type_reads_the_declared_effect_type() {
        let mut f = Field::new(8000);
        let holder = card_on_field(&mut f, 0, 0, card_type::SPELL);
        let e = link(&mut f, holder, 0);
        assert!(is_has_type(&f, e, effect_type::ACTIVATE));
        assert!(!is_has_type(&f, e, effect_type::IGNITION));
        assert!(
            !is_has_type(&f, e, effect_type::SINGLE),
            "SetType folds FIELD in for an activation, not SINGLE"
        );
        assert!(
            !is_has_type(&f, usize::MAX, effect_type::ACTIVATE),
            "and no effect is nothing"
        );
    }

    /// **`GetBattleTarget` is symmetric, and answers nothing off the
    /// pair** — it reads the two `core` slots rather than the card.
    #[test]
    fn get_battle_target_reads_both_slots_and_only_those() {
        let mut f = Field::new(8000);
        let attacker = card_on_field(&mut f, 0, 0, card_type::MONSTER);
        let defender = card_on_field(&mut f, 1, 0, card_type::MONSTER);
        let bystander = card_on_field(&mut f, 0, 1, card_type::MONSTER);

        assert_eq!(get_battle_target(&f, attacker), None, "no battle yet");

        f.core.attacker = Some(attacker);
        f.core.attack_target = Some(defender);
        assert_eq!(get_battle_target(&f, attacker), Some(defender));
        assert_eq!(get_battle_target(&f, defender), Some(attacker));
        assert_eq!(
            get_battle_target(&f, bystander),
            None,
            "a card in no battle has no battle target"
        );

        // A direct attack: an attacker with no target answers nothing.
        f.core.attack_target = None;
        assert_eq!(get_battle_target(&f, attacker), None);
    }
}

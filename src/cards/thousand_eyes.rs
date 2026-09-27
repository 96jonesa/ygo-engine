//! Thousand-Eyes Restrict (`63519819`) — `cardscripts/c63519819.lua`.
//!
//! The defining card of the format, and the pool's first monster that
//! **equips another monster to itself**. Five effects on the card, and
//! four more that appear on whatever it takes.
//!
//! ## The equip is a monster, and it keeps its own position
//!
//! `Duel.Equip(tp,tc,c,up)` reads `(player, equip_card, target, faceup)`:
//! the opponent's monster is the equip card and this card is the target.
//! `up` is `false`, so the taken monster keeps whatever position it had —
//! a face-down monster arrives in the Spell & Trap row face-down. That is
//! why `s.atkval` guards on `IsFacedown()`, and why the substitute effect
//! registered on it carries `EFFECT_FLAG_SET_AVAILABLE`: an equip effect
//! on a face-down card is off by default (`effect::is_available`'s equip
//! arm), and this one has to stay on.
//!
//! ## `aux.AddEREquipLimit` and `Card.EquipByEffectAndLimitRegister`
//!
//! Two library helpers (`cards_specific_functions.lua:62`,
//! `utility.lua:1904`) that between them decide *why the taken monster
//! stays equipped*. Three effects and one flag:
//!
//! * `m1`, code `89785779`, on this card: a marker whose value and
//!   operation other "Eyes Restrict" cards call. **No card in this pool
//!   reads it** — `tools/check_constants.py`'s `ABSENT_FROM_POOL` scan
//!   says so — so its two callbacks are registered and never exercised.
//! * `m2`, code `89785779 + EFFECT_EQUIP_LIMIT`, on this card: the
//!   marker the limit below looks for. The ignition effect's label object
//!   points at it.
//! * On the taken monster, an `EFFECT_EQUIP_LIMIT` whose value is
//!   `Card.EquipByEffectLimit`: *the limit's owner is the card I am
//!   attached to, and that card still carries `m2`*. When this card
//!   leaves the field `m2` goes with it, the limit fails, and the taken
//!   monster is destroyed by rule.
//! * And a flag effect `id` on the taken monster, which is how the card
//!   tells "the monster I took" from "a monster equipped to me some
//!   other way": `s.eqcon`, `s.adcon`, `s.atkval` all filter the equip
//!   group by it.
//!
//! ## One condition, two calling conventions
//!
//! `s.eqcon` is the ignition effect's condition, called with the eight
//! event parameters; it is *also* `m1`'s condition, called by
//! `is_available` with the effect alone (the slot Reaper on the Nightmare
//! added). The port writes the question once, read-only, and adapts it
//! to both.
//!
//! ## The substitute reads the reason, not the attacker
//!
//! `s.repval(e,re,r,rp)` is `r&REASON_BATTLE~=0`: when this card would be
//! destroyed by battle, the taken monster is destroyed instead. The value
//! seam hands the reason over in `ctx.args[1]`, the second of the three
//! the destruction sweep pushes.
//!
//! ## `e3` is a clone with `SET_AVAILABLE`, and that flag is about the
//! *target*
//!
//! `EFFECT_CANNOT_CHANGE_POSITION` over both Monster Zones with the
//! handler exempted, exactly as `e2` (`EFFECT_CANNOT_ATTACK`) is — plus
//! `EFFECT_FLAG_SET_AVAILABLE`. On a field effect that flag is read by
//! `is_target`, not `is_available`: it lets the effect reach a
//! **face-down** monster. A set monster cannot flip while this card is
//! out; it can still be told it cannot attack, but a set monster never
//! attacks anyway, so `e2` does not need it.

use crate::board::location;
use crate::card::{card_type, reason};
use crate::effect::{effect_type, flag, flag2, Ctx, Effect, LabelObject, Yield};
use crate::event::{category, code, CardId, EffectId};
use crate::field::{reset, resets, Field};
use crate::host_question::{hint, hintmsg};
use crate::script_api as api;

pub const CODE: u32 = 63519819;

/// `Fusion.AddProcMix(c,true,true,64631466,27125110)`, each material pinned
/// against the script by `tools/check_constants.py`.
const MATERIAL_A: u32 = 64_631_466;
const MATERIAL_B: u32 = 27_125_110;
const MATERIALS: [u32; 2] = [MATERIAL_A, MATERIAL_B];

/// The library's "Eyes Restrict" marker code (`cards_specific_functions.lua:62`,
/// `e1:SetCode(89785779)`) — a bare literal, pinned by `LUA_LITERALS`.
const ER_MARKER: u32 = 89_785_779;

const MZONE: u16 = location::MZONE as u16;
const MZONE32: u32 = location::MZONE as u32;

pub fn initial_effect(f: &mut Field, c: CardId) {
    api::enable_revive_limit(f, c);
    super::proc_fusion::add_proc_mix(f, c, true, true, &MATERIALS);
    // Equip an opponent's monster to this card
    let e1 = api::create_effect(f, c);
    api::set_description(f, e1, api::stringid(CODE, 0));
    api::set_category(f, e1, category::EQUIP);
    api::set_type(f, e1, effect_type::IGNITION);
    api::set_property(f, e1, flag::CARD_TARGET, 0);
    api::set_range(f, e1, MZONE);
    api::set_count_limit(f, e1, 1, 0, 0);
    api::set_condition(f, e1, eqcon);
    api::set_target(f, e1, eqtg);
    api::set_operation(f, e1, eqop);
    api::register_effect(f, c, e1, false);
    add_er_equip_limit(f, c, e1);
    // Nothing else may attack
    let e2 = api::create_effect(f, c);
    api::set_type(f, e2, effect_type::FIELD);
    api::set_range(f, e2, MZONE);
    api::set_code(f, e2, code::CANNOT_ATTACK);
    api::set_target_range(f, e2, MZONE, MZONE);
    api::set_target_filter(f, e2, antarget);
    api::register_effect(f, c, e2, false);
    // `local e3=e2:Clone()` — nor change position, face-down ones included
    let e3 = api::create_effect(f, c);
    api::set_type(f, e3, effect_type::FIELD);
    api::set_range(f, e3, MZONE);
    api::set_code(f, e3, code::CANNOT_CHANGE_POSITION);
    api::set_property(f, e3, flag::SET_AVAILABLE, 0);
    api::set_target_range(f, e3, MZONE, MZONE);
    api::set_target_filter(f, e3, antarget);
    api::register_effect(f, c, e3, false);
    // Its stats are the taken monster's
    let e4 = api::create_effect(f, c);
    api::set_type(f, e4, effect_type::SINGLE);
    api::set_property(f, e4, flag::SINGLE_RANGE, 0);
    api::set_range(f, e4, MZONE);
    api::set_code(f, e4, code::SET_ATTACK);
    api::set_avail_condition(f, e4, adcon);
    api::set_value_fn(f, e4, atkval);
    api::register_effect(f, c, e4, false);
    let e5 = api::create_effect(f, c);
    api::set_type(f, e5, effect_type::SINGLE);
    api::set_property(f, e5, flag::SINGLE_RANGE, 0);
    api::set_range(f, e5, MZONE);
    api::set_code(f, e5, code::SET_DEFENSE);
    api::set_avail_condition(f, e5, adcon);
    api::set_value_fn(f, e5, defval);
    api::register_effect(f, c, e5, false);
}

/// `aux.AddEREquipLimit(c, con, equipval, equipop, linkedeff)` with the
/// three optional trailing arguments (`prop`, `resetflag`, `resetcount`)
/// `nil`, which is the only way this pool calls it.
///
/// Registered in the reference's order: `m1` (the marker other cards
/// call), then `m2` (the one the equip limit looks for), then the
/// ignition effect's label pointed at `m2`.
fn add_er_equip_limit(f: &mut Field, c: CardId, linkedeff: EffectId) {
    let m1 = api::create_effect(f, c);
    api::set_avail_condition(f, m1, eqcon_of);
    api::set_type(f, m1, effect_type::SINGLE);
    api::set_property(f, m1, flag::CANNOT_DISABLE, flag2::MAJESTIC_MUST_COPY);
    api::set_code(f, m1, ER_MARKER);
    api::set_label_object(f, m1, Some(LabelObject::Effect(linkedeff)));
    api::set_value_fn(f, m1, equipval);
    api::set_operation(f, m1, equipop_from_marker);
    api::register_effect(f, c, m1, false);
    let m2 = api::create_effect(f, c);
    api::set_type(f, m2, effect_type::SINGLE);
    // `finalprop&~EFFECT_FLAG_CANNOT_DISABLE` — nothing in the first word.
    api::set_property(f, m2, 0, flag2::MAJESTIC_MUST_COPY);
    api::set_code(f, m2, ER_MARKER + code::EQUIP_LIMIT);
    api::register_effect(f, c, m2, false);
    api::set_label_object(f, linkedeff, Some(LabelObject::Effect(m2)));
}

/// The monsters equipped to `c` that it took with its own effect — the
/// ones carrying the flag `id`. `GetEquipGroup():Filter(s.eqfilter,nil)`.
fn taken(f: &Field, c: CardId) -> Vec<CardId> {
    api::get_equip_group(f, c)
        .iter()
        .copied()
        .filter(|&eq| api::get_flag_effect(f, eq, CODE) != 0)
        .collect()
}

/// `s.eqcon` as a question about the effect alone — nothing it took is
/// still attached.
fn eqcon_of(f: &Field, e: EffectId) -> bool {
    api::get_handler(f, e).is_some_and(|c| taken(f, c).is_empty())
}

/// `s.eqcon` in the ignition effect's eight-parameter convention.
fn eqcon(f: &mut Field, ctx: &Ctx) -> bool {
    eqcon_of(f, ctx.reason_effect)
}

/// `function(ec,_,tp) return ec:IsControler(1-tp) end` — `m1`'s value,
/// asked by another card's script with its own player; nothing in this
/// pool asks.
fn equipval(_e: &Effect, f: &Field, ctx: &Ctx) -> i64 {
    let tp = ctx.player;
    i64::from(
        api::get_handler(f, ctx.reason_effect).is_some_and(|ec| api::is_controler(f, ec, 1 - tp)),
    )
}

/// `function(c,e,tp,tc) equipop(c,e,tp,tc) end` — `m1`'s operation, the
/// same four-argument helper with `tc` arriving as the context's card.
fn equipop_from_marker(f: &mut Field, ctx: &Ctx) -> Yield {
    let Some(tc) = ctx.card else {
        return api::done();
    };
    equipop(f, ctx.reason_effect, ctx.player, tc)
}

/// `s.eqtg`.
fn eqtg(f: &mut Field, ctx: &Ctx, chk: bool, chkc: Option<CardId>) -> Yield {
    let tp = ctx.player;
    if let Some(c) = chkc {
        return api::yes(
            api::is_location(f, c, MZONE)
                && api::is_controler(f, c, 1 - tp)
                && api::is_able_to_change_controler(f, c),
        );
    }
    let filter = |f: &mut Field, c: CardId| api::is_able_to_change_controler(f, c);
    if !chk {
        return api::yes(
            api::get_location_count(f, tp, location::SZONE) > 0
                && api::is_existing_target(f, Some(&filter), tp, 0, MZONE32, 1, api::Except::None),
        );
    }
    api::hint(f, hint::SELECTMSG, tp, hintmsg::EQUIP);
    api::select_target(
        f,
        tp,
        Some(&filter),
        tp,
        0,
        MZONE32,
        1,
        1,
        api::Except::None,
    );
    api::suspend(|f, _ctx| {
        let g = api::selected_targets(f).unwrap_or_default();
        api::set_operation_info(f, 0, category::EQUIP, Some(g), 1, 0, 0);
        api::done()
    })
}

/// `s.equipop(c,e,tp,tc)` — `Card.EquipByEffectAndLimitRegister`, then the
/// substitute.
///
/// The equip suspends and may fail (no seat, a target that has left), and
/// the register helper returns false in that case — nothing is registered
/// on a monster that did not arrive.
fn equipop(f: &mut Field, e: EffectId, tp: u8, tc: CardId) -> Yield {
    let Some(c) = api::get_handler(f, e) else {
        return api::done();
    };
    // `Card.EquipByEffectAndLimitRegister(c,e,tp,tc,id)` — `mustbefaceup`
    // is nil, so `up` is false and the monster keeps its position.
    api::equip(f, tp, tc, c, false, false);
    api::suspend(move |f, _ctx| {
        if api::resumed_value(f) == 0 {
            return api::done();
        }
        // `tc:RegisterFlagEffect(code,RESET_EVENT+RESETS_STANDARD,0,0)`
        api::register_flag_effect(f, tc, CODE, reset::EVENT | resets::STANDARD, 0, 0, 0);
        let te = api::get_label_object_effect(f, e);
        let lim = api::create_effect(f, c);
        api::set_type(f, lim, effect_type::SINGLE);
        api::set_property(f, lim, flag::OWNER_RELATE, 0);
        api::set_code(f, lim, code::EQUIP_LIMIT);
        api::set_reset(f, lim, reset::EVENT | resets::STANDARD, 1);
        api::set_value_fn(f, lim, equip_by_effect_limit);
        api::set_label_object(f, lim, te.map(LabelObject::Effect));
        api::register_effect(f, tc, lim, false);
        // substitute
        let sub = api::create_effect(f, c);
        api::set_type(f, sub, effect_type::EQUIP);
        api::set_code(f, sub, code::DESTROY_SUBSTITUTE);
        api::set_property(f, sub, flag::SET_AVAILABLE | flag::IGNORE_IMMUNE, 0);
        api::set_reset(f, sub, reset::EVENT | resets::STANDARD, 1);
        api::set_value_fn(f, sub, repval);
        api::register_effect(f, tc, sub, false);
        api::done()
    })
}

/// `Card.EquipByEffectLimit(e,c)` (`utility.lua:1896`) — the limit's owner
/// is the card it is attached to, and that card still carries the marker
/// the limit was made against.
fn equip_by_effect_limit(_e: &Effect, f: &Field, ctx: &Ctx) -> i64 {
    let e = ctx.reason_effect;
    let Some(c) = ctx.card else {
        return 0;
    };
    if api::get_owner(f, e) != Some(c) {
        return 0;
    }
    let wanted = api::get_label_object_effect(f, e);
    i64::from(
        api::get_card_effect(f, c, ER_MARKER + code::EQUIP_LIMIT)
            .into_iter()
            .any(|te| Some(te) == wanted),
    )
}

/// `s.eqop`.
fn eqop(f: &mut Field, ctx: &Ctx) -> Yield {
    let e = ctx.reason_effect;
    let tp = ctx.player;
    let Some(tc) = api::get_first_target(f) else {
        return api::done();
    };
    if api::is_relate_to_effect(f, tc, e)
        && api::is_monster(f, tc)
        && api::is_controler(f, tc, 1 - tp)
        && eqcon(f, ctx)
    {
        return equipop(f, e, tp, tc);
    }
    api::done()
}

/// `s.repval(e,re,r,rp)` — `r&REASON_BATTLE~=0`.
fn repval(_e: &Effect, _f: &Field, ctx: &Ctx) -> i64 {
    let r = ctx.args.get(1).copied().unwrap_or(0) as u32;
    i64::from(r & reason::BATTLE != 0)
}

/// `s.antarget(e,c)` — everything but this card.
fn antarget(f: &Field, e: EffectId, target: Option<CardId>, _a: &[i64]) -> bool {
    target.is_some_and(|c| api::get_handler(f, e) != Some(c))
}

/// `s.adcon` — something it took is attached.
fn adcon(f: &Field, e: EffectId) -> bool {
    api::get_handler(f, e).is_some_and(|c| !taken(f, c).is_empty())
}

/// The shared body of `s.atkval` and `s.defval`: the first taken
/// monster's printed stat, or zero if it is face-down, was not printed as
/// a monster, or has a `?` for the stat.
fn taken_stat(f: &Field, e: EffectId, stat: fn(&Field, CardId) -> i32) -> i64 {
    let Some(c) = api::get_handler(f, e) else {
        return 0;
    };
    let Some(&first) = taken(f, c).first() else {
        return 0;
    };
    let v = stat(f, first);
    if api::is_facedown(f, first)
        || api::get_original_type(f, first) & card_type::MONSTER == 0
        || v < 0
    {
        0
    } else {
        i64::from(v)
    }
}

/// `s.atkval(e,c)`.
fn atkval(_e: &Effect, f: &Field, ctx: &Ctx) -> i64 {
    taken_stat(f, ctx.reason_effect, api::get_text_attack)
}

/// `s.defval(e,c)`.
fn defval(_e: &Effect, f: &Field, ctx: &Ctx) -> i64 {
    taken_stat(f, ctx.reason_effect, api::get_text_defense)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::position;
    use crate::card::{attribute, race, status, Card, CardData};
    use crate::chain::Chain;
    use crate::duel::phases;
    use crate::event::Event;
    use crate::field::Message;
    use crate::processor::{Kind, Status};

    /// Thousand-Eyes in `tp`'s Monster Zone, initialised.
    fn board(tp: u8) -> (Field, CardId) {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        f.infos.turn_player = tp;
        f.infos.phase = phases::MAIN1;
        let mut d = Card::with_data(crate::cards::card_data(CODE).expect("printed data"), tp);
        d.current.controller = tp;
        d.set_status(status::EFFECT_ENABLED, true);
        let ter = f.new_card(d);
        f.add_card(tp, ter, location::MZONE, 0, false);
        f.cards[ter].current.position = position::FACEUP_ATTACK;
        f.initialize_card(ter);
        (f, ter)
    }

    fn monster_with(
        f: &mut Field,
        player: u8,
        seq: u32,
        type_: u32,
        atk: i32,
        def: i32,
        faceup: bool,
    ) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 6100 + seq + u32::from(player) * 50,
                type_,
                level: 4,
                attack: atk,
                defense: def,
                ..Default::default()
            },
            player,
        );
        c.current.controller = player;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(player, id, location::MZONE, seq, false);
        f.cards[id].current.position = if faceup {
            position::FACEUP_ATTACK
        } else {
            position::FACEDOWN_DEFENSE
        };
        id
    }

    fn monster(f: &mut Field, player: u8, seq: u32) -> CardId {
        monster_with(
            f,
            player,
            seq,
            card_type::MONSTER | card_type::NORMAL,
            1700,
            1000,
            true,
        )
    }

    fn ignition(f: &Field, ter: CardId) -> EffectId {
        f.cards[ter]
            .field_effect
            .iter()
            .map(|(_, e)| *e)
            .find(|&e| {
                f.effects
                    .get(e)
                    .is_some_and(|x| x.is_type(effect_type::IGNITION))
            })
            .expect("the ignition effect")
    }

    fn single(f: &Field, c: CardId, code_: u32) -> EffectId {
        let got = f.cards[c].single_effect.equal_range(code_);
        assert_eq!(got.len(), 1, "exactly one single effect with code {code_}");
        got[0]
    }

    fn ctx_for<'a>(e: EffectId, tp: u8, ev: &'a Event, card: Option<CardId>) -> Ctx<'a> {
        Ctx {
            reason_effect: e,
            player: tp,
            event: ev,
            card,
            args: &[],
        }
    }

    /// Ask the target's `chk == 0` arm.
    fn can_target(f: &mut Field, ter: CardId, tp: u8) -> bool {
        let e = ignition(f, ter);
        // The target scan asks each card whether it may be *this*
        // effect's target, which needs the reason effect in place.
        f.core.reason_effect = Some(e);
        let ev = Event::new(0);
        let ctx = ctx_for(e, tp, &ev, None);
        eqtg(f, &ctx, false, None).finished().unwrap_or(0) != 0
    }

    /// Ask the target's `chkc` arm about one card.
    fn accepts(f: &mut Field, ter: CardId, tp: u8, c: CardId) -> bool {
        let e = ignition(f, ter);
        f.core.reason_effect = Some(e);
        let ev = Event::new(0);
        let ctx = ctx_for(e, tp, &ev, None);
        eqtg(f, &ctx, false, Some(c)).finished().unwrap_or(0) != 0
    }

    /// Answer the engine's questions until it settles: a target pick by
    /// index, and a seat in the Spell & Trap row for the taken monster.
    fn settle(f: &mut Field, pick: usize) -> Vec<CardId> {
        let mut offered = Vec::new();
        for _ in 0..8192 {
            match f.process() {
                Status::Continue => {
                    if f.core.units.is_empty() && f.core.subunits.is_empty() {
                        return offered;
                    }
                }
                Status::Awaiting => match f.messages.last() {
                    Some(Message::SelectCard { min, cards, .. }) => {
                        offered = cards.clone();
                        let min = usize::from(*min);
                        f.core.return_cards.clear();
                        f.core.returns.set_i32(0, 0);
                        f.core.returns.set_i32(1, min as i32);
                        for i in 0..min {
                            f.core.returns.set_i32(i + 2, (pick + i) as i32);
                        }
                    }
                    Some(Message::SelectPlace { player, flag, .. }) => {
                        let (pl, flag) = (*player, *flag);
                        let seq = (0..5u32)
                            .find(|s| flag & (0x100 << s) == 0)
                            .expect("a free spell seat");
                        f.core.returns.set_i8(0, pl as i8);
                        f.core.returns.set_i8(1, location::SZONE as i8);
                        f.core.returns.set_i8(2, seq as i8);
                    }
                    other => panic!("unexpected question: {other:?}"),
                },
                other => panic!("stopped with {other:?}"),
            }
        }
        panic!("did not settle");
    }

    /// The target step alone: build the link, run `s.eqtg`, answer the
    /// offer with `pick`. Returns what was offered. The link is left in
    /// place so a test can read the announcement off it.
    fn target_step(f: &mut Field, ter: CardId, tp: u8, pick: usize) -> Vec<CardId> {
        let e = ignition(f, ter);
        let mut ch = Chain::new(e, Event::new(0));
        ch.triggering_player = tp;
        ch.chain_count = 1;
        ch.chain_id = 71;
        f.core.current_chain.push(ch);
        f.core.chain_solving = true;
        f.cards[ter].create_chain_relation(e, 71);
        f.core.sub_solving_event.push_back(Event::new(0));
        f.emplace(Kind::ExecuteTarget {
            resume: None,
            effect: e,
            player: tp,
            subject: None,
            args: Vec::new(),
            was_disabled: false,
        });
        settle(f, pick)
    }

    /// The operation step, on a link `target_step` (or a test by hand)
    /// left in place.
    fn operation_step(f: &mut Field, ter: CardId, tp: u8) {
        let e = ignition(f, ter);
        f.core.sub_solving_event.push_back(Event::new(0));
        f.emplace(Kind::ExecuteOperation {
            resume: None,
            effect: e,
            player: tp,
            subject: None,
            args: Vec::new(),
            was_disabled: false,
        });
        settle(f, 0);
        f.core.chain_solving = false;
        f.core.current_chain.clear();
    }

    /// Activate the ignition effect end to end, picking `pick` from the
    /// offer.
    fn take(f: &mut Field, ter: CardId, tp: u8, pick: usize) {
        target_step(f, ter, tp, pick);
        operation_step(f, ter, tp);
    }

    fn drive(f: &mut Field) {
        for _ in 0..256 {
            if f.core.units.is_empty() && f.core.subunits.is_empty() {
                break;
            }
            if f.process() != Status::Continue {
                break;
            }
        }
    }

    mod registration {
        use super::*;

        /// **Five effects on the card and two markers**, wired to each
        /// other through label objects.
        #[test]
        fn the_effects_and_the_marker_pair() {
            let (f, ter) = board(0);
            let e1 = ignition(&f, ter);
            let x = f.effects.get(e1).expect("the ignition");
            assert_eq!(x.category, category::EQUIP);
            assert!(x.is_flag(flag::CARD_TARGET));
            assert_eq!(x.range, MZONE);
            assert_eq!(x.count_limit, 1, "SetCountLimit(1)");
            assert!(x.condition.is_some() && x.target.is_some() && x.operation.is_some());

            let m1 = single(&f, ter, ER_MARKER);
            let m2 = single(&f, ter, ER_MARKER + code::EQUIP_LIMIT);
            let a = f.effects.get(m1).expect("m1");
            assert!(
                a.is_flag(flag::CANNOT_DISABLE),
                "the first marker cannot be disabled"
            );
            assert!(a.is_flag2(flag2::MAJESTIC_MUST_COPY));
            assert!(a.avail_condition.is_some(), "and carries the condition");
            assert!(a.value_fn.is_some() && a.operation.is_some());
            let b = f.effects.get(m2).expect("m2");
            assert!(!b.is_flag(flag::CANNOT_DISABLE), "the second can");
            assert!(b.is_flag2(flag2::MAJESTIC_MUST_COPY));
            assert_eq!(
                api::get_label_object_effect(&f, m1),
                Some(e1),
                "m1 points at the ignition"
            );
            assert_eq!(
                api::get_label_object_effect(&f, e1),
                Some(m2),
                "and the ignition points at m2"
            );

            for wanted in [code::CANNOT_ATTACK, code::CANNOT_CHANGE_POSITION] {
                let got = f.cards[ter].field_effect.equal_range(wanted);
                assert_eq!(got.len(), 1, "{wanted}");
                let y = f.effects.get(got[0]).expect("registered");
                assert!(y.is_type(effect_type::FIELD));
                assert_eq!((y.s_range, y.o_range), (MZONE, MZONE));
                assert!(y.target_filter.is_some(), "filtered");
                assert_eq!(
                    y.is_flag(flag::SET_AVAILABLE),
                    wanted == code::CANNOT_CHANGE_POSITION,
                    "only the position ban reaches face-down monsters"
                );
            }
            for wanted in [code::SET_ATTACK, code::SET_DEFENSE] {
                let z = f.effects.get(single(&f, ter, wanted)).expect("registered");
                assert!(z.is_flag(flag::SINGLE_RANGE));
                assert_eq!(z.range, MZONE);
                assert!(z.avail_condition.is_some(), "conditional");
                assert!(z.value_fn.is_some(), "and a function value");
            }
            assert_eq!(api::fusion_materials(&f, ter), &MATERIALS);
            for wanted in [code::UNSUMMONABLE_CARD, code::REVIVE_LIMIT] {
                assert_eq!(f.cards[ter].single_effect.equal_range(wanted).len(), 1);
            }
        }

        /// **The printed line, transcribed from the oracle's table.**
        #[test]
        fn its_printed_line_matches_the_oracles_table() {
            let d = crate::cards::card_data(CODE).expect("printed data");
            assert!(d.is_type(card_type::FUSION) && d.is_type(card_type::MONSTER));
            assert_eq!((d.level, d.attack, d.defense), (1, 0, 0));
            assert_eq!(d.attribute, attribute::DARK);
            assert_eq!(d.race, race::SPELLCASTER);
        }
    }

    mod the_target {
        use super::*;

        /// The opponent's monster on the field, able to change control.
        #[test]
        fn an_opponents_monster_that_can_change_control() {
            let (mut f, ter) = board(0);
            let theirs = monster(&mut f, 1, 0);
            let mine = monster(&mut f, 0, 1);
            assert!(accepts(&mut f, ter, 0, theirs));
            assert!(!accepts(&mut f, ter, 0, mine), "not my own");
            assert!(can_target(&mut f, ter, 0));
        }

        /// **A monster that cannot change control is not a target**,
        /// which is the one thing `IsAbleToChangeControler` asks.
        #[test]
        fn a_monster_that_cannot_change_control_is_not() {
            let (mut f, ter) = board(0);
            let theirs = monster(&mut f, 1, 0);
            let lock = api::create_effect(&mut f, theirs);
            api::set_type(&mut f, lock, effect_type::SINGLE);
            api::set_code(&mut f, lock, code::CANNOT_CHANGE_CONTROL);
            api::register_effect(&mut f, theirs, lock, false);
            assert!(!accepts(&mut f, ter, 0, theirs));
            assert!(!can_target(&mut f, ter, 0));
        }

        /// **The offer is filtered, not just the check**: a monster that
        /// cannot change control is left out of the choice.
        #[test]
        fn the_offer_omits_a_monster_that_cannot_change_control() {
            let (mut f, ter) = board(0);
            let locked = monster(&mut f, 1, 0);
            let free = monster(&mut f, 1, 1);
            let lock = api::create_effect(&mut f, locked);
            api::set_type(&mut f, lock, effect_type::SINGLE);
            api::set_code(&mut f, lock, code::CANNOT_CHANGE_CONTROL);
            api::register_effect(&mut f, locked, lock, false);
            let offered = target_step(&mut f, ter, 0, 0);
            assert_eq!(offered, vec![free]);
        }

        /// **The announcement names the chosen monster** as an equip, and
        /// the prompt says "equip".
        #[test]
        fn the_announcement_and_the_prompt() {
            let (mut f, ter) = board(0);
            let theirs = monster(&mut f, 1, 0);
            target_step(&mut f, ter, 0, 0);
            let info = f.core.current_chain[0]
                .opinfos
                .get(&category::EQUIP)
                .expect("announced");
            assert_eq!(info.cards.as_deref(), Some(&[theirs][..]));
            assert_eq!(info.count, 1);
            assert!(
                f.messages.iter().any(|m| matches!(
                    m,
                    Message::Hint { kind, value, .. }
                        if *kind == hint::SELECTMSG && *value == hintmsg::EQUIP
                )),
                "HINT_SELECTMSG / HINTMSG_EQUIP"
            );
        }

        /// A monster in the graveyard is not a target, however it is
        /// controlled.
        #[test]
        fn a_monster_off_the_monster_zone_is_not() {
            let (mut f, ter) = board(0);
            let theirs = monster(&mut f, 1, 0);
            f.move_card(1, theirs, location::GRAVE, 0, false);
            assert!(!accepts(&mut f, ter, 0, theirs));
        }

        /// **A seat in the Spell & Trap row is needed** — the taken
        /// monster goes there.
        #[test]
        fn a_full_spell_row_stops_it() {
            let (mut f, ter) = board(0);
            monster(&mut f, 1, 0);
            assert!(can_target(&mut f, ter, 0), "with room");
            for seq in 0..5 {
                let mut c = Card::with_data(
                    CardData {
                        code: 6300 + seq,
                        type_: card_type::SPELL | card_type::CONTINUOUS,
                        ..Default::default()
                    },
                    0,
                );
                c.current.controller = 0;
                let id = f.new_card(c);
                f.add_card(0, id, location::SZONE, seq, false);
                f.cards[id].current.position = position::FACEUP;
            }
            assert!(!can_target(&mut f, ter, 0), "with none");
        }

        /// **With something already taken, it cannot activate.** The
        /// condition counts the equip group by the flag, so an ordinary
        /// equip spell on it does not count.
        #[test]
        fn once_it_holds_a_taken_monster_it_may_not_take_another() {
            let (mut f, ter) = board(0);
            monster(&mut f, 1, 0);
            monster(&mut f, 1, 1);
            let e = ignition(&f, ter);
            let ev = Event::new(0);
            assert!(eqcon(&mut f, &ctx_for(e, 0, &ev, None)));
            take(&mut f, ter, 0, 0);
            assert!(!eqcon(&mut f, &ctx_for(e, 0, &ev, None)), "one is enough");
            assert!(
                !eqcon_of(&f, single(&f, ter, ER_MARKER)),
                "and the marker agrees"
            );
        }

        /// An equip that arrived some other way does not count.
        #[test]
        fn an_ordinary_equip_does_not_count() {
            let (mut f, ter) = board(0);
            let spell = f.new_card(Card::with_data(
                CardData {
                    code: 6400,
                    type_: card_type::SPELL | card_type::EQUIP,
                    ..Default::default()
                },
                0,
            ));
            f.add_card(0, spell, location::SZONE, 0, false);
            f.cards[spell].equiping_target = Some(ter);
            f.cards[ter].equiping_cards.push(spell);
            assert!(
                eqcon_of(&f, single(&f, ter, ER_MARKER)),
                "no flag, no count"
            );
            assert!(
                !adcon(&f, single(&f, ter, code::SET_ATTACK)),
                "and no stats"
            );
        }
    }

    mod taking_a_monster {
        use super::*;

        /// **The monster moves into the Spell & Trap row, equipped to
        /// this card, keeping its position.**
        #[test]
        fn the_monster_is_equipped_and_keeps_its_position() {
            let (mut f, ter) = board(0);
            let theirs = monster(&mut f, 1, 0);
            take(&mut f, ter, 0, 0);
            assert_eq!(f.cards[theirs].current.location, location::SZONE);
            assert_eq!(f.cards[theirs].current.controller, 0, "in my row");
            assert_eq!(f.cards[theirs].equiping_target, Some(ter));
            assert_eq!(f.cards[ter].equiping_cards, vec![theirs]);
            assert!(f.cards[theirs].current.is_faceup(), "face-up stays face-up");
            assert_eq!(
                api::get_flag_effect(&f, theirs, CODE),
                1,
                "flagged as taken"
            );
        }

        /// **A face-down monster arrives face-down** — `up` is false.
        #[test]
        fn a_face_down_monster_arrives_face_down() {
            let (mut f, ter) = board(0);
            let theirs = monster_with(
                &mut f,
                1,
                0,
                card_type::MONSTER | card_type::NORMAL,
                1700,
                1000,
                false,
            );
            take(&mut f, ter, 0, 0);
            assert_eq!(f.cards[theirs].current.location, location::SZONE);
            assert!(!f.cards[theirs].current.is_faceup(), "still face-down");
        }

        /// **The limit and the substitute land on the taken monster**,
        /// owned by this card.
        #[test]
        fn the_limit_and_the_substitute_are_registered_on_it() {
            let (mut f, ter) = board(0);
            let theirs = monster(&mut f, 1, 0);
            take(&mut f, ter, 0, 0);
            let lim = single(&f, theirs, code::EQUIP_LIMIT);
            let l = f.effects.get(lim).expect("the limit");
            assert_eq!(l.owner, Some(ter), "owned by this card");
            assert!(l.is_flag(flag::OWNER_RELATE));
            assert_eq!(l.reset_flag, reset::EVENT | resets::STANDARD);
            assert_eq!(
                api::get_label_object_effect(&f, lim),
                Some(single(&f, ter, ER_MARKER + code::EQUIP_LIMIT)),
                "pointed at m2"
            );
            let flagged = f.cards[theirs]
                .single_effect
                .equal_range(api::flag_code(CODE));
            assert_eq!(flagged.len(), 1);
            assert_eq!(
                f.effects.get(flagged[0]).expect("the flag").reset_flag,
                reset::EVENT | resets::STANDARD,
                "the flag leaves with the card"
            );
            let subs = f.cards[theirs]
                .equip_effect
                .equal_range(code::DESTROY_SUBSTITUTE);
            assert_eq!(subs.len(), 1, "the substitute, as an equip effect");
            let s = f.effects.get(subs[0]).expect("the substitute");
            assert!(s.is_flag(flag::SET_AVAILABLE) && s.is_flag(flag::IGNORE_IMMUNE));
            assert_eq!(s.reset_flag, reset::EVENT | resets::STANDARD);
            assert_eq!(s.owner, Some(ter));
        }

        /// **The limit holds while this card carries `m2`, and for this
        /// card only.**
        #[test]
        fn the_limit_holds_for_this_card_while_it_carries_the_marker() {
            let (mut f, ter) = board(0);
            let theirs = monster(&mut f, 1, 0);
            let other = monster(&mut f, 0, 2);
            take(&mut f, ter, 0, 0);
            assert!(
                f.is_affected_by_effect_against(theirs, code::EQUIP_LIMIT, ter)
                    .is_some(),
                "attached to me: allowed"
            );
            assert!(
                f.is_affected_by_effect_against(theirs, code::EQUIP_LIMIT, other)
                    .is_none(),
                "attached to someone else: not"
            );
            // **Pointed at the wrong marker, the limit fails**: it is not
            // "this card carries *a* marker" but "this card carries the one
            // the limit was made against".
            let lim = single(&f, theirs, code::EQUIP_LIMIT);
            let m1 = single(&f, ter, ER_MARKER);
            api::set_label_object(&mut f, lim, Some(LabelObject::Effect(m1)));
            assert!(
                f.is_affected_by_effect_against(theirs, code::EQUIP_LIMIT, ter)
                    .is_none(),
                "the wrong marker is no marker"
            );
            api::set_label_object(&mut f, lim, None);
            assert!(
                f.is_affected_by_effect_against(theirs, code::EQUIP_LIMIT, ter)
                    .is_none(),
                "and no marker at all is none either"
            );
        }

        /// **The limit asks about its *owner*, not about any marker.** A
        /// second Thousand-Eyes carries a marker of its own; a limit
        /// relabelled to point at that one still fails for it, because the
        /// limit belongs to the first.
        #[test]
        fn the_limit_belongs_to_the_card_that_made_it() {
            let (mut f, ter) = board(0);
            let mut d = Card::with_data(crate::cards::card_data(CODE).expect("printed data"), 0);
            d.current.controller = 0;
            d.set_status(status::EFFECT_ENABLED, true);
            let ter2 = f.new_card(d);
            f.add_card(0, ter2, location::MZONE, 3, false);
            f.cards[ter2].current.position = position::FACEUP_ATTACK;
            f.initialize_card(ter2);
            let theirs = monster(&mut f, 1, 0);
            take(&mut f, ter, 0, 0);
            let lim = single(&f, theirs, code::EQUIP_LIMIT);
            let m2_of_ter2 = single(&f, ter2, ER_MARKER + code::EQUIP_LIMIT);
            api::set_label_object(&mut f, lim, Some(LabelObject::Effect(m2_of_ter2)));
            assert!(
                f.is_affected_by_effect_against(theirs, code::EQUIP_LIMIT, ter2)
                    .is_none(),
                "the marker matches but the owner does not"
            );
        }

        /// **Negate Thousand-Eyes and the taken monster comes off**: a
        /// disabled card's marker is not *in force*, and `GetCardEffect`
        /// reads only what is.
        #[test]
        fn a_disabled_thousand_eyes_loses_its_grip() {
            let (mut f, ter) = board(0);
            let theirs = monster(&mut f, 1, 0);
            take(&mut f, ter, 0, 0);
            assert!(f
                .is_affected_by_effect_against(theirs, code::EQUIP_LIMIT, ter)
                .is_some());
            f.cards[ter].set_status(status::DISABLED, true);
            assert!(
                api::get_card_effect(&f, ter, ER_MARKER + code::EQUIP_LIMIT).is_empty(),
                "the marker is not in force"
            );
            assert!(f
                .is_affected_by_effect_against(theirs, code::EQUIP_LIMIT, ter)
                .is_none());
        }

        /// The value functions themselves, so a `?` stat is seen as the
        /// zero the script returns and not as the `-2` a stat reader might
        /// clamp anyway.
        #[test]
        fn the_stat_values_return_zero_for_a_question_mark() {
            let (mut f, ter) = board(0);
            monster_with(
                &mut f,
                1,
                0,
                card_type::MONSTER | card_type::EFFECT,
                -2,
                -2,
                true,
            );
            take(&mut f, ter, 0, 0);
            let e4 = single(&f, ter, code::SET_ATTACK);
            let e5 = single(&f, ter, code::SET_DEFENSE);
            let ev = Event::new(0);
            let x4 = f.effects.get(e4).unwrap();
            let x5 = f.effects.get(e5).unwrap();
            assert_eq!(atkval(x4, &f, &ctx_for(e4, 0, &ev, Some(ter))), 0);
            assert_eq!(defval(x5, &f, &ctx_for(e5, 0, &ev, Some(ter))), 0);
        }

        /// **The stats become the taken monster's printed ones.**
        #[test]
        fn its_stats_become_the_taken_monsters() {
            let (mut f, ter) = board(0);
            let theirs = monster_with(
                &mut f,
                1,
                0,
                card_type::MONSTER | card_type::EFFECT,
                1900,
                700,
                true,
            );
            assert_eq!(
                (api::get_attack(&mut f, ter), api::get_defense(&mut f, ter)),
                (0, 0)
            );
            take(&mut f, ter, 0, 0);
            assert_eq!(
                (api::get_attack(&mut f, ter), api::get_defense(&mut f, ter)),
                (1900, 700)
            );
            let _ = theirs;
        }

        /// **A face-down taken monster lends nothing.**
        #[test]
        fn a_face_down_taken_monster_lends_zero() {
            let (mut f, ter) = board(0);
            monster_with(
                &mut f,
                1,
                0,
                card_type::MONSTER | card_type::NORMAL,
                1900,
                700,
                false,
            );
            take(&mut f, ter, 0, 0);
            assert_eq!(
                (api::get_attack(&mut f, ter), api::get_defense(&mut f, ter)),
                (0, 0)
            );
        }

        /// A `?` stat (printed as `-2`) lends zero, not `-2`, and a card
        /// not printed as a monster lends zero whatever it says.
        #[test]
        fn a_question_mark_and_a_non_monster_lend_zero() {
            let (mut f, ter) = board(0);
            let theirs = monster_with(
                &mut f,
                1,
                0,
                card_type::MONSTER | card_type::EFFECT,
                -2,
                500,
                true,
            );
            take(&mut f, ter, 0, 0);
            assert_eq!(api::get_attack(&mut f, ter), 0, "? attack");
            assert_eq!(api::get_defense(&mut f, ter), 500, "but a real defence");
            // Rewrite the printed line under it: a Trap that is somehow
            // a monster on the field lends nothing.
            f.cards[theirs].data.type_ = card_type::TRAP;
            f.cards[theirs].data.attack = 1000;
            assert_eq!(api::get_attack(&mut f, ter), 0);
            assert_eq!(api::get_defense(&mut f, ter), 0);
        }

        /// The first taken monster is the one read, and a second flag
        /// on some later equip does not shadow it.
        #[test]
        fn the_first_taken_monster_is_the_one_read() {
            let (mut f, ter) = board(0);
            monster_with(
                &mut f,
                1,
                0,
                card_type::MONSTER | card_type::NORMAL,
                1200,
                300,
                true,
            );
            take(&mut f, ter, 0, 0);
            // A second, flagged by hand, pushed behind the first.
            let second = monster_with(
                &mut f,
                1,
                1,
                card_type::MONSTER | card_type::NORMAL,
                2500,
                2100,
                true,
            );
            f.move_card(0, second, location::SZONE, 1, false);
            f.cards[second].current.position = position::FACEUP;
            f.cards[second].equiping_target = Some(ter);
            f.cards[ter].equiping_cards.push(second);
            api::register_flag_effect(
                &mut f,
                second,
                CODE,
                reset::EVENT | resets::STANDARD,
                0,
                0,
                0,
            );
            assert_eq!(api::get_attack(&mut f, ter), 1200);
        }

        /// **A failed equip registers nothing.** The Spell row is filled
        /// between the target and the resolution.
        #[test]
        fn a_failed_equip_registers_nothing() {
            let (mut f, ter) = board(0);
            let theirs = monster(&mut f, 1, 0);
            let e = ignition(&f, ter);
            let mut ch = Chain::new(e, Event::new(0));
            ch.triggering_player = 0;
            ch.chain_count = 1;
            ch.chain_id = 71;
            ch.target_cards = vec![theirs];
            f.core.current_chain.push(ch);
            f.core.chain_solving = true;
            f.cards[ter].create_chain_relation(e, 71);
            f.cards[theirs].create_chain_relation(e, 71);
            for seq in 0..5 {
                let mut c = Card::with_data(
                    CardData {
                        code: 6300 + seq,
                        type_: card_type::SPELL | card_type::CONTINUOUS,
                        ..Default::default()
                    },
                    0,
                );
                c.current.controller = 0;
                let id = f.new_card(c);
                f.add_card(0, id, location::SZONE, seq, false);
                f.cards[id].current.position = position::FACEUP;
            }
            operation_step(&mut f, ter, 0);
            assert_ne!(f.cards[theirs].current.location, location::SZONE);
            assert_eq!(api::get_flag_effect(&f, theirs, CODE), 0, "not flagged");
            assert!(f.cards[theirs]
                .single_effect
                .equal_range(code::EQUIP_LIMIT)
                .is_empty());
            assert!(f.cards[theirs]
                .equip_effect
                .equal_range(code::DESTROY_SUBSTITUTE)
                .is_empty());
        }

        /// **The operation re-checks all four things**: relation, being a
        /// monster, being the opponent's, and the condition.
        #[test]
        fn the_operation_re_checks_before_taking() {
            type Spoil = fn(&mut Field, CardId, CardId);
            let cases: [(&str, Spoil); 4] = [
                ("unrelated", |f, _ter, tc| f.cards[tc].clear_relate_effect()),
                ("not a monster", |f, _ter, tc| {
                    f.cards[tc].data.type_ = card_type::SPELL
                }),
                ("mine now", |f, _ter, tc| f.cards[tc].current.controller = 0),
                ("already holding one", |f, ter, _tc| {
                    api::register_flag_effect(f, ter, CODE, 0, 0, 0, 0);
                    let dummy = f.new_card(Card::with_data(
                        CardData {
                            code: 6500,
                            type_: card_type::MONSTER,
                            ..Default::default()
                        },
                        1,
                    ));
                    f.add_card(0, dummy, location::SZONE, 4, false);
                    f.cards[dummy].equiping_target = Some(ter);
                    f.cards[ter].equiping_cards.push(dummy);
                    api::register_flag_effect(f, dummy, CODE, 0, 0, 0, 0);
                }),
            ];
            // The unspoiled sibling first: the same driver does take.
            {
                let (mut f, ter) = board(0);
                let theirs = monster(&mut f, 1, 0);
                let e = ignition(&f, ter);
                let mut ch = Chain::new(e, Event::new(0));
                ch.triggering_player = 0;
                ch.chain_count = 1;
                ch.chain_id = 71;
                ch.target_cards = vec![theirs];
                f.core.current_chain.push(ch);
                f.core.chain_solving = true;
                f.cards[ter].create_chain_relation(e, 71);
                f.cards[theirs].create_chain_relation(e, 71);
                operation_step(&mut f, ter, 0);
                assert_eq!(f.cards[theirs].equiping_target, Some(ter), "unspoiled");
            }
            for (why, spoil) in cases {
                let (mut f, ter) = board(0);
                let theirs = monster(&mut f, 1, 0);
                let e = ignition(&f, ter);
                let mut ch = Chain::new(e, Event::new(0));
                ch.triggering_player = 0;
                ch.chain_count = 1;
                ch.chain_id = 71;
                ch.target_cards = vec![theirs];
                f.core.current_chain.push(ch);
                f.core.chain_solving = true;
                f.cards[ter].create_chain_relation(e, 71);
                f.cards[theirs].create_chain_relation(e, 71);
                spoil(&mut f, ter, theirs);
                // `settle`, not `drive`: a take asks for a seat, and a
                // driver that cannot answer it leaves every case looking
                // refused.
                operation_step(&mut f, ter, 0);
                assert_ne!(f.cards[theirs].equiping_target, Some(ter), "{why}");
            }
        }
    }

    mod the_substitute {
        use super::*;

        fn destroy(f: &mut Field, c: CardId, why: u32) {
            api::destroy(f, vec![c], why);
            drive(f);
        }

        /// **Destroyed by battle, the taken monster dies instead.**
        #[test]
        fn battle_destruction_falls_on_the_taken_monster() {
            let (mut f, ter) = board(0);
            let theirs = monster(&mut f, 1, 0);
            take(&mut f, ter, 0, 0);
            destroy(&mut f, ter, reason::BATTLE);
            assert_eq!(
                f.cards[ter].current.location,
                location::MZONE,
                "this card stays"
            );
            assert_eq!(
                f.cards[theirs].current.location,
                location::GRAVE,
                "the taken one goes"
            );
        }

        /// **Destroyed by an effect, this card dies and the substitute
        /// does not answer** — `r&REASON_BATTLE~=0`.
        #[test]
        fn effect_destruction_is_not_substituted() {
            let (mut f, ter) = board(0);
            monster(&mut f, 1, 0);
            take(&mut f, ter, 0, 0);
            destroy(&mut f, ter, reason::EFFECT);
            assert_eq!(f.cards[ter].current.location, location::GRAVE);
        }

        /// And a face-down taken monster still substitutes —
        /// `SET_AVAILABLE` on an equip effect.
        #[test]
        fn a_face_down_taken_monster_still_substitutes() {
            let (mut f, ter) = board(0);
            let theirs = monster_with(
                &mut f,
                1,
                0,
                card_type::MONSTER | card_type::NORMAL,
                1700,
                1000,
                false,
            );
            take(&mut f, ter, 0, 0);
            assert!(!f.cards[theirs].current.is_faceup());
            destroy(&mut f, ter, reason::BATTLE);
            assert_eq!(f.cards[ter].current.location, location::MZONE);
            assert_eq!(f.cards[theirs].current.location, location::GRAVE);
        }

        /// The value reads the second pushed argument — the reason — and
        /// nothing else.
        #[test]
        fn the_value_reads_the_reason_argument() {
            let (f, ter) = board(0);
            let e = single(&f, ter, ER_MARKER);
            let ev = Event::new(0);
            let x = f.effects.get(e).unwrap();
            let battle = [0, i64::from(reason::BATTLE | reason::DESTROY), 1];
            let effect = [0, i64::from(reason::EFFECT | reason::DESTROY), 1];
            let ctx = Ctx {
                reason_effect: e,
                player: 0,
                event: &ev,
                card: Some(ter),
                args: &battle,
            };
            assert_eq!(repval(x, &f, &ctx), 1);
            let ctx = Ctx {
                reason_effect: e,
                player: 0,
                event: &ev,
                card: Some(ter),
                args: &effect,
            };
            assert_eq!(repval(x, &f, &ctx), 0);
            let ctx = Ctx {
                reason_effect: e,
                player: 0,
                event: &ev,
                card: Some(ter),
                args: &[],
            };
            assert_eq!(repval(x, &f, &ctx), 0, "no arguments, no battle");
        }
    }

    mod the_two_auras {
        use super::*;

        /// **Everything else may neither attack nor change position; this
        /// card may do both.**
        #[test]
        fn every_other_monster_is_frozen_and_this_one_is_not() {
            let (mut f, ter) = board(0);
            let mine = monster(&mut f, 0, 1);
            let theirs = monster(&mut f, 1, 0);
            for c in [mine, theirs] {
                assert!(f.is_affected_by_effect(c, code::CANNOT_ATTACK).is_some());
                assert!(f
                    .is_affected_by_effect(c, code::CANNOT_CHANGE_POSITION)
                    .is_some());
            }
            assert!(f.is_affected_by_effect(ter, code::CANNOT_ATTACK).is_none());
            assert!(f
                .is_affected_by_effect(ter, code::CANNOT_CHANGE_POSITION)
                .is_none());
        }

        /// **A face-down monster is held in position but not told it
        /// cannot attack** — `SET_AVAILABLE` on `e3` alone.
        #[test]
        fn a_face_down_monster_is_reached_by_the_position_ban_only() {
            let (mut f, ter) = board(0);
            let _ = ter;
            let set = monster_with(
                &mut f,
                1,
                0,
                card_type::MONSTER | card_type::NORMAL,
                1700,
                1000,
                false,
            );
            assert!(f
                .is_affected_by_effect(set, code::CANNOT_CHANGE_POSITION)
                .is_some());
            assert!(f.is_affected_by_effect(set, code::CANNOT_ATTACK).is_none());
        }

        /// The filter itself: everything but the handler.
        #[test]
        fn the_filter_exempts_the_handler_alone() {
            let (mut f, ter) = board(0);
            let other = monster(&mut f, 1, 0);
            let e = f.cards[ter].field_effect.equal_range(code::CANNOT_ATTACK)[0];
            assert!(antarget(&f, e, Some(other), &[]));
            assert!(!antarget(&f, e, Some(ter), &[]));
            assert!(!antarget(&f, e, None, &[]));
        }
    }

    mod the_exports {
        use super::*;

        /// `GetCardEffect` answers by code, and `GetTextDefense` reads
        /// zero for a card with no level.
        #[test]
        fn get_card_effect_is_by_code_and_text_defense_honours_no_level() {
            let (mut f, ter) = board(0);
            let m1 = single(&f, ter, ER_MARKER);
            let m2 = single(&f, ter, ER_MARKER + code::EQUIP_LIMIT);
            assert_eq!(api::get_card_effect(&f, ter, ER_MARKER), vec![m1]);
            assert_eq!(
                api::get_card_effect(&f, ter, ER_MARKER + code::EQUIP_LIMIT),
                vec![m2]
            );
            let all = api::get_card_effect(&f, ter, 0);
            assert!(
                all.contains(&m1) && all.contains(&m2),
                "code 0 is every code"
            );

            let theirs = monster_with(
                &mut f,
                1,
                0,
                card_type::MONSTER | card_type::NORMAL,
                1700,
                1000,
                true,
            );
            assert_eq!(api::get_text_defense(&f, theirs), 1000);
            f.cards[theirs].set_status(status::NO_LEVEL, true);
            assert_eq!(api::get_text_defense(&f, theirs), 0);
        }
    }

    mod the_marker_callbacks {
        use super::*;

        /// `m1`'s value: "is my handler the asking player's opponent" —
        /// what another Eyes Restrict card would ask. Nothing in this pool
        /// does, so this is the only place it is exercised.
        #[test]
        fn the_markers_value_asks_about_the_asking_player() {
            let (f, ter) = board(0);
            let m1 = single(&f, ter, ER_MARKER);
            let x = f.effects.get(m1).unwrap();
            let ev = Event::new(0);
            assert_eq!(
                equipval(x, &f, &ctx_for(m1, 1, &ev, None)),
                1,
                "asked by the opponent"
            );
            assert_eq!(
                equipval(x, &f, &ctx_for(m1, 0, &ev, None)),
                0,
                "asked by its controller"
            );
        }

        /// `m1`'s operation is the same helper, with the monster arriving
        /// as the context's card.
        #[test]
        fn the_markers_operation_takes_the_context_card() {
            let (mut f, ter) = board(0);
            let theirs = monster(&mut f, 1, 0);
            let m1 = single(&f, ter, ER_MARKER);
            let ev = Event::new(0);
            f.core.reason_effect = Some(m1);
            equipop_from_marker(&mut f, &ctx_for(m1, 0, &ev, Some(theirs)));
            settle(&mut f, 0);
            assert_eq!(f.cards[theirs].equiping_target, Some(ter));
            let ev2 = Event::new(0);
            assert!(
                matches!(
                    equipop_from_marker(&mut f, &ctx_for(m1, 0, &ev2, None)),
                    Yield::Done(_)
                ),
                "no card, nothing to do"
            );
        }
    }
}

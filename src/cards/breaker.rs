//! Breaker the Magical Warrior — `c71413901.lua`.
//!
//! The thirty-fourth card, and the pool's first to use **counters**. Three
//! effects and two declarations, and the declarations are the interesting
//! part: a card cannot hold a counter it has not been given permission to
//! hold.
//!
//! ## Permission and limit are effects, not fields
//!
//! ```lua
//! c:EnableCounterPermit(COUNTER_SPELL)
//! c:SetCounterLimit(COUNTER_SPELL,1)
//! ```
//!
//! Both register single effects whose **code is a base plus the counter
//! type**: `EFFECT_COUNTER_PERMIT | 0x1` and `EFFECT_COUNTER_LIMIT | 0x1`.
//! The permit's *value* is the range it applies in, defaulted by printed
//! type — Monster Zone for a monster. The limit's value is the number.
//!
//! A counter placed under a permit goes in the card's **temporary** half
//! and is lost when the card is disabled, which is the whole reason the
//! permission exists as an effect rather than a flag.
//!
//! ## One counter, and only on a Normal Summon
//!
//! `EVENT_SUMMON_SUCCESS`, not `EVENT_SPSUMMON_SUCCESS` or a flip — a
//! Breaker that arrives any other way arrives without its counter, and is
//! then a 1600-attack monster with nothing to spend.
//!
//! The trigger is **forced** (`EFFECT_TYPE_TRIGGER_F`) and its target
//! announces the counter with the counter *type* in the parameter slot,
//! where most announcements put a count.
//!
//! ## The attack bonus is per counter, and reads the card it is asked about
//!
//! ```lua
//! e2:SetValue(function(e,c) return c:GetCounter(COUNTER_SPELL)*300 end)
//! ```
//!
//! `EFFECT_FLAG_SINGLE_RANGE` and a Monster Zone range, so it applies only
//! while Breaker is on the field — and it is recomputed every time it is
//! asked, so spending the counter takes the 300 back in the same moment.
//!
//! ## The cost is the counter
//!
//! `Cost.RemoveCounterFromSelf(COUNTER_SPELL,1)` (`utility.lua:1485`) is
//! the library's two-in-one: asked with `chk == 0` it answers
//! `IsCanRemoveCounter`, and asked to pay it calls `RemoveCounter`. Note
//! that "can remove" is not "has one" — a replacement effect makes the
//! removal payable however few counters there are.
//!
//! Every call here is to `script_api`, by the rule in `cards/mod.rs`.

use crate::board::location;
use crate::card::reason;
use crate::counters::counter_type;
use crate::effect::{effect_type, flag, Ctx, Effect, Yield};
use crate::event::{category, code, CardId};
use crate::field::Field;
use crate::host_question::{hint, hintmsg};
use crate::script_api as api;

pub const CODE: u32 = 71_413_901;

/// The attack each counter is worth.
const PER_COUNTER: i64 = 300;

/// `s.counter_place_list = {COUNTER_SPELL}` — the declaration that this
/// card *places* spell counters.
///
/// Read only by `Card.ListsCounter` / `Card.PlacesCounter`
/// (`utility.lua:2381`), which nothing in this pool calls — the only
/// references to them are deprecated shims. Recorded here rather than
/// dropped, and pinned by a test, so that a later card which does read it
/// finds it already stated.
pub const COUNTER_PLACE_LIST: [u16; 1] = [counter_type::SPELL];

pub fn initial_effect(f: &mut Field, c: CardId) {
    api::enable_counter_permit(f, c, counter_type::SPELL, None);
    api::set_counter_limit(f, c, counter_type::SPELL, 1);
    // Place 1 Spell Counter on it (max. 1)
    let e1 = api::create_effect(f, c);
    api::set_description(f, e1, api::stringid(CODE, 0));
    api::set_category(f, e1, category::COUNTER);
    api::set_type(f, e1, effect_type::SINGLE | effect_type::TRIGGER_F);
    api::set_code(f, e1, code::SUMMON_SUCCESS);
    api::set_target(f, e1, countertg);
    api::set_operation(f, e1, counterop);
    api::register_effect(f, c, e1, false);
    // Gains 300 ATK for each Spell Counter on it
    let e2 = api::create_effect(f, c);
    api::set_type(f, e2, effect_type::SINGLE);
    api::set_property(f, e2, flag::SINGLE_RANGE, 0);
    api::set_code(f, e2, code::UPDATE_ATTACK);
    api::set_range(f, e2, u16::from(location::MZONE));
    api::set_value_fn(f, e2, attack_bonus);
    api::register_effect(f, c, e2, false);
    // Destroy 1 Spell/Trap on the field
    let e3 = api::create_effect(f, c);
    api::set_description(f, e3, api::stringid(CODE, 1));
    api::set_category(f, e3, category::DESTROY);
    api::set_type(f, e3, effect_type::IGNITION);
    api::set_property(f, e3, flag::CARD_TARGET, 0);
    api::set_range(f, e3, u16::from(location::MZONE));
    api::set_cost(f, e3, cost);
    api::set_target(f, e3, destg);
    api::set_operation(f, e3, desop);
    api::register_effect(f, c, e3, false);
}

const ONFIELD: u32 = location::ONFIELD as u32;

/// `e1`'s target — announce the counter. The parameter slot carries the
/// counter **type**, where most categories put a count.
fn countertg(f: &mut Field, ctx: &Ctx, chk: bool, _chkc: Option<CardId>) -> Yield {
    if !chk {
        return api::yes(true);
    }
    let handler = api::get_handler(f, ctx.reason_effect);
    api::set_operation_info(
        f,
        0,
        category::COUNTER,
        handler.map(|c| vec![c]),
        1,
        ctx.player,
        i32::from(counter_type::SPELL),
    );
    api::yes(true)
}

fn counterop(f: &mut Field, ctx: &Ctx) -> Yield {
    let e = ctx.reason_effect;
    let Some(c) = api::get_handler(f, e) else {
        return api::done();
    };
    if api::is_relate_to_effect(f, c, e) {
        api::add_counter(f, c, counter_type::SPELL, 1, false);
    }
    api::done()
}

/// `e2`'s value — 300 for each spell counter on the card being asked
/// about.
fn attack_bonus(_e: &Effect, f: &Field, ctx: &Ctx) -> i64 {
    let Some(c) = ctx.card else {
        return 0;
    };
    i64::from(api::get_counter(f, c, counter_type::SPELL)) * PER_COUNTER
}

/// `Cost.RemoveCounterFromSelf(COUNTER_SPELL, 1)` — the library's
/// helper, spelled out.
fn cost(f: &mut Field, ctx: &Ctx, chk: bool) -> bool {
    let tp = ctx.player;
    let Some(c) = api::get_handler(f, ctx.reason_effect) else {
        return false;
    };
    if !chk {
        return api::is_can_remove_counter(f, c, tp, counter_type::SPELL, 1, reason::COST);
    }
    api::remove_counter(f, c, tp, counter_type::SPELL, 1, reason::COST);
    true
}

fn destg(f: &mut Field, ctx: &Ctx, chk: bool, chkc: Option<CardId>) -> Yield {
    let tp = ctx.player;
    if let Some(c) = chkc {
        return api::yes(api::is_on_field(f, c) && api::is_spell_trap(f, c));
    }
    if !chk {
        // **Both** rows: the scan is two-sided, so it can break its own
        // controller's cards as readily as the opponent's.
        return api::yes(api::is_existing_target(
            f,
            Some(&|f: &mut Field, c: CardId| api::is_spell_trap(f, c)),
            tp,
            ONFIELD,
            ONFIELD,
            1,
            api::Except::None,
        ));
    }
    api::hint(f, hint::SELECTMSG, tp, hintmsg::DESTROY);
    api::select_target(
        f,
        tp,
        Some(&|f: &mut Field, c: CardId| api::is_spell_trap(f, c)),
        tp,
        ONFIELD,
        ONFIELD,
        1,
        1,
        api::Except::None,
    );
    api::suspend(move |f, _ctx| {
        let Some(g) = api::selected_targets(f) else {
            return api::done();
        };
        api::set_operation_info(f, 0, category::DESTROY, Some(g), 1, tp, 0);
        api::done()
    })
}

fn desop(f: &mut Field, ctx: &Ctx) -> Yield {
    let e = ctx.reason_effect;
    let Some(tc) = api::get_first_target(f) else {
        return api::done();
    };
    if api::is_relate_to_effect(f, tc, e) {
        api::destroy(f, vec![tc], reason::EFFECT);
    }
    api::done()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::position;
    use crate::card::{card_type, status, Card, CardData};
    use crate::chain::Chain;
    use crate::counters::counter;
    use crate::event::{EffectId, Event};
    use crate::field::Message;
    use crate::processor::{Kind, Status};

    fn spell_trap(f: &mut Field, player: u8, code_: u32, seq: u32, faceup: bool) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: code_,
                type_: card_type::TRAP,
                ..Default::default()
            },
            player,
        );
        c.current.controller = player;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(player, id, location::SZONE, seq, false);
        f.cards[id].current.position = if faceup {
            position::FACEUP
        } else {
            position::FACEDOWN
        };
        id
    }

    fn vanilla(f: &mut Field, player: u8, code_: u32, seq: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: code_,
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

    /// `tp`'s Main Phase 1 with Breaker face-up in its first Monster Zone.
    fn field_as(tp: u8) -> (Field, CardId) {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        f.infos.turn_player = tp;
        f.infos.phase = crate::duel::phases::MAIN1;
        let mut d = Card::with_data(crate::cards::card_data(CODE).expect("printed data"), tp);
        d.current.controller = tp;
        d.set_status(status::EFFECT_ENABLED, true);
        let br = f.new_card(d);
        f.add_card(tp, br, location::MZONE, 0, false);
        f.cards[br].current.position = position::FACEUP_ATTACK;
        f.initialize_card(br);
        (f, br)
    }

    fn field() -> (Field, CardId) {
        field_as(0)
    }

    fn trigger_effect(f: &Field, br: CardId) -> EffectId {
        f.cards[br].single_effect.equal_range(code::SUMMON_SUCCESS)[0]
    }
    fn bonus_effect(f: &Field, br: CardId) -> EffectId {
        f.cards[br].single_effect.equal_range(code::UPDATE_ATTACK)[0]
    }
    fn ignition_effect(f: &Field, br: CardId) -> EffectId {
        f.cards[br].field_effect.equal_range(0)[0]
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

    /// **Three effects and two declarations.** The declarations are what
    /// let the card hold a counter at all, and both are effects whose
    /// code carries the counter type.
    #[test]
    fn the_script_declares_a_permit_and_a_limit_and_registers_three_effects() {
        let (f, br) = field();
        let permit = f.cards[br]
            .single_effect
            .equal_range(code::COUNTER_PERMIT | u32::from(counter_type::SPELL));
        assert_eq!(permit.len(), 1, "permitted to hold spell counters");
        assert_eq!(
            f.effects.get(permit[0]).unwrap().value,
            i64::from(location::MZONE),
            "in the Monster Zone, defaulted from its printed type"
        );

        let limit = f.cards[br]
            .single_effect
            .equal_range(code::COUNTER_LIMIT | u32::from(counter_type::SPELL));
        assert_eq!(limit.len(), 1);
        assert_eq!(f.effects.get(limit[0]).unwrap().value, 1, "at most one");

        let e1 = f.effects.get(trigger_effect(&f, br)).unwrap();
        assert!(e1.is_type(effect_type::SINGLE));
        assert!(e1.is_type(effect_type::TRIGGER_F), "forced");
        assert_eq!(e1.category, category::COUNTER);

        let e2 = f.effects.get(bonus_effect(&f, br)).unwrap();
        assert!(e2.is_flag(flag::SINGLE_RANGE));
        assert_eq!(e2.range, u16::from(location::MZONE));

        let e3 = f.effects.get(ignition_effect(&f, br)).unwrap();
        assert!(e3.is_type(effect_type::IGNITION));
        assert_eq!(e3.category, category::DESTROY);
        assert_eq!(e3.range, u16::from(location::MZONE));
        assert!(e3.is_flag(flag::CARD_TARGET));
        assert!(e3.cost.is_some());
    }

    /// **The place list says which counters this card places.** Nothing
    /// in this pool reads it; pinned so the declaration is not silently
    /// lost.
    #[test]
    fn the_place_list_names_the_spell_counter() {
        assert_eq!(COUNTER_PLACE_LIST, [counter_type::SPELL]);
    }

    /// **The library's counter table, as literals.**
    ///
    /// `check_constants.py` compares these against
    /// `card_counter_constants.lua`, but only when someone runs it; these
    /// are the numbers themselves. Several carry
    /// `COUNTER_WITHOUT_PERMIT` (`0x1000`) in the type word — the bit
    /// that decides whether a card needs permission to hold them — and
    /// dropping it from one entry is invisible everywhere except here.
    #[test]
    fn the_counter_table_is_what_the_library_says() {
        assert_eq!(counter_type::A, 0x100e);
        assert_eq!(counter_type::BUSHIDO, 0x3);
        assert_eq!(counter_type::EC, 0x217);
        assert_eq!(counter_type::FEATHER, 0x10);
        assert_eq!(counter_type::FOG, 0x1019);
        assert_eq!(counter_type::KAIJU, 0x37);
        assert_eq!(counter_type::PREDATOR, 0x1041);
        assert_eq!(counter_type::RESONANCE, 0x211);
        assert_eq!(counter_type::SIGNAL, 0x1148);
        assert_eq!(counter_type::SPELL, 0x1);
        assert_eq!(counter_type::VENOM, 0x1009);
    }

    /// **One counter on a Normal Summon, and the limit holds it to one.**
    #[test]
    fn it_takes_one_counter_and_no_more() {
        let (mut f, br) = field();
        let e = trigger_effect(&f, br);
        f.cards[br].create_chain_relation(e, 11);
        let ev = Event::new(code::SUMMON_SUCCESS);
        assert_eq!(api::get_counter(&f, br, counter_type::SPELL), 0);
        counterop(&mut f, &ctx_for(e, &ev, 0));
        assert_eq!(api::get_counter(&f, br, counter_type::SPELL), 1);
        // A second summon adds nothing: the limit is one.
        counterop(&mut f, &ctx_for(e, &ev, 0));
        assert_eq!(
            api::get_counter(&f, br, counter_type::SPELL),
            1,
            "the limit holds"
        );
    }

    /// **A counter placed under a permit goes in the *temporary* half**,
    /// which is what disabling the card takes away.
    ///
    /// Reading `get_counter(SPELL)` cannot tell the halves apart, so this
    /// looks at the stored pair directly — the permission is the whole
    /// reason for the split.
    #[test]
    fn the_counter_is_a_permitted_one_and_is_lost_on_being_disabled() {
        let (mut f, br) = field();
        let e = trigger_effect(&f, br);
        f.cards[br].create_chain_relation(e, 11);
        let ev = Event::new(code::SUMMON_SUCCESS);
        counterop(&mut f, &ctx_for(e, &ev, 0));
        let stored = f.cards[br].counters[&counter_type::SPELL];
        assert_eq!(stored, [0, 1], "permanent none, temporary one");
        assert_eq!(
            api::get_counter(&f, br, counter_type::SPELL | counter::WITHOUT_PERMIT),
            0,
            "and not in the without-permit slot, which is a separate key"
        );
    }

    /// **An unrelated Breaker takes no counter.**
    #[test]
    fn a_breaker_no_longer_related_takes_no_counter() {
        let (mut f, br) = field();
        let e = trigger_effect(&f, br);
        // No chain relation created.
        let ev = Event::new(code::SUMMON_SUCCESS);
        counterop(&mut f, &ctx_for(e, &ev, 0));
        assert_eq!(api::get_counter(&f, br, counter_type::SPELL), 0);
    }

    /// **The announcement names the card, the player, and the counter
    /// *type*** — the last in the slot most categories use for a count.
    #[test]
    fn the_announcement_names_the_counter_type() {
        for tp in [0u8, 1u8] {
            let (mut f, br) = field_as(tp);
            let e = trigger_effect(&f, br);
            let mut ch = Chain::new(e, Event::new(code::SUMMON_SUCCESS));
            ch.triggering_player = tp;
            ch.chain_count = 1;
            f.core.current_chain.push(ch);
            let ev = Event::new(code::SUMMON_SUCCESS);
            countertg(&mut f, &ctx_for(e, &ev, tp), true, None);
            let info = f.core.current_chain[0]
                .opinfos
                .get(&category::COUNTER)
                .expect("the counter category");
            assert_eq!(info.cards.as_deref(), Some(&[br][..]));
            assert_eq!(info.count, 1);
            assert_eq!(info.player, tp, "the summoning player");
            assert_eq!(
                info.param,
                i32::from(counter_type::SPELL),
                "the counter type, not a count"
            );
        }
    }

    /// **300 for each counter, recomputed every time it is asked** — so
    /// spending the counter takes the attack back in the same moment.
    #[test]
    fn the_attack_bonus_is_three_hundred_a_counter() {
        let (mut f, br) = field();
        let e = bonus_effect(&f, br);
        let ev = Event::new(0);
        let ask = |f: &Field| {
            let ctx = Ctx {
                reason_effect: e,
                player: 0,
                event: &ev,
                card: Some(br),
                args: &[],
            };
            attack_bonus(f.effects.get(e).unwrap(), f, &ctx)
        };
        assert_eq!(ask(&f), 0, "no counters, no bonus");
        let tr = trigger_effect(&f, br);
        f.cards[br].create_chain_relation(tr, 11);
        counterop(&mut f, &ctx_for(tr, &Event::new(code::SUMMON_SUCCESS), 0));
        assert_eq!(ask(&f), 300);
        // The effective attack agrees.
        assert_eq!(f.get_attack(br), 1600 + 300);
        f.remove_counter(br, counter_type::SPELL, 1);
        assert_eq!(ask(&f), 0, "and it goes back with the counter");
        assert_eq!(f.get_attack(br), 1600);
    }

    /// **The bonus is asked about the card it is on**, not about
    /// whatever Breaker happens to be. With no card in the context there
    /// is no bonus.
    #[test]
    fn the_bonus_reads_the_card_it_is_asked_about() {
        let (mut f, br) = field();
        let other = vanilla(&mut f, 0, 9_001, 1);
        let e = bonus_effect(&f, br);
        let tr = trigger_effect(&f, br);
        f.cards[br].create_chain_relation(tr, 11);
        counterop(&mut f, &ctx_for(tr, &Event::new(code::SUMMON_SUCCESS), 0));
        let ev = Event::new(0);
        let ask = |f: &Field, c: Option<CardId>| {
            let ctx = Ctx {
                reason_effect: e,
                player: 0,
                event: &ev,
                card: c,
                args: &[],
            };
            attack_bonus(f.effects.get(e).unwrap(), f, &ctx)
        };
        assert_eq!(ask(&f, Some(br)), 300);
        assert_eq!(ask(&f, Some(other)), 0, "a monster with no counter");
        assert_eq!(ask(&f, None), 0);
    }

    /// **The cost is one counter, checked before it is paid.**
    #[test]
    fn the_cost_is_one_spell_counter() {
        let (mut f, br) = field();
        let e = ignition_effect(&f, br);
        let ev = Event::new(0);
        let ctx = ctx_for(e, &ev, 0);
        assert!(!cost(&mut f, &ctx, false), "no counter, no cost to pay");
        let tr = trigger_effect(&f, br);
        f.cards[br].create_chain_relation(tr, 11);
        counterop(&mut f, &ctx_for(tr, &Event::new(code::SUMMON_SUCCESS), 0));
        assert!(cost(&mut f, &ctx, false), "one is enough");

        assert!(cost(&mut f, &ctx, true));
        match &f.core.subunits[0].kind {
            Kind::RemoveCounter {
                pcard,
                counter_type: ct,
                count,
                reason: why,
                ..
            } => {
                assert_eq!(*pcard, Some(br), "off itself");
                assert_eq!(*ct, counter_type::SPELL);
                assert_eq!(*count, 1);
                assert_ne!(why & reason::COST, 0, "as a cost");
            }
            other => panic!("expected a RemoveCounter: {other:?}"),
        }
    }

    /// **It wants a Spell or Trap on the field, either side**, and each
    /// clause gets a board where only it refuses.
    #[test]
    fn it_wants_a_spell_or_trap_on_the_field() {
        let (mut f, br) = field();
        let e = ignition_effect(&f, br);
        let ev = Event::new(0);
        let ctx = ctx_for(e, &ev, 0);
        // `core.reason_effect` is what `ExecuteTarget` would have set,
        // and the scan's **targeting** mode reads it to decide whether a
        // card can be targeted by this effect at all. Without it every
        // scan comes back empty and the negative assertions below hold
        // for the wrong reason.
        let ask = |f: &mut Field, chkc: Option<CardId>| {
            f.core.reason_effect = Some(e);
            destg(f, &ctx, false, chkc).finished().unwrap_or(0) != 0
        };
        assert!(!ask(&mut f, None), "nothing to break");

        let mine = spell_trap(&mut f, 0, 9_100, 0, false);
        assert!(ask(&mut f, Some(mine)), "its own counts");
        assert!(ask(&mut f, None));
        let theirs = spell_trap(&mut f, 1, 9_101, 0, true);
        assert!(ask(&mut f, Some(theirs)), "and so does the opponent's");

        // A monster is not a Spell or Trap.
        let monster = vanilla(&mut f, 1, 9_102, 1);
        assert!(!ask(&mut f, Some(monster)));
        // A Trap in a graveyard is a Spell/Trap but not on the field.
        f.move_card(1, theirs, location::GRAVE, 0, false);
        assert!(!ask(&mut f, Some(theirs)), "off the field");
    }

    /// **The same, as player 1.**
    #[test]
    fn the_scan_is_two_sided_whichever_player_asks() {
        for tp in [0u8, 1u8] {
            let (mut f, br) = field_as(tp);
            let e = ignition_effect(&f, br);
            let ev = Event::new(0);
            let ctx = ctx_for(e, &ev, tp);
            f.core.reason_effect = Some(e);
            assert!(destg(&mut f, &ctx, false, None).finished().unwrap_or(0) == 0);
            // Put the only Spell/Trap on the **opponent's** side.
            spell_trap(&mut f, 1 - tp, 9_100, 0, false);
            f.core.reason_effect = Some(e);
            assert!(destg(&mut f, &ctx, false, None).finished().unwrap_or(0) != 0);
        }
    }

    /// Drive the target then the operation, answering the one question.
    fn resolve(f: &mut Field, br: CardId, e: EffectId, tp: u8, pick: usize) -> (u8, Vec<CardId>) {
        let mut ch = Chain::new(e, Event::new(0));
        ch.triggering_player = tp;
        ch.chain_count = 1;
        ch.chain_id = 11;
        f.core.current_chain.push(ch);
        f.core.chain_solving = true;
        f.cards[br].create_chain_relation(e, 11);
        f.core.sub_solving_event.push_back(Event::new(0));
        f.emplace(Kind::ExecuteTarget {
            resume: None,
            effect: e,
            player: tp,
            subject: None,
            args: Vec::new(),
            was_disabled: false,
        });
        let mut offered = Vec::new();
        let mut asked_of = u8::MAX;
        let mut operated = false;
        for _ in 0..8192 {
            match f.process() {
                Status::Continue => {
                    if f.core.units.is_empty() && f.core.subunits.is_empty() {
                        if operated {
                            break;
                        }
                        operated = true;
                        f.core.sub_solving_event.push_back(Event::new(0));
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
                    Some(Message::SelectCard {
                        player, min, cards, ..
                    }) => {
                        asked_of = *player;
                        offered = cards.clone();
                        let min = usize::from(*min);
                        f.core.return_cards.clear();
                        f.core.returns.set_i32(0, 0);
                        f.core.returns.set_i32(1, min as i32);
                        for i in 0..min {
                            f.core.returns.set_i32(i + 2, (pick + i) as i32);
                        }
                    }
                    other => panic!("unexpected question: {other:?}"),
                },
                other => panic!("stopped with {other:?}"),
            }
        }
        (asked_of, offered)
    }

    /// **It destroys the one chosen, from either side, and announces
    /// it** — prefaced by the destroy prompt.
    #[test]
    fn it_destroys_the_spell_or_trap_it_chose() {
        let (mut f, br) = field();
        let e = ignition_effect(&f, br);
        let mine = spell_trap(&mut f, 0, 9_100, 0, false);
        let theirs = spell_trap(&mut f, 1, 9_101, 0, true);
        let (asked_of, offered) = resolve(&mut f, br, e, 0, 0);
        assert_eq!(asked_of, 0, "its own controller chooses");
        let mut want = vec![mine, theirs];
        want.sort_unstable();
        let mut got = offered.clone();
        got.sort_unstable();
        assert_eq!(got, want, "both sides offered");
        let chosen = offered[0];
        let spared = *want.iter().find(|&&c| c != chosen).unwrap();
        assert_eq!(
            f.cards[chosen].current.location,
            location::GRAVE,
            "the one chosen is gone"
        );
        assert_eq!(f.cards[spared].current.location, location::SZONE);
        assert!(f
            .messages
            .iter()
            .any(|m| matches!(m, Message::Hint { kind, player, value }
                if *kind == hint::SELECTMSG && *player == 0 && *value == hintmsg::DESTROY)));
        let info = f.core.current_chain[0]
            .opinfos
            .get(&category::DESTROY)
            .expect("the destroy category");
        assert_eq!(info.cards.as_deref(), Some(&[chosen][..]));
        assert_eq!(info.count, 1);
        assert_eq!(info.player, 0);
    }

    /// **A target no longer related is left alone.**
    #[test]
    fn a_target_no_longer_related_is_not_destroyed() {
        let (mut f, br) = field();
        let e = ignition_effect(&f, br);
        let tc = spell_trap(&mut f, 1, 9_101, 0, true);
        let mut ch = Chain::new(e, Event::new(0));
        ch.triggering_player = 0;
        ch.chain_count = 1;
        ch.chain_id = 11;
        ch.target_cards = vec![tc];
        f.core.current_chain.push(ch);
        f.core.chain_solving = true;
        // No relation created for the target.
        let ev = Event::new(0);
        desop(&mut f, &ctx_for(e, &ev, 0));
        assert!(f.core.subunits.is_empty(), "nothing queued");
    }
}

//! Jinzo — `c77585513.lua`.
//!
//! The thirty-fifth card, and the pool's first **continuous negation**.
//! Five field effects and a library marker, and the striking thing is how
//! little of it is one mechanism: stopping Traps takes four different
//! prohibitions because there are four different moments to stop them at.
//!
//! | effect | code | stops |
//! |---|---|---|
//! | `e1` | `EFFECT_CANNOT_TRIGGER` | a Trap in a hand or row from triggering |
//! | `e2` | `EFFECT_CANNOT_ACTIVATE` | the opponent activating a Trap at all |
//! | `e3` | `EFFECT_DISABLE` | a Trap already in a row from applying |
//! | `e4` | `EVENT_CHAIN_SOLVING` | a Trap effect resolving, by negating it |
//! | `e5` | `EFFECT_DISABLE_TRAPMONSTER` | a trap monster from applying |
//!
//! ## Two of them are about the *card*, two about the *effect*
//!
//! `e1`, `e3` and `e5` take a **target range** and a filter, and apply to
//! cards: hand and row for the trigger ban, the row for the disable, the
//! Monster Zone for trap monsters. They are what stops a Trap being a
//! Trap.
//!
//! `e2` is the odd one. It is `EFFECT_FLAG_PLAYER_TARGET` with a target
//! range of `(1, 1)` — *both players*, not a location — and its value is a
//! function asked about the **activating effect** rather than about a
//! card. That is the difference between "this card cannot do anything"
//! and "you cannot activate that".
//!
//! And it carries `RESET_PHASE|PHASE_END`, which is a lifetime the other
//! four do not have.
//!
//! ## `e4` negates rather than prohibits
//!
//! ```lua
//! local tl=Duel.GetChainInfo(ev,CHAININFO_TRIGGERING_LOCATION)
//! if tl==LOCATION_SZONE and re:IsTrapEffect() then Duel.NegateEffect(ev) end
//! ```
//!
//! A continuous field effect on `EVENT_CHAIN_SOLVING`, which fires as each
//! link begins to resolve. Two tests, and both are about the *link* rather
//! than the card: where it was activated from, and whether the effect's
//! **active** type is Trap. A Trap activated from somewhere other than a
//! Spell & Trap Zone is left alone.
//!
//! `ev` is the chain count, so `GetChainInfo(ev, ...)` and
//! `NegateEffect(ev)` both name the link that is solving.
//!
//! ## The marker at the end
//!
//! `aux.DoubleSnareValidity(c, LOCATION_MZONE)`
//! (`cards_specific_functions.lua:766`) registers a single effect with
//! code **3682106** — Double Snare's own card number, used as a flag that
//! card reads. Double Snare is not in this pool, so nothing reads it; it
//! is registered anyway, and pinned, rather than dropped.
//!
//! Every call here is to `script_api`, by the rule in `cards/mod.rs`.

use crate::board::location;
use crate::duel::phases;
use crate::effect::{effect_type, flag, Ctx, Yield};
use crate::event::{code, CardId};
use crate::field::{reset, Field};
use crate::script_api as api;

pub const CODE: u32 = 77_585_513;

const HAND_OR_SZONE: u16 = (location::HAND | location::SZONE) as u16;

pub fn initial_effect(f: &mut Field, c: CardId) {
    // cannot trigger
    let e1 = api::create_effect(f, c);
    api::set_type(f, e1, effect_type::FIELD);
    api::set_code(f, e1, code::CANNOT_TRIGGER);
    api::set_property(f, e1, flag::SET_AVAILABLE, 0);
    api::set_range(f, e1, u16::from(location::MZONE));
    api::set_target_range(f, e1, HAND_OR_SZONE, HAND_OR_SZONE);
    api::set_target_filter(f, e1, api::target_is_trap);
    api::register_effect(f, c, e1, false);
    // cannot activate
    let e2 = api::create_effect(f, c);
    api::set_type(f, e2, effect_type::FIELD);
    api::set_code(f, e2, code::CANNOT_ACTIVATE);
    api::set_property(f, e2, flag::PLAYER_TARGET, 0);
    api::set_target_range(f, e2, 1, 1);
    api::set_value_fn(f, e2, aclimit);
    api::set_reset(f, e2, reset::PHASE | u32::from(phases::END), 1);
    api::register_effect(f, c, e2, false);
    // disable
    let e3 = api::create_effect(f, c);
    api::set_type(f, e3, effect_type::FIELD);
    api::set_code(f, e3, code::DISABLE);
    api::set_range(f, e3, u16::from(location::MZONE));
    api::set_target_range(
        f,
        e3,
        u16::from(location::SZONE),
        u16::from(location::SZONE),
    );
    api::set_target_filter(f, e3, api::target_is_trap);
    api::register_effect(f, c, e3, false);
    // disable effect
    let e4 = api::create_effect(f, c);
    api::set_type(f, e4, effect_type::FIELD | effect_type::CONTINUOUS);
    api::set_code(f, e4, code::CHAIN_SOLVING);
    api::set_range(f, e4, u16::from(location::MZONE));
    api::set_operation(f, e4, disop);
    api::register_effect(f, c, e4, false);
    // disable trap monster
    let e5 = api::create_effect(f, c);
    api::set_type(f, e5, effect_type::FIELD);
    api::set_code(f, e5, code::DISABLE_TRAPMONSTER);
    api::set_range(f, e5, u16::from(location::MZONE));
    api::set_target_range(
        f,
        e5,
        u16::from(location::MZONE),
        u16::from(location::MZONE),
    );
    api::set_target_filter(f, e5, api::target_is_trap);
    api::register_effect(f, c, e5, false);
    api::double_snare_validity(f, c, u16::from(location::MZONE), 0);
}

/// `s.disop` — negate a Trap effect resolving from a Spell & Trap Zone.
fn disop(f: &mut Field, ctx: &Ctx) -> Yield {
    // `ev` is the chain count of the link that is solving.
    let ev = ctx.event.event_value as u8;
    let Some(re) = api::get_chain_triggering_effect(f, ev) else {
        return api::done();
    };
    if api::get_chain_triggering_location(f, ev) == u16::from(location::SZONE)
        && api::is_trap_effect(f, re)
    {
        api::negate_effect(f, ev);
    }
    api::done()
}

/// `s.aclimit(e, re, tp)` — the activating effect's handler is a Trap.
///
/// A **value** function on `EFFECT_CANNOT_ACTIVATE`, and the thing to get
/// right is which effect it is asked about. `is_action_check` puts the
/// effect **trying to activate** in `ctx.reason_effect` — the script's
/// `re` — and the player in `ctx.player`. `ctx.card` is nothing here,
/// because the question is about an activation and not about a card.
fn aclimit(_e: &crate::effect::Effect, f: &Field, ctx: &Ctx) -> i64 {
    i64::from(api::handler_is_trap(f, ctx.reason_effect))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::position;
    use crate::card::{card_type, status, Card, CardData};
    use crate::chain::{chain_flag, Chain};
    use crate::effect::Effect;
    use crate::event::{EffectId, Event};
    use crate::processor::Status;

    /// Jinzo face-up in `tp`'s first Monster Zone, on `tp`'s turn.
    fn field_as(tp: u8) -> (Field, CardId) {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        f.infos.turn_player = tp;
        f.infos.phase = phases::MAIN1;
        let mut d = Card::with_data(crate::cards::card_data(CODE).expect("printed data"), tp);
        d.current.controller = tp;
        d.set_status(status::EFFECT_ENABLED, true);
        let j = f.new_card(d);
        f.add_card(tp, j, location::MZONE, 0, false);
        f.cards[j].current.position = position::FACEUP_ATTACK;
        f.initialize_card(j);
        f.apply_field_effect(j);
        (f, j)
    }

    fn field() -> (Field, CardId) {
        field_as(0)
    }

    fn effect_with(f: &Field, j: CardId, code_: u32) -> EffectId {
        f.cards[j].field_effect.equal_range(code_)[0]
    }

    /// A Trap in `player`'s Spell & Trap row, with an activate effect.
    fn trap_with_effect(f: &mut Field, player: u8, seq: u32) -> (CardId, EffectId) {
        let mut c = Card::with_data(
            CardData {
                code: 9_500 + seq,
                type_: card_type::TRAP,
                ..Default::default()
            },
            player,
        );
        c.current.controller = player;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(player, id, location::SZONE, seq, false);
        f.cards[id].current.position = position::FACEUP;
        let mut e = Effect::new(effect_type::ACTIVATE, code::FREE_CHAIN);
        e.owner = Some(id);
        e.handler = Some(id);
        e.range = u16::from(location::SZONE);
        let eid = f.new_effect(e);
        f.cards[id].field_effect.insert(code::FREE_CHAIN, eid);
        f.cards[id].indexer.insert(eid);
        (id, eid)
    }

    /// **Five field effects and a marker**, each with its own code, range
    /// and target range — because stopping Traps takes a different
    /// prohibition at each moment.
    #[test]
    fn the_script_registers_five_prohibitions_and_a_marker() {
        let (f, j) = field();
        let e1 = f
            .effects
            .get(effect_with(&f, j, code::CANNOT_TRIGGER))
            .unwrap();
        assert!(e1.is_type(effect_type::FIELD));
        assert!(e1.is_flag(flag::SET_AVAILABLE), "even a Set Trap");
        assert_eq!(e1.range, u16::from(location::MZONE));
        // Spelled out rather than compared against the constant: an
        // assertion that reads the same constant the code does agrees
        // with any value it is given.
        let hand_or_szone = u16::from(location::HAND) | u16::from(location::SZONE);
        assert_eq!(e1.s_range, hand_or_szone, "a hand and a row");
        assert_eq!(e1.o_range, hand_or_szone);
        assert!(e1.target_filter.is_some());

        let e2 = f
            .effects
            .get(effect_with(&f, j, code::CANNOT_ACTIVATE))
            .unwrap();
        assert!(e2.is_flag(flag::PLAYER_TARGET), "a player, not a location");
        assert_eq!((e2.s_range, e2.o_range), (1, 1), "both players");
        assert!(e2.value_fn.is_some(), "and a function, not a filter");
        assert_ne!(e2.reset_flag & reset::PHASE, 0);
        assert_ne!(e2.reset_flag & u32::from(phases::END), 0);

        let e3 = f.effects.get(effect_with(&f, j, code::DISABLE)).unwrap();
        assert_eq!(e3.s_range, u16::from(location::SZONE));
        assert_eq!(e3.o_range, u16::from(location::SZONE));

        let e4 = f
            .effects
            .get(effect_with(&f, j, code::CHAIN_SOLVING))
            .unwrap();
        assert!(e4.is_type(effect_type::FIELD));
        assert!(e4.is_type(effect_type::CONTINUOUS));
        assert_eq!(e4.range, u16::from(location::MZONE));
        assert!(e4.operation.is_some());

        let e5 = f
            .effects
            .get(effect_with(&f, j, code::DISABLE_TRAPMONSTER))
            .unwrap();
        assert_eq!(e5.range, u16::from(location::MZONE), "only from the field");
        assert_eq!(e5.s_range, u16::from(location::MZONE));
        assert_eq!(e5.o_range, u16::from(location::MZONE));
        assert_eq!(e3.range, u16::from(location::MZONE));

        // The literal, not the constant: `aux.DoubleSnareValidity` uses
        // Double Snare's own card number as the flag, and the whole
        // point of the marker is that *that* number is what another card
        // looks for.
        assert_eq!(api::CARD_DOUBLE_SNARE, 3_682_106);
        let marker = f.cards[j].single_effect.equal_range(3_682_106);
        assert_eq!(marker.len(), 1, "Double Snare's flag");
        let m = f.effects.get(marker[0]).unwrap();
        assert!(m.is_flag(flag::CANNOT_DISABLE));
        assert!(m.is_flag(flag::SINGLE_RANGE));
        assert_eq!(m.range, u16::from(location::MZONE));
    }

    /// **The four card-facing prohibitions all filter on being a Trap**,
    /// and the filter is the same one.
    #[test]
    fn the_prohibitions_apply_to_traps_and_nothing_else() {
        let (mut f, j) = field();
        let (trap, _) = trap_with_effect(&mut f, 1, 0);
        let mut spell = Card::with_data(
            CardData {
                code: 9_600,
                type_: card_type::SPELL,
                ..Default::default()
            },
            1,
        );
        spell.current.controller = 1;
        let spell = f.new_card(spell);
        f.add_card(1, spell, location::SZONE, 1, false);
        f.cards[spell].current.position = position::FACEUP;

        for code_ in [
            code::CANNOT_TRIGGER,
            code::DISABLE,
            code::DISABLE_TRAPMONSTER,
        ] {
            let e = effect_with(&f, j, code_);
            assert!(f.is_fit_target_function(e, trap), "{code_} accepts a Trap");
            assert!(
                !f.is_fit_target_function(e, spell),
                "{code_} leaves a Spell alone"
            );
        }
        let _ = j;
    }

    /// **A Trap in a Spell & Trap Zone is disabled while Jinzo is
    /// there**, and a Spell beside it is not.
    #[test]
    fn a_trap_in_the_row_is_disabled() {
        let (mut f, _) = field();
        let (trap, _) = trap_with_effect(&mut f, 1, 0);
        f.adjust_all();
        for _ in 0..4096 {
            match f.process() {
                Status::Continue => {
                    if f.core.units.is_empty() && f.core.subunits.is_empty() {
                        break;
                    }
                }
                other => panic!("stopped with {other:?}"),
            }
        }
        assert!(
            f.cards[trap].is_status(status::DISABLED),
            "the Trap is switched off"
        );
    }

    /// **`s.aclimit` answers about the effect that is trying to
    /// activate**, not about a card on the board.
    #[test]
    fn the_activation_ban_reads_the_activating_effect() {
        let (mut f, j) = field();
        let (_, trap_effect) = trap_with_effect(&mut f, 1, 0);
        let mut spell = Card::with_data(
            CardData {
                code: 9_600,
                type_: card_type::SPELL,
                ..Default::default()
            },
            1,
        );
        spell.current.controller = 1;
        let spell = f.new_card(spell);
        f.add_card(1, spell, location::SZONE, 1, false);
        let mut se = Effect::new(effect_type::ACTIVATE, code::FREE_CHAIN);
        se.owner = Some(spell);
        se.handler = Some(spell);
        let spell_effect = f.new_effect(se);

        let e2 = effect_with(&f, j, code::CANNOT_ACTIVATE);
        let ev = Event::new(0);
        let ask = |f: &Field, re: EffectId| {
            let ctx = Ctx {
                reason_effect: re,
                player: 1,
                event: &ev,
                card: None,
                args: &[],
            };
            aclimit(f.effects.get(e2).unwrap(), f, &ctx)
        };
        assert_eq!(ask(&f, trap_effect), 1, "a Trap is banned");
        assert_eq!(ask(&f, spell_effect), 0, "a Spell is not");
    }

    /// **`s.disop` negates a Trap effect resolving from a Spell & Trap
    /// Zone**, and leaves everything else alone.
    ///
    /// Four boards, one per clause: the positive case, a Spell effect, a
    /// Trap effect activated from somewhere else, and the link already
    /// negated.
    #[test]
    fn it_negates_a_trap_resolving_from_the_row() {
        for (is_trap, from_row, expect) in [
            (true, true, true),
            (false, true, false),
            (true, false, false),
            (false, false, false),
        ] {
            let (mut f, j) = field();
            let ty = if is_trap {
                card_type::TRAP
            } else {
                card_type::SPELL
            };
            let mut c = Card::with_data(
                CardData {
                    code: 9_700,
                    type_: ty,
                    ..Default::default()
                },
                1,
            );
            c.current.controller = 1;
            let tc = f.new_card(c);
            f.add_card(1, tc, location::SZONE, 0, false);
            f.cards[tc].current.position = position::FACEUP;
            let mut e = Effect::new(effect_type::ACTIVATE, code::FREE_CHAIN);
            e.owner = Some(tc);
            e.handler = Some(tc);
            e.card_type = ty;
            let re = f.new_effect(e);

            let mut ch = Chain::new(re, Event::new(code::FREE_CHAIN));
            ch.chain_count = 1;
            ch.chain_id = 11;
            ch.triggering_player = 1;
            ch.triggering_location = if from_row {
                u16::from(location::SZONE)
            } else {
                u16::from(location::HAND)
            };
            f.core.current_chain.push(ch);

            let e4 = effect_with(&f, j, code::CHAIN_SOLVING);
            let mut ev = Event::new(code::CHAIN_SOLVING);
            ev.event_value = 1;
            let ctx = Ctx {
                reason_effect: e4,
                player: 0,
                event: &ev,
                card: None,
                args: &[],
            };
            disop(&mut f, &ctx);
            let negated = f.core.current_chain[0].flag & chain_flag::DISABLE_EFFECT != 0;
            assert_eq!(negated, expect, "trap {is_trap}, from the row {from_row}");
        }
    }

    /// **`ev` names the link that is solving, not the topmost one.**
    ///
    /// With two links on the chain, negating "the top" instead of the one
    /// the event named hits the wrong effect — and reading the location
    /// off the wrong link answers about the wrong card.
    #[test]
    fn it_negates_the_link_the_event_named() {
        let (mut f, j) = field();
        // Link 1: a Trap from the row, which should be negated.
        let mut t = Card::with_data(
            CardData {
                code: 9_700,
                type_: card_type::TRAP,
                ..Default::default()
            },
            1,
        );
        t.current.controller = 1;
        let tc = f.new_card(t);
        f.add_card(1, tc, location::SZONE, 0, false);
        f.cards[tc].current.position = position::FACEUP;
        let mut te = Effect::new(effect_type::ACTIVATE, code::FREE_CHAIN);
        te.owner = Some(tc);
        te.handler = Some(tc);
        te.card_type = card_type::TRAP;
        let trap_effect = f.new_effect(te);
        let mut ch1 = Chain::new(trap_effect, Event::new(code::FREE_CHAIN));
        ch1.chain_count = 1;
        ch1.chain_id = 11;
        ch1.triggering_player = 1;
        ch1.triggering_location = u16::from(location::SZONE);
        f.core.current_chain.push(ch1);

        // Link 2: a Spell from the hand, which should not be.
        let mut sp = Card::with_data(
            CardData {
                code: 9_800,
                type_: card_type::SPELL,
                ..Default::default()
            },
            0,
        );
        sp.current.controller = 0;
        let sc = f.new_card(sp);
        f.add_card(0, sc, location::SZONE, 1, false);
        f.cards[sc].current.position = position::FACEUP;
        let mut se = Effect::new(effect_type::ACTIVATE, code::FREE_CHAIN);
        se.owner = Some(sc);
        se.handler = Some(sc);
        se.card_type = card_type::SPELL;
        let spell_effect = f.new_effect(se);
        let mut ch2 = Chain::new(spell_effect, Event::new(code::FREE_CHAIN));
        ch2.chain_count = 2;
        ch2.chain_id = 12;
        ch2.triggering_player = 0;
        ch2.triggering_location = u16::from(location::HAND);
        f.core.current_chain.push(ch2);

        let e4 = effect_with(&f, j, code::CHAIN_SOLVING);
        let mut ev = Event::new(code::CHAIN_SOLVING);
        // The **first** link is solving.
        ev.event_value = 1;
        let ctx = Ctx {
            reason_effect: e4,
            player: 0,
            event: &ev,
            card: None,
            args: &[],
        };
        disop(&mut f, &ctx);
        assert_ne!(
            f.core.current_chain[0].flag & chain_flag::DISABLE_EFFECT,
            0,
            "the Trap link was negated"
        );
        assert_eq!(
            f.core.current_chain[1].flag & chain_flag::DISABLE_EFFECT,
            0,
            "and the Spell link on top of it was not"
        );

        // The mirror: the same two links with the roles reversed, and the
        // **second** solving. Without this half a hard-coded `1` reads
        // the same as the event's own count.
        f.core.current_chain.clear();
        let mut ch1 = Chain::new(spell_effect, Event::new(code::FREE_CHAIN));
        ch1.chain_count = 1;
        ch1.chain_id = 21;
        ch1.triggering_player = 0;
        ch1.triggering_location = u16::from(location::HAND);
        f.core.current_chain.push(ch1);
        let mut ch2 = Chain::new(trap_effect, Event::new(code::FREE_CHAIN));
        ch2.chain_count = 2;
        ch2.chain_id = 22;
        ch2.triggering_player = 1;
        ch2.triggering_location = u16::from(location::SZONE);
        f.core.current_chain.push(ch2);

        let mut ev = Event::new(code::CHAIN_SOLVING);
        ev.event_value = 2;
        let ctx = Ctx {
            reason_effect: e4,
            player: 0,
            event: &ev,
            card: None,
            args: &[],
        };
        disop(&mut f, &ctx);
        assert_eq!(
            f.core.current_chain[0].flag & chain_flag::DISABLE_EFFECT,
            0,
            "the Spell link below was not touched"
        );
        assert_ne!(
            f.core.current_chain[1].flag & chain_flag::DISABLE_EFFECT,
            0,
            "and the Trap link the event named was"
        );
    }
}

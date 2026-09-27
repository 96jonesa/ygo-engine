//! Ryu Senshi (`49868263`) — `cardscripts/c49868263.lua`.
//!
//! Dark Balter's mirror, and then two effects more. The quick negation is
//! the same shape with `TYPE_TRAP` where that card had `TYPE_SPELL`; what
//! follows is about Spells that aim at Ryu Senshi rather than Spells it
//! answers on the chain.
//!
//! ## One card, both readings of the active type
//!
//! ```lua
//! -- s.discon
//! re:IsHasType(EFFECT_TYPE_ACTIVATE) and re:GetActiveType()==TYPE_TRAP
//! -- s.disop2
//! re:IsSpellEffect()          -- utility.lua:790: e:IsActiveType(TYPE_SPELL)
//! ```
//!
//! The first is an **equality** over the whole printed type word, so it
//! answers Normal Traps and nothing else — every Trap in this pool is a
//! Normal Trap, so the distinction costs nothing here, but it is the same
//! line that confines Dark Balter to Normal Spells and it is transcribed
//! the same way.
//!
//! The second is the library's helper, and the library wrote a **mask**:
//! any Spell at all, Quick-Play and Equip included. Two readings of the
//! same question, forty lines apart, and the port keeps them apart
//! because the reference does.
//!
//! ## Two ways a Spell can aim at this card, and two different answers
//!
//! `e2` is Fiend Skull Dragon's second half with a destruction bolted on:
//! a Spell effect **already on the chain** carrying
//! `EFFECT_FLAG_CARD_TARGET`, whose chain link names Ryu Senshi, is
//! negated — and if its card is still related to the effect, destroyed.
//!
//! `e3` and `e4` are about a Spell that is **sitting there** targeting
//! it: an Equip Spell, in this pool. They share a filter and split the
//! work — `EFFECT_DISABLE` switches the Spell off, `EFFECT_SELF_DESTROY`
//! marks it for the destruction sweep. Neither is a chain link; both read
//! `Card.GetCardTarget`, which is the *card's* target list and points the
//! opposite way from the chain link's.
//!
//! Snatch Steal and Premature Burial are the pool's two Equip Spells, so
//! both halves have something real to bite on.
//!
//! ## `Duel.NegateEffect(ev) and ...IsRelateToEffect(re)`
//!
//! The `and` short-circuits, so the relation is asked only when the
//! negation took. A Spell whose card has already left the field is
//! negated and not destroyed — which is what `IsRelateToEffect` is for,
//! and why it is asked after the negation rather than before.
//!
//! ## `GetCardTargetCount()>0` is redundant, and is kept
//!
//! An empty target list cannot contain anything, so the count test adds
//! nothing to the `IsContains` below it. The reference wrote it; the port
//! writes it. A mutant that drops it would be equivalent by construction,
//! so none is listed.

use crate::board::location;
use crate::card::{card_type, reason, status};
use crate::effect::{effect_type, flag, Ctx, Yield};
use crate::event::{category, code, CardId, EffectId};
use crate::field::Field;
use crate::script_api as api;

pub const CODE: u32 = 49868263;

/// `Fusion.AddProcMix(c,false,false,75953262,67957315)` — two vanilla
/// monsters with no script, each pinned against `s.initial_effect` by
/// `tools/check_constants.py`.
const MATERIAL_A: u32 = 75_953_262;
const MATERIAL_B: u32 = 67_957_315;
const MATERIALS: [u32; 2] = [MATERIAL_A, MATERIAL_B];

/// `Cost.PayLP(1000)`, pinned against the script.
const COST_LP: u32 = 1000;

const MZONE: u16 = location::MZONE as u16;
const SZONE: u16 = location::SZONE as u16;

pub fn initial_effect(f: &mut Field, c: CardId) {
    api::enable_revive_limit(f, c);
    super::proc_fusion::add_proc_mix(f, c, false, false, &MATERIALS);
    // Negate a Trap's activation
    let e1 = api::create_effect(f, c);
    api::set_description(f, e1, api::stringid(CODE, 0));
    api::set_category(f, e1, category::DISABLE);
    api::set_type(f, e1, effect_type::QUICK_O);
    api::set_code(f, e1, code::CHAINING);
    api::set_range(f, e1, MZONE);
    api::set_condition(f, e1, discon);
    api::set_cost(f, e1, cost);
    api::set_target(f, e1, distg);
    api::set_operation(f, e1, disop);
    api::register_effect(f, c, e1, false);
    // Negate — and destroy — a targeting Spell as it resolves
    let e2 = api::create_effect(f, c);
    api::set_type(f, e2, effect_type::FIELD | effect_type::CONTINUOUS);
    api::set_code(f, e2, code::CHAIN_SOLVING);
    api::set_range(f, e2, MZONE);
    api::set_operation(f, e2, disop2);
    api::register_effect(f, c, e2, false);
    // Switch off a Spell that is sitting there targeting it
    let e3 = api::create_effect(f, c);
    api::set_type(f, e3, effect_type::FIELD);
    api::set_code(f, e3, code::DISABLE);
    api::set_range(f, e3, MZONE);
    api::set_target_range(f, e3, SZONE, SZONE);
    api::set_target_filter(f, e3, distg2);
    api::register_effect(f, c, e3, false);
    // And mark it for the destruction sweep
    let e4 = api::create_effect(f, c);
    api::set_type(f, e4, effect_type::FIELD);
    api::set_code(f, e4, code::SELF_DESTROY);
    api::set_range(f, e4, MZONE);
    api::set_target_range(f, e4, SZONE, SZONE);
    api::set_target_filter(f, e4, distg2);
    api::register_effect(f, c, e4, false);
    api::double_snare_validity(f, c, MZONE, 0);
}

/// `s.discon` — a Normal Trap, being activated, while this card is still
/// alive. Dark Balter's condition with the type swapped.
fn discon(f: &mut Field, ctx: &Ctx) -> bool {
    let Some(c) = api::get_handler(f, ctx.reason_effect) else {
        return false;
    };
    if api::is_status(f, c, status::BATTLE_DESTROYED) {
        return false;
    }
    let Some(re) = ctx.event.reason_effect else {
        return false;
    };
    api::is_has_type(f, re, effect_type::ACTIVATE)
        // **An equality, not a mask** — see the module comment.
        && api::get_active_type(f, re) == card_type::TRAP
        && api::is_chain_disablable(f, ctx.event.event_value as u8)
}

/// `Cost.PayLP(1000)` — the library's helper, spelled out.
fn cost(f: &mut Field, ctx: &Ctx, chk: bool) -> bool {
    let tp = ctx.player;
    if !chk {
        return api::check_lp_cost(f, tp, COST_LP);
    }
    api::pay_lp_cost(f, tp, COST_LP);
    true
}

/// `s.distg` — no target to choose, only the announcement.
fn distg(f: &mut Field, ctx: &Ctx, chk: bool, _chkc: Option<CardId>) -> Yield {
    if !chk {
        return api::yes(true);
    }
    let eg = api::event_cards(ctx).to_vec();
    api::set_operation_info(f, 0, category::DISABLE, Some(eg), 1, 0, 0);
    api::yes(true)
}

/// `s.disop` — negate the link the `EVENT_CHAINING` event named.
fn disop(f: &mut Field, ctx: &Ctx) -> Yield {
    api::negate_effect(f, ctx.event.event_value as u8);
    api::done()
}

/// `s.disop2` — a resolving Spell that targeted this card is negated, and
/// its card destroyed if it is still related to the effect.
fn disop2(f: &mut Field, ctx: &Ctx) -> Yield {
    // `ev` is the chain count of the link that is solving.
    let ev = ctx.event.event_value as u8;
    let Some(re) = api::get_chain_triggering_effect(f, ev) else {
        return api::done();
    };
    if !(api::is_spell_effect(f, re) && api::is_has_property(f, re, flag::CARD_TARGET, 0)) {
        return api::done();
    }
    let targets = api::get_chain_target_cards(f, ev);
    let Some(handler) = api::get_handler(f, ctx.reason_effect) else {
        return api::done();
    };
    if !targets.contains(&handler) {
        return api::done();
    }
    // **The `and` short-circuits**: the relation is asked only when the
    // negation took, so a Spell whose card has already left is negated
    // and not destroyed.
    if api::negate_effect(f, ev) {
        if let Some(h) = api::get_handler(f, re) {
            if api::is_relate_to_effect(f, h, re) {
                api::destroy(f, vec![h], reason::EFFECT);
            }
        }
    }
    api::done()
}

/// `s.distg2` — the filter `e3` and `e4` share: a Spell that is currently
/// targeting this card.
///
/// `c:IsSpell()` reads the **effective** type in the reference. The seam
/// hands out `&Field` and the effective-type reader needs it mutably, so
/// this reads the printed line — the same approximation
/// `fiend_skull_dragon::target_is_flip` makes, licensed by the same
/// `ABSENT_FROM_POOL` scan for the `EFFECT_ADD_TYPE` family.
fn distg2(f: &Field, e: EffectId, target: Option<CardId>, _a: &[i64]) -> bool {
    let Some(c) = target else {
        return false;
    };
    let Some(handler) = api::get_handler(f, e) else {
        return false;
    };
    api::get_card_target_count(f, c) > 0
        && api::is_type_readonly(f, c, card_type::SPELL)
        && api::get_card_target(f, c).contains(&handler)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::position;
    use crate::card::{attribute, race, Card, CardData};
    use crate::chain::Chain;
    use crate::duel::phases;
    use crate::event::Event;
    use crate::processor::Status;

    /// Ryu Senshi in `tp`'s Monster Zone, initialised.
    fn board(tp: u8) -> (Field, CardId) {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        f.infos.turn_player = tp;
        f.infos.phase = phases::MAIN1;
        let mut d = Card::with_data(crate::cards::card_data(CODE).expect("printed data"), tp);
        d.current.controller = tp;
        d.set_status(status::EFFECT_ENABLED, true);
        let rs = f.new_card(d);
        f.add_card(tp, rs, location::MZONE, 0, false);
        f.cards[rs].current.position = position::FACEUP_ATTACK;
        f.initialize_card(rs);
        (f, rs)
    }

    fn quick_effect(f: &Field, rs: CardId) -> EffectId {
        f.cards[rs].field_effect.equal_range(code::CHAINING)[0]
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

    /// A Spell or Trap in `player`'s Spell row, face-up.
    fn spelltrap(f: &mut Field, player: u8, seq: u32, type_: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 8100 + seq + u32::from(player) * 50,
                type_,
                ..Default::default()
            },
            player,
        );
        c.current.controller = player;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(player, id, location::SZONE, seq, false);
        f.cards[id].current.position = position::FACEUP;
        id
    }

    /// A card of the given printed type, with an effect of the given
    /// declared type on it, put on the chain as **link one** — and with a
    /// decoy link on top so `ev` and "the current link" come apart.
    ///
    /// The decoy is the lesson `port-mutation-blind-spots` records as
    /// *one chain link is not enough*, applied before the mutation run
    /// rather than after it.
    fn chain_of(
        f: &mut Field,
        card_ty: u32,
        eff_ty: u16,
        property: u32,
        targets: Vec<CardId>,
    ) -> (CardId, EffectId) {
        let holder = spelltrap(f, 1, 0, card_ty);
        let e = api::create_effect(f, holder);
        api::set_type(f, e, eff_ty);
        api::set_property(f, e, property, 0);
        api::register_effect(f, holder, e, false);
        f.cards[holder].relate_effect_insert_for_test(e);

        let mut ch = Chain::new(e, Event::new(0));
        ch.triggering_player = 1;
        ch.chain_count = 1;
        ch.chain_id = 41;
        ch.target_cards = targets;
        f.core.current_chain.push(ch);

        let decoy_holder = spelltrap(f, 1, 1, card_type::SPELL);
        let decoy = api::create_effect(f, decoy_holder);
        api::set_type(f, decoy, effect_type::ACTIVATE);
        api::set_property(f, decoy, flag::CARD_TARGET, 0);
        api::register_effect(f, decoy_holder, decoy, false);
        let mut top = Chain::new(decoy, Event::new(0));
        top.triggering_player = 1;
        top.chain_count = 2;
        top.chain_id = 42;
        // Targets that would give the wrong answer if the decoy were read.
        top.target_cards = Vec::new();
        f.core.current_chain.push(top);
        (holder, e)
    }

    /// Run `discon` against the chaining of `re`, announced as link one.
    fn asks(f: &mut Field, rs: CardId, holder: CardId, re: EffectId) -> bool {
        let e = quick_effect(f, rs);
        let mut ev = Event::new(code::CHAINING);
        ev.trigger_card = Some(holder);
        ev.event_cards = vec![holder];
        ev.reason_effect = Some(re);
        ev.event_value = 1;
        ev.event_player = 1;
        ev.reason_player = 1;
        let ctx = Ctx {
            reason_effect: e,
            player: 0,
            event: &ev,
            card: None,
            args: &[],
        };
        discon(f, &ctx)
    }

    /// Would Ryu Senshi offer to negate an activation on a card of this
    /// printed type?
    fn offers_against(card_ty: u32) -> bool {
        let (mut f, rs) = board(0);
        let holder = spelltrap(&mut f, 1, 0, card_ty);
        let e = api::create_effect(&mut f, holder);
        api::set_type(&mut f, e, effect_type::ACTIVATE);
        api::register_effect(&mut f, holder, e, false);
        asks(&mut f, rs, holder, e)
    }

    /// Run `s.disop2` against link one, and report whether that link was
    /// negated.
    fn solve(f: &mut Field, rs: CardId) -> bool {
        let e = f.cards[rs].field_effect.equal_range(code::CHAIN_SOLVING)[0];
        let mut ev = Event::new(code::CHAIN_SOLVING);
        ev.event_value = 1;
        let ctx = Ctx {
            reason_effect: e,
            player: 0,
            event: &ev,
            card: None,
            args: &[],
        };
        f.core.reason_effect = Some(e);
        disop2(f, &ctx);
        drive(f);
        f.core.reason_effect = None;
        f.core.current_chain[0].flag & crate::chain::chain_flag::DISABLE_EFFECT != 0
    }

    mod registration {
        use super::*;

        /// **Five registered effects**, four of them doing work.
        #[test]
        fn five_effects_and_what_each_answers() {
            let (f, rs) = board(0);
            let quick = f.cards[rs].field_effect.equal_range(code::CHAINING);
            assert_eq!(quick.len(), 1);
            let x = f.effects.get(quick[0]).expect("the negation");
            assert!(x.is_type(effect_type::QUICK_O));
            assert_eq!(x.range, MZONE);
            assert_eq!(x.category, category::DISABLE);
            assert!(x.condition.is_some(), "the condition is attached");
            assert!(x.cost.is_some(), "and so is the cost");
            assert!(x.target.is_some() && x.operation.is_some());

            let solving = f.cards[rs].field_effect.equal_range(code::CHAIN_SOLVING);
            assert_eq!(solving.len(), 1);
            assert!(f
                .effects
                .get(solving[0])
                .expect("the solver")
                .is_type(effect_type::CONTINUOUS));

            // The two that share `s.distg2`, over the Spell rows.
            for wanted in [code::DISABLE, code::SELF_DESTROY] {
                let got = f.cards[rs].field_effect.equal_range(wanted);
                assert_eq!(got.len(), 1, "{wanted}");
                let y = f.effects.get(got[0]).expect("registered");
                assert!(y.is_type(effect_type::FIELD));
                assert_eq!(y.range, MZONE, "it has to be on the field");
                assert_eq!((y.s_range, y.o_range), (SZONE, SZONE), "both Spell rows");
                assert!(y.target_filter.is_some(), "and the filter is attached");
            }

            assert_eq!(
                f.cards[rs]
                    .single_effect
                    .equal_range(api::CARD_DOUBLE_SNARE)
                    .len(),
                1,
                "the Double Snare marker"
            );
            assert_eq!(api::fusion_materials(&f, rs), &MATERIALS);
            for wanted in [code::UNSUMMONABLE_CARD, code::REVIVE_LIMIT] {
                assert_eq!(f.cards[rs].single_effect.equal_range(wanted).len(), 1);
            }
        }

        /// **The printed line, transcribed from the oracle's table.**
        #[test]
        fn its_printed_line_matches_the_oracles_table() {
            let d = crate::cards::card_data(CODE).expect("printed data");
            assert!(d.is_type(card_type::FUSION) && d.is_type(card_type::MONSTER));
            assert_eq!((d.level, d.attack, d.defense), (6, 2000, 1200));
            assert_eq!(d.attribute, attribute::EARTH);
            assert_eq!(d.race, race::WARRIOR);
        }
    }

    mod the_condition {
        use super::*;

        /// **A Normal Trap is answerable, a Spell is not** — the mirror of
        /// Dark Balter.
        #[test]
        fn a_normal_trap_is_answerable_and_a_spell_is_not() {
            assert!(offers_against(card_type::TRAP));
            assert!(!offers_against(card_type::SPELL));
        }

        /// **And a Trap carrying a second type bit is not.**
        /// `re:GetActiveType()==TYPE_TRAP` is an equality over the whole
        /// printed word, so a Continuous or Counter Trap is out. Every
        /// Trap in this pool is a Normal Trap, so the distinction costs
        /// nothing here — it is transcribed because it is what the script
        /// says, and a mask would be wrong the moment one arrived.
        #[test]
        fn a_trap_with_a_second_type_bit_is_not() {
            for extra in [card_type::CONTINUOUS, card_type::COUNTER] {
                assert!(
                    !offers_against(card_type::TRAP | extra),
                    "a Trap with {extra:#x} is not TYPE_TRAP"
                );
            }
        }

        /// It has to be an *activation*.
        #[test]
        fn a_traps_non_activation_effect_is_not_answerable() {
            let (mut f, rs) = board(0);
            let holder = spelltrap(&mut f, 1, 0, card_type::TRAP);
            let e = api::create_effect(&mut f, holder);
            api::set_type(&mut f, e, effect_type::IGNITION);
            api::register_effect(&mut f, holder, e, false);
            assert!(!asks(&mut f, rs, holder, e));
        }

        /// **A Ryu Senshi that has already lost its battle does not
        /// negate on the way out.**
        #[test]
        fn a_battle_destroyed_ryu_senshi_does_not_answer() {
            let (mut f, rs) = board(0);
            let holder = spelltrap(&mut f, 1, 0, card_type::TRAP);
            let e = api::create_effect(&mut f, holder);
            api::set_type(&mut f, e, effect_type::ACTIVATE);
            api::register_effect(&mut f, holder, e, false);
            assert!(asks(&mut f, rs, holder, e), "alive, it answers");
            f.cards[rs].set_status(status::BATTLE_DESTROYED, true);
            assert!(!asks(&mut f, rs, holder, e));
        }

        /// **`Duel.IsChainDisablable` is consulted**, though on this path
        /// it can only answer yes — the state is driven directly, exactly
        /// as it is for Dark Balter, and for the same reason.
        #[test]
        fn it_consults_whether_the_link_may_be_negated() {
            let (mut f, rs) = board(0);
            let holder = spelltrap(&mut f, 1, 0, card_type::TRAP);
            let e = api::create_effect(&mut f, holder);
            api::set_type(&mut f, e, effect_type::ACTIVATE);
            api::set_property(&mut f, e, flag::CANNOT_DISABLE, 0);
            api::register_effect(&mut f, holder, e, false);
            let mut ch = Chain::new(e, Event::new(0));
            ch.chain_count = 1;
            ch.chain_id = 41;
            f.core.current_chain.push(ch);

            assert!(asks(&mut f, rs, holder, e), "while the chain is built");
            f.core.chain_solving = true;
            assert!(!asks(&mut f, rs, holder, e), "and not once it is solving");
        }
    }

    mod the_cost {
        use super::*;

        fn pay(f: &mut Field, rs: CardId, chk: bool) -> bool {
            let e = quick_effect(f, rs);
            let ev = Event::new(code::CHAINING);
            let ctx = Ctx {
                reason_effect: e,
                player: 0,
                event: &ev,
                card: None,
                args: &[],
            };
            let answer = cost(f, &ctx, chk);
            drive(f);
            answer
        }

        /// A thousand, and it is the payer's own.
        #[test]
        fn it_pays_a_thousand_of_the_activating_players_life_points() {
            let (mut f, rs) = board(0);
            assert!(pay(&mut f, rs, false));
            assert!(pay(&mut f, rs, true));
            assert_eq!(f.players[0].lp, 7000);
            assert_eq!(f.players[1].lp, 8000);
        }

        /// Below the price it may not be activated at all.
        #[test]
        fn it_cannot_be_paid_out_of_less_than_a_thousand() {
            let (mut f, rs) = board(0);
            f.players[0].lp = 1000;
            assert!(pay(&mut f, rs, false), "exactly enough");
            f.players[0].lp = 999;
            assert!(!pay(&mut f, rs, false));
        }
    }

    mod the_quick_negation {
        use super::*;

        /// The announcement names the chaining card as a disable, and the
        /// negation names the link the event carried rather than the
        /// topmost one.
        #[test]
        fn it_announces_and_negates_the_link_it_answered() {
            let (mut f, rs) = board(0);
            let (holder, _re) = chain_of(
                &mut f,
                card_type::TRAP,
                effect_type::ACTIVATE,
                0,
                Vec::new(),
            );
            let e = quick_effect(&f, rs);
            let mut ev = Event::new(code::CHAINING);
            ev.trigger_card = Some(holder);
            ev.event_cards = vec![holder];
            ev.event_value = 1;
            let ctx = Ctx {
                reason_effect: e,
                player: 0,
                event: &ev,
                card: None,
                args: &[],
            };
            assert!(matches!(distg(&mut f, &ctx, false, None), Yield::Done(1)));
            distg(&mut f, &ctx, true, None);
            // The announcement lands on the current link, which the decoy
            // in `chain_of` made link two.
            let info = f.core.current_chain[1]
                .opinfos
                .get(&category::DISABLE)
                .expect("announced");
            assert_eq!(info.cards.as_deref(), Some(&[holder][..]));
            assert_eq!(info.count, 1);

            disop(&mut f, &ctx);
            let disabled = crate::chain::chain_flag::DISABLE_EFFECT;
            assert_ne!(f.core.current_chain[0].flag & disabled, 0, "link one");
            assert_eq!(f.core.current_chain[1].flag & disabled, 0, "not link two");
        }
    }

    mod the_resolving_spell {
        use super::*;

        fn gone(f: &Field, c: CardId) -> bool {
            f.cards[c].current.location != location::SZONE
        }

        /// **A targeting Spell aimed at it is negated *and destroyed*.**
        /// The destruction is what separates this half from Fiend Skull
        /// Dragon's, which only negates.
        #[test]
        fn a_targeting_spell_aimed_at_it_is_negated_and_destroyed() {
            let (mut f, rs) = board(0);
            let (holder, _re) = chain_of(
                &mut f,
                card_type::SPELL,
                effect_type::ACTIVATE,
                flag::CARD_TARGET,
                vec![rs],
            );
            assert!(solve(&mut f, rs), "negated");
            assert!(gone(&f, holder), "and destroyed");
        }

        /// **Negated but not destroyed** when its card is no longer
        /// related to the effect — the short-circuit in
        /// `Duel.NegateEffect(ev) and ...IsRelateToEffect(re)`.
        #[test]
        fn an_unrelated_spells_card_is_negated_but_survives() {
            let (mut f, rs) = board(0);
            let (holder, _re) = chain_of(
                &mut f,
                card_type::SPELL,
                effect_type::ACTIVATE,
                flag::CARD_TARGET,
                vec![rs],
            );
            f.cards[holder].clear_relate_effect();
            assert!(solve(&mut f, rs), "still negated");
            assert!(!gone(&f, holder), "but not destroyed");
        }

        /// **The destruction is `REASON_EFFECT`.** A card destroyed by
        /// battle and one destroyed by an effect are read apart by every
        /// trigger in the pool, so the reason is part of the behaviour
        /// rather than bookkeeping.
        #[test]
        fn the_destruction_is_by_effect() {
            let (mut f, rs) = board(0);
            let (holder, _re) = chain_of(
                &mut f,
                card_type::SPELL,
                effect_type::ACTIVATE,
                flag::CARD_TARGET,
                vec![rs],
            );
            assert!(solve(&mut f, rs));
            assert_ne!(f.cards[holder].reason & reason::EFFECT, 0, "by an effect");
            assert_eq!(f.cards[holder].reason & reason::BATTLE, 0, "not by battle");
            assert_ne!(f.cards[holder].reason & reason::DESTROY, 0, "destroyed");
        }

        /// **A link that cannot be negated is not destroyed either.**
        ///
        /// `Duel.NegateEffect(ev) and ...` short-circuits on the
        /// *negation's* answer, not on whether it was attempted, and
        /// `field::disable_chain` refuses a link flagged
        /// `EFFECT_FLAG_CANNOT_DISABLE`. A port that ran the destroy
        /// unconditionally would blow up a Spell it had just failed to
        /// stop.
        #[test]
        fn a_link_that_refuses_negation_is_not_destroyed() {
            let (mut f, rs) = board(0);
            let (holder, re) = chain_of(
                &mut f,
                card_type::SPELL,
                effect_type::ACTIVATE,
                flag::CARD_TARGET,
                vec![rs],
            );
            api::set_property(&mut f, re, flag::CANNOT_DISABLE, 0);
            assert!(!solve(&mut f, rs), "the negation is refused");
            assert!(!gone(&f, holder), "so the card survives");
        }

        /// And the same for a link that was **already** negated:
        /// `disable_chain` answers false the second time.
        #[test]
        fn an_already_negated_link_is_not_destroyed_again() {
            let (mut f, rs) = board(0);
            let (holder, _re) = chain_of(
                &mut f,
                card_type::SPELL,
                effect_type::ACTIVATE,
                flag::CARD_TARGET,
                vec![rs],
            );
            f.core.current_chain[0].flag |= crate::chain::chain_flag::DISABLE_EFFECT;
            solve(&mut f, rs);
            assert!(!gone(&f, holder), "negating twice destroys nothing");
        }

        /// **A Quick-Play counts**, because `IsSpellEffect` is a mask
        /// where the quick effect's condition is an equality.
        #[test]
        fn a_quickplay_spell_counts_here_where_it_would_not_in_the_condition() {
            let (mut f, rs) = board(0);
            let (holder, _re) = chain_of(
                &mut f,
                card_type::SPELL | card_type::QUICKPLAY,
                effect_type::ACTIVATE,
                flag::CARD_TARGET,
                vec![rs],
            );
            assert!(solve(&mut f, rs));
            assert!(gone(&f, holder));
        }

        /// A Trap is left alone — this half is the Spell half.
        #[test]
        fn a_targeting_trap_is_left_alone() {
            let (mut f, rs) = board(0);
            let (holder, _re) = chain_of(
                &mut f,
                card_type::TRAP,
                effect_type::ACTIVATE,
                flag::CARD_TARGET,
                vec![rs],
            );
            assert!(!solve(&mut f, rs));
            assert!(!gone(&f, holder));
        }

        /// A Spell aimed at something else is left alone.
        #[test]
        fn a_spell_aimed_elsewhere_is_left_alone() {
            let (mut f, rs) = board(0);
            let other = spelltrap(&mut f, 0, 2, card_type::SPELL);
            let (holder, _re) = chain_of(
                &mut f,
                card_type::SPELL,
                effect_type::ACTIVATE,
                flag::CARD_TARGET,
                vec![other],
            );
            assert!(!solve(&mut f, rs));
            assert!(!gone(&f, holder));
        }

        /// A Spell that does not target is left alone, however aimed.
        #[test]
        fn a_spell_without_the_target_flag_is_left_alone() {
            let (mut f, rs) = board(0);
            let (holder, _re) =
                chain_of(&mut f, card_type::SPELL, effect_type::ACTIVATE, 0, vec![rs]);
            assert!(!solve(&mut f, rs));
            assert!(!gone(&f, holder));
        }
    }

    mod the_standing_spell {
        use super::*;

        fn filters(f: &Field, rs: CardId) -> (EffectId, EffectId) {
            (
                f.cards[rs].field_effect.equal_range(code::DISABLE)[0],
                f.cards[rs].field_effect.equal_range(code::SELF_DESTROY)[0],
            )
        }

        /// An Equip Spell pointed at Ryu Senshi, as `equip` leaves it.
        fn equip_targeting(f: &mut Field, at: CardId, ty: u32) -> CardId {
            let s = spelltrap(f, 1, 0, ty);
            f.cards[s].effect_target_cards.push(at);
            f.cards[at].effect_target_owner.push(s);
            s
        }

        /// **Both effects use the same filter, and it matches an Equip
        /// Spell aimed at this card.**
        #[test]
        fn a_spell_targeting_it_is_switched_off_and_marked() {
            let (mut f, rs) = board(0);
            let eq = equip_targeting(&mut f, rs, card_type::SPELL | card_type::EQUIP);
            let (dis, des) = filters(&f, rs);
            assert!(distg2(&f, dis, Some(eq), &[]), "the disable");
            assert!(distg2(&f, des, Some(eq), &[]), "and the self-destroy");
        }

        /// A Spell targeting something else does not match.
        #[test]
        fn a_spell_targeting_something_else_does_not_match() {
            let (mut f, rs) = board(0);
            let other = spelltrap(&mut f, 0, 3, card_type::MONSTER);
            let eq = equip_targeting(&mut f, other, card_type::SPELL | card_type::EQUIP);
            let (dis, _) = filters(&f, rs);
            assert!(!distg2(&f, dis, Some(eq), &[]));
        }

        /// **A Trap targeting it does not match** — `c:IsSpell()`.
        #[test]
        fn a_trap_targeting_it_does_not_match() {
            let (mut f, rs) = board(0);
            let tr = equip_targeting(&mut f, rs, card_type::TRAP | card_type::CONTINUOUS);
            let (dis, _) = filters(&f, rs);
            assert!(!distg2(&f, dis, Some(tr), &[]));
        }

        /// A Spell that targets nothing does not match, and neither does
        /// nothing at all.
        #[test]
        fn a_spell_with_no_targets_does_not_match() {
            let (mut f, rs) = board(0);
            let plain = spelltrap(&mut f, 1, 4, card_type::SPELL);
            let (dis, _) = filters(&f, rs);
            assert!(!distg2(&f, dis, Some(plain), &[]));
            assert!(!distg2(&f, dis, None, &[]));
        }

        /// **And the effect actually applies**, not just the filter: a
        /// Spell equipped to Ryu Senshi is disabled and lands in the
        /// self-destroy sweep.
        #[test]
        fn the_field_effects_reach_the_spell_through_the_engine() {
            let (mut f, rs) = board(0);
            let eq = equip_targeting(&mut f, rs, card_type::SPELL | card_type::EQUIP);
            assert!(
                f.is_affected_by_effect(eq, code::DISABLE).is_some(),
                "switched off"
            );
            assert!(
                f.is_affected_by_effect(eq, code::SELF_DESTROY).is_some(),
                "and marked for destruction"
            );
            let plain = spelltrap(&mut f, 1, 5, card_type::SPELL);
            assert!(
                f.is_affected_by_effect(plain, code::DISABLE).is_none(),
                "a Spell aimed at nothing is untouched"
            );
        }
    }
}

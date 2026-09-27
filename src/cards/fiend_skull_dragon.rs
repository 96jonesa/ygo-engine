//! Fiend Skull Dragon (`66235877`) — `cardscripts/c66235877.lua`.
//!
//! ```text
//! "Summoned Skull" + "Red-Eyes B. Dragon"
//! Negate the effects of Flip monsters.
//! ```
//!
//! Four registered effects for one printed line, and only two of them are
//! about Flip monsters at all.
//!
//! ## Negating a Flip monster takes two effects, not one
//!
//! `e1` is `EFFECT_DISABLE` over both Monster Zones, filtered to
//! `TYPE_FLIP` — that stops a Flip monster's *continuous* effects and
//! marks it disabled. `e2` is a continuous field effect on
//! `EVENT_CHAIN_SOLVING` that negates a resolving link whose **active
//! type** is Flip.
//!
//! The second is not redundant. A flip effect that has already been put
//! on the chain is not stopped by disabling its card; it has to be
//! negated as it resolves. Porting only `e1` would leave every flip
//! effect that made it onto a chain resolving normally.
//!
//! ## The second half of `s.disop` is about *this card*
//!
//! ```lua
//! if re:IsTrapEffect() and re:IsHasProperty(EFFECT_FLAG_CARD_TARGET) then
//!     local g=Duel.GetChainInfo(ev,CHAININFO_TARGET_CARDS)
//!     if g and g:IsContains(e:GetHandler()) then Duel.NegateEffect(ev) end
//! end
//! ```
//!
//! Nothing in the card text says this, and it is not about Flip monsters:
//! a targeting Trap aimed at Fiend Skull Dragon is negated. That is the
//! Summoned Skull half of its lineage showing through, and it is
//! transcribed because the script has it — a reading of the printed line
//! alone would drop it.
//!
//! ## `Effect.IsActiveType`, not the card's type
//!
//! `re:IsActiveType(TYPE_FLIP)` asks what the **effect** is acting as. An
//! effect a Flip monster granted to something else still answers Flip,
//! and a Flip monster's non-flip effect does not. Asking the handler card
//! its type would get both wrong.
//!
//! ## The third effect is a marker
//!
//! `e3` has this card's own code and no behaviour: Summoner of Illusions
//! reads it to know what it summoned. That card is not in this pool, so
//! nothing reads it — registered anyway, like the Double Snare marker
//! beside it.

use crate::board::location;
use crate::card::card_type;
use crate::effect::{effect_type, flag, Ctx, Yield};
use crate::event::{code, CardId};
use crate::field::Field;
use crate::script_api as api;

pub const CODE: u32 = 66235877;

/// `Fusion.AddProcMix(c,false,false,93220472,16475472)` — Summoned Skull
/// and Red-Eyes B. Dragon, each pinned against the script by
/// `tools/check_constants.py`.
const MATERIAL_A: u32 = 93_220_472;
const MATERIAL_B: u32 = 16_475_472;
const MATERIALS: [u32; 2] = [MATERIAL_A, MATERIAL_B];

const MZONE: u16 = location::MZONE as u16;

pub fn initial_effect(f: &mut Field, c: CardId) {
    api::enable_revive_limit(f, c);
    super::proc_fusion::add_proc_mix(f, c, false, false, &MATERIALS);
    // Negate FLIP monster
    let e1 = api::create_effect(f, c);
    api::set_type(f, e1, effect_type::FIELD);
    api::set_code(f, e1, code::DISABLE);
    api::set_range(f, e1, MZONE);
    api::set_target_range(f, e1, MZONE, MZONE);
    api::set_target_filter(f, e1, target_is_flip);
    api::register_effect(f, c, e1, false);
    // Negate flip effects
    let e2 = api::create_effect(f, c);
    api::set_type(f, e2, effect_type::FIELD | effect_type::CONTINUOUS);
    api::set_code(f, e2, code::CHAIN_SOLVING);
    api::set_range(f, e2, MZONE);
    api::set_operation(f, e2, disop);
    api::register_effect(f, c, e2, false);
    // Summoner of Illusions interaction — a marker with no behaviour
    let e3 = api::create_effect(f, c);
    api::set_type(f, e3, effect_type::SINGLE);
    api::set_code(f, e3, CODE);
    api::set_range(f, e3, MZONE);
    api::register_effect(f, c, e3, false);
    api::double_snare_validity(f, c, MZONE, 0);
}

/// `aux.TargetBoolFunction(Card.IsType, TYPE_FLIP)` — the **effective**
/// type, as `Card.IsType` is.
///
/// The seam hands out `&Field` and the effective-type reader needs it
/// mutably, so this reads the printed line — the same approximation
/// `target_is_trap` makes, licensed by the same `ABSENT_FROM_POOL` scan:
/// no pool card has an `EFFECT_ADD_TYPE`, `_REMOVE_TYPE` or
/// `_CHANGE_TYPE`, so the two answers agree here.
fn target_is_flip(
    f: &Field,
    _e: crate::event::EffectId,
    target: Option<CardId>,
    _a: &[i64],
) -> bool {
    target.is_some_and(|c| api::is_type_readonly(f, c, card_type::FLIP))
}

/// `s.disop` — two negations, and only the first is about Flip monsters.
fn disop(f: &mut Field, ctx: &Ctx) -> Yield {
    // `ev` is the chain count of the link that is solving.
    let ev = ctx.event.event_value as u8;
    let Some(re) = api::get_chain_triggering_effect(f, ev) else {
        return api::done();
    };
    if api::is_active_type(f, re, card_type::FLIP) {
        api::negate_effect(f, ev);
    }
    if api::is_trap_effect(f, re) && api::is_has_property(f, re, flag::CARD_TARGET, 0) {
        let targets = api::get_chain_target_cards(f, ev);
        if api::get_handler(f, ctx.reason_effect).is_some_and(|h| targets.contains(&h)) {
            api::negate_effect(f, ev);
        }
    }
    api::done()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::position;
    use crate::card::{status, Card, CardData};
    use crate::chain::Chain;
    use crate::duel::phases;
    use crate::event::{EffectId, Event};
    use crate::processor::Status;

    fn monster(f: &mut Field, player: u8, seq: u32, type_: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 5100 + seq + u32::from(player) * 50,
                type_,
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

    /// Fiend Skull Dragon in `tp`'s Monster Zone.
    fn board(tp: u8) -> (Field, CardId) {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        f.infos.turn_player = tp;
        f.infos.phase = phases::MAIN1;
        let mut d = Card::with_data(crate::cards::card_data(CODE).expect("printed data"), tp);
        d.current.controller = tp;
        d.set_status(status::EFFECT_ENABLED, true);
        let fsd = f.new_card(d);
        f.add_card(tp, fsd, location::MZONE, 0, false);
        f.cards[fsd].current.position = position::FACEUP_ATTACK;
        f.initialize_card(fsd);
        (f, fsd)
    }

    fn chain_solving_effect(f: &Field, fsd: CardId) -> EffectId {
        f.cards[fsd].field_effect.equal_range(code::CHAIN_SOLVING)[0]
    }

    /// Put `re` on the chain as **link one**, with a second link sitting
    /// on top of it, and run the continuous effect against link one.
    ///
    /// The decoy matters. With a single link, "the link that is solving"
    /// and "the current link" are the same chain link, and a port that
    /// read either would pass every test — the shape
    /// `port-mutation-blind-spots` calls *one chain link is not enough*.
    /// Here `ev` is 1 and the current link is 2, so the two come apart,
    /// and the decoy carries different targets to prove which one was
    /// read.
    fn solve(f: &mut Field, fsd: CardId, re: EffectId, targets: Vec<CardId>) -> bool {
        let mut ch = Chain::new(re, Event::new(0));
        ch.triggering_player = 0;
        ch.chain_count = 1;
        ch.chain_id = 11;
        ch.target_cards = targets;
        f.core.current_chain.push(ch);
        // The decoy on top: a different effect, and targets that would
        // give the wrong answer if it were read instead.
        let decoy_holder = monster(f, 0, 4, card_type::SPELL);
        let decoy = api::create_effect(f, decoy_holder);
        api::set_type(f, decoy, effect_type::ACTIVATE);
        api::register_effect(f, decoy_holder, decoy, false);
        let mut top = Chain::new(decoy, Event::new(0));
        top.triggering_player = 0;
        top.chain_count = 2;
        top.chain_id = 12;
        top.target_cards = vec![fsd];
        f.core.current_chain.push(top);

        let e = chain_solving_effect(f, fsd);
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
        disop(f, &ctx);
        for _ in 0..256 {
            if f.core.units.is_empty() && f.core.subunits.is_empty() {
                break;
            }
            if f.process() != Status::Continue {
                break;
            }
        }
        // `disable_chain` marks the link rather than removing it, so a
        // negation shows as the flag plus the reason it was set with.
        f.core.current_chain[0].flag & crate::chain::chain_flag::DISABLE_EFFECT != 0
    }

    /// An effect of the given type on a card of the given type, ready to
    /// be put on a chain.
    fn effect_of_type(
        f: &mut Field,
        card_ty: u32,
        eff_ty: u16,
        property: u32,
    ) -> (CardId, EffectId) {
        let holder = monster(f, 1, 1, card_ty);
        let e = api::create_effect(f, holder);
        api::set_type(f, e, eff_ty);
        api::set_property(f, e, property, 0);
        api::set_range(f, e, MZONE);
        api::register_effect(f, holder, e, false);
        (holder, e)
    }

    mod registration {
        use super::*;

        /// **Four effects for one printed line**, and two of them are
        /// markers no card in this pool reads.
        #[test]
        fn four_effects_and_the_two_markers() {
            let (f, fsd) = board(0);
            let dis = f.cards[fsd].field_effect.equal_range(code::DISABLE);
            assert_eq!(dis.len(), 1, "the disable");
            let x = f.effects.get(dis[0]).expect("the disable");
            assert!(x.is_type(effect_type::FIELD));
            assert_eq!((x.s_range, x.o_range), (MZONE, MZONE), "both rows");
            assert_eq!(x.range, MZONE, "and only while it is on the field");

            let solving = f.cards[fsd].field_effect.equal_range(code::CHAIN_SOLVING);
            assert_eq!(solving.len(), 1);
            let y = f.effects.get(solving[0]).expect("the negation");
            assert!(y.is_type(effect_type::FIELD));
            assert!(y.is_type(effect_type::CONTINUOUS));

            assert_eq!(
                f.cards[fsd].single_effect.equal_range(CODE).len(),
                1,
                "the Summoner of Illusions marker, carrying its own code"
            );
            assert_eq!(
                f.cards[fsd]
                    .single_effect
                    .equal_range(api::CARD_DOUBLE_SNARE)
                    .len(),
                1,
                "and Double Snare's"
            );
            assert_eq!(
                f.cards[fsd]
                    .single_effect
                    .equal_range(code::FUSION_MATERIAL)
                    .len(),
                1,
                "the fusion procedure"
            );
            assert_eq!(api::fusion_materials(&f, fsd), &MATERIALS);
            for wanted in [code::UNSUMMONABLE_CARD, code::REVIVE_LIMIT] {
                assert_eq!(f.cards[fsd].single_effect.equal_range(wanted).len(), 1);
            }
        }

        /// **The printed line, transcribed rather than guessed.** WIND,
        /// not DARK — the first draft of this card assumed the attribute
        /// from the artwork and the `card_data` pin caught it.
        #[test]
        fn its_printed_line_matches_the_oracles_table() {
            let d = crate::cards::card_data(CODE).expect("printed data");
            assert!(d.is_type(card_type::FUSION) && d.is_type(card_type::MONSTER));
            assert_eq!((d.level, d.attack, d.defense), (5, 2000, 1200));
            assert_eq!(d.attribute, crate::card::attribute::WIND);
            assert_eq!(d.race, crate::card::race::DRAGON);
        }

        /// The disable applies to Flip monsters and nothing else.
        #[test]
        fn the_disable_is_filtered_to_flip_monsters() {
            let (mut f, fsd) = board(0);
            let flip = monster(&mut f, 1, 1, card_type::MONSTER | card_type::FLIP);
            let plain = monster(&mut f, 1, 2, card_type::MONSTER | card_type::EFFECT);
            let dis = f.cards[fsd].field_effect.equal_range(code::DISABLE)[0];
            assert!(target_is_flip(&f, dis, Some(flip), &[]));
            assert!(!target_is_flip(&f, dis, Some(plain), &[]));
            assert!(!target_is_flip(&f, dis, None, &[]), "and nothing is not");
            // **And the filter is attached**: a disable with no target is
            // unconditional, and would stop every monster on the field.
            assert!(
                f.effects
                    .get(dis)
                    .expect("the disable")
                    .target_filter
                    .is_some(),
                "the disable is filtered"
            );
        }
    }

    mod the_negation {
        use super::*;

        /// **A resolving Flip effect is negated.**
        #[test]
        fn a_flip_effect_is_negated() {
            let (mut f, fsd) = board(0);
            let (_, re) = effect_of_type(
                &mut f,
                card_type::MONSTER | card_type::FLIP,
                effect_type::SINGLE | effect_type::TRIGGER_O,
                0,
            );
            assert!(solve(&mut f, fsd, re, Vec::new()));
        }

        /// And an ordinary monster's is not.
        #[test]
        fn an_ordinary_effect_is_left_alone() {
            let (mut f, fsd) = board(0);
            let (_, re) = effect_of_type(
                &mut f,
                card_type::MONSTER | card_type::EFFECT,
                effect_type::SINGLE | effect_type::TRIGGER_O,
                0,
            );
            assert!(!solve(&mut f, fsd, re, Vec::new()));
        }

        /// **A targeting Trap aimed at this card is negated** — the half
        /// of the script that is not about Flip monsters at all, and that
        /// a reading of the printed line would have dropped.
        #[test]
        fn a_targeting_trap_aimed_at_it_is_negated() {
            let (mut f, fsd) = board(0);
            let (_, re) = effect_of_type(
                &mut f,
                card_type::TRAP,
                effect_type::ACTIVATE,
                flag::CARD_TARGET,
            );
            assert!(solve(&mut f, fsd, re, vec![fsd]));
        }

        /// **Aimed at something else, it is not.**
        #[test]
        fn a_targeting_trap_aimed_elsewhere_is_left_alone() {
            let (mut f, fsd) = board(0);
            let other = monster(&mut f, 0, 2, card_type::MONSTER);
            let (_, re) = effect_of_type(
                &mut f,
                card_type::TRAP,
                effect_type::ACTIVATE,
                flag::CARD_TARGET,
            );
            assert!(!solve(&mut f, fsd, re, vec![other]));
        }

        /// **A Trap that does not target is left alone**, however aimed.
        #[test]
        fn a_trap_without_the_target_flag_is_left_alone() {
            let (mut f, fsd) = board(0);
            let (_, re) = effect_of_type(&mut f, card_type::TRAP, effect_type::ACTIVATE, 0);
            assert!(!solve(&mut f, fsd, re, vec![fsd]));
        }

        /// **And a targeting *Spell* is left alone** — it is the Trap
        /// half of the test that matters, not the targeting half.
        #[test]
        fn a_targeting_spell_is_left_alone() {
            let (mut f, fsd) = board(0);
            let (_, re) = effect_of_type(
                &mut f,
                card_type::SPELL,
                effect_type::ACTIVATE,
                flag::CARD_TARGET,
            );
            assert!(!solve(&mut f, fsd, re, vec![fsd]));
        }
    }
}

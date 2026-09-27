//! Dark Balter the Terrible (`80071763`) — `cardscripts/c80071763.lua`.
//!
//! Two effects: a quick negation paid for in Life Points, and a
//! continuous one that silences whatever this card kills in battle.
//!
//! ## The pool's first quick effect
//!
//! Everything in this pool so far has been an ignition effect, a trigger,
//! a continuous field effect or a Spell/Trap activation. `e1` is
//! `EFFECT_TYPE_QUICK_O` on `EVENT_CHAINING`: it is offered *while a
//! chain is being built*, in response to the link that was just added,
//! and it joins the chain above that link. The core has carried the path
//! since `quick_effect.rs` was written; this is the first card to walk
//! it.
//!
//! ## `GetActiveType()==TYPE_SPELL` is an equality, and it matters
//!
//! ```lua
//! return re:IsHasType(EFFECT_TYPE_ACTIVATE) and re:GetActiveType()==TYPE_SPELL
//!     and Duel.IsChainDisablable(ev)
//! ```
//!
//! `Card.get_type` answers with the whole printed type word, so a
//! Quick-Play Spell answers `TYPE_SPELL|TYPE_QUICKPLAY` and an Equip
//! answers `TYPE_SPELL|TYPE_EQUIP`. Against `==TYPE_SPELL` both are
//! **false**. Dark Balter negates Normal Spells and nothing else — not
//! Book of Moon, not Mystical Space Typhoon, not Scapegoat, not Enemy
//! Controller, not Premature Burial, not Snatch Steal.
//!
//! That is six of this pool's Spells on the other side of the line from
//! nine, so writing `is_active_type` — the mask test, which is what the
//! surrounding cards use — would have been wrong in a way the card text
//! ("negate the effect of a Spell Card") actively encourages. The
//! reference is the authority and the reference wrote `==`.
//!
//! ## `Duel.IsChainDisablable` is a yes here
//!
//! It consults the real predicate only while `core.chain_solving` is set
//! (`libduel.cpp:3916`), and this condition runs on `EVENT_CHAINING`,
//! before the chain solves. So the call is a constant `true` on this
//! card's path — transcribed anyway, because the port's job is the
//! reference's shape and because `solve_chain` still refuses an
//! undisablable link later.
//!
//! ## `STATUS_BATTLE_DESTROYED` guards both effects, for opposite reasons
//!
//! In `s.discon` it stops a Dark Balter that has already lost a battle
//! from negating on its way to the graveyard. In `s.disop2` it is the
//! other half of "destroyed it *and* survived": both cards are checked,
//! because two monsters can destroy each other.
//!
//! ## The silence outlives the graveyard
//!
//! `s.disop2` registers `EFFECT_DISABLE` and `EFFECT_DISABLE_EFFECT` on
//! the loser with `RESET_EVENT|RESETS_STANDARD_EXC_GRAVE` — the standard
//! reset set **minus** `RESET_LEAVE` and `RESET_TOGRAVE`. A monster
//! destroyed by battle is on its way to the graveyard by definition, so a
//! reset that fired on the way there would undo the effect before it ever
//! applied. Excluding those two is the whole point: the disable follows
//! the card into the graveyard.
//!
//! Both effects are created on **this** card (`Effect.CreateEffect(c)`,
//! where `c` is the handler) and registered on the **other** one. That
//! split is what makes `is_affect_by_effect` the loser's question to
//! answer, and it is why `register_effect` rather than
//! `register_unchecked` is the right seam.

use crate::board::location;
use crate::card::{card_type, status};
use crate::effect::{effect_type, Ctx, Yield};
use crate::event::{category, code, CardId};
use crate::field::{reset, resets, Field};
use crate::script_api as api;

pub const CODE: u32 = 80071763;

/// `Fusion.AddProcMix(c,false,false,52860176,38742075)`. Neither material
/// has a script — they are vanilla monsters, and nothing in this pool
/// names them.
///
/// The two are free constants rather than array entries so that
/// `tools/check_constants.py` can pin each against the literal in
/// `s.initial_effect`: a mistyped card number is invisible otherwise,
/// since a material list nothing reads stays self-consistent whatever it
/// holds.
const MATERIAL_A: u32 = 52_860_176;
const MATERIAL_B: u32 = 38_742_075;
const MATERIALS: [u32; 2] = [MATERIAL_A, MATERIAL_B];

/// `Cost.PayLP(1000)`, pinned against the script.
const COST_LP: u32 = 1000;

const MZONE: u16 = location::MZONE as u16;

pub fn initial_effect(f: &mut Field, c: CardId) {
    api::enable_revive_limit(f, c);
    super::proc_fusion::add_proc_mix(f, c, false, false, &MATERIALS);
    // Negate a Spell's activation
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
    // Silence what it kills in battle
    let e2 = api::create_effect(f, c);
    api::set_type(f, e2, effect_type::SINGLE | effect_type::CONTINUOUS);
    api::set_code(f, e2, code::BATTLED);
    api::set_range(f, e2, MZONE);
    api::set_operation(f, e2, disop2);
    api::register_effect(f, c, e2, false);
}

/// `s.discon` — a Normal Spell, being activated, while this card is still
/// alive.
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
        && api::get_active_type(f, re) == card_type::SPELL
        && api::is_chain_disablable(f, ctx.event.event_value as u8)
}

/// `Cost.PayLP(1000)` — the library's helper, spelled out.
///
/// `e:GetChainData().cost_lp_paid=lp_value` is the one line dropped: no
/// script in this pool reads `cost_lp_paid`, which
/// `tools/check_constants.py` pins as `ABSENT_FROM_POOL`.
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
    // `eg` — the card whose activation is being answered.
    let eg = api::event_cards(ctx).to_vec();
    api::set_operation_info(f, 0, category::DISABLE, Some(eg), 1, 0, 0);
    api::yes(true)
}

/// `s.disop` — `Duel.NegateEffect(ev)`, where `ev` is the chain count the
/// `EVENT_CHAINING` event carried.
fn disop(f: &mut Field, ctx: &Ctx) -> Yield {
    api::negate_effect(f, ctx.event.event_value as u8);
    api::done()
}

/// `s.disop2` — it destroyed something in battle and survived.
fn disop2(f: &mut Field, ctx: &Ctx) -> Yield {
    let Some(c) = api::get_handler(f, ctx.reason_effect) else {
        return api::done();
    };
    let Some(bc) = api::get_battle_target(f, c) else {
        return api::done();
    };
    if api::is_status(f, bc, status::BATTLE_DESTROYED)
        && !api::is_status(f, c, status::BATTLE_DESTROYED)
    {
        for code_ in [code::DISABLE, code::DISABLE_EFFECT] {
            let e = api::create_effect(f, c);
            api::set_type(f, e, effect_type::SINGLE);
            api::set_code(f, e, code_);
            api::set_reset(f, e, reset::EVENT | resets::STANDARD_EXC_GRAVE, 1);
            api::register_effect(f, bc, e, false);
        }
    }
    api::done()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::position;
    use crate::card::{attribute, race, Card, CardData};
    use crate::chain::Chain;
    use crate::duel::phases;
    use crate::event::{EffectId, Event};

    /// Dark Balter in `tp`'s Monster Zone, initialised.
    fn board(tp: u8) -> (Field, CardId) {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        f.infos.turn_player = tp;
        f.infos.phase = phases::MAIN1;
        let mut d = Card::with_data(crate::cards::card_data(CODE).expect("printed data"), tp);
        d.current.controller = tp;
        d.set_status(status::EFFECT_ENABLED, true);
        let db = f.new_card(d);
        f.add_card(tp, db, location::MZONE, 0, false);
        f.cards[db].current.position = position::FACEUP_ATTACK;
        f.initialize_card(db);
        (f, db)
    }

    fn quick_effect(f: &Field, db: CardId) -> EffectId {
        f.cards[db].field_effect.equal_range(code::CHAINING)[0]
    }

    fn battled_effect(f: &Field, db: CardId) -> EffectId {
        f.cards[db].single_effect.equal_range(code::BATTLED)[0]
    }

    /// A plain monster on the field.
    fn monster(f: &mut Field, player: u8, seq: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 7100 + seq + u32::from(player) * 50,
                type_: card_type::MONSTER | card_type::EFFECT,
                level: 4,
                attack: 1500,
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

    /// A Spell of the given printed type in the opponent's Spell row,
    /// with an `EFFECT_TYPE_ACTIVATE` effect on it, put on the chain as
    /// link one — and the `EVENT_CHAINING` event that announced it.
    ///
    /// `card_ty` is what makes or breaks the condition: the script asks
    /// `re:GetActiveType()==TYPE_SPELL`, and `get_active_type` for an
    /// activation hands back the **handler's whole printed type word**.
    fn chaining(f: &mut Field, card_ty: u32, eff_ty: u16) -> (CardId, EffectId) {
        let mut c = Card::with_data(
            CardData {
                code: 7300,
                type_: card_ty,
                ..Default::default()
            },
            1,
        );
        c.current.controller = 1;
        let holder = f.new_card(c);
        f.add_card(1, holder, location::SZONE, 0, false);
        f.cards[holder].current.position = position::FACEUP;
        let e = api::create_effect(f, holder);
        api::set_type(f, e, eff_ty);
        api::register_effect(f, holder, e, false);

        let mut ch = Chain::new(e, Event::new(0));
        ch.triggering_player = 1;
        ch.chain_count = 1;
        ch.chain_id = 21;
        f.core.current_chain.push(ch);
        (holder, e)
    }

    /// Run `discon` against the chaining of `re`, announced as link
    /// `count`.
    fn asks(f: &mut Field, db: CardId, holder: CardId, re: EffectId, count: u32) -> bool {
        let e = quick_effect(f, db);
        let mut ev = Event::new(code::CHAINING);
        ev.trigger_card = Some(holder);
        ev.event_cards = vec![holder];
        ev.reason_effect = Some(re);
        ev.event_value = count;
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

    /// The whole question, for one printed Spell type: would Dark Balter
    /// offer to negate it?
    fn offers_against(card_ty: u32) -> bool {
        let (mut f, db) = board(0);
        let (holder, re) = chaining(&mut f, card_ty, effect_type::ACTIVATE);
        asks(&mut f, db, holder, re, 1)
    }

    mod registration {
        use super::*;

        /// Two effects, and neither of them is a marker — this card has
        /// none of the ones its Fusion neighbours carry.
        #[test]
        fn two_effects_and_no_markers() {
            let (f, db) = board(0);
            let quick = f.cards[db].field_effect.equal_range(code::CHAINING);
            assert_eq!(quick.len(), 1, "the negation");
            let x = f.effects.get(quick[0]).expect("the negation");
            assert!(x.is_type(effect_type::QUICK_O), "a quick effect");
            assert!(x.is_type(effect_type::FIELD), "which SetType folds in");
            assert_eq!(x.range, MZONE, "only from the Monster Zone");
            assert_eq!(x.category, category::DISABLE);
            assert!(x.condition.is_some() && x.cost.is_some());
            assert!(x.target.is_some() && x.operation.is_some());

            let battled = f.cards[db].single_effect.equal_range(code::BATTLED);
            assert_eq!(battled.len(), 1, "the silencer");
            let y = f.effects.get(battled[0]).expect("the silencer");
            assert!(y.is_type(effect_type::SINGLE));
            assert!(y.is_type(effect_type::CONTINUOUS));
            assert!(
                y.is_type(effect_type::ACTIONS),
                "and CONTINUOUS folds ACTIONS in, which is what \
                 process_single_event insists on"
            );

            assert_eq!(
                f.cards[db]
                    .single_effect
                    .equal_range(code::FUSION_MATERIAL)
                    .len(),
                1,
                "the fusion procedure"
            );
            assert_eq!(api::fusion_materials(&f, db), &MATERIALS);
            for wanted in [code::UNSUMMONABLE_CARD, code::REVIVE_LIMIT] {
                assert_eq!(f.cards[db].single_effect.equal_range(wanted).len(), 1);
            }
            assert!(
                f.cards[db].single_effect.equal_range(CODE).is_empty(),
                "no Summoner of Illusions marker: this script has none"
            );
            assert!(
                f.cards[db]
                    .single_effect
                    .equal_range(api::CARD_DOUBLE_SNARE)
                    .is_empty(),
                "and no Double Snare marker either"
            );
        }

        /// **The printed line, transcribed from the oracle's table.**
        #[test]
        fn its_printed_line_matches_the_oracles_table() {
            let d = crate::cards::card_data(CODE).expect("printed data");
            assert!(d.is_type(card_type::FUSION) && d.is_type(card_type::MONSTER));
            assert_eq!((d.level, d.attack, d.defense), (5, 2000, 1200));
            assert_eq!(d.attribute, attribute::DARK);
            assert_eq!(d.race, race::FIEND);
        }
    }

    mod the_condition {
        use super::*;

        /// **A Normal Spell is answerable.**
        #[test]
        fn a_normal_spell_is_answerable() {
            assert!(offers_against(card_type::SPELL));
        }

        /// **And nothing else is.** `re:GetActiveType()==TYPE_SPELL` is an
        /// equality over the whole printed type word, so every Spell that
        /// carries a second bit is out — six of this pool's fifteen.
        ///
        /// A mask test would pass all four of these, which is why the
        /// distinction gets a test of its own rather than a comment.
        #[test]
        fn a_spell_with_a_second_type_bit_is_not() {
            for extra in [
                card_type::QUICKPLAY,
                card_type::CONTINUOUS,
                card_type::EQUIP,
                card_type::FIELD,
            ] {
                assert!(
                    !offers_against(card_type::SPELL | extra),
                    "a Spell with {extra:#x} is not TYPE_SPELL"
                );
            }
        }

        /// A Trap is not a Spell.
        #[test]
        fn a_trap_is_not_answerable() {
            assert!(!offers_against(card_type::TRAP));
        }

        /// **It has to be an *activation*.** A Spell's ignition effect —
        /// an effect on an already-face-up Continuous Spell, say — chains
        /// without `EFFECT_TYPE_ACTIVATE`, and is not answerable.
        #[test]
        fn a_spells_non_activation_effect_is_not_answerable() {
            let (mut f, db) = board(0);
            let (holder, re) = chaining(&mut f, card_type::SPELL, effect_type::IGNITION);
            assert!(!asks(&mut f, db, holder, re, 1));
        }

        /// **`Duel.IsChainDisablable` is consulted, even though it can
        /// only answer yes here.**
        ///
        /// On `EVENT_CHAINING` the chain is being built, not solved, so
        /// the export returns `true` whatever the link says
        /// (`libduel.cpp:3916`) — dropping the call from `s.discon`
        /// changes nothing the harness can see, and the mutant that drops
        /// it survives every other test in this file.
        ///
        /// So the state is driven directly: `chain_solving` set, and a
        /// link that may not be negated. It is a state ocgcore does not
        /// reach on this card's path — a chain is not added to while one
        /// is resolving — and the assertion is about the transcription
        /// rather than about a duel: `s.discon` asks the question, and a
        /// port that skipped it would answer yes where the script answers
        /// no.
        #[test]
        fn it_consults_whether_the_link_may_be_negated() {
            let (mut f, db) = board(0);
            let (holder, re) = chaining(&mut f, card_type::SPELL, effect_type::ACTIVATE);
            assert!(asks(&mut f, db, holder, re, 1), "an ordinary link");

            api::set_property(&mut f, re, crate::effect::flag::CANNOT_DISABLE, 0);
            assert!(
                asks(&mut f, db, holder, re, 1),
                "still yes while the chain is only being built"
            );
            f.core.chain_solving = true;
            assert!(
                !asks(&mut f, db, holder, re, 1),
                "and no once the predicate is actually consulted"
            );
        }

        /// **A Dark Balter that has already lost its battle does not
        /// negate on the way out.**
        #[test]
        fn a_battle_destroyed_balter_does_not_answer() {
            let (mut f, db) = board(0);
            let (holder, re) = chaining(&mut f, card_type::SPELL, effect_type::ACTIVATE);
            assert!(asks(&mut f, db, holder, re, 1), "alive, it answers");
            f.cards[db].set_status(status::BATTLE_DESTROYED, true);
            assert!(!asks(&mut f, db, holder, re, 1));
        }
    }

    mod the_cost {
        use super::*;

        fn pay(f: &mut Field, db: CardId, chk: bool) -> bool {
            let e = quick_effect(f, db);
            let ev = Event::new(code::CHAINING);
            let ctx = Ctx {
                reason_effect: e,
                player: 0,
                event: &ev,
                card: None,
                args: &[],
            };
            let answer = cost(f, &ctx, chk);
            // `Duel.PayLPCost` queues a processor unit rather than
            // subtracting on the spot, so the payment has to be driven.
            drive(f);
            answer
        }

        fn drive(f: &mut Field) {
            for _ in 0..64 {
                if f.core.units.is_empty() && f.core.subunits.is_empty() {
                    break;
                }
                if f.process() != crate::processor::Status::Continue {
                    break;
                }
            }
        }

        /// **A thousand, and it is the payer's own.**
        #[test]
        fn it_pays_a_thousand_of_the_activating_players_life_points() {
            let (mut f, db) = board(0);
            assert!(pay(&mut f, db, false), "8000 is enough");
            assert!(pay(&mut f, db, true));
            assert_eq!(f.players[0].lp, 7000);
            assert_eq!(f.players[1].lp, 8000, "and the opponent is untouched");
        }

        /// **Below the price it may not be activated at all** — and
        /// exactly the price may, which is the edge `check_lp_cost` is
        /// built around.
        #[test]
        fn it_cannot_be_paid_out_of_less_than_a_thousand() {
            let (mut f, db) = board(0);
            f.players[0].lp = 1000;
            assert!(pay(&mut f, db, false), "exactly enough");
            f.players[0].lp = 999;
            assert!(!pay(&mut f, db, false));
        }
    }

    mod the_target_and_the_negation {
        use super::*;

        /// The announcement names the chaining card, one of it, as a
        /// disable.
        #[test]
        fn the_announcement_names_the_card_being_answered() {
            let (mut f, db) = board(0);
            let (holder, _re) = chaining(&mut f, card_type::SPELL, effect_type::ACTIVATE);
            // Dark Balter's own link, on top of the Spell's.
            let e = quick_effect(&f, db);
            let mut own = Chain::new(e, Event::new(code::CHAINING));
            own.triggering_player = 0;
            own.chain_count = 2;
            own.chain_id = 22;
            f.core.current_chain.push(own);

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
            let info = f.core.current_chain[1]
                .opinfos
                .get(&category::DISABLE)
                .expect("the disable was announced");
            assert_eq!(info.cards.as_deref(), Some(&[holder][..]));
            assert_eq!(info.count, 1);
        }

        /// **The negation names the link the event carried, not the
        /// topmost one.** Dark Balter's own link sits above the Spell's,
        /// so a port that negated "the current link" would negate itself.
        #[test]
        fn it_negates_the_link_it_was_chained_to() {
            let (mut f, db) = board(0);
            let (holder, _re) = chaining(&mut f, card_type::SPELL, effect_type::ACTIVATE);
            let e = quick_effect(&f, db);
            let mut own = Chain::new(e, Event::new(code::CHAINING));
            own.triggering_player = 0;
            own.chain_count = 2;
            own.chain_id = 22;
            f.core.current_chain.push(own);

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
            disop(&mut f, &ctx);
            let disabled = crate::chain::chain_flag::DISABLE_EFFECT;
            assert_ne!(
                f.core.current_chain[0].flag & disabled,
                0,
                "the Spell's link is negated"
            );
            assert_eq!(
                f.core.current_chain[1].flag & disabled,
                0,
                "and Dark Balter's own is not"
            );
        }
    }

    mod the_battle_silence {
        use super::*;

        /// Put Dark Balter and `other` in a battle, mark who died, and run
        /// the continuous effect.
        fn battle(f: &mut Field, db: CardId, other: CardId, db_died: bool, other_died: bool) {
            f.core.attacker = Some(db);
            f.core.attack_target = Some(other);
            f.cards[db].set_status(status::BATTLE_DESTROYED, db_died);
            f.cards[other].set_status(status::BATTLE_DESTROYED, other_died);
            let e = battled_effect(f, db);
            let mut ev = Event::new(code::BATTLED);
            ev.trigger_card = Some(db);
            let ctx = Ctx {
                reason_effect: e,
                player: 0,
                event: &ev,
                card: None,
                args: &[],
            };
            f.core.reason_effect = Some(e);
            disop2(f, &ctx);
            f.core.reason_effect = None;
        }

        fn silenced(f: &Field, c: CardId) -> bool {
            !f.cards[c]
                .single_effect
                .equal_range(code::DISABLE)
                .is_empty()
        }

        /// **Both effects, on the loser.**
        #[test]
        fn it_silences_what_it_destroyed() {
            let (mut f, db) = board(0);
            let victim = monster(&mut f, 1, 1);
            battle(&mut f, db, victim, false, true);
            assert_eq!(
                f.cards[victim]
                    .single_effect
                    .equal_range(code::DISABLE)
                    .len(),
                1,
                "EFFECT_DISABLE"
            );
            assert_eq!(
                f.cards[victim]
                    .single_effect
                    .equal_range(code::DISABLE_EFFECT)
                    .len(),
                1,
                "and EFFECT_DISABLE_EFFECT — one is not enough"
            );
            assert!(!silenced(&f, db), "and not on itself");
        }

        /// **The silence survives the trip to the graveyard.**
        ///
        /// `RESETS_STANDARD_EXC_GRAVE` is the standard set minus
        /// `RESET_LEAVE` and `RESET_TOGRAVE`. A monster destroyed by
        /// battle is on its way to the graveyard by definition, so an
        /// effect that reset on either would be gone before it applied —
        /// which is the whole point of the card.
        #[test]
        fn the_silence_is_not_reset_by_the_trip_to_the_graveyard() {
            let (mut f, db) = board(0);
            let victim = monster(&mut f, 1, 1);
            battle(&mut f, db, victim, false, true);
            for wanted in [code::DISABLE, code::DISABLE_EFFECT] {
                let id = f.cards[victim].single_effect.equal_range(wanted)[0];
                let x = f.effects.get(id).expect("registered");
                assert_ne!(x.reset_flag & reset::EVENT, 0, "an event reset");
                assert_eq!(
                    x.reset_flag & reset::TOGRAVE,
                    0,
                    "but not on the way to the graveyard"
                );
                assert_eq!(x.reset_flag & reset::LEAVE, 0, "nor on leaving the field");
                assert_eq!(x.reset_flag, reset::EVENT | resets::STANDARD_EXC_GRAVE);
                assert_eq!(x.reset_count, 1);
                assert_eq!(
                    f.effects.get(id).and_then(|x| x.owner),
                    Some(db),
                    "owned by Dark Balter, registered on the loser"
                );
            }
        }

        /// **Mutual destruction silences nothing.** Both halves of the
        /// guard are needed: "it died" and "I did not".
        #[test]
        fn mutual_destruction_silences_nothing() {
            let (mut f, db) = board(0);
            let victim = monster(&mut f, 1, 1);
            battle(&mut f, db, victim, true, true);
            assert!(!silenced(&f, victim));
        }

        /// A monster that survived the battle is not silenced.
        #[test]
        fn a_survivor_is_not_silenced() {
            let (mut f, db) = board(0);
            let survivor = monster(&mut f, 1, 1);
            battle(&mut f, db, survivor, false, false);
            assert!(!silenced(&f, survivor));
        }

        /// **And it works from the other side of the battle**, because
        /// `GetBattleTarget` reads both `core` slots: Dark Balter blocking
        /// an attack silences the attacker just the same.
        #[test]
        fn it_silences_an_attacker_it_destroyed() {
            let (mut f, db) = board(0);
            let attacker = monster(&mut f, 1, 1);
            f.core.attacker = Some(attacker);
            f.core.attack_target = Some(db);
            f.cards[attacker].set_status(status::BATTLE_DESTROYED, true);
            let e = battled_effect(&f, db);
            let mut ev = Event::new(code::BATTLED);
            ev.trigger_card = Some(db);
            let ctx = Ctx {
                reason_effect: e,
                player: 0,
                event: &ev,
                card: None,
                args: &[],
            };
            f.core.reason_effect = Some(e);
            disop2(&mut f, &ctx);
            assert!(silenced(&f, attacker));
        }

        /// A Dark Balter in no battle at all silences nothing — the nil
        /// arm of `GetBattleTarget`.
        #[test]
        fn out_of_a_battle_it_silences_nothing() {
            let (mut f, db) = board(0);
            let other = monster(&mut f, 1, 1);
            f.cards[other].set_status(status::BATTLE_DESTROYED, true);
            let e = battled_effect(&f, db);
            let mut ev = Event::new(code::BATTLED);
            ev.trigger_card = Some(db);
            let ctx = Ctx {
                reason_effect: e,
                player: 0,
                event: &ev,
                card: None,
                args: &[],
            };
            f.core.reason_effect = Some(e);
            disop2(&mut f, &ctx);
            assert!(!silenced(&f, other));
        }
    }
}

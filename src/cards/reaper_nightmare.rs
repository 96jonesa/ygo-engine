//! Reaper on the Nightmare (`85684223`) — `cardscripts/c85684223.lua`.
//!
//! Seven registered effects, the most of any card in this pool, and the
//! interesting ones are the three that spell out a single sentence of
//! card text: *destroyed when it becomes the target of an effect.*
//!
//! ## "Becomes a target" is three effects and two markers
//!
//! The obvious port — destroy it the moment `EVENT_BECOME_TARGET`
//! arrives — is wrong twice over, and the script says how:
//!
//! * `e6` on `EVENT_BECOME_TARGET` **only writes a note**: a flag effect
//!   `id+1` whose label is the chain count that targeted it, reset on
//!   `RESET_CHAIN`. Nothing is destroyed yet, because the effect that
//!   targeted it may still be negated, and a negated effect never
//!   targeted anything.
//! * `e4` on `EVENT_CHAIN_SOLVED` reads the note back. The link has now
//!   finished, this card is still among its targets, and one of the
//!   card's labels matches the link that solved — *now* it dies.
//! * `e5` on `EVENT_BATTLED` is the deferral. If the chain solved inside
//!   a damage step before the damage was worked out, `e4` writes a
//!   **second** marker (`id`, reset at the end of the damage phase)
//!   instead of destroying, and `e5` finishes the job once the battle is
//!   over. A monster that would have died mid-damage-step gets to finish
//!   its battle first.
//!
//! Two markers, distinguished only by `id` against `id+1`, and the port
//! keeps them that way rather than inventing names: the reference's
//! `RegisterFlagEffect` code is `(id & 0xfffffff) | 0x10000000`, so two
//! adjacent card numbers are two distinct markers and nothing else.
//!
//! ## `e6` is the one that cannot be disabled
//!
//! `e4` and `e5` are clones of one another; `e6` is a clone **plus**
//! `EFFECT_FLAG_CANNOT_DISABLE`. Only the note-taker is protected. The
//! two that act check `IsHasEffect(EFFECT_DISABLE)` and `IsDisabled()`
//! themselves and decline — so a disabled Reaper still records what
//! targeted it, and simply does not act on the record.
//!
//! ## `e3` is a different question about targeting
//!
//! `GetOwnerTargetCount()>0` — anything *continuously* targeting it, an
//! Equip Spell in this pool — makes it self-destroy without any chain
//! being involved. `EFFECT_SELF_DESTROY` with `EFFECT_FLAG_SINGLE_RANGE`,
//! which is how a single effect asks to be consulted only on the field.
//!
//! Ryu Senshi reads `GetCardTarget` (what a card aims *at*); this reads
//! `GetOwnerTargetCount` (what aims at it). The two live on opposite
//! fields of the same card and are a standing invitation to write the
//! wrong one.
//!
//! ## The direct attack, and the discard it pays for
//!
//! `e7` is `EFFECT_DIRECT_ATTACK`, `e1` is `EFFECT_INDESTRUCTABLE_BATTLE`
//! with value 1, and `e2` is the mandatory trigger that discards.
//!
//! `s.condition` is `ep~=tp and Duel.GetAttackTarget()==nil` — the damage
//! was dealt to the *other* player, by an attack that had no target, i.e.
//! a direct attack. The second half is what stops the discard when this
//! card runs over a monster.

use crate::board::location;
use crate::card::reason;
use crate::duel::phases;
use crate::effect::{effect_type, flag, Ctx, Yield};
use crate::event::{category, code, CardId, EffectId};
use crate::field::{reset, resets, Field};
use crate::script_api as api;

pub const CODE: u32 = 85684223;

/// `Fusion.AddProcMix(c,true,true,23205979,59290628)` — and note the two
/// `true`s, where every other Fusion Monster in this pool passes
/// `false,false`: substitutes are allowed and the Instant Fusion arm is
/// open. Both are read only by the condition this pool cannot reach.
const MATERIAL_A: u32 = 23_205_979;
const MATERIAL_B: u32 = 59_290_628;
const MATERIALS: [u32; 2] = [MATERIAL_A, MATERIAL_B];

/// The two flag-effect markers. `id` is the deferral — "die when this
/// battle is over" — and `id+1` is the note `e6` takes when something
/// targets this card.
const FLAG_DEFERRED: u32 = CODE;
const FLAG_TARGETED: u32 = CODE + 1;

const MZONE: u16 = location::MZONE as u16;

pub fn initial_effect(f: &mut Field, c: CardId) {
    api::enable_revive_limit(f, c);
    super::proc_fusion::add_proc_mix(f, c, true, true, &MATERIALS);
    // Not destroyed by battle
    let e1 = api::create_effect(f, c);
    api::set_type(f, e1, effect_type::SINGLE);
    api::set_code(f, e1, code::INDESTRUCTABLE_BATTLE);
    api::set_value(f, e1, 1);
    api::register_effect(f, c, e1, false);
    // Direct damage makes the opponent discard
    let e2 = api::create_effect(f, c);
    api::set_description(f, e2, api::stringid(CODE, 0));
    api::set_category(f, e2, category::HANDES);
    api::set_type(f, e2, effect_type::SINGLE | effect_type::TRIGGER_F);
    api::set_code(f, e2, code::BATTLE_DAMAGE);
    api::set_condition(f, e2, condition);
    api::set_target(f, e2, target);
    api::set_operation(f, e2, operation);
    api::register_effect(f, c, e2, false);
    // Anything standing there targeting it kills it
    let e3 = api::create_effect(f, c);
    api::set_type(f, e3, effect_type::SINGLE);
    api::set_property(f, e3, flag::SINGLE_RANGE, 0);
    api::set_range(f, e3, MZONE);
    api::set_code(f, e3, code::SELF_DESTROY);
    api::set_avail_condition(f, e3, sdcon);
    api::register_effect(f, c, e3, false);
    // The three that spell out "destroyed when targeted"
    let e4 = api::create_effect(f, c);
    api::set_type(f, e4, effect_type::FIELD | effect_type::CONTINUOUS);
    api::set_range(f, e4, MZONE);
    api::set_code(f, e4, code::CHAIN_SOLVED);
    api::set_operation(f, e4, desop1);
    api::register_effect(f, c, e4, false);
    // `local e5=e4:Clone()` — same type, range and property, new code and
    // operation. Spelled out rather than cloned: the port has no `Clone`
    // and writing one for three call sites would hide what differs.
    let e5 = api::create_effect(f, c);
    api::set_type(f, e5, effect_type::FIELD | effect_type::CONTINUOUS);
    api::set_range(f, e5, MZONE);
    api::set_code(f, e5, code::BATTLED);
    api::set_operation(f, e5, desop2);
    api::register_effect(f, c, e5, false);
    let e6 = api::create_effect(f, c);
    api::set_type(f, e6, effect_type::FIELD | effect_type::CONTINUOUS);
    api::set_range(f, e6, MZONE);
    // **Only the note-taker is protected.** The two that act ask about
    // their own disabling instead.
    api::set_property(f, e6, flag::CANNOT_DISABLE, 0);
    api::set_code(f, e6, code::BECOME_TARGET);
    api::set_operation(f, e6, register);
    api::register_effect(f, c, e6, false);
    // It can attack directly
    let e7 = api::create_effect(f, c);
    api::set_type(f, e7, effect_type::SINGLE);
    api::set_code(f, e7, code::DIRECT_ATTACK);
    api::register_effect(f, c, e7, false);
}

/// `s.condition` — the damage went to the other player, and the attack
/// had no target: a direct attack.
fn condition(f: &mut Field, ctx: &Ctx) -> bool {
    ctx.event.event_player != ctx.player && api::get_attack_target(f).is_none()
}

/// `s.target` — one card, from the other player's hand.
fn target(f: &mut Field, ctx: &Ctx, chk: bool, _chkc: Option<CardId>) -> Yield {
    if !chk {
        return api::yes(true);
    }
    api::set_operation_info(f, 0, category::HANDES, None, 0, 1 - ctx.player, 1);
    api::yes(true)
}

/// `s.operation` — a random discard from the damaged player's hand.
fn operation(f: &mut Field, ctx: &Ctx) -> Yield {
    // `ep`, the player the damage was dealt to, not `1 - tp`: the
    // reference reads the event's player here even though the condition
    // has already proved the two agree.
    let ep = ctx.event.event_player;
    let g = api::get_field_group(f, ep, u32::from(location::HAND), 0);
    if g.is_empty() {
        return api::done();
    }
    api::random_select_request(f, &g, 1 - ctx.player, 1);
    api::suspend(|f, _ctx| {
        let sg = api::random_selected(f);
        api::send_to_grave(f, sg, reason::DISCARD | reason::EFFECT);
        api::done()
    })
}

/// `s.sdcon(e)` — anything at all is targeting this card.
///
/// **One argument, and that is not an accident.** `effect::is_available`
/// calls a continuous effect's condition with the effect alone, where an
/// activation condition gets the eight event parameters. This card is the
/// pool's first continuous effect with a condition, and it is what found
/// that the port's `is_available` was not calling one at all.
fn sdcon(f: &Field, e: EffectId) -> bool {
    api::get_handler(f, e).is_some_and(|c| api::get_owner_target_count(f, c) > 0)
}

/// `s.register` — note which chain link targeted this card, and nothing
/// more. The link may still be negated, and a negated effect never
/// targeted anything.
fn register(f: &mut Field, ctx: &Ctx) -> Yield {
    let Some(c) = api::get_handler(f, ctx.reason_effect) else {
        return api::done();
    };
    api::register_flag_effect(
        f,
        c,
        FLAG_TARGETED,
        reset::CHAIN,
        0,
        1,
        i64::from(ctx.event.event_value),
    );
    api::done()
}

/// `s.desop1` — the link that targeted this card has finished solving.
fn desop1(f: &mut Field, ctx: &Ctx) -> Yield {
    let ev = ctx.event.event_value;
    let Some(c) = api::get_handler(f, ctx.reason_effect) else {
        return api::done();
    };
    let targets = api::get_chain_target_cards(f, ev as u8);
    if !targets.contains(&c) {
        return api::done();
    }
    // `for _,ch in ipairs({c:GetFlagEffectLabel(id+1)})` — one of the
    // notes has to name *this* link. A card targeted twice in one chain
    // carries two notes.
    if !api::get_flag_effect_label(f, c, FLAG_TARGETED)
        .iter()
        .any(|&label| label == i64::from(ev))
    {
        return api::done();
    }
    let phase = api::get_current_phase(f);
    if phase & (phases::DAMAGE | phases::DAMAGE_CAL) != 0 && !api::is_damage_calculated(f) {
        // **Deferred**: finish the battle first.
        api::register_flag_effect(
            f,
            c,
            FLAG_DEFERRED,
            reset::PHASE | u32::from(phases::DAMAGE) | resets::STANDARD,
            0,
            1,
            0,
        );
    } else if !api::is_has_effect(f, c, code::DISABLE) && !api::is_disabled(f, c) {
        api::destroy(f, vec![c], reason::EFFECT);
    }
    api::done()
}

/// `s.desop2` — the battle is over and a deferral is owed.
fn desop2(f: &mut Field, ctx: &Ctx) -> Yield {
    let Some(c) = api::get_handler(f, ctx.reason_effect) else {
        return api::done();
    };
    if api::get_flag_effect(f, c, FLAG_DEFERRED) > 0
        && !api::is_has_effect(f, c, code::DISABLE)
        && !api::is_disabled(f, c)
    {
        api::destroy(f, vec![c], reason::EFFECT);
    }
    api::done()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::position;
    use crate::card::{attribute, card_type, race, status, Card, CardData};
    use crate::chain::Chain;
    use crate::event::EffectId;
    use crate::event::Event;
    use crate::processor::Status;

    /// Reaper in `tp`'s Monster Zone, initialised.
    fn board(tp: u8) -> (Field, CardId) {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        f.infos.turn_player = tp;
        f.infos.phase = phases::MAIN1;
        let mut d = Card::with_data(crate::cards::card_data(CODE).expect("printed data"), tp);
        d.current.controller = tp;
        d.set_status(status::EFFECT_ENABLED, true);
        let r = f.new_card(d);
        f.add_card(tp, r, location::MZONE, 0, false);
        f.cards[r].current.position = position::FACEUP_ATTACK;
        f.initialize_card(r);
        (f, r)
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

    fn effect_for(f: &Field, r: CardId, code_: u32) -> EffectId {
        f.cards[r].field_effect.equal_range(code_)[0]
    }

    fn alive(f: &Field, r: CardId) -> bool {
        f.cards[r].current.location == location::MZONE
    }

    /// A plain monster on the field, for a chain to name instead.
    fn plain_monster(f: &mut Field, player: u8) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 9600 + u32::from(player),
                type_: card_type::MONSTER,
                ..Default::default()
            },
            player,
        );
        c.current.controller = player;
        let id = f.new_card(c);
        f.add_card(player, id, location::MZONE, 2, false);
        id
    }

    /// A chain link naming `targets`, at chain count one.
    fn link_targeting(f: &mut Field, targets: Vec<CardId>) -> EffectId {
        let mut c = Card::with_data(
            CardData {
                code: 9500,
                type_: card_type::SPELL,
                ..Default::default()
            },
            1,
        );
        c.current.controller = 1;
        let holder = f.new_card(c);
        f.add_card(1, holder, location::SZONE, 0, false);
        f.cards[holder].current.position = position::FACEUP;
        let e = api::create_effect(f, holder);
        api::set_type(f, e, effect_type::ACTIVATE);
        api::set_property(f, e, flag::CARD_TARGET, 0);
        api::register_effect(f, holder, e, false);
        let mut ch = Chain::new(e, Event::new(0));
        ch.triggering_player = 1;
        ch.chain_count = 1;
        ch.chain_id = 51;
        ch.target_cards = targets;
        f.core.current_chain.push(ch);
        e
    }

    /// Run one of the three continuous field effects, with `ev` as the
    /// event value.
    fn run(f: &mut Field, r: CardId, code_: u32, event_code: u32, ev: u32) {
        let e = effect_for(f, r, code_);
        let mut event = Event::new(event_code);
        event.event_value = ev;
        let ctx = Ctx {
            reason_effect: e,
            player: 0,
            event: &event,
            card: None,
            args: &[],
        };
        f.core.reason_effect = Some(e);
        match code_ {
            code::BECOME_TARGET => register(f, &ctx),
            code::CHAIN_SOLVED => desop1(f, &ctx),
            _ => desop2(f, &ctx),
        };
        drive(f);
        f.core.reason_effect = None;
    }

    mod registration {
        use super::*;

        /// **Seven effects**, and the three that spell out one sentence
        /// of card text are told apart by their codes alone.
        #[test]
        fn seven_effects_and_only_the_note_taker_is_protected() {
            let (f, r) = board(0);
            for wanted in [
                code::INDESTRUCTABLE_BATTLE,
                code::DIRECT_ATTACK,
                code::SELF_DESTROY,
            ] {
                assert_eq!(
                    f.cards[r].single_effect.equal_range(wanted).len(),
                    1,
                    "{wanted}"
                );
            }
            assert_eq!(
                f.cards[r]
                    .single_effect
                    .equal_range(code::BATTLE_DAMAGE)
                    .len(),
                1,
                "the discard trigger"
            );

            let mut protected = Vec::new();
            for wanted in [code::CHAIN_SOLVED, code::BATTLED, code::BECOME_TARGET] {
                let got = f.cards[r].field_effect.equal_range(wanted);
                assert_eq!(got.len(), 1, "{wanted}");
                let x = f.effects.get(got[0]).expect("registered");
                assert!(x.is_type(effect_type::FIELD));
                assert!(x.is_type(effect_type::CONTINUOUS));
                assert_eq!(x.range, MZONE);
                if x.is_flag(flag::CANNOT_DISABLE) {
                    protected.push(wanted);
                }
            }
            assert_eq!(
                protected,
                vec![code::BECOME_TARGET],
                "only the note-taker survives a disable; the two that act \
                 ask about their own disabling instead"
            );

            let battle = f.cards[r]
                .single_effect
                .equal_range(code::INDESTRUCTABLE_BATTLE)[0];
            assert_eq!(
                f.effects.get(battle).expect("registered").value,
                1,
                "SetValue(1)"
            );

            let sd = f.cards[r].single_effect.equal_range(code::SELF_DESTROY)[0];
            let y = f.effects.get(sd).expect("registered");
            assert!(y.is_flag(flag::SINGLE_RANGE), "asked only on the field");
            assert_eq!(y.range, MZONE);
            assert!(
                y.avail_condition.is_some(),
                "and the condition is attached — to the read-only slot, \
                 which is the one `is_available` consults"
            );
            assert!(
                y.condition.is_none(),
                "not the activation slot, which nothing would ever call \
                 for a continuous effect"
            );

            let handes = f.cards[r].single_effect.equal_range(code::BATTLE_DAMAGE)[0];
            let z = f.effects.get(handes).expect("registered");
            assert!(z.is_type(effect_type::TRIGGER_F), "mandatory");
            assert_eq!(z.category, category::HANDES);
            assert!(z.condition.is_some() && z.target.is_some() && z.operation.is_some());

            assert_eq!(api::fusion_materials(&f, r), &MATERIALS);
            for wanted in [code::UNSUMMONABLE_CARD, code::REVIVE_LIMIT] {
                assert_eq!(
                    f.cards[r].single_effect.equal_range(wanted).len(),
                    1,
                    "EnableReviveLimit"
                );
            }
        }

        /// **The printed line, transcribed from the oracle's table.** 800
        /// attack, not 1800 — the first draft of this card guessed the
        /// statline from the artwork and this table corrected it before
        /// the test was even written.
        #[test]
        fn its_printed_line_matches_the_oracles_table() {
            let d = crate::cards::card_data(CODE).expect("printed data");
            assert!(d.is_type(card_type::FUSION) && d.is_type(card_type::MONSTER));
            assert_eq!((d.level, d.attack, d.defense), (5, 800, 600));
            assert_eq!(d.attribute, attribute::DARK);
            assert_eq!(d.race, race::ZOMBIE);
        }

        /// The two markers are **adjacent card numbers**, which is all
        /// that keeps them apart.
        #[test]
        fn the_two_markers_are_different_ids() {
            assert_eq!(FLAG_DEFERRED, CODE);
            assert_eq!(FLAG_TARGETED, CODE + 1);
            assert_ne!(FLAG_DEFERRED, FLAG_TARGETED);
        }
    }

    mod becoming_a_target {
        use super::*;

        /// **`EVENT_BECOME_TARGET` writes a note and destroys nothing.**
        /// The effect that targeted it may still be negated, and a
        /// negated effect never targeted anything.
        #[test]
        fn becoming_a_target_only_takes_a_note() {
            let (mut f, r) = board(0);
            link_targeting(&mut f, vec![r]);
            run(&mut f, r, code::BECOME_TARGET, code::BECOME_TARGET, 1);
            assert!(alive(&f, r), "nothing has resolved yet");
            assert_eq!(
                api::get_flag_effect_label(&f, r, FLAG_TARGETED),
                vec![1],
                "the note names the link"
            );
        }

        /// **And the note dies with the chain**: `RESET_CHAIN`.
        #[test]
        fn the_note_resets_with_the_chain() {
            let (mut f, r) = board(0);
            link_targeting(&mut f, vec![r]);
            run(&mut f, r, code::BECOME_TARGET, code::BECOME_TARGET, 1);
            let e = f.cards[r]
                .single_effect
                .equal_range(api::flag_code(FLAG_TARGETED))[0];
            let x = f.effects.get(e).expect("the note");
            assert_eq!(x.reset_flag, reset::CHAIN);
            assert_eq!(x.reset_count, 1);
        }

        /// **The link solving is what kills it.**
        #[test]
        fn the_link_solving_destroys_it() {
            let (mut f, r) = board(0);
            link_targeting(&mut f, vec![r]);
            run(&mut f, r, code::BECOME_TARGET, code::BECOME_TARGET, 1);
            run(&mut f, r, code::CHAIN_SOLVED, code::CHAIN_SOLVED, 1);
            assert!(!alive(&f, r));
        }

        /// **A link that solved without a note leaves it alone** — which
        /// is the negation case: `EVENT_BECOME_TARGET` never fired, so
        /// nothing was written.
        #[test]
        fn a_link_that_never_targeted_it_leaves_it_alone() {
            let (mut f, r) = board(0);
            link_targeting(&mut f, vec![r]);
            run(&mut f, r, code::CHAIN_SOLVED, code::CHAIN_SOLVED, 1);
            assert!(alive(&f, r), "no note was taken");
        }

        /// **A note naming a different link leaves it alone.** The label
        /// is not decoration: a card targeted by link one and asked about
        /// link two is not destroyed.
        #[test]
        fn a_note_naming_another_link_leaves_it_alone() {
            let (mut f, r) = board(0);
            let e = link_targeting(&mut f, vec![r]);
            // A second link on top, naming **something else**. With both
            // links naming this card, a port reading link zero instead of
            // the link the event named would pass — the shape
            // `port-mutation-blind-spots` calls *one chain link is not
            // enough*, which is only avoided by making the two disagree.
            let bystander = plain_monster(&mut f, 1);
            let mut top = Chain::new(e, Event::new(0));
            top.chain_count = 2;
            top.chain_id = 52;
            top.target_cards = vec![bystander];
            f.core.current_chain.push(top);
            // The note names link one.
            run(&mut f, r, code::BECOME_TARGET, code::BECOME_TARGET, 1);
            run(&mut f, r, code::CHAIN_SOLVED, code::CHAIN_SOLVED, 2);
            assert!(
                alive(&f, r),
                "link two neither names it nor matches the note"
            );
            run(&mut f, r, code::CHAIN_SOLVED, code::CHAIN_SOLVED, 1);
            assert!(!alive(&f, r), "and link one is");
        }

        /// **The label is compared, not merely counted.** Here *both*
        /// links name this card, so the target test cannot tell them
        /// apart and only the label can: the note names link one, and
        /// being asked about link two must leave the card alone.
        ///
        /// The sibling test above makes the two links' *targets*
        /// disagree; this one makes only their labels. Neither alone
        /// proves both readers.
        #[test]
        fn a_note_naming_another_link_is_not_a_note_naming_this_one() {
            let (mut f, r) = board(0);
            let e = link_targeting(&mut f, vec![r]);
            let mut top = Chain::new(e, Event::new(0));
            top.chain_count = 2;
            top.chain_id = 53;
            top.target_cards = vec![r];
            f.core.current_chain.push(top);
            run(&mut f, r, code::BECOME_TARGET, code::BECOME_TARGET, 1);
            assert_eq!(api::get_flag_effect_label(&f, r, FLAG_TARGETED), vec![1]);
            run(&mut f, r, code::CHAIN_SOLVED, code::CHAIN_SOLVED, 2);
            assert!(alive(&f, r), "link two carries no note");
            run(&mut f, r, code::CHAIN_SOLVED, code::CHAIN_SOLVED, 1);
            assert!(!alive(&f, r), "link one does");
        }

        /// **A link that solved but no longer names it leaves it alone.**
        #[test]
        fn a_link_that_no_longer_names_it_leaves_it_alone() {
            let (mut f, r) = board(0);
            let other = plain_monster(&mut f, 0);
            link_targeting(&mut f, vec![r]);
            run(&mut f, r, code::BECOME_TARGET, code::BECOME_TARGET, 1);
            f.core.current_chain[0].target_cards = vec![other];
            run(&mut f, r, code::CHAIN_SOLVED, code::CHAIN_SOLVED, 1);
            assert!(alive(&f, r));
        }

        /// **A Reaper flipped face-down stops projecting its field
        /// effects**, so the link solving finds nothing to run.
        ///
        /// This is the second engine defect this card found: the port's
        /// `is_activateable` had no branch for continuous effects at all,
        /// where the reference's third arm refuses a `EFFECT_TYPE_FIELD`
        /// effect whose handler is face-down on the field. Book of Moon
        /// turning Reaper face-down mid-chain is exactly that, and the
        /// harness diverged on it.
        #[test]
        fn a_face_down_reaper_projects_nothing() {
            let (mut f, r) = board(0);
            link_targeting(&mut f, vec![r]);
            let e = effect_for(&f, r, code::CHAIN_SOLVED);
            let mut event = Event::new(code::CHAIN_SOLVED);
            event.event_value = 1;

            assert!(
                f.is_activateable(e, 0, &event, false, false, false, false, false),
                "face-up, the field effect is live"
            );
            f.cards[r].current.position = position::FACEDOWN_DEFENSE;
            assert!(
                !f.is_activateable(e, 0, &event, false, false, false, false, false),
                "face-down, it is not"
            );
            f.cards[r].current.position = position::FACEUP_ATTACK;
            f.cards[r].set_status(status::EFFECT_ENABLED, false);
            assert!(
                !f.is_activateable(e, 0, &event, false, false, false, false, false),
                "and neither is a card whose effects are switched off"
            );
        }

        /// **A disabled Reaper still takes the note and declines to
        /// act.** `e6` cannot be disabled; `e4` asks about itself.
        #[test]
        fn a_disabled_reaper_notes_but_does_not_die() {
            let (mut f, r) = board(0);
            link_targeting(&mut f, vec![r]);
            run(&mut f, r, code::BECOME_TARGET, code::BECOME_TARGET, 1);
            let d = api::create_effect(&mut f, r);
            api::set_type(&mut f, d, effect_type::SINGLE);
            api::set_code(&mut f, d, code::DISABLE);
            api::register_effect(&mut f, r, d, false);
            run(&mut f, r, code::CHAIN_SOLVED, code::CHAIN_SOLVED, 1);
            assert!(alive(&f, r), "disabled, it does not act");
            assert_eq!(
                api::get_flag_effect_label(&f, r, FLAG_TARGETED),
                vec![1],
                "but the note was taken anyway"
            );
        }
    }

    mod the_damage_step_deferral {
        use super::*;

        fn in_damage_step(f: &mut Field, calculated: bool) {
            f.infos.phase = phases::DAMAGE;
            f.core.damage_calculated = calculated;
        }

        /// **Mid-damage-step, it is not destroyed — it is marked.**
        #[test]
        fn a_chain_solving_before_damage_defers_instead_of_destroying() {
            let (mut f, r) = board(0);
            link_targeting(&mut f, vec![r]);
            run(&mut f, r, code::BECOME_TARGET, code::BECOME_TARGET, 1);
            in_damage_step(&mut f, false);
            run(&mut f, r, code::CHAIN_SOLVED, code::CHAIN_SOLVED, 1);
            assert!(alive(&f, r), "it finishes its battle first");
            assert_eq!(api::get_flag_effect(&f, r, FLAG_DEFERRED), 1, "marked");
            // **And the mark expires with the damage phase.** A deferral
            // that outlived it would kill the card at some later battle
            // it had nothing to do with.
            let e = f.cards[r]
                .single_effect
                .equal_range(api::flag_code(FLAG_DEFERRED))[0];
            let x = f.effects.get(e).expect("the deferral");
            assert_ne!(x.reset_flag & reset::PHASE, 0, "a phase reset");
            assert_ne!(
                x.reset_flag & u32::from(phases::DAMAGE),
                0,
                "and it is the damage phase"
            );
            assert_eq!(
                x.reset_flag & resets::STANDARD,
                resets::STANDARD,
                "plus the standard set"
            );
        }

        /// **And `EVENT_BATTLED` collects the debt.**
        #[test]
        fn the_battle_ending_collects_the_deferral() {
            let (mut f, r) = board(0);
            link_targeting(&mut f, vec![r]);
            run(&mut f, r, code::BECOME_TARGET, code::BECOME_TARGET, 1);
            in_damage_step(&mut f, false);
            run(&mut f, r, code::CHAIN_SOLVED, code::CHAIN_SOLVED, 1);
            run(&mut f, r, code::BATTLED, code::BATTLED, 0);
            assert!(!alive(&f, r));
        }

        /// **`EVENT_BATTLED` with no deferral owed does nothing** — every
        /// battle this card survives raises it.
        ///
        /// The note from `e6` is deliberately present: the two markers
        /// differ only in their id, and `s.desop2` must read the
        /// *deferral*. A port reading the targeting note would kill a
        /// card that was merely targeted earlier in the turn.
        #[test]
        fn a_battle_with_no_deferral_owed_does_nothing() {
            let (mut f, r) = board(0);
            link_targeting(&mut f, vec![r]);
            run(&mut f, r, code::BECOME_TARGET, code::BECOME_TARGET, 1);
            assert_eq!(api::get_flag_effect(&f, r, FLAG_TARGETED), 1);
            assert_eq!(api::get_flag_effect(&f, r, FLAG_DEFERRED), 0);
            run(&mut f, r, code::BATTLED, code::BATTLED, 0);
            assert!(alive(&f, r));
        }

        /// **Once the damage is calculated, the deferral is not taken** —
        /// it dies on the spot, still inside the damage step.
        #[test]
        fn after_the_damage_is_calculated_it_dies_immediately() {
            let (mut f, r) = board(0);
            link_targeting(&mut f, vec![r]);
            run(&mut f, r, code::BECOME_TARGET, code::BECOME_TARGET, 1);
            in_damage_step(&mut f, true);
            run(&mut f, r, code::CHAIN_SOLVED, code::CHAIN_SOLVED, 1);
            assert!(!alive(&f, r));
        }

        /// **Damage Calculation counts as well as the Damage Step** — the
        /// test is a mask over both phase bits.
        #[test]
        fn damage_calculation_defers_too() {
            let (mut f, r) = board(0);
            link_targeting(&mut f, vec![r]);
            run(&mut f, r, code::BECOME_TARGET, code::BECOME_TARGET, 1);
            f.infos.phase = phases::DAMAGE_CAL;
            f.core.damage_calculated = false;
            run(&mut f, r, code::CHAIN_SOLVED, code::CHAIN_SOLVED, 1);
            assert!(alive(&f, r));
            assert_eq!(api::get_flag_effect(&f, r, FLAG_DEFERRED), 1);
        }

        /// A disabled Reaper does not collect its own deferral either.
        #[test]
        fn a_disabled_reaper_does_not_collect_the_deferral() {
            let (mut f, r) = board(0);
            link_targeting(&mut f, vec![r]);
            run(&mut f, r, code::BECOME_TARGET, code::BECOME_TARGET, 1);
            in_damage_step(&mut f, false);
            run(&mut f, r, code::CHAIN_SOLVED, code::CHAIN_SOLVED, 1);
            let d = api::create_effect(&mut f, r);
            api::set_type(&mut f, d, effect_type::SINGLE);
            api::set_code(&mut f, d, code::DISABLE);
            api::register_effect(&mut f, r, d, false);
            run(&mut f, r, code::BATTLED, code::BATTLED, 0);
            assert!(alive(&f, r));
        }
    }

    mod the_standing_target {
        use super::*;

        fn asks(f: &Field, r: CardId) -> bool {
            let e = f.cards[r].single_effect.equal_range(code::SELF_DESTROY)[0];
            sdcon(f, e)
        }

        /// **`GetOwnerTargetCount` is the other direction from Ryu
        /// Senshi's `GetCardTarget`.** Something aiming at this card
        /// counts; this card aiming at something does not.
        #[test]
        fn anything_targeting_it_makes_it_self_destroy() {
            let (mut f, r) = board(0);
            assert!(!asks(&f, r), "nothing is aiming at it");

            let aimer = f.new_card(Card::with_data(
                CardData {
                    code: 9700,
                    type_: card_type::SPELL | card_type::EQUIP,
                    ..Default::default()
                },
                1,
            ));
            f.add_card(1, aimer, location::SZONE, 0, false);
            // The wrong direction first: this card aiming at the equip.
            f.cards[r].effect_target_cards.push(aimer);
            assert!(!asks(&f, r), "aiming at something is not the test");

            f.cards[r].effect_target_owner.push(aimer);
            assert!(asks(&f, r), "being aimed at is");
        }

        /// The self-destroy is registered through the engine's own
        /// reader, not only as a condition function.
        #[test]
        fn the_engine_sees_the_self_destroy() {
            let (mut f, r) = board(0);
            assert!(f.is_affected_by_effect(r, code::SELF_DESTROY).is_none());
            let aimer = f.new_card(Card::with_data(
                CardData {
                    code: 9700,
                    type_: card_type::SPELL | card_type::EQUIP,
                    ..Default::default()
                },
                1,
            ));
            f.add_card(1, aimer, location::SZONE, 0, false);
            f.cards[r].effect_target_owner.push(aimer);
            assert!(f.is_affected_by_effect(r, code::SELF_DESTROY).is_some());
        }
    }

    mod the_direct_damage_discard {
        use super::*;

        fn trigger_effect(f: &Field, r: CardId) -> EffectId {
            f.cards[r].single_effect.equal_range(code::BATTLE_DAMAGE)[0]
        }

        fn ask(f: &mut Field, r: CardId, damaged: u8, attack_target: Option<CardId>) -> bool {
            let e = trigger_effect(f, r);
            let mut ev = Event::new(code::BATTLE_DAMAGE);
            ev.event_player = damaged;
            f.core.attacker = Some(r);
            f.core.attack_target = attack_target;
            let ctx = Ctx {
                reason_effect: e,
                player: 0,
                event: &ev,
                card: None,
                args: &[],
            };
            condition(f, &ctx)
        }

        /// **Damage to the opponent, from an attack with no target.**
        #[test]
        fn only_a_direct_attack_on_the_opponent_counts() {
            let (mut f, r) = board(0);
            assert!(ask(&mut f, r, 1, None), "a direct attack");

            let blocker = f.new_card(Card::with_data(
                CardData {
                    code: 9800,
                    type_: card_type::MONSTER,
                    ..Default::default()
                },
                1,
            ));
            f.add_card(1, blocker, location::MZONE, 0, false);
            assert!(
                !ask(&mut f, r, 1, Some(blocker)),
                "running over a monster is not a direct attack"
            );
            assert!(
                !ask(&mut f, r, 0, None),
                "and damage to its own controller is not it either"
            );
        }

        /// **A random card out of the damaged player's hand.**
        #[test]
        fn it_discards_one_at_random_from_the_damaged_players_hand() {
            let (mut f, r) = board(0);
            for seq in 0..3 {
                let c = f.new_card(Card::with_data(
                    CardData {
                        code: 9900 + seq,
                        type_: card_type::MONSTER,
                        ..Default::default()
                    },
                    1,
                ));
                f.add_card(1, c, location::HAND, seq, false);
            }
            let e = trigger_effect(&f, r);
            let mut ev = Event::new(code::BATTLE_DAMAGE);
            ev.event_player = 1;
            let ctx = Ctx {
                reason_effect: e,
                player: 0,
                event: &ev,
                card: None,
                args: &[],
            };
            assert!(matches!(target(&mut f, &ctx, false, None), Yield::Done(1)));
            // The pick is a suspension point (a chance node in solver
            // mode); in generator mode the request answers itself, so
            // the rest of the operation runs at once.
            let Yield::Suspended(mut rest) = operation(&mut f, &ctx) else {
                panic!("the random pick suspends the operation");
            };
            assert!(matches!((rest.0)(&mut f, &ctx), Yield::Done(0)));
            drive(&mut f);
            assert_eq!(f.players[1].hand.len(), 2, "one left the hand");
            assert_eq!(f.players[1].grave.len(), 1, "for the graveyard");
            // **And it went as a discard.** `REASON_DISCARD` is what a
            // card that cares about being discarded reads; sending it as
            // a plain effect would be a different event to half this
            // pool.
            let gone = f.players[1].grave[0];
            assert_ne!(f.cards[gone].reason & reason::DISCARD, 0, "discarded");
            assert_ne!(f.cards[gone].reason & reason::EFFECT, 0, "by an effect");
        }

        /// An empty hand discards nothing and does not panic.
        #[test]
        fn an_empty_hand_discards_nothing() {
            let (mut f, r) = board(0);
            let e = trigger_effect(&f, r);
            let mut ev = Event::new(code::BATTLE_DAMAGE);
            ev.event_player = 1;
            let ctx = Ctx {
                reason_effect: e,
                player: 0,
                event: &ev,
                card: None,
                args: &[],
            };
            operation(&mut f, &ctx);
            drive(&mut f);
            assert_eq!(f.players[1].grave.len(), 0);
        }

        /// The announcement names one card from the other player's hand.
        #[test]
        fn the_announcement_names_one_from_the_other_hand() {
            let (mut f, r) = board(0);
            let e = trigger_effect(&f, r);
            let mut ch = Chain::new(e, Event::new(code::BATTLE_DAMAGE));
            ch.chain_count = 1;
            ch.chain_id = 61;
            f.core.current_chain.push(ch);
            let mut ev = Event::new(code::BATTLE_DAMAGE);
            ev.event_player = 1;
            let ctx = Ctx {
                reason_effect: e,
                player: 0,
                event: &ev,
                card: None,
                args: &[],
            };
            target(&mut f, &ctx, true, None);
            let info = f.core.current_chain[0]
                .opinfos
                .get(&category::HANDES)
                .expect("announced");
            assert_eq!(info.player, 1, "the opponent's hand");
            assert_eq!(info.param, 1, "one card");
        }
    }
}

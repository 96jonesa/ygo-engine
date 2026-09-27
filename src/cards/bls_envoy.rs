//! Black Luster Soldier – Envoy of the Beginning (`72989439`) —
//! `cardscripts/c72989439.lua`.
//!
//! ```text
//! Cannot be Normal Summoned or Set. Must first be Special Summoned
//! (from your hand) by banishing 1 LIGHT and 1 DARK monster from your GY.
//! Once per turn, you can either: Target 1 monster on the field; banish
//! it, or: Target 1 Defense Position monster your opponent controls;
//! destroy it. If this card attacks and destroys a monster by battle, it
//! can make a second attack in a row.
//! ```
//!
//! The second user of [`super::aux_select_unselect`], and the point of
//! doing it straight after Chaos Sorcerer: a library with one caller is
//! an unproven abstraction. It needed no change to the loop.
//!
//! ## Its `rescon` is a different rule wearing the same clothes
//!
//! Chaos Sorcerer's is "one LIGHT and one DARK". This one is
//! `aux.ChkfMMZ(1)` — a **seat count of at least one**, where Chaos
//! Sorcerer inlines `> 0` — and then `sg:IsExists(s.atchk1,1,nil,sg)`,
//! which reads: *some LIGHT in the selection has exactly one DARK among
//! the others*. On a two-card selection that is the same answer, but it
//! is not the same predicate, and transcribing one from the other would
//! be a paraphrase. Note `atchk1`'s exception argument is `c` itself, so
//! the DARK count excludes the LIGHT being tested — a LIGHT-and-DARK card
//! would otherwise count itself.
//!
//! ## Three places this differs from Chaos Sorcerer's banish effect
//!
//! They look like the same effect and are not, which is exactly the shape
//! that invites copying the wrong line across:
//!
//! 1. Its `chkc` arm has **no face-up test** — `IsLocation` and
//!    `IsAbleToRemove` only.
//! 2. Its `SetOperationInfo` names player **0**, not `tp`. A literal in
//!    the script, so a literal here.
//! 3. Its resolution checks **only** `IsRelateToEffect` — not face-up,
//!    not monster.
//!
//! ## The flag effect is what stops both halves in one turn
//!
//! `rmcost` registers a flag effect under the card's own number, and
//! `atcon` refuses while `GetFlagEffect(id) ~= 0`. So a turn that used the
//! banish cannot also take the second attack. The `EFFECT_CANNOT_ATTACK`
//! oath already stops the attack outright; the flag is what stops the
//! *chain* attack being granted at all.

use super::aux_select_unselect::{self as su, Args};
use crate::board::{location, position};
use crate::card::{attribute, reason, status};
use crate::effect::{effect_type, flag, Ctx, LabelObject, Yield};
use crate::event::{category, code, CardId, EffectId};
use crate::field::{resets, Field};
use crate::host_question::{hint, hintmsg};
use crate::script_api as api;

pub const CODE: u32 = 72989439;

/// `EFFECT_CANNOT_ATTACK`'s client hint — a library string.
const CANNOT_ATTACK_HINT: u64 = 3206;

const MZONE_OR_GRAVE: u32 = (location::MZONE | location::GRAVE) as u32;
const MZONE: u32 = location::MZONE as u32;

pub fn initial_effect(f: &mut Field, c: CardId) {
    api::enable_revive_limit(f, c);
    // Special summon procedure (from hand)
    let e1 = api::create_effect(f, c);
    api::set_description(f, e1, api::stringid(CODE, 0));
    api::set_type(f, e1, effect_type::FIELD);
    api::set_code(f, e1, code::SPSUMMON_PROC);
    api::set_property(f, e1, flag::UNCOPYABLE, 0);
    api::set_range(f, e1, u16::from(location::HAND));
    api::set_condition(f, e1, spcon1);
    // `spcon1(e, nil)` is true: the availability answer, and the
    // renumbering that comes with it. See `api::spcon_nil_guard`.
    api::set_avail_condition(f, e1, api::spcon_nil_guard);
    api::set_target(f, e1, sptg1);
    api::set_operation(f, e1, spop1);
    api::register_effect(f, c, e1, false);
    // Banish 1 monster on the field
    let e2 = api::create_effect(f, c);
    api::set_description(f, e2, api::stringid(CODE, 1));
    api::set_category(f, e2, category::REMOVE);
    api::set_property(f, e2, flag::CARD_TARGET, 0);
    api::set_type(f, e2, effect_type::IGNITION);
    api::set_count_limit(f, e2, 1, 0, 0);
    api::set_range(f, e2, u16::from(location::MZONE));
    api::set_cost(f, e2, rmcost);
    api::set_target(f, e2, rmtg);
    api::set_operation(f, e2, rmop);
    api::register_effect(f, c, e2, false);
    // Make a second attack in a row
    let e3 = api::create_effect(f, c);
    api::set_description(f, e3, api::stringid(CODE, 2));
    api::set_type(f, e3, effect_type::SINGLE | effect_type::TRIGGER_O);
    api::set_code(f, e3, code::BATTLE_DESTROYING);
    api::set_condition(f, e3, atcon);
    api::set_operation(f, e3, atop);
    api::register_effect(f, c, e3, false);
}

/// `s.atchk1(c, sg)` — is `c` a LIGHT with **exactly one** DARK among the
/// rest of the selection?
///
/// `sg:FilterCount(Card.IsAttribute, c, ATTRIBUTE_DARK)` excludes `c`,
/// which matters for a monster that is both attributes: without the
/// exception it would count itself and a LIGHT-and-DARK pair would read
/// as two DARKs.
fn is_light_with_one_dark(f: &mut Field, c: CardId, sg: &[CardId]) -> bool {
    if !api::is_attribute(f, c, attribute::LIGHT) {
        return false;
    }
    let darks = sg
        .iter()
        .filter(|&&x| x != c && api::is_attribute(f, x, attribute::DARK))
        .count();
    darks == 1
}

/// `s.rescon` — a seat for the summon, and a LIGHT paired with exactly
/// one DARK.
fn rescon(
    f: &mut Field,
    sg: &[CardId],
    _e: EffectId,
    tp: u8,
    _mg: &[CardId],
    _c: CardId,
) -> (bool, bool) {
    // `aux.ChkfMMZ(1)` — at least one seat, counted with the whole
    // selection gone.
    let res = api::get_mzone_count(f, tp, api::Except::Group(sg), None, None) >= 1
        && sg.iter().any(|&x| is_light_with_one_dark(f, x, sg));
    (res, false)
}

/// `s.spfilter1(c, att)` — a monster of that attribute which can be
/// banished as a cost, in a pile `aux.SpElimFilter` counts.
fn spfilter_light(f: &mut Field, c: CardId) -> bool {
    spfilter(f, c, attribute::LIGHT)
}

fn spfilter_dark(f: &mut Field, c: CardId) -> bool {
    spfilter(f, c, attribute::DARK)
}

fn spfilter_either(f: &mut Field, c: CardId) -> bool {
    spfilter(f, c, attribute::LIGHT | attribute::DARK)
}

fn spfilter(f: &mut Field, c: CardId, att: u32) -> bool {
    api::is_attribute(f, c, att)
        && api::is_able_to_remove_as_cost(f, c, None)
        && api::sp_elim_filter(f, c, true, false)
}

fn args(tp: u8, cancelable: bool) -> Args {
    Args {
        minc: 2,
        maxc: 2,
        rescon: Some(rescon),
        seltp: tp,
        hintmsg: hintmsg::REMOVE,
        finishcon: None,
        breakcon: None,
        cancelable,
    }
}

/// `s.spcon1` — may this card summon itself right now?
///
/// **It scans twice and merges**, where Chaos Sorcerer scans once: the
/// two halves are counted separately so that `#rg1 > 0 and #rg2 > 0` can
/// reject a graveyard full of one attribute before the search runs.
///
/// `ft > -2` is not `ft > 0`. `GetLocationCount` goes **negative** when
/// more monsters must leave than there are seats, and the reference is
/// allowing for the materials themselves being on the field — the real
/// seat test is inside `rescon`.
fn spcon1(f: &mut Field, ctx: &Ctx) -> bool {
    if ctx.card.is_none() {
        return true;
    }
    let tp = api::get_handler_player(f, ctx.reason_effect);
    let rg1 = api::get_matching_group(
        f,
        Some(&spfilter_light),
        tp,
        MZONE_OR_GRAVE,
        0,
        api::Except::None,
    );
    let rg2 = api::get_matching_group(
        f,
        Some(&spfilter_dark),
        tp,
        MZONE_OR_GRAVE,
        0,
        api::Except::None,
    );
    let mut rg = rg1.clone();
    for c in &rg2 {
        if !rg.contains(c) {
            rg.push(*c);
        }
    }
    rg.sort_unstable();
    let ft = api::get_location_count(f, tp, location::MZONE);
    ft > -2
        && !rg1.is_empty()
        && !rg2.is_empty()
        && su::can_select_unselect(f, &rg, ctx.reason_effect, tp, args(tp, false))
}

/// `s.sptg1` — choose the two materials.
///
/// One scan here, with both attributes, where the condition took two —
/// the merge above and this filter describe the same set.
fn sptg1(f: &mut Field, ctx: &Ctx, _chk: bool, _chkc: Option<CardId>) -> Yield {
    let tp = ctx.player;
    let rg = api::get_matching_group(
        f,
        Some(&spfilter_either),
        tp,
        MZONE_OR_GRAVE,
        0,
        api::Except::None,
    );
    su::select_unselect_group(f, ctx, rg, args(tp, true), sptg1_done)
}

fn sptg1_done(f: &mut Field, ctx: &Ctx, sg: Vec<CardId>) -> Yield {
    if sg.is_empty() {
        return api::yes(false);
    }
    // `g:KeepAlive()` has nothing to do: `Field::groups` is append-only.
    let g = api::new_group(f, sg);
    api::set_label_object(f, ctx.reason_effect, Some(LabelObject::Group(g)));
    api::yes(true)
}

/// `s.spop1` — banish what `s.sptg1` chose, as a cost.
fn spop1(f: &mut Field, ctx: &Ctx) -> Yield {
    let Some(g) = api::get_label_object_group(f, ctx.reason_effect) else {
        return api::done();
    };
    let cards = api::group_cards(f, g);
    api::remove(f, cards, position::FACEUP, reason::COST);
    // `g:DeleteGroup()` — likewise nothing to do.
    api::done()
}

/// `s.rmcost` — cannot attack this turn, and a flag saying the once-per-
/// turn has been spent.
fn rmcost(f: &mut Field, ctx: &Ctx, chk: bool) -> bool {
    let Some(c) = api::get_handler(f, ctx.reason_effect) else {
        return false;
    };
    if !chk {
        return api::get_attack_announced_count(f, c) == 0;
    }
    let e1 = api::create_effect(f, c);
    api::set_description(f, e1, CANNOT_ATTACK_HINT);
    api::set_type(f, e1, effect_type::SINGLE);
    api::set_property(
        f,
        e1,
        flag::CANNOT_DISABLE | flag::OATH | flag::CLIENT_HINT,
        0,
    );
    api::set_code(f, e1, code::CANNOT_ATTACK);
    api::set_reset(f, e1, resets::STANDARD_PHASE_END, 1);
    // `c:RegisterEffect(e1, true)` — **forced**, so it registers even
    // while the card is somewhere registration would normally refuse.
    api::register_effect(f, c, e1, true);
    api::register_flag_effect(f, c, CODE, resets::STANDARD_PHASE_END, 0, 1, 0);
    true
}

/// `Card.IsAbleToRemove` on its own — no face-up test, unlike Chaos
/// Sorcerer's.
fn able_to_remove(f: &mut Field, c: CardId) -> bool {
    api::is_able_to_remove(f, c, None, None, None)
}

/// `s.rmtg` — name the monster to banish.
fn rmtg(f: &mut Field, ctx: &Ctx, chk: bool, chkc: Option<CardId>) -> Yield {
    let tp = ctx.player;
    if let Some(chkc) = chkc {
        return api::yes(
            api::is_location(f, chkc, u16::from(location::MZONE))
                && api::is_able_to_remove(f, chkc, None, None, None),
        );
    }
    if !chk {
        return api::yes(api::is_existing_target(
            f,
            Some(&able_to_remove),
            tp,
            MZONE,
            MZONE,
            1,
            api::Except::None,
        ));
    }
    api::hint(f, hint::SELECTMSG, tp, hintmsg::REMOVE);
    api::select_target(
        f,
        tp,
        Some(&able_to_remove),
        tp,
        MZONE,
        MZONE,
        1,
        1,
        api::Except::None,
    );
    api::suspend(move |f, _ctx| {
        let g = api::selected_targets(f);
        // Player **0**, a literal in the script — not `tp`.
        api::set_operation_info(f, 0, category::REMOVE, g, 1, 0, 0);
        api::done()
    })
}

/// `s.rmop` — banish it if it is still the card that was named. One test,
/// where Chaos Sorcerer's has three.
fn rmop(f: &mut Field, ctx: &Ctx) -> Yield {
    let Some(tc) = api::get_first_target(f) else {
        return api::done();
    };
    if api::is_relate_to_effect(f, tc, ctx.reason_effect) {
        api::remove(f, vec![tc], position::FACEUP, reason::EFFECT);
    }
    api::done()
}

/// `s.atcon` — four tests, and none of them is about the destroyed
/// monster.
///
/// `aux.bdocon` is `IsRelateToBattle() and IsStatus(STATUS_OPPO_BATTLE)`:
/// the card is still the one that fought, and it fought an **opponent's**
/// monster. The flag effect is the once-per-turn lock shared with the
/// banish half.
fn atcon(f: &mut Field, ctx: &Ctx) -> bool {
    let Some(c) = api::get_handler(f, ctx.reason_effect) else {
        return false;
    };
    api::get_attacker(f) == Some(c)
        && api::is_relate_to_battle(f, c)
        && api::is_status(f, c, status::OPPO_BATTLE)
        && api::get_flag_effect(f, c, CODE) == 0
        && api::can_chain_attack(f, c, None, false)
}

/// `s.atop` — `Duel.ChainAttack()`, with no named target.
fn atop(f: &mut Field, _ctx: &Ctx) -> Yield {
    api::chain_attack(f, None);
    api::done()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{card_type, Card, CardData};
    use crate::chain::Chain;
    use crate::duel::phases;
    use crate::event::Event;
    use crate::field::Message;
    use crate::processor::{Kind, Status};

    fn monster(f: &mut Field, player: u8, loc: u8, seq: u32, attr: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 6500 + seq + u32::from(player) * 50,
                type_: card_type::MONSTER | card_type::NORMAL,
                level: 4,
                attack: 1200,
                defense: 1000,
                attribute: attr,
                ..Default::default()
            },
            player,
        );
        c.current.controller = player;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(player, id, loc, seq, false);
        f.cards[id].current.position = if loc == location::MZONE {
            position::FACEUP_ATTACK
        } else {
            position::FACEUP
        };
        id
    }

    fn envoy(f: &mut Field, player: u8, loc: u8, seq: u32) -> CardId {
        let mut c = Card::with_data(crate::cards::card_data(CODE).expect("printed data"), player);
        c.current.controller = player;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(player, id, loc, seq, false);
        f.cards[id].current.position = if loc == location::MZONE {
            position::FACEUP_ATTACK
        } else {
            position::FACEUP
        };
        f.initialize_card(id);
        id
    }

    /// Spirit Elimination, which `aux.SpElimFilter` asks after by card
    /// number. Not in the Goat pool, so this is the only way to reach the
    /// branch of the filter that reads the **field** — and with it the
    /// only boards where the materials occupy Monster Zone seats.
    fn spirit_elimination_applies_to(f: &mut Field, player: u8) {
        let mut c = Card::with_data(
            CardData {
                code: 69832741,
                type_: card_type::SPELL,
                ..Default::default()
            },
            player,
        );
        c.current.controller = player;
        c.set_status(status::EFFECT_ENABLED, true);
        let holder = f.new_card(c);
        f.add_card(player, holder, location::SZONE, 4, false);
        f.cards[holder].current.position = position::FACEUP;
        let e = api::create_effect(f, holder);
        api::set_type(f, e, effect_type::FIELD);
        api::set_code(f, e, 69832741);
        api::set_range(f, e, u16::from(location::SZONE));
        api::set_property(f, e, flag::PLAYER_TARGET, 0);
        api::set_target_range(f, e, 1, 0);
        api::register_effect(f, holder, e, false);
    }

    fn procedure_of(f: &Field, c: CardId) -> EffectId {
        f.cards[c].field_effect.equal_range(code::SPSUMMON_PROC)[0]
    }

    /// **The procedure answers "available" unconditionally.** The script's
    /// `spcon` opens with `if c==nil then return true end`; the reference
    /// asks it that way from `effect::is_available`, marks the effect
    /// available on the true and renumbers it once — the number that
    /// orders the special-summon menu against another procedure.
    #[test]
    fn the_procedure_answers_available_unconditionally() {
        let (f, bls, _) = board(0, &[]);
        let e = procedure_of(&f, bls);
        let cond = f
            .effects
            .get(e)
            .and_then(|x| x.avail_condition)
            .expect("the availability answer is registered");
        assert!(cond(&f, e), "and it is true");
    }

    fn ignition_of(f: &Field, c: CardId) -> EffectId {
        f.cards[c].field_effect.equal_range(0)[0]
    }

    fn attack_trigger_of(f: &Field, c: CardId) -> EffectId {
        f.cards[c]
            .single_effect
            .equal_range(code::BATTLE_DESTROYING)[0]
    }

    /// Envoy in `tp`'s hand, with the named attributes in the graveyard.
    fn board(tp: u8, grave: &[u32]) -> (Field, CardId, Vec<CardId>) {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        f.infos.turn_player = tp;
        f.infos.phase = phases::MAIN1;
        let bls = envoy(&mut f, tp, location::HAND, 0);
        let buried = grave
            .iter()
            .enumerate()
            .map(|(i, &a)| monster(&mut f, tp, location::GRAVE, i as u32, a))
            .collect();
        (f, bls, buried)
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

    fn banished(f: &Field, c: CardId) -> bool {
        f.cards[c].current.location == location::REMOVED
    }

    mod registration {
        use super::*;

        /// **Three printed effects, the revive limit, and a printed type
        /// that carries `TYPE_SPSUMMON`** — without which ocgcore's idle
        /// command offers a two-tribute Normal Summon.
        #[test]
        fn the_three_effects_and_the_printed_line() {
            let (f, bls, _) = board(0, &[]);
            let data = crate::cards::card_data(CODE).expect("printed data");
            assert!(
                data.is_type(card_type::SPSUMMON),
                "cannot be Normal Summoned, on the printed line"
            );
            assert_eq!((data.level, data.attack, data.defense), (8, 3000, 2500));
            assert_eq!(data.attribute, attribute::LIGHT);

            let proc = f.effects.get(procedure_of(&f, bls)).expect("procedure");
            assert!(proc.is_type(effect_type::FIELD));
            assert_eq!(proc.range, u16::from(location::HAND));
            assert!(proc.is_flag(flag::UNCOPYABLE));
            assert!(
                !proc.is_flag(flag::CANNOT_DISABLE),
                "unlike Chaos Sorcerer's, this procedure is not undisableable"
            );

            let ign = f.effects.get(ignition_of(&f, bls)).expect("ignition");
            assert!(ign.is_type(effect_type::IGNITION));
            assert_eq!(ign.category, category::REMOVE);
            assert!(ign.is_flag(flag::CARD_TARGET));
            assert_eq!(ign.count_limit, 1);

            let at = f
                .effects
                .get(attack_trigger_of(&f, bls))
                .expect("attack trigger");
            assert!(at.is_type(effect_type::SINGLE));
            assert!(at.is_type(effect_type::TRIGGER_O), "optional");
            assert_eq!(at.code, code::BATTLE_DESTROYING);
            assert_eq!(at.description, api::stringid(CODE, 2));

            for wanted in [code::UNSUMMONABLE_CARD, code::REVIVE_LIMIT] {
                assert_eq!(f.cards[bls].single_effect.equal_range(wanted).len(), 1);
            }
        }
    }

    mod spcon {
        use super::*;

        fn summonable(f: &mut Field, bls: CardId) -> bool {
            let proc = procedure_of(f, bls);
            f.is_spsummonable(bls, proc)
        }

        #[test]
        fn one_of_each_attribute_is_enough() {
            let (mut f, bls, _) = board(0, &[attribute::LIGHT, attribute::DARK]);
            assert!(summonable(&mut f, bls));
        }

        /// **The two attributes are counted separately before the search
        /// runs.** Two materials of one attribute is the same total as
        /// one of each, and only the split test tells them apart early.
        #[test]
        fn a_graveyard_of_one_attribute_refuses() {
            for attr in [attribute::LIGHT, attribute::DARK] {
                let (mut f, bls, _) = board(0, &[attr, attr, attr]);
                assert!(!summonable(&mut f, bls), "three of one attribute");
            }
        }

        #[test]
        fn asked_about_no_card_it_answers_yes() {
            let (mut f, bls, _) = board(0, &[]);
            let e = procedure_of(&f, bls);
            let ev = Event::new(0);
            let ctx = Ctx {
                reason_effect: e,
                player: 0,
                event: &ev,
                card: None,
                args: &[],
            };
            assert!(spcon1(&mut f, &ctx));
        }

        /// **`ft > -2` is not `ft > 0`.** The seat count goes negative,
        /// and the real seat test lives in `rescon` — so a full row still
        /// passes this gate and is refused later.
        #[test]
        fn a_full_row_passes_the_location_gate_and_fails_the_seat_one() {
            let (mut f, bls, _) = board(0, &[attribute::LIGHT, attribute::DARK]);
            for i in 0..5 {
                monster(&mut f, 0, location::MZONE, i, attribute::EARTH);
            }
            assert_eq!(
                api::get_location_count(&mut f, 0, location::MZONE),
                0,
                "no seats, but not below -2"
            );
            assert!(!summonable(&mut f, bls), "refused, by rescon");
        }

        /// **A monster of both attributes pairs with a DARK.**
        /// `FilterCount`'s exception is the card being tested, so it does
        /// not count itself — without that, a both-attribute card reads as
        /// two DARKs and the pair is refused.
        #[test]
        fn a_card_of_both_attributes_does_not_count_itself() {
            let (mut f, bls, _) = board(0, &[]);
            monster(
                &mut f,
                0,
                location::GRAVE,
                0,
                attribute::LIGHT | attribute::DARK,
            );
            monster(&mut f, 0, location::GRAVE, 1, attribute::DARK);
            assert!(summonable(&mut f, bls));
        }

        /// **The row is counted with both materials gone.** With Spirit
        /// Elimination applying the materials are on the field, so a full
        /// row is still fine: banishing them is what makes the seat.
        /// `ft > -2` lets that board through, and `rescon` is what
        /// actually decides.
        #[test]
        fn a_full_row_is_fine_when_the_materials_are_in_it() {
            let (mut f, bls, _) = board(0, &[]);
            spirit_elimination_applies_to(&mut f, 0);
            monster(&mut f, 0, location::MZONE, 0, attribute::LIGHT);
            monster(&mut f, 0, location::MZONE, 1, attribute::DARK);
            for i in 2..5 {
                monster(&mut f, 0, location::MZONE, i, attribute::EARTH);
            }
            assert_eq!(
                api::get_location_count(&mut f, 0, location::MZONE),
                0,
                "no free seat by the plain count"
            );
            assert!(summonable(&mut f, bls), "but banishing the two makes one");
        }

        /// And a **face-down** monster on the field is not a material —
        /// `aux.SpElimFilter`'s `mustbefaceup`, which only bites there.
        #[test]
        fn a_face_down_monster_is_not_a_material() {
            let (mut f, bls, _) = board(0, &[]);
            spirit_elimination_applies_to(&mut f, 0);
            monster(&mut f, 0, location::MZONE, 0, attribute::LIGHT);
            let hidden = monster(&mut f, 0, location::MZONE, 1, attribute::DARK);
            assert!(summonable(&mut f, bls), "the positive sibling");
            f.cards[hidden].current.position = position::FACEDOWN_DEFENSE;
            assert!(!summonable(&mut f, bls));
        }

        /// **Two DARKs are not a pair.** `atchk1` wants a *LIGHT* with one
        /// DARK beside it, so the second DARK stops being offered once the
        /// first is taken — and the pair is never selectable.
        #[test]
        fn two_darks_are_never_a_pair() {
            let (mut f, bls, buried) =
                board(0, &[attribute::LIGHT, attribute::DARK, attribute::DARK]);
            let run = summon_with(&mut f, bls, 0, &[Some(buried[1]), Some(buried[0])]);
            assert_eq!(
                run.asks[1].1,
                vec![buried[0]],
                "only the LIGHT remains legal beside a DARK"
            );
            assert!(!run.asks[1].1.contains(&buried[2]), "not the other DARK");
        }

        /// **`atchk1` is "exactly one DARK", not "at least one".** With a
        /// two-card selection those agree, so the predicate is tested
        /// directly on a longer one — which is the only place the
        /// reference's `FilterCount(...) == 1` differs from `>= 1`.
        #[test]
        fn atchk1_wants_exactly_one_dark() {
            let (mut f, _, _) = board(0, &[]);
            let light = monster(&mut f, 0, location::GRAVE, 0, attribute::LIGHT);
            let d1 = monster(&mut f, 0, location::GRAVE, 1, attribute::DARK);
            let d2 = monster(&mut f, 0, location::GRAVE, 2, attribute::DARK);
            assert!(
                super::is_light_with_one_dark(&mut f, light, &[light, d1]),
                "one DARK beside it"
            );
            assert!(
                !super::is_light_with_one_dark(&mut f, light, &[light, d1, d2]),
                "two is not one"
            );
            assert!(
                !super::is_light_with_one_dark(&mut f, light, &[light]),
                "and nor is none"
            );
            assert!(
                !super::is_light_with_one_dark(&mut f, d1, &[d1, d2]),
                "the card itself must be a LIGHT"
            );
        }

        /// A face-down monster on the field is not a material.
        #[test]
        fn the_material_filter_wants_a_banishable_card() {
            let (mut f, bls, buried) = board(0, &[attribute::LIGHT, attribute::DARK]);
            assert!(summonable(&mut f, bls), "the positive sibling");
            let mut e = crate::effect::Effect::new(effect_type::SINGLE, code::CANNOT_REMOVE);
            e.owner = Some(buried[0]);
            e.handler = Some(buried[0]);
            let id = f.new_effect(e);
            f.cards[buried[0]]
                .single_effect
                .insert(code::CANNOT_REMOVE, id);
            f.cards[buried[0]].indexer.insert(id);
            assert!(!summonable(&mut f, bls));
        }
    }

    /// Everything a driven run said and was asked.
    #[derive(Default)]
    struct Run {
        asks: Vec<(u8, Vec<CardId>, Vec<CardId>)>,
        card_questions: Vec<(u8, Vec<CardId>)>,
        hints: Vec<(u8, u8, u64)>,
        accepted: bool,
    }

    #[allow(clippy::too_many_arguments)]
    fn drive(
        f: &mut Field,
        e: EffectId,
        tp: u8,
        subject: Option<CardId>,
        answers: &[Option<CardId>],
        target_reports_a_verdict: bool,
    ) -> Run {
        let mut run = Run::default();
        let mut seen = 0usize;
        let mut next = 0usize;
        let mut operated = false;
        for _ in 0..8192 {
            while seen < f.messages.len() {
                if let Message::Hint {
                    kind,
                    player,
                    value,
                } = &f.messages[seen]
                {
                    run.hints.push((*kind, *player, *value));
                }
                seen += 1;
            }
            match f.process() {
                Status::Continue => {
                    if f.core.units.is_empty() && f.core.subunits.is_empty() {
                        if operated {
                            break;
                        }
                        operated = true;
                        run.accepted = f.core.returns.get() != 0;
                        if target_reports_a_verdict && !run.accepted {
                            break;
                        }
                        f.core.sub_solving_event.push_back(Event::new(0));
                        f.emplace(Kind::ExecuteOperation {
                            resume: None,
                            effect: e,
                            player: tp,
                            subject,
                            args: Vec::new(),
                            was_disabled: false,
                        });
                    }
                }
                Status::Awaiting => match f.messages.last() {
                    Some(Message::SelectUnselectCard {
                        player,
                        select,
                        unselect,
                        ..
                    }) => {
                        run.asks.push((*player, select.clone(), unselect.clone()));
                        let answer = answers.get(next).copied().unwrap_or(None);
                        next += 1;
                        f.core.return_cards.clear();
                        match answer {
                            Some(card) => {
                                let i = select
                                    .iter()
                                    .chain(unselect.iter())
                                    .position(|&x| x == card)
                                    .unwrap_or_else(|| panic!("{card:?} was not offered"));
                                f.core.returns.set_i32(0, 1);
                                f.core.returns.set_i32(1, i as i32);
                            }
                            None => f.core.returns.set_i32(0, -1),
                        }
                    }
                    Some(Message::SelectCard {
                        player, min, cards, ..
                    }) => {
                        run.card_questions.push((*player, cards.clone()));
                        let min = usize::from(*min);
                        let pick = answers
                            .get(next)
                            .copied()
                            .flatten()
                            .and_then(|card| cards.iter().position(|&x| x == card))
                            .unwrap_or(0);
                        next += 1;
                        f.core.return_cards.clear();
                        f.core.returns.set_i32(0, 0);
                        f.core.returns.set_i32(1, min as i32);
                        for i in 0..min {
                            f.core.returns.set_i32(i + 2, (pick + i) as i32);
                        }
                    }
                    other => panic!("unexpected question: {other:?}"),
                },
                Status::End => break,
            }
        }
        run
    }

    fn summon_with(f: &mut Field, bls: CardId, tp: u8, answers: &[Option<CardId>]) -> Run {
        let e = procedure_of(f, bls);
        f.core.sub_solving_event.push_back(Event::new(0));
        f.emplace(Kind::ExecuteTarget {
            resume: None,
            effect: e,
            player: tp,
            subject: Some(bls),
            args: Vec::new(),
            was_disabled: false,
        });
        drive(f, e, tp, Some(bls), answers, true)
    }

    mod the_summon_procedure {
        use super::*;

        /// End to end: the two chosen materials are banished face-up as a
        /// **cost**.
        #[test]
        fn it_banishes_the_two_chosen_materials_as_a_cost() {
            let (mut f, bls, buried) =
                board(0, &[attribute::LIGHT, attribute::DARK, attribute::DARK]);
            let run = summon_with(&mut f, bls, 0, &[Some(buried[0]), Some(buried[2])]);
            assert!(run.accepted);
            assert!(banished(&f, buried[0]) && banished(&f, buried[2]));
            assert!(!banished(&f, buried[1]), "only the two that were named");
            for &m in &[buried[0], buried[2]] {
                assert_eq!(f.cards[m].current.position, position::FACEUP);
                assert!(
                    f.cards[m].reason & reason::COST != 0,
                    "a cost, not an effect"
                );
            }
        }

        /// The offer narrows the same way Chaos Sorcerer's does, through a
        /// different `rescon`.
        #[test]
        fn the_second_question_offers_only_the_other_attribute() {
            let (mut f, bls, buried) = board(
                0,
                &[
                    attribute::LIGHT,
                    attribute::DARK,
                    attribute::LIGHT,
                    attribute::DARK,
                ],
            );
            let run = summon_with(&mut f, bls, 0, &[Some(buried[0]), Some(buried[1])]);
            assert_eq!(run.asks.len(), 2);
            assert_eq!(
                run.asks[1].1,
                vec![buried[3], buried[1]],
                "only the DARKs, once a LIGHT is taken"
            );
            assert_eq!(run.asks[1].2, vec![buried[0]]);
        }

        /// **`rescon`'s seat check is the target's only one**, as it is for
        /// Chaos Sorcerer — `spcon1`'s `ft > -2` is a looser gate, so this
        /// drives the target on a board the condition would refuse.
        #[test]
        fn the_selection_refuses_when_there_is_no_seat() {
            let (mut f, bls, buried) = board(0, &[attribute::LIGHT, attribute::DARK]);
            for i in 0..5 {
                monster(&mut f, 0, location::MZONE, i, attribute::EARTH);
            }
            let run = summon_with(&mut f, bls, 0, &[Some(buried[0]), Some(buried[1])]);
            assert!(run.asks.is_empty(), "nothing legal, so nothing asked");
            assert!(!run.accepted);
        }

        #[test]
        fn cancelling_refuses_the_summon() {
            let (mut f, bls, buried) = board(0, &[attribute::LIGHT, attribute::DARK]);
            let run = summon_with(&mut f, bls, 0, &[None]);
            assert!(!run.accepted);
            assert!(!banished(&f, buried[0]) && !banished(&f, buried[1]));
        }

        /// The controller is asked, with the banish prompt, once per pick.
        #[test]
        fn each_question_carries_the_banish_prompt() {
            let (mut f, bls, buried) = board(1, &[attribute::LIGHT, attribute::DARK]);
            let run = summon_with(&mut f, bls, 1, &[Some(buried[0]), Some(buried[1])]);
            assert_eq!(
                run.hints.iter().filter(|h| h.0 == hint::SELECTMSG).count(),
                2
            );
            assert!(run.asks.iter().all(|q| q.0 == 1));
        }
    }

    mod the_banish_effect {
        use super::*;

        fn as_reason(f: &mut Field, e: EffectId, tp: u8) {
            f.core.reason_effect = Some(e);
            f.core.reason_player = tp;
        }

        fn on_field_as(
            tp: u8,
            mine: &[u32],
            theirs: &[u32],
        ) -> (Field, CardId, Vec<CardId>, Vec<CardId>) {
            let mut f = Field::new(8000);
            f.infos.turn_id = 3;
            f.infos.turn_player = tp;
            f.infos.phase = phases::MAIN1;
            let bls = envoy(&mut f, tp, location::MZONE, 0);
            let ours = mine
                .iter()
                .enumerate()
                .map(|(i, &a)| monster(&mut f, tp, location::MZONE, i as u32 + 1, a))
                .collect();
            let theirs = theirs
                .iter()
                .enumerate()
                .map(|(i, &a)| monster(&mut f, 1 - tp, location::MZONE, i as u32, a))
                .collect();
            (f, bls, ours, theirs)
        }

        fn on_field(mine: &[u32], theirs: &[u32]) -> (Field, CardId, Vec<CardId>, Vec<CardId>) {
            on_field_as(0, mine, theirs)
        }

        /// **Paying sets two things**: the cannot-attack oath, and a flag
        /// effect under the card's own number that locks the second attack
        /// out for the turn.
        #[test]
        fn paying_registers_the_oath_and_the_once_per_turn_flag() {
            let (mut f, bls, _, _) = on_field(&[], &[attribute::EARTH]);
            let e = ignition_of(&f, bls);
            as_reason(&mut f, e, 0);
            let ev = Event::new(0);
            assert!(rmcost(&mut f, &ctx_for(e, &ev, 0), true));

            let ids = f.cards[bls].single_effect.equal_range(code::CANNOT_ATTACK);
            assert_eq!(ids.len(), 1);
            let x = f.effects.get(ids[0]).expect("the prohibition");
            assert!(x.is_flag(flag::OATH));
            assert!(x.is_flag(flag::CANNOT_DISABLE));
            assert_eq!(x.description, CANNOT_ATTACK_HINT);
            assert_ne!(x.reset_flag, 0, "it expires at the end phase");

            assert_eq!(
                api::get_flag_effect(&f, bls, CODE),
                1,
                "the once-per-turn flag, under the card's own number"
            );
            let flag_ids = f.cards[bls]
                .single_effect
                .equal_range(api::flag_code(CODE))
                .to_vec();
            let fe = f.effects.get(flag_ids[0]).expect("the flag effect");
            assert_ne!(
                fe.reset_flag, 0,
                "and it expires — a flag that never does locks the card out for the duel"
            );
        }

        /// **The prohibition is registered `forced`.** Without that, a
        /// card the reason effect cannot touch — a monster mid-summon, for
        /// instance — would pay the cost and keep its attack.
        #[test]
        fn the_prohibition_is_registered_even_when_the_card_is_untouchable() {
            let (mut f, bls, _, _) = on_field(&[], &[attribute::EARTH]);
            let e = ignition_of(&f, bls);
            as_reason(&mut f, e, 0);
            f.cards[bls].set_status(status::SUMMONING, true);
            let ev = Event::new(0);
            assert!(rmcost(&mut f, &ctx_for(e, &ev, 0), true));
            assert_eq!(
                f.cards[bls]
                    .single_effect
                    .equal_range(code::CANNOT_ATTACK)
                    .len(),
                1,
                "registered anyway"
            );
        }

        #[test]
        fn a_monster_that_attacked_cannot_pay() {
            let (mut f, bls, _, _) = on_field(&[], &[attribute::EARTH]);
            let e = ignition_of(&f, bls);
            as_reason(&mut f, e, 0);
            let ev = Event::new(0);
            assert!(rmcost(&mut f, &ctx_for(e, &ev, 0), false));
            f.cards[bls].attack_announce_count = 1;
            assert!(!rmcost(&mut f, &ctx_for(e, &ev, 0), false));
        }

        /// **A face-down monster is a legal target here**, where Chaos
        /// Sorcerer's identical-looking effect refuses one. Two scripts,
        /// two rules.
        #[test]
        fn a_face_down_monster_is_a_legal_target() {
            let (mut f, bls, _, theirs) = on_field(&[], &[attribute::EARTH]);
            f.cards[theirs[0]].current.position = position::FACEDOWN_DEFENSE;
            let e = ignition_of(&f, bls);
            as_reason(&mut f, e, 0);
            let ev = Event::new(0);
            assert_eq!(
                rmtg(&mut f, &ctx_for(e, &ev, 0), false, Some(theirs[0])).finished(),
                Some(1),
                "the chkc arm has no face-up test"
            );
            f.cards[bls].current.position = position::FACEDOWN_DEFENSE;
            assert_eq!(
                rmtg(&mut f, &ctx_for(e, &ev, 0), false, None).finished(),
                Some(1),
                "and neither does the scan"
            );
        }

        /// It does ask about the zone, though.
        #[test]
        fn the_card_target_check_refuses_a_monster_outside_the_row() {
            let (mut f, bls, _, _) = on_field(&[], &[attribute::EARTH]);
            let buried = monster(&mut f, 1, location::GRAVE, 0, attribute::EARTH);
            let e = ignition_of(&f, bls);
            as_reason(&mut f, e, 0);
            let ev = Event::new(0);
            assert_eq!(
                rmtg(&mut f, &ctx_for(e, &ev, 0), false, Some(buried)).finished(),
                Some(0)
            );
        }

        /// **The scan is two-sided**, which needs our own side empty of
        /// anything legal — and Envoy is itself in the row.
        #[test]
        fn the_scan_reaches_both_sides() {
            let (mut f, bls, _, _) = on_field(&[], &[attribute::EARTH]);
            let e = ignition_of(&f, bls);
            as_reason(&mut f, e, 0);
            let ev = Event::new(0);
            let mut gone = crate::effect::Effect::new(effect_type::SINGLE, code::CANNOT_REMOVE);
            gone.owner = Some(bls);
            gone.handler = Some(bls);
            let id = f.new_effect(gone);
            f.cards[bls].single_effect.insert(code::CANNOT_REMOVE, id);
            f.cards[bls].indexer.insert(id);
            assert_eq!(
                rmtg(&mut f, &ctx_for(e, &ev, 0), false, None).finished(),
                Some(1),
                "only the opponent's monster is legal, and it counts"
            );
        }

        /// **The operation info names player zero, not the activator.** A
        /// literal in the script, and only visible from player 1.
        #[test]
        fn the_operation_info_always_names_player_zero() {
            for tp in [0u8, 1u8] {
                let (mut f, bls, _, theirs) = on_field_as(tp, &[], &[attribute::EARTH]);
                let e = ignition_of(&f, bls);
                let mut ch = Chain::new(e, Event::new(0));
                ch.triggering_player = tp;
                ch.chain_count = 1;
                ch.chain_id = 11;
                f.core.current_chain.push(ch);
                f.core.chain_solving = true;
                f.core.sub_solving_event.push_back(Event::new(0));
                f.emplace(Kind::ExecuteTarget {
                    resume: None,
                    effect: e,
                    player: tp,
                    subject: None,
                    args: Vec::new(),
                    was_disabled: false,
                });
                drive(&mut f, e, tp, None, &[Some(theirs[0])], false);
                let info = f.core.current_chain[0]
                    .opinfos
                    .get(&category::REMOVE)
                    .expect("the remove category");
                assert_eq!(info.player, 0, "player zero whoever activated");
                assert_eq!(info.cards.as_deref(), Some(&[theirs[0]][..]));
            }
        }

        fn ready_to_resolve(f: &mut Field, e: EffectId, tc: CardId, relate: bool) {
            let mut ch = Chain::new(e, Event::new(0));
            ch.triggering_player = 0;
            ch.chain_count = 1;
            ch.chain_id = 11;
            ch.target_cards = vec![tc];
            f.core.current_chain.push(ch);
            if relate {
                f.cards[tc].relate_effect_insert_for_test(e);
            }
            f.core.reason_effect = Some(e);
            f.core.reason_player = 0;
        }

        fn resolve_now(f: &mut Field, e: EffectId) {
            let ev = Event::new(0);
            rmop(f, &ctx_for(e, &ev, 0));
            for _ in 0..4096 {
                if f.core.units.is_empty() && f.core.subunits.is_empty() {
                    break;
                }
                match f.process() {
                    Status::Continue => {}
                    other => panic!("unexpected {other:?} while resolving"),
                }
            }
        }

        /// **The resolution has one test, not three.** A monster turned
        /// face-down between target and resolution is still banished —
        /// where Chaos Sorcerer's would leave it alone.
        #[test]
        fn a_target_turned_face_down_is_still_banished() {
            let (mut f, bls, _, theirs) = on_field(&[], &[attribute::EARTH]);
            let e = ignition_of(&f, bls);
            ready_to_resolve(&mut f, e, theirs[0], true);
            f.cards[theirs[0]].current.position = position::FACEDOWN_DEFENSE;
            resolve_now(&mut f, e);
            assert!(banished(&f, theirs[0]));
        }

        /// But the relation is still required.
        #[test]
        fn a_target_that_lost_its_relation_is_not_banished() {
            let (mut f, bls, _, theirs) = on_field(&[], &[attribute::EARTH]);
            let e = ignition_of(&f, bls);
            ready_to_resolve(&mut f, e, theirs[0], true);
            resolve_now(&mut f, e);
            assert!(banished(&f, theirs[0]), "the positive sibling");

            let (mut f, bls, _, theirs) = on_field(&[], &[attribute::EARTH]);
            let e = ignition_of(&f, bls);
            ready_to_resolve(&mut f, e, theirs[0], false);
            resolve_now(&mut f, e);
            assert!(!banished(&f, theirs[0]));
        }
    }

    mod the_second_attack {
        use super::*;

        /// Envoy mid-battle, having destroyed an opponent's monster.
        fn after_destroying() -> (Field, CardId, CardId) {
            let mut f = Field::new(8000);
            f.infos.turn_id = 3;
            f.infos.turn_player = 0;
            let bls = envoy(&mut f, 0, location::MZONE, 0);
            let victim = monster(&mut f, 1, location::MZONE, 1, attribute::EARTH);
            f.core.attacker = Some(bls);
            f.core.pre_field[0] = f.cards[bls].fieldid_r;
            f.cards[bls].set_status(status::OPPO_BATTLE, true);
            f.cards[bls].announce_count = 1;
            (f, bls, victim)
        }

        fn holds(f: &mut Field, bls: CardId) -> bool {
            let e = attack_trigger_of(f, bls);
            let ev = Event::new(code::BATTLE_DESTROYING);
            atcon(f, &ctx_for(e, &ev, 0))
        }

        #[test]
        fn it_holds_after_destroying_an_opponents_monster() {
            let (mut f, bls, _) = after_destroying();
            assert!(holds(&mut f, bls));
        }

        /// **`aux.bdocon` is two tests.** The card must still be in the
        /// battle it fought, and it must have fought an *opponent's*
        /// monster — `STATUS_OPPO_BATTLE`.
        #[test]
        fn it_wants_an_opponents_monster_and_the_same_battle() {
            let (mut f, bls, _) = after_destroying();
            f.cards[bls].set_status(status::OPPO_BATTLE, false);
            assert!(!holds(&mut f, bls), "not an opponent's monster");

            let (mut f, bls, _) = after_destroying();
            f.core.pre_field = [f.cards[bls].fieldid_r + 1; 2];
            assert!(!holds(&mut f, bls), "no longer in that battle");
        }

        #[test]
        fn it_wants_to_be_the_attacker() {
            let (mut f, bls, victim) = after_destroying();
            f.core.attacker = Some(victim);
            assert!(!holds(&mut f, bls));
        }

        /// **The once-per-turn flag locks it out.** Using the banish half
        /// and then taking a second attack in the same turn is what this
        /// stops.
        #[test]
        fn the_banish_half_locks_it_out_for_the_turn() {
            let (mut f, bls, _) = after_destroying();
            assert!(holds(&mut f, bls), "the positive sibling");
            api::register_flag_effect(&mut f, bls, CODE, resets::STANDARD_PHASE_END, 0, 1, 0);
            assert!(!holds(&mut f, bls), "the banish half was used this turn");
        }

        /// And the battle system's own answer is asked last.
        #[test]
        fn a_monster_with_no_attacks_left_does_not_hold() {
            let (mut f, bls, _) = after_destroying();
            f.cards[bls].announce_count = 2;
            assert!(!holds(&mut f, bls));
        }

        /// The operation grants the attack, recording the attacker by
        /// identity and naming no target.
        #[test]
        fn resolving_grants_the_chain_attack() {
            let (mut f, bls, _) = after_destroying();
            let e = attack_trigger_of(&f, bls);
            f.core.reason_effect = Some(e);
            let ev = Event::new(code::BATTLE_DESTROYING);
            atop(&mut f, &ctx_for(e, &ev, 0));
            assert!(f.core.chain_attack);
            assert_eq!(f.core.chain_attacker_id, f.cards[bls].fieldid);
            assert_eq!(f.core.chain_attack_target, None, "no target is named");
        }

        /// **`Duel.ChainAttack()` is called with no argument**, so the
        /// battle's current target is not carried into the second attack.
        /// Passing one would pin the new attack to the monster just
        /// destroyed.
        #[test]
        fn it_does_not_reuse_the_battle_target() {
            let (mut f, bls, victim) = after_destroying();
            f.core.attack_target = Some(victim);
            let e = attack_trigger_of(&f, bls);
            f.core.reason_effect = Some(e);
            let ev = Event::new(code::BATTLE_DESTROYING);
            atop(&mut f, &ctx_for(e, &ev, 0));
            assert_eq!(
                f.core.chain_attack_target, None,
                "the monster it just fought is not the new target"
            );
        }
    }
}

//! Chaos Sorcerer (`9596126`) — `cardscripts/c9596126.lua`.
//!
//! ```text
//! Cannot be Normal Summoned or Set. Must first be Special Summoned
//! (from your hand) by banishing 1 LIGHT and 1 DARK monster from your GY.
//! Once per turn: You can target 1 face-up monster on the field; banish
//! it. This card cannot attack the turn you activate this effect.
//! ```
//!
//! The pool's first `EFFECT_SPSUMMON_PROC` — a monster that summons
//! *itself* by its own printed procedure — and the first card to need
//! [`super::aux_select_unselect`], the library's incremental selection.
//!
//! ## Why the materials cannot be one question
//!
//! "1 LIGHT and 1 DARK" is not "any 2 of the LIGHTs and DARKs". A plain
//! `SelectCard` with `min = max = 2` would happily take two LIGHTs,
//! because it settles both picks before checking anything. The library
//! asks one at a time and re-derives the legal set after each answer, so
//! once a LIGHT is chosen the remaining LIGHTs disappear from the offer.
//! `s.rescon` is the rule that drives the narrowing.
//!
//! With `minc == maxc == 2` the loop breaks on `maxc` before
//! `#sg >= minc` can hold, so `finishable` is false on every pass: the
//! player may back out of the *first* pick (`cancelable and #sg == 0`) and
//! not the second. A `-1` on the second question is a retry.
//!
//! ## The banish is a cost paid in the operation
//!
//! `s.spop` banishes with `REASON_COST`, not `REASON_EFFECT`, even though
//! it runs as the procedure's *operation*. That is how a rule summon's
//! procedure works — the target half chooses and records, the operation
//! half pays — and the reason bits are what other cards read.
//!
//! ## Three ways in, and what they share
//!
//! `s.spcon` (offered at all?), `s.sptg` (choose) and `s.spop` (pay) are
//! three separate entries into the same selection. The condition runs the
//! `chk == 0` feasibility search; the target runs the interactive loop and
//! parks its answer in `SetLabelObject`; the operation reads that back. A
//! board the condition accepts and the target then finds nothing on would
//! offer a summon that fizzles, which is why they are tested against each
//! other rather than only on their own.

use super::aux_select_unselect::{self as su, Args};
use crate::board::{location, position};
use crate::card::{attribute, reason};
use crate::effect::{effect_type, flag, Ctx, LabelObject, Yield};
use crate::event::{category, code, CardId, EffectId};
use crate::field::{resets, Field};
use crate::host_question::{hint, hintmsg};

use crate::script_api as api;

pub const CODE: u32 = 9596126;

/// `EFFECT_CANNOT_ATTACK`'s client hint — a library string, not one of
/// this card's own.
const CANNOT_ATTACK_HINT: u64 = 3206;

const MZONE_OR_GRAVE: u32 = (location::MZONE | location::GRAVE) as u32;
const MZONE: u32 = location::MZONE as u32;

pub fn initial_effect(f: &mut Field, c: CardId) {
    api::enable_revive_limit(f, c);
    // Must first be Special Summoned by banishing 1 LIGHT and 1 DARK
    let e0 = api::create_effect(f, c);
    api::set_description(f, e0, api::stringid(CODE, 0));
    api::set_type(f, e0, effect_type::FIELD);
    api::set_property(f, e0, flag::UNCOPYABLE | flag::CANNOT_DISABLE, 0);
    api::set_code(f, e0, code::SPSUMMON_PROC);
    api::set_range(f, e0, u16::from(location::HAND));
    api::set_condition(f, e0, spcon);
    // `spcon(e, nil)` is true: the availability answer, and the
    // renumbering that comes with it. See `api::spcon_nil_guard`.
    api::set_avail_condition(f, e0, api::spcon_nil_guard);
    api::set_target(f, e0, sptg);
    api::set_operation(f, e0, spop);
    api::register_effect(f, c, e0, false);
    // Banish 1 face-up monster on the field
    let e1 = api::create_effect(f, c);
    api::set_description(f, e1, api::stringid(CODE, 1));
    api::set_category(f, e1, category::REMOVE);
    api::set_type(f, e1, effect_type::IGNITION);
    api::set_property(f, e1, flag::CARD_TARGET, 0);
    api::set_range(f, e1, u16::from(location::MZONE));
    api::set_count_limit(f, e1, 1, 0, 0);
    api::set_cost(f, e1, rmcost);
    api::set_target(f, e1, rmtg);
    api::set_operation(f, e1, rmop);
    api::register_effect(f, c, e1, false);
}

/// `s.spcostfilter` — a LIGHT or DARK monster that may be banished as a
/// cost, in a pile `aux.SpElimFilter` says counts.
fn spcostfilter(f: &mut Field, c: CardId) -> bool {
    api::is_attribute(f, c, attribute::LIGHT | attribute::DARK)
        && api::is_able_to_remove_as_cost(f, c, None)
        && api::sp_elim_filter(f, c, true, false)
}

/// `s.rescon` — the selection is legal once it holds one of each
/// attribute **and** leaves a seat for the summon.
///
/// The seat test is why `Duel.GetMZoneCount` has to take a group: the two
/// materials may themselves be on the field, and banishing them is what
/// makes room. Asking about them one at a time would undercount.
fn rescon(
    f: &mut Field,
    sg: &[CardId],
    _e: EffectId,
    tp: u8,
    _mg: &[CardId],
    _c: CardId,
) -> (bool, bool) {
    let res = api::get_mzone_count(f, tp, api::Except::Group(sg), None, None) > 0
        && sg
            .iter()
            .any(|&x| api::is_attribute(f, x, attribute::LIGHT))
        && sg.iter().any(|&x| api::is_attribute(f, x, attribute::DARK));
    // The reference returns one value; `stop` is `nil`, which is false.
    (res, false)
}

/// The arguments both halves of the selection share. `chk == 0` ignores
/// everything from `seltp` on, which is exactly what the reference's
/// trailing `nil`s mean.
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

/// `s.spcon` — may this card summon itself right now?
fn spcon(f: &mut Field, ctx: &Ctx) -> bool {
    // `if c==nil then return true end` — asked about the procedure rather
    // than about a particular card.
    if ctx.card.is_none() {
        return true;
    }
    let tp = api::get_handler_player(f, ctx.reason_effect);
    let g = api::get_matching_group(
        f,
        Some(&spcostfilter),
        tp,
        MZONE_OR_GRAVE,
        0,
        api::Except::None,
    );
    g.len() >= 2
        && api::get_mzone_count(f, tp, api::Except::Group(&g), None, None) > 0
        && su::can_select_unselect(f, &g, ctx.reason_effect, tp, args(tp, false))
}

/// `s.sptg` — choose the two materials.
///
/// `chk` is not read, as the reference's is not: a procedure's target is
/// only ever run to *perform* the choice.
fn sptg(f: &mut Field, ctx: &Ctx, _chk: bool, _chkc: Option<CardId>) -> Yield {
    let tp = ctx.player;
    let g = api::get_matching_group(
        f,
        Some(&spcostfilter),
        tp,
        MZONE_OR_GRAVE,
        0,
        api::Except::None,
    );
    su::select_unselect_group(f, ctx, g, args(tp, true), sptg_done)
}

/// The reference's `if #sg>0 then e:SetLabelObject(sg); return true end`.
fn sptg_done(f: &mut Field, ctx: &Ctx, sg: Vec<CardId>) -> Yield {
    if sg.is_empty() {
        return api::yes(false);
    }
    let g = api::new_group(f, sg);
    api::set_label_object(f, ctx.reason_effect, Some(LabelObject::Group(g)));
    api::yes(true)
}

/// `s.spop` — banish what `s.sptg` chose.
fn spop(f: &mut Field, ctx: &Ctx) -> Yield {
    let Some(g) = api::get_label_object_group(f, ctx.reason_effect) else {
        return api::done();
    };
    let cards = api::group_cards(f, g);
    api::remove(f, cards, position::FACEUP, reason::COST);
    api::done()
}

/// `s.rmcost` — the cost is not a payment but a **promise**: this card
/// cannot attack for the rest of the turn.
///
/// `EFFECT_FLAG_OATH` is what makes that promise survive the effect being
/// negated — an oath effect is released when the chain it was paid for
/// resolves, not when the card leaves.
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
    api::register_effect(f, c, e1, false);
    true
}

/// `aux.FaceupFilter(Card.IsAbleToRemove)`. The library builds this by
/// closing over the inner filter; a card here writes the conjunction out,
/// because the port's filters are closures at the call site already.
fn faceup_able_to_remove(f: &mut Field, c: CardId) -> bool {
    api::is_faceup(f, c) && api::is_able_to_remove(f, c, None, None, None)
}

/// `s.rmtg` — name the monster to banish. Both sides of the field.
fn rmtg(f: &mut Field, ctx: &Ctx, chk: bool, chkc: Option<CardId>) -> Yield {
    let tp = ctx.player;
    if let Some(chkc) = chkc {
        // The `chkc` arm is not the filter above: it asks about location
        // explicitly, because a card target may be anywhere.
        return api::yes(
            api::is_location(f, chkc, u16::from(location::MZONE))
                && api::is_able_to_remove(f, chkc, None, None, None)
                && api::is_faceup(f, chkc),
        );
    }
    if !chk {
        return api::yes(api::is_existing_target(
            f,
            Some(&faceup_able_to_remove),
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
        Some(&faceup_able_to_remove),
        tp,
        MZONE,
        MZONE,
        1,
        1,
        api::Except::None,
    );
    api::suspend(move |f, _ctx| {
        let g = api::selected_targets(f);
        api::set_operation_info(f, 0, category::REMOVE, g, 1, tp, 0);
        api::done()
    })
}

/// `s.rmop` — banish it, if it is still the card that was named.
fn rmop(f: &mut Field, ctx: &Ctx) -> Yield {
    let Some(tc) = api::get_first_target(f) else {
        return api::done();
    };
    if api::is_relate_to_effect(f, tc, ctx.reason_effect)
        && api::is_faceup(f, tc)
        && api::is_monster(f, tc)
    {
        api::remove(f, vec![tc], position::FACEUP, reason::EFFECT);
    }
    api::done()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{card_type, status, Card, CardData};
    use crate::duel::phases;
    use crate::event::Event;
    use crate::field::Message;
    use crate::processor::{Kind, Status};

    /// A vanilla monster of the given attribute, in `loc`.
    fn monster(f: &mut Field, player: u8, loc: u8, seq: u32, attr: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 6000 + seq,
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

    /// Chaos Sorcerer itself, in `loc`.
    fn sorcerer(f: &mut Field, player: u8, loc: u8, seq: u32) -> CardId {
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
    /// branch of the filter that reads the field.
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

    fn procedure_of(f: &Field, cs: CardId) -> EffectId {
        f.cards[cs].field_effect.equal_range(code::SPSUMMON_PROC)[0]
    }

    /// **The procedure answers "available" unconditionally.** The script's
    /// `spcon` opens with `if c==nil then return true end`; the reference
    /// asks it that way from `effect::is_available`, marks the effect
    /// available on the true and renumbers it once — the number that
    /// orders the special-summon menu against another procedure.
    #[test]
    fn the_procedure_answers_available_unconditionally() {
        let (f, cs, _) = board(0, &[]);
        let e = procedure_of(&f, cs);
        let cond = f
            .effects
            .get(e)
            .and_then(|x| x.avail_condition)
            .expect("the availability answer is registered");
        assert!(cond(&f, e), "and it is true");
    }

    fn ignition_of(f: &Field, cs: CardId) -> EffectId {
        f.cards[cs].field_effect.equal_range(0)[0]
    }

    /// Chaos Sorcerer in `tp`'s hand, with the named attributes in the
    /// graveyard and an empty board.
    fn board(tp: u8, grave: &[u32]) -> (Field, CardId, Vec<CardId>) {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        f.infos.turn_player = tp;
        f.infos.phase = phases::MAIN1;
        let cs = sorcerer(&mut f, tp, location::HAND, 0);
        let buried = grave
            .iter()
            .enumerate()
            .map(|(i, &a)| monster(&mut f, tp, location::GRAVE, i as u32, a))
            .collect();
        (f, cs, buried)
    }

    mod registration {
        use super::*;

        /// **Both printed effects, and the two the revive limit adds.**
        #[test]
        fn the_two_printed_effects_and_the_revive_limit() {
            let (f, cs, _) = board(0, &[]);
            let proc = f.effects.get(procedure_of(&f, cs)).expect("procedure");
            assert!(proc.is_type(effect_type::FIELD));
            assert_eq!(proc.code, code::SPSUMMON_PROC);
            assert_eq!(proc.range, u16::from(location::HAND), "from the hand");
            assert!(proc.is_flag(flag::UNCOPYABLE));
            assert!(proc.is_flag(flag::CANNOT_DISABLE));
            assert_eq!(proc.description, api::stringid(CODE, 0));

            let ign = f.effects.get(ignition_of(&f, cs)).expect("ignition");
            assert!(ign.is_type(effect_type::IGNITION));
            assert_eq!(ign.category, category::REMOVE, "the printed category");
            assert!(ign.is_flag(flag::CARD_TARGET));
            assert_eq!(ign.range, u16::from(location::MZONE));
            assert_eq!(ign.count_limit, 1, "once per turn");
            assert_eq!(ign.description, api::stringid(CODE, 1));

            for wanted in [code::UNSUMMONABLE_CARD, code::REVIVE_LIMIT] {
                assert_eq!(
                    f.cards[cs].single_effect.equal_range(wanted).len(),
                    1,
                    "EnableReviveLimit registered {wanted}"
                );
            }
        }
    }

    mod spcon {
        use super::*;

        fn summonable(f: &mut Field, cs: CardId) -> bool {
            let proc = procedure_of(f, cs);
            f.is_spsummonable(cs, proc)
        }

        #[test]
        fn one_of_each_attribute_is_enough() {
            let (mut f, cs, _) = board(0, &[attribute::LIGHT, attribute::DARK]);
            assert!(summonable(&mut f, cs));
        }

        /// **Two of the same attribute is not.** The plain count is two
        /// either way; only the feasibility search tells them apart.
        #[test]
        fn two_of_one_attribute_is_not() {
            let (mut f, cs, _) = board(0, &[attribute::LIGHT, attribute::LIGHT]);
            assert!(!summonable(&mut f, cs));
            let (mut f, cs, _) = board(0, &[attribute::DARK, attribute::DARK]);
            assert!(!summonable(&mut f, cs));
        }

        #[test]
        fn one_material_is_not_enough() {
            let (mut f, cs, _) = board(0, &[attribute::LIGHT]);
            assert!(!summonable(&mut f, cs));
        }

        /// A monster of neither attribute is not a material at all.
        #[test]
        fn the_wrong_attributes_do_not_count() {
            let (mut f, cs, _) = board(0, &[attribute::EARTH, attribute::WIND]);
            assert!(!summonable(&mut f, cs));
            let (mut f, cs, _) = board(0, &[attribute::LIGHT, attribute::EARTH]);
            assert!(!summonable(&mut f, cs), "one of them is not a material");
        }

        /// Without Spirit Elimination the graveyard is the only pile
        /// that counts, so a full row simply refuses.
        #[test]
        fn a_full_row_refuses_when_the_materials_are_buried() {
            let (mut f, cs, _) = board(0, &[attribute::LIGHT, attribute::DARK]);
            for i in 0..5 {
                monster(&mut f, 0, location::MZONE, i, attribute::EARTH);
            }
            assert!(!summonable(&mut f, cs), "nowhere to put it");
        }

        /// **With Spirit Elimination applying the piles swap**: the
        /// materials are the face-up monsters on the field, and the
        /// graveyard stops counting.
        #[test]
        fn spirit_elimination_moves_the_materials_to_the_field() {
            let (mut f, cs, _) = board(0, &[attribute::LIGHT, attribute::DARK]);
            spirit_elimination_applies_to(&mut f, 0);
            assert!(!summonable(&mut f, cs), "the graveyard no longer counts");
            monster(&mut f, 0, location::MZONE, 0, attribute::LIGHT);
            monster(&mut f, 0, location::MZONE, 1, attribute::DARK);
            assert!(summonable(&mut f, cs), "but the field does");
        }

        /// And then the seat matters: **the row is counted with both
        /// materials gone**, because banishing them is what makes room.
        /// Excluding one, or none, undercounts.
        #[test]
        fn a_full_row_is_fine_when_the_materials_are_in_it() {
            let (mut f, cs, _) = board(0, &[]);
            spirit_elimination_applies_to(&mut f, 0);
            monster(&mut f, 0, location::MZONE, 0, attribute::LIGHT);
            monster(&mut f, 0, location::MZONE, 1, attribute::DARK);
            for i in 2..5 {
                monster(&mut f, 0, location::MZONE, i, attribute::EARTH);
            }
            assert!(summonable(&mut f, cs), "banishing the two frees a seat");
        }

        /// **The condition is asked about the procedure itself** — with no
        /// card — before it is asked about a card, and answers yes.
        /// `is_spsummonable` always names a card, so only a direct call
        /// reaches this arm.
        #[test]
        fn asked_about_no_card_it_answers_yes() {
            let (mut f, cs, _) = board(0, &[]);
            let e = procedure_of(&f, cs);
            let ev = Event::new(0);
            let ctx = Ctx {
                reason_effect: e,
                player: 0,
                event: &ev,
                card: None,
                args: &[],
            };
            assert!(spcon(&mut f, &ctx), "an empty graveyard does not matter");
        }

        /// **A material that cannot be banished as a cost is not a
        /// material**, even with the right attribute in the right pile.
        #[test]
        fn a_material_that_cannot_be_banished_does_not_count() {
            let (mut f, cs, buried) = board(0, &[attribute::LIGHT, attribute::DARK]);
            assert!(summonable(&mut f, cs), "the positive sibling");
            let mut e = crate::effect::Effect::new(effect_type::SINGLE, code::CANNOT_REMOVE);
            e.owner = Some(buried[0]);
            e.handler = Some(buried[0]);
            let id = f.new_effect(e);
            f.cards[buried[0]]
                .single_effect
                .insert(code::CANNOT_REMOVE, id);
            f.cards[buried[0]].indexer.insert(id);
            assert!(!summonable(&mut f, cs), "the LIGHT cannot pay");
        }

        /// **The seat is part of the condition, not just of the choice.**
        /// A full row with both materials buried leaves nowhere to put the
        /// summon, and `rescon` is where that is asked.
        #[test]
        fn rescon_refuses_when_there_is_no_seat() {
            let (mut f, cs, _) = board(0, &[attribute::LIGHT, attribute::DARK]);
            assert!(summonable(&mut f, cs), "the positive sibling");
            for i in 0..5 {
                monster(&mut f, 0, location::MZONE, i, attribute::EARTH);
            }
            assert!(!summonable(&mut f, cs), "the row is full");
        }

        /// A face-down monster is not a material, which is
        /// `aux.SpElimFilter`'s `mustbefaceup` and only visible on the
        /// field.
        #[test]
        fn a_face_down_monster_is_not_a_material() {
            let (mut f, cs, _) = board(0, &[]);
            spirit_elimination_applies_to(&mut f, 0);
            monster(&mut f, 0, location::MZONE, 0, attribute::LIGHT);
            let hidden = monster(&mut f, 0, location::MZONE, 1, attribute::DARK);
            assert!(summonable(&mut f, cs), "the positive sibling");
            f.cards[hidden].current.position = position::FACEDOWN_DEFENSE;
            assert!(!summonable(&mut f, cs), "face-down, so not a material");
        }
    }

    /// Drive the procedure's target and then its operation, answering the
    /// selection with `answers` — the **cards** to name, or `None` for
    /// `-1`. Cards rather than indices because the host sorts what it is
    /// offered, so an index is not stable across boards.
    fn summon_with(f: &mut Field, cs: CardId, tp: u8, answers: &[Option<CardId>]) -> Run {
        let e = procedure_of(f, cs);
        f.core.sub_solving_event.push_back(Event::new(0));
        f.emplace(Kind::ExecuteTarget {
            resume: None,
            effect: e,
            player: tp,
            subject: Some(cs),
            args: Vec::new(),
            was_disabled: false,
        });
        drive(f, e, tp, Some(cs), answers, true, true)
    }

    /// Everything a driven run said and was asked.
    #[derive(Default)]
    struct Run {
        /// Each incremental question: who, offered, taken.
        asks: Vec<(u8, Vec<CardId>, Vec<CardId>)>,
        /// Each `SelectCard`: who, and what they were offered.
        card_questions: Vec<(u8, Vec<CardId>)>,
        hints: Vec<(u8, u8, u64)>,
        /// Whether the target half said yes.
        accepted: bool,
    }

    #[allow(clippy::too_many_arguments)]
    fn drive(
        f: &mut Field,
        e: EffectId,
        tp: u8,
        subject: Option<CardId>,
        answers: &[Option<CardId>],
        then_operate: bool,
        // A **procedure's** target reports yes or no, and a no ends the
        // summon. An ordinary card's target reports nothing, so its zero
        // is not a refusal and the operation runs regardless.
        target_reports_a_verdict: bool,
    ) -> Run {
        let mut run = Run::default();
        let mut seen = 0usize;
        let mut next = 0usize;
        let mut operated = !then_operate;
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
                Status::Awaiting => {
                    match f.messages.last() {
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
                                    .unwrap_or_else(|| {
                                        panic!("{card:?} was not offered in {select:?}/{unselect:?}")
                                    });
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
                    }
                }
                Status::End => break,
            }
        }
        run
    }

    fn banished(f: &Field, c: CardId) -> bool {
        f.cards[c].current.location == location::REMOVED
    }

    mod the_summon_procedure {
        use super::*;

        /// **End to end**: the two chosen materials are banished face-up,
        /// as a **cost**, and the target half accepts.
        #[test]
        fn it_banishes_the_two_chosen_materials_as_a_cost() {
            let (mut f, cs, buried) =
                board(0, &[attribute::LIGHT, attribute::DARK, attribute::DARK]);
            let run = summon_with(&mut f, cs, 0, &[Some(buried[0]), Some(buried[2])]);
            assert!(run.accepted, "the procedure's target said yes");
            assert!(banished(&f, buried[0]) && banished(&f, buried[2]));
            assert!(!banished(&f, buried[1]), "and only the two that were named");
            for &m in &[buried[0], buried[2]] {
                assert_eq!(
                    f.cards[m].current.position,
                    position::FACEUP,
                    "banished face-up"
                );
                assert!(
                    f.cards[m].reason & reason::COST != 0,
                    "and as a cost, not as an effect"
                );
            }
        }

        /// The offer narrows: once a LIGHT is taken, the other LIGHT is
        /// no longer a legal answer.
        #[test]
        fn the_second_question_offers_only_the_other_attribute() {
            let (mut f, cs, buried) = board(
                0,
                &[
                    attribute::LIGHT,
                    attribute::DARK,
                    attribute::LIGHT,
                    attribute::DARK,
                ],
            );
            let run = summon_with(&mut f, cs, 0, &[Some(buried[0]), Some(buried[1])]);
            assert_eq!(run.asks.len(), 2);
            let mut offered = run.asks[0].1.clone();
            offered.sort_unstable();
            assert_eq!(offered, buried, "everything to start with");
            // The host sorts what it is shown, so this is the order the
            // list actually arrives in — not the group's.
            assert_eq!(
                run.asks[0].1,
                vec![buried[3], buried[2], buried[1], buried[0]],
                "offered in the host's sort order"
            );
            assert_eq!(
                run.asks[1].1,
                vec![buried[3], buried[1]],
                "only the DARKs, once a LIGHT is taken"
            );
            assert_eq!(run.asks[1].2, vec![buried[0]], "and it may be given back");
        }

        /// **`rescon`'s seat check is the target's only one.** `spcon`
        /// asks about seats itself, before the search, which masks the
        /// same test inside `rescon` on every board the condition lets
        /// through — so this drives the target directly, on a board the
        /// condition would have refused.
        #[test]
        fn the_selection_refuses_when_there_is_no_seat() {
            let (mut f, cs, buried) = board(0, &[attribute::LIGHT, attribute::DARK]);
            for i in 0..5 {
                monster(&mut f, 0, location::MZONE, i, attribute::EARTH);
            }
            let run = summon_with(&mut f, cs, 0, &[Some(buried[0]), Some(buried[1])]);
            assert!(run.asks.is_empty(), "nothing legal, so nothing asked");
            assert!(!run.accepted);
            assert!(!banished(&f, buried[0]) && !banished(&f, buried[1]));
        }

        /// Backing out of the first question summons nothing.
        #[test]
        fn cancelling_refuses_the_summon() {
            let (mut f, cs, buried) = board(0, &[attribute::LIGHT, attribute::DARK]);
            let run = summon_with(&mut f, cs, 0, &[None]);
            assert!(!run.accepted, "the target half said no");
            assert!(!banished(&f, buried[0]) && !banished(&f, buried[1]));
        }

        /// The player is asked, with the banish prompt, once per pick.
        #[test]
        fn each_question_carries_the_banish_prompt() {
            let (mut f, cs, buried) = board(1, &[attribute::LIGHT, attribute::DARK]);
            let run = summon_with(&mut f, cs, 1, &[Some(buried[0]), Some(buried[1])]);
            assert_eq!(
                run.hints
                    .iter()
                    .filter(|h| h.0 == hint::SELECTMSG)
                    .copied()
                    .collect::<Vec<_>>(),
                vec![
                    (hint::SELECTMSG, 1, hintmsg::REMOVE),
                    (hint::SELECTMSG, 1, hintmsg::REMOVE)
                ]
            );
            assert!(run.asks.iter().all(|q| q.0 == 1), "the controller chooses");
        }
    }

    mod the_banish_effect {
        use super::*;
        use crate::chain::Chain;

        /// A target **scan** dereferences `core.reason_effect` for the
        /// targeting test, which the executor sets in play and a direct
        /// call has to set for itself.
        fn as_reason(f: &mut Field, e: EffectId, tp: u8) {
            f.core.reason_effect = Some(e);
            f.core.reason_player = tp;
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

        /// Chaos Sorcerer on the field, with monsters on both sides.
        fn on_field(mine: &[u32], theirs: &[u32]) -> (Field, CardId, Vec<CardId>, Vec<CardId>) {
            on_field_as(0, mine, theirs)
        }

        /// The same, controlled by `tp`. **Every test that reads a player
        /// out of what the card wrote needs this**: run only as player 0
        /// and a literal `0` in the script is indistinguishable from `tp`.
        fn on_field_as(
            tp: u8,
            mine: &[u32],
            theirs: &[u32],
        ) -> (Field, CardId, Vec<CardId>, Vec<CardId>) {
            let mut f = Field::new(8000);
            f.infos.turn_id = 3;
            f.infos.turn_player = tp;
            f.infos.phase = phases::MAIN1;
            let cs = sorcerer(&mut f, tp, location::MZONE, 0);
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
            (f, cs, ours, theirs)
        }

        /// **The cost is a promise, not a payment**: the monster may not
        /// attack for the rest of the turn.
        #[test]
        fn paying_registers_the_cannot_attack_oath() {
            let (mut f, cs, _, _) = on_field(&[], &[attribute::EARTH]);
            let e = ignition_of(&f, cs);
            as_reason(&mut f, e, 0);
            let ev = Event::new(0);
            assert!(rmcost(&mut f, &ctx_for(e, &ev, 0), true), "paid");
            let ids = f.cards[cs].single_effect.equal_range(code::CANNOT_ATTACK);
            assert_eq!(ids.len(), 1, "one prohibition");
            let x = f.effects.get(ids[0]).expect("the effect");
            assert!(
                x.is_flag(flag::OATH),
                "released with the chain, not the card"
            );
            assert!(x.is_flag(flag::CANNOT_DISABLE));
            assert!(x.is_flag(flag::CLIENT_HINT));
            assert_eq!(x.description, CANNOT_ATTACK_HINT);
            assert_ne!(x.reset_flag, 0, "and it expires at the end phase");
        }

        /// **A monster that has already declared an attack may not pay.**
        #[test]
        fn a_monster_that_attacked_cannot_pay() {
            let (mut f, cs, _, _) = on_field(&[], &[attribute::EARTH]);
            let e = ignition_of(&f, cs);
            as_reason(&mut f, e, 0);
            let ev = Event::new(0);
            assert!(rmcost(&mut f, &ctx_for(e, &ev, 0), false), "not yet it has");
            f.cards[cs].attack_announce_count = 1;
            assert!(!rmcost(&mut f, &ctx_for(e, &ev, 0), false), "now it has");
        }

        /// The `chkc` arm is the one that validates a named target, and
        /// it asks about the zone itself.
        #[test]
        fn the_card_target_check_wants_a_face_up_monster_in_the_row() {
            let (mut f, cs, _, theirs) = on_field(&[], &[attribute::EARTH]);
            let e = ignition_of(&f, cs);
            as_reason(&mut f, e, 0);
            let ev = Event::new(0);
            let tc = theirs[0];
            assert_eq!(
                rmtg(&mut f, &ctx_for(e, &ev, 0), false, Some(tc)).finished(),
                Some(1),
                "the positive sibling"
            );
            f.cards[tc].current.position = position::FACEDOWN_DEFENSE;
            assert_eq!(
                rmtg(&mut f, &ctx_for(e, &ev, 0), false, Some(tc)).finished(),
                Some(0),
                "face-down is not a legal target"
            );
        }

        /// **And it asks about the zone**, which the scan's filter does
        /// not have to: a card target may be named anywhere, so the row
        /// test is the `chkc` arm's own.
        #[test]
        fn the_card_target_check_refuses_a_monster_outside_the_row() {
            let (mut f, cs, _, _) = on_field(&[], &[attribute::EARTH]);
            let buried = monster(&mut f, 1, location::GRAVE, 0, attribute::EARTH);
            let e = ignition_of(&f, cs);
            as_reason(&mut f, e, 0);
            let ev = Event::new(0);
            assert_eq!(
                rmtg(&mut f, &ctx_for(e, &ev, 0), false, Some(buried)).finished(),
                Some(0),
                "a face-up banishable monster, but not on the field"
            );
        }

        /// **The scan is two-sided**, and saying so needs a board where
        /// *our* side holds nothing legal — Chaos Sorcerer is itself a
        /// face-up monster in the row, so a test that leaves it face-up
        /// cannot tell a one-sided scan from a two-sided one.
        #[test]
        fn the_scan_reaches_both_sides() {
            let (mut f, cs, _, _) = on_field(&[], &[attribute::EARTH]);
            f.cards[cs].current.position = position::FACEDOWN_DEFENSE;
            let e = ignition_of(&f, cs);
            as_reason(&mut f, e, 0);
            let ev = Event::new(0);
            assert_eq!(
                rmtg(&mut f, &ctx_for(e, &ev, 0), false, None).finished(),
                Some(1),
                "the opponent's monster is the only legal target, and counts"
            );

            let (mut f, cs, _, _) = on_field(&[attribute::EARTH], &[]);
            let e = ignition_of(&f, cs);
            as_reason(&mut f, e, 0);
            assert_eq!(
                rmtg(&mut f, &ctx_for(e, &ev, 0), false, None).finished(),
                Some(1),
                "and so is one of ours"
            );
        }

        /// A face-down monster is not a legal target, so a board holding
        /// only one refuses.
        #[test]
        fn a_board_of_face_down_monsters_refuses() {
            let (mut f, cs, _, theirs) = on_field(&[], &[attribute::EARTH]);
            f.cards[theirs[0]].current.position = position::FACEDOWN_DEFENSE;
            // Chaos Sorcerer itself is face-up in the row, so take it out
            // of the count to leave nothing legal behind.
            f.cards[cs].current.position = position::FACEDOWN_DEFENSE;
            let e = ignition_of(&f, cs);
            as_reason(&mut f, e, 0);
            let ev = Event::new(0);
            assert_eq!(
                rmtg(&mut f, &ctx_for(e, &ev, 0), false, None).finished(),
                Some(0)
            );
        }

        /// Set a resolved chain link up by hand, so the board can be
        /// changed **between** the target and the operation — which is
        /// where the resolution's three tests earn their place.
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

        /// `Duel.Remove` **queues** a `SendTo`; a direct call to the
        /// operation only emplaces it, so the board does not change until
        /// the processor has been let run.
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

        /// **A monster that is no longer the one that was named survives.**
        /// The relation is cut when the card leaves and comes back, and
        /// without this test the check reads as decoration.
        #[test]
        fn a_target_that_lost_its_relation_is_not_banished() {
            let (mut f, cs, _, theirs) = on_field(&[], &[attribute::EARTH]);
            let e = ignition_of(&f, cs);
            ready_to_resolve(&mut f, e, theirs[0], true);
            resolve_now(&mut f, e);
            assert!(banished(&f, theirs[0]), "the positive sibling");

            let (mut f, cs, _, theirs) = on_field(&[], &[attribute::EARTH]);
            let e = ignition_of(&f, cs);
            ready_to_resolve(&mut f, e, theirs[0], false);
            resolve_now(&mut f, e);
            assert!(!banished(&f, theirs[0]), "no relation, no banish");
        }

        /// **A monster turned face-down before the effect resolves
        /// survives** — the card says *face-up* monster, and the target
        /// half is not the last word.
        #[test]
        fn a_target_turned_face_down_is_not_banished() {
            let (mut f, cs, _, theirs) = on_field(&[], &[attribute::EARTH]);
            let e = ignition_of(&f, cs);
            ready_to_resolve(&mut f, e, theirs[0], true);
            f.cards[theirs[0]].current.position = position::FACEDOWN_DEFENSE;
            resolve_now(&mut f, e);
            assert!(!banished(&f, theirs[0]));
        }

        /// **The banish is recorded against the activating player**, not
        /// against player zero. A suite that only ever activates as player
        /// 0 cannot tell those apart, so this one activates as player 1.
        #[test]
        fn the_operation_info_names_the_activating_player() {
            for tp in [0u8, 1u8] {
                let (mut f, cs, _, theirs) = on_field_as(tp, &[], &[attribute::EARTH]);
                let e = ignition_of(&f, cs);
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
                drive(&mut f, e, tp, None, &[Some(theirs[0])], false, false);
                let info = f.core.current_chain[0]
                    .opinfos
                    .get(&category::REMOVE)
                    .expect("the remove category");
                assert_eq!(info.player, tp, "recorded against the activating player");
                assert_eq!(info.count, 1);
                assert_eq!(info.cards.as_deref(), Some(&[theirs[0]][..]));
            }
        }

        /// End to end: the named monster is banished face-up, as an
        /// **effect** this time, and nothing else is.
        #[test]
        fn the_named_monster_is_banished() {
            let (mut f, cs, _, theirs) = on_field(&[attribute::EARTH], &[attribute::EARTH]);
            let e = ignition_of(&f, cs);
            let mut ch = Chain::new(e, Event::new(0));
            ch.triggering_player = 0;
            ch.chain_count = 1;
            ch.chain_id = 11;
            f.core.current_chain.push(ch);
            f.core.chain_solving = true;
            f.core.sub_solving_event.push_back(Event::new(0));
            f.emplace(Kind::ExecuteTarget {
                resume: None,
                effect: e,
                player: 0,
                subject: None,
                args: Vec::new(),
                was_disabled: false,
            });
            let run = drive(&mut f, e, 0, None, &[Some(theirs[0])], true, false);
            assert_eq!(
                run.hints.iter().filter(|h| h.0 == hint::SELECTMSG).count(),
                1,
                "one banish prompt"
            );
            assert_eq!(run.card_questions.len(), 1, "one target question");
            assert!(banished(&f, theirs[0]), "the named monster went");
            assert!(
                f.cards[theirs[0]].reason & reason::EFFECT != 0,
                "as an effect, where the summon's materials were a cost"
            );
            assert!(!banished(&f, cs), "and nothing else did");
        }
    }
}

//! Call of the Haunted — `c97077563.lua`.
//!
//! The thirty-second card, and Premature Burial's opposite number: a
//! **continuous Trap** that revives a monster and is tied to it in both
//! directions. Four effects, and three of them exist to keep the pairing
//! honest.
//!
//! ## Why the summon is split in two
//!
//! ```lua
//! if tc:IsRelateToEffect(e) and Duel.SpecialSummonStep(tc,0,tp,tp,false,false,POS_FACEUP_ATTACK)
//!     and c:IsRelateToEffect(e) then
//!     c:SetCardTarget(tc)
//! end
//! Duel.SpecialSummonComplete()
//! ```
//!
//! `SpecialSummonStep` puts the monster on the field without finishing
//! the summon, so the card target can be recorded **while the monster is
//! there and before anything may react to its arrival**. A single
//! `Duel.SpecialSummon` would open the window first, and whatever came
//! through it could remove the Trap before it ever recorded what it
//! revived — after which the two halves below would have nothing to act
//! on.
//!
//! `SpecialSummonComplete` is called **outside** the `if`, unconditionally.
//! It is a no-op when nothing is pending, which is the reference's own
//! shortcut and is why the script can be written that way.
//!
//! Note also the second `IsRelateToEffect`: the Trap is checked *again*
//! after the monster has arrived, because arriving can destroy it.
//!
//! ## Three effects to destroy the monster, not one
//!
//! `e3` is the one that acts, on `EVENT_LEAVE_FIELD`. But it reads a
//! label off `e2`, which fires earlier on `EVENT_LEAVE_FIELD_P` — the
//! *pre*-leave event, raised before anything moves:
//!
//! ```lua
//! e2:SetOperation(function(e) e:SetLabel(e:GetHandler():IsDisabled() and 1 or 0) end)
//! e3:SetLabelObject(e2)
//! ```
//!
//! A Trap that was **disabled** when it left the field does not destroy
//! the monster. By the time `EVENT_LEAVE_FIELD` is raised the card is
//! already gone and its disabled status can no longer be read, so the
//! answer has to be taken a moment earlier and carried across. `e2` is a
//! one-line effect whose entire job is to be a place to write it down, and
//! `SetLabelObject` is how `e3` finds it.
//!
//! `e2` is `EFFECT_FLAG_CANNOT_DISABLE`, which is the joke that makes it
//! work: the effect that records "I was disabled" is the one thing about
//! the card that cannot be.
//!
//! ## And one to destroy the Trap
//!
//! `e4` is the other direction — a **field** effect ranged to the Spell &
//! Trap row, watching every `EVENT_LEAVE_FIELD` on the board, and firing
//! when the card it revived is among the cards leaving *by destruction*.
//! It reads the event group rather than the board, because by the time it
//! runs the monster is gone.
//!
//! Every call here is to `script_api`, by the rule in `cards/mod.rs`.

use crate::board::{location, position};
use crate::card::reason;
use crate::effect::{effect_type, flag, Ctx, LabelObject, Yield};
use crate::event::{category, code, CardId};
use crate::field::{timing, timings, Field};
use crate::host_question::{hint, hintmsg};
use crate::script_api as api;

pub const CODE: u32 = 97_077_563;

pub fn initial_effect(f: &mut Field, c: CardId) {
    // Activate this card by targeting 1 monster in your GY; Special
    // Summon that target in Attack Position
    let e1 = api::create_effect(f, c);
    api::set_category(f, e1, category::SPECIAL_SUMMON);
    api::set_type(f, e1, effect_type::ACTIVATE);
    api::set_property(f, e1, flag::CARD_TARGET, 0);
    api::set_code(f, e1, code::FREE_CHAIN);
    api::set_target(f, e1, target);
    api::set_operation(f, e1, activate);
    api::set_hint_timing(
        f,
        e1,
        0,
        timing::STANDBY_PHASE | timing::MAIN_END | timings::CHECK_MONSTER_E,
    );
    api::set_description(f, e1, api::stringid(CODE, 0));
    api::register_effect(f, c, e1, false);
    // When this card leaves the field, destroy that monster
    let e2 = api::create_effect(f, c);
    api::set_type(f, e2, effect_type::SINGLE | effect_type::CONTINUOUS);
    api::set_property(f, e2, flag::CANNOT_DISABLE, 0);
    api::set_code(f, e2, code::LEAVE_FIELD_P);
    api::set_operation(f, e2, record_disabled);
    api::register_effect(f, c, e2, false);
    let e3 = api::create_effect(f, c);
    api::set_type(f, e3, effect_type::SINGLE | effect_type::CONTINUOUS);
    api::set_code(f, e3, code::LEAVE_FIELD);
    api::set_operation(f, e3, mondesop);
    api::set_label_object(f, e3, Some(LabelObject::Effect(e2)));
    api::register_effect(f, c, e3, false);
    // When that monster is destroyed, destroy this card
    let e4 = api::create_effect(f, c);
    api::set_type(f, e4, effect_type::FIELD | effect_type::CONTINUOUS);
    api::set_code(f, e4, code::LEAVE_FIELD);
    api::set_range(f, e4, u16::from(location::SZONE));
    api::set_condition(f, e4, selfdescon);
    api::set_operation(f, e4, selfdesop);
    api::register_effect(f, c, e4, false);
}

const GRAVE: u32 = location::GRAVE as u32;

/// `s.spfilter` — a monster this effect could revive **face-up in attack
/// position**, which is the only position it offers.
fn spfilter(e: crate::event::EffectId, tp: u8) -> impl Fn(&mut Field, CardId) -> bool {
    move |f: &mut Field, c: CardId| {
        api::is_can_be_special_summoned_in(f, c, e, 0, tp, false, false, position::FACEUP_ATTACK)
    }
}

fn target(f: &mut Field, ctx: &Ctx, chk: bool, chkc: Option<CardId>) -> Yield {
    let tp = ctx.player;
    let e = ctx.reason_effect;
    let sp = spfilter(e, tp);
    if let Some(c) = chkc {
        // The clauses in the reference's order, which is the reverse of
        // Premature Burial's — controller first, then location.
        return api::yes(
            api::is_controler(f, c, tp)
                && api::is_location(f, c, u16::from(location::GRAVE))
                && sp(f, c),
        );
    }
    if !chk {
        return api::yes(
            api::get_location_count(f, tp, location::MZONE) > 0
                && api::is_existing_target(f, Some(&sp), tp, GRAVE, 0, 1, api::Except::None),
        );
    }
    api::hint(f, hint::SELECTMSG, tp, hintmsg::SPSUMMON);
    api::select_target(f, tp, Some(&sp), tp, GRAVE, 0, 1, 1, api::Except::None);
    api::suspend(move |f, _ctx| {
        let Some(g) = api::selected_targets(f) else {
            return api::done();
        };
        // The announcement names `tp` as the player, where Premature
        // Burial leaves that slot at zero.
        api::set_operation_info(f, 0, category::SPECIAL_SUMMON, Some(g), 1, tp, 0);
        api::done()
    })
}

fn activate(f: &mut Field, ctx: &Ctx) -> Yield {
    let tp = ctx.player;
    let e = ctx.reason_effect;
    let Some(c) = api::get_handler(f, e) else {
        return api::done();
    };
    let Some(tc) = api::get_first_target(f) else {
        return api::done();
    };
    if !api::is_relate_to_effect(f, tc, e) {
        return finish(f);
    }
    api::special_summon_step(f, tc, 0, tp, tp, false, false, position::FACEUP_ATTACK);
    api::suspend(move |f, _ctx| {
        // Both halves of the `and`: the step must have worked, and the
        // Trap must *still* be related — a monster arriving can destroy
        // it, and then there is nothing to record the target on.
        if api::resumed_value(f) != 0 && api::is_relate_to_effect(f, c, e) {
            api::set_card_target(f, c, tc);
        }
        finish(f)
    })
}

/// `Duel.SpecialSummonComplete()` — called whatever happened above.
fn finish(f: &mut Field) -> Yield {
    if !api::special_summon_complete(f) {
        return api::done();
    }
    api::suspend(|_f, _ctx| api::done())
}

/// `e2`'s operation — write down whether the card was disabled, now,
/// while it can still be read.
fn record_disabled(f: &mut Field, ctx: &Ctx) -> Yield {
    let Some(c) = api::get_handler(f, ctx.reason_effect) else {
        return api::done();
    };
    let was = i64::from(api::is_disabled(f, c));
    api::set_label(f, ctx.reason_effect, vec![was]);
    api::done()
}

/// `s.mondesop` — destroy the monster, unless the Trap was disabled when
/// it left.
fn mondesop(f: &mut Field, ctx: &Ctx) -> Yield {
    let Some(recorder) = api::get_label_object_effect(f, ctx.reason_effect) else {
        return api::done();
    };
    if api::get_label_of(f, recorder) != 0 {
        return api::done();
    }
    let Some(c) = api::get_handler(f, ctx.reason_effect) else {
        return api::done();
    };
    let Some(tc) = api::get_first_card_target(f, c) else {
        return api::done();
    };
    if api::is_location(f, tc, u16::from(location::MZONE)) {
        api::destroy(f, vec![tc], reason::EFFECT);
    }
    api::done()
}

/// `s.selfdescon` — is the monster we revived among the cards leaving,
/// and is it leaving because it was **destroyed**?
fn selfdescon(f: &mut Field, ctx: &Ctx) -> bool {
    let Some(c) = api::get_handler(f, ctx.reason_effect) else {
        return false;
    };
    let Some(tc) = api::get_first_card_target(f, c) else {
        return false;
    };
    api::event_cards(ctx).contains(&tc) && api::is_reason(f, tc, reason::DESTROY)
}

fn selfdesop(f: &mut Field, ctx: &Ctx) -> Yield {
    let Some(c) = api::get_handler(f, ctx.reason_effect) else {
        return api::done();
    };
    api::destroy(f, vec![c], reason::EFFECT);
    api::done()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{card_type, status, Card, CardData};
    use crate::chain::Chain;
    use crate::duel::phases;
    use crate::effect::Effect;
    use crate::event::{EffectId, Event};
    use crate::field::Message;
    use crate::processor::{Kind, Status};

    fn monster(f: &mut Field, owner: u8, code_: u32, loc: u8, seq: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: code_,
                type_: card_type::MONSTER | card_type::NORMAL,
                level: 4,
                attack: 1700,
                defense: 1000,
                ..Default::default()
            },
            owner,
        );
        c.current.controller = owner;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(owner, id, loc, seq, false);
        if loc == location::MZONE {
            f.cards[id].current.position = position::FACEUP_ATTACK;
        }
        id
    }

    /// `tp`'s Main Phase 1 with the Trap face-up in its row, `mine` in
    /// `tp`'s graveyard and `theirs` in the opponent's.
    fn field_as(tp: u8, mine: u32, theirs: u32) -> (Field, CardId, Vec<CardId>, Vec<CardId>) {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        f.infos.turn_player = tp;
        f.infos.phase = phases::MAIN1;
        let mut d = Card::with_data(crate::cards::card_data(CODE).expect("printed data"), tp);
        d.current.controller = tp;
        d.set_status(status::EFFECT_ENABLED, true);
        let coth = f.new_card(d);
        f.add_card(tp, coth, location::SZONE, 0, false);
        f.cards[coth].current.position = position::FACEUP;
        f.initialize_card(coth);
        let ours = (0..mine)
            .map(|i| monster(&mut f, tp, 7_000 + i, location::GRAVE, i))
            .collect();
        let theirs = (0..theirs)
            .map(|i| monster(&mut f, 1 - tp, 8_000 + i, location::GRAVE, i))
            .collect();
        (f, coth, ours, theirs)
    }

    fn field(mine: u32, theirs: u32) -> (Field, CardId, Vec<CardId>, Vec<CardId>) {
        field_as(0, mine, theirs)
    }

    fn e1_of(f: &Field, c: CardId) -> EffectId {
        f.cards[c].field_effect.equal_range(code::FREE_CHAIN)[0]
    }
    fn e2_of(f: &Field, c: CardId) -> EffectId {
        f.cards[c].single_effect.equal_range(code::LEAVE_FIELD_P)[0]
    }
    fn e3_of(f: &Field, c: CardId) -> EffectId {
        f.cards[c].single_effect.equal_range(code::LEAVE_FIELD)[0]
    }
    fn e4_of(f: &Field, c: CardId) -> EffectId {
        f.cards[c].field_effect.equal_range(code::LEAVE_FIELD)[0]
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

    /// Ask the target function directly, with what `ExecuteTarget` would
    /// have set: `core.reason_effect`, which the scan's targeting mode
    /// reads.
    fn ask_target(f: &mut Field, e: EffectId, tp: u8, chkc: Option<CardId>) -> bool {
        f.core.reason_effect = Some(e);
        let ev = Event::new(code::FREE_CHAIN);
        let ctx = ctx_for(e, &ev, tp);
        target(f, &ctx, false, chkc).finished().unwrap_or(0) != 0
    }

    #[derive(Default)]
    struct Run {
        asked: Vec<(u8, u8, u8, Vec<CardId>)>,
        hints: Vec<(u8, u8, u64)>,
        /// `MSG_CARD_TARGET`, as (owner seat, target seat).
        card_targets: Vec<(Seat, Seat)>,
    }

    /// Controller, location, sequence — the part of a `LocInfo` a test can
    /// assert on.
    type Seat = (u8, u8, u32);

    fn resolve_as(f: &mut Field, tp: u8, e: EffectId, pick: usize) -> Run {
        let mut ch = Chain::new(e, Event::new(code::FREE_CHAIN));
        ch.triggering_player = tp;
        ch.chain_count = 1;
        ch.chain_id = 11;
        f.core.current_chain.push(ch);
        f.core.chain_solving = true;
        let handler = api::get_handler(f, e).expect("an activating card");
        f.cards[handler].create_chain_relation(e, 11);
        f.core
            .sub_solving_event
            .push_back(Event::new(code::FREE_CHAIN));
        f.emplace(Kind::ExecuteTarget {
            resume: None,
            effect: e,
            player: tp,
            subject: None,
            args: Vec::new(),
            was_disabled: false,
        });
        let mut run = Run::default();
        let mut seen = 0usize;
        let mut operated = false;
        for _ in 0..8192 {
            while seen < f.messages.len() {
                match &f.messages[seen] {
                    Message::Hint {
                        kind,
                        player,
                        value,
                    } => run.hints.push((*kind, *player, *value)),
                    Message::CardTarget { owner, target } => run.card_targets.push((
                        (owner.controller, owner.location, owner.sequence),
                        (target.controller, target.location, target.sequence),
                    )),
                    _ => {}
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
                        f.core
                            .sub_solving_event
                            .push_back(Event::new(code::FREE_CHAIN));
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
                        player,
                        min,
                        max,
                        cards,
                        ..
                    }) => {
                        run.asked.push((*player, *min, *max, cards.clone()));
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
                            .find(|s| flag & (1 << s) == 0)
                            .expect("a free monster seat");
                        f.core.returns.set_i8(0, pl as i8);
                        f.core.returns.set_i8(1, location::MZONE as i8);
                        f.core.returns.set_i8(2, seq as i8);
                    }
                    other => panic!("unexpected question: {other:?}"),
                },
                other => panic!("stopped with {other:?}"),
            }
        }
        run
    }

    fn resolve(f: &mut Field, e: EffectId, pick: usize) -> Run {
        resolve_as(f, 0, e, pick)
    }

    /// **Four printed effects**, and the three that keep the pairing
    /// honest are each shaped differently: two single continuous effects
    /// on the two leave-the-field events, and a *field* effect ranged to
    /// the Spell & Trap row watching the whole board.
    #[test]
    fn the_script_registers_an_activation_and_three_tie_keeping_effects() {
        let (f, coth, _, _) = field(1, 0);
        let e1 = f.effects.get(e1_of(&f, coth)).unwrap();
        assert!(e1.is_type(effect_type::ACTIVATE));
        assert_eq!(e1.category, category::SPECIAL_SUMMON);
        assert!(e1.is_flag(flag::CARD_TARGET));
        assert!(e1.cost.is_none(), "no cost, unlike Premature Burial");
        assert_eq!(
            e1.hint_timing,
            [
                0,
                timing::STANDBY_PHASE | timing::MAIN_END | timings::CHECK_MONSTER_E
            ],
            "the opponent's slot, not its own"
        );

        let e2 = f.effects.get(e2_of(&f, coth)).unwrap();
        assert!(e2.is_type(effect_type::SINGLE));
        assert!(e2.is_type(effect_type::CONTINUOUS));
        assert!(
            e2.is_flag(flag::CANNOT_DISABLE),
            "the effect that records being disabled cannot be"
        );

        let e3id = e3_of(&f, coth);
        let e3 = f.effects.get(e3id).unwrap();
        assert!(e3.is_type(effect_type::SINGLE));
        assert!(e3.is_type(effect_type::CONTINUOUS));
        assert_eq!(
            e3.label_object,
            Some(LabelObject::Effect(e2_of(&f, coth))),
            "it remembers the recorder"
        );

        let e4 = f.effects.get(e4_of(&f, coth)).unwrap();
        assert!(e4.is_type(effect_type::FIELD));
        assert!(e4.is_type(effect_type::CONTINUOUS));
        assert_eq!(e4.range, u16::from(location::SZONE));
        assert!(e4.condition.is_some());
    }

    /// **A monster of its own, and a seat to put it in.**
    #[test]
    fn it_needs_a_monster_of_its_own_and_room_for_it() {
        let (mut f, coth, _, _) = field(0, 0);
        let e = e1_of(&f, coth);
        assert!(!ask_target(&mut f, e, 0, None));
        let (mut f, coth, _, _) = field(0, 2);
        let e = e1_of(&f, coth);
        assert!(!ask_target(&mut f, e, 0, None), "not their graveyard");
        let (mut f, coth, _, _) = field(1, 0);
        for i in 0..5 {
            monster(&mut f, 0, 9_000 + i, location::MZONE, i);
        }
        let e = e1_of(&f, coth);
        assert!(!ask_target(&mut f, e, 0, None), "a full row");
        let (mut f, coth, _, _) = field(1, 0);
        let e = e1_of(&f, coth);
        assert!(ask_target(&mut f, e, 0, None));
    }

    /// **The same, as player 1.**
    #[test]
    fn the_scan_follows_the_activating_player() {
        let (mut f, coth, _, _) = field_as(1, 0, 2);
        let e = e1_of(&f, coth);
        assert!(!ask_target(&mut f, e, 1, None));
        let (mut f, coth, _, _) = field_as(1, 2, 0);
        let e = e1_of(&f, coth);
        assert!(ask_target(&mut f, e, 1, None));
    }

    /// **`chkc` asks three questions**, each with a board where only it
    /// refuses, plus their shared positive sibling.
    #[test]
    fn the_target_check_wants_its_own_graveyard_and_a_summonable_monster() {
        let (mut f, coth, mine, theirs) = field(1, 1);
        let e = e1_of(&f, coth);
        assert!(ask_target(&mut f, e, 0, Some(mine[0])), "positive sibling");
        assert!(!ask_target(&mut f, e, 0, Some(theirs[0])), "theirs");

        // Its own and summonable, but in the hand rather than a graveyard.
        let (mut f, coth, _, _) = field(0, 0);
        let in_hand = monster(&mut f, 0, 9_400, location::HAND, 0);
        let e = e1_of(&f, coth);
        let sp = spfilter(e, 0);
        assert!(sp(&mut f, in_hand), "summonable from there");
        assert!(!ask_target(&mut f, e, 0, Some(in_hand)), "but not a grave");

        // And the filter's `nocheck` is false, which is only visible from
        // a hand or deck: a card under `EFFECT_REVIVE_LIMIT` that has not
        // completed its procedure is refused from there, and would be
        // waved through if the check were skipped.
        let mut lim = Effect::new(effect_type::SINGLE, code::REVIVE_LIMIT);
        lim.owner = Some(in_hand);
        lim.handler = Some(in_hand);
        let id = f.new_effect(lim);
        f.cards[in_hand]
            .single_effect
            .insert(code::REVIVE_LIMIT, id);
        f.cards[in_hand].indexer.insert(id);
        assert!(
            !sp(&mut f, in_hand),
            "a revive limit refuses it from the hand"
        );

        // Its own graveyard, but not summonable.
        let (mut f, coth, mine, _) = field(1, 0);
        f.cards[mine[0]].set_status(status::FORBIDDEN, true);
        let e = e1_of(&f, coth);
        assert!(!ask_target(&mut f, e, 0, Some(mine[0])));
    }

    /// **The filter asks about attack position specifically**, not
    /// face-up in general.
    ///
    /// The two differ only when something forces a position. A
    /// `FORCE_SPSUMMON_POSITION` aura permitting *defence* leaves
    /// `FACEUP_ATTACK` with nothing and `FACEUP` with defence — so under
    /// it this card can revive nothing, while a card asking about
    /// `POS_FACEUP` still could. That is the whole content of the
    /// argument, and nothing else in the port reads it.
    #[test]
    fn the_filter_asks_for_face_up_attack_and_not_merely_face_up() {
        let (mut f, coth, mine, _) = field(1, 0);
        let e = e1_of(&f, coth);
        let tc = mine[0];
        let sp = spfilter(e, 0);
        assert!(sp(&mut f, tc), "the positive sibling");

        // An aura on player 0 forcing face-up **defence**.
        let mut source = Card::new(1, 0);
        source.current.controller = 0;
        source.current.location = location::MZONE;
        source.current.position = position::FACEUP_ATTACK;
        source.set_status(status::EFFECT_ENABLED, true);
        let source = f.new_card(source);
        let mut aura = Effect::new(effect_type::FIELD, code::FORCE_SPSUMMON_POSITION);
        aura.owner = Some(source);
        aura.handler = Some(source);
        aura.effect_owner = 0;
        aura.flag[0] |= flag::PLAYER_TARGET;
        aura.range = u16::from(location::MZONE);
        aura.s_range = u16::from(location::MZONE);
        aura.value = i64::from(position::FACEUP_DEFENSE);
        let id = f.new_effect(aura);
        f.field_effects
            .aura
            .insert(code::FORCE_SPSUMMON_POSITION, id);
        f.field_effects.indexer.insert(id);

        assert!(
            !sp(&mut f, tc),
            "attack position is not available, so this card cannot revive"
        );
        assert!(
            api::is_can_be_special_summoned_in(&mut f, tc, e, 0, 0, false, false, position::FACEUP),
            "but face-up in general still is"
        );
    }

    /// **One monster, chosen by the activating player, from its own
    /// graveyard**, prefaced by the special-summon prompt.
    #[test]
    fn it_asks_its_controller_for_one_of_its_own() {
        let (mut f, coth, mine, _) = field(2, 2);
        let e = e1_of(&f, coth);
        let run = resolve(&mut f, e, 0);
        assert_eq!(run.asked.len(), 1);
        let (player, min, max, offered) = &run.asked[0];
        assert_eq!(*player, 0);
        assert_eq!((*min, *max), (1, 1));
        assert_eq!(offered, &vec![mine[1], mine[0]], "its own, top first");
        assert!(run
            .hints
            .iter()
            .any(|&(k, p, v)| (k, p, v) == (hint::SELECTMSG, 0, hintmsg::SPSUMMON)));
    }

    /// **The announcement names the player.** Where Premature Burial
    /// leaves that slot at zero, this one writes `tp` — so a suite that
    /// only ever activates as player 0 cannot tell them apart, and this
    /// activates as player 1.
    #[test]
    fn the_announcement_names_the_summoning_player() {
        let (mut f, coth, _, _) = field_as(1, 2, 0);
        let e = e1_of(&f, coth);
        let run = resolve_as(&mut f, 1, e, 0);
        let chosen = run.asked[0].3[0];
        let ops = &f.core.current_chain[0].opinfos;
        let sp = ops
            .get(&category::SPECIAL_SUMMON)
            .expect("the summon category");
        assert_eq!(sp.cards.as_deref(), Some(&[chosen][..]));
        assert_eq!(sp.count, 1);
        assert_eq!(sp.player, 1, "the activating player, not a literal zero");
    }

    /// **It revives face-up in attack position and records what it
    /// revived** — the card target, which is what both leave-the-field
    /// halves read later.
    #[test]
    fn it_revives_the_chosen_monster_and_remembers_it() {
        let (mut f, coth, mine, _) = field(2, 0);
        let e = e1_of(&f, coth);
        let run = resolve(&mut f, e, 0);
        let chosen = run.asked[0].3[0];
        let left_behind = *mine.iter().find(|&&c| c != chosen).unwrap();
        assert_eq!(f.cards[chosen].current.location, location::MZONE);
        assert_eq!(f.cards[chosen].current.controller, 0);
        assert_eq!(f.cards[chosen].current.position, position::FACEUP_ATTACK);
        assert_eq!(f.cards[left_behind].current.location, location::GRAVE);
        // Remembered, both directions — and **not** by an equip: the Trap
        // stays in its row unattached.
        assert_eq!(api::get_first_card_target(&f, coth), Some(chosen));
        assert!(f.cards[chosen].effect_target_owner.contains(&coth));
        assert_eq!(f.cards[coth].equiping_target, None, "not an equip");
        // And announced, both sides as seats.
        assert_eq!(run.card_targets.len(), 1);
        let (owner, target_loc) = run.card_targets[0];
        assert_eq!(owner, (0, location::SZONE, 0));
        assert_eq!(
            target_loc,
            (0, location::MZONE, f.cards[chosen].current.sequence)
        );
    }

    /// Drive the operation alone on a chain the caller staged.
    fn operate_only(f: &mut Field, e: EffectId, tp: u8) {
        f.core
            .sub_solving_event
            .push_back(Event::new(code::FREE_CHAIN));
        f.emplace(Kind::ExecuteOperation {
            resume: None,
            effect: e,
            player: tp,
            subject: None,
            args: Vec::new(),
            was_disabled: false,
        });
        for _ in 0..4096 {
            match f.process() {
                Status::Continue => {
                    if f.core.units.is_empty() && f.core.subunits.is_empty() {
                        return;
                    }
                }
                Status::Awaiting => match f.messages.last() {
                    Some(Message::SelectPlace { player, flag, .. }) => {
                        let (pl, flag) = (*player, *flag);
                        let seq = (0..5u32).find(|s| flag & (1 << s) == 0).unwrap();
                        f.core.returns.set_i8(0, pl as i8);
                        f.core.returns.set_i8(1, location::MZONE as i8);
                        f.core.returns.set_i8(2, seq as i8);
                    }
                    other => panic!("unexpected question: {other:?}"),
                },
                other => panic!("stopped with {other:?}"),
            }
        }
        panic!("did not settle");
    }

    fn staged(relate_trap: bool, relate_monster: bool) -> (Field, CardId, CardId, EffectId) {
        let (mut f, coth, ours, _) = field(1, 0);
        let e = e1_of(&f, coth);
        let tc = ours[0];
        let mut ch = Chain::new(e, Event::new(code::FREE_CHAIN));
        ch.triggering_player = 0;
        ch.chain_count = 1;
        ch.chain_id = 11;
        ch.target_cards = vec![tc];
        f.core.current_chain.push(ch);
        f.core.chain_solving = true;
        if relate_trap {
            f.cards[coth].create_chain_relation(e, 11);
        }
        if relate_monster {
            f.cards[tc].create_chain_relation(e, 11);
        }
        (f, coth, tc, e)
    }

    /// **The monster must be related before the summon, and the Trap
    /// after it.** The two checks sit on opposite sides of
    /// `SpecialSummonStep` in the reference, and the asymmetry is the
    /// point: an unrelated monster is not summoned at all, while an
    /// unrelated Trap is summoned for and then declines to remember it.
    #[test]
    fn the_monster_is_checked_before_the_summon_and_the_trap_after() {
        // Both related: revived and remembered.
        let (mut f, coth, tc, e) = staged(true, true);
        operate_only(&mut f, e, 0);
        assert_eq!(f.cards[tc].current.location, location::MZONE);
        assert_eq!(api::get_first_card_target(&f, coth), Some(tc));

        // The monster unrelated: not summoned at all.
        let (mut f, coth, tc, e) = staged(true, false);
        operate_only(&mut f, e, 0);
        assert_eq!(f.cards[tc].current.location, location::GRAVE);
        assert_eq!(api::get_first_card_target(&f, coth), None);

        // The Trap unrelated: the monster **is** revived, and is not
        // remembered — which is the whole reason the summon is split.
        let (mut f, coth, tc, e) = staged(false, true);
        operate_only(&mut f, e, 0);
        assert_eq!(
            f.cards[tc].current.location,
            location::MZONE,
            "summoned anyway"
        );
        assert_eq!(
            api::get_first_card_target(&f, coth),
            None,
            "but not remembered"
        );
    }

    /// **The recorder writes whether the card was disabled**, on the
    /// pre-leave event while that can still be read.
    #[test]
    fn the_recorder_writes_the_disabled_status() {
        for disabled in [false, true] {
            let (mut f, coth, _, _) = field(1, 0);
            let e2 = e2_of(&f, coth);
            f.cards[coth].set_status(status::DISABLED, disabled);
            let ev = Event::new(code::LEAVE_FIELD_P);
            record_disabled(&mut f, &ctx_for(e2, &ev, 0));
            assert_eq!(
                api::get_label_of(&f, e2),
                i64::from(disabled),
                "disabled {disabled}"
            );
        }
    }

    /// **A Trap that was disabled when it left does not destroy the
    /// monster**, and one that was not does.
    ///
    /// Reached here by driving the pair directly: on the harness the
    /// monster always leaves first, so by the time `e3` runs it is no
    /// longer in a Monster Zone and the last clause refuses — which is
    /// correct, and is why the destroy itself is not exercised by a game.
    #[test]
    fn the_leave_half_respects_the_recorded_disabled_status() {
        for (disabled, expect) in [(false, true), (true, false)] {
            let (mut f, coth, _, _) = field(2, 0);
            let e1 = e1_of(&f, coth);
            let run = resolve(&mut f, e1, 0);
            let chosen = run.asked[0].3[0];
            assert_eq!(f.cards[chosen].current.location, location::MZONE);

            f.cards[coth].set_status(status::DISABLED, disabled);
            let e2 = e2_of(&f, coth);
            let pre = Event::new(code::LEAVE_FIELD_P);
            record_disabled(&mut f, &ctx_for(e2, &pre, 0));

            let e3 = e3_of(&f, coth);
            let ev = Event::new(code::LEAVE_FIELD);
            mondesop(&mut f, &ctx_for(e3, &ev, 0));
            let queued = f
                .core
                .subunits
                .iter()
                .any(|u| matches!(u.kind, Kind::Destroy { .. }));
            assert_eq!(queued, expect, "disabled {disabled}");
        }
    }

    /// **And only while the monster is still in a Monster Zone**, which
    /// is the clause a harness game always lands on.
    #[test]
    fn the_leave_half_leaves_a_monster_that_has_already_gone() {
        let (mut f, coth, _, _) = field(2, 0);
        let e1 = e1_of(&f, coth);
        let run = resolve(&mut f, e1, 0);
        let chosen = run.asked[0].3[0];
        f.move_card(0, chosen, location::GRAVE, 0, false);
        let e2 = e2_of(&f, coth);
        let pre = Event::new(code::LEAVE_FIELD_P);
        record_disabled(&mut f, &ctx_for(e2, &pre, 0));
        let e3 = e3_of(&f, coth);
        let ev = Event::new(code::LEAVE_FIELD);
        mondesop(&mut f, &ctx_for(e3, &ev, 0));
        assert!(f.core.subunits.is_empty());
    }

    /// **It destroys the monster it remembers, by effect.**
    #[test]
    fn the_leave_half_destroys_the_monster_it_remembers() {
        let (mut f, coth, _, _) = field(2, 0);
        let e1 = e1_of(&f, coth);
        let run = resolve(&mut f, e1, 0);
        let chosen = run.asked[0].3[0];
        f.core.current_chain.clear();
        f.core.chain_solving = false;
        let e2 = e2_of(&f, coth);
        let pre = Event::new(code::LEAVE_FIELD_P);
        record_disabled(&mut f, &ctx_for(e2, &pre, 0));
        let e3 = e3_of(&f, coth);
        let ev = Event::new(code::LEAVE_FIELD);
        mondesop(&mut f, &ctx_for(e3, &ev, 0));
        match &f.core.subunits[0].kind {
            Kind::Destroy {
                targets,
                reason: why,
                ..
            } => {
                assert_eq!(
                    f.group(*targets).iter().copied().collect::<Vec<_>>(),
                    vec![chosen]
                );
                assert_ne!(why & reason::EFFECT, 0);
            }
            other => panic!("expected a Destroy: {other:?}"),
        }
    }

    /// **The self-destruct fires only for its own monster, and only when
    /// that monster was destroyed.** Four boards: the positive one, a
    /// different card leaving, the right card leaving for another reason,
    /// and nothing remembered at all.
    #[test]
    fn the_self_destruct_wants_its_own_monster_destroyed() {
        let (mut f, coth, _, _) = field(2, 0);
        let e1 = e1_of(&f, coth);
        let run = resolve(&mut f, e1, 0);
        let chosen = run.asked[0].3[0];
        let bystander = monster(&mut f, 0, 9_800, location::MZONE, 4);
        let e4 = e4_of(&f, coth);

        let ask = |f: &mut Field, cards: Vec<CardId>, reason_of: Option<(CardId, u32)>| {
            if let Some((c, why)) = reason_of {
                f.cards[c].reason = why;
            }
            let mut ev = Event::new(code::LEAVE_FIELD);
            ev.event_cards = cards;
            selfdescon(f, &ctx_for(e4, &ev, 0))
        };
        assert!(
            ask(&mut f, vec![chosen], Some((chosen, reason::DESTROY))),
            "its own monster, destroyed"
        );
        assert!(
            !ask(&mut f, vec![bystander], Some((bystander, reason::DESTROY))),
            "someone else's departure"
        );
        assert!(
            !ask(&mut f, vec![chosen], Some((chosen, reason::EFFECT))),
            "left, but not destroyed"
        );

        // Nothing remembered: a Trap that never resolved.
        let (mut f, coth, _, _) = field(1, 0);
        let e4 = e4_of(&f, coth);
        let mut ev = Event::new(code::LEAVE_FIELD);
        ev.event_cards = vec![coth];
        assert!(!selfdescon(&mut f, &ctx_for(e4, &ev, 0)));
    }

    /// **The self-destruct destroys the Trap, not the monster.**
    #[test]
    fn the_self_destruct_destroys_the_trap() {
        let (mut f, coth, _, _) = field(2, 0);
        let e1 = e1_of(&f, coth);
        let run = resolve(&mut f, e1, 0);
        let chosen = run.asked[0].3[0];
        let e4 = e4_of(&f, coth);
        let ev = Event::new(code::LEAVE_FIELD);
        selfdesop(&mut f, &ctx_for(e4, &ev, 0));
        match &f.core.subunits[0].kind {
            Kind::Destroy {
                targets,
                reason: why,
                ..
            } => {
                let got: Vec<CardId> = f.group(*targets).iter().copied().collect();
                assert_eq!(got, vec![coth], "itself, not {chosen}");
                assert_ne!(why & reason::EFFECT, 0);
            }
            other => panic!("expected a Destroy: {other:?}"),
        }
    }

    /// **The reader finds the recorder through the label object**, and
    /// gives up when it points at nothing.
    #[test]
    fn the_reader_needs_the_recorder() {
        let (mut f, coth, _, _) = field(2, 0);
        let e1 = e1_of(&f, coth);
        resolve(&mut f, e1, 0);
        let e3 = e3_of(&f, coth);
        assert_eq!(api::get_label_object_effect(&f, e3), Some(e2_of(&f, coth)));
        // Point it at a card instead and the reader finds nothing.
        api::set_label_object(&mut f, e3, Some(LabelObject::Card(coth)));
        assert_eq!(
            api::get_label_object_effect(&f, e3),
            None,
            "a card is not an effect"
        );
        let ev = Event::new(code::LEAVE_FIELD);
        mondesop(&mut f, &ctx_for(e3, &ev, 0));
        assert!(f.core.subunits.is_empty(), "and does nothing");
    }

    /// **`SpecialSummonComplete` is a no-op with nothing pending**, which
    /// is what lets the script call it outside its own `if`.
    #[test]
    fn completing_nothing_queues_nothing() {
        let mut f = Field::new(8000);
        f.core.operated_set = vec![0];
        assert!(!api::special_summon_complete(&mut f));
        assert!(f.core.subunits.is_empty());
        assert!(f.core.operated_set.is_empty(), "and clears the set");
        assert_eq!(f.core.returns.at_i32(0), 0);
    }

    /// An unused binding kept for the reader: `Effect` is imported for
    /// the registration test's type assertions.
    #[allow(dead_code)]
    fn _uses_effect(_: &Effect) {}

    /// **A summon that fails records nothing.** With no seat left by the
    /// time it resolves, `SpecialSummonStep` answers zero and the Trap
    /// does not go on to remember a monster still in the graveyard.
    #[test]
    fn a_failed_summon_records_nothing() {
        let (mut f, coth, tc, e) = staged(true, true);
        for i in 0..5 {
            monster(&mut f, 0, 9_000 + i, location::MZONE, i);
        }
        operate_only(&mut f, e, 0);
        assert_eq!(f.cards[tc].current.location, location::GRAVE);
        assert_eq!(api::get_first_card_target(&f, coth), None);
    }

    /// **The summon is made with the checks on.** A monster carrying an
    /// `EFFECT_SPSUMMON_CONDITION` that refuses is not revived, which is
    /// only true because `SpecialSummonStep` is called with `nocheck`
    /// false.
    #[test]
    fn a_monster_that_refuses_to_be_summoned_is_not_revived() {
        fn refuses(_e: &Effect, _f: &Field, _ctx: &Ctx) -> i64 {
            0
        }
        let (mut f, coth, tc, e) = staged(true, true);
        let mut cond = Effect::new(effect_type::SINGLE, code::SPSUMMON_CONDITION);
        cond.owner = Some(tc);
        cond.handler = Some(tc);
        let id = f.new_effect(cond);
        f.effects.get_mut(id).unwrap().value_fn = Some(refuses);
        f.cards[tc]
            .single_effect
            .insert(code::SPSUMMON_CONDITION, id);
        f.cards[tc].indexer.insert(id);

        operate_only(&mut f, e, 0);
        assert_eq!(f.cards[tc].current.location, location::GRAVE);
        assert_eq!(api::get_first_card_target(&f, coth), None);
    }
}

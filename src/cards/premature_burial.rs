//! Premature Burial — `c70828912.lua`.
//!
//! The thirty-first card, and the pool's first **equip**. It pays 800
//! life, special summons a monster from its controller's graveyard, and
//! then attaches itself to what it revived — so that destroying the
//! Spell destroys the monster too.
//!
//! ## Two effects, and the second is why the card is famous
//!
//! `e1` is the activation. `e2` is a continuous single effect on
//! `EVENT_LEAVE_FIELD` with **no condition** — it fires whenever the
//! Spell leaves the field, and decides for itself whether to act. That
//! is the destroy-the-monster half, and it reads two things:
//!
//! ```lua
//! if c:IsReason(REASON_DESTROY) and tc and tc:IsLocation(LOCATION_MZONE) then
//! ```
//!
//! **Destroyed, specifically.** A Premature Burial returned to the hand,
//! banished or sent to the graveyard by anything that is not a
//! destruction leaves the monster alone. And `tc` comes from
//! `GetFirstCardTarget`, not from the chain — the chain is long gone by
//! the time this fires, and what the card remembers is the equip
//! relationship `Duel.Equip` recorded on it.
//!
//! ## The summon must succeed before the equip is attempted
//!
//! ```lua
//! if Duel.SpecialSummon(tc,0,tp,tp,false,false,POS_FACEUP_ATTACK)==0 then return end
//! Duel.Equip(tp,c,tc)
//! ```
//!
//! `SpecialSummon` answers with a **count**, not a flag, and zero means
//! the summon was refused — at which point the card stops rather than
//! equipping itself to a monster that is still in the graveyard.
//!
//! ## The equip limit is a function, and it names a card
//!
//! ```lua
//! function s.eqlimit(e,c) return e:GetLabelObject()==c end
//! ```
//!
//! `EFFECT_EQUIP_LIMIT` is asked, once per adjust, **about the equip
//! card's current target**: may this card stay attached to *that*? The
//! answer here is "only to the monster I revived", which is what stops
//! the Spell being moved onto something else and keeps the pairing
//! honest. The card it names is carried on the effect itself, by
//! `SetLabelObject` — the port's first use of that slot.
//!
//! It is registered `EFFECT_FLAG_CANNOT_DISABLE` and reset on
//! `RESET_EVENT|RESETS_STANDARD`, so it dies with the Spell rather than
//! outliving it.
//!
//! Every call here is to `script_api`, by the rule in `cards/mod.rs`.

use crate::board::{location, position};
use crate::card::reason;
use crate::effect::{effect_type, flag, Ctx, Effect, LabelObject, Yield};
use crate::event::{category, code, CardId};
use crate::field::{reset, resets, Field};
use crate::host_question::{hint, hintmsg};
use crate::script_api as api;

pub const CODE: u32 = 70_828_912;

/// The life this costs, as the script names it.
const COST_LP: u32 = 800;

pub fn initial_effect(f: &mut Field, c: CardId) {
    // Activate
    let e1 = api::create_effect(f, c);
    api::set_category(f, e1, category::SPECIAL_SUMMON);
    api::set_type(f, e1, effect_type::ACTIVATE);
    api::set_code(f, e1, code::FREE_CHAIN);
    api::set_property(f, e1, flag::CARD_TARGET, 0);
    api::set_cost(f, e1, cost);
    api::set_target(f, e1, target);
    api::set_operation(f, e1, operation);
    api::register_effect(f, c, e1, false);
    // Destroy
    let e2 = api::create_effect(f, c);
    api::set_type(f, e2, effect_type::CONTINUOUS | effect_type::SINGLE);
    api::set_code(f, e2, code::LEAVE_FIELD);
    api::set_operation(f, e2, desop);
    api::register_effect(f, c, e2, false);
}

const GRAVE: u32 = location::GRAVE as u32;

/// `Cost.PayLP(800)` — the library's helper, spelled out.
fn cost(f: &mut Field, ctx: &Ctx, chk: bool) -> bool {
    let tp = ctx.player;
    if !chk {
        return api::check_lp_cost(f, tp, COST_LP);
    }
    api::pay_lp_cost(f, tp, COST_LP);
    true
}

/// `s.spfilter` — a monster this effect could special summon.
fn spfilter(e: crate::event::EffectId, tp: u8) -> impl Fn(&mut Field, CardId) -> bool {
    move |f: &mut Field, c: CardId| api::is_can_be_special_summoned(f, c, e, 0, tp, false, false)
}

fn target(f: &mut Field, ctx: &Ctx, chk: bool, chkc: Option<CardId>) -> Yield {
    let tp = ctx.player;
    let e = ctx.reason_effect;
    let sp = spfilter(e, tp);
    if let Some(c) = chkc {
        return api::yes(
            api::is_location(f, c, u16::from(location::GRAVE))
                && api::is_controler(f, c, tp)
                && sp(f, c),
        );
    }
    if !chk {
        // A seat to revive into, and something to revive.
        return api::yes(
            api::get_location_count(f, tp, location::MZONE) > 0
                && api::is_existing_target(f, Some(&sp), tp, GRAVE, 0, 1, api::Except::None),
        );
    }
    api::hint(f, hint::SELECTMSG, tp, hintmsg::SPSUMMON);
    api::select_target(f, tp, Some(&sp), tp, GRAVE, 0, 1, 1, api::Except::None);
    let handler = api::get_handler(f, e);
    api::suspend(move |f, _ctx| {
        let Some(g) = api::selected_targets(f) else {
            return api::done();
        };
        api::set_operation_info(f, 0, category::SPECIAL_SUMMON, Some(g), 1, 0, 0);
        // The card announces that it will **equip itself**, which is the
        // half a reader of the announcement would otherwise miss.
        api::set_operation_info(f, 0, category::EQUIP, handler.map(|c| vec![c]), 1, 0, 0);
        api::done()
    })
}

/// `s.eqlimit` — may this equip card stay attached to `ctx.card`?
///
/// Only to the monster the effect remembers. `ctx.card` is the slot the
/// reference fills by pushing the card before the call.
fn eqlimit(e: &Effect, _f: &Field, ctx: &Ctx) -> i64 {
    i64::from(
        matches!(ctx.card, Some(c) if api::get_label_object(e).and_then(LabelObject::card) == Some(c)),
    )
}

fn operation(f: &mut Field, ctx: &Ctx) -> Yield {
    let tp = ctx.player;
    let e = ctx.reason_effect;
    let Some(c) = api::get_handler(f, e) else {
        return api::done();
    };
    let Some(tc) = api::get_first_target(f) else {
        return api::done();
    };
    if !api::is_relate_to_effect(f, c, e) || !api::is_relate_to_effect(f, tc, e) {
        return api::done();
    }
    api::special_summon(
        f,
        vec![tc],
        0,
        tp,
        tp,
        false,
        false,
        position::FACEUP_ATTACK,
    );
    api::suspend(move |f, _ctx| {
        // A count, not a flag: zero means the summon was refused.
        if api::resumed_value(f) == 0 {
            return api::done();
        }
        api::equip(f, tp, c, tc, true, false);
        api::suspend(move |f, _ctx| {
            // Add Equip limit
            let e1 = api::create_effect(f, c);
            api::set_type(f, e1, effect_type::SINGLE);
            api::set_code(f, e1, code::EQUIP_LIMIT);
            api::set_property(f, e1, flag::CANNOT_DISABLE, 0);
            api::set_reset(f, e1, reset::EVENT | resets::STANDARD, 1);
            api::set_value_fn(f, e1, eqlimit);
            api::set_label_object(f, e1, Some(LabelObject::Card(tc)));
            api::register_effect(f, c, e1, false);
            api::done()
        })
    })
}

fn desop(f: &mut Field, ctx: &Ctx) -> Yield {
    let Some(c) = api::get_handler(f, ctx.reason_effect) else {
        return api::done();
    };
    let Some(tc) = api::get_first_card_target(f, c) else {
        return api::done();
    };
    if api::is_reason(f, c, reason::DESTROY) && api::is_location(f, tc, u16::from(location::MZONE))
    {
        api::destroy(f, vec![tc], reason::EFFECT);
    }
    api::done()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{card_type, status, Card, CardData};
    use crate::chain::Chain;
    use crate::duel::phases;
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

    /// `tp`'s Main Phase 1 with the Spell face-up in its row, `mine` in
    /// `tp`'s graveyard and `theirs` in the opponent's.
    fn field_as(tp: u8, mine: u32, theirs: u32) -> (Field, CardId, Vec<CardId>, Vec<CardId>) {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        f.infos.turn_player = tp;
        f.infos.phase = phases::MAIN1;
        let mut d = Card::with_data(crate::cards::card_data(CODE).expect("printed data"), tp);
        d.current.controller = tp;
        d.set_status(status::EFFECT_ENABLED, true);
        let pb = f.new_card(d);
        f.add_card(tp, pb, location::SZONE, 0, false);
        f.cards[pb].current.position = position::FACEUP;
        f.initialize_card(pb);
        let ours = (0..mine)
            .map(|i| monster(&mut f, tp, 7_000 + i, location::GRAVE, i))
            .collect();
        let theirs = (0..theirs)
            .map(|i| monster(&mut f, 1 - tp, 8_000 + i, location::GRAVE, i))
            .collect();
        (f, pb, ours, theirs)
    }

    fn field(mine: u32, theirs: u32) -> (Field, CardId, Vec<CardId>, Vec<CardId>) {
        field_as(0, mine, theirs)
    }

    fn activate_effect(f: &Field, pb: CardId) -> EffectId {
        f.cards[pb].field_effect.equal_range(code::FREE_CHAIN)[0]
    }

    fn leave_effect(f: &Field, pb: CardId) -> EffectId {
        f.cards[pb].single_effect.equal_range(code::LEAVE_FIELD)[0]
    }

    /// Ask the target function directly, with the one piece of state
    /// `ExecuteTarget` would have set: `core.reason_effect`, which the
    /// scan's **targeting** mode reads to decide whether a card can be
    /// targeted by *this* effect at all.
    fn ask_target(f: &mut Field, e: EffectId, tp: u8, chkc: Option<CardId>) -> bool {
        f.core.reason_effect = Some(e);
        let ev = Event::new(code::FREE_CHAIN);
        let ctx = ctx_for(e, &ev, tp);
        target(f, &ctx, false, chkc).finished().unwrap_or(0) != 0
    }

    fn ctx_for(e: EffectId, ev: &Event, tp: u8) -> Ctx<'_> {
        Ctx {
            reason_effect: e,
            player: tp,
            event: ev,
            card: None,
            args: &[],
        }
    }

    /// Controller, location, sequence — the part of a `LocInfo` a test
    /// can assert on.
    type Seat = (u8, u8, u32);

    #[derive(Default)]
    struct Run {
        /// Each `SelectCard`: who was asked, the bounds, and the offer.
        asked: Vec<(u8, u8, u8, Vec<CardId>)>,
        /// Each `Hint`, in order.
        hints: Vec<(u8, u8, u64)>,
        /// Each `Equip` message, as (equip seat, target seat).
        equips: Vec<(Seat, Seat)>,
    }

    /// Drive target then operation, answering the two questions the card
    /// asks. `pick` is the index taken from the offer.
    fn resolve_as(f: &mut Field, tp: u8, e: EffectId, pick: usize) -> Run {
        let mut ch = Chain::new(e, Event::new(code::FREE_CHAIN));
        ch.triggering_player = tp;
        ch.chain_count = 1;
        ch.chain_id = 11;
        f.core.current_chain.push(ch);
        f.core.chain_solving = true;
        // What `AddChain` does for a real activation: the activating card
        // is related to its own chain link, and the operation refuses to
        // act if it is not.
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
                    Message::Equip { equip, target } => run.equips.push((
                        (equip.controller, equip.location, equip.sequence),
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

    /// **Two printed effects.** An activation that names a card and
    /// declares a special summon with a cost, and a continuous single
    /// effect on leaving the field with **no condition** — it fires every
    /// time and decides for itself.
    #[test]
    fn the_script_registers_an_activation_and_a_leave_the_field_effect() {
        let (f, pb, _, _) = field(1, 0);
        let ids = f.cards[pb].field_effect.equal_range(code::FREE_CHAIN);
        assert_eq!(ids.len(), 1);
        let e = f.effects.get(ids[0]).unwrap();
        assert!(e.is_type(effect_type::ACTIVATE));
        assert_eq!(e.category, category::SPECIAL_SUMMON);
        assert!(e.is_flag(flag::CARD_TARGET), "it names a card");
        assert!(e.cost.is_some());

        let ids = f.cards[pb].single_effect.equal_range(code::LEAVE_FIELD);
        assert_eq!(ids.len(), 1);
        let e2 = f.effects.get(ids[0]).unwrap();
        assert!(e2.is_type(effect_type::CONTINUOUS));
        assert!(e2.is_type(effect_type::SINGLE));
        assert!(e2.condition.is_none(), "no condition: it always fires");
        assert!(e2.operation.is_some());
    }

    /// **The cost is 800 life, checked before it is paid**, and paid by
    /// the activating player.
    #[test]
    fn the_cost_is_eight_hundred_life() {
        let (mut f, pb, _, _) = field(1, 0);
        let e = activate_effect(&f, pb);
        let ev = Event::new(code::FREE_CHAIN);
        let ctx = ctx_for(e, &ev, 0);
        assert!(cost(&mut f, &ctx, false));
        f.players[0].lp = 800;
        assert!(cost(&mut f, &ctx, false), "exactly enough is enough");
        f.players[0].lp = 799;
        assert!(!cost(&mut f, &ctx, false), "and one short is not");

        f.players[0].lp = 8000;
        assert!(cost(&mut f, &ctx, true));
        match &f.core.subunits[0].kind {
            Kind::PayLPCost { playerid, cost } => {
                assert_eq!(*playerid, 0);
                assert_eq!(*cost, 800);
            }
            other => panic!("expected a PayLPCost: {other:?}"),
        }
    }

    /// **A refused summon stops the card.** With no seat left by the
    /// time it resolves, the summon fails and the Spell does not go on to
    /// equip itself to a monster still in the graveyard.
    #[test]
    fn a_refused_summon_leaves_the_spell_unattached() {
        let (mut f, pb, mine, _) = field(1, 0);
        let e = activate_effect(&f, pb);
        let chosen = mine[0];
        let mut ch = Chain::new(e, Event::new(code::FREE_CHAIN));
        ch.triggering_player = 0;
        ch.chain_count = 1;
        ch.chain_id = 11;
        ch.target_cards = vec![chosen];
        f.core.current_chain.push(ch);
        f.core.chain_solving = true;
        f.cards[chosen].create_chain_relation(e, 11);
        f.cards[pb].create_chain_relation(e, 11);
        // Fill the row between the activation and the resolution.
        for i in 0..5 {
            monster(&mut f, 0, 9_000 + i, location::MZONE, i);
        }
        f.core
            .sub_solving_event
            .push_back(Event::new(code::FREE_CHAIN));
        f.emplace(Kind::ExecuteOperation {
            resume: None,
            effect: e,
            player: 0,
            subject: None,
            args: Vec::new(),
            was_disabled: false,
        });
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
        assert_eq!(
            f.cards[chosen].current.location,
            location::GRAVE,
            "still in the graveyard"
        );
        assert_eq!(f.cards[pb].equiping_target, None, "and nothing attached");
        assert!(f.cards[pb]
            .single_effect
            .equal_range(code::EQUIP_LIMIT)
            .is_empty());
    }

    /// **A Spell that never equipped anything destroys nothing** — the
    /// common case on the harness, where a face-down copy is destroyed
    /// before it is ever activated.
    #[test]
    fn a_spell_that_never_equipped_destroys_nothing() {
        let (mut f, pb, _, _) = field(1, 0);
        f.cards[pb].reason = reason::DESTROY | reason::EFFECT;
        let ev = Event::new(code::LEAVE_FIELD);
        let e2 = leave_effect(&f, pb);
        desop(&mut f, &ctx_for(e2, &ev, 0));
        assert!(f.core.subunits.is_empty());
    }

    /// **A monster of its own, and a seat to put it in.** Four boards,
    /// and only the last has both — which is the positive sibling the
    /// three refusals need.
    #[test]
    fn it_needs_a_monster_of_its_own_and_room_for_it() {
        // Nothing in either graveyard.
        let (mut f, pb, _, _) = field(0, 0);
        let e = activate_effect(&f, pb);
        assert!(!ask_target(&mut f, e, 0, None));
        // Only the opponent's graveyard is stocked.
        let (mut f, pb, _, _) = field(0, 2);
        let e = activate_effect(&f, pb);
        assert!(
            !ask_target(&mut f, e, 0, None),
            "their graveyard is not its graveyard"
        );
        // Its own graveyard, but no seat.
        let (mut f, pb, _, _) = field(1, 0);
        for i in 0..5 {
            monster(&mut f, 0, 9_000 + i, location::MZONE, i);
        }
        let e = activate_effect(&f, pb);
        assert!(!ask_target(&mut f, e, 0, None), "a full row");
        // Both.
        let (mut f, pb, _, _) = field(1, 0);
        let e = activate_effect(&f, pb);
        assert!(ask_target(&mut f, e, 0, None));
    }

    /// **The same, as player 1** — the scan follows `tp`, not the table.
    /// A suite that only ever activates as player 0 cannot tell `tp` from
    /// a literal `0`.
    #[test]
    fn the_scan_follows_the_activating_player() {
        let (mut f, pb, _, _) = field_as(1, 0, 2);
        let e = activate_effect(&f, pb);
        assert!(!ask_target(&mut f, e, 1, None), "only player 0's graveyard");
        let (mut f, pb, _, _) = field_as(1, 2, 0);
        let e = activate_effect(&f, pb);
        assert!(ask_target(&mut f, e, 1, None));
    }

    /// **`chkc` asks three separate questions**, and each gets a board
    /// where only it refuses — plus the positive sibling they share.
    #[test]
    fn the_target_check_wants_its_own_graveyard_and_a_summonable_monster() {
        let (mut f, pb, mine, theirs) = field(1, 1);
        let e = activate_effect(&f, pb);
        assert!(
            ask_target(&mut f, e, 0, Some(mine[0])),
            "the positive sibling"
        );
        assert!(
            !ask_target(&mut f, e, 0, Some(theirs[0])),
            "the opponent's graveyard"
        );

        // Its own and perfectly summonable, but in the **hand** rather
        // than the graveyard. A monster already on the field would not
        // separate the clauses: it fails the summonability test too, so
        // the location clause would never be the one refusing.
        let (mut f, pb, _, _) = field(0, 0);
        let in_hand = monster(&mut f, 0, 9_400, location::HAND, 0);
        let e = activate_effect(&f, pb);
        let sp = spfilter(e, 0);
        assert!(sp(&mut f, in_hand), "it could be summoned from there");
        assert!(
            !ask_target(&mut f, e, 0, Some(in_hand)),
            "but a hand is not a graveyard"
        );

        // Its own graveyard, but not summonable.
        let (mut f, pb, mine, _) = field(1, 0);
        f.cards[mine[0]].set_status(status::FORBIDDEN, true);
        let e = activate_effect(&f, pb);
        assert!(
            !ask_target(&mut f, e, 0, Some(mine[0])),
            "it could not be summoned"
        );
    }

    /// **One monster, chosen by the activating player, from its own
    /// graveyard**, prefaced by the special-summon prompt.
    ///
    /// The order is pinned too: the graveyard comes back **top first**,
    /// so the most recently buried monster is offered before the one
    /// under it.
    #[test]
    fn it_asks_its_controller_for_one_of_its_own() {
        let (mut f, pb, mine, _) = field(2, 2);
        let e = activate_effect(&f, pb);
        let run = resolve(&mut f, e, 0);
        assert_eq!(run.asked.len(), 1, "one question");
        let (player, min, max, offered) = &run.asked[0];
        assert_eq!(*player, 0);
        assert_eq!((*min, *max), (1, 1), "exactly one");
        assert_eq!(
            offered,
            &vec![mine[1], mine[0]],
            "its own graveyard, top first"
        );
        assert!(
            run.hints
                .iter()
                .any(|&(k, p, v)| (k, p, v) == (hint::SELECTMSG, 0, hintmsg::SPSUMMON)),
            "prompted as a special summon: {:?}",
            run.hints
        );
    }

    /// **The same question, asked as player 1**, over player 1's own
    /// graveyard.
    #[test]
    fn the_question_follows_the_activating_player() {
        let (mut f, pb, mine, _) = field_as(1, 2, 2);
        let e = activate_effect(&f, pb);
        let run = resolve_as(&mut f, 1, e, 0);
        let (player, _, _, offered) = &run.asked[0];
        assert_eq!(*player, 1);
        assert_eq!(offered, &vec![mine[1], mine[0]]);
    }

    /// **The announcement names both halves** — the special summon *and*
    /// the equip, the second naming the Spell itself.
    #[test]
    fn it_announces_the_summon_and_the_equip() {
        let (mut f, pb, _, _) = field(2, 0);
        let e = activate_effect(&f, pb);
        let run = resolve(&mut f, e, 1);
        let chosen = run.asked[0].3[1];
        let ops = &f.core.current_chain[0].opinfos;
        let sp = ops
            .get(&category::SPECIAL_SUMMON)
            .expect("the summon category");
        assert_eq!(sp.cards.as_deref(), Some(&[chosen][..]));
        assert_eq!(sp.count, 1);
        let eq = ops.get(&category::EQUIP).expect("the equip category");
        assert_eq!(
            eq.cards.as_deref(),
            Some(&[pb][..]),
            "the Spell equips itself, not the monster"
        );
        assert_eq!(eq.count, 1);
    }

    /// **It revives face-up in attack position on its own side, and then
    /// attaches itself to what it revived.**
    #[test]
    fn it_revives_the_chosen_monster_and_equips_itself_to_it() {
        let (mut f, pb, mine, _) = field(2, 0);
        let e = activate_effect(&f, pb);
        let run = resolve(&mut f, e, 0);
        let chosen = run.asked[0].3[0];
        let left_behind = *mine.iter().find(|&&c| c != chosen).unwrap();
        assert_eq!(f.cards[chosen].current.location, location::MZONE);
        assert_eq!(f.cards[chosen].current.controller, 0, "its own side");
        assert_eq!(f.cards[chosen].current.position, position::FACEUP_ATTACK);
        assert_eq!(
            f.cards[left_behind].current.location,
            location::GRAVE,
            "only the one chosen"
        );
        // The attachment, both directions, and the card-target record the
        // leave-the-field half will read later.
        assert_eq!(f.cards[pb].equiping_target, Some(chosen));
        assert!(f.cards[chosen].equiping_cards.contains(&pb));
        assert_eq!(api::get_first_card_target(&f, pb), Some(chosen));
        // And the message says which seat is attached to which.
        assert!(
            f.core.equiping_cards.is_empty(),
            "a whole equip, not one step of a batch"
        );
        assert_eq!(run.equips.len(), 1);
        let (equip, target_loc) = run.equips[0];
        assert_eq!(equip, (0, location::SZONE, 0));
        assert_eq!(
            target_loc,
            (0, location::MZONE, f.cards[chosen].current.sequence)
        );
    }

    /// **The equip limit names the monster it revived**, cannot be
    /// disabled, and dies with the Spell.
    #[test]
    fn the_equip_limit_names_the_revived_monster() {
        let (mut f, pb, mine, _) = field(2, 0);
        let e = activate_effect(&f, pb);
        let run = resolve(&mut f, e, 0);
        let chosen = run.asked[0].3[0];
        let other = *mine.iter().find(|&&c| c != chosen).unwrap();
        let ids: Vec<EffectId> = f.cards[pb]
            .single_effect
            .equal_range(code::EQUIP_LIMIT)
            .to_vec();
        assert_eq!(ids.len(), 1);
        let limit = f.effects.get(ids[0]).unwrap();
        assert!(limit.is_type(effect_type::SINGLE));
        assert!(limit.is_flag(flag::CANNOT_DISABLE));
        assert_eq!(limit.label_object, Some(LabelObject::Card(chosen)));
        assert_ne!(limit.reset_flag & reset::EVENT, 0);
        assert_eq!(
            limit.reset_flag & resets::STANDARD,
            resets::STANDARD,
            "every standard reset, so it dies with the Spell"
        );

        // The value function answers for that monster and nothing else.
        let ev = Event::new(code::FREE_CHAIN);
        let yes = Ctx {
            reason_effect: e,
            player: 0,
            event: &ev,
            card: Some(chosen),
            args: &[],
        };
        let no = Ctx {
            card: Some(other),
            ..yes
        };
        let nobody = Ctx { card: None, ..yes };
        let limit = f.effects.get(ids[0]).unwrap();
        assert_eq!(eqlimit(limit, &f, &yes), 1);
        assert_eq!(eqlimit(limit, &f, &no), 0, "not some other monster");
        assert_eq!(eqlimit(limit, &f, &nobody), 0, "and not nothing");
        // Nothing remembered and nothing asked about is still no. Two
        // absences are not a match, which a bare equality would call one.
        api::set_label_object(&mut f, ids[0], None);
        let limit = f.effects.get(ids[0]).unwrap();
        assert_eq!(
            eqlimit(limit, &f, &nobody),
            0,
            "two absences are not a match"
        );
    }

    /// **The rules destroy an equip card whose limit stops answering**,
    /// which is the whole point of the limit: `adjust_equips` asks it once
    /// per adjust about the card's *current* target.
    #[test]
    fn the_limit_is_what_keeps_the_pairing_honest() {
        let (mut f, pb, _, _) = field(2, 0);
        let e = activate_effect(&f, pb);
        resolve(&mut f, e, 0);
        assert_eq!(f.cards[pb].current.location, location::SZONE);
        // Move the attachment to a monster the limit does not name.
        let intruder = monster(&mut f, 0, 9_900, location::MZONE, 4);
        let old = f.cards[pb].equiping_target.expect("attached");
        f.cards[old].equiping_cards.retain(|&c| c != pb);
        f.cards[pb].equiping_target = Some(intruder);
        f.cards[intruder].equiping_cards.push(pb);
        f.adjust_all();
        drain(&mut f);
        assert_ne!(
            f.cards[pb].current.location,
            location::SZONE,
            "the rules took it off the row"
        );
    }

    /// Run the queue out with nothing left to answer.
    fn drain(f: &mut Field) {
        for _ in 0..4096 {
            match f.process() {
                Status::Continue => {
                    if f.core.units.is_empty() && f.core.subunits.is_empty() {
                        return;
                    }
                }
                other => panic!("stopped with {other:?}"),
            }
        }
        panic!("did not settle");
    }

    /// **Destroyed, specifically.** The leave-the-field half fires every
    /// time and acts only when the Spell was *destroyed* and the monster
    /// it remembers is still on the field — three clauses, and a board
    /// each where only that one refuses.
    ///
    /// Driven directly, because the current play policy never destroys an
    /// equipped copy while its monster is still there: in every harness
    /// game the revived monster dies in battle first, and the Spell then
    /// goes by `REASON_LOST_TARGET` with the card-target tie already cut.
    #[test]
    fn it_destroys_the_monster_only_when_it_was_destroyed() {
        for (why, monster_on_field, expect) in [
            (reason::DESTROY | reason::EFFECT, true, true),
            (reason::EFFECT, true, false),
            (reason::DESTROY | reason::RULE, false, false),
            (reason::DESTROY | reason::EFFECT, false, false),
        ] {
            let (mut f, pb, _, _) = field(2, 0);
            let e = activate_effect(&f, pb);
            let run = resolve(&mut f, e, 0);
            let chosen = run.asked[0].3[0];
            if !monster_on_field {
                f.move_card(0, chosen, location::GRAVE, 0, false);
            }
            f.cards[pb].reason = why;
            let ev = Event::new(code::LEAVE_FIELD);
            let e2 = leave_effect(&f, pb);
            desop(&mut f, &ctx_for(e2, &ev, 0));
            let queued = f
                .core
                .subunits
                .iter()
                .any(|u| matches!(u.kind, Kind::Destroy { .. }));
            assert_eq!(
                queued, expect,
                "reason {why:#x}, monster on field {monster_on_field}"
            );
        }
    }

    /// **It destroys the monster it remembers, by effect** — and what it
    /// remembers comes from the *equip*, not from the chain, which is
    /// long gone by the time this fires.
    #[test]
    fn the_leave_half_destroys_what_the_equip_recorded() {
        let (mut f, pb, _, _) = field(2, 0);
        let e = activate_effect(&f, pb);
        let run = resolve(&mut f, e, 0);
        let chosen = run.asked[0].3[0];
        // The chain is over.
        f.core.current_chain.clear();
        f.core.chain_solving = false;
        assert!(api::get_first_target(&f).is_none(), "no chain to read");
        assert_eq!(api::get_first_card_target(&f, pb), Some(chosen));

        f.cards[pb].reason = reason::DESTROY | reason::EFFECT;
        let ev = Event::new(code::LEAVE_FIELD);
        let e2 = leave_effect(&f, pb);
        desop(&mut f, &ctx_for(e2, &ev, 0));
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
                assert_ne!(why & reason::EFFECT, 0, "by effect, not by rule");
                assert_eq!(why & reason::RULE, 0);
            }
            other => panic!("expected a Destroy: {other:?}"),
        }
    }

    /// Drive the operation alone, on a chain the caller has set up — for
    /// the boards the ordinary path cannot produce.
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

    /// Put a chain link in place naming `tc`, relating whichever of the
    /// two the caller asks for.
    fn staged(
        mine: u32,
        relate_spell: bool,
        relate_monster: bool,
    ) -> (Field, CardId, CardId, EffectId) {
        let (mut f, pb, ours, _) = field(mine, 0);
        let e = activate_effect(&f, pb);
        let tc = ours[0];
        let mut ch = Chain::new(e, Event::new(code::FREE_CHAIN));
        ch.triggering_player = 0;
        ch.chain_count = 1;
        ch.chain_id = 11;
        ch.target_cards = vec![tc];
        f.core.current_chain.push(ch);
        f.core.chain_solving = true;
        if relate_spell {
            f.cards[pb].create_chain_relation(e, 11);
        }
        if relate_monster {
            f.cards[tc].create_chain_relation(e, 11);
        }
        (f, pb, tc, e)
    }

    /// **Both cards must still be related to the effect.** The Spell may
    /// have been chained away and the monster may have been moved, and
    /// either one alone stops the resolution — so each gets a board, with
    /// the both-related one as the positive sibling.
    #[test]
    fn it_refuses_unless_both_the_spell_and_the_monster_are_still_related() {
        for (spell, monster_related, expect) in [
            (true, true, true),
            (false, true, false),
            (true, false, false),
        ] {
            let (mut f, pb, tc, e) = staged(1, spell, monster_related);
            operate_only(&mut f, e, 0);
            let summoned = f.cards[tc].current.location == location::MZONE;
            assert_eq!(
                summoned, expect,
                "spell related {spell}, monster related {monster_related}"
            );
            assert_eq!(
                f.cards[pb].equiping_target.is_some(),
                expect,
                "and the attachment follows"
            );
        }
    }

    /// **The summon is made with the checks on.** A monster carrying an
    /// `EFFECT_SPSUMMON_CONDITION` that refuses — the "cannot be Special
    /// Summoned except by…" shape — is not revived, which is only true
    /// because `Duel.SpecialSummon` is called with `nocheck` false.
    ///
    /// Driven through the operation directly: the same condition makes
    /// the monster fail the card's own filter, so it would never be
    /// offered by the ordinary path.
    #[test]
    fn a_monster_that_refuses_to_be_summoned_is_not_revived() {
        fn refuses(_e: &Effect, _f: &Field, _ctx: &Ctx) -> i64 {
            0
        }
        let (mut f, pb, tc, e) = staged(1, true, true);
        let mut cond = crate::effect::Effect::new(effect_type::SINGLE, code::SPSUMMON_CONDITION);
        cond.owner = Some(tc);
        cond.handler = Some(tc);
        let id = f.new_effect(cond);
        f.effects.get_mut(id).unwrap().value_fn = Some(refuses);
        f.cards[tc]
            .single_effect
            .insert(code::SPSUMMON_CONDITION, id);
        f.cards[tc].indexer.insert(id);

        operate_only(&mut f, e, 0);
        assert_eq!(
            f.cards[tc].current.location,
            location::GRAVE,
            "the condition refused it"
        );
        assert_eq!(f.cards[pb].equiping_target, None);
    }
}

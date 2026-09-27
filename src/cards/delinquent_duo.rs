//! Delinquent Duo — `c44763025.lua`.
//!
//! The twenty-fourth card, and the pool's first with a **cost**. It pays
//! 1000 life, then takes two cards from the opponent's hand: one at
//! random, and one the *opponent's opponent* chooses.
//!
//! ## The first cost
//!
//! `Cost.PayLP(1000)` is the library's helper (`utility.lua:1695`), and
//! it is two functions in one: asked with `chk == 0` it answers
//! `Duel.CheckLPCost`, and asked to pay it calls `Duel.PayLPCost`. The
//! port's `Cost` seam already had that shape — `fn(&mut Field, &Ctx, chk:
//! bool) -> bool` — and this is the first card to use it.
//!
//! `CheckLPCost` is not "has that much life". Cost-changing effects
//! compose over the figure first, a cost reduced to zero or less is
//! payable by definition, and a replacement effect makes it payable
//! however little life the player has.
//!
//! ## The target player is chain state, not an argument
//!
//! `Duel.SetTargetPlayer(tp)` writes onto the chain link, and the
//! operation reads it back with `Duel.GetChainInfo(0,
//! CHAININFO_TARGET_PLAYER)` rather than using `tp`. On this card they
//! are the same player — but the indirection is the script's, and it is
//! what `EFFECT_FLAG_PLAYER_TARGET` is for: the announcement names a
//! player, and the resolution acts on whoever was named.
//!
//! ## One at random, then one chosen — and the roll is observable
//!
//! `g:RandomSelect(p,1)` rolls the duel's generator. The reference's loop
//! keeps drawing until its result *set* grows, so a duplicate draw costs
//! a roll and changes nothing; picking without replacement would consume
//! a different number of values and every later roll would diverge.
//!
//! The second card is chosen by **`1-p`, the player whose hand it is** —
//! the one losing the cards picks which one goes. That is worth stating
//! because it reads backwards: `p` is the player the chain *named*, the
//! group is `GetFieldGroup(p, 0, HAND)` and so belongs to `1-p`, and the
//! selection and its hint both go to `1-p` as well. The reference is the
//! specification here, and it hands the choice to the hand's owner.
//!
//! Every call here is to `script_api`, by the rule in `cards/mod.rs`.

use crate::board::location;
use crate::card::reason;
use crate::effect::{effect_type, flag, Ctx, Yield};
use crate::event::{category, code, CardId};
use crate::field::Field;
use crate::host_question::{hint, hintmsg};
use crate::script_api as api;

pub const CODE: u32 = 44_763_025;

/// The life this costs, as the script names it.
const COST_LP: u32 = 1000;

pub fn initial_effect(f: &mut Field, c: CardId) {
    // Activate
    let e1 = api::create_effect(f, c);
    api::set_category(f, e1, category::HANDES);
    api::set_type(f, e1, effect_type::ACTIVATE);
    api::set_property(f, e1, flag::PLAYER_TARGET, 0);
    api::set_code(f, e1, code::FREE_CHAIN);
    api::set_cost(f, e1, cost);
    api::set_target(f, e1, target);
    api::set_operation(f, e1, activate);
    api::register_effect(f, c, e1, false);
}

const HAND: u32 = location::HAND as u32;

/// `Cost.PayLP(1000)` — the library's helper, spelled out.
fn cost(f: &mut Field, ctx: &Ctx, chk: bool) -> bool {
    let tp = ctx.player;
    if !chk {
        return api::check_lp_cost(f, tp, COST_LP);
    }
    api::pay_lp_cost(f, tp, COST_LP);
    true
}

fn target(f: &mut Field, ctx: &Ctx, chk: bool, _chkc: Option<CardId>) -> Yield {
    let tp = ctx.player;
    if !chk {
        // Something in the opponent's hand to take.
        return api::yes(api::get_field_group_count(f, tp, 0, HAND) > 0);
    }
    api::set_target_player(f, tp);
    // Two cards, from the *other* player's hand.
    api::set_operation_info(f, 0, category::HANDES, None, 0, 1 - tp, 2);
    api::yes(true)
}

fn activate(f: &mut Field, _ctx: &Ctx) -> Yield {
    // The player the announcement named, not `ctx.player`.
    let Some(p) = api::get_chain(f, 0).map(|ch| ch.target_player) else {
        return api::done();
    };
    let g = api::get_field_group(f, p, 0, HAND);
    if g.is_empty() {
        return api::done();
    }
    api::random_select_request(f, &g, p, 1);
    api::suspend(move |f, _ctx| duo_after_pick(f, p, g.clone()))
}

/// The rest of the activation, once the random pick is in.
fn duo_after_pick(f: &mut Field, p: u8, g: Vec<CardId>) -> Yield {
    let sg = api::random_selected(f);
    api::send_to_grave(f, sg.clone(), reason::EFFECT | reason::DISCARD);
    // `g:RemoveCard(sg:GetFirst())` — the second choice is from what is
    // left, and the first card is gone from the group whether or not the
    // send has finished.
    let rest: Vec<CardId> = g.into_iter().filter(|c| !sg.contains(c)).collect();
    api::suspend(move |f, _ctx| {
        if rest.is_empty() {
            return api::done();
        }
        // Chosen by `1 - p`, which is the player whose hand it is.
        api::hint(f, hint::SELECTMSG, 1 - p, hintmsg::DISCARD);
        api::group_select(f, &rest, 1 - p, 1, 1, api::Except::None);
        api::suspend(move |f, _ctx| {
            let chosen = api::group_selected(f);
            if !chosen.is_empty() {
                api::send_to_grave(f, chosen, reason::EFFECT | reason::DISCARD);
            }
            api::done()
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{card_type, status, Card, CardData};
    use crate::chain::Chain;
    use crate::duel::phases;
    use crate::event::Event;
    use crate::field::Message;
    use crate::position;
    use crate::processor::{Kind, Status};

    fn put(f: &mut Field, owner: u8, code_: u32, loc: u8, seq: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: code_,
                type_: card_type::MONSTER | card_type::NORMAL,
                level: 4,
                attack: 1000,
                defense: 1000,
                ..Default::default()
            },
            owner,
        );
        c.current.controller = owner;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(owner, id, loc, seq, false);
        id
    }

    /// `tp`'s Main Phase 1 with the Spell face-up in its row and `theirs`
    /// cards in the opponent's hand.
    fn field_as(tp: u8, theirs: u32) -> (Field, CardId, Vec<CardId>) {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        f.infos.turn_player = tp;
        f.infos.phase = phases::MAIN1;
        let mut d = Card::with_data(crate::cards::card_data(CODE).expect("printed data"), tp);
        d.current.controller = tp;
        d.set_status(status::EFFECT_ENABLED, true);
        let dd = f.new_card(d);
        f.add_card(tp, dd, location::SZONE, 0, false);
        f.cards[dd].current.position = position::FACEUP;
        f.initialize_card(dd);
        let hand = (0..theirs)
            .map(|i| put(&mut f, 1 - tp, 5_000 + i, location::HAND, i))
            .collect();
        (f, dd, hand)
    }

    fn field(theirs: u32) -> (Field, CardId, Vec<CardId>) {
        field_as(0, theirs)
    }

    fn effect_of(f: &Field, dd: CardId) -> crate::event::EffectId {
        f.cards[dd].field_effect.equal_range(code::FREE_CHAIN)[0]
    }

    fn ctx_for(e: crate::event::EffectId, ev: &Event, tp: u8) -> Ctx<'_> {
        Ctx {
            reason_effect: e,
            player: tp,
            event: ev,
            card: None,
            args: &[],
        }
    }

    struct Run {
        offered: Vec<Vec<CardId>>,
        asked: Vec<(u8, u8, u8)>,
    }

    fn resolve_as(f: &mut Field, tp: u8, e: crate::event::EffectId, choice: usize) -> Run {
        let mut ch = Chain::new(e, Event::new(code::FREE_CHAIN));
        ch.triggering_player = tp;
        ch.chain_count = 1;
        ch.chain_id = 11;
        f.core.current_chain.push(ch);
        f.core.chain_solving = true;
        // The target half runs first, which is what names the player.
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
        let mut run = Run {
            offered: Vec::new(),
            asked: Vec::new(),
        };
        let mut operated = false;
        for _ in 0..8192 {
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
                        run.asked.push((*player, *min, *max));
                        let min = usize::from(*min);
                        run.offered.push(cards.clone());
                        f.core.return_cards.clear();
                        f.core.returns.set_i32(0, 0);
                        f.core.returns.set_i32(1, min as i32);
                        for i in 0..min {
                            f.core.returns.set_i32(i + 2, (choice + i) as i32);
                        }
                    }
                    other => panic!("unexpected question: {other:?}"),
                },
                other => panic!("stopped with {other:?}"),
            }
        }
        run
    }

    fn resolve(f: &mut Field, e: crate::event::EffectId, choice: usize) -> Run {
        resolve_as(f, 0, e, choice)
    }

    /// **One printed activate effect that names a player**, with a cost
    /// and the hand-destruction category.
    #[test]
    fn the_script_registers_a_costed_activate_effect() {
        let (f, dd, _) = field(0);
        let ids = f.cards[dd].field_effect.equal_range(code::FREE_CHAIN);
        assert_eq!(ids.len(), 1);
        let e = f.effects.get(ids[0]).unwrap();
        assert!(e.is_type(effect_type::ACTIVATE));
        assert!(e.is_flag(flag::PLAYER_TARGET), "it names a player");
        assert!(!e.is_flag(flag::CARD_TARGET), "and not a card");
        assert!(e.is_flag(crate::effect::flag::INITIAL));
        assert_eq!(e.category, category::HANDES);
        assert!(e.cost.is_some(), "the pool's first cost");
    }

    /// **The cost is 1000 life, checked before it is paid.**
    #[test]
    fn the_cost_is_a_thousand_life() {
        let (mut f, dd, _) = field(1);
        let e = effect_of(&f, dd);
        let ev = Event::new(code::FREE_CHAIN);
        let ctx = ctx_for(e, &ev, 0);
        assert!(cost(&mut f, &ctx, false), "8000 life is plenty");
        f.players[0].lp = 1000;
        assert!(cost(&mut f, &ctx, false), "exactly enough is enough");
        f.players[0].lp = 999;
        assert!(!cost(&mut f, &ctx, false), "and one short is not");

        // Paying queues the cost against the activating player.
        f.players[0].lp = 8000;
        assert!(cost(&mut f, &ctx, true));
        match &f.core.subunits[0].kind {
            Kind::PayLPCost { playerid, cost } => {
                assert_eq!(*playerid, 0, "the activating player pays");
                assert_eq!(*cost, 1000);
            }
            other => panic!("expected a PayLPCost: {other:?}"),
        }
    }

    /// **It needs something in the opponent's hand**, not its own.
    #[test]
    fn it_needs_a_card_in_the_opponents_hand() {
        let (mut f, dd, _) = field(0);
        let e = effect_of(&f, dd);
        let ev = Event::new(code::FREE_CHAIN);
        let ctx = ctx_for(e, &ev, 0);
        // A full hand of my own is not what it reads.
        for i in 0..3 {
            put(&mut f, 0, 6_000 + i, location::HAND, i);
        }
        assert!(
            target(&mut f, &ctx, false, None).finished().unwrap_or(0) == 0,
            "my own hand does not count"
        );
        let (mut f, dd, _) = field(1);
        let e = effect_of(&f, dd);
        let ctx = ctx_for(e, &ev, 0);
        assert!(target(&mut f, &ctx, false, None).finished().unwrap_or(0) != 0);
    }

    /// **The announcement names the opponent and two cards**, and the
    /// chain link remembers the target player.
    #[test]
    fn it_announces_two_from_the_opponents_hand() {
        for tp in [0u8, 1] {
            let (mut f, dd, _) = field_as(tp, 2);
            let e = effect_of(&f, dd);
            resolve_as(&mut f, tp, e, 0);
            let link = &f.core.current_chain[0];
            assert_eq!(link.target_player, tp, "the player it named");
            let op = link
                .opinfos
                .get(&category::HANDES)
                .cloned()
                .unwrap_or_else(|| panic!("a HANDES operation for tp={tp}"));
            assert_eq!(op.player, 1 - tp, "the hand that loses cards");
            assert_eq!(op.param, 2, "two of them");
        }
    }

    /// **Two cards go: one at random, then one the *other* player
    /// picks.** The second choice is offered to whoever is not losing
    /// them, from what the first one left.
    #[test]
    fn one_is_taken_at_random_and_one_is_chosen() {
        let (mut f, dd, hand) = field(3);
        let e = effect_of(&f, dd);
        let run = resolve(&mut f, e, 0);
        let gone: Vec<CardId> = hand
            .iter()
            .copied()
            .filter(|&c| f.cards[c].current.location == location::GRAVE)
            .collect();
        assert_eq!(gone.len(), 2, "two cards left the hand");
        for &c in &gone {
            assert!(f.cards[c].reason & reason::DISCARD != 0, "discarded");
            assert!(f.cards[c].reason & reason::EFFECT != 0, "by an effect");
        }
        // One question, asked of the activating player, over what the
        // random pick left.
        assert_eq!(run.asked.len(), 1, "only the second card is chosen");
        assert_eq!(
            run.asked[0],
            (1, 1, 1),
            "`1 - p`: the player whose hand it is picks which one goes"
        );
        assert_eq!(run.offered[0].len(), 2, "the two the random pick left");
        let random_pick = *gone
            .iter()
            .find(|c| !run.offered[0].contains(c))
            .expect("the random one was not offered again");
        assert!(
            !run.offered[0].contains(&random_pick),
            "the first card is out of the group"
        );
    }

    /// **The prompt is the discard message, to the player choosing.**
    #[test]
    fn the_prompt_is_the_discard_message() {
        let (mut f, dd, _) = field(2);
        let e = effect_of(&f, dd);
        resolve(&mut f, e, 0);
        assert!(
            f.messages.iter().any(|m| matches!(
                m,
                Message::Hint { kind, player: 1, value }
                    if *kind == hint::SELECTMSG && *value == hintmsg::DISCARD
            )),
            "HINTMSG_DISCARD to `1 - p`, who makes the choice"
        );
    }

    /// **A one-card hand loses that card and nothing is asked.** The
    /// random pick empties the group, so there is no second choice.
    #[test]
    fn a_single_card_hand_is_taken_without_a_question() {
        let (mut f, dd, hand) = field(1);
        let e = effect_of(&f, dd);
        let run = resolve(&mut f, e, 0);
        assert_eq!(f.cards[hand[0]].current.location, location::GRAVE);
        assert!(
            run.asked.is_empty(),
            "nothing left to choose from: {:?}",
            run.asked
        );
        // And the question is not *attempted*: asking over an empty
        // group would still announce itself, because `SelectCard`'s step
        // 0 answers an empty list with a `SELECTMSG` hint of value zero
        // before returning. No hint at all is the proof that the guard
        // returned first.
        assert!(
            !f.messages
                .iter()
                .any(|m| matches!(m, Message::Hint { kind, .. } if *kind == hint::SELECTMSG)),
            "no selection was even begun: {:?}",
            f.messages
        );
    }

    /// **A hand emptied between activation and resolution takes
    /// nothing.** Unreachable through `target`, which refuses an empty
    /// hand — but the hand can empty while the chain sits there.
    #[test]
    fn a_hand_emptied_in_between_loses_nothing() {
        let (mut f, dd, hand) = field(1);
        let e = effect_of(&f, dd);
        let mut ch = Chain::new(e, Event::new(code::FREE_CHAIN));
        ch.triggering_player = 0;
        ch.chain_count = 1;
        ch.chain_id = 11;
        ch.target_player = 0;
        f.core.current_chain.push(ch);
        // Gone before it resolves.
        f.remove_card(hand[0]);
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
                Status::Awaiting => panic!("nothing should be asked"),
                _ => break,
            }
        }
        assert!(
            !f.messages
                .iter()
                .any(|m| matches!(m, Message::Hint { kind, .. } if *kind == hint::SELECTMSG)),
            "nothing was begun"
        );
    }

    /// **The operation reads the player the chain named**, not the
    /// activating player. They coincide on this card, so the test makes
    /// them differ by naming the other one.
    #[test]
    fn the_operation_reads_the_named_player() {
        let (mut f, dd, _) = field(2);
        let mine: Vec<CardId> = (0..2)
            .map(|i| put(&mut f, 0, 6_100 + i, location::HAND, i))
            .collect();
        let e = effect_of(&f, dd);
        let mut ch = Chain::new(e, Event::new(code::FREE_CHAIN));
        ch.triggering_player = 0;
        ch.chain_count = 1;
        ch.chain_id = 11;
        // The announcement named player 1, so player 1's *opponent* —
        // that is, player 0 — is the hand that loses cards.
        ch.target_player = 1;
        f.core.current_chain.push(ch);
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
        for _ in 0..8192 {
            match f.process() {
                Status::Continue => {
                    if f.core.units.is_empty() && f.core.subunits.is_empty() {
                        break;
                    }
                }
                Status::Awaiting => match f.messages.last() {
                    Some(Message::SelectCard { min, .. }) => {
                        let min = usize::from(*min);
                        f.core.return_cards.clear();
                        f.core.returns.set_i32(0, 0);
                        f.core.returns.set_i32(1, min as i32);
                        for i in 0..min {
                            f.core.returns.set_i32(i + 2, i as i32);
                        }
                    }
                    other => panic!("unexpected question: {other:?}"),
                },
                other => panic!("stopped with {other:?}"),
            }
        }
        let mine_gone = mine
            .iter()
            .filter(|&&c| f.cards[c].current.location == location::GRAVE)
            .count();
        assert_eq!(
            mine_gone, 2,
            "the named player was 1, so player 0's hand is the one emptied"
        );
    }
}

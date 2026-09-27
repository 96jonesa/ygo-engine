//! Graceful Charity — `c79571449.lua`.
//!
//! The first card that **suspends**. Pot of Greed draws and is done;
//! this one draws, **looks at how many it got**, and only then shuffles,
//! breaks the timing window and discards two. That branch is the whole
//! reason the seam learned to yield — see `processor-loop.md`,
//! "Operations can suspend now".
//!
//! ## The script, and what each step waits for
//!
//! ```lua
//! if Duel.Draw(p,d,REASON_EFFECT)==3 then
//!     Duel.ShuffleHand(p)
//!     Duel.BreakEffect()
//!     Duel.DiscardHand(p,nil,2,2,REASON_EFFECT|REASON_DISCARD)
//! end
//! ```
//!
//! `Duel.Draw` queues a processor and yields; its **result** is the number
//! actually drawn, which is what `== 3` tests. `ShuffleHand` and
//! `BreakEffect` are synchronous and queue nothing. `DiscardHand` queues a
//! selection, so the card waits again — and has nothing left to do
//! afterwards.
//!
//! ## Why the count can be under three
//!
//! `target(chk = 0)` refuses unless the player can draw three, so the
//! activation is legal only with three in the deck. But the draw happens
//! at **resolution**, and a deck can be emptier by then — another card
//! resolving first, in a chain. That is not a hypothetical the test has to
//! invent: it is the difference between "may I activate this" and "what
//! happened when it resolved", and the reason the script asks again rather
//! than trusting the target check.
//!
//! Every call here is to `script_api`, by the rule in `cards/mod.rs`.

use crate::card::reason;
use crate::effect::{effect_type, flag, Ctx, Yield};
use crate::event::{category, code, CardId};
use crate::field::Field;
use crate::script_api as api;

pub const CODE: u32 = 79_571_449;

pub fn initial_effect(f: &mut Field, c: CardId) {
    // Activate
    let e1 = api::create_effect(f, c);
    api::set_category(f, e1, category::DRAW | category::HANDES);
    api::set_type(f, e1, effect_type::ACTIVATE);
    api::set_property(f, e1, flag::PLAYER_TARGET, 0);
    api::set_code(f, e1, code::FREE_CHAIN);
    api::set_target(f, e1, target);
    api::set_operation(f, e1, activate);
    api::register_effect(f, c, e1, false);
}

fn target(f: &mut Field, ctx: &Ctx, chk: bool, _chkc: Option<CardId>) -> Yield {
    let tp = ctx.player;
    if !chk {
        return api::yes(api::is_player_can_draw(f, tp, 3));
    }
    api::set_target_player(f, tp);
    api::set_target_param(f, 3);
    api::set_operation_info(f, 0, category::DRAW, None, 0, tp, 3);
    api::set_operation_info(f, 0, category::HANDES, None, 0, tp, 2);
    api::yes(true)
}

fn activate(f: &mut Field, _ctx: &Ctx) -> Yield {
    let (p, d) = api::get_chain_target_player_param(f, 0);
    api::draw(f, p, d as u32, reason::EFFECT);
    api::suspend(move |f, _| {
        if api::resumed_value(f) != 3 {
            return api::done();
        }
        api::shuffle_hand(f, p);
        api::break_effect(f);
        api::discard_hand(
            f,
            p,
            None,
            2,
            2,
            reason::EFFECT | reason::DISCARD,
            api::Except::None,
        );
        api::suspend(|_, _| api::done())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::location;
    use crate::card::{card_type, status, Card, CardData};
    use crate::chain::Chain;
    use crate::duel::phases;
    use crate::event::Event;
    use crate::field::Message;
    use crate::processor::{Kind, Status};

    /// `target`, asked as a plain yes/no. It answers through `returns`
    /// like any card function, so a test reads the finished value rather
    /// than the `Yield`.
    fn asks(f: &mut Field, ctx: &Ctx, chk: bool, chkc: Option<CardId>) -> bool {
        target(f, ctx, chk, chkc).finished().unwrap_or(0) != 0
    }

    fn put(f: &mut Field, owner: u8, code_: u32, type_: u32, loc: u8, seq: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: code_,
                type_,
                ..Default::default()
            },
            owner,
        );
        c.current.controller = owner;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(owner, id, loc, seq, false);
        let fid = f.next_field_id_raw();
        f.cards[id].fieldid = fid;
        f.cards[id].fieldid_r = fid;
        id
    }

    /// Player 0 in Main Phase 1 with Graceful Charity in hand, `deck`
    /// cards to draw from and `hand` spare cards to discard.
    fn field(deck: u32, hand: u32) -> (Field, CardId) {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        f.infos.turn_player = 0;
        f.infos.phase = phases::MAIN1;
        for i in 0..deck {
            put(&mut f, 0, 5_053_103, card_type::MONSTER, location::DECK, i);
        }
        for i in 0..hand {
            put(
                &mut f,
                0,
                6_000_000 + i,
                card_type::MONSTER,
                location::HAND,
                i,
            );
        }
        let gc = put(&mut f, 0, CODE, card_type::SPELL, location::HAND, hand);
        f.initialize_card(gc);
        (f, gc)
    }

    /// **One printed activate effect**, with both categories and a player
    /// target: it draws *and* it makes a player give cards up.
    #[test]
    fn the_script_registers_a_printed_activate_effect() {
        let (f, gc) = field(5, 0);
        let ids = f.cards[gc].field_effect.equal_range(code::FREE_CHAIN);
        assert_eq!(ids.len(), 1);
        let e = f.effects.get(ids[0]).unwrap();
        assert!(e.is_type(effect_type::ACTIVATE));
        assert!(e.is_flag(flag::PLAYER_TARGET));
        assert!(e.is_flag(crate::effect::flag::INITIAL));
        assert_eq!(e.category, category::DRAW | category::HANDES);
    }

    /// **It is offered only when three can be drawn.**
    #[test]
    fn it_needs_three_in_the_deck_to_be_offered() {
        for (deck, want) in [(3u32, true), (2, false)] {
            let (mut f, _) = field(deck, 0);
            f.emplace(Kind::IdleCommand {
                state: Default::default(),
            });
            let mut offered = Vec::new();
            for _ in 0..4096 {
                match f.process() {
                    Status::Continue => continue,
                    Status::Awaiting => {
                        if let Some(Message::SelectIdleCmd { activatable, .. }) = f.messages.last()
                        {
                            offered = activatable.iter().map(|o| o.code).collect();
                        }
                        break;
                    }
                    other => panic!("stopped with {other:?}"),
                }
            }
            assert_eq!(offered.contains(&CODE), want, "deck of {deck}");
        }
    }

    /// **What the target records is what the operation later draws.**
    /// The resolution reads the player and count back off the chain, so a
    /// wrong number recorded here is a wrong number drawn — and the tests
    /// that resolve directly would never notice, because they hand the
    /// count in themselves.
    #[test]
    fn the_target_records_three_for_its_controller() {
        let (mut f, gc) = field(5, 0);
        let e = f.cards[gc].field_effect.equal_range(code::FREE_CHAIN)[0];
        let mut ch = Chain::new(e, Event::new(code::FREE_CHAIN));
        ch.triggering_player = 0;
        ch.chain_count = 1;
        f.core.current_chain.push(ch);
        let ev = Event::new(code::FREE_CHAIN);
        let ctx = Ctx {
            reason_effect: e,
            player: 0,
            event: &ev,
            card: None,
            args: &[],
        };
        assert!(asks(&mut f, &ctx, true, None));
        assert_eq!(
            api::get_chain_target_player_param(&f, 0),
            (0, 3),
            "its controller, and three"
        );
        let opinfos = &f.core.current_chain[0].opinfos;
        assert_eq!(
            opinfos.get(&category::DRAW).map(|t| t.param),
            Some(3),
            "the draw it will do"
        );
        assert_eq!(
            opinfos.get(&category::HANDES).map(|t| t.param),
            Some(2),
            "and the two it will take back"
        );
    }

    /// Run the operation alone, with the chain link the activation would
    /// have left, answering any selection with the smallest legal choice.
    fn resolve(f: &mut Field, gc: CardId, player: u8, param: i32) {
        let e = f.cards[gc].field_effect.equal_range(code::FREE_CHAIN)[0];
        let mut ch = Chain::new(e, Event::new(code::FREE_CHAIN));
        ch.triggering_player = player;
        ch.chain_count = 1;
        ch.target_player = player;
        ch.target_param = param;
        f.core.current_chain.push(ch);
        f.core
            .sub_solving_event
            .push_back(Event::new(code::FREE_CHAIN));
        f.emplace(Kind::ExecuteOperation {
            resume: None,
            effect: e,
            player,
            subject: None,
            args: Vec::new(),
            was_disabled: false,
        });
        for _ in 0..4096 {
            match f.process() {
                Status::Continue => {
                    if f.core.units.is_empty() {
                        return;
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
        panic!("did not settle");
    }

    /// **Drawing three then discarding two**, end to end through the
    /// suspension: the hand is three bigger and two smaller, the deck
    /// three smaller, and two cards reached the graveyard.
    #[test]
    fn it_draws_three_and_discards_two() {
        let (mut f, gc) = field(6, 1);
        let deck_before = f.players[0].main.len();
        let hand_before = f.players[0].hand.len();
        // Open the window on a timing nothing later re-raises, so that
        // finding it gone afterwards means this card shut it.
        f.core.hint_timing = [crate::field::timing::DRAW_PHASE; 2];
        resolve(&mut f, gc, 0, 3);
        assert_eq!(f.players[0].main.len(), deck_before - 3, "three drawn");
        assert_eq!(
            f.players[0].hand.len(),
            hand_before + 3 - 2,
            "and two given up"
        );
        assert_eq!(f.players[0].grave.len(), 2);
        for c in f.players[0].grave.clone() {
            assert!(f.cards[c].reason & crate::card::reason::DISCARD != 0);
        }
        // The two calls between the draw and the discard, asserted where
        // the *card* makes them rather than where the API implements
        // them: the shuffle announces itself, and the timing window is
        // shut behind it.
        assert!(
            f.messages
                .iter()
                .any(|m| matches!(m, Message::ShuffleHand { .. })),
            "the hand was shuffled before the discard"
        );
        assert_eq!(
            f.core.hint_timing[0] & crate::field::timing::DRAW_PHASE,
            0,
            "and the timing window was broken"
        );
    }

    /// **Fewer than three drawn means nothing is discarded.**
    ///
    /// This is the test the whole suspension mechanism exists for. The
    /// script branches on `Duel.Draw(...) == 3`, and the only way that
    /// branch can be taken correctly is if the *result* of the queued draw
    /// came back to the card. An implementation that resumed but read a
    /// stale or zero value would discard here, or discard in the case
    /// above and not here — and the harness alone would not say so,
    /// because a deck this short cannot legally activate the card.
    #[test]
    fn fewer_than_three_drawn_discards_nothing() {
        let (mut f, gc) = field(2, 3);
        let hand_before = f.players[0].hand.len();
        resolve(&mut f, gc, 0, 3);
        assert!(f.players[0].main.is_empty(), "the deck gave what it had");
        assert_eq!(
            f.players[0].hand.len(),
            hand_before + 2,
            "two drawn, and none given up"
        );
        assert!(
            f.players[0].grave.is_empty(),
            "the branch was not taken: nothing was discarded"
        );
    }
}

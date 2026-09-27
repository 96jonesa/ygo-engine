//! Morphing Jar — `c33508719.lua`.
//!
//! A flip effect that empties **both** hands and refills them: each
//! player discards everything and draws five. The first card to act on
//! both players in one resolution, and the first to suspend **three**
//! times.
//!
//! ## What it waits for, and what it does not
//!
//! ```lua
//! local g=Duel.GetFieldGroup(tp,LOCATION_HAND,LOCATION_HAND)
//! if #g>0 then Duel.SendtoGrave(g,REASON_DISCARD|REASON_EFFECT) end
//! Duel.BreakEffect()
//! Duel.Draw(tp,5,REASON_EFFECT)
//! Duel.Draw(1-tp,5,REASON_EFFECT)
//! ```
//!
//! `GetFieldGroup` is a plain read. `SendtoGrave` queues and yields, but
//! **only if there is anything to send** — with both hands empty the
//! card never suspends there, which is the script's `#g > 0` guard doing
//! real work rather than tidiness. `BreakEffect` is synchronous. Each
//! `Draw` yields.
//!
//! ## Both hands, from one call
//!
//! `GetFieldGroup(tp, LOCATION_HAND, LOCATION_HAND)` passes the same
//! mask twice, which is how a scan covering both sides is spelled — the
//! two arguments are the controller's zones and the opponent's, not a
//! location and a redundant copy of it.
//!
//! Every call here is to `script_api`, by the rule in `cards/mod.rs`.

use crate::board::location;
use crate::card::reason;
use crate::effect::{effect_type, Ctx, Yield};
use crate::event::{category, CardId, PLAYER_ALL};
use crate::field::Field;
use crate::script_api as api;

pub const CODE: u32 = 33_508_719;

pub fn initial_effect(f: &mut Field, c: CardId) {
    // flip
    let e1 = api::create_effect(f, c);
    api::set_category(f, e1, category::HANDES | category::DRAW);
    api::set_type(f, e1, effect_type::SINGLE | effect_type::FLIP);
    api::set_target(f, e1, target);
    api::set_operation(f, e1, operation);
    api::register_effect(f, c, e1, false);
}

fn target(f: &mut Field, _ctx: &Ctx, chk: bool, _chkc: Option<CardId>) -> Yield {
    if !chk {
        return api::yes(true);
    }
    api::set_operation_info(f, 0, category::HANDES, None, 0, PLAYER_ALL, 0);
    api::set_operation_info(f, 0, category::DRAW, None, 0, PLAYER_ALL, 5);
    api::yes(true)
}

fn operation(f: &mut Field, ctx: &Ctx) -> Yield {
    let tp = ctx.player;
    let hand = u32::from(location::HAND);
    let g = api::get_field_group(f, tp, hand, hand);
    if g.is_empty() {
        // Nothing to discard, so nothing to wait for.
        return refill(f, tp);
    }
    api::send_to_grave(f, g, reason::DISCARD | reason::EFFECT);
    api::suspend(move |f, _| refill(f, tp))
}

/// The rest, once whatever was in hand has gone.
fn refill(f: &mut Field, tp: u8) -> Yield {
    api::break_effect(f);
    api::draw(f, tp, 5, reason::EFFECT);
    api::suspend(move |f, _| {
        api::draw(f, 1 - tp, 5, reason::EFFECT);
        api::suspend(|_, _| api::done())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{card_type, status, Card, CardData};
    use crate::chain::Chain;
    use crate::duel::phases;
    use crate::event::{code, Event};
    use crate::field::Message;
    use crate::position;
    use crate::processor::{Kind, Status};

    fn put(f: &mut Field, owner: u8, code_: u32, loc: u8, seq: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: code_,
                type_: card_type::MONSTER,
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

    /// Both players with `hand` cards in hand and eight in the deck, and
    /// Morphing Jar face-down on player 0's side.
    fn field(hand: u32) -> (Field, CardId) {
        let mut f = Field::new(8000);
        f.infos.turn_id = 4;
        f.infos.turn_player = 0;
        f.infos.phase = phases::MAIN1;
        for p in 0..2u8 {
            for i in 0..8u32 {
                put(&mut f, p, 5_053_103, location::DECK, i);
            }
            for i in 0..hand {
                put(&mut f, p, 7_000 + i, location::HAND, i);
            }
        }
        let mj = put(&mut f, 0, CODE, location::MZONE, 0);
        f.cards[mj].data = crate::cards::card_data(CODE).expect("printed data");
        f.cards[mj].current.position = position::FACEDOWN_DEFENSE;
        f.initialize_card(mj);
        (f, mj)
    }

    fn effect_of(f: &Field, mj: CardId) -> crate::event::EffectId {
        f.cards[mj].single_effect.equal_range(code::FLIP)[0]
    }

    /// Resolve the operation with a chain link in place, running the
    /// machine until it settles.
    fn resolve(f: &mut Field, mj: CardId) {
        let e = effect_of(f, mj);
        let mut ch = Chain::new(e, Event::new(code::FLIP));
        ch.triggering_player = 0;
        ch.chain_count = 1;
        f.core.current_chain.push(ch);
        f.core.sub_solving_event.push_back(Event::new(code::FLIP));
        f.emplace(Kind::ExecuteOperation {
            resume: None,
            effect: e,
            player: 0,
            subject: Some(mj),
            args: Vec::new(),
            was_disabled: false,
        });
        for _ in 0..8192 {
            match f.process() {
                Status::Continue => {
                    if f.core.units.is_empty() {
                        return;
                    }
                }
                other => panic!("stopped with {other:?}"),
            }
        }
        panic!("did not settle");
    }

    /// **One printed flip effect**, with both categories.
    #[test]
    fn the_script_registers_a_flip_effect() {
        let (f, mj) = field(0);
        let ids = f.cards[mj].single_effect.equal_range(code::FLIP);
        assert_eq!(ids.len(), 1);
        let e = f.effects.get(ids[0]).unwrap();
        assert!(e.is_type(effect_type::SINGLE) && e.is_type(effect_type::FLIP));
        assert_eq!(e.category, category::HANDES | category::DRAW);
    }

    /// **It records both operations against *both* players.**
    /// `PLAYER_ALL` is the whole of "each player" in an operation info.
    #[test]
    fn it_records_both_operations_for_both_players() {
        let (mut f, mj) = field(2);
        let e = effect_of(&f, mj);
        let mut ch = Chain::new(e, Event::new(code::FLIP));
        ch.triggering_player = 0;
        ch.chain_count = 1;
        f.core.current_chain.push(ch);
        let ev = Event::new(code::FLIP);
        let ctx = Ctx {
            reason_effect: e,
            player: 0,
            event: &ev,
            card: Some(mj),
            args: &[],
        };
        assert!(target(&mut f, &ctx, true, None).finished().unwrap_or(0) != 0);
        let ops = &f.core.current_chain[0].opinfos;
        assert_eq!(
            ops.get(&category::HANDES).map(|t| t.player),
            Some(PLAYER_ALL)
        );
        let draw = ops.get(&category::DRAW).expect("a draw operation");
        assert_eq!((draw.player, draw.param), (PLAYER_ALL, 5));
    }

    /// **Both hands go, and both players draw five.** The whole card, and
    /// three suspensions deep: the discard, then one draw, then the
    /// other.
    #[test]
    fn both_hands_are_emptied_and_refilled() {
        let (mut f, mj) = field(3);
        let decks_before = [f.players[0].main.len(), f.players[1].main.len()];
        // Open a timing nothing later re-raises, so that finding it gone
        // means this card shut it.
        f.core.hint_timing = [crate::field::timing::DRAW_PHASE; 2];
        resolve(&mut f, mj);
        assert_eq!(
            f.core.hint_timing[0] & crate::field::timing::DRAW_PHASE,
            0,
            "the timing window was broken between the discard and the draws"
        );
        for (p, &before) in decks_before.iter().enumerate() {
            assert_eq!(f.players[p].hand.len(), 5, "player {p} holds five");
            assert_eq!(f.players[p].main.len(), before - 5, "player {p} drew five");
            assert_eq!(f.players[p].grave.len(), 3, "and gave up three");
            for c in f.players[p].grave.clone() {
                assert!(f.cards[c].reason & crate::card::reason::DISCARD != 0);
                assert!(
                    f.cards[c].current.is_faceup(),
                    "sent face-up, as SendtoGrave's default is"
                );
            }
        }
    }

    /// **With both hands already empty it still draws.** The script's
    /// `#g > 0` guard means the card does not wait for a discard that
    /// will not happen — and the draws must follow regardless.
    #[test]
    fn empty_hands_still_draw() {
        // First: with nothing to send, nothing is *queued* to send. The
        // reference's `#g > 0` guard keeps the queue clean, and an empty
        // send still emplaces a unit, so dropping the guard would show
        // here and nowhere else.
        let (mut probe, mj_probe) = field(0);
        let e = effect_of(&probe, mj_probe);
        let ev = Event::new(code::FLIP);
        let ctx = Ctx {
            reason_effect: e,
            player: 0,
            event: &ev,
            card: Some(mj_probe),
            args: &[],
        };
        probe.core.subunits.clear();
        operation(&mut probe, &ctx);
        assert!(
            !probe
                .core
                .subunits
                .iter()
                .any(|u| matches!(u.kind, Kind::SendTo { .. })),
            "no send was queued for an empty hand"
        );

        let (mut f, mj) = field(0);
        resolve(&mut f, mj);
        for p in 0..2usize {
            assert_eq!(f.players[p].hand.len(), 5, "player {p} still drew five");
            assert!(f.players[p].grave.is_empty(), "and discarded nothing");
        }
        let drew: usize = f
            .messages
            .iter()
            .filter(|m| matches!(m, Message::Draw { .. }))
            .count();
        assert_eq!(drew, 2, "one draw message per player");
    }

    /// **It reads both hands, not only its controller's.** The scan is
    /// given the hand mask twice — once for each side — and a card that
    /// read it once would leave the opponent holding their cards.
    #[test]
    fn it_empties_the_opponents_hand_too() {
        let (mut f, mj) = field(2);
        let theirs: Vec<_> = f.players[1].hand.clone();
        resolve(&mut f, mj);
        for c in theirs {
            assert_eq!(
                f.cards[c].current.location,
                location::GRAVE,
                "the opponent's hand went too"
            );
        }
    }
}

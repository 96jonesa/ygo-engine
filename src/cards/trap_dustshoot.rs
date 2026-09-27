//! Trap Dustshoot — `c64697231.lua`.
//!
//! The twenty-fifth card. It shows the opponent's hand to its controller
//! and puts one monster from it back into the deck — and then shuffles
//! the hand, because its owner has just had it read.
//!
//! ## The condition counts a hand it does not own
//!
//! `Duel.GetFieldGroupCount(tp,0,LOCATION_HAND) > 3` — strictly more
//! than three, so a hand of exactly four is the smallest it works on.
//! The masks are `s = 0, o = HAND`, so this counts the **opponent's**
//! hand throughout.
//!
//! ## It reveals before it filters
//!
//! `Duel.ConfirmCards(p, g)` is called on the whole hand, and only then
//! is the hand filtered down to monsters. So a hand with no monster in it
//! is still **shown** — the reveal happens, and nothing else does. A port
//! that filtered first would show only the monsters, or nothing at all.
//!
//! ## The last shuffle is the point of the card
//!
//! `Duel.ShuffleHand(1-p)` runs whenever the hand was non-empty, monster
//! or not. Its owner has had the whole hand read, so the order it is in
//! is no longer private — and a hand shuffle really does roll the duel's
//! generator (`DUEL_PSEUDO_SHUFFLE` skips only the deck). Dropping it
//! would desynchronise every later roll, which is exactly the shape of
//! the bug Magician of Faith turned up.
//!
//! Every call here is to `script_api`, by the rule in `cards/mod.rs`.

use crate::board::location;
use crate::card::reason;
use crate::effect::{effect_type, flag, Ctx, Yield};
use crate::event::{category, code, CardId};
use crate::field::{timing, Field};
use crate::host_question::{hint, hintmsg};
use crate::script_api as api;

pub const CODE: u32 = 64_697_231;

pub fn initial_effect(f: &mut Field, c: CardId) {
    // Activate
    let e1 = api::create_effect(f, c);
    api::set_category(f, e1, category::TODECK);
    api::set_type(f, e1, effect_type::ACTIVATE);
    api::set_hint_timing(f, e1, 0, timing::TOHAND);
    api::set_property(f, e1, flag::PLAYER_TARGET, 0);
    api::set_code(f, e1, code::FREE_CHAIN);
    api::set_condition(f, e1, condition);
    api::set_target(f, e1, target);
    api::set_operation(f, e1, activate);
    api::register_effect(f, c, e1, false);
}

const HAND: u32 = location::HAND as u32;

fn condition(f: &mut Field, ctx: &Ctx) -> bool {
    // Strictly more than three, and it is the opponent's hand.
    api::get_field_group_count(f, ctx.player, 0, HAND) > 3
}

fn target(f: &mut Field, ctx: &Ctx, chk: bool, _chkc: Option<CardId>) -> Yield {
    if !chk {
        return api::yes(true);
    }
    api::set_target_player(f, ctx.player);
    api::yes(true)
}

fn activate(f: &mut Field, _ctx: &Ctx) -> Yield {
    let Some(p) = api::get_chain(f, 0).map(|ch| ch.target_player) else {
        return api::done();
    };
    let g = api::get_field_group(f, p, 0, HAND);
    if g.is_empty() {
        return api::done();
    }
    // The whole hand, before any filtering: a hand with no monster in it
    // is still shown.
    api::confirm_cards(f, p, g.clone());
    let monsters: Vec<CardId> = g
        .iter()
        .copied()
        .filter(|&c| api::is_monster(f, c))
        .collect();
    if monsters.is_empty() {
        // Nothing to take, but the hand has still been read.
        api::shuffle_hand(f, 1 - p);
        return api::done();
    }
    api::hint(f, hint::SELECTMSG, p, hintmsg::TODECK);
    api::group_select(f, &monsters, p, 1, 1, api::Except::None);
    api::suspend(move |f, _ctx| {
        let chosen = api::group_selected(f);
        if !chosen.is_empty() {
            api::send_to_deck(f, chosen, None, api::SEQ_DECKSHUFFLE as i32, reason::EFFECT);
        }
        api::suspend(move |f, _ctx| {
            // Whatever happened above, the hand has been read.
            api::shuffle_hand(f, 1 - p);
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

    fn put(f: &mut Field, owner: u8, code_: u32, type_: u32, loc: u8, seq: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: code_,
                type_,
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

    const MON: u32 = card_type::MONSTER | card_type::NORMAL;
    const SPELL: u32 = card_type::SPELL;

    /// `tp`'s Main Phase 1 with the Trap face-up in its row and the
    /// opponent holding `hand` cards of the given types. Each player also
    /// has a deck card, so a shuffle has a pile to announce.
    fn field_as(tp: u8, hand: &[u32]) -> (Field, CardId, Vec<CardId>) {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        f.infos.turn_player = tp;
        f.infos.phase = phases::MAIN1;
        let mut d = Card::with_data(crate::cards::card_data(CODE).expect("printed data"), tp);
        d.current.controller = tp;
        d.set_status(status::EFFECT_ENABLED, true);
        let td = f.new_card(d);
        f.add_card(tp, td, location::SZONE, 0, false);
        f.cards[td].current.position = position::FACEUP;
        f.initialize_card(td);
        let theirs = hand
            .iter()
            .enumerate()
            .map(|(i, &ty)| {
                put(
                    &mut f,
                    1 - tp,
                    5_000 + i as u32,
                    ty,
                    location::HAND,
                    i as u32,
                )
            })
            .collect();
        put(&mut f, tp, 5_900, MON, location::DECK, 0);
        put(&mut f, 1 - tp, 5_901, MON, location::DECK, 0);
        (f, td, theirs)
    }

    fn field(hand: &[u32]) -> (Field, CardId, Vec<CardId>) {
        field_as(0, hand)
    }

    fn effect_of(f: &Field, td: CardId) -> crate::event::EffectId {
        f.cards[td].field_effect.equal_range(code::FREE_CHAIN)[0]
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
        confirms: Vec<(u8, Vec<u32>)>,
        hand_shuffles: Vec<u8>,
    }

    fn resolve_as(f: &mut Field, tp: u8, e: crate::event::EffectId, choice: usize) -> Run {
        let mut ch = Chain::new(e, Event::new(code::FREE_CHAIN));
        ch.triggering_player = tp;
        ch.chain_count = 1;
        ch.chain_id = 11;
        f.core.current_chain.push(ch);
        f.core.chain_solving = true;
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
            confirms: Vec::new(),
            hand_shuffles: Vec::new(),
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
        for m in &f.messages {
            match m {
                Message::ConfirmCards { player, codes } => {
                    run.confirms.push((*player, codes.clone()));
                }
                Message::ShuffleHand { player, .. } => run.hand_shuffles.push(*player),
                _ => {}
            }
        }
        run
    }

    fn resolve(f: &mut Field, e: crate::event::EffectId, choice: usize) -> Run {
        resolve_as(f, 0, e, choice)
    }

    /// **One printed activate effect that names a player**, with a
    /// condition and the to-deck category, offered at the to-hand timing
    /// on the opponent's side.
    #[test]
    fn the_script_registers_a_conditional_activate_effect() {
        let (f, td, _) = field(&[]);
        let ids = f.cards[td].field_effect.equal_range(code::FREE_CHAIN);
        assert_eq!(ids.len(), 1);
        let e = f.effects.get(ids[0]).unwrap();
        assert!(e.is_type(effect_type::ACTIVATE));
        assert!(e.is_flag(flag::PLAYER_TARGET), "it names a player");
        assert!(e.is_flag(crate::effect::flag::INITIAL));
        assert_eq!(e.category, category::TODECK);
        assert!(e.condition.is_some());
        assert_eq!(
            e.hint_timing,
            [0, timing::TOHAND],
            "nothing on its own side; the to-hand timing on the opponent's"
        );
    }

    /// **The condition is strictly more than three cards**, in the
    /// *opponent's* hand.
    #[test]
    fn the_condition_wants_four_in_the_opponents_hand() {
        let ev = Event::new(code::FREE_CHAIN);
        for (n, want) in [(2usize, false), (3, false), (4, true), (5, true)] {
            let hand: Vec<u32> = std::iter::repeat_n(MON, n).collect();
            let (mut f, td, _) = field(&hand);
            let e = effect_of(&f, td);
            let ctx = ctx_for(e, &ev, 0);
            assert_eq!(condition(&mut f, &ctx), want, "a hand of {n}");
        }
        // And it is the opponent's hand, not its own: four of mine is not
        // enough.
        let (mut f, td, _) = field(&[]);
        for i in 0..4 {
            put(&mut f, 0, 6_000 + i, MON, location::HAND, i);
        }
        let e = effect_of(&f, td);
        let ctx = ctx_for(e, &ev, 0);
        assert!(!condition(&mut f, &ctx), "my own hand does not count");
    }

    /// **The whole hand is shown, then one monster goes back.** The
    /// reveal goes to the activating player, the choice is theirs, and
    /// the card returns to its owner's deck.
    #[test]
    fn it_shows_the_hand_and_takes_a_monster() {
        let (mut f, td, hand) = field(&[MON, SPELL, MON, SPELL]);
        let e = effect_of(&f, td);
        let run = resolve(&mut f, e, 0);
        // Everything was revealed, not just the monsters.
        let codes: Vec<u32> = hand.iter().map(|&c| f.cards[c].data.code).collect();
        assert_eq!(run.confirms.len(), 1, "one reveal");
        assert_eq!(run.confirms[0].0, 0, "shown to the activating player");
        assert_eq!(run.confirms[0].1, codes, "the whole hand, Spells included");
        // But only the monsters are offered.
        let mut want = vec![hand[0], hand[2]];
        want.sort_unstable();
        let mut offered = run.offered[0].clone();
        offered.sort_unstable();
        assert_eq!(offered, want, "monsters only");
        assert_eq!(run.asked[0], (0, 1, 1), "the activating player chooses");
        let taken = run.offered[0][0];
        assert_eq!(f.cards[taken].current.location, location::DECK);
        assert_eq!(
            f.cards[taken].current.controller, 1,
            "back to its owner's deck, not the taker's"
        );
        assert!(f.cards[taken].reason & reason::EFFECT != 0);
        // `SEQ_DECKSHUFFLE` is "anywhere, then shuffle": the deck it went
        // into is flagged, and the executor announces it. A sequence of
        // `0` would put it on top and owe nothing.
        assert!(
            f.messages
                .iter()
                .any(|m| matches!(m, Message::ShuffleDeck { player: 1 })),
            "the deck it went back into was shuffled: {:?}",
            f.messages
                .iter()
                .filter(|m| matches!(m, Message::ShuffleDeck { .. }))
                .collect::<Vec<_>>()
        );
    }

    /// **The prompt is the to-deck message, to the activating player.**
    #[test]
    fn the_prompt_is_the_to_deck_message() {
        let (mut f, td, _) = field(&[MON, MON, MON, MON]);
        let e = effect_of(&f, td);
        resolve(&mut f, e, 0);
        assert!(
            f.messages.iter().any(|m| matches!(
                m,
                Message::Hint { kind, player: 0, value }
                    if *kind == hint::SELECTMSG && *value == hintmsg::TODECK
            )),
            "HINTMSG_TODECK to the activating player"
        );
    }

    /// **A hand with no monster in it is still shown, and still
    /// shuffled.** The reveal comes before the filter, and the shuffle
    /// happens because the hand has been read — not because anything was
    /// taken.
    #[test]
    fn a_monsterless_hand_is_still_shown_and_shuffled() {
        let (mut f, td, hand) = field(&[SPELL, SPELL, SPELL, SPELL]);
        let e = effect_of(&f, td);
        let run = resolve(&mut f, e, 0);
        let codes: Vec<u32> = hand.iter().map(|&c| f.cards[c].data.code).collect();
        assert_eq!(run.confirms.len(), 1, "it was still shown");
        assert_eq!(run.confirms[0], (0, codes));
        assert!(run.asked.is_empty(), "nothing to choose");
        for &c in &hand {
            assert_eq!(f.cards[c].current.location, location::HAND, "nothing taken");
        }
        assert!(
            run.hand_shuffles.contains(&1),
            "and the hand was shuffled anyway: {:?}",
            run.hand_shuffles
        );
    }

    /// **The hand is shuffled after a card is taken too**, and it is the
    /// hand that was read — the opponent's.
    #[test]
    fn the_read_hand_is_shuffled() {
        let (mut f, td, _) = field(&[MON, MON, SPELL, SPELL]);
        let e = effect_of(&f, td);
        let run = resolve(&mut f, e, 0);
        assert!(
            run.hand_shuffles.contains(&1),
            "the opponent's hand: {:?}",
            run.hand_shuffles
        );
        assert!(
            !run.hand_shuffles.contains(&0),
            "and not the activating player's: {:?}",
            run.hand_shuffles
        );
    }

    /// **The operation reads the player the chain named**, not its own
    /// argument — proved by naming the other one.
    #[test]
    fn the_operation_reads_the_named_player() {
        let (mut f, td, _) = field(&[MON, MON, MON, MON]);
        let mine: Vec<CardId> = (0..4)
            .map(|i| put(&mut f, 0, 6_100 + i, MON, location::HAND, i))
            .collect();
        let e = effect_of(&f, td);
        let mut ch = Chain::new(e, Event::new(code::FREE_CHAIN));
        ch.triggering_player = 0;
        ch.chain_count = 1;
        ch.chain_id = 11;
        // Named player 1, so player 1's *opponent* — player 0 — is read.
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
            .filter(|&&c| f.cards[c].current.location == location::DECK)
            .count();
        assert_eq!(mine_gone, 1, "player 0's hand was the one read");
    }
}

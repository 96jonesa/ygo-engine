//! Magical Merchant — `c32362575.lua`.
//!
//! The twenty-sixth card, and the first to **excavate**: it digs through
//! the top of its own deck until it finds a Spell or Trap, takes that,
//! and sends everything it dug past to the graveyard.
//!
//! ## It finds the card by arithmetic, not by digging
//!
//! ```lua
//! local g = Duel.GetMatchingGroup(Card.IsSpellTrap,tp,LOCATION_DECK,0,nil)
//! local dcount = Duel.GetFieldGroupCount(tp,LOCATION_DECK,0)
//! ... for tc in g:Iter() do if tc:GetSequence()>seq then seq=...; spcard=tc end end
//! Duel.ConfirmDecktop(tp,dcount-seq)
//! ```
//!
//! The script never walks the deck one card at a time. It takes *every*
//! Spell/Trap in the deck, keeps the one with the **highest sequence**,
//! and computes how deep that is. A deck is stored bottom-first, so the
//! highest sequence is the card nearest the top, and `dcount - seq` is
//! how many cards from the top down to and including it.
//!
//! That reads backwards until the storage order is in view, and getting
//! it wrong gives a card that digs from the bottom.
//!
//! ## Two arms, and they excavate different amounts
//!
//! If the found card can go to the hand it is taken, and the cards
//! **above** it are discarded: `dcount - seq - 1`, one fewer, because the
//! card itself left by another route. If it cannot, the whole dig
//! including it is discarded: `dcount - seq`. An off-by-one here would
//! either mill the taken card or leave one behind.
//!
//! ## `DisableShuffleCheck` before the hand
//!
//! Taking a card out of a deck sets `shuffle_deck_check`, and this card
//! is about to `ShuffleHand` instead — the deck has been read from the
//! top, so shuffling it would defeat the reveal the opponent just got.
//! The flag is suppressed for the one move.
//!
//! ## An empty result still reveals
//!
//! With no Spell or Trap anywhere in the deck, the card shows the
//! **whole deck** and shuffles it. Nothing is taken and nothing is
//! milled — but the reveal happens, which is the card's cost to its
//! controller.
//!
//! Every call here is to `script_api`, by the rule in `cards/mod.rs`.

use crate::board::location;
use crate::card::reason;
use crate::effect::{effect_type, Ctx, Yield};
use crate::event::{category, CardId};
use crate::field::Field;
use crate::script_api as api;

pub const CODE: u32 = 32_362_575;

pub fn initial_effect(f: &mut Field, c: CardId) {
    // Excavate cards until you find a Spell/Trap
    let e1 = api::create_effect(f, c);
    api::set_description(f, e1, api::stringid(CODE, 0));
    api::set_category(
        f,
        e1,
        category::TOHAND | category::SEARCH | category::DECKDES,
    );
    api::set_type(f, e1, effect_type::SINGLE | effect_type::FLIP);
    api::set_operation(f, e1, operation);
    api::register_effect(f, c, e1, false);
}

const DECK: u32 = location::DECK as u32;

fn spell_trap(f: &mut Field, c: CardId) -> bool {
    api::is_spell_trap(f, c)
}

fn operation(f: &mut Field, ctx: &Ctx) -> Yield {
    let tp = ctx.player;
    let dcount = api::get_field_group_count(f, tp, DECK, 0);
    if dcount == 0 {
        return api::done();
    }
    // The dig reads the deck from the top, all the way down in the worst
    // case: in solver mode that is a chance node, settled by the host
    // before anything is looked at.
    api::reveal_deck_request(f, tp, dcount);
    api::suspend(move |f, ctx| dig(f, ctx.player))
}

/// The dig itself, once the deck's order is settled.
fn dig(f: &mut Field, tp: u8) -> Yield {
    let g = api::get_matching_group(f, Some(&spell_trap), tp, DECK, 0, api::Except::None);
    let dcount = api::get_field_group_count(f, tp, DECK, 0);
    if g.is_empty() {
        // Nothing to find: the whole deck is shown, and shuffled.
        api::confirm_deck_top(f, tp, dcount);
        api::shuffle_deck(f, tp);
        return api::done();
    }
    // The highest sequence is the one nearest the top.
    let mut seq: i64 = -1;
    let mut found: Option<CardId> = None;
    for &tc in &g {
        let s = i64::from(api::get_sequence(f, tc));
        if s > seq {
            seq = s;
            found = Some(tc);
        }
    }
    let Some(found) = found else {
        return api::done();
    };
    let depth = dcount as i64 - seq;
    api::confirm_deck_top(f, tp, depth as usize);
    if api::is_able_to_hand(f, found, None) {
        // The deck is about to be read from the top; do not let it be
        // shuffled underneath that.
        api::disable_shuffle_check(f, true);
        api::send_to_hand(f, vec![found], None, reason::EFFECT);
        api::suspend(move |f, ctx| {
            let tp = ctx.player;
            // One fewer: the found card left by another route.
            api::discard_deck(f, tp, (depth - 1) as u16, reason::EFFECT | reason::EXCAVATE);
            api::suspend(move |f, ctx| {
                let tp = ctx.player;
                api::confirm_cards(f, 1 - tp, vec![found]);
                api::shuffle_hand(f, tp);
                api::done()
            })
        })
    } else {
        // It stays where it is, so the whole dig goes.
        api::discard_deck(f, tp, depth as u16, reason::EFFECT | reason::EXCAVATE);
        api::suspend(|_, _| api::done())
    }
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

    const MON: u32 = card_type::MONSTER | card_type::NORMAL;
    const SPELL: u32 = card_type::SPELL;

    /// Put a card on top of `owner`'s deck. The pile is stored
    /// bottom-first, so each call is a card *above* the last.
    fn on_deck(f: &mut Field, owner: u8, code_: u32, type_: u32) -> CardId {
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
        f.add_card(owner, id, location::DECK, 0, false);
        id
    }

    /// `tp`'s Main Phase 1 with the merchant face-up in the monster row,
    /// and a deck built **bottom-first** from `deck`.
    fn field_as(tp: u8, deck: &[u32]) -> (Field, CardId, Vec<CardId>) {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        f.infos.turn_player = tp;
        f.infos.phase = phases::MAIN1;
        let mut m = Card::with_data(crate::cards::card_data(CODE).expect("printed data"), tp);
        m.current.controller = tp;
        m.set_status(status::EFFECT_ENABLED, true);
        let mm = f.new_card(m);
        f.add_card(tp, mm, location::MZONE, 0, false);
        f.cards[mm].current.position = position::FACEUP_ATTACK;
        f.initialize_card(mm);
        let cards = deck
            .iter()
            .enumerate()
            .map(|(i, &ty)| on_deck(&mut f, tp, 7_000 + i as u32, ty))
            .collect();
        (f, mm, cards)
    }

    fn field(deck: &[u32]) -> (Field, CardId, Vec<CardId>) {
        field_as(0, deck)
    }

    fn effect_of(f: &Field, mm: CardId) -> crate::event::EffectId {
        f.cards[mm].single_effect.equal_range(code::FLIP)[0]
    }

    #[derive(Debug, PartialEq, Eq)]
    struct Run {
        confirms: Vec<(u8, Vec<u32>)>,
        deck_tops: Vec<(u8, Vec<u32>)>,
        hand_shuffles: Vec<u8>,
        deck_shuffles: Vec<u8>,
    }

    fn resolve_as(f: &mut Field, tp: u8, e: crate::event::EffectId) -> Run {
        let mut ch = Chain::new(e, Event::new(code::FLIP));
        ch.triggering_player = tp;
        ch.chain_count = 1;
        ch.chain_id = 11;
        f.core.current_chain.push(ch);
        f.core.chain_solving = true;
        f.core.sub_solving_event.push_back(Event::new(code::FLIP));
        f.emplace(Kind::ExecuteOperation {
            resume: None,
            effect: e,
            player: tp,
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
                Status::Awaiting => panic!("nothing should be asked"),
                _ => break,
            }
        }
        let mut run = Run {
            confirms: Vec::new(),
            deck_tops: Vec::new(),
            hand_shuffles: Vec::new(),
            deck_shuffles: Vec::new(),
        };
        for m in &f.messages {
            match m {
                Message::ConfirmCards { player, codes } => {
                    run.confirms.push((*player, codes.clone()))
                }
                Message::ConfirmDeckTop { player, codes } => {
                    run.deck_tops.push((*player, codes.clone()))
                }
                Message::ShuffleHand { player, .. } => run.hand_shuffles.push(*player),
                Message::ShuffleDeck { player } => run.deck_shuffles.push(*player),
                _ => {}
            }
        }
        run
    }

    fn resolve(f: &mut Field, e: crate::event::EffectId) -> Run {
        resolve_as(f, 0, e)
    }

    /// **In solver mode the dig is a chance node.** Before the deck is
    /// looked at, the host is asked to settle its whole order — the
    /// question names every card, since the dig may go all the way down
    /// — and nothing has been revealed when it is asked. With the order
    /// kept, the resolution is the faithful mode's exactly.
    #[test]
    fn in_chance_mode_the_dig_asks_for_the_whole_deck_first() {
        let deck = [SPELL, MON, MON, SPELL, MON];
        let (mut plain, mm, _) = field(&deck);
        let e = effect_of(&plain, mm);
        let expected = resolve(&mut plain, e);

        let (mut f, mm, _) = field(&deck);
        let e = effect_of(&f, mm);
        f.set_chance_mode(true);
        let mut ch = Chain::new(e, Event::new(code::FLIP));
        ch.triggering_player = 0;
        ch.chain_count = 1;
        ch.chain_id = 11;
        f.core.current_chain.push(ch);
        f.core.chain_solving = true;
        f.core.sub_solving_event.push_back(Event::new(code::FLIP));
        f.emplace(Kind::ExecuteOperation {
            resume: None,
            effect: e,
            player: 0,
            subject: None,
            args: Vec::new(),
            was_disabled: false,
        });
        let mut asked = 0;
        for _ in 0..8192 {
            match f.process() {
                Status::Continue => {
                    if f.core.units.is_empty() && f.core.subunits.is_empty() {
                        break;
                    }
                }
                Status::Awaiting => {
                    asked += 1;
                    assert!(
                        matches!(
                            f.messages.last(),
                            Some(Message::SelectDeckTop {
                                player: 0,
                                count: 5
                            })
                        ),
                        "the whole deck: {:?}",
                        f.messages.last()
                    );
                    assert!(
                        !f.messages
                            .iter()
                            .any(|m| matches!(m, Message::ConfirmDeckTop { .. })),
                        "nothing revealed before the question"
                    );
                    f.core.returns.set(0);
                }
                _ => break,
            }
        }
        assert_eq!(asked, 1, "asked once");
        let mut run = Run {
            confirms: Vec::new(),
            deck_tops: Vec::new(),
            hand_shuffles: Vec::new(),
            deck_shuffles: Vec::new(),
        };
        for m in &f.messages {
            match m {
                Message::ConfirmCards { player, codes } => {
                    run.confirms.push((*player, codes.clone()))
                }
                Message::ConfirmDeckTop { player, codes } => {
                    run.deck_tops.push((*player, codes.clone()))
                }
                Message::ShuffleHand { player, .. } => run.hand_shuffles.push(*player),
                Message::ShuffleDeck { player } => run.deck_shuffles.push(*player),
                _ => {}
            }
        }
        assert_eq!(run, expected, "the faithful mode's resolution");
    }

    /// **One printed flip effect**, declaring all three of what it does.
    #[test]
    fn the_script_registers_a_flip_effect() {
        let (f, mm, _) = field(&[]);
        let ids = f.cards[mm].single_effect.equal_range(code::FLIP);
        assert_eq!(ids.len(), 1);
        let e = f.effects.get(ids[0]).unwrap();
        assert!(e.is_type(effect_type::SINGLE));
        assert!(e.is_type(effect_type::FLIP));
        assert!(e.is_flag(crate::effect::flag::INITIAL));
        assert_eq!(
            e.category,
            category::TOHAND | category::SEARCH | category::DECKDES,
            "it takes a card, it searches, and it mills"
        );
        assert_eq!(e.description, api::stringid(CODE, 0));
    }

    /// **It digs to the *nearest* Spell, not the deepest.**
    ///
    /// The deck here is, bottom to top: Spell, monster, monster, Spell,
    /// monster. The Spell nearest the top is the fourth card up, so the
    /// dig is two cards deep — the monster on top, then that Spell. The
    /// bottom Spell is never reached.
    #[test]
    fn it_digs_to_the_spell_nearest_the_top() {
        let (mut f, mm, deck) = field(&[SPELL, MON, MON, SPELL, MON]);
        let (deep_spell, near_spell, top) = (deck[0], deck[3], deck[4]);
        let e = effect_of(&f, mm);
        resolve(&mut f, e);

        assert_eq!(
            f.cards[near_spell].current.location,
            location::HAND,
            "the Spell nearest the top is taken"
        );
        assert_eq!(
            f.cards[top].current.location,
            location::GRAVE,
            "the monster above it is milled"
        );
        assert_eq!(
            f.cards[deep_spell].current.location,
            location::DECK,
            "and the deeper Spell is never reached"
        );
        for &c in &deck[1..3] {
            assert_eq!(
                f.cards[c].current.location,
                location::DECK,
                "nor anything below the one that was found"
            );
        }
        assert!(f.cards[top].reason & reason::EXCAVATE != 0, "dug past");
    }

    /// **The scan is its own deck only.** A deck of monsters with the
    /// opponent holding Spells is a fruitless dig: the whole deck is
    /// shown and shuffled, and nothing is taken.
    #[test]
    fn the_scan_does_not_reach_the_opponents_deck() {
        let (mut f, mm, deck) = field(&[MON, MON]);
        let theirs = on_deck(&mut f, 1, 7_500, SPELL);
        let e = effect_of(&f, mm);
        let run = resolve(&mut f, e);
        assert!(
            run.deck_shuffles.contains(&0),
            "nothing found, so the deck is shown and shuffled"
        );
        assert_eq!(
            f.cards[theirs].current.location,
            location::DECK,
            "their Spell is not in reach"
        );
        for &c in &deck {
            assert_eq!(f.cards[c].current.location, location::DECK);
        }
        assert!(run.confirms.is_empty(), "and nothing was taken");
    }

    /// **The dig is shown top-first, and only as deep as it went.**
    #[test]
    fn the_dig_is_shown_top_first() {
        let (mut f, mm, deck) = field(&[MON, SPELL, MON]);
        let e = effect_of(&f, mm);
        let codes: Vec<u32> = deck.iter().map(|&c| f.cards[c].data.code).collect();
        let run = resolve(&mut f, e);
        assert_eq!(run.deck_tops.len(), 1, "one reveal of the dig");
        assert_eq!(run.deck_tops[0].0, 0);
        assert_eq!(
            run.deck_tops[0].1,
            vec![codes[2], codes[1]],
            "the top card first, then the Spell under it — and no deeper"
        );
    }

    /// **The found card is revealed to the opponent and the hand is
    /// shuffled** — the deck has been read from the top, so the hand is
    /// what gets disturbed, not the deck.
    #[test]
    fn the_taken_card_is_revealed_and_the_hand_shuffled() {
        let (mut f, mm, deck) = field(&[MON, SPELL]);
        let spell_code = f.cards[deck[1]].data.code;
        let e = effect_of(&f, mm);
        let run = resolve(&mut f, e);
        assert_eq!(
            run.confirms,
            vec![(1u8, vec![spell_code])],
            "shown to the opponent"
        );
        assert!(
            run.hand_shuffles.contains(&0),
            "the hand is shuffled: {:?}",
            run.hand_shuffles
        );
        assert!(
            !run.deck_shuffles.contains(&0),
            "and the deck is not: {:?}",
            run.deck_shuffles
        );
    }

    /// **A deck with no Spell or Trap in it is shown whole and
    /// shuffled**, and nothing is taken or milled.
    #[test]
    fn a_deck_with_nothing_to_find_is_shown_and_shuffled() {
        let (mut f, mm, deck) = field(&[MON, MON, MON]);
        let e = effect_of(&f, mm);
        let codes: Vec<u32> = deck.iter().map(|&c| f.cards[c].data.code).collect();
        let run = resolve(&mut f, e);
        assert_eq!(run.deck_tops.len(), 1);
        assert_eq!(
            run.deck_tops[0].1,
            vec![codes[2], codes[1], codes[0]],
            "the whole deck, top first"
        );
        assert!(
            run.deck_shuffles.contains(&0),
            "and it is shuffled: {:?}",
            run.deck_shuffles
        );
        for &c in &deck {
            assert_eq!(f.cards[c].current.location, location::DECK, "nothing moved");
        }
        assert!(run.confirms.is_empty(), "nothing was taken to reveal");
    }

    /// **An empty deck does nothing at all.**
    #[test]
    fn an_empty_deck_does_nothing() {
        let (mut f, mm, _) = field(&[]);
        let e = effect_of(&f, mm);
        let run = resolve(&mut f, e);
        assert!(run.deck_tops.is_empty(), "nothing shown");
        assert!(run.deck_shuffles.is_empty(), "nothing shuffled");
        assert!(run.confirms.is_empty());
    }

    /// **A Spell that cannot be taken is milled with the rest of the
    /// dig.** The other arm: one card deeper than the taking arm, because
    /// the found card goes too.
    #[test]
    fn a_spell_that_cannot_be_taken_is_milled_with_the_dig() {
        let (mut f, mm, deck) = field(&[MON, SPELL, MON]);
        let (spell, top) = (deck[1], deck[2]);
        // It cannot leave for a hand.
        let mut ban = crate::effect::Effect::new(effect_type::SINGLE, code::CANNOT_TO_HAND);
        ban.owner = Some(spell);
        ban.handler = Some(spell);
        let ban = f.new_effect(ban);
        f.cards[spell]
            .single_effect
            .insert(code::CANNOT_TO_HAND, ban);

        let e = effect_of(&f, mm);
        let run = resolve(&mut f, e);
        assert_eq!(
            f.cards[spell].current.location,
            location::GRAVE,
            "the Spell goes to the graveyard with the dig"
        );
        assert_eq!(f.cards[top].current.location, location::GRAVE);
        assert_eq!(
            f.cards[deck[0]].current.location,
            location::DECK,
            "and the card under it stays"
        );
        assert!(run.confirms.is_empty(), "nothing was taken, so no reveal");
        assert!(run.hand_shuffles.is_empty(), "and no hand shuffle");
    }
}

//! Drawing cards.
//!
//! `field::process(Processors::Draw&)` and `is_player_can_draw`. Two steps,
//! and most of the work is in the first.
//!
//! ## The top of the deck is the *back* of the vector
//!
//! `list_main.back()` is the card drawn. A port that took the front would
//! draw from the bottom, and under `DUEL_PSEUDO_SHUFFLE` — where insertion
//! order is draw order — that is the difference between replaying a known
//! game and replaying its mirror image.
//!
//! ## A draw does not go through `remove_card`
//!
//! The card is taken off the deck **by hand**: the previous state is
//! snapshotted, `current` is blanked, and `add_card` puts it in the hand.
//! `remove_card` is not called.
//!
//! That is not an optimisation. `remove_card` would clear the field-effect
//! registration a second time and, more importantly, would record the card
//! in `just_sent_cards` through its own path — `Draw` records it explicitly
//! and only while a chain is running. Routing a draw through `remove_card`
//! changes what the simultaneity check in the gather sees.
//!
//! ## Running out of deck does not fail the draw
//!
//! `core.overdraw[playerid]` is set and the loop stops; the cards drawn so
//! far still arrive, and the *return* is how many were actually drawn rather
//! than how many were asked for. Losing the duel for an empty deck happens
//! later, in the win check — not here.

use crate::board::{location, position};
use crate::card::reason;
use crate::event::{code, CardId, PLAYER_NONE};
use crate::field::{timing, Field, Message};
use crate::processor::Kind;

impl Field {
    /// `field::is_player_affected_by_effect`.
    pub fn is_player_affected_by_effect(&self, playerid: u8, code_: u32) -> Option<usize> {
        // No copy of the range: `is_available` is read-only, so the walk can
        // borrow the index directly.
        self.field_effects
            .aura
            .equal_range(code_)
            .iter()
            .copied()
            .find(|&id| {
                self.effects
                    .get(id)
                    .is_some_and(|e| e.is_target_player(&self.cards, playerid))
                    && self.is_available(id)
            })
    }

    /// `field::is_player_can_draw`.
    pub fn is_player_can_draw(&mut self, playerid: u8) -> bool {
        self.is_player_affected_by_effect(playerid, code::CANNOT_DRAW)
            .is_none()
    }

    /// One step of `Draw`. Returns true when the unit is finished.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn draw_step(
        &mut self,
        step: u16,
        playerid: u8,
        count: &mut u32,
        why: u32,
        by: Option<usize>,
        reason_player: u8,
        drawn_set: &mut Vec<CardId>,
        chance_asked: &mut bool,
    ) -> bool {
        match step {
            0 => {
                // A rule draw ignores "cannot draw": the turn's draw is not
                // something an effect declines on the player's behalf.
                if why & reason::RULE == 0 && !self.is_player_can_draw(playerid) {
                    self.core.returns.set(0);
                    return true;
                }
                if *count == 0 {
                    self.core.returns.set(0);
                    return true;
                }
                // Solver mode: the top of the deck is about to be seen, and
                // that is a chance node. Ask the host to settle it, and
                // come back to this step — nothing has moved yet — once
                // it has answered. Asked only for a draw that will happen:
                // the refusals above come first, and an empty deck asks
                // nothing.
                if self.core.chance_mode && !*chance_asked {
                    *chance_asked = true;
                    let deck = self.players[playerid as usize].main.len() as u32;
                    let top = (*count).min(deck);
                    if top > 0 {
                        self.emplace(Kind::SelectDeckTop {
                            player: playerid,
                            count: top,
                        });
                        self.set_step(crate::processor::RESTART);
                        return false;
                    }
                }
                self.core.overdraw[playerid as usize] = false;

                let mut drawn = Vec::new();
                let mut public_count = 0u32;
                for _ in 0..*count {
                    let Some(&top) = self.players[playerid as usize].main.last() else {
                        // Out of deck: stop, and remember why.
                        self.core.overdraw[playerid as usize] = true;
                        break;
                    };
                    self.enable_field_effect(top, false);
                    self.cancel_field_effect(top);
                    self.players[playerid as usize].main.pop();
                    if !self.core.current_chain.is_empty() {
                        self.core.just_sent_cards.insert(top);
                    }

                    // Taken off the deck by hand rather than through
                    // `remove_card` — see the module note.
                    let c = &mut self.cards[top];
                    c.previous = c.current;
                    c.current.controller = PLAYER_NONE;
                    c.current.location = 0;
                    c.reason_effect = by;
                    c.reason_player = reason_player;
                    c.reason = why | reason::DRAW;

                    self.add_card(playerid, top, location::HAND, 0, false);
                    self.enable_field_effect(top, true);

                    let public = self.is_affected_by_effect(top, code::PUBLIC).is_some();
                    if public {
                        public_count += 1;
                    }
                    self.cards[top].current.position = if public {
                        position::FACEUP
                    } else {
                        position::FACEDOWN
                    };
                    drawn.push(top);
                    self.reset_card(top, crate::field::reset::TOHAND, crate::field::reset::EVENT);
                }

                self.core.hint_timing[playerid as usize] |= timing::DRAW + timing::TOHAND;
                self.adjust_instant();
                // The count becomes how many were *actually* drawn, which is
                // what the caller reads back.
                *count = drawn.len() as u32;
                drawn_set.clear();
                drawn_set.extend(&drawn);

                if drawn.is_empty() {
                    return false;
                }
                self.messages.push(Message::Draw {
                    player: playerid,
                    codes: drawn.iter().map(|&c| self.cards[c].data.code).collect(),
                });
                // A reversed deck is face-up, so drawing from one shows the
                // opponent what was taken — unless every card drawn was
                // public anyway, in which case they already knew.
                if self.core.deck_reversed && public_count < drawn.len() as u32 {
                    self.messages.push(Message::ConfirmCards {
                        player: 1 - playerid,
                        codes: drawn.iter().map(|&c| self.cards[c].data.code).collect(),
                    });
                    self.core
                        .revealed
                        .extend(drawn.iter().map(|&c| (1 - playerid, c)));
                    self.shuffle(playerid, location::HAND);
                }

                for &card in &drawn {
                    // A card drawn into a hand that is not its owner's gets
                    // a client hint, so the reader can see whose it is.
                    if self.cards[card].owner != self.cards[card].current.controller {
                        self.add_borrowed_card_hint(card);
                    }
                    for ev in [code::DRAW, code::TO_HAND, code::MOVE] {
                        self.raise_single_event(
                            card,
                            vec![],
                            ev,
                            by,
                            why,
                            reason_player,
                            playerid,
                            0,
                        );
                    }
                }
                self.process_single_event();
                let n = drawn.len() as u32;
                for ev in [code::DRAW, code::TO_HAND, code::MOVE] {
                    self.raise_event_over(drawn.clone(), ev, by, why, reason_player, playerid, n);
                }
                self.process_instant_event();
                false
            }
            1 => {
                self.core.operated_set = drawn_set.to_vec();
                self.core.returns.set(*count as i32);
                true
            }
            _ => true,
        }
    }

    /// Queue a draw. `field::draw` in the reference is this emplacement.
    pub fn draw(
        &mut self,
        playerid: u8,
        count: u32,
        why: u32,
        by: Option<usize>,
        reason_player: u8,
    ) {
        self.emplace(Kind::Draw {
            playerid,
            count,
            why,
            by,
            reason_player,
            drawn_set: Vec::new(),
            chance_asked: false,
        });
    }

    /// The marker effect put on a card that arrived in a hand other than its
    /// owner's. `EFFECT_FLAG_CANNOT_DISABLE | CLIENT_HINT`, description 67,
    /// and a reset mask of `RESET_EVENT + 0x1fe0000` — every way the card
    /// might leave.
    pub(crate) fn add_borrowed_card_hint(&mut self, card: CardId) {
        use crate::effect::{effect_type, flag, Effect};
        use crate::field::reset;
        let mut hint = Effect::new(effect_type::SINGLE, 0);
        hint.owner = Some(card);
        hint.handler = Some(card);
        hint.flag[0] = flag::CANNOT_DISABLE | flag::CLIENT_HINT;
        hint.description = 67;
        // The reference writes `RESET_EVENT + 0x1fe0000`. Spelled out as the
        // bits it is, which is how the missing `TURN_SET` was caught — a
        // literal would have hidden it.
        hint.reset_flag = reset::EVENT
            + reset::TURN_SET
            + reset::TOGRAVE
            + reset::REMOVE
            + reset::TEMP_REMOVE
            + reset::TOHAND
            + reset::TODECK
            + reset::LEAVE
            + reset::TOFIELD;
        debug_assert_eq!(
            hint.reset_flag - reset::EVENT,
            0x1fe_0000,
            "the decomposition must equal the reference's literal"
        );
        // `pcard->add_effect(deffect)`: through the registration, which
        // is what writes the client's `MSG_CARD_HINT` and files the resets.
        let hint = self.new_effect(hint);
        self.add_card_effect(card, hint);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{card_type, Card, CardData};
    use crate::effect::{effect_type, flag, Effect};
    use crate::field::reset;

    fn deck_of(f: &mut Field, player: u8, n: u32) -> Vec<CardId> {
        (0..n)
            .map(|i| {
                let c = f.new_card_nowhere(Card::with_data(
                    CardData {
                        code: i + 1,
                        type_: card_type::MONSTER,
                        ..Default::default()
                    },
                    player,
                ));
                f.add_card(player, c, location::DECK, 0, false);
                c
            })
            .collect()
    }

    /// Run a draw and report how many were actually drawn.
    ///
    /// The answer is `core.returns`, **not** the count parameter. The count
    /// is unit state that step 0 overwrites with the number drawn — but the
    /// two early-return paths (cannot draw, count of zero) set `returns` and
    /// leave the count alone. Reading the count there gives the number
    /// *asked for*, which is how this helper first reported a refused draw
    /// as a successful one.
    fn draw_now(f: &mut Field, player: u8, count: u32, why: u32) -> i32 {
        let mut n = count;
        let mut drawn = Vec::new();
        let mut asked = false;
        if !f.draw_step(0, player, &mut n, why, None, player, &mut drawn, &mut asked) {
            f.draw_step(1, player, &mut n, why, None, player, &mut drawn, &mut asked);
        }
        f.core.returns.get()
    }

    /// Drive the processor until a question is asked or the queue empties;
    /// the question asked, if any.
    fn run_until_asked(f: &mut Field) -> Option<Message> {
        use crate::processor::Status;
        for _ in 0..256 {
            if f.core.units.is_empty() && f.core.subunits.is_empty() {
                return None;
            }
            match f.process() {
                Status::Awaiting => return f.messages.last().cloned(),
                Status::Continue => {}
                Status::End => return None,
            }
        }
        panic!("the draw never settled");
    }

    /// **In solver mode a draw first asks for the deck's top.** Nothing
    /// has moved when the question is asked; the host settles the order —
    /// here, moving the bottom card to the top — and the draw then takes
    /// exactly what the host put there.
    #[test]
    fn in_chance_mode_a_draw_asks_for_the_deck_top_first() {
        let mut f = Field::new(8000);
        let deck = deck_of(&mut f, 0, 5);
        f.set_chance_mode(true);
        f.draw(0, 2, reason::RULE, None, 0);
        let asked = run_until_asked(&mut f);
        assert!(
            matches!(
                asked,
                Some(Message::SelectDeckTop {
                    player: 0,
                    count: 2
                })
            ),
            "the deck-top question, for two cards: {asked:?}"
        );
        assert!(f.players[0].hand.is_empty(), "nothing drawn yet");
        assert_eq!(f.players[0].main.len(), 5, "and the deck is whole");

        let order = vec![deck[1], deck[2], deck[3], deck[4], deck[0]];
        assert!(f.set_deck_order(0, &order));
        f.core.returns.set(0);
        assert!(run_until_asked(&mut f).is_none(), "no second question");
        let drawn = f.messages.iter().find_map(|m| match m {
            Message::Draw { player: 0, codes } => Some(codes.clone()),
            _ => None,
        });
        assert_eq!(
            drawn,
            Some(vec![1, 5]),
            "the card the host put on top came first, then the old top"
        );
        assert_eq!(f.players[0].hand, vec![deck[0], deck[4]]);
        assert_eq!(f.core.returns.get(), 2, "two drawn");
    }

    /// The question covers only what the deck can give, and is not asked
    /// at all when nothing will be drawn — an empty deck, or a refused
    /// draw — so the faithful mode's refusals come first.
    #[test]
    fn the_deck_top_question_is_capped_and_skipped_when_nothing_will_be_drawn() {
        let mut f = Field::new(8000);
        deck_of(&mut f, 0, 1);
        f.set_chance_mode(true);
        f.draw(0, 3, reason::RULE, None, 0);
        let asked = run_until_asked(&mut f);
        assert!(
            matches!(
                asked,
                Some(Message::SelectDeckTop {
                    player: 0,
                    count: 1
                })
            ),
            "capped at the one card there is: {asked:?}"
        );
        f.core.returns.set(0);
        assert!(run_until_asked(&mut f).is_none());
        assert_eq!(f.core.returns.get(), 1, "one drawn");
        assert!(f.core.overdraw[0], "and the shortfall is recorded");

        // Empty: no question, the overdraw path as in the faithful mode.
        f.draw(0, 1, reason::RULE, None, 0);
        assert!(
            run_until_asked(&mut f).is_none(),
            "an empty deck asks nothing"
        );
        assert_eq!(f.core.returns.get(), 0);

        // Refused: no question either.
        let mut g = Field::new(8000);
        let source = {
            let mut c = Card::with_data(
                CardData {
                    code: 99,
                    type_: card_type::MONSTER,
                    ..Default::default()
                },
                0,
            );
            c.current.controller = 0;
            c.current.position = crate::board::position::FACEUP_ATTACK;
            c.set_status(crate::card::status::EFFECT_ENABLED, true);
            let id = g.new_card(c);
            g.add_card(0, id, location::MZONE, 0, false);
            id
        };
        let mut e = Effect::new(effect_type::FIELD, code::CANNOT_DRAW);
        e.owner = Some(source);
        e.handler = Some(source);
        e.effect_owner = 0;
        e.flag[0] |= flag::PLAYER_TARGET;
        e.range = u16::from(location::MZONE);
        e.s_range = u16::from(location::MZONE);
        let e = g.new_effect(e);
        g.field_effects.aura.insert(code::CANNOT_DRAW, e);
        g.field_effects.indexer.insert(e);
        deck_of(&mut g, 0, 3);
        g.set_chance_mode(true);
        g.draw(0, 1, reason::EFFECT, None, 0);
        assert!(
            run_until_asked(&mut g).is_none(),
            "a refused draw asks nothing"
        );
        assert_eq!(g.core.returns.get(), 0);
        assert_eq!(g.players[0].main.len(), 3);
    }

    /// The top of the deck is the **back** of the vector. Under
    /// pseudo-shuffle, where insertion order is draw order, taking the front
    /// would replay a known game's mirror image.
    #[test]
    fn the_top_of_the_deck_is_the_back_of_the_vector() {
        let mut f = Field::new(8000);
        let deck = deck_of(&mut f, 0, 3);
        assert_eq!(f.players[0].main, deck);

        draw_now(&mut f, 0, 1, 0);
        assert_eq!(f.players[0].hand, vec![deck[2]], "the last inserted");
        assert_eq!(f.players[0].main, deck[..2].to_vec());
    }

    /// Running out of deck does not fail the draw: what was drawn arrives,
    /// the overdraw is recorded, and the count returned is what was actually
    /// taken.
    #[test]
    fn an_empty_deck_is_recorded_rather_than_refused() {
        let mut f = Field::new(8000);
        deck_of(&mut f, 0, 2);

        let drawn = draw_now(&mut f, 0, 5, 0);
        assert_eq!(drawn, 2, "two drawn, not five");
        assert_eq!(f.players[0].hand.len(), 2, "and they arrived");
        assert!(f.core.overdraw[0], "the overdraw is recorded for later");
        assert_eq!(f.core.returns.get(), 2);
    }

    /// Drawing from an already-empty deck draws nothing and still records.
    #[test]
    fn drawing_from_nothing_draws_nothing() {
        let mut f = Field::new(8000);
        assert_eq!(draw_now(&mut f, 0, 1, 0), 0);
        assert!(f.core.overdraw[0]);
    }

    /// A count of zero is refused before anything is touched.
    #[test]
    fn a_count_of_zero_does_nothing() {
        let mut f = Field::new(8000);
        deck_of(&mut f, 0, 3);
        assert_eq!(draw_now(&mut f, 0, 0, 0), 0);
        assert!(!f.core.overdraw[0], "not an overdraw, just nothing asked");
        assert_eq!(f.players[0].main.len(), 3);
    }

    /// `EFFECT_CANNOT_DRAW` stops an effect draw — but **not** a rule draw.
    /// The turn's draw is not something an effect declines on the player's
    /// behalf.
    #[test]
    fn cannot_draw_stops_an_effect_draw_but_not_the_turns() {
        let mut f = Field::new(8000);
        let source = {
            let mut c = Card::with_data(
                CardData {
                    code: 99,
                    type_: card_type::MONSTER,
                    ..Default::default()
                },
                0,
            );
            c.current.controller = 0;
            c.current.position = crate::board::position::FACEUP_ATTACK;
            c.set_status(crate::card::status::EFFECT_ENABLED, true);
            let id = f.new_card(c);
            f.add_card(0, id, location::MZONE, 0, false);
            id
        };
        let mut e = Effect::new(effect_type::FIELD, code::CANNOT_DRAW);
        e.owner = Some(source);
        e.handler = Some(source);
        e.effect_owner = 0;
        e.flag[0] |= flag::PLAYER_TARGET;
        e.range = u16::from(location::MZONE);
        e.s_range = u16::from(location::MZONE);
        let e = f.new_effect(e);
        f.field_effects.aura.insert(code::CANNOT_DRAW, e);
        f.field_effects.indexer.insert(e);

        deck_of(&mut f, 0, 3);
        assert_eq!(draw_now(&mut f, 0, 1, 0), 0, "an effect draw is stopped");
        assert_eq!(
            draw_now(&mut f, 0, 1, reason::RULE),
            1,
            "the turn's draw is not"
        );
    }

    /// A drawn card carries `REASON_DRAW`, which is what exempts it from the
    /// hand shuffle in `add_card`.
    #[test]
    fn a_drawn_card_carries_the_draw_reason_and_skips_the_shuffle() {
        let mut f = Field::new(8000);
        deck_of(&mut f, 0, 2);
        draw_now(&mut f, 0, 1, reason::EFFECT);

        let drawn = f.players[0].hand[0];
        assert_ne!(f.cards[drawn].reason & reason::DRAW, 0);
        assert_ne!(
            f.cards[drawn].reason & reason::EFFECT,
            0,
            "and the caller's"
        );
        assert!(
            !f.core.shuffle_hand_check[0],
            "a draw owes no hand shuffle — the opponent saw it arrive"
        );
    }

    /// The card remembers it was in the deck.
    #[test]
    fn a_drawn_card_remembers_where_it_was() {
        let mut f = Field::new(8000);
        deck_of(&mut f, 0, 2);
        draw_now(&mut f, 0, 1, 0);
        let drawn = f.players[0].hand[0];
        assert_eq!(f.cards[drawn].previous.location, location::DECK);
        assert_eq!(f.cards[drawn].current.location, location::HAND);
    }

    /// Drawing sets the draw and to-hand timings, which is what lets a
    /// trigger answer "when a card is drawn".
    #[test]
    fn drawing_opens_the_draw_timings() {
        let mut f = Field::new(8000);
        deck_of(&mut f, 0, 2);
        draw_now(&mut f, 0, 1, 0);
        assert_ne!(f.core.hint_timing[0] & timing::DRAW, 0);
        assert_ne!(f.core.hint_timing[0] & timing::TOHAND, 0);
    }

    /// A card drawn into a hand that is not its owner's is marked, so a
    /// reader can see whose it is.
    #[test]
    fn a_borrowed_card_is_marked() {
        let mut f = Field::new(8000);
        // Owned by player 1, sitting in player 0's deck.
        let c = f.new_card_nowhere(Card::with_data(
            CardData {
                code: 1,
                type_: card_type::MONSTER,
                ..Default::default()
            },
            1,
        ));
        f.add_card(0, c, location::DECK, 0, false);

        draw_now(&mut f, 0, 1, 0);
        assert!(
            !f.cards[c].indexer.is_empty(),
            "the borrowed-card hint was added"
        );
        let hint = *f.cards[c].indexer.iter().next().unwrap();
        let e = f.effects.get(hint).unwrap();
        assert_eq!(e.description, 67);
        assert!(e.is_flag(flag::CLIENT_HINT));
        assert_eq!(
            e.reset_flag - reset::EVENT,
            0x1fe_0000,
            "and the reference's reset mask, decomposed"
        );
    }

    /// An owned card gets no such mark.
    #[test]
    fn an_owned_card_is_not_marked() {
        let mut f = Field::new(8000);
        let deck = deck_of(&mut f, 0, 1);
        draw_now(&mut f, 0, 1, 0);
        assert!(f.cards[deck[0]].indexer.is_empty());
    }
}

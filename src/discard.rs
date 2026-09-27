//! Discarding: from the hand by choice, and from the deck by count.
//!
//! `field::process(Processors::DiscardHand&)` and
//! `field::process(Processors::DiscardDeck&)`. Two units that share a name
//! and almost nothing else.
//!
//! ## `DiscardHand` is a question; `DiscardDeck` is a move
//!
//! `DiscardHand` asks the player which cards, then hands the answer to
//! `send_to` and is finished — the whole unit is a prompt and a delegation,
//! and every rule about *how* a card reaches the graveyard lives in
//! `SendTo`.
//!
//! `DiscardDeck` asks nothing. It takes cards off the top of the deck by
//! count and moves them **itself**, with its own copy of the move loop
//! rather than a call to `send_to`. That duplication is the reference's,
//! and it is not gratuitous: milling reads `sendto_param.location` that
//! case 0 computed from each card's own redirects, so the cards in one
//! batch can land in four different places.
//!
//! ## The redirect is decided a step before the move
//!
//! Case 0 walks the doomed cards and writes `sendto_param.location`;
//! case 1 reads it back. Nothing between them can be assumed constant —
//! that is why the reference splits them, and why this does too. A port
//! that computed the destination inside the move loop would sample it
//! after whatever case 0's own writes (`reason`, `reason_effect`) provoked.
//!
//! ## Running out of deck is not an error
//!
//! `DiscardDeck` mills what it can. The loop breaks on an empty deck and
//! the *return* is how many actually moved, not how many were asked for —
//! `Draw`'s rule, for the same reason. A caller that assumed its count was
//! honoured would be wrong on the last few cards of a duel.
//!
//! ## The top of the deck is the back of the vector
//!
//! Case 0 walks `list_main` in **reverse** and case 1 pops the back. Both
//! are the top of the deck. Taking the front would mill from the bottom,
//! which under `DUEL_PSEUDO_SHUFFLE` is a different game rather than a
//! different shuffle.

use crate::board::{location, position};
use crate::card::{reason, status};
use crate::event::{code, CardId, PLAYER_NONE};
use crate::field::{global_flag, reset, Field, Message};
use crate::host_question::hint;
use crate::processor::Kind;

impl Field {
    /// Queue a discard from the hand.
    pub fn discard_hand(&mut self, playerid: u8, min: u8, max: u8, why: u32) {
        self.emplace(Kind::DiscardHand {
            playerid,
            min,
            max,
            reason: why,
        });
    }

    /// Queue a mill from the top of the deck.
    pub fn discard_deck(&mut self, playerid: u8, count: u16, why: u32) {
        self.emplace(Kind::DiscardDeck {
            playerid,
            count,
            reason: why,
            discarded: Vec::new(),
        });
    }

    /// One step of `DiscardHand`. Two cases: ask, then send.
    pub(crate) fn discard_hand_step(
        &mut self,
        step: u16,
        playerid: u8,
        min: u8,
        max: u8,
        why: u32,
    ) -> bool {
        match step {
            0 => {
                // 501 is "discard", 504 is "send to the graveyard". The
                // same unit does both, and only the *reason* distinguishes
                // them: a cost that discards and an effect that sends from
                // the hand are the same move with different prompts.
                self.messages.push(Message::Hint {
                    kind: hint::SELECTMSG,
                    player: playerid,
                    value: if why & reason::DISCARD != 0 { 501 } else { 504 },
                });
                self.emplace(Kind::SelectCard {
                    player: playerid,
                    cancelable: false,
                    min,
                    max,
                });
                false
            }
            1 => {
                if self.core.return_cards.list.is_empty() {
                    self.core.returns.set(0);
                } else {
                    let chosen: Vec<CardId> = self.core.return_cards.list.clone();
                    let (by, rp) = (self.core.reason_effect, self.core.reason_player);
                    // `playerid` is `PLAYER_NONE`: the graveyard a card
                    // reaches is its **owner's**, not the discarding
                    // player's, and `PLAYER_NONE` is how `send_to` is told
                    // to work that out per card.
                    self.send_to(
                        chosen,
                        by,
                        why,
                        rp,
                        PLAYER_NONE,
                        u16::from(location::GRAVE),
                        0,
                        position::FACEUP,
                        false,
                    );
                }
                true
            }
            _ => true,
        }
    }

    /// One step of `DiscardDeck`. Three cases: decide, move, report.
    pub(crate) fn discard_deck_step(
        &mut self,
        step: u16,
        playerid: u8,
        count: u16,
        why: u32,
        discarded: &mut Vec<CardId>,
    ) -> bool {
        match step {
            0 => self.discard_deck_decide(playerid, count, why),
            1 => self.discard_deck_move(playerid, count, why, discarded),
            2 => {
                self.core.operated_set = std::mem::take(discarded);
                self.core.returns.set(self.core.operated_set.len() as i32);
                true
            }
            _ => true,
        }
    }

    /// Case 0: where is each card going, and does the opponent get to see
    /// what is left on top?
    fn discard_deck_decide(&mut self, playerid: u8, count: u16, why: u32) -> bool {
        if self
            .is_player_affected_by_effect(playerid, code::CANNOT_DISCARD_DECK)
            .is_some()
        {
            self.core.operated_set.clear();
            self.core.returns.set(0);
            return true;
        }

        let (by, rp) = (self.core.reason_effect, self.core.reason_player);
        let doomed: Vec<CardId> = self.players[playerid as usize]
            .main
            .iter()
            .rev()
            .take(count as usize)
            .copied()
            .collect();
        for card in doomed {
            self.cards[card].sendto_param.location = location::GRAVE;
            self.cards[card].reason_effect = by;
            self.cards[card].reason_player = rp;
            self.cards[card].reason = why;
            // Only the low half is the destination; the high half carries
            // the redirecting player and is not wanted here.
            let redirect = self.destination_redirect(card, location::GRAVE, why) & 0xffff;
            if redirect != 0 {
                self.cards[card].sendto_param.location = redirect as u8;
            }
        }

        // What will be on top **after** the mill, announced before it
        // happens — and only when somebody is watching a face-up deck.
        if self.core.global_flag & global_flag::DECK_REVERSE_CHECK != 0 {
            let main = &self.players[playerid as usize].main;
            if main.len() > count as usize {
                let top = main[main.len() - 1 - count as usize];
                let faceup_defense = self.cards[top].current.position == position::FACEUP_DEFENSE;
                if self.core.deck_reversed || faceup_defense {
                    self.messages.push(Message::DeckTop {
                        player: playerid,
                        sequence: u32::from(count),
                        code: self.cards[top].data.code,
                        position: self.cards[top].current.position,
                    });
                }
            }
        }
        false
    }

    /// Case 1: take them off the deck and put them where case 0 decided.
    fn discard_deck_move(
        &mut self,
        playerid: u8,
        count: u16,
        why: u32,
        discarded: &mut Vec<CardId>,
    ) -> bool {
        let (by, rp) = (self.core.reason_effect, self.core.reason_player);
        let (mut tohand, mut todeck, mut tograve, mut removed) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        discarded.clear();

        for _ in 0..count {
            let Some(&card) = self.players[playerid as usize].main.last() else {
                break;
            };
            let dest = self.cards[card].sendto_param.location;
            self.discard_deck_reset(card, dest);

            // Allocated here, filled either side of the move — as the
            // reference does.
            let message = self.open_move_message(card);
            self.enable_field_effect(card, false);
            self.cancel_field_effect(card);
            self.players[playerid as usize].main.pop();
            if !self.core.current_chain.is_empty() {
                self.core.just_sent_cards.insert(card);
            }

            let c = &mut self.cards[card];
            c.previous = c.current;
            c.current.controller = PLAYER_NONE;
            c.current.location = 0;

            let owner = self.cards[card].owner;
            self.add_card(owner, card, dest, 0, false);
            self.enable_field_effect(card, true);
            // Face-up wherever it lands: a milled card is public.
            self.cards[card].current.position = position::FACEUP;
            self.close_move_message(message, card);

            let ev = match dest {
                location::HAND => {
                    if self.cards[card].owner != self.cards[card].current.controller {
                        self.add_borrowed_card_hint(card);
                    }
                    if !self.core.current_chain.is_empty() {
                        self.add_resolving_hand_hint(card);
                    }
                    tohand.push(card);
                    Some(code::TO_HAND)
                }
                location::DECK | location::EXTRA => {
                    todeck.push(card);
                    Some(code::TO_DECK)
                }
                location::GRAVE => {
                    tograve.push(card);
                    Some(code::TO_GRAVE)
                }
                location::REMOVED => {
                    removed.push(card);
                    Some(code::REMOVE)
                }
                _ => None,
            };
            // **The card's own reason fields, not `core`'s.** The group
            // events below read `core`; the reference sources the two
            // differently and they can disagree, so this does too.
            let c = &self.cards[card];
            let (cby, cwhy, crp) = (c.reason_effect, c.reason, c.reason_player);
            if let Some(ev) = ev {
                self.raise_single_event(card, vec![], ev, cby, cwhy, crp, 0, 0);
            }
            self.raise_single_event(card, vec![], code::MOVE, cby, cwhy, crp, 0, 0);
            discarded.push(card);
        }

        for (group, ev) in [
            (tohand, code::TO_HAND),
            (todeck, code::TO_DECK),
            (tograve, code::TO_GRAVE),
            (removed, code::REMOVE),
        ] {
            if !group.is_empty() {
                self.raise_event_over(group, ev, by, why, rp, 0, 0);
            }
        }
        self.raise_event_over(discarded.clone(), code::MOVE, by, why, rp, 0, 0);
        self.process_single_event();
        self.process_instant_event();
        self.adjust_instant();
        false
    }

    /// The reset a card takes on the way out of the deck, which depends on
    /// where it is going.
    ///
    /// The hand and deck cases also clear `STATUS_PROC_COMPLETE`: a card
    /// that goes back to a private zone has not finished being summoned,
    /// whatever it was doing before.
    fn discard_deck_reset(&mut self, card: CardId, dest: u8) {
        match dest {
            location::GRAVE => self.reset_card(card, reset::TOGRAVE, reset::EVENT),
            location::HAND => {
                self.reset_card(card, reset::TOHAND, reset::EVENT);
                self.cards[card].set_status(status::PROC_COMPLETE, false);
            }
            location::DECK => {
                self.reset_card(card, reset::TODECK, reset::EVENT);
                self.cards[card].set_status(status::PROC_COMPLETE, false);
            }
            location::REMOVED => {
                // A *temporary* banish is its own reset: the card is coming
                // back, so what it forgets is narrower.
                let level = if self.cards[card].reason & reason::TEMPORARY != 0 {
                    reset::TEMP_REMOVE
                } else {
                    reset::REMOVE
                };
                self.reset_card(card, level, reset::EVENT);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{card_type, Card, CardData};
    use crate::duel::phases;
    use crate::effect::{effect_type, flag, Effect};
    use crate::processor::Status;

    fn field() -> Field {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        f.infos.turn_player = 0;
        f.infos.phase = phases::MAIN1;
        f
    }

    /// `n` cards in `player`'s deck, bottom first — so the **last**
    /// returned is the top, which is what a mill takes.
    ///
    /// Every card is added at sequence 0, as `Draw`'s own helper does.
    /// Adding at sequence `i` instead builds the deck upside down, and
    /// then every assertion about "the top" names the wrong card while
    /// still looking right.
    fn deck_of(f: &mut Field, player: u8, n: u32) -> Vec<CardId> {
        (0..n)
            .map(|i| {
                let mut c = Card::with_data(
                    CardData {
                        code: 90000 + i,
                        type_: card_type::MONSTER,
                        level: 4,
                        ..Default::default()
                    },
                    player,
                );
                c.current.controller = player;
                let id = f.new_card(c);
                f.add_card(player, id, location::DECK, 0, false);
                let fid = f.next_field_id_raw();
                f.cards[id].fieldid = fid;
                f.cards[id].fieldid_r = fid;
                id
            })
            .collect()
    }

    fn hand_of(f: &mut Field, player: u8, n: u32) -> Vec<CardId> {
        (0..n)
            .map(|i| {
                let mut c = Card::with_data(
                    CardData {
                        code: 80000 + i,
                        type_: card_type::MONSTER,
                        level: 4,
                        ..Default::default()
                    },
                    player,
                );
                c.current.controller = player;
                let id = f.new_card(c);
                f.add_card(player, id, location::HAND, i, false);
                id
            })
            .collect()
    }

    /// An `EFFECT_FIELD` aura aimed at a player.
    ///
    /// Under `EFFECT_FLAG_ABSOLUTE_TARGET` the ranges are **absolute**:
    /// `s_range` names player 0 and `o_range` names player 1, whoever owns
    /// the effect. Setting `s_range` and registering under player 1 aims
    /// the aura at player 0 — which reads as the prohibition applying
    /// everywhere.
    fn player_aura(f: &mut Field, code_: u32, player: u8) -> crate::event::EffectId {
        let mut anchor = Card::with_data(
            CardData {
                code: 70000,
                type_: card_type::MONSTER,
                level: 4,
                ..Default::default()
            },
            player,
        );
        anchor.current.controller = player;
        anchor.set_status(status::EFFECT_ENABLED, true);
        let a = f.new_card(anchor);
        f.add_card(player, a, location::MZONE, 0, false);
        f.cards[a].current.position = position::FACEUP_ATTACK;
        let mut e = Effect::new(effect_type::FIELD, code_);
        e.owner = Some(a);
        e.handler = Some(a);
        e.flag[0] = flag::PLAYER_TARGET | flag::ABSOLUTE_TARGET;
        e.range = u16::from(location::MZONE);
        if player == 0 {
            e.s_range = 1;
        } else {
            e.o_range = 1;
        }
        let id = f.new_effect(e);
        f.add_effect(id, player);
        id
    }

    /// Run to a stop, answering nothing. A question is a setup error.
    fn run(f: &mut Field) -> Status {
        for _ in 0..512 {
            match f.process() {
                Status::Continue => continue,
                other => return other,
            }
        }
        panic!("did not settle");
    }

    mod discard_deck {
        use super::*;

        /// The top of the deck is the **back** of the vector. Milling two
        /// takes the two highest-numbered cards, not the two lowest.
        #[test]
        fn it_takes_from_the_top_of_the_deck() {
            let mut f = field();
            let deck = deck_of(&mut f, 0, 5);
            f.discard_deck(0, 2, reason::EFFECT);
            run(&mut f);
            assert_eq!(f.players[0].main.len(), 3, "three left in the deck");
            for &milled in &deck[3..] {
                assert_eq!(
                    f.cards[milled].current.location,
                    location::GRAVE,
                    "the top two are in the graveyard"
                );
            }
            for &kept in &deck[..3] {
                assert_eq!(f.cards[kept].current.location, location::DECK);
            }
        }

        /// **The return is how many actually moved.** Poisoned first, so a
        /// unit that never wrote it would fail rather than agree with an
        /// uninitialised zero.
        #[test]
        fn a_short_deck_mills_what_it_has_and_says_so() {
            let mut f = field();
            deck_of(&mut f, 0, 2);
            f.core.returns.set(-99);
            f.discard_deck(0, 5, reason::EFFECT);
            run(&mut f);
            assert_eq!(f.players[0].main.len(), 0, "the deck is empty");
            assert_eq!(f.core.returns.get(), 2, "two moved, not the five asked for");
            assert_eq!(f.core.operated_set.len(), 2);
        }

        /// An empty deck is not an error either.
        #[test]
        fn milling_from_nothing_mills_nothing() {
            let mut f = field();
            f.core.returns.set(-99);
            f.discard_deck(0, 3, reason::EFFECT);
            run(&mut f);
            assert_eq!(f.core.returns.get(), 0);
            assert!(f.core.operated_set.is_empty());
        }

        /// **`EFFECT_CANNOT_DISCARD_DECK` refuses outright**, and clears
        /// the operated set rather than leaving the last one there.
        #[test]
        fn the_prohibition_refuses_the_mill() {
            let mut f = field();
            let deck = deck_of(&mut f, 0, 5);
            player_aura(&mut f, code::CANNOT_DISCARD_DECK, 0);
            f.core.operated_set = vec![deck[0]];
            f.core.returns.set(-99);
            f.discard_deck(0, 2, reason::EFFECT);
            run(&mut f);
            assert_eq!(f.players[0].main.len(), 5, "nothing was milled");
            assert_eq!(f.core.returns.get(), 0);
            assert!(f.core.operated_set.is_empty(), "the stale set is cleared");
        }

        /// The prohibition is **per player**: the opponent's aura does not
        /// stop this player's mill.
        #[test]
        fn the_prohibition_is_read_for_the_milling_player() {
            let mut f = field();
            deck_of(&mut f, 0, 5);
            player_aura(&mut f, code::CANNOT_DISCARD_DECK, 1);
            f.discard_deck(0, 2, reason::EFFECT);
            run(&mut f);
            assert_eq!(f.players[0].main.len(), 3, "player 0 milled anyway");
        }

        /// **A redirect sends the milled card somewhere else**, and the
        /// destination is read per card rather than assumed to be the
        /// graveyard for the batch.
        #[test]
        fn a_to_hand_redirect_lands_in_the_hand_instead() {
            let mut f = field();
            let deck = deck_of(&mut f, 0, 3);
            let top = deck[2];
            let mut e = Effect::new(effect_type::SINGLE, code::TO_GRAVE_REDIRECT);
            e.owner = Some(top);
            e.handler = Some(top);
            e.value = i64::from(location::HAND);
            e.range = u16::from(location::DECK);
            let id = f.new_effect(e);
            f.cards[top]
                .single_effect
                .insert(code::TO_GRAVE_REDIRECT, id);
            f.cards[top].indexer.insert(id);

            f.discard_deck(0, 2, reason::EFFECT);
            run(&mut f);
            assert_eq!(
                f.cards[top].current.location,
                location::HAND,
                "the redirected card went to the hand"
            );
            assert_eq!(
                f.cards[deck[1]].current.location,
                location::GRAVE,
                "its neighbour in the same batch still went to the graveyard"
            );
        }

        /// A milled card lands **face-up** wherever it goes. Poisoned to
        /// face-down first, so the assertion cannot pass on a default.
        #[test]
        fn a_milled_card_is_face_up() {
            let mut f = field();
            let deck = deck_of(&mut f, 0, 2);
            for &c in &deck {
                f.cards[c].current.position = position::FACEDOWN_DEFENSE;
            }
            f.discard_deck(0, 1, reason::EFFECT);
            run(&mut f);
            assert_eq!(f.cards[deck[1]].current.position, position::FACEUP);
        }

        /// The reason travels with the card, which is what the events
        /// raised from the mill will read.
        #[test]
        fn the_reason_reaches_the_card() {
            let mut f = field();
            let deck = deck_of(&mut f, 0, 2);
            f.cards[deck[1]].reason = 0;
            f.discard_deck(0, 1, reason::EFFECT | reason::DISCARD);
            run(&mut f);
            assert_eq!(
                f.cards[deck[1]].reason,
                reason::EFFECT | reason::DISCARD,
                "the card carries the mill's reason"
            );
        }

        /// **A temporary banish takes the narrower reset.** A card being
        /// removed `REASON_TEMPORARY` is coming back, so it forgets less:
        /// `RESET_TEMP_REMOVE` rather than `RESET_REMOVE`.
        ///
        /// Observed through an effect flagged to reset on `REMOVE` — it
        /// survives the temporary form and not the permanent one.
        #[test]
        fn a_temporary_banish_spares_what_a_permanent_one_resets() {
            for (why, survives) in [
                (reason::EFFECT | reason::TEMPORARY, true),
                (reason::EFFECT, false),
            ] {
                let mut f = field();
                let deck = deck_of(&mut f, 0, 2);
                let top = deck[1];

                // The mill's destination is the graveyard, so the
                // redirect consulted is `TO_GRAVE_REDIRECT` — whatever it
                // redirects *to*.
                let mut r = Effect::new(effect_type::SINGLE, code::TO_GRAVE_REDIRECT);
                r.owner = Some(top);
                r.handler = Some(top);
                r.value = i64::from(location::REMOVED);
                r.range = u16::from(location::DECK);
                let rid = f.new_effect(r);
                f.cards[top]
                    .single_effect
                    .insert(code::TO_GRAVE_REDIRECT, rid);
                f.cards[top].indexer.insert(rid);

                let mut marker = Effect::new(effect_type::SINGLE, code::UPDATE_ATTACK);
                marker.owner = Some(top);
                marker.handler = Some(top);
                marker.reset_flag = reset::EVENT | reset::REMOVE;
                let mid = f.new_effect(marker);
                f.cards[top].single_effect.insert(code::UPDATE_ATTACK, mid);
                f.cards[top].indexer.insert(mid);

                f.discard_deck(0, 1, why);
                run(&mut f);
                assert_eq!(
                    f.cards[top].current.location,
                    location::REMOVED,
                    "reason {why:#x}: the card was banished"
                );
                assert_eq!(
                    f.cards[top].indexer.contains(&mid),
                    survives,
                    "reason {why:#x}: the REMOVE-reset effect"
                );
            }
        }

        /// **`STATUS_PROC_COMPLETE` is cleared on the way to a private
        /// zone**, and only there. A card milled to the graveyard keeps it.
        #[test]
        fn a_card_redirected_to_the_hand_forgets_it_was_summoned() {
            let mut f = field();
            let deck = deck_of(&mut f, 0, 3);
            let (to_hand, to_grave) = (deck[2], deck[1]);
            for &c in &[to_hand, to_grave] {
                f.cards[c].set_status(status::PROC_COMPLETE, true);
            }
            let mut e = Effect::new(effect_type::SINGLE, code::TO_GRAVE_REDIRECT);
            e.owner = Some(to_hand);
            e.handler = Some(to_hand);
            e.value = i64::from(location::HAND);
            e.range = u16::from(location::DECK);
            let id = f.new_effect(e);
            f.cards[to_hand]
                .single_effect
                .insert(code::TO_GRAVE_REDIRECT, id);
            f.cards[to_hand].indexer.insert(id);

            f.discard_deck(0, 2, reason::EFFECT);
            run(&mut f);
            assert!(
                !f.cards[to_hand].is_status(status::PROC_COMPLETE),
                "the hand-bound card forgot"
            );
            assert!(
                f.cards[to_grave].is_status(status::PROC_COMPLETE),
                "the graveyard-bound one did not"
            );
        }
    }

    mod discard_hand {
        use super::*;

        /// The unit's whole first case is a prompt: a hint, then the card
        /// question. **The hint's text depends on the reason** — 501 for a
        /// discard, 504 for anything else.
        #[test]
        fn the_prompt_says_discard_only_when_the_reason_does() {
            for (why, expected) in [(reason::DISCARD, 501u64), (reason::EFFECT, 504)] {
                let mut f = field();
                hand_of(&mut f, 0, 3);
                f.discard_hand(0, 1, 1, why);
                assert_eq!(f.process(), Status::Continue);
                let hint = f.messages.iter().find_map(|m| match m {
                    Message::Hint {
                        kind,
                        player,
                        value,
                        ..
                    } if *kind == hint::SELECTMSG => Some((*player, *value)),
                    _ => None,
                });
                assert_eq!(hint, Some((0, expected)), "reason {why:#x}");
            }
        }

        /// It asks, and the question is not cancelable: a discard already
        /// decided on is not declined at the prompt.
        /// **`min` and `max` are asked for in that order**, and they are
        /// deliberately different here: a question for exactly two cards
        /// cannot tell a swapped pair from a correct one.
        #[test]
        fn it_asks_for_the_cards_and_will_not_take_no() {
            let mut f = field();
            hand_of(&mut f, 0, 4);
            f.discard_hand(0, 1, 3, reason::DISCARD);
            assert_eq!(f.process(), Status::Continue);
            let asked = f.core.units.iter().chain(f.core.subunits.iter()).any(|u| {
                matches!(
                    u.kind,
                    Kind::SelectCard {
                        player: 0,
                        cancelable: false,
                        min: 1,
                        max: 3,
                    }
                )
            });
            assert!(asked, "a SelectCard for one-to-three, uncancelable");
        }

        /// **The chosen cards are sent.** The answer arrives in
        /// `return_cards`, and the unit's second case hands it to `SendTo`.
        #[test]
        fn the_chosen_cards_go_to_the_graveyard() {
            let mut f = field();
            let hand = hand_of(&mut f, 0, 3);
            f.discard_hand(0, 1, 1, reason::DISCARD);
            assert_eq!(f.process(), Status::Continue);
            // Stand in for the question: answer it directly.
            f.core
                .units
                .retain(|u| !matches!(u.kind, Kind::SelectCard { .. }));
            f.core
                .subunits
                .retain(|u| !matches!(u.kind, Kind::SelectCard { .. }));
            f.core.return_cards.list = vec![hand[1]];
            run(&mut f);
            assert_eq!(f.cards[hand[1]].current.location, location::GRAVE);
            assert_eq!(f.cards[hand[0]].current.location, location::HAND);
        }

        /// **A discarded card goes to its OWNER's graveyard**, which is
        /// why the send names `PLAYER_NONE` rather than the discarding
        /// player. The two coincide for every ordinary card, so this needs
        /// a borrowed one to say anything at all.
        #[test]
        fn a_borrowed_card_is_discarded_to_its_owners_graveyard() {
            let mut f = field();
            // Owned by player 1, sitting in player 0's hand.
            let mut c = Card::with_data(
                CardData {
                    code: 81234,
                    type_: card_type::MONSTER,
                    level: 4,
                    ..Default::default()
                },
                1,
            );
            c.current.controller = 0;
            let borrowed = f.new_card(c);
            f.add_card(0, borrowed, location::HAND, 0, false);

            f.discard_hand(0, 1, 1, reason::DISCARD);
            assert_eq!(f.process(), Status::Continue);
            f.core
                .units
                .retain(|u| !matches!(u.kind, Kind::SelectCard { .. }));
            f.core
                .subunits
                .retain(|u| !matches!(u.kind, Kind::SelectCard { .. }));
            f.core.return_cards.list = vec![borrowed];
            run(&mut f);

            assert_eq!(f.cards[borrowed].current.location, location::GRAVE);
            assert!(
                f.players[1].grave.contains(&borrowed),
                "in player 1's graveyard, not player 0's"
            );
            assert!(!f.players[0].grave.contains(&borrowed));
        }

        /// **An empty answer writes zero and sends nothing.** Poisoned
        /// first: without it the assertion passes on an untouched `returns`.
        #[test]
        fn an_empty_answer_is_recorded_rather_than_sent() {
            let mut f = field();
            let hand = hand_of(&mut f, 0, 2);
            f.discard_hand(0, 0, 2, reason::DISCARD);
            assert_eq!(f.process(), Status::Continue);
            f.core
                .units
                .retain(|u| !matches!(u.kind, Kind::SelectCard { .. }));
            f.core
                .subunits
                .retain(|u| !matches!(u.kind, Kind::SelectCard { .. }));
            f.core.return_cards.list.clear();
            f.core.returns.set(-99);
            // Step the unit's second case alone. Running to a stop would
            // let a `SendTo` queued in error settle and write its own
            // zero into `returns`, which is exactly the difference this
            // test exists to see.
            assert!(f.discard_hand_step(1, 0, 0, 2, reason::DISCARD));
            assert_eq!(f.core.returns.get(), 0, "the empty answer is recorded");
            assert!(
                !f.core
                    .units
                    .iter()
                    .chain(f.core.subunits.iter())
                    .any(|u| matches!(u.kind, Kind::SendTo { .. })),
                "and nothing is sent"
            );
            run(&mut f);
            for &c in &hand {
                assert_eq!(f.cards[c].current.location, location::HAND);
            }
        }
    }
}

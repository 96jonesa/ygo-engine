//! Moving cards: the field's positional bookkeeping.
//!
//! `field::add_card`, `remove_card`, `move_card` and `reset_sequence`. This
//! is the layer under every operation that puts a card somewhere — summons,
//! sends to the graveyard, activations placing a spell — and it is where the
//! two representations of "where a card is" are kept in step: the slot
//! arrays and pile vectors on one side, the `used_location` bitfield and the
//! card's own `current` on the other.
//!
//! ## Sequences are dense in piles and fixed in zones
//!
//! A card in a zone keeps the sequence it was put at. A card in a pile has
//! its sequence recomputed from its position in the vector, by
//! `reset_sequence`, whenever the pile changes — which is why every erase
//! from a pile is followed by one. Miss it and every card after the removed
//! one is addressed by a stale index.
//!
//! `reset_sequence` refuses to touch the field zones, which is the same rule
//! stated from the other side.

use crate::board::location;
use crate::card::{card_type, status, Card};
use crate::event::{code, CardId};
use crate::field::{Field, Message};

impl Field {
    /// `field::get_extra_deck_types` — which monsters live in the Extra
    /// Deck. A duel option adds Ritual monsters to the list.
    pub fn extra_deck_types(&self) -> u32 {
        let base = card_type::FUSION | card_type::SYNCHRO | card_type::XYZ | card_type::LINK;
        if self.is_flag(crate::duel::flags::EXTRA_DECK_RITUAL) {
            base | card_type::RITUAL
        } else {
            base
        }
    }

    /// `field::reset_sequence` — renumber a pile after it changed.
    ///
    /// Deliberately a no-op for the field zones: a card there keeps the
    /// sequence it was placed at, because the sequence *is* the seat.
    pub fn reset_sequence(&mut self, playerid: u8, loc: u8) {
        if loc & location::ONFIELD != 0 {
            return;
        }
        let Some(pile) = self.players[playerid as usize].pile(loc).cloned() else {
            return;
        };
        for (i, &card) in pile.iter().enumerate() {
            self.cards[card].current.sequence = i as u32;
        }
    }

    /// `field::add_card` — put a card somewhere it currently is not.
    ///
    /// Returns without doing anything if the card is already somewhere, or
    /// if the seat is unusable. Both are silent in the reference and stay
    /// silent here: callers treat "could not place" as an ordinary outcome.
    pub fn add_card(
        &mut self,
        playerid: u8,
        card: CardId,
        mut loc: u8,
        sequence: u32,
        pzone: bool,
    ) {
        if self.cards[card].current.location != 0 {
            return;
        }
        if !self.is_location_useable(playerid as usize, u16::from(loc), sequence) {
            return;
        }

        // Extra-deck monsters cannot be put in the hand or main deck, and
        // are redirected. The Fusion exception is explicit in the reference:
        // a Fusion *spell* is allowed to start in the extra deck.
        let extra_types = self.extra_deck_types();
        let is_extra = self.cards[card].is_extra_deck_monster(extra_types)
            || self.cards[card].data.is_type(card_type::FUSION);
        if is_extra {
            if loc & (location::HAND | location::DECK) != 0 {
                loc = location::EXTRA;
                self.cards[card].sendto_param.position = crate::board::position::FACEDOWN_DEFENSE;
            }
        } else if loc == location::EXTRA {
            // Only a face-up Pendulum monster may sit in the extra deck
            // without being one of its monsters; anything else goes to the
            // main deck instead.
            let faceup_pendulum = self.cards[card].data.is_type(card_type::PENDULUM)
                && self.cards[card].sendto_param.position & crate::board::position::FACEUP != 0;
            if !faceup_pendulum {
                loc = location::DECK;
                self.cards[card].sendto_param.position = crate::board::position::FACEDOWN_DEFENSE;
            }
        }

        self.cards[card].current.controller = playerid;
        self.cards[card].current.location = loc;
        let p = playerid as usize;

        match loc {
            location::MZONE => {
                self.players[p].mzone[sequence as usize] = Some(card);
                self.cards[card].current.sequence = sequence;
            }
            location::SZONE => {
                self.players[p].szone[sequence as usize] = Some(card);
                self.cards[card].current.sequence = sequence;
            }
            location::DECK => {
                // The sequence is not a position here but an *instruction*:
                // 0 means the top, 1 the bottom, anything else the top with
                // a shuffle owed.
                match sequence {
                    0 => {
                        self.players[p].main.push(card);
                        self.cards[card].current.sequence = self.players[p].main.len() as u32 - 1;
                    }
                    1 => {
                        self.players[p].main.insert(0, card);
                        self.reset_sequence(playerid, location::DECK);
                    }
                    _ => {
                        self.players[p].main.push(card);
                        self.cards[card].current.sequence = self.players[p].main.len() as u32 - 1;
                        if !self.core.shuffle_check_disabled {
                            self.core.shuffle_deck_check[p] = true;
                        }
                    }
                }
                self.cards[card].sendto_param.position = crate::board::position::FACEDOWN;
            }
            location::HAND => {
                self.players[p].hand.push(card);
                self.cards[card].current.sequence = self.players[p].hand.len() as u32 - 1;
                let public = self.is_affected_by_effect(card, code::PUBLIC).is_some();
                self.cards[card].sendto_param.position = if public {
                    crate::board::position::FACEUP
                } else {
                    crate::board::position::FACEDOWN
                };
                // A drawn card does not need the hand shuffled — the
                // opponent already knows one arrived and where from.
                let drawn = self.cards[card].reason & crate::card::reason::DRAW != 0;
                if !drawn && !self.core.shuffle_check_disabled {
                    self.core.shuffle_hand_check[p] = true;
                }
            }
            location::GRAVE => {
                self.players[p].grave.push(card);
                self.cards[card].current.sequence = self.players[p].grave.len() as u32 - 1;
            }
            location::REMOVED => {
                self.players[p].removed.push(card);
                self.cards[card].current.sequence = self.players[p].removed.len() as u32 - 1;
            }
            location::EXTRA => {
                // Face-up Pendulums live at the end of the extra deck, so
                // anything else is inserted before them.
                let faceup_pendulum = self.cards[card].data.is_type(card_type::PENDULUM)
                    && self.cards[card].sendto_param.position & crate::board::position::FACEUP != 0;
                let p_count = self.players[p].extra_p_count;
                if p_count == 0 || faceup_pendulum {
                    self.players[p].extra.push(card);
                } else {
                    let at = self.players[p].extra.len() - p_count;
                    self.players[p].extra.insert(at, card);
                }
                if faceup_pendulum {
                    self.players[p].extra_p_count += 1;
                }
                self.reset_sequence(playerid, location::EXTRA);
            }
            _ => {}
        }

        self.cards[card].current.pzone = pzone;
        self.apply_field_effect(card);
        self.cards[card].fieldid = self.next_field_id_raw();
        self.cards[card].fieldid_r = self.cards[card].fieldid;
        self.cards[card].turnid = self.infos.turn_id;
        if self
            .check_unique_onfield(
                card,
                self.cards[card].current.controller,
                u16::from(loc),
                None,
            )
            .is_some()
        {
            self.cards[card].unique_fieldid = u32::MAX;
        }
        if loc == location::MZONE {
            self.players[p].used_location |= 1 << sequence;
        }
        if loc == location::SZONE {
            self.players[p].used_location |= 256 << sequence;
        }
    }

    /// `field::remove_card` — take a card out of wherever it is.
    ///
    /// The card is left *nowhere*: controller `PLAYER_NONE`, location 0. It
    /// is `add_card`'s job to put it somewhere, and `move_card` calls the
    /// two in sequence. A card between the two is in a state no rule can
    /// see, which is why nothing may run in between.
    pub fn remove_card(&mut self, card: CardId) {
        let (controller, loc, sequence) = {
            let c = &self.cards[card];
            (c.current.controller, c.current.location, c.current.sequence)
        };
        if controller == crate::event::PLAYER_NONE || loc == 0 {
            return;
        }
        let p = controller as usize;
        match loc {
            location::MZONE => self.players[p].mzone[sequence as usize] = None,
            location::SZONE => self.players[p].szone[sequence as usize] = None,
            location::DECK => {
                self.players[p].main.remove(sequence as usize);
                self.reset_sequence(controller, location::DECK);
                if !self.core.shuffle_check_disabled {
                    self.core.shuffle_deck_check[p] = true;
                }
            }
            location::HAND => {
                self.players[p].hand.remove(sequence as usize);
                self.reset_sequence(controller, location::HAND);
            }
            location::GRAVE => {
                self.players[p].grave.remove(sequence as usize);
                self.reset_sequence(controller, location::GRAVE);
            }
            location::REMOVED => {
                self.players[p].removed.remove(sequence as usize);
                self.reset_sequence(controller, location::REMOVED);
            }
            location::EXTRA => {
                self.players[p].extra.remove(sequence as usize);
                self.reset_sequence(controller, location::EXTRA);
                let faceup_pendulum = self.cards[card].data.is_type(card_type::PENDULUM)
                    && self.cards[card].current.position & crate::board::position::FACEUP != 0;
                if faceup_pendulum {
                    self.players[p].extra_p_count -= 1;
                }
            }
            _ => {}
        }
        self.cancel_field_effect(card);
        if loc == location::MZONE {
            self.players[p].used_location &= !(1 << sequence);
        }
        if loc == location::SZONE {
            self.players[p].used_location &= !(256 << sequence);
        }
        // Only while a chain is running: this is what the simultaneity check
        // in the gather reads, and outside a chain there is nothing for it
        // to be simultaneous with.
        if !self.core.current_chain.is_empty() {
            self.core.just_sent_cards.insert(card);
        }

        let c = &mut self.cards[card];
        c.previous = c.current;
        c.current.controller = crate::event::PLAYER_NONE;
        c.current.location = 0;
        c.current.sequence = 0;
    }

    /// `field::swap_card` — exchange two cards' seats.
    ///
    /// Not two moves. A move goes through `remove_card`/`add_card` and so
    /// passes through *nowhere*, which raises leave-the-field bookkeeping
    /// and renumbers piles; two cards trading places must not do that. So
    /// when both are in the **same kind** of location the reference edits
    /// the slot arrays in place, and only falls back to remove-then-add
    /// when the two locations differ.
    ///
    /// Three refusals, all silent:
    ///
    /// 1. Either card is off the field.
    /// 2. A card is being asked to move to a seat that is not usable —
    ///    checked only when the seat actually **changes**, so a card
    ///    staying put is never refused for sitting where it already sits.
    /// 3. The two would collide: same player, same location, and one is
    ///    being sent to the seat the other is leaving *and* named as the
    ///    same target. (`p1 == p2 && l1 == l2 && (new1 == s2 || new2 ==
    ///    s1)` — a genuine exchange within one row is spelled by giving
    ///    each card the other's seat, which this rejects.)
    ///
    /// **`fieldid` is only re-stamped when the controller changes.** A
    /// card that merely slid along its own row is the same card as far as
    /// every "since when" rule is concerned; a card that changed hands is
    /// not.
    fn swap_card_seats(&mut self, card1: CardId, card2: CardId, new_s1: u32, new_s2: u32) -> bool {
        let (p1, l1, s1) = {
            let c = &self.cards[card1].current;
            (c.controller, c.location, c.sequence)
        };
        let (p2, l2, s2) = {
            let c = &self.cards[card2].current;
            (c.controller, c.location, c.sequence)
        };
        if l1 & location::ONFIELD == 0 || l2 & location::ONFIELD == 0 {
            return false;
        }
        if (new_s1 != s1 && !self.is_location_useable(p1 as usize, u16::from(l1), new_s1))
            || (new_s2 != s2 && !self.is_location_useable(p2 as usize, u16::from(l2), new_s2))
        {
            return false;
        }
        if p1 == p2 && l1 == l2 && (new_s1 == s2 || new_s2 == s1) {
            return false;
        }

        if l1 == l2 {
            for (card, to_player, to_seq) in [(card1, p2, new_s2), (card2, p1, new_s1)] {
                let c = &mut self.cards[card];
                c.previous.controller = c.current.controller;
                c.previous.location = c.current.location;
                c.previous.sequence = c.current.sequence;
                c.previous.position = c.current.position;
                c.previous.pzone = c.current.pzone;
                c.current.controller = to_player;
                c.current.sequence = to_seq;
            }
            if p1 != p2 {
                for card in [card1, card2] {
                    self.cards[card].fieldid = self.next_field_id_raw();
                    let (con, loc) = {
                        let c = &self.cards[card].current;
                        (c.controller, c.location)
                    };
                    if self
                        .check_unique_onfield(card, con, u16::from(loc), None)
                        .is_some()
                    {
                        self.cards[card].unique_fieldid = u32::MAX;
                    }
                }
            }
            let (a, b) = (p1 as usize, p2 as usize);
            if l1 == location::MZONE {
                self.players[a].mzone[s1 as usize] = None;
                self.players[a].used_location &= !(1 << s1);
                self.players[b].mzone[s2 as usize] = None;
                self.players[b].used_location &= !(1 << s2);
                self.players[b].mzone[new_s2 as usize] = Some(card1);
                self.players[b].used_location |= 1 << new_s2;
                self.players[a].mzone[new_s1 as usize] = Some(card2);
                self.players[a].used_location |= 1 << new_s1;
            } else if l1 == location::SZONE {
                self.players[a].szone[s1 as usize] = None;
                self.players[a].used_location &= !(256 << s1);
                self.players[b].szone[s2 as usize] = None;
                self.players[b].used_location &= !(256 << s2);
                self.players[b].szone[new_s2 as usize] = Some(card1);
                self.players[b].used_location |= 256 << new_s2;
                self.players[a].szone[new_s1 as usize] = Some(card2);
                self.players[a].used_location |= 256 << new_s1;
            }
        } else {
            self.remove_card(card1);
            self.remove_card(card2);
            self.add_card(p2, card1, l2, new_s2, false);
            self.add_card(p1, card2, l1, new_s1, false);
        }
        true
    }

    /// `field::swap_card` — the seat exchange, plus the messages it writes.
    ///
    /// **Which message depends on whether either card moved seats.** Both
    /// keeping their own sequence is a pure exchange and gets one
    /// `MSG_SWAP`; otherwise each card gets its own `MSG_MOVE`, and the
    /// *order* of the two moves flips — card 1 first when only card 2
    /// changed seat, card 2 first otherwise.
    pub fn swap_card(&mut self, card1: CardId, card2: CardId, new_s1: u32, new_s2: u32) {
        let (code1, s1) = (
            self.cards[card1].data.code,
            self.cards[card1].current.sequence,
        );
        let (code2, s2) = (
            self.cards[card2].data.code,
            self.cards[card2].current.sequence,
        );
        // `info1`/`info2`: where each card was, taken before the seats move.
        let (info1, info2) = (self.get_info_location(card1), self.get_info_location(card2));
        if !self.swap_card_seats(card1, card2, new_s1, new_s2) {
            return;
        }
        if s1 == new_s1 && s2 == new_s2 {
            self.messages.push(Message::Swap {
                first: code1,
                second: code2,
            });
        } else {
            let order = if s1 == new_s1 {
                [(card1, code1, info1), (card2, code2, info2)]
            } else {
                [(card2, code2, info2), (card1, code1, info1)]
            };
            for (card, code, previous) in order {
                self.messages.push(Message::Move {
                    code,
                    previous,
                    current: self.get_info_location(card),
                    reason: 0,
                });
            }
        }
    }

    /// `field::move_card` — put a card somewhere, from wherever it is.
    ///
    /// Four cases, and only the last is "remove then add". The first three
    /// are moves *within* a location, which the reference handles specially
    /// rather than letting the card pass through nowhere — a card that left
    /// and re-entered would raise leave-the-field triggers that a
    /// rearrangement must not.
    pub fn move_card(
        &mut self,
        playerid: u8,
        card: CardId,
        mut loc: u8,
        sequence: u32,
        pzone: bool,
    ) -> bool {
        if !self.is_location_useable(playerid as usize, u16::from(loc), sequence) {
            return false;
        }
        let (preplayer, presequence, precurrent, prepzone) = {
            let c = &self.cards[card];
            (
                c.current.controller,
                c.current.sequence,
                c.current.location,
                c.current.pzone,
            )
        };

        let extra_types = self.extra_deck_types();
        if self.cards[card].is_extra_deck_monster(extra_types)
            && loc & (location::HAND | location::DECK) != 0
        {
            loc = location::EXTRA;
            self.cards[card].sendto_param.position = crate::board::position::FACEDOWN_DEFENSE;
        }

        if precurrent == 0 {
            self.add_card(playerid, card, loc, sequence, pzone);
            return true;
        }

        if precurrent == loc && prepzone == pzone {
            match loc {
                location::DECK if preplayer == playerid => {
                    self.move_within_deck(card, playerid, sequence);
                    return true;
                }
                location::DECK => self.remove_card(card),
                l if l & location::ONFIELD != 0 => {
                    return self.move_within_field(
                        card,
                        playerid,
                        loc,
                        sequence,
                        preplayer,
                        presequence,
                    );
                }
                location::HAND if preplayer == playerid => return false,
                location::HAND => self.remove_card(card),
                _ => return self.move_to_pile_bottom(card, loc),
            }
        } else {
            // A Pendulum monster leaving the field for the graveyard goes to
            // the extra deck face-up instead — but only if it got there by a
            // summon or activation that was not negated. A negated one was
            // never really on the field.
            let pendulum_to_extra = self.cards[card].data.is_type(card_type::PENDULUM)
                && loc == location::GRAVE
                && self.is_capable_send_to_extra(card, playerid)
                && ((precurrent == location::MZONE
                    && !self.cards[card].is_status(status::SUMMON_DISABLED))
                    || (precurrent == location::SZONE
                        && !self.cards[card].is_status(status::ACTIVATE_DISABLED)));
            if pendulum_to_extra {
                loc = location::EXTRA;
                self.cards[card].sendto_param.position = crate::board::position::FACEUP_DEFENSE;
            }
            self.remove_card(card);
        }
        self.add_card(playerid, card, loc, sequence, pzone);
        true
    }

    /// Rearranging within one player's deck: top, bottom, or top-and-shuffle.
    fn move_within_deck(&mut self, card: CardId, playerid: u8, sequence: u32) {
        let p = playerid as usize;
        let preplayer = self.cards[card].current.controller as usize;
        let at = self.cards[card].current.sequence as usize;
        let message = self.open_move_message(card);
        self.players[preplayer].main.remove(at);
        match sequence {
            0 => self.players[p].main.push(card),
            1 => self.players[p].main.insert(0, card),
            _ => {
                self.players[p].main.push(card);
                if !self.core.shuffle_check_disabled {
                    self.core.shuffle_deck_check[p] = true;
                }
            }
        }
        self.reset_sequence(playerid, location::DECK);
        self.cards[card].previous.controller = preplayer as u8;
        self.cards[card].current.controller = playerid;
        self.close_move_message(message, card);
    }

    /// Moving between seats on the field, possibly across the table.
    ///
    /// The surprising part is at the end: a move *within* one player's side
    /// writes a message and leaves `fieldid` alone, while a move that
    /// changes controller writes none and gives the card a **new** field id
    /// plus a fresh uniqueness check. The reference expresses that as
    /// `if(message) ... else ...`, so the presence of a client message
    /// decides a rules-visible identity change. Kept as the reference has
    /// it, because the coupling is the behaviour.
    fn move_within_field(
        &mut self,
        card: CardId,
        playerid: u8,
        loc: u8,
        sequence: u32,
        preplayer: u8,
        presequence: u32,
    ) -> bool {
        if playerid == preplayer && sequence == presequence {
            return false;
        }
        let p = playerid as usize;
        let occupied = if loc == location::MZONE {
            sequence as usize >= self.players[p].mzone.len()
                || self.players[p].mzone[sequence as usize].is_some()
        } else {
            sequence as usize >= self.players[p].szone.len()
                || self.players[p].szone[sequence as usize].is_some()
        };
        if occupied {
            return false;
        }

        let same_side = preplayer == playerid;
        let message = same_side.then(|| self.open_move_message(card));
        if !self.core.current_chain.is_empty() {
            self.core.just_sent_cards.insert(card);
        }
        self.cards[card].previous = self.cards[card].current;

        let pre = preplayer as usize;
        if loc == location::MZONE {
            self.players[pre].mzone[presequence as usize] = None;
            self.players[pre].used_location &= !(1 << presequence);
            self.players[p].mzone[sequence as usize] = Some(card);
            self.players[p].used_location |= 1 << sequence;
        } else {
            self.players[pre].szone[presequence as usize] = None;
            self.players[pre].used_location &= !(256 << presequence);
            self.players[p].szone[sequence as usize] = Some(card);
            self.players[p].used_location |= 256 << sequence;
        }
        self.cards[card].current.controller = playerid;
        self.cards[card].current.sequence = sequence;
        if let Some(at) = message {
            self.close_move_message(at, card);
        }

        if !same_side {
            self.cards[card].fieldid = self.next_field_id_raw();
            let controller = self.cards[card].current.controller;
            let current = self.cards[card].current.location;
            if self
                .check_unique_onfield(card, controller, u16::from(current), None)
                .is_some()
            {
                self.cards[card].unique_fieldid = u32::MAX;
            }
        }
        true
    }

    /// Moving a card to the bottom of the pile it is already in — which is
    /// what "send an already-buried card to the graveyard" means.
    ///
    /// Refused when it is already there, which is not merely an
    /// optimisation: the caller treats `false` as "nothing happened", and
    /// re-sending the top card would renumber the pile for no reason.
    fn move_to_pile_bottom(&mut self, card: CardId, loc: u8) -> bool {
        let controller = self.cards[card].current.controller;
        let p = controller as usize;
        let at = self.cards[card].current.sequence as usize;
        let len = match loc {
            location::GRAVE => self.players[p].grave.len(),
            location::REMOVED => self.players[p].removed.len(),
            _ => self.players[p].extra.len(),
        };
        // The extra deck has no such guard in the reference.
        if loc != location::EXTRA && at + 1 == len {
            return false;
        }
        let message = self.open_move_message(card);
        let pile = match loc {
            location::GRAVE => &mut self.players[p].grave,
            location::REMOVED => &mut self.players[p].removed,
            _ => &mut self.players[p].extra,
        };
        pile.remove(at);
        pile.push(card);
        self.reset_sequence(controller, loc);
        self.close_move_message(message, card);
        true
    }

    /// `card::apply_field_effect` — register the card's field effects now
    /// that it is somewhere they apply from.
    ///
    /// The range test has an exception that reads like a special case and is
    /// a rule: an optional trigger ranged to the hand applies **even when
    /// the card is out of range**, provided its code is not a phase event.
    /// That is how a hand trap is findable while sitting in a hand the
    /// effect is not ranged to — and excluding phase events is what stops a
    /// card in the hand answering every phase change.
    pub fn apply_field_effect(&mut self, card: CardId) {
        if self.cards[card].current.controller == crate::event::PLAYER_NONE {
            return;
        }
        for (_, effect) in self.cards[card]
            .field_effect
            .iter()
            .copied()
            .collect::<Vec<_>>()
        {
            if self.field_effect_applies(effect, card) {
                let owner = self.cards[card].current.controller;
                self.add_effect(effect, owner);
            }
        }
        let (unique_code, unique_location, loc) = {
            let c = &self.cards[card];
            (c.unique_code, c.unique_location, c.current.location)
        };
        if unique_code != 0 && unique_location & u16::from(loc) != 0 {
            let p = self.cards[card].current.controller as usize;
            if !self.core.unique_cards[p].contains(&card) {
                self.core.unique_cards[p].push(card);
            }
        }
    }

    /// `card::cancel_field_effect` — the exact mirror, including the same
    /// range exception. The two must agree: an effect added under the
    /// exception and removed without it stays registered on a card that has
    /// left.
    pub fn cancel_field_effect(&mut self, card: CardId) {
        if self.cards[card].current.controller == crate::event::PLAYER_NONE {
            return;
        }
        for (_, effect) in self.cards[card]
            .field_effect
            .iter()
            .copied()
            .collect::<Vec<_>>()
        {
            if self.field_effect_applies(effect, card) {
                self.remove_effect(effect);
            }
        }
        let (unique_code, unique_location, loc) = {
            let c = &self.cards[card];
            (c.unique_code, c.unique_location, c.current.location)
        };
        if unique_code != 0 && unique_location & u16::from(loc) != 0 {
            let p = self.cards[card].current.controller as usize;
            self.core.unique_cards[p].retain(|&c| c != card);
        }
    }

    /// The shared range test, so that apply and cancel cannot drift.
    fn field_effect_applies(&self, effect: crate::event::EffectId, card: CardId) -> bool {
        let Some(e) = self.effects.get(effect) else {
            return false;
        };
        if e.in_range(&self.cards, &self.cards[card]) {
            return true;
        }
        e.range & u16::from(location::HAND) != 0
            && e.is_type(crate::effect::effect_type::TRIGGER_O)
            && e.code & code::PHASE == 0
    }
}

/// Convenience for tests and callers that need a bare card in nowhere.
impl Field {
    pub fn new_card_nowhere(&mut self, card: Card) -> CardId {
        let mut card = card;
        card.current.controller = crate::event::PLAYER_NONE;
        card.current.location = 0;
        self.new_card(card)
    }
}

impl Field {
    /// `field::shuffle` — disturb a pile so nobody can track what is where.
    ///
    /// Three things about it are easy to assume wrongly.
    ///
    /// **`DUEL_PSEUDO_SHUFFLE` does not switch shuffling off.** The guard is
    /// `location == LOCATION_HAND || !is_flag(DUEL_PSEUDO_SHUFFLE)`, so the
    /// *hand* is shuffled whatever the flag says. Only the deck and extra
    /// deck are left alone. A configuration chosen to make deck order
    /// deterministic therefore still rolls for hands.
    ///
    /// **A fully face-up hand is not shuffled**, and the check is cleared
    /// rather than left pending: there is nothing hidden to protect, so the
    /// shuffle would only churn the order a client is showing.
    ///
    /// **The extra deck shuffles all but its tail.** Face-up Pendulum
    /// monsters live at the end and are public, so the upper bound excludes
    /// them.
    ///
    /// The swap loop is the reference's own, not a library shuffle: it draws
    /// `get_next_integer(i, upper_bound - 1)` for each `i`, and how many
    /// values it consumes is part of the observable behaviour.
    pub fn shuffle(&mut self, playerid: u8, loc: u8) {
        if loc & (location::HAND | location::DECK | location::EXTRA) == 0 {
            return;
        }
        let p = playerid as usize;
        if self.players[p].pile(loc).is_none_or(Vec::is_empty) {
            return;
        }

        if loc == location::HAND {
            let all_face_up = self.players[p]
                .hand
                .iter()
                .all(|&c| self.cards[c].current.is_faceup());
            if all_face_up {
                self.core.shuffle_hand_check[p] = false;
                return;
            }
        }

        if loc == location::HAND || !self.is_flag(crate::duel::flags::PSEUDO_SHUFFLE) {
            let len = self.players[p].pile(loc).map_or(0, Vec::len);
            let upper = if loc == location::EXTRA {
                len - self.players[p].extra_p_count
            } else {
                len
            };
            if upper > 1 {
                for i in 0..upper - 1 {
                    let r = self.rng.next_integer(i as i32, upper as i32 - 1) as usize;
                    if let Some(pile) = self.players[p].pile_mut(loc) {
                        pile.swap(i, r);
                    }
                }
                self.reset_sequence(playerid, loc);
            }
        }

        match loc {
            location::HAND => {
                self.core.shuffle_hand_check[p] = false;
                let codes = self.pile_codes(playerid, loc);
                self.messages.push(Message::ShuffleHand {
                    player: playerid,
                    codes,
                });
                // The client lost track of which hand card carries which
                // hint, so every client hint (not a player-targeted one)
                // is announced again, card by card in the new order.
                let hand = self.players[p].hand.clone();
                for card in hand {
                    let hints: Vec<(u64,)> = self.cards[card]
                        .indexer
                        .iter()
                        .filter_map(|&e| self.effects.get(e))
                        .filter(|e| {
                            e.is_flag(crate::effect::flag::CLIENT_HINT)
                                && !e.is_flag(crate::effect::flag::PLAYER_TARGET)
                        })
                        .map(|e| (e.description,))
                        .collect();
                    for (description,) in hints {
                        let info = self.get_info_location(card);
                        self.messages.push(Message::CardHint {
                            controller: info.controller,
                            location: info.location,
                            sequence: info.sequence,
                            kind: crate::field::chint::DESC_ADD,
                            value: description,
                        });
                    }
                }
            }
            location::EXTRA => {
                let codes = self.pile_codes(playerid, loc);
                self.messages.push(Message::ShuffleExtra {
                    player: playerid,
                    codes,
                });
            }
            _ => {
                self.core.shuffle_deck_check[p] = false;
                self.messages
                    .push(Message::ShuffleDeck { player: playerid });
            }
        }
    }

    fn pile_codes(&self, playerid: u8, loc: u8) -> Vec<u32> {
        self.players[playerid as usize]
            .pile(loc)
            .map(|pile| pile.iter().map(|&c| self.cards[c].data.code).collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::position;
    use crate::card::CardData;
    use crate::field::Field;

    fn bare(f: &mut Field, code_: u32, type_: u32) -> CardId {
        f.new_card_nowhere(Card::with_data(
            CardData {
                code: code_,
                type_,
                ..Default::default()
            },
            0,
        ))
    }

    fn monster(f: &mut Field) -> CardId {
        bare(f, 18036057, card_type::MONSTER | card_type::EFFECT)
    }

    mod add_card {
        use super::*;

        /// A card already somewhere is not placed again, silently.
        #[test]
        fn a_card_that_is_somewhere_is_not_placed_again() {
            let mut f = Field::new(8000);
            let c = monster(&mut f);
            f.add_card(0, c, location::MZONE, 0, false);
            f.add_card(0, c, location::MZONE, 1, false);
            assert_eq!(f.cards[c].current.sequence, 0, "still the first seat");
            assert!(f.players[0].mzone[1].is_none());
        }

        /// A taken seat is refused, also silently.
        #[test]
        fn a_taken_seat_is_refused() {
            let mut f = Field::new(8000);
            let first = monster(&mut f);
            let second = monster(&mut f);
            f.add_card(0, first, location::MZONE, 0, false);
            f.add_card(0, second, location::MZONE, 0, false);
            assert_eq!(f.players[0].mzone[0], Some(first));
            assert_eq!(
                f.cards[second].current.location, 0,
                "the second card is still nowhere"
            );
        }

        /// Placing marks the seat in the bitfield as well as the array, and
        /// the two must agree — the bitfield is what zone counting reads.
        #[test]
        fn placing_marks_the_bitfield_and_the_array() {
            let mut f = Field::new(8000);
            let c = monster(&mut f);
            f.add_card(0, c, location::MZONE, 2, false);
            assert_eq!(f.players[0].mzone[2], Some(c));
            assert_ne!(f.players[0].used_location & (1 << 2), 0);
            assert!(!f.is_location_useable(0, u16::from(location::MZONE), 2));

            let s = bare(&mut f, 1, card_type::SPELL);
            f.add_card(0, s, location::SZONE, 1, false);
            assert_ne!(f.players[0].used_location & (256 << 1), 0);
        }

        /// In the deck the sequence is an instruction, not a position:
        /// 0 is the top, 1 the bottom, anything else the top with a shuffle
        /// owed.
        #[test]
        fn the_deck_sequence_is_an_instruction() {
            let mut f = Field::new(8000);
            let first = bare(&mut f, 1, card_type::MONSTER);
            let second = bare(&mut f, 2, card_type::MONSTER);
            let third = bare(&mut f, 3, card_type::MONSTER);

            f.add_card(0, first, location::DECK, 0, false);
            f.add_card(0, second, location::DECK, 0, false);
            assert_eq!(f.players[0].main, vec![first, second], "0 is the top");

            f.add_card(0, third, location::DECK, 1, false);
            assert_eq!(
                f.players[0].main,
                vec![third, first, second],
                "1 is the bottom"
            );
            assert_eq!(f.cards[first].current.sequence, 1, "and renumbers");

            let fourth = bare(&mut f, 4, card_type::MONSTER);
            assert!(!f.core.shuffle_deck_check[0]);
            f.add_card(0, fourth, location::DECK, 2, false);
            assert!(f.core.shuffle_deck_check[0], "anything else owes a shuffle");
        }

        /// A card arriving in the hand owes a hand shuffle — unless it was
        /// drawn, because the opponent already knows one arrived.
        #[test]
        fn a_drawn_card_owes_no_hand_shuffle() {
            let mut f = Field::new(8000);
            let c = monster(&mut f);
            f.add_card(0, c, location::HAND, 0, false);
            assert!(f.core.shuffle_hand_check[0]);

            let mut f = Field::new(8000);
            let c = monster(&mut f);
            f.cards[c].reason = crate::card::reason::DRAW;
            f.add_card(0, c, location::HAND, 0, false);
            assert!(!f.core.shuffle_hand_check[0]);
        }

        /// An extra-deck monster cannot be put in the hand or main deck.
        #[test]
        fn an_extra_deck_monster_is_redirected() {
            let mut f = Field::new(8000);
            let fusion = bare(&mut f, 1, card_type::MONSTER | card_type::FUSION);
            f.add_card(0, fusion, location::HAND, 0, false);
            assert_eq!(f.cards[fusion].current.location, location::EXTRA);
            assert_eq!(f.players[0].extra, vec![fusion]);
            assert!(f.players[0].hand.is_empty());
        }

        /// And an ordinary card cannot be put in the extra deck.
        #[test]
        fn an_ordinary_card_is_redirected_out_of_the_extra_deck() {
            let mut f = Field::new(8000);
            let c = monster(&mut f);
            f.add_card(0, c, location::EXTRA, 0, false);
            assert_eq!(f.cards[c].current.location, location::DECK);
        }

        /// Face-up Pendulums sit at the end of the extra deck, so anything
        /// else is inserted before them.
        #[test]
        fn face_up_pendulums_sit_at_the_end_of_the_extra_deck() {
            let mut f = Field::new(8000);
            let pendulum = bare(
                &mut f,
                1,
                card_type::MONSTER | card_type::PENDULUM | card_type::FUSION,
            );
            f.cards[pendulum].sendto_param.position = position::FACEUP_ATTACK;
            f.add_card(0, pendulum, location::EXTRA, 0, false);
            assert_eq!(f.players[0].extra_p_count, 1);

            let fusion = bare(&mut f, 2, card_type::MONSTER | card_type::FUSION);
            f.add_card(0, fusion, location::EXTRA, 0, false);
            assert_eq!(
                f.players[0].extra,
                vec![fusion, pendulum],
                "the fusion goes before the face-up pendulum"
            );
        }

        /// Every placed card gets a fresh field id, and `fieldid_r` starts
        /// equal to it.
        #[test]
        fn placing_stamps_a_field_id() {
            let mut f = Field::new(8000);
            let first = monster(&mut f);
            let second = monster(&mut f);
            f.add_card(0, first, location::MZONE, 0, false);
            f.add_card(0, second, location::MZONE, 1, false);
            assert!(f.cards[second].fieldid > f.cards[first].fieldid);
            assert_eq!(f.cards[first].fieldid_r, f.cards[first].fieldid);
        }
    }

    mod remove_card {
        use super::*;

        /// Removing leaves the card nowhere, and records where it was.
        #[test]
        fn removing_leaves_the_card_nowhere() {
            let mut f = Field::new(8000);
            let c = monster(&mut f);
            f.add_card(0, c, location::MZONE, 3, false);
            f.remove_card(c);

            assert_eq!(f.cards[c].current.location, 0);
            assert_eq!(f.cards[c].current.controller, crate::event::PLAYER_NONE);
            assert_eq!(f.cards[c].previous.location, location::MZONE);
            assert_eq!(f.cards[c].previous.sequence, 3);
            assert!(f.players[0].mzone[3].is_none());
            assert_eq!(f.players[0].used_location & (1 << 3), 0, "the seat is free");
        }

        /// Removing from a pile renumbers everything after it.
        #[test]
        fn removing_from_a_pile_renumbers_the_rest() {
            let mut f = Field::new(8000);
            let cards: Vec<_> = (0..3)
                .map(|i| bare(&mut f, i + 1, card_type::MONSTER))
                .collect();
            for &c in &cards {
                f.add_card(0, c, location::GRAVE, 0, false);
            }
            assert_eq!(f.cards[cards[2]].current.sequence, 2);

            f.remove_card(cards[0]);
            assert_eq!(f.players[0].grave, vec![cards[1], cards[2]]);
            assert_eq!(f.cards[cards[1]].current.sequence, 0);
            assert_eq!(
                f.cards[cards[2]].current.sequence, 1,
                "renumbered, not left stale"
            );
        }

        /// A card leaving *during a chain* is recorded as just sent, which
        /// is what the gather's simultaneity check reads. Outside a chain it
        /// is not, because there is nothing to be simultaneous with.
        #[test]
        fn leaving_during_a_chain_is_recorded() {
            use crate::chain::Chain;
            use crate::event::Event;

            let mut f = Field::new(8000);
            let c = monster(&mut f);
            f.add_card(0, c, location::MZONE, 0, false);
            f.remove_card(c);
            assert!(f.core.just_sent_cards.is_empty(), "no chain running");

            let mut f = Field::new(8000);
            let c = monster(&mut f);
            f.add_card(0, c, location::MZONE, 0, false);
            f.core
                .current_chain
                .push(Chain::new(0, Event::new(code::CHAINING)));
            f.remove_card(c);
            assert!(f.core.just_sent_cards.contains(&c));
        }
    }

    mod move_card {
        use super::*;

        /// Moving between seats keeps the card on the field throughout —
        /// it does not pass through nowhere, so nothing sees it leave.
        #[test]
        fn moving_between_seats_does_not_leave_the_field() {
            let mut f = Field::new(8000);
            let c = monster(&mut f);
            f.add_card(0, c, location::MZONE, 0, false);
            let before = f.cards[c].fieldid;

            assert!(f.move_card(0, c, location::MZONE, 2, false));
            assert_eq!(f.players[0].mzone[0], None);
            assert_eq!(f.players[0].mzone[2], Some(c));
            assert_eq!(f.players[0].used_location & 1, 0);
            assert_ne!(f.players[0].used_location & (1 << 2), 0);
            assert_eq!(
                f.cards[c].fieldid, before,
                "same side: the card keeps its identity"
            );
        }

        /// Moving across the table gives the card a **new** field id, where
        /// moving within a side does not. The reference couples that to
        /// whether a client message was written, which is surprising enough
        /// to be worth a test.
        #[test]
        fn crossing_the_table_is_a_new_identity() {
            let mut f = Field::new(8000);
            let c = monster(&mut f);
            f.add_card(0, c, location::MZONE, 0, false);
            let before = f.cards[c].fieldid;
            let messages = f.messages.len();

            assert!(f.move_card(1, c, location::MZONE, 0, false));
            assert_eq!(f.cards[c].current.controller, 1);
            assert_eq!(f.players[1].mzone[0], Some(c));
            assert!(f.cards[c].fieldid > before, "a new identity");
            assert_eq!(
                f.messages.len(),
                messages,
                "and no move message, which is what the reference ties it to"
            );
        }

        /// Moving to the seat it is already in is refused.
        #[test]
        fn moving_to_the_same_seat_is_refused() {
            let mut f = Field::new(8000);
            let c = monster(&mut f);
            f.add_card(0, c, location::MZONE, 1, false);
            assert!(!f.move_card(0, c, location::MZONE, 1, false));
        }

        /// Moving to an occupied seat is refused, leaving both where they
        /// were.
        #[test]
        fn moving_onto_an_occupied_seat_is_refused() {
            let mut f = Field::new(8000);
            let a = monster(&mut f);
            let b = monster(&mut f);
            f.add_card(0, a, location::MZONE, 0, false);
            f.add_card(0, b, location::MZONE, 1, false);
            assert!(!f.move_card(0, a, location::MZONE, 1, false));
            assert_eq!(f.players[0].mzone[0], Some(a));
            assert_eq!(f.players[0].mzone[1], Some(b));
        }

        /// Between locations it is remove-then-add, so the card does leave.
        #[test]
        fn moving_between_locations_leaves_and_arrives() {
            let mut f = Field::new(8000);
            let c = monster(&mut f);
            f.add_card(0, c, location::MZONE, 0, false);
            assert!(f.move_card(0, c, location::GRAVE, 0, false));
            assert_eq!(f.cards[c].current.location, location::GRAVE);
            assert_eq!(f.cards[c].previous.location, location::MZONE);
            assert!(f.players[0].mzone[0].is_none());
            assert_eq!(f.players[0].grave, vec![c]);
        }

        /// A card already at the bottom of its pile is not re-sent: the
        /// caller reads false as "nothing happened".
        #[test]
        fn a_card_already_at_the_bottom_is_not_moved() {
            let mut f = Field::new(8000);
            let a = bare(&mut f, 1, card_type::MONSTER);
            let b = bare(&mut f, 2, card_type::MONSTER);
            f.add_card(0, a, location::GRAVE, 0, false);
            f.add_card(0, b, location::GRAVE, 0, false);

            assert!(
                !f.move_card(0, b, location::GRAVE, 0, false),
                "already last"
            );
            assert!(f.move_card(0, a, location::GRAVE, 0, false), "not last");
            assert_eq!(f.players[0].grave, vec![b, a]);
            assert_eq!(f.cards[a].current.sequence, 1);
        }

        /// A card in nowhere is simply added.
        #[test]
        fn a_card_from_nowhere_is_added() {
            let mut f = Field::new(8000);
            let c = monster(&mut f);
            assert!(f.move_card(0, c, location::HAND, 0, false));
            assert_eq!(f.cards[c].current.location, location::HAND);
        }

        /// Moving within one player's deck rearranges rather than leaving.
        #[test]
        fn moving_within_the_deck_rearranges() {
            let mut f = Field::new(8000);
            let a = bare(&mut f, 1, card_type::MONSTER);
            let b = bare(&mut f, 2, card_type::MONSTER);
            f.add_card(0, a, location::DECK, 0, false);
            f.add_card(0, b, location::DECK, 0, false);
            assert_eq!(f.players[0].main, vec![a, b]);

            assert!(f.move_card(0, a, location::DECK, 0, false));
            assert_eq!(f.players[0].main, vec![b, a], "moved to the top");
            assert_eq!(f.cards[a].current.location, location::DECK, "never left");
        }
    }

    mod shuffling {
        use super::*;

        fn hand_of(f: &mut Field, n: usize, face_up: bool) -> Vec<CardId> {
            let mut out = Vec::new();
            for i in 0..n {
                let c = bare(f, i as u32 + 1, card_type::MONSTER);
                f.add_card(0, c, location::HAND, 0, false);
                f.cards[c].current.position = if face_up {
                    position::FACEUP_ATTACK
                } else {
                    position::FACEDOWN_DEFENSE
                };
                out.push(c);
            }
            out
        }

        /// `DUEL_PSEUDO_SHUFFLE` leaves the deck alone — and *only* the
        /// deck. This is the assumption worth pinning: the configuration
        /// this project runs under still rolls for hands.
        #[test]
        fn pseudo_shuffle_spares_the_deck_but_not_the_hand() {
            let mut f = Field::new(8000);
            assert!(f.is_flag(crate::duel::flags::PSEUDO_SHUFFLE));
            let deck: Vec<CardId> = (0..6)
                .map(|i| {
                    let c = bare(&mut f, i + 1, card_type::MONSTER);
                    f.add_card(0, c, location::DECK, 0, false);
                    c
                })
                .collect();
            f.shuffle(0, location::DECK);
            assert_eq!(f.players[0].main, deck, "deck order is untouched");

            let hand = hand_of(&mut f, 8, false);
            f.shuffle(0, location::HAND);
            assert_ne!(
                f.players[0].hand, hand,
                "the hand is shuffled even under pseudo-shuffle"
            );
        }

        /// A fully face-up hand has nothing to hide, so it is not shuffled —
        /// and the pending check is cleared rather than left set.
        #[test]
        fn a_face_up_hand_is_not_shuffled() {
            let mut f = Field::new(8000);
            let hand = hand_of(&mut f, 6, true);
            f.core.shuffle_hand_check[0] = true;

            f.shuffle(0, location::HAND);
            assert_eq!(f.players[0].hand, hand, "left alone");
            assert!(!f.core.shuffle_hand_check[0], "and the check is cleared");
            assert!(
                !f.messages
                    .iter()
                    .any(|m| matches!(m, Message::ShuffleHand { .. })),
                "and nothing is announced"
            );
        }

        /// One hidden card is enough to make the whole hand worth shuffling.
        #[test]
        fn one_hidden_card_is_enough() {
            let mut f = Field::new(8000);
            let hand = hand_of(&mut f, 8, true);
            f.cards[hand[3]].current.position = position::FACEDOWN_DEFENSE;
            f.shuffle(0, location::HAND);
            assert_ne!(f.players[0].hand, hand);
        }

        /// Shuffling renumbers, so a card's sequence still addresses it.
        #[test]
        fn shuffling_renumbers_the_pile() {
            let mut f = Field::new(8000);
            hand_of(&mut f, 8, false);
            f.shuffle(0, location::HAND);
            for (i, &c) in f.players[0].hand.iter().enumerate() {
                assert_eq!(
                    f.cards[c].current.sequence, i as u32,
                    "sequence must match position"
                );
            }
        }

        /// The extra deck shuffles all but its face-up Pendulum tail, which
        /// is public and stays put.
        #[test]
        fn the_extra_decks_face_up_tail_stays_put() {
            let mut f = Field::with_flags(8000, 0); // no pseudo-shuffle
            for i in 0..6 {
                let c = bare(&mut f, i + 1, card_type::MONSTER | card_type::FUSION);
                f.add_card(0, c, location::EXTRA, 0, false);
            }
            let pendulum = bare(
                &mut f,
                99,
                card_type::MONSTER | card_type::PENDULUM | card_type::FUSION,
            );
            f.cards[pendulum].sendto_param.position = position::FACEUP_ATTACK;
            f.add_card(0, pendulum, location::EXTRA, 0, false);
            assert_eq!(f.players[0].extra_p_count, 1);

            f.shuffle(0, location::EXTRA);
            assert_eq!(
                f.players[0].extra.last(),
                Some(&pendulum),
                "the face-up pendulum is still at the end"
            );
        }

        /// Two duels with the same seed shuffle identically, and with
        /// different seeds they do not. That is what a solver replaying a
        /// game depends on.
        #[test]
        fn the_seed_decides_the_shuffle() {
            fn shuffled(seed: [u64; 4]) -> Vec<u32> {
                let mut f =
                    Field::with_flags_and_seed(8000, crate::duel::REFERENCE_CONFIGURATION, seed);
                for i in 0..10 {
                    let c = bare(&mut f, i + 1, card_type::MONSTER);
                    f.add_card(0, c, location::HAND, 0, false);
                    f.cards[c].current.position = position::FACEDOWN_DEFENSE;
                }
                f.shuffle(0, location::HAND);
                f.players[0]
                    .hand
                    .iter()
                    .map(|&c| f.cards[c].data.code)
                    .collect()
            }
            let a = shuffled([1, 2, 3, 4]);
            assert_eq!(a, shuffled([1, 2, 3, 4]), "same seed, same shuffle");
            assert_ne!(a, shuffled([5, 6, 7, 8]), "different seed, different");
        }

        /// The deck's announcement carries no card codes — the point of
        /// shuffling it is that nobody may see them.
        #[test]
        fn the_deck_announcement_reveals_nothing() {
            let mut f = Field::with_flags(8000, 0);
            for i in 0..4 {
                let c = bare(&mut f, i + 1, card_type::MONSTER);
                f.add_card(0, c, location::DECK, 0, false);
            }
            f.shuffle(0, location::DECK);
            let announced = f.messages.iter().find_map(|m| match m {
                Message::ShuffleDeck { player } => Some(*player),
                _ => None,
            });
            assert_eq!(announced, Some(0));
            assert!(
                !f.messages
                    .iter()
                    .any(|m| matches!(m, Message::ShuffleHand { .. })),
                "and it is not a hand message"
            );
        }
    }

    /// A card's field effects are registered when it arrives and taken away
    /// when it leaves, and the two use the same range test — an effect
    /// registered under the hand exception and removed without it would
    /// stay registered on a card that has gone.
    #[test]
    fn field_effects_follow_the_card_on_and_off() {
        use crate::effect::{effect_type, Effect};
        let mut f = Field::new(8000);
        let c = monster(&mut f);
        let mut e = Effect::new(
            effect_type::FIELD | effect_type::CONTINUOUS | effect_type::ACTIONS,
            code::CHAINING,
        );
        e.owner = Some(c);
        e.handler = Some(c);
        e.range = u16::from(location::MZONE);
        let e = f.new_effect(e);
        f.cards[c].field_effect.insert(code::CHAINING, e);

        f.add_card(0, c, location::MZONE, 0, false);
        assert_eq!(
            f.field_effects.continuous.equal_range(code::CHAINING),
            &[e],
            "registered on arrival"
        );

        f.remove_card(c);
        assert!(
            f.field_effects
                .continuous
                .equal_range(code::CHAINING)
                .is_empty(),
            "and taken away on leaving"
        );
    }

    /// The hand exception: an optional trigger ranged to the hand applies
    /// even from out of range, so a hand trap is findable. A phase-coded
    /// effect is excluded, which is what stops a card in the hand answering
    /// every phase change.
    #[test]
    fn a_hand_trigger_applies_from_out_of_range() {
        use crate::effect::{effect_type, Effect};
        for (ev_code, expected) in [(code::CHAINING, true), (code::PHASE | 0x4, false)] {
            let mut f = Field::new(8000);
            let c = monster(&mut f);
            let mut e = Effect::new(
                effect_type::FIELD | effect_type::TRIGGER_O | effect_type::ACTIONS,
                ev_code,
            );
            e.owner = Some(c);
            e.handler = Some(c);
            e.range = u16::from(location::HAND);
            let e = f.new_effect(e);
            f.cards[c].field_effect.insert(ev_code, e);

            // Placed in the graveyard: out of the effect's range entirely.
            f.add_card(0, c, location::GRAVE, 0, false);
            let registered = !f.field_effects.trigger_o.equal_range(ev_code).is_empty();
            assert_eq!(registered, expected, "event code {ev_code:#x}");
        }
    }

    mod swap_card {
        use super::*;

        fn seated(f: &mut Field, player: u8, code_: u32, loc: u8, seq: u32) -> CardId {
            let id = bare(f, code_, card_type::MONSTER | card_type::EFFECT);
            f.cards[id].owner = player;
            f.add_card(player, id, loc, seq, false);
            f.cards[id].current.position = position::FACEUP_ATTACK;
            id
        }

        /// Two monsters across the table taking **each other's** seat
        /// numbers: one `Swap` message, and each ends up in the other's
        /// row. Note which argument is which — `new_s1` is where *card
        /// 2* lands, in card 1's row.
        #[test]
        fn two_cards_take_each_others_seats() {
            let mut f = Field::new(8000);
            let a = seated(&mut f, 0, 1, location::MZONE, 1);
            let b = seated(&mut f, 1, 2, location::MZONE, 3);
            f.messages.clear();
            f.swap_card(a, b, 1, 3);
            assert_eq!(f.cards[a].current.controller, 1);
            assert_eq!(f.cards[a].current.sequence, 3);
            assert_eq!(f.cards[b].current.controller, 0);
            assert_eq!(f.cards[b].current.sequence, 1);
            assert_eq!(f.players[1].mzone[3], Some(a));
            assert_eq!(f.players[0].mzone[1], Some(b));
            assert_eq!(f.players[0].used_location & 0x1f, 1 << 1);
            assert_eq!(f.players[1].used_location & 0x1f, 1 << 3);
            assert!(matches!(
                f.messages.as_slice(),
                [Message::Swap {
                    first: 1,
                    second: 2
                }]
            ));
        }

        /// Where each card came from is recorded, because a rule that
        /// asks "where was it?" must not see where it is now.
        #[test]
        fn each_card_remembers_where_it_was() {
            let mut f = Field::new(8000);
            let a = seated(&mut f, 0, 1, location::MZONE, 1);
            let b = seated(&mut f, 1, 2, location::MZONE, 3);
            f.swap_card(a, b, 1, 3);
            assert_eq!(f.cards[a].previous.controller, 0);
            assert_eq!(f.cards[a].previous.sequence, 1);
            assert_eq!(f.cards[a].previous.location, location::MZONE);
            assert_eq!(f.cards[b].previous.controller, 1);
            assert_eq!(f.cards[b].previous.sequence, 3);
        }

        /// A change of hands re-stamps the field id; a slide along one
        /// player's own row does not, because it is still the same card
        /// as far as every "since when" rule is concerned.
        #[test]
        fn only_a_change_of_hands_restamps_the_field_id() {
            let mut f = Field::new(8000);
            let a = seated(&mut f, 0, 1, location::MZONE, 0);
            let b = seated(&mut f, 1, 2, location::MZONE, 0);
            let (was_a, was_b) = (f.cards[a].fieldid, f.cards[b].fieldid);
            f.swap_card(a, b, 0, 0);
            assert_ne!(f.cards[a].fieldid, was_a);
            assert_ne!(f.cards[b].fieldid, was_b);

            let mut f = Field::new(8000);
            let a = seated(&mut f, 0, 1, location::MZONE, 0);
            let b = seated(&mut f, 0, 2, location::MZONE, 1);
            let (was_a, was_b) = (f.cards[a].fieldid, f.cards[b].fieldid);
            // The same exchange within one player's own row: they take
            // each other's seats, and nobody changes hands.
            f.swap_card(a, b, 0, 1);
            assert_eq!(f.cards[a].current.sequence, 1, "they did exchange");
            assert_eq!(f.cards[b].current.sequence, 0);
            assert_eq!(f.cards[a].fieldid, was_a, "same side, same card");
            assert_eq!(f.cards[b].fieldid, was_b);
        }

        /// Three silent refusals, each leaving the board exactly as it
        /// was — and a positive sibling for the seat check, because a
        /// card **staying put** is never asked whether its own seat is
        /// usable.
        #[test]
        fn it_refuses_rather_than_half_swapping() {
            // Off the field.
            let mut f = Field::new(8000);
            let a = seated(&mut f, 0, 1, location::MZONE, 0);
            let b = seated(&mut f, 1, 2, location::GRAVE, 0);
            f.messages.clear();
            f.swap_card(a, b, 0, 0);
            assert_eq!(f.cards[a].current.controller, 0, "nothing moved");
            assert_eq!(f.cards[b].current.location, location::GRAVE);
            assert!(f.messages.is_empty(), "and nothing was announced");

            // One row, and each is sent to the seat the other is leaving.
            let mut f = Field::new(8000);
            let a = seated(&mut f, 0, 1, location::MZONE, 0);
            let b = seated(&mut f, 0, 2, location::MZONE, 1);
            f.messages.clear();
            f.swap_card(a, b, 1, 0);
            assert_eq!(f.cards[a].current.sequence, 0, "refused");
            assert_eq!(f.cards[b].current.sequence, 1);
            assert!(f.messages.is_empty());

            // A seat that is not usable.
            let mut f = Field::new(8000);
            let a = seated(&mut f, 0, 1, location::MZONE, 0);
            let b = seated(&mut f, 1, 2, location::MZONE, 0);
            f.players[0].disabled_location |= 1 << 2;
            f.messages.clear();
            f.swap_card(a, b, 2, 0);
            assert_eq!(f.cards[a].current.controller, 0, "refused");
            assert!(f.messages.is_empty());
            // The sibling: the same board, with each card staying on the
            // seat it already holds. A blocked seat nobody is asking for
            // is not consulted.
            f.swap_card(a, b, 0, 0);
            assert_eq!(f.cards[a].current.controller, 1, "and this one went");
        }

        /// One card given as **both halves** is refused, and it is the
        /// only board that reaches the collision guard.
        ///
        /// The guard reads `p1 == p2 && l1 == l2 && (new_s1 == s2 ||
        /// new_s2 == s1)`. For two *distinct* cards in one row the seat
        /// check above it always fires first — either destination is a
        /// seat the other card is sitting in, so `used_location` marks it
        /// unusable. The same card twice is what slips past that: no seat
        /// changes, so no seat is checked.
        #[test]
        fn one_card_given_as_both_halves_is_refused() {
            let mut f = Field::new(8000);
            let a = seated(&mut f, 0, 1, location::MZONE, 2);
            f.messages.clear();
            f.swap_card(a, a, 2, 2);
            assert!(f.messages.is_empty(), "nothing announced");
            assert_eq!(f.cards[a].previous.location, 0, "and nothing recorded");
            assert_eq!(f.cards[a].current.sequence, 2);
            assert_eq!(f.players[0].mzone[2], Some(a));
        }

        /// A swap that also reseats writes two `Move`s instead of a
        /// `Swap`, and the card that kept its seat is named **second**.
        #[test]
        fn a_reseating_swap_writes_two_moves() {
            let mut f = Field::new(8000);
            let a = seated(&mut f, 0, 1, location::MZONE, 0);
            let b = seated(&mut f, 1, 2, location::MZONE, 0);
            f.messages.clear();
            // `a` keeps seat 0; `b` arrives at seat 2 rather than 0.
            f.swap_card(a, b, 0, 2);
            assert!(
                matches!(
                    f.messages.as_slice(),
                    [Message::Move { code: 1, .. }, Message::Move { code: 2, .. }]
                ),
                "{:?}",
                f.messages
            );

            let mut f = Field::new(8000);
            let a = seated(&mut f, 0, 1, location::MZONE, 0);
            let b = seated(&mut f, 1, 2, location::MZONE, 0);
            f.messages.clear();
            // Now `a` is the one reseated, so the order flips.
            f.swap_card(a, b, 2, 0);
            assert!(
                matches!(
                    f.messages.as_slice(),
                    [Message::Move { code: 2, .. }, Message::Move { code: 1, .. }]
                ),
                "{:?}",
                f.messages
            );
        }
    }
}

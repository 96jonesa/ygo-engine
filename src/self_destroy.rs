//! The three passes that clean up after a board change: `SelfDestroyUnique`,
//! `SelfDestroy` and `SelfToGrave`.
//!
//! `adjust_self_destroy_set` gathers the cards each pass is about; these are
//! the units that act on them. All three run **outside a chain**, as a rule
//! rather than as an effect, which is why the destruction reason is
//! `PLAYER_SELFDES` — a player id that is not a player, marking "nobody did
//! this, the rules did".
//!
//! ## Two of them are loops written as one step
//!
//! `SelfDestroy` and `SelfToGrave` take **one card per pass** and then
//! restart from the top, rather than iterating. That is not a stylistic
//! choice: destroying a card can change which other cards want to destroy
//! themselves, so the set has to be re-read after each one. The restart is
//! spelled `arg.step = Processors::restart`, whose wrap to zero is what makes
//! the unit re-enter at its first case.
//!
//! ## `temp.reason_effect` is a saved copy, again
//!
//! Each pass points the card's reason at the effect that is removing it, and
//! parks the old one on `temp` first — the same arrangement the Special
//! Summon entry functions use, and for the same reason: a replacement effect
//! may cancel the move and the card must go back to being what it was.

use crate::board::position;
use crate::card::reason;
use crate::event::{CardId, PLAYER_NONE};
use crate::field::Field;
use crate::processor::{Kind, RESTART};
use crate::{board::location, event::code};

impl Field {
    /// One step of `SelfDestroyUnique` — a card whose "only one face-up"
    /// limit is over, choosing which copy survives.
    ///
    /// ## The four-pass search for a card to keep
    ///
    /// The reference tries four filters in order, each falling back to the
    /// next when it finds nothing:
    ///
    /// 1. this player's copies that are **not** already marked for
    ///    destruction (`unique_fieldid != UINT_MAX`);
    /// 2. the opponent's, same condition;
    /// 3. this player's, marked or not;
    /// 4. the opponent's, marked or not.
    ///
    /// It reads as repetition and is not: the first two prefer a copy that
    /// can actually be kept, and the last two are the fallback for a board
    /// where every copy is already doomed. Note that each pass **flips
    /// `playerid`**, so the fourth is the opponent's again only because the
    /// third flipped back.
    pub(crate) fn self_destroy_unique_step(&mut self, step: u16, card: CardId, player: u8) -> bool {
        match step {
            0 => self.sdu_step_0(card, player),
            1 => self.sdu_step_1(card, player),
            _ => {
                self.core.unique_destroy_set.remove(&card);
                true
            }
        }
    }

    fn sdu_step_0(&mut self, card: CardId, player: u8) -> bool {
        if !self.core.unique_cards[player as usize].contains(&card) {
            self.core.unique_destroy_set.remove(&card);
            return true;
        }
        let targets = self.unique_targets(card, player, None);
        match targets.len() {
            0 => self.cards[card].unique_fieldid = 0,
            1 => {
                let only = *targets.iter().next().unwrap();
                self.cards[card].unique_fieldid = self.cards[only].fieldid;
            }
            _ => {
                let mut playerid = player;
                self.core.select_cards.clear();
                for round in 0..4 {
                    let unmarked_only = round < 2;
                    self.core.select_cards = targets
                        .iter()
                        .copied()
                        .filter(|&t| {
                            self.cards[t].current.controller == playerid
                                && (!unmarked_only || self.cards[t].unique_fieldid != u32::MAX)
                        })
                        .collect();
                    if !self.core.select_cards.is_empty() {
                        break;
                    }
                    playerid = 1 - playerid;
                }
                if self.core.select_cards.len() == 1 {
                    self.core.return_cards.clear();
                    let only = self.core.select_cards[0];
                    self.core.return_cards.list.push(only);
                } else {
                    self.messages.push(crate::field::Message::Hint {
                        kind: crate::host_question::hint::SELECTMSG,
                        player: playerid,
                        value: 534,
                    });
                    self.emplace(Kind::SelectCard {
                        player: playerid,
                        cancelable: false,
                        min: 1,
                        max: 1,
                    });
                }
                return false;
            }
        }
        self.core.unique_destroy_set.remove(&card);
        true
    }

    /// Step 1: keep the chosen copy, destroy the rest.
    ///
    /// The kept card's `fieldid` becomes the limiter's `unique_fieldid`,
    /// which is how "this is the one that counts" is recorded — and is what
    /// the gather reads next time round.
    fn sdu_step_1(&mut self, card: CardId, player: u8) -> bool {
        let mut targets = self.unique_targets(card, player, None);
        let Some(&kept) = self.core.return_cards.list.first() else {
            self.core.unique_destroy_set.remove(&card);
            return true;
        };
        self.cards[card].unique_fieldid = self.cards[kept].fieldid;
        targets.remove(&kept);

        let (by, controller) = (
            self.cards[card].unique_effect,
            self.cards[card].current.controller,
        );
        for &doomed in &targets {
            let c = &mut self.cards[doomed];
            c.temp.reason_effect = c.reason_effect;
            c.temp.reason_player = c.reason_player;
            c.reason_effect = by;
            c.reason_player = controller;
        }
        self.destroy(
            targets,
            None,
            reason::RULE,
            crate::event::PLAYER_SELFDES,
            PLAYER_NONE,
            0,
            0,
        );
        false
    }

    /// One step of `SelfDestroy` — the cards whose own effects destroy them.
    ///
    /// One card, then restart. `returns` is set to zero at the end because
    /// the unit shares its tail with the machines that report a count, and a
    /// rule pass has none to report.
    pub(crate) fn self_destroy_step(&mut self, step: u16) -> bool {
        if step != 0 {
            self.core.returns.set(0);
            self.core.operated_set.clear();
            return true;
        }
        let Some(&card) = self.core.self_destroy_set.iter().next() else {
            // The set is empty: fall through to the tail.
            return false;
        };
        if let Some(by) = self.is_affected_by_effect(card, code::SELF_DESTROY) {
            let player = self
                .effects
                .get(by)
                .map_or(PLAYER_NONE, |e| e.get_handler_player(&self.cards));
            let c = &mut self.cards[card];
            c.temp.reason_effect = c.reason_effect;
            c.temp.reason_player = c.reason_player;
            c.reason_effect = Some(by);
            c.reason_player = player;
            self.destroy(
                [card],
                Some(by),
                reason::EFFECT,
                crate::event::PLAYER_SELFDES,
                PLAYER_NONE,
                0,
                0,
            );
        }
        self.core.self_destroy_set.remove(&card);
        self.set_step(RESTART);
        false
    }

    /// One step of `SelfToGrave` — the same shape, sending rather than
    /// destroying.
    ///
    /// **It is a send, not a destruction**, so nothing can replace it as a
    /// destruction and no destruction event is raised. The two passes look
    /// alike and are different rules.
    pub(crate) fn self_to_grave_step(&mut self, step: u16) -> bool {
        if step != 0 {
            self.core.returns.set(0);
            self.core.operated_set.clear();
            return true;
        }
        let Some(&card) = self.core.self_tograve_set.iter().next() else {
            return false;
        };
        if let Some(by) = self.is_affected_by_effect(card, code::SELF_TOGRAVE) {
            let player = self
                .effects
                .get(by)
                .map_or(PLAYER_NONE, |e| e.get_handler_player(&self.cards));
            let c = &mut self.cards[card];
            c.temp.reason_effect = c.reason_effect;
            c.temp.reason_player = c.reason_player;
            c.reason_effect = Some(by);
            c.reason_player = player;
            self.send_to(
                [card],
                None,
                reason::EFFECT,
                PLAYER_NONE,
                PLAYER_NONE,
                u16::from(location::GRAVE),
                0,
                position::FACEUP,
                false,
            );
        }
        self.core.self_tograve_set.remove(&card);
        self.set_step(RESTART);
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::position;
    use crate::card::{card_type, status, Card, CardData};
    use crate::effect::{effect_type, Effect};
    use crate::event::EffectId;
    use crate::field::Message;
    use crate::processor::Status;

    fn on_field(f: &mut Field, player: u8, seat: u32, code_: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: code_,
                type_: card_type::MONSTER,
                ..Default::default()
            },
            player,
        );
        c.current.controller = player;
        c.code = code_;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(player, id, location::MZONE, seat, false);
        f.cards[id].current.position = position::FACEUP_ATTACK;
        f.cards[id].fieldid = f.next_field_id_raw();
        id
    }

    fn single(f: &mut Field, card: CardId, code_: u32) -> EffectId {
        let mut e = Effect::new(effect_type::SINGLE, code_);
        e.owner = Some(card);
        e.handler = Some(card);
        let id = f.new_effect(e);
        f.cards[card].single_effect.insert(code_, id);
        f.cards[card].indexer.insert(id);
        id
    }

    /// A card carrying an "only one face-up" limit, registered as one.
    fn limiter(f: &mut Field, player: u8, seat: u32, code_: u32) -> CardId {
        let c = on_field(f, player, seat, code_);
        f.cards[c].unique_code = code_;
        f.cards[c].unique_location = u16::from(location::MZONE);
        f.cards[c].unique_pos = [1, 1];
        f.cards[c].unique_fieldid = 1;
        let mut e = Effect::new(effect_type::SINGLE, 0);
        e.owner = Some(c);
        e.handler = Some(c);
        let e = f.new_effect(e);
        f.cards[c].unique_effect = Some(e);
        f.core.unique_cards[player as usize].push(c);
        c
    }

    /// Run, answering a card choice with the card at `pick` of the offer.
    fn run(f: &mut Field, pick: Option<usize>) -> Status {
        for _ in 0..2048 {
            match f.process() {
                Status::Continue => continue,
                // A `Retry` means the last answer was not accepted — which
                // here means it was never given, because the caller entered
                // the driver at an already-pending question. Answering it is
                // the same work.
                Status::Awaiting => match (f.messages.last(), pick) {
                    (Some(Message::SelectCard { .. } | Message::Retry), Some(i)) => {
                        f.core.returns.set_i32(0, 0);
                        f.core.returns.set_i32(1, 1);
                        f.core.returns.set_i32(2, i as i32);
                    }
                    (Some(Message::SelectChain { .. }), _) => f.core.returns.set(-1),
                    (Some(Message::SelectEffectYesNo { .. }), _) => f.core.returns.set(0),
                    _ => return Status::Awaiting,
                },
                other => return other,
            }
        }
        panic!("did not settle");
    }

    mod unique {
        use super::*;

        /// A limiter that is no longer registered is dropped **without
        /// acting** — which means without touching its own field id, the
        /// thing every other path here writes.
        #[test]
        fn an_unregistered_limiter_is_dropped_without_acting() {
            let mut f = Field::new(8000);
            let c = limiter(&mut f, 0, 0, 777);
            f.cards[c].unique_fieldid = 12345;
            f.core.unique_cards[0].clear();
            f.core.unique_destroy_set.insert(c);
            f.emplace(Kind::SelfDestroyUnique { card: c, player: 0 });
            run(&mut f, None);
            assert!(f.core.unique_destroy_set.is_empty());
            assert_eq!(f.cards[c].unique_fieldid, 12345, "left as it was");
        }

        /// **No copies means the limit is idle**, recorded as a field id of
        /// zero; one copy records *that* copy's id.
        #[test]
        fn the_field_id_records_which_copy_counts() {
            let mut f = Field::new(8000);
            let c = limiter(&mut f, 0, 0, 777);
            // Its own `unique_code` matches itself, so it is its own target.
            assert_eq!(f.unique_targets(c, 0, None).len(), 1);
            f.core.unique_destroy_set.insert(c);
            f.emplace(Kind::SelfDestroyUnique { card: c, player: 0 });
            run(&mut f, None);
            assert_eq!(
                f.cards[c].unique_fieldid, f.cards[c].fieldid,
                "one copy: the limit points at it"
            );

            let mut g = Field::new(8000);
            let c = limiter(&mut g, 0, 0, 777);
            g.cards[c].unique_code = 999;
            g.core.unique_destroy_set.insert(c);
            g.emplace(Kind::SelfDestroyUnique { card: c, player: 0 });
            run(&mut g, None);
            assert_eq!(g.cards[c].unique_fieldid, 0, "no copies: idle");
        }

        /// **Two copies is a question**, and the answer decides which one
        /// lives.
        #[test]
        fn two_copies_are_a_question_and_the_rest_are_destroyed() {
            let mut f = Field::new(8000);
            let a = limiter(&mut f, 0, 0, 777);
            let b = on_field(&mut f, 0, 1, 777);
            f.cards[b].unique_code = 777;
            f.cards[b].unique_location = u16::from(location::MZONE);
            f.cards[b].unique_pos = [1, 1];
            // Arriving next to a copy that is already face-up marks a card
            // `u32::MAX`, and the first two passes of the search skip such
            // copies. Cleared here so that both are candidates and the
            // question is actually asked; the marking itself is the subject
            // of the test below.
            f.cards[b].unique_fieldid = 0;

            f.core.unique_destroy_set.insert(a);
            f.emplace(Kind::SelfDestroyUnique { card: a, player: 0 });
            assert_eq!(run(&mut f, None), Status::Awaiting, "which to keep?");
            assert!(
                f.messages.iter().any(|m| matches!(
                    m,
                    Message::Hint {
                        kind: crate::host_question::hint::SELECTMSG,
                        value: 534,
                        ..
                    }
                )),
                "asked with the right prompt"
            );
            let offered = f.core.select_cards.clone();
            assert_eq!(offered.len(), 2);

            // Keep the first of the two offered.
            let kept = offered[0];
            let doomed = offered[1];
            run(&mut f, Some(0));
            assert_eq!(f.cards[kept].current.location, location::MZONE, "kept");
            assert_eq!(
                f.cards[doomed].current.location,
                location::GRAVE,
                "and the other is destroyed"
            );
            assert_eq!(
                f.cards[a].unique_fieldid, f.cards[kept].fieldid,
                "the limit now points at the survivor"
            );
            // **`PLAYER_SELFDES` is what makes the reason stick.** `destroy`
            // leaves the reason fields alone for that player id, which is
            // why the pass sets them beforehand.
            assert_eq!(
                f.cards[doomed].reason_effect, f.cards[a].unique_effect,
                "destroyed by the limit, and it says so"
            );
            assert_eq!(f.cards[doomed].reason_player, 0);
        }

        /// **A copy already marked for destruction is not preferred.**
        ///
        /// `unique_fieldid == u32::MAX` is that marking, and the first two
        /// passes of the search skip such copies — so with one marked and one
        /// not, there is nothing to ask and the unmarked one is kept.
        #[test]
        fn a_marked_copy_is_not_offered_while_an_unmarked_one_exists() {
            let mut f = Field::new(8000);
            let a = limiter(&mut f, 0, 0, 777);
            let marked = on_field(&mut f, 0, 1, 777);
            let clean = on_field(&mut f, 0, 2, 777);
            for c in [marked, clean] {
                f.cards[c].unique_code = 777;
                f.cards[c].unique_location = u16::from(location::MZONE);
                f.cards[c].unique_pos = [1, 1];
            }
            f.cards[marked].unique_fieldid = u32::MAX;
            f.cards[a].unique_code = 0;

            f.core.unique_destroy_set.insert(a);
            f.cards[a].unique_code = 777;
            f.emplace(Kind::SelfDestroyUnique { card: a, player: 0 });
            run(&mut f, None);
            assert!(
                f.core.select_cards.iter().all(|&c| c != marked),
                "the marked copy was passed over"
            );
            assert!(
                !f.messages
                    .iter()
                    .any(|m| matches!(m, Message::SelectCard { .. })),
                "and one candidate is taken without asking"
            );
        }

        /// **The search changes sides when this player has no copy.**
        ///
        /// The limiter's own name is set to something it does not itself
        /// carry, so every copy is the opponent's — the first pass finds
        /// nothing and the second is the one that answers.
        #[test]
        fn the_search_crosses_to_the_opponent() {
            let mut f = Field::new(8000);
            let a = limiter(&mut f, 0, 0, 111);
            f.cards[a].unique_code = 777;
            for seat in 0..2 {
                let other = on_field(&mut f, 1, seat, 777);
                f.cards[other].unique_code = 777;
                f.cards[other].unique_location = u16::from(location::MZONE);
                f.cards[other].unique_pos = [1, 1];
                f.cards[other].unique_fieldid = 0;
            }

            f.core.unique_destroy_set.insert(a);
            f.emplace(Kind::SelfDestroyUnique { card: a, player: 0 });
            assert_eq!(run(&mut f, None), Status::Awaiting);
            assert!(
                f.messages.iter().any(|m| matches!(
                    m,
                    Message::Hint {
                        kind: crate::host_question::hint::SELECTMSG,
                        player: 1,
                        value: 534,
                    }
                )),
                "the opponent is the one asked"
            );
        }
    }

    mod self_destroy {
        use super::*;

        #[test]
        fn a_card_with_the_effect_destroys_itself() {
            let mut f = Field::new(8000);
            let c = on_field(&mut f, 0, 0, 777);
            single(&mut f, c, code::SELF_DESTROY);
            f.core.self_destroy_set.insert(c);
            f.emplace(Kind::SelfDestroy);
            run(&mut f, None);
            assert_eq!(f.cards[c].current.location, location::GRAVE);
            assert!(f.core.self_destroy_set.is_empty());
        }

        /// **One card per pass, then restart** — so every card in the set is
        /// reached, however many there are.
        #[test]
        fn every_card_in_the_set_is_reached() {
            let mut f = Field::new(8000);
            let a = on_field(&mut f, 0, 0, 777);
            let b = on_field(&mut f, 0, 1, 778);
            let c = on_field(&mut f, 1, 0, 779);
            for card in [a, b, c] {
                single(&mut f, card, code::SELF_DESTROY);
                f.core.self_destroy_set.insert(card);
            }
            f.emplace(Kind::SelfDestroy);
            run(&mut f, None);
            for card in [a, b, c] {
                assert_eq!(f.cards[card].current.location, location::GRAVE);
            }
            assert!(f.core.self_destroy_set.is_empty());
        }

        /// A card whose effect is gone by the time the pass reaches it is
        /// taken off the set without being destroyed.
        #[test]
        fn a_card_whose_effect_vanished_is_only_dropped() {
            let mut f = Field::new(8000);
            let c = on_field(&mut f, 0, 0, 777);
            f.core.self_destroy_set.insert(c);
            f.emplace(Kind::SelfDestroy);
            run(&mut f, None);
            assert_eq!(f.cards[c].current.location, location::MZONE, "still there");
            assert!(f.core.self_destroy_set.is_empty(), "but off the set");
        }

        /// The card's old reason is saved before the pass points it at the
        /// destroying effect.
        ///
        /// Asserted on the step itself rather than after the run: `destroy`
        /// parks `temp` again on its own way through, so by the time the
        /// card reaches the graveyard the copy is the *destruction's*, not
        /// this pass's.
        #[test]
        fn the_old_reason_is_saved_first() {
            let mut f = Field::new(8000);
            let c = on_field(&mut f, 0, 0, 777);
            let e = single(&mut f, c, code::SELF_DESTROY);
            f.cards[c].reason_player = 1;
            f.core.self_destroy_set.insert(c);
            f.core
                .units
                .push_front(crate::processor::Unit::new(Kind::SelfDestroy));

            assert!(
                !f.self_destroy_step(0),
                "the pass restarts rather than ending"
            );
            assert_eq!(f.cards[c].temp.reason_player, 1, "the old one is parked");
            assert_eq!(f.cards[c].temp.reason_effect, None);
            assert_eq!(f.cards[c].reason_effect, Some(e), "and the new one is set");
        }
    }

    mod self_to_grave {
        use super::*;

        /// **It is a send, not a destruction.** The card reaches the
        /// graveyard, and nothing is destroyed on the way.
        #[test]
        fn the_card_is_sent_rather_than_destroyed() {
            let mut f = Field::new(8000);
            let c = on_field(&mut f, 0, 0, 777);
            single(&mut f, c, code::SELF_TOGRAVE);
            f.core.self_tograve_set.insert(c);
            f.emplace(Kind::SelfToGrave);
            run(&mut f, None);
            assert_eq!(f.cards[c].current.location, location::GRAVE);
            assert_eq!(f.cards[c].reason & reason::DESTROY, 0, "not a destruction");
            assert_ne!(f.cards[c].reason & reason::EFFECT, 0);
            assert!(f.core.self_tograve_set.is_empty());
        }

        #[test]
        fn every_card_in_the_set_is_reached() {
            let mut f = Field::new(8000);
            let a = on_field(&mut f, 0, 0, 777);
            let b = on_field(&mut f, 1, 0, 778);
            for card in [a, b] {
                single(&mut f, card, code::SELF_TOGRAVE);
                f.core.self_tograve_set.insert(card);
            }
            f.emplace(Kind::SelfToGrave);
            run(&mut f, None);
            for card in [a, b] {
                assert_eq!(f.cards[card].current.location, location::GRAVE);
            }
        }
    }
}

//! Choosing tributes, one card at a time.
//!
//! `SelectTribute` is not a question — it is a **loop around** two questions.
//! Steps 1-3 cycle: offer what may still be taken, take one answer, and
//! either finish or go round again. Each pass recomputes what is on offer
//! from what has been chosen so far, which is what lets the offer shrink as
//! the requirement is met.
//!
//! It exists because tributes are not a fixed-size choice. A monster worth
//! two tributes changes how many more are needed the moment it is picked, so
//! the set of legal next choices depends on the ones already made — which a
//! single `SelectCard` cannot express.
//!
//! ## Three groups, and the shortcuts that avoid the loop entirely
//!
//! The candidates arrive in `release_cards`, `release_cards_ex` and
//! `release_cards_ex_oneof` (see `get_summon_release_list`). Step 0 takes a
//! shortcut to the plain `SelectTributeP` question in two cases, and both are
//! "there is nothing to be clever about":
//!
//! - **no extra-release cards at all** — the ordinary list is the whole
//!   story;
//! - **enough extra-release cards to fill the maximum by themselves** —
//!   nothing else can be needed.
//!
//! Both are guarded by there being **room on the field**: with no room, the
//! tributes have to make it, and which ones are taken then matters.
//!
//! ## `must_choose_one` is the no-room case
//!
//! When there is no room in the Monster Zone, at least one tribute must come
//! from a seat the summon could use — otherwise the summon has nowhere to go.
//! Those cards are `must_choose_one`, and while none of them has been chosen
//! the offer narrows to them as the maximum runs down. That is the whole
//! purpose of the `force` flag threaded through steps 1 and 2.

use crate::board::location;
use crate::event::{code, CardId, EffectId};
use crate::field::Field;
use crate::processor::Kind;

/// The state `SelectTribute` carries across its loop.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SelectTributeState {
    /// Cards that must supply a seat, because there is no free one.
    pub must_choose_one: Vec<CardId>,
    /// The `EFFECT_EXTRA_RELEASE_SUM` spent by the one-of card that was
    /// chosen, if one was. Held so it can be charged at the end — and
    /// **cleared if that card is unchosen**, which is why it is unit state
    /// rather than a local.
    pub extra_release_effect: Option<EffectId>,
}

impl Field {
    /// `field::select_tribute_cards` — queue a tribute selection.
    #[allow(clippy::too_many_arguments)]
    pub fn select_tribute_cards(
        &mut self,
        target: CardId,
        player: u8,
        cancelable: bool,
        min: u8,
        max: u8,
        toplayer: u8,
        zone: u32,
    ) {
        self.emplace(Kind::SelectTribute {
            target,
            player,
            cancelable,
            min,
            max,
            toplayer,
            zone,
            state: Box::default(),
        });
    }

    /// One step of `SelectTribute`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn select_tribute_step(
        &mut self,
        step: u16,
        target: CardId,
        player: u8,
        cancelable: bool,
        min: &mut u8,
        max: &mut u8,
        toplayer: u8,
        zone: u32,
        state: &mut SelectTributeState,
    ) -> bool {
        match step {
            0 => self.select_tribute_step_0(target, player, cancelable, *min, *max, toplayer, zone),
            1 => self.select_tribute_step_1(
                target, player, cancelable, *min, *max, toplayer, zone, state,
            ),
            2 => self.select_tribute_step_2(player, cancelable, min, max, state),
            3 => self.select_tribute_step_3(min, max, state),
            _ => true,
        }
    }

    /// The free seats available to this summon, after forced-zone effects.
    fn tribute_room(&mut self, target: CardId, player: u8, toplayer: u8, zone: u32) -> (i32, u32) {
        let forced = self.get_forced_zones(
            Some(target),
            toplayer,
            location::MZONE,
            player,
            Field::LOCATION_REASON_TOFIELD,
        );
        let zone = zone & 0x1f & forced;
        (
            self.get_tofield_count(
                Some(target),
                toplayer,
                location::MZONE,
                player,
                Field::LOCATION_REASON_TOFIELD,
                zone,
            ),
            zone,
        )
    }

    /// Step 0: take the plain question when there is nothing to be clever
    /// about.
    ///
    /// Both shortcuts finish the unit — `return TRUE` — after emplacing
    /// `SelectTributeP`, so the answer the caller eventually reads is that
    /// question's, not this machine's.
    #[allow(clippy::too_many_arguments)]
    fn select_tribute_step_0(
        &mut self,
        target: CardId,
        player: u8,
        cancelable: bool,
        min: u8,
        max: u8,
        toplayer: u8,
        zone: u32,
    ) -> bool {
        self.core.operated_set.clear();
        let (room, _) = self.tribute_room(target, player, toplayer, zone);
        if room > 0 {
            let extras = self.core.release_cards_ex.len() + self.core.release_cards_ex_oneof.len();
            if extras == 0 {
                self.core.select_cards = self.core.release_cards.clone();
                self.ask_tribute_p(player, cancelable, min, max);
                return true;
            }
            if self.core.release_cards_ex.len() >= max as usize {
                self.core.select_cards = self.core.release_cards_ex.clone();
                self.ask_tribute_p(player, cancelable, min, max);
                return true;
            }
        }
        false
    }

    fn ask_tribute_p(&mut self, player: u8, cancelable: bool, min: u8, max: u8) {
        self.messages.push(crate::field::Message::Hint {
            kind: crate::host_question::hint::SELECTMSG,
            player,
            value: 500,
        });
        self.emplace(Kind::SelectTributeP {
            player,
            cancelable,
            min,
            max,
        });
    }

    /// Step 1: work out what must be chosen, and make the first offer.
    ///
    /// `must_choose_one` is populated only when there is **no room**: those
    /// are the candidates sitting in a seat the summon could use, and one of
    /// them has to go or the summon has nowhere to land.
    ///
    /// The one-of list is pruned here against the other two: a card that is
    /// already an ordinary or extra-release candidate is not *also* a
    /// "one of these" candidate, and leaving it in both would let it be
    /// offered twice.
    ///
    /// The escape hatch before that — no extras and `min` above what the
    /// ordinary list is worth — falls back to the plain question. Note it
    /// offers the cards **only if** `rmax > 0`: with nothing worth taking it
    /// asks with an empty list, which `SelectTributeP` answers immediately.
    #[allow(clippy::too_many_arguments)]
    fn select_tribute_step_1(
        &mut self,
        target: CardId,
        player: u8,
        cancelable: bool,
        min: u8,
        max: u8,
        toplayer: u8,
        zone: u32,
        state: &mut SelectTributeState,
    ) -> bool {
        let (room, zone) = self.tribute_room(target, player, toplayer, zone);
        let mut rmax = 0u32;
        let mut must_choose_one: Vec<CardId> = Vec::new();
        for &c in &self.core.release_cards.clone() {
            let loc = self.cards[c].current;
            if loc.location == location::MZONE
                && loc.controller == toplayer
                && (zone >> loc.sequence) & 1 != 0
                && room <= 0
            {
                must_choose_one.push(c);
            }
            rmax += self.cards[c].release_param;
        }

        if self.core.release_cards_ex.is_empty()
            && self.core.release_cards_ex_oneof.is_empty()
            && u32::from(min) > rmax
        {
            // Nothing worth taking: ask with an empty list rather than a
            // partial one.
            self.core.select_cards = if rmax > 0 {
                self.core.release_cards.clone()
            } else {
                Vec::new()
            };
            self.ask_tribute_p(player, cancelable, min, max);
            return true;
        }

        let force = !must_choose_one.is_empty();
        // A card cannot be both a "one of these" candidate and an ordinary
        // one; prune so it is not offered twice.
        let (ordinary, extra) = (
            self.core.release_cards.clone(),
            self.core.release_cards_ex.clone(),
        );
        self.core
            .release_cards_ex_oneof
            .retain(|c| !ordinary.contains(c) && !extra.contains(c));

        self.core.select_cards.clear();
        self.core.unselect_cards.clear();
        // Signed: `max - exsize` can go below zero, and the reference's
        // unsigned wraparound makes the comparison simply false. In Rust it
        // would panic, so the subtraction is done where it cannot.
        if i64::from(max) - self.core.release_cards_ex.len() as i64 == 1 && force {
            self.core
                .select_cards
                .extend(must_choose_one.iter().copied());
            self.core
                .select_cards
                .extend(self.core.release_cards_ex.clone());
        } else if max <= 1 && force {
            self.core
                .select_cards
                .extend(must_choose_one.iter().copied());
        } else {
            self.core.select_cards.extend(ordinary);
            self.core.select_cards.extend(extra);
            self.core
                .select_cards
                .extend(self.core.release_cards_ex_oneof.clone());
        }
        self.messages.push(crate::field::Message::Hint {
            kind: crate::host_question::hint::SELECTMSG,
            player,
            value: 500,
        });
        self.emplace(Kind::SelectUnselectCard {
            player,
            cancelable,
            min,
            max,
            finishable: false,
        });
        state.must_choose_one = must_choose_one;
        self.set_step(2);
        false
    }
}

impl Field {
    /// Step 2: re-offer, with what has already been chosen deducted.
    ///
    /// The bookkeeping at the top is the loop's arithmetic, and the two
    /// counters mean different things: `rmin` is **how many cards** have been
    /// chosen, `rmax` **what they are worth**. The remaining maximum is
    /// reduced by the count and the remaining minimum by the worth, because
    /// the maximum bounds cards and the minimum bounds tributes.
    ///
    /// The *original* bounds are kept and passed to the question. The
    /// deducted ones drive this machine's own decisions; the question is
    /// asked in the caller's terms.
    ///
    /// `force` is recomputed and **cleared** once any of the must-choose
    /// cards has been taken: the requirement is satisfied, so the offer can
    /// widen again.
    fn select_tribute_step_2(
        &mut self,
        player: u8,
        cancelable: bool,
        min: &mut u8,
        max: &mut u8,
        state: &mut SelectTributeState,
    ) -> bool {
        let chosen = self.core.operated_set.clone();
        let rmin = chosen.len() as u32;
        let rmax: u32 = chosen.iter().map(|&c| self.cards[c].release_param).sum();
        let (oldmin, oldmax) = (*min, *max);
        // Worth against the minimum, count against the maximum.
        *min = if rmax > u32::from(*min) {
            0
        } else {
            *min - rmax as u8
        };
        *max = if rmin > u32::from(*max) {
            0
        } else {
            *max - rmin as u8
        };

        let force = !state.must_choose_one.is_empty()
            && !state.must_choose_one.iter().any(|c| chosen.contains(c));

        self.core.select_cards.clear();
        self.core.unselect_cards.clear();
        let exsize = self
            .core
            .release_cards_ex
            .iter()
            .filter(|c| !chosen.contains(c))
            .count();

        let unchosen = |f: &Field, list: &[CardId]| -> Vec<CardId> {
            list.iter()
                .copied()
                .filter(|c| !f.core.operated_set.contains(c))
                .collect()
        };
        if i64::from(*max) - exsize as i64 == 1 && force {
            let must = unchosen(self, &state.must_choose_one);
            let ex = unchosen(self, &self.core.release_cards_ex.clone());
            self.core.select_cards.extend(must);
            self.core.select_cards.extend(ex);
        } else if *max <= 1 && force {
            self.core
                .select_cards
                .extend(state.must_choose_one.iter().copied());
        } else if exsize > 0 && exsize == *max as usize {
            let ex = unchosen(self, &self.core.release_cards_ex.clone());
            self.core.select_cards.extend(ex);
        } else {
            let ordinary = unchosen(self, &self.core.release_cards.clone());
            let ex = unchosen(self, &self.core.release_cards_ex.clone());
            self.core.select_cards.extend(ordinary);
            self.core.select_cards.extend(ex);
            // Once a one-of card has been taken, the rest stop being
            // offered: only one of them may ever be used.
            if state.extra_release_effect.is_none() {
                let oneof = unchosen(self, &self.core.release_cards_ex_oneof.clone());
                self.core.select_cards.extend(oneof);
            }
        }

        // Cancelling is only allowed while nothing has been chosen;
        // finishing only when the requirement is already met.
        let canc = rmin == 0 && cancelable;
        let finishable = *min == 0 && !force && exsize == 0;
        self.core.unselect_cards = chosen;

        self.messages.push(crate::field::Message::Hint {
            kind: crate::host_question::hint::SELECTMSG,
            player,
            value: 500,
        });
        self.emplace(Kind::SelectUnselectCard {
            player,
            cancelable: canc,
            min: oldmin,
            max: oldmax,
            finishable,
        });
        false
    }

    /// Step 3: apply the one card that came back, then finish or loop.
    ///
    /// The answer **toggles**: a card not yet chosen is added, one already
    /// chosen is removed. That is what makes `SelectUnselectCard`'s second
    /// list an undo.
    ///
    /// Taking a one-of card records the `EFFECT_EXTRA_RELEASE_SUM` it
    /// spends; *un*-taking it clears the record. Without the clearing, a
    /// player who tried a one-of card and changed their mind would still
    /// have its count charged at the end.
    ///
    /// Two ways to finish, and they are different: **cancelled with the
    /// minimum met**, or **the maximum exhausted**. Cancelling with the
    /// minimum unmet is not a finish — the loop goes round again, which is
    /// why `finishable` is computed as carefully as it is at step 2.
    fn select_tribute_step_3(
        &mut self,
        min: &mut u8,
        max: &mut u8,
        state: &mut SelectTributeState,
    ) -> bool {
        let canceled = self.core.return_cards.canceled;
        if canceled && self.core.operated_set.is_empty() {
            return true;
        }
        if !canceled {
            if let Some(&picked) = self.core.return_cards.list.first() {
                if let Some(pos) = self.core.operated_set.iter().position(|&c| c == picked) {
                    self.core.operated_set.remove(pos);
                    if self.core.release_cards_ex_oneof.contains(&picked) {
                        state.extra_release_effect = None;
                    }
                } else {
                    self.core.operated_set.push(picked);
                    if self.core.release_cards_ex_oneof.contains(&picked) {
                        state.extra_release_effect =
                            self.is_affected_by_effect(picked, code::EXTRA_RELEASE_SUM);
                    }
                }
            }
        }

        let chosen = self.core.operated_set.clone();
        let rmin = chosen.len() as u32;
        let rmax: u32 = chosen.iter().map(|&c| self.cards[c].release_param).sum();
        *min = if rmax > u32::from(*min) {
            0
        } else {
            *min - rmax as u8
        };
        *max = if rmin > u32::from(*max) {
            0
        } else {
            *max - rmin as u8
        };

        if (canceled && *min == 0) || *max == 0 {
            self.core.return_cards.clear();
            self.core.return_cards.list = chosen;
            if let Some(e) = state.extra_release_effect {
                self.dec_count(e, crate::event::PLAYER_NONE);
            }
            return true;
        }
        self.set_step(1);
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::position;
    use crate::card::{card_type, status, Card, CardData};
    use crate::processor::Status;

    fn monster(f: &mut Field, player: u8, seat: u32, worth: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 18036057,
                type_: card_type::MONSTER,
                ..Default::default()
            },
            player,
        );
        c.current.controller = player;
        c.current.position = position::FACEUP_ATTACK;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(player, id, location::MZONE, seat, false);
        f.cards[id].release_param = worth;
        id
    }

    fn in_hand(f: &mut Field) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 72989439,
                type_: card_type::MONSTER,
                level: 7,
                ..Default::default()
            },
            0,
        );
        c.current.controller = 0;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(0, id, location::HAND, 0, false);
        id
    }

    fn run(f: &mut Field) -> Status {
        for _ in 0..512 {
            match f.process() {
                Status::Continue => continue,
                other => return other,
            }
        }
        panic!("did not settle");
    }

    fn sent(f: &Field, want: &str) -> bool {
        f.messages
            .iter()
            .any(|m| format!("{m:?}").starts_with(want))
    }

    /// Answer the pending `SelectUnselectCard` with a card, by looking it up
    /// in the offered lists the way a host would.
    fn pick(f: &mut Field, card: CardId) {
        let idx = f
            .core
            .select_cards
            .iter()
            .position(|&c| c == card)
            .map(|i| i as i32)
            .or_else(|| {
                f.core
                    .unselect_cards
                    .iter()
                    .position(|&c| c == card)
                    .map(|i| (i + f.core.select_cards.len()) as i32)
            })
            .expect("the card is on one of the two lists");
        f.core.returns.set_i32(0, 1);
        f.core.returns.set_i32(1, idx);
    }

    /// With room on the field and no extra-release cards, the machine takes
    /// the shortcut to the plain question and does not loop.
    #[test]
    fn the_simple_case_delegates_to_the_plain_question() {
        let mut f = Field::new(8000);
        let target = in_hand(&mut f);
        let a = monster(&mut f, 0, 0, 1);
        f.core.release_cards = vec![a];

        f.select_tribute_cards(target, 0, false, 1, 1, 0, 0x1f);
        assert_eq!(run(&mut f), Status::Awaiting);
        assert!(
            sent(&f, "SelectTribute {"),
            "the plain question, not the loop"
        );
        assert!(!sent(&f, "SelectUnselectCard"));
    }

    /// With **no room** on the field the shortcut is refused, even when
    /// the ordinary list is the whole story — the tributes have to make the
    /// room, so which ones are taken matters and the loop has to run.
    #[test]
    fn a_full_field_refuses_the_shortcut() {
        let mut f = Field::new(8000);
        let target = in_hand(&mut f);
        // Fill all five main Monster Zones.
        let filled: Vec<CardId> = (0..5).map(|i| monster(&mut f, 0, i, 1)).collect();
        f.core.release_cards = filled;

        f.select_tribute_cards(target, 0, false, 1, 1, 0, 0x1f);
        assert_eq!(run(&mut f), Status::Awaiting);
        assert!(
            sent(&f, "SelectUnselectCard"),
            "no room, so the loop runs rather than the plain question"
        );
        assert!(!sent(&f, "SelectTribute {"));
    }

    /// With extra-release cards in play the machine loops, offering one card
    /// at a time.
    #[test]
    fn extra_release_cards_drive_the_loop() {
        let mut f = Field::new(8000);
        let target = in_hand(&mut f);
        let mine = monster(&mut f, 0, 0, 1);
        let theirs = monster(&mut f, 1, 0, 1);
        f.core.release_cards = vec![mine];
        f.core.release_cards_ex = vec![theirs];

        f.select_tribute_cards(target, 0, false, 2, 2, 0, 0x1f);
        assert_eq!(run(&mut f), Status::Awaiting);
        assert!(
            sent(&f, "SelectUnselectCard"),
            "the loop, not the plain question"
        );

        pick(&mut f, mine);
        assert_eq!(run(&mut f), Status::Awaiting, "one taken, one to go");
        pick(&mut f, theirs);
        assert_eq!(run(&mut f), Status::End);
        assert_eq!(f.core.return_cards.list.len(), 2);
    }

    /// The answer **toggles**: picking an already-chosen card removes it.
    /// That is what the second list is for.
    #[test]
    fn picking_a_chosen_card_removes_it() {
        let mut f = Field::new(8000);
        let target = in_hand(&mut f);
        let a = monster(&mut f, 0, 0, 1);
        let b = monster(&mut f, 0, 1, 1);
        let theirs = monster(&mut f, 1, 0, 1);
        f.core.release_cards = vec![a, b];
        f.core.release_cards_ex = vec![theirs];

        f.select_tribute_cards(target, 0, false, 2, 2, 0, 0x1f);
        run(&mut f);
        pick(&mut f, a);
        run(&mut f);
        assert_eq!(f.core.operated_set, vec![a], "chosen");

        // Now pick it again, from the unselect list.
        pick(&mut f, a);
        run(&mut f);
        assert!(f.core.operated_set.is_empty(), "and un-chosen");
    }

    /// A card worth two satisfies a minimum of two on its own, so the
    /// selection becomes **finishable** after a single pick.
    ///
    /// The other candidate is a *one-of* card rather than an extra-release
    /// one: `finishable` requires no unchosen extra-release cards to remain,
    /// so with one pending the player would still be made to answer. That is
    /// correct, and it is not what this test is about.
    #[test]
    fn a_double_tribute_makes_the_selection_finishable() {
        let mut f = Field::new(8000);
        let target = in_hand(&mut f);
        let big = monster(&mut f, 0, 0, 2);
        let theirs = monster(&mut f, 1, 0, 1);
        f.core.release_cards = vec![big];
        f.core.release_cards_ex_oneof = vec![theirs];

        f.select_tribute_cards(target, 0, false, 2, 2, 0, 0x1f);
        run(&mut f);
        pick(&mut f, big);
        // The loop does not end here: the maximum is *two cards* and only
        // one has been taken, so the player is asked again and may take
        // another. What the double satisfies is the **minimum**, which is
        // what makes finishing legal — so finishing is how it ends.
        assert_eq!(run(&mut f), Status::Awaiting);
        f.core.returns.set_i32(0, -1);
        assert_eq!(
            run(&mut f),
            Status::End,
            "worth two, so finishing is allowed"
        );
        assert_eq!(f.core.return_cards.list, vec![big]);
    }

    /// The maximum bounds **cards**, so two worth one each exhaust a maximum
    /// of two even though a single double would also have done.
    #[test]
    fn the_maximum_counts_cards_not_worth() {
        let mut f = Field::new(8000);
        let target = in_hand(&mut f);
        let a = monster(&mut f, 0, 0, 1);
        let b = monster(&mut f, 0, 1, 1);
        let theirs = monster(&mut f, 1, 0, 1);
        f.core.release_cards = vec![a, b];
        f.core.release_cards_ex = vec![theirs];

        f.select_tribute_cards(target, 0, false, 0, 2, 0, 0x1f);
        run(&mut f);
        pick(&mut f, a);
        run(&mut f);
        pick(&mut f, b);
        assert_eq!(run(&mut f), Status::End, "two cards is the maximum");
        assert_eq!(f.core.return_cards.list.len(), 2);
    }

    /// A one-of card, once taken, stops the others being offered — only one
    /// of them may ever be used.
    #[test]
    fn taking_a_one_of_card_withdraws_the_rest() {
        let mut f = Field::new(8000);
        let target = in_hand(&mut f);
        let ordinary = monster(&mut f, 0, 0, 1);
        let one_of_a = monster(&mut f, 1, 0, 1);
        let one_of_b = monster(&mut f, 1, 1, 1);
        // The withdrawal is driven by the effect the chosen card *spends*,
        // so the cards must actually carry one. Without it nothing is
        // recorded and the rest stay on offer — correctly.
        for c in [one_of_a, one_of_b] {
            let mut e = crate::effect::Effect::new(
                crate::effect::effect_type::SINGLE,
                code::EXTRA_RELEASE_SUM,
            );
            e.owner = Some(c);
            e.handler = Some(c);
            let id = f.new_effect(e);
            f.cards[c].single_effect.insert(code::EXTRA_RELEASE_SUM, id);
            f.cards[c].indexer.insert(id);
        }
        f.core.release_cards = vec![ordinary];
        f.core.release_cards_ex_oneof = vec![one_of_a, one_of_b];

        f.select_tribute_cards(target, 0, false, 0, 3, 0, 0x1f);
        run(&mut f);
        assert!(
            f.core.select_cards.contains(&one_of_b),
            "both are offered to begin with"
        );
        pick(&mut f, one_of_a);
        run(&mut f);
        assert!(
            !f.core.select_cards.contains(&one_of_b),
            "and the other is withdrawn once one is taken"
        );
    }

    /// The one-of list is pruned against the other two, so a card in both is
    /// not offered twice.
    #[test]
    fn a_card_in_two_lists_is_offered_once() {
        let mut f = Field::new(8000);
        let target = in_hand(&mut f);
        let shared = monster(&mut f, 0, 0, 1);
        let theirs = monster(&mut f, 1, 0, 1);
        f.core.release_cards = vec![shared];
        f.core.release_cards_ex = vec![theirs];
        f.core.release_cards_ex_oneof = vec![shared];

        f.select_tribute_cards(target, 0, false, 0, 3, 0, 0x1f);
        run(&mut f);
        let count = f.core.select_cards.iter().filter(|&&c| c == shared).count();
        assert_eq!(count, 1, "offered once, not twice");
    }
}

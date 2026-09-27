//! Choosing what to release.
//!
//! `field::process(Processors::SelectRelease&)`. The sibling of
//! `SelectTribute`, and the same shape: a fast path that asks one plain
//! question, and a slow path that loops a one-card-at-a-time question
//! until the selection is legal.
//!
//! ## Three pools, and why they cannot be one list
//!
//! - `release_cards` — the ordinary ones.
//! - `release_cards_ex` — ones that **must** be included if anything is.
//! - `release_cards_ex_oneof` — ones permitted by a "release one of these
//!   instead" effect, of which **at most one** may be taken.
//!
//! The third is what forces the loop: whether a card is still offerable
//! depends on what has already been chosen, which a single `SelectCard`
//! cannot express.
//!
//! ## The jump table
//!
//! | from | `arg.step` | runs next |
//! |---|---|---|
//! | 0, the plain path | — (returns false) | case 1 |
//! | 0, the loop path | `1` | **case 2** |
//! | 2 | — (returns false) | case 3 |
//! | 3, still choosing | `1` | **case 2** |
//!
//! Both `arg.step = 1` reach case **2** — the increment rule — so the
//! number written is the case *before* the one that runs, and case 1 is
//! never revisited once the loop starts.
//!
//! ## The three shortcuts in case 0
//!
//! The plain question is taken when the choice cannot go wrong:
//!
//! - **`allminimum`** — at most one "one of these" card, and the pools
//!   together hold exactly `min`. Every card is taken, so there is nothing
//!   to get wrong.
//! - **`allmust`** — the must-include pool alone fills `max`, or there is
//!   nothing else to choose from.
//! - **`onlyself`** — neither special pool has anything in it.
//!
//! `must_choose_one` overrides all three: when the field cannot fit the
//! summon, some specific occupied zone has to be cleared, and that is a
//! constraint the plain question cannot carry.
//!
//! ## `min` is rewritten under `allmust`
//!
//! The plain question asks for `release_cards_ex.size()` rather than
//! `min`. Those are the cards that must be taken anyway, and asking for
//! the original `min` would demand more than the offer contains.

use crate::board::location;
use crate::event::{code, CardId, EffectId};
use crate::field::{Field, Message};
use crate::host_question::hint;
use crate::processor::Kind;

/// `SelectRelease`'s own state — the two things it accumulates across the
/// loop.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SelectReleaseState {
    /// Set when the field has no room: one of *these* cards has to go, and
    /// the plain question cannot say so.
    pub must_choose_one: Option<Vec<CardId>>,
    /// The "release one of these instead" permission the current selection
    /// is leaning on, if any. Cleared when that card is unchosen.
    pub extra_release_nonsum: Option<EffectId>,
}

impl Field {
    /// Queue a release selection.
    #[allow(clippy::too_many_arguments)]
    pub fn select_release_cards(
        &mut self,
        playerid: u8,
        cancelable: bool,
        min: u16,
        max: u16,
        check_field: bool,
        to_check: Option<CardId>,
        toplayer: u8,
        zone: u32,
    ) {
        self.emplace(Kind::SelectRelease {
            playerid,
            cancelable,
            min,
            max,
            check_field,
            to_check,
            toplayer,
            zone,
            state: Box::default(),
        });
    }

    /// One step of `SelectRelease`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn select_release_step(
        &mut self,
        step: u16,
        playerid: u8,
        cancelable: bool,
        min: u16,
        max: u16,
        check_field: bool,
        to_check: Option<CardId>,
        toplayer: u8,
        zone: u32,
        state: &mut SelectReleaseState,
    ) -> bool {
        match step {
            0 => self.sr_offer(
                playerid,
                cancelable,
                min,
                max,
                check_field,
                to_check,
                toplayer,
                zone,
                state,
            ),
            1 => self.sr_take_plain_answer(min),
            2 => self.sr_ask_one(playerid, min, max, state),
            3 => self.sr_absorb_one(min, max, state),
            _ => true,
        }
    }

    /// Case 0: work out whether the plain question will do, and ask it if
    /// so.
    #[allow(clippy::too_many_arguments)]
    fn sr_offer(
        &mut self,
        playerid: u8,
        cancelable: bool,
        min: u16,
        max: u16,
        check_field: bool,
        to_check: Option<CardId>,
        toplayer: u8,
        zone: u32,
        state: &mut SelectReleaseState,
    ) -> bool {
        if check_field {
            let forced = self.get_forced_zones(
                to_check,
                toplayer,
                location::MZONE,
                playerid,
                Field::LOCATION_REASON_TOFIELD,
            );
            let zone = zone & 0x1f & forced;
            let room = self.get_useable_count(
                to_check,
                toplayer,
                location::MZONE,
                playerid,
                Field::LOCATION_REASON_TOFIELD,
                zone,
            );
            if room < i32::from(min) {
                // Not enough room: whatever is released has to include a
                // card sitting in one of the zones the summon needs.
                let must: Vec<CardId> = self
                    .core
                    .release_cards
                    .iter()
                    .copied()
                    .filter(|&c| {
                        let cur = &self.cards[c].current;
                        cur.location == location::MZONE
                            && cur.controller == toplayer
                            && (zone >> cur.sequence) & 1 != 0
                    })
                    .collect();
                state.must_choose_one = Some(must);
            }
        }
        self.core.operated_set.clear();
        self.core.return_cards.clear();

        let (ord, ex, oneof) = (
            self.core.release_cards.len(),
            self.core.release_cards_ex.len(),
            self.core.release_cards_ex_oneof.len(),
        );
        // Every card is taken, so nothing can be chosen wrongly.
        let allminimum = oneof <= 1 && (ex + ord + oneof) == usize::from(min);
        // The must-include pool fills the request by itself, or there is
        // nothing else on offer.
        let allmust = ex >= usize::from(max) || (ord + oneof) == 0;
        // Neither special pool has anything in it.
        let onlyself = (ex + oneof) == 0;

        if (!allminimum && !allmust && !onlyself) || state.must_choose_one.is_some() {
            // 1, not 2: the increment lands on case 2. See the module note.
            self.set_step(1);
            return false;
        }

        self.core.select_cards.clear();
        self.core.select_cards.extend(&self.core.release_cards_ex);
        if ex < usize::from(max) {
            let mut rest = self.core.release_cards.clone();
            rest.extend(&self.core.release_cards_ex_oneof);
            self.core.select_cards.extend(rest);
        }
        // **`min` becomes the must-include count.** Those are taken
        // anyway, and the original `min` may exceed what is on offer.
        let min = if allmust { ex as u16 } else { min };
        self.messages.push(Message::Hint {
            kind: hint::SELECTMSG,
            player: playerid,
            value: 500,
        });
        self.emplace(Kind::SelectCard {
            player: playerid,
            cancelable,
            min: min as u8,
            max: max as u8,
        });
        false
    }

    /// Case 1: take the plain question's answer.
    fn sr_take_plain_answer(&mut self, min: u16) -> bool {
        if self.core.return_cards.canceled {
            return true;
        }
        let chosen: Vec<CardId> = self.core.return_cards.list.clone();
        let count = chosen.len();
        for c in chosen {
            if !self.core.operated_set.contains(&c) {
                self.core.operated_set.push(c);
            }
        }
        if usize::from(min) > count {
            // Something went wrong in the selection: refuse it rather than
            // release too few.
            self.core.return_cards.clear();
            self.core.return_cards.canceled = true;
        } else if let Some(&first) = self.core.release_cards_ex_oneof.first() {
            // The plain path can only have taken the one "one of these"
            // card there was, so its permission is the one to spend.
            if let Some(e) = self.is_affected_by_effect(first, code::EXTRA_RELEASE_NONSUM) {
                self.core.dec_count_reserve.push(e);
            }
        }
        true
    }

    /// Case 2: offer whatever is still legal, one card at a time.
    fn sr_ask_one(
        &mut self,
        playerid: u8,
        min: u16,
        max: u16,
        state: &mut SelectReleaseState,
    ) -> bool {
        self.core.unselect_cards = self.core.operated_set.clone();

        let mut finishable = self.core.operated_set.len() >= usize::from(min);
        let mut must_chosen = state.must_choose_one.is_none();

        // Every must-include card still outstanding both blocks finishing
        // and eats one slot of the remaining allowance.
        let mut to_select = 0i32;
        for c in self.core.release_cards_ex.clone() {
            if !self.core.operated_set.contains(&c) {
                finishable = false;
                to_select += 1;
            }
        }
        if !must_chosen {
            if let Some(must) = &state.must_choose_one {
                must_chosen = self.core.operated_set.iter().any(|c| must.contains(c));
            }
        }

        let mut curmax = max as i32 - self.core.operated_set.len() as i32;
        if !must_chosen {
            // One slot is reserved for the card that has to be included.
            curmax -= 1;
        }
        curmax -= to_select;
        finishable = finishable && must_chosen;

        let mut diff: Vec<CardId> = self.core.release_cards_ex.clone();
        if curmax <= 0 {
            if !must_chosen {
                if let Some(must) = &state.must_choose_one {
                    if curmax == 0 {
                        // Room for the forced card and the outstanding
                        // must-includes.
                        diff.extend(must);
                    } else {
                        // Room for nothing else at all.
                        diff = must.clone();
                    }
                }
            }
        } else {
            diff.extend(&self.core.release_cards);
            if state.extra_release_nonsum.is_none() {
                // A "one of these" permission is already spent, so none of
                // that pool is offered again.
                diff.extend(&self.core.release_cards_ex_oneof);
            }
        }

        // `card_sort` is creation order, which is what a `CardId` already
        // is — so the set difference is over sorted ids.
        diff.sort_unstable();
        diff.dedup();
        let chosen = self.core.unselect_cards.clone();
        self.core.select_cards = diff.into_iter().filter(|c| !chosen.contains(c)).collect();

        self.messages.push(Message::Hint {
            kind: hint::SELECTMSG,
            player: playerid,
            value: 500,
        });
        self.emplace(Kind::SelectUnselectCard {
            player: playerid,
            cancelable: finishable || self.core.operated_set.is_empty(),
            min: min as u8,
            max: max as u8,
            finishable,
        });
        false
    }

    /// Case 3: fold one answer into the selection, and decide whether to
    /// go round again.
    fn sr_absorb_one(&mut self, min: u16, max: u16, state: &mut SelectReleaseState) -> bool {
        let finished_early = self.core.return_cards.canceled
            && (self.core.operated_set.is_empty()
                || self.core.operated_set.len() >= usize::from(min));
        if finished_early {
            if let Some(e) = state.extra_release_nonsum {
                self.core.dec_count_reserve.push(e);
            }
            self.core.return_cards.list = self.core.operated_set.clone();
            if !self.core.operated_set.is_empty() {
                // Cancelling with a legal selection is not a cancel.
                self.core.return_cards.canceled = false;
            }
            return true;
        }

        let Some(&picked) = self.core.return_cards.list.first() else {
            return true;
        };
        let is_oneof = self.core.release_cards_ex_oneof.contains(&picked);
        if let Some(pos) = self.core.operated_set.iter().position(|&c| c == picked) {
            self.core.operated_set.remove(pos);
            if is_oneof {
                state.extra_release_nonsum = None;
            }
        } else {
            self.core.operated_set.push(picked);
            if is_oneof {
                state.extra_release_nonsum =
                    self.is_affected_by_effect(picked, code::EXTRA_RELEASE_NONSUM);
            }
        }

        if self.core.operated_set.len() == usize::from(max) {
            self.core.return_cards.list = self.core.operated_set.clone();
            if let Some(e) = state.extra_release_nonsum {
                self.core.dec_count_reserve.push(e);
            }
            return true;
        }
        // 1, not 2: the increment lands on case 2, going round again.
        self.set_step(1);
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::position;
    use crate::card::{card_type, status, Card, CardData};
    use crate::duel::phases;
    use crate::processor::Kind;

    fn field() -> Field {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        f.infos.turn_player = 0;
        f.infos.phase = phases::MAIN1;
        f
    }

    fn monster(f: &mut Field, player: u8) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 30000,
                type_: card_type::MONSTER,
                level: 4,
                ..Default::default()
            },
            player,
        );
        c.current.controller = player;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        let seat = f.players[player as usize]
            .mzone
            .iter()
            .position(Option::is_none)
            .expect("a free seat") as u32;
        f.add_card(player, id, location::MZONE, seat, false);
        f.cards[id].current.position = position::FACEUP_ATTACK;
        id
    }

    /// Step case 0 alone and report which path it took.
    ///
    /// The plain path queues a `SelectCard` there and then; the loop path
    /// queues **nothing** — it only advances the step, and the question
    /// arrives in case 2. So "no question" is the loop's signature, which
    /// is why this cannot simply look for a `SelectUnselectCard`.
    fn offer(f: &mut Field, min: u16, max: u16) -> Option<&'static str> {
        let mut state = SelectReleaseState::default();
        let done = f.select_release_step(0, 0, false, min, max, false, None, 0, 0, &mut state);
        assert!(!done, "case 0 never finishes the unit");
        let plain = f
            .core
            .units
            .iter()
            .chain(f.core.subunits.iter())
            .any(|u| matches!(u.kind, Kind::SelectCard { .. }));
        Some(if plain { "plain" } else { "loop" })
    }

    mod the_shortcuts {
        use super::*;

        /// **`onlyself`** — neither special pool has anything, so the
        /// plain question is enough.
        #[test]
        fn only_ordinary_cards_take_the_plain_question() {
            let mut f = field();
            let a = monster(&mut f, 0);
            let b = monster(&mut f, 0);
            f.core.release_cards = vec![a, b];
            assert_eq!(offer(&mut f, 1, 2), Some("plain"));
        }

        /// **`allminimum`** — the pools together hold exactly `min`, so
        /// every card is taken and nothing can be chosen wrongly. The
        /// special pools are non-empty here, so `onlyself` is not what is
        /// carrying the test.
        #[test]
        fn taking_everything_takes_the_plain_question() {
            let mut f = field();
            let a = monster(&mut f, 0);
            let ex = monster(&mut f, 0);
            let oneof = monster(&mut f, 0);
            f.core.release_cards = vec![a];
            f.core.release_cards_ex = vec![ex];
            f.core.release_cards_ex_oneof = vec![oneof];
            assert_eq!(offer(&mut f, 3, 3), Some("plain"), "three cards, min three");
        }

        /// **`allmust`** — the must-include pool fills `max` by itself.
        #[test]
        fn a_full_must_pool_takes_the_plain_question() {
            let mut f = field();
            let ex1 = monster(&mut f, 0);
            let ex2 = monster(&mut f, 0);
            let spare = monster(&mut f, 0);
            f.core.release_cards = vec![spare];
            f.core.release_cards_ex = vec![ex1, ex2];
            assert_eq!(offer(&mut f, 1, 2), Some("plain"));
        }

        /// **`allminimum` requires at most ONE "one of these" card.**
        ///
        /// Two of them cannot both be taken however the totals line up, so
        /// the shortcut does not apply even when the pools hold exactly
        /// `min`. A test with one such card cannot show this: `oneof <= 1`
        /// holds either way.
        #[test]
        fn two_one_of_cards_defeat_the_all_minimum_shortcut() {
            let mut f = field();
            let one = monster(&mut f, 0);
            let two = monster(&mut f, 0);
            f.core.release_cards_ex_oneof = vec![one, two];
            assert_eq!(
                offer(&mut f, 2, 2),
                Some("loop"),
                "two cards and min two, but only one may be taken"
            );
        }

        /// **`allmust` compares against `max`, not `min`.**
        ///
        /// One must-include with a minimum of one but a maximum of three
        /// leaves real choosing to do. Comparing against `min` would call
        /// it settled and ask the plain question.
        #[test]
        fn the_must_pool_is_measured_against_the_maximum() {
            let mut f = field();
            let ex = monster(&mut f, 0);
            let ord = monster(&mut f, 0);
            let oneof = monster(&mut f, 0);
            f.core.release_cards = vec![ord];
            f.core.release_cards_ex = vec![ex];
            f.core.release_cards_ex_oneof = vec![oneof];
            assert_eq!(
                offer(&mut f, 1, 3),
                Some("loop"),
                "one must-include does not fill a maximum of three"
            );
        }

        /// **And the general case does not.** Two pools with room to
        /// choose between them needs the loop.
        #[test]
        fn a_real_choice_takes_the_loop() {
            let mut f = field();
            let a = monster(&mut f, 0);
            let b = monster(&mut f, 0);
            let oneof = monster(&mut f, 0);
            f.core.release_cards = vec![a, b];
            f.core.release_cards_ex_oneof = vec![oneof];
            assert_eq!(offer(&mut f, 1, 2), Some("loop"));
        }

        /// **`must_choose_one` overrides every shortcut.** Even a case
        /// that would otherwise take the plain question loops, because the
        /// plain question cannot say "and one of *these* specifically".
        #[test]
        fn a_forced_zone_overrides_the_shortcuts() {
            let mut f = field();
            let a = monster(&mut f, 0);
            let b = monster(&mut f, 0);
            f.core.release_cards = vec![a, b];
            let mut state = SelectReleaseState {
                must_choose_one: Some(vec![a]),
                ..Default::default()
            };
            assert!(!f.select_release_step(0, 0, false, 1, 2, false, None, 0, 0, &mut state));
            assert!(
                f.core
                    .units
                    .iter()
                    .chain(f.core.subunits.iter())
                    .all(|u| !matches!(u.kind, Kind::SelectCard { .. })),
                "onlyself would otherwise have taken the plain question"
            );
        }

        /// **Under `allmust`, `min` becomes the must-include count.**
        /// Asking for the original `min` would demand more than the offer
        /// holds.
        #[test]
        fn allmust_rewrites_the_minimum() {
            let mut f = field();
            let ex1 = monster(&mut f, 0);
            let ex2 = monster(&mut f, 0);
            f.core.release_cards_ex = vec![ex1, ex2];
            let mut state = SelectReleaseState::default();
            f.select_release_step(0, 0, false, 1, 2, false, None, 0, 0, &mut state);
            let asked = f
                .core
                .units
                .iter()
                .chain(f.core.subunits.iter())
                .find_map(|u| match u.kind {
                    Kind::SelectCard { min, max, .. } => Some((min, max)),
                    _ => None,
                });
            assert_eq!(asked, Some((2, 2)), "min became the must-include count");
        }

        /// The plain question withholds the optional pools once the
        /// must-include pool already fills `max`.
        #[test]
        fn a_full_must_pool_is_the_whole_offer() {
            let mut f = field();
            let ex1 = monster(&mut f, 0);
            let ex2 = monster(&mut f, 0);
            let spare = monster(&mut f, 0);
            f.core.release_cards = vec![spare];
            f.core.release_cards_ex = vec![ex1, ex2];
            offer(&mut f, 1, 2);
            assert_eq!(f.core.select_cards.len(), 2, "only the must-includes");
            assert!(!f.core.select_cards.contains(&spare));
        }
    }

    mod the_plain_answer {
        use super::*;

        /// A cancelled question ends the unit.
        #[test]
        fn a_cancel_ends_it() {
            let mut f = field();
            f.core.return_cards.canceled = true;
            assert!(f.select_release_step(
                1,
                0,
                false,
                1,
                2,
                false,
                None,
                0,
                0,
                &mut Default::default()
            ));
        }

        /// **Too few cards is turned into a cancel**, not accepted.
        #[test]
        fn too_few_chosen_becomes_a_cancel() {
            let mut f = field();
            let a = monster(&mut f, 0);
            f.core.return_cards.list = vec![a];
            assert!(f.select_release_step(
                1,
                0,
                false,
                2,
                3,
                false,
                None,
                0,
                0,
                &mut Default::default()
            ));
            assert!(f.core.return_cards.canceled, "refused");
            assert!(f.core.return_cards.list.is_empty());
        }

        /// Enough cards is accepted, and they land in `operated_set`.
        #[test]
        fn enough_chosen_is_accepted() {
            let mut f = field();
            let a = monster(&mut f, 0);
            let b = monster(&mut f, 0);
            f.core.return_cards.list = vec![a, b];
            assert!(f.select_release_step(
                1,
                0,
                false,
                2,
                3,
                false,
                None,
                0,
                0,
                &mut Default::default()
            ));
            assert!(!f.core.return_cards.canceled);
            assert_eq!(f.core.operated_set, vec![a, b]);
        }
    }

    mod the_loop {
        use super::*;

        /// **Choosing a card adds it; choosing it again takes it back.**
        #[test]
        fn a_second_pick_unpicks() {
            let mut f = field();
            let a = monster(&mut f, 0);
            let b = monster(&mut f, 0);
            f.core.release_cards = vec![a, b];
            let mut state = SelectReleaseState::default();

            f.core.return_cards.list = vec![a];
            assert!(!f.select_release_step(3, 0, false, 1, 3, false, None, 0, 0, &mut state));
            assert_eq!(f.core.operated_set, vec![a]);

            f.core.return_cards.list = vec![a];
            assert!(!f.select_release_step(3, 0, false, 1, 3, false, None, 0, 0, &mut state));
            assert!(f.core.operated_set.is_empty(), "unpicked");
        }

        /// **Reaching `max` finishes without another question.**
        #[test]
        fn reaching_the_maximum_finishes() {
            let mut f = field();
            let a = monster(&mut f, 0);
            let b = monster(&mut f, 0);
            f.core.release_cards = vec![a, b];
            let mut state = SelectReleaseState::default();
            f.core.operated_set = vec![a];
            f.core.return_cards.list = vec![b];
            assert!(
                f.select_release_step(3, 0, false, 1, 2, false, None, 0, 0, &mut state),
                "two of a maximum two"
            );
            assert_eq!(f.core.return_cards.list, vec![a, b]);
        }

        /// **Cancelling with a legal selection is not a cancel.** The
        /// chosen cards become the answer and the cancel flag is cleared.
        #[test]
        fn cancelling_with_enough_chosen_confirms_instead() {
            let mut f = field();
            let a = monster(&mut f, 0);
            let b = monster(&mut f, 0);
            f.core.operated_set = vec![a, b];
            f.core.return_cards.canceled = true;
            assert!(f.select_release_step(
                3,
                0,
                false,
                2,
                3,
                false,
                None,
                0,
                0,
                &mut Default::default()
            ));
            assert_eq!(f.core.return_cards.list, vec![a, b]);
            assert!(!f.core.return_cards.canceled, "confirmed, not cancelled");
        }

        /// Cancelling with nothing chosen really is a cancel.
        #[test]
        fn cancelling_with_nothing_chosen_stays_a_cancel() {
            let mut f = field();
            f.core.return_cards.canceled = true;
            assert!(f.select_release_step(
                3,
                0,
                false,
                2,
                3,
                false,
                None,
                0,
                0,
                &mut Default::default()
            ));
            assert!(f.core.return_cards.canceled);
            assert!(f.core.return_cards.list.is_empty());
        }

        /// **An outstanding must-include blocks finishing** and is still
        /// offered.
        #[test]
        fn an_unchosen_must_include_blocks_finishing() {
            let mut f = field();
            let ex = monster(&mut f, 0);
            let a = monster(&mut f, 0);
            let b = monster(&mut f, 0);
            f.core.release_cards = vec![a, b];
            f.core.release_cards_ex = vec![ex];
            f.core.operated_set = vec![a];
            let mut state = SelectReleaseState::default();
            f.select_release_step(2, 0, false, 1, 3, false, None, 0, 0, &mut state);
            let finishable =
                f.core
                    .units
                    .iter()
                    .chain(f.core.subunits.iter())
                    .find_map(|u| match u.kind {
                        Kind::SelectUnselectCard { finishable, .. } => Some(finishable),
                        _ => None,
                    });
            assert_eq!(finishable, Some(false), "the must-include is outstanding");
            assert!(f.core.select_cards.contains(&ex), "and still offered");
        }

        /// **Already-chosen cards are not offered again.**
        #[test]
        fn the_offer_excludes_what_is_chosen() {
            let mut f = field();
            let a = monster(&mut f, 0);
            let b = monster(&mut f, 0);
            f.core.release_cards = vec![a, b];
            f.core.operated_set = vec![a];
            f.select_release_step(
                2,
                0,
                false,
                1,
                3,
                false,
                None,
                0,
                0,
                &mut Default::default(),
            );
            assert!(!f.core.select_cards.contains(&a), "already chosen");
            assert!(f.core.select_cards.contains(&b));
            assert_eq!(f.core.unselect_cards, vec![a], "and can be unpicked");
        }

        /// **A spent "one of these" permission withdraws that whole
        /// pool.** Taking a second would break the one-of rule.
        #[test]
        fn a_spent_one_of_permission_withdraws_the_pool() {
            let mut f = field();
            let a = monster(&mut f, 0);
            let one = monster(&mut f, 0);
            let two = monster(&mut f, 0);
            f.core.release_cards = vec![a];
            f.core.release_cards_ex_oneof = vec![one, two];

            let mut state = SelectReleaseState::default();
            f.select_release_step(2, 0, false, 1, 3, false, None, 0, 0, &mut state);
            assert!(f.core.select_cards.contains(&one), "offered while unspent");

            state.extra_release_nonsum = Some(0);
            f.core.operated_set.clear();
            f.select_release_step(2, 0, false, 1, 3, false, None, 0, 0, &mut state);
            assert!(!f.core.select_cards.contains(&one), "withdrawn once spent");
            assert!(!f.core.select_cards.contains(&two));
            assert!(f.core.select_cards.contains(&a), "the ordinary pool stays");
        }
    }
}

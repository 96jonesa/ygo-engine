//! `TrapMonsterAdjust` — trap monsters that have stopped being monsters.
//!
//! A Trap Monster occupies a Monster Zone while it counts as a monster. When
//! `EFFECT_DISABLE_TRAPMONSTER` takes that away, it has to go back to a
//! Spell/Trap Zone — and if there is no room there, to the graveyard.
//!
//! `Adjust` gathers the cards; this unit finds them room, asks a player to
//! give some up when there is not enough, and moves what survives.
//!
//! ## Both players, one loop, run twice
//!
//! There is no pair of loops: the machine runs cases 1-3 for the turn player,
//! sets `oppo_selection` and jumps back to run them again for the opponent.
//! Case 3 is the whole of the "or the other one" logic, and it is two lines.
//!
//! ## Case 2 falls through into case 3
//!
//! The reference writes `[[fallthrough]]` — case 2 does its work and case 3's
//! body runs in the *same* dispatch, not the next one. The only fall-through
//! in the port so far.
//!
//! It is also **equivalent to not falling through**, and worth knowing why
//! before someone removes it: case 2 returning without it would advance the
//! step to 3 and run case 3 on the next dispatch, reaching the same state one
//! call later. Kept because the reference has it, and because "one dispatch
//! earlier" is the kind of difference that stops being nothing the moment
//! something else reads the queue.
//!
//! ## Without the duel option, the whole first half is skipped
//!
//! `DUEL_TRAP_MONSTERS_NOT_USE_ZONE` — which this project's configuration
//! **does** set — is what makes a trap monster occupy only a Monster Zone. In
//! a configuration without it, a trap monster already holds its Spell/Trap
//! Zone, so there is nothing to find room for and case 0 jumps straight to
//! the move.

use crate::board::location;
use crate::card::{card_type, reason};
use crate::event::{code, CardId, PLAYER_NONE};
use crate::field::{Field, Message};
use crate::processor::Kind;
use std::collections::BTreeSet;

/// The state `TrapMonsterAdjust` carries.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrapMonsterAdjustState {
    /// Whether the second pass — the opponent's — is the one running.
    pub oppo_selection: bool,
    /// The cards that found no room and are going to the graveyard.
    pub to_grave: BTreeSet<CardId>,
}

impl Field {
    /// One step of `TrapMonsterAdjust`.
    pub(crate) fn trap_monster_adjust_step(
        &mut self,
        step: u16,
        state: &mut TrapMonsterAdjustState,
    ) -> bool {
        match step {
            0 => {
                if !self.is_flag(crate::duel::flags::TRAP_MONSTERS_NOT_USE_ZONE) {
                    self.set_step(3);
                    return false;
                }
                // Only ever reached once — case 3 jumps back to *case 1*,
                // not here — so this is clearing a list that is already
                // empty. Kept as the reference has it.
                state.to_grave.clear();
                false
            }
            1 => self.tma_find_room(state),
            2 => {
                self.tma_take_the_answer(state);
                // `[[fallthrough]]`: case 3's body runs in this dispatch.
                self.tma_switch_sides(state)
            }
            3 => self.tma_switch_sides(state),
            4 => self.tma_move(state),
            _ => true,
        }
    }

    fn tma_player(&self, state: &TrapMonsterAdjustState) -> u8 {
        if state.oppo_selection {
            1 - self.infos.turn_player
        } else {
            self.infos.turn_player
        }
    }

    /// Step 1: how much Spell/Trap room this player has, and what to do when
    /// it is not enough.
    ///
    /// Three outcomes. **No room at all** sends every one of this player's
    /// trap monsters to the graveyard and clears the set — no question, no
    /// choice. **Not enough room** asks the player which of them to give up,
    /// for exactly the surplus. **Enough room** does nothing and moves on.
    ///
    /// The question asks for the *surplus*, not for the survivors: the cards
    /// chosen are the ones that go, which is the opposite of what "select"
    /// usually means here.
    fn tma_find_room(&mut self, state: &mut TrapMonsterAdjustState) -> bool {
        let player = self.tma_player(state);
        self.refresh_location_info_instant();
        let room = self.get_useable_count(None, player, location::SZONE, player, 0, 0xff);
        let waiting = self.core.trap_monster_adjust_set[player as usize].len() as i32;
        if room <= 0 {
            let doomed = std::mem::take(&mut self.core.trap_monster_adjust_set[player as usize]);
            state.to_grave.extend(doomed);
            self.set_step(2);
        } else if waiting > room {
            let surplus = (waiting - room) as u8;
            self.core.select_cards = self.core.trap_monster_adjust_set[player as usize]
                .iter()
                .copied()
                .collect();
            self.messages.push(Message::Hint {
                kind: crate::host_question::hint::SELECTMSG,
                player,
                value: 502,
            });
            self.emplace(Kind::SelectCard {
                player,
                cancelable: false,
                min: surplus,
                max: surplus,
            });
        } else {
            self.set_step(2);
        }
        false
    }

    /// Step 2: the chosen cards are the ones that go.
    fn tma_take_the_answer(&mut self, state: &mut TrapMonsterAdjustState) {
        let player = self.tma_player(state);
        for card in self.core.return_cards.list.clone() {
            state.to_grave.insert(card);
            self.core.trap_monster_adjust_set[player as usize].remove(&card);
        }
    }

    /// Step 3: the other player's turn to find room, once.
    fn tma_switch_sides(&mut self, state: &mut TrapMonsterAdjustState) -> bool {
        if !state.oppo_selection {
            state.oppo_selection = true;
            self.set_step(0);
        }
        false
    }

    /// Step 4: move what survived, and send the rest.
    ///
    /// **`RESET_TURN_SET` is cleared first.** A trap monster that goes back
    /// to a Spell/Trap Zone stops counting as "set this turn" — it is not
    /// arriving as a set card, it is returning to what it was.
    ///
    /// `ret = 2` on the move is what puts it back in *its own* seat when the
    /// configuration gives trap monsters one; the extra
    /// `refresh_location_info_instant` in that configuration is because the
    /// seat it is returning to has to be counted as free first.
    fn tma_move(&mut self, state: &mut TrapMonsterAdjustState) -> bool {
        let mut p = self.infos.turn_player;
        for _ in 0..2 {
            for card in self.core.trap_monster_adjust_set[p as usize].clone() {
                self.reset_card(
                    card,
                    crate::field::reset::TURN_SET,
                    crate::field::reset::EVENT,
                );
                if !self.is_flag(crate::duel::flags::TRAP_MONSTERS_NOT_USE_ZONE) {
                    self.refresh_location_info_instant();
                }
                let position = self.cards[card].current.position;
                self.move_to_field(
                    card,
                    p,
                    p,
                    u16::from(location::SZONE),
                    position,
                    false,
                    2,
                    0xff,
                    false,
                    0,
                    true,
                );
            }
            p = 1 - p;
        }
        let doomed = std::mem::take(&mut state.to_grave);
        if !doomed.is_empty() {
            self.send_to(
                doomed,
                None,
                reason::RULE,
                PLAYER_NONE,
                PLAYER_NONE,
                u16::from(location::GRAVE),
                0,
                crate::board::position::FACEUP,
                false,
            );
        }
        true
    }

    /// `Adjust`'s gather: the trap monsters that have stopped being monsters.
    ///
    /// Split out here rather than left in `Adjust` because it is the half of
    /// the pass that decides *whether* the unit runs at all, and the two read
    /// better together.
    pub fn gather_trap_monster_adjust(&mut self) -> bool {
        self.core.trap_monster_adjust_set[0].clear();
        self.core.trap_monster_adjust_set[1].clear();
        for p in 0..2usize {
            // By seat, so the walk borrows nothing while it inserts.
            for seat in 0..self.players[p].mzone.len() {
                let Some(card) = self.players[p].mzone[seat] else {
                    continue;
                };
                let is_trap_monster =
                    self.get_type(card, None, 0, p as u8) & card_type::TRAPMONSTER != 0;
                if is_trap_monster
                    && self
                        .is_affected_by_effect(card, code::DISABLE_TRAPMONSTER)
                        .is_some()
                {
                    self.core.trap_monster_adjust_set[p].insert(card);
                }
            }
        }
        !self.core.trap_monster_adjust_set[0].is_empty()
            || !self.core.trap_monster_adjust_set[1].is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::position;
    use crate::card::{status, Card, CardData};
    use crate::effect::{effect_type, Effect};
    use crate::processor::Status;

    /// A trap monster in a Monster Zone, with its "I am a monster" effect
    /// already switched off.
    fn trap_monster(f: &mut Field, player: u8, seat: u32, disabled: bool) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 44095762 + seat,
                type_: card_type::TRAP | card_type::TRAPMONSTER | card_type::MONSTER,
                ..Default::default()
            },
            player,
        );
        c.current.controller = player;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(player, id, location::MZONE, seat, false);
        f.cards[id].current.position = position::FACEUP_ATTACK;
        if disabled {
            let mut e = Effect::new(effect_type::SINGLE, code::DISABLE_TRAPMONSTER);
            e.owner = Some(id);
            e.handler = Some(id);
            let eid = f.new_effect(e);
            f.cards[id]
                .single_effect
                .insert(code::DISABLE_TRAPMONSTER, eid);
            f.cards[id].indexer.insert(eid);
        }
        id
    }

    /// Fill a player's spell/trap row so there is no room to go back to.
    fn fill_szone(f: &mut Field, player: u8, seats: u32) {
        for seat in 0..seats {
            let mut c = Card::with_data(
                CardData {
                    code: 5318639,
                    type_: card_type::SPELL,
                    ..Default::default()
                },
                player,
            );
            c.current.controller = player;
            let id = f.new_card(c);
            f.add_card(player, id, location::SZONE, seat, false);
        }
    }

    /// What kind of question is outstanding. `Retry` means the last answer
    /// was rejected, so the *previous* question is still the one to answer —
    /// which is why the driver has to remember rather than read the last
    /// message.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Pending {
        None,
        Cards,
        Place(u8),
    }

    /// Run, giving up the cards at `give_up` and placing into the first free
    /// seat of **the row being asked about**.
    fn run(f: &mut Field, give_up: Option<&[usize]>) -> Status {
        // A caller that brought an answer is entering at a card question —
        // possibly one already asked, whose validation step runs first and
        // emits `Retry` before any new message.
        let mut pending = if give_up.is_some() {
            Pending::Cards
        } else {
            Pending::None
        };
        for _ in 0..2048 {
            match f.process() {
                Status::Continue => continue,
                Status::Awaiting => {
                    match f.messages.last() {
                        Some(Message::SelectCard { .. }) => {
                            if give_up.is_none() {
                                // The caller wants to look at the question
                                // rather than answer it.
                                return Status::Awaiting;
                            }
                            pending = Pending::Cards;
                        }
                        Some(Message::SelectPlace { player, .. }) => {
                            pending = Pending::Place(*player);
                        }
                        Some(Message::SelectChain { .. }) => {
                            f.core.returns.set(-1);
                            continue;
                        }
                        // A `Retry` leaves `pending` as it was.
                        Some(Message::Retry) => {}
                        _ => return Status::Awaiting,
                    }
                    match pending {
                        Pending::Cards => {
                            let picks = give_up.unwrap_or(&[]);
                            f.core.returns.set_i32(0, 0);
                            f.core.returns.set_i32(1, picks.len() as i32);
                            for (i, &pick) in picks.iter().enumerate() {
                                f.core.returns.set_i32(i + 2, pick as i32);
                            }
                        }
                        Pending::Place(p) => {
                            let seat = (0..5)
                                .find(|&i| f.players[p as usize].szone[i].is_none())
                                .unwrap_or(0) as i8;
                            f.core.returns.set_i8(0, i8::from(p != 0));
                            f.core.returns.set_i8(1, location::SZONE as i8);
                            f.core.returns.set_i8(2, seat);
                        }
                        Pending::None => return Status::Awaiting,
                    }
                }
                other => return other,
            }
        }
        panic!("did not settle");
    }

    fn adjust(f: &mut Field) {
        f.emplace(Kind::TrapMonsterAdjust {
            state: Box::default(),
        });
    }

    mod the_gather {
        use super::*;

        /// Only a trap monster whose monster-ness has been switched off.
        #[test]
        fn only_disabled_trap_monsters_are_gathered() {
            let mut f = Field::new(8000);
            let off = trap_monster(&mut f, 0, 0, true);
            let on = trap_monster(&mut f, 0, 1, false);
            assert!(f.gather_trap_monster_adjust());
            assert_eq!(f.core.trap_monster_adjust_set[0], BTreeSet::from([off]));
            assert!(!f.core.trap_monster_adjust_set[0].contains(&on));
        }

        /// **The card has to be a trap monster**, not merely something the
        /// effect happens to be on.
        #[test]
        fn an_ordinary_monster_carrying_the_effect_is_not_gathered() {
            let mut f = Field::new(8000);
            let mut c = Card::with_data(
                CardData {
                    code: 18036057,
                    type_: card_type::MONSTER,
                    ..Default::default()
                },
                0,
            );
            c.current.controller = 0;
            c.set_status(status::EFFECT_ENABLED, true);
            let id = f.new_card(c);
            f.add_card(0, id, location::MZONE, 0, false);
            f.cards[id].current.position = position::FACEUP_ATTACK;
            let mut e = Effect::new(effect_type::SINGLE, code::DISABLE_TRAPMONSTER);
            e.owner = Some(id);
            e.handler = Some(id);
            let eid = f.new_effect(e);
            f.cards[id]
                .single_effect
                .insert(code::DISABLE_TRAPMONSTER, eid);
            f.cards[id].indexer.insert(eid);

            assert!(!f.gather_trap_monster_adjust());
        }

        /// And the gather reports whether there is anything to do, which is
        /// what decides whether the unit runs at all.
        #[test]
        fn an_empty_board_reports_nothing_to_do() {
            let mut f = Field::new(8000);
            trap_monster(&mut f, 0, 0, false);
            assert!(!f.gather_trap_monster_adjust());
        }

        /// A previous pass's answer is cleared first.
        #[test]
        fn the_previous_gather_is_discarded() {
            let mut f = Field::new(8000);
            let stale = trap_monster(&mut f, 0, 0, false);
            f.core.trap_monster_adjust_set[0].insert(stale);
            assert!(!f.gather_trap_monster_adjust());
            assert!(f.core.trap_monster_adjust_set[0].is_empty());
        }
    }

    /// With room to spare, the trap monster goes back to a Spell/Trap Zone.
    #[test]
    fn a_trap_monster_with_room_goes_back() {
        let mut f = Field::new(8000);
        let c = trap_monster(&mut f, 0, 0, true);
        f.core.trap_monster_adjust_set[0].insert(c);
        adjust(&mut f);
        assert_eq!(run(&mut f, Some(&[])), Status::End);
        assert_eq!(f.cards[c].current.location, location::SZONE);
    }

    /// **`RESET_TURN_SET` is taken on the way**: the effects a card gained
    /// by being set this turn go, because it is not arriving as a newly set
    /// card — it is returning to what it was.
    ///
    /// It is the *effects* carrying that reset flag that go, not a status:
    /// `card::reset` with `RESET_EVENT` walks the card's effects and drops
    /// the ones whose `reset_flag` matches.
    #[test]
    fn the_set_this_turn_effects_are_reset() {
        let mut f = Field::new(8000);
        let c = trap_monster(&mut f, 0, 0, true);
        let mut e = Effect::new(effect_type::SINGLE, code::UPDATE_ATTACK);
        e.owner = Some(c);
        e.handler = Some(c);
        e.reset_flag = crate::field::reset::EVENT + crate::field::reset::TURN_SET;
        let id = f.new_effect(e);
        f.cards[c].single_effect.insert(code::UPDATE_ATTACK, id);
        f.cards[c].indexer.insert(id);
        assert!(f.cards[c].indexer.contains(&id));

        f.core.trap_monster_adjust_set[0].insert(c);
        adjust(&mut f);
        run(&mut f, Some(&[]));
        assert!(
            !f.cards[c].indexer.contains(&id),
            "the set-this-turn effect is gone"
        );
    }

    /// **No room at all sends every one of them to the graveyard**, with no
    /// question asked.
    #[test]
    fn no_room_sends_them_all_to_the_graveyard() {
        let mut f = Field::new(8000);
        let a = trap_monster(&mut f, 0, 0, true);
        let b = trap_monster(&mut f, 0, 1, true);
        fill_szone(&mut f, 0, 5);
        f.core.trap_monster_adjust_set[0].extend([a, b]);
        adjust(&mut f);
        assert_eq!(run(&mut f, Some(&[])), Status::End);
        assert_eq!(f.cards[a].current.location, location::GRAVE);
        assert_eq!(f.cards[b].current.location, location::GRAVE);
        assert!(
            !f.messages
                .iter()
                .any(|m| matches!(m, Message::SelectCard { .. })),
            "nothing to choose between"
        );
    }

    /// **Exactly enough room is not a question.** The comparison is `>`,
    /// not `>=`: as many trap monsters as there are free seats all fit.
    #[test]
    fn exactly_enough_room_asks_nothing() {
        let mut f = Field::new(8000);
        let a = trap_monster(&mut f, 0, 0, true);
        fill_szone(&mut f, 0, 4);
        f.core.trap_monster_adjust_set[0].insert(a);
        adjust(&mut f);
        assert_eq!(run(&mut f, Some(&[])), Status::End);
        // The prompt is pushed *before* the question is emplaced, and a
        // question for zero cards answers itself without a `SelectCard`
        // message — so the prompt is what "it asked" looks like here.
        assert!(
            !f.messages.iter().any(|m| matches!(
                m,
                Message::Hint {
                    kind: crate::host_question::hint::SELECTMSG,
                    value: 502,
                    ..
                }
            )),
            "one card, one seat: nothing to choose"
        );
        assert_eq!(f.cards[a].current.location, location::SZONE);
    }

    /// **Not enough room is a question, and the cards chosen are the ones
    /// that go** — the opposite of what a selection usually means here.
    #[test]
    fn not_enough_room_asks_which_to_give_up() {
        let mut f = Field::new(8000);
        let a = trap_monster(&mut f, 0, 0, true);
        let b = trap_monster(&mut f, 0, 1, true);
        let c = trap_monster(&mut f, 0, 2, true);
        fill_szone(&mut f, 0, 4);
        f.core.trap_monster_adjust_set[0].extend([a, b, c]);
        adjust(&mut f);
        assert_eq!(run(&mut f, None), Status::Awaiting, "which two to give up?");
        assert!(
            f.messages.iter().any(|m| matches!(
                m,
                Message::Hint {
                    kind: crate::host_question::hint::SELECTMSG,
                    value: 502,
                    ..
                }
            )),
            "asked with the right prompt"
        );
        let offered = f.core.select_cards.clone();
        assert_eq!(offered.len(), 3);

        run(&mut f, Some(&[0, 1]));
        let kept = offered[2];
        assert_eq!(
            f.cards[kept].current.location,
            location::SZONE,
            "the survivor"
        );
        assert_eq!(f.cards[offered[0]].current.location, location::GRAVE);
        assert_eq!(f.cards[offered[1]].current.location, location::GRAVE);
    }

    /// **Both players are asked, one after the other** — the machine runs
    /// its middle three cases twice.
    #[test]
    fn both_players_are_dealt_with() {
        let mut f = Field::new(8000);
        f.infos.turn_player = 0;
        let mine = trap_monster(&mut f, 0, 0, true);
        let theirs = trap_monster(&mut f, 1, 0, true);
        f.core.trap_monster_adjust_set[0].insert(mine);
        f.core.trap_monster_adjust_set[1].insert(theirs);
        adjust(&mut f);
        run(&mut f, Some(&[]));
        assert_eq!(f.cards[mine].current.location, location::SZONE);
        assert_eq!(f.cards[theirs].current.location, location::SZONE);
    }

    /// One player with no room and the other with plenty: the machine does
    /// not confuse the two.
    #[test]
    fn each_player_is_measured_against_their_own_row() {
        let mut f = Field::new(8000);
        f.infos.turn_player = 0;
        let mine = trap_monster(&mut f, 0, 0, true);
        let theirs = trap_monster(&mut f, 1, 0, true);
        fill_szone(&mut f, 1, 5);
        f.core.trap_monster_adjust_set[0].insert(mine);
        f.core.trap_monster_adjust_set[1].insert(theirs);
        adjust(&mut f);
        run(&mut f, Some(&[]));
        assert_eq!(f.cards[mine].current.location, location::SZONE, "room");
        assert_eq!(f.cards[theirs].current.location, location::GRAVE, "none");
    }

    /// **Without the duel option the first half is skipped entirely** — a
    /// trap monster already holds its Spell/Trap Zone, so there is nothing
    /// to find room for.
    #[test]
    fn without_the_duel_option_no_room_is_looked_for() {
        let mut f = Field::with_flags(
            8000,
            crate::duel::REFERENCE_CONFIGURATION & !crate::duel::flags::TRAP_MONSTERS_NOT_USE_ZONE,
        );
        let a = trap_monster(&mut f, 0, 0, true);
        let b = trap_monster(&mut f, 0, 1, true);
        fill_szone(&mut f, 0, 5);
        f.core.trap_monster_adjust_set[0].extend([a, b]);
        adjust(&mut f);
        run(&mut f, Some(&[]));
        assert!(
            !f.messages
                .iter()
                .any(|m| matches!(m, Message::SelectCard { .. })),
            "no room was looked for"
        );
        assert_ne!(
            f.cards[a].current.location,
            location::GRAVE,
            "and nothing was given up"
        );
    }
}

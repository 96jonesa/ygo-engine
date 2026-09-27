//! `swap_control` and `SwapControl` — two monsters changing hands at once.
//!
//! `GetControl`'s sibling, and the differences are the interesting part.
//! Where `get_control` moves cards **to** a player and may run out of room,
//! this one is an *exchange*: each side gives one up and gets one back, so
//! the seat arithmetic nets to zero and the whole "there is no room, so
//! destroy them" machinery is absent. What it gains instead is symmetry —
//! every check is written out twice, once per side.
//!
//! ## One gate, then a loop of pairs
//!
//! Case 0 is a **gate**: it validates both groups completely and bails to
//! case 10 — which answers `false` and clears the operated set — if
//! anything is wrong. Note how it does that: it sets `step = 9` *first*, so
//! that every `return FALSE` below lands on case 10, and only sets `step =
//! 0` again at the very bottom once everything has passed. A gate written
//! as a default rather than as an exit.
//!
//! Then cases 1–3 walk the two groups in lockstep, one pair at a time, and
//! each pair costs **two questions**: which seat each side wants to put its
//! newly acquired monster in. The seats are asked for separately because
//! they are in different rows, and the asking player is the one who *owns
//! the row*, not the one who played the card.
//!
//! ## The two seats are not the two vacancies
//!
//! Case 1 asks player 1 for a seat with the flag
//! `(flag & ~(1 << s1) & 0xff) | ~0x1f` — the free seats of *their own* row,
//! **plus** the seat the departing monster is vacating, and nothing above
//! seat 4. So a player may put the incoming monster where their own
//! outgoing one stood, or anywhere else they have room. The two answers then
//! go to `swap_card`, which does the exchange in one step rather than as two
//! moves.
//!
//! ## What a control change resets
//!
//! `RESET_CONTROL` on both, `STATUS_ATTACK_CANCELED` on both, and
//! `set_control` for the new controller — the same trio `get_control`
//! applies, because the consequences of changing hands do not depend on why.
//! The events come at the end, once, over both groups joined: a card that
//! reacts to a control change sees the finished board, not a half-swapped
//! one.

use crate::card::{reason, status};
use crate::event::{code, CardId, EffectId, PLAYER_NONE};
use crate::field::{Field, GroupId};
use crate::host_question::hint;
use crate::processor::Kind;

/// The state `SwapControl` carries.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SwapControlState {
    /// The seat the first side chose, held across the second question.
    /// The reference keeps it on the processor argument for exactly the
    /// same reason: `returns` is overwritten by the second `SelectPlace`.
    pub self_selected_sequence: u8,
    /// How far the pair loop has got. The reference walks two iterators;
    /// one index into two snapshots is the same thing.
    pub cursor: usize,
    /// The orders the loop walks, taken once the gate has passed.
    pub order1: Vec<CardId>,
    pub order2: Vec<CardId>,
}

impl Field {
    /// `field::swap_control` — exchange control of these two groups.
    pub fn swap_control(
        &mut self,
        reason_effect: Option<EffectId>,
        reason_player: u8,
        targets1: impl IntoIterator<Item = CardId>,
        targets2: impl IntoIterator<Item = CardId>,
        reset_phase: u16,
        reset_count: u8,
    ) {
        let g1 = self.new_group(targets1);
        let g2 = self.new_group(targets2);
        self.emplace(Kind::SwapControl {
            reason_effect,
            reason_player,
            targets1: g1,
            targets2: g2,
            reset_phase,
            reset_count,
            state: Box::default(),
        });
    }

    /// One step of `SwapControl`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn swap_control_step(
        &mut self,
        step: u16,
        reason_effect: Option<EffectId>,
        reason_player: u8,
        targets1: GroupId,
        targets2: GroupId,
        reset_phase: u16,
        reset_count: u8,
        state: &mut SwapControlState,
    ) -> bool {
        match step {
            0 => self.sc_gate(reason_effect, reason_player, targets1, targets2, state),
            1 => self.sc_ask_first(reason_player, targets1, targets2, state),
            2 => self.sc_ask_second(reason_player, targets1, targets2, state),
            3 => self.sc_swap_pair(targets1, targets2, reset_phase, reset_count, state),
            4 => self.sc_raise(reason_effect, reason_player, targets1, targets2),
            5 => {
                let both: Vec<CardId> = self.group(targets1).iter().copied().collect();
                self.core.operated_set = both;
                self.core.returns.set(1);
                true
            }
            10 => {
                self.core.operated_set.clear();
                self.core.returns.set(0);
                true
            }
            _ => true,
        }
    }

    /// Case 0: the gate. Everything is checked before anything is done.
    ///
    /// The refusals, in the reference's order: either group empty, the two
    /// groups a different size, the two leaders under the same control (or
    /// under nobody's), and then per card — an Xyz material, a card under
    /// the wrong control, a card off the Monster Zone, a card that cannot
    /// change hands, and a card the reason effect does not affect.
    ///
    /// The last two refusals are the seat count, and they are subtle: the
    /// row must have room for **every target of that side that is sitting
    /// above seat 4**, because an Extra Monster Zone monster coming back
    /// needs a main seat while one already in a main seat is merely
    /// changing sides. In a five-zone configuration no target is ever above
    /// seat 4, so the decrement never fires.
    fn sc_gate(
        &mut self,
        reason_effect: Option<EffectId>,
        reason_player: u8,
        targets1: GroupId,
        targets2: GroupId,
        state: &mut SwapControlState,
    ) -> bool {
        // Set first, cleared last: every refusal below falls through to
        // case 10 without having to say so.
        self.set_step(9);
        let g1: Vec<CardId> = self.group(targets1).iter().copied().collect();
        let g2: Vec<CardId> = self.group(targets2).iter().copied().collect();
        if g1.is_empty() || g2.is_empty() || g1.len() != g2.len() {
            return false;
        }
        for &card in g1.iter().chain(g2.iter()) {
            self.filter_disable_related_cards(card);
        }
        let (p1, p2) = (
            self.cards[g1[0]].current.controller,
            self.cards[g2[0]].current.controller,
        );
        if p1 == p2 || p1 == PLAYER_NONE || p2 == PLAYER_NONE {
            return false;
        }
        for (group, owner) in [(&g1, p1), (&g2, p2)] {
            for &card in group {
                let c = &self.cards[card];
                if c.overlay_target.is_some()
                    || c.current.controller != owner
                    || c.current.location != crate::board::location::MZONE
                {
                    return false;
                }
                if !self.is_capable_change_control(card) {
                    return false;
                }
                if reason_effect.is_some() && !self.is_affect_by_effect(card, reason_effect) {
                    return false;
                }
            }
        }
        for (group, owner) in [(&g1, p1), (&g2, p2)] {
            let mut ct = self.get_useable_count(
                None,
                owner,
                crate::board::location::MZONE,
                reason_player,
                Self::LOCATION_REASON_CONTROL,
                0xff,
            );
            for &card in group {
                if self.cards[card].current.sequence >= 5 {
                    ct -= 1;
                }
            }
            if ct < 0 {
                return false;
            }
        }
        for &card in g1.iter().chain(g2.iter()) {
            if self.cards[card].unique_code != 0
                && self.cards[card].unique_location & u16::from(crate::board::location::MZONE) != 0
            {
                self.remove_unique_card(card);
            }
        }
        state.order1 = g1;
        state.order2 = g2;
        self.set_step(0);
        false
    }

    /// Case 1: ask the first side where to seat what it is about to gain,
    /// or leave the loop.
    ///
    /// The hint names the **other** card — the one arriving — so the
    /// question reads "where do you want this?" rather than naming the
    /// monster being given away.
    fn sc_ask_first(
        &mut self,
        reason_player: u8,
        targets1: GroupId,
        targets2: GroupId,
        state: &mut SwapControlState,
    ) -> bool {
        let _ = (targets1, targets2);
        let Some(&card1) = state.order1.get(state.cursor) else {
            self.set_step(3);
            return false;
        };
        let card2 = state.order2[state.cursor];
        self.ask_for_seat(card1, card2, reason_player);
        false
    }

    /// Case 2: the same question, of the other side. The first answer is
    /// stowed on the way past, because the second question will overwrite
    /// `returns`.
    fn sc_ask_second(
        &mut self,
        reason_player: u8,
        targets1: GroupId,
        targets2: GroupId,
        state: &mut SwapControlState,
    ) -> bool {
        let _ = (targets1, targets2);
        state.self_selected_sequence = self.core.returns.at_i8(2) as u8;
        let card1 = state.order1[state.cursor];
        let card2 = state.order2[state.cursor];
        self.ask_for_seat(card2, card1, reason_player);
        false
    }

    /// The question both cases ask: `giver`'s controller picks a seat for
    /// `arriving`.
    ///
    /// The mask is `(free & ~(1 << giver's seat) & 0xff) | ~0x1f`: their own
    /// free seats, *minus* the one being vacated — which reads backwards
    /// until you remember the flag names what is **forbidden**, so clearing
    /// a bit makes that seat available. Everything from bit 5 up is set,
    /// which forbids it.
    fn ask_for_seat(&mut self, giver: CardId, arriving: CardId, reason_player: u8) {
        let (player, seat) = {
            let c = &self.cards[giver].current;
            (c.controller, c.sequence)
        };
        let mut flag = 0u32;
        self.get_useable_count_with_flag(
            None,
            player,
            crate::board::location::MZONE,
            reason_player,
            Self::LOCATION_REASON_CONTROL,
            0xff,
            &mut flag,
        );
        let flag = (flag & !(1 << seat) & 0xff) | !0x1f;
        let code = self.cards[arriving].data.code;
        self.messages.push(crate::field::Message::Hint {
            kind: hint::SELECTMSG,
            player,
            value: u64::from(code),
        });
        self.emplace(Kind::SelectPlace {
            player,
            flag,
            count: 1,
            disable_field: false,
        });
    }

    /// Case 3: exchange the pair, hand each to its new controller, and go
    /// back for the next.
    fn sc_swap_pair(
        &mut self,
        targets1: GroupId,
        targets2: GroupId,
        reset_phase: u16,
        reset_count: u8,
        state: &mut SwapControlState,
    ) -> bool {
        let _ = (targets1, targets2);
        let card1 = state.order1[state.cursor];
        let card2 = state.order2[state.cursor];
        let p1 = self.cards[card1].current.controller;
        let p2 = self.cards[card2].current.controller;
        let new_s1 = u32::from(state.self_selected_sequence);
        let new_s2 = self.core.returns.at_i8(2) as u32;
        self.swap_card(card1, card2, new_s1, new_s2);
        for card in [card1, card2] {
            self.reset_card(
                card,
                crate::field::reset::CONTROL,
                crate::field::reset::EVENT,
            );
        }
        self.set_control(card1, p2, reset_phase, reset_count);
        self.set_control(card2, p1, reset_phase, reset_count);
        for card in [card1, card2] {
            self.cards[card].set_status(status::ATTACK_CANCELED, true);
        }
        state.cursor += 1;
        self.set_step(0);
        false
    }

    /// Case 4: the two groups become one, the uniqueness limits go back,
    /// and the events are raised over the lot.
    ///
    /// The join is into `targets1`, which is why case 5 reads the operated
    /// set out of that group alone.
    fn sc_raise(
        &mut self,
        reason_effect: Option<EffectId>,
        reason_player: u8,
        targets1: GroupId,
        targets2: GroupId,
    ) -> bool {
        let second: Vec<CardId> = self.group(targets2).iter().copied().collect();
        for card in second {
            self.group_mut(targets1).insert(card);
        }
        let all: Vec<CardId> = self.group(targets1).iter().copied().collect();
        for &card in &all {
            self.filter_disable_related_cards(card);
            if self.cards[card].unique_code != 0
                && self.cards[card].unique_location & u16::from(crate::board::location::MZONE) != 0
            {
                self.add_unique_card(card);
            }
            let con = self.cards[card].current.controller;
            for event in [code::CONTROL_CHANGED, code::MOVE] {
                self.raise_single_event(
                    card,
                    vec![],
                    event,
                    reason_effect,
                    reason::EFFECT,
                    reason_player,
                    con,
                    0,
                );
            }
        }
        for event in [code::CONTROL_CHANGED, code::MOVE] {
            self.raise_event_over(
                all.clone(),
                event,
                reason_effect,
                reason::EFFECT,
                reason_player,
                0,
                0,
            );
        }
        self.process_single_event();
        self.process_instant_event();
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::{location, position};
    use crate::card::{card_type, Card, CardData};
    use crate::effect::{effect_type, Effect};
    use crate::field::Message;
    use crate::processor::Status;

    fn monster(f: &mut Field, player: u8, code_: u32, seq: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: code_,
                type_: card_type::MONSTER | card_type::NORMAL,
                level: 4,
                attack: 1000,
                defense: 1000,
                ..Default::default()
            },
            player,
        );
        c.current.controller = player;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(player, id, location::MZONE, seq, false);
        f.cards[id].current.position = position::FACEUP_ATTACK;
        id
    }

    /// `mine` monsters for player 0 and `theirs` for player 1, seated
    /// from zero.
    fn board(mine: u32, theirs: u32) -> (Field, Vec<CardId>, Vec<CardId>) {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        let a = (0..mine)
            .map(|i| monster(&mut f, 0, 7_000 + i, i))
            .collect();
        let b = (0..theirs)
            .map(|i| monster(&mut f, 1, 8_000 + i, i))
            .collect();
        (f, a, b)
    }

    /// Run `swap_control` far enough to see whether the gate let it
    /// through. The first seat question means it did — a refusal asks
    /// nothing at all.
    fn gate_passes(f: &mut Field, g1: &[CardId], g2: &[CardId]) -> bool {
        f.swap_control(None, 0, g1.to_vec(), g2.to_vec(), 0, 0);
        for _ in 0..512 {
            match f.process() {
                Status::Awaiting => return true,
                Status::Continue => {
                    if f.core.units.is_empty() && f.core.subunits.is_empty() {
                        break;
                    }
                }
                _ => break,
            }
        }
        false
    }

    /// Run it to completion, taking the lowest free seat each time, and
    /// report how many seats were asked for.
    fn run_to_end(f: &mut Field) -> usize {
        let mut seats = 0;
        for _ in 0..512 {
            match f.process() {
                Status::Awaiting => {
                    let Some(Message::SelectPlace { player, flag, .. }) = f.messages.last() else {
                        panic!("unexpected question: {:?}", f.messages.last())
                    };
                    let (pl, flag) = (*player, *flag);
                    seats += 1;
                    let seq = (0..5u32)
                        .find(|s| flag & (1 << s) == 0)
                        .expect("a free monster seat");
                    f.core.returns.set_i8(0, pl as i8);
                    f.core.returns.set_i8(1, location::MZONE as i8);
                    f.core.returns.set_i8(2, seq as i8);
                }
                Status::Continue => {
                    if f.core.units.is_empty() && f.core.subunits.is_empty() {
                        break;
                    }
                }
                other => panic!("stopped with {other:?}"),
            }
        }
        seats
    }

    /// **The gate's shape refusals, one board each** — and the positive
    /// sibling, because every one of these is a negative assertion.
    #[test]
    fn the_gate_refuses_a_swap_it_cannot_make() {
        // An empty side, and both sides empty.
        let (mut f, ours, _) = board(1, 0);
        assert!(!gate_passes(&mut f, &ours, &[]));
        let (mut f, _, _) = board(0, 0);
        assert!(!gate_passes(&mut f, &[], &[]));
        // Two monsters on the same side.
        let (mut f, ours, _) = board(2, 0);
        assert!(!gate_passes(&mut f, &[ours[0]], &[ours[1]]));
        // Uneven sizes.
        let (mut f, ours, theirs) = board(2, 1);
        assert!(!gate_passes(&mut f, &ours, &theirs));
        // A monster that is nowhere at all.
        let (mut f, ours, theirs) = board(1, 1);
        f.remove_card(theirs[0]);
        assert!(!gate_passes(&mut f, &ours, &theirs));
        // The positive sibling: the same shape, nothing wrong with it.
        let (mut f, ours, theirs) = board(1, 1);
        assert!(gate_passes(&mut f, &ours, &theirs));
    }

    /// **The wrong zone and the wrong side are both refused**, and
    /// neither has a stand-in among the shape checks.
    #[test]
    fn the_gate_refuses_the_wrong_zone_and_the_wrong_side() {
        // In the graveyard: a real controller, the wrong location.
        let (mut f, ours, theirs) = board(1, 1);
        f.move_card(1, theirs[0], location::GRAVE, 0, false);
        assert!(!gate_passes(&mut f, &ours, &theirs));

        // The second group's leader names player 1, and the card behind
        // it is player 0's. A group is a *set* ordered by card id, so the
        // odd card out has to be **created last** — put it first and it
        // becomes the leader, and the refusal that fires is `p1 == p2`
        // instead, which proves nothing about this clause.
        let (mut f, ours, theirs) = board(2, 1);
        let stray = monster(&mut f, 0, 7_900, 2);
        assert!(stray > theirs[0], "the stray must sort after the leader");
        assert!(!gate_passes(
            &mut f,
            &[ours[0], ours[1]],
            &[theirs[0], stray]
        ));
        // The positive sibling: the same sizes, both sides consistent.
        let (mut f, ours, theirs) = board(2, 2);
        assert!(gate_passes(
            &mut f,
            &[ours[0], ours[1]],
            &[theirs[0], theirs[1]]
        ));
    }

    /// **A monster under a control lock is refused**, which `GetControl`
    /// and this share — and it is the check a caller's own filter cannot
    /// be relied on to have made.
    #[test]
    fn the_gate_refuses_a_monster_that_cannot_change_hands() {
        let (mut f, ours, theirs) = board(1, 1);
        let mut e = Effect::new(effect_type::SINGLE, code::CANNOT_CHANGE_CONTROL);
        e.owner = Some(theirs[0]);
        e.handler = Some(theirs[0]);
        let id = f.new_effect(e);
        f.cards[theirs[0]]
            .single_effect
            .insert(code::CANNOT_CHANGE_CONTROL, id);
        f.cards[theirs[0]].indexer.insert(id);
        assert!(!gate_passes(&mut f, &ours, &theirs));
        assert_eq!(f.core.returns.at_i32(0), 0);
        assert!(f.core.operated_set.is_empty());
    }

    /// **A monster immune to the effect that asked is refused**, and an
    /// exchange with no effect behind it is not asked the question at
    /// all.
    #[test]
    fn the_gate_refuses_a_monster_the_effect_cannot_touch() {
        let (mut f, ours, theirs) = board(1, 1);
        let source = ours[0];
        let mut e = Effect::new(effect_type::SINGLE, 0);
        e.owner = Some(source);
        e.handler = Some(source);
        let by = f.new_effect(e);

        // An immunity lives in its own container and must answer with a
        // non-zero value.
        let mut imm = Effect::new(effect_type::SINGLE, code::IMMUNE_EFFECT);
        imm.owner = Some(theirs[0]);
        imm.handler = Some(theirs[0]);
        imm.value = 1;
        let imm = f.new_effect(imm);
        f.cards[theirs[0]].immune_effect.push(imm);
        f.cards[theirs[0]].indexer.insert(imm);

        f.swap_control(Some(by), 0, ours.clone(), theirs.clone(), 0, 0);
        assert_eq!(run_to_end(&mut f), 0, "nothing was asked");
        assert_eq!(f.core.returns.at_i32(0), 0);
        // The sibling: the same board with no effect named goes through.
        let (mut f, ours, theirs) = board(1, 1);
        assert!(gate_passes(&mut f, &ours, &theirs));
    }

    /// **A full row is still room enough**, because the exchange gives a
    /// seat back — the boundary is `ct < 0`, not `ct <= 0`.
    #[test]
    fn a_full_row_is_still_room_enough() {
        let (mut f, mine, theirs) = board(5, 5);
        assert_eq!(
            f.get_useable_count(
                None,
                0,
                location::MZONE,
                0,
                Field::LOCATION_REASON_CONTROL,
                0xff
            ),
            0,
            "not a seat to spare"
        );
        assert!(gate_passes(&mut f, &[mine[0]], &[theirs[0]]));
    }

    /// **A monster above the row costs its own side a seat.** The only
    /// board on a five-zone field where the count can go negative: a
    /// full row *plus* a monster in an extra zone, which has no main
    /// seat to come home to.
    ///
    /// This is also the one place the per-side loop matters — the
    /// shortfall is on the **second** group, so a count taken over one
    /// side only would let it through.
    #[test]
    fn a_monster_above_the_row_needs_a_seat_on_its_own_side() {
        let (mut f, mine, _) = board(1, 5);
        let above = monster(&mut f, 1, 8_500, 5);
        assert_eq!(f.cards[above].current.sequence, 5);
        assert!(
            !gate_passes(&mut f, &[mine[0]], &[above]),
            "their row is full and the monster needs a seat in it"
        );
        // Free one of their main seats and it goes through.
        let (mut f, mine, _) = board(1, 4);
        let above = monster(&mut f, 1, 8_500, 5);
        assert!(
            gate_passes(&mut f, &[mine[0]], &[above]),
            "now there is room"
        );
    }

    /// **A completed exchange answers true and reports both monsters**,
    /// joined into one group — which is what a script reading the
    /// operated set would see.
    #[test]
    fn a_completed_exchange_reports_both_monsters() {
        let (mut f, ours, theirs) = board(1, 1);
        let (a, b) = (ours[0], theirs[0]);
        f.swap_control(None, 0, ours, theirs, 0, 0);
        assert_eq!(run_to_end(&mut f), 2, "one seat question each");
        assert_eq!(f.core.returns.at_i32(0), 1, "it happened");
        let mut got = f.core.operated_set.clone();
        got.sort_unstable();
        let mut want = vec![a, b];
        want.sort_unstable();
        assert_eq!(got, want);
        assert_eq!(f.cards[a].current.controller, 1);
        assert_eq!(f.cards[b].current.controller, 0);
    }

    /// **Control is handed over as an *effect*, not as an assignment.**
    ///
    /// `swap_card` moves each monster into the other row and sets
    /// `current.controler` on the way, so the board looks right either
    /// way. What `set_control` adds is an `EFFECT_SET_CONTROL` naming the
    /// new controller — and that is the thing control *lapsing* reads, so
    /// a swap that skipped it would look correct until the moment the
    /// control was supposed to end.
    #[test]
    fn each_monster_carries_an_effect_naming_its_new_controller() {
        let (mut f, ours, theirs) = board(1, 1);
        let (a, b) = (ours[0], theirs[0]);
        f.swap_control(None, 0, ours, theirs, 0, 0);
        run_to_end(&mut f);
        for (card, want) in [(a, 1u8), (b, 0u8)] {
            let ids = f.cards[card].single_effect.equal_range(code::SET_CONTROL);
            assert_eq!(ids.len(), 1, "one control effect on card {card}");
            let e = f.effects.get(ids[0]).unwrap();
            assert_eq!(e.value, i64::from(want), "it names the new controller");
            assert_eq!(
                f.refresh_control_status(card).0,
                want,
                "and that is who the card answers to"
            );
        }
    }

    /// **Both events are raised.**
    #[test]
    fn the_exchange_announces_a_control_change_and_a_move() {
        let (mut f, ours, theirs) = board(1, 1);
        f.swap_control(None, 0, ours, theirs, 0, 0);
        run_to_end(&mut f);
        for want in [code::CONTROL_CHANGED, code::MOVE] {
            assert!(
                f.core
                    .instant_event
                    .iter()
                    .chain(f.core.used_event.iter())
                    .any(|ev| ev.event_code == want),
                "event {want} was raised"
            );
        }
    }

    /// **A swapped monster stops attacking and loses what it held by
    /// being on that side.**
    #[test]
    fn the_exchange_cancels_attacks_and_resets_control_effects() {
        let (mut f, ours, theirs) = board(1, 1);
        let (a, b) = (ours[0], theirs[0]);
        let mut e = Effect::new(effect_type::SINGLE, code::UPDATE_ATTACK);
        e.owner = Some(a);
        e.handler = Some(a);
        e.reset_flag = crate::field::reset::EVENT + crate::field::reset::CONTROL;
        let watched = f.new_effect(e);
        f.cards[a]
            .single_effect
            .insert(code::UPDATE_ATTACK, watched);
        f.cards[a].indexer.insert(watched);

        f.swap_control(None, 0, ours, theirs, 0, 0);
        run_to_end(&mut f);
        for card in [a, b] {
            assert!(f.cards[card].is_status(status::ATTACK_CANCELED));
        }
        assert!(!f.cards[a].indexer.contains(&watched));
    }
}

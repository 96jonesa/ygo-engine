//! `get_control` and `GetControl` — moving a monster to the other side.
//!
//! Eight cases, and the shape is one the port has seen before: a filter, a
//! "there is not room for all of these" question, a cursor loop that moves
//! them one at a time, and a tail that raises the events.
//!
//! ## Control is a *move*, not an assignment
//!
//! Case 3 calls `move_to_field`. A monster changing hands physically leaves
//! one player's row and arrives in the other's, which is why it needs a seat,
//! why it can be refused for want of one, and why the whole machine is built
//! around counting seats rather than around setting a field.
//!
//! ## What happens when there is not enough room
//!
//! **No room at all** destroys every target. **Not enough** asks the
//! *receiving* player which to give up — and the ones chosen are destroyed,
//! not left behind. Both are `REASON_RULE` destructions with no effect
//! attributed, because the rules did it.
//!
//! ## The uniqueness dance
//!
//! A card with an "only one face-up" limit is taken **out** of the uniqueness
//! registry before it moves (case 2) and put **back** after (case 5), because
//! the registry is keyed by controller. Skipping either half leaves the
//! registry pointing at the wrong side, which nothing would notice until the
//! next uniqueness check ran and found nothing.

use crate::board::location;
use crate::card::{card_type, reason, status};
use crate::event::{code, CardId, EffectId, PLAYER_NONE};
use crate::field::{Field, GroupId, Message};
use crate::processor::Kind;
use std::collections::BTreeSet;

/// The state `GetControl` carries.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GetControlState {
    /// The targets that could not be given a seat.
    pub destroy_set: BTreeSet<CardId>,
    /// How far the move loop has got. The reference keeps an iterator on the
    /// group; an index into a snapshot is the same thing for a set that the
    /// loop does not modify.
    pub cursor: usize,
    /// The order the loop walks, taken once when the loop starts.
    pub order: Vec<CardId>,
}

impl Field {
    /// `card::is_capable_change_control`.
    pub fn is_capable_change_control(&mut self, card: CardId) -> bool {
        self.is_affected_by_effect(card, code::CANNOT_CHANGE_CONTROL)
            .is_none()
    }

    /// `card::is_control_can_be_changed` — the *whole* question, where
    /// [`Self::is_capable_change_control`] is only the lock.
    ///
    /// Five refusals, and the middle two are about **room on the other
    /// side**: a monster cannot change hands into a full row, and a trap
    /// monster additionally needs a Spell & Trap seat there. `ignore_mzone`
    /// waives the first — a caller that is going to make room itself, or
    /// that is asking hypothetically, passes it.
    ///
    /// Note which player is counted: `1 - current.controler` for the room,
    /// with `current.controler` as the one doing the putting.
    pub fn is_control_can_be_changed(
        &mut self,
        card: CardId,
        ignore_mzone: bool,
        zone: u32,
    ) -> bool {
        let (controller, loc) = {
            let c = &self.cards[card].current;
            (c.controller, c.location)
        };
        if controller == PLAYER_NONE || loc != location::MZONE {
            return false;
        }
        if !ignore_mzone
            && self.get_useable_count(
                Some(card),
                1 - controller,
                location::MZONE,
                controller,
                Self::LOCATION_REASON_CONTROL,
                zone,
            ) <= 0
        {
            return false;
        }
        if !self.is_flag(crate::duel::flags::TRAP_MONSTERS_NOT_USE_ZONE) {
            let is_trap_monster =
                self.get_type(card, None, 0, controller) & card_type::TRAPMONSTER != 0;
            if is_trap_monster
                && self.get_useable_count(
                    Some(card),
                    1 - controller,
                    location::SZONE,
                    controller,
                    Self::LOCATION_REASON_CONTROL,
                    0xff,
                ) <= 0
            {
                return false;
            }
        }
        self.is_capable_change_control(card)
    }

    /// `field::add_unique_card` — put a uniqueness limit back into the
    /// registry, on whichever sides it watches.
    ///
    /// `unique_fieldid` is reset to zero: the card has just arrived, so
    /// whatever copy the limit used to point at is no longer the answer.
    pub fn add_unique_card(&mut self, card: CardId) {
        let con = self.cards[card].current.controller;
        if con == PLAYER_NONE {
            return;
        }
        let pos = self.cards[card].unique_pos;
        if pos[0] != 0 && !self.core.unique_cards[con as usize].contains(&card) {
            self.core.unique_cards[con as usize].push(card);
        }
        if pos[1] != 0 && !self.core.unique_cards[1 - con as usize].contains(&card) {
            self.core.unique_cards[1 - con as usize].push(card);
        }
        self.cards[card].unique_fieldid = 0;
    }

    /// `field::get_control` — give these cards to `playerid`.
    #[allow(clippy::too_many_arguments)]
    pub fn get_control(
        &mut self,
        targets: impl IntoIterator<Item = CardId>,
        reason_effect: Option<EffectId>,
        chose_player: u8,
        playerid: u8,
        reset_phase: u16,
        reset_count: u8,
        zone: u32,
    ) {
        let group = self.new_group(targets);
        self.emplace(Kind::GetControl {
            reason_effect,
            chose_player,
            targets: group,
            playerid,
            reset_phase,
            reset_count,
            zone,
            state: Box::default(),
        });
    }

    /// One step of `GetControl`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn get_control_step(
        &mut self,
        step: u16,
        reason_effect: Option<EffectId>,
        chose_player: u8,
        targets: GroupId,
        playerid: u8,
        reset_phase: u16,
        reset_count: u8,
        zone: u32,
        state: &mut GetControlState,
    ) -> bool {
        match step {
            0 => self.gc_filter(reason_effect, targets, playerid, zone, state),
            1 => {
                let chosen: Vec<CardId> = self.core.return_cards.list.clone();
                state.destroy_set.extend(chosen.iter().copied());
                for card in chosen {
                    self.group_mut(targets).remove(&card);
                }
                false
            }
            2 => self.gc_start_loop(targets, state),
            3 => self.gc_move_one(chose_player, targets, playerid, zone, state),
            4 => self.gc_settle_one(playerid, reset_phase, reset_count, state),
            5 => self.gc_raise(reason_effect, chose_player, targets, playerid),
            6 => {
                let doomed = std::mem::take(&mut state.destroy_set);
                if !doomed.is_empty() {
                    self.destroy(doomed, None, reason::RULE, PLAYER_NONE, PLAYER_NONE, 0, 0);
                }
                false
            }
            7 => {
                let survivors: Vec<CardId> = self.group(targets).iter().copied().collect();
                self.core.returns.set(survivors.len() as i32);
                self.core.operated_set = survivors;
                true
            }
            _ => true,
        }
    }

    /// Step 0: drop the targets that cannot change hands, then find seats.
    ///
    /// Six reasons a card is dropped, and the last is the one that is easy to
    /// miss: **in a configuration where a trap monster occupies a Spell/Trap
    /// Zone as well, it needs room in that row too** — so a trap monster can
    /// be blocked from changing hands by a full spell/trap row rather than a
    /// full monster row.
    fn gc_filter(
        &mut self,
        reason_effect: Option<EffectId>,
        targets: GroupId,
        playerid: u8,
        zone: u32,
        state: &mut GetControlState,
    ) -> bool {
        state.destroy_set.clear();
        for card in self.group(targets).iter().copied().collect::<Vec<_>>() {
            self.filter_disable_related_cards(card);
            let c = &self.cards[card];
            let mut change = c.overlay_target.is_none()
                && c.current.controller != playerid
                && c.current.controller != PLAYER_NONE
                && c.current.location == location::MZONE;
            if change && !self.is_capable_change_control(card) {
                change = false;
            }
            if change {
                if let Some(by) = reason_effect {
                    if !self.is_affect_by_effect(card, Some(by)) {
                        change = false;
                    }
                }
            }
            if change && !self.is_flag(crate::duel::flags::TRAP_MONSTERS_NOT_USE_ZONE) {
                let is_trap_monster =
                    self.get_type(card, None, 0, playerid) & card_type::TRAPMONSTER != 0;
                if is_trap_monster
                    && self.get_useable_count(
                        Some(card),
                        playerid,
                        location::SZONE,
                        playerid,
                        Self::LOCATION_REASON_CONTROL,
                        0xff,
                    ) <= 0
                {
                    change = false;
                }
            }
            if !change {
                self.group_mut(targets).remove(&card);
            }
        }

        let room = self.get_useable_count(
            None,
            playerid,
            location::MZONE,
            playerid,
            Self::LOCATION_REASON_CONTROL,
            zone,
        );
        if room <= 0 {
            let all: BTreeSet<CardId> = self.group(targets).clone();
            self.group_mut(targets).clear();
            state.destroy_set = all;
            self.set_step(5);
            return false;
        }
        let waiting = self.group(targets).len() as i32;
        if waiting > room {
            self.core.select_cards = self.group(targets).iter().copied().collect();
            self.messages.push(Message::Hint {
                kind: crate::host_question::hint::SELECTMSG,
                player: playerid,
                value: 502,
            });
            let surplus = (waiting - room) as u8;
            self.emplace(Kind::SelectCard {
                player: playerid,
                cancelable: false,
                min: surplus,
                max: surplus,
            });
        } else {
            self.set_step(1);
        }
        false
    }

    /// Step 2: take the cards out of the uniqueness registry, and start the
    /// loop.
    ///
    /// **Re-entered once per card**, because case 4 jumps back here — so the
    /// registry removal runs again each time round. Harmless (the second
    /// removal finds nothing) and faithful; the cursor is what actually
    /// advances.
    fn gc_start_loop(&mut self, targets: GroupId, state: &mut GetControlState) -> bool {
        for card in self.group(targets).iter().copied().collect::<Vec<_>>() {
            if self.cards[card].unique_code != 0
                && self.cards[card].unique_location & u16::from(location::MZONE) != 0
            {
                self.remove_unique_card(card);
            }
        }
        state.order = self.group(targets).iter().copied().collect();
        false
    }

    /// Step 3: move the next card, or leave the loop.
    fn gc_move_one(
        &mut self,
        chose_player: u8,
        targets: GroupId,
        playerid: u8,
        zone: u32,
        state: &mut GetControlState,
    ) -> bool {
        let Some(&card) = state.order.get(state.cursor) else {
            self.adjust_instant();
            self.set_step(4);
            return false;
        };
        let _ = targets;
        let move_player = if chose_player == PLAYER_NONE {
            playerid
        } else {
            chose_player
        };
        let position = self.cards[card].current.position;
        self.move_to_field(
            card,
            move_player,
            playerid,
            u16::from(location::MZONE),
            position,
            false,
            0,
            zone,
            false,
            0,
            true,
        );
        false
    }

    /// Step 4: record the change on the card that just moved, and go back
    /// for the next.
    ///
    /// **`STATUS_ATTACK_CANCELED`** is set because a monster that changes
    /// hands mid-battle is no longer attacking. `RESET_CONTROL` then takes
    /// the effects it held by virtue of being on that side.
    fn gc_settle_one(
        &mut self,
        playerid: u8,
        reset_phase: u16,
        reset_count: u8,
        state: &mut GetControlState,
    ) -> bool {
        let Some(&card) = state.order.get(state.cursor) else {
            return false;
        };
        self.cards[card].set_status(status::ATTACK_CANCELED, true);
        self.set_control(card, playerid, reset_phase, reset_count);
        self.reset_card(
            card,
            crate::field::reset::CONTROL,
            crate::field::reset::EVENT,
        );
        self.filter_disable_related_cards(card);
        state.cursor += 1;
        self.set_step(2);
        false
    }

    /// Step 5: put the uniqueness limits back, and raise the events.
    ///
    /// A card that is **no longer on the field** is dropped from the group
    /// first: it may have been destroyed on arrival by a uniqueness limit, or
    /// by anything else the move set off.
    ///
    /// `reason_player` is `PLAYER_NONE` when nobody chose — a rule-driven
    /// control change has no author.
    fn gc_raise(
        &mut self,
        reason_effect: Option<EffectId>,
        chose_player: u8,
        targets: GroupId,
        playerid: u8,
    ) -> bool {
        let reason_player = if chose_player == PLAYER_NONE {
            PLAYER_NONE
        } else {
            self.core.reason_player
        };
        for card in self.group(targets).iter().copied().collect::<Vec<_>>() {
            if !self.cards[card]
                .current
                .is_location(u16::from(location::ONFIELD))
            {
                self.group_mut(targets).remove(&card);
                continue;
            }
            if self.cards[card].unique_code != 0
                && self.cards[card].unique_location & u16::from(location::MZONE) != 0
            {
                self.add_unique_card(card);
            }
            for event in [code::CONTROL_CHANGED, code::MOVE] {
                self.raise_single_event(
                    card,
                    vec![],
                    event,
                    reason_effect,
                    reason::EFFECT,
                    reason_player,
                    playerid,
                    0,
                );
            }
        }
        let survivors: Vec<CardId> = self.group(targets).iter().copied().collect();
        if !survivors.is_empty() {
            for event in [code::CONTROL_CHANGED, code::MOVE] {
                self.raise_event_over(
                    survivors.clone(),
                    event,
                    reason_effect,
                    reason::EFFECT,
                    reason_player,
                    playerid,
                    0,
                );
            }
        }
        self.process_single_event();
        self.process_instant_event();
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::position;
    use crate::card::{card_type, status, Card, CardData};
    use crate::effect::{effect_type, Effect};
    use crate::processor::Status;

    fn monster(f: &mut Field, player: u8, seat: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 18036057 + seat,
                type_: card_type::MONSTER,
                ..Default::default()
            },
            player,
        );
        c.current.controller = player;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(player, id, location::MZONE, seat, false);
        f.cards[id].current.position = position::FACEUP_ATTACK;
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

    fn fill_mzone(f: &mut Field, player: u8, seats: u32) {
        for seat in 0..seats {
            monster(f, player, seat);
        }
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Pending {
        None,
        Cards,
        Place(u8),
    }

    /// Run, giving up the cards at `give_up` and placing into the first free
    /// seat of the row being asked about. `None` stops at a card question so
    /// the caller can look at it.
    fn run(f: &mut Field, give_up: Option<&[usize]>) -> Status {
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
                                .find(|&i| f.players[p as usize].mzone[i].is_none())
                                .unwrap_or(0) as i8;
                            f.core.returns.set_i8(0, i8::from(p != 0));
                            f.core.returns.set_i8(1, location::MZONE as i8);
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

    fn take(f: &mut Field, cards: &[CardId], to: u8) {
        f.get_control(cards.to_vec(), None, PLAYER_NONE, to, 0, 0, 0xff);
    }

    /// **Control is a move.** The card leaves one row and arrives in the
    /// other's, and says so.
    #[test]
    fn a_monster_changes_hands_by_moving() {
        let mut f = Field::new(8000);
        let c = monster(&mut f, 0, 0);
        take(&mut f, &[c], 1);
        assert_eq!(run(&mut f, Some(&[])), Status::End);
        assert_eq!(f.cards[c].current.controller, 1);
        assert_eq!(f.cards[c].current.location, location::MZONE);
        assert_eq!(f.players[1].mzone[0], Some(c), "in the other player's row");
        assert!(f.players[0].mzone.iter().all(|s| *s != Some(c)));
        assert_eq!(f.core.returns.get(), 1);
        assert_eq!(f.core.operated_set, vec![c]);
    }

    /// **The newest `EFFECT_SET_CONTROL` wins, wherever it lives.**
    ///
    /// `refresh_control_status` takes the *last* of `filter_effect`'s
    /// answers, and the reference's `filter_effect` sorts by effect id
    /// (`card.cpp`, `if(sort) std::sort(..., effect_sort_id)`), so "last"
    /// means *newest*. A control change made after an Equip Spell's
    /// continuous `SET_CONTROL` therefore beats it — Creature Swap
    /// giving a Snatch-Stolen monster back is the pool's case.
    ///
    /// The port's `filter_effect` walked its sources in a fixed order
    /// with the card's own effects first and equips after, so the older
    /// equip effect came last and pulled the monster straight back.
    #[test]
    fn a_newer_control_change_beats_an_older_equip_effect() {
        let mut f = Field::new(8000);
        // Player 0's monster, currently held by player 1 …
        let c = monster(&mut f, 1, 0);
        f.cards[c].owner = 0;
        // … by an Equip Spell in player 1's row whose equip effect says
        // "controller = 1", registered *before* anything else.
        let mut d = Card::with_data(
            CardData {
                code: 45_986_603,
                type_: card_type::SPELL | card_type::EQUIP,
                ..Default::default()
            },
            1,
        );
        d.current.controller = 1;
        d.set_status(status::EFFECT_ENABLED, true);
        let eq = f.new_card(d);
        f.add_card(1, eq, location::SZONE, 0, false);
        f.cards[eq].current.position = position::FACEUP;
        f.cards[eq].equiping_target = Some(c);
        f.cards[c].equiping_cards.push(eq);
        let mut e = Effect::new(effect_type::EQUIP, code::SET_CONTROL);
        e.owner = Some(eq);
        e.handler = Some(eq);
        e.value = 1;
        let steal = f.new_effect(e);
        f.cards[eq].equip_effect.insert(code::SET_CONTROL, steal);
        assert_eq!(
            f.refresh_control_status(c),
            (1, Some(steal)),
            "held by the equip"
        );

        // Now a later effect gives the monster to player 0.
        take(&mut f, &[c], 0);
        assert_eq!(run(&mut f, Some(&[])), Status::End);
        let (who, by) = f.refresh_control_status(c);
        assert_eq!(who, 0, "the newer change wins");
        assert_ne!(
            by,
            Some(steal),
            "and it is not the equip's effect that decides"
        );
        assert!(
            f.effects.get(by.unwrap()).unwrap().id.get() > f.effects.get(steal).unwrap().id.get(),
            "newest by id"
        );
        assert_eq!(f.cards[c].current.controller, 0);
    }

    /// **`RESET_TURN_SET` pins the controller.** A monster held by an
    /// Equip Spell's continuous `SET_CONTROL` and then turned face-down
    /// loses the equip; the reference re-establishes its *current*
    /// controller with a fresh single effect. Without it the monster
    /// snapped back to its owner (fuzz seed 23: a Snatch-Stolen monster
    /// flipped by Book of Moon).
    #[test]
    fn turning_face_down_pins_the_current_controller() {
        let mut f = Field::new(8000);
        let c = monster(&mut f, 1, 0);
        f.cards[c].owner = 0;
        let mut d = Card::with_data(
            CardData {
                code: 45_986_603,
                type_: card_type::SPELL | card_type::EQUIP,
                ..Default::default()
            },
            1,
        );
        d.current.controller = 1;
        d.set_status(status::EFFECT_ENABLED, true);
        let eq = f.new_card(d);
        f.add_card(1, eq, location::SZONE, 0, false);
        f.cards[eq].current.position = position::FACEUP;
        f.cards[eq].equiping_target = Some(c);
        f.cards[c].equiping_cards.push(eq);
        let mut e = Effect::new(effect_type::EQUIP, code::SET_CONTROL);
        e.owner = Some(eq);
        e.handler = Some(eq);
        e.value = 1;
        let steal = f.new_effect(e);
        f.cards[eq].equip_effect.insert(code::SET_CONTROL, steal);
        let steal_id = f.effects.get(steal).unwrap().id.get();
        assert_eq!(f.refresh_control_status(c), (1, Some(steal)));

        f.reset_card(c, crate::field::reset::TURN_SET, crate::field::reset::EVENT);
        let pinned = f.cards[c].single_effect.equal_range(code::SET_CONTROL);
        assert_eq!(pinned.len(), 1, "a fresh single SET_CONTROL");
        let x = f.effects.get(pinned[0]).unwrap();
        assert_eq!(x.value, 1, "the controller it has right now");
        // Newer than the equip's: `add_effect` hands out a fresh id on
        // both engines (the reference's copy of the old id is overwritten
        // at once), and newer is what wins in `refresh_control_status`.
        assert!(x.id.get() > steal_id, "newer than what it replaces");
        assert!(x.is_flag(crate::effect::flag::CANNOT_DISABLE));
        assert_eq!(
            x.reset_flag,
            crate::field::reset::EVENT | Field::TURN_SET_CONTROL_RESETS
        );
        assert_eq!(
            Field::TURN_SET_CONTROL_RESETS,
            0x00ec_0000,
            "the reference's literal"
        );

        // The equip goes; control stays.
        f.cards[c].equiping_cards.clear();
        f.cards[eq].equiping_target = None;
        assert_eq!(f.refresh_control_status(c).0, 1, "still held");
    }

    /// **A plain single control effect is not re-pinned.** The block
    /// fires only when what holds the card is continuous or conditional;
    /// an unconditional single effect — what `set_control` registers —
    /// survives the flip on its own, and a second copy would be noise
    /// that the newest-wins reader then has to sort.
    #[test]
    fn turning_face_down_leaves_a_plain_control_effect_alone() {
        let mut f = Field::new(8000);
        let c = monster(&mut f, 0, 0);
        take(&mut f, &[c], 1);
        assert_eq!(run(&mut f, Some(&[])), Status::End);
        let before = f.cards[c]
            .single_effect
            .equal_range(code::SET_CONTROL)
            .len();
        assert_eq!(before, 1, "the control effect set_control registered");
        f.reset_card(c, crate::field::reset::TURN_SET, crate::field::reset::EVENT);
        assert_eq!(
            f.cards[c]
                .single_effect
                .equal_range(code::SET_CONTROL)
                .len(),
            1,
            "no second copy"
        );
        assert_eq!(f.refresh_control_status(c).0, 1);
    }

    /// **`STATUS_ATTACK_CANCELED`** — a monster that changes hands
    /// mid-battle is no longer attacking.
    #[test]
    fn the_attack_is_cancelled() {
        let mut f = Field::new(8000);
        let c = monster(&mut f, 0, 0);
        take(&mut f, &[c], 1);
        run(&mut f, Some(&[]));
        assert!(f.cards[c].is_status(status::ATTACK_CANCELED));
    }

    /// **Control is *established*, not merely assigned.** `set_control`
    /// registers an `EFFECT_SET_CONTROL` on the card, which is what
    /// `refresh_control_status` later reads — moving the card alone would
    /// leave the board agreeing and the registry silent.
    #[test]
    fn control_is_established_by_an_effect() {
        let mut f = Field::new(8000);
        let c = monster(&mut f, 0, 0);
        assert!(f.cards[c]
            .single_effect
            .equal_range(code::SET_CONTROL)
            .is_empty());
        take(&mut f, &[c], 1);
        run(&mut f, Some(&[]));
        assert!(
            !f.cards[c]
                .single_effect
                .equal_range(code::SET_CONTROL)
                .is_empty(),
            "the change is recorded as an effect"
        );
    }

    /// **Both single events reach the card's own effects.**
    ///
    /// A single event with nothing listening leaves no trace, so the card is
    /// given a trigger for each. The `EVENT_MOVE` one fires **twice** — once
    /// for the placement `move_to_field` announces and once for this
    /// machine's own — which is what makes the second raise observable at
    /// all; counting is the assertion, not presence.
    ///
    /// This machine opens no window, so the chains wait on `new_ochain`.
    #[test]
    fn both_single_events_reach_the_cards_own_effects() {
        let mut f = Field::new(8000);
        let c = monster(&mut f, 0, 0);
        let mut listeners = Vec::new();
        for want in [code::CONTROL_CHANGED, code::MOVE] {
            let mut e = Effect::new(
                effect_type::SINGLE | effect_type::ACTIONS | effect_type::TRIGGER_O,
                want,
            );
            e.owner = Some(c);
            e.handler = Some(c);
            e.range = u16::from(location::MZONE);
            let id = f.new_effect(e);
            f.cards[c].single_effect.insert(want, id);
            f.cards[c].indexer.insert(id);
            listeners.push(id);
        }

        take(&mut f, &[c], 1);
        run(&mut f, Some(&[]));
        let count = |id: EffectId| {
            f.core
                .new_ochain
                .iter()
                .filter(|ch| ch.triggering_effect == id)
                .count()
        };
        assert_eq!(
            count(listeners[0]),
            1,
            "the control change is announced once"
        );
        assert_eq!(
            count(listeners[1]),
            2,
            "and the move twice — the placement's and this machine's"
        );
    }

    /// **A card that has left the field by the time the events are raised is
    /// dropped**, rather than announced as having changed hands.
    ///
    /// Driven by hand to the step that raises them, because what puts a card
    /// there is something else destroying it mid-move — and the cheapest
    /// honest way to be at that point is to be at that point.
    #[test]
    fn a_card_that_left_the_field_is_not_announced() {
        let mut f = Field::new(8000);
        let a = monster(&mut f, 0, 0);
        let b = monster(&mut f, 0, 1);
        take(&mut f, &[a, b], 1);
        for _ in 0..2048 {
            let at_raise = f
                .queue()
                .next()
                .is_some_and(|u| matches!(u.kind, Kind::GetControl { .. }) && u.step == 5);
            if at_raise {
                // `a` is taken off the field between the move and the
                // announcement.
                f.remove_card(a);
                f.add_card(1, a, location::GRAVE, 0, false);
                break;
            }
            match f.process() {
                Status::Continue => continue,
                Status::Awaiting => match f.messages.last() {
                    Some(Message::SelectPlace { player, .. }) => {
                        let p = *player as usize;
                        let seat = (0..5)
                            .find(|&i| f.players[p].mzone[i].is_none())
                            .unwrap_or(0) as i8;
                        f.core.returns.set_i8(0, i8::from(p != 0));
                        f.core.returns.set_i8(1, location::MZONE as i8);
                        f.core.returns.set_i8(2, seat);
                    }
                    _ => break,
                },
                _ => break,
            }
        }
        run(&mut f, Some(&[]));
        assert_eq!(f.core.operated_set, vec![b], "only the one still there");
        assert_eq!(f.core.returns.get(), 1);
    }

    /// `RESET_CONTROL` takes the effects the card held by virtue of being on
    /// that side.
    #[test]
    fn the_control_scoped_effects_are_reset() {
        let mut f = Field::new(8000);
        let c = monster(&mut f, 0, 0);
        let mut e = Effect::new(effect_type::SINGLE, code::UPDATE_ATTACK);
        e.owner = Some(c);
        e.handler = Some(c);
        e.reset_flag = crate::field::reset::EVENT + crate::field::reset::CONTROL;
        let id = f.new_effect(e);
        f.cards[c].single_effect.insert(code::UPDATE_ATTACK, id);
        f.cards[c].indexer.insert(id);

        take(&mut f, &[c], 1);
        run(&mut f, Some(&[]));
        assert!(!f.cards[c].indexer.contains(&id));
    }

    /// Both events are raised, over the survivors.
    #[test]
    fn the_change_is_announced() {
        let mut f = Field::new(8000);
        let c = monster(&mut f, 0, 0);
        take(&mut f, &[c], 1);
        run(&mut f, Some(&[]));
        for want in [code::CONTROL_CHANGED, code::MOVE] {
            assert!(
                f.core
                    .instant_event
                    .iter()
                    .chain(f.core.used_event.iter())
                    .any(|e| e.event_code == want),
                "event {want}"
            );
        }
    }

    mod the_filter {
        use super::*;

        /// Six reasons a target is dropped; each on its own.
        #[test]
        fn a_card_already_on_that_side_is_dropped() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 1, 0);
            take(&mut f, &[c], 1);
            assert_eq!(run(&mut f, Some(&[])), Status::End);
            assert_eq!(f.core.returns.get(), 0, "nothing changed hands");
        }

        #[test]
        fn a_card_off_the_monster_zones_is_dropped() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            f.remove_card(c);
            f.add_card(0, c, location::GRAVE, 0, false);
            take(&mut f, &[c], 1);
            run(&mut f, Some(&[]));
            assert_eq!(f.core.returns.get(), 0);
            assert_eq!(f.cards[c].current.location, location::GRAVE);
        }

        /// `EFFECT_CANNOT_CHANGE_CONTROL` refuses.
        #[test]
        fn a_card_that_cannot_change_hands_is_dropped() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            single(&mut f, c, code::CANNOT_CHANGE_CONTROL);
            take(&mut f, &[c], 1);
            run(&mut f, Some(&[]));
            assert_eq!(f.core.returns.get(), 0);
            assert_eq!(f.cards[c].current.controller, 0);
        }

        /// A card the reason effect may not touch is dropped — and with no
        /// reason effect the question is not asked at all.
        #[test]
        fn a_card_the_effect_cannot_touch_is_dropped() {
            let mut f = Field::new(8000);
            let source = monster(&mut f, 1, 0);
            let mut e = Effect::new(effect_type::SINGLE, 0);
            e.owner = Some(source);
            e.handler = Some(source);
            let by = f.new_effect(e);

            let c = monster(&mut f, 0, 0);
            // An immunity lives in its own container and must answer with a
            // non-zero value; `single_effect` is not where it is looked for.
            let mut imm = Effect::new(effect_type::SINGLE, code::IMMUNE_EFFECT);
            imm.owner = Some(c);
            imm.handler = Some(c);
            imm.value = 1;
            let imm = f.new_effect(imm);
            f.cards[c].immune_effect.push(imm);
            f.cards[c].indexer.insert(imm);

            f.get_control(vec![c], Some(by), PLAYER_NONE, 1, 0, 0, 0xff);
            run(&mut f, Some(&[]));
            assert_eq!(f.core.returns.get(), 0, "immune to the effect that asked");
        }
    }

    mod room {
        use super::*;

        /// **No room at all destroys every target**, with no question asked.
        #[test]
        fn no_room_destroys_them_all() {
            let mut f = Field::new(8000);
            let a = monster(&mut f, 0, 0);
            let b = monster(&mut f, 0, 1);
            fill_mzone(&mut f, 1, 5);
            take(&mut f, &[a, b], 1);
            assert_eq!(run(&mut f, Some(&[])), Status::End);
            assert_eq!(f.cards[a].current.location, location::GRAVE);
            assert_eq!(f.cards[b].current.location, location::GRAVE);
            assert_eq!(f.core.returns.get(), 0);
            assert!(
                !f.messages
                    .iter()
                    .any(|m| matches!(m, Message::SelectCard { .. })),
                "nothing to choose between"
            );
        }

        /// **Not enough room asks the receiving player which to give up, and
        /// the ones chosen are destroyed** rather than left behind.
        #[test]
        fn not_enough_room_destroys_the_surplus() {
            let mut f = Field::new(8000);
            let a = monster(&mut f, 0, 0);
            let b = monster(&mut f, 0, 1);
            let c = monster(&mut f, 0, 2);
            fill_mzone(&mut f, 1, 3);
            take(&mut f, &[a, b, c], 1);
            assert_eq!(run(&mut f, None), Status::Awaiting, "which to give up?");
            let offered = f.core.select_cards.clone();
            assert_eq!(offered.len(), 3);

            run(&mut f, Some(&[0]));
            assert_eq!(f.cards[offered[0]].current.location, location::GRAVE);
            assert_eq!(f.cards[offered[1]].current.controller, 1);
            assert_eq!(f.cards[offered[2]].current.controller, 1);
            assert_eq!(f.core.returns.get(), 2, "two changed hands");
        }

        /// Exactly enough room asks nothing.
        #[test]
        fn exactly_enough_room_asks_nothing() {
            let mut f = Field::new(8000);
            let a = monster(&mut f, 0, 0);
            let b = monster(&mut f, 0, 1);
            fill_mzone(&mut f, 1, 3);
            take(&mut f, &[a, b], 1);
            assert_eq!(run(&mut f, Some(&[])), Status::End);
            assert_eq!(f.core.returns.get(), 2);
            assert!(!f.messages.iter().any(|m| matches!(
                m,
                Message::Hint {
                    kind: crate::host_question::hint::SELECTMSG,
                    value: 502,
                    ..
                }
            )));
        }
    }

    mod uniqueness {
        use super::*;

        /// **A uniqueness limit is taken out of the registry before the move
        /// and put back after**, because the registry is keyed by controller.
        #[test]
        fn the_registry_follows_the_card() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            f.cards[c].unique_code = 18036057;
            f.cards[c].unique_location = u16::from(location::MZONE);
            f.cards[c].unique_pos = [1, 0];
            f.cards[c].unique_fieldid = 7;
            let mut e = Effect::new(effect_type::SINGLE, 0);
            e.owner = Some(c);
            e.handler = Some(c);
            let e = f.new_effect(e);
            f.cards[c].unique_effect = Some(e);
            f.core.unique_cards[0].push(c);

            take(&mut f, &[c], 1);
            run(&mut f, Some(&[]));
            assert_eq!(f.cards[c].current.controller, 1);
            assert!(
                f.core.unique_cards[1].contains(&c),
                "registered on its new side"
            );
            assert!(
                !f.core.unique_cards[0].contains(&c),
                "and not on the old one"
            );
            assert_eq!(
                f.cards[c].unique_fieldid, 0,
                "and its answer is forgotten, because it has just arrived"
            );
        }
    }

    mod is_control_can_be_changed {
        use super::*;
        use crate::card::CardData;

        fn spare_monster(f: &mut Field, player: u8, seat: u32) -> CardId {
            let mut c = Card::with_data(
                CardData {
                    code: 30_000 + seat + u32::from(player) * 10,
                    type_: card_type::MONSTER,
                    ..Default::default()
                },
                player,
            );
            c.current.controller = player;
            c.set_status(status::EFFECT_ENABLED, true);
            let id = f.new_card(c);
            f.add_card(player, id, location::MZONE, seat, false);
            f.cards[id].current.position = position::FACEUP_ATTACK;
            id
        }

        /// **A card off the Monster Zone cannot change hands**, and
        /// neither can one that is nowhere at all.
        #[test]
        fn it_wants_a_monster_on_the_field() {
            let mut f = Field::new(8000);
            let c = spare_monster(&mut f, 0, 0);
            assert!(f.is_control_can_be_changed(c, false, 0xff), "the sibling");
            f.move_card(0, c, location::GRAVE, 0, false);
            assert!(!f.is_control_can_be_changed(c, false, 0xff), "buried");
            let mut f = Field::new(8000);
            let c = spare_monster(&mut f, 0, 0);
            f.remove_card(c);
            assert!(!f.is_control_can_be_changed(c, false, 0xff), "nowhere");
        }

        /// **The room is counted on the receiving side**, and
        /// `ignore_mzone` waives it.
        ///
        /// The two halves matter separately: with the *other* row full
        /// the answer is no, and with this row full it is still yes —
        /// which is the test that tells the two players apart.
        #[test]
        fn it_wants_room_on_the_other_side_unless_told_to_ignore_it() {
            let mut f = Field::new(8000);
            let c = spare_monster(&mut f, 0, 0);
            for seat in 0..5 {
                spare_monster(&mut f, 1, seat);
            }
            assert!(
                !f.is_control_can_be_changed(c, false, 0xff),
                "their row is full"
            );
            assert!(
                f.is_control_can_be_changed(c, true, 0xff),
                "unless the caller says it will make room"
            );

            // The mirror: this player's own row full changes nothing.
            let mut f = Field::new(8000);
            let c = spare_monster(&mut f, 0, 0);
            for seat in 1..5 {
                spare_monster(&mut f, 0, seat);
            }
            assert!(
                f.is_control_can_be_changed(c, false, 0xff),
                "its own row is not the one counted"
            );
        }

        /// **And it still asks about the lock**, which is the one thing
        /// `is_capable_change_control` answers on its own.
        #[test]
        fn it_still_asks_about_the_control_lock() {
            let mut f = Field::new(8000);
            let c = spare_monster(&mut f, 0, 0);
            assert!(f.is_control_can_be_changed(c, false, 0xff));
            single(&mut f, c, code::CANNOT_CHANGE_CONTROL);
            assert!(!f.is_control_can_be_changed(c, false, 0xff));
            assert!(
                !f.is_control_can_be_changed(c, true, 0xff),
                "and ignoring the zone does not waive it"
            );
        }
    }
}

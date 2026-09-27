//! Turning a card over, or sideways: `change_position` and `ChangePos`.
//!
//! Six steps, and the shape is unusual: **steps 0-3 run twice.** Step 3
//! flips an `oppo_selection` flag and jumps back to 0, so the whole filter
//! and the trap-monster question are asked once for each player. That is not
//! an optimisation of a loop — the two passes ask *different players* the
//! same question, and a batch can contain both players' cards.
//!
//! ## `position_param` carries the target position and a flag word
//!
//! The low byte is the position to move to; the high half is a flag the
//! flip event carries. `change_position` over a set of cards writes a
//! *different* target per card, chosen by where each one currently is —
//! which is how "change every monster's position" means face-up attack for
//! the face-down ones and face-up defence for the attacking ones in a single
//! call.
//!
//! ## A trap monster turned face-down goes back to its spell/trap seat
//!
//! Which is why the machine needs a card selection at all. If there is no
//! room in the spell/trap row the trap monster is sent to the graveyard
//! instead, and if there is *partial* room the player chooses which of them
//! survive — a question asked with `SelectCard` over the surplus.

use crate::board::{location, position};
use crate::card::{card_type, reason, status};
use crate::event::{code, CardId, EffectId, PLAYER_NONE};
use crate::field::{timing, Field, GroupId, Message};
use crate::host_question::hint;
use crate::processor::Kind;
use std::collections::BTreeSet;

/// The unit state `ChangePos` carries across its two passes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChangePosState {
    /// Trap monsters with nowhere to go, collected across both passes and
    /// sent to the graveyard at the end.
    pub to_grave: BTreeSet<CardId>,
    /// False on the first pass, true on the second. The reference uses it
    /// both to pick which player is asked and as the "have I already looped"
    /// guard, which is why one flag is enough.
    pub oppo_selection: bool,
}

impl Field {
    /// `field::change_position` over a set — each card gets its own target.
    ///
    /// The four arguments are the four positions a card might currently be
    /// in, and the target is chosen by that. A caller saying
    /// "(`au`, `ad`, `du`, `dd`)" is writing a small transition table, not
    /// four alternatives.
    #[allow(clippy::too_many_arguments)]
    pub fn change_position(
        &mut self,
        targets: impl IntoIterator<Item = CardId>,
        reason_effect: Option<EffectId>,
        reason_player: u8,
        au: u8,
        ad: u8,
        du: u8,
        dd: u8,
        flag: u32,
        enable: bool,
    ) {
        let cards: Vec<CardId> = targets.into_iter().collect();
        for &card in &cards {
            let npos = match self.cards[card].current.position {
                position::FACEUP_ATTACK => au,
                position::FACEDOWN_DEFENSE => dd,
                position::FACEUP_DEFENSE => du,
                _ => ad,
            };
            self.cards[card].position_param = u32::from(npos) | flag;
        }
        let group = self.new_group(cards);
        self.emplace(Kind::ChangePos {
            targets: group,
            reason_effect,
            reason_player,
            enable,
            state: Box::default(),
        });
    }

    /// The single-card arity, where the caller already knows the target.
    pub fn change_position_card(
        &mut self,
        target: CardId,
        reason_effect: Option<EffectId>,
        reason_player: u8,
        npos: u8,
        flag: u32,
        enable: bool,
    ) {
        self.cards[target].position_param = u32::from(npos) | flag;
        let group = self.new_group([target]);
        self.emplace(Kind::ChangePos {
            targets: group,
            reason_effect,
            reason_player,
            enable,
            state: Box::default(),
        });
    }

    /// `card::is_capable_turn_set` — may this card be turned face-down?
    ///
    /// Four refusals. The first two are about what the card *is* — a token
    /// has no face-down state and a Link monster has no defence position —
    /// and the last two are the prohibition asked of the card and of the
    /// player separately.
    pub fn is_capable_turn_set(&mut self, card: CardId, playerid: u8) -> bool {
        let data = &self.cards[card].data;
        if data.is_type(card_type::TOKEN)
            || (data.is_type(card_type::LINK) && data.is_type(card_type::MONSTER))
        {
            return false;
        }
        if self.cards[card].current.is_position(position::FACEDOWN) {
            return false;
        }
        if self
            .is_affected_by_effect(card, code::CANNOT_TURN_SET)
            .is_some()
        {
            return false;
        }
        self.is_player_affected_by_effect(playerid, code::CANNOT_TURN_SET)
            .is_none()
    }

    /// `field::refresh_location_info_instant` — recompute which seats are
    /// disabled, from scratch.
    ///
    /// Rebuilt rather than adjusted: both players' masks are cleared first
    /// and every `EFFECT_DISABLE_FIELD` re-applied. An effect that stopped
    /// applying therefore stops disabling, with no bookkeeping.
    ///
    /// Three sources, and they are packed differently from each other:
    ///
    /// - `EFFECT_DISABLE_FIELD` — one value covering **both** players, the
    ///   low half for player 0 and the high half for player 1, each masked
    ///   `0xff7f`. The gap at bit 7 is the seat that does not exist.
    /// - `EFFECT_USE_EXTRA_MZONE` — the *handler's* player only, taking the
    ///   value's high half down to five bits.
    /// - `EFFECT_USE_EXTRA_SZONE` — likewise, but shifted into the
    ///   spell/trap byte.
    ///
    /// The last two lines are the subtle part: **the two Extra Monster Zones
    /// are shared**, so each player's mask inherits the other's bits 5 and 6
    /// — crossed over, because a zone that is player 0's seat 5 is player 1's
    /// seat 6.
    pub fn refresh_location_info_instant(&mut self) {
        self.players[0].disabled_location = 0;
        self.players[1].disabled_location = 0;

        for e in self.filter_field_effect(code::DISABLE_FIELD) {
            let value = self.effect_plain_value(e) as u32;
            self.players[0].disabled_location |= value & 0xff7f;
            self.players[1].disabled_location |= (value >> 16) & 0xff7f;
        }
        for (code_, shift, mask) in [
            (code::USE_EXTRA_MZONE, 16u32, 0x1fu32),
            (code::USE_EXTRA_SZONE, 8, 0x1f00),
        ] {
            for e in self.filter_field_effect(code_) {
                let p = self
                    .effects
                    .get(e)
                    .map_or(PLAYER_NONE, |x| x.get_handler_player(&self.cards));
                if p > 1 {
                    continue;
                }
                let value = self.effect_plain_value(e) as u32;
                self.players[p as usize].disabled_location |= (value >> shift) & mask;
            }
        }
        // The two Extra Monster Zones are shared, and crossed: one player's
        // seat 5 is the other's seat 6.
        let (a, b) = (
            self.players[0].disabled_location,
            self.players[1].disabled_location,
        );
        self.players[0].disabled_location |= (((b >> 5) & 1) << 6) | (((b >> 6) & 1) << 5);
        self.players[1].disabled_location |= (((a >> 5) & 1) << 6) | (((a >> 6) & 1) << 5);
    }

    /// An effect's value asked with no card and no arguments.
    pub(crate) fn effect_plain_value(&self, effect: EffectId) -> i64 {
        let Some(e) = self.effects.get(effect) else {
            return 0;
        };
        let ev = crate::event::Event::new(0);
        let ctx = crate::effect::Ctx {
            reason_effect: effect,
            player: PLAYER_NONE,
            event: &ev,
            card: None,
            args: &[],
        };
        e.get_value(self, &ctx)
    }
}

impl Field {
    /// One step of `ChangePos`.
    pub(crate) fn change_pos_step(
        &mut self,
        step: u16,
        targets: GroupId,
        reason_effect: Option<EffectId>,
        reason_player: u8,
        enable: bool,
        state: &mut ChangePosState,
    ) -> bool {
        match step {
            0 => self.change_pos_step_0(targets, reason_effect, reason_player),
            1 => self.change_pos_step_1(targets, reason_player, state),
            2 => {
                for card in std::mem::take(&mut self.core.return_cards.list) {
                    state.to_grave.insert(card);
                    self.group_mut(targets).remove(&card);
                }
                false
            }
            3 => {
                // The first pass asked one player; go back and ask the
                // other. One flag serves as both "which player" and "have I
                // already looped".
                if !state.oppo_selection {
                    state.oppo_selection = true;
                    self.set_step(crate::processor::RESTART);
                }
                false
            }
            4 => self.change_pos_step_4(targets, reason_effect, reason_player, enable, state),
            5 => {
                self.core.operated_set = self.group(targets).iter().copied().collect();
                self.core.returns.set(self.core.operated_set.len() as i32);
                true
            }
            _ => true,
        }
    }

    /// Step 0: drop the cards that cannot change position.
    ///
    /// Six refusals, and two are worth stating:
    ///
    /// - **The position is already what is being asked for.** Not an error,
    ///   just nothing to do — and the card leaves the batch, so it is not
    ///   counted in what the operation reports.
    /// - **Turning face-up to face-down asks `is_capable_turn_set`**, but
    ///   only for a non-token. A token turned face-down is handled later, at
    ///   step 4, by forcing it face-up instead.
    ///
    /// `EFFECT_CANNOT_CHANGE_POS_E` is then asked *only when an effect is
    /// behind the change* — a rules change of position cannot be forbidden
    /// by a clause about effects. Its value names the positions it forbids,
    /// and **a value of zero forbids both**: an effect that declines to be
    /// specific forbids everything, which is the opposite of the natural
    /// reading.
    fn change_pos_step_0(
        &mut self,
        targets: GroupId,
        reason_effect: Option<EffectId>,
        reason_player: u8,
    ) -> bool {
        for card in self.group(targets).iter().copied().collect::<Vec<_>>() {
            let npos = (self.cards[card].position_param & 0xff) as u8;
            let opos = self.cards[card].current.position;
            let loc = self.cards[card].current.location;
            let data_type = self.cards[card].data.type_;

            let refused = loc != location::MZONE && loc != location::SZONE
                || (data_type & card_type::LINK != 0 && data_type & card_type::MONSTER != 0)
                || self.cards[card].get_status(status::SUMMONING | status::SPSUMMON_STEP)
                || (reason_effect.is_some() && !self.is_affect_by_effect(card, reason_effect))
                || npos == opos
                || (data_type & card_type::TOKEN == 0
                    && opos & position::FACEUP != 0
                    && npos & position::FACEDOWN != 0
                    && !self.is_capable_turn_set(card, reason_player));
            if refused {
                self.group_mut(targets).remove(&card);
                continue;
            }

            let Some(by) = reason_effect else { continue };
            let forbidding = self.filter_effect(card, code::CANNOT_CHANGE_POS_E);
            if forbidding.is_empty() {
                continue;
            }
            let mut disallowed = 0u8;
            for e in forbidding {
                let val = self.effect_value_for_card_pub(e, card) as u8;
                // Zero means "all of them", not "none".
                disallowed |= if val != 0 {
                    val
                } else {
                    position::FACEUP | position::FACEDOWN
                };
            }
            let _ = by;
            if npos & disallowed != 0 {
                self.group_mut(targets).remove(&card);
            }
        }
        false
    }

    /// Step 1: find room for the trap monsters that are being turned down.
    ///
    /// A trap monster turned face-down leaves the Monster Zone and wants its
    /// spell/trap seat back. Three outcomes:
    ///
    /// - **No room at all**: every one of them is sent to the graveyard, and
    ///   the machine skips the selection.
    /// - **Room for all**: nothing to decide.
    /// - **Partial room**: the player chooses which `size - room` of them to
    ///   give up, via `SelectCard`.
    ///
    /// Only *this pass's* player's trap monsters are considered, which is
    /// what the two passes are for.
    fn change_pos_step_1(
        &mut self,
        targets: GroupId,
        reason_player: u8,
        state: &mut ChangePosState,
    ) -> bool {
        // `1 - reason_player` on a `uint8_t`, and it **wraps**: `Adjust`
        // emplaces this unit with `PLAYER_NONE` (2), so the opponent pass
        // computes 255 and matches nobody. That is the reference's
        // behaviour, not an accident to be corrected — a rule-driven
        // position change has no player whose trap monsters need seats, and
        // the wrap is how it says so.
        let playerid = if state.oppo_selection {
            1u8.wrapping_sub(reason_player)
        } else {
            reason_player
        };
        let mut ssets: Vec<CardId> = Vec::new();
        for card in self.group(targets).iter().copied().collect::<Vec<_>>() {
            let npos = (self.cards[card].position_param & 0xff) as u8;
            let opos = self.cards[card].current.position;
            if opos & position::FACEUP != 0 && npos & position::FACEDOWN != 0 {
                let is_trap_monster =
                    self.get_type(card, None, 0, playerid) & card_type::TRAPMONSTER != 0;
                if is_trap_monster && self.cards[card].current.controller == playerid {
                    ssets.push(card);
                }
            }
        }
        if ssets.is_empty() {
            // `arg.step = 2` in the reference's `else` branch, which by the
            // increment rule means **step 3 runs next** — case 2 is skipped.
            // That matters: case 2 drains `return_cards`, and with no trap
            // monster there was no selection to drain. Falling into it
            // instead consumes whatever the *previous* question left in the
            // buffer and sends those cards to the graveyard.
            self.set_step(2);
            return false;
        }
        self.core.return_cards.clear();
        self.refresh_location_info_instant();
        // `get_useable_count(nullptr, ...)`: the question is about the row,
        // not about any one of the trap monsters going into it.
        let room = self.get_useable_count(None, playerid, location::SZONE, playerid, 0, 0xff);
        if room <= 0 {
            for card in ssets {
                state.to_grave.insert(card);
                self.group_mut(targets).remove(&card);
            }
            // `arg.step = 2` again. Unlike the branch above this one is
            // not *observable*: `return_cards` was cleared a few lines up,
            // so draining it in case 2 would be a no-op either way. It is
            // written the reference's way regardless — the two stop being
            // the same the moment anything is added between the clear and
            // this test.
            self.set_step(2);
            return false;
        }
        if ssets.len() as i32 > room {
            let surplus = (ssets.len() as i32 - room) as u8;
            self.core.select_cards = ssets;
            self.messages.push(Message::Hint {
                kind: hint::SELECTMSG,
                player: playerid,
                value: 502,
            });
            self.emplace(Kind::SelectCard {
                player: playerid,
                cancelable: false,
                min: surplus,
                max: surplus,
            });
        }
        false
    }
}

impl Field {
    /// Step 4: actually turn the cards, and deal with what that means.
    ///
    /// Processed in `card_operation_sort` order, and each card's old
    /// position is recorded in `previous.position` before the new one is
    /// written — a flip effect asks what it was.
    ///
    /// **A token turned face-down is turned face-up defence instead.** There
    /// is no face-down token; step 0 deliberately let it through so this
    /// could substitute rather than refuse.
    ///
    /// Three things a *flip face-up* does that a turn face-down does not:
    ///
    /// - a **new `fieldid`**, because it is newly visible and ordering
    ///   questions treat it as having just arrived;
    /// - `EVENT_FLIP`, but **only in the Monster Zone** — a spell turned
    ///   face-up is not flipped in the rules' sense;
    /// - enabling its field effects — *unless* the change came from an
    ///   action effect on a Monster Zone card, in which case it is deferred
    ///   to `delayed_enable_set`. That deferral is what stops a monster
    ///   flipped by an effect applying its own effects mid-resolution.
    ///
    /// And what a *turn face-down* does: clears a negated summon's status,
    /// resets `RESET_TURN_SET`, cuts its targets, marks it set this turn,
    /// disables its field effects, **erases `previous.location`**, and
    /// clears its summon type and special-summon counters. The card is being
    /// treated as though it had never been face-up.
    fn change_pos_step_4(
        &mut self,
        targets: GroupId,
        reason_effect: Option<EffectId>,
        reason_player: u8,
        enable: bool,
        state: &mut ChangePosState,
    ) -> bool {
        let mut ordered: Vec<CardId> = self.group(targets).iter().copied().collect();
        if ordered.len() > 1 {
            ordered.sort_by(|&a, &b| {
                if self.card_operation_sort(a, b) {
                    std::cmp::Ordering::Less
                } else if self.card_operation_sort(b, a) {
                    std::cmp::Ordering::Greater
                } else {
                    std::cmp::Ordering::Equal
                }
            });
        }
        let mut equipings: Vec<CardId> = Vec::new();
        let mut flips: Vec<CardId> = Vec::new();
        let mut ssets: Vec<CardId> = Vec::new();
        let mut pos_changed: Vec<CardId> = Vec::new();

        for card in ordered {
            let mut npos = (self.cards[card].position_param & 0xff) as u8;
            let opos = self.cards[card].current.position;
            let flag = self.cards[card].position_param >> 16;
            // There is no face-down token.
            if self.cards[card].data.is_type(card_type::TOKEN) && npos & position::FACEDOWN != 0 {
                npos = position::FACEUP_DEFENSE;
            }
            self.cards[card].previous.position = opos;
            self.cards[card].current.position = npos;
            if npos & position::DEFENSE != 0
                && self
                    .is_affected_by_effect(card, code::DEFENSE_ATTACK)
                    .is_none()
            {
                self.cards[card].set_status(status::ATTACK_CANCELED, true);
            }
            self.cards[card].set_status(status::JUST_POS, true);
            let controller = self.cards[card].current.controller;
            self.core.hint_timing[controller as usize] |= timing::POS_CHANGE;

            if opos & position::FACEDOWN != 0 && npos & position::FACEUP != 0 {
                let fid = self.next_field_id_raw();
                self.cards[card].fieldid = fid;
                let loc = self.cards[card].current.location;
                if self
                    .check_unique_onfield(card, controller, u16::from(loc), None)
                    .is_some()
                {
                    self.cards[card].unique_fieldid = u32::MAX;
                }
                if loc == location::MZONE {
                    self.raise_single_event(
                        card,
                        vec![],
                        code::FLIP,
                        reason_effect,
                        0,
                        reason_player,
                        0,
                        flag,
                    );
                    flips.push(card);
                }
                if enable {
                    // An action effect flipping a monster defers the enable,
                    // so the monster cannot apply its own effects while the
                    // effect that flipped it is still resolving.
                    let from_action_effect = reason_effect.is_some_and(|e| {
                        self.effects
                            .get(e)
                            .is_some_and(|x| x.effect_type & 0x7f0 != 0)
                    });
                    if !from_action_effect || loc != location::MZONE {
                        self.enable_field_effect(card, true);
                    } else {
                        self.core.delayed_enable_set.insert(card);
                    }
                } else {
                    self.refresh_disable_status(card);
                }
            }
            if self.cards[card].current.location == location::MZONE {
                self.raise_single_event(
                    card,
                    vec![],
                    code::CHANGE_POS,
                    reason_effect,
                    0,
                    reason_player,
                    0,
                    0,
                );
                pos_changed.push(card);
            }

            let mut trapmonster = false;
            if opos & position::FACEUP != 0 && npos & position::FACEDOWN != 0 {
                if self.get_type(card, None, 0, reason_player) & card_type::TRAPMONSTER != 0 {
                    trapmonster = true;
                }
                if self.cards[card].get_status(status::SUMMON_DISABLED | status::ACTIVATE_DISABLED)
                {
                    self.cards[card]
                        .set_status(status::SUMMON_DISABLED | status::ACTIVATE_DISABLED, false);
                }
                self.reset_card(
                    card,
                    crate::field::reset::TURN_SET,
                    crate::field::reset::EVENT,
                );
                self.clear_card_target(card);
                self.cards[card].set_status(status::SET_TURN, true);
                self.enable_field_effect(card, false);
                // Treated as never having been face-up.
                self.cards[card].previous.location = 0;
            }
            if npos & position::FACEDOWN != 0 && !self.cards[card].equiping_cards.is_empty() {
                for equipper in self.cards[card].equiping_cards.clone() {
                    equipings.push(equipper);
                    self.unequip(equipper);
                }
            }
            if npos & position::FACEDOWN != 0 && self.cards[card].equiping_target.is_some() {
                self.unequip(card);
            }
            if trapmonster {
                self.refresh_location_info_instant();
                let controller = self.cards[card].current.controller;
                self.move_to_field(
                    card,
                    controller,
                    controller,
                    u16::from(location::SZONE),
                    position::FACEDOWN,
                    false,
                    2,
                    0xff,
                    false,
                    0,
                    // `confirm`: the reference's default, and it is `true`.
                    true,
                );
                self.raise_single_event(
                    card,
                    vec![],
                    code::SSET,
                    reason_effect,
                    0,
                    reason_player,
                    0,
                    0,
                );
                ssets.push(card);
            }
        }

        self.adjust_instant();
        self.process_single_event();
        for (cards, ev) in [
            (flips, code::FLIP),
            (ssets, code::SSET),
            (pos_changed, code::CHANGE_POS),
        ] {
            if !cards.is_empty() {
                self.raise_event_over(cards, ev, reason_effect, 0, reason_player, 0, 0);
            }
        }
        self.process_instant_event();
        if !equipings.is_empty() {
            self.destroy(
                equipings,
                None,
                reason::LOST_TARGET + reason::RULE,
                PLAYER_NONE,
                PLAYER_NONE,
                0,
                0,
            );
        }
        let to_grave: Vec<CardId> = std::mem::take(&mut state.to_grave).into_iter().collect();
        if !to_grave.is_empty() {
            self.send_to(
                to_grave,
                None,
                reason::RULE,
                PLAYER_NONE,
                PLAYER_NONE,
                u16::from(location::GRAVE),
                0,
                position::FACEUP,
                false,
            );
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{Card, CardData};
    use crate::processor::Status;

    fn monster(f: &mut Field, player: u8, pos: u8) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 18036057,
                type_: card_type::MONSTER,
                ..Default::default()
            },
            player,
        );
        c.current.controller = player;
        c.current.position = pos;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        let seat = f.cards.len() as u32 - 1;
        f.add_card(player, id, location::MZONE, seat, false);
        id
    }

    fn run(f: &mut Field) -> Status {
        for _ in 0..256 {
            match f.process() {
                Status::Continue => continue,
                other => return other,
            }
        }
        panic!("did not settle");
    }

    fn raised(f: &Field, ev: u32) -> bool {
        f.core.instant_event.iter().any(|e| e.event_code == ev)
    }

    /// **A position change does not read the previous question's
    /// answer.** Case 2 of the unit drains `return_cards` into the
    /// to-graveyard set, and it is reached *only* when case 1 asked which
    /// trap monsters to keep. With no trap monster among the targets the
    /// reference skips case 2 (`arg.step = 2`, so step 3 runs next);
    /// running it anyway consumes whatever the last question left behind —
    /// an attack target, a tribute choice — and sends those cards to the
    /// graveyard by rule.
    ///
    /// The bystander here stands in for that: it is in the buffer, it is
    /// not a target of the position change, and it must not move.
    #[test]
    fn a_stale_selection_is_not_drained_into_the_graveyard() {
        let mut f = Field::new(8000);
        let turning = monster(&mut f, 0, position::FACEUP_ATTACK);
        let bystander = monster(&mut f, 0, position::FACEUP_ATTACK);
        // What the previous question left in the buffer.
        f.core.return_cards.list = vec![bystander];
        f.change_position(
            [turning],
            None,
            0,
            position::FACEUP_DEFENSE,
            position::FACEUP_ATTACK,
            position::FACEUP_ATTACK,
            position::FACEUP_ATTACK,
            0,
            true,
        );
        run(&mut f);
        assert_eq!(
            f.cards[turning].current.position,
            position::FACEUP_DEFENSE,
            "the target still turns"
        );
        assert_eq!(
            f.cards[bystander].current.location,
            location::MZONE,
            "the stale selection is left alone"
        );
        assert_eq!(
            f.cards[bystander].reason & crate::card::reason::RULE,
            0,
            "and nothing was done to it by rule"
        );
    }

    /// **The no-room branch sends the trap monster away on its own.** A
    /// trap monster turned face-down needs a Spell/Trap seat to go back
    /// to; with none, it goes to the graveyard there and then.
    ///
    /// This does *not* discriminate that branch's step number the way the
    /// test above does, and saying so is the point: `return_cards` is
    /// cleared a few lines before the branch is reached, so running case 2
    /// there drains an empty list. The step is written the reference's way
    /// for faithfulness, not because this test could tell.
    #[test]
    fn a_trap_monster_with_no_seat_goes_to_the_graveyard_alone() {
        let mut f = Field::new(8000);
        let mut c = Card::with_data(
            CardData {
                code: 4206964,
                type_: card_type::TRAP | card_type::MONSTER | card_type::TRAPMONSTER,
                ..Default::default()
            },
            0,
        );
        c.current.controller = 0;
        c.current.position = position::FACEUP_ATTACK;
        c.set_status(status::EFFECT_ENABLED, true);
        let trap_monster = f.new_card(c);
        f.add_card(0, trap_monster, location::MZONE, 0, false);
        // Fill the Spell/Trap row so there is no seat to return to.
        for seat in 0..5u32 {
            let mut c = Card::with_data(
                CardData {
                    code: 5555,
                    type_: card_type::SPELL | card_type::CONTINUOUS,
                    ..Default::default()
                },
                0,
            );
            c.current.controller = 0;
            let id = f.new_card(c);
            f.add_card(0, id, location::SZONE, seat, false);
            f.cards[id].current.position = position::FACEUP;
        }
        let bystander = monster(&mut f, 1, position::FACEUP_ATTACK);
        f.core.return_cards.list = vec![bystander];
        f.change_position(
            [trap_monster],
            None,
            0,
            position::FACEDOWN_DEFENSE,
            position::FACEDOWN_DEFENSE,
            position::FACEDOWN_DEFENSE,
            position::FACEDOWN_DEFENSE,
            0,
            true,
        );
        run(&mut f);
        assert_eq!(
            f.cards[trap_monster].current.location,
            location::GRAVE,
            "no seat, so it leaves"
        );
        assert_eq!(
            f.cards[bystander].current.location,
            location::MZONE,
            "the stale selection is still left alone"
        );
    }

    /// The transition table: one call moves different cards to different
    /// positions, chosen by where each already is.
    #[test]
    fn each_card_gets_its_own_target() {
        let mut f = Field::new(8000);
        let attacking = monster(&mut f, 0, position::FACEUP_ATTACK);
        let set = monster(&mut f, 0, position::FACEDOWN_DEFENSE);
        f.change_position(
            [attacking, set],
            None,
            0,
            position::FACEUP_DEFENSE, // from face-up attack
            position::FACEUP_ATTACK,  // from face-down attack
            position::FACEUP_ATTACK,  // from face-up defence
            position::FACEUP_ATTACK,  // from face-down defence
            0,
            true,
        );
        run(&mut f);
        assert_eq!(
            f.cards[attacking].current.position,
            position::FACEUP_DEFENSE
        );
        assert_eq!(f.cards[set].current.position, position::FACEUP_ATTACK);
    }

    /// A card already in the asked-for position leaves the batch, so it is
    /// not counted.
    #[test]
    fn a_card_already_in_position_is_dropped() {
        let mut f = Field::new(8000);
        let c = monster(&mut f, 0, position::FACEUP_ATTACK);
        f.change_position_card(c, None, 0, position::FACEUP_ATTACK, 0, true);
        run(&mut f);
        assert_eq!(f.core.returns.get(), 0, "nothing changed");
    }

    /// Flipping face-up raises `EVENT_FLIP` and gives the card a new
    /// `fieldid` — it is newly visible.
    #[test]
    fn flipping_face_up_raises_flip_and_renumbers() {
        let mut f = Field::new(8000);
        let c = monster(&mut f, 0, position::FACEDOWN_DEFENSE);
        let before = f.cards[c].fieldid;
        f.change_position_card(c, None, 0, position::FACEUP_ATTACK, 0, true);
        run(&mut f);
        assert_eq!(f.cards[c].current.position, position::FACEUP_ATTACK);
        assert!(raised(&f, code::FLIP));
        assert!(raised(&f, code::CHANGE_POS));
        assert_ne!(f.cards[c].fieldid, before, "newly visible, newly numbered");
        assert_eq!(f.cards[c].previous.position, position::FACEDOWN_DEFENSE);
    }

    /// Turning face-down is treated as though the card had never been
    /// face-up: it is marked set this turn and its previous location erased.
    #[test]
    fn turning_face_down_erases_where_it_came_from() {
        let mut f = Field::new(8000);
        let c = monster(&mut f, 0, position::FACEUP_ATTACK);
        f.cards[c].previous.location = location::HAND;
        f.change_position_card(c, None, 0, position::FACEDOWN_DEFENSE, 0, true);
        run(&mut f);
        assert_eq!(f.cards[c].current.position, position::FACEDOWN_DEFENSE);
        assert!(f.cards[c].is_status(status::SET_TURN));
        assert_eq!(f.cards[c].previous.location, 0);
        assert!(!raised(&f, code::FLIP), "turning down is not a flip");
    }

    /// There is no face-down token: it is turned face-up defence instead of
    /// being refused, which is why step 0 lets a token through.
    #[test]
    fn a_token_turned_down_goes_face_up_defence() {
        let mut f = Field::new(8000);
        let mut c = Card::with_data(
            CardData {
                code: 73915051,
                type_: card_type::MONSTER | card_type::TOKEN,
                ..Default::default()
            },
            0,
        );
        c.current.controller = 0;
        c.current.position = position::FACEUP_ATTACK;
        c.set_status(status::EFFECT_ENABLED, true);
        let t = f.new_card(c);
        f.add_card(0, t, location::MZONE, 0, false);

        f.change_position_card(t, None, 0, position::FACEDOWN_DEFENSE, 0, true);
        run(&mut f);
        assert_eq!(
            f.cards[t].current.position,
            position::FACEUP_DEFENSE,
            "substituted, not refused"
        );
    }

    /// Moving to a defence position cancels the attack.
    #[test]
    fn going_to_defence_cancels_the_attack() {
        let mut f = Field::new(8000);
        let c = monster(&mut f, 0, position::FACEUP_ATTACK);
        f.change_position_card(c, None, 0, position::FACEUP_DEFENSE, 0, true);
        run(&mut f);
        assert!(f.cards[c].is_status(status::ATTACK_CANCELED));
        assert!(f.cards[c].is_status(status::JUST_POS));
    }

    /// `EFFECT_CANNOT_CHANGE_POS_E` with a value of **zero** forbids both
    /// positions, not neither. An effect that declines to be specific
    /// forbids everything — the opposite of the natural reading.
    #[test]
    fn a_position_prohibition_with_no_value_forbids_everything() {
        use crate::effect::{effect_type, Effect};
        let mut f = Field::new(8000);
        let c = monster(&mut f, 0, position::FACEUP_ATTACK);
        // an effect to be the reason, so the prohibition is consulted at all
        let mut by = Effect::new(effect_type::SINGLE, code::UPDATE_ATTACK);
        by.owner = Some(c);
        by.handler = Some(c);
        let by = f.new_effect(by);

        let mut bar = Effect::new(effect_type::SINGLE, code::CANNOT_CHANGE_POS_E);
        bar.owner = Some(c);
        bar.handler = Some(c);
        bar.value = 0;
        let bar = f.new_effect(bar);
        f.cards[c]
            .single_effect
            .insert(code::CANNOT_CHANGE_POS_E, bar);
        f.cards[c].indexer.insert(bar);

        f.change_position_card(c, Some(by), 0, position::FACEUP_DEFENSE, 0, true);
        run(&mut f);
        assert_eq!(
            f.cards[c].current.position,
            position::FACEUP_ATTACK,
            "a zero value forbids both positions"
        );
        assert_eq!(f.core.returns.get(), 0);
    }

    /// `EVENT_FLIP` is a Monster Zone event. A spell turned face-up is not
    /// "flipped" in the rules' sense, even though its position changed.
    #[test]
    fn a_spell_turned_face_up_is_not_flipped() {
        let mut f = Field::new(8000);
        let mut c = Card::with_data(
            CardData {
                code: 55144522,
                type_: card_type::SPELL,
                ..Default::default()
            },
            0,
        );
        c.current.controller = 0;
        c.current.position = position::FACEDOWN_DEFENSE;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(0, id, location::SZONE, 0, false);

        f.change_position_card(id, None, 0, position::FACEUP_ATTACK, 0, true);
        run(&mut f);
        assert_eq!(f.cards[id].current.position, position::FACEUP_ATTACK);
        assert!(
            !raised(&f, code::FLIP),
            "a spell is not flipped, only turned"
        );
    }

    mod is_capable_turn_set {
        use super::*;

        #[test]
        fn an_ordinary_face_up_monster_can_be_set() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, position::FACEUP_ATTACK);
            assert!(f.is_capable_turn_set(c, 0));
        }

        /// Already face-down: nothing to turn.
        #[test]
        fn a_face_down_card_cannot() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, position::FACEDOWN_DEFENSE);
            assert!(!f.is_capable_turn_set(c, 0));
        }

        /// A token has no face-down state.
        #[test]
        fn a_token_cannot() {
            let mut f = Field::new(8000);
            let mut c = Card::with_data(
                CardData {
                    code: 73915051,
                    type_: card_type::MONSTER | card_type::TOKEN,
                    ..Default::default()
                },
                0,
            );
            c.current.controller = 0;
            c.current.position = position::FACEUP_ATTACK;
            let t = f.new_card(c);
            f.add_card(0, t, location::MZONE, 0, false);
            assert!(!f.is_capable_turn_set(t, 0));
        }
    }

    mod refresh_location_info_instant {
        use super::*;
        use crate::effect::{effect_type, Effect};

        /// A field-wide effect with no card behind it.
        ///
        /// `EFFECT_FLAG_FIELD_ONLY` is what makes it available without a
        /// handler — `filter_field_effect` gates on `is_available`, and an
        /// effect with no handler and no flag is not available to anyone.
        fn disable_field(f: &mut Field, value: i64) {
            let mut e = Effect::new(effect_type::FIELD, code::DISABLE_FIELD);
            e.effect_owner = 0;
            e.flag[0] |= crate::effect::flag::FIELD_ONLY;
            e.value = value;
            let id = f.new_effect(e);
            f.field_effects.aura.insert(code::DISABLE_FIELD, id);
            f.field_effects.indexer.insert(id);
        }

        /// The mask is rebuilt from scratch, so an effect that stopped
        /// applying stops disabling with no bookkeeping.
        #[test]
        fn the_mask_is_rebuilt_not_adjusted() {
            let mut f = Field::new(8000);
            f.players[0].disabled_location = 0xffff;
            f.refresh_location_info_instant();
            assert_eq!(f.players[0].disabled_location, 0, "cleared, then rebuilt");
        }

        /// One value covers both players: the low half for player 0, the
        /// high half for player 1.
        #[test]
        fn one_value_covers_both_players() {
            let mut f = Field::new(8000);
            disable_field(&mut f, 0x0002_0001);
            f.refresh_location_info_instant();
            assert_eq!(f.players[0].disabled_location & 0xff7f, 0x1);
            assert_eq!(f.players[1].disabled_location & 0xff7f, 0x2);
        }

        /// Bit 7 is masked out of both halves — the seat that does not
        /// exist.
        #[test]
        fn bit_seven_is_not_a_seat() {
            let mut f = Field::new(8000);
            disable_field(&mut f, 0x0080_0080);
            f.refresh_location_info_instant();
            assert_eq!(f.players[0].disabled_location & 0x80, 0);
            assert_eq!(f.players[1].disabled_location & 0x80, 0);
        }

        /// The two Extra Monster Zones are shared, and **crossed**: one
        /// player's seat 5 is the other's seat 6.
        ///
        /// Checked in **both** directions: the crossing is two separate
        /// lines in the reference, and a test of one direction passes while
        /// the other is wrong.
        #[test]
        fn the_extra_monster_zones_are_shared_and_crossed() {
            let mut f = Field::new(8000);
            disable_field(&mut f, 1 << 5); // player 0's seat 5
            f.refresh_location_info_instant();
            assert_ne!(
                f.players[1].disabled_location & (1 << 6),
                0,
                "player 0's seat 5 is player 1's seat 6"
            );
            assert_eq!(f.players[1].disabled_location & (1 << 5), 0);

            let mut g = Field::new(8000);
            disable_field(&mut g, (1 << 5) << 16); // player 1's seat 5
            g.refresh_location_info_instant();
            assert_ne!(
                g.players[0].disabled_location & (1 << 6),
                0,
                "and the other way round"
            );
            assert_eq!(g.players[0].disabled_location & (1 << 5), 0);
        }
    }
}

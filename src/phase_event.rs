//! `PhaseEvent` — the window a phase opens when it begins or ends.
//!
//! `Turn` emplaces one at every phase boundary. It gathers what may be
//! activated *because the phase changed*, offers it, resolves what was taken,
//! and comes back for more until both players decline. At the End Phase it
//! also enforces the hand limit.
//!
//! ## The counts decide how the question is asked
//!
//! Case 0's gather is five sweeps over five different indices, and each keeps
//! its own count. The counts are not there for arithmetic — they choose the
//! *shape* of the question:
//!
//! | what was found | how it is asked |
//! |---|---|
//! | nothing | not asked; `-1` |
//! | exactly one continuous effect | not asked; taken |
//! | exactly one optional trigger | `SelectEffectYesNo` about the card |
//! | anything else | `SelectChain` over the list |
//!
//! The third of those is why case 1 exists. A yes/no answers 1 or 0 and a
//! list answers an index or -1, so case 1 subtracts one and the two become
//! the same shape — and the paths that did **not** ask jump straight to case
//! 2, skipping it, because they wrote their answer in the list's shape
//! already.
//!
//! ## Priority passes back and forth once
//!
//! `priority_passed` and `is_opponent` together are the "both players
//! declined" test: a decline from one flips to the other and restarts; a
//! decline from the second ends the window. Anything actually activated
//! clears the flag, so the pair get another turn each.
//!
//! That last clause is **not covered by a test**, and the attempt is worth
//! recording. Reaching it needs something taken *after* a decline, and the
//! only thing this port can take here without touching unported machinery is
//! a continuous effect — which is taken with no question, so the decline has
//! to come from a second effect, and the two together loop the window rather
//! than settling unless both carry count limits. A scenario contrived enough
//! to satisfy all of that is not evidence about the rule; the mutation that
//! removes the clause survives, and is recorded here rather than chased.
//!
//! ## The End Phase hand limit
//!
//! Case 20 is reached only at the End Phase and only after both players have
//! finished responding. `core.hand_adjusted` is what stops it happening
//! twice — and it also suppresses the *optional* half of the gather on the
//! way round again, because a player who has just discarded to the hand size
//! does not get another window in which to change it.

use crate::board::location;
use crate::card::{reason, status};
use crate::chain::Chain;
use crate::duel::phases;
use crate::event::{code, EffectId, Event, PLAYER_NONE};
use crate::field::{Field, Message};
use crate::processor::{Kind, RESTART};

/// The state `PhaseEvent` carries.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PhaseEventState {
    /// Whether the *opponent* is the one being offered the window.
    pub is_opponent: bool,
    /// Whether the last offer was declined. Two declines in a row — one from
    /// each player — end the window.
    pub priority_passed: bool,
}

/// How many of each kind the gather found. Only the shape matters, which is
/// why they are counted rather than collected separately.
#[derive(Clone, Copy, Debug, Default)]
struct Found {
    mandatory_trigger: u32,
    optional_trigger: u32,
    free_chain: u32,
    continuous: u32,
}

impl Field {
    /// One step of `PhaseEvent`.
    pub(crate) fn phase_event_step(
        &mut self,
        step: u16,
        phase: u16,
        state: &mut PhaseEventState,
    ) -> bool {
        match step {
            0 => self.pe_gather(phase, state),
            1 => {
                // A yes/no answered 1 or 0; a list answers an index or -1.
                let n = self.core.returns.get();
                self.core.returns.set(n - 1);
                false
            }
            2 => self.pe_take(state),
            3 => self.pe_solve(),
            4 => {
                self.adjust_instant();
                self.emplace(Kind::PointEvent {
                    skip: crate::point_event::PointEventSkip::default(),
                });
                self.set_step(RESTART);
                false
            }
            20 => self.pe_hand_limit(phase),
            21 => {
                let discarded: Vec<_> = self.core.return_cards.list.clone();
                if !discarded.is_empty() {
                    let tp = self.infos.turn_player;
                    self.send_to(
                        discarded,
                        None,
                        reason::RULE | reason::DISCARD | reason::ADJUST,
                        tp,
                        PLAYER_NONE,
                        u16::from(location::GRAVE),
                        0,
                        crate::board::position::FACEUP,
                        false,
                    );
                }
                false
            }
            22 => {
                self.core.hand_adjusted = true;
                self.emplace(Kind::PointEvent {
                    skip: crate::point_event::PointEventSkip::default(),
                });
                *state = PhaseEventState::default();
                self.set_step(RESTART);
                false
            }
            25 => {
                self.core.hint_timing[self.infos.turn_player as usize] = 0;
                self.reset_phase(phase);
                self.adjust_all();
                false
            }
            26 => {
                self.core.quick_f_chain.clear();
                self.core.instant_event.clear();
                self.core.point_event.clear();
                self.core.delayed_activate_event.clear();
                self.core.full_event.clear();
                true
            }
            _ => true,
        }
    }

    /// Case 0: what may be activated because the phase changed.
    ///
    /// **A skipped phase opens no window at all** — the five `EFFECT_SKIP_*`
    /// effects and `force_turn_end` jump straight to the phase reset. The
    /// skip codes are not contiguous (180, 181, 183, 189), which is the sort
    /// of thing a port invents by counting.
    fn pe_gather(&mut self, phase: u16, state: &mut PhaseEventState) -> bool {
        if self.phase_is_skipped(phase) {
            self.set_step(24);
            return false;
        }
        let phase_event = code::PHASE + u32::from(phase);
        let mut ev = Event::new(phase_event);
        ev.event_player = self.infos.turn_player;
        let check_player = if state.is_opponent {
            1 - self.infos.turn_player
        } else {
            self.infos.turn_player
        };
        self.core.select_chains.clear();
        let mut found = Found::default();

        for e in self
            .field_effects
            .trigger_f
            .equal_range(phase_event)
            .to_vec()
        {
            if self.offer_phase_effect(e, check_player, &ev, false) {
                found.mandatory_trigger += 1;
            }
        }
        for e in self
            .field_effects
            .continuous
            .equal_range(phase_event)
            .to_vec()
        {
            if self.offer_phase_effect(e, check_player, &ev, true) {
                found.continuous += 1;
            }
        }
        found.continuous += self.offer_lapsing_control(phase, check_player);

        self.core.spe_effect[check_player as usize] = 0;
        if !self.core.hand_adjusted {
            for e in self
                .field_effects
                .trigger_o
                .equal_range(phase_event)
                .to_vec()
            {
                if self.offer_phase_effect(e, check_player, &ev, false) {
                    found.optional_trigger += 1;
                    self.core.spe_effect[check_player as usize] += 1;
                }
            }
            self.core.hint_timing[self.infos.turn_player as usize] = phase_timing(phase);
            for (index, count_hint) in [
                (
                    self.field_effects
                        .activate
                        .equal_range(code::FREE_CHAIN)
                        .to_vec(),
                    true,
                ),
                (
                    self.field_effects
                        .quick_o
                        .equal_range(code::FREE_CHAIN)
                        .to_vec(),
                    true,
                ),
            ] {
                for e in index {
                    if !self.is_chainable(e, check_player)
                        || !self.offer_phase_effect(e, check_player, &ev, false)
                    {
                        continue;
                    }
                    if count_hint
                        && (self.check_hint_timing(e) || self.check_cteffect_hint(e, check_player))
                    {
                        self.core.spe_effect[check_player as usize] += 1;
                    }
                    found.free_chain += 1;
                }
            }
            for e in self
                .field_effects
                .continuous
                .equal_range(code::FREE_CHAIN)
                .to_vec()
            {
                if self.offer_phase_effect(e, check_player, &ev, true) {
                    found.free_chain += 1;
                }
            }
        }

        self.ask_or_take(found, check_player)
    }

    /// One candidate: renumber it and put it on the offer list if it can be
    /// activated.
    ///
    /// **The renumbering is not bookkeeping.** Every effect that reaches the
    /// offer gets a fresh `id` from `infos.field_id`, and effect id is what
    /// every sort keys on — so the order a phase's effects are offered in is
    /// the order they were *found*, not the order they were created.
    fn offer_phase_effect(
        &mut self,
        effect: EffectId,
        check_player: u8,
        ev: &Event,
        own_side_only: bool,
    ) -> bool {
        if own_side_only {
            let owner = self
                .effects
                .get(effect)
                .map_or(PLAYER_NONE, |e| e.get_handler_player(&self.cards));
            if owner != check_player {
                return false;
            }
        } else if let Some(handler) = self
            .effects
            .get(effect)
            .and_then(|e| e.get_handler(&self.cards))
        {
            let snapshot = self.cards[handler].state();
            if let Some(e) = self.effects.get_mut(effect) {
                e.set_activate_location(&snapshot);
            }
        }
        if !self.is_activateable(effect, check_player, ev, false, false, false, false, false) {
            return false;
        }
        let id = self.next_field_id_raw();
        if let Some(e) = self.effects.get_mut(effect) {
            e.id.set(id);
        }
        let mut chain = Chain::new(effect, ev.clone());
        chain.triggering_player = check_player;
        self.core.select_chains.push_back(chain);
        true
    }

    /// A control change that lapses **at the End Phase rather than at the
    /// turn's end** is offered here, so its owner can watch it go.
    ///
    /// Five conditions, and the last is the one that makes it an offer at
    /// all: the effect's value must still name the card's current
    /// controller. An effect that has already stopped applying is not
    /// something to announce.
    fn offer_lapsing_control(&mut self, phase: u16, check_player: u8) -> u32 {
        let mut found = 0;
        let tp = self.infos.turn_player;
        for e in self.field_effects.pheff.iter().copied().collect::<Vec<_>>() {
            let Some(eff) = self.effects.get(e) else {
                continue;
            };
            if eff.code != code::SET_CONTROL
                || eff.reset_flag & u32::from(phase) == 0
                || eff.reset_count != 1
            {
                continue;
            }
            let pid = eff.get_handler_player(&self.cards);
            if pid != check_player {
                continue;
            }
            let own_turn = eff.reset_flag & crate::field::reset::SELF_TURN != 0 && pid == tp;
            let their_turn = eff.reset_flag & crate::field::reset::OPPO_TURN != 0 && pid != tp;
            if !(own_turn || their_turn) {
                continue;
            }
            let Some(handler) = eff.get_handler(&self.cards) else {
                continue;
            };
            if self.effect_value_for_card_pub(e, handler) as u8
                != self.cards[handler].current.controller
            {
                continue;
            }
            let mut chain = Chain::new(e, Event::new(0));
            chain.triggering_player = check_player;
            self.core.select_chains.push_back(chain);
            found += 1;
        }
        found
    }

    fn phase_is_skipped(&mut self, phase: u16) -> bool {
        let tp = self.infos.turn_player;
        let skip = match phase {
            phases::DRAW => code::SKIP_DP,
            phases::STANDBY => code::SKIP_SP,
            phases::BATTLE_START | phases::BATTLE => code::SKIP_BP,
            phases::END => code::SKIP_EP,
            // Other phases have no skip of their own and open their window.
            _ => return false,
        };
        self.core.force_turn_end || self.is_player_affected_by_effect(tp, skip).is_some()
    }

    /// The four shapes the question can take.
    fn ask_or_take(&mut self, found: Found, check_player: u8) -> bool {
        let total = self.core.select_chains.len();
        if total == 0 {
            self.core.returns.set(-1);
            self.set_step(1);
            return false;
        }
        let only_a_continuous = found.mandatory_trigger == 0
            && found.continuous == 1
            && found.optional_trigger == 0
            && found.free_chain == 0;
        if only_a_continuous {
            self.core.returns.set(0);
            self.set_step(1);
            return false;
        }
        self.messages.push(Message::Hint {
            kind: crate::host_question::hint::EVENT,
            player: check_player,
            value: phase_hint(self.infos.phase),
        });
        let only_an_optional = found.mandatory_trigger == 0
            && found.optional_trigger == 1
            && found.free_chain == 0
            && found.continuous == 0;
        if only_an_optional {
            let card = self
                .core
                .select_chains
                .front()
                .and_then(|c| self.effects.get(c.triggering_effect))
                .and_then(|e| e.get_handler(&self.cards))
                .unwrap_or(usize::MAX);
            self.emplace(Kind::SelectEffectYesNo {
                player: check_player,
                description: 0,
                card,
            });
        } else {
            self.emplace(Kind::SelectChain {
                player: check_player,
                spe_count: self.core.spe_effect[check_player as usize],
                forced: found.mandatory_trigger + found.continuous > 0,
            });
            self.set_step(1);
        }
        false
    }

    /// Case 2: take what was chosen, or pass priority.
    ///
    /// Three kinds of thing can be chosen and each leaves by a different
    /// door: a **non-action** effect is simply removed (it was an offer to
    /// let something lapse), a **continuous** one is solved on the spot, and
    /// anything else goes onto the chain.
    fn pe_take(&mut self, state: &mut PhaseEventState) -> bool {
        let answer = self.core.returns.get();
        if answer == -1 {
            if state.priority_passed {
                self.set_step(19);
            } else {
                state.priority_passed = true;
                state.is_opponent = !state.is_opponent;
                self.set_step(RESTART);
            }
            return false;
        }
        state.priority_passed = false;
        let Some(chain) = self.core.select_chains.get(answer as usize).cloned() else {
            return false;
        };
        let effect = chain.triggering_effect;
        let is_action = self
            .effects
            .get(effect)
            .is_some_and(|e| e.is_type(crate::effect::effect_type::ACTIONS));
        let is_continuous = self
            .effects
            .get(effect)
            .is_some_and(|e| e.is_type(crate::effect::effect_type::CONTINUOUS));

        if !is_action {
            self.remove_effect_from_wherever(effect);
            self.adjust_all();
            self.set_step(3);
        } else if !is_continuous {
            let check_player = if state.is_opponent {
                1 - self.infos.turn_player
            } else {
                self.infos.turn_player
            };
            let mut taken = self.core.select_chains.remove(answer as usize).unwrap();
            taken.flag = 0;
            taken.chain_id = self.next_field_id();
            taken.triggering_player = check_player;
            if let Some(handler) = self
                .effects
                .get(effect)
                .and_then(|e| e.get_handler(&self.cards))
            {
                let snapshot = self.cards[handler].state();
                taken.set_triggering_state(&snapshot);
                self.cards[handler].set_status(status::CHAINING, true);
            }
            self.core.new_chains.push_back(taken);
            self.dec_count(effect, check_player);
            self.core.select_chains.clear();
            self.emplace(Kind::AddChain {
                is_activated_effect: false,
            });
            let asked = if self.is_flag(crate::duel::flags::INVERTED_QUICK_PRIORITY) {
                check_player
            } else {
                1 - check_player
            };
            self.emplace(Kind::QuickEffect {
                skip_freechain: false,
                player: asked,
                is_opponent: false,
            });
            self.infos.priorities = [0, 0];
        } else {
            self.core.select_chains.clear();
            let owner = self
                .effects
                .get(effect)
                .map_or(PLAYER_NONE, |e| e.get_handler_player(&self.cards));
            self.solve_continuous(owner, effect, chain.evt.clone());
            self.set_step(3);
        }
        false
    }

    /// Case 3: let the chain resolve, then come back for another offer.
    fn pe_solve(&mut self) -> bool {
        self.core.chain_limit.clear();
        for chain in self.core.current_chain.clone() {
            if let Some(handler) = self
                .effects
                .get(chain.triggering_effect)
                .and_then(|e| e.get_handler(&self.cards))
            {
                self.cards[handler].set_status(status::CHAINING, false);
            }
        }
        self.emplace(Kind::SolveChain {
            skip: crate::solve_chain::SolveChainSkip::default(),
        });
        self.set_step(RESTART);
        false
    }

    /// Case 20: the End Phase hand limit.
    ///
    /// Six cards unless an effect says otherwise, and the **last**
    /// `EFFECT_HAND_LIMIT` decides. The player chooses which of the surplus
    /// to discard — the count asked for is the surplus, not the survivors.
    fn pe_hand_limit(&mut self, phase: u16) -> bool {
        if phase != phases::END {
            self.set_step(24);
            return false;
        }
        let tp = self.infos.turn_player;
        let mut limit = 6i32;
        if let Some(&e) = self.filter_player_effect(tp, code::HAND_LIMIT).last() {
            limit = self.effect_plain_value(e) as i32;
        }
        let held = self.players[tp as usize].hand.len() as i32;
        if held <= limit || self.is_flag(crate::duel::flags::NO_HAND_LIMIT) {
            self.set_step(24);
            return false;
        }
        self.core.select_cards = self.players[tp as usize].hand.clone();
        self.messages.push(Message::Hint {
            kind: crate::host_question::hint::SELECTMSG,
            player: tp,
            value: 501,
        });
        let surplus = (held - limit) as u8;
        self.emplace(Kind::SelectCard {
            player: tp,
            cancelable: false,
            min: surplus,
            max: surplus,
        });
        false
    }
}

impl Field {
    /// `field::reset_phase` — take away the effects whose time is up.
    ///
    /// Every phase-scoped effect is asked, and the ones that answer yes are
    /// removed. `effect::reset` for `RESET_PHASE` is where the decrement
    /// lives: an effect lasting "until the end of your next turn" carries a
    /// count of two and is asked twice.
    ///
    /// **The turn test is part of the decrement, not of the removal.** An
    /// effect scoped to its owner's turn is not counted down on the
    /// opponent's — but one whose count has *already* reached zero is
    /// removed whichever turn it is, because the count check sits outside
    /// the turn check.
    pub fn reset_phase(&mut self, phase: u16) {
        let tp = self.infos.turn_player;
        for e in self.field_effects.pheff.iter().copied().collect::<Vec<_>>() {
            let Some(eff) = self.effects.get(e) else {
                continue;
            };
            if eff.reset_flag & crate::field::reset::PHASE == 0 {
                continue;
            }
            let pid = eff.get_handler_player(&self.cards);
            let own_turn = eff.reset_flag & crate::field::reset::SELF_TURN != 0 && pid == tp;
            let their_turn = eff.reset_flag & crate::field::reset::OPPO_TURN != 0 && pid != tp;
            let this_phase = u32::from(phase) & 0x3ff & eff.reset_flag != 0;
            if (own_turn || their_turn) && this_phase {
                if let Some(eff) = self.effects.get_mut(e) {
                    eff.reset_count = eff.reset_count.saturating_sub(1);
                }
            }
            if self.effects.get(e).is_some_and(|x| x.reset_count == 0) {
                self.remove_effect_from_wherever(e);
            }
        }
    }
}

/// The timing a phase's window charges, which is what a card's "during the
/// End Phase" condition reads.
fn phase_timing(phase: u16) -> u32 {
    use crate::field::timing;
    match phase {
        phases::DRAW => timing::DRAW_PHASE,
        phases::STANDBY => timing::STANDBY_PHASE,
        phases::BATTLE_START => timing::BATTLE_START,
        phases::BATTLE => timing::BATTLE_END,
        _ => timing::END_PHASE,
    }
}

/// The `HINT_EVENT` description each phase's window is announced with.
fn phase_hint(phase: u16) -> u64 {
    match phase {
        phases::DRAW => 20,
        phases::STANDBY => 21,
        phases::BATTLE_START => 28,
        phases::BATTLE => 25,
        _ => 26,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::position;
    use crate::card::{card_type, Card, CardData};
    use crate::effect::{effect_type, flag, Effect};
    use crate::event::CardId;
    use crate::processor::Status;

    fn on_field(f: &mut Field, player: u8, seat: u32) -> CardId {
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

    fn in_hand(f: &mut Field, player: u8, seat: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 55144522 + seat,
                type_: card_type::SPELL,
                ..Default::default()
            },
            player,
        );
        c.current.controller = player;
        let id = f.new_card(c);
        f.add_card(player, id, location::HAND, seat, false);
        id
    }

    /// A phase-triggered effect on a card, in one of the field indices.
    fn trigger(f: &mut Field, card: CardId, ty: u16, phase: u16) -> EffectId {
        let phase_event = code::PHASE + u32::from(phase);
        let mut e = Effect::new(ty | effect_type::FIELD | effect_type::ACTIONS, phase_event);
        e.owner = Some(card);
        e.handler = Some(card);
        e.range = u16::from(location::MZONE);
        e.description = 1;
        let id = f.new_effect(e);
        if let Some(index) = f
            .field_effects
            .index_for_mut(ty | effect_type::FIELD | effect_type::ACTIONS)
        {
            index.insert(phase_event, id);
        }
        f.field_effects.indexer.insert(id);
        id
    }

    fn player_effect(f: &mut Field, card: CardId, code_: u32) -> EffectId {
        let mut e = Effect::new(effect_type::FIELD, code_);
        e.owner = Some(card);
        e.handler = Some(card);
        e.flag[0] = flag::PLAYER_TARGET | flag::ABSOLUTE_TARGET;
        e.range = u16::from(location::MZONE);
        e.s_range = 1;
        let id = f.new_effect(e);
        f.add_effect(id, 0);
        id
    }

    fn run(f: &mut Field, answers: &mut Vec<i32>, discard: &[usize]) -> Status {
        for _ in 0..4096 {
            match f.process() {
                Status::Continue => continue,
                Status::Awaiting => match f.messages.last() {
                    Some(Message::SelectChain { .. }) => {
                        let a = if answers.is_empty() {
                            -1
                        } else {
                            answers.remove(0)
                        };
                        f.core.returns.set(a);
                    }
                    Some(Message::SelectEffectYesNo { .. }) => {
                        let a = if answers.is_empty() {
                            0
                        } else {
                            answers.remove(0)
                        };
                        f.core.returns.set(a);
                    }
                    Some(Message::SelectCard { .. } | Message::Retry) => {
                        f.core.returns.set_i32(0, 0);
                        f.core.returns.set_i32(1, discard.len() as i32);
                        for (i, &d) in discard.iter().enumerate() {
                            f.core.returns.set_i32(i + 2, d as i32);
                        }
                    }
                    _ => return Status::Awaiting,
                },
                other => return other,
            }
        }
        panic!("did not settle");
    }

    /// Run until a question is asked, and leave it unanswered.
    fn run_until_question(f: &mut Field) -> Status {
        for _ in 0..4096 {
            match f.process() {
                Status::Continue => continue,
                other => return other,
            }
        }
        panic!("did not settle");
    }

    fn phase_event(f: &mut Field, phase: u16) {
        f.infos.phase = phase;
        f.emplace(Kind::PhaseEvent {
            phase,
            state: Default::default(),
        });
    }

    /// A phase with nothing to offer closes without asking, and resets the
    /// hint timing on the way out.
    #[test]
    fn a_quiet_phase_closes() {
        let mut f = Field::new(8000);
        f.core.hint_timing[0] = 0xff;
        phase_event(&mut f, phases::DRAW);
        assert_eq!(run(&mut f, &mut vec![], &[]), Status::End);
        assert_eq!(f.core.hint_timing[0], 0, "the timing is cleared at the end");
        assert!(!f
            .messages
            .iter()
            .any(|m| matches!(m, Message::SelectChain { .. })));
    }

    mod skips {
        use super::*;

        /// **Each phase has its own skip code**, and they are not
        /// contiguous. A skipped phase opens no window at all.
        #[test]
        fn each_phase_has_its_own_skip() {
            for (phase, skip) in [
                (phases::DRAW, code::SKIP_DP),
                (phases::STANDBY, code::SKIP_SP),
                (phases::BATTLE_START, code::SKIP_BP),
                (phases::BATTLE, code::SKIP_BP),
                (phases::END, code::SKIP_EP),
            ] {
                let mut f = Field::new(8000);
                let anchor = on_field(&mut f, 0, 0);
                let c = on_field(&mut f, 0, 1);
                trigger(&mut f, c, effect_type::TRIGGER_O, phase);
                player_effect(&mut f, anchor, skip);
                phase_event(&mut f, phase);
                run(&mut f, &mut vec![], &[]);
                assert!(
                    !f.messages.iter().any(|m| matches!(
                        m,
                        Message::Hint {
                            kind: crate::host_question::hint::EVENT,
                            ..
                        }
                    )),
                    "phase {phase:#x} was skipped"
                );
            }
        }

        /// And the wrong skip does not close the window.
        #[test]
        fn the_wrong_skip_does_not_close_it() {
            let mut f = Field::new(8000);
            let anchor = on_field(&mut f, 0, 0);
            let c = on_field(&mut f, 0, 1);
            trigger(&mut f, c, effect_type::TRIGGER_O, phases::DRAW);
            player_effect(&mut f, anchor, code::SKIP_EP);
            phase_event(&mut f, phases::DRAW);
            run(&mut f, &mut vec![0], &[]);
            assert!(
                f.messages.iter().any(|m| matches!(
                    m,
                    Message::Hint {
                        kind: crate::host_question::hint::EVENT,
                        ..
                    }
                )),
                "the End Phase skip says nothing about the Draw Phase"
            );
        }

        /// A forced turn end closes every phase's window.
        #[test]
        fn a_forced_turn_end_closes_them_all() {
            let mut f = Field::new(8000);
            let c = on_field(&mut f, 0, 0);
            trigger(&mut f, c, effect_type::TRIGGER_O, phases::DRAW);
            f.core.force_turn_end = true;
            phase_event(&mut f, phases::DRAW);
            run(&mut f, &mut vec![], &[]);
            assert!(!f.messages.iter().any(|m| matches!(
                m,
                Message::Hint {
                    kind: crate::host_question::hint::EVENT,
                    ..
                }
            )));
        }
    }

    mod the_shape_of_the_question {
        use super::*;

        /// **One optional trigger is a yes/no about the card**, not a list.
        #[test]
        fn one_optional_trigger_is_a_yes_no() {
            let mut f = Field::new(8000);
            let c = on_field(&mut f, 0, 0);
            trigger(&mut f, c, effect_type::TRIGGER_O, phases::DRAW);
            phase_event(&mut f, phases::DRAW);
            run(&mut f, &mut vec![0], &[]);
            assert!(
                f.messages
                    .iter()
                    .any(|m| matches!(m, Message::SelectEffectYesNo { .. })),
                "asked about the card"
            );
            assert!(!f
                .messages
                .iter()
                .any(|m| matches!(m, Message::SelectChain { .. })));
        }

        /// **One continuous effect is taken without asking at all.** It is
        /// the only shape that resolves with no question — a lingering
        /// effect whose phase has come does not need permission.
        #[test]
        fn one_continuous_effect_is_taken_silently() {
            let mut f = Field::new(8000);
            let c = on_field(&mut f, 0, 0);
            let e = trigger(&mut f, c, effect_type::CONTINUOUS, phases::DRAW);
            if let Some(x) = f.effects.get_mut(e) {
                x.operation = Some(|f, _| {
                    f.players[0].lp -= 500;
                    crate::effect::Yield::Done(0)
                });
                // Once. A continuous effect with no limit that stays
                // activatable is offered again the moment the window comes
                // round, which is the window doing its job and an infinite
                // loop in a test.
                x.flag[0] |= flag::COUNT_LIMIT;
                x.count_limit = 1;
            }
            phase_event(&mut f, phases::DRAW);
            run(&mut f, &mut vec![], &[]);
            assert!(
                !f.messages.iter().any(|m| matches!(
                    m,
                    Message::Hint {
                        kind: crate::host_question::hint::EVENT,
                        ..
                    }
                )),
                "nothing was asked"
            );
            assert_eq!(f.players[0].lp, 7500, "and it resolved");
        }

        /// **Two of them is a list.**
        #[test]
        fn two_offers_are_a_list() {
            let mut f = Field::new(8000);
            let a = on_field(&mut f, 0, 0);
            let b = on_field(&mut f, 0, 1);
            trigger(&mut f, a, effect_type::TRIGGER_O, phases::DRAW);
            trigger(&mut f, b, effect_type::TRIGGER_O, phases::DRAW);
            phase_event(&mut f, phases::DRAW);
            run(&mut f, &mut vec![-1, -1], &[]);
            assert!(
                f.messages
                    .iter()
                    .any(|m| matches!(m, Message::SelectChain { .. })),
                "offered as a list"
            );
        }

        /// A **mandatory** trigger makes the list forced — the player picks
        /// the order, not whether.
        #[test]
        fn a_mandatory_trigger_forces_the_question() {
            let mut f = Field::new(8000);
            let a = on_field(&mut f, 0, 0);
            let b = on_field(&mut f, 0, 1);
            trigger(&mut f, a, effect_type::TRIGGER_F, phases::DRAW);
            trigger(&mut f, b, effect_type::TRIGGER_O, phases::DRAW);
            phase_event(&mut f, phases::DRAW);
            // Stopped at the question rather than answered: taking a
            // mandatory trigger reaches `check_chain_counter`, which is not
            // ported.
            run_until_question(&mut f);
            let forced = f
                .messages
                .iter()
                .any(|m| matches!(m, Message::SelectChain { forced: true, .. }));
            assert!(forced, "a mandatory trigger cannot be declined");
        }
    }

    mod priority {
        use super::*;

        /// **Declining passes to the opponent once, and a second decline
        /// ends the window** — so the hint is emitted for each player.
        #[test]
        fn priority_passes_once_each_way() {
            let mut f = Field::new(8000);
            f.infos.turn_player = 0;
            let mine = on_field(&mut f, 0, 0);
            let theirs = on_field(&mut f, 1, 0);
            trigger(&mut f, mine, effect_type::TRIGGER_O, phases::DRAW);
            trigger(&mut f, theirs, effect_type::TRIGGER_O, phases::DRAW);
            phase_event(&mut f, phases::DRAW);
            run(&mut f, &mut vec![0, 0], &[]);
            let asked: Vec<u8> = f
                .messages
                .iter()
                .filter_map(|m| match m {
                    Message::Hint {
                        kind: crate::host_question::hint::EVENT,
                        player,
                        ..
                    } => Some(*player),
                    _ => None,
                })
                .collect();
            assert!(
                asked.contains(&0) && asked.contains(&1),
                "both were asked: {asked:?}"
            );
        }
    }

    mod the_hand_limit {
        use super::*;

        /// Over the limit at the End Phase, the surplus is discarded.
        #[test]
        fn the_surplus_is_discarded() {
            let mut f = Field::new(8000);
            f.infos.turn_player = 0;
            let mut hand = Vec::new();
            for seat in 0..7 {
                hand.push(in_hand(&mut f, 0, seat));
            }
            phase_event(&mut f, phases::END);
            run(&mut f, &mut vec![], &[0]);
            assert_eq!(f.players[0].hand.len(), 6, "down to six");
            assert!(f.core.hand_adjusted, "and it does not happen twice");
        }

        /// Six or fewer is not asked about.
        #[test]
        fn a_legal_hand_is_left_alone() {
            let mut f = Field::new(8000);
            f.infos.turn_player = 0;
            for seat in 0..6 {
                in_hand(&mut f, 0, seat);
            }
            phase_event(&mut f, phases::END);
            run(&mut f, &mut vec![], &[]);
            assert_eq!(f.players[0].hand.len(), 6);
            assert!(!f.messages.iter().any(|m| matches!(
                m,
                Message::Hint {
                    kind: crate::host_question::hint::SELECTMSG,
                    value: 501,
                    ..
                }
            )));
        }

        /// **Only at the End Phase.**
        #[test]
        fn other_phases_do_not_check_it() {
            let mut f = Field::new(8000);
            f.infos.turn_player = 0;
            for seat in 0..8 {
                in_hand(&mut f, 0, seat);
            }
            phase_event(&mut f, phases::DRAW);
            run(&mut f, &mut vec![], &[]);
            assert_eq!(
                f.players[0].hand.len(),
                8,
                "nobody counts in the Draw Phase"
            );
        }

        /// `EFFECT_HAND_LIMIT` moves the line, and the **last** one decides.
        #[test]
        fn an_effect_moves_the_limit() {
            let mut f = Field::new(8000);
            f.infos.turn_player = 0;
            let anchor = on_field(&mut f, 0, 0);
            let e = player_effect(&mut f, anchor, code::HAND_LIMIT);
            if let Some(x) = f.effects.get_mut(e) {
                x.value = 2;
            }
            for seat in 0..4 {
                in_hand(&mut f, 0, seat);
            }
            phase_event(&mut f, phases::END);
            run(&mut f, &mut vec![], &[0, 1]);
            assert_eq!(f.players[0].hand.len(), 2);
        }

        /// **The last `EFFECT_HAND_LIMIT` decides**, not the first.
        #[test]
        fn the_last_hand_limit_decides() {
            let mut f = Field::new(8000);
            f.infos.turn_player = 0;
            let a = on_field(&mut f, 0, 0);
            let b = on_field(&mut f, 0, 1);
            for (card, v) in [(a, 5i64), (b, 2)] {
                let e = player_effect(&mut f, card, code::HAND_LIMIT);
                if let Some(x) = f.effects.get_mut(e) {
                    x.value = v;
                }
            }
            for seat in 0..4 {
                in_hand(&mut f, 0, seat);
            }
            phase_event(&mut f, phases::END);
            run(&mut f, &mut vec![], &[0, 1]);
            assert_eq!(f.players[0].hand.len(), 2, "the later effect decides");
        }

        /// The duel option switches it off entirely.
        #[test]
        fn the_duel_option_switches_it_off() {
            let mut f = Field::with_flags(
                8000,
                crate::duel::REFERENCE_CONFIGURATION | crate::duel::flags::NO_HAND_LIMIT,
            );
            f.infos.turn_player = 0;
            for seat in 0..8 {
                in_hand(&mut f, 0, seat);
            }
            phase_event(&mut f, phases::END);
            run(&mut f, &mut vec![], &[]);
            assert_eq!(f.players[0].hand.len(), 8);
        }
    }

    mod resetting {
        use super::*;

        /// **`reset_phase` counts down, and removes at zero.** An effect
        /// lasting two turns survives the first pass.
        #[test]
        fn a_two_turn_effect_survives_one_pass() {
            let mut f = Field::new(8000);
            let c = on_field(&mut f, 0, 0);
            let mut e = Effect::new(effect_type::FIELD, code::UPDATE_ATTACK);
            e.owner = Some(c);
            e.handler = Some(c);
            e.flag[0] = flag::FIELD_ONLY;
            // A `FIELD_ONLY` effect answers `get_handler_player` from
            // `effect_owner` rather than from the card, and the turn test
            // below reads that.
            e.effect_owner = 0;
            e.range = u16::from(location::MZONE);
            e.reset_flag = crate::field::reset::PHASE
                | crate::field::reset::SELF_TURN
                | u32::from(phases::END);
            e.reset_count = 2;
            let id = f.new_effect(e);
            f.add_effect(id, 0);
            assert!(f.field_effects.pheff.contains(&id));

            f.reset_phase(phases::END);
            assert_eq!(f.effects.get(id).map(|x| x.reset_count), Some(1));
            assert!(f.field_effects.pheff.contains(&id), "still there");

            f.reset_phase(phases::END);
            assert!(!f.field_effects.pheff.contains(&id), "and now gone");
        }

        /// The window itself resets the phase on its way out — the direct
        /// tests above are about the function, this one about the machine
        /// calling it.
        #[test]
        fn the_window_resets_the_phase_on_its_way_out() {
            let mut f = Field::new(8000);
            let c = on_field(&mut f, 0, 0);
            let mut e = Effect::new(effect_type::FIELD, code::UPDATE_ATTACK);
            e.owner = Some(c);
            e.handler = Some(c);
            e.flag[0] = flag::FIELD_ONLY;
            e.effect_owner = 0;
            e.range = u16::from(location::MZONE);
            e.reset_flag = crate::field::reset::PHASE
                | crate::field::reset::SELF_TURN
                | u32::from(phases::END);
            e.reset_count = 1;
            let id = f.new_effect(e);
            f.add_effect(id, 0);

            phase_event(&mut f, phases::END);
            run(&mut f, &mut vec![], &[]);
            assert!(
                !f.field_effects.pheff.contains(&id),
                "its phase came and the window took it"
            );
        }

        /// **The turn test guards the decrement, not the removal.** An
        /// effect scoped to its owner's turn is not counted down on the
        /// opponent's.
        #[test]
        fn the_opponents_turn_does_not_count_down_an_own_turn_effect() {
            let mut f = Field::new(8000);
            f.infos.turn_player = 1;
            let c = on_field(&mut f, 0, 0);
            let mut e = Effect::new(effect_type::FIELD, code::UPDATE_ATTACK);
            e.owner = Some(c);
            e.handler = Some(c);
            e.flag[0] = flag::FIELD_ONLY;
            // A `FIELD_ONLY` effect answers `get_handler_player` from
            // `effect_owner` rather than from the card, and the turn test
            // below reads that.
            e.effect_owner = 0;
            e.range = u16::from(location::MZONE);
            e.reset_flag = crate::field::reset::PHASE
                | crate::field::reset::SELF_TURN
                | u32::from(phases::END);
            e.reset_count = 1;
            let id = f.new_effect(e);
            f.add_effect(id, 0);

            f.reset_phase(phases::END);
            assert_eq!(
                f.effects.get(id).map(|x| x.reset_count),
                Some(1),
                "not this player's turn"
            );
            f.infos.turn_player = 0;
            f.reset_phase(phases::END);
            assert!(!f.field_effects.pheff.contains(&id));
        }

        /// A different phase does not count it down either.
        #[test]
        fn a_different_phase_does_not_count_it_down() {
            let mut f = Field::new(8000);
            let c = on_field(&mut f, 0, 0);
            let mut e = Effect::new(effect_type::FIELD, code::UPDATE_ATTACK);
            e.owner = Some(c);
            e.handler = Some(c);
            e.flag[0] = flag::FIELD_ONLY;
            // A `FIELD_ONLY` effect answers `get_handler_player` from
            // `effect_owner` rather than from the card, and the turn test
            // below reads that.
            e.effect_owner = 0;
            e.range = u16::from(location::MZONE);
            e.reset_flag = crate::field::reset::PHASE
                | crate::field::reset::SELF_TURN
                | u32::from(phases::END);
            e.reset_count = 1;
            let id = f.new_effect(e);
            f.add_effect(id, 0);

            f.reset_phase(phases::DRAW);
            assert_eq!(f.effects.get(id).map(|x| x.reset_count), Some(1));
        }
    }

    /// The two per-phase tables, pinned against each other.
    #[test]
    fn each_phase_has_its_own_timing_and_prompt() {
        use crate::field::timing;
        for (phase, t, hint) in [
            (phases::DRAW, timing::DRAW_PHASE, 20u64),
            (phases::STANDBY, timing::STANDBY_PHASE, 21),
            (phases::BATTLE_START, timing::BATTLE_START, 28),
            (phases::BATTLE, timing::BATTLE_END, 25),
            (phases::END, timing::END_PHASE, 26),
        ] {
            assert_eq!(phase_timing(phase), t, "timing for {phase:#x}");
            assert_eq!(phase_hint(phase), hint, "hint for {phase:#x}");
        }
    }
}

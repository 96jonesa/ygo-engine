//! The Main Phase: `IdleCommand`.
//!
//! The turn player's menu, and the loop that keeps offering it. Every case
//! but three ends with `set_step(RESTART)` — back to case 0, rebuild the
//! lists, ask again — so this is not a sequence of steps at all. It is a
//! **dispatcher with a loop around it**, and reading it top to bottom
//! suggests a progression that does not exist.
//!
//! ```text
//!   0  gather, build five lists, ask ────────┐
//!   1  dispatch on the answer                │
//!        ctype 0 summon      → 5 ────────────┤
//!        ctype 1 spsummon    → 6 ────────────┤
//!        ctype 2 reposition  → 7 ──┬─────────┤
//!        ctype 3 mset        → 8 ──│─────────┤   every one of these
//!        ctype 4 sset        → 9 ──│─────────┤   returns to case 0
//!        ctype 5 activate    → 2 ──│─────────┤
//!                  continuous → 3 ─│─────────┤
//!        ctype 8 shuffle     ──────│─────────┘
//!        ctype 6/7 leave     → 10 ─│──→ 11  ← the only way out
//!                                  └──→ 12, 13  (a face-down monster)
//! ```
//!
//! ## There is no case 4
//!
//! The reference's switch skips from 3 to 5. It is not a gap: the processor
//! increments `step` after a handler returns false, so `arg.step = 4` means
//! "case 5 next". Every jump in case 1 is written one lower than the case it
//! reaches, and case 4 is simply the number nobody lands on.
//!
//! ## Case 0's gather is *bare*, unlike `PhaseEvent`'s
//!
//! Both build `core.select_chains`. `PhaseEvent` fills in each chain's id
//! and triggering player as it gathers; `IdleCommand` sets nothing but
//! `triggering_effect` and leaves the rest for case 1 to fill in **on the
//! one chain the player picked**. So a chain offered here and declined was
//! never a chain in any meaningful sense — it had no id, no event, no
//! triggering state. That asymmetry is easy to read as an omission.
//!
//! ## The two ways to leave, and why one of them asks
//!
//! `to_bp` and `to_ep` are recomputed at the top of every pass. When the
//! phase is being *skipped* — `EFFECT_SKIP_M1`/`M2`, or `force_turn_end` —
//! the menu is not offered at all, and if both destinations are open the
//! player is asked which by `SelectOption` rather than by the menu. That is
//! the only place in this unit where a question is asked with the menu
//! suppressed.
//!
//! ## `EFFECT_MUST_ATTACK` takes the End Phase away
//!
//! A monster obliged to attack means the player may not skip the battle, so
//! `to_ep` goes down. Note what is asked of the monster: `is_capable_attack`
//! **and** the effect. A monster under `MUST_ATTACK` that could not attack
//! anyway does not trap its controller in the Battle Phase.

use crate::board::{location, position};
use crate::card::status;
use crate::chain::Chain;
use crate::event::{code, CardId, EffectId, Event, PLAYER_NONE};
use crate::field::{timing, Field};
use crate::processor::{Kind, RESTART};

/// The `IdleCommand` unit's own state: the phase it decided to leave for,
/// and the card it is part-way through repositioning.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IdleCommandState {
    /// The phase code written into `returns` on the way out. `6` is the
    /// Battle Phase and `7` the End Phase — **the `ctype` values**, not
    /// `PHASE_*` constants, because the caller reads them as commands.
    pub phase_to_change_to: u8,
    /// The face-down attack-position monster whose two possible answers are
    /// being put to the player in cases 12-13.
    pub card_to_reposition: Option<CardId>,
}

/// The zone mask the reference's `summon` and `mset` default to: the five
/// main monster zones.
///
/// `field.h` declares `zone = 0x1f` on both, and `IdleCommand` calls them
/// without it. A default argument is invisible at the call site, so it is
/// the easiest thing in a port to drop — and dropping it here leaves the
/// mask at zero, which is *no* zone rather than *any*, so the summon is
/// refused with nothing to show for it.
pub(crate) const DEFAULT_SUMMON_ZONE: u32 = 0x1f;

impl Field {
    /// `field::process(Processors::IdleCommand&)`.
    pub(crate) fn idle_command_step(&mut self, step: u16, state: &mut IdleCommandState) -> bool {
        match step {
            0 => self.idle_step_0(state),
            1 => self.idle_step_1(state),
            2 => self.idle_step_2(),
            3 => self.idle_step_3(),
            5 => self.idle_step_5(),
            6 => self.idle_step_6(),
            7 => self.idle_step_7(state),
            8 => self.idle_step_8(),
            9 => self.idle_step_9(),
            10 => self.idle_step_10(),
            11 => self.idle_step_11(state),
            12 => self.idle_step_12(state),
            13 => self.idle_step_13(state),
            // Case 4 does not exist, and neither does anything past 13.
            // The reference's switch falls through to `return TRUE`.
            _ => true,
        }
    }

    /// Case 0: decide whether the phase is happening, gather what can be
    /// done in it, and ask.
    fn idle_step_0(&mut self, state: &mut IdleCommandState) -> bool {
        self.core.select_chains.clear();

        // A forced attack pre-empts the whole phase.
        if self.core.set_forced_attack {
            self.core.set_forced_attack = false;
            self.set_step(RESTART);
            self.emplace(Kind::ForcedBattle {
                state: Default::default(),
            });
            return false;
        }

        let tp = self.infos.turn_player;
        self.core.to_bp = true;
        self.core.to_ep = true;

        // No Battle Phase on the opening turn (the duel option that would
        // allow one is off in this configuration), none from Main Phase 2,
        // none while prohibited, and none while the turn is being wound up.
        let first_turn_no_battle = !self.is_flag(crate::duel::flags::ATTACK_FIRST_TURN)
            && self.infos.turn_id == 1
            && self
                .is_player_affected_by_effect(tp, code::BP_FIRST_TURN)
                .is_none();
        if first_turn_no_battle
            || self.infos.phase == crate::duel::phases::MAIN2
            || self
                .is_player_affected_by_effect(tp, code::CANNOT_BP)
                .is_some()
            || self.core.force_turn_end
        {
            self.core.to_bp = false;
        }

        if self.infos.phase == crate::duel::phases::MAIN1 {
            let mzone: Vec<CardId> = self.players[tp as usize]
                .mzone
                .iter()
                .flatten()
                .copied()
                .collect();
            let mut must_attack = false;
            for card in mzone {
                if self.is_capable_attack(card)
                    && self
                        .is_affected_by_effect(card, code::MUST_ATTACK)
                        .is_some()
                {
                    must_attack = true;
                    break;
                }
            }
            // Only from Main Phase 1, and only while there is a battle to
            // be had: a player with nowhere to go may always end the turn.
            if self.core.to_bp
                && (must_attack
                    || self
                        .is_player_affected_by_effect(tp, code::CANNOT_EP)
                        .is_some())
            {
                self.core.to_ep = false;
            }
        }

        // The phase is skipped: leave without offering anything, asking
        // only which way when both ways are open.
        let skipped = (self.infos.phase == crate::duel::phases::MAIN1
            && self
                .is_player_affected_by_effect(tp, code::SKIP_M1)
                .is_some())
            || (self.infos.phase == crate::duel::phases::MAIN2
                && self
                    .is_player_affected_by_effect(tp, code::SKIP_M2)
                    .is_some())
            || self.core.force_turn_end;
        if skipped {
            if self.core.to_bp && self.core.to_ep {
                self.core.select_options = vec![80, 81];
                self.emplace(Kind::SelectOption { player: tp });
                self.set_step(11);
            } else {
                state.phase_to_change_to = if self.core.to_bp { 6 } else { 7 };
                self.set_step(10);
                let phase = self.infos.phase;
                self.reset_phase(phase);
                self.adjust_all();
            }
            return false;
        }

        // `core.skip_m2` is a one-shot, spent as it is read: answer "go to
        // the End Phase" and let case 1 dispatch on it.
        if self.infos.phase == crate::duel::phases::MAIN2 && self.core.skip_m2 {
            self.core.skip_m2 = false;
            self.core.returns.set(7);
            return false;
        }

        self.idle_gather_chains(tp);
        self.idle_gather_cards(tp);
        self.emplace(Kind::SelectIdleCmd { player: tp });
        false
    }

    /// The four registries case 0 offers from, in the reference's order.
    ///
    /// Three are keyed on `EVENT_FREE_CHAIN` and the fourth — ignition —
    /// is walked whole, because an ignition effect is not keyed to an event
    /// at all. The continuous one is the odd one out twice over: it is the
    /// only registry filtered to the turn player's **own** effects, and the
    /// only one whose entries do not get `set_activate_location` called on
    /// them first.
    fn idle_gather_chains(&mut self, tp: u8) {
        let nil = Event::new(code::FREE_CHAIN);
        let activate = self
            .field_effects
            .activate
            .equal_range(code::FREE_CHAIN)
            .to_vec();
        let quick = self
            .field_effects
            .quick_o
            .equal_range(code::FREE_CHAIN)
            .to_vec();
        for e in activate.into_iter().chain(quick) {
            self.idle_offer(e, tp, &nil, false);
        }
        for e in self
            .field_effects
            .continuous
            .equal_range(code::FREE_CHAIN)
            .to_vec()
        {
            self.idle_offer(e, tp, &nil, true);
        }
        // The ignition index is walked **whole**, not looked up: an
        // ignition effect is keyed by its own code, not by an event, so
        // there is nothing to look it up by.
        let ignition: Vec<EffectId> = self
            .field_effects
            .ignition
            .iter()
            .map(|&(_, e)| e)
            .collect();
        for e in ignition {
            self.idle_offer(e, tp, &nil, false);
        }
    }

    /// One candidate, offered or not.
    ///
    /// The chain that goes into `core.select_chains` carries **only** its
    /// effect — no id, no triggering player, no state. Case 1 fills those in
    /// on the one the player picks.
    fn idle_offer(&mut self, effect: EffectId, tp: u8, nil: &Event, own_side_only: bool) {
        if own_side_only {
            let owner = self
                .effects
                .get(effect)
                .map_or(PLAYER_NONE, |e| e.get_handler_player(&self.cards));
            if owner != tp {
                return;
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
        if !self.is_activateable(effect, tp, nil, false, false, false, false, false) {
            return;
        }
        self.core
            .select_chains
            .push_back(Chain::new(effect, Event::new(code::FREE_CHAIN)));
    }

    /// The five card lists, each a filter over one or two zones.
    ///
    /// Two things here are not symmetrical and look like slips. The summon
    /// list walks the hand **and the Monster Zones** — the second is the
    /// Gemini re-summon, which is a normal summon of a card already on the
    /// field. And the reposition list asks two different questions of one
    /// card: a face-up or face-down-attack monster is asked whether it may
    /// be turned, a face-down one whether it may be flip summoned, and a
    /// face-down **attack** monster is asked both, which is why it can enter
    /// the list for either reason and why case 7 has to ask the player which
    /// was meant.
    fn idle_gather_cards(&mut self, tp: u8) {
        let hand: Vec<CardId> = self.players[tp as usize].hand.clone();
        let mzone: Vec<CardId> = self.players[tp as usize]
            .mzone
            .iter()
            .flatten()
            .copied()
            .collect();

        self.core.summonable_cards.clear();
        for card in hand.iter().chain(mzone.iter()).copied() {
            if self.is_can_be_summoned(card, tp, false, None, 0, 0xff) {
                self.core.summonable_cards.push(card);
            }
        }

        self.core.spsummonable_cards.clear();
        for e in self.filter_field_effect(code::SPSUMMON_PROC) {
            let Some(handler) = self.effects.get(e).and_then(|x| x.get_handler(&self.cards)) else {
                continue;
            };
            let controller = self.cards[handler].current.controller;
            if !self.check_count_limit(e, controller) {
                continue;
            }
            if controller == tp && self.is_special_summonable(handler, tp, 0) {
                self.core.spsummonable_cards.push(handler);
            }
        }
        // `EFFECT_SPSUMMON_PROC_G`, the group procedures. Unlike the
        // single-card ones these are asked by running the procedure's
        // *condition* directly, with the reason effect and player swapped
        // in around it, rather than through a predicate.
        for e in self.filter_field_effect(code::SPSUMMON_PROC_G) {
            let Some(handler) = self.effects.get(e).and_then(|x| x.get_handler(&self.cards)) else {
                continue;
            };
            if !self.check_count_limit(e, tp) {
                continue;
            }
            let controller = self.cards[handler].current.controller;
            let both_sides = self
                .effects
                .get(e)
                .is_some_and(|x| x.is_flag(crate::effect::flag::BOTH_SIDE));
            if controller != tp && !both_sides {
                continue;
            }
            let (old_effect, old_player) = (self.core.reason_effect, self.core.reason_player);
            self.core.reason_effect = Some(e);
            self.core.reason_player = controller;
            self.save_lp_cost();
            let holds = self.check_procedure_group_condition(e, handler);
            self.restore_lp_cost();
            self.core.reason_effect = old_effect;
            self.core.reason_player = old_player;
            if holds {
                self.core.spsummonable_cards.push(handler);
            }
        }

        self.core.repositionable_cards.clear();
        for card in mzone {
            let turnable = self.cards[card]
                .current
                .is_position(position::FACEUP | position::FACEDOWN_ATTACK)
                && self.is_capable_change_position(card, tp);
            let flippable = self.cards[card].current.is_position(position::FACEDOWN)
                && self.is_can_be_flip_summoned(card, tp);
            if turnable || flippable {
                self.core.repositionable_cards.push(card);
            }
        }

        self.core.msetable_cards.clear();
        self.core.ssetable_cards.clear();
        for card in hand {
            if self.is_setable_mzone(card, tp, false, None, 0, 0xff) {
                self.core.msetable_cards.push(card);
            }
            if self.is_setable_szone(card, tp, false) {
                self.core.ssetable_cards.push(card);
            }
        }
    }

    /// `check_condition(peff->condition, 2)` with the effect and its card
    /// pushed — the group procedure's own condition, asked directly.
    fn check_procedure_group_condition(&mut self, effect: EffectId, card: CardId) -> bool {
        let Some(condition) = self.effects.get(effect).and_then(|e| e.condition) else {
            // No condition is a condition that holds, as everywhere else.
            return true;
        };
        let ev = Event::new(0);
        let ctx = crate::effect::Ctx {
            reason_effect: effect,
            player: self.cards[card].current.controller,
            event: &ev,
            card: Some(card),
            args: &[],
        };
        condition(self, &ctx)
    }

    /// Case 1: dispatch on the packed answer.
    ///
    /// `ctype` names the list and `sel` indexes it. Every branch but the
    /// last two sets a step and returns, which reaches the case **one
    /// higher** than the number written.
    fn idle_step_1(&mut self, state: &mut IdleCommandState) -> bool {
        let answer = self.core.returns.get() as u32;
        let ctype = answer & 0xffff;
        let sel = (answer >> 16) as usize;
        let tp = self.infos.turn_player;

        match ctype {
            5 => self.idle_activate(sel, tp),
            0 => {
                self.set_step(4);
                false
            }
            1 => {
                self.set_step(5);
                false
            }
            2 => {
                self.set_step(6);
                false
            }
            3 => {
                self.set_step(7);
                false
            }
            4 => {
                self.set_step(8);
                false
            }
            8 => {
                self.set_step(RESTART);
                self.shuffle(tp, location::HAND);
                self.infos.can_shuffle = false;
                false
            }
            // 6 and 7: leaving the phase. The answer itself becomes the
            // phase to change to, which is why `ctype` is stored rather
            // than a `PHASE_*` constant.
            _ => {
                self.set_step(9);
                self.messages.push(crate::field::Message::Hint {
                    kind: crate::host_question::hint::EVENT,
                    player: 1 - tp,
                    value: 23,
                });
                self.core.select_chains.clear();
                self.core.hint_timing[tp as usize] = timing::MAIN_END;
                self.emplace(Kind::QuickEffect {
                    skip_freechain: false,
                    player: 1 - tp,
                    is_opponent: false,
                });
                self.infos.priorities[tp as usize] = 1;
                self.infos.priorities[1 - tp as usize] = 0;
                state.phase_to_change_to = ctype as u8;
                false
            }
        }
    }

    /// `ctype == 5`: the player activated something.
    ///
    /// A continuous effect does not go on the chain at all — it is solved
    /// where it stands, and case 3 picks up afterwards. Everything else
    /// becomes a chain link, and this is where the chain record the gather
    /// left bare is finally filled in.
    fn idle_activate(&mut self, sel: usize, tp: u8) -> bool {
        let Some(chain) = self.core.select_chains.get(sel).cloned() else {
            return true;
        };
        let effect = chain.triggering_effect;
        let is_continuous = self
            .effects
            .get(effect)
            .is_some_and(|e| e.is_type(crate::effect::effect_type::CONTINUOUS));
        if is_continuous {
            let player = self
                .effects
                .get(effect)
                .map_or(PLAYER_NONE, |e| e.get_handler_player(&self.cards));
            self.core.select_chains.clear();
            self.solve_continuous(player, effect, Event::new(code::FREE_CHAIN));
            self.set_step(2);
            return false;
        }

        let Some(handler) = self
            .effects
            .get(effect)
            .and_then(|e| e.get_handler(&self.cards))
        else {
            return true;
        };

        // The event is the effect's own code with everything else blank:
        // an ignition or free-chain activation answers to nothing that
        // happened, so there is no event player, value, card or reason.
        let mut new_chain = chain;
        new_chain.flag = 0;
        new_chain.chain_id = self.next_field_id();
        let mut evt = Event::new(self.effects.get(effect).map_or(0, |e| e.code));
        evt.event_player = PLAYER_NONE;
        evt.event_value = 0;
        evt.event_cards.clear();
        evt.reason = 0;
        evt.reason_effect = None;
        evt.reason_player = PLAYER_NONE;
        new_chain.evt = evt;
        let snapshot = self.cards[handler].state();
        new_chain.set_triggering_state(&snapshot);
        new_chain.triggering_player = tp;

        self.core.new_chains.push_back(new_chain);
        self.cards[handler].set_status(status::CHAINING, true);
        self.dec_count(effect, tp);
        self.core.select_chains.clear();

        self.emplace(Kind::AddChain {
            is_activated_effect: false,
        });
        self.emplace(Kind::QuickEffect {
            skip_freechain: false,
            player: 1 - tp,
            is_opponent: false,
        });
        self.infos.priorities = [0, 0];
        self.core.select_chains.clear();
        false
    }

    /// Case 2: the activation's window has closed, so resolve the chain.
    ///
    /// `STATUS_CHAINING` comes off **every** link, not only the one just
    /// added: by here the whole chain is built and the flag has done its
    /// job of keeping a chaining card from being chained to again.
    fn idle_step_2(&mut self) -> bool {
        self.core.chain_limit.clear();
        let handlers: Vec<CardId> = self
            .core
            .current_chain
            .iter()
            .filter_map(|c| {
                self.effects
                    .get(c.triggering_effect)
                    .and_then(|e| e.get_handler(&self.cards))
            })
            .collect();
        for handler in handlers {
            self.cards[handler].set_status(status::CHAINING, false);
        }
        self.emplace(Kind::SolveChain {
            skip: crate::solve_chain::SolveChainSkip::default(),
        });
        self.set_step(RESTART);
        false
    }

    /// Case 3: a continuous effect was solved in place, so settle the board
    /// and open a window on whatever it did.
    fn idle_step_3(&mut self) -> bool {
        self.adjust_instant();
        self.emplace(Kind::PointEvent {
            skip: crate::point_event::PointEventSkip::default(),
        });
        self.set_step(RESTART);
        false
    }

    /// Case 5: Normal Summon. `summon_cancelable` is set because the player
    /// chose this freely and may back out of the tribute question.
    fn idle_step_5(&mut self) -> bool {
        let Some(target) = self.idle_pick(|c| &c.summonable_cards) else {
            return true;
        };
        self.core.summon_cancelable = true;
        // `zone` is **0x1f**, the five main monster zones — the reference's
        // *default argument*, which this call site leaves off and which the
        // port therefore has to spell out. Zero is the empty mask, and a
        // summon into no permitted zone is filtered out of every procedure
        // and silently does nothing.
        self.summon(
            self.infos.turn_player,
            target,
            None,
            false,
            0,
            DEFAULT_SUMMON_ZONE,
        );
        self.set_step(RESTART);
        false
    }

    /// Case 6: a Special Summon by the card's own procedure.
    fn idle_step_6(&mut self) -> bool {
        let Some(target) = self.idle_pick(|c| &c.spsummonable_cards) else {
            return true;
        };
        self.core.summon_cancelable = true;
        self.special_summon_rule(self.infos.turn_player, target, 0);
        self.set_step(RESTART);
        false
    }

    /// Case 7: change a monster's position — four different things
    /// depending on which way up it currently is.
    ///
    /// Face-up either way is a plain change. Face-**down defence** is a
    /// flip summon, with no question asked. Face-down **attack** is the
    /// awkward one: it could become face-down defence *or* be flip
    /// summoned, so the player is asked, and only the legal answers are
    /// offered.
    ///
    /// `STATUS_FORM_CHANGED` is set on every path including the one that
    /// yields to a question — so a card that is *asked about* has already
    /// spent its position change for the turn.
    fn idle_step_7(&mut self, state: &mut IdleCommandState) -> bool {
        let Some(target) = self.idle_pick(|c| &c.repositionable_cards) else {
            return true;
        };
        let tp = self.infos.turn_player;
        let pos = self.cards[target].current.position;

        if pos == position::FACEUP_ATTACK || pos == position::FACEUP_DEFENSE {
            let to = if pos == position::FACEUP_ATTACK {
                position::FACEUP_DEFENSE
            } else {
                position::FACEUP_ATTACK
            };
            self.core.phase_action = true;
            self.change_position([target], None, tp, to, to, to, to, 0, false);
            self.adjust_all();
            self.emplace(Kind::PointEvent {
                skip: crate::point_event::PointEventSkip::default(),
            });
        } else if pos == position::FACEDOWN_ATTACK {
            state.card_to_reposition = Some(target);
            let mut positions = 0u8;
            if self.is_capable_change_position(target, tp) {
                positions |= position::FACEDOWN_DEFENSE;
            }
            if self.is_can_be_flip_summoned(target, tp) {
                positions |= position::FACEUP_ATTACK;
            }
            let code_ = self.cards[target].data.code;
            self.emplace(Kind::SelectPosition {
                player: tp,
                code: code_,
                positions,
            });
            self.set_step(12);
            return false;
        } else {
            let controller = self.cards[target].current.controller;
            self.emplace(Kind::FlipSummon {
                sumplayer: controller,
                target,
                state: Default::default(),
            });
        }
        self.cards[target].set_status(status::FORM_CHANGED, true);
        self.set_step(RESTART);
        false
    }

    /// Case 8: Set a monster. **The card's own controller** sets it, not
    /// the turn player — they are the same here, and the reference reads
    /// the card.
    fn idle_step_8(&mut self) -> bool {
        let Some(target) = self.idle_pick(|c| &c.msetable_cards) else {
            return true;
        };
        self.core.summon_cancelable = true;
        let controller = self.cards[target].current.controller;
        // 0x1f for the same reason as the Normal Summon above.
        self.mset(controller, target, None, false, 0, DEFAULT_SUMMON_ZONE);
        self.set_step(RESTART);
        false
    }

    /// Case 9: Set a Spell or Trap.
    fn idle_step_9(&mut self) -> bool {
        let Some(target) = self.idle_pick(|c| &c.ssetable_cards) else {
            return true;
        };
        let controller = self.cards[target].current.controller;
        self.emplace(Kind::SpellSet {
            setplayer: controller,
            toplayer: controller,
            target,
            reason_effect: None,
        });
        self.set_step(RESTART);
        false
    }

    /// Case 10: the player announced they are leaving, and the opponent's
    /// last window has closed.
    ///
    /// If the opponent used it, there is a chain to resolve and the phase
    /// is *not* left — back to case 0, where the menu is offered again. The
    /// announcement is undone by the response to it.
    fn idle_step_10(&mut self) -> bool {
        self.core.chain_limit.clear();
        if !self.core.current_chain.is_empty() {
            let handlers: Vec<CardId> = self
                .core
                .current_chain
                .iter()
                .filter_map(|c| {
                    self.effects
                        .get(c.triggering_effect)
                        .and_then(|e| e.get_handler(&self.cards))
                })
                .collect();
            for handler in handlers {
                self.cards[handler].set_status(status::CHAINING, false);
            }
            self.emplace(Kind::SolveChain {
                skip: crate::solve_chain::SolveChainSkip::default(),
            });
            self.set_step(RESTART);
            return false;
        }
        let phase = self.infos.phase;
        self.reset_phase(phase);
        self.adjust_all();
        false
    }

    /// Case 11: the only exit. The phase to change to is handed back in
    /// `returns`, and the hand may be shuffled again next Main Phase.
    fn idle_step_11(&mut self, state: &mut IdleCommandState) -> bool {
        self.core.returns.set(i32::from(state.phase_to_change_to));
        self.infos.can_shuffle = true;
        true
    }

    /// Case 12: the skipped phase's `SelectOption` was answered — option 0
    /// is the Battle Phase, option 1 the End Phase.
    fn idle_step_12(&mut self, state: &mut IdleCommandState) -> bool {
        state.phase_to_change_to = if self.core.returns.get() == 0 { 6 } else { 7 };
        let phase = self.infos.phase;
        self.reset_phase(phase);
        self.adjust_all();
        self.set_step(10);
        false
    }

    /// Case 13: the face-down attack-position monster's question was
    /// answered. Face-up attack is a flip summon; anything else is a plain
    /// turn to face-down defence.
    fn idle_step_13(&mut self, state: &mut IdleCommandState) -> bool {
        let Some(target) = state.card_to_reposition else {
            return true;
        };
        let tp = self.infos.turn_player;
        if self.core.returns.get() == i32::from(position::FACEUP_ATTACK) {
            let controller = self.cards[target].current.controller;
            self.emplace(Kind::FlipSummon {
                sumplayer: controller,
                target,
                state: Default::default(),
            });
        } else {
            self.core.phase_action = true;
            let to = position::FACEDOWN_DEFENSE;
            self.change_position([target], None, tp, to, to, to, to, 0, false);
            self.adjust_all();
            self.emplace(Kind::PointEvent {
                skip: crate::point_event::PointEventSkip::default(),
            });
        }
        self.cards[target].set_status(status::FORM_CHANGED, true);
        self.set_step(RESTART);
        false
    }

    /// `core.<list>[returns.at<int32_t>(0) >> 16]` — the card the answer
    /// named, from the list its `ctype` named.
    ///
    /// `SelectIdleCmd` already validated the index, so the reference
    /// subscripts without checking. Here an out-of-range answer finishes
    /// the unit instead of panicking: a host that reaches this with a bad
    /// index has bypassed the question, and dying is worse than stopping.
    fn idle_pick(&self, list: impl Fn(&crate::field::Core) -> &Vec<CardId>) -> Option<CardId> {
        let index = (self.core.returns.get() as u32 >> 16) as usize;
        list(&self.core).get(index).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{card_type, Card, CardData};
    use crate::duel::phases;
    use crate::effect::{effect_type, Effect};
    use crate::field::Message;
    use crate::processor::Status;

    fn field() -> Field {
        let mut f = Field::new(8000);
        // Turn 2 onward, so the opening turn's missing Battle Phase is not
        // silently doing the work in every test.
        f.infos.turn_id = 3;
        f.infos.phase = phases::MAIN1;
        f
    }

    fn card_at(f: &mut Field, player: u8, loc: u8, seat: u32, type_: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 18036057 + seat,
                type_,
                level: 4,
                ..Default::default()
            },
            player,
        );
        c.current.controller = player;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(player, id, loc, seat, false);
        if loc & location::ONFIELD != 0 {
            f.cards[id].current.position = position::FACEUP_ATTACK;
        }
        id
    }

    fn in_hand(f: &mut Field) -> CardId {
        let seat = f.players[0].hand.len() as u32;
        card_at(f, 0, location::HAND, seat, card_type::MONSTER)
    }

    /// An `EFFECT_FIELD` prohibition aimed at a player.
    fn player_effect(f: &mut Field, code_: u32, player: u8) -> EffectId {
        let anchor = card_at(f, player, location::MZONE, 4, card_type::MONSTER);
        let mut e = Effect::new(effect_type::FIELD, code_);
        e.owner = Some(anchor);
        e.handler = Some(anchor);
        e.flag[0] = crate::effect::flag::PLAYER_TARGET | crate::effect::flag::ABSOLUTE_TARGET;
        e.range = u16::from(location::MZONE);
        e.s_range = 1;
        let id = f.new_effect(e);
        f.add_effect(id, player);
        id
    }

    fn start(f: &mut Field) {
        f.emplace(Kind::IdleCommand {
            state: Default::default(),
        });
    }

    /// Pack an answer the way the host does.
    fn answer(kind: u32, index: u32) -> i32 {
        ((index << 16) | kind) as i32
    }

    /// The zone defaults, as their own group: the reference leaves these
    /// arguments off, so the port is the only place the value is written
    /// down and the only place it can be wrong.
    mod the_zone_defaults {
        use super::*;

        /// Run on, answering the one question a summon asks: where to put
        /// it. Anything else is a setup error and stops the loop.
        fn run_answering_place(f: &mut Field, done: impl Fn(&Field) -> bool) -> bool {
            for _ in 0..512 {
                if done(f) {
                    return true;
                }
                match f.process() {
                    Status::Continue => continue,
                    Status::Awaiting => match f.messages.last() {
                        Some(Message::SelectPlace { player, .. }) => {
                            let p = *player;
                            f.core.returns.set_i8(0, p as i8);
                            f.core.returns.set_i8(1, location::MZONE as i8);
                            f.core.returns.set_i8(2, 0);
                        }
                        other => panic!("unexpected question: {other:?}"),
                    },
                    _ => return done(f),
                }
            }
            panic!("did not settle");
        }

        /// **The Normal Summon reaches the field.**
        ///
        /// The bug this pins was a `zone` of `0` where the reference
        /// defaults to `0x1f`. Zero is the *empty* mask, so every summon
        /// procedure was filtered out for want of a legal seat and
        /// `SummonRule` popped without doing anything — no error, no
        /// message, the menu simply came back. Asserting on the queued
        /// unit would not have caught it; only the card's arrival does.
        #[test]
        fn answering_summon_puts_the_card_in_the_monster_zone() {
            let mut f = field();
            let c = in_hand(&mut f);
            start(&mut f);
            assert_eq!(to_menu(&mut f), Status::Awaiting, "the menu is offered");
            f.core.returns.set(answer(0, 0));
            assert!(
                run_answering_place(&mut f, |f| f.cards[c].current.location == location::MZONE),
                "the summoned card never reached the Monster Zone"
            );
        }

        /// The same for a Set, which takes the same default from the same
        /// declaration.
        #[test]
        fn answering_mset_puts_the_card_in_the_monster_zone() {
            let mut f = field();
            let c = in_hand(&mut f);
            start(&mut f);
            assert_eq!(to_menu(&mut f), Status::Awaiting);
            f.core.returns.set(answer(3, 0));
            assert!(
                run_answering_place(&mut f, |f| f.cards[c].current.location == location::MZONE),
                "the set card never reached the Monster Zone"
            );
        }

        /// And the value itself, so a drift shows up as a failed literal
        /// rather than as a summon that quietly stops working.
        #[test]
        fn the_default_is_the_five_main_monster_zones() {
            assert_eq!(DEFAULT_SUMMON_ZONE, 0x1f);
        }
    }

    /// Run until the menu is offered, or the unit finishes.
    ///
    /// Anything else it stops to ask is a failure: these tests want the
    /// *first* question, and a machine that asks something on the way there
    /// is doing something the test did not set up.
    fn to_menu(f: &mut Field) -> Status {
        for _ in 0..4096 {
            match f.process() {
                Status::Continue => continue,
                Status::Awaiting => return Status::Awaiting,
                other => return other,
            }
        }
        panic!("did not settle");
    }

    /// Every unit waiting to run: the queue and the subunits a handler has
    /// just emplaced, which are spliced in on the next call.
    fn queued(f: &Field, pred: impl Fn(&Kind) -> bool) -> bool {
        f.core
            .units
            .iter()
            .chain(f.core.subunits.iter())
            .any(|u| pred(&u.kind))
    }

    /// Step until the condition holds, or the machine stops.
    fn run_until(f: &mut Field, done: impl Fn(&Field) -> bool) -> bool {
        for _ in 0..512 {
            if done(f) {
                return true;
            }
            if f.process() != Status::Continue {
                return done(f);
            }
        }
        panic!("did not settle");
    }

    fn menu(f: &Field) -> Option<&Message> {
        f.messages
            .iter()
            .rev()
            .find(|m| matches!(m, Message::SelectIdleCmd { .. }))
    }

    fn last_question(f: &Field) -> Option<&Message> {
        f.messages.iter().rev().find(|m| {
            matches!(
                m,
                Message::SelectIdleCmd { .. }
                    | Message::SelectOption { .. }
                    | Message::SelectPosition { .. }
                    | Message::SelectPlace { .. }
                    | Message::SelectChain { .. }
                    | Message::SelectYesNo { .. }
            )
        })
    }

    /// The shape of the whole unit: it offers a menu, and a command that is
    /// not "leave" brings the menu back rather than finishing.
    #[test]
    fn it_offers_a_menu_and_loops() {
        let mut f = field();
        let c = in_hand(&mut f);
        start(&mut f);
        assert_eq!(to_menu(&mut f), Status::Awaiting);
        let Some(Message::SelectIdleCmd { summonable, .. }) = menu(&f) else {
            panic!("no menu");
        };
        assert_eq!(summonable.len(), 1, "the card in hand is offered");
        assert_eq!(summonable[0].code, f.cards[c].data.code);

        let before = f.messages.len();
        f.core.returns.set(answer(8, 0));
        // Shuffling is refused (one card), so the menu comes straight back.
        assert_eq!(to_menu(&mut f), Status::Awaiting);
        assert!(f.messages.len() > before, "asked again");
    }

    /// **Leaving is the only way out**, and the answer is handed back in
    /// `returns` as the `ctype` the player sent — 6 for the Battle Phase,
    /// 7 for the End Phase.
    #[test]
    fn leaving_is_the_only_exit_and_returns_the_destination() {
        for ctype in [6u32, 7] {
            let mut f = field();
            start(&mut f);
            to_menu(&mut f);
            f.core.returns.set(answer(ctype, 0));
            // The opponent's last window opens and is declined.
            loop {
                match f.process() {
                    Status::Continue => continue,
                    Status::Awaiting => match f.messages.last() {
                        Some(Message::SelectChain { .. }) => f.core.returns.set(-1),
                        other => panic!("unexpected question: {other:?}"),
                    },
                    Status::End => break,
                }
            }
            assert_eq!(f.core.returns.get(), ctype as i32);
            assert!(f.infos.can_shuffle, "the hand may be shuffled again");
        }
    }

    mod leaving_the_phase {
        use super::*;

        /// `EFFECT_CANNOT_BP` takes the Battle Phase off the menu.
        #[test]
        fn cannot_bp_withdraws_the_battle_option() {
            let mut f = field();
            player_effect(&mut f, code::CANNOT_BP, 0);
            start(&mut f);
            to_menu(&mut f);
            let Some(Message::SelectIdleCmd { to_bp, to_ep, .. }) = menu(&f) else {
                panic!("no menu");
            };
            assert!(!to_bp);
            assert!(to_ep, "and the End Phase is still open");
        }

        /// **Main Phase 2 has no Battle Phase after it**, whatever else is
        /// true.
        #[test]
        fn main_phase_2_has_no_battle_after_it() {
            let mut f = field();
            f.infos.phase = phases::MAIN2;
            start(&mut f);
            to_menu(&mut f);
            let Some(Message::SelectIdleCmd { to_bp, .. }) = menu(&f) else {
                panic!("no menu");
            };
            assert!(!to_bp);
        }

        /// **The opening turn has no Battle Phase**, and
        /// `EFFECT_BP_FIRST_TURN` gives it back.
        #[test]
        fn the_opening_turn_has_no_battle_unless_something_grants_it() {
            let mut f = field();
            f.infos.turn_id = 1;
            start(&mut f);
            to_menu(&mut f);
            let Some(Message::SelectIdleCmd { to_bp, .. }) = menu(&f) else {
                panic!("no menu");
            };
            assert!(!to_bp, "turn 1");

            let mut f = field();
            f.infos.turn_id = 1;
            player_effect(&mut f, code::BP_FIRST_TURN, 0);
            start(&mut f);
            to_menu(&mut f);
            let Some(Message::SelectIdleCmd { to_bp, .. }) = menu(&f) else {
                panic!("no menu");
            };
            assert!(to_bp, "granted back");
        }

        /// **A monster that must attack takes the End Phase away** — but
        /// only one that *could* attack. The two conditions are read
        /// together, and a monster under `MUST_ATTACK` which cannot attack
        /// does not trap its controller.
        #[test]
        fn must_attack_closes_the_end_phase_only_if_it_can_attack() {
            let mut f = field();
            let m = card_at(&mut f, 0, location::MZONE, 0, card_type::MONSTER);
            let mut e = Effect::new(effect_type::SINGLE, code::MUST_ATTACK);
            e.owner = Some(m);
            e.handler = Some(m);
            let id = f.new_effect(e);
            f.cards[m].single_effect.insert(code::MUST_ATTACK, id);
            f.cards[m].indexer.insert(id);
            start(&mut f);
            to_menu(&mut f);
            let Some(Message::SelectIdleCmd { to_bp, to_ep, .. }) = menu(&f) else {
                panic!("no menu");
            };
            assert!(to_bp);
            assert!(!to_ep, "obliged to attack, so the turn may not be ended");

            // The same monster face-down cannot attack, so it obliges
            // nothing.
            let mut f = field();
            let m = card_at(&mut f, 0, location::MZONE, 0, card_type::MONSTER);
            f.cards[m].current.position = position::FACEDOWN_DEFENSE;
            let mut e = Effect::new(effect_type::SINGLE, code::MUST_ATTACK);
            e.owner = Some(m);
            e.handler = Some(m);
            let id = f.new_effect(e);
            f.cards[m].single_effect.insert(code::MUST_ATTACK, id);
            f.cards[m].indexer.insert(id);
            start(&mut f);
            to_menu(&mut f);
            let Some(Message::SelectIdleCmd { to_ep, .. }) = menu(&f) else {
                panic!("no menu");
            };
            assert!(to_ep, "it cannot attack, so it obliges nothing");
        }

        /// A monster under `EFFECT_MUST_ATTACK`, face-up in attack
        /// position so that it can actually attack.
        fn obliged_attacker(f: &mut Field) -> CardId {
            let m = card_at(f, 0, location::MZONE, 0, card_type::MONSTER);
            let mut e = Effect::new(effect_type::SINGLE, code::MUST_ATTACK);
            e.owner = Some(m);
            e.handler = Some(m);
            let id = f.new_effect(e);
            f.cards[m].single_effect.insert(code::MUST_ATTACK, id);
            f.cards[m].indexer.insert(id);
            m
        }

        /// **`EFFECT_CANNOT_EP` closes the End Phase on its own**, with no
        /// monster involved — the other half of the same disjunction.
        #[test]
        fn cannot_ep_closes_the_end_phase() {
            let mut f = field();
            player_effect(&mut f, code::CANNOT_EP, 0);
            start(&mut f);
            to_menu(&mut f);
            let Some(Message::SelectIdleCmd { to_bp, to_ep, .. }) = menu(&f) else {
                panic!("no menu");
            };
            assert!(to_bp, "there is a battle to go to");
            assert!(!to_ep, "so the turn may not simply be ended");
        }

        /// **The End Phase is only closed while a Battle Phase is
        /// available.** A player obliged to attack but with no Battle Phase
        /// to attack in may still end the turn — otherwise they would have
        /// no legal command at all.
        #[test]
        fn the_end_phase_stays_open_when_there_is_no_battle() {
            let mut f = field();
            obliged_attacker(&mut f);
            player_effect(&mut f, code::CANNOT_BP, 0);
            start(&mut f);
            to_menu(&mut f);
            let Some(Message::SelectIdleCmd { to_bp, to_ep, .. }) = menu(&f) else {
                panic!("no menu");
            };
            assert!(!to_bp);
            assert!(to_ep, "or the player would be stuck");
        }

        /// **`must_attack` is only read from Main Phase 1.** In Main Phase
        /// 2 there is no Battle Phase to be obliged into, and the guard is
        /// the phase test rather than `to_bp`.
        #[test]
        fn must_attack_is_not_read_from_main_phase_2() {
            let mut f = field();
            f.infos.phase = phases::MAIN2;
            let m = card_at(&mut f, 0, location::MZONE, 0, card_type::MONSTER);
            let mut e = Effect::new(effect_type::SINGLE, code::MUST_ATTACK);
            e.owner = Some(m);
            e.handler = Some(m);
            let id = f.new_effect(e);
            f.cards[m].single_effect.insert(code::MUST_ATTACK, id);
            f.cards[m].indexer.insert(id);
            start(&mut f);
            to_menu(&mut f);
            let Some(Message::SelectIdleCmd { to_ep, .. }) = menu(&f) else {
                panic!("no menu");
            };
            assert!(to_ep);
        }

        /// **A skipped phase offers no menu at all.** With both ways out
        /// open the player is asked which by `SelectOption`, and the answer
        /// picks the destination.
        #[test]
        fn a_skipped_phase_asks_which_way_out_instead_of_offering_a_menu() {
            for (option, expected) in [(0i32, 6i32), (1, 7)] {
                let mut f = field();
                in_hand(&mut f);
                player_effect(&mut f, code::SKIP_M1, 0);
                start(&mut f);
                to_menu(&mut f);
                assert!(
                    matches!(last_question(&f), Some(Message::SelectOption { .. })),
                    "asked which way out, not offered a menu"
                );
                assert!(menu(&f).is_none(), "and never offered the menu");

                f.core.returns.set(option);
                loop {
                    match f.process() {
                        Status::Continue => continue,
                        Status::Awaiting => match f.messages.last() {
                            Some(Message::SelectChain { .. }) => f.core.returns.set(-1),
                            other => panic!("unexpected: {other:?}"),
                        },
                        Status::End => break,
                    }
                }
                assert_eq!(f.core.returns.get(), expected);
            }
        }

        /// With only one way out of a skipped phase there is nothing to
        /// ask, and the unit leaves without a question.
        #[test]
        fn a_skipped_phase_with_one_way_out_asks_nothing() {
            let mut f = field();
            f.infos.phase = phases::MAIN2; // so `to_bp` is already false
            player_effect(&mut f, code::SKIP_M2, 0);
            start(&mut f);
            assert_eq!(to_menu(&mut f), Status::End);
            assert!(last_question(&f).is_none(), "nothing was asked");
            assert_eq!(f.core.returns.get(), 7, "the End Phase");
        }

        /// **`core.skip_m2` is a one-shot**, spent as it is read — unlike
        /// `EFFECT_SKIP_M2`, which is a standing effect.
        #[test]
        fn skip_m2_is_spent_as_it_is_read() {
            let mut f = field();
            f.infos.phase = phases::MAIN2;
            f.core.skip_m2 = true;
            start(&mut f);
            loop {
                match f.process() {
                    Status::Continue => continue,
                    Status::Awaiting => match f.messages.last() {
                        Some(Message::SelectChain { .. }) => f.core.returns.set(-1),
                        other => panic!("unexpected: {other:?}"),
                    },
                    Status::End => break,
                }
            }
            assert_eq!(f.core.returns.get(), 7, "it left for the End Phase");
            assert!(!f.core.skip_m2, "and the flag is spent");
            assert!(menu(&f).is_none(), "without ever offering the menu");
        }
    }

    mod acting {
        use super::*;

        /// Command 0 summons the card the index named.
        #[test]
        fn the_summon_command_summons_that_card() {
            let mut f = field();
            let a = in_hand(&mut f);
            let b = in_hand(&mut f);
            start(&mut f);
            to_menu(&mut f);
            let Some(Message::SelectIdleCmd { summonable, .. }) = menu(&f) else {
                panic!("no menu");
            };
            assert_eq!(summonable.len(), 2);
            // Index 1 is the second card, and the list is in hand order.
            assert_eq!(summonable[1].code, f.cards[b].data.code);

            f.core.returns.set(answer(0, 1));
            assert!(
                run_until(&mut f, |f| queued(f, |k| {
                    matches!(k, Kind::SummonRule { target, .. } if *target == b)
                })),
                "the second card, not the first"
            );
            assert_ne!(a, b);
            assert!(
                f.core.summon_cancelable,
                "a freely chosen summon may be backed out of"
            );
        }

        /// Command 3 sets a monster, and command 4 sets a Spell — each from
        /// its own list, so the same index means different cards.
        #[test]
        fn the_two_set_commands_read_different_lists() {
            let mut f = field();
            let monster = in_hand(&mut f);
            let spell = card_at(&mut f, 0, location::HAND, 1, card_type::SPELL);
            start(&mut f);
            to_menu(&mut f);
            let Some(Message::SelectIdleCmd {
                msetable, ssetable, ..
            }) = menu(&f)
            else {
                panic!("no menu");
            };
            assert_eq!(msetable.len(), 1);
            assert_eq!(msetable[0].code, f.cards[monster].data.code);
            assert_eq!(ssetable.len(), 1);
            assert_eq!(ssetable[0].code, f.cards[spell].data.code);

            f.core.returns.set(answer(4, 0));
            assert!(
                run_until(&mut f, |f| queued(f, |k| {
                    matches!(k, Kind::SpellSet { target, .. } if *target == spell)
                })),
                "the spell, from the spell list"
            );
        }

        /// **A face-up monster is turned without a question**, and spends
        /// its position change for the turn.
        #[test]
        fn a_face_up_monster_is_turned_outright() {
            let mut f = field();
            let m = card_at(&mut f, 0, location::MZONE, 0, card_type::MONSTER);
            start(&mut f);
            to_menu(&mut f);
            let Some(Message::SelectIdleCmd { repositionable, .. }) = menu(&f) else {
                panic!("no menu");
            };
            assert_eq!(repositionable.len(), 1);

            f.core.returns.set(answer(2, 0));
            run_until(&mut f, |f| {
                f.cards[m].current.position == position::FACEUP_DEFENSE
            });
            assert_eq!(f.cards[m].current.position, position::FACEUP_DEFENSE);
            assert!(f.cards[m].is_status(status::FORM_CHANGED));
            assert!(f.core.phase_action);
        }

        /// **A face-down defence monster is flip summoned**, with nothing
        /// asked — there is only one thing it could mean.
        #[test]
        fn a_face_down_defence_monster_is_flip_summoned() {
            let mut f = field();
            let m = card_at(&mut f, 0, location::MZONE, 0, card_type::MONSTER);
            f.cards[m].current.position = position::FACEDOWN_DEFENSE;
            start(&mut f);
            to_menu(&mut f);
            f.core.returns.set(answer(2, 0));
            assert!(
                run_until(&mut f, |f| queued(f, |k| {
                    matches!(k, Kind::FlipSummon { target, .. } if *target == m)
                })),
                "a flip summon, not a position change"
            );
        }

        /// **A face-down attack monster is asked which was meant**, and
        /// both answers are offered because both are legal. Answering
        /// face-up attack flip summons it; anything else turns it to
        /// face-down defence.
        #[test]
        fn a_face_down_attack_monster_is_asked_which_was_meant() {
            let mut f = field();
            let m = card_at(&mut f, 0, location::MZONE, 0, card_type::MONSTER);
            f.cards[m].current.position = position::FACEDOWN_ATTACK;
            start(&mut f);
            to_menu(&mut f);
            f.core.returns.set(answer(2, 0));
            assert_eq!(to_menu(&mut f), Status::Awaiting);
            let Some(Message::SelectPosition { positions, .. }) = last_question(&f) else {
                panic!("expected a position question, got {:?}", last_question(&f));
            };
            assert_eq!(
                *positions,
                position::FACEDOWN_DEFENSE | position::FACEUP_ATTACK,
                "both answers are legal, so both are offered"
            );

            f.core.returns.set(i32::from(position::FACEUP_ATTACK));
            assert!(
                run_until(&mut f, |f| queued(f, |k| {
                    matches!(k, Kind::FlipSummon { target, .. } if *target == m)
                })),
                "face-up attack means a flip summon"
            );
            assert!(f.cards[m].is_status(status::FORM_CHANGED));
        }

        /// The other answer to the same question.
        #[test]
        fn the_other_answer_turns_it_face_down_defence() {
            let mut f = field();
            let m = card_at(&mut f, 0, location::MZONE, 0, card_type::MONSTER);
            f.cards[m].current.position = position::FACEDOWN_ATTACK;
            start(&mut f);
            to_menu(&mut f);
            f.core.returns.set(answer(2, 0));
            to_menu(&mut f);
            f.core.returns.set(i32::from(position::FACEDOWN_DEFENSE));
            run_until(&mut f, |f| {
                f.cards[m].current.position == position::FACEDOWN_DEFENSE
            });
            assert_eq!(f.cards[m].current.position, position::FACEDOWN_DEFENSE);
            assert!(f.cards[m].is_status(status::FORM_CHANGED));
        }

        /// **Shuffling is spent once per Main Phase.** `infos.can_shuffle`
        /// goes down on use and is restored only on the way out of the
        /// phase.
        #[test]
        fn shuffling_is_spent_once_and_restored_on_the_way_out() {
            let mut f = field();
            in_hand(&mut f);
            in_hand(&mut f);
            start(&mut f);
            to_menu(&mut f);
            let Some(Message::SelectIdleCmd { can_shuffle, .. }) = menu(&f) else {
                panic!("no menu");
            };
            assert!(can_shuffle);

            f.core.returns.set(answer(8, 0));
            to_menu(&mut f);
            assert!(
                f.messages
                    .iter()
                    .any(|m| matches!(m, Message::ShuffleHand { player: 0, .. })),
                "the hand was actually shuffled"
            );
            assert!(!f.infos.can_shuffle, "spent");
            let Some(Message::SelectIdleCmd { can_shuffle, .. }) = menu(&f) else {
                panic!("no menu");
            };
            assert!(!can_shuffle, "and not offered again");

            f.core.returns.set(answer(7, 0));
            loop {
                match f.process() {
                    Status::Continue => continue,
                    Status::Awaiting => match f.messages.last() {
                        Some(Message::SelectChain { .. }) => f.core.returns.set(-1),
                        other => panic!("unexpected: {other:?}"),
                    },
                    Status::End => break,
                }
            }
            assert!(f.infos.can_shuffle, "restored on the way out");
        }
    }

    mod activating {
        use super::*;

        /// An ignition effect on a face-up Spell in the row.
        ///
        /// **Not** an `EFFECT_TYPE_ACTIVATE`: that is the activation of the
        /// card itself, and a card already face-up on the field has done
        /// that. An ignition effect is the thing a face-up card offers, and
        /// it reaches the same case-1 path.
        fn activatable(f: &mut Field, seat: u32) -> (CardId, EffectId) {
            let c = card_at(f, 0, location::SZONE, seat, card_type::SPELL);
            let mut e = Effect::new(
                effect_type::FIELD | effect_type::ACTIONS | effect_type::IGNITION,
                0,
            );
            e.owner = Some(c);
            e.handler = Some(c);
            e.range = u16::from(location::SZONE);
            e.description = 7;
            let id = f.new_effect(e);
            f.add_effect(id, 0);
            (c, id)
        }

        /// Activating an effect: it becomes a **new chain**, with everything
        /// the bare gather left blank now filled in, and the opponent is
        /// given a window before it resolves.
        #[test]
        fn an_activation_becomes_a_chain_with_its_record_filled_in() {
            let mut f = field();
            let (card, effect) = activatable(&mut f, 0);
            start(&mut f);
            to_menu(&mut f);

            f.core.returns.set(answer(5, 0));
            assert!(run_until(&mut f, |f| !f.core.new_chains.is_empty()));
            let chain = &f.core.new_chains[0];
            assert_eq!(chain.triggering_effect, effect);
            assert_ne!(chain.chain_id, 0, "given an id at selection time");
            assert_eq!(chain.triggering_player, 0, "the turn player activated it");
            assert_eq!(
                chain.evt.event_code,
                f.effects.get(effect).map_or(0, |e| e.code),
                "the event is the effect's own code"
            );
            assert_eq!(
                chain.evt.event_player, PLAYER_NONE,
                "answering to nothing that happened"
            );
            assert!(chain.evt.event_cards.is_empty());
            assert!(chain.evt.reason_effect.is_none());

            assert!(
                f.cards[card].is_status(status::CHAINING),
                "the card is chaining"
            );
            assert!(
                queued(&f, |k| matches!(k, Kind::AddChain { .. })),
                "AddChain"
            );
            assert!(
                queued(&f, |k| matches!(k, Kind::QuickEffect { player: 1, .. })),
                "and a window for the opponent"
            );
            assert_eq!(f.infos.priorities, [0, 0]);
            assert!(f.core.select_chains.is_empty(), "the offer list is cleared");
        }

        /// **The triggering player is the one who activated**, which for
        /// this unit is always the turn player.
        #[test]
        fn the_triggering_player_is_the_turn_player() {
            let mut f = field();
            activatable(&mut f, 0);
            f.infos.turn_player = 0;
            start(&mut f);
            to_menu(&mut f);
            f.core.returns.set(answer(5, 0));
            run_until(&mut f, |f| !f.core.new_chains.is_empty());
            assert_eq!(f.core.new_chains[0].triggering_player, 0);
        }

        /// **The activation count is spent.** A once-per-turn effect is
        /// used up by being chosen here.
        #[test]
        fn the_activation_count_is_spent() {
            let mut f = field();
            let (_, effect) = activatable(&mut f, 0);
            if let Some(e) = f.effects.get_mut(effect) {
                e.flag[0] |= crate::effect::flag::COUNT_LIMIT;
                e.count_limit = 1;
                e.count_limit_max = 1;
            }
            start(&mut f);
            to_menu(&mut f);
            f.core.returns.set(answer(5, 0));
            run_until(&mut f, |f| !f.core.new_chains.is_empty());
            assert_eq!(
                f.effects.get(effect).map(|e| e.count_limit),
                Some(0),
                "spent"
            );
        }

        /// **A continuous effect is solved where it stands.** It never
        /// becomes a chain link; the board is settled and a window opened
        /// afterwards instead.
        #[test]
        fn a_continuous_activation_is_solved_in_place() {
            let mut f = field();
            let c = card_at(&mut f, 0, location::SZONE, 0, card_type::SPELL);
            let mut e = Effect::new(
                effect_type::FIELD | effect_type::ACTIONS | effect_type::CONTINUOUS,
                code::FREE_CHAIN,
            );
            e.owner = Some(c);
            e.handler = Some(c);
            e.range = u16::from(location::SZONE);
            e.s_range = 1;
            e.o_range = 1;
            let id = f.new_effect(e);
            f.add_effect(id, 0);

            start(&mut f);
            to_menu(&mut f);
            f.core.returns.set(answer(5, 0));
            assert!(run_until(&mut f, |f| queued(f, |k| matches!(
                k,
                Kind::PointEvent { .. }
            ))));
            assert!(
                f.core.new_chains.is_empty(),
                "a continuous effect does not go on the chain"
            );
            assert!(
                !f.cards[c].is_status(status::CHAINING),
                "and nothing is marked chaining"
            );
        }

        /// **`is_activateable` is consulted.** An effect whose condition
        /// fails is not offered at all.
        #[test]
        fn an_effect_whose_condition_fails_is_not_offered() {
            let mut f = field();
            let (_, effect) = activatable(&mut f, 0);
            if let Some(e) = f.effects.get_mut(effect) {
                e.condition = Some(|_, _| false);
            }
            start(&mut f);
            to_menu(&mut f);
            let Some(Message::SelectIdleCmd { activatable, .. }) = menu(&f) else {
                panic!("no menu");
            };
            assert!(activatable.is_empty(), "the condition refused it");
        }

        /// **Case 2 takes `STATUS_CHAINING` off every link** — not only the
        /// one just added — and hands the chain to `SolveChain`.
        ///
        /// Entered directly: driving here from the menu goes through
        /// `AddChain`, which reaches `check_chain_counter` and that is not
        /// ported. The case itself is what is under test.
        #[test]
        fn the_chain_is_released_and_solved_after_the_window() {
            let mut f = field();
            let (first, e1) = activatable(&mut f, 0);
            let (second, e2) = activatable(&mut f, 1);
            for c in [first, second] {
                f.cards[c].set_status(status::CHAINING, true);
            }
            for e in [e1, e2] {
                f.core
                    .current_chain
                    .push(Chain::new(e, Event::new(code::FREE_CHAIN)));
            }
            f.core.chain_limit.push(e1);

            f.emplace_at(
                Kind::IdleCommand {
                    state: Default::default(),
                },
                2,
            );
            f.process();
            assert!(
                queued(&f, |k| matches!(k, Kind::SolveChain { .. })),
                "SolveChain was emplaced"
            );
            assert!(
                f.core.chain_limit.is_empty(),
                "and the chain limits were dropped"
            );
            for c in [first, second] {
                assert!(
                    !f.cards[c].is_status(status::CHAINING),
                    "every link is released, not just the last"
                );
            }
        }
    }

    mod the_way_out {
        use super::*;

        /// **Announcing a departure opens a window for the opponent**, with
        /// the Main-End timing so they know why they are being asked.
        #[test]
        fn announcing_a_departure_opens_the_opponents_window() {
            let mut f = field();
            start(&mut f);
            to_menu(&mut f);
            f.core.returns.set(answer(7, 0));
            assert!(run_until(&mut f, |f| queued(f, |k| matches!(
                k,
                Kind::QuickEffect { player: 1, .. }
            ))));
            assert_ne!(
                f.core.hint_timing[0] & timing::MAIN_END,
                0,
                "the Main-End timing hint"
            );
            assert_eq!(f.infos.priorities, [1, 0], "the turn player keeps priority");
        }

        /// **A response undoes the announcement.** Case 10 finds a chain
        /// standing, resolves it, and restarts the unit — so the menu comes
        /// back and the phase continues rather than ending.
        #[test]
        fn a_response_brings_the_menu_back_instead_of_leaving() {
            let mut f = field();
            start(&mut f);
            to_menu(&mut f);
            let menus_before = f
                .messages
                .iter()
                .filter(|m| matches!(m, Message::SelectIdleCmd { .. }))
                .count();

            f.core.returns.set(answer(7, 0));
            assert!(run_until(&mut f, |f| queued(f, |k| matches!(
                k,
                Kind::QuickEffect { .. }
            ))));
            // Stand in for the opponent having used their window: drop it
            // and leave a chain where it would have put one.
            f.core
                .subunits
                .retain(|u| !matches!(u.kind, Kind::QuickEffect { .. }));
            let c = card_at(&mut f, 1, location::SZONE, 0, card_type::TRAP);
            let mut e = Effect::new(effect_type::FIELD | effect_type::ACTIONS, 0);
            e.owner = Some(c);
            e.handler = Some(c);
            let id = f.new_effect(e);
            f.core
                .current_chain
                .push(Chain::new(id, Event::new(code::FREE_CHAIN)));

            assert!(
                run_until(&mut f, |f| queued(f, |k| matches!(
                    k,
                    Kind::SolveChain { .. }
                ))),
                "the chain is resolved rather than the phase left"
            );
            // Let the machine carry on as though the chain had resolved.
            f.core.current_chain.clear();
            f.core
                .subunits
                .retain(|u| !matches!(u.kind, Kind::SolveChain { .. }));
            assert_eq!(to_menu(&mut f), Status::Awaiting);
            let menus_after = f
                .messages
                .iter()
                .filter(|m| matches!(m, Message::SelectIdleCmd { .. }))
                .count();
            assert!(menus_after > menus_before, "the menu came back");
        }

        /// **A forced attack pre-empts the phase entirely** — no lists, no
        /// menu, straight to `ForcedBattle`, and the flag is spent.
        ///
        /// `ForcedBattle` is not ported, so this asserts the unit is
        /// *queued* rather than running it.
        #[test]
        fn a_forced_attack_pre_empts_the_phase() {
            let mut f = field();
            in_hand(&mut f);
            f.core.set_forced_attack = true;
            start(&mut f);
            f.process();
            assert!(
                queued(&f, |k| matches!(k, Kind::ForcedBattle { .. })),
                "straight to the battle"
            );
            assert!(!f.core.set_forced_attack, "and the flag is spent");
            assert!(menu(&f).is_none(), "no menu was offered");
        }
    }

    mod the_gather {
        use super::*;

        /// An ignition effect is offered, and the list it lands in is the
        /// menu's activate list.
        #[test]
        fn an_ignition_effect_is_offered() {
            let mut f = field();
            let c = card_at(&mut f, 0, location::SZONE, 0, card_type::SPELL);
            let mut e = Effect::new(
                effect_type::FIELD | effect_type::ACTIONS | effect_type::IGNITION,
                0,
            );
            e.owner = Some(c);
            e.handler = Some(c);
            e.range = u16::from(location::SZONE);
            e.description = 99;
            let id = f.new_effect(e);
            f.add_effect(id, 0);

            start(&mut f);
            to_menu(&mut f);
            let Some(Message::SelectIdleCmd { activatable, .. }) = menu(&f) else {
                panic!("no menu");
            };
            assert_eq!(activatable.len(), 1);
            assert_eq!(activatable[0].description, 99);
        }

        /// **The offered chain is bare.** Unlike `PhaseEvent`'s gather, the
        /// chains put in `select_chains` here carry only their effect — no
        /// id, no triggering player. Case 1 fills those in on the one the
        /// player picks.
        #[test]
        fn the_offered_chain_carries_only_its_effect() {
            let mut f = field();
            let c = card_at(&mut f, 0, location::SZONE, 0, card_type::SPELL);
            let mut e = Effect::new(
                effect_type::FIELD | effect_type::ACTIONS | effect_type::IGNITION,
                0,
            );
            e.owner = Some(c);
            e.handler = Some(c);
            e.range = u16::from(location::SZONE);
            let id = f.new_effect(e);
            f.add_effect(id, 0);

            start(&mut f);
            to_menu(&mut f);
            let offered = &f.core.select_chains[0];
            assert_eq!(offered.triggering_effect, id);
            assert_eq!(offered.chain_id, 0, "no id yet");
            assert_eq!(
                offered.triggering_player, PLAYER_NONE,
                "and no triggering player"
            );
        }

        /// **The continuous registry is filtered to the turn player's own
        /// effects** — the only one of the four that is.
        #[test]
        fn a_continuous_effect_of_the_opponents_is_not_offered() {
            for (owner, offered) in [(0u8, 1usize), (1, 0)] {
                let mut f = field();
                let c = card_at(&mut f, owner, location::SZONE, 0, card_type::SPELL);
                let mut e = Effect::new(
                    effect_type::FIELD | effect_type::ACTIONS | effect_type::CONTINUOUS,
                    code::FREE_CHAIN,
                );
                e.owner = Some(c);
                e.handler = Some(c);
                e.range = u16::from(location::SZONE);
                e.s_range = 1;
                e.o_range = 1;
                let id = f.new_effect(e);
                f.add_effect(id, owner);

                start(&mut f);
                to_menu(&mut f);
                let Some(Message::SelectIdleCmd { activatable, .. }) = menu(&f) else {
                    panic!("no menu");
                };
                assert_eq!(
                    activatable.len(),
                    offered,
                    "a continuous effect owned by player {owner}"
                );
            }
        }

        /// A card with a working `EFFECT_SPSUMMON_PROC`, so it belongs in
        /// the special-summon list.
        fn with_procedure(f: &mut Field, player: u8, seat: u32) -> (CardId, EffectId) {
            let c = card_at(f, player, location::HAND, seat, card_type::MONSTER);
            let mut e = Effect::new(effect_type::FIELD, code::SPSUMMON_PROC);
            e.owner = Some(c);
            e.handler = Some(c);
            e.range = u16::from(location::HAND);
            e.condition = Some(|_, _| true);
            let id = f.new_effect(e);
            f.cards[c].field_effect.insert(code::SPSUMMON_PROC, id);
            f.cards[c].indexer.insert(id);
            f.add_effect(id, player);
            (c, id)
        }

        #[test]
        fn a_card_with_a_procedure_is_offered() {
            let mut f = field();
            let (c, _) = with_procedure(&mut f, 0, 0);
            start(&mut f);
            to_menu(&mut f);
            let Some(Message::SelectIdleCmd { spsummonable, .. }) = menu(&f) else {
                panic!("no menu");
            };
            assert_eq!(spsummonable.len(), 1);
            assert_eq!(spsummonable[0].code, f.cards[c].data.code);
        }

        /// **A spent count limit takes the card off the list** — and does
        /// so *before* the card's summon cost is asked.
        ///
        /// The card coming off the list is not by itself evidence that this
        /// gather read the count: `filter_spsummon_procedure`, inside
        /// `is_special_summonable`, checks the same count again and would
        /// withdraw it anyway. What the outer check buys is ordering, and a
        /// counting cost is the only instrument that sees it.
        #[test]
        fn a_spent_procedure_count_withdraws_the_card_before_the_cost() {
            use std::sync::atomic::{AtomicU32, Ordering};
            static ASKED: AtomicU32 = AtomicU32::new(0);

            fn counting_cost(f: &mut Field, card: CardId) {
                let mut e = Effect::new(effect_type::SINGLE, code::SPSUMMON_COST);
                e.owner = Some(card);
                e.handler = Some(card);
                e.cost = Some(|_, _, _| {
                    ASKED.fetch_add(1, Ordering::SeqCst);
                    true
                });
                let id = f.new_effect(e);
                f.cards[card].single_effect.insert(code::SPSUMMON_COST, id);
                f.cards[card].indexer.insert(id);
            }

            // The baseline: with the count available, the cost is asked.
            let mut f = field();
            let (c, _) = with_procedure(&mut f, 0, 0);
            counting_cost(&mut f, c);
            ASKED.store(0, Ordering::SeqCst);
            start(&mut f);
            to_menu(&mut f);
            let Some(Message::SelectIdleCmd { spsummonable, .. }) = menu(&f) else {
                panic!("no menu");
            };
            assert_eq!(spsummonable.len(), 1);
            assert_ne!(ASKED.load(Ordering::SeqCst), 0, "the cost was asked");

            let mut f = field();
            let (c, e) = with_procedure(&mut f, 0, 0);
            counting_cost(&mut f, c);
            if let Some(x) = f.effects.get_mut(e) {
                x.flag[0] |= crate::effect::flag::COUNT_LIMIT;
                x.count_limit = 0;
                x.count_limit_max = 1;
            }
            ASKED.store(0, Ordering::SeqCst);
            start(&mut f);
            to_menu(&mut f);
            let Some(Message::SelectIdleCmd { spsummonable, .. }) = menu(&f) else {
                panic!("no menu");
            };
            assert!(spsummonable.is_empty(), "the count is spent");
            assert_eq!(
                ASKED.load(Ordering::SeqCst),
                0,
                "and the count was read above the cost, so the cost never ran"
            );
        }

        /// **The opponent's cards are not offered**, however summonable
        /// they are.
        #[test]
        fn the_opponents_cards_are_not_offered() {
            let mut f = field();
            with_procedure(&mut f, 1, 0);
            start(&mut f);
            to_menu(&mut f);
            let Some(Message::SelectIdleCmd { spsummonable, .. }) = menu(&f) else {
                panic!("no menu");
            };
            assert!(spsummonable.is_empty(), "not the turn player's card");
        }

        /// **The summon list walks the Monster Zones as well as the hand.**
        /// That is the Gemini re-summon, and it is easy to read the second
        /// loop as a copy-paste slip.
        #[test]
        fn the_summon_list_walks_the_field_too() {
            let mut f = field();
            in_hand(&mut f);
            let m = card_at(&mut f, 0, location::MZONE, 0, card_type::MONSTER);
            // A Gemini monster on the field is summonable again: it needs
            // `EFFECT_GEMINI_SUMMONABLE` present and `GEMINI_STATUS`
            // absent — it has not yet gained its effect.
            let mut e = Effect::new(effect_type::SINGLE, code::GEMINI_SUMMONABLE);
            e.owner = Some(m);
            e.handler = Some(m);
            let id = f.new_effect(e);
            f.cards[m].single_effect.insert(code::GEMINI_SUMMONABLE, id);
            f.cards[m].indexer.insert(id);
            f.cards[m].data.type_ |= card_type::GEMINI;

            start(&mut f);
            to_menu(&mut f);
            let Some(Message::SelectIdleCmd { summonable, .. }) = menu(&f) else {
                panic!("no menu");
            };
            assert_eq!(summonable.len(), 2, "the hand card and the Gemini");
            assert_eq!(
                summonable[1].location,
                location::MZONE,
                "and the field one comes second, because the hand is walked first"
            );
        }

        /// **The lists are rebuilt from scratch on every pass.** A card
        /// that stops being summonable between two menus is gone from the
        /// second.
        #[test]
        fn the_lists_are_rebuilt_every_pass() {
            let mut f = field();
            let c = in_hand(&mut f);
            in_hand(&mut f);
            f.infos.can_shuffle = true;
            start(&mut f);
            to_menu(&mut f);
            let Some(Message::SelectIdleCmd { summonable, .. }) = menu(&f) else {
                panic!("no menu");
            };
            assert_eq!(summonable.len(), 2);

            f.cards[c].set_status(status::FORBIDDEN, true);
            // Shuffling is legal with two cards, and it is the one command
            // that restarts the machine without emplacing anything.
            f.core.returns.set(answer(8, 0));
            to_menu(&mut f);
            let Some(Message::SelectIdleCmd { summonable, .. }) = menu(&f) else {
                panic!("no menu");
            };
            assert_eq!(summonable.len(), 1, "rebuilt, not remembered");
        }
    }
}

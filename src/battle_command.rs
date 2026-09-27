//! The Battle Phase: `BattleCommand`.
//!
//! Forty-four cases, the largest unit in the engine, and three loops living
//! in one switch. Reading it as a sequence is hopeless; reading it as three
//! machines that hand off to each other is tractable.
//!
//! ```text
//!   the battle-step loop          the attack                the damage step
//!   ────────────────────          ──────────                ───────────────
//!   0  gather, offer menu
//!   1  dispatch ──────────────→   3  cost paid?
//!   2  solve a chain              4  pick a target
//!  40  solve, or end the step     5  the answer
//!  41  battle phase over          6  target fixed
//!  42  second battle phase?       7  count the announce
//!  43  its answer                 8  announce it
//!                                 9  the window
//!                                10  rolled back?
//!                                11  disabled? replay? ────→ 19 set up
//!                                12  the replay answer       20 battle start
//!                                13  attack over             21 flip the target
//!                                                            22 battle confirm
//!                                                            23 damage-cal phase
//!                                                            24 pre-calculate
//!                                                            25 cancelled?
//!                                                            26 CALCULATE
//!                                                            27 apply the damage
//!                                                            28 destroy
//!                                                            29 replacements
//!                                                            30 mark destroyed
//!                                                            31 EVENT_BATTLED
//!                                                            32 the window
//!                                                            33 send them away
//!                                                            34 destroyed events
//!                                                            39 step over
//! ```
//!
//! ## Case 0 filters activate effects on **speed**
//!
//! `get_speed() > 1`. Every other gather in the engine offers whatever is
//! activatable; this one additionally demands spell speed 2. That is what
//! keeps a monster's `EFFECT_TYPE_ACTIVATE` — speed 0 — off the Battle
//! Phase menu, and the filter is applied to `activate_effect` only: the
//! quick and continuous registries are gathered unfiltered.
//!
//! ## The attack can be *rolled back*, and that is not the same as cancelled
//!
//! Three different things end an attack and they are not interchangeable:
//!
//! | | |
//! |---|---|
//! | `STATUS_ATTACK_CANCELED` | the attack is negated; no replay |
//! | `core.attack_rollback` | the board changed under it; **a replay is offered** |
//! | `return_cards.canceled` | the player backed out at the target prompt |
//!
//! A replay re-enters at case 3 with `is_replaying_attack` set, which is
//! what stops case 7 counting the announcement twice.
//!
//! ## `announce_count` is incremented in four places
//!
//! Cases 7, 11, 13 and `DamageStep`'s case 0, each under a different
//! condition, and the conditions do not overlap by accident. Case 13's is
//! the subtle one: it counts the attack **only if the attacker is still the
//! same instance** (`fieldid_r == pre_field[0]`) and either replays are off
//! or the attack was cancelled. An attacker that left and came back does
//! not spend an attack.
//!
//! ## Case 31 falls through to 32
//!
//! The only `[[fallthrough]]` in the unit, and it is conditional: with
//! `core.effect_damage_step` set the case returns *finished* instead, handing
//! `cards_destroyed_by_battle` to the `DamageStep` waiting in `core.reserved`.
//! So the same case either ends the unit or runs the next one, depending on
//! who called it.
//!
//! **A fallthrough inverts the step rule.** Everywhere else `set_step(n)`
//! means *case `n + 1` runs next*, because the processor increments after
//! the handler returns. A fallthrough already ran the next case's body
//! inline, so the number written is the case just executed, not one below
//! it — the reference says `arg.step = 32` and then runs case 32. Writing
//! `set_step(31)` here, which is what the ordinary rule would suggest,
//! makes case 32 run a **second** time, and the only symptom is a duplicate
//! response window that no unit test distinguishes.

use crate::board::{location, position};
use crate::card::{reason, status};
use crate::chain::Chain;
use crate::event::{code, CardId, EffectId, Event, PLAYER_NONE};
use crate::field::GroupId;
use crate::field::{timing, Field, Message};
use crate::processor::{Kind, RESTART};

/// `BattleCommand`'s own state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BattleCommandState {
    /// What to hand back to `Turn` — the `ctype` the menu answered with.
    pub phase_to_change_to: u8,
    /// Set when this run is carrying out an attack an effect forced, in
    /// which case the unit answers `2` and restarts rather than looping.
    pub forced_attack: bool,
    pub forced_attack_done: bool,
    /// Set while re-declaring an attack whose board changed. Stops case 7
    /// counting the announcement a second time.
    pub is_replaying_attack: bool,
    /// Set when the declaration failed outright — no legal target, or the
    /// attacker was negated before it could be made.
    pub attack_announce_failed: bool,
    /// A second Battle Phase is due, and whether the player may decline it.
    pub repeat_battle_phase: bool,
    pub second_battle_phase_is_optional: bool,
    /// Whether the window before this one produced anything to resolve.
    /// Only read under `DUEL_6_STEP_BATLLE_STEP`.
    pub previous_point_event_had_any_trigger_to_resolve: bool,
    pub reason_player: u8,
    pub damage_change_effect: Option<EffectId>,
    pub reason_card: Option<CardId>,
    /// The cards this battle destroyed, carried from case 28 to case 33 and
    /// handed to a waiting `DamageStep` at case 31.
    pub cards_destroyed_by_battle: Option<GroupId>,
    /// `EFFECT_MUST_ATTACK_MONSTER` pairs, from the target gather.
    pub must_attack_map: Vec<(EffectId, CardId)>,
}

impl Field {
    /// `field::process(Processors::BattleCommand&)`.
    pub(crate) fn battle_command_step(
        &mut self,
        step: u16,
        state: &mut BattleCommandState,
    ) -> bool {
        match step {
            0 => self.bc_gather(state),
            1 => self.bc_dispatch(state),
            2 => self.bc_solve_chain(RESTART),
            3 => self.bc_cost_check(state),
            4 => self.bc_pick_target(state),
            5 => self.bc_direct_answer(state),
            6 => self.bc_target_fixed(state),
            7 => self.bc_count_announce(state),
            8 => self.bc_announce(state),
            9 => self.bc_battle_window(),
            10 => {
                // The window's answer: something was activated, so go back
                // and announce again.
                if self.core.returns.get() != 0 {
                    self.set_step(8);
                } else {
                    self.adjust_all();
                }
                false
            }
            11 => self.bc_disabled_or_replay(state),
            12 => {
                if self.core.returns.get() != 0 {
                    state.is_replaying_attack = true;
                    state.attack_announce_failed = false;
                    self.set_step(3);
                }
                false
            }
            13 => self.bc_attack_over(state),
            14 => self.bc_pay_attack_costs(),
            15 => {
                self.adjust_instant();
                self.emplace(Kind::PointEvent {
                    skip: crate::point_event::PointEventSkip::default(),
                });
                self.set_step(RESTART);
                false
            }
            16 => self.bc_must_attack_narrow(state),
            19 => self.bc_damage_setup(),
            20 => self.bc_battle_start(state),
            21 => self.bc_flip_target(),
            22 => self.bc_battle_confirm(state),
            23 => {
                if self.attacker_cancelled() {
                    self.set_step(33);
                    return false;
                }
                self.infos.phase = crate::duel::phases::DAMAGE_CAL;
                self.adjust_all();
                false
            }
            24 => self.bc_pre_calculate(),
            25 => {
                if self.attacker_cancelled() {
                    self.reset_phase(crate::duel::phases::DAMAGE_CAL);
                    self.adjust_all();
                    self.infos.phase = crate::duel::phases::DAMAGE;
                    self.set_step(33);
                }
                false
            }
            26 => self.bc_calculate(state),
            27 => self.bc_apply_damage(state),
            28 => self.bc_destroy(state),
            29 => self.bc_destroy_replace(),
            30 => self.bc_mark_destroyed(state),
            31 => self.bc_battled(state),
            32 => self.bc_battled_window(),
            33 => self.bc_send_destroyed(state),
            34 => self.bc_destroyed_events(state),
            // Case 38 is empty in the reference — the landing pad case 34
            // jumps to, so that the window it opened is resolved before 39.
            38 => false,
            39 => self.bc_step_over(state),
            40 => self.bc_end_of_step(state),
            41 => self.bc_phase_over(state),
            42 => self.bc_finish(state),
            43 => {
                let bp_twice = self.core.returns.get();
                self.core.returns.set(i32::from(state.phase_to_change_to));
                self.core.returns.set_i32(1, bp_twice);
                true
            }
            _ => true,
        }
    }

    fn attacker_cancelled(&self) -> bool {
        self.core
            .attacker
            .is_some_and(|a| self.cards[a].is_status(status::ATTACK_CANCELED))
    }

    /// Case 0: what can be done in the Battle Phase.
    fn bc_gather(&mut self, state: &mut BattleCommandState) -> bool {
        self.core.select_chains.clear();
        if !self.core.chain_attack {
            self.core.chain_attacker_id = 0;
            self.core.chain_attack_target = None;
        }
        self.core.attack_player = false;
        self.core.attacker = None;
        self.core.attack_target = None;

        let tp = self.infos.turn_player;
        let skip = self.is_player_affected_by_effect(tp, code::SKIP_BP);
        if skip.is_some() || self.core.force_turn_end {
            self.set_step(41);
            state.phase_to_change_to = 2;
            state.repeat_battle_phase = self
                .is_player_affected_by_effect(tp, code::BP_TWICE)
                .is_some();
            // **The skip effect's own value decides how much is skipped.**
            // A zero value (or a forced turn end) resets only the Battle
            // Step and runs the phase event; a non-zero one resets the
            // whole Battle Phase and runs nothing.
            let value = skip.map_or(0, |e| self.effect_plain_value(e));
            if self.core.force_turn_end || value == 0 {
                self.reset_phase(crate::duel::phases::BATTLE_STEP);
                self.adjust_all();
                self.infos.phase = crate::duel::phases::BATTLE;
                self.emplace(Kind::PhaseEvent {
                    phase: crate::duel::phases::BATTLE,
                    state: Default::default(),
                });
            } else {
                self.core.hint_timing[tp as usize] = 0;
                self.reset_phase(crate::duel::phases::BATTLE);
                self.adjust_all();
            }
            return false;
        }

        self.bc_gather_chains(tp);
        self.bc_gather_attackers(tp);
        self.core.attack_cancelable = true;
        self.core.attack_cost_paid = false;
        self.emplace(Kind::SelectBattleCmd { player: tp });
        false
    }

    /// The three registries case 0 offers from.
    ///
    /// **Only the activate registry is filtered on speed.** `get_speed() >
    /// 1` keeps a monster's activation — speed 0 — off the menu; the quick
    /// and continuous registries are gathered as they are.
    ///
    /// Adding the same filter to the quick registry is an **equivalent
    /// mutant**: `QUICK_O` and `QUICK_F` are speed 2 by construction, so
    /// `> 1` always passes there. The filter discriminates only on
    /// activate effects, which is exactly where the reference puts it.
    fn bc_gather_chains(&mut self, tp: u8) {
        let nil = Event::new(code::FREE_CHAIN);
        for e in self
            .field_effects
            .activate
            .equal_range(code::FREE_CHAIN)
            .to_vec()
        {
            if let Some(h) = self.effects.get(e).and_then(|x| x.get_handler(&self.cards)) {
                let snap = self.cards[h].state();
                if let Some(x) = self.effects.get_mut(e) {
                    x.set_activate_location(&snap);
                }
            }
            if self.is_activateable(e, tp, &nil, false, false, false, false, false)
                && self.get_speed(e) > 1
            {
                self.core
                    .select_chains
                    .push_back(Chain::new(e, Event::new(code::FREE_CHAIN)));
            }
        }
        for e in self
            .field_effects
            .quick_o
            .equal_range(code::FREE_CHAIN)
            .to_vec()
        {
            if let Some(h) = self.effects.get(e).and_then(|x| x.get_handler(&self.cards)) {
                let snap = self.cards[h].state();
                if let Some(x) = self.effects.get_mut(e) {
                    x.set_activate_location(&snap);
                }
            }
            if self.is_activateable(e, tp, &nil, false, false, false, false, false) {
                self.core
                    .select_chains
                    .push_back(Chain::new(e, Event::new(code::FREE_CHAIN)));
            }
        }
        for e in self
            .field_effects
            .continuous
            .equal_range(code::FREE_CHAIN)
            .to_vec()
        {
            let owner = self
                .effects
                .get(e)
                .map_or(PLAYER_NONE, |x| x.get_handler_player(&self.cards));
            if owner == tp && self.is_activateable(e, tp, &nil, false, false, false, false, false) {
                self.core
                    .select_chains
                    .push_back(Chain::new(e, Event::new(code::FREE_CHAIN)));
            }
        }
    }

    /// The attackable list, and the two flags that say how the phase may
    /// be left.
    ///
    /// **`EFFECT_FIRST_ATTACK` replaces the whole list** rather than
    /// reordering it: a monster that must attack first is the only monster
    /// offered. And `EFFECT_MUST_ATTACK` takes away *both* exits — a player
    /// with an obliged monster may neither go to Main Phase 2 nor end the
    /// turn.
    fn bc_gather_attackers(&mut self, tp: u8) {
        self.core.attackable_cards.clear();
        let mut first_attack = Vec::new();
        let mut must_attack = Vec::new();

        if self
            .is_player_affected_by_effect(tp, code::CANNOT_ATTACK_ANNOUNCE)
            .is_none()
        {
            let mzone: Vec<CardId> = self.players[tp as usize]
                .mzone
                .iter()
                .flatten()
                .copied()
                .collect();
            for card in mzone {
                if !self.is_capable_attack_announce(card, tp) {
                    continue;
                }
                let chain_attack = self.core.chain_attack
                    && self.core.chain_attacker_id == self.cards[card].fieldid;
                let mut targets = Vec::new();
                self.get_attack_target(card, &mut targets, chain_attack, true, None);
                if targets.is_empty() && !self.cards[card].direct_attackable {
                    continue;
                }
                self.core.attackable_cards.push(card);
                if self
                    .is_affected_by_effect(card, code::FIRST_ATTACK)
                    .is_some()
                {
                    first_attack.push(card);
                }
                if self
                    .is_affected_by_effect(card, code::MUST_ATTACK)
                    .is_some()
                {
                    must_attack.push(card);
                }
            }
            if !first_attack.is_empty() {
                self.core.attackable_cards = first_attack;
            }
        }

        self.core.to_m2 = !self.is_flag(crate::duel::flags::NO_MAIN_PHASE_2);
        self.core.to_ep = true;
        if !must_attack.is_empty()
            || self
                .is_player_affected_by_effect(tp, code::CANNOT_M2)
                .is_some()
            || self.core.force_turn_end
        {
            self.core.to_m2 = false;
        }
        if !must_attack.is_empty() {
            self.core.to_ep = false;
        }
    }

    /// Case 1: act on the menu's answer.
    fn bc_dispatch(&mut self, state: &mut BattleCommandState) -> bool {
        let answer = self.core.returns.get();
        let ctype = answer & 0xffff;
        let sel = (answer >> 16) as usize;
        let tp = self.infos.turn_player;

        if state.forced_attack_done || (ctype != 0 && ctype != 1) {
            self.set_step(39);
            state.phase_to_change_to = ctype as u8;
            self.messages.push(Message::Hint {
                kind: crate::host_question::hint::EVENT,
                player: 1 - tp,
                value: 29,
            });
            self.core.select_chains.clear();
            self.core.hint_timing[tp as usize] = timing::BATTLE_STEP_END;
            self.emplace(Kind::QuickEffect {
                skip_freechain: false,
                player: 1 - tp,
                is_opponent: false,
            });
            self.infos.priorities[tp as usize] = 1;
            self.infos.priorities[1 - tp as usize] = 0;
            return false;
        }
        if ctype == 0 {
            return self.bc_activate(sel, tp);
        }
        self.bc_declare_attack(sel, state, tp)
    }

    /// `ctype == 0`: an effect was activated. The same shape as
    /// `IdleCommand`'s.
    fn bc_activate(&mut self, sel: usize, tp: u8) -> bool {
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
            self.set_step(14);
            return false;
        }
        let Some(handler) = self
            .effects
            .get(effect)
            .and_then(|e| e.get_handler(&self.cards))
        else {
            return true;
        };
        let mut new_chain = chain;
        new_chain.flag = 0;
        new_chain.chain_id = self.next_field_id();
        let mut evt = Event::new(self.effects.get(effect).map_or(0, |e| e.code));
        evt.event_player = PLAYER_NONE;
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
        false
    }

    /// `ctype == 1`: an attack was declared. Gathers the attack costs and
    /// asks their order if there is more than one.
    ///
    /// **A cost makes the attack uncancelable.** `attack_cancelable` goes
    /// down the moment a cost with an operation is found, because the
    /// player is about to pay for something they can then not back out of.
    fn bc_declare_attack(&mut self, sel: usize, state: &mut BattleCommandState, tp: u8) -> bool {
        self.set_step(2);
        state.attack_announce_failed = false;
        if !state.forced_attack {
            let Some(&attacker) = self.core.attackable_cards.get(sel) else {
                return true;
            };
            self.core.attacker = Some(attacker);
        }
        let Some(attacker) = self.core.attacker else {
            return true;
        };
        self.cards[attacker].set_status(status::ATTACK_CANCELED, false);
        self.cards[attacker].attack_controler = self.cards[attacker].current.controller;
        self.core.pre_field[0] = self.cards[attacker].fieldid_r;
        if self.core.chain_attack && self.core.chain_attacker_id != self.cards[attacker].fieldid {
            self.core.chain_attack = false;
            self.core.chain_attacker_id = 0;
        }

        self.core.tpchain.clear();
        let mut costs = self.filter_player_effect(tp, code::ATTACK_COST);
        costs.extend(self.filter_effect(attacker, code::ATTACK_COST));
        for e in costs {
            if self.effects.get(e).is_some_and(|x| x.operation.is_some()) {
                self.core
                    .tpchain
                    .push_back(Chain::new(e, Event::new(code::FREE_CHAIN)));
                self.core.attack_cancelable = false;
            }
        }
        match self.core.tpchain.len() {
            0 => {}
            1 => {
                let e = self.core.tpchain[0].triggering_effect;
                self.core.sub_solving_event.push_back(Event::new(0));
                self.emplace(Kind::ExecuteOperation {
                    resume: None,
                    effect: e,
                    player: tp,
                    subject: None,
                    args: Vec::new(),
                    was_disabled: false,
                });
                self.adjust_all();
            }
            _ => {
                self.emplace(Kind::SortChain { player: tp });
                self.set_step(13);
            }
        }
        false
    }

    /// Case 2 / case 40's body: release the chain and solve it.
    fn bc_solve_chain(&mut self, next: u16) -> bool {
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
        for h in handlers {
            self.cards[h].set_status(status::CHAINING, false);
        }
        self.emplace(Kind::SolveChain {
            skip: crate::solve_chain::SolveChainSkip::default(),
        });
        self.set_step(next);
        false
    }

    /// Case 3: was the attack negated, and was its cost actually paid?
    fn bc_cost_check(&mut self, state: &mut BattleCommandState) -> bool {
        state.is_replaying_attack = false;
        if self.attacker_cancelled() {
            state.attack_announce_failed = true;
            self.set_step(6);
            return false;
        }
        // **An unpaid, uncancelable cost abandons the attack.** The player
        // committed to paying and something stopped them, so the whole
        // declaration comes off.
        if !self.core.attack_cost_paid && !self.core.attack_cancelable {
            self.bc_restart_or_answer(state);
            return false;
        }
        if state.forced_attack {
            self.set_step(6);
        }
        false
    }

    /// The tail every abandoned attack shares: a forced attack answers `2`
    /// and re-enters case 0; an ordinary one restarts the loop.
    fn bc_restart_or_answer(&mut self, state: &BattleCommandState) {
        if state.forced_attack {
            self.core.returns.set(2);
            self.set_step(0);
        } else {
            self.set_step(RESTART);
        }
    }

    /// Case 4: which monster is being attacked.
    ///
    /// Four shapes, and which one applies is decided by `atype` and by
    /// `EFFECT_PATRICIAN_OF_DARKNESS` — the card that makes the *opponent*
    /// choose the target.
    fn bc_pick_target(&mut self, state: &mut BattleCommandState) -> bool {
        let Some(attacker) = self.core.attacker else {
            return true;
        };
        let tp = self.infos.turn_player;
        self.core.attack_player = false;
        self.core.select_cards.clear();
        self.core.return_cards.clear();
        state.must_attack_map.clear();
        let mut targets = Vec::new();
        let atype = self.get_attack_target(
            attacker,
            &mut targets,
            self.core.chain_attack,
            true,
            Some(&mut state.must_attack_map),
        );
        self.core.select_cards = targets;

        if self.cards[attacker].direct_attackable {
            if self.core.select_cards.is_empty() {
                self.core.returns.set(-2);
                self.set_step(5);
                return false;
            }
            // Both a direct attack and monsters available: ask whether to
            // go direct. The *opponent* is asked under Patrician.
            if self
                .is_player_affected_by_effect(tp, code::PATRICIAN_OF_DARKNESS)
                .is_some()
            {
                let code_ = self.cards[attacker].data.code;
                self.emplace(Kind::SelectEffectYesNo {
                    player: 1 - tp,
                    description: 31,
                    card: code_ as usize,
                });
            } else {
                self.emplace(Kind::SelectYesNo {
                    player: tp,
                    description: 31,
                });
            }
            return false;
        }
        if self.core.select_cards.is_empty() {
            state.attack_announce_failed = true;
            self.set_step(6);
            return false;
        }

        // How many *distinct effects* compelled a target. One effect naming
        // several monsters is a choice; several effects each naming one is
        // not, and case 16 exists for the first case.
        let distinct: std::collections::BTreeSet<EffectId> =
            state.must_attack_map.iter().map(|&(e, _)| e).collect();
        let patrician = self
            .is_player_affected_by_effect(tp, code::PATRICIAN_OF_DARKNESS)
            .is_some();
        if patrician
            || (atype == crate::battle::attack_type::COMPELLED_ATTACKER && distinct.len() != 1)
        {
            if self.core.select_cards.len() == 1 {
                let only = self.core.select_cards[0];
                self.core.return_cards.list.push(only);
            } else {
                let info = self.get_info_location(attacker);
                self.messages
                    .push(Message::CardSelected { cards: vec![info] });
                self.messages.push(Message::Hint {
                    kind: crate::host_question::hint::SELECTMSG,
                    player: 1 - tp,
                    value: 549,
                });
                self.emplace(Kind::SelectCard {
                    player: 1 - tp,
                    cancelable: false,
                    min: 1,
                    max: 1,
                });
                if !patrician
                    && atype == crate::battle::attack_type::COMPELLED_ATTACKER
                    && state.must_attack_map.len() != distinct.len()
                {
                    self.set_step(15);
                    return false;
                }
            }
        } else {
            self.messages.push(Message::Hint {
                kind: crate::host_question::hint::SELECTMSG,
                player: tp,
                value: 549,
            });
            self.emplace(Kind::SelectCard {
                player: tp,
                cancelable: self.core.attack_cancelable,
                min: 1,
                max: 1,
            });
        }
        self.set_step(5);
        false
    }

    /// Case 5: the answer to "attack directly?".
    fn bc_direct_answer(&mut self, state: &mut BattleCommandState) -> bool {
        let tp = self.infos.turn_player;
        if self.core.returns.get() != 0 {
            self.core.returns.set(-2);
            return false;
        }
        if self.core.select_cards.is_empty() {
            state.attack_announce_failed = true;
            self.set_step(6);
            return false;
        }
        let opposel = self
            .is_player_affected_by_effect(tp, code::PATRICIAN_OF_DARKNESS)
            .is_some();
        let asked = if opposel { 1 - tp } else { tp };
        // **Patrician takes the cancel away too**: the opponent choosing
        // your target may not decline on your behalf.
        let cancelable = self.core.attack_cancelable && !opposel;
        self.messages.push(Message::Hint {
            kind: crate::host_question::hint::SELECTMSG,
            player: asked,
            value: 549,
        });
        self.emplace(Kind::SelectCard {
            player: asked,
            cancelable,
            min: 1,
            max: 1,
        });
        false
    }

    /// Case 6: the target is fixed, or the attack was backed out of.
    fn bc_target_fixed(&mut self, state: &mut BattleCommandState) -> bool {
        if self.core.return_cards.canceled {
            // **A cancelled replay does not restart the loop** — it goes to
            // case 13, which finishes the attack properly.
            if state.is_replaying_attack {
                self.set_step(12);
                return false;
            }
            self.bc_restart_or_answer(state);
            return false;
        }
        self.core.attack_target = if self.core.returns.get() == -2 {
            None
        } else {
            self.core.return_cards.list.first().copied()
        };
        self.core.pre_field[1] = self
            .core
            .attack_target
            .map_or(0, |t| self.cards[t].fieldid_r);
        false
    }

    /// Case 7: count the declaration, unless this is a replay.
    fn bc_count_announce(&mut self, state: &mut BattleCommandState) -> bool {
        let Some(attacker) = self.core.attacker else {
            return true;
        };
        if !state.is_replaying_attack {
            self.core.phase_action = true;
            let tp = self.infos.turn_player;
            self.core.attack_state_count[tp as usize] += 1;
            self.check_card_counter(attacker, crate::summon_support::activity::ATTACK, tp);
            self.cards[attacker].attack_announce_count += 1;
        }
        if state.attack_announce_failed {
            self.cards[attacker].announce_count += 1;
            self.core.chain_attack = false;
            self.bc_restart_or_answer(state);
        }
        false
    }

    /// Case 8: announce it, and pin the board it was declared against.
    ///
    /// `core.opp_mzone` is the set of opposing field ids **at declaration**.
    /// A replay is offered later when that set no longer matches — the
    /// attack was aimed at a board that has changed.
    fn bc_announce(&mut self, state: &mut BattleCommandState) -> bool {
        let Some(attacker) = self.core.attacker else {
            return true;
        };
        let tp = self.infos.turn_player;
        self.core.attack_cancelable = true;
        let attacker_info = self.get_info_location(attacker);
        let target_info = self
            .core
            .attack_target
            .map(|t| self.get_info_location(t))
            .unwrap_or_default();
        if let Some(t) = self.core.attack_target {
            self.raise_single_event(t, Vec::new(), code::BE_BATTLE_TARGET, None, 0, 0, 1 - tp, 0);
            self.raise_event(Some(t), code::BE_BATTLE_TARGET, None, 0, 0, 1 - tp, 0);
        }
        self.messages.push(Message::Attack {
            attacker: attacker_info,
            target: target_info,
        });
        self.core.attack_rollback = false;
        self.core.opp_mzone = self.players[1 - tp as usize]
            .mzone
            .iter()
            .flatten()
            .map(|&c| self.cards[c].fieldid_r)
            .collect();
        if !state.is_replaying_attack {
            self.raise_single_event(
                attacker,
                Vec::new(),
                code::ATTACK_ANNOUNCE,
                None,
                0,
                0,
                tp,
                0,
            );
            self.raise_event(Some(attacker), code::ATTACK_ANNOUNCE, None, 0, 0, tp, 0);
        }
        self.cards[attacker].attack_controler = self.cards[attacker].current.controller;
        self.core.pre_field[0] = self.cards[attacker].fieldid_r;
        self.process_single_event();
        self.process_instant_event();
        self.core.hint_timing[tp as usize] = timing::ATTACK;
        self.emplace(Kind::PointEvent {
            skip: crate::point_event::PointEventSkip::default(),
        });
        false
    }

    /// Case 9: the Battle Phase window, opened for **both** players.
    fn bc_battle_window(&mut self) -> bool {
        let tp = self.infos.turn_player;
        if self
            .is_player_affected_by_effect(tp, code::SKIP_BP)
            .is_some()
            || self.attacker_cancelled()
            || self.core.attack_rollback
        {
            self.set_step(10);
            return false;
        }
        for p in 0..2u8 {
            self.messages.push(Message::Hint {
                kind: crate::host_question::hint::EVENT,
                player: p,
                value: 24,
            });
        }
        self.core.hint_timing = [timing::BATTLE_PHASE, timing::BATTLE_PHASE];
        // Entered at step 30 — the window only, skipping the gather.
        self.emplace_at(
            Kind::PointEvent {
                skip: crate::point_event::PointEventSkip::default(),
            },
            30,
        );
        false
    }

    /// Case 11: the attack was negated, or the board changed and a replay
    /// is due.
    fn bc_disabled_or_replay(&mut self, _state: &mut BattleCommandState) -> bool {
        let Some(attacker) = self.core.attacker else {
            return true;
        };
        let tp = self.infos.turn_player;
        if self
            .is_affected_by_effect(attacker, code::ATTACK_DISABLED)
            .is_some()
        {
            // `card::reset(EFFECT_ATTACK_DISABLED, RESET_CODE)` — drop the
            // effect by its code rather than by an event.
            self.reset_card(attacker, code::ATTACK_DISABLED, crate::field::reset::CODE);
            self.messages.push(Message::AttackDisabled);
            self.cards[attacker].set_status(status::ATTACK_CANCELED, true);
        }
        if self
            .is_player_affected_by_effect(tp, code::SKIP_BP)
            .is_some()
            || self.attacker_cancelled()
        {
            self.set_step(12);
            return false;
        }
        if !self.core.attack_rollback {
            self.cards[attacker].announce_count += 1;
            let fid = self
                .core
                .attack_target
                .map_or(0, |t| self.cards[t].fieldid_r);
            let target = self.core.attack_target;
            self.cards[attacker].announced_cards.add(target, fid);
            self.attack_all_target_check();
            self.set_step(18);
            return false;
        }
        // Rolled back: is there still anything to attack?
        let mut targets = Vec::new();
        let chain = self.core.chain_attack;
        self.get_attack_target(attacker, &mut targets, chain, true, None);
        if targets.is_empty() && !self.cards[attacker].direct_attackable {
            self.set_step(12);
            return false;
        }
        // **Three ways the replay question is answered**, and only one of
        // them asks the player.
        if self.is_flag(crate::duel::flags::STORE_ATTACK_REPLAYS) && !self.core.chain_attack {
            self.core.returns.set(0);
        } else if self
            .is_affected_by_effect(attacker, code::MUST_ATTACK)
            .is_none()
        {
            self.emplace(Kind::SelectYesNo {
                player: tp,
                description: 30,
            });
        } else {
            self.core.returns.set(1);
            self.core.attack_cancelable = false;
        }
        false
    }

    /// Case 13: the attack is over without a damage step.
    ///
    /// **The announcement is counted only if the attacker is still the same
    /// instance**, and only when replays are off or the attack was
    /// cancelled. A monster that left and came back does not spend one.
    fn bc_attack_over(&mut self, state: &mut BattleCommandState) -> bool {
        if let Some(attacker) = self.core.attacker {
            let same = self.cards[attacker].fieldid_r == self.core.pre_field[0];
            let replays_off = !(self.is_flag(crate::duel::flags::STORE_ATTACK_REPLAYS)
                && !self.core.chain_attack);
            if same && (replays_off || self.attacker_cancelled()) {
                self.cards[attacker].announce_count += 1;
                let fid = self
                    .core
                    .attack_target
                    .map_or(0, |t| self.cards[t].fieldid_r);
                let target = self.core.attack_target;
                self.cards[attacker].announced_cards.add(target, fid);
                self.attack_all_target_check();
            }
        }
        self.core.chain_attack = false;
        self.bc_restart_or_answer(state);
        self.reset_phase(crate::duel::phases::DAMAGE);
        self.adjust_all();
        false
    }

    /// Case 14: run the attack costs, now that their order is settled.
    fn bc_pay_attack_costs(&mut self) -> bool {
        let tp = self.infos.turn_player;
        let costs: Vec<EffectId> = self
            .core
            .tpchain
            .iter()
            .map(|c| c.triggering_effect)
            .collect();
        for e in costs {
            self.core.sub_solving_event.push_back(Event::new(0));
            self.emplace(Kind::ExecuteOperation {
                resume: None,
                effect: e,
                player: tp,
                subject: None,
                args: Vec::new(),
                was_disabled: false,
            });
            self.adjust_all();
        }
        self.core.tpchain.clear();
        self.set_step(2);
        false
    }

    /// Case 16: one compelling effect named several monsters, so narrow to
    /// the ones *that* effect named.
    fn bc_must_attack_narrow(&mut self, state: &mut BattleCommandState) -> bool {
        let Some(&selected) = self.core.return_cards.list.first() else {
            return true;
        };
        let Some(&(effect, _)) = state.must_attack_map.iter().find(|&&(_, c)| c == selected) else {
            return true;
        };
        let same_effect: Vec<CardId> = state
            .must_attack_map
            .iter()
            .filter(|&&(e, _)| e == effect)
            .map(|&(_, c)| c)
            .collect();
        if same_effect.len() < 2 {
            self.core.attack_target = Some(selected);
            self.core.pre_field[1] = self.cards[selected].fieldid_r;
            self.set_step(5);
            return false;
        }
        self.core.select_cards = same_effect;
        let tp = self.infos.turn_player;
        self.messages.push(Message::Hint {
            kind: crate::host_question::hint::SELECTMSG,
            player: tp,
            value: 549,
        });
        self.emplace(Kind::SelectCard {
            player: tp,
            cancelable: self.core.attack_cancelable,
            min: 1,
            max: 1,
        });
        self.set_step(5);
        false
    }

    /// Case 19: enter the Damage Phase and pin both monsters.
    fn bc_damage_setup(&mut self) -> bool {
        let Some(attacker) = self.core.attacker else {
            return true;
        };
        let tp = self.infos.turn_player;
        self.infos.phase = crate::duel::phases::DAMAGE;
        self.core.chain_attack = false;
        self.core.damage_calculated = false;
        self.core.selfdes_disabled = true;
        self.core.flip_delayed = true;
        self.cards[attacker].attack_controler = self.cards[attacker].current.controller;
        self.core.pre_field[0] = self.cards[attacker].fieldid_r;
        match self.core.attack_target {
            Some(t) => {
                self.cards[t].attack_controler = self.cards[t].current.controller;
                self.core.pre_field[1] = self.cards[t].fieldid_r;
            }
            None => self.core.pre_field[1] = 0,
        }
        self.cards[attacker].attacked_count += 1;
        let fid = self
            .core
            .attack_target
            .map_or(0, |t| self.cards[t].fieldid_r);
        let target = self.core.attack_target;
        self.cards[attacker].attacked_cards.add(target, fid);
        self.core.battled_count[tp as usize] += 1;
        self.adjust_all();
        false
    }

    /// Case 20: the damage step begins.
    fn bc_battle_start(&mut self, state: &mut BattleCommandState) -> bool {
        let Some(attacker) = self.core.attacker else {
            return true;
        };
        self.messages.push(Message::DamageStepStart);
        self.raise_single_event(attacker, Vec::new(), code::BATTLE_START, None, 0, 0, 0, 0);
        if let Some(t) = self.core.attack_target {
            self.raise_single_event(t, Vec::new(), code::BATTLE_START, None, 0, 0, 0, 1);
        }
        self.raise_event(None, code::BATTLE_START, None, 0, 0, 0, 0);
        self.process_single_event();
        self.process_instant_event();
        state.previous_point_event_had_any_trigger_to_resolve = false;
        let pending = !self.core.new_fchain.is_empty() || !self.core.new_ochain.is_empty();
        if !self.is_flag(crate::duel::flags::SIX_STEP_BATTLE_STEP) || pending {
            state.previous_point_event_had_any_trigger_to_resolve = pending;
            for p in 0..2u8 {
                self.messages.push(Message::Hint {
                    kind: crate::host_question::hint::EVENT,
                    player: p,
                    value: 40,
                });
            }
            self.bc_substep_window();
        }
        false
    }

    /// Case 21: flip a face-down target up, saving both positions first.
    fn bc_flip_target(&mut self) -> bool {
        if self.attacker_cancelled() {
            self.set_step(33);
            return false;
        }
        let Some(t) = self.core.attack_target else {
            return false;
        };
        if let Some(a) = self.core.attacker {
            self.cards[a].temp.position = self.cards[a].current.position;
        }
        self.cards[t].temp.position = self.cards[t].current.position;
        if self.cards[t].current.is_position(position::FACEDOWN) {
            let up = self.cards[t].current.position >> 1;
            self.change_position([t], None, PLAYER_NONE, up, up, up, up, 0, true);
            self.adjust_all();
        }
        false
    }

    /// Case 22: the battle is confirmed.
    ///
    /// **`pre_field[1]` is re-pinned only for a target that *was*
    /// face-down** — the flip in case 21 gave it a new field id, and this
    /// is where the attack is re-aimed at the card it revealed.
    fn bc_battle_confirm(&mut self, state: &mut BattleCommandState) -> bool {
        let Some(attacker) = self.core.attacker else {
            return true;
        };
        self.raise_single_event(attacker, Vec::new(), code::BATTLE_CONFIRM, None, 0, 0, 0, 0);
        if let Some(t) = self.core.attack_target {
            if self.cards[t].temp.position & position::FACEDOWN != 0 {
                self.core.pre_field[1] = self.cards[t].fieldid_r;
            }
            self.raise_single_event(t, Vec::new(), code::BATTLE_CONFIRM, None, 0, 0, 0, 1);
        }
        self.raise_event(None, code::BATTLE_CONFIRM, None, 0, 0, 0, 0);
        self.process_single_event();
        self.process_instant_event();
        let pending = !self.core.new_fchain.is_empty() || !self.core.new_ochain.is_empty();
        if !self.is_flag(crate::duel::flags::SIX_STEP_BATTLE_STEP)
            || !state.previous_point_event_had_any_trigger_to_resolve
            || pending
        {
            for p in 0..2u8 {
                self.messages.push(Message::Hint {
                    kind: crate::host_question::hint::EVENT,
                    player: p,
                    value: 41,
                });
            }
            let tp = self.infos.turn_player;
            self.core.hint_timing[tp as usize] = timing::DAMAGE_STEP;
            self.bc_substep_window();
        }
        false
    }

    /// The damage sub-step's response window. The third `PointEvent`
    /// argument is a duel option, and it is the same in all four places.
    fn bc_substep_window(&mut self) {
        let single = self.is_flag(crate::duel::flags::SINGLE_CHAIN_IN_DAMAGE_SUBSTEP);
        self.emplace(Kind::PointEvent {
            // The reference's third `PointEvent` argument is `skip_new`,
            // and the duel option it carries is
            // `DUEL_SINGLE_CHAIN_IN_DAMAGE_SUBSTEP` — one chain per damage
            // sub-step. Same flag in all four places it appears.
            skip: crate::point_event::PointEventSkip {
                new: single,
                ..Default::default()
            },
        });
    }

    /// Case 24: the damage is worked out, and announced *before* it lands.
    fn bc_pre_calculate(&mut self) -> bool {
        let Some(attacker) = self.core.attacker else {
            return true;
        };
        self.calculate_battle_damage();
        self.raise_single_event(
            attacker,
            Vec::new(),
            code::PRE_DAMAGE_CALCULATE,
            None,
            0,
            0,
            0,
            0,
        );
        if let Some(t) = self.core.attack_target {
            self.raise_single_event(t, Vec::new(), code::PRE_DAMAGE_CALCULATE, None, 0, 0, 0, 1);
        }
        self.raise_event(None, code::PRE_DAMAGE_CALCULATE, None, 0, 0, 0, 0);
        self.process_single_event();
        self.process_instant_event();
        for p in 0..2u8 {
            self.messages.push(Message::Hint {
                kind: crate::host_question::hint::EVENT,
                player: p,
                value: 42,
            });
        }
        let tp = self.infos.turn_player;
        self.core.hint_timing[tp as usize] = timing::DAMAGE_CAL;
        self.bc_substep_window();
        false
    }

    /// Case 26: the calculation that counts.
    ///
    /// `calculate_battle_damage` is run **twice** — once in case 24 for the
    /// pre-calculation window and again here — because effects in that
    /// window may have changed the numbers.
    fn bc_calculate(&mut self, state: &mut BattleCommandState) -> bool {
        let Some(attacker) = self.core.attacker else {
            return true;
        };
        let tp = self.infos.turn_player;
        let aa = self.get_attack(attacker);
        let ad = self.get_defense(attacker);
        self.cards[attacker].set_status(status::BATTLE_RESULT, false);
        self.cards[attacker].set_status(status::BATTLE_DESTROYED, false);
        let (mut da, mut dd) = (0, 0);
        if let Some(t) = self.core.attack_target {
            da = self.get_attack(t);
            dd = self.get_defense(t);
            self.cards[t].set_status(status::BATTLE_RESULT, false);
            self.cards[t].set_status(status::BATTLE_DESTROYED, false);
            let pa = self.cards[attacker].current.controller;
            let pd = self.cards[t].current.controller;
            if pa != pd {
                self.cards[attacker].set_status(status::OPPO_BATTLE, true);
                self.cards[t].set_status(status::OPPO_BATTLE, true);
            }
        }

        let outcome = self.calculate_battle_damage();
        let mut bd = outcome.destroyed;
        // `EFFECT_INDESTRUCTABLE_BATTLE` is read **against the other
        // card**, and announces itself with a hint before taking the
        // destruction away.
        if bd[0] {
            let saved = self.core.attack_target.and_then(|t| {
                self.is_affected_by_effect_against(attacker, code::INDESTRUCTABLE_BATTLE, t)
            });
            match saved {
                Some(e) => {
                    self.announce_indestructible(e);
                    bd[0] = false;
                }
                None => self.cards[attacker].set_status(status::BATTLE_RESULT, true),
            }
        }
        if bd[1] {
            if let Some(t) = self.core.attack_target {
                let saved =
                    self.is_affected_by_effect_against(t, code::INDESTRUCTABLE_BATTLE, attacker);
                match saved {
                    Some(e) => {
                        self.announce_indestructible(e);
                        bd[1] = false;
                    }
                    None => self.cards[t].set_status(status::BATTLE_RESULT, true),
                }
            }
        }

        let attacker_info = self.get_info_location(attacker);
        let target_info = self
            .core
            .attack_target
            .map(|t| self.get_info_location(t))
            .unwrap_or_default();
        self.messages.push(Message::Battle {
            attacker: attacker_info,
            attacker_attack: aa,
            attacker_defense: ad,
            attacker_destroyed: bd[0],
            target: target_info,
            target_attack: da,
            target_defense: dd,
            target_destroyed: bd[1],
        });

        state.damage_change_effect = outcome.damage_change;
        state.reason_card = outcome.reason_card;
        if let Some(rc) = outcome.reason_card {
            state.reason_player = self.cards[rc].current.controller;
        }
        if outcome.damage_change.is_none() {
            if let Some(rc) = outcome.reason_card {
                let rp = self.cards[rc].current.controller;
                for p in [tp, 1 - tp] {
                    let amount = self.core.battle_damage[p as usize];
                    if amount == 0 {
                        continue;
                    }
                    self.raise_single_event(
                        attacker,
                        Vec::new(),
                        code::PRE_BATTLE_DAMAGE,
                        None,
                        0,
                        rp,
                        p,
                        amount,
                    );
                    if let Some(t) = self.core.attack_target {
                        self.raise_single_event(
                            t,
                            Vec::new(),
                            code::PRE_BATTLE_DAMAGE,
                            None,
                            0,
                            rp,
                            p,
                            amount,
                        );
                    }
                    self.raise_event(Some(rc), code::PRE_BATTLE_DAMAGE, None, 0, rp, p, amount);
                }
            }
        }
        self.process_single_event();
        self.process_instant_event();
        self.core.damage_calculated = true;
        false
    }

    fn announce_indestructible(&mut self, effect: EffectId) {
        let code_ = self
            .effects
            .get(effect)
            .and_then(|e| e.owner)
            .map_or(0, |o| self.cards[o].data.code);
        self.messages.push(Message::Hint {
            kind: crate::host_question::hint::CARD,
            player: 0,
            value: u64::from(code_),
        });
    }

    /// Case 27: the damage lands.
    ///
    /// **`EFFECT_BATTLE_DAMAGE_TO_EFFECT` changes the *reason*, not the
    /// amount** — the same numbers are dealt as effect damage instead of
    /// battle damage, so nothing that responds to battle damage sees it.
    fn bc_apply_damage(&mut self, state: &mut BattleCommandState) -> bool {
        let Some(attacker) = self.core.attacker else {
            return true;
        };
        let tp = self.infos.turn_player;
        self.infos.phase = crate::duel::phases::DAMAGE;
        self.core.hint_timing[tp as usize] = 0;
        self.core.chain_attack = false;
        let fid = self
            .core
            .attack_target
            .map_or(0, |t| self.cards[t].fieldid_r);
        let target = self.core.attack_target;
        self.cards[attacker].battled_cards.add(target, fid);
        if let Some(t) = target {
            let afid = self.cards[attacker].fieldid_r;
            self.cards[t].battled_cards.add(Some(attacker), afid);
        }
        let damchange = state.damage_change_effect.take();
        let rc = state.reason_card;
        let rp = state.reason_player;
        for p in 0..2usize {
            let amount = self.core.battle_damage[p];
            if amount == 0 {
                continue;
            }
            match damchange {
                None => self.damage(None, reason::BATTLE, rp, rc, p as u8, amount, false),
                Some(e) => self.damage(Some(e), reason::EFFECT, rp, rc, p as u8, amount, false),
            }
        }
        self.reset_phase(crate::duel::phases::DAMAGE_CAL);
        self.adjust_all();
        false
    }

    /// Case 28: work out what the battle destroyed, and send it.
    ///
    /// Each destroyed card's **previous reason is saved** into `temp`
    /// before being overwritten with `REASON_BATTLE`, so an effect that
    /// looks at why it left can see both.
    fn bc_destroy(&mut self, state: &mut BattleCommandState) -> bool {
        let Some(attacker) = self.core.attacker else {
            return true;
        };
        let target = self.core.attack_target;
        let mut des: Vec<CardId> = Vec::new();

        if self.cards[attacker].is_status(status::BATTLE_RESULT)
            && self.cards[attacker].current.location == location::MZONE
            && self.cards[attacker].fieldid_r == self.core.pre_field[0]
        {
            if let Some(t) = target {
                des.push(attacker);
                self.bc_confirm_destruction(attacker, t);
            }
        }
        if let Some(t) = target {
            if self.cards[t].is_status(status::BATTLE_RESULT)
                && self.cards[t].current.location == location::MZONE
                && self.cards[t].fieldid_r == self.core.pre_field[1]
            {
                des.push(t);
                self.bc_confirm_destruction(t, attacker);
            }
        }
        self.cards[attacker].set_status(status::BATTLE_RESULT, false);
        if let Some(t) = target {
            self.cards[t].set_status(status::BATTLE_RESULT, false);
        }
        self.core.battle_destroy_rep.clear();
        self.core.desrep_chain.clear();
        if !des.is_empty() {
            let group = self.new_group(des);
            self.emplace_at(
                Kind::Destroy {
                    targets: group,
                    reason_effect: None,
                    reason: reason::BATTLE,
                    reason_player: PLAYER_NONE,
                },
                10,
            );
            state.cards_destroyed_by_battle = Some(group);
        }
        false
    }

    /// One card's destruction: save its old reason, take the new one, and
    /// work out where it is going.
    ///
    /// `EFFECT_BATTLE_DESTROY_REDIRECT` is read on **the other card** and
    /// its value packs a destination and a sequence into one integer —
    /// `dest >> 16` is the sequence and the low half is the location.
    fn bc_confirm_destruction(&mut self, card: CardId, by: CardId) {
        let c = &mut self.cards[card];
        c.temp.reason = c.reason;
        c.temp.reason_card = c.reason_card;
        c.temp.reason_effect = c.reason_effect;
        c.temp.reason_player = c.reason_player;
        c.reason_effect = None;
        c.reason = reason::BATTLE;
        c.reason_card = Some(by);
        let by_controller = self.cards[by].current.controller;
        self.cards[card].reason_player = by_controller;

        let mut dest = u32::from(location::GRAVE);
        let mut seq = 0u32;
        let is_monster = self.cards[card]
            .data
            .is_type(crate::card::card_type::MONSTER);
        if is_monster {
            if let Some(e) = self.is_affected_by_effect(by, code::BATTLE_DESTROY_REDIRECT) {
                let v = self.effect_value_about(e, card) as u32;
                seq = v >> 16;
                dest = v & 0xffff;
            }
        }
        let owner = self.cards[card].owner;
        let p = &mut self.cards[card].sendto_param;
        p.playerid = owner;
        p.position = position::FACEUP;
        p.location = dest as u8;
        p.sequence = seq;
        self.cards[card].set_status(status::DESTROY_CONFIRMED, true);
    }

    /// Case 29: the replacements a destruction offers.
    fn bc_destroy_replace(&mut self) -> bool {
        if !self.core.battle_destroy_rep.is_empty() {
            let reps: Vec<CardId> = self.core.battle_destroy_rep.iter().copied().collect();
            self.destroy(
                reps,
                None,
                reason::EFFECT | reason::REPLACE,
                PLAYER_NONE,
                PLAYER_NONE,
                0,
                0,
            );
        }
        if !self.core.desrep_chain.is_empty() {
            let group = self.new_group(Vec::new());
            self.emplace_at(
                Kind::OperationReplace {
                    replace_effect: usize::MAX,
                    targets: group,
                    target: None,
                    is_destroy: false,
                },
                15,
            );
        }
        self.adjust_all();
        false
    }

    /// Case 30: mark what actually died.
    fn bc_mark_destroyed(&mut self, state: &mut BattleCommandState) -> bool {
        if let Some(g) = state.cards_destroyed_by_battle {
            let cards: Vec<CardId> = self.groups[g].iter().copied().collect();
            for c in cards {
                self.cards[c].set_status(status::BATTLE_DESTROYED, true);
                self.cards[c].set_status(status::DESTROY_CONFIRMED, false);
                self.filter_disable_related_cards(c);
            }
        }
        self.core.selfdes_disabled = false;
        self.adjust_all();
        if self.is_flag(crate::duel::flags::SIX_STEP_BATTLE_STEP) {
            if self.core.effect_damage_step == 0 {
                for p in 0..2u8 {
                    self.messages.push(Message::Hint {
                        kind: crate::host_question::hint::EVENT,
                        player: p,
                        value: 45,
                    });
                }
                let tp = self.infos.turn_player;
                self.core.hint_timing[tp as usize] = timing::DAMAGE_CAL;
                self.bc_substep_window();
            } else {
                self.break_effect(false);
            }
        }
        false
    }

    /// Case 31: the battle happened.
    ///
    /// **Falls through to case 32 — conditionally.** With
    /// `core.effect_damage_step` set it returns *finished* instead, handing
    /// its destroyed-card group to the `DamageStep` waiting in
    /// `core.reserved`. The same case either ends the unit or runs the next
    /// one, depending on who called it.
    fn bc_battled(&mut self, state: &mut BattleCommandState) -> bool {
        let Some(attacker) = self.core.attacker else {
            return true;
        };
        self.core.flip_delayed = false;
        let fb: Vec<Chain> = self.core.new_fchain_b.drain(..).collect();
        for c in fb.into_iter().rev() {
            self.core.new_fchain.push_front(c);
        }
        let ob: Vec<Chain> = self.core.new_ochain_b.drain(..).collect();
        for c in ob.into_iter().rev() {
            self.core.new_ochain.push_front(c);
        }
        self.raise_single_event(
            attacker,
            Vec::new(),
            code::BATTLED,
            None,
            0,
            PLAYER_NONE,
            0,
            0,
        );
        let mut battled = vec![attacker];
        if let Some(t) = self.core.attack_target {
            battled.push(t);
            self.raise_single_event(t, Vec::new(), code::BATTLED, None, 0, PLAYER_NONE, 0, 1);
        }
        self.raise_event_over(battled, code::BATTLED, None, 0, PLAYER_NONE, 0, 0);
        self.process_single_event();
        self.process_instant_event();
        if self.core.effect_damage_step != 0 {
            // Hand the group to the parked `DamageStep`.
            if let Some(unit) = self.core.reserved.as_mut() {
                if let Kind::DamageStep { state: ds } = &mut unit.kind {
                    ds.cards_destroyed_by_battle = state.cards_destroyed_by_battle;
                }
            }
            return true;
        }
        // 32, not 31: case 32's body runs inline just below, so the
        // processor's increment has to land on 33. See the module note.
        self.set_step(32);
        self.bc_battled_window();
        false
    }

    /// Case 32: the window on the battle having happened.
    fn bc_battled_window(&mut self) -> bool {
        for p in 0..2u8 {
            self.messages.push(Message::Hint {
                kind: crate::host_question::hint::EVENT,
                player: p,
                value: 43,
            });
        }
        self.core.hint_timing[0] |= timing::BATTLED;
        self.core.hint_timing[1] |= timing::BATTLED;
        self.bc_substep_window();
        false
    }

    /// Case 33: send away what is still there to send.
    ///
    /// The group is **filtered again** first: a card that left the Monster
    /// Zone, or came back as a different instance, is dropped. So a
    /// destruction the battle decided on can still be undone by anything
    /// that moved the card in between.
    fn bc_send_destroyed(&mut self, state: &mut BattleCommandState) -> bool {
        if let Some(g) = state.cards_destroyed_by_battle {
            let kept: Vec<CardId> = self.groups[g]
                .iter()
                .copied()
                .filter(|&c| {
                    self.cards[c].current.location == location::MZONE
                        && (self.cards[c].fieldid_r == self.core.pre_field[0]
                            || self.cards[c].fieldid_r == self.core.pre_field[1])
                })
                .collect();
            self.groups[g] = kept.into_iter().collect();
            self.emplace_at(
                Kind::Destroy {
                    targets: g,
                    reason_effect: None,
                    reason: reason::BATTLE,
                    reason_player: PLAYER_NONE,
                },
                3,
            );
        }
        self.adjust_all();
        false
    }

    /// Case 34: the destroyed-by-battle events, in pairs.
    ///
    /// **`EVENT_BATTLE_DESTROYING` goes to the card that did it and
    /// `EVENT_BATTLE_DESTROYED` to the one it happened to**, and both are
    /// raised as single events *and* as field events, because the two are
    /// gathered from different registries.
    fn bc_destroyed_events(&mut self, state: &mut BattleCommandState) -> bool {
        let Some(attacker) = self.core.attacker else {
            return true;
        };
        let target = self.core.attack_target;
        state.cards_destroyed_by_battle = None;
        self.core.damage_calculated = true;
        self.core.selfdes_disabled = false;
        self.core.flip_delayed = false;
        let fb: Vec<Chain> = self.core.new_fchain_b.drain(..).collect();
        for c in fb.into_iter().rev() {
            self.core.new_fchain.push_front(c);
        }
        let ob: Vec<Chain> = self.core.new_ochain_b.drain(..).collect();
        for c in ob.into_iter().rev() {
            self.core.new_ochain.push_front(c);
        }

        let mut ing: Vec<CardId> = Vec::new();
        let mut ed: Vec<CardId> = Vec::new();
        if self.cards[attacker].is_status(status::BATTLE_DESTROYED)
            && self.cards[attacker].reason & reason::BATTLE != 0
        {
            if let Some(t) = target {
                let why = self.cards[attacker].reason;
                let tc = self.cards[t].current.controller;
                self.raise_single_event(
                    t,
                    Vec::new(),
                    code::BATTLE_DESTROYING,
                    None,
                    why,
                    tc,
                    0,
                    1,
                );
                self.raise_single_event(
                    attacker,
                    Vec::new(),
                    code::BATTLE_DESTROYED,
                    None,
                    why,
                    tc,
                    0,
                    0,
                );
                self.raise_single_event(attacker, Vec::new(), code::DESTROYED, None, why, tc, 0, 0);
                ing.push(t);
                ed.push(attacker);
            }
        }
        if let Some(t) = target {
            if self.cards[t].is_status(status::BATTLE_DESTROYED)
                && self.cards[t].reason & reason::BATTLE != 0
            {
                let why = self.cards[t].reason;
                let ac = self.cards[attacker].current.controller;
                self.raise_single_event(
                    attacker,
                    Vec::new(),
                    code::BATTLE_DESTROYING,
                    None,
                    why,
                    ac,
                    0,
                    0,
                );
                self.raise_single_event(t, Vec::new(), code::BATTLE_DESTROYED, None, why, ac, 0, 1);
                self.raise_single_event(t, Vec::new(), code::DESTROYED, None, why, ac, 0, 1);
                ing.push(attacker);
                ed.push(t);
            }
        }
        if !ing.is_empty() {
            self.raise_event_over(ing, code::BATTLE_DESTROYING, None, 0, 0, 0, 0);
        }
        if !ed.is_empty() {
            self.raise_event_over(ed.clone(), code::BATTLE_DESTROYED, None, 0, 0, 0, 0);
            self.raise_event_over(ed, code::DESTROYED, None, 0, 0, 0, 0);
        }
        self.raise_single_event(
            attacker,
            Vec::new(),
            code::DAMAGE_STEP_END,
            None,
            0,
            0,
            0,
            0,
        );
        if let Some(t) = target {
            self.raise_single_event(t, Vec::new(), code::DAMAGE_STEP_END, None, 0, 0, 0, 1);
        }
        self.raise_event(None, code::DAMAGE_STEP_END, None, 0, 0, 0, 0);
        self.cards[attacker].set_status(status::BATTLE_DESTROYED, false);
        if let Some(t) = target {
            self.cards[t].set_status(status::BATTLE_DESTROYED, false);
        }
        self.process_single_event();
        self.process_instant_event();
        for p in 0..2u8 {
            self.messages.push(Message::Hint {
                kind: crate::host_question::hint::EVENT,
                player: p,
                value: 44,
            });
        }
        self.emplace(Kind::PointEvent {
            skip: crate::point_event::PointEventSkip {
                new: true,
                ..Default::default()
            },
        });
        self.set_step(38);
        false
    }

    /// Case 39: the damage step is over.
    fn bc_step_over(&mut self, state: &mut BattleCommandState) -> bool {
        if let Some(a) = self.core.attacker {
            self.cards[a].set_status(status::OPPO_BATTLE, false);
        }
        if let Some(t) = self.core.attack_target {
            self.cards[t].set_status(status::OPPO_BATTLE, false);
        }
        if state.forced_attack {
            state.forced_attack_done = true;
            self.set_step(0);
        } else {
            self.set_step(RESTART);
        }
        self.infos.phase = crate::duel::phases::BATTLE_STEP;
        self.messages.push(Message::DamageStepEnd);
        self.reset_phase(crate::duel::phases::DAMAGE);
        self.adjust_all();
        // Called from `DamageStep`: finish rather than loop.
        self.core.effect_damage_step != 0
    }

    /// Case 40: the Battle Step ends, unless a chain is waiting.
    fn bc_end_of_step(&mut self, state: &mut BattleCommandState) -> bool {
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
            for h in handlers {
                self.cards[h].set_status(status::CHAINING, false);
            }
            self.emplace(Kind::SolveChain {
                skip: crate::solve_chain::SolveChainSkip::default(),
            });
            // **A forced attack does not restart here.** It has its own
            // exit through case 0.
            if !state.forced_attack {
                self.set_step(RESTART);
            }
            return false;
        }
        self.reset_phase(crate::duel::phases::BATTLE_STEP);
        self.adjust_all();
        false
    }

    /// Case 41: the Battle Phase is over. Decide whether a second one is
    /// due, and whether it may be declined.
    ///
    /// **A second Battle Phase is optional only if *every*
    /// `EFFECT_BP_TWICE` says so** — one effect with a value other than 1
    /// makes it compulsory for all of them.
    fn bc_phase_over(&mut self, state: &mut BattleCommandState) -> bool {
        let tp = self.infos.turn_player;
        state.second_battle_phase_is_optional = true;
        let effects = self.filter_player_effect(tp, code::BP_TWICE);
        for e in &effects {
            let optional = self
                .effects
                .get(*e)
                .is_some_and(|x| x.value != 0 || x.has_function_value())
                && self.effect_plain_value(*e) == 1;
            if !optional {
                state.second_battle_phase_is_optional = false;
                break;
            }
        }
        state.repeat_battle_phase = !effects.is_empty();
        self.infos.phase = crate::duel::phases::BATTLE;
        self.emplace(Kind::PhaseEvent {
            phase: crate::duel::phases::BATTLE,
            state: Default::default(),
        });
        self.adjust_all();
        false
    }

    /// Case 42: hand back to `Turn`, asking about the second Battle Phase
    /// first if it is optional.
    fn bc_finish(&mut self, state: &mut BattleCommandState) -> bool {
        self.core.attacker = None;
        self.core.attack_target = None;
        if state.repeat_battle_phase && state.second_battle_phase_is_optional {
            let tp = self.infos.turn_player;
            self.emplace(Kind::SelectYesNo {
                player: tp,
                description: 32,
            });
            return false;
        }
        self.core.returns.set(i32::from(state.phase_to_change_to));
        self.core
            .returns
            .set_i32(1, i32::from(state.repeat_battle_phase));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{card_type, Card, CardData};
    use crate::duel::phases;
    use crate::effect::{effect_type, flag, Effect};
    use crate::processor::Status;

    fn field() -> Field {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        f.infos.turn_player = 0;
        f.infos.phase = phases::BATTLE_STEP;
        f
    }

    fn monster(f: &mut Field, player: u8, seat: u32, atk: i32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 18036057 + seat + u32::from(player) * 100,
                type_: card_type::MONSTER,
                level: 4,
                attack: atk,
                defense: atk,
                ..Default::default()
            },
            player,
        );
        c.current.controller = player;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(player, id, location::MZONE, seat, false);
        f.cards[id].current.position = position::FACEUP_ATTACK;
        let fid = f.next_field_id_raw();
        f.cards[id].fieldid = fid;
        f.cards[id].fieldid_r = fid;
        id
    }

    fn single(f: &mut Field, card: CardId, code_: u32, value: i64) -> EffectId {
        let mut e = Effect::new(effect_type::SINGLE, code_);
        e.owner = Some(card);
        e.handler = Some(card);
        e.value = value;
        let id = f.new_effect(e);
        f.cards[card].single_effect.insert(code_, id);
        f.cards[card].indexer.insert(id);
        id
    }

    fn player_effect(f: &mut Field, code_: u32, player: u8, value: i64) -> EffectId {
        let seat = f.players[player as usize]
            .mzone
            .iter()
            .rposition(Option::is_none)
            .unwrap_or(6) as u32;
        let a = monster(f, player, seat, 0);
        let mut e = Effect::new(effect_type::FIELD, code_);
        e.owner = Some(a);
        e.handler = Some(a);
        e.flag[0] = flag::PLAYER_TARGET | flag::ABSOLUTE_TARGET;
        e.range = u16::from(location::MZONE);
        if player == 0 {
            e.s_range = 1;
        } else {
            e.o_range = 1;
        }
        e.value = value;
        let id = f.new_effect(e);
        f.add_effect(id, player);
        id
    }

    fn start_at(f: &mut Field, step: u16) {
        f.emplace_at(
            Kind::BattleCommand {
                state: Box::default(),
            },
            step,
        );
    }

    fn step(f: &mut Field) -> Status {
        f.process()
    }

    /// Run until the machine stops. Anything it asks is a test setup
    /// error, so this panics rather than answering.
    fn run(f: &mut Field) -> Status {
        for _ in 0..512 {
            match f.process() {
                Status::Continue => continue,
                Status::Awaiting => panic!("unexpected question: {:?}", f.messages.last()),
                other => return other,
            }
        }
        panic!("did not settle");
    }

    fn queued(f: &Field, pred: impl Fn(&Kind) -> bool) -> bool {
        f.core
            .units
            .iter()
            .chain(f.core.subunits.iter())
            .any(|u| pred(&u.kind))
    }

    /// Whether the predicate ever held while running to a stop.
    fn ever(f: &mut Field, pred: impl Fn(&Field) -> bool) -> bool {
        for _ in 0..512 {
            if pred(f) {
                return true;
            }
            match f.process() {
                Status::Continue => continue,
                _ => return pred(f),
            }
        }
        panic!("did not settle");
    }

    fn menu(f: &Field) -> Option<&Message> {
        f.messages
            .iter()
            .rev()
            .find(|m| matches!(m, Message::SelectBattleCmd { .. }))
    }

    mod the_gather {
        use super::*;

        /// A monster with an opposing target is offered.
        #[test]
        fn an_attacker_with_a_target_is_offered() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 1800);
            monster(&mut f, 1, 0, 1200);
            start_at(&mut f, 0);
            assert!(ever(&mut f, |f| menu(f).is_some()));
            let Some(Message::SelectBattleCmd { attackable, .. }) = menu(&f) else {
                panic!("no menu");
            };
            assert_eq!(attackable.len(), 1);
            assert_eq!(attackable[0].code, f.cards[a].data.code);
        }

        /// **A monster with nothing to attack is not offered** — neither a
        /// target nor a direct attack.
        #[test]
        fn an_attacker_with_nothing_to_do_is_not_offered() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 1800);
            single(&mut f, a, code::CANNOT_DIRECT_ATTACK, 1);
            start_at(&mut f, 0);
            ever(&mut f, |f| menu(f).is_some());
            let Some(Message::SelectBattleCmd { attackable, .. }) = menu(&f) else {
                panic!("no menu");
            };
            assert!(attackable.is_empty(), "no target and no direct attack");
        }

        /// **`EFFECT_FIRST_ATTACK` replaces the whole list**, rather than
        /// reordering it: a monster that must attack first is the *only*
        /// one offered.
        #[test]
        fn first_attack_replaces_the_list() {
            let mut f = field();
            monster(&mut f, 0, 0, 1800);
            let b = monster(&mut f, 0, 1, 1000);
            monster(&mut f, 1, 0, 1200);
            single(&mut f, b, code::FIRST_ATTACK, 1);
            start_at(&mut f, 0);
            ever(&mut f, |f| menu(f).is_some());
            let Some(Message::SelectBattleCmd { attackable, .. }) = menu(&f) else {
                panic!("no menu");
            };
            assert_eq!(attackable.len(), 1, "only the one that must go first");
            assert_eq!(attackable[0].code, f.cards[b].data.code);
        }

        /// **`EFFECT_MUST_ATTACK` closes both exits.** A player with an
        /// obliged monster may neither go to Main Phase 2 nor end the turn.
        #[test]
        fn must_attack_closes_both_exits() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 1800);
            monster(&mut f, 1, 0, 1200);
            single(&mut f, a, code::MUST_ATTACK, 1);
            start_at(&mut f, 0);
            ever(&mut f, |f| menu(f).is_some());
            let Some(Message::SelectBattleCmd { to_m2, to_ep, .. }) = menu(&f) else {
                panic!("no menu");
            };
            assert!(!to_m2, "no Main Phase 2");
            assert!(!to_ep, "and no End Phase");
        }

        /// **`EFFECT_CANNOT_M2` closes only one of them.**
        #[test]
        fn cannot_m2_closes_only_main_phase_2() {
            let mut f = field();
            monster(&mut f, 0, 0, 1800);
            monster(&mut f, 1, 0, 1200);
            player_effect(&mut f, code::CANNOT_M2, 0, 1);
            start_at(&mut f, 0);
            ever(&mut f, |f| menu(f).is_some());
            let Some(Message::SelectBattleCmd { to_m2, to_ep, .. }) = menu(&f) else {
                panic!("no menu");
            };
            assert!(!to_m2);
            assert!(to_ep, "the End Phase is still open");
        }

        /// **`EFFECT_CANNOT_ATTACK_ANNOUNCE` on the player empties the
        /// list** without consulting any monster.
        #[test]
        fn a_player_who_cannot_announce_gets_no_attackers() {
            let mut f = field();
            monster(&mut f, 0, 0, 1800);
            monster(&mut f, 1, 0, 1200);
            player_effect(&mut f, code::CANNOT_ATTACK_ANNOUNCE, 0, 1);
            start_at(&mut f, 0);
            ever(&mut f, |f| menu(f).is_some());
            let Some(Message::SelectBattleCmd { attackable, .. }) = menu(&f) else {
                panic!("no menu");
            };
            assert!(attackable.is_empty());
        }

        /// **Only the activate registry is filtered on speed.** A monster's
        /// `EFFECT_TYPE_ACTIVATE` is speed 0 and is kept off the menu; a
        /// quick effect on the same monster is offered.
        #[test]
        fn the_activate_registry_is_filtered_on_speed() {
            // A monster's activate effect: speed 0, not offered.
            let mut f = field();
            let c = monster(&mut f, 0, 0, 1800);
            let mut e = Effect::new(
                effect_type::FIELD | effect_type::ACTIONS | effect_type::ACTIVATE,
                code::FREE_CHAIN,
            );
            e.owner = Some(c);
            e.handler = Some(c);
            e.range = u16::from(location::MZONE);
            let id = f.new_effect(e);
            f.add_effect(id, 0);
            start_at(&mut f, 0);
            ever(&mut f, |f| menu(f).is_some());
            let Some(Message::SelectBattleCmd { activatable, .. }) = menu(&f) else {
                panic!("no menu");
            };
            assert!(activatable.is_empty(), "speed 0 is not offered");

            // The same monster with a quick effect: speed 2, offered.
            let mut f = field();
            let c = monster(&mut f, 0, 0, 1800);
            let mut e = Effect::new(
                effect_type::FIELD | effect_type::ACTIONS | effect_type::QUICK_O,
                code::FREE_CHAIN,
            );
            e.owner = Some(c);
            e.handler = Some(c);
            e.range = u16::from(location::MZONE);
            e.description = 99;
            let id = f.new_effect(e);
            f.add_effect(id, 0);
            start_at(&mut f, 0);
            ever(&mut f, |f| menu(f).is_some());
            let Some(Message::SelectBattleCmd { activatable, .. }) = menu(&f) else {
                panic!("no menu");
            };
            assert_eq!(activatable.len(), 1, "the quick registry is not filtered");
        }

        /// **The list is rebuilt, not appended to.** A stale entry from
        /// the previous pass does not survive.
        #[test]
        fn the_attacker_list_is_rebuilt() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 1800);
            monster(&mut f, 1, 0, 1200);
            // A monster that is not there any more.
            f.core.attackable_cards = vec![999];
            start_at(&mut f, 0);
            ever(&mut f, |f| menu(f).is_some());
            assert_eq!(f.core.attackable_cards, vec![a], "only the real one");
        }

        /// **A monster that cannot *announce* is not offered**, even with
        /// a legal target — `is_capable_attack_announce` is stricter than
        /// `is_capable_attack`.
        #[test]
        fn a_monster_that_cannot_announce_is_not_offered() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 1800);
            monster(&mut f, 1, 0, 1200);
            single(&mut f, a, code::CANNOT_ATTACK_ANNOUNCE, 1);
            start_at(&mut f, 0);
            ever(&mut f, |f| menu(f).is_some());
            let Some(Message::SelectBattleCmd { attackable, .. }) = menu(&f) else {
                panic!("no menu");
            };
            assert!(attackable.is_empty(), "it can attack, but not declare one");
        }

        /// **The continuous registry offers only the turn player's own
        /// effects** — the one filter the other two registries do not have.
        #[test]
        fn a_continuous_effect_of_the_opponents_is_not_offered() {
            for (owner, offered) in [(0u8, 1usize), (1, 0)] {
                let mut f = field();
                let c = monster(&mut f, owner, 0, 1000);
                let mut e = Effect::new(
                    effect_type::FIELD | effect_type::ACTIONS | effect_type::CONTINUOUS,
                    code::FREE_CHAIN,
                );
                e.owner = Some(c);
                e.handler = Some(c);
                e.range = u16::from(location::MZONE);
                e.s_range = 1;
                e.o_range = 1;
                let id = f.new_effect(e);
                f.add_effect(id, owner);
                start_at(&mut f, 0);
                ever(&mut f, |f| menu(f).is_some());
                let Some(Message::SelectBattleCmd { activatable, .. }) = menu(&f) else {
                    panic!("no menu");
                };
                assert_eq!(activatable.len(), offered, "owned by player {owner}");
            }
        }

        /// **`EFFECT_BP_TWICE` is read on the skip path too**, so a
        /// skipped Battle Phase can still be followed by a second one.
        #[test]
        fn a_skipped_phase_still_notices_bp_twice() {
            let mut f = field();
            player_effect(&mut f, code::SKIP_BP, 0, 0);
            player_effect(&mut f, code::BP_TWICE, 0, 1);
            start_at(&mut f, 0);
            step(&mut f);
            let repeat = f.core.units.iter().find_map(|u| match &u.kind {
                Kind::BattleCommand { state } => Some(state.repeat_battle_phase),
                _ => None,
            });
            assert_eq!(repeat, Some(true));
        }

        /// **`EFFECT_SKIP_BP` leaves before anything is gathered**, and its
        /// *value* decides how much is skipped: zero resets only the Battle
        /// Step and runs the phase event, non-zero resets the whole phase
        /// and runs nothing.
        #[test]
        fn skip_bp_leaves_and_its_value_decides_how() {
            let mut f = field();
            monster(&mut f, 0, 0, 1800);
            player_effect(&mut f, code::SKIP_BP, 0, 0);
            start_at(&mut f, 0);
            step(&mut f);
            assert!(menu(&f).is_none(), "nothing offered");
            assert_eq!(f.infos.phase, phases::BATTLE, "the phase moved on");
            assert!(
                queued(&f, |k| matches!(
                    k,
                    Kind::PhaseEvent {
                        phase: phases::BATTLE,
                        ..
                    }
                )),
                "and the phase event runs"
            );

            let mut f = field();
            monster(&mut f, 0, 0, 1800);
            player_effect(&mut f, code::SKIP_BP, 0, 1);
            start_at(&mut f, 0);
            step(&mut f);
            assert_ne!(f.infos.phase, phases::BATTLE, "a non-zero value does not");
            assert!(!queued(&f, |k| matches!(k, Kind::PhaseEvent { .. })));
        }
    }

    mod the_menu_answer {
        use super::*;

        fn answer(kind: u32, index: u32) -> i32 {
            ((index << 16) | kind) as i32
        }

        /// **Anything but activate-or-attack leaves the phase**, opening
        /// the opponent's last window first.
        #[test]
        fn leaving_opens_the_opponents_window() {
            let mut f = field();
            f.core.returns.set(answer(2, 0));
            start_at(&mut f, 1);
            step(&mut f);
            assert!(queued(&f, |k| matches!(
                k,
                Kind::QuickEffect { player: 1, .. }
            )));
            assert_ne!(f.core.hint_timing[0] & timing::BATTLE_STEP_END, 0);
            assert_eq!(f.infos.priorities, [1, 0]);
        }

        /// **Declaring an attack takes the named monster** and pins it.
        #[test]
        fn declaring_an_attack_pins_the_attacker() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 1800);
            let b = monster(&mut f, 0, 1, 1000);
            f.core.attackable_cards = vec![a, b];
            f.cards[b].set_status(status::ATTACK_CANCELED, true);
            // A stale value from a previous attack, so the pin is
            // observable.
            f.cards[b].attack_controler = 1;
            f.core.returns.set(answer(1, 1));
            start_at(&mut f, 1);
            step(&mut f);
            assert_eq!(f.core.attacker, Some(b), "the second, not the first");
            assert_eq!(f.core.pre_field[0], f.cards[b].fieldid_r);
            // Poisoned first: `attack_controler` defaults to 0, so
            // asserting 0 against a fresh card cannot tell "pinned" from
            // "never written".
            assert_eq!(
                f.cards[b].attack_controler, 0,
                "pinned to the current controller"
            );
            assert!(
                !f.cards[b].is_status(status::ATTACK_CANCELED),
                "and its cancelled flag is cleared"
            );
        }

        /// **An attack cost makes the attack uncancelable**, because the
        /// player is about to pay for something they cannot then undo.
        #[test]
        fn an_attack_cost_removes_the_cancel() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 1800);
            let e = single(&mut f, a, code::ATTACK_COST, 0);
            if let Some(x) = f.effects.get_mut(e) {
                x.operation = Some(|_, _| crate::effect::Yield::Done(0));
            }
            f.core.attackable_cards = vec![a];
            f.core.attack_cancelable = true;
            f.core.returns.set(answer(1, 0));
            start_at(&mut f, 1);
            step(&mut f);
            assert!(!f.core.attack_cancelable);
            assert!(
                queued(&f, |k| matches!(k, Kind::ExecuteOperation { .. })),
                "and the cost is run"
            );
        }

        /// **A cost with no operation is not collected**, and does not
        /// take the cancel away — there is nothing for the player to be
        /// committed to.
        #[test]
        fn a_cost_without_an_operation_is_ignored() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 1800);
            single(&mut f, a, code::ATTACK_COST, 0); // no operation
            f.core.attackable_cards = vec![a];
            f.core.attack_cancelable = true;
            f.core.returns.set(answer(1, 0));
            start_at(&mut f, 1);
            step(&mut f);
            assert!(f.core.tpchain.is_empty(), "nothing to run");
            assert!(f.core.attack_cancelable, "so the cancel stays");
        }

        /// **Two costs are put in an order first.**
        #[test]
        fn two_attack_costs_are_sorted() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 1800);
            for _ in 0..2 {
                let mut e = Effect::new(effect_type::SINGLE, code::ATTACK_COST);
                e.owner = Some(a);
                e.handler = Some(a);
                e.operation = Some(|_, _| crate::effect::Yield::Done(0));
                let id = f.new_effect(e);
                f.cards[a].single_effect.insert(code::ATTACK_COST, id);
                f.cards[a].indexer.insert(id);
            }
            f.core.attackable_cards = vec![a];
            f.core.returns.set(answer(1, 0));
            start_at(&mut f, 1);
            step(&mut f);
            assert!(queued(&f, |k| matches!(k, Kind::SortChain { .. })));
            assert_eq!(f.core.tpchain.len(), 2);
        }
    }

    mod the_attack {
        use super::*;

        /// **A cancelled attacker fails the declaration** rather than
        /// proceeding.
        #[test]
        fn a_cancelled_attacker_fails_the_declaration() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 1800);
            f.core.attacker = Some(a);
            f.cards[a].set_status(status::ATTACK_CANCELED, true);
            start_at(&mut f, 3);
            step(&mut f);
            // Jumps to case 6, which with no target list abandons it.
            assert!(
                !f.messages
                    .iter()
                    .any(|m| matches!(m, Message::Attack { .. })),
                "nothing announced"
            );
        }

        /// **An unpaid, uncancelable cost abandons the attack.**
        /// **Case 31 leaves the step at 32, not 31.**
        ///
        /// It runs case 32's body inline — the reference's
        /// `[[fallthrough]]` — so the processor's increment has to land on
        /// 33. Writing 31, which the ordinary step rule would suggest,
        /// sends the machine back through case 32 and opens the
        /// battle-window a second time.
        ///
        /// Pinned on both halves: the step, and the fact that exactly one
        /// window opens. The step alone would pass on a version that
        /// forgot the inline call.
        #[test]
        fn the_fallthrough_case_runs_the_next_case_once_and_skips_past_it() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 1800);
            f.core.attacker = Some(a);
            f.core.attack_target = None;
            f.core.effect_damage_step = 0;
            start_at(&mut f, 31);
            step(&mut f);

            let at = f.core.units.iter().find_map(|u| match u.kind {
                Kind::BattleCommand { .. } => Some(u.step),
                _ => None,
            });
            // The unit is read *after* the processor's increment, so 33
            // here is the case that runs next. A `set_step(31)` shows up
            // as 32 — case 32 about to run for the second time.
            assert_eq!(at, Some(33), "case 33 runs next, not 32 again");

            let windows = f
                .messages
                .iter()
                .filter(|m| {
                    matches!(
                        m,
                        Message::Hint {
                            value: 43,
                            player: 0,
                            ..
                        }
                    )
                })
                .count();
            assert_eq!(windows, 1, "the battled window opens exactly once");
        }

        #[test]
        fn an_unpaid_uncancelable_cost_abandons_it() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 1800);
            f.core.attacker = Some(a);
            f.core.attack_cost_paid = false;
            f.core.attack_cancelable = false;
            start_at(&mut f, 3);
            step(&mut f);
            let at_top = f.core.units.iter().find_map(|u| match u.kind {
                Kind::BattleCommand { .. } => Some(u.step),
                _ => None,
            });
            assert_eq!(at_top, Some(0), "back to the menu");
        }

        /// **A cancelled attacker fails the declaration and is counted**
        /// — the attack is spent even though nothing happened.
        #[test]
        fn a_cancelled_attacker_is_counted_as_a_failed_declaration() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 1800);
            f.core.attacker = Some(a);
            f.cards[a].set_status(status::ATTACK_CANCELED, true);
            start_at(&mut f, 3);
            step(&mut f);
            let failed = f.core.units.iter().find_map(|u| match &u.kind {
                Kind::BattleCommand { state } => Some((u.step, state.attack_announce_failed)),
                _ => None,
            });
            assert_eq!(failed, Some((7, true)), "recorded, and on to case 7");
        }

        /// **A cancelable attack survives an unpaid cost.** Only an
        /// uncancelable one is abandoned, because only then has the player
        /// committed.
        #[test]
        fn a_cancelable_attack_survives_an_unpaid_cost() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 1800);
            f.core.attacker = Some(a);
            f.core.attack_cost_paid = false;
            f.core.attack_cancelable = true;
            start_at(&mut f, 3);
            step(&mut f);
            let at = f.core.units.iter().find_map(|u| match u.kind {
                Kind::BattleCommand { .. } => Some(u.step),
                _ => None,
            });
            assert_eq!(at, Some(4), "on to the target question, not abandoned");
        }

        /// **Only one target means no question**, and the attack proceeds
        /// with it.
        #[test]
        fn a_single_target_is_taken_without_asking() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 1800);
            let x = monster(&mut f, 1, 0, 1200);
            single(&mut f, a, code::CANNOT_DIRECT_ATTACK, 1);
            f.core.attacker = Some(a);
            start_at(&mut f, 4);
            step(&mut f);
            assert!(
                queued(&f, |k| matches!(k, Kind::SelectCard { .. })),
                "one target still asks — the reference does not short-circuit here"
            );
            assert_eq!(f.core.select_cards, vec![x]);
        }

        /// **A direct attack with monsters available asks whether to go
        /// direct**, and `EFFECT_PATRICIAN_OF_DARKNESS` asks the
        /// *opponent* instead.
        #[test]
        fn the_direct_attack_question_can_be_asked_of_the_opponent() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 1800);
            monster(&mut f, 1, 0, 1200);
            single(&mut f, a, code::DIRECT_ATTACK, 1);
            f.core.attacker = Some(a);
            start_at(&mut f, 4);
            step(&mut f);
            assert!(
                queued(&f, |k| matches!(k, Kind::SelectYesNo { player: 0, .. })),
                "the turn player is asked"
            );

            let mut f = field();
            let a = monster(&mut f, 0, 0, 1800);
            monster(&mut f, 1, 0, 1200);
            single(&mut f, a, code::DIRECT_ATTACK, 1);
            player_effect(&mut f, code::PATRICIAN_OF_DARKNESS, 0, 1);
            f.core.attacker = Some(a);
            start_at(&mut f, 4);
            step(&mut f);
            assert!(
                queued(&f, |k| matches!(
                    k,
                    Kind::SelectEffectYesNo { player: 1, .. }
                )),
                "the opponent is asked instead"
            );
        }

        /// **No target and no direct attack fails the declaration**, and
        /// `arg.step = 6` reaches case **7** — which is where
        /// `attack_announce_failed` is read and the attack wound up.
        #[test]
        fn no_target_at_all_fails_the_declaration() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 1800);
            single(&mut f, a, code::CANNOT_DIRECT_ATTACK, 1);
            f.core.attacker = Some(a);
            start_at(&mut f, 4);
            step(&mut f);
            let at = f.core.units.iter().find_map(|u| match &u.kind {
                Kind::BattleCommand { state } => Some((u.step, state.attack_announce_failed)),
                _ => None,
            });
            assert_eq!(
                at,
                Some((7, true)),
                "case 7 next, with the failure recorded"
            );
        }

        /// **Patrician takes the cancel away.** The opponent choosing
        /// your target may not decline on your behalf.
        #[test]
        fn patrician_removes_the_cancel() {
            for (patrician, expect) in [(false, true), (true, false)] {
                let mut f = field();
                let a = monster(&mut f, 0, 0, 1800);
                monster(&mut f, 1, 0, 1200);
                if patrician {
                    player_effect(&mut f, code::PATRICIAN_OF_DARKNESS, 0, 1);
                }
                f.core.attacker = Some(a);
                f.core.attack_cancelable = true;
                f.core.select_cards = vec![1];
                f.core.returns.set(0);
                start_at(&mut f, 5);
                step(&mut f);
                let cancelable = f
                    .core
                    .units
                    .iter()
                    .chain(f.core.subunits.iter())
                    .find_map(|u| match u.kind {
                        Kind::SelectCard { cancelable, .. } => Some(cancelable),
                        _ => None,
                    });
                assert_eq!(cancelable, Some(expect), "patrician = {patrician}");
            }
        }

        /// **`-2` in `returns` means a direct attack**, and clears the
        /// target rather than picking one.
        ///
        /// The selection list must be **non-empty** for this to prove
        /// anything: with nothing selected, "take `-2` to mean direct" and
        /// "take the first selection" both give `None`, and the test
        /// passes against either.
        #[test]
        fn minus_two_means_a_direct_attack() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 1800);
            let x = monster(&mut f, 1, 0, 1200);
            f.core.attacker = Some(a);
            // A selection is present and must be *ignored*.
            f.core.return_cards.list.push(x);
            f.core.returns.set(-2);
            start_at(&mut f, 6);
            step(&mut f);
            assert!(
                f.core.attack_target.is_none(),
                "direct, despite {x:?} being selected"
            );
            assert_eq!(f.core.pre_field[1], 0);
        }

        /// **A named target is taken and pinned.** The counterpart to the
        /// `-2` case above: anything other than `-2` uses the selection.
        #[test]
        fn a_named_target_is_taken_and_pinned() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 1800);
            let x = monster(&mut f, 1, 0, 1200);
            f.core.attacker = Some(a);
            f.core.return_cards.list.push(x);
            f.core.returns.set(0);
            start_at(&mut f, 6);
            step(&mut f);
            assert_eq!(f.core.attack_target, Some(x));
            assert_eq!(
                f.core.pre_field[1], f.cards[x].fieldid_r,
                "and pinned, so a swap is noticed later"
            );
        }

        /// **Backing out restarts the loop** — unless this was a replay,
        /// which goes on to finish the attack properly.
        #[test]
        fn backing_out_restarts_unless_replaying() {
            let mut f = field();
            f.core.return_cards.canceled = true;
            start_at(&mut f, 6);
            step(&mut f);
            let at = f.core.units.iter().find_map(|u| match u.kind {
                Kind::BattleCommand { .. } => Some(u.step),
                _ => None,
            });
            assert_eq!(at, Some(0), "back to the menu");

            let mut f = field();
            f.core.return_cards.canceled = true;
            f.emplace_at(
                Kind::BattleCommand {
                    state: Box::new(BattleCommandState {
                        is_replaying_attack: true,
                        ..Default::default()
                    }),
                },
                6,
            );
            step(&mut f);
            let at = f.core.units.iter().find_map(|u| match u.kind {
                Kind::BattleCommand { .. } => Some(u.step),
                _ => None,
            });
            assert_eq!(at, Some(13), "a cancelled replay finishes the attack");
        }

        /// **A replay does not count the declaration twice.**
        #[test]
        fn a_replay_does_not_count_the_announcement_again() {
            for (replaying, expected) in [(false, 1u16), (true, 0)] {
                let mut f = field();
                let a = monster(&mut f, 0, 0, 1800);
                f.core.attacker = Some(a);
                f.emplace_at(
                    Kind::BattleCommand {
                        state: Box::new(BattleCommandState {
                            is_replaying_attack: replaying,
                            ..Default::default()
                        }),
                    },
                    7,
                );
                step(&mut f);
                assert_eq!(
                    f.cards[a].attack_announce_count, expected,
                    "replaying = {replaying}"
                );
                assert_eq!(f.core.attack_state_count[0], expected);
            }
        }

        /// **A failed declaration spends the attack**: `announce_count`
        /// goes up and the chain attack is cleared, so the monster cannot
        /// simply try again.
        #[test]
        fn a_failed_declaration_still_spends_the_attack() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 1800);
            f.core.attacker = Some(a);
            f.core.chain_attack = true;
            f.emplace_at(
                Kind::BattleCommand {
                    state: Box::new(BattleCommandState {
                        attack_announce_failed: true,
                        ..Default::default()
                    }),
                },
                7,
            );
            step(&mut f);
            assert_eq!(f.cards[a].announce_count, 1, "spent");
            assert!(!f.core.chain_attack);
        }

        /// **A replay does not raise `EVENT_ATTACK_ANNOUNCE` again.** The
        /// attack was already announced; only the board pin is refreshed.
        #[test]
        fn a_replay_does_not_announce_again() {
            for (replaying, expect) in [(false, true), (true, false)] {
                let mut f = field();
                let a = monster(&mut f, 0, 0, 1800);
                f.core.attacker = Some(a);
                f.emplace_at(
                    Kind::BattleCommand {
                        state: Box::new(BattleCommandState {
                            is_replaying_attack: replaying,
                            ..Default::default()
                        }),
                    },
                    8,
                );
                step(&mut f);
                let raised = f
                    .core
                    .queue_event
                    .iter()
                    .chain(f.core.instant_event.iter())
                    .chain(f.core.used_event.iter())
                    .any(|e| e.event_code == code::ATTACK_ANNOUNCE);
                assert_eq!(raised, expect, "replaying = {replaying}");
            }
        }

        /// **Case 8 pins the opposing board**, which is what a later
        /// replay check compares against.
        #[test]
        fn the_announcement_pins_the_opposing_board() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 1800);
            let x = monster(&mut f, 1, 0, 1200);
            let y = monster(&mut f, 1, 1, 1000);
            f.core.attacker = Some(a);
            f.core.attack_target = Some(x);
            start_at(&mut f, 8);
            step(&mut f);
            let expect: std::collections::BTreeSet<u32> =
                [f.cards[x].fieldid_r, f.cards[y].fieldid_r]
                    .into_iter()
                    .collect();
            assert_eq!(f.core.opp_mzone, expect);
            assert!(!f.core.attack_rollback, "and the rollback flag is cleared");
            assert!(f
                .messages
                .iter()
                .any(|m| matches!(m, Message::Attack { .. })));
        }
    }

    mod the_damage_step {
        use super::*;

        /// **The rollback flag is cleared at the announcement**, so a
        /// rollback from a previous attack does not carry over.
        #[test]
        fn the_announcement_clears_a_stale_rollback() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 1800);
            f.core.attacker = Some(a);
            f.core.attack_rollback = true;
            start_at(&mut f, 8);
            step(&mut f);
            assert!(!f.core.attack_rollback);
        }

        /// **The battle statuses are cleared before the calculation.** A
        /// card that lost a previous battle does not start this one
        /// already marked.
        #[test]
        fn the_battle_statuses_are_cleared_first() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 1800);
            let x = monster(&mut f, 1, 0, 1200);
            f.cards[a].set_status(status::BATTLE_RESULT, true);
            f.cards[a].set_status(status::BATTLE_DESTROYED, true);
            f.core.attacker = Some(a);
            f.core.attack_target = Some(x);
            start_at(&mut f, 26);
            step(&mut f);
            assert!(
                !f.cards[a].is_status(status::BATTLE_RESULT),
                "the winner is not marked"
            );
            assert!(!f.cards[a].is_status(status::BATTLE_DESTROYED));
        }

        /// **`OPPO_BATTLE` is only for battles between the two sides.** A
        /// monster fighting one of its own is not marked.
        #[test]
        fn a_same_side_battle_is_not_marked_opposing() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 1800);
            let own = monster(&mut f, 0, 1, 1200);
            f.core.attacker = Some(a);
            f.core.attack_target = Some(own);
            start_at(&mut f, 26);
            step(&mut f);
            assert!(!f.cards[a].is_status(status::OPPO_BATTLE));
            assert!(!f.cards[own].is_status(status::OPPO_BATTLE));
        }

        /// **Indestructible also clears the flag in the message**, not
        /// just the status — the host is told the monster survived.
        #[test]
        fn indestructible_clears_the_reported_destruction() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 1800);
            let x = monster(&mut f, 1, 0, 1200);
            single(&mut f, x, code::INDESTRUCTABLE_BATTLE, 1);
            f.core.attacker = Some(a);
            f.core.attack_target = Some(x);
            start_at(&mut f, 26);
            step(&mut f);
            let destroyed = f.messages.iter().find_map(|m| match m {
                Message::Battle {
                    target_destroyed, ..
                } => Some(*target_destroyed),
                _ => None,
            });
            assert_eq!(destroyed, Some(false), "reported as surviving");
        }

        /// **A card that has left the Monster Zone is not destroyed**,
        /// even with a matching field id.
        #[test]
        fn a_card_outside_the_monster_zone_is_not_destroyed() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 1800);
            let x = monster(&mut f, 1, 0, 1200);
            f.core.attacker = Some(a);
            f.core.attack_target = Some(x);
            f.core.pre_field[0] = f.cards[a].fieldid_r;
            f.cards[x].set_status(status::BATTLE_RESULT, true);
            // It left, but keeps its field id.
            let fid = f.cards[x].fieldid_r;
            f.remove_card(x);
            f.add_card(1, x, location::GRAVE, 0, false);
            f.cards[x].fieldid_r = fid;
            f.core.pre_field[1] = fid;
            start_at(&mut f, 28);
            step(&mut f);
            assert!(
                !queued(&f, |k| matches!(k, Kind::Destroy { .. })),
                "not in a Monster Zone, so not destroyed"
            );
        }

        /// Case 19 enters the Damage Phase and counts the battle.
        #[test]
        fn it_enters_the_damage_phase() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 1800);
            let x = monster(&mut f, 1, 0, 1200);
            f.core.attacker = Some(a);
            f.core.attack_target = Some(x);
            start_at(&mut f, 19);
            step(&mut f);
            assert_eq!(f.infos.phase, phases::DAMAGE);
            assert!(f.core.selfdes_disabled);
            assert!(f.core.flip_delayed);
            assert!(!f.core.damage_calculated);
            assert_eq!(f.cards[a].attacked_count, 1);
            assert_eq!(f.core.battled_count[0], 1);
            assert_eq!(f.core.pre_field[1], f.cards[x].fieldid_r);
        }

        /// **Case 26 is where the battle is decided**, and it writes the
        /// battle-result statuses that case 28 acts on.
        #[test]
        fn the_calculation_marks_the_loser() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 1800);
            let x = monster(&mut f, 1, 0, 1200);
            f.core.attacker = Some(a);
            f.core.attack_target = Some(x);
            start_at(&mut f, 26);
            step(&mut f);
            assert!(
                f.cards[x].is_status(status::BATTLE_RESULT),
                "the weaker one is marked"
            );
            assert!(!f.cards[a].is_status(status::BATTLE_RESULT));
            assert_eq!(f.core.battle_damage, [0, 600]);
            assert!(f.core.damage_calculated);
            assert!(f
                .messages
                .iter()
                .any(|m| matches!(m, Message::Battle { .. })));
        }

        /// **Both are marked `OPPO_BATTLE`** when they belong to different
        /// players — which is how an effect tells a battle between the two
        /// sides from one within a side.
        #[test]
        fn opposing_battles_are_marked() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 1800);
            let x = monster(&mut f, 1, 0, 1200);
            f.core.attacker = Some(a);
            f.core.attack_target = Some(x);
            start_at(&mut f, 26);
            step(&mut f);
            assert!(f.cards[a].is_status(status::OPPO_BATTLE));
            assert!(f.cards[x].is_status(status::OPPO_BATTLE));
        }

        /// **`EFFECT_INDESTRUCTABLE_BATTLE` takes the destruction away**
        /// and announces itself with a hint.
        #[test]
        fn indestructible_in_battle_survives_and_announces() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 1800);
            let x = monster(&mut f, 1, 0, 1200);
            single(&mut f, x, code::INDESTRUCTABLE_BATTLE, 1);
            f.core.attacker = Some(a);
            f.core.attack_target = Some(x);
            start_at(&mut f, 26);
            step(&mut f);
            assert!(
                !f.cards[x].is_status(status::BATTLE_RESULT),
                "not marked for destruction"
            );
            assert_eq!(f.core.battle_damage, [0, 600], "but the damage still lands");
            assert!(
                f.messages
                    .iter()
                    .any(|m| matches!(m, Message::Hint { kind, .. } if *kind == crate::host_question::hint::CARD)),
                "and the saving card is announced"
            );
        }

        /// **Case 27 turns the computed damage into `Damage` units**, and
        /// `EFFECT_BATTLE_DAMAGE_TO_EFFECT` changes the *reason* rather
        /// than the amount.
        #[test]
        fn the_damage_is_dealt_with_the_right_reason() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 1800);
            let x = monster(&mut f, 1, 0, 1200);
            f.core.attacker = Some(a);
            f.core.attack_target = Some(x);
            f.core.battle_damage = [0, 600];
            f.emplace_at(
                Kind::BattleCommand {
                    state: Box::new(BattleCommandState {
                        reason_card: Some(a),
                        reason_player: 0,
                        ..Default::default()
                    }),
                },
                27,
            );
            step(&mut f);
            let dealt = f
                .core
                .units
                .iter()
                .chain(f.core.subunits.iter())
                .find_map(|u| match &u.kind {
                    Kind::Damage { arg } => Some((arg.playerid, arg.amount, arg.reason)),
                    _ => None,
                });
            assert_eq!(dealt, Some((1, 600, reason::BATTLE)));
        }

        /// **Case 28 destroys only a card that is still the same
        /// instance** in the Monster Zone.
        #[test]
        fn only_the_same_instance_is_destroyed() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 1800);
            let x = monster(&mut f, 1, 0, 1200);
            f.core.attacker = Some(a);
            f.core.attack_target = Some(x);
            f.core.pre_field[0] = f.cards[a].fieldid_r;
            f.core.pre_field[1] = f.cards[x].fieldid_r + 1; // a different one
            f.cards[x].set_status(status::BATTLE_RESULT, true);
            start_at(&mut f, 28);
            step(&mut f);
            assert!(
                !queued(&f, |k| matches!(k, Kind::Destroy { .. })),
                "the target is not the card that fought"
            );
        }

        /// And when it *is* the same instance, it goes — with its reason
        /// saved and replaced.
        #[test]
        fn the_loser_is_destroyed_with_its_reason_recorded() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 1800);
            let x = monster(&mut f, 1, 0, 1200);
            f.core.attacker = Some(a);
            f.core.attack_target = Some(x);
            f.core.pre_field[0] = f.cards[a].fieldid_r;
            f.core.pre_field[1] = f.cards[x].fieldid_r;
            f.cards[x].reason = reason::EFFECT;
            f.cards[x].set_status(status::BATTLE_RESULT, true);
            start_at(&mut f, 28);
            step(&mut f);
            assert!(queued(&f, |k| matches!(k, Kind::Destroy { .. })));
            assert_eq!(f.cards[x].reason, reason::BATTLE, "the new reason");
            assert_eq!(f.cards[x].temp.reason, reason::EFFECT, "the old one, saved");
            assert_eq!(f.cards[x].reason_card, Some(a));
            assert!(f.cards[x].is_status(status::DESTROY_CONFIRMED));
        }

        /// **Case 33 filters the group again** — a card that left, or came
        /// back as a different instance, is dropped before it is sent.
        #[test]
        fn a_card_that_left_is_dropped_before_being_sent() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 1800);
            let x = monster(&mut f, 1, 0, 1200);
            f.core.pre_field[0] = f.cards[a].fieldid_r;
            f.core.pre_field[1] = f.cards[x].fieldid_r;
            let g = f.new_group(vec![a, x]);
            // `x` leaves before the group is sent.
            f.remove_card(x);
            f.add_card(1, x, location::GRAVE, 0, false);
            f.emplace_at(
                Kind::BattleCommand {
                    state: Box::new(BattleCommandState {
                        cards_destroyed_by_battle: Some(g),
                        ..Default::default()
                    }),
                },
                33,
            );
            step(&mut f);
            let kept: Vec<CardId> = f.groups[g].iter().copied().collect();
            assert_eq!(kept, vec![a], "only the one still there");
        }
    }

    mod leaving_the_phase {
        use super::*;

        /// **A second Battle Phase is optional only if every
        /// `EFFECT_BP_TWICE` says so** — one effect with a value other
        /// than 1 makes it compulsory for all.
        #[test]
        fn one_compulsory_effect_makes_the_second_phase_compulsory() {
            let mut f = field();
            player_effect(&mut f, code::BP_TWICE, 0, 1);
            start_at(&mut f, 41);
            step(&mut f);
            let optional = f.core.units.iter().find_map(|u| match &u.kind {
                Kind::BattleCommand { state } => Some(state.second_battle_phase_is_optional),
                _ => None,
            });
            assert_eq!(optional, Some(true), "a value of 1 is optional");

            let mut f = field();
            player_effect(&mut f, code::BP_TWICE, 0, 1);
            player_effect(&mut f, code::BP_TWICE, 0, 2);
            start_at(&mut f, 41);
            step(&mut f);
            let optional = f.core.units.iter().find_map(|u| match &u.kind {
                Kind::BattleCommand { state } => Some(state.second_battle_phase_is_optional),
                _ => None,
            });
            assert_eq!(optional, Some(false), "one compulsory one settles it");
        }

        /// **No `EFFECT_BP_TWICE` means no repeat at all.**
        #[test]
        fn without_the_effect_there_is_no_repeat() {
            let mut f = field();
            start_at(&mut f, 41);
            step(&mut f);
            let repeat = f.core.units.iter().find_map(|u| match &u.kind {
                Kind::BattleCommand { state } => Some(state.repeat_battle_phase),
                _ => None,
            });
            assert_eq!(repeat, Some(false));
        }

        /// **The attack is cleared on the way out**, so nothing downstream
        /// reads a stale attacker.
        #[test]
        fn the_attack_is_cleared_on_the_way_out() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 1800);
            let x = monster(&mut f, 1, 0, 1200);
            f.core.attacker = Some(a);
            f.core.attack_target = Some(x);
            f.emplace_at(
                Kind::BattleCommand {
                    state: Box::new(BattleCommandState {
                        phase_to_change_to: 2,
                        ..Default::default()
                    }),
                },
                42,
            );
            run(&mut f);
            assert!(f.core.attacker.is_none());
            assert!(f.core.attack_target.is_none());
        }

        /// **No `EFFECT_BP_TWICE` means no second phase**, and the unit
        /// hands the destination back.
        #[test]
        fn without_the_effect_it_hands_back_and_finishes() {
            let mut f = field();
            f.emplace_at(
                Kind::BattleCommand {
                    state: Box::new(BattleCommandState {
                        phase_to_change_to: 2,
                        ..Default::default()
                    }),
                },
                42,
            );
            run(&mut f);
            assert_eq!(f.core.returns.get(), 2, "the destination");
            assert_eq!(f.core.returns.at_i32(1), 0, "and no repeat");
            assert!(f.core.attacker.is_none());
        }

        /// **An optional second phase is asked about**, and the answer
        /// travels in slot 1 alongside the destination in slot 0.
        #[test]
        fn an_optional_second_phase_is_asked_and_answered() {
            let mut f = field();
            f.emplace_at(
                Kind::BattleCommand {
                    state: Box::new(BattleCommandState {
                        phase_to_change_to: 2,
                        repeat_battle_phase: true,
                        second_battle_phase_is_optional: true,
                        ..Default::default()
                    }),
                },
                42,
            );
            step(&mut f);
            assert!(queued(&f, |k| matches!(k, Kind::SelectYesNo { .. })));

            let mut f = field();
            f.core.returns.set(1);
            f.emplace_at(
                Kind::BattleCommand {
                    state: Box::new(BattleCommandState {
                        phase_to_change_to: 2,
                        ..Default::default()
                    }),
                },
                43,
            );
            run(&mut f);
            assert_eq!(f.core.returns.get(), 2, "slot 0 is the destination");
            assert_eq!(f.core.returns.at_i32(1), 1, "slot 1 is the repeat");
        }

        /// **A compulsory second phase is not asked about.**
        #[test]
        fn a_compulsory_second_phase_is_not_asked() {
            let mut f = field();
            f.emplace_at(
                Kind::BattleCommand {
                    state: Box::new(BattleCommandState {
                        phase_to_change_to: 2,
                        repeat_battle_phase: true,
                        second_battle_phase_is_optional: false,
                        ..Default::default()
                    }),
                },
                42,
            );
            run(&mut f);
            assert_eq!(f.core.returns.at_i32(1), 1, "repeating, without a question");
            assert!(
                !queued(&f, |k| matches!(k, Kind::SelectYesNo { .. })),
                "and nothing was asked"
            );
        }
    }
}

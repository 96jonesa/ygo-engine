//! Starting a duel and running its turns: `Startup`, `Turn`, `RefreshRelay`.
//!
//! This is the outermost loop. `Startup` runs once; `Turn` runs forever,
//! handing off to itself with the players swapped, and every other unit in
//! the engine is reached from inside it.
//!
//! ```text
//!   Startup ──→ Turn(player 0)
//!                 │
//!                 ├─ 0   clear the turn's state
//!                 ├─ 1   new turn, or skip it entirely
//!                 ├─ 2-3 Draw Phase
//!                 ├─ 5-6 Standby Phase
//!                 ├─ 7-9 Main Phase 1 ──→ IdleCommand
//!                 ├─10-12 Battle Phase ──→ BattleCommand (not ported)
//!                 ├─13-15 Main Phase 2 ──→ IdleCommand
//!                 ├─16-19 End Phase
//!                 └─20   restart, with the other player
//! ```
//!
//! ## Case 0 is the only place a turn's state is cleared
//!
//! Twenty-odd fields across two players, the whole Monster and Spell/Trap
//! rows, and six action counters. It is a long block of assignments and it
//! is the sort of thing a port abbreviates — but a field that is not
//! cleared here is never cleared at all, and the symptom is an effect that
//! silently stops working on turn two.
//!
//! **The battle system's tallies are cleared here even though
//! `BattleCommand` is not ported.** Nothing writes them yet. They are
//! cleared anyway, because the reset and the writer are different pieces of
//! work and omitting the reset "until it matters" is how it gets forgotten.
//!
//! ## Two events per phase, one word apart
//!
//! `EVENT_PHASE_START + p` is raised here, when a phase begins.
//! `EVENT_PHASE + p` is what `PhaseEvent` gathers on. They are different
//! codes (`0x2000` and `0x1000`) and different moments.
//!
//! ## Every jump is written one lower than the case it reaches
//!
//! `arg.step = n` means "case n+1 next", because the processor increments
//! after the handler returns false. Reading `arg.step = 3` as "go to case
//! 3" gets the destination wrong by one, and the wrong destination is
//! always a case that exists — so nothing crashes and the turn just runs a
//! phase it should not have.
//!
//! Written out, because this is the table a reader actually needs:
//!
//! | where | `arg.step =` | case reached | what is skipped |
//! |---|---|---|---|
//! | 1, `SKIP_TURN` | 18 | **19** | the entire turn; only `EVENT_TURN_END` runs |
//! | 2, `SKIP_DP` | 3 | **4** → 5 | the draw *and* the draw phase event |
//! | 5, `SKIP_SP` | 6 | **7** | the standby phase event |
//! | 10, answer 7 | 15 | **16** | the Battle Phase and Main Phase 2 |
//! | 10, `SKIP_BP` | 15 | **16** | the same |
//! | 13, 2nd battle | 9 | **10** | nothing — the Battle Phase is re-entered |
//! | 13, no Main 2 | 15 | **16** | Main Phase 2's menu |
//! | 16, `SKIP_EP` | 18 | **19** | the End Phase's `PhaseEvent` |
//!
//! Two consequences that are easy to get backwards:
//!
//! - **A skipped Draw Phase does not draw.** The jump clears case 3, which
//!   is the draw. Case 4 is the empty one left behind when
//!   `EVENT_PHASE_PRESTART` was removed.
//! - **A skipped End Phase skips the hand limit**, because the hand limit
//!   lives in case 18's `PhaseEvent` and the jump lands past it. So does
//!   `SKIP_TURN`, which takes the same exit.
//!
//! ## The second Battle Phase re-enters the battle
//!
//! Case 13 reads a second return slot from `BattleCommand`. If it is set
//! and no second battle has happened yet, the unit jumps to `arg.step = 9`
//! — which is **case 10**, the Battle Phase's start, not case 9's Main
//! Phase 1 menu. Every monster's attack tallies are cleared on the way, and
//! `has_performed_second_battle_phase` stops a third.

use crate::card::status;
use crate::duel::phases;
use crate::event::{code, CardId};
use crate::field::{Field, Message};
use crate::processor::{Kind, RESTART};

impl Field {
    /// `field::process(Processors::Startup&)` — two steps, run once.
    pub(crate) fn startup_step(&mut self, step: u16) -> bool {
        if step == 0 {
            self.core.shuffle_hand_check = [false, false];
            self.core.shuffle_deck_check = [false, false];
            self.raise_event(None, code::STARTUP, None, 0, 0, 0, 0);
            self.process_instant_event();
            return false;
        }
        // The checks are cleared **again**, because raising `EVENT_STARTUP`
        // may have disturbed a pile.
        for p in 0..2usize {
            self.core.shuffle_hand_check[p] = false;
            self.core.shuffle_deck_check[p] = false;
            let start = self.players[p].start_count;
            if start > 0 {
                self.draw(p as u8, start as u32, crate::card::reason::RULE, None, 2);
            }
            self.startup_extra_hands(p as u8);
        }
        self.emplace(Kind::Turn {
            turn_player: 0,
            state: Default::default(),
        });
        true
    }

    /// The opening hands of a tag duel's *other* decks, dealt without going
    /// through `draw`.
    ///
    /// `extra_lists_main` is empty outside a tag duel, so this does nothing
    /// in this port's configuration. Named rather than omitted: the loop is
    /// in the reference between two things that do run, and a reader who
    /// finds it missing cannot tell whether it was considered.
    fn startup_extra_hands(&mut self, _player: u8) {}

    /// `field::process(Processors::RefreshRelay&)` — hand the deck on in a
    /// relay duel.
    ///
    /// Steps 0 and 1 are **the two players**, not two stages: the case
    /// label is used as the player index. `DUEL_RELAY` is off in this
    /// port's configuration, so `recharge` is never set and this is two
    /// no-op steps.
    pub(crate) fn refresh_relay_step(&mut self, step: u16) -> bool {
        match step {
            0 | 1 => {
                if self.players[step as usize].recharge {
                    self.next_player(step as u8);
                }
                false
            }
            _ => true,
        }
    }

    /// `field::get_draw_count` — how many this player draws this turn.
    ///
    /// **A zero from any effect wins outright**; otherwise the largest
    /// wins. So an effect that says "draw nothing" cannot be outbid, and
    /// one that says "draw three" does not stack with another saying
    /// "draw two".
    pub fn get_draw_count(&mut self, playerid: u8) -> i32 {
        let mut count = self.players[playerid as usize].draw_count;
        for e in self.filter_player_effect(playerid, code::DRAW_COUNT) {
            let c = self.effect_plain_value(e) as i32;
            if c == 0 {
                return 0;
            }
            if c > count {
                count = c;
            }
        }
        count
    }

    /// `field::process(Processors::Turn&)`.
    pub(crate) fn turn_step(
        &mut self,
        step: u16,
        turn_player: &mut u8,
        state: &mut TurnState,
    ) -> bool {
        let tp = *turn_player;
        match step {
            0 => self.turn_step_0(),
            1 => self.turn_step_1(tp),
            2 => self.turn_step_2(tp),
            3 => self.turn_step_3(tp),
            // Case 4 held `EVENT_PHASE_PRESTART`, which the reference
            // removed. The empty case is kept so the numbering does not
            // shift under the jumps that target it.
            4 | 8 => false,
            5 => self.turn_step_5(tp),
            6 => self.turn_step_6(),
            7 => self.turn_step_7(tp),
            9 => self.turn_step_9(),
            10 => self.turn_step_10(tp),
            11 => self.turn_step_11(),
            12 => self.turn_step_12(),
            13 => self.turn_step_13(tp, state),
            14 | 17 => self.point_event_if_chains(),
            15 => self.turn_step_15(),
            16 => self.turn_step_16(tp),
            18 => self.turn_step_18(),
            19 => self.turn_step_19(tp),
            20 => {
                self.clear_pending_chains();
                self.set_step(RESTART);
                *turn_player = 1 - *turn_player;
                false
            }
            _ => true,
        }
    }

    /// The four pending-chain lists, cleared at the top of nearly every
    /// phase. They accumulate during a phase and must not leak into the
    /// next one.
    pub(crate) fn clear_pending_chains(&mut self) {
        self.core.new_fchain.clear();
        self.core.new_ochain.clear();
        self.core.quick_f_chain.clear();
        self.core.delayed_quick_tmp.clear();
    }

    /// `if(core.new_fchain.size() || core.new_ochain.size())` — open a
    /// window only if the phase's start actually triggered something.
    fn point_event_if_chains(&mut self) -> bool {
        if !self.core.new_fchain.is_empty() || !self.core.new_ochain.is_empty() {
            self.emplace(Kind::PointEvent {
                skip: crate::point_event::PointEventSkip::default(),
            });
        }
        false
    }

    /// Case 0: clear everything a turn owns.
    fn turn_step_0(&mut self) -> bool {
        self.core.used_event.clear();
        // The reference deletes the effects here; this arena drops them
        // when the list does.
        self.core.reseted_effects.clear();
        self.core.effect_count_code.clear();

        for p in 0..2usize {
            let mzone: Vec<CardId> = self.players[p].mzone.iter().flatten().copied().collect();
            for c in mzone {
                let card = &mut self.cards[c];
                card.set_status(status::SUMMON_TURN, false);
                card.set_status(status::FLIP_SUMMON_TURN, false);
                card.set_status(status::SPSUMMON_TURN, false);
                card.set_status(status::SET_TURN, false);
                card.set_status(status::FORM_CHANGED, false);
                card.indestructable_effects.clear();
                card.attack_announce_count = 0;
                card.announce_count = 0;
                card.attacked_count = 0;
                card.announced_cards.clear();
                card.attacked_cards.clear();
                card.battled_cards.clear();
                card.attack_all_target = true;
            }
            let szone: Vec<CardId> = self.players[p].szone.iter().flatten().copied().collect();
            for c in szone {
                self.cards[c].set_status(status::SET_TURN, false);
                self.cards[c].indestructable_effects.clear();
            }

            self.core.summon_state_count[p] = 0;
            self.core.normalsummon_state_count[p] = 0;
            self.core.flipsummon_state_count[p] = 0;
            self.core.spsummon_state_count[p] = 0;
            self.core.spsummon_state_count_rst[p] = 0;
            self.core.attack_state_count[p] = 0;
            self.core.battle_phase_count[p] = 0;
            self.core.battled_count[p] = 0;
            self.core.summon_count[p] = 0;
            self.core.extra_summon[p] = false;
            self.core.spsummon_once_map[p].clear();
            self.core.spsummon_once_map_rst[p].clear();
        }
        self.emplace(Kind::RefreshRelay);
        false
    }

    /// Case 1: announce the turn, recharge what recharges, and decide
    /// whether the turn happens at all.
    ///
    /// `EFFECT_SKIP_TURN` resets **three** phases before jumping — draw,
    /// standby and end — because the jump lands on case 19, past all three,
    /// and each has to be told it did not happen. Note what that skips:
    /// case 18's `PhaseEvent`, and so the End Phase hand limit.
    fn turn_step_1(&mut self, turn_player: u8) -> bool {
        self.core.force_turn_end = false;
        self.core.spsummon_rst = false;
        for e in self.field_effects.rechargeable.clone() {
            let no_reset = self
                .effects
                .get(e)
                .is_some_and(|x| x.is_flag(crate::effect::flag::NO_TURN_RESET));
            if !no_reset {
                if let Some(x) = self.effects.get_mut(e) {
                    // `effect::recharge`, which only touches a counted one.
                    if x.is_flag(crate::effect::flag::COUNT_LIMIT) {
                        x.count_limit = x.count_limit_max;
                    }
                }
            }
        }
        for counter in [
            &mut self.core.summon_counter,
            &mut self.core.normalsummon_counter,
            &mut self.core.spsummon_counter,
            &mut self.core.flipsummon_counter,
            &mut self.core.attack_counter,
            &mut self.core.chain_counter,
        ] {
            for entry in counter.values_mut() {
                entry.player_amount = [0, 0];
            }
        }
        // The per-card special-summon tallies belong to the effects that
        // impose them, so they are reached through the effect list rather
        // than by walking the field.
        for e in self.field_effects.spsummon_count_eff.clone() {
            let no_reset = self
                .effects
                .get(e)
                .is_some_and(|x| x.is_flag(crate::effect::flag::NO_TURN_RESET));
            if no_reset {
                continue;
            }
            if let Some(h) = self.effects.get(e).and_then(|x| x.get_handler(&self.cards)) {
                self.cards[h].spsummon_counter = [0, 0];
                self.cards[h].spsummon_counter_rst = [0, 0];
            }
        }

        self.infos.turn_id += 1;
        self.infos.turn_id_by_player[turn_player as usize] += 1;
        self.infos.turn_player = turn_player;
        self.messages.push(Message::NewTurn {
            player: turn_player,
        });
        if !self.is_flag(crate::duel::flags::RELAY) && self.infos.turn_id != 1 {
            self.tag_swap(turn_player);
        }

        if self
            .is_player_affected_by_effect(turn_player, code::SKIP_TURN)
            .is_some()
        {
            self.set_step(18);
            self.reset_phase(phases::DRAW);
            self.reset_phase(phases::STANDBY);
            self.reset_phase(phases::END);
            self.adjust_all();
            return false;
        }

        self.infos.phase = phases::DRAW;
        self.core.phase_action = false;
        self.core.hand_adjusted = false;
        self.raise_event(
            None,
            code::PHASE_START + u32::from(phases::DRAW),
            None,
            0,
            0,
            turn_player,
            0,
        );
        self.process_instant_event();
        self.adjust_all();
        false
    }

    /// Case 2: the Draw Phase's announcement.
    ///
    /// **`EFFECT_SKIP_DP` skips the draw too.** `arg.step = 3` lands on
    /// case **4**, not case 3 — so case 3, which is the draw, does not run.
    /// Reading the assignment as its own case number is the single easiest
    /// mistake to make in this unit, and it produces a turn that draws when
    /// it should not.
    fn turn_step_2(&mut self, turn_player: u8) -> bool {
        self.clear_pending_chains();
        if self
            .is_player_affected_by_effect(turn_player, code::SKIP_DP)
            .is_some()
            || self.core.force_turn_end
        {
            self.set_step(3);
            self.reset_phase(phases::DRAW);
            self.adjust_all();
            return false;
        }
        let phase = self.infos.phase;
        self.messages.push(Message::NewPhase { phase });
        self.raise_event(None, code::PREDRAW, None, 0, 0, turn_player, 0);
        self.process_instant_event();
        self.messages.push(Message::Hint {
            kind: crate::host_question::hint::EVENT,
            player: turn_player,
            value: 27,
        });
        if !self.core.new_fchain.is_empty() || !self.core.new_ochain.is_empty() {
            self.emplace(Kind::PointEvent {
                skip: crate::point_event::PointEventSkip {
                    trigger: true,
                    ..Default::default()
                },
            });
        }
        false
    }

    /// Case 3: the draw itself.
    ///
    /// No draw on the opening turn unless `DUEL_1ST_TURN_DRAW` is set — it
    /// is not, in this port's configuration — so the first player begins
    /// with their opening hand and nothing more.
    fn turn_step_3(&mut self, turn_player: u8) -> bool {
        if self.is_flag(crate::duel::flags::FIRST_TURN_DRAW) || self.infos.turn_id > 1 {
            let mut count = self.get_draw_count(turn_player);
            if count > 0 && self.is_flag(crate::duel::flags::DRAW_UNTIL_5) {
                let hand = self.players[turn_player as usize].hand.len() as i32;
                count = count.max(5 - hand);
            }
            if count > 0 {
                self.draw(
                    turn_player,
                    count as u32,
                    crate::card::reason::RULE,
                    None,
                    turn_player,
                );
                self.emplace(Kind::PointEvent {
                    skip: crate::point_event::PointEventSkip::default(),
                });
            }
        }
        self.emplace(Kind::PhaseEvent {
            phase: phases::DRAW,
            state: Default::default(),
        });
        false
    }

    /// Case 5: the Standby Phase's announcement.
    fn turn_step_5(&mut self, turn_player: u8) -> bool {
        self.infos.phase = phases::STANDBY;
        self.core.phase_action = false;
        self.clear_pending_chains();
        if self.is_flag(crate::duel::flags::NO_STANDBY_PHASE)
            || self
                .is_player_affected_by_effect(turn_player, code::SKIP_SP)
                .is_some()
            || self.core.force_turn_end
        {
            self.set_step(6);
            self.reset_phase(phases::STANDBY);
            self.adjust_all();
            return false;
        }
        let phase = self.infos.phase;
        self.messages.push(Message::NewPhase { phase });
        self.raise_event(
            None,
            code::PHASE_START + u32::from(phases::STANDBY),
            None,
            0,
            0,
            turn_player,
            0,
        );
        self.process_instant_event();
        false
    }

    /// Case 6: the Standby Phase's window, with a condition no other phase
    /// has.
    ///
    /// The reference's comment names a card — `c89642993`, Ultimate Offering
    /// — and the extra test is `instant_event.back().event_code != EVENT_
    /// PHASE_START + PHASE_STANDBY`. It opens a window when the *last*
    /// instant event is something other than the standby start, even with
    /// no chains pending, so an effect that raised its own event during the
    /// standby announcement still gets a response window.
    fn turn_step_6(&mut self) -> bool {
        let standby_start = code::PHASE_START + u32::from(phases::STANDBY);
        let last_is_standby = self
            .core
            .instant_event
            .back()
            .is_some_and(|e| e.event_code == standby_start);
        if !self.core.new_fchain.is_empty() || !self.core.new_ochain.is_empty() || !last_is_standby
        {
            self.emplace(Kind::PointEvent {
                skip: crate::point_event::PointEventSkip::default(),
            });
        }
        self.emplace(Kind::PhaseEvent {
            phase: phases::STANDBY,
            state: Default::default(),
        });
        false
    }

    /// Case 7: Main Phase 1 begins. **No skip check** — Main Phase 1 always
    /// happens; `EFFECT_SKIP_M1` is read by `IdleCommand`, not here.
    fn turn_step_7(&mut self, turn_player: u8) -> bool {
        self.infos.phase = phases::MAIN1;
        self.core.phase_action = false;
        self.raise_event(
            None,
            code::PHASE_START + u32::from(phases::MAIN1),
            None,
            0,
            0,
            turn_player,
            0,
        );
        self.process_instant_event();
        self.adjust_all();
        false
    }

    /// Case 9: offer the Main Phase 1 menu.
    fn turn_step_9(&mut self) -> bool {
        self.clear_pending_chains();
        let phase = self.infos.phase;
        self.messages.push(Message::NewPhase { phase });
        self.emplace(Kind::IdleCommand {
            state: Default::default(),
        });
        false
    }

    /// Case 10: the menu's answer, and the Battle Phase's start.
    ///
    /// A `7` means the End Phase: `arg.step = 15` reaches case **16**,
    /// the End Phase — so Main Phase 2's menu (case 15) is skipped as well
    /// as the battle. `EFFECT_SKIP_BP` takes the same exit, so a Battle
    /// Phase that is entered and skipped costs the player their Main Phase
    /// 2 too.
    ///
    /// `battle_phase_count` is bumped **before** the skip check, so a
    /// Battle Phase that is entered and immediately skipped still counts as
    /// having been entered.
    fn turn_step_10(&mut self, turn_player: u8) -> bool {
        if self.core.returns.get() == 7 {
            self.set_step(15);
            return false;
        }
        self.infos.phase = phases::BATTLE_START;
        self.clear_pending_chains();
        self.core.phase_action = false;
        self.core.battle_phase_count[self.infos.turn_player as usize] += 1;
        let phase = self.infos.phase;
        self.messages.push(Message::NewPhase { phase });
        if self
            .is_player_affected_by_effect(turn_player, code::SKIP_BP)
            .is_some()
            || self.core.force_turn_end
        {
            self.set_step(15);
            self.reset_phase(phases::BATTLE_START);
            self.reset_phase(phases::BATTLE_STEP);
            self.reset_phase(phases::BATTLE);
            self.adjust_all();
            return false;
        }
        self.raise_event(
            None,
            code::PHASE_START + u32::from(phases::BATTLE_START),
            None,
            0,
            0,
            turn_player,
            0,
        );
        self.process_instant_event();
        self.adjust_all();
        false
    }

    /// Case 11: the Battle Phase's start window.
    fn turn_step_11(&mut self) -> bool {
        self.point_event_if_chains();
        self.emplace(Kind::PhaseEvent {
            phase: phases::BATTLE_START,
            state: Default::default(),
        });
        false
    }

    /// Case 12: the Battle Step. **`BattleCommand` is not ported** — this
    /// reaches a named panic rather than doing nothing.
    fn turn_step_12(&mut self) -> bool {
        self.infos.phase = phases::BATTLE_STEP;
        self.clear_pending_chains();
        self.core.phase_action = false;
        self.core.chain_attack = false;
        self.emplace(Kind::BattleCommand {
            state: Box::default(),
        });
        false
    }

    /// Case 13: the Battle Phase's answer — a second Battle Phase, or
    /// Main Phase 2, or neither.
    ///
    /// **Slot 1 of `returns` is the second-battle flag and slot 0 is the
    /// command.** Two different answers in one array, read by two different
    /// tests in the same case.
    ///
    /// A second Battle Phase sets `arg.step = 9`, which reaches **case
    /// 10** — the Battle Phase's start, not case 9's Main Phase 1 menu. So
    /// the player gets another battle, not another Main Phase. Every
    /// monster's attack tallies are cleared on the way, and
    /// `has_performed_second_battle_phase` stops a third.
    fn turn_step_13(&mut self, turn_player: u8, state: &mut TurnState) -> bool {
        if !state.has_performed_second_battle_phase && self.core.returns.at_i32(1) != 0 {
            state.has_performed_second_battle_phase = true;
            self.set_step(9);
            for p in 0..2usize {
                let mzone: Vec<CardId> = self.players[p].mzone.iter().flatten().copied().collect();
                for c in mzone {
                    let card = &mut self.cards[c];
                    card.attack_announce_count = 0;
                    card.announce_count = 0;
                    card.attacked_count = 0;
                    card.announced_cards.clear();
                    card.attacked_cards.clear();
                    card.battled_cards.clear();
                }
            }
            return false;
        }
        state.has_performed_second_battle_phase = false;
        if self.is_flag(crate::duel::flags::NO_MAIN_PHASE_2) {
            self.set_step(15);
            self.adjust_all();
            return false;
        }
        // `core.skip_m2` is the one-shot `IdleCommand` consumes: a player
        // who ended the Battle Phase with "go to the End Phase" gets Main
        // Phase 2 entered and immediately left, rather than not entered.
        self.core.skip_m2 = self.core.returns.get() == 3 || self.core.force_turn_end;

        self.infos.phase = phases::MAIN2;
        self.core.phase_action = false;
        self.raise_event(
            None,
            code::PHASE_START + u32::from(phases::MAIN2),
            None,
            0,
            0,
            turn_player,
            0,
        );
        self.process_instant_event();
        self.adjust_all();
        false
    }

    /// Case 15: offer the Main Phase 2 menu.
    ///
    /// **`infos.can_shuffle` is restored here**, not in `IdleCommand` —
    /// so the hand may be shuffled once in each Main Phase, and the two
    /// permissions are separate.
    fn turn_step_15(&mut self) -> bool {
        self.clear_pending_chains();
        let phase = self.infos.phase;
        self.messages.push(Message::NewPhase { phase });
        self.infos.can_shuffle = true;
        self.emplace(Kind::IdleCommand {
            state: Default::default(),
        });
        false
    }

    /// Case 16: the End Phase.
    fn turn_step_16(&mut self, turn_player: u8) -> bool {
        self.infos.phase = phases::END;
        self.core.phase_action = false;
        if self
            .is_player_affected_by_effect(turn_player, code::SKIP_EP)
            .is_some()
        {
            self.set_step(18);
            self.reset_phase(phases::END);
            self.adjust_all();
            return false;
        }
        let phase = self.infos.phase;
        self.messages.push(Message::NewPhase { phase });
        self.raise_event(
            None,
            code::PHASE_START + u32::from(phases::END),
            None,
            0,
            0,
            turn_player,
            0,
        );
        self.process_instant_event();
        self.adjust_all();
        false
    }

    /// Case 18: the End Phase's window — where the hand limit is enforced.
    /// Both `SKIP_EP` and `SKIP_TURN` jump *past* this case, so neither
    /// enforces the hand limit.
    fn turn_step_18(&mut self) -> bool {
        self.clear_pending_chains();
        self.emplace(Kind::PhaseEvent {
            phase: phases::END,
            state: Default::default(),
        });
        false
    }

    /// Case 19: the turn is over.
    fn turn_step_19(&mut self, turn_player: u8) -> bool {
        self.raise_event(None, code::TURN_END, None, 0, 0, turn_player, 0);
        self.process_instant_event();
        self.adjust_all();
        false
    }
}

/// `Turn`'s own state: whether the second Battle Phase has already been
/// taken this turn.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TurnState {
    pub has_performed_second_battle_phase: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::location;
    use crate::card::{card_type, Card, CardData};
    use crate::effect::{effect_type, flag, Effect};
    use crate::event::EffectId;
    use crate::processor::Status;

    fn field() -> Field {
        Field::new(8000)
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
            f.cards[id].current.position = crate::board::position::FACEUP_ATTACK;
        }
        id
    }

    fn deck_of(f: &mut Field, player: u8, n: u32) {
        for i in 0..n {
            let mut c = Card::with_data(
                CardData {
                    code: 1000 + i,
                    type_: card_type::MONSTER,
                    level: 4,
                    ..Default::default()
                },
                player,
            );
            c.current.controller = player;
            let id = f.new_card(c);
            f.add_card(player, id, location::DECK, i, false);
        }
    }

    /// A player-targeted field effect.
    fn player_effect(f: &mut Field, code_: u32, player: u8) -> EffectId {
        let anchor = card_at(f, player, location::MZONE, 4, card_type::MONSTER);
        let mut e = Effect::new(effect_type::FIELD, code_);
        e.owner = Some(anchor);
        e.handler = Some(anchor);
        e.flag[0] = flag::PLAYER_TARGET | flag::ABSOLUTE_TARGET;
        e.range = u16::from(location::MZONE);
        e.s_range = 1;
        let id = f.new_effect(e);
        f.add_effect(id, player);
        id
    }

    fn turn_at(f: &mut Field, step: u16, turn_player: u8) {
        f.emplace_at(
            Kind::Turn {
                turn_player,
                state: Default::default(),
            },
            step,
        );
    }

    /// Run one step of whatever is at the front.
    fn step(f: &mut Field) -> Status {
        f.process()
    }

    /// Run until the machine stops, answering nothing. Panics if it asks.
    fn run(f: &mut Field) -> Status {
        for _ in 0..2048 {
            match f.process() {
                Status::Continue => continue,
                other => return other,
            }
        }
        panic!("did not settle");
    }

    /// Whether the predicate was **ever** true while running to the stop.
    ///
    /// The instrument for "was this unit emplaced at all". Checking after
    /// the run is no good: a subunit is emplaced, runs, and is gone before
    /// the next turn case, so a `queued` test at the end sees nothing
    /// whether or not it happened.
    fn ever(f: &mut Field, pred: impl Fn(&Field) -> bool) -> bool {
        for _ in 0..512 {
            if pred(f) {
                return true;
            }
            if f.process() != Status::Continue {
                return pred(f);
            }
        }
        panic!("did not settle");
    }

    /// Step until the predicate holds, or the machine stops. The skip
    /// branches call `adjust_all`, which emplaces `Adjust` ahead of the
    /// turn — so a test that wants the *next* turn case has to run past it.
    fn until(f: &mut Field, pred: impl Fn(&Field) -> bool) -> bool {
        for _ in 0..512 {
            if pred(f) {
                return true;
            }
            if f.process() != Status::Continue {
                return pred(f);
            }
        }
        panic!("did not settle");
    }

    /// Whether an event with this code is anywhere in the pipeline — the
    /// pending queue, the batch being processed, or the spent list.
    fn raised(f: &Field, code_: u32) -> bool {
        f.core
            .queue_event
            .iter()
            .chain(f.core.instant_event.iter())
            .chain(f.core.used_event.iter())
            .any(|e| e.event_code == code_)
    }

    fn queued(f: &Field, pred: impl Fn(&Kind) -> bool) -> bool {
        f.core
            .units
            .iter()
            .chain(f.core.subunits.iter())
            .any(|u| pred(&u.kind))
    }

    mod startup {
        use super::*;

        /// **Startup deals the opening hands and hands off to `Turn`.**
        #[test]
        fn it_deals_and_starts_the_first_turn() {
            let mut f = field();
            deck_of(&mut f, 0, 40);
            deck_of(&mut f, 1, 40);
            f.push_back(Kind::Startup);
            // Two steps, then the draws and the Turn unit are queued.
            step(&mut f);
            step(&mut f);
            assert!(
                queued(&f, |k| matches!(k, Kind::Turn { turn_player: 0, .. })),
                "the first turn is player 0's"
            );
            assert!(queued(&f, |k| matches!(k, Kind::Draw { .. })), "and a draw");
        }

        /// **`EVENT_STARTUP` is raised, and before anything is dealt** —
        /// case 0 is entirely that event, and the draws are case 1.
        #[test]
        fn the_startup_event_precedes_the_deal() {
            let mut f = field();
            deck_of(&mut f, 0, 40);
            f.push_back(Kind::Startup);
            step(&mut f);
            assert!(raised(&f, code::STARTUP), "EVENT_STARTUP was raised");
            assert!(
                !queued(&f, |k| matches!(k, Kind::Draw { .. })),
                "nothing dealt yet"
            );
            assert_eq!(f.players[0].hand.len(), 0);
        }

        /// A player with no starting count draws nothing.
        #[test]
        fn a_zero_start_count_deals_nothing() {
            let mut f = field();
            deck_of(&mut f, 0, 40);
            deck_of(&mut f, 1, 40);
            f.players[0].start_count = 0;
            f.players[1].start_count = 0;
            f.push_back(Kind::Startup);
            step(&mut f);
            step(&mut f);
            assert!(!queued(&f, |k| matches!(k, Kind::Draw { .. })));
        }
    }

    mod the_turn_reset {
        use super::*;

        /// **Case 0 clears every per-turn status on the field.** A card
        /// summoned last turn is an ordinary card this turn.
        #[test]
        fn the_per_turn_statuses_are_cleared() {
            let mut f = field();
            let m = card_at(&mut f, 0, location::MZONE, 0, card_type::MONSTER);
            let s = card_at(&mut f, 0, location::SZONE, 0, card_type::SPELL);
            for st in [
                status::SUMMON_TURN,
                status::FLIP_SUMMON_TURN,
                status::SPSUMMON_TURN,
                status::SET_TURN,
                status::FORM_CHANGED,
            ] {
                f.cards[m].set_status(st, true);
            }
            f.cards[s].set_status(status::SET_TURN, true);

            turn_at(&mut f, 0, 0);
            step(&mut f);
            for st in [
                status::SUMMON_TURN,
                status::FLIP_SUMMON_TURN,
                status::SPSUMMON_TURN,
                status::SET_TURN,
                status::FORM_CHANGED,
            ] {
                assert!(!f.cards[m].is_status(st), "monster status {st:#x}");
            }
            assert!(
                !f.cards[s].is_status(status::SET_TURN),
                "and the spell row's set-turn too"
            );
        }

        /// **The battle tallies are cleared even though nothing writes
        /// them yet**, and `attack_all_target` is reset to `true`, not to
        /// the zero the others get.
        #[test]
        fn the_battle_tallies_are_cleared() {
            let mut f = field();
            let m = card_at(&mut f, 0, location::MZONE, 0, card_type::MONSTER);
            let other = card_at(&mut f, 1, location::MZONE, 0, card_type::MONSTER);
            {
                let c = &mut f.cards[m];
                c.announce_count = 3;
                c.attack_announce_count = 2;
                c.attacked_count = 1;
                c.announced_cards.add(Some(other), 7);
                c.attacked_cards.add(Some(other), 7);
                c.battled_cards.add(Some(other), 7);
                c.attack_all_target = false;
            }
            f.core.attack_state_count = [4, 5];
            f.core.battle_phase_count = [1, 2];
            f.core.battled_count = [3, 4];

            turn_at(&mut f, 0, 0);
            step(&mut f);
            let c = &f.cards[m];
            assert_eq!(c.announce_count, 0);
            assert_eq!(c.attack_announce_count, 0);
            assert_eq!(c.attacked_count, 0);
            assert!(c.announced_cards.is_empty());
            assert!(c.attacked_cards.is_empty());
            assert!(c.battled_cards.is_empty());
            assert!(c.attack_all_target, "reset to true, not to false");
            assert_eq!(f.core.attack_state_count, [0, 0]);
            assert_eq!(f.core.battle_phase_count, [0, 0]);
            assert_eq!(f.core.battled_count, [0, 0]);
        }

        /// **The summon state counts and permissions are cleared for both
        /// players**, not only the turn player's.
        #[test]
        fn the_summon_counts_are_cleared_for_both_players() {
            let mut f = field();
            f.core.summon_count = [1, 1];
            f.core.extra_summon = [true, true];
            f.core.summon_state_count = [2, 2];
            f.core.normalsummon_state_count = [2, 2];
            f.core.flipsummon_state_count = [2, 2];
            f.core.spsummon_state_count = [2, 2];

            turn_at(&mut f, 0, 0);
            step(&mut f);
            assert_eq!(f.core.summon_count, [0, 0]);
            assert_eq!(f.core.extra_summon, [false, false]);
            assert_eq!(f.core.summon_state_count, [0, 0]);
            assert_eq!(f.core.normalsummon_state_count, [0, 0]);
            assert_eq!(f.core.flipsummon_state_count, [0, 0]);
            assert_eq!(f.core.spsummon_state_count, [0, 0]);
        }

        /// **Case 0 emplaces `RefreshRelay`**, which is what carries the
        /// reset into the next case.
        #[test]
        fn it_hands_off_to_refresh_relay() {
            let mut f = field();
            turn_at(&mut f, 0, 0);
            step(&mut f);
            assert!(queued(&f, |k| matches!(k, Kind::RefreshRelay)));
        }

        /// **Case 1 recharges counted effects**, and
        /// `EFFECT_FLAG_NO_TURN_RESET` exempts one.
        #[test]
        fn counted_effects_recharge_unless_exempt() {
            let mut f = field();
            let c = card_at(&mut f, 0, location::MZONE, 0, card_type::MONSTER);
            let mut ids = Vec::new();
            for exempt in [false, true] {
                let mut e = Effect::new(effect_type::FIELD, 0);
                e.owner = Some(c);
                e.handler = Some(c);
                e.flag[0] = flag::COUNT_LIMIT | if exempt { flag::NO_TURN_RESET } else { 0 };
                e.count_limit = 0;
                e.count_limit_max = 3;
                let id = f.new_effect(e);
                f.field_effects.rechargeable.insert(id);
                ids.push(id);
            }

            turn_at(&mut f, 1, 0);
            step(&mut f);
            assert_eq!(
                f.effects.get(ids[0]).map(|e| e.count_limit),
                Some(3),
                "recharged"
            );
            assert_eq!(
                f.effects.get(ids[1]).map(|e| e.count_limit),
                Some(0),
                "NO_TURN_RESET is exempt"
            );
        }

        /// **Only a *counted* effect recharges.** `effect::recharge` tests
        /// `EFFECT_FLAG_COUNT_LIMIT` first, so an uncounted effect in the
        /// rechargeable set keeps whatever `count_limit` it had.
        #[test]
        fn an_uncounted_effect_is_left_alone() {
            let mut f = field();
            let c = card_at(&mut f, 0, location::MZONE, 0, card_type::MONSTER);
            let mut e = Effect::new(effect_type::FIELD, 0);
            e.owner = Some(c);
            e.handler = Some(c);
            // No COUNT_LIMIT flag.
            e.count_limit = 0;
            e.count_limit_max = 3;
            let id = f.new_effect(e);
            f.field_effects.rechargeable.insert(id);

            turn_at(&mut f, 1, 0);
            step(&mut f);
            assert_eq!(
                f.effects.get(id).map(|e| e.count_limit),
                Some(0),
                "untouched, because it is not a counted effect"
            );
        }

        /// **`force_turn_end` is cleared at the top of every turn.** It is
        /// a one-turn condition, and a turn that inherits it winds itself
        /// up immediately.
        #[test]
        fn force_turn_end_is_cleared() {
            let mut f = field();
            f.core.force_turn_end = true;
            f.core.spsummon_rst = true;
            turn_at(&mut f, 1, 0);
            step(&mut f);
            assert!(!f.core.force_turn_end);
            assert!(!f.core.spsummon_rst);
        }

        /// **All six action counters are cleared together.** They are
        /// cleared as a group in the reference and splitting the group is
        /// how one gets forgotten.
        #[test]
        fn all_six_action_counters_are_cleared() {
            let mut f = field();
            for counter in [
                &mut f.core.summon_counter,
                &mut f.core.normalsummon_counter,
                &mut f.core.spsummon_counter,
                &mut f.core.flipsummon_counter,
                &mut f.core.attack_counter,
                &mut f.core.chain_counter,
            ] {
                counter.insert(
                    7,
                    crate::field::ActionCount {
                        check: None,
                        player_amount: [2, 3],
                    },
                );
            }

            turn_at(&mut f, 1, 0);
            step(&mut f);
            for (name, counter) in [
                ("summon", &f.core.summon_counter),
                ("normalsummon", &f.core.normalsummon_counter),
                ("spsummon", &f.core.spsummon_counter),
                ("flipsummon", &f.core.flipsummon_counter),
                ("attack", &f.core.attack_counter),
                ("chain", &f.core.chain_counter),
            ] {
                assert_eq!(
                    counter.get(&7).map(|a| a.player_amount),
                    Some([0, 0]),
                    "{name}_counter"
                );
                assert!(counter.contains_key(&7), "{name}: zeroed, not removed");
            }
        }

        /// **The turn counters advance and the turn player is set.**
        #[test]
        fn the_turn_is_announced_and_counted() {
            let mut f = field();
            f.infos.turn_id = 0;
            turn_at(&mut f, 1, 1);
            step(&mut f);
            assert_eq!(f.infos.turn_id, 1);
            assert_eq!(f.infos.turn_id_by_player[1], 1);
            assert_eq!(f.infos.turn_player, 1);
            assert!(f
                .messages
                .iter()
                .any(|m| matches!(m, Message::NewTurn { player: 1 })));
        }
    }

    mod the_phase_skips {
        use super::*;

        /// **`EFFECT_SKIP_TURN` skips the whole turn**, resetting three
        /// phases in one go and landing past the End Phase.
        #[test]
        fn skip_turn_resets_three_phases_and_jumps_to_the_end() {
            let mut f = field();
            player_effect(&mut f, code::SKIP_TURN, 0);
            turn_at(&mut f, 1, 0);
            step(&mut f);
            assert_ne!(f.infos.phase, phases::DRAW, "the Draw Phase never began");
            assert!(
                !f.messages
                    .iter()
                    .any(|m| matches!(m, Message::NewPhase { .. })),
                "and no phase was announced"
            );
            // `arg.step = 18` reaches case **19**, which raises
            // `EVENT_TURN_END` — and skips case 18's `PhaseEvent`, so the
            // End Phase hand limit is not enforced either.
            assert!(
                !ever(&mut f, |f| queued(f, |k| matches!(
                    k,
                    Kind::PhaseEvent {
                        phase: phases::END,
                        ..
                    }
                ))),
                "case 18 was skipped, so no hand limit"
            );

            let mut f = field();
            player_effect(&mut f, code::SKIP_TURN, 0);
            turn_at(&mut f, 1, 0);
            assert!(
                ever(&mut f, |f| raised(f, code::TURN_END)),
                "and case 19 ran"
            );
        }

        /// **The event raised on entering a phase is `EVENT_PHASE_START`,
        /// not `EVENT_PHASE`.** They are different codes — `0x2000` and
        /// `0x1000` — and different moments: the first is "this phase has
        /// begun", the second is what `PhaseEvent` gathers on.
        #[test]
        fn entering_a_phase_raises_phase_start_not_phase() {
            let mut f = field();
            turn_at(&mut f, 1, 0);
            step(&mut f);
            assert!(
                raised(&f, code::PHASE_START + u32::from(phases::DRAW)),
                "EVENT_PHASE_START + PHASE_DRAW"
            );
            assert!(
                !raised(&f, code::PHASE + u32::from(phases::DRAW)),
                "and not EVENT_PHASE + PHASE_DRAW"
            );
        }

        /// **`EFFECT_SKIP_DP` skips the draw as well as the
        /// announcement.** `arg.step = 3` reaches case **4**, so case 3 —
        /// the draw — never runs. The baseline half of this test is what
        /// makes it mean anything: without the effect, the same position
        /// draws.
        #[test]
        fn skip_dp_skips_the_draw_itself() {
            // Baseline: no skip, and the draw happens.
            let mut f = field();
            deck_of(&mut f, 0, 10);
            f.infos.turn_id = 5;
            turn_at(&mut f, 2, 0);
            assert!(until(&mut f, |f| queued(f, |k| matches!(
                k,
                Kind::Draw { .. }
            ))));

            let mut f = field();
            deck_of(&mut f, 0, 10);
            player_effect(&mut f, code::SKIP_DP, 0);
            f.infos.turn_id = 5;
            turn_at(&mut f, 2, 0);
            let reached_standby = until(&mut f, |f| f.infos.phase == phases::STANDBY);
            assert!(reached_standby, "it ran on to the Standby Phase");
            assert!(
                !queued(&f, |k| matches!(k, Kind::Draw { .. })),
                "and never drew"
            );
            assert!(
                !f.messages
                    .iter()
                    .any(|m| matches!(m, Message::NewPhase { phase } if *phase == phases::DRAW)),
                "nor announced the Draw Phase"
            );
        }

        /// **`core.force_turn_end` skips the Draw Phase too**, by the same
        /// branch as `EFFECT_SKIP_DP` — a turn being wound up by force does
        /// not draw.
        #[test]
        fn force_turn_end_skips_the_draw_phase() {
            let mut f = field();
            deck_of(&mut f, 0, 10);
            f.infos.turn_id = 5;
            f.core.force_turn_end = true;
            turn_at(&mut f, 2, 0);
            assert!(until(&mut f, |f| f.infos.phase == phases::STANDBY));
            assert!(!queued(&f, |k| matches!(k, Kind::Draw { .. })), "no draw");
        }

        /// **`EFFECT_SKIP_SP` skips the standby phase event too.**
        /// `arg.step = 6` reaches case **7**, which is Main Phase 1 — case
        /// 6, the phase event, does not run.
        #[test]
        fn skip_sp_skips_the_phase_event_too() {
            // Baseline: without the effect, case 6 emplaces the event.
            let mut f = field();
            turn_at(&mut f, 5, 0);
            assert!(until(&mut f, |f| queued(f, |k| matches!(
                k,
                Kind::PhaseEvent {
                    phase: phases::STANDBY,
                    ..
                }
            ))));

            let mut f = field();
            player_effect(&mut f, code::SKIP_SP, 0);
            turn_at(&mut f, 5, 0);
            step(&mut f);
            assert_eq!(f.infos.phase, phases::STANDBY, "the phase is still entered");
            // `ever`, not `queued`: a `PhaseEvent` that did fire would be
            // emplaced, run and gone by the time Main Phase 1 is reached.
            assert!(
                !ever(&mut f, |f| queued(f, |k| matches!(
                    k,
                    Kind::PhaseEvent {
                        phase: phases::STANDBY,
                        ..
                    }
                ))),
                "the standby phase event never fired"
            );
            assert_eq!(f.infos.phase, phases::MAIN1, "it ran on to Main Phase 1");
        }

        /// **The Standby Phase opens a window on a condition no other
        /// phase has.**
        ///
        /// The reference's comment names a card — `c89642993` — and the
        /// extra test is that the *last* instant event is something other
        /// than the standby start. So an effect that raised its own event
        /// during the standby announcement gets a response window even
        /// with no chains pending, which no other phase grants.
        #[test]
        fn the_standby_window_opens_on_a_foreign_last_event() {
            use crate::event::Event;

            // The ordinary case: the standby start is the last event and
            // nothing is pending, so no window.
            let mut f = field();
            f.core
                .instant_event
                .push_back(Event::new(code::PHASE_START + u32::from(phases::STANDBY)));
            turn_at(&mut f, 6, 0);
            step(&mut f);
            assert!(
                !queued(&f, |k| matches!(k, Kind::PointEvent { .. })),
                "nothing happened, so no window"
            );

            // Something else raised an event during the announcement.
            let mut f = field();
            f.core
                .instant_event
                .push_back(Event::new(code::PHASE_START + u32::from(phases::STANDBY)));
            f.core.instant_event.push_back(Event::new(code::TO_GRAVE));
            turn_at(&mut f, 6, 0);
            step(&mut f);
            assert!(
                queued(&f, |k| matches!(k, Kind::PointEvent { .. })),
                "a foreign last event opens the window"
            );
        }

        /// **`EFFECT_SKIP_EP` skips the End Phase's `PhaseEvent`** — which
        /// is where the hand limit lives, so a skipped End Phase does not
        /// enforce it. `arg.step = 18` reaches case **19**, past case 18.
        #[test]
        fn skip_ep_skips_the_hand_limit() {
            // Baseline: without the effect, case 18 emplaces the event.
            let mut f = field();
            turn_at(&mut f, 16, 0);
            assert!(until(&mut f, |f| queued(f, |k| matches!(
                k,
                Kind::PhaseEvent {
                    phase: phases::END,
                    ..
                }
            ))));

            let mut f = field();
            player_effect(&mut f, code::SKIP_EP, 0);
            turn_at(&mut f, 16, 0);
            step(&mut f);
            assert_eq!(f.infos.phase, phases::END);
            // Case 19 raises EVENT_TURN_END, which is how we know we landed
            // past 18 rather than on it.
            assert!(
                !ever(&mut f, |f| queued(f, |k| matches!(
                    k,
                    Kind::PhaseEvent {
                        phase: phases::END,
                        ..
                    }
                ))),
                "the hand limit was never reached"
            );

            // Case 19 raises `EVENT_TURN_END`, which is how we know it
            // landed past 18 rather than on it. A fresh run, because the
            // turn restarts and case 0 clears `used_event` — the event is
            // gone by the time the machine stops.
            let mut f = field();
            player_effect(&mut f, code::SKIP_EP, 0);
            turn_at(&mut f, 16, 0);
            assert!(
                ever(&mut f, |f| raised(f, code::TURN_END)),
                "it landed on case 19"
            );
        }

        /// **`DUEL_NO_STANDBY_PHASE` does the same as `EFFECT_SKIP_SP`** —
        /// the duel option and the effect share one branch.
        #[test]
        fn the_no_standby_option_takes_the_same_branch() {
            let mut f = field();
            f.flags |= crate::duel::flags::NO_STANDBY_PHASE;
            turn_at(&mut f, 5, 0);
            step(&mut f);
            assert!(!f
                .messages
                .iter()
                .any(|m| matches!(m, Message::NewPhase { .. })));
        }
    }

    mod the_draw {
        use super::*;

        /// **No draw on the opening turn.** `DUEL_1ST_TURN_DRAW` is off in
        /// this configuration, so the first player begins with their
        /// opening hand and nothing more.
        #[test]
        fn the_opening_turn_does_not_draw() {
            let mut f = field();
            deck_of(&mut f, 0, 10);
            f.infos.turn_id = 1;
            turn_at(&mut f, 3, 0);
            step(&mut f);
            assert!(
                !queued(&f, |k| matches!(k, Kind::Draw { .. })),
                "turn 1 draws nothing"
            );
            assert!(
                queued(&f, |k| matches!(
                    k,
                    Kind::PhaseEvent {
                        phase: phases::DRAW,
                        ..
                    }
                )),
                "but the phase still runs"
            );

            let mut f = field();
            deck_of(&mut f, 0, 10);
            f.infos.turn_id = 2;
            turn_at(&mut f, 3, 0);
            step(&mut f);
            assert!(
                queued(&f, |k| matches!(k, Kind::Draw { .. })),
                "turn 2 does"
            );
        }

        /// **A zero from any `EFFECT_DRAW_COUNT` wins outright**; otherwise
        /// the largest wins. So "draw nothing" cannot be outbid.
        #[test]
        fn a_zero_draw_count_beats_every_other() {
            let mut f = field();
            let anchor = card_at(&mut f, 0, location::MZONE, 4, card_type::MONSTER);
            for value in [3i64, 0] {
                let mut e = Effect::new(effect_type::FIELD, code::DRAW_COUNT);
                e.owner = Some(anchor);
                e.handler = Some(anchor);
                e.flag[0] = flag::PLAYER_TARGET | flag::ABSOLUTE_TARGET;
                e.range = u16::from(location::MZONE);
                e.s_range = 1;
                e.value = value;
                let id = f.new_effect(e);
                f.add_effect(id, 0);
            }
            assert_eq!(f.get_draw_count(0), 0, "the zero wins");
        }

        /// The largest wins when none of them is zero, and the player's own
        /// count is the floor.
        #[test]
        fn the_largest_draw_count_wins() {
            let mut f = field();
            assert_eq!(f.get_draw_count(0), 1, "the player's own count");

            let anchor = card_at(&mut f, 0, location::MZONE, 4, card_type::MONSTER);
            for value in [2i64, 5, 3] {
                let mut e = Effect::new(effect_type::FIELD, code::DRAW_COUNT);
                e.owner = Some(anchor);
                e.handler = Some(anchor);
                e.flag[0] = flag::PLAYER_TARGET | flag::ABSOLUTE_TARGET;
                e.range = u16::from(location::MZONE);
                e.s_range = 1;
                e.value = value;
                let id = f.new_effect(e);
                f.add_effect(id, 0);
            }
            assert_eq!(f.get_draw_count(0), 5, "not 2 + 5 + 3");
        }
    }

    mod the_phase_sequence {
        use super::*;

        /// **Main Phase 1 has no skip check** — it always happens, and
        /// `EFFECT_SKIP_M1` is `IdleCommand`'s business, not `Turn`'s.
        #[test]
        fn main_phase_1_always_begins() {
            let mut f = field();
            player_effect(&mut f, code::SKIP_M1, 0);
            turn_at(&mut f, 7, 0);
            step(&mut f);
            assert_eq!(f.infos.phase, phases::MAIN1, "entered regardless");
        }

        /// Case 9 offers the Main Phase 1 menu.
        #[test]
        fn case_9_offers_the_menu() {
            let mut f = field();
            f.infos.phase = phases::MAIN1;
            turn_at(&mut f, 9, 0);
            step(&mut f);
            assert!(queued(&f, |k| matches!(k, Kind::IdleCommand { .. })));
            assert!(f.messages.iter().any(|m| matches!(
                m,
                Message::NewPhase {
                    phase: phases::MAIN1
                }
            )));
        }

        /// **A `7` from the menu means the End Phase**, and `arg.step = 15`
        /// reaches case **16** — so Main Phase 2's menu is skipped along
        /// with the battle.
        #[test]
        fn a_seven_from_the_menu_skips_the_battle_and_main_phase_2() {
            let mut f = field();
            f.infos.phase = phases::MAIN1;
            f.core.returns.set(7);
            turn_at(&mut f, 10, 0);
            step(&mut f);
            assert_ne!(
                f.infos.phase,
                phases::BATTLE_START,
                "the Battle Phase was not entered"
            );
            assert_eq!(f.core.battle_phase_count, [0, 0], "and not counted");
            assert!(until(&mut f, |f| f.infos.phase == phases::END));
            assert!(
                !queued(&f, |k| matches!(k, Kind::IdleCommand { .. })),
                "and Main Phase 2's menu was never offered"
            );
        }

        /// **`battle_phase_count` is bumped before the skip check**, so a
        /// Battle Phase entered and immediately skipped still counts.
        #[test]
        fn a_skipped_battle_phase_still_counts_as_entered() {
            let mut f = field();
            f.infos.phase = phases::MAIN1;
            f.core.returns.set(6);
            player_effect(&mut f, code::SKIP_BP, 0);
            turn_at(&mut f, 10, 0);
            step(&mut f);
            assert_eq!(
                f.core.battle_phase_count[0], 1,
                "counted despite being skipped"
            );
            // `arg.step = 15` reaches case 16, so the skip costs the
            // player Main Phase 2 as well as the battle.
            assert!(until(&mut f, |f| f.infos.phase == phases::END));
            assert!(
                !queued(&f, |k| matches!(k, Kind::IdleCommand { .. })),
                "no Main Phase 2 either"
            );
        }

        /// **Case 12 enters the Battle Step**, which is `BattleCommand`.
        #[test]
        fn the_battle_step_hands_off_to_the_battle_command() {
            let mut f = field();
            turn_at(&mut f, 12, 0);
            step(&mut f);
            assert_eq!(f.infos.phase, phases::BATTLE_STEP);
            assert!(queued(&f, |k| matches!(k, Kind::BattleCommand { .. })));
        }

        /// **`chain_attack` is cleared entering the Battle Step**, before
        /// `BattleCommand` is reached — so the panic is caught and the flag
        /// checked after it.
        #[test]
        fn the_battle_step_clears_the_chain_attack_flag() {
            let mut f = field();
            f.core.chain_attack = true;
            turn_at(&mut f, 12, 0);
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                step(&mut f);
            }));
            assert!(!f.core.chain_attack, "cleared before the battle runs");
            assert_eq!(f.infos.phase, phases::BATTLE_STEP, "and the phase is set");
        }

        /// **A second Battle Phase rewinds to case 9** — Main Phase 1's
        /// menu — and clears every monster's attack tallies on the way.
        /// `has_performed_second_battle_phase` stops a third.
        #[test]
        fn a_second_battle_phase_rewinds_to_main_phase_1() {
            let mut f = field();
            let m = card_at(&mut f, 0, location::MZONE, 0, card_type::MONSTER);
            f.cards[m].announce_count = 2;
            f.cards[m].attacked_count = 1;
            f.infos.phase = phases::BATTLE_STEP;
            f.core.returns.set_i32(1, 1); // the second-battle flag
            turn_at(&mut f, 13, 0);
            step(&mut f);
            assert_eq!(
                f.cards[m].announce_count, 0,
                "tallies cleared for the rerun"
            );
            assert_eq!(f.cards[m].attacked_count, 0);
            // `set_step(9)` reaches case **10**, the Battle Phase's start
            // — not case 9's Main Phase 1 menu. So the player gets another
            // battle, not another Main Phase.
            f.core.returns.set(0);
            step(&mut f);
            assert_eq!(
                f.infos.phase,
                phases::BATTLE_START,
                "the Battle Phase is re-entered"
            );
            assert!(
                !queued(&f, |k| matches!(k, Kind::IdleCommand { .. })),
                "and no Main Phase menu is offered"
            );
        }

        /// **A third Battle Phase is refused.**
        /// `has_performed_second_battle_phase` is what stops the rewind
        /// happening twice, and it is reset for the next turn on the way
        /// past.
        #[test]
        fn a_third_battle_phase_is_refused() {
            let mut f = field();
            f.core.returns.set_i32(0, 0);
            f.core.returns.set_i32(1, 1); // the flag is still set
            f.emplace_at(
                Kind::Turn {
                    turn_player: 0,
                    state: TurnState {
                        has_performed_second_battle_phase: true,
                    },
                },
                13,
            );
            step(&mut f);
            assert_eq!(
                f.infos.phase,
                phases::MAIN2,
                "it goes on to Main Phase 2 rather than rewinding"
            );
        }

        /// **Slot 0 is the command and slot 1 is the second-battle flag.**
        /// A `3` in slot 0 sets `core.skip_m2`, so Main Phase 2 is entered
        /// and immediately left rather than not entered.
        #[test]
        fn a_three_from_the_battle_phase_sets_the_main_2_skip() {
            let mut f = field();
            f.core.returns.set_i32(0, 3);
            f.core.returns.set_i32(1, 0);
            turn_at(&mut f, 13, 0);
            step(&mut f);
            assert!(f.core.skip_m2, "Main Phase 2 will be left at once");
            assert_eq!(f.infos.phase, phases::MAIN2, "but it is still entered");

            let mut f = field();
            f.core.returns.set_i32(0, 0);
            f.core.returns.set_i32(1, 0);
            turn_at(&mut f, 13, 0);
            step(&mut f);
            assert!(!f.core.skip_m2);
        }

        /// **Case 15 restores the shuffle permission**, so the hand may be
        /// shuffled once in each Main Phase.
        #[test]
        fn main_phase_2_restores_the_shuffle_permission() {
            let mut f = field();
            f.infos.can_shuffle = false;
            f.infos.phase = phases::MAIN2;
            turn_at(&mut f, 15, 0);
            step(&mut f);
            assert!(f.infos.can_shuffle);
            assert!(queued(&f, |k| matches!(k, Kind::IdleCommand { .. })));
        }

        /// **Case 20 hands the turn to the other player and restarts.**
        #[test]
        fn the_turn_hands_off_to_the_other_player() {
            let mut f = field();
            turn_at(&mut f, 20, 0);
            step(&mut f);
            let next = f
                .core
                .units
                .iter()
                .find_map(|u| match u.kind {
                    Kind::Turn { turn_player, .. } => Some(turn_player),
                    _ => None,
                })
                .expect("the Turn unit is still queued");
            assert_eq!(next, 1, "the other player");
            let step_now = f
                .core
                .units
                .iter()
                .find_map(|u| match u.kind {
                    Kind::Turn { .. } => Some(u.step),
                    _ => None,
                })
                .expect("the Turn unit is still queued");
            // `set_step(RESTART)` writes `u16::MAX`, and the processor's
            // `wrapping_add(1)` has already turned it into 0 by the time
            // the handler returns. Seeing 0 here is seeing the wrap;
            // without the restart it would be 21.
            assert_eq!(step_now, 0, "and it restarts at case 0");
        }
    }

    mod refresh_relay {
        use super::*;

        /// **Two steps, and they are the two players** — the case label is
        /// a player index, not a stage. With `DUEL_RELAY` off, `recharge`
        /// is never set and both steps do nothing.
        #[test]
        fn it_does_nothing_without_relay() {
            let mut f = field();
            f.push_back(Kind::RefreshRelay);
            assert_eq!(run(&mut f), Status::End);
        }

        /// And the step really is read as a player: a relay duel would
        /// reach the unported hand-off.
        #[test]
        #[should_panic(expected = "next_player needs relay duels")]
        fn a_recharging_player_reaches_the_unported_hand_off() {
            let mut f = field();
            f.players[0].recharge = true;
            f.push_back(Kind::RefreshRelay);
            step(&mut f);
        }

        /// **The step is the player index, not a stage.** Only player 1
        /// recharging must still reach the hand-off — which it can only do
        /// if step 1 reads `players[1]`.
        #[test]
        #[should_panic(expected = "next_player needs relay duels")]
        fn step_1_reads_player_1() {
            let mut f = field();
            f.players[0].recharge = false;
            f.players[1].recharge = true;
            f.push_back(Kind::RefreshRelay);
            step(&mut f); // step 0: player 0, nothing
            step(&mut f); // step 1: player 1, panics
        }
    }
}

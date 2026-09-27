//! `AddChain`: putting an activation onto the chain stack.
//!
//! A translation of `field::process(Processors::AddChain&)`. Twelve numbered
//! steps, of which one — 9 — is never a case label: the reference uses
//! `arg.step = 9` as a *jump*, and the loop's increment then runs case 10.
//! Every such assignment here means "the next case to run is n + 1", and the
//! numbering is kept rather than tidied because it is how the reference's
//! own jumps read.
//!
//! ## The control flow, which is not linear
//!
//! ```text
//! 0 ──(activate effect)──> 10 ─> 11 ──(step = 0)──┐
//! │                        │                      │
//! │                        └──(no permission)─────┤
//! └──────────────────────────────────────────────>1 ─> 2 ─> 3 ─> … ─> 8 (done)
//! ```
//!
//! Steps 10 and 11 are a subroutine, not a tail: they decide whether a card
//! may be activated from where it is (a trap in the hand, either in the turn
//! it was set), and then *return to step 1*. An activate effect therefore
//! visits 0, 10, [11], 1, 2, … while a triggered one visits 0, 1, 2, ….
//!
//! ## What is not ported yet
//!
//! The step order is the deliverable here, and it is complete. Several steps
//! call subsystems that do not exist, and every one of those is a named
//! function that **panics** rather than a silent no-op — see
//! `docs/processor-loop.md` for the list. Nothing can reach them: no duel
//! starts yet, so no unit runs.

use crate::card::status;
use crate::chain::chain_flag;
use crate::effect::{effect_type, flag};
use crate::event::{code, PLAYER_NONE};
use crate::field::Field;
use crate::processor::Kind;

impl Field {
    /// One step of `AddChain`. Returns true when the unit is finished.
    pub(crate) fn add_chain_step(&mut self, step: u16, is_activated_effect: &mut bool) -> bool {
        match step {
            0 => {
                if self.core.new_chains.is_empty() {
                    return true;
                }
                *is_activated_effect = false;
                let effect = self.core.new_chains[0].triggering_effect;
                // An activate effect detours through the subroutine that
                // checks for permission to activate from where it is.
                if self
                    .effects
                    .get(effect)
                    .is_some_and(|e| e.is_type(effect_type::ACTIVATE))
                {
                    self.set_step(9);
                }
                false
            }

            1 => {
                let chain = self.core.new_chains[0].clone();
                // `EFFECT_ACTIVATE_COST` — a field-wide extra cost on
                // activating anything. Its `target` decides whether it
                // applies; its `operation` is the cost, run as a subunit.
                for id in self.filter_player_effect(chain.triggering_player, code::ACTIVATE_COST) {
                    let Some(e) = self.effects.get(id) else {
                        continue;
                    };
                    let (target_filter, operation) = (e.target_filter, e.operation);
                    if let Some(filter) = target_filter {
                        if !filter(self, id, None, &[]) {
                            continue;
                        }
                    }
                    if operation.is_some() {
                        self.core.sub_solving_event.push_back(chain.evt.clone());
                        self.emplace(Kind::ExecuteOperation {
                            resume: None,
                            effect: id,
                            player: chain.triggering_player,
                            subject: None,
                            args: Vec::new(),
                            was_disabled: false,
                        });
                    }
                }
                if !*is_activated_effect {
                    return false;
                }
                if self.place_activating_card(chain.triggering_effect) {
                    return true;
                }
                false
            }

            2 => {
                let chain_index = 0;
                let effect = self.core.new_chains[chain_index].triggering_effect;
                let Some(handler) = self
                    .effects
                    .get(effect)
                    .and_then(|e| e.get_handler(&self.cards))
                else {
                    return false;
                };
                let is_activate = self
                    .effects
                    .get(effect)
                    .is_some_and(|e| e.is_type(effect_type::ACTIVATE));

                let mut chain = self.core.new_chains.pop_front().unwrap();
                if is_activate {
                    let snapshot = self.cards[handler].state();
                    chain.set_triggering_state(&snapshot);
                }
                self.write_chaining_message(effect, &chain);
                self.core.chain_limit.clear();

                // The effect remembers what its card counted as, and a trap
                // monster stops counting as a trap for the rest of the chain.
                let card_type = self.get_type(handler, None, 0, PLAYER_NONE);
                if let Some(e) = self.effects.get_mut(effect) {
                    e.card_type = card_type;
                    if e.card_type
                        & (crate::card::card_type::TRAP | crate::card::card_type::MONSTER)
                        == (crate::card::card_type::TRAP | crate::card::card_type::MONSTER)
                    {
                        e.card_type -= crate::card::card_type::TRAP;
                    }
                }
                self.set_active_type(effect);
                let overlay_target = self.cards[handler].overlay_target;
                if let Some(e) = self.effects.get_mut(effect) {
                    e.active_handler = overlay_target;
                }

                chain.chain_count = self.core.current_chain.len() as u8 + 1;
                chain.target_cards.clear();
                chain.target_player = PLAYER_NONE;
                chain.target_param = 0;
                chain.disable_reason = None;
                chain.disable_player = PLAYER_NONE;
                chain.replace_op = None;
                if self.cards[handler].current.location == crate::board::location::HAND {
                    chain.flag |= chain_flag::HAND_EFFECT;
                }
                let (chain_id, triggering_player) = (chain.chain_id, chain.triggering_player);
                self.core.current_chain.push(chain);
                self.check_chain_counter(effect, triggering_player, chain_id);

                // A triggered event that was not raised by `raise_event`
                // relates its handler. The mask 0x2a0 is TRIGGER_O |
                // TRIGGER_F | QUICK_F: those *were* raised, so they are
                // excluded — unless the code is a phase event, which is
                // raised differently again.
                let (field_only, e_type, e_code) = self
                    .effects
                    .get(effect)
                    .map(|e| (e.is_flag(flag::FIELD_ONLY), e.effect_type, e.code))
                    .unwrap_or((false, 0, 0));
                if !field_only && (e_type & 0x2a0 == 0 || e_code & code::PHASE_MASK == code::PHASE)
                {
                    self.cards[handler].create_chain_relation(effect, chain_id);
                }
                if let Some(e) = self.effects.get_mut(effect) {
                    e.effect_owner = triggering_player;
                }
                // `EFFECT_DISABLE_EFFECT` is checked here, before the cost:
                // a card disabled now must not pay for an effect that will
                // be negated anyway.
                self.apply_disable_chain(effect, handler, chain_id);
                false
            }

            3 => {
                let chain = self.core.current_chain.last().unwrap().clone();
                let effect = chain.triggering_effect;
                if !self.is_flag(crate::duel::flags::USE_TRAPS_IN_NEW_CHAIN)
                    && self.get_cteffect_storing(effect, chain.triggering_player)
                {
                    let (damage_step_ok, damage_cal_ok) = self
                        .effects
                        .get(effect)
                        .map(|e| (e.is_flag(flag::DAMAGE_STEP), e.is_flag(flag::DAMAGE_CAL)))
                        .unwrap_or((false, false));
                    let phase = self.infos.phase;
                    let damage_step = phase == crate::duel::phases::DAMAGE && !damage_step_ok;
                    let damage_cal = phase == crate::duel::phases::DAMAGE_CAL && !damage_cal_ok;
                    if damage_step || damage_cal {
                        // During the Damage Step the choice is not offered;
                        // the continuous trap's own effect is taken.
                        self.core.returns.set(1);
                        return false;
                    }
                    let handler = self
                        .effects
                        .get(effect)
                        .and_then(|e| e.get_handler(&self.cards))
                        .unwrap_or(usize::MAX);
                    self.emplace(Kind::SelectEffectYesNo {
                        player: chain.triggering_player,
                        description: 94,
                        card: handler,
                    });
                } else {
                    self.core.returns.set(0);
                }
                false
            }

            4 => {
                if self.core.returns.get() == 0 {
                    self.core.select_chains.clear();
                    self.core.select_options.clear();
                    self.set_step(5);
                    return false;
                }
                if self.core.select_chains.len() > 1 {
                    let player = self.core.current_chain.last().unwrap().triggering_player;
                    self.emplace(Kind::SelectOption { player });
                } else {
                    self.core.returns.set(0);
                }
                false
            }

            5 => {
                let picked = self.core.returns.get() as usize;
                let Some(chosen) = self.core.select_chains.get(picked).cloned() else {
                    return false;
                };
                let last = self.core.current_chain.len() - 1;
                let player = self.core.current_chain[last].triggering_player;
                let effect = chosen.triggering_effect;
                let Some(handler) = self
                    .effects
                    .get(effect)
                    .and_then(|e| e.get_handler(&self.cards))
                else {
                    return false;
                };

                self.core.current_chain[last].triggering_effect = effect;
                self.core.current_chain[last].evt = chosen.evt;
                let chain_id = self.core.current_chain[last].chain_id;
                self.cards[handler].create_chain_relation(effect, chain_id);
                self.dec_count(effect, player);

                // The continuous trap's own effect is now the one being
                // activated, so it gains ACTIVATE for the duration.
                let was_activate = self
                    .effects
                    .get(effect)
                    .is_some_and(|e| e.is_type(effect_type::ACTIVATE));
                if !was_activate {
                    if let Some(e) = self.effects.get_mut(effect) {
                        e.effect_type |= effect_type::ACTIVATE;
                    }
                    self.core.current_chain[last].flag |= chain_flag::ACTIVATING;
                }
                self.core.select_chains.clear();
                self.core.select_options.clear();
                self.add_client_hint_effect(handler);
                false
            }

            6 => {
                let chain = self.core.current_chain.last().unwrap().clone();
                if self
                    .effects
                    .get(chain.triggering_effect)
                    .is_some_and(|e| e.cost.is_some())
                {
                    self.core.sub_solving_event.push_back(chain.evt);
                    self.emplace(Kind::ExecuteCost {
                        effect: chain.triggering_effect,
                        player: chain.triggering_player,
                        subject: None,
                        args: Vec::new(),
                        was_disabled: false,
                    });
                }
                false
            }

            7 => {
                let chain = self.core.current_chain.last().unwrap().clone();
                if self
                    .effects
                    .get(chain.triggering_effect)
                    .is_some_and(|e| e.target.is_some())
                {
                    self.core.sub_solving_event.push_back(chain.evt);
                    self.emplace(Kind::ExecuteTarget {
                        resume: None,
                        effect: chain.triggering_effect,
                        player: chain.triggering_player,
                        subject: None,
                        args: Vec::new(),
                        was_disabled: false,
                    });
                }
                false
            }

            8 => {
                self.break_effect(false);
                let chain = self.core.current_chain.last().unwrap().clone();
                let effect = chain.triggering_effect;
                let Some(handler) = self
                    .effects
                    .get(effect)
                    .and_then(|e| e.get_handler(&self.cards))
                else {
                    return true;
                };

                // Targets learn they were targeted — once per card as a
                // single event, then once over the set as a field event.
                if !chain.target_cards.is_empty()
                    && self
                        .effects
                        .get(effect)
                        .is_some_and(|e| e.is_flag(flag::CARD_TARGET))
                {
                    for &target in &chain.target_cards {
                        self.raise_single_event(
                            target,
                            vec![],
                            code::BECOME_TARGET,
                            Some(effect),
                            0,
                            chain.triggering_player,
                            0,
                            u32::from(chain.chain_count),
                        );
                    }
                    self.process_single_event();
                    if !chain.target_cards.is_empty() {
                        self.raise_event_over(
                            chain.target_cards.clone(),
                            code::BECOME_TARGET,
                            Some(effect),
                            0,
                            chain.triggering_player,
                            chain.triggering_player,
                            u32::from(chain.chain_count),
                        );
                    }
                }

                // A spell or trap that resolves and leaves is marked now, so
                // that everything downstream knows it is on its way out.
                if self
                    .effects
                    .get(effect)
                    .is_some_and(|e| e.is_type(effect_type::ACTIVATE))
                {
                    self.core.leave_confirmed.push(handler);
                    let printed = self.cards[handler].data.type_;
                    use crate::card::card_type as ct;
                    let stays = printed
                        & (ct::CONTINUOUS | ct::FIELD | ct::EQUIP | ct::PENDULUM | ct::LINK)
                        != 0;
                    if !stays
                        && self
                            .is_affected_by_effect(handler, code::REMAIN_FIELD)
                            .is_none()
                    {
                        self.cards[handler].set_status(status::LEAVE_CONFIRMED, true);
                    }
                }

                let last = self.core.current_chain.len() - 1;
                if self.is_continuous_card_on_field(effect, handler) {
                    self.core.current_chain[last].flag |= chain_flag::CONTINUOUS_CARD;
                }
                self.core.phase_action = true;

                self.write_chained_message(chain.chain_count);
                self.raise_event(
                    Some(handler),
                    code::CHAINING,
                    Some(effect),
                    0,
                    chain.triggering_player,
                    chain.triggering_player,
                    u32::from(chain.chain_count),
                );
                self.process_instant_event();
                self.core.just_sent_cards.clear();
                self.core.real_chain_count += 1;
                if !self.core.new_chains.is_empty() {
                    self.emplace(Kind::AddChain {
                        is_activated_effect: false,
                    });
                }
                self.adjust_all();
                true
            }

            // The subroutine: may this card be activated from where it is?
            10 => {
                *is_activated_effect = true;
                let effect = self.core.new_chains[0].triggering_effect;
                let Some(handler) = self
                    .effects
                    .get(effect)
                    .and_then(|e| e.get_handler(&self.cards))
                else {
                    self.set_step(0);
                    return false;
                };
                let Some(permit) = self.permission_code_for(handler) else {
                    self.set_step(0);
                    return false;
                };

                self.core.select_effects.clear();
                self.core.select_options.clear();
                let controller = self.cards[handler].current.controller;
                let candidates = self.filter_effect(handler, permit);
                if candidates.is_empty() {
                    return false;
                }

                // A permitting effect with a value or a count limit is a
                // *choice* the player makes; one with neither is free, and
                // contributes a single "just activate it" option.
                let mut free = false;
                for id in candidates {
                    let Some(e) = self.effects.get(id) else {
                        continue;
                    };
                    if e.has_function_value() || e.has_count_limit() {
                        if self.check_count_limit(id, controller) {
                            let description = e.description;
                            self.core.select_effects.push(Some(id));
                            self.core.select_options.push(description);
                        }
                    } else {
                        free = true;
                    }
                }
                if self.core.select_options.is_empty() {
                    // Nothing choosable and nothing free: fall through to
                    // step 1 without a decision.
                    self.set_step(0);
                    return false;
                }
                if free {
                    self.core.select_options.insert(0, 99);
                    self.core.select_effects.insert(0, None);
                }
                if self.core.select_options.len() == 1 {
                    self.core.returns.set(0);
                } else {
                    self.emplace(Kind::SelectOption { player: controller });
                }
                false
            }

            11 => {
                let effect = self.core.new_chains[0].triggering_effect;
                let handler = self
                    .effects
                    .get(effect)
                    .and_then(|e| e.get_handler(&self.cards));
                if let (Some(handler), Some(&Some(chosen))) = (
                    handler,
                    self.core
                        .select_effects
                        .get(self.core.returns.get() as usize),
                ) {
                    let controller = self.cards[handler].current.controller;
                    self.dec_count(chosen, controller);
                    self.consume_permission(chosen, effect, handler);
                }
                // Back to step 1 — the subroutine returns rather than ends.
                self.set_step(0);
                false
            }

            _ => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::{location, position};
    use crate::card::{card_type, Card, CardData};
    use crate::chain::Chain;
    use crate::effect::Effect;
    use crate::event::Event;
    use crate::processor::{Kind, Status};

    /// A set trap with one activate effect, and a chain queued for it.
    fn queued_activation(f: &mut Field, activate: bool) -> (usize, usize) {
        let mut c = Card::with_data(
            CardData {
                code: 44095762,
                type_: card_type::TRAP,
                ..Default::default()
            },
            0,
        );
        c.current.location = location::SZONE;
        c.current.position = position::FACEDOWN_DEFENSE;
        let card = f.new_card(c);

        let ty = if activate {
            effect_type::ACTIVATE | effect_type::ACTIONS
        } else {
            effect_type::TRIGGER_O | effect_type::ACTIONS
        };
        let mut e = Effect::new(ty, code::FREE_CHAIN);
        e.owner = Some(card);
        e.handler = Some(card);
        let effect = f.new_effect(e);

        let mut chain = Chain::new(effect, Event::new(code::FREE_CHAIN));
        chain.triggering_player = 0;
        f.core.new_chains.push_back(chain);
        (card, effect)
    }

    fn step_of(f: &Field) -> Option<u16> {
        f.queue().next().map(|u| u.step)
    }

    /// Nothing queued: the unit retires at once.
    #[test]
    fn an_empty_queue_finishes_immediately() {
        let mut f = Field::new(8000);
        f.push_back(Kind::AddChain {
            is_activated_effect: false,
        });
        assert_eq!(f.process(), Status::Continue);
        assert_eq!(f.process(), Status::End, "the unit retired");
    }

    /// A triggered effect goes straight on: step 0 does not jump, so the
    /// next case to run is 1.
    #[test]
    fn a_triggered_effect_does_not_detour() {
        let mut f = Field::new(8000);
        queued_activation(&mut f, false);
        f.push_back(Kind::AddChain {
            is_activated_effect: false,
        });
        f.process();
        assert_eq!(step_of(&f), Some(1), "straight to step 1");
    }

    /// An activate effect detours through the permission subroutine. The
    /// reference assigns `step = 9`, and the loop's increment means case 10
    /// runs next — 9 is never a case label.
    #[test]
    fn an_activate_effect_detours_to_the_permission_subroutine() {
        let mut f = Field::new(8000);
        queued_activation(&mut f, true);
        f.push_back(Kind::AddChain {
            is_activated_effect: false,
        });
        f.process();
        assert_eq!(step_of(&f), Some(10), "assigning 9 means case 10 runs next");
    }

    /// The subroutine *returns* to step 1 rather than ending, and it marks
    /// the unit as handling an activated effect on the way.
    #[test]
    fn the_subroutine_returns_to_step_one() {
        let mut f = Field::new(8000);
        // A set trap not set this turn and not in hand needs no permission,
        // so step 10 falls straight through.
        queued_activation(&mut f, true);
        f.push_back(Kind::AddChain {
            is_activated_effect: false,
        });
        f.process(); // step 0 -> jump
        assert_eq!(step_of(&f), Some(10));
        f.process(); // step 10 -> no permission needed -> back to 1
        assert_eq!(step_of(&f), Some(1), "the subroutine returned");

        let kind = f.queue().next().map(|u| u.kind.clone());
        match kind {
            Some(Kind::AddChain {
                is_activated_effect,
            }) => assert!(
                is_activated_effect,
                "and recorded that this is an activated effect"
            ),
            other => panic!("unexpected unit: {other:?}"),
        }
    }

    /// A trap set this turn needs permission, and with none available the
    /// subroutine still runs to completion — through 11 — rather than
    /// refusing.
    ///
    /// That reads wrong until you notice where the check already happened:
    /// `is_activateable` refuses an activation with no permitting effect
    /// long before `AddChain` sees it. By the time this subroutine runs,
    /// permission is known to exist; its job is to decide *which* one to
    /// spend, not whether any does. The reference makes this visible by
    /// leaving the empty case alone: `if(!eset.empty())` skips the whole
    /// block and falls to the next step untouched.
    #[test]
    fn the_subroutine_completes_even_with_nothing_to_offer() {
        let mut f = Field::new(8000);
        let (card, _) = queued_activation(&mut f, true);
        f.cards[card].set_status(status::SET_TURN, true);
        f.push_back(Kind::AddChain {
            is_activated_effect: false,
        });
        f.process(); // 0 -> 10
        assert_eq!(step_of(&f), Some(10));
        f.process(); // 10: a permission code, but nothing offering it
        assert_eq!(step_of(&f), Some(11), "falls to 11, not back to 1");
        assert!(f.core.select_options.is_empty());
        f.process(); // 11 -> back to 1
        assert_eq!(step_of(&f), Some(1));
    }

    /// With a permitting effect that has a count limit, the subroutine
    /// offers it as a choice.
    #[test]
    fn a_permitting_effect_becomes_an_offer() {
        let mut f = Field::new(8000);
        let (card, _) = queued_activation(&mut f, true);
        f.cards[card].set_status(status::SET_TURN, true);

        let mut permit = Effect::new(effect_type::SINGLE, code::TRAP_ACT_IN_SET_TURN);
        permit.owner = Some(card);
        permit.handler = Some(card);
        permit.flag[0] |= flag::COUNT_LIMIT;
        permit.count_limit = 1;
        permit.description = 1234;
        let permit = f.new_effect(permit);
        f.cards[card]
            .single_effect
            .insert(code::TRAP_ACT_IN_SET_TURN, permit);

        f.push_back(Kind::AddChain {
            is_activated_effect: false,
        });
        f.process();
        f.process();
        assert_eq!(f.core.select_options, vec![1234]);
        assert_eq!(f.core.select_effects, vec![Some(permit)]);
        assert_eq!(
            f.core.returns.get(),
            0,
            "a single option is taken without asking"
        );
    }

    /// A permitting effect with neither a value nor a count limit is free,
    /// and contributes the "just activate it" option at the front.
    #[test]
    fn a_free_permission_adds_the_plain_option_first() {
        let mut f = Field::new(8000);
        let (card, _) = queued_activation(&mut f, true);
        f.cards[card].set_status(status::SET_TURN, true);

        for (limited, description) in [(false, 0u64), (true, 1234)] {
            let mut permit = Effect::new(effect_type::SINGLE, code::TRAP_ACT_IN_SET_TURN);
            permit.owner = Some(card);
            permit.handler = Some(card);
            permit.description = description;
            if limited {
                permit.flag[0] |= flag::COUNT_LIMIT;
                permit.count_limit = 1;
            }
            let permit = f.new_effect(permit);
            f.cards[card]
                .single_effect
                .insert(code::TRAP_ACT_IN_SET_TURN, permit);
        }

        f.push_back(Kind::AddChain {
            is_activated_effect: false,
        });
        f.process();
        f.process();
        assert_eq!(
            f.core.select_options,
            vec![99, 1234],
            "the free option is inserted at the front"
        );
        assert_eq!(f.core.select_effects[0], None, "and stands for no effect");
    }
}

//! `PointEvent`: the window in which a timing's triggers are offered.
//!
//! A translation of `field::process(Processors::PointEvent&)`. This is where
//! SEGOC lives — Simultaneous Effects Go On Chain, the rule deciding the
//! order in which effects that all triggered at once are put onto the chain.
//!
//! ## The shape
//!
//! ```text
//! 0 ──(skip_trigger: step = 7)─────────────────────────────┐
//! │                                                        │
//! └> 1 ─> 2 ─> 3 ──(more, or another player: step = 1)──> 2 │
//!             │                                            │
//!             └> 4 ─> [5] ─> 6 ──(step = 3)──> 4            │
//!                             │                            │
//!                             └────────────────────────────┴> 7 ─> 8 ─> 9 ─> 10
//!
//! 30 <─> 31   32 <─> 33        (a separate entry, at step 30)
//! ```
//!
//! Steps 2-3 offer **mandatory** triggers and 4-6 offer **optional** ones,
//! each looping until the current player has nothing left and then switching
//! to the other player. Step 7 onward is the tail: ignition priority, the
//! quick-effect window, and finally either solving the chain that was built
//! or clearing up.
//!
//! Steps 30-33 are a **separate entry point**, reached only by emplacing the
//! unit at step 30 (`emplace_process<PointEvent>(Step{ 30 }, ...)`). They
//! offer each player their free-chain continuous effects in turn, looping
//! until declined, and then restart the unit at step 0.
//!
//! ## Step 5 runs on one path only
//!
//! Both the "nothing offered" and the "chose from a list" paths assign
//! `arg.step = 5`, and the loop's increment therefore runs case **6**. Only
//! the yes/no path falls through without assigning, so case 5 runs only
//! after `SelectEffectYesNo` — and all it does is subtract one, turning
//! yes/no (1/0) into an index/decline (0/-1) so that case 6 can read both
//! paths the same way.
//!
//! That is why -1 and -2 mean different things: -1 is "declined", from
//! either path, and -2 is "there was nothing to decline".

use crate::board::location;
use crate::card::{card_type, status};
use crate::chain::{chain_flag, Chain};
use crate::duel::{flags, phases};
use crate::effect::{effect_type, flag};
use crate::event::{code, Event, PLAYER_NONE};
use crate::field::Field;
use crate::processor::{Kind, RESTART};

/// The three `skip_*` flags the window is opened with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PointEventSkip {
    pub trigger: bool,
    pub freechain: bool,
    pub new: bool,
}

impl Field {
    /// One step of `PointEvent`. Returns true when the unit is finished.
    pub(crate) fn point_event_step(&mut self, step: u16, skip: PointEventSkip) -> bool {
        match step {
            0 => {
                self.core.select_chains.clear();
                // The events this window is *about* become the point events,
                // which is what makes them answerable for its duration.
                let instant = std::mem::take(&mut self.core.instant_event);
                self.core.point_event.extend(instant);
                if skip.trigger {
                    self.set_step(7);
                    return false;
                }
                // The `_s` lists are the working copies the two offer loops
                // consume. They are spliced to the *front*, so anything left
                // over from an earlier window is offered after the new
                // arrivals rather than before.
                let f = std::mem::take(&mut self.core.new_fchain);
                for chain in f.into_iter().rev() {
                    self.core.new_fchain_s.push_front(chain);
                }
                let o = std::mem::take(&mut self.core.new_ochain);
                for chain in o.into_iter().rev() {
                    self.core.new_ochain_s.push_front(chain);
                }
                let delayed = std::mem::take(&mut self.core.delayed_activate_event);
                self.core.full_event.extend(delayed);
                self.core.delayed_quick.clear();
                std::mem::swap(
                    &mut self.core.delayed_quick_tmp,
                    &mut self.core.delayed_quick,
                );
                self.core.current_player = self.infos.turn_player;
                self.set_step(1);
                false
            }

            // **Unreachable.** Every jump to "the mandatory loop" assigns
            // `step = 1`, and the loop's increment then runs case 2 — so
            // nothing ever dispatches case 1. It is kept because the
            // reference keeps it, and because deleting it would make the
            // assignments read as jumps to the gather rather than to the
            // step after this one.
            1 => false,

            2 => {
                self.core.select_chains.clear();
                let mut kept = Vec::new();
                let current = self.core.current_player;
                for chain in std::mem::take(&mut self.core.new_fchain_s) {
                    match self.refresh_mandatory_trigger(chain) {
                        Some(chain) => {
                            if chain.triggering_player == current {
                                self.core.select_chains.push_back(chain.clone());
                            }
                            kept.push(chain);
                        }
                        None => continue,
                    }
                }
                self.core.new_fchain_s = kept.into();

                if self.core.select_chains.is_empty() {
                    self.core.returns.set(-1);
                } else {
                    self.keep_only_first_trigger_group();
                    if self.core.select_chains.len() == 1 {
                        self.core.returns.set(0);
                    } else {
                        // A mandatory trigger must be chosen, so the offer
                        // is forced: the player picks the order, not whether.
                        self.emplace(Kind::SelectChain {
                            player: current,
                            spe_count: 0x7f,
                            forced: true,
                        });
                    }
                }
                false
            }

            3 => {
                if self.core.returns.get() == -1 {
                    if !self.core.new_fchain_s.is_empty() {
                        // The turn player had nothing; ask the other.
                        self.core.current_player = 1 - self.infos.turn_player;
                        self.set_step(1);
                    } else {
                        self.core.current_player = self.infos.turn_player;
                    }
                    return false;
                }
                self.take_offered_chain(SelectChainKind::Mandatory);
                self.set_step(1);
                false
            }

            4 => {
                self.core.select_chains.clear();
                let mut kept = Vec::new();
                let current = self.core.current_player;
                for chain in std::mem::take(&mut self.core.new_ochain_s) {
                    match self.refresh_optional_trigger(chain) {
                        Some(chain) => {
                            if chain.triggering_player == current {
                                self.core.select_chains.push_back(chain.clone());
                            }
                            kept.push(chain);
                        }
                        None => continue,
                    }
                }
                self.core.new_ochain_s = kept.into();

                if self.core.select_chains.is_empty() {
                    self.core.returns.set(-2);
                    self.set_step(5);
                    return false;
                }
                if self.is_flag(flags::TCG_SEGOC_NONPUBLIC) {
                    self.core.new_ochain_h.clear();
                }
                self.keep_only_first_trigger_group();

                // One option with nothing yet on the chain is a yes/no
                // question rather than a list. This is the only path that
                // reaches case 5.
                if self.core.select_chains.len() == 1 && self.core.current_chain.is_empty() {
                    let handler = self
                        .core
                        .select_chains
                        .front()
                        .and_then(|c| self.effects.get(c.triggering_effect))
                        .and_then(|e| e.get_handler(&self.cards))
                        .unwrap_or(usize::MAX);
                    self.emplace(Kind::SelectEffectYesNo {
                        player: current,
                        description: 221,
                        card: handler,
                    });
                } else {
                    self.emplace(Kind::SelectChain {
                        player: current,
                        spe_count: 0x7f,
                        forced: false,
                    });
                    self.set_step(5);
                }
                false
            }

            // Reached only from the yes/no path above: turn 1/0 into 0/-1
            // so that case 6 can read a list answer and a yes/no answer the
            // same way.
            5 => {
                let n = self.core.returns.get();
                self.core.returns.set(n - 1);
                false
            }

            6 => {
                let ret = self.core.returns.get();
                let first_trigger = self.is_flag(flags::TCG_SEGOC_FIRSTTRIGGER);

                // Nothing was offered, or the player declined everything.
                if ret == -2 || (ret == -1 && !first_trigger) {
                    let offered: Vec<u16> =
                        self.core.select_chains.iter().map(|c| c.chain_id).collect();
                    for chain in self.core.select_chains.clone() {
                        if let Some(e) = self.effects.get_mut(chain.triggering_effect) {
                            e.active_type = 0;
                        }
                    }
                    self.core
                        .new_ochain_s
                        .retain(|c| !offered.contains(&c.chain_id));
                    if !self.core.new_ochain_s.is_empty() {
                        self.core.current_player = 1 - self.infos.turn_player;
                        self.set_step(3);
                    } else {
                        self.core.current_player = self.infos.turn_player;
                        self.set_step(6);
                    }
                    return false;
                }
                // Declined under TCG SEGOC: only the group sharing the front
                // event id is given up, and only for this player. The rest
                // stay on offer.
                if ret == -1 {
                    let discarded = self.core.select_chains.front().map(|c| c.event_id);
                    let current = self.core.current_player;
                    if let Some(discarded) = discarded {
                        self.core.new_ochain_s.retain(|c| {
                            !(c.event_id == discarded && c.triggering_player == current)
                        });
                    }
                    self.set_step(3);
                    return false;
                }
                self.take_offered_chain(SelectChainKind::Optional);
                self.set_step(3);
                false
            }

            7 => {
                self.core.select_chains.clear();
                false
            }

            8 => {
                self.gather_ignition_priority(skip);
                false
            }

            9 => {
                self.infos.priorities = [0, 0];
                if self.core.current_chain.is_empty() {
                    if !self.core.hand_adjusted {
                        // Whose window it is. Inverted quick priority gives
                        // it to the non-turn player first.
                        let player = if self.is_flag(flags::INVERTED_QUICK_PRIORITY) {
                            1 - self.infos.turn_player
                        } else {
                            self.infos.turn_player
                        };
                        self.emplace(Kind::QuickEffect {
                            skip_freechain: skip.freechain,
                            player,
                            is_opponent: false,
                        });
                    }
                } else {
                    // Mid-chain the window belongs to the *other* player
                    // from whoever built the last link.
                    let last = self.core.current_chain.last().unwrap().triggering_player;
                    self.emplace(Kind::QuickEffect {
                        skip_freechain: skip.freechain,
                        player: 1 - last,
                        is_opponent: false,
                    });
                }
                false
            }

            10 => {
                self.core.new_ochain_h.clear();
                self.core.full_event.clear();
                self.core.delayed_quick.clear();
                self.core.chain_limit.clear();
                if !self.core.current_chain.is_empty() {
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
                        skip: crate::solve_chain::SolveChainSkip {
                            trigger: skip.trigger,
                            freechain: skip.freechain,
                            new: skip.new,
                        },
                    });
                } else {
                    let point = std::mem::take(&mut self.core.point_event);
                    self.core.used_event.extend(point);
                    self.reset_chain();
                    self.core.returns.set(0);
                }
                self.take_forced_attack();
                true
            }

            // The separate entry: each player's free-chain continuous
            // effects, offered until declined.
            30 => self.offer_free_chain_continuous(self.infos.turn_player, 31),
            31 => {
                if self.core.returns.get() == -1 {
                    return false;
                }
                self.solve_offered_continuous();
                // 29 + 1 = 30: ask the same player again.
                self.set_step(29);
                false
            }
            32 => self.offer_free_chain_continuous(1 - self.infos.turn_player, RESTART),
            33 => {
                if self.core.returns.get() == -1 {
                    self.set_step(RESTART);
                    return false;
                }
                self.solve_offered_continuous();
                self.set_step(31);
                false
            }

            // The reference's fall-through, which also takes a pending
            // forced attack. Reached for any step outside the two ranges.
            _ => {
                self.take_forced_attack();
                true
            }
        }
    }

    /// An attack that a card forced and that has been waiting for a window
    /// to close. Checked both at the end of step 10 and on the unit's
    /// fall-through, which is why it is a helper.
    fn take_forced_attack(&mut self) {
        if self.core.set_forced_attack {
            self.core.set_forced_attack = false;
            self.emplace(Kind::ForcedBattle {
                state: Default::default(),
            });
        }
    }

    /// `DUEL_TCG_SEGOC_FIRSTTRIGGER` — keep only the offers sharing the
    /// earliest event id, so a player orders one batch at a time rather than
    /// interleaving batches.
    ///
    /// Not in this project's configuration, so the whole function is inert
    /// here; it is ported because the flag exists and a configuration change
    /// must not silently lose it.
    fn keep_only_first_trigger_group(&mut self) {
        if !self.is_flag(flags::TCG_SEGOC_FIRSTTRIGGER) {
            return;
        }
        let mut sorted: Vec<Chain> = self.core.select_chains.drain(..).collect();
        sorted.sort_by_key(|c| c.event_id);
        let Some(first) = sorted.first().map(|c| c.event_id) else {
            return;
        };
        sorted.retain(|c| c.event_id == first);
        self.core.select_chains = sorted.into();
    }

    /// Take the chain the player chose, mark its handler as chaining, spend
    /// its activation, and queue `AddChain`.
    fn take_offered_chain(&mut self, kind: SelectChainKind) {
        let index = self.core.returns.get() as usize;
        let Some(chain) = self.core.select_chains.remove(index) else {
            return;
        };
        let (effect, tp, chain_id) = (
            chain.triggering_effect,
            chain.triggering_player,
            chain.chain_id,
        );
        if let Some(handler) = self
            .effects
            .get(effect)
            .and_then(|e| e.get_handler(&self.cards))
        {
            self.cards[handler].set_status(status::CHAINING, true);
        }
        self.dec_count(effect, tp);
        self.core.new_chains.push_back(chain);
        self.emplace(Kind::AddChain {
            is_activated_effect: false,
        });
        match kind {
            SelectChainKind::Mandatory => {
                self.core.new_fchain_s.retain(|c| c.chain_id != chain_id);
            }
            SelectChainKind::Optional => {
                self.core.new_ochain_s.retain(|c| c.chain_id != chain_id);
                self.core.new_ochain_h.retain(|c| c.chain_id != chain_id);
            }
        }
    }

    /// Offer one player their free-chain continuous effects. Returns false
    /// (not finished) always; `empty_step` is where to jump when there are
    /// none to offer.
    fn offer_free_chain_continuous(&mut self, player: u8, empty_step: u16) -> bool {
        self.core.select_chains.clear();
        self.core.spe_effect[player as usize] = 0;
        let nil = Event::new(code::FREE_CHAIN);
        for effect in self
            .field_effects
            .continuous
            .equal_range(code::FREE_CHAIN)
            .to_vec()
        {
            let owned_by = self
                .effects
                .get(effect)
                .map_or(PLAYER_NONE, |e| e.get_handler_player(&self.cards));
            if owned_by != player {
                continue;
            }
            if self.is_activateable(effect, player, &nil, false, false, false, false, false) {
                let mut chain = Chain::new(effect, nil.clone());
                chain.triggering_player = player;
                self.core.select_chains.push_back(chain);
                self.core.spe_effect[player as usize] += 1;
            }
        }
        if self.core.select_chains.is_empty() {
            self.set_step(empty_step);
        } else {
            let spe_count = self.core.spe_effect[player as usize];
            self.emplace(Kind::SelectChain {
                player,
                spe_count,
                forced: false,
            });
        }
        false
    }

    fn solve_offered_continuous(&mut self) {
        let index = self.core.returns.get() as usize;
        let Some(chain) = self.core.select_chains.get(index).cloned() else {
            return;
        };
        self.core.select_chains.clear();
        let player = self
            .effects
            .get(chain.triggering_effect)
            .map_or(PLAYER_NONE, |e| e.get_handler_player(&self.cards));
        self.solve_continuous(
            player,
            chain.triggering_effect,
            Event::new(code::FREE_CHAIN),
        );
    }
}

/// Which of the two offer loops a taken chain came from. They clean up
/// different lists, and the optional one has a second to clear.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SelectChainKind {
    Mandatory,
    Optional,
}

impl Field {
    /// The per-chain body of case 2: bring a mandatory trigger's snapshot up
    /// to date, then decide whether it is still offerable.
    ///
    /// `None` means drop it. The reference also clears `active_type` on the
    /// way out, which matters: an effect that was offered and then withdrawn
    /// must not keep claiming the type it had when it triggered.
    fn refresh_mandatory_trigger(&mut self, mut chain: Chain) -> Option<Chain> {
        let effect = chain.triggering_effect;
        let handler = self
            .effects
            .get(effect)
            .and_then(|e| e.get_handler(&self.cards))?;
        self.refresh_triggering_state(&mut chain, handler);

        let tp = chain.triggering_player;
        let in_hand = self.cards[handler].current.location == location::HAND;
        let private = self.is_flag(flags::TRIGGER_WHEN_PRIVATE_KNOWLEDGE);
        let ok = (!chain.was_just_sent || in_hand)
            && self.check_trigger_effect(&chain)
            && self.is_chainable(effect, tp)
            // neglect_cond is TRUE: the condition was checked at gather
            // time and must not be asked again, because the board has moved
            // since and a "when this was destroyed" condition would now be
            // false.
            && self.is_activateable(effect, tp, &chain.evt.clone(), true, false, false, private, private);
        if ok {
            Some(chain)
        } else {
            if let Some(e) = self.effects.get_mut(effect) {
                e.active_type = 0;
            }
            None
        }
    }

    /// The per-chain body of case 4. Three things the mandatory version does
    /// not do, each of which is a rule:
    ///
    /// - a field effect ranged to the hand, whose handler *is* in the hand,
    ///   gets its relation created here if its condition now holds. That is
    ///   the "reveal it from your hand" case, and it is why the optional
    ///   gather was willing to offer it without a relation.
    /// - `check_nonpublic_trigger` decides whether a trigger from a hidden
    ///   zone may be used at all, and records it as a hand trigger.
    /// - `check_spself_from_hand_trigger` stops a player summoning
    ///   themselves from the hand twice on one chain.
    fn refresh_optional_trigger(&mut self, mut chain: Chain) -> Option<Chain> {
        let effect = chain.triggering_effect;
        let handler = self
            .effects
            .get(effect)
            .and_then(|e| e.get_handler(&self.cards))?;

        let mut updated = false;
        if self.cards[handler].has_chain_relation(effect, chain.chain_id) {
            chain.set_triggering_state(&self.cards[handler].state());
            updated = true;
        }

        let (field_only, is_field, hand_ranged) = self
            .effects
            .get(effect)
            .map(|e| {
                (
                    e.is_flag(flag::FIELD_ONLY),
                    e.is_type(effect_type::FIELD),
                    e.range & u16::from(location::HAND) != 0,
                )
            })
            .unwrap_or((false, false, false));
        if !field_only
            && is_field
            && hand_ranged
            && self.cards[handler].current.location == location::HAND
        {
            let controller = self.cards[handler].current.controller;
            if !self.cards[handler].has_chain_relation(effect, chain.chain_id)
                && self.is_condition_check(effect, controller, &chain.evt.clone())
            {
                self.cards[handler].create_chain_relation(effect, chain.chain_id);
            }
            let snapshot = self.cards[handler].state();
            if let Some(e) = self.effects.get_mut(effect) {
                e.set_activate_location(&snapshot);
            }
            chain.triggering_player = self.cards[handler].current.controller;
            chain.set_triggering_state(&snapshot);
            updated = true;
        }

        let by_event = self
            .effects
            .get(effect)
            .is_some_and(|e| e.is_flag(flag::EVENT_PLAYER));
        if chain.triggering_player != self.cards[handler].current.controller && !by_event {
            chain.triggering_player = self.cards[handler].current.controller;
            if !updated {
                chain.set_triggering_state(&self.cards[handler].state());
            }
        }

        let tp = chain.triggering_player;
        let in_hand = self.cards[handler].current.location == location::HAND;
        let private = self.is_flag(flags::TRIGGER_WHEN_PRIVATE_KNOWLEDGE);
        let ok = (!chain.was_just_sent || in_hand)
            && (private || self.check_nonpublic_trigger(&mut chain))
            && self.check_trigger_effect(&chain)
            && self.is_chainable(effect, tp)
            && self.is_activateable(
                effect,
                tp,
                &chain.evt.clone(),
                true,
                false,
                false,
                private,
                private,
            )
            && self.check_spself_from_hand_trigger(&chain);
        if ok {
            Some(chain)
        } else {
            if let Some(e) = self.effects.get_mut(effect) {
                e.active_type = 0;
            }
            None
        }
    }

    /// The reference's once-only `update_triggering_state` lambda. The guard
    /// is why it is a helper rather than two calls: the snapshot is taken at
    /// most once per refresh even when both reasons to take it apply.
    fn refresh_triggering_state(&mut self, chain: &mut Chain, handler: usize) {
        let mut updated = false;
        if self.cards[handler].has_chain_relation(chain.triggering_effect, chain.chain_id) {
            chain.set_triggering_state(&self.cards[handler].state());
            updated = true;
        }
        let by_event = self
            .effects
            .get(chain.triggering_effect)
            .is_some_and(|e| e.is_flag(flag::EVENT_PLAYER));
        if chain.triggering_player != self.cards[handler].current.controller && !by_event {
            chain.triggering_player = self.cards[handler].current.controller;
            if !updated {
                chain.set_triggering_state(&self.cards[handler].state());
            }
        }
    }

    /// `effect::get_speed` — spell speed.
    ///
    /// Note that an *activated* monster effect is speed 0, not 1: a monster
    /// card has no activation speed of its own, and `is_chainable` refuses
    /// anything at speed 1 or below that is an activate effect. The Counter
    /// Trap's 3 is what lets it chain to another trap.
    pub fn get_speed(&self, id: crate::event::EffectId) -> i32 {
        let Some(e) = self.effects.get(id) else {
            return 0;
        };
        if !e.is_type(effect_type::ACTIONS) {
            return 0;
        }
        if e.is_type(effect_type::TRIGGER_O | effect_type::TRIGGER_F | effect_type::IGNITION) {
            return 1;
        }
        if e.is_type(effect_type::QUICK_O | effect_type::QUICK_F) {
            return 2;
        }
        if !e.is_type(effect_type::ACTIVATE) {
            return 0;
        }
        let Some(handler) = e.handler else {
            return 0;
        };
        let printed = self.cards[handler].data.type_;
        if printed & card_type::MONSTER != 0 {
            return 0;
        }
        if printed & card_type::SPELL != 0 {
            let quick = printed & card_type::QUICKPLAY != 0
                || self
                    .is_affected_by_effect(handler, code::BECOME_QUICK)
                    .is_some();
            return if quick { 2 } else { 1 };
        }
        if printed & card_type::COUNTER != 0 {
            3
        } else {
            2
        }
    }

    /// `effect::is_chainable` — may this go on the chain *now*?
    ///
    /// The speed comparison is the familiar rule. The exception above it is
    /// not: an optional trigger whose handler is **in the hand** is compared
    /// against `> 2` rather than `<`, so it may be chained to a speed-2
    /// effect but not to a Counter Trap. Collapsing the two into one
    /// comparison gets hand triggers wrong in both directions.
    pub fn is_chainable(&self, id: crate::event::EffectId, tp: u8) -> bool {
        let Some(e) = self.effects.get(id) else {
            return false;
        };
        if !e.is_type(effect_type::ACTIONS) {
            return false;
        }
        let speed = self.get_speed(id);
        if e.is_type(effect_type::ACTIVATE) && speed <= 1 && !e.is_flag2(crate::effect::flag2::COF)
        {
            return false;
        }
        if let Some(last) = self.core.current_chain.last() {
            let last_speed = self.get_speed(last.triggering_effect);
            let hand_trigger = !e.is_flag(flag::FIELD_ONLY)
                && e.is_type(effect_type::TRIGGER_O)
                && e.get_handler(&self.cards)
                    .is_some_and(|h| self.cards[h].current.location == location::HAND);
            if hand_trigger {
                if last_speed > 2 {
                    return false;
                }
            } else if speed < last_speed {
                return false;
            }
        }
        let _ = tp;
        // `chain_limit` and `chain_limit_p` hold per-chain restrictions set
        // by cards; they are empty until such a card exists.
        true
    }

    /// `check_trigger_effect` — may this trigger be used from where its card
    /// is?
    ///
    /// Under `DUEL_TRIGGER_ONLY_IN_LOCATION` — which **is** in this
    /// project's configuration — the whole question collapses to "is the
    /// handler still related to this chain", and none of the rest runs. The
    /// rest is ported anyway, because the flag is a duel option.
    pub fn check_trigger_effect(&self, chain: &Chain) -> bool {
        let effect = chain.triggering_effect;
        let Some(handler) = self
            .effects
            .get(effect)
            .and_then(|e| e.get_handler(&self.cards))
        else {
            return false;
        };
        let related = self.cards[handler].has_chain_relation(effect, chain.chain_id);
        if self.is_flag(flags::TRIGGER_ONLY_IN_LOCATION) {
            return related;
        }
        let Some(e) = self.effects.get(effect) else {
            return false;
        };
        if e.is_type(effect_type::FIELD) && !related {
            return false;
        }
        if e.code == code::FLIP && self.infos.phase == phases::DAMAGE {
            return true;
        }
        if self.is_flag(flags::TRIGGER_WHEN_PRIVATE_KNOWLEDGE) {
            return true;
        }
        let loc = self.cards[handler].current.location;
        if loc & location::DECK != 0 && chain.triggering_location != u16::from(location::DECK) {
            return false;
        }
        let hidden_zone = u16::from(location::DECK | location::HAND | location::EXTRA);
        if chain.triggering_location & hidden_zone != 0
            && chain.triggering_position & crate::board::position::FACEDOWN != 0
        {
            return true;
        }
        loc & (location::DECK | location::HAND | location::EXTRA) == 0
            || self.cards[handler].current.is_faceup()
    }

    /// `check_nonpublic_trigger` — a trigger from a hidden zone.
    ///
    /// Two things at once, which is why it takes the chain mutably: it marks
    /// the chain as a hand trigger and files it in `new_ochain_h`, *and*
    /// answers whether it may be used. Under TCG SEGOC the answer is always
    /// yes and only the filing happens.
    pub fn check_nonpublic_trigger(&mut self, chain: &mut Chain) -> bool {
        let effect = chain.triggering_effect;
        let Some(handler) = self
            .effects
            .get(effect)
            .and_then(|e| e.get_handler(&self.cards))
        else {
            return false;
        };
        let related = self.cards[handler].has_chain_relation(effect, chain.chain_id);
        let hidden = u16::from(location::HAND | location::DECK);
        let Some(e) = self.effects.get(effect) else {
            return false;
        };
        let from_hidden = !e.is_flag(flag::FIELD_ONLY)
            && ((e.is_type(effect_type::SINGLE)
                && !e.is_flag(flag::SINGLE_RANGE)
                && related
                && chain.triggering_location & hidden != 0)
                || e.range & hidden != 0);
        if !from_hidden {
            return true;
        }
        let range = e.range;
        chain.flag |= chain_flag::HAND_TRIGGER;
        self.core.new_ochain_h.push_back(chain.clone());
        if self.is_flag(flags::TCG_SEGOC_NONPUBLIC) {
            return true;
        }
        let face_down_in_hand = chain.triggering_location == u16::from(location::HAND)
            && chain.triggering_position & crate::board::position::FACEDOWN != 0;
        let in_deck = chain.triggering_location == u16::from(location::DECK);
        let out_of_range = range != 0 && range & chain.triggering_location == 0;
        !(face_down_in_hand || in_deck || out_of_range)
    }

    /// `check_spself_from_hand_trigger` — one self-summon from the hand per
    /// player per chain.
    pub fn check_spself_from_hand_trigger(&self, chain: &Chain) -> bool {
        let summons_self = self
            .effects
            .get(chain.triggering_effect)
            .is_some_and(|e| e.status & effect_status::SUMMON_SELF != 0);
        if !(summons_self && chain.flag & chain_flag::HAND_TRIGGER != 0) {
            return true;
        }
        let tp = chain.triggering_player;
        !self.core.current_chain.iter().any(|c| {
            c.triggering_player == tp
                && self
                    .effects
                    .get(c.triggering_effect)
                    .is_some_and(|e| e.status & effect_status::SUMMON_SELF != 0)
                && c.flag & chain_flag::HAND_TRIGGER != 0
        })
    }

    /// Step 8: ignition effects that keep priority.
    ///
    /// A ruling that only applies under the obsolete-ignition or TCG
    /// fast-effect options, and only in a Main Phase with no chain running.
    /// Neither option is in this project's configuration, so this is inert
    /// here — ported because the options exist.
    fn gather_ignition_priority(&mut self, skip: PointEventSkip) {
        let enabled = self.is_flag(flags::OCG_OBSOLETE_IGNITION)
            || self.is_flag(flags::TCG_FAST_EFFECT_IGNITION);
        let in_main = self.infos.phase == phases::MAIN1 || self.infos.phase == phases::MAIN2;
        if skip.freechain || !enabled || !in_main {
            return;
        }
        if !self.core.current_chain.is_empty() {
            return;
        }
        if !self.is_flag(flags::TCG_FAST_EFFECT_IGNITION) && !self.ignition_window_is_open() {
            return;
        }
        let turn_player = self.infos.turn_player;
        let codes: Vec<(u32, crate::event::EffectId)> =
            self.field_effects.ignition.iter().copied().collect();
        for (_, effect) in codes {
            let Some(handler) = self
                .effects
                .get(effect)
                .and_then(|e| e.get_handler(&self.cards))
            else {
                continue;
            };
            let e_code = self.effects.get(effect).map_or(0, |e| e.code);
            let mut evt = Event::new(e_code);
            evt.event_player = PLAYER_NONE;
            evt.reason_player = PLAYER_NONE;
            let on_field = self.cards[handler].current.location == location::MZONE;
            if (self.is_flag(flags::TCG_FAST_EFFECT_IGNITION) || on_field)
                && self.is_chainable(effect, turn_player)
                && self.is_activateable(
                    effect,
                    turn_player,
                    &evt,
                    false,
                    false,
                    false,
                    false,
                    false,
                )
            {
                let mut chain = Chain::new(effect, evt);
                chain.chain_id = self.next_field_id();
                chain.triggering_player = turn_player;
                chain.set_triggering_state(&self.cards[handler].state());
                self.core.ignition_priority_chains.push_back(chain);
            }
        }
    }

    /// The OCG side of the obsolete-ignition ruling: the window is open
    /// after a chain ended, or after the turn player summoned.
    fn ignition_window_is_open(&self) -> bool {
        if self.check_event(code::CHAIN_END).is_some() {
            return true;
        }
        for c in [
            code::SUMMON_SUCCESS,
            code::SPSUMMON_SUCCESS,
            code::FLIP_SUMMON_SUCCESS,
        ] {
            if let Some(e) = self.check_event(c) {
                return e.reason_player == self.infos.turn_player;
            }
        }
        false
    }
}

/// `EFFECT_STATUS_*` — per-effect status bits. Two of the three are live;
/// the reference has the middle one commented out.
pub mod effect_status {
    pub const AVAILABLE: u32 = 0x0001;
    // `EFFECT_STATUS_ACTIVATED 0x0002` is commented out in the reference
    // header; `Effect.IsActivated` is computed, not a status bit.
    pub const SUMMON_SELF: u32 = 0x0004;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::position;
    use crate::card::{Card, CardData};
    use crate::effect::Effect;
    use crate::processor::Kind;

    /// A face-up monster whose trigger is already gathered onto `new_fchain`
    /// or `new_ochain`, as a window would find it.
    fn gathered(f: &mut Field, controller: u8, mandatory: bool) -> (usize, usize, u16) {
        let mut c = Card::with_data(
            CardData {
                code: 18036057,
                type_: card_type::MONSTER | card_type::EFFECT,
                ..Default::default()
            },
            controller,
        );
        c.current.controller = controller;
        c.current.location = location::MZONE;
        c.current.position = position::FACEUP_ATTACK;
        c.set_status(status::EFFECT_ENABLED, true);
        let card = f.new_card(c);

        // `FIELD` too: the indices the window searches hold field
        // effects, not single ones.
        let ty = if mandatory {
            effect_type::TRIGGER_F
        } else {
            effect_type::TRIGGER_O
        } | effect_type::FIELD
            | effect_type::ACTIONS;
        let mut e = Effect::new(ty, code::CHAINING);
        e.owner = Some(card);
        e.handler = Some(card);
        e.effect_owner = controller;
        e.range = u16::from(location::MZONE);
        let effect = f.new_effect(e);

        let mut chain = Chain::new(effect, Event::new(code::CHAINING));
        chain.chain_id = f.next_field_id();
        chain.triggering_player = controller;
        chain.triggering_controler = controller;
        chain.triggering_location = u16::from(location::MZONE);
        chain.triggering_position = position::FACEUP_ATTACK;
        let chain_id = chain.chain_id;
        f.cards[card].create_chain_relation(effect, chain_id);
        if mandatory {
            f.core.new_fchain.push_back(chain);
        } else {
            f.core.new_ochain.push_back(chain);
        }
        (card, effect, chain_id)
    }

    fn window(f: &mut Field) {
        f.push_back(Kind::PointEvent {
            skip: PointEventSkip::default(),
        });
    }

    fn step_of(f: &Field) -> Option<u16> {
        f.queue().next().map(|u| u.step)
    }

    /// Run the unit until it is about to dispatch `target`. Counting
    /// `process` calls would be fragile, because case 1 is never dispatched:
    /// step 0 assigns 1 and the increment lands on 2.
    fn run_to(f: &mut Field, target: u16) {
        for _ in 0..32 {
            if step_of(f) == Some(target) {
                return;
            }
            f.process();
        }
        panic!("never reached step {target}, stuck at {:?}", step_of(f));
    }

    /// Step 0 moves the instant events into the window and hands the working
    /// lists over. Nothing is offered yet.
    #[test]
    fn opening_the_window_takes_the_events_and_the_lists() {
        let mut f = Field::new(8000);
        f.infos.phase = phases::MAIN1;
        gathered(&mut f, 0, true);
        f.core.instant_event.push_back(Event::new(code::CHAINING));
        window(&mut f);

        f.process();
        assert!(f.core.instant_event.is_empty());
        // Case 1 is never dispatched: step 0 assigns 1, the increment makes
        // it 2, and case 2 is what runs next.
        assert_eq!(f.core.point_event.len(), 1, "the window's events");
        assert_eq!(f.core.new_fchain_s.len(), 1, "the working copy");
        assert!(f.core.new_fchain.is_empty());
        assert_eq!(f.core.current_player, f.infos.turn_player);
        assert_eq!(step_of(&f), Some(2), "assigning 1 lands on case 2");
    }

    /// `skip_trigger` jumps the whole offer region: step 0 assigns 7, so
    /// case 8 runs next and no trigger is ever offered.
    #[test]
    fn skip_trigger_jumps_past_both_offer_loops() {
        let mut f = Field::new(8000);
        f.infos.phase = phases::MAIN1;
        gathered(&mut f, 0, true);
        f.push_back(Kind::PointEvent {
            skip: PointEventSkip {
                trigger: true,
                ..Default::default()
            },
        });
        f.process();
        assert_eq!(step_of(&f), Some(8));
        assert!(
            f.core.new_fchain_s.is_empty(),
            "the working lists were never taken"
        );
        assert_eq!(f.core.new_fchain.len(), 1, "and the trigger still waits");
    }

    /// A single mandatory trigger is taken without asking, and queues
    /// `AddChain`.
    #[test]
    fn one_mandatory_trigger_is_taken_without_asking() {
        let mut f = Field::new(8000);
        f.infos.phase = phases::MAIN1;
        let (card, effect, _) = gathered(&mut f, 0, true);
        window(&mut f);

        run_to(&mut f, 2);
        f.process(); // 2: gather and offer
        assert_eq!(f.core.select_chains.len(), 1);
        assert_eq!(
            f.core.returns.get(),
            0,
            "a single mandatory offer is automatic"
        );

        f.process(); // 3: take it
        assert_eq!(f.core.new_chains.len(), 1);
        assert!(f.core.new_fchain_s.is_empty(), "consumed");
        assert!(f.cards[card].is_status(status::CHAINING));
        assert!(f
            .core
            .subunits
            .iter()
            .any(|u| matches!(u.kind, Kind::AddChain { .. })));
        let _ = effect;
        assert_eq!(step_of(&f), Some(2), "and back round for the next");
    }

    /// Two mandatory triggers for one player is a forced *ordering* choice,
    /// not a yes/no: the player picks which goes on first.
    #[test]
    fn two_mandatory_triggers_are_a_forced_ordering_choice() {
        let mut f = Field::new(8000);
        f.infos.phase = phases::MAIN1;
        gathered(&mut f, 0, true);
        gathered(&mut f, 0, true);
        window(&mut f);
        run_to(&mut f, 2);
        f.process();

        assert_eq!(f.core.select_chains.len(), 2);
        let forced = f.core.subunits.iter().find_map(|u| match u.kind {
            Kind::SelectChain { forced, player, .. } => Some((forced, player)),
            _ => None,
        });
        assert_eq!(
            forced,
            Some((true, 0)),
            "forced: the player chooses the order, not whether"
        );
    }

    /// The turn player is asked first, and the window switches to the
    /// opponent only once the turn player has nothing left.
    #[test]
    fn the_opponent_is_asked_only_after_the_turn_player() {
        let mut f = Field::new(8000);
        f.infos.phase = phases::MAIN1;
        f.infos.turn_player = 0;
        gathered(&mut f, 1, true); // only the opponent has one
        window(&mut f);

        run_to(&mut f, 2);
        f.process(); // 2: nothing for the turn player
        assert!(f.core.select_chains.is_empty());
        assert_eq!(f.core.returns.get(), -1);

        f.process(); // 3: switch
        assert_eq!(f.core.current_player, 1, "now the opponent");
        assert_eq!(step_of(&f), Some(2));

        f.process(); // 2 again: now it is offered
        assert_eq!(f.core.select_chains.len(), 1);
    }

    /// With nothing left for either player, the mandatory loop falls
    /// through to the optional one rather than switching again.
    #[test]
    fn an_empty_mandatory_list_falls_through_to_the_optional_loop() {
        let mut f = Field::new(8000);
        f.infos.phase = phases::MAIN1;
        window(&mut f);
        run_to(&mut f, 2);
        f.process(); // 2: nothing at all
        assert_eq!(f.core.returns.get(), -1);
        f.process(); // 3: new_fchain_s is empty, so fall through
        assert_eq!(f.core.current_player, f.infos.turn_player);
        assert_eq!(step_of(&f), Some(4), "the optional loop");
    }

    /// A lone optional trigger with nothing on the chain is a yes/no
    /// question, and that is the only path reaching case 5.
    #[test]
    fn a_lone_optional_trigger_is_a_yes_no_question() {
        let mut f = Field::new(8000);
        f.infos.phase = phases::MAIN1;
        gathered(&mut f, 0, false);
        window(&mut f);
        run_to(&mut f, 4);
        f.process(); // 4
        assert!(f
            .core
            .subunits
            .iter()
            .any(|u| matches!(u.kind, Kind::SelectEffectYesNo { .. })));
        assert_eq!(step_of(&f), Some(5), "the only path that reaches case 5");
    }

    /// Case 5 turns a yes/no answer into an index, so that case 6 can read
    /// both answer shapes the same way: yes(1) becomes 0, no(0) becomes -1.
    #[test]
    fn case_five_turns_yes_no_into_an_index() {
        for (answer, expected) in [(1, 0), (0, -1)] {
            let mut f = Field::new(8000);
            f.core.returns.set(answer);
            f.point_event_step(5, PointEventSkip::default());
            assert_eq!(f.core.returns.get(), expected);
        }
    }

    /// Declining the optional offer drops it and does not re-offer it.
    #[test]
    fn declining_an_optional_trigger_drops_it() {
        let mut f = Field::new(8000);
        f.infos.phase = phases::MAIN1;
        let (_, effect, _) = gathered(&mut f, 0, false);
        window(&mut f);
        run_to(&mut f, 4);
        f.process(); // 4: the yes/no offer, leaving step 5

        // The emplaced `SelectEffectYesNo` has no handler, so the answer is
        // supplied directly and the remaining steps driven by hand.
        f.core.subunits.clear();
        f.core.returns.set(0); // "no"
        f.point_event_step(5, PointEventSkip::default());
        assert_eq!(f.core.returns.get(), -1);
        f.point_event_step(6, PointEventSkip::default());
        assert!(f.core.new_ochain_s.is_empty(), "given up");
        assert!(f.core.new_chains.is_empty(), "and nothing was chained");
        assert_eq!(
            f.effects.get(effect).unwrap().active_type,
            0,
            "the withdrawn effect stops claiming a type"
        );
    }

    /// A chain that was built is handed to `SolveChain`, and the chaining
    /// marks are given back first.
    ///
    /// The other branch — no chain built, so clear up — ends in
    /// `reset_chain`, which is not ported, so it is not driven here.
    #[test]
    fn a_built_chain_is_handed_to_solve_chain() {
        let mut f = Field::new(8000);
        f.infos.phase = phases::MAIN1;
        let (card, effect, _) = gathered(&mut f, 0, true);
        let mut chain = Chain::new(effect, Event::new(code::CHAINING));
        chain.chain_count = 1;
        f.core.current_chain.push(chain);
        f.cards[card].set_status(status::CHAINING, true);

        assert!(f.point_event_step(10, PointEventSkip::default()));
        assert!(f
            .core
            .subunits
            .iter()
            .any(|u| matches!(u.kind, Kind::SolveChain { .. })));
        assert!(
            !f.cards[card].is_status(status::CHAINING),
            "the chaining mark is given back before solving"
        );
    }

    /// Mid-chain the quick-effect window belongs to the *other* player from
    /// whoever built the last link.
    #[test]
    fn the_quick_window_alternates_with_the_chain() {
        let mut f = Field::new(8000);
        f.infos.phase = phases::MAIN1;
        f.infos.turn_player = 0;
        let (_, effect, _) = gathered(&mut f, 0, true);
        let mut chain = Chain::new(effect, Event::new(code::CHAINING));
        chain.triggering_player = 1;
        f.core.current_chain.push(chain);

        f.point_event_step(9, PointEventSkip::default());
        let player = f.core.subunits.iter().find_map(|u| match u.kind {
            Kind::QuickEffect { player, .. } => Some(player),
            _ => None,
        });
        assert_eq!(player, Some(0), "the other player from the last link");
    }

    /// With no chain, it is the turn player's window — unless the duel
    /// inverts quick priority.
    #[test]
    fn the_first_quick_window_belongs_to_the_turn_player() {
        let mut f = Field::new(8000);
        f.infos.turn_player = 0;
        f.point_event_step(9, PointEventSkip::default());
        let player = f.core.subunits.iter().find_map(|u| match u.kind {
            Kind::QuickEffect { player, .. } => Some(player),
            _ => None,
        });
        assert_eq!(player, Some(0));

        let mut f = Field::with_flags(8000, flags::INVERTED_QUICK_PRIORITY);
        f.infos.turn_player = 0;
        f.point_event_step(9, PointEventSkip::default());
        let player = f.core.subunits.iter().find_map(|u| match u.kind {
            Kind::QuickEffect { player, .. } => Some(player),
            _ => None,
        });
        assert_eq!(player, Some(1), "inverted");
    }

    /// A pending forced attack is taken when the window closes.
    #[test]
    fn a_forced_attack_is_taken_when_the_window_closes() {
        let mut f = Field::new(8000);
        f.infos.phase = phases::MAIN1;
        // A chain on the stack, so step 10 takes the solving branch rather
        // than the clean-up branch — the latter ends in `reset_chain`,
        // which is not ported.
        let (_, effect, _) = gathered(&mut f, 0, true);
        let mut chain = Chain::new(effect, Event::new(code::CHAINING));
        chain.chain_count = 1;
        f.core.current_chain.push(chain);
        f.core.set_forced_attack = true;
        f.point_event_step(10, PointEventSkip::default());
        assert!(!f.core.set_forced_attack);
        assert!(f
            .core
            .subunits
            .iter()
            .any(|u| matches!(u.kind, Kind::ForcedBattle { .. })));
    }

    mod spell_speed {
        use super::*;

        fn effect_of(f: &mut Field, ty: u16, printed: u32) -> usize {
            let mut c = Card::with_data(
                CardData {
                    code: 1,
                    type_: printed,
                    ..Default::default()
                },
                0,
            );
            c.current.location = location::SZONE;
            let card = f.new_card(c);
            let mut e = Effect::new(ty | effect_type::ACTIONS, code::FREE_CHAIN);
            e.owner = Some(card);
            e.handler = Some(card);
            f.new_effect(e)
        }

        /// An *activated* monster effect is speed 0, not 1 — a monster card
        /// has no activation speed of its own.
        #[test]
        fn speed_comes_from_the_card_for_an_activation() {
            let mut f = Field::new(8000);
            let monster = effect_of(&mut f, effect_type::ACTIVATE, card_type::MONSTER);
            assert_eq!(f.get_speed(monster), 0);

            let spell = effect_of(&mut f, effect_type::ACTIVATE, card_type::SPELL);
            assert_eq!(f.get_speed(spell), 1);

            let quick = effect_of(
                &mut f,
                effect_type::ACTIVATE,
                card_type::SPELL | card_type::QUICKPLAY,
            );
            assert_eq!(f.get_speed(quick), 2);

            let trap = effect_of(&mut f, effect_type::ACTIVATE, card_type::TRAP);
            assert_eq!(f.get_speed(trap), 2);

            let counter = effect_of(
                &mut f,
                effect_type::ACTIVATE,
                card_type::TRAP | card_type::COUNTER,
            );
            assert_eq!(f.get_speed(counter), 3);
        }

        /// And from the effect type otherwise.
        #[test]
        fn speed_comes_from_the_effect_type_otherwise() {
            let mut f = Field::new(8000);
            for (ty, expected) in [
                (effect_type::TRIGGER_O, 1),
                (effect_type::TRIGGER_F, 1),
                (effect_type::IGNITION, 1),
                (effect_type::QUICK_O, 2),
                (effect_type::QUICK_F, 2),
            ] {
                let id = effect_of(&mut f, ty, 0);
                assert_eq!(f.get_speed(id), expected, "type {ty:#x}");
            }
        }

        /// A speed-1 activation cannot be chained to anything, which is why
        /// a normal Spell cannot be chained.
        #[test]
        fn a_speed_one_activation_never_chains() {
            let mut f = Field::new(8000);
            let spell = effect_of(&mut f, effect_type::ACTIVATE, card_type::SPELL);
            assert!(!f.is_chainable(spell, 0));

            let quick = effect_of(
                &mut f,
                effect_type::ACTIVATE,
                card_type::SPELL | card_type::QUICKPLAY,
            );
            assert!(f.is_chainable(quick, 0));
        }

        /// A slower effect cannot be chained to a faster one.
        #[test]
        fn speed_must_not_decrease_up_the_chain() {
            let mut f = Field::new(8000);
            let counter = effect_of(
                &mut f,
                effect_type::ACTIVATE,
                card_type::TRAP | card_type::COUNTER,
            );
            let trigger = effect_of(&mut f, effect_type::TRIGGER_O, card_type::MONSTER);
            assert!(f.is_chainable(trigger, 0), "with nothing on the chain");

            let mut chain = Chain::new(counter, Event::new(code::FREE_CHAIN));
            chain.chain_count = 1;
            f.core.current_chain.push(chain);
            assert!(!f.is_chainable(trigger, 0), "speed 1 cannot answer speed 3");
        }

        /// The exception: an optional trigger whose handler is in the hand
        /// is compared against `> 2` rather than `<`, so it answers a
        /// speed-2 effect but not a Counter Trap.
        #[test]
        fn a_hand_trigger_is_compared_differently() {
            let mut f = Field::new(8000);
            let quick = effect_of(
                &mut f,
                effect_type::ACTIVATE,
                card_type::SPELL | card_type::QUICKPLAY,
            );
            let counter = effect_of(
                &mut f,
                effect_type::ACTIVATE,
                card_type::TRAP | card_type::COUNTER,
            );
            let hand = effect_of(&mut f, effect_type::TRIGGER_O, card_type::MONSTER);
            let handler = f.effects.get(hand).unwrap().handler.unwrap();
            f.cards[handler].current.location = location::HAND;

            let mut chain = Chain::new(quick, Event::new(code::FREE_CHAIN));
            chain.chain_count = 1;
            f.core.current_chain.push(chain);
            assert!(
                f.is_chainable(hand, 0),
                "a hand trigger answers speed 2, where a field trigger could not"
            );

            f.core.current_chain.clear();
            let mut chain = Chain::new(counter, Event::new(code::FREE_CHAIN));
            chain.chain_count = 1;
            f.core.current_chain.push(chain);
            assert!(!f.is_chainable(hand, 0), "but not speed 3");
        }
    }
}

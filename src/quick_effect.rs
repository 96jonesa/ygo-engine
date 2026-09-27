//! `QuickEffect`: the window in which a player may answer.
//!
//! A translation of `field::process(Processors::QuickEffect&)`. Four steps,
//! two of which loop, and the pair of them is what turns a chain into a
//! conversation: each player is offered a window, and declining passes it
//! back until both have declined in a row.
//!
//! ```text
//! 0 ─> 1 ──(took one, or the other player has one: RESTART)──> 0
//!      │
//!      └─> 2 ─> 3 ──(took one)──> AddChain + QuickEffect(other player)
//!                │
//!                └──(declined, other player has not)──> QuickEffect(Step 1, other)
//!                └──(both declined)──> narrow the timings and stop
//! ```
//!
//! Steps 0-1 handle **mandatory** quick effects, which are not optional and
//! so are not really a window at all; steps 2-3 are the offer proper.
//!
//! ## Declining is what ends it, and it takes two
//!
//! `infos.priorities` is a two-entry record of who has declined since the
//! last activation. Step 3 sets the declining player's entry and re-opens
//! the window for the *other* player; only when **both** entries are set
//! does the window close. Any activation clears both, because answering
//! restarts the conversation.
//!
//! The re-open enters the unit at **step 1**, not 0, so the mandatory sweep
//! is not repeated. It relies on `returns` still holding -1 from the decline
//! — which is why case 1 reads a value the step before it did not set.

use crate::board::{location, position};
use crate::card::status;
use crate::chain::Chain;
use crate::effect::{effect_type, flag};
use crate::event::{code, EffectId, Event};
use crate::field::{timing, Field};
use crate::processor::{Kind, RESTART};

impl Field {
    /// One step of `QuickEffect`. Returns true when the unit is finished.
    pub(crate) fn quick_effect_step(
        &mut self,
        step: u16,
        skip_freechain: bool,
        priority: u8,
        is_opponent: &mut bool,
    ) -> bool {
        match step {
            0 => {
                let check_player = if *is_opponent {
                    1 - self.infos.turn_player
                } else {
                    self.infos.turn_player
                };
                self.core.select_chains.clear();

                // Keys first, and a chain cloned only when it is offered: cloning
                // every stored chain (its targets and two maps) per window was
                // the largest allocation in the game.
                let mut keys = self.take_effects();
                keys.extend(self.core.quick_f_chain.keys().copied());
                for effect in keys.iter().copied() {
                    let Some((chain_id, triggering_player)) = self
                        .core
                        .quick_f_chain
                        .get(&effect)
                        .map(|c| (c.chain_id, c.triggering_player))
                    else {
                        continue;
                    };
                    let related = self
                        .effects
                        .get(effect)
                        .and_then(|e| e.get_handler(&self.cards))
                        .is_some_and(|h| self.cards[h].has_chain_relation(effect, chain_id));
                    let usable = self.is_chainable(effect, triggering_player)
                        && self.check_count_limit(effect, triggering_player)
                        && related;
                    if !usable {
                        self.core.quick_f_chain.remove(&effect);
                        continue;
                    }
                    if triggering_player == check_player {
                        let chain = self.core.quick_f_chain[&effect].clone();
                        self.core.select_chains.push_back(chain);
                    }
                }
                self.give_effects(keys);

                match self.core.select_chains.len() {
                    0 => self.core.returns.set(-1),
                    1 => self.core.returns.set(0),
                    // Forced: a mandatory quick effect is not declinable, so
                    // the choice is only which order.
                    _ => self.emplace(Kind::SelectChain {
                        player: check_player,
                        spe_count: 0,
                        forced: true,
                    }),
                }
                false
            }

            1 => {
                if self.core.returns.get() == -1 {
                    if !self.core.quick_f_chain.is_empty() {
                        // The current player has none left but somebody
                        // does: switch and sweep again. The flag only ever
                        // goes one way, which is why this terminates — the
                        // turn player's are always exhausted first.
                        *is_opponent = true;
                        self.set_step(RESTART);
                    } else if !self.core.new_chains.is_empty() {
                        let last = self.core.new_chains.back().unwrap().triggering_player;
                        self.emplace(Kind::AddChain {
                            is_activated_effect: false,
                        });
                        self.emplace(Kind::QuickEffect {
                            skip_freechain: false,
                            player: 1 - last,
                            is_opponent: false,
                        });
                        self.infos.priorities = [0, 0];
                        return true;
                    }
                    return false;
                }
                let index = self.core.returns.get() as usize;
                let Some(chain) = self.core.select_chains.remove(index) else {
                    return false;
                };
                let (effect, tp) = (chain.triggering_effect, chain.triggering_player);
                if let Some(handler) = self
                    .effects
                    .get(effect)
                    .and_then(|e| e.get_handler(&self.cards))
                {
                    self.cards[handler].set_status(status::CHAINING, true);
                }
                self.dec_count(effect, tp);
                self.core.new_chains.push_back(chain);
                self.core.quick_f_chain.remove(&effect);
                self.set_step(RESTART);
                false
            }

            2 => {
                self.gather_quick_offers(priority, skip_freechain);
                let spe_count = self.core.spe_effect[priority as usize];
                self.emplace(Kind::SelectChain {
                    player: priority,
                    spe_count,
                    forced: false,
                });
                false
            }

            3 => {
                if !self.core.select_chains.is_empty() && self.core.returns.get() != -1 {
                    let index = self.core.returns.get() as usize;
                    if let Some(chain) = self.core.select_chains.remove(index) {
                        let effect = chain.triggering_effect;
                        self.core.delayed_quick.remove(&(effect, chain.evt.clone()));
                        if let Some(handler) = self
                            .effects
                            .get(effect)
                            .and_then(|e| e.get_handler(&self.cards))
                        {
                            self.cards[handler].set_status(status::CHAINING, true);
                        }
                        self.core.new_chains.push_back(chain);
                        self.dec_count(effect, priority);
                        self.emplace(Kind::AddChain {
                            is_activated_effect: false,
                        });
                        self.emplace(Kind::QuickEffect {
                            skip_freechain: false,
                            player: 1 - priority,
                            is_opponent: false,
                        });
                        // Answering restarts the conversation: neither
                        // player has passed on the new state.
                        self.infos.priorities = [0, 0];
                    }
                } else {
                    self.infos.priorities[priority as usize] = 1;
                    if self.infos.priorities[0] == 0 || self.infos.priorities[1] == 0 {
                        // Pass it back — entering at step 1, so the
                        // mandatory sweep is not repeated. `returns` still
                        // holds -1 from this decline, which is what case 1
                        // reads.
                        self.emplace_at(
                            Kind::QuickEffect {
                                skip_freechain,
                                player: 1 - priority,
                                is_opponent: false,
                            },
                            1,
                        );
                    } else {
                        // Both passed. Only the damage-step timings survive.
                        self.core.hint_timing[0] &= timing::DAMAGE_STEP | timing::DAMAGE_CAL;
                        self.core.hint_timing[1] &= timing::DAMAGE_STEP | timing::DAMAGE_CAL;
                    }
                }
                self.core.select_chains.clear();
                true
            }

            _ => true,
        }
    }

    /// Everything a player may activate in this window, in the reference's
    /// order — which is the order they are offered in, so it is observable.
    ///
    /// Six sources, and the last two are separated by `spe_effect`: the
    /// count is taken *before* the free-chain sources are added, so the
    /// client can tell "things that answer what just happened" from "things
    /// you could do anyway". That distinction is a hint rather than a rule,
    /// but it is the reference's hint and it is cheap to keep.
    fn gather_quick_offers(&mut self, priority: u8, skip_freechain: bool) {
        if !self.core.ignition_priority_chains.is_empty() {
            std::mem::swap(
                &mut self.core.select_chains,
                &mut self.core.ignition_priority_chains,
            );
        }

        // 1 & 2. Activate and quick effects answering an event of this
        // window. `point_event` first, then `instant_event`.
        // The copies below are the borrow checker's, not the rule's:
        // `offer_if_ready` takes `&mut self`, so the lists it walks cannot be
        // borrowed from `self.core` meanwhile. They come from the scratch
        // pool and go back, so a window allocates nothing after the first.
        let mut events = self.take_events();
        events.extend(
            self.core
                .point_event
                .iter()
                .chain(self.core.instant_event.iter())
                .cloned(),
        );
        let mut ids = self.take_effects();
        for ev in &events {
            ids.clear();
            ids.extend_from_slice(self.field_effects.activate.equal_range(ev.event_code));
            ids.extend_from_slice(self.field_effects.quick_o.equal_range(ev.event_code));
            for effect in ids.iter().copied() {
                self.offer_if_ready(effect, priority, ev, OfferGate::NotDelayed);
            }
        }
        self.give_events(events);

        self.offer_hand_triggers(priority);

        let mut full = self.take_events();
        full.extend(self.core.full_event.iter().cloned());
        for ev in &full {
            ids.clear();
            ids.extend_from_slice(self.field_effects.activate.equal_range(ev.event_code));
            for effect in ids.iter().copied() {
                self.offer_if_ready(effect, priority, ev, OfferGate::Delayed);
            }
        }
        self.give_events(full);

        let mut delayed = self.take_delayed();
        delayed.extend(self.core.delayed_quick.iter().cloned());
        for (effect, ev) in &delayed {
            self.offer_if_ready(*effect, priority, ev, OfferGate::NeglectCondition);
        }
        self.give_delayed(delayed);

        // The count of offers that *answer* something, taken before the
        // free-chain sources are added.
        self.core.spe_effect[priority as usize] = self.core.select_chains.len() as u8;

        // 6. Free-chain activations: what a player could do anyway.
        if !skip_freechain {
            let nil = Event::new(code::FREE_CHAIN);
            ids.clear();
            ids.extend_from_slice(self.field_effects.activate.equal_range(code::FREE_CHAIN));
            for effect in ids.iter().copied() {
                if self.offer_if_ready(effect, priority, &nil, OfferGate::Plain)
                    && (self.check_hint_timing(effect)
                        || self.check_cteffect_hint(effect, priority))
                {
                    self.core.spe_effect[priority as usize] += 1;
                }
            }
            ids.clear();
            ids.extend_from_slice(self.field_effects.quick_o.equal_range(code::FREE_CHAIN));
            for effect in ids.iter().copied() {
                if self.offer_if_ready(effect, priority, &nil, OfferGate::Plain)
                    && self.check_hint_timing(effect)
                {
                    self.core.spe_effect[priority as usize] += 1;
                }
            }
        }
        self.give_effects(ids);

        // Mid-chain, or while an attack is pending, the distinction is
        // dropped and everything counts as answering.
        if !self.core.current_chain.is_empty()
            || self.core.hint_timing[0] & timing::ATTACK != 0
            || self.core.hint_timing[1] & timing::ATTACK != 0
        {
            self.core.spe_effect[priority as usize] = self.core.select_chains.len() as u8;
        }
    }

    /// Offer one effect against one event, if it passes its gate. Returns
    /// whether it was offered.
    fn offer_if_ready(
        &mut self,
        effect: EffectId,
        priority: u8,
        ev: &Event,
        gate: OfferGate,
    ) -> bool {
        let Some(handler) = self
            .effects
            .get(effect)
            .and_then(|e| e.get_handler(&self.cards))
        else {
            return false;
        };
        // Every source sets the activation location first, before any test.
        let snapshot = self.cards[handler].state();
        if let Some(e) = self.effects.get_mut(effect) {
            e.set_activate_location(&snapshot);
        }
        let delayed = self
            .effects
            .get(effect)
            .is_some_and(|e| e.is_flag(flag::DELAY));
        let gate_ok = match gate {
            OfferGate::NotDelayed => !delayed,
            OfferGate::Delayed => delayed,
            OfferGate::Plain | OfferGate::NeglectCondition => true,
        };
        if !gate_ok {
            return false;
        }
        let neglect_cond = matches!(gate, OfferGate::NeglectCondition);
        if !(self.is_chainable(effect, priority)
            && self.is_activateable(
                effect,
                priority,
                ev,
                neglect_cond,
                false,
                false,
                false,
                false,
            ))
        {
            return false;
        }

        let mut chain = Chain::new(effect, ev.clone());
        chain.chain_id = self.next_field_id();
        chain.set_triggering_state(&snapshot);
        chain.triggering_player = priority;
        self.core.select_chains.push_back(chain);
        true
    }

    /// The hand and deck triggers `PointEvent` filed rather than offered.
    ///
    /// They are offered *here* rather than there because a trigger from a
    /// hidden zone is answered in the quick window, not in the trigger
    /// window — which is the mechanism behind a hand trap.
    fn offer_hand_triggers(&mut self, priority: u8) {
        let held: Vec<Chain> = self.core.new_ochain_h.iter().cloned().collect();
        let mut refreshed = Vec::with_capacity(held.len());
        for mut chain in held {
            let effect = chain.triggering_effect;
            let Some(handler) = self
                .effects
                .get(effect)
                .and_then(|e| e.get_handler(&self.cards))
            else {
                refreshed.push(chain);
                continue;
            };

            // The same refresh `PointEvent`'s optional loop does: a field
            // effect ranged to the hand, whose card is in the hand, gets its
            // relation now if its condition holds.
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
                chain.triggering_player = controller;
                chain.set_triggering_state(&snapshot);
            }

            // Offered only from a hidden position: face-down in the hand, or
            // in the deck. A hand trigger whose card is face-up was already
            // offered in the trigger window.
            let hidden = (chain.triggering_location == u16::from(location::HAND)
                && chain.triggering_position & position::FACEDOWN != 0)
                || chain.triggering_location == u16::from(location::DECK);
            let offerable = chain.triggering_player == priority
                && !self.cards[handler].is_status(status::CHAINING)
                && hidden
                && self.cards[handler].has_chain_relation(effect, chain.chain_id)
                && self.is_chainable(effect, priority)
                && self.is_activateable(
                    effect,
                    priority,
                    &chain.evt.clone(),
                    true,
                    false,
                    false,
                    false,
                    false,
                )
                && self.check_spself_from_hand_trigger(&chain);
            if offerable {
                self.core.select_chains.push_back(chain.clone());
            }
            refreshed.push(chain);
        }
        self.core.new_ochain_h = refreshed.into();
    }

    /// `check_hint_timing` — is this effect's declared timing one that is
    /// live right now?
    ///
    /// The effect's two entries are `[own, opponent's]` while the field's
    /// are by absolute player, so for player 1 the two cross. Indexing both
    /// the same way silently mislabels every hint for the second player.
    pub fn check_hint_timing(&self, effect: EffectId) -> bool {
        let Some(e) = self.effects.get(effect) else {
            return false;
        };
        let p = e.get_handler_player(&self.cards);
        let (own, other) = if p == 0 {
            (self.core.hint_timing[0], self.core.hint_timing[1])
        } else {
            (self.core.hint_timing[1], self.core.hint_timing[0])
        };
        e.hint_timing[0] & own != 0 || e.hint_timing[1] & other != 0
    }

    /// `check_cteffect_hint` — would activating this continuous trap let a
    /// field effect of its own be used?
    ///
    /// `get_cteffect`'s question asked for the client's benefit, with one
    /// extra condition: a free-chain field effect only counts if its own
    /// timing is live.
    pub fn check_cteffect_hint(&mut self, effect: EffectId, playerid: u8) -> bool {
        let Some(candidates) = self.cteffect_candidates_pub(effect) else {
            return false;
        };
        // `EVENT_PHASE | phase` here where `get_cteffect` writes
        // `EVENT_PHASE + phase`. The phase values are all below 0x1000 so
        // the two agree; kept as the reference writes each.
        let phase_code = code::PHASE | u32::from(self.infos.phase);
        for (event_code, fid) in candidates {
            if event_code == code::FREE_CHAIN || event_code == phase_code {
                let nil = Event::new(event_code);
                if self.is_activateable(fid, playerid, &nil, false, false, false, false, true)
                    && (event_code != code::FREE_CHAIN || self.check_hint_timing(fid))
                {
                    return true;
                }
            } else {
                let events: Vec<Event> = self
                    .core
                    .point_event
                    .iter()
                    .chain(self.core.instant_event.iter())
                    .filter(|ev| ev.event_code == event_code)
                    .cloned()
                    .collect();
                for ev in events {
                    if self.is_activateable(fid, playerid, &ev, false, false, false, false, true) {
                        return true;
                    }
                }
            }
        }
        false
    }
}

/// Which gate an offer source applies to `EFFECT_FLAG_DELAY`, and whether it
/// re-asks the effect's condition.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum OfferGate {
    /// Answering an event now: delayed effects are excluded.
    NotDelayed,
    /// The delayed-activation pass: only delayed effects.
    Delayed,
    /// No delay test at all.
    Plain,
    /// No delay test, and the condition was already checked.
    NeglectCondition,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{card_type, Card, CardData};
    use crate::duel::phases;
    use crate::effect::Effect;
    use crate::field::Field;

    /// A face-up monster carrying one quick effect.
    fn quick_on_field(f: &mut Field, controller: u8, ty: u16, ev_code: u32) -> (usize, usize) {
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

        let mut e = Effect::new(ty | effect_type::FIELD | effect_type::ACTIONS, ev_code);
        e.owner = Some(card);
        e.handler = Some(card);
        e.effect_owner = controller;
        e.range = u16::from(location::MZONE);
        let effect = f.new_effect(e);
        f.register_effect(effect);
        (card, effect)
    }

    fn mandatory_quick(f: &mut Field, controller: u8) -> (usize, usize) {
        let (card, effect) = quick_on_field(f, controller, effect_type::QUICK_F, code::CHAINING);
        let mut chain = Chain::new(effect, Event::new(code::CHAINING));
        chain.chain_id = f.next_field_id();
        chain.triggering_player = controller;
        f.cards[card].create_chain_relation(effect, chain.chain_id);
        f.core.quick_f_chain.insert(effect, chain);
        (card, effect)
    }

    /// Declining takes two: the window closes only once both players have
    /// passed in a row.
    mod priority {
        use super::*;

        #[test]
        fn one_decline_passes_the_window_to_the_other_player() {
            let mut f = Field::new(8000);
            f.infos.phase = phases::MAIN1;
            f.core.returns.set(-1);
            let mut is_opponent = false;

            let done = f.quick_effect_step(3, false, 0, &mut is_opponent);
            assert!(done);
            assert_eq!(f.infos.priorities, [1, 0], "player 0 has passed");

            let handed = f.core.subunits.iter().find_map(|u| match u.kind {
                Kind::QuickEffect { player, .. } => Some((player, u.step)),
                _ => None,
            });
            assert_eq!(
                handed,
                Some((1, 1)),
                "passed to player 1, entering at step 1 so the mandatory \
                 sweep is not repeated"
            );
        }

        #[test]
        fn the_second_decline_closes_the_window() {
            let mut f = Field::new(8000);
            f.infos.phase = phases::MAIN1;
            f.infos.priorities = [1, 0];
            f.core.hint_timing = [0xffff_ffff, 0xffff_ffff];
            f.core.returns.set(-1);
            let mut is_opponent = false;

            f.quick_effect_step(3, false, 1, &mut is_opponent);
            assert_eq!(f.infos.priorities, [1, 1]);
            assert!(
                !f.core
                    .subunits
                    .iter()
                    .any(|u| matches!(u.kind, Kind::QuickEffect { .. })),
                "nobody is asked again"
            );
            assert_eq!(
                f.core.hint_timing[0],
                timing::DAMAGE_STEP | timing::DAMAGE_CAL,
                "only the damage-step timings survive"
            );
        }

        /// Activating clears both entries: answering restarts the
        /// conversation, so a player who passed earlier gets asked again.
        #[test]
        fn activating_restarts_the_conversation() {
            let mut f = Field::new(8000);
            f.infos.phase = phases::MAIN1;
            f.infos.priorities = [1, 0];
            let (card, effect) = quick_on_field(&mut f, 0, effect_type::QUICK_O, code::FREE_CHAIN);
            let mut chain = Chain::new(effect, Event::new(code::FREE_CHAIN));
            chain.chain_id = f.next_field_id();
            chain.triggering_player = 0;
            f.core.select_chains.push_back(chain);
            f.core.returns.set(0);

            let mut is_opponent = false;
            f.quick_effect_step(3, false, 0, &mut is_opponent);

            assert_eq!(f.infos.priorities, [0, 0], "both asked again");
            assert_eq!(f.core.new_chains.len(), 1);
            assert!(f.cards[card].is_status(status::CHAINING));
            assert!(f
                .core
                .subunits
                .iter()
                .any(|u| matches!(u.kind, Kind::AddChain { .. })));
            let next = f.core.subunits.iter().find_map(|u| match u.kind {
                Kind::QuickEffect { player, .. } => Some(player),
                _ => None,
            });
            assert_eq!(next, Some(1), "and the other player answers next");
        }
    }

    mod mandatory_sweep {
        use super::*;

        /// A single mandatory quick effect is taken without asking.
        #[test]
        fn one_mandatory_quick_effect_is_automatic() {
            let mut f = Field::new(8000);
            f.infos.phase = phases::MAIN1;
            mandatory_quick(&mut f, 0);
            let mut is_opponent = false;

            f.quick_effect_step(0, false, 0, &mut is_opponent);
            assert_eq!(f.core.select_chains.len(), 1);
            assert_eq!(f.core.returns.get(), 0, "no question asked");
        }

        /// Two is a forced ordering choice, as in the trigger window.
        #[test]
        fn two_mandatory_quick_effects_are_a_forced_choice() {
            let mut f = Field::new(8000);
            f.infos.phase = phases::MAIN1;
            mandatory_quick(&mut f, 0);
            mandatory_quick(&mut f, 0);
            let mut is_opponent = false;

            f.quick_effect_step(0, false, 0, &mut is_opponent);
            let prompt = f.core.subunits.iter().find_map(|u| match u.kind {
                Kind::SelectChain { forced, player, .. } => Some((forced, player)),
                _ => None,
            });
            assert_eq!(prompt, Some((true, 0)));
        }

        /// Only the current side's are offered; the other player's stay in
        /// the map for the second sweep.
        #[test]
        fn only_the_current_sides_are_offered() {
            let mut f = Field::new(8000);
            f.infos.phase = phases::MAIN1;
            f.infos.turn_player = 0;
            mandatory_quick(&mut f, 1);
            let mut is_opponent = false;

            f.quick_effect_step(0, false, 0, &mut is_opponent);
            assert!(f.core.select_chains.is_empty(), "not the turn player's");
            assert_eq!(f.core.quick_f_chain.len(), 1, "and still waiting");
            assert_eq!(f.core.returns.get(), -1);

            f.quick_effect_step(1, false, 0, &mut is_opponent);
            assert!(is_opponent, "the sweep switches sides");
        }

        /// An entry whose card lost its relation is dropped rather than
        /// offered — the card it was on is gone or changed.
        #[test]
        fn an_unrelated_entry_is_dropped() {
            let mut f = Field::new(8000);
            f.infos.phase = phases::MAIN1;
            let (card, _) = mandatory_quick(&mut f, 0);
            f.cards[card].clear_relate_effect();
            let mut is_opponent = false;

            f.quick_effect_step(0, false, 0, &mut is_opponent);
            assert!(f.core.quick_f_chain.is_empty(), "dropped");
            assert!(f.core.select_chains.is_empty());
        }

        /// With the map empty and a chain waiting, the sweep hands over to
        /// `AddChain` and the other player's window.
        #[test]
        fn an_exhausted_sweep_hands_over() {
            let mut f = Field::new(8000);
            f.infos.phase = phases::MAIN1;
            let (_, effect) = quick_on_field(&mut f, 0, effect_type::QUICK_F, code::CHAINING);
            let mut chain = Chain::new(effect, Event::new(code::CHAINING));
            chain.triggering_player = 1;
            f.core.new_chains.push_back(chain);
            f.core.returns.set(-1);
            f.infos.priorities = [1, 1];

            let mut is_opponent = false;
            assert!(f.quick_effect_step(1, false, 0, &mut is_opponent));
            assert_eq!(f.infos.priorities, [0, 0]);
            let next = f.core.subunits.iter().find_map(|u| match u.kind {
                Kind::QuickEffect { player, .. } => Some(player),
                _ => None,
            });
            assert_eq!(next, Some(0), "the other player from the chain's owner");
        }
    }

    mod offers {
        use super::*;

        /// The free-chain sources are counted separately: `spe_effect` is
        /// taken before they are added, so the client can tell "answers
        /// what just happened" from "could do anyway".
        #[test]
        fn free_chain_offers_are_counted_separately() {
            let mut f = Field::new(8000);
            f.infos.phase = phases::MAIN1;
            // One answering the event, one available anyway.
            quick_on_field(&mut f, 0, effect_type::QUICK_O, code::CHAINING);
            quick_on_field(&mut f, 0, effect_type::QUICK_O, code::FREE_CHAIN);
            f.core.point_event.push_back(Event::new(code::CHAINING));

            f.gather_quick_offers(0, false);
            assert_eq!(f.core.select_chains.len(), 2, "both are offered");
            assert_eq!(f.core.spe_effect[0], 1, "but only one answers the event");
        }

        /// `skip_freechain` leaves out what a player could do anyway.
        #[test]
        fn skip_freechain_omits_the_anyway_offers() {
            let mut f = Field::new(8000);
            f.infos.phase = phases::MAIN1;
            quick_on_field(&mut f, 0, effect_type::QUICK_O, code::FREE_CHAIN);

            f.gather_quick_offers(0, true);
            assert!(f.core.select_chains.is_empty());

            let mut f = Field::new(8000);
            f.infos.phase = phases::MAIN1;
            quick_on_field(&mut f, 0, effect_type::QUICK_O, code::FREE_CHAIN);
            f.gather_quick_offers(0, false);
            assert_eq!(f.core.select_chains.len(), 1);
        }

        /// Mid-chain the distinction is dropped: everything counts as
        /// answering.
        #[test]
        fn mid_chain_everything_counts_as_answering() {
            let mut f = Field::new(8000);
            f.infos.phase = phases::MAIN1;
            let (_, on_chain) = quick_on_field(&mut f, 0, effect_type::QUICK_O, code::CHAINING);
            quick_on_field(&mut f, 0, effect_type::QUICK_O, code::FREE_CHAIN);
            let mut chain = Chain::new(on_chain, Event::new(code::CHAINING));
            chain.chain_count = 1;
            f.core.current_chain.push(chain);

            f.gather_quick_offers(0, false);
            assert_eq!(
                f.core.spe_effect[0] as usize,
                f.core.select_chains.len(),
                "with a chain running, every offer counts"
            );
        }

        /// A set quick-play spell: an activate effect that is fast enough
        /// to be chained. A monster's activate effect is speed 0 and would
        /// be refused by `is_chainable` before the delay test was reached.
        fn set_quickplay(f: &mut Field, controller: u8, ev_code: u32) -> usize {
            let mut c = Card::with_data(
                CardData {
                    code: 14087893,
                    type_: card_type::SPELL | card_type::QUICKPLAY,
                    ..Default::default()
                },
                controller,
            );
            c.current.controller = controller;
            c.current.location = location::SZONE;
            c.current.position = position::FACEDOWN_DEFENSE;
            let card = f.new_card(c);
            let mut e = Effect::new(effect_type::ACTIVATE | effect_type::ACTIONS, ev_code);
            e.owner = Some(card);
            e.handler = Some(card);
            e.effect_owner = controller;
            let effect = f.new_effect(e);
            f.register_effect(effect);
            effect
        }

        /// A delayed effect is not offered against the event it responds to;
        /// it waits for the delayed pass.
        #[test]
        fn a_delayed_effect_waits_for_its_own_pass() {
            let mut f = Field::new(8000);
            f.infos.phase = phases::MAIN1;
            let effect = set_quickplay(&mut f, 0, code::CHAINING);
            f.effects.get_mut(effect).unwrap().flag[0] |= flag::DELAY;
            f.core.point_event.push_back(Event::new(code::CHAINING));

            f.gather_quick_offers(0, true);
            assert!(
                f.core.select_chains.is_empty(),
                "not offered against a point event"
            );

            f.core.select_chains.clear();
            f.core.full_event.push_back(Event::new(code::CHAINING));
            f.gather_quick_offers(0, true);
            assert_eq!(
                f.core.select_chains.len(),
                1,
                "but offered in the delayed pass"
            );
        }

        /// The ignition-priority chains are taken over wholesale when there
        /// are any.
        #[test]
        fn ignition_priority_chains_are_taken_over() {
            let mut f = Field::new(8000);
            f.infos.phase = phases::MAIN1;
            let (_, effect) = quick_on_field(&mut f, 0, effect_type::IGNITION, code::FREE_CHAIN);
            let mut chain = Chain::new(effect, Event::new(code::FREE_CHAIN));
            chain.triggering_player = 0;
            f.core.ignition_priority_chains.push_back(chain);

            f.gather_quick_offers(0, true);
            assert_eq!(f.core.select_chains.len(), 1);
            assert!(f.core.ignition_priority_chains.is_empty(), "swapped out");
        }
    }

    /// The effect's hint entries are `[own, opponent's]` while the field's
    /// are by absolute player, so for player 1 the two cross. Indexing both
    /// the same way silently mislabels every hint for the second player.
    #[test]
    fn hint_timings_cross_for_the_second_player() {
        let mut f = Field::new(8000);
        let (_, effect) = quick_on_field(&mut f, 1, effect_type::QUICK_O, code::FREE_CHAIN);
        f.effects.get_mut(effect).unwrap().hint_timing = [timing::ATTACK, 0];

        // The effect's *own* timing must be matched against its controller's
        // half of the field's, which for player 1 is index 1.
        f.core.hint_timing = [0, timing::ATTACK];
        assert!(f.check_hint_timing(effect));

        f.core.hint_timing = [timing::ATTACK, 0];
        assert!(
            !f.check_hint_timing(effect),
            "the opponent's half does not answer the effect's own entry"
        );
    }
}

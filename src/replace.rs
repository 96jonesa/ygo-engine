//! Replacement effects: "if this card would be destroyed, instead…".
//!
//! `field::operation_replace` and the `OperationReplace` unit. Shared by
//! `Destroy`, `Release` and `SendTo` — each asks the same question of the
//! same machinery, with a different `EFFECT_*_REPLACE` code.
//!
//! ## The turn player is asked first
//!
//! `operation_replace` walks the registered effects once, emplacing the turn
//! player's immediately and collecting the opponent's into a second list
//! that is emplaced after. Since emplaced units are spliced to the front of
//! the queue in order, that puts the turn player's replacements ahead of the
//! opponent's — which is the rule, expressed as two loops rather than a
//! sort.
//!
//! ## Five entry points, and two of them are a different rule
//!
//! | entry | shape | operations run |
//! |---|---|---|
//! | 0 | one card | **immediately** (step 2) |
//! | 5 | the whole group | **immediately** (step 7) |
//! | 10 | one card | **deferred** to step 15 |
//! | 12 | the whole group | **deferred** to step 15 |
//! | 15 | run the deferred operations, one at a time | |
//!
//! The deferred pair is `Destroy`'s, and the deferral is the point:
//! `Destroy` asks *every* replacement effect whether it applies before
//! running any of their operations, so one replacement cannot change what
//! another sees. `SendTo` and `Release` have no such need and use the
//! immediate forms.
//!
//! Step 16 loops back by assigning `step = 14`, so the increment runs 15
//! again — draining `desrep_chain` one entry per pass.
//!
//! ## A group is shared, mutable state
//!
//! The reference passes a `group*` that the unit **erases from**, and the
//! caller reads afterwards to see what survived. That is not a value: it is
//! a handle. Here it is a `GroupId` into an arena on the field, for the same
//! reason `CardId` is — a port that passed a `Vec` by value would have each
//! replacement effect cancel a move in its own private copy.

use crate::card::status;
use crate::event::{CardId, EffectId, Event, PLAYER_NONE};
use crate::field::{Field, GroupId};
use crate::processor::Kind;

impl Field {
    /// `field::operation_replace` — offer every registered replacement
    /// effect a chance at this group, turn player first.
    pub fn operation_replace(&mut self, code_: u32, step: u16, targets: GroupId) {
        let is_destroy = code_ == crate::event::code::DESTROY_REPLACE;
        let turn_player = self.infos.turn_player;
        let registered = self.field_effects.continuous.equal_range(code_).to_vec();

        let mut opponents = Vec::new();
        for effect in registered {
            let owner = self
                .effects
                .get(effect)
                .map_or(PLAYER_NONE, |e| e.get_handler_player(&self.cards));
            if owner == turn_player {
                self.emplace_at(
                    Kind::OperationReplace {
                        replace_effect: effect,
                        targets,
                        target: None,
                        is_destroy,
                    },
                    step,
                );
            } else {
                opponents.push(effect);
            }
        }
        for effect in opponents {
            self.emplace_at(
                Kind::OperationReplace {
                    replace_effect: effect,
                    targets,
                    target: None,
                    is_destroy,
                },
                step,
            );
        }
    }

    /// One step of `OperationReplace`.
    pub(crate) fn operation_replace_step(
        &mut self,
        step: u16,
        replace_effect: EffectId,
        targets: GroupId,
        target: Option<CardId>,
        is_destroy: bool,
    ) -> bool {
        match step {
            // ---- one card, operation run immediately ----
            0 => {
                // A previous replacement already claimed this card.
                if self.core.returns.get() != 0 {
                    return true;
                }
                let Some(target) = target else {
                    return true;
                };
                self.begin_replacement(replace_effect, targets, Some(target), false)
            }
            1 => {
                if self.core.returns.get() != 0 {
                    self.cancel_move(replace_effect, targets, target, is_destroy, true);
                } else {
                    self.set_step(2);
                }
                false
            }
            2 => self.run_replacement_operation(replace_effect),
            3 => {
                self.core.continuous_chain.pop_back();
                self.core.solving_event.pop_front();
                true
            }

            // ---- the whole group, operation run immediately ----
            5 => {
                if self.group(targets).is_empty() {
                    return true;
                }
                self.begin_replacement(replace_effect, targets, None, true)
            }
            6 => {
                if self.core.returns.get() != 0 {
                    self.cancel_group_moves(replace_effect, targets, is_destroy, true);
                } else {
                    self.set_step(7);
                }
                false
            }
            7 => self.run_replacement_operation(replace_effect),
            8 => {
                self.core.continuous_chain.pop_back();
                self.core.solving_event.pop_front();
                true
            }

            // ---- one card, operation deferred ----
            10 => {
                if self.core.returns.get() != 0 {
                    return true;
                }
                let Some(target) = target else {
                    return true;
                };
                // Note: no `solving_event.push_front` here. The deferred
                // forms carry the event on the chain entry instead, because
                // the operation runs long after this step has returned.
                self.begin_replacement_deferred(replace_effect, targets, Some(target), false)
            }
            11 => {
                if self.core.returns.get() != 0 {
                    self.cancel_move(replace_effect, targets, target, is_destroy, false);
                    if let Some(front) = self.core.continuous_chain.front().cloned() {
                        self.core.desrep_chain.push_back(front);
                    }
                }
                self.core.continuous_chain.pop_front();
                true
            }

            // ---- the whole group, operation deferred ----
            12 => {
                if self.group(targets).is_empty() {
                    return true;
                }
                self.begin_replacement_deferred(replace_effect, targets, None, true)
            }
            13 => {
                if self.core.returns.get() != 0 {
                    // Note the difference from step 6: no self-destroy
                    // exemption here. A card destroying itself can still
                    // have that destruction replaced in this pass.
                    self.cancel_group_moves(replace_effect, targets, is_destroy, false);
                    if let Some(front) = self.core.continuous_chain.front().cloned() {
                        self.core.desrep_chain.push_back(front);
                    }
                }
                self.core.continuous_chain.pop_front();
                true
            }

            // ---- run the deferred operations, one per pass ----
            15 => {
                let Some(chain) = self.core.desrep_chain.pop_front() else {
                    return true;
                };
                let effect = chain.triggering_effect;
                let evt = chain.evt.clone();
                self.core.continuous_chain.push_back(chain);
                if self
                    .effects
                    .get(effect)
                    .is_some_and(|e| e.operation.is_some())
                {
                    self.core.sub_solving_event.push_back(evt);
                    let player = self
                        .effects
                        .get(effect)
                        .map_or(PLAYER_NONE, |e| e.get_handler_player(&self.cards));
                    self.emplace(Kind::ExecuteOperation {
                        resume: None,
                        effect,
                        player,
                        subject: None,
                        args: Vec::new(),
                        was_disabled: false,
                    });
                }
                false
            }
            16 => {
                self.core.continuous_chain.pop_back();
                // 14 + 1 = 15: round again for the next deferred operation.
                self.set_step(14);
                false
            }

            _ => true,
        }
    }

    /// The shared opening of steps 0 and 5: build the event, check the
    /// effect can activate, put it on the continuous chain, and ask its
    /// target.
    ///
    /// `group_form` additionally requires the effect to have a **value** —
    /// the per-card filter that decides which of the group it covers. An
    /// effect without one cannot answer "which", so the group form declines.
    fn begin_replacement(
        &mut self,
        replace_effect: EffectId,
        targets: GroupId,
        target: Option<CardId>,
        group_form: bool,
    ) -> bool {
        let Some(evt) = self.replacement_event(replace_effect, targets, target, group_form) else {
            return true;
        };
        self.push_continuous_chain(replace_effect, &evt);
        self.core.solving_event.push_front(evt.clone());
        self.core.sub_solving_event.push_back(evt);
        self.ask_replacement_target(replace_effect);
        false
    }

    /// The same, for the deferred forms — which do **not** push onto
    /// `solving_event`, because their operation runs from the chain entry
    /// later rather than from the front of that list now.
    fn begin_replacement_deferred(
        &mut self,
        replace_effect: EffectId,
        targets: GroupId,
        target: Option<CardId>,
        group_form: bool,
    ) -> bool {
        // The single-card deferred form requires a target function where the
        // immediate one requires it too; the group forms require a value.
        if !group_form
            && self
                .effects
                .get(replace_effect)
                .is_some_and(|e| e.target.is_none())
        {
            return true;
        }
        let Some(evt) = self.replacement_event(replace_effect, targets, target, group_form) else {
            return true;
        };
        self.push_continuous_chain(replace_effect, &evt);
        self.core.sub_solving_event.push_back(evt);
        self.ask_replacement_target(replace_effect);
        false
    }

    /// Build the event a replacement effect is asked about, and return
    /// `None` if the effect cannot act on it.
    ///
    /// The reason fields come from the *card being moved*, not from the
    /// replacement effect — the question being asked is "this is happening
    /// for that reason; do you want to stop it?".
    fn replacement_event(
        &mut self,
        replace_effect: EffectId,
        targets: GroupId,
        target: Option<CardId>,
        group_form: bool,
    ) -> Option<Event> {
        let source = match target {
            Some(c) => c,
            None => *self.group(targets).iter().next()?,
        };
        if !group_form
            && self
                .effects
                .get(replace_effect)
                .is_some_and(|e| e.target.is_none())
        {
            return None;
        }
        let player = self
            .effects
            .get(replace_effect)
            .map_or(PLAYER_NONE, |e| e.get_handler_player(&self.cards));

        let mut evt = Event::new(0);
        evt.event_cards = self.group(targets).iter().copied().collect();
        evt.event_player = player;
        evt.event_value = 0;
        evt.reason = self.cards[source].reason;
        evt.reason_effect = self.cards[source].reason_effect;
        evt.reason_player = self.cards[source].reason_player;

        if !self.is_activateable(
            replace_effect,
            player,
            &evt,
            false,
            false,
            false,
            false,
            false,
        ) {
            return None;
        }
        // The group forms also need a value: it is the per-card filter that
        // says which of the group the effect covers.
        if group_form
            && self
                .effects
                .get(replace_effect)
                .is_some_and(|e| e.value == 0 && e.value_fn.is_none())
        {
            return None;
        }
        Some(evt)
    }

    fn push_continuous_chain(&mut self, replace_effect: EffectId, evt: &Event) {
        let player = self
            .effects
            .get(replace_effect)
            .map_or(PLAYER_NONE, |e| e.get_handler_player(&self.cards));
        let mut chain = crate::chain::Chain::new(replace_effect, evt.clone());
        chain.chain_id = 0;
        chain.chain_count = 0;
        chain.triggering_player = player;
        self.core.continuous_chain.push_back(chain);
    }

    fn ask_replacement_target(&mut self, replace_effect: EffectId) {
        let player = self
            .effects
            .get(replace_effect)
            .map_or(PLAYER_NONE, |e| e.get_handler_player(&self.cards));
        self.emplace(Kind::ExecuteTarget {
            resume: None,
            effect: replace_effect,
            player,
            subject: None,
            args: Vec::new(),
            was_disabled: false,
        });
    }

    fn run_replacement_operation(&mut self, replace_effect: EffectId) -> bool {
        if self
            .effects
            .get(replace_effect)
            .is_some_and(|e| e.operation.is_none())
        {
            return false;
        }
        if let Some(evt) = self.core.solving_event.front().cloned() {
            self.core.sub_solving_event.push_back(evt);
        }
        let player = self
            .effects
            .get(replace_effect)
            .map_or(PLAYER_NONE, |e| e.get_handler_player(&self.cards));
        self.emplace(Kind::ExecuteOperation {
            resume: None,
            effect: replace_effect,
            player,
            subject: None,
            args: Vec::new(),
            was_disabled: false,
        });
        false
    }

    /// One card is spared: taken out of the group, its reason restored from
    /// the copy made when the move was set up.
    ///
    /// `exempt_self_destroy` is the step 1/6 behaviour: a card being
    /// destroyed *by its own effect* is not spared, because a card cannot
    /// replace its own self-destruction. The deferred forms (11, 13) have no
    /// such exemption.
    fn cancel_move(
        &mut self,
        replace_effect: EffectId,
        targets: GroupId,
        target: Option<CardId>,
        is_destroy: bool,
        exempt_self_destroy: bool,
    ) {
        let Some(target) = target else {
            return;
        };
        let self_destroying = exempt_self_destroy && self.is_self_destroy_reason(target);
        if !self_destroying {
            self.group_mut(targets).remove(&target);
            self.restore_reason(target);
            if is_destroy {
                self.core.destroy_canceled.insert(target);
            }
        }
        let player = self
            .effects
            .get(replace_effect)
            .map_or(PLAYER_NONE, |e| e.get_handler_player(&self.cards));
        self.dec_count(replace_effect, player);
    }

    /// The group form: every card the effect's value covers is spared.
    fn cancel_group_moves(
        &mut self,
        replace_effect: EffectId,
        targets: GroupId,
        is_destroy: bool,
        exempt_self_destroy: bool,
    ) {
        for card in self.group(targets).iter().copied().collect::<Vec<_>>() {
            let covered = self.effect_value_for_card_pub(replace_effect, card) != 0;
            if !covered {
                continue;
            }
            if exempt_self_destroy && self.is_self_destroy_reason(card) {
                continue;
            }
            self.restore_reason(card);
            if is_destroy {
                self.core.destroy_canceled.insert(card);
            }
            self.group_mut(targets).remove(&card);
        }
        let player = self
            .effects
            .get(replace_effect)
            .map_or(PLAYER_NONE, |e| e.get_handler_player(&self.cards));
        self.dec_count(replace_effect, player);
    }

    /// `effect::is_self_destroy_related`, asked of the reason behind a move.
    fn is_self_destroy_reason(&self, card: CardId) -> bool {
        use crate::event::code;
        self.cards[card]
            .reason_effect
            .and_then(|e| self.effects.get(e))
            .is_some_and(|e| {
                matches!(
                    e.code,
                    code::UNIQUE_CHECK | code::SELF_DESTROY | code::SELF_TOGRAVE
                )
            })
    }

    /// Put back the reason a card had before this operation touched it.
    /// Shared with `Destroy` and `Release`, which drop cards for their own
    /// reasons and must leave them exactly as they were found.
    pub(crate) fn restore_reason(&mut self, card: CardId) {
        let c = &mut self.cards[card];
        c.reason = c.temp.reason;
        c.reason_effect = c.temp.reason_effect;
        c.reason_player = c.temp.reason_player;
    }

    /// Whether a card survived a replacement pass — what a caller reads
    /// after the units have run.
    pub fn destruction_was_cancelled(&self, card: CardId) -> bool {
        self.core.destroy_canceled.contains(&card)
    }

    /// A card marked as leaving that a replacement then spared keeps the
    /// mark unless something clears it; the reference re-checks rather than
    /// trusting, and so does `SolveChain` step 11.
    pub fn still_leaving(&self, card: CardId) -> bool {
        self.cards[card].is_status(status::LEAVE_CONFIRMED)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::{location, position};
    use crate::card::{card_type, reason, Card, CardData};
    use crate::effect::{effect_type, Effect};
    use crate::event::code;

    fn on_field(f: &mut Field, controller: u8) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 18036057,
                type_: card_type::MONSTER | card_type::EFFECT,
                ..Default::default()
            },
            controller,
        );
        c.current.controller = controller;
        c.current.position = position::FACEUP_ATTACK;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        let seat = f.players[controller as usize]
            .mzone
            .iter()
            .position(Option::is_none)
            .expect("a free seat") as u32;
        f.add_card(controller, id, location::MZONE, seat, false);
        id
    }

    /// A registered replacement effect belonging to `controller`.
    fn replacement(f: &mut Field, controller: u8, code_: u32) -> EffectId {
        let source = on_field(f, controller);
        let mut e = Effect::new(effect_type::CONTINUOUS | effect_type::ACTIONS, code_);
        e.owner = Some(source);
        e.handler = Some(source);
        e.effect_owner = controller;
        e.range = u16::from(location::MZONE);
        let id = f.new_effect(e);
        f.field_effects.continuous.insert(code_, id);
        f.field_effects.indexer.insert(id);
        id
    }

    /// A card set up to be moved, with its reason saved as `send_to` would.
    fn about_to_move(f: &mut Field, card: CardId, why: u32, by: Option<EffectId>) {
        f.cards[card].temp.reason = f.cards[card].reason;
        f.cards[card].temp.reason_effect = f.cards[card].reason_effect;
        f.cards[card].temp.reason_player = f.cards[card].reason_player;
        f.cards[card].reason = why;
        f.cards[card].reason_effect = by;
        f.cards[card].reason_player = 0;
    }

    /// The turn player's replacement effects are offered before the
    /// opponent's — expressed as two loops rather than a sort, because
    /// emplaced units are spliced to the front in order.
    #[test]
    fn the_turn_player_is_offered_first() {
        let mut f = Field::new(8000);
        f.infos.turn_player = 0;
        // Registered opponent-first, so only the ordering can put them right.
        let theirs = replacement(&mut f, 1, code::SEND_REPLACE);
        let ours = replacement(&mut f, 0, code::SEND_REPLACE);
        let card = on_field(&mut f, 0);
        let group = f.new_group([card]);

        f.operation_replace(code::SEND_REPLACE, 5, group);
        let order: Vec<EffectId> = f
            .core
            .subunits
            .iter()
            .filter_map(|u| match u.kind {
                Kind::OperationReplace { replace_effect, .. } => Some(replace_effect),
                _ => None,
            })
            .collect();
        assert_eq!(order, vec![ours, theirs], "turn player's first");
    }

    /// Every emplaced unit starts at the step it was asked for, which is how
    /// one unit serves five entry points.
    #[test]
    fn the_units_start_at_the_requested_step() {
        let mut f = Field::new(8000);
        replacement(&mut f, 0, code::DESTROY_REPLACE);
        let card = on_field(&mut f, 0);
        let group = f.new_group([card]);

        f.operation_replace(code::DESTROY_REPLACE, 12, group);
        let steps: Vec<u16> = f.core.subunits.iter().map(|u| u.step).collect();
        assert_eq!(steps, vec![12]);
    }

    /// `is_destroy` is derived from the code, not passed in — so only a
    /// destruction records a cancellation.
    #[test]
    fn only_a_destruction_records_a_cancellation() {
        let mut f = Field::new(8000);
        replacement(&mut f, 0, code::DESTROY_REPLACE);
        let card = on_field(&mut f, 0);
        let group = f.new_group([card]);
        f.operation_replace(code::DESTROY_REPLACE, 5, group);
        assert!(matches!(
            f.core.subunits[0].kind,
            Kind::OperationReplace {
                is_destroy: true,
                ..
            }
        ));

        let mut g = Field::new(8000);
        replacement(&mut g, 0, code::SEND_REPLACE);
        let card = on_field(&mut g, 0);
        let group = g.new_group([card]);
        g.operation_replace(code::SEND_REPLACE, 5, group);
        assert!(matches!(
            g.core.subunits[0].kind,
            Kind::OperationReplace {
                is_destroy: false,
                ..
            }
        ));
    }

    /// A group is a shared handle: what the unit erases, the caller sees.
    #[test]
    fn a_group_is_shared_not_copied() {
        let mut f = Field::new(8000);
        let a = on_field(&mut f, 0);
        let b = on_field(&mut f, 0);
        let group = f.new_group([a, b]);

        f.group_mut(group).remove(&a);
        assert_eq!(f.group(group).len(), 1, "the caller sees the erase");
        assert!(f.group(group).contains(&b));
    }

    /// Sparing a card takes it out of the group and puts back the reason it
    /// had before the move was set up.
    #[test]
    fn sparing_a_card_restores_its_previous_reason() {
        let mut f = Field::new(8000);
        let effect = replacement(&mut f, 0, code::DESTROY_REPLACE);
        let card = on_field(&mut f, 0);
        f.cards[card].reason = reason::BATTLE;
        about_to_move(&mut f, card, reason::EFFECT, None);
        assert_eq!(f.cards[card].reason, reason::EFFECT);

        let group = f.new_group([card]);
        f.core.returns.set(1);
        f.operation_replace_step(1, effect, group, Some(card), true);

        assert!(f.group(group).is_empty(), "taken out of the group");
        assert_eq!(
            f.cards[card].reason,
            reason::BATTLE,
            "and the earlier reason restored"
        );
        assert!(f.destruction_was_cancelled(card));
    }

    /// A card being destroyed by its **own** effect cannot have that
    /// replaced — in the immediate forms. The deferred forms have no such
    /// exemption, which is the difference between steps 1/6 and 11/13.
    #[test]
    fn a_self_destruction_is_exempt_only_in_the_immediate_forms() {
        for (step, spared) in [(1u16, false), (11u16, true)] {
            let mut f = Field::new(8000);
            let effect = replacement(&mut f, 0, code::DESTROY_REPLACE);
            let card = on_field(&mut f, 0);

            let mut own = Effect::new(effect_type::SINGLE, code::SELF_DESTROY);
            own.owner = Some(card);
            own.handler = Some(card);
            let own = f.new_effect(own);

            about_to_move(&mut f, card, reason::EFFECT, Some(own));
            let group = f.new_group([card]);
            f.core.returns.set(1);
            f.operation_replace_step(step, effect, group, Some(card), true);

            assert_eq!(
                f.group(group).is_empty(),
                spared,
                "step {step} should {} the self-destroying card",
                if spared { "spare" } else { "not spare" }
            );
        }
    }

    /// The group form spares exactly the cards the effect's value covers.
    #[test]
    fn the_group_form_spares_what_its_value_covers() {
        fn only_the_first(f: &Field, _: EffectId, card: CardId) -> i64 {
            i64::from(f.cards[card].current.sequence == 0)
        }
        let mut f = Field::new(8000);
        let effect = replacement(&mut f, 0, code::SEND_REPLACE);
        f.effects.get_mut(effect).unwrap().flag[0] |= crate::effect::flag::FUNC_VALUE;
        // A value keyed on the card, so only one of the two is covered.
        f.effects.get_mut(effect).unwrap().value_fn = None;
        let _ = only_the_first;

        let a = on_field(&mut f, 0);
        let b = on_field(&mut f, 0);
        about_to_move(&mut f, a, reason::EFFECT, None);
        about_to_move(&mut f, b, reason::EFFECT, None);
        let group = f.new_group([a, b]);

        // With a constant non-zero value every card is covered.
        f.effects.get_mut(effect).unwrap().flag[0] &= !crate::effect::flag::FUNC_VALUE;
        f.effects.get_mut(effect).unwrap().value = 1;
        f.core.returns.set(1);
        f.operation_replace_step(6, effect, group, None, false);
        assert!(f.group(group).is_empty(), "all covered, all spared");
    }

    /// A value of zero covers nothing, so nothing is spared.
    #[test]
    fn a_value_of_zero_spares_nothing() {
        let mut f = Field::new(8000);
        let effect = replacement(&mut f, 0, code::SEND_REPLACE);
        f.effects.get_mut(effect).unwrap().value = 0;
        let a = on_field(&mut f, 0);
        about_to_move(&mut f, a, reason::EFFECT, None);
        let group = f.new_group([a]);

        f.core.returns.set(1);
        f.operation_replace_step(6, effect, group, None, false);
        assert_eq!(f.group(group).len(), 1, "nothing covered");
    }

    /// Step 16 loops back to 15 by assigning 14, draining the deferred
    /// operations one per pass.
    #[test]
    fn the_deferred_operations_drain_one_per_pass() {
        let mut f = Field::new(8000);
        let effect = replacement(&mut f, 0, code::DESTROY_REPLACE);
        let card = on_field(&mut f, 0);
        let group = f.new_group([card]);

        // Two deferred entries.
        for _ in 0..2 {
            let mut chain = crate::chain::Chain::new(effect, Event::new(0));
            chain.chain_id = 0;
            f.core.desrep_chain.push_back(chain);
        }

        assert!(!f.operation_replace_step(15, effect, group, None, true));
        assert_eq!(f.core.desrep_chain.len(), 1, "one taken");
        assert_eq!(f.core.continuous_chain.len(), 1);

        f.push_back(Kind::OperationReplace {
            replace_effect: effect,
            targets: group,
            target: None,
            is_destroy: true,
        });
        f.operation_replace_step(16, effect, group, None, true);
        assert!(f.core.continuous_chain.is_empty(), "and popped");

        // With none left, the unit finishes.
        f.core.desrep_chain.clear();
        assert!(f.operation_replace_step(15, effect, group, None, true));
    }

    /// An empty group is nothing to replace.
    #[test]
    fn an_empty_group_finishes_at_once() {
        let mut f = Field::new(8000);
        let effect = replacement(&mut f, 0, code::SEND_REPLACE);
        let group = f.new_group([]);
        assert!(f.operation_replace_step(5, effect, group, None, false));
        assert!(f.operation_replace_step(12, effect, group, None, true));
    }

    /// A card already claimed by an earlier replacement is skipped.
    #[test]
    fn an_already_replaced_card_is_skipped() {
        let mut f = Field::new(8000);
        let effect = replacement(&mut f, 0, code::DESTROY_REPLACE);
        let card = on_field(&mut f, 0);
        let group = f.new_group([card]);
        f.core.returns.set(1);
        assert!(f.operation_replace_step(0, effect, group, Some(card), true));
        assert!(f.operation_replace_step(10, effect, group, Some(card), true));
    }
}

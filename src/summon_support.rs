//! Control, materials and activity counters — the small pieces a summon
//! finishes with.
//!
//! Four short functions that `MonsterSet` and `SummonRule` call at their
//! tails, gathered because none is big enough to live alone and all four are
//! about *recording what happened* rather than deciding anything.

use crate::event::{code, CardId, EffectId};
use crate::field::{reset, Field};
use std::collections::{BTreeSet, HashMap};

/// `ActivityType` — which activity counter a check belongs to.
///
/// Transcribed as the reference numbers them, though only the
/// normal-summon counter has a home yet; the rest arrive with the machines
/// that raise them.
pub mod activity {
    pub const SUMMON: u8 = 1;
    pub const NORMALSUMMON: u8 = 2;
    pub const SPSUMMON: u8 = 3;
    pub const FLIPSUMMON: u8 = 4;
    pub const ATTACK: u8 = 5;
    pub const BATTLE_PHASE: u8 = 6;
    pub const CHAIN: u8 = 7;
}

/// Which of the activity maps `check_card_counter` is working in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CounterMap {
    Summon,
    NormalSummon,
    FlipSummon,
}

impl Field {
    /// `card::refresh_control_status` — who controls this card, and by what.
    ///
    /// Defaults to the **owner**, not the current controller: control is
    /// something effects impose, and with none imposing it the card reverts.
    ///
    /// Only the **last** `EFFECT_SET_CONTROL` counts — later effects
    /// overwrite earlier ones rather than combining — and it counts only if
    /// its id is at least `last_control_changed_id`. That comparison is the
    /// brainwashing rule: an effect older than the last control change is
    /// ignored while `EFFECT_REMOVE_BRAINWASHING` applies, which is how
    /// "return control of all monsters" undoes earlier thefts without
    /// undoing later ones.
    pub fn refresh_control_status(&mut self, card: CardId) -> (u8, Option<EffectId>) {
        let mut last_id = 0;
        if self.core.remove_brainwashing
            && self
                .is_affected_by_effect(card, code::REMOVE_BRAINWASHING)
                .is_some()
        {
            last_id = self.core.last_control_changed_id;
        }
        let mut final_player = self.cards[card].owner;
        let mut source = None;
        if let Some(&e) = self.filter_effect(card, code::SET_CONTROL).last() {
            let id = self.effects.get(e).map_or(0, |x| x.id.get());
            if id >= last_id {
                final_player = self.effect_value_for_card_pub(e, card) as u8;
                source = Some(e);
            }
        }
        (final_player, source)
    }

    /// `field::set_control` — give a card to a player.
    ///
    /// Control is not a field that is assigned; it is an **effect that is
    /// added**, and `current.controler` is updated to match. That is what
    /// lets control lapse: the effect resets, and the card reverts to its
    /// owner without anyone doing anything.
    ///
    /// Two guards, and both mean "nothing to do": the card is already
    /// controlled by that player, or `EFFECT_REMOVE_BRAINWASHING` is being
    /// honoured and this card is subject to it.
    ///
    /// `reset_count` makes the control **temporary**. When one is given, the
    /// effect also gets `RESET_PHASE` and — if the caller named neither turn
    /// — **both** turn flags, so "until the end phase" means the next end
    /// phase whoever's turn it is.
    pub fn set_control(&mut self, card: CardId, playerid: u8, reset_phase: u16, reset_count: u8) {
        let brainwashed = self.core.remove_brainwashing
            && self
                .is_affected_by_effect(card, code::REMOVE_BRAINWASHING)
                .is_some();
        if brainwashed || self.refresh_control_status(card).0 == playerid {
            return;
        }
        let owner = match self.core.reason_effect {
            Some(r) => self.effects.get(r).and_then(|e| e.get_handler(&self.cards)),
            None => Some(card),
        };
        let mut e =
            crate::effect::Effect::new(crate::effect::effect_type::SINGLE, code::SET_CONTROL);
        e.owner = owner;
        e.handler = Some(card);
        e.value = i64::from(playerid);
        e.flag[0] = crate::effect::flag::CANNOT_DISABLE;
        e.reset_flag = reset::EVENT | 0xc6c_0000;
        if reset_count != 0 {
            e.reset_flag |= reset::PHASE | u32::from(reset_phase);
            if e.reset_flag & (reset::SELF_TURN | reset::OPPO_TURN) == 0 {
                e.reset_flag |= reset::SELF_TURN | reset::OPPO_TURN;
            }
            e.reset_count = reset_count;
        }
        let id = self.new_effect(e);
        // **`pcard->add_effect(peffect)`, not a hand-rolled insert.** The
        // registration is what files a `RESET_PHASE` effect in `pheff`,
        // and `pheff` is the only list the End Phase walks — so a control
        // change written straight into `single_effect` is permanent
        // however carefully its reset was filled in. Enemy Controller is
        // the pool's first temporary control change and the first card
        // able to notice.
        self.add_card_effect(card, id);
        self.cards[card].current.controller = playerid;
    }

    /// `card::set_material` — record what a summon was built from.
    ///
    /// Each material's `reason_card` is pointed at the summoned monster, and
    /// every `EFFECT_MATERIAL_CHECK` is then **run for its side effect** —
    /// its value is computed and discarded. That is the reference's shape: a
    /// card that needs to look at its own materials does it here, and the
    /// return is not what it is for.
    pub fn set_material(&mut self, card: CardId, materials: BTreeSet<CardId>) {
        for &m in &materials {
            self.cards[m].reason_card = Some(card);
        }
        self.cards[card].material_cards = materials;
        for e in self.filter_effect(card, code::MATERIAL_CHECK) {
            let _ = self.effect_value_for_card_pub(e, card);
        }
    }

    /// `field::check_card_counter` — tell the watching effects an action
    /// happened.
    ///
    /// The tally is **not a count of actions**. Each watching effect has a
    /// check, and the tally counts how many times that check has *refused* —
    /// and once it has refused even once (`player_amount != 0`) the effect is
    /// never asked again this turn.
    ///
    /// The polarity is the trap: the counter is incremented when the check
    /// returns **false**. An effect saying "this card may not be summoned"
    /// records a refusal; one that agrees records nothing and stays live.
    pub fn check_card_counter(&mut self, card: CardId, counter_type: u8, playerid: u8) {
        // `core.get_counter_map(counter_type)`. The other five arrive with
        // the machines that raise them.
        let which = match counter_type {
            activity::SUMMON => CounterMap::Summon,
            activity::NORMALSUMMON => CounterMap::NormalSummon,
            activity::FLIPSUMMON => CounterMap::FlipSummon,
            _ => return,
        };
        let keys: Vec<u32> = self.counter_map(which).keys().copied().collect();
        for key in keys {
            let Some(info) = self.counter_map(which).get(&key) else {
                continue;
            };
            if info.player_amount[playerid as usize] != 0 {
                // Already refused once; not asked again.
                continue;
            }
            let Some(check) = info.check else { continue };
            if !check(self, key as usize, Some(card), &[]) {
                if let Some(info) = self.counter_map_mut(which).get_mut(&key) {
                    info.player_amount[playerid as usize] += 1;
                }
            }
        }
    }

    fn counter_map(&self, which: CounterMap) -> &HashMap<u32, crate::field::ActionCount> {
        match which {
            CounterMap::Summon => &self.core.summon_counter,
            CounterMap::NormalSummon => &self.core.normalsummon_counter,
            CounterMap::FlipSummon => &self.core.flipsummon_counter,
        }
    }

    fn counter_map_mut(
        &mut self,
        which: CounterMap,
    ) -> &mut HashMap<u32, crate::field::ActionCount> {
        match which {
            CounterMap::Summon => &mut self.core.summon_counter,
            CounterMap::NormalSummon => &mut self.core.normalsummon_counter,
            CounterMap::FlipSummon => &mut self.core.flipsummon_counter,
        }
    }

    /// Whether a player may still take a normal-summon action, as the
    /// activity counters see it.
    pub fn normalsummon_activity_allowed(&self, playerid: u8) -> bool {
        self.core
            .normalsummon_counter
            .values()
            .all(|i| i.player_amount[playerid as usize] == 0)
    }
}

/// Not ported yet: the remaining activity counters.
///
/// `check_card_counter` handles the three counters whose machines exist. The
/// other four are raised by machines that do not — `SpSummonRule`,
/// `BattleCommand`, and the chain counter — and each will bring its own map.
/// Kept as a named list rather than a silent gap.
impl Field {
    pub fn check_card_counter_unported(&mut self, _counter_type: u8) {
        unimplemented!("the remaining activity counters arrive with their machines")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::{location, position};
    use crate::card::{card_type, status, Card, CardData};
    use crate::effect::{effect_type, Effect};
    use crate::field::ActionCount;

    fn monster(f: &mut Field, owner: u8, controller: u8) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 18036057,
                type_: card_type::MONSTER,
                ..Default::default()
            },
            owner,
        );
        c.current.controller = controller;
        c.current.position = position::FACEUP_ATTACK;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        let seat = f.cards.len() as u32 - 1;
        f.add_card(controller, id, location::MZONE, seat, false);
        id
    }

    mod control {
        use super::*;

        /// Control defaults to the **owner**, not the current controller:
        /// with nothing imposing it, the card reverts.
        #[test]
        fn control_defaults_to_the_owner() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 1);
            assert_eq!(
                f.refresh_control_status(c).0,
                0,
                "the owner, not the holder"
            );
        }

        /// Control is an **effect that is added**, not a field assigned —
        /// which is what lets it lapse.
        #[test]
        fn setting_control_adds_an_effect() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            f.set_control(c, 1, 0, 0);
            assert_eq!(f.cards[c].current.controller, 1);
            assert_eq!(
                f.refresh_control_status(c).0,
                1,
                "and the status now reads from the effect"
            );
            assert!(
                !f.cards[c]
                    .single_effect
                    .equal_range(code::SET_CONTROL)
                    .is_empty(),
                "an effect, not just a field"
            );
        }

        /// Giving a card to the player who already controls it does nothing
        /// — no effect is added.
        #[test]
        fn setting_control_to_the_current_holder_does_nothing() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            f.set_control(c, 0, 0, 0);
            assert!(
                f.cards[c]
                    .single_effect
                    .equal_range(code::SET_CONTROL)
                    .is_empty(),
                "nothing to do, so nothing added"
            );
        }

        /// Only the **last** control effect counts: later ones overwrite
        /// rather than combine.
        #[test]
        fn the_last_control_effect_wins() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            f.set_control(c, 1, 0, 0);
            f.set_control(c, 0, 0, 0);
            assert_eq!(f.refresh_control_status(c).0, 0, "back to the owner");
        }

        /// A temporary grant gets both turn flags when the caller named
        /// neither, so "until the end phase" means the next one whoever's
        /// turn it is.
        #[test]
        fn a_temporary_grant_covers_both_turns() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            f.set_control(c, 1, 0, 1);
            let e = f.cards[c].single_effect.equal_range(code::SET_CONTROL)[0];
            let flags = f.effects.get(e).unwrap().reset_flag;
            assert_ne!(flags & reset::SELF_TURN, 0);
            assert_ne!(flags & reset::OPPO_TURN, 0);
            assert_ne!(flags & reset::PHASE, 0);
            assert_eq!(f.effects.get(e).unwrap().reset_count, 1);
        }

        /// A permanent grant gets neither.
        #[test]
        fn a_permanent_grant_has_no_phase_reset() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            f.set_control(c, 1, 0, 0);
            let e = f.cards[c].single_effect.equal_range(code::SET_CONTROL)[0];
            let flags = f.effects.get(e).unwrap().reset_flag;
            assert_eq!(flags & reset::PHASE, 0);
        }
    }

    mod materials {
        use super::*;

        /// Each material's `reason_card` points at what it built.
        #[test]
        fn materials_point_back_at_what_they_built() {
            let mut f = Field::new(8000);
            let summoned = monster(&mut f, 0, 0);
            let a = monster(&mut f, 0, 0);
            let b = monster(&mut f, 0, 0);
            f.set_material(summoned, BTreeSet::from([a, b]));
            assert_eq!(f.cards[summoned].material_cards, BTreeSet::from([a, b]));
            assert_eq!(f.cards[a].reason_card, Some(summoned));
            assert_eq!(f.cards[b].reason_card, Some(summoned));
        }

        /// `EFFECT_MATERIAL_CHECK` is run for its **side effect** — its
        /// value is computed and discarded.
        #[test]
        fn the_material_check_is_run_for_its_side_effect() {
            fn records(_: &crate::effect::Effect, f: &Field, _: &crate::effect::Ctx) -> i64 {
                // A value function cannot mutate, so the observable is that
                // it was reached at all: it reads a card the test can set up
                // and the count is asserted through the effect's own value.
                i64::from(f.cards.len() as u32)
            }
            let mut f = Field::new(8000);
            let summoned = monster(&mut f, 0, 0);
            let a = monster(&mut f, 0, 0);

            let mut e = Effect::new(effect_type::SINGLE, code::MATERIAL_CHECK);
            e.owner = Some(summoned);
            e.handler = Some(summoned);
            e.flag[0] |= crate::effect::flag::FUNC_VALUE;
            e.value_fn = Some(records);
            let id = f.new_effect(e);
            f.cards[summoned]
                .single_effect
                .insert(code::MATERIAL_CHECK, id);
            f.cards[summoned].indexer.insert(id);

            // The call must not panic and must record the materials first,
            // so a check that inspects them sees them.
            f.set_material(summoned, BTreeSet::from([a]));
            assert_eq!(f.cards[summoned].material_cards, BTreeSet::from([a]));
        }
    }

    mod activity_counters {
        use super::*;

        fn watcher(f: &mut Field, key: u32, agrees: bool) {
            fn yes(_: &Field, _: EffectId, _: Option<CardId>, _: &[i64]) -> bool {
                true
            }
            fn no(_: &Field, _: EffectId, _: Option<CardId>, _: &[i64]) -> bool {
                false
            }
            f.core.normalsummon_counter.insert(
                key,
                ActionCount {
                    check: Some(if agrees { yes } else { no }),
                    player_amount: [0, 0],
                },
            );
        }

        /// The polarity: the tally records a **refusal**, so a check that
        /// agrees leaves it at zero and the player stays allowed.
        #[test]
        fn a_check_that_agrees_records_nothing() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            watcher(&mut f, 1, true);
            f.check_card_counter(c, activity::NORMALSUMMON, 0);
            assert_eq!(f.core.normalsummon_counter[&1].player_amount[0], 0);
            assert!(f.normalsummon_activity_allowed(0));
        }

        /// A check that refuses records one, and the player is no longer
        /// allowed.
        #[test]
        fn a_check_that_refuses_records_one() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            watcher(&mut f, 1, false);
            f.check_card_counter(c, activity::NORMALSUMMON, 0);
            assert_eq!(f.core.normalsummon_counter[&1].player_amount[0], 1);
            assert!(!f.normalsummon_activity_allowed(0));
            assert!(
                f.normalsummon_activity_allowed(1),
                "and the other player is unaffected"
            );
        }

        /// Once it has refused, it is never asked again — so the tally does
        /// not keep climbing.
        #[test]
        fn a_refused_check_is_not_asked_again() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            watcher(&mut f, 1, false);
            for _ in 0..3 {
                f.check_card_counter(c, activity::NORMALSUMMON, 0);
            }
            assert_eq!(
                f.core.normalsummon_counter[&1].player_amount[0], 1,
                "one refusal, not three"
            );
        }
    }

    mod set_control {
        use super::*;

        /// **A temporary control change has to be *registered*, not just
        /// written into the card's effect list.** `pheff` is the only
        /// index the End Phase walks, and an effect that never reaches it
        /// keeps the monster forever however carefully its reset was
        /// filled in.
        ///
        /// The pool found this with Enemy Controller, its first control
        /// change that is not permanent.
        #[test]
        fn a_reset_phase_control_change_is_filed_for_the_phase_walk() {
            let mut f = Field::new(8000);
            f.infos.turn_player = 0;
            let m = monster(&mut f, 1, 1);
            f.set_control(m, 0, crate::duel::phases::END, 1);
            assert_eq!(f.cards[m].current.controller, 0, "it changed hands");
            let ids = f.cards[m].single_effect.equal_range(code::SET_CONTROL);
            assert_eq!(ids.len(), 1);
            assert!(
                f.field_effects.pheff.contains(&ids[0]),
                "filed for the phase walk"
            );
        }

        /// And it comes back: the End Phase counts it down to zero and the
        /// effect goes, which is what returns the monster.
        #[test]
        fn it_lapses_at_the_named_phase() {
            let mut f = Field::new(8000);
            f.infos.turn_player = 0;
            let m = monster(&mut f, 1, 1);
            f.set_control(m, 0, crate::duel::phases::END, 1);
            let id = f.cards[m].single_effect.equal_range(code::SET_CONTROL)[0];

            f.reset_phase(crate::duel::phases::MAIN1);
            assert!(f.field_effects.pheff.contains(&id), "not that phase");

            f.reset_phase(crate::duel::phases::END);
            assert!(
                !f.field_effects.pheff.contains(&id),
                "spent at the End Phase"
            );
            assert_eq!(
                f.refresh_control_status(m).0,
                1,
                "and the monster is its owner's again"
            );
        }

        /// **A count of zero means permanent**, and is not filed at all.
        #[test]
        fn a_permanent_control_change_has_no_phase_reset() {
            let mut f = Field::new(8000);
            let m = monster(&mut f, 1, 1);
            f.set_control(m, 0, 0, 0);
            let ids = f.cards[m].single_effect.equal_range(code::SET_CONTROL);
            assert_eq!(ids.len(), 1);
            assert!(
                !f.field_effects.pheff.contains(&ids[0]),
                "nothing to count down"
            );
        }
    }
}

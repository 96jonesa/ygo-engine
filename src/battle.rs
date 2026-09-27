//! The battle floor: attack legality, `SelectBattleCmd`, `AttackDisable`,
//! `ForcedBattle`.
//!
//! Everything `BattleCommand` reads before it can be written, and the two
//! small units either side of it. `BattleCommand` itself is not ported;
//! `ForcedBattle` reaches it through the named panic.
//!
//! ## `get_attack_target` is a question that changes the card
//!
//! It looks like a filter — "which monsters may this one attack" — and it
//! is, but it **writes `pcard->direct_attackable` as a side effect**, first
//! clearing it and then setting it at the very end if a direct attack is
//! legal. So `direct_attackable` is not a property to be read; it is the
//! second half of this function's answer, and reading it without having
//! just called this reads whatever the last call left behind.
//!
//! ## The four attack types, and why only two of them filter
//!
//! `atype` names *why* the target list is what it is:
//!
//! | `atype` | |
//! |---|---|
//! | 1 | something must be attacked (`EFFECT_ONLY_BE_ATTACKED`) |
//! | 2 | this card may only attack certain monsters |
//! | 3 | this card must attack certain monsters |
//! | 4 | the ordinary case |
//!
//! The player's own "cannot be selected" prohibitions are applied **only
//! for 2 and 4**. Under 1 and 3 the attack is compelled, and a compulsion
//! overrides a card's protection from being chosen.
//!
//! Each of 1, 2 and 3 also **returns early without filling the list** when
//! the compelled set is not exactly one card (or, for 3, is empty). The
//! caller gets `atype` and an empty list, which does not mean "no legal
//! targets" — it means "ask again differently". A port that treats an empty
//! list as no targets breaks every card in that family.
//!
//! ## The extra-attack test has an escape hatch inside it
//!
//! An attacker past its attack allowance normally gets no targets. The
//! exception is `EFFECT_ATTACK_ALL`, which still contributes targets — but
//! only those it has not already hit as many times as its own value allows.
//! That is the "attacks all your monsters once each" pattern, and it is
//! expressed as a branch *inside* the allowance check rather than beside
//! it.

use crate::board::location;
use crate::card::status;
use crate::event::{code, CardId, EffectId, PLAYER_NONE};
use crate::field::{Field, Message};

/// What kind of attack-target restriction produced the list.
pub mod attack_type {
    /// `EFFECT_ONLY_BE_ATTACKED` — a monster demands to be attacked.
    pub const COMPELLED_TARGET: i32 = 1;
    /// `EFFECT_ONLY_ATTACK_MONSTER` — this card may attack only some.
    pub const RESTRICTED: i32 = 2;
    /// `EFFECT_MUST_ATTACK_MONSTER` — this card must attack some.
    pub const COMPELLED_ATTACKER: i32 = 3;
    /// No restriction.
    pub const ORDINARY: i32 = 4;
}

impl Field {
    /// `card::is_capable_attack_announce` — may this card *declare* an
    /// attack?
    ///
    /// Strictly narrower than `is_capable_attack`, which it calls first.
    /// The difference is `EFFECT_CANNOT_ATTACK_ANNOUNCE` and the attack
    /// cost: a card can be perfectly able to attack and still be unable to
    /// announce one, which is how "you must pay to attack" is expressed.
    pub fn is_capable_attack_announce(&mut self, card: CardId, playerid: u8) -> bool {
        if !self.is_capable_attack(card) {
            return false;
        }
        if self
            .is_affected_by_effect(card, code::CANNOT_ATTACK_ANNOUNCE)
            .is_some()
            && self
                .is_affected_by_effect(card, code::UNSTOPPABLE_ATTACK)
                .is_none()
        {
            return false;
        }
        self.save_lp_cost();
        let affordable = self.check_cost_condition(card, code::ATTACK_COST, playerid);
        self.restore_lp_cost();
        affordable
    }

    /// `field::get_attack_target` — the monsters this card may attack, and
    /// (as a side effect) whether it may attack directly.
    ///
    /// Returns the `attack_type::*` that produced the list. See the module
    /// notes: an empty list under types 1-3 does **not** mean "no targets".
    pub fn get_attack_target(
        &mut self,
        card: CardId,
        out: &mut Vec<CardId>,
        chain_attack: bool,
        select_target: bool,
        must_attack_map: Option<&mut Vec<(EffectId, CardId)>>,
    ) -> i32 {
        self.cards[card].direct_attackable = false;
        let p = self.cards[card].current.controller;
        let opponent: Vec<CardId> = self.players[1 - p as usize]
            .mzone
            .iter()
            .flatten()
            .copied()
            .collect();

        let mut auto_attack = Vec::new();
        let mut only_attack = Vec::new();
        let mut must_attack = Vec::new();
        let mut must_pairs = Vec::new();
        for target in opponent.iter().copied() {
            if self
                .is_affected_by_effect(target, code::ONLY_BE_ATTACKED)
                .is_some()
            {
                auto_attack.push(target);
            }
            if self
                .is_affected_by_effect_against(card, code::ONLY_ATTACK_MONSTER, target)
                .is_some()
            {
                only_attack.push(target);
            }
            if let Some(e) =
                self.is_affected_by_effect_against(card, code::MUST_ATTACK_MONSTER, target)
            {
                must_attack.push(target);
                must_pairs.push((e, target));
            }
        }
        if let Some(map) = must_attack_map {
            map.extend(must_pairs);
        }

        // The four families, in the reference's order — and the first three
        // return *without* filling `out` when the compelled set is not a
        // single card.
        let (atype, pool) = if !auto_attack.is_empty() {
            if auto_attack.len() != 1 {
                return attack_type::COMPELLED_TARGET;
            }
            (attack_type::COMPELLED_TARGET, auto_attack)
        } else if self
            .is_affected_by_effect(card, code::ONLY_ATTACK_MONSTER)
            .is_some()
        {
            if only_attack.len() != 1 {
                return attack_type::RESTRICTED;
            }
            (attack_type::RESTRICTED, only_attack)
        } else if self
            .is_affected_by_effect(card, code::MUST_ATTACK_MONSTER)
            .is_some()
        {
            if must_attack.is_empty() {
                return attack_type::COMPELLED_ATTACKER;
            }
            (attack_type::COMPELLED_ATTACKER, must_attack)
        } else {
            // The ordinary case gathers the *whole* opposing row, including
            // the empty seats — the reference pushes `atarget` without a
            // null check here and filters later, so a row with gaps still
            // produces the right count.
            let mut pool: Vec<CardId> = opponent
                .iter()
                .copied()
                .filter(|&t| Some(t) != self.core.attacker)
                .collect();
            // `EFFECT_SELF_ATTACK` adds the attacker's *own* side, unless
            // an attacks-all effect already has opposing targets to work
            // with.
            let attacks_all = self.is_affected_by_effect(card, code::ATTACK_ALL).is_some();
            if self
                .is_player_affected_by_effect(p, code::SELF_ATTACK)
                .is_some()
                && (!attacks_all || pool.is_empty())
            {
                let own: Vec<CardId> = self.players[p as usize]
                    .mzone
                    .iter()
                    .flatten()
                    .copied()
                    .filter(|&t| Some(t) != self.core.attacker)
                    .collect();
                pool.extend(own);
            }
            (attack_type::ORDINARY, pool)
        };

        let extra_count = self.best_effect_value(card, code::EXTRA_ATTACK);
        let extra_count_m = self.best_effect_value(card, code::EXTRA_ATTACK_MONSTER);

        // Has this card already used up its attacks?
        let announce = i32::from(self.cards[card].announce_count);
        let direct_already = self.cards[card].announced_cards.count(0, false) > 0;
        let spent = !chain_attack
            && announce > extra_count
            && (extra_count_m == 0 || announce > extra_count_m || direct_already);
        if spent {
            self.attack_all_targets(card, &pool, atype, select_target, out);
            return atype;
        }

        let mut mcount = 0u32;
        for target in pool {
            if atype >= attack_type::RESTRICTED
                && self
                    .is_affected_by_effect_against(target, code::IGNORE_BATTLE_TARGET, card)
                    .is_some()
            {
                continue;
            }
            if self.cards[target].current.controller != p {
                mcount += 1;
            }
            if chain_attack
                && self.core.chain_attack_target.is_some()
                && Some(target) != self.core.chain_attack_target
            {
                continue;
            }
            if select_target && self.selection_is_blocked(card, target, atype) {
                continue;
            }
            out.push(target);
        }

        if atype <= attack_type::COMPELLED_ATTACKER {
            return atype;
        }

        // The direct attack, and the only place `direct_attackable` is set.
        let no_monsters = mcount == 0;
        let may_direct = no_monsters
            || self
                .is_affected_by_effect(card, code::DIRECT_ATTACK)
                .is_some()
            || self.core.attack_player;
        let blocked = self
            .is_affected_by_effect(card, code::CANNOT_DIRECT_ATTACK)
            .is_some()
            || (!chain_attack && extra_count_m != 0 && announce > extra_count)
            || (chain_attack && self.core.chain_attack_target.is_some());
        if may_direct && !blocked {
            self.cards[card].direct_attackable = true;
        }
        atype
    }

    /// The `EFFECT_ATTACK_ALL` escape hatch, taken when the attacker is
    /// otherwise out of attacks.
    ///
    /// It contributes only targets this card has not already hit as many
    /// times as the effect's own value allows — so "attack every monster
    /// once" and "attack every monster twice" are the same effect with
    /// different values, and the tally is what tells them apart.
    fn attack_all_targets(
        &mut self,
        card: CardId,
        pool: &[CardId],
        atype: i32,
        select_target: bool,
        out: &mut Vec<CardId>,
    ) {
        let Some(effect) = self.is_affected_by_effect(card, code::ATTACK_ALL) else {
            return;
        };
        if !self.cards[card].attack_all_target {
            return;
        }
        for target in pool.iter().copied() {
            if !self.effect_condition_holds_for(effect, target) {
                continue;
            }
            let fid = self.cards[target].fieldid_r;
            let allowance = self.effect_value_for(effect, target) as u32;
            if self.cards[card].announced_cards.count(fid, true) >= allowance {
                continue;
            }
            if atype >= attack_type::RESTRICTED
                && self
                    .is_affected_by_effect_against(target, code::IGNORE_BATTLE_TARGET, card)
                    .is_some()
            {
                continue;
            }
            if select_target && self.selection_is_blocked(card, target, atype) {
                continue;
            }
            out.push(target);
        }
    }

    /// The two "may not be chosen" prohibitions, read **only** for the
    /// unforced attack types. Under a compulsion the attack happens anyway.
    fn selection_is_blocked(&self, card: CardId, target: CardId, atype: i32) -> bool {
        if atype != attack_type::RESTRICTED && atype != attack_type::ORDINARY {
            return false;
        }
        self.is_affected_by_effect_against(target, code::CANNOT_BE_BATTLE_TARGET, card)
            .is_some()
            || self
                .is_affected_by_effect_against(card, code::CANNOT_SELECT_BATTLE_TARGET, target)
                .is_some()
    }

    /// The largest value any effect with this code gives, or zero.
    fn best_effect_value(&mut self, card: CardId, code_: u32) -> i32 {
        let mut best = 0;
        for e in self.filter_effect(card, code_) {
            let v = self.effect_value_for(e, card) as i32;
            if v > best {
                best = v;
            }
        }
        best
    }

    /// An effect's value asked about one card.
    fn effect_value_for(&self, effect: EffectId, card: CardId) -> i64 {
        let Some(e) = self.effects.get(effect) else {
            return 0;
        };
        let ev = crate::event::Event::new(0);
        let ctx = crate::effect::Ctx {
            reason_effect: effect,
            player: self.cards[card].current.controller,
            event: &ev,
            card: Some(card),
            args: &[],
        };
        e.get_value(self, &ctx)
    }

    /// `check_value_condition(1)` — the effect's value read as a predicate
    /// about one card.
    fn effect_condition_holds_for(&self, effect: EffectId, card: CardId) -> bool {
        self.effect_value_for(effect, card) != 0
    }

    /// `field::confirm_attack_target` — is the declared attack still legal?
    ///
    /// Re-derives the legal targets from scratch rather than trusting what
    /// was offered, because anything may have changed since. `select_target`
    /// is **false** here: the "cannot be chosen" prohibitions gate the
    /// declaration, not whether a declared attack stands.
    pub fn confirm_attack_target(&mut self) -> bool {
        let Some(attacker) = self.core.attacker else {
            return false;
        };
        let mut targets = Vec::new();
        let chain = self.core.chain_attack;
        self.get_attack_target(attacker, &mut targets, chain, false, None);
        match self.core.attack_target {
            Some(t) => targets.contains(&t),
            None => self.cards[attacker].direct_attackable,
        }
    }

    /// `field::process(Processors::SelectBattleCmd&)` — the Battle Phase's
    /// menu.
    ///
    /// The same two-part shape as `SelectIdleCmd` and a much shorter list:
    /// activate, attack, go to Main Phase 2, go to the End Phase. Like that
    /// one it sorts the chain offers **inside the question**, and never
    /// short-circuits.
    pub(crate) fn select_battle_cmd_step(&mut self, step: u16, player: u8) -> bool {
        if step == 0 {
            let mut offered: Vec<crate::chain::Chain> = self.core.select_chains.drain(..).collect();
            offered.sort_by_key(|c| {
                self.effects
                    .get(c.triggering_effect)
                    .map_or(0, |e| e.id.get())
            });
            self.core.select_chains = offered.into();
            let activatable = self
                .core
                .select_chains
                .iter()
                .map(|c| {
                    let effect = c.triggering_effect;
                    let handler = self
                        .effects
                        .get(effect)
                        .and_then(|e| e.get_handler(&self.cards));
                    crate::field::ChainOffer {
                        code: handler.map_or(0, |h| self.cards[h].data.code),
                        info: handler
                            .map(|h| self.get_info_location(h))
                            .unwrap_or_default(),
                        description: self.effects.get(effect).map_or(0, |e| e.description),
                        client_mode: self.client_mode(effect),
                    }
                })
                .collect();
            let attackable = self
                .core
                .attackable_cards
                .clone()
                .into_iter()
                .map(|c| crate::field::AttackOffer {
                    code: self.cards[c].data.code,
                    controller: self.cards[c].current.controller,
                    location: self.cards[c].current.location,
                    sequence: self.cards[c].current.sequence,
                    direct_attackable: self.cards[c].direct_attackable,
                })
                .collect();
            self.messages.push(Message::SelectBattleCmd {
                player,
                activatable,
                attackable,
                to_m2: self.core.to_m2,
                to_ep: self.core.to_ep,
            });
            return false;
        }
        let answer = self.core.returns.get() as u32;
        let kind = answer & 0xffff;
        let index = (answer >> 16) as usize;
        let legal = match kind {
            0 => index < self.core.select_chains.len(),
            1 => index < self.core.attackable_cards.len(),
            2 => self.core.to_m2,
            3 => self.core.to_ep,
            _ => false,
        };
        if !legal {
            self.messages.push(Message::Retry);
            return false;
        }
        true
    }

    /// `field::process(Processors::AttackDisable&)` — negate the attack
    /// that is happening.
    ///
    /// **The answer is in `returns`, not in whether the unit finished**:
    /// `0` for "there was nothing to negate", `1` for "negated". Seven
    /// conditions produce the `0`, and the one that is easy to miss is
    /// `attacker->fieldid_r != core.pre_field[0]` — the attacker must be
    /// *the same instance* that declared the attack. A card that left and
    /// returned has a new field id and is not it.
    pub(crate) fn attack_disable_step(&mut self, step: u16) -> bool {
        if step != 0 {
            self.core.returns.set(1);
            return true;
        }
        let Some(attacker) = self.core.attacker else {
            self.core.returns.set(0);
            return true;
        };
        let reason = self.core.reason_effect;
        let refused = self.core.effect_damage_step != 0
            || self.cards[attacker].fieldid_r != self.core.pre_field[0]
            || self.cards[attacker].current.location != location::MZONE
            || !self.is_capable_attack(attacker)
            || !self.is_affect_by_effect(attacker, reason)
            || self
                .is_affected_by_effect(attacker, code::UNSTOPPABLE_ATTACK)
                .is_some();
        if refused {
            self.core.returns.set(0);
            return true;
        }

        let mut e =
            crate::effect::Effect::new(crate::effect::effect_type::SINGLE, code::ATTACK_DISABLED);
        e.owner = Some(attacker);
        e.handler = Some(attacker);
        let id = self.new_effect(e);
        self.cards[attacker]
            .single_effect
            .insert(code::ATTACK_DISABLED, id);
        self.cards[attacker].indexer.insert(id);
        self.cards[attacker].set_status(status::ATTACK_CANCELED, true);

        let rp = self.core.reason_player;
        self.raise_event(
            Some(attacker),
            code::ATTACK_DISABLED_EVENT,
            reason,
            0,
            rp,
            PLAYER_NONE,
            0,
        );
        self.process_instant_event();
        false
    }

    /// `field::process(Processors::ForcedBattle&)` — an effect is making an
    /// attack happen outside the Battle Phase.
    ///
    /// Case 0 is six refusals and then a whole fake Battle Phase: the
    /// current phase is saved, `PHASE_BATTLE_STEP` is entered, and
    /// `BattleCommand` is emplaced **at step 1** so it skips its own
    /// attack-declaration step. Case 1 puts the phase back.
    ///
    /// The attack tallies are cleared **twice**, once on the way in and
    /// once on the way out — a forced attack neither spends the turn's
    /// attacks nor leaves its own behind.
    pub(crate) fn forced_battle_step(&mut self, step: u16, state: &mut ForcedBattleState) -> bool {
        if step != 0 {
            return self.forced_battle_finish(state);
        }
        let tp = self.infos.turn_player;
        if self
            .is_player_affected_by_effect(tp, code::CANNOT_BP)
            .is_some()
        {
            return true;
        }
        self.core.battle_phase_count[tp as usize] += 1;
        if self
            .is_player_affected_by_effect(tp, code::SKIP_BP)
            .is_some()
            || self.core.force_turn_end
        {
            self.messages.push(Message::NewPhase {
                phase: crate::duel::phases::BATTLE_START,
            });
            self.reset_phase(crate::duel::phases::BATTLE_START);
            self.reset_phase(crate::duel::phases::BATTLE_STEP);
            self.reset_phase(crate::duel::phases::BATTLE);
            self.adjust_all();
            let phase = self.infos.phase;
            self.messages.push(Message::NewPhase { phase });
            return true;
        }

        state.backup_phase = self.infos.phase;
        let Some(attacker) = self.core.forced_attacker else {
            return true;
        };
        let wanted = self.core.forced_attack_target;
        if !self.is_capable_attack_announce(attacker, tp) {
            return true;
        }
        let mut targets = Vec::new();
        self.get_attack_target(attacker, &mut targets, false, true, None);
        let no_target = targets.is_empty() && !self.cards[attacker].direct_attackable;
        let wrong_target = wanted.is_some_and(|t| !targets.contains(&t));
        if no_target || wrong_target {
            return true;
        }

        self.core.attacker = Some(attacker);
        self.core.attack_target = wanted;
        self.clear_attack_tallies();
        self.core.attack_cancelable = true;
        self.core.attack_cost_paid = false;
        self.core.chain_attacker_id = 0;
        self.core.chain_attack_target = None;
        self.core.returns.set(1);

        let phase = self.infos.phase;
        self.reset_phase(phase);
        self.reset_phase(crate::duel::phases::BATTLE_START);
        self.infos.phase = crate::duel::phases::BATTLE_STEP;
        self.clear_pending_chains();
        self.messages.push(Message::NewPhase {
            phase: crate::duel::phases::BATTLE_START,
        });
        // `emplace_process<BattleCommand>(Step{ 1 }, nullptr, true)` — at
        // step **1**, so the menu is skipped: the attack has already been
        // declared by whatever forced it, and `forced_attack` is what makes
        // the unit answer `2` and hand back rather than looping.
        self.emplace_at(
            crate::processor::Kind::BattleCommand {
                state: Box::new(crate::battle_command::BattleCommandState {
                    forced_attack: true,
                    ..Default::default()
                }),
            },
            1,
        );
        false
    }

    /// Case 1: put the phase back and forget the attack.
    fn forced_battle_finish(&mut self, state: &mut ForcedBattleState) -> bool {
        let phase = self.infos.phase;
        self.reset_phase(phase);
        self.infos.phase = state.backup_phase;
        self.clear_pending_chains();
        self.clear_attack_tallies();
        self.core.attacker = None;
        self.core.attack_target = None;
        let phase = self.infos.phase;
        self.messages.push(Message::NewPhase { phase });
        true
    }

    /// Every monster's per-attack tallies, on both sides. The same block
    /// `Turn`'s case 0 runs, and `ForcedBattle` runs it twice.
    pub(crate) fn clear_attack_tallies(&mut self) {
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
    }
}

/// `ForcedBattle`'s own state: the phase to go back to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ForcedBattleState {
    pub backup_phase: u16,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::position;
    use crate::card::{card_type, AttackerMap, Card, CardData};
    use crate::effect::{effect_type, flag, Effect};
    use crate::field::Message;
    use crate::processor::{Kind, Status};

    fn field() -> Field {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        f
    }

    fn monster(f: &mut Field, player: u8, seat: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 18036057 + seat + u32::from(player) * 100,
                type_: card_type::MONSTER,
                level: 4,
                attack: 1000,
                ..Default::default()
            },
            player,
        );
        c.current.controller = player;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(player, id, location::MZONE, seat, false);
        f.cards[id].current.position = position::FACEUP_ATTACK;
        // A distinct, non-zero field id. `add_card` does not assign one,
        // and zero is the `AttackerMap`'s direct-attack key — so a fixture
        // that leaves it at zero files every monster under the player's
        // slot.
        let fid = f.next_field_id_raw();
        f.cards[id].fieldid = fid;
        f.cards[id].fieldid_r = fid;
        id
    }

    /// An `EFFECT_SINGLE` on a card.
    ///
    /// The value is **1**, not the default 0, because
    /// `is_affected_by_effect_against` asks the effect's value about the
    /// target and takes zero as "does not apply to this one". A fixture
    /// that leaves the value at zero builds a prohibition that prohibits
    /// nothing.
    fn single(f: &mut Field, card: CardId, code_: u32) -> EffectId {
        single_valued(f, card, code_, 1)
    }

    fn single_valued(f: &mut Field, card: CardId, code_: u32, value: i64) -> EffectId {
        let mut e = Effect::new(effect_type::SINGLE, code_);
        e.owner = Some(card);
        e.handler = Some(card);
        e.value = value;
        let id = f.new_effect(e);
        f.cards[card].single_effect.insert(code_, id);
        f.cards[card].indexer.insert(id);
        id
    }

    fn player_effect(f: &mut Field, code_: u32, player: u8) -> EffectId {
        let anchor = monster(f, player, 4);
        let mut e = Effect::new(effect_type::FIELD, code_);
        e.owner = Some(anchor);
        e.handler = Some(anchor);
        e.flag[0] = flag::PLAYER_TARGET | flag::ABSOLUTE_TARGET;
        e.range = u16::from(location::MZONE);
        // `ABSOLUTE_TARGET` makes these name the players outright:
        // `s_range` is player 0, `o_range` is player 1. Not "own side".
        if player == 0 {
            e.s_range = 1;
        } else {
            e.o_range = 1;
        }
        let id = f.new_effect(e);
        f.add_effect(id, player);
        id
    }

    /// The legal targets and whether a direct attack is on.
    fn targets(f: &mut Field, attacker: CardId) -> (Vec<CardId>, bool, i32) {
        let mut out = Vec::new();
        let atype = f.get_attack_target(attacker, &mut out, false, true, None);
        (out, f.cards[attacker].direct_attackable, atype)
    }

    mod attacker_map {
        use super::*;

        /// **The key is the field id, not the card.** The same card with a
        /// new field id is a different target — which is what makes "attack
        /// each monster once" let an attacker hit a card that left and came
        /// back.
        #[test]
        fn it_counts_by_field_id() {
            let mut m = AttackerMap::default();
            m.add(Some(1), 10);
            m.add(Some(1), 10);
            assert_eq!(m.count(10, true), 2);
            // The same card, a new field id.
            m.add(Some(1), 11);
            assert_eq!(m.count(11, true), 1, "a different entry");
            assert_eq!(m.count(10, true), 2, "and the old one is untouched");
        }

        /// **Key 0 is the direct attack**, tallied in the same map under a
        /// key no real card can have.
        #[test]
        fn the_player_is_key_zero() {
            let mut m = AttackerMap::default();
            m.add(None, 999);
            assert_eq!(m.count(0, false), 1, "the field id is ignored for None");
            assert_eq!(m.count(999, true), 0, "and it is not filed under it");
        }

        #[test]
        fn an_unseen_target_counts_zero() {
            let m = AttackerMap::default();
            assert_eq!(m.count(5, true), 0);
            assert!(m.is_empty());
        }

        /// **A lookup for the player ignores the field id it is handed.**
        /// The caller passes a card's id and a flag; when the flag says
        /// "not a card", the id must be discarded rather than used.
        #[test]
        fn the_null_lookup_discards_the_field_id() {
            let mut m = AttackerMap::default();
            m.add(None, 0);
            m.add(Some(1), 7);
            assert_eq!(m.count(7, false), 1, "asked about the player, not card 7");
            assert_eq!(m.count(7, true), 1, "asked about card 7");
            assert_eq!(m.count(99, false), 1, "any id, still the player");
        }
    }

    mod is_capable_attack_announce {
        use super::*;

        /// It is strictly narrower than `is_capable_attack`.
        #[test]
        fn it_builds_on_is_capable_attack() {
            let mut f = field();
            let m = monster(&mut f, 0, 0);
            assert!(f.is_capable_attack_announce(m, 0));

            f.cards[m].current.position = position::FACEDOWN_DEFENSE;
            assert!(!f.is_capable_attack_announce(m, 0), "cannot attack at all");
        }

        /// **`CANNOT_ATTACK_ANNOUNCE` is a different prohibition from
        /// `CANNOT_ATTACK`**, and `UNSTOPPABLE_ATTACK` waives it.
        #[test]
        fn the_announce_prohibition_is_its_own_and_is_waivable() {
            let mut f = field();
            let m = monster(&mut f, 0, 0);
            single(&mut f, m, code::CANNOT_ATTACK_ANNOUNCE);
            assert!(f.is_capable_attack(m), "it can still attack");
            assert!(!f.is_capable_attack_announce(m, 0), "but not declare one");

            single(&mut f, m, code::UNSTOPPABLE_ATTACK);
            assert!(f.is_capable_attack_announce(m, 0), "waived");
        }

        /// **A refusing attack cost refuses**, and the LP stack comes back
        /// level.
        #[test]
        fn a_refusing_attack_cost_refuses() {
            let mut f = field();
            let m = monster(&mut f, 0, 0);
            let e = single(&mut f, m, code::ATTACK_COST);
            if let Some(x) = f.effects.get_mut(e) {
                x.cost = Some(|_, _, _| false);
            }
            let before = f.core.lp_cost[0].count;
            assert!(!f.is_capable_attack_announce(m, 0));
            assert_eq!(f.core.lp_cost[0].count, before);
        }
    }

    mod get_attack_target {
        use super::*;

        /// The ordinary case: every opposing monster is a target, and with
        /// monsters present there is no direct attack.
        #[test]
        fn the_ordinary_case_offers_the_opposing_row() {
            let mut f = field();
            let a = monster(&mut f, 0, 0);
            let x = monster(&mut f, 1, 0);
            let y = monster(&mut f, 1, 1);
            let (out, direct, atype) = targets(&mut f, a);
            assert_eq!(atype, attack_type::ORDINARY);
            assert_eq!(out, vec![x, y]);
            assert!(!direct, "monsters are in the way");
        }

        /// **An empty opposing row means a direct attack**, and the flag is
        /// written by this call.
        #[test]
        fn an_empty_row_means_a_direct_attack() {
            let mut f = field();
            let a = monster(&mut f, 0, 0);
            let (out, direct, _) = targets(&mut f, a);
            assert!(out.is_empty());
            assert!(direct);
        }

        /// **`direct_attackable` is cleared on entry.** A stale `true` from
        /// a previous call does not survive a call that does not set it.
        #[test]
        fn the_flag_is_cleared_on_entry() {
            let mut f = field();
            let a = monster(&mut f, 0, 0);
            let _ = targets(&mut f, a);
            assert!(f.cards[a].direct_attackable, "set by the empty row");

            monster(&mut f, 1, 0);
            let (_, direct, _) = targets(&mut f, a);
            assert!(!direct, "cleared, not left over");
        }

        /// **A restricted attacker never attacks directly**, even with an
        /// empty opposing row. Types 1-3 return before the direct-attack
        /// block is reached at all.
        #[test]
        fn a_restricted_attacker_never_attacks_directly() {
            let mut f = field();
            let a = monster(&mut f, 0, 0);
            let x = monster(&mut f, 1, 0);
            // Exactly one restricted target, so the early "not exactly one"
            // return is *not* taken and the walk reaches the end of the
            // function — and `DIRECT_ATTACK`, which would otherwise grant a
            // direct attack outright.
            single(&mut f, a, code::ONLY_ATTACK_MONSTER);
            single(&mut f, a, code::DIRECT_ATTACK);
            let (out, direct, atype) = targets(&mut f, a);
            assert_eq!(atype, attack_type::RESTRICTED);
            assert_eq!(out, vec![x]);
            assert!(
                !direct,
                "type 2 returns before the direct-attack block, so DIRECT_ATTACK never applies"
            );
        }

        /// **`EFFECT_CANNOT_DIRECT_ATTACK` blocks it** even with an empty
        /// row, and `EFFECT_DIRECT_ATTACK` allows it with a full one.
        #[test]
        fn the_two_direct_attack_effects_pull_opposite_ways() {
            let mut f = field();
            let a = monster(&mut f, 0, 0);
            single(&mut f, a, code::CANNOT_DIRECT_ATTACK);
            let (_, direct, _) = targets(&mut f, a);
            assert!(!direct, "blocked despite an empty row");

            let mut f = field();
            let a = monster(&mut f, 0, 0);
            monster(&mut f, 1, 0);
            single(&mut f, a, code::DIRECT_ATTACK);
            let (out, direct, _) = targets(&mut f, a);
            assert!(!out.is_empty(), "the monster is still a target");
            assert!(direct, "and so is the player");
        }

        /// **`EFFECT_ONLY_BE_ATTACKED` compels**, and with more than one
        /// such monster the list comes back **empty with type 1** — which
        /// does not mean "no targets".
        #[test]
        fn a_compelled_target_wins_and_two_of_them_return_empty() {
            let mut f = field();
            let a = monster(&mut f, 0, 0);
            let x = monster(&mut f, 1, 0);
            monster(&mut f, 1, 1);
            single(&mut f, x, code::ONLY_BE_ATTACKED);
            let (out, _, atype) = targets(&mut f, a);
            assert_eq!(atype, attack_type::COMPELLED_TARGET);
            assert_eq!(out, vec![x], "only the compelling monster");

            let mut f = field();
            let a = monster(&mut f, 0, 0);
            let x = monster(&mut f, 1, 0);
            let y = monster(&mut f, 1, 1);
            single(&mut f, x, code::ONLY_BE_ATTACKED);
            single(&mut f, y, code::ONLY_BE_ATTACKED);
            let (out, _, atype) = targets(&mut f, a);
            assert_eq!(atype, attack_type::COMPELLED_TARGET);
            assert!(
                out.is_empty(),
                "two compulsions return early with nothing — not 'no targets'"
            );
        }

        /// **A compulsion overrides "cannot be attacked".** The selection
        /// prohibitions are read only for types 2 and 4.
        #[test]
        fn a_compulsion_overrides_the_selection_prohibitions() {
            let mut f = field();
            let a = monster(&mut f, 0, 0);
            let x = monster(&mut f, 1, 0);
            single(&mut f, x, code::CANNOT_BE_BATTLE_TARGET);

            // Ordinary: the prohibition applies.
            let (out, _, atype) = targets(&mut f, a);
            assert_eq!(atype, attack_type::ORDINARY);
            assert!(out.is_empty(), "protected");

            // Compelled: it does not.
            single(&mut f, x, code::ONLY_BE_ATTACKED);
            let (out, _, atype) = targets(&mut f, a);
            assert_eq!(atype, attack_type::COMPELLED_TARGET);
            assert_eq!(out, vec![x], "the compulsion wins");
        }

        /// **The two selection prohibitions sit on opposite cards.**
        /// `CANNOT_BE_BATTLE_TARGET` is on the target; `CANNOT_SELECT_
        /// BATTLE_TARGET` is on the attacker and names the target it may
        /// not pick. Both have to be read, and a test of one says nothing
        /// about the other.
        #[test]
        fn the_attacker_side_selection_prohibition_is_read() {
            let mut f = field();
            let a = monster(&mut f, 0, 0);
            let x = monster(&mut f, 1, 0);
            single(&mut f, a, code::CANNOT_SELECT_BATTLE_TARGET);
            let (out, _, _) = targets(&mut f, a);
            assert!(out.is_empty(), "the attacker may not pick it");
            assert!(
                !f.cards[x].is_status(status::ATTACK_CANCELED),
                "and nothing happened to the target"
            );
        }

        /// **`select_target = false` skips the selection prohibitions**
        /// entirely — which is why `confirm_attack_target` passes false: it
        /// is checking a declared attack, not offering one.
        #[test]
        fn select_target_false_ignores_the_selection_prohibitions() {
            let mut f = field();
            let a = monster(&mut f, 0, 0);
            let x = monster(&mut f, 1, 0);
            single(&mut f, x, code::CANNOT_BE_BATTLE_TARGET);

            let mut out = Vec::new();
            f.get_attack_target(a, &mut out, false, true, None);
            assert!(out.is_empty(), "offered: blocked");

            let mut out = Vec::new();
            f.get_attack_target(a, &mut out, false, false, None);
            assert_eq!(out, vec![x], "confirmed: not blocked");
        }

        /// **A `MUST_ATTACK_MONSTER` that matches nothing returns empty
        /// with type 3**, rather than falling through to the ordinary case
        /// — so a monster compelled to attack something that is not there
        /// attacks nothing at all.
        /// **A compulsion that matches nothing returns type 3 with an
        /// empty list** — it does *not* fall through to the ordinary case.
        /// So a monster compelled to attack something that is not there
        /// attacks nothing, rather than attacking freely.
        ///
        /// The blanket lookup and the per-target one are the same effect
        /// asked two ways: `is_affected_by_effect` ignores the value, while
        /// `is_affected_by_effect_against` requires it to be non-zero for
        /// that target. A function value that answers zero for every card
        /// on the field is therefore present but unsatisfiable.
        #[test]
        fn an_unsatisfiable_compulsion_returns_empty() {
            let mut f = field();
            let a = monster(&mut f, 0, 0);
            let x = monster(&mut f, 1, 0);
            let e = single_valued(&mut f, a, code::MUST_ATTACK_MONSTER, 1);
            if let Some(v) = f.effects.get_mut(e) {
                v.flag[0] |= flag::FUNC_VALUE;
                v.value_fn = Some(|_, _, _| 0);
            }
            let (out, direct, atype) = targets(&mut f, a);
            assert_eq!(atype, attack_type::COMPELLED_ATTACKER);
            assert!(out.is_empty(), "nothing satisfies the compulsion");
            assert!(
                !out.contains(&x),
                "and it did not fall through to the ordinary case"
            );
            assert!(!direct, "nor to a direct attack");
        }

        /// **`EFFECT_MUST_ATTACK_MONSTER` fills the out-map** so the caller
        /// learns *which effect* compelled each target.
        #[test]
        fn the_must_attack_map_records_the_effect() {
            let mut f = field();
            let a = monster(&mut f, 0, 0);
            let x = monster(&mut f, 1, 0);
            let e = single(&mut f, a, code::MUST_ATTACK_MONSTER);
            let mut out = Vec::new();
            let mut map = Vec::new();
            let atype = f.get_attack_target(a, &mut out, false, true, Some(&mut map));
            assert_eq!(atype, attack_type::COMPELLED_ATTACKER);
            assert_eq!(map, vec![(e, x)]);
        }

        /// **`IGNORE_BATTLE_TARGET` is read for every type except 1.**
        /// The guard is `atype >= 2`, and 4 — the ordinary case — is above
        /// it, so the only family it exempts is the compelled target: a
        /// monster that demands to be attacked cannot then ignore the
        /// attack.
        #[test]
        fn ignore_battle_target_is_read_for_every_type_but_the_compelled_one() {
            // Type 4: removed, because 4 >= 2.
            let mut f = field();
            let a = monster(&mut f, 0, 0);
            let x = monster(&mut f, 1, 0);
            single(&mut f, x, code::IGNORE_BATTLE_TARGET);
            let (out, _, atype) = targets(&mut f, a);
            assert_eq!(atype, attack_type::ORDINARY);
            assert!(out.is_empty(), "read in the ordinary case too");

            // Type 1: exempt. A monster demanding to be attacked cannot
            // also ignore the attack.
            let mut f = field();
            let a = monster(&mut f, 0, 0);
            let x = monster(&mut f, 1, 0);
            single(&mut f, x, code::IGNORE_BATTLE_TARGET);
            single(&mut f, x, code::ONLY_BE_ATTACKED);
            let (out, _, atype) = targets(&mut f, a);
            assert_eq!(atype, attack_type::COMPELLED_TARGET);
            assert_eq!(out, vec![x], "the compulsion wins over the ignore");
        }

        /// **An attacker out of attacks gets nothing**, and
        /// `EFFECT_EXTRA_ATTACK` buys more.
        #[test]
        fn a_spent_attacker_gets_nothing_until_extra_attacks() {
            let mut f = field();
            let a = monster(&mut f, 0, 0);
            monster(&mut f, 1, 0);
            f.cards[a].announce_count = 1;
            let (out, _, _) = targets(&mut f, a);
            assert!(out.is_empty(), "already attacked");

            single_valued(&mut f, a, code::EXTRA_ATTACK, 1);
            let (out, _, _) = targets(&mut f, a);
            assert_eq!(out.len(), 1, "one more attack bought");
        }

        /// **A chain attack waives the allowance entirely.** An attacker
        /// that has used its attack still gets targets when the attack is
        /// being chained.
        #[test]
        fn a_chain_attack_waives_the_allowance() {
            let mut f = field();
            let a = monster(&mut f, 0, 0);
            let x = monster(&mut f, 1, 0);
            f.cards[a].announce_count = 1;

            let mut out = Vec::new();
            f.get_attack_target(a, &mut out, false, true, None);
            assert!(out.is_empty(), "spent");

            let mut out = Vec::new();
            f.get_attack_target(a, &mut out, true, true, None);
            assert_eq!(out, vec![x], "but a chain attack gets one anyway");
        }

        /// **`EFFECT_ATTACK_ALL` is an escape hatch inside the allowance
        /// check**: a spent attacker still gets targets it has not already
        /// hit as often as the effect's value allows.
        #[test]
        fn attack_all_gives_a_spent_attacker_unhit_targets() {
            let mut f = field();
            let a = monster(&mut f, 0, 0);
            let x = monster(&mut f, 1, 0);
            let y = monster(&mut f, 1, 1);
            single_valued(&mut f, a, code::ATTACK_ALL, 1);
            // `attack_all_target` is value-initialised **false** in the
            // reference and set true by `card::reset` and by `Turn`'s case
            // 0 — so a card that has been through a turn has it, and a
            // fresh fixture does not.
            f.cards[a].attack_all_target = true;
            f.cards[a].announce_count = 1;
            // It already hit `x` once, which is its allowance for `x`.
            let fid = f.cards[x].fieldid_r;
            f.cards[a].announced_cards.add(Some(x), fid);

            let (out, _, _) = targets(&mut f, a);
            assert_eq!(out, vec![y], "only the one it has not hit");
        }

        /// **The attacks-all effect is asked about each target.** Its
        /// value doubles as a per-target condition: zero means "not this
        /// one", and the same number is the allowance for the ones it
        /// admits.
        #[test]
        fn attack_all_asks_its_effect_about_each_target() {
            let mut f = field();
            let a = monster(&mut f, 0, 0);
            let x = monster(&mut f, 1, 0);
            let y = monster(&mut f, 1, 1);
            let e = single_valued(&mut f, a, code::ATTACK_ALL, 1);
            // A *function* value, so the condition and the allowance can
            // disagree: zero for the first target (refused outright) and
            // two for the second (admitted, with room for two attacks).
            // A constant cannot separate them, because the reference asks
            // the same function for both.
            if let Some(v) = f.effects.get_mut(e) {
                v.flag[0] |= flag::FUNC_VALUE;
                v.value_fn = Some(|_, _, ctx| if ctx.card == Some(1) { 0 } else { 2 });
            }
            f.cards[a].attack_all_target = true;
            f.cards[a].announce_count = 1;
            let (out, _, _) = targets(&mut f, a);
            assert!(!out.contains(&x), "the condition refused {x:?}");
            assert!(out.contains(&y), "and admitted {y:?}");
        }

        /// **`attack_all_target` gates the whole escape hatch.**
        #[test]
        fn attack_all_is_off_once_the_flag_is_down() {
            let mut f = field();
            let a = monster(&mut f, 0, 0);
            monster(&mut f, 1, 0);
            single_valued(&mut f, a, code::ATTACK_ALL, 1);
            f.cards[a].announce_count = 1;
            f.cards[a].attack_all_target = true;
            let (out, _, _) = targets(&mut f, a);
            assert_eq!(out.len(), 1, "the hatch is open");

            f.cards[a].attack_all_target = false;
            let (out, _, _) = targets(&mut f, a);
            assert!(out.is_empty(), "and shut once the flag is down");
        }

        /// **Only the *opponent's* monsters block a direct attack.** The
        /// count is of targets whose controller is not the attacker's, so
        /// a `SELF_ATTACK` row full of the attacker's own monsters still
        /// leaves the player open.
        #[test]
        fn only_opposing_monsters_block_a_direct_attack() {
            let mut f = field();
            let a = monster(&mut f, 0, 0);
            let own = monster(&mut f, 0, 1);
            player_effect(&mut f, code::SELF_ATTACK, 0);
            let (out, direct, _) = targets(&mut f, a);
            assert!(out.contains(&own), "its own side is attackable");
            assert!(
                direct,
                "and the player is still open, because no opponent is there"
            );
        }

        /// **`EFFECT_SELF_ATTACK` adds the attacker's own side.**
        #[test]
        fn self_attack_adds_the_attackers_own_row() {
            let mut f = field();
            let a = monster(&mut f, 0, 0);
            let own = monster(&mut f, 0, 1);
            player_effect(&mut f, code::SELF_ATTACK, 0);
            let (out, _, _) = targets(&mut f, a);
            assert!(out.contains(&own), "its own side is attackable");
        }

        /// **The attacker never targets itself**, on either side.
        #[test]
        fn the_attacker_is_not_its_own_target() {
            let mut f = field();
            let a = monster(&mut f, 0, 0);
            monster(&mut f, 0, 1);
            player_effect(&mut f, code::SELF_ATTACK, 0);
            f.core.attacker = Some(a);
            let (out, _, _) = targets(&mut f, a);
            assert!(!out.contains(&a));
        }
    }

    mod confirm_attack_target {
        use super::*;

        /// A declared attack against a monster still on the field stands.
        #[test]
        fn a_standing_attack_is_confirmed() {
            let mut f = field();
            let a = monster(&mut f, 0, 0);
            let x = monster(&mut f, 1, 0);
            f.core.attacker = Some(a);
            f.core.attack_target = Some(x);
            assert!(f.confirm_attack_target());
        }

        /// **A target that has left is not.**
        #[test]
        fn a_departed_target_is_not() {
            let mut f = field();
            let a = monster(&mut f, 0, 0);
            let x = monster(&mut f, 1, 0);
            f.core.attacker = Some(a);
            f.core.attack_target = Some(x);
            f.remove_card(x);
            assert!(!f.confirm_attack_target());
        }

        /// **A declared attack against a protected monster still
        /// stands.** `confirm_attack_target` asks with `select_target =
        /// false`, because the "cannot be chosen" prohibitions gate the
        /// *declaration*, not whether one already made holds up.
        #[test]
        fn a_protected_target_is_still_confirmed() {
            let mut f = field();
            let a = monster(&mut f, 0, 0);
            let x = monster(&mut f, 1, 0);
            single(&mut f, x, code::CANNOT_BE_BATTLE_TARGET);
            f.core.attacker = Some(a);
            f.core.attack_target = Some(x);
            // It could never have been *offered*...
            let (out, _, _) = targets(&mut f, a);
            assert!(out.is_empty(), "not offerable");
            // ...but a declared attack against it is confirmed.
            assert!(f.confirm_attack_target());
        }

        /// **A direct attack is confirmed through `direct_attackable`**,
        /// which this call recomputes rather than trusts.
        #[test]
        fn a_direct_attack_is_confirmed_by_recomputing() {
            let mut f = field();
            let a = monster(&mut f, 0, 0);
            f.core.attacker = Some(a);
            f.core.attack_target = None;
            assert!(f.confirm_attack_target(), "an empty row");

            monster(&mut f, 1, 0);
            assert!(
                !f.confirm_attack_target(),
                "a monster arrived, so the direct attack is off"
            );
        }
    }

    mod select_battle_cmd {
        use super::*;

        fn answer(kind: u32, index: u32) -> i32 {
            ((index << 16) | kind) as i32
        }

        fn run(f: &mut Field) -> Status {
            for _ in 0..64 {
                match f.process() {
                    Status::Continue => continue,
                    other => return other,
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

        /// The menu carries the attackable list, and `direct_attackable`
        /// travels with each offer.
        #[test]
        fn the_menu_carries_the_attack_list() {
            let mut f = field();
            let a = monster(&mut f, 0, 0);
            f.cards[a].direct_attackable = true;
            f.core.attackable_cards = vec![a];
            f.core.to_m2 = true;

            f.emplace(Kind::SelectBattleCmd { player: 0 });
            assert_eq!(run(&mut f), Status::Awaiting);
            let Some(Message::SelectBattleCmd {
                attackable,
                to_m2,
                to_ep,
                ..
            }) = menu(&f)
            else {
                panic!("no menu");
            };
            assert_eq!(attackable.len(), 1);
            assert!(attackable[0].direct_attackable);
            assert!(*to_m2);
            assert!(!*to_ep);
        }

        /// **The activate list is sorted inside the question**, by effect
        /// id — the same comparator and the same placement as the idle
        /// menu's, so the index the host replies with means the sorted
        /// order.
        #[test]
        fn the_activate_list_is_sorted() {
            use crate::chain::Chain;
            let mut f = field();
            let a = monster(&mut f, 0, 0);
            let b = monster(&mut f, 0, 1);
            // `b`'s effect is created first, so it sorts first.
            let mut eb = Effect::new(effect_type::ACTIVATE, 0);
            eb.owner = Some(b);
            eb.handler = Some(b);
            eb.description = 111;
            let eb = f.new_effect(eb);
            let mut ea = Effect::new(effect_type::ACTIVATE, 0);
            ea.owner = Some(a);
            ea.handler = Some(a);
            ea.description = 222;
            let ea = f.new_effect(ea);
            for e in [ea, eb] {
                f.core
                    .select_chains
                    .push_back(Chain::new(e, crate::event::Event::new(0)));
            }

            f.emplace(Kind::SelectBattleCmd { player: 0 });
            run(&mut f);
            let Some(Message::SelectBattleCmd { activatable, .. }) = menu(&f) else {
                panic!("no menu");
            };
            assert_eq!(
                activatable
                    .iter()
                    .map(|c| c.description)
                    .collect::<Vec<_>>(),
                vec![111, 222],
                "by effect id, not gather order"
            );
            assert_eq!(
                f.core.select_chains[0].triggering_effect, eb,
                "and the list is left sorted"
            );
        }

        /// Each index is checked against its own list, and kind 4 and above
        /// is refused.
        #[test]
        fn the_answer_is_validated_per_list() {
            let mut f = field();
            let a = monster(&mut f, 0, 0);
            f.core.attackable_cards = vec![a];

            f.emplace(Kind::SelectBattleCmd { player: 0 });
            run(&mut f);
            f.core.returns.set(answer(1, 1));
            assert_eq!(
                run(&mut f),
                Status::Awaiting,
                "one attacker, index 1 is past"
            );
            f.core.returns.set(answer(4, 0));
            assert_eq!(run(&mut f), Status::Awaiting, "kind 4 does not exist");
            f.core.returns.set(answer(0, 0));
            assert_eq!(run(&mut f), Status::Awaiting, "no chains to activate");
            f.core.returns.set(answer(1, 0));
            assert_eq!(run(&mut f), Status::End);
        }

        /// Leaving needs the corresponding flag.
        #[test]
        fn leaving_needs_its_flag() {
            for (kind, set_m2, set_ep) in [(2u32, true, false), (3, false, true)] {
                let mut f = field();
                f.core.to_m2 = set_m2;
                f.core.to_ep = set_ep;
                f.emplace(Kind::SelectBattleCmd { player: 0 });
                run(&mut f);
                f.core.returns.set(answer(kind, 0));
                assert_eq!(run(&mut f), Status::End, "kind {kind} with its flag");

                let mut f = field();
                f.emplace(Kind::SelectBattleCmd { player: 0 });
                run(&mut f);
                f.core.returns.set(answer(kind, 0));
                assert_eq!(run(&mut f), Status::Awaiting, "kind {kind} without it");
            }
        }
    }

    mod attack_disable {
        use super::*;

        fn attacking(f: &mut Field) -> CardId {
            let a = monster(f, 0, 0);
            f.core.attacker = Some(a);
            f.core.pre_field[0] = f.cards[a].fieldid_r;
            a
        }

        /// The ordinary case: the attack is negated, and the answer comes
        /// back through `returns`.
        #[test]
        fn it_negates_and_answers_one() {
            let mut f = field();
            let a = attacking(&mut f);
            f.emplace(Kind::AttackDisable);
            for _ in 0..64 {
                if f.process() != Status::Continue {
                    break;
                }
            }
            assert_eq!(f.core.returns.get(), 1, "negated");
            assert!(f.cards[a].is_status(status::ATTACK_CANCELED));
            assert!(
                f.is_affected_by_effect(a, code::ATTACK_DISABLED).is_some(),
                "and carries the disabling effect"
            );
        }

        /// Run to a stop and report the answer, having first poisoned
        /// `returns` so that "nobody wrote an answer" cannot be mistaken
        /// for "answered 0" — which is what the first version of these
        /// tests did, and why three mutations survived them.
        fn answer_of(f: &mut Field) -> i32 {
            f.core.returns.set(-99);
            for _ in 0..64 {
                if f.process() == Status::Continue {
                    continue;
                }
                break;
            }
            f.core.returns.get()
        }

        /// **With no attacker there is nothing to negate**, and the unit
        /// answers 0 rather than failing.
        #[test]
        fn no_attacker_answers_zero() {
            let mut f = field();
            f.emplace(Kind::AttackDisable);
            assert_eq!(answer_of(&mut f), 0);
        }

        /// **The attacker must be the same instance that declared.** A card
        /// with a different field id from `pre_field[0]` is not it.
        #[test]
        fn a_different_instance_is_not_the_attacker() {
            let mut f = field();
            let a = attacking(&mut f);
            f.core.pre_field[0] = f.cards[a].fieldid_r + 1;
            f.emplace(Kind::AttackDisable);
            assert_eq!(answer_of(&mut f), 0);
            assert!(!f.cards[a].is_status(status::ATTACK_CANCELED));
        }

        /// **`EFFECT_UNSTOPPABLE_ATTACK` refuses**, as does an attacker
        /// that has left the Monster Zone or can no longer attack.
        #[test]
        fn the_three_refusals_on_the_attacker() {
            let mut f = field();
            let a = attacking(&mut f);
            single(&mut f, a, code::UNSTOPPABLE_ATTACK);
            f.emplace(Kind::AttackDisable);
            assert_eq!(answer_of(&mut f), 0, "unstoppable");
            assert!(!f.cards[a].is_status(status::ATTACK_CANCELED));

            let mut f = field();
            let a = attacking(&mut f);
            f.cards[a].current.position = position::FACEDOWN_DEFENSE;
            f.emplace(Kind::AttackDisable);
            assert_eq!(answer_of(&mut f), 0, "cannot attack");
            assert!(!f.cards[a].is_status(status::ATTACK_CANCELED));

            let mut f = field();
            let a = attacking(&mut f);
            f.remove_card(a);
            f.add_card(0, a, location::GRAVE, 0, false);
            // `add_card` hands out a **new** field id, so `pre_field[0]`
            // has to be re-pinned or the instance check fires first and
            // this test never reaches the location check at all.
            f.core.pre_field[0] = f.cards[a].fieldid_r;
            f.emplace(Kind::AttackDisable);
            assert_eq!(answer_of(&mut f), 0, "not in a Monster Zone");
            assert!(!f.cards[a].is_status(status::ATTACK_CANCELED));
        }

        /// **`effect_damage_step` refuses** — an attack cannot be negated
        /// during effect damage calculation.
        #[test]
        fn effect_damage_calculation_refuses() {
            let mut f = field();
            let a = attacking(&mut f);
            f.core.effect_damage_step = 1;
            f.emplace(Kind::AttackDisable);
            assert_eq!(answer_of(&mut f), 0);
            assert!(!f.cards[a].is_status(status::ATTACK_CANCELED));
        }
    }

    mod forced_battle {
        use super::*;

        fn start(f: &mut Field) {
            f.emplace(Kind::ForcedBattle {
                state: Default::default(),
            });
        }

        /// **`EFFECT_CANNOT_BP` refuses outright**, before the phase count
        /// is even bumped.
        #[test]
        fn cannot_bp_refuses_without_counting() {
            let mut f = field();
            player_effect(&mut f, code::CANNOT_BP, 0);
            start(&mut f);
            f.process();
            assert_eq!(f.core.battle_phase_count, [0, 0], "not counted");
        }

        /// **`EFFECT_SKIP_BP` counts the phase and then skips it**, which
        /// is the same ordering `Turn`'s case 10 has.
        #[test]
        fn skip_bp_counts_then_skips() {
            let mut f = field();
            f.infos.phase = crate::duel::phases::MAIN1;
            player_effect(&mut f, code::SKIP_BP, 0);
            start(&mut f);
            for _ in 0..64 {
                if f.process() != Status::Continue {
                    break;
                }
            }
            assert_eq!(f.core.battle_phase_count[0], 1, "counted");
            assert_eq!(f.infos.phase, crate::duel::phases::MAIN1, "and not entered");
        }

        /// **`EFFECT_SKIP_BP` refuses even with a legal attack ready.**
        /// Without the skip the same position reaches the unported battle
        /// step, so the refusal is what the test is about.
        /// **`EFFECT_SKIP_BP` takes the *announcing* branch**, which is
        /// not the same as merely refusing.
        ///
        /// The attacker would be refused either way: `is_capable_attack`
        /// reads `SKIP_BP` too, so the announce check below would decline
        /// it. What distinguishes the skip branch is that it **announces**
        /// — a `MSG_NEW_PHASE` for the Battle Phase's start and another
        /// for the phase it returns to — so the players see the Battle
        /// Phase entered and skipped rather than nothing happening. The
        /// messages are the only observable difference.
        #[test]
        fn skip_bp_announces_the_phase_it_skips() {
            let mut f = field();
            f.infos.phase = crate::duel::phases::MAIN1;
            let a = monster(&mut f, 0, 0);
            f.core.forced_attacker = Some(a);
            player_effect(&mut f, code::SKIP_BP, 0);
            start(&mut f);
            for _ in 0..64 {
                if f.process() != Status::Continue {
                    break;
                }
            }
            assert!(f.core.attacker.is_none(), "no attack was set up");
            assert_eq!(f.infos.phase, crate::duel::phases::MAIN1);
            let phases: Vec<u16> = f
                .messages
                .iter()
                .filter_map(|m| match m {
                    Message::NewPhase { phase } => Some(*phase),
                    _ => None,
                })
                .collect();
            assert_eq!(
                phases,
                vec![
                    crate::duel::phases::BATTLE_START,
                    crate::duel::phases::MAIN1
                ],
                "entered and skipped, announced both ways"
            );
        }

        /// **An attacker that cannot announce refuses**, leaving the phase
        /// alone.
        #[test]
        fn an_attacker_that_cannot_announce_refuses() {
            let mut f = field();
            f.infos.phase = crate::duel::phases::MAIN1;
            let a = monster(&mut f, 0, 0);
            single(&mut f, a, code::CANNOT_ATTACK_ANNOUNCE);
            f.core.forced_attacker = Some(a);
            start(&mut f);
            f.process();
            assert_eq!(f.infos.phase, crate::duel::phases::MAIN1);
            assert!(f.core.attacker.is_none());
        }

        /// **A named target that is not among the legal ones refuses** —
        /// and the test needs *another* legal target present, or the
        /// "no targets at all" refusal masks this one.
        #[test]
        fn an_illegal_named_target_refuses() {
            let mut f = field();
            f.infos.phase = crate::duel::phases::MAIN1;
            let a = monster(&mut f, 0, 0);
            let x = monster(&mut f, 1, 0);
            let y = monster(&mut f, 1, 1);
            single(&mut f, x, code::CANNOT_BE_BATTLE_TARGET);
            f.core.forced_attacker = Some(a);
            f.core.forced_attack_target = Some(x);
            start(&mut f);
            f.process();
            assert!(
                f.core.attacker.is_none(),
                "the named target is not legal, though {y:?} is"
            );
        }

        /// **No legal target at all refuses**, separately from the named
        /// target being wrong.
        #[test]
        fn no_legal_target_refuses() {
            let mut f = field();
            f.infos.phase = crate::duel::phases::MAIN1;
            let a = monster(&mut f, 0, 0);
            let x = monster(&mut f, 1, 0);
            single(&mut f, x, code::CANNOT_BE_BATTLE_TARGET);
            // No named target, and the only monster is protected — so the
            // list is empty and there is no direct attack either.
            f.core.forced_attacker = Some(a);
            f.core.forced_attack_target = None;
            start(&mut f);
            f.process();
            assert!(f.core.attacker.is_none());
        }

        /// **The whole fake Battle Phase**: the phase is saved, the
        /// tallies cleared, and the Battle Step entered — emplacing
        /// `BattleCommand` at **step 1**, past the menu, with
        /// `forced_attack` set.
        #[test]
        fn it_enters_a_battle_step_and_hands_off() {
            let mut f = field();
            f.infos.phase = crate::duel::phases::MAIN1;
            let a = monster(&mut f, 0, 0);
            f.core.forced_attacker = Some(a);
            start(&mut f);
            f.process();
            let handed = f
                .core
                .units
                .iter()
                .chain(f.core.subunits.iter())
                .find_map(|u| match &u.kind {
                    crate::processor::Kind::BattleCommand { state } => {
                        Some((u.step, state.forced_attack))
                    }
                    _ => None,
                });
            assert_eq!(
                handed,
                Some((1, true)),
                "at step 1, and marked as a forced attack"
            );
        }

        /// The state it leaves behind before reaching the panic.
        #[test]
        fn it_sets_the_attack_up_before_handing_off() {
            let mut f = field();
            f.infos.phase = crate::duel::phases::MAIN1;
            let a = monster(&mut f, 0, 0);
            // The tallies belong to *another* monster: setting them on the
            // attacker would make it fail the legality check above, which
            // reads `announce_count` before anything is cleared.
            let other = monster(&mut f, 0, 1);
            f.cards[other].announce_count = 3;
            f.core.forced_attacker = Some(a);
            f.core.chain_attacker_id = 9;
            start(&mut f);
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                f.process();
            }));
            assert_eq!(f.core.attacker, Some(a));
            assert_eq!(f.infos.phase, crate::duel::phases::BATTLE_STEP);
            assert_eq!(
                f.cards[other].announce_count, 0,
                "tallies cleared on the way in"
            );
            assert!(f.core.attack_cancelable);
            assert!(!f.core.attack_cost_paid);
            assert_eq!(f.core.chain_attacker_id, 0);
            assert_eq!(f.core.returns.get(), 1);
        }

        /// **Case 1 puts the phase back** and clears the tallies a second
        /// time, so a forced attack leaves nothing behind.
        #[test]
        fn case_1_restores_the_phase_and_clears_again() {
            let mut f = field();
            let a = monster(&mut f, 0, 0);
            f.cards[a].announce_count = 2;
            f.core.attacker = Some(a);
            f.core.attack_target = Some(a);
            f.infos.phase = crate::duel::phases::BATTLE_STEP;
            f.emplace_at(
                Kind::ForcedBattle {
                    state: ForcedBattleState {
                        backup_phase: crate::duel::phases::MAIN1,
                    },
                },
                1,
            );
            f.process();
            assert_eq!(f.infos.phase, crate::duel::phases::MAIN1, "restored");
            assert!(f.core.attacker.is_none());
            assert!(f.core.attack_target.is_none());
            assert_eq!(f.cards[a].announce_count, 0, "cleared on the way out too");
        }
    }
}

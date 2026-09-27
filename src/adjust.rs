//! `Adjust` — everything the rules do between one action and the next.
//!
//! The pass that runs after every board change, and the keystone of the
//! subsystem: `adjust_all` emplaces it, and until now every machine's tests
//! have had to drop the unit rather than run it.
//!
//! Thirteen passes over the board in a fixed order, and **the order is the
//! rule**. A win is checked before anything else, because a duel that is over
//! does not adjust; positions are forced after control changes, because
//! whose monster it is decides what forces it.
//!
//! ## It runs again if anything changed
//!
//! `core.re_adjust` is set by any pass that *did* something, and case 16
//! restarts the whole unit when it is. The loop terminates because each pass
//! that sets the flag has also consumed something finite — a card destroyed,
//! a position forced, a control change made.
//!
//! ## What is not ported, and why each is unreachable
//!
//! - **The attack check** (case 14) needs the battle system. Its own first
//!   line is "no attacker, nothing to do", and with no battle system there is
//!   never an attacker — so the branch beyond it is a named panic that
//!   nothing can reach.
//! - **Granted effects** (case 8) need the `EFFECT_TYPE_GRANT` registry,
//!   which `add_effect` does not yet build. With nothing registered the pass
//!   has nothing to walk.
//! - **`DUEL_RELAY`** (case 1) is not in this project's configuration.

use crate::board::{location, position};
use crate::card::{card_type, reason, status};
use crate::event::{code, CardId, PLAYER_NONE};
use crate::field::{global_flag, Field, Message};
use crate::processor::{Kind, RESTART};
use std::collections::BTreeSet;

impl Field {
    /// One step of `Adjust`.
    pub(crate) fn adjust_step(&mut self, step: u16) -> bool {
        match step {
            0 => {
                self.core.re_adjust = false;
                false
            }
            1 => self.adjust_win_check(),
            2 => self.adjust_disable_check(),
            3 => self.adjust_trap_monsters(),
            4 => self.adjust_control(),
            5 => self.adjust_brainwashing(),
            8 => {
                if self.adjust_grant_effect() {
                    self.core.re_adjust = true;
                }
                false
            }
            9 => {
                if self.core.selfdes_disabled {
                    self.set_step(10);
                    return false;
                }
                self.adjust_self_destroy_set();
                false
            }
            10 => self.adjust_equips(),
            11 => self.adjust_positions(),
            12 => self.adjust_hands(),
            13 => self.adjust_reverse_deck(),
            14 => self.adjust_attack(),
            15 => {
                self.raise_event(None, code::ADJUST, None, 0, PLAYER_NONE, PLAYER_NONE, 0);
                self.process_instant_event();
                false
            }
            16 => self.adjust_finish(),
            _ => true,
        }
    }

    /// Case 1: has anybody lost?
    ///
    /// Two ways to lose — life points at zero, and having tried to draw from
    /// an empty deck — each waivable by its own effect, and each checked for
    /// both players in a fixed order that ends with the **both at once**
    /// cases. Those two are why the earlier assignments are not returns: a
    /// later condition overwrites an earlier one, and "both players are out"
    /// must win over "player 0 is out".
    ///
    /// `winp == 5` means nobody, and is the sentinel the whole function is
    /// written around. A win already recorded by an effect is announced only
    /// if the board has not produced one of its own.
    pub(crate) fn adjust_win_check(&mut self) -> bool {
        if self.core.force_turn_end {
            return false;
        }
        let (mut winp, mut rea) = (5u8, 1u8);
        let lp = [self.players[0].lp, self.players[1].lp];
        let over = self.core.overdraw;
        let keeps_lp = [
            self.is_player_affected_by_effect(0, code::CANNOT_LOSE_LP)
                .is_some(),
            self.is_player_affected_by_effect(1, code::CANNOT_LOSE_LP)
                .is_some(),
        ];
        let keeps_deck = [
            self.is_player_affected_by_effect(0, code::CANNOT_LOSE_DECK)
                .is_some(),
            self.is_player_affected_by_effect(1, code::CANNOT_LOSE_DECK)
                .is_some(),
        ];

        if lp[0] <= 0 && lp[1] > 0 && !keeps_lp[0] {
            (winp, rea) = (1, 1);
        }
        if over[0] && !over[1] && !keeps_deck[0] {
            (winp, rea) = (1, 2);
        }
        if lp[1] <= 0 && lp[0] > 0 && !keeps_lp[1] {
            (winp, rea) = (0, 1);
        }
        if over[1] && !over[0] && !keeps_deck[1] {
            (winp, rea) = (0, 2);
        }
        if lp[1] <= 0 && lp[0] <= 0 && !(keeps_lp[0] && keeps_lp[1]) {
            winp = if keeps_lp[0] {
                0
            } else if keeps_lp[1] {
                1
            } else {
                PLAYER_NONE
            };
            rea = 1;
        }
        if over[0] && over[1] && !(keeps_deck[0] && keeps_deck[1]) {
            winp = if keeps_deck[0] {
                0
            } else if keeps_deck[1] {
                1
            } else {
                PLAYER_NONE
            };
            rea = 2;
        }
        if self.is_flag(crate::duel::flags::RELAY) {
            return self.adjust_relay_unported();
        }

        if winp != 5 {
            self.messages.push(Message::Win {
                player: winp,
                reason: rea,
            });
            self.core.overdraw = [false, false];
            self.core.win_player = 5;
            self.core.win_reason = 0;
        } else if self.core.win_player != 5 {
            self.messages.push(Message::Win {
                player: self.core.win_player,
                reason: self.core.win_reason,
            });
            self.core.win_player = 5;
            self.core.win_reason = 0;
            self.core.overdraw = [false, false];
        }
        false
    }

    /// Not ported: `DUEL_RELAY`, which this project's configuration does not
    /// set. It rewrites the winner when a relay partner is still standing.
    fn adjust_relay_unported(&mut self) -> bool {
        unimplemented!("the relay win check needs DUEL_RELAY's partner rules")
    }

    /// Case 2: ask every card on the field whether it is disabled, then
    /// recompute the unusable seats.
    ///
    /// **The turn player's cards are queued first**, which decides the order
    /// the disable checks resolve in.
    fn adjust_disable_check(&mut self) -> bool {
        let mut p = self.infos.turn_player;
        for _ in 0..2 {
            for seat in 0..self.players[p as usize].mzone.len() {
                if let Some(card) = self.players[p as usize].mzone[seat] {
                    self.add_to_disable_check_list(card);
                }
            }
            for seat in 0..self.players[p as usize].szone.len() {
                if let Some(card) = self.players[p as usize].szone[seat] {
                    self.add_to_disable_check_list(card);
                }
            }
            p = 1 - p;
        }
        self.adjust_disable_check_list();
        self.emplace(Kind::RefreshLoc {
            state: Default::default(),
        });
        false
    }

    /// Case 3: the trap monsters that have stopped being monsters.
    fn adjust_trap_monsters(&mut self) -> bool {
        if self.gather_trap_monster_adjust() {
            self.core.re_adjust = true;
            self.emplace(Kind::TrapMonsterAdjust {
                state: Box::default(),
            });
        }
        false
    }

    /// Case 4: the cards whose controller no longer matches what the effects
    /// say.
    ///
    /// **The two `get_control` calls are crossed**: the set gathered on the
    /// *non*-turn player's side goes to the turn player, and vice versa. A
    /// card is in `control_adjust_set[p]` because it is *sitting* on p's side
    /// while belonging to the other.
    ///
    /// `readjust_map` is the loop-breaker. A card whose effect keeps
    /// demanding a control change it cannot complete is counted, and on the
    /// fourth attempt destroyed — which is the rules' answer to two effects
    /// fighting over one monster.
    ///
    /// **The fourth attempt is not reachable with what is ported.** A demand
    /// that can be satisfied settles in one round, because `set_control`
    /// registers a *later* `EFFECT_SET_CONTROL` and `refresh_control_status`
    /// prefers the last one; a demand that cannot is answered by `GetControl`
    /// destroying the card for want of a seat. What is left — `set_control`
    /// declining because `EFFECT_REMOVE_BRAINWASHING` applies — needs the
    /// brainwashing pass and the `last_control_changed_id` interaction
    /// together, and is left for when something in the card pool actually
    /// does it. Recorded rather than tested with a scenario this port cannot
    /// yet claim is faithful.
    fn adjust_control(&mut self) -> bool {
        self.core.control_adjust_set[0].clear();
        self.core.control_adjust_set[1].clear();
        let mut reason_cards: BTreeSet<CardId> = BTreeSet::new();
        for p in 0..2usize {
            for seat in 0..self.players[p].mzone.len() {
                let Some(card) = self.players[p].mzone[seat] else {
                    continue;
                };
                let current = self.cards[card].current.controller;
                let (should, by) = self.refresh_control_status(card);
                if current != should && self.is_capable_change_control(card) {
                    self.core.control_adjust_set[p].insert(card);
                    let counts = by.is_some_and(|e| {
                        self.effects.get(e).is_some_and(|x| {
                            !x.is_type(crate::effect::effect_type::SINGLE) || x.condition.is_some()
                        })
                    });
                    if counts {
                        if let Some(handler) =
                            by.and_then(|e| self.effects.get(e)).and_then(|e| e.handler)
                        {
                            reason_cards.insert(handler);
                        }
                    }
                }
            }
        }
        if !self.core.control_adjust_set[0].is_empty()
            || !self.core.control_adjust_set[1].is_empty()
        {
            self.core.re_adjust = true;
            self.cross_control();
            for card in reason_cards {
                let n = self.core.readjust_map.entry(card).or_insert(0);
                *n += 1;
                if *n > 3 {
                    self.destroy([card], None, reason::RULE, PLAYER_NONE, PLAYER_NONE, 0, 0);
                }
            }
        }
        self.core.last_control_changed_id = self.infos.field_id.get();
        false
    }

    /// The two crossed `get_control` calls both cases 4 and 5 make.
    fn cross_control(&mut self) {
        let tp = self.infos.turn_player;
        let theirs: Vec<CardId> = self.core.control_adjust_set[(1 - tp) as usize]
            .iter()
            .copied()
            .collect();
        let mine: Vec<CardId> = self.core.control_adjust_set[tp as usize]
            .iter()
            .copied()
            .collect();
        self.get_control(theirs, None, PLAYER_NONE, tp, 0, 0, 0xff);
        self.get_control(mine, None, PLAYER_NONE, 1 - tp, 0, 0, 0xff);
    }

    /// Case 5: `EFFECT_REMOVE_BRAINWASHING` — give every stolen monster back.
    ///
    /// **The condition-less `EFFECT_SET_CONTROL` effects are removed**, which
    /// is the reference's own comment's "opposite of `check_control_effect`":
    /// a control change made unconditionally is undone, and one made by an
    /// effect that keeps asking is left to keep asking.
    fn adjust_brainwashing(&mut self) -> bool {
        if self.core.global_flag & global_flag::BRAINWASHING_CHECK == 0 {
            self.set_step(7);
            return false;
        }
        self.core.control_adjust_set[0].clear();
        self.core.control_adjust_set[1].clear();
        let any = !self
            .filter_field_effect(code::REMOVE_BRAINWASHING)
            .is_empty();
        self.core.remove_brainwashing = any;
        if any {
            for p in 0..2usize {
                for seat in 0..self.players[p].mzone.len() {
                    let Some(card) = self.players[p].mzone[seat] else {
                        continue;
                    };
                    if self
                        .is_affected_by_effect(card, code::REMOVE_BRAINWASHING)
                        .is_none()
                    {
                        continue;
                    }
                    for e in self.cards[card]
                        .single_effect
                        .equal_range(code::SET_CONTROL)
                        .to_vec()
                    {
                        if self.effects.get(e).is_some_and(|x| x.condition.is_none()) {
                            self.remove_effect_from_wherever(e);
                        }
                    }
                    if p as u8 != self.cards[card].owner && self.is_capable_change_control(card) {
                        self.core.control_adjust_set[p].insert(card);
                    }
                }
            }
        }
        if !self.core.control_adjust_set[0].is_empty()
            || !self.core.control_adjust_set[1].is_empty()
        {
            self.core.re_adjust = true;
            self.cross_control();
        }
        self.set_step(7);
        false
    }

    /// Not ported: the `EFFECT_TYPE_GRANT` registry, which `add_effect` does
    /// not build yet. With nothing registered there is nothing to walk, so
    /// this answers "nothing changed".
    ///
    /// When `add_effect` grows its GRANT branch, this has to grow with it —
    /// which is the same shape of gap the port has hit four times and the
    /// reason it is named rather than inlined as a `false`.
    pub fn adjust_grant_effect(&mut self) -> bool {
        false
    }

    /// Case 10: an equip card whose target no longer satisfies its limit is
    /// destroyed.
    ///
    /// The limit is asked **about the target**, not about the equip card:
    /// `is_affected_by_effect(EFFECT_EQUIP_LIMIT, equiping_target)`.
    fn adjust_equips(&mut self) -> bool {
        let mut doomed: BTreeSet<CardId> = BTreeSet::new();
        let mut p = self.infos.turn_player;
        for _ in 0..2 {
            for seat in 0..5.min(self.players[p as usize].szone.len()) {
                let Some(card) = self.players[p as usize].szone[seat] else {
                    continue;
                };
                let Some(target) = self.cards[card].equiping_target else {
                    continue;
                };
                if self
                    .is_affected_by_effect_against(card, code::EQUIP_LIMIT, target)
                    .is_none()
                {
                    doomed.insert(card);
                }
            }
            p = 1 - p;
        }
        if !doomed.is_empty() {
            self.core.re_adjust = true;
            self.destroy(doomed, None, reason::RULE, PLAYER_NONE, PLAYER_NONE, 0, 0);
        }
        false
    }

    /// Case 11: the positions an effect forces.
    ///
    /// **The last `EFFECT_SET_POSITION` wins** — `eset.back()` — rather than
    /// the strongest or the first.
    ///
    /// `STATUS_CONTINUOUS_POS` records whether the card had *just* changed
    /// position when the force was applied, which is how a position that is
    /// being held differs from one that is being changed. `STATUS_JUST_POS`
    /// is cleared either way, and only for cards a force applies to.
    fn adjust_positions(&mut self) -> bool {
        let mut pos_adjust: BTreeSet<CardId> = BTreeSet::new();
        let mut p = self.infos.turn_player;
        for _ in 0..2 {
            for seat in 0..self.players[p as usize].mzone.len() {
                let Some(card) = self.players[p as usize].mzone[seat] else {
                    continue;
                };
                let ty = self.cards[card].data.type_;
                if ty & card_type::LINK != 0 && ty & card_type::MONSTER != 0 {
                    continue;
                }
                if self
                    .is_affected_by_effect(card, code::CANNOT_CHANGE_POS_E)
                    .is_some()
                {
                    continue;
                }
                let forcing = self.filter_effect(card, code::SET_POSITION);
                let Some(&last) = forcing.last() else {
                    continue;
                };
                let pos = self.effect_value_for_card_pub(last, card) as u32;
                if (pos & 0xff) as u8 != self.cards[card].current.position {
                    pos_adjust.insert(card);
                    self.cards[card].position_param = pos;
                    let just = self.cards[card].is_status(status::JUST_POS);
                    self.cards[card].set_status(status::CONTINUOUS_POS, just);
                } else {
                    self.cards[card].set_status(status::CONTINUOUS_POS, false);
                }
                self.cards[card].set_status(status::JUST_POS, false);
            }
            p = 1 - p;
        }
        if !pos_adjust.is_empty() {
            self.core.re_adjust = true;
            let group = self.new_group(pos_adjust);
            self.emplace(Kind::ChangePos {
                targets: group,
                reason_effect: None,
                reason_player: PLAYER_NONE,
                enable: true,
                state: Box::default(),
            });
        }
        false
    }

    /// Case 12: hands are face-down unless something makes them public, and
    /// a hand that *was* face-up is shuffled.
    ///
    /// The shuffle is because the opponent has just seen the order.
    fn adjust_hands(&mut self) -> bool {
        for p in 0..2usize {
            for i in 0..self.players[p].hand.len() {
                let card = self.players[p].hand[i];
                let public = self.is_affected_by_effect(card, code::PUBLIC).is_some();
                if !public && self.cards[card].current.is_faceup() {
                    self.core.shuffle_hand_check[p] = true;
                }
                self.cards[card].current.position = if public {
                    position::FACEUP
                } else {
                    position::FACEDOWN
                };
            }
        }
        let tp = self.infos.turn_player;
        for p in [tp, 1 - tp] {
            if self.core.shuffle_hand_check[p as usize] {
                self.shuffle(p, location::HAND);
            }
        }
        false
    }

    /// Case 13: `EFFECT_REVERSE_DECK` turns both decks over, once, when the
    /// answer changes.
    fn adjust_reverse_deck(&mut self) -> bool {
        if self.core.global_flag & global_flag::DECK_REVERSE_CHECK == 0 {
            return false;
        }
        let reversed = !self.filter_field_effect(code::REVERSE_DECK).is_empty();
        if self.core.deck_reversed == reversed {
            return false;
        }
        self.core.deck_reversed = reversed;
        self.reverse_deck(0);
        self.reverse_deck(1);
        self.messages.push(Message::ReverseDeck);
        if reversed {
            for p in 0..2usize {
                if let Some(&top) = self.players[p].main.last() {
                    self.messages.push(Message::DeckTop {
                        player: p as u8,
                        sequence: 0,
                        code: self.cards[top].data.code,
                        position: self.cards[top].current.position,
                    });
                }
            }
        }
        false
    }

    /// `field::reverse_deck` — turn one deck over, sequences and all.
    pub fn reverse_deck(&mut self, playerid: u8) {
        let p = playerid as usize;
        let count = self.players[p].main.len();
        if count == 0 {
            return;
        }
        self.players[p].main.reverse();
        for i in 0..count {
            let card = self.players[p].main[i];
            self.cards[card].current.sequence = i as u32;
        }
    }

    /// Case 14: is the attack still legal? Run on every adjust pass while
    /// an attack is in progress.
    ///
    /// **Two completely different checks, chosen by phase.** Outside the
    /// damage step the attack can be *rolled back* — offered again, because
    /// the board it was declared against has changed. Inside it, there is
    /// no replay: anything wrong simply cancels the attack.
    ///
    /// `EFFECT_UNSTOPPABLE_ATTACK` is read **first and for one purpose
    /// only**: it stops an already-cancelled attack from returning early,
    /// so the checks below still run on it.
    fn adjust_attack(&mut self) -> bool {
        let Some(attacker) = self.core.attacker else {
            return false;
        };
        if self
            .is_affected_by_effect(attacker, code::UNSTOPPABLE_ATTACK)
            .is_none()
            && self.cards[attacker].is_status(status::ATTACK_CANCELED)
        {
            return false;
        }

        let in_damage_step = self.infos.phase == crate::duel::phases::DAMAGE
            || self.infos.phase == crate::duel::phases::DAMAGE_CAL;
        if !in_damage_step {
            // **Control changing cancels the attack.** `attack_controler`
            // was pinned at declaration, and a monster that changed hands
            // is not the card that attacked.
            if !self.is_capable_attack(attacker)
                || self.cards[attacker].current.controller != self.cards[attacker].attack_controler
                || self.cards[attacker].fieldid_r != self.core.pre_field[0]
            {
                self.cards[attacker].set_status(status::ATTACK_CANCELED, true);
                return false;
            }
            if self.core.attack_rollback {
                return false;
            }
            // The opposing board, compared against what it was at
            // declaration. **A different set means a replay**, even if the
            // declared target is still there — a new monster appearing is
            // enough.
            let now: std::collections::BTreeSet<u32> = self.players
                [1 - self.infos.turn_player as usize]
                .mzone
                .iter()
                .flatten()
                .map(|&c| self.cards[c].fieldid_r)
                .collect();
            if now != self.core.opp_mzone || !self.confirm_attack_target() {
                self.core.attack_rollback = true;
            }
            return false;
        }

        // Inside the damage step: five ways the attack stops being legal,
        // and all of them simply cancel it.
        let target = self.core.attack_target;
        let bad_attacker = self.cards[attacker].current.location != location::MZONE
            || self.cards[attacker].fieldid_r != self.core.pre_field[0]
            || (self.cards[attacker].current.position & position::DEFENSE != 0
                && self
                    .is_affected_by_effect(attacker, code::DEFENSE_ATTACK)
                    .is_none())
            || self.cards[attacker].current.controller != self.cards[attacker].attack_controler;
        let bad_target = target.is_some_and(|t| {
            self.cards[t].current.location != location::MZONE
                || self.cards[t].current.controller != self.cards[t].attack_controler
                || self.cards[t].fieldid_r != self.core.pre_field[1]
        });
        if bad_attacker || bad_target {
            self.cards[attacker].set_status(status::ATTACK_CANCELED, true);
        }
        false
    }

    /// Case 16: go round again if anything changed, otherwise shuffle what
    /// is owed and stop.
    fn adjust_finish(&mut self) -> bool {
        if self.core.re_adjust {
            self.set_step(RESTART);
            return false;
        }
        for p in 0..2u8 {
            if self.core.shuffle_hand_check[p as usize] {
                self.shuffle(p, location::HAND);
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    /// **`break_effect` announces the win itself.** The reference's
    /// `break_effect` ends with the same LP/deck-out block as `Adjust`, so
    /// a player who drew from an empty deck loses at the next break — before
    /// the chain that break opens writes `MSG_CHAINED`, not after.
    #[test]
    fn a_break_announces_a_deck_out() {
        use crate::field::{Field, Message};
        let mut f = Field::new(8000);
        f.core.overdraw[1] = true;
        f.break_effect(false);
        assert!(
            f.messages.iter().any(|m| matches!(
                m,
                Message::Win {
                    player: 0,
                    reason: 2
                }
            )),
            "player 1 decked out, so player 0 wins by reason 2, announced by the break"
        );
        assert_eq!(f.core.overdraw, [false, false], "and the flag is consumed");
        let before = f.messages.len();
        f.break_effect(false);
        assert_eq!(
            f.messages.len(),
            before,
            "a second break has nothing to announce"
        );
    }

    use super::*;
    use crate::card::{Card, CardData};
    use crate::effect::{effect_type, flag, Effect};
    use crate::event::EffectId;
    use crate::processor::Status;

    fn monster(f: &mut Field, player: u8, seat: u32) -> CardId {
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

    /// Run the adjust pass, answering the questions it can raise.
    fn run(f: &mut Field) -> Status {
        for _ in 0..4096 {
            match f.process() {
                Status::Continue => continue,
                Status::Awaiting => match f.messages.last() {
                    Some(Message::SelectPlace { player, .. }) => {
                        let p = *player as usize;
                        let seat = (0..5)
                            .find(|&i| f.players[p].mzone[i].is_none())
                            .unwrap_or(0) as i8;
                        f.core.returns.set_i8(0, i8::from(p != 0));
                        f.core.returns.set_i8(1, location::MZONE as i8);
                        f.core.returns.set_i8(2, seat);
                    }
                    Some(Message::SelectChain { .. }) => f.core.returns.set(-1),
                    Some(Message::SelectEffectYesNo { .. }) => f.core.returns.set(0),
                    _ => return Status::Awaiting,
                },
                other => return other,
            }
        }
        panic!("did not settle");
    }

    fn adjust(f: &mut Field) {
        f.emplace(Kind::Adjust);
    }

    /// An untouched board adjusts to nothing and stops.
    #[test]
    fn a_quiet_board_settles() {
        let mut f = Field::new(8000);
        monster(&mut f, 0, 0);
        adjust(&mut f);
        assert_eq!(run(&mut f), Status::End);
        assert!(!f.core.re_adjust);
    }

    mod the_win_check {
        use super::*;

        #[test]
        fn zero_life_points_loses() {
            let mut f = Field::new(8000);
            f.players[0].lp = 0;
            adjust(&mut f);
            run(&mut f);
            assert!(f.messages.iter().any(|m| matches!(
                m,
                Message::Win {
                    player: 1,
                    reason: 1
                }
            )));
        }

        /// **Both at zero is a draw**, and the ordering is what makes it one:
        /// the both-players case is checked *after* the single-player ones
        /// and overwrites them.
        #[test]
        fn both_at_zero_is_a_draw() {
            let mut f = Field::new(8000);
            f.players[0].lp = 0;
            f.players[1].lp = 0;
            adjust(&mut f);
            run(&mut f);
            assert!(
                f.messages.iter().any(|m| matches!(
                    m,
                    Message::Win {
                        player: PLAYER_NONE,
                        reason: 1
                    }
                )),
                "nobody won"
            );
        }

        /// An effect that says a player cannot lose by life points is read
        /// for **that** player, and decides the draw case too.
        #[test]
        fn an_effect_can_hold_a_player_up() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            let mut e = Effect::new(effect_type::FIELD, code::CANNOT_LOSE_LP);
            e.owner = Some(c);
            e.handler = Some(c);
            e.flag[0] = flag::PLAYER_TARGET | flag::ABSOLUTE_TARGET;
            e.range = u16::from(location::MZONE);
            e.s_range = 1;
            let id = f.new_effect(e);
            f.add_effect(id, 0);

            f.players[0].lp = 0;
            f.players[1].lp = 0;
            adjust(&mut f);
            run(&mut f);
            assert!(
                f.messages.iter().any(|m| matches!(
                    m,
                    Message::Win {
                        player: 0,
                        reason: 1
                    }
                )),
                "the one who cannot lose wins"
            );
        }

        /// Drawing from an empty deck is the other way to lose, and reports
        /// its own reason.
        #[test]
        fn an_empty_deck_loses_with_its_own_reason() {
            let mut f = Field::new(8000);
            f.core.overdraw = [true, false];
            adjust(&mut f);
            run(&mut f);
            assert!(f.messages.iter().any(|m| matches!(
                m,
                Message::Win {
                    player: 1,
                    reason: 2
                }
            )));
            assert_eq!(f.core.overdraw, [false, false], "and the flag is cleared");
        }

        /// A win an effect already recorded is announced when the board has
        /// produced none of its own — and cleared, so it is announced once.
        #[test]
        fn an_effects_win_is_announced_once() {
            let mut f = Field::new(8000);
            f.core.win_player = 1;
            f.core.win_reason = 9;
            adjust(&mut f);
            run(&mut f);
            assert!(f.messages.iter().any(|m| matches!(
                m,
                Message::Win {
                    player: 1,
                    reason: 9
                }
            )));
            assert_eq!(f.core.win_player, 5);
        }

        /// **A forced turn end suspends the check.** The duel is being wound
        /// up, not lost.
        #[test]
        fn a_forced_turn_end_suspends_the_check() {
            let mut f = Field::new(8000);
            f.players[0].lp = 0;
            f.core.force_turn_end = true;
            adjust(&mut f);
            run(&mut f);
            assert!(!f.messages.iter().any(|m| matches!(m, Message::Win { .. })));
        }
    }

    mod the_passes {
        use super::*;

        /// The seats are recomputed — `RefreshLoc` runs as part of the pass.
        #[test]
        fn the_unusable_seats_are_recomputed() {
            let mut f = Field::new(8000);
            f.players[0].disabled_location = 0x1f;
            adjust(&mut f);
            run(&mut f);
            assert_eq!(f.players[0].disabled_location, 0, "rebuilt from nothing");
        }

        /// An equip card whose target no longer satisfies its limit is
        /// destroyed, and the pass asks for another round.
        #[test]
        fn an_equip_that_lost_its_limit_is_destroyed() {
            let mut f = Field::new(8000);
            let target = monster(&mut f, 0, 0);
            let mut c = Card::with_data(
                CardData {
                    code: 70828912,
                    type_: card_type::SPELL | card_type::EQUIP,
                    ..Default::default()
                },
                0,
            );
            c.current.controller = 0;
            let equip = f.new_card(c);
            f.add_card(0, equip, location::SZONE, 0, false);
            f.cards[equip].current.position = position::FACEUP_ATTACK;
            f.cards[equip].equiping_target = Some(target);

            adjust(&mut f);
            run(&mut f);
            assert_eq!(
                f.cards[equip].current.location,
                location::GRAVE,
                "no limit says it may stay"
            );
        }

        /// **The last `EFFECT_SET_POSITION` wins**, and the card is turned.
        #[test]
        fn a_forced_position_is_applied() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            single(
                &mut f,
                c,
                code::SET_POSITION,
                i64::from(position::FACEUP_ATTACK),
            );
            single(
                &mut f,
                c,
                code::SET_POSITION,
                i64::from(position::FACEUP_DEFENSE),
            );
            adjust(&mut f);
            run(&mut f);
            assert_eq!(
                f.cards[c].current.position,
                position::FACEUP_DEFENSE,
                "the last one decides"
            );
        }

        /// A Link Monster is passed over by the position pass, because it has
        /// no defence position to be put into.
        #[test]
        fn a_link_monster_is_not_repositioned() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            f.cards[c].data.type_ = card_type::MONSTER | card_type::LINK;
            single(
                &mut f,
                c,
                code::SET_POSITION,
                i64::from(position::FACEUP_DEFENSE),
            );
            adjust(&mut f);
            run(&mut f);
            assert_eq!(f.cards[c].current.position, position::FACEUP_ATTACK);
        }

        /// **A hand is face-down unless an effect makes it public**, and a
        /// hand that was face-up is shuffled because it has been seen.
        #[test]
        fn hands_are_turned_down_and_shuffled() {
            let mut f = Field::new(8000);
            let a = in_hand(&mut f, 0, 0);
            let b = in_hand(&mut f, 0, 1);
            f.cards[a].current.position = position::FACEUP;
            f.cards[b].current.position = position::FACEUP;
            adjust(&mut f);
            run(&mut f);
            assert_eq!(f.cards[a].current.position, position::FACEDOWN);
            assert_eq!(f.cards[b].current.position, position::FACEDOWN);
        }

        /// `EFFECT_PUBLIC` keeps a card face-up, and it is **not** counted
        /// as something the opponent has just seen — so it does not cost a
        /// shuffle. The card starts face-up, which is what makes the two
        /// halves of that separable.
        #[test]
        fn a_public_card_stays_face_up() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f, 0, 0);
            f.cards[c].current.position = position::FACEUP;
            single(&mut f, c, code::PUBLIC, 0);
            // A second, private card, so the hand is not *all* face-up —
            // otherwise `shuffle` returns early and a hand that should have
            // been shuffled looks the same as one that should not.
            let other = in_hand(&mut f, 0, 1);
            f.cards[other].current.position = position::FACEDOWN;
            // Putting cards into a hand marks it for shuffling on its own;
            // the pass is what this test is about.
            f.core.shuffle_hand_check = [false, false];

            adjust(&mut f);
            run(&mut f);
            assert_eq!(f.cards[c].current.position, position::FACEUP);
            assert!(
                !f.messages
                    .iter()
                    .any(|m| matches!(m, Message::ShuffleHand { .. })),
                "a public card is not something the opponent has just seen"
            );
        }

        /// The disable check runs: a card an effect disables comes out of
        /// the pass marked.
        #[test]
        fn the_disable_check_runs() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            single(&mut f, c, code::DISABLE, 0);
            assert!(!f.cards[c].is_status(status::DISABLED));
            adjust(&mut f);
            run(&mut f);
            assert!(f.cards[c].is_status(status::DISABLED));
        }

        /// **The control sets are crossed.** A card sitting on one side
        /// while belonging to the other is handed *across*, which is what
        /// the two calls' swapped players do.
        #[test]
        fn a_misplaced_card_is_handed_across() {
            let mut f = Field::new(8000);
            f.infos.turn_player = 0;
            // Owned by player 1, sitting on player 0's side, with nothing
            // saying it should be: `refresh_control_status` answers "its
            // owner".
            let mut c = Card::with_data(
                CardData {
                    code: 18036057,
                    type_: card_type::MONSTER,
                    ..Default::default()
                },
                1,
            );
            c.current.controller = 0;
            c.set_status(status::EFFECT_ENABLED, true);
            let c = f.new_card(c);
            f.add_card(0, c, location::MZONE, 0, false);
            f.cards[c].current.position = position::FACEUP_ATTACK;

            adjust(&mut f);
            run(&mut f);
            assert_eq!(
                f.cards[c].current.controller, 1,
                "given to the player it belongs to"
            );
        }

        /// `EVENT_ADJUST` is raised at the end of every pass.
        #[test]
        fn the_adjust_event_is_raised() {
            let mut f = Field::new(8000);
            adjust(&mut f);
            run(&mut f);
            assert!(f
                .core
                .instant_event
                .iter()
                .chain(f.core.used_event.iter())
                .any(|e| e.event_code == code::ADJUST));
        }

        /// **Self-destruction is suppressed by `selfdes_disabled` — and so
        /// is the equip check**, because the skip jumps past it.
        #[test]
        fn disabling_self_destruction_also_skips_the_equip_check() {
            let mut f = Field::new(8000);
            let target = monster(&mut f, 0, 0);
            let mut c = Card::with_data(
                CardData {
                    code: 70828912,
                    type_: card_type::SPELL | card_type::EQUIP,
                    ..Default::default()
                },
                0,
            );
            c.current.controller = 0;
            let equip = f.new_card(c);
            f.add_card(0, equip, location::SZONE, 0, false);
            f.cards[equip].current.position = position::FACEUP_ATTACK;
            f.cards[equip].equiping_target = Some(target);

            f.core.selfdes_disabled = true;
            adjust(&mut f);
            run(&mut f);
            assert_eq!(
                f.cards[equip].current.location,
                location::SZONE,
                "the jump skips the equip check too"
            );
        }
    }

    mod the_deck {
        use super::*;

        /// The decks are turned over **once**, when the answer changes, and
        /// a second pass with the same answer says nothing.
        #[test]
        fn the_deck_is_reversed_once() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            let mut e = Effect::new(effect_type::FIELD, code::REVERSE_DECK);
            e.owner = Some(c);
            e.handler = Some(c);
            e.range = u16::from(location::MZONE);
            let id = f.new_effect(e);
            f.add_effect(id, 0);
            assert_ne!(
                f.core.global_flag & global_flag::DECK_REVERSE_CHECK,
                0,
                "registering it raised the flag that gates the pass"
            );

            adjust(&mut f);
            run(&mut f);
            let announced = f
                .messages
                .iter()
                .filter(|m| matches!(m, Message::ReverseDeck))
                .count();
            assert_eq!(announced, 1);
            assert!(f.core.deck_reversed);

            let before = f.messages.len();
            adjust(&mut f);
            run(&mut f);
            assert!(
                !f.messages[before..]
                    .iter()
                    .any(|m| matches!(m, Message::ReverseDeck)),
                "the answer did not change, so nothing is said"
            );
        }

        /// Without the global flag the pass does not run at all — which is
        /// the flag's whole purpose.
        #[test]
        fn the_pass_is_gated_on_the_global_flag() {
            let mut f = Field::new(8000);
            assert_eq!(f.core.global_flag & global_flag::DECK_REVERSE_CHECK, 0);
            f.core.deck_reversed = false;
            adjust(&mut f);
            run(&mut f);
            assert!(!f.messages.iter().any(|m| matches!(m, Message::ReverseDeck)));
        }
    }

    mod going_round_again {
        use super::*;

        /// **A pass that changed something makes the whole unit run again.**
        ///
        /// The forced position is the cheapest thing to observe it with: the
        /// first round applies it and sets the flag, and the second finds
        /// nothing left to do and stops.
        #[test]
        fn a_change_makes_it_run_again() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            single(
                &mut f,
                c,
                code::SET_POSITION,
                i64::from(position::FACEUP_DEFENSE),
            );
            adjust(&mut f);
            // Counted at the case that raises `EVENT_ADJUST`, which is
            // reached exactly once per round.
            let mut rounds = 0;
            for _ in 0..4096 {
                if f.queue()
                    .next()
                    .is_some_and(|u| matches!(u.kind, Kind::Adjust) && u.step == 15)
                {
                    rounds += 1;
                }
                match f.process() {
                    Status::Continue => continue,
                    Status::Awaiting => match f.messages.last() {
                        Some(Message::SelectChain { .. }) => f.core.returns.set(-1),
                        Some(Message::SelectEffectYesNo { .. }) => f.core.returns.set(0),
                        _ => break,
                    },
                    _ => break,
                }
            }
            assert_eq!(f.cards[c].current.position, position::FACEUP_DEFENSE);
            assert_eq!(rounds, 2, "one round to turn it, one to find nothing left");
            assert!(!f.core.re_adjust, "and stopped when nothing changed");
        }
    }

    /// The attack check takes its own first branch when there is no
    /// attacker, which is always — the battle system is not ported.
    #[test]
    fn the_attack_check_is_skipped_without_an_attacker() {
        let mut f = Field::new(8000);
        assert!(f.core.attacker.is_none());
        adjust(&mut f);
        assert_eq!(run(&mut f), Status::End);
    }
}

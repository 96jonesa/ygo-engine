//! The damage step: `DamageStep` and `calculate_battle_damage`.
//!
//! Four short cases and one long function. The cases are bookkeeping —
//! swap the attack in, announce it, swap it out — and the function is where
//! a battle's arithmetic actually happens.
//!
//! ## `core.attacker` is *swapped*, not assigned
//!
//! `std::swap(core.attacker, arg.attacker)` in case 0 and again in case 3.
//! So the unit holds whatever was there before and gives it back on the way
//! out, which is what lets a damage step run inside another one — an effect
//! that causes a battle during a battle. Assigning instead of swapping
//! works until exactly that happens.
//!
//! ## The `reserved` slot
//!
//! Case 1 does not stay on the queue. It moves itself into `core.reserved`
//! and returns *finished*, and `SolveChain` puts it back on the subunit
//! queue when the chain it made way for ends. Two units are therefore
//! interleaved without either being on the queue at the same time.
//!
//! ## `calculate_battle_damage` writes to `core`, and returns three things
//!
//! It fills `core.battle_damage[2]` and hands back the damage-changing
//! effect, the card that caused the damage, and which of the two monsters
//! were destroyed. The caller is `BattleCommand`, which is not ported.
//!
//! ### The shape of it
//!
//! ```text
//!   attacker vs monster in ATTACK position
//!     a > d  → target's controller takes a-d, target destroyed
//!     a < d  → attacker's controller takes d-a, attacker destroyed
//!     a = d  → both destroyed, unless a is 0 and the duel option is off
//!
//!   attacker vs monster in DEFENCE position
//!     a > d  → no damage *unless* EFFECT_PIERCE; target destroyed
//!     a < d  → attacker's controller takes d-a, and **nothing is
//!              destroyed** — the attacker survives
//!
//!   direct attack
//!     → the other player takes the attacker's value
//! ```
//!
//! The `a < d` against a defence-position monster is the one to look at
//! twice: `bd[0]` is *not* set, so the attacker is not destroyed even
//! though its controller takes damage.
//!
//! ### The redirection block appears twice
//!
//! Reflect, also, change, avoid — four families that move or resize the
//! damage — are written out **twice** in the reference: once inside the
//! pierce branch and once in the general block below it. The two copies are
//! identical bar which cards they read from. They are one function here,
//! and that is a deliberate departure: two copies of thirty lines is two
//! places for a fix to be applied to one of.
//!
//! ### `DOUBLE_DAMAGE` and `HALF_DAMAGE` are sentinel *values*
//!
//! `0x80000000` and `0x80000001` — an effect returning one of them is not
//! setting the damage to four billion, it is asking for a multiplier. And
//! an effect asking for both at once cancels itself out.

use crate::board::{location, position};
use crate::card::status;
use crate::event::{code, CardId, EffectId, Event, PLAYER_NONE};
use crate::field::{Field, Message};
use crate::processor::Kind;

/// The two sentinel values an `EFFECT_CHANGE_BATTLE_DAMAGE` may return
/// instead of an amount.
pub mod damage_sentinel {
    pub const DOUBLE: i64 = 0x8000_0000;
    pub const HALF: i64 = 0x8000_0001;
}

/// What `calculate_battle_damage` hands back besides `core.battle_damage`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BattleOutcome {
    /// An `EFFECT_BATTLE_DAMAGE_TO_EFFECT` on the card that caused the
    /// damage — the caller turns battle damage into effect damage with it.
    pub damage_change: Option<EffectId>,
    /// The card the damage is attributed to, or `None` when there is no
    /// damage at all.
    pub reason_card: Option<CardId>,
    /// `[attacker destroyed, target destroyed]`.
    pub destroyed: [bool; 2],
}

impl Field {
    /// `field::attack_all_target_check` — an "attacks everything" effect
    /// loses its blanket permission when it meets a target it may not hit.
    ///
    /// A **direct** attack clears it outright, with no effect consulted:
    /// there was no target to ask about.
    pub fn attack_all_target_check(&mut self) {
        let Some(attacker) = self.core.attacker else {
            return;
        };
        let Some(target) = self.core.attack_target else {
            self.cards[attacker].attack_all_target = false;
            return;
        };
        let Some(e) = self.is_affected_by_effect(attacker, code::ATTACK_ALL) else {
            return;
        };
        if self.effect_value_about(e, target) == 0 {
            self.cards[attacker].attack_all_target = false;
        }
    }

    /// `field::process(Processors::DamageStep&)`.
    pub(crate) fn damage_step_step(&mut self, step: u16, state: &mut DamageStepState) -> bool {
        match step {
            0 => self.damage_step_0(state),
            // Case 1 is where the unit steps aside. The reference
            // emplaces `BattleCommand` at step 26, sets its own step to 2,
            // moves itself into `core.reserved` and reports **finished** —
            // so it leaves the queue and `SolveChain` puts it back when the
            // chain ends.
            //
            // The emplacement is the first thing it does and
            // `BattleCommand` is not ported, so the parking below it cannot
            // be reached yet. `core.reserved` and
            // [`Field::restore_reserved_unit`] are in place for it, and
            // `SolveChain`'s case 12 already calls the restore.
            1 => {
                self.infos.phase = crate::duel::phases::DAMAGE_CAL;
                self.emplace_at(
                    Kind::BattleCommand {
                        state: Box::default(),
                    },
                    26,
                );
                // **Step aside.** The unit leaves the queue entirely and
                // reports finished; `SolveChain` puts it back when the
                // chain it made way for ends.
                self.set_step(2);
                self.core.reserved = Some(crate::processor::Unit::at(
                    Kind::DamageStep {
                        state: Box::new(state.clone()),
                    },
                    2,
                ));
                true
            }
            2 => {
                self.core.effect_damage_step = 2;
                // `BattleCommand` at step 32 — the after-damage half,
                // carrying the group case 31 handed back.
                self.emplace_at(
                    Kind::BattleCommand {
                        state: Box::new(crate::battle_command::BattleCommandState {
                            cards_destroyed_by_battle: state.cards_destroyed_by_battle,
                            ..Default::default()
                        }),
                    },
                    32,
                );
                false
            }
            _ => self.damage_step_3(state),
        }
    }

    /// Case 0: take over the attack, announce it, and flip a face-down
    /// target up.
    fn damage_step_0(&mut self, state: &mut DamageStepState) -> bool {
        // **A damage step already running refuses a second one**, unless
        // this is a genuinely new attack rather than a re-entry.
        if self.core.effect_damage_step != 0 && !state.new_attack {
            return true;
        }
        self.core.effect_damage_step = 1;
        std::mem::swap(&mut self.core.attacker, &mut state.attacker);
        std::mem::swap(&mut self.core.attack_target, &mut state.attack_target);
        state.backup_phase = self.infos.phase;

        let attacker = self.core.attacker;
        let target = self.core.attack_target;
        let gone = attacker.is_none_or(|a| self.cards[a].current.location != location::MZONE)
            || target.is_some_and(|t| self.cards[t].current.location != location::MZONE);
        if gone {
            self.set_step(2);
            return false;
        }
        let attacker = attacker.expect("checked above");

        if state.new_attack {
            let tp = self.infos.turn_player as usize;
            self.core.attack_state_count[tp] += 1;
            self.core.battled_count[tp] += 1;
            let tp = self.infos.turn_player;
            self.check_card_counter(attacker, crate::summon_support::activity::ATTACK, tp);
        }
        let fid = target.map_or(0, |t| self.cards[t].fieldid_r);
        self.cards[attacker].announced_cards.add(target, fid);
        self.attack_all_target_check();

        let attacker_info = self.get_info_location(attacker);
        let target_info = target
            .map(|t| self.get_info_location(t))
            .unwrap_or_default();
        self.messages.push(Message::Attack {
            attacker: attacker_info,
            target: target_info,
        });
        self.infos.phase = crate::duel::phases::DAMAGE;
        self.messages.push(Message::DamageStepStart);
        self.core.pre_field[0] = self.cards[attacker].fieldid_r;
        self.cards[attacker].attacked_count += 1;
        match target {
            Some(t) => {
                self.core.pre_field[1] = self.cards[t].fieldid_r;
                if self.cards[t].current.is_position(position::FACEDOWN) {
                    // **`position >> 1`** turns face-down into the face-up
                    // of the same orientation: attack to attack, defence
                    // to defence. Not a fixed position.
                    let up = self.cards[t].current.position >> 1;
                    self.change_position([t], None, PLAYER_NONE, up, up, up, up, 0, true);
                    self.adjust_all();
                }
            }
            None => self.core.pre_field[1] = 0,
        }
        false
    }

    /// Case 3: give the attack back and put the phase back.
    ///
    /// **The cards marked `STATUS_ATTACK_CANCELED` are the ones the swap
    /// just restored** — the *outer* attack, if this damage step was
    /// nested inside another. The swap happens first, so in the ordinary
    /// unnested case `core.attacker` is `None` afterwards and **nothing is
    /// marked at all**.
    ///
    /// That reads like a bug, and is the reference. Marking the attack
    /// that just finished would be the natural translation and is a
    /// different engine.
    fn damage_step_3(&mut self, state: &mut DamageStepState) -> bool {
        std::mem::swap(&mut self.core.attacker, &mut state.attacker);
        std::mem::swap(&mut self.core.attack_target, &mut state.attack_target);
        if let Some(a) = self.core.attacker {
            self.cards[a].set_status(status::ATTACK_CANCELED, true);
        }
        if let Some(t) = self.core.attack_target {
            self.cards[t].set_status(status::ATTACK_CANCELED, true);
        }
        self.core.effect_damage_step = 0;
        self.infos.phase = state.backup_phase;
        true
    }

    /// `field::calculate_battle_damage` — the arithmetic of a battle.
    ///
    /// Fills `core.battle_damage` and returns what the caller needs
    /// besides. See the module notes for the shape.
    pub fn calculate_battle_damage(&mut self) -> BattleOutcome {
        let Some(attacker) = self.core.attacker else {
            return BattleOutcome::default();
        };
        let target = self.core.attack_target;
        self.core.battle_damage = [0, 0];

        let pa = self.cards[attacker].current.controller;
        let mut a = self.attacker_value(attacker);
        let mut damp = 0u8;
        let mut reason_card = None;
        let mut destroyed = [false, false];
        let mut pierce = false;

        match target {
            Some(t) => {
                let pd = self.cards[t].current.controller;
                let d = self.target_value(t);
                if self.cards[t].current.is_position(position::ATTACK) {
                    if a > d {
                        damp = pd;
                        self.core.battle_damage[damp as usize] = (a - d) as u32;
                        reason_card = Some(attacker);
                        destroyed[1] = true;
                    } else if a < d {
                        damp = pa;
                        self.core.battle_damage[damp as usize] = (d - a) as u32;
                        reason_card = Some(t);
                        destroyed[0] = true;
                    } else if a != 0 || self.is_flag(crate::duel::flags::ZERO_ATK_DESTROYED) {
                        // **A 0-ATK tie destroys nothing** unless the duel
                        // option says otherwise.
                        destroyed = [true, true];
                    }
                } else if a > d {
                    // Defence position: no damage unless something pierces.
                    let piercers = self.filter_effect(attacker, code::PIERCE);
                    if !piercers.is_empty() {
                        pierce = true;
                        self.apply_pierce(attacker, t, a - d, &piercers, pa, pd);
                        reason_card = Some(attacker);
                    }
                    destroyed[1] = true;
                } else if a < d {
                    // **Nothing is destroyed here.** The attacker's
                    // controller takes the difference and the attacker
                    // survives, which is what makes `bd[0]` conspicuously
                    // absent.
                    damp = pa;
                    self.core.battle_damage[damp as usize] = (d - a) as u32;
                    reason_card = Some(t);
                }
                // `a` is consumed above; keep the binding honest.
                a = 0;
                let _ = a;
            }
            None => {
                if a != 0 {
                    damp = 1 - pa;
                    self.core.battle_damage[damp as usize] = a as u32;
                    reason_card = Some(attacker);
                }
            }
        }

        // The general redirection block, skipped when pierce already ran
        // its own copy, and skipped entirely when the damage is being
        // turned into effect damage instead.
        let mut damage_change = None;
        if let Some(rc) = reason_card {
            if !pierce {
                damage_change = self.is_affected_by_effect(rc, code::BATTLE_DAMAGE_TO_EFFECT);
                if damage_change.is_none() {
                    let dam_card = if rc == attacker {
                        target
                    } else {
                        Some(attacker)
                    };
                    self.redirect_battle_damage(rc, dam_card, damp, pa);
                }
            }
        }

        // **No damage at all means no reason card**, which is how the
        // caller tells "nothing happened" from "something was destroyed
        // without damage".
        if self.core.battle_damage == [0, 0] {
            reason_card = None;
        }
        BattleOutcome {
            damage_change,
            reason_card,
            destroyed,
        }
    }

    /// The attacker's value in this battle.
    ///
    /// `EFFECT_CHANGE_BATTLE_STAT` overrides everything. Otherwise a
    /// face-up **defence**-position attacker uses its DEF, but only when
    /// `EFFECT_DEFENSE_ATTACK`'s value is truthy — the effect's presence
    /// alone is not enough.
    fn attacker_value(&mut self, attacker: CardId) -> i32 {
        let ad = self.get_defense(attacker);
        let mut a = self.get_attack(attacker);
        if self.cards[attacker]
            .current
            .is_position(position::FACEUP_DEFENSE)
        {
            if let Some(e) = self.is_affected_by_effect(attacker, code::DEFENSE_ATTACK) {
                if self.effect_value_about(e, attacker) != 0 {
                    a = ad;
                }
            }
        }
        if let Some(e) = self.is_affected_by_effect(attacker, code::CHANGE_BATTLE_STAT) {
            a = self.effect_value_about(e, attacker) as i32;
        }
        a
    }

    /// The target's value: the stat override, else ATK in attack position
    /// and DEF in defence.
    fn target_value(&mut self, target: CardId) -> i32 {
        if let Some(e) = self.is_affected_by_effect(target, code::CHANGE_BATTLE_STAT) {
            return self.effect_value_about(e, target) as i32;
        }
        if self.cards[target].current.is_position(position::ATTACK) {
            self.get_attack(target)
        } else {
            self.get_defense(target)
        }
    }

    /// `EFFECT_PIERCE`: the excess becomes damage, to whichever side each
    /// piercing effect names.
    ///
    /// **Each effect names a side by its owner**: `dp[1 - handler_player]`.
    /// So a piercing effect its controller owns damages the *opponent*, and
    /// one the opponent owns damages its controller — several effects can
    /// therefore open both sides at once.
    fn apply_pierce(
        &mut self,
        attacker: CardId,
        target: CardId,
        excess: i32,
        piercers: &[EffectId],
        pa: u8,
        pd: u8,
    ) {
        let mut dp = [false, false];
        for &e in piercers {
            let owner = self
                .effects
                .get(e)
                .map_or(PLAYER_NONE, |x| x.get_handler_player(&self.cards));
            if owner < 2 {
                dp[1 - owner as usize] = true;
            }
        }
        for (i, &on) in dp.iter().enumerate() {
            if on {
                self.core.battle_damage[i] = excess as u32;
            }
        }
        // `DOUBLE_DAMAGE` from any piercing effect doubles both sides that
        // are open. There is no `HALF_DAMAGE` here — the reference's is
        // commented out, and is left out rather than invented.
        let doubled = piercers
            .iter()
            .any(|&e| self.effect_plain_value(e) == damage_sentinel::DOUBLE);
        if doubled {
            for (i, &on) in dp.iter().enumerate() {
                if on {
                    self.core.battle_damage[i] *= 2;
                }
            }
        }

        // **The reference assigns the *outer* `damp` here**, because all
        // of this is inline in one function there. Factoring the pierce
        // branch out gives it a local instead, and the write-back is
        // dropped.
        //
        // That is safe only because the outer `damp` is dead after this:
        // the general redirection block is guarded by `!pierce`, and the
        // final "no damage at all" test reads both sides symmetrically, so
        // which one is `damp` cannot change its answer. Recorded because
        // the safety is a property of the two call sites, not of the
        // refactor — a third reader of `damp` would break it silently.
        let mut both = dp[0] && dp[1];
        let mut damp = if dp[0] { 0u8 } else { 1 };
        if !both {
            let mirrored = self
                .is_affected_by_effect(attacker, code::BOTH_BATTLE_DAMAGE)
                .is_some()
                || self
                    .is_affected_by_effect(target, code::BOTH_BATTLE_DAMAGE)
                    .is_some();
            if mirrored {
                self.core.battle_damage[1 - damp as usize] = self.core.battle_damage[damp as usize];
                both = true;
            }
        }
        if both {
            damp = pd;
        }
        self.move_and_resize(Some(attacker), Some(target), damp, pa, both, pd);
    }

    /// The reflect / also / change / avoid block.
    ///
    /// **Written twice in the reference** — once for pierce and once for
    /// everything else — and identical bar which cards it reads. One
    /// function here; the duplication is the departure, and it is
    /// deliberate.
    fn redirect_battle_damage(
        &mut self,
        reason_card: CardId,
        dam_card: Option<CardId>,
        damp: u8,
        pa: u8,
    ) {
        let mut both = false;
        let mirrored = self
            .is_affected_by_effect(reason_card, code::BOTH_BATTLE_DAMAGE)
            .is_some()
            || dam_card.is_some_and(|c| {
                self.is_affected_by_effect(c, code::BOTH_BATTLE_DAMAGE)
                    .is_some()
            });
        if mirrored {
            self.core.battle_damage[1 - damp as usize] = self.core.battle_damage[damp as usize];
            both = true;
        }
        self.move_and_resize(Some(reason_card), dam_card, damp, pa, both, damp);
    }

    /// The half of the redirection block the two copies share exactly:
    /// reflect, also, the `CHANGE_BATTLE_DAMAGE` pass, and the two
    /// "no damage" prohibitions.
    fn move_and_resize(
        &mut self,
        from_card: Option<CardId>,
        to_card: Option<CardId>,
        damp: u8,
        pa: u8,
        both: bool,
        avoid_side: u8,
    ) {
        let from = from_card;
        let to = to_card;

        // `EFFECT_REFLECT_BATTLE_DAMAGE`, read on the card first and then
        // on the player — a card-level reflection shadows the player's.
        let mut reflect: [Option<EffectId>; 2] = [None, None];
        reflect[damp as usize] = to
            .and_then(|c| {
                from.and_then(|f| {
                    self.is_affected_by_effect_against(c, code::REFLECT_BATTLE_DAMAGE, f)
                })
            })
            .or_else(|| self.is_player_affected_by_effect(damp, code::REFLECT_BATTLE_DAMAGE));
        reflect[1 - damp as usize] = from
            .and_then(|f| {
                to.and_then(|c| {
                    self.is_affected_by_effect_against(f, code::REFLECT_BATTLE_DAMAGE, c)
                })
            })
            .or_else(|| self.is_player_affected_by_effect(1 - damp, code::REFLECT_BATTLE_DAMAGE));

        let mut also = [false, false];
        if !both {
            also[damp as usize] = to.is_some_and(|c| {
                self.is_affected_by_effect(c, code::ALSO_BATTLE_DAMAGE)
                    .is_some()
            }) || self
                .is_player_affected_by_effect(damp, code::ALSO_BATTLE_DAMAGE)
                .is_some();
            also[1 - damp as usize] = from.is_some_and(|c| {
                self.is_affected_by_effect(c, code::ALSO_BATTLE_DAMAGE)
                    .is_some()
            }) || self
                .is_player_affected_by_effect(1 - damp, code::ALSO_BATTLE_DAMAGE)
                .is_some();
        }

        if both {
            // **The turn player's effect applies first** — four branches
            // in a fixed order, and the order is the tie-break.
            let owner = |e: Option<EffectId>| {
                e.and_then(|x| self.effects.get(x))
                    .map_or(PLAYER_NONE, |x| x.get_handler_player(&self.cards))
            };
            let (pa_i, op_i) = (pa as usize, 1 - pa as usize);
            if reflect[pa_i].is_some() && owner(reflect[pa_i]) == pa {
                self.core.battle_damage[op_i] += self.core.battle_damage[pa_i];
                self.core.battle_damage[pa_i] = 0;
            } else if reflect[op_i].is_some() && owner(reflect[op_i]) == pa {
                self.core.battle_damage[pa_i] += self.core.battle_damage[op_i];
                self.core.battle_damage[op_i] = 0;
            } else if reflect[pa_i].is_some() && owner(reflect[pa_i]) == 1 - pa {
                self.core.battle_damage[op_i] += self.core.battle_damage[pa_i];
                self.core.battle_damage[pa_i] = 0;
            } else if reflect[op_i].is_some() && owner(reflect[op_i]) == 1 - pa {
                self.core.battle_damage[pa_i] += self.core.battle_damage[op_i];
                self.core.battle_damage[op_i] = 0;
            }
        } else {
            let (d, o) = (damp as usize, 1 - damp as usize);
            if reflect[d].is_some() {
                if !also[o] {
                    self.core.battle_damage[o] += self.core.battle_damage[d];
                    self.core.battle_damage[d] = 0;
                } else {
                    self.core.battle_damage[o] += self.core.battle_damage[d];
                    self.core.battle_damage[d] = self.core.battle_damage[o];
                }
            } else if also[d] {
                if reflect[o].is_none() {
                    self.core.battle_damage[o] += self.core.battle_damage[d];
                } else {
                    self.core.battle_damage[o] += self.core.battle_damage[d];
                    self.core.battle_damage[d] += self.core.battle_damage[o];
                    self.core.battle_damage[o] = 0;
                }
            }
        }

        self.apply_change_battle_damage(from, to, damp);

        // The two prohibitions, each reading three places.
        let no_damage_to =
            |f: &mut Self, side: u8, holder: Option<CardId>, other: Option<CardId>| {
                holder.is_some_and(|c| f.is_affected_by_effect(c, code::NO_BATTLE_DAMAGE).is_some())
                    || other.is_some_and(|c| {
                        holder.is_some_and(|h| {
                            f.is_affected_by_effect_against(c, code::AVOID_BATTLE_DAMAGE, h)
                                .is_some()
                        })
                    })
                    || f.is_player_affected_by_effect(side, code::AVOID_BATTLE_DAMAGE)
                        .is_some()
            };
        if no_damage_to(self, avoid_side, from, to) {
            self.core.battle_damage[avoid_side as usize] = 0;
        }
        if no_damage_to(self, 1 - avoid_side, to, from) {
            self.core.battle_damage[1 - avoid_side as usize] = 0;
        }
    }

    /// The `EFFECT_CHANGE_BATTLE_DAMAGE` pass, run once per player.
    ///
    /// Four sources gathered and **sorted by effect id**, then walked for
    /// each side. `DOUBLE_DAMAGE` and `HALF_DAMAGE` are sentinels rather
    /// than amounts, an effect asking for both cancels itself, and a
    /// **zero stops the walk immediately** — later effects do not get to
    /// put the damage back.
    ///
    /// The last plain value wins, and it is applied **only if there is
    /// damage to change**: `dam_value >= 0 && battle_damage[p] > 0`. So an
    /// effect that sets a number cannot create damage where there was none.
    fn apply_change_battle_damage(&mut self, from: Option<CardId>, to: Option<CardId>, damp: u8) {
        let mut eset: Vec<EffectId> = Vec::new();
        if let Some(c) = from {
            eset.extend(self.filter_effect(c, code::CHANGE_BATTLE_DAMAGE));
        }
        if let Some(c) = to {
            eset.extend(self.filter_effect(c, code::CHANGE_BATTLE_DAMAGE));
        }
        eset.extend(self.filter_player_effect(damp, code::CHANGE_BATTLE_DAMAGE));
        eset.extend(self.filter_player_effect(1 - damp, code::CHANGE_BATTLE_DAMAGE));
        self.sort_by_effect_id(&mut eset);

        let subject = from.or(to);
        for p in 0..2u8 {
            let mut double_dam = false;
            let mut half_dam = false;
            let mut dam_value: i32 = -1;
            for &e in &eset {
                let is_player_target = self
                    .effects
                    .get(e)
                    .is_some_and(|x| x.is_flag(crate::effect::flag::PLAYER_TARGET));
                let val = if !is_player_target {
                    self.change_damage_value(e, Some(i64::from(p)), subject)
                } else if self
                    .effects
                    .get(e)
                    .is_some_and(|x| x.is_target_player(&self.cards, p))
                {
                    self.change_damage_value(e, None, subject)
                } else {
                    // Not aimed at this player: the reference leaves `val`
                    // at -1, which is "no opinion".
                    -1
                };
                if val == damage_sentinel::DOUBLE {
                    double_dam = true;
                } else if val == damage_sentinel::HALF {
                    half_dam = true;
                } else if val > 0 {
                    dam_value = val as i32;
                } else if val == 0 {
                    dam_value = 0;
                    break;
                }
            }
            if double_dam && half_dam {
                double_dam = false;
                half_dam = false;
            }
            if double_dam {
                self.core.battle_damage[p as usize] *= 2;
            }
            if half_dam {
                self.core.battle_damage[p as usize] /= 2;
            }
            if dam_value >= 0 && self.core.battle_damage[p as usize] > 0 {
                self.core.battle_damage[p as usize] = dam_value as u32;
            }
        }
    }

    /// One `EFFECT_CHANGE_BATTLE_DAMAGE` asked about one side.
    fn change_damage_value(
        &self,
        effect: EffectId,
        player: Option<i64>,
        subject: Option<CardId>,
    ) -> i64 {
        let Some(e) = self.effects.get(effect) else {
            return -1;
        };
        let args: Vec<i64> = player.into_iter().collect();
        let ev = Event::new(0);
        let ctx = crate::effect::Ctx {
            reason_effect: effect,
            player: PLAYER_NONE,
            event: &ev,
            card: subject,
            args: &args,
        };
        e.get_value(self, &ctx)
    }

    /// An effect's value asked about one card.
    pub(crate) fn effect_value_about(&self, effect: EffectId, card: CardId) -> i64 {
        let Some(e) = self.effects.get(effect) else {
            return 0;
        };
        let ev = Event::new(0);
        let ctx = crate::effect::Ctx {
            reason_effect: effect,
            player: self.cards[card].current.controller,
            event: &ev,
            card: Some(card),
            args: &[],
        };
        e.get_value(self, &ctx)
    }

    /// `core.reserved` put back on the subunit queue. `SolveChain` calls
    /// this where the reference writes the `exchange`.
    pub(crate) fn restore_reserved_unit(&mut self) {
        if let Some(unit) = self.core.reserved.take() {
            self.core.subunits.push(unit);
        }
    }
}

/// `DamageStep`'s own state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DamageStepState {
    pub backup_phase: u16,
    pub new_attack: bool,
    /// Filled in by `BattleCommand`'s case 31 while this unit waits in
    /// `core.reserved` — the cards that battle destroyed, handed across so
    /// case 2 can pass them back.
    pub cards_destroyed_by_battle: Option<crate::field::GroupId>,
    /// **The previous attack**, swapped out of `core` on the way in and
    /// back on the way out — not this step's attacker.
    pub attacker: Option<CardId>,
    pub attack_target: Option<CardId>,
}

impl Field {
    /// `field::damage_step` — emplace one.
    pub fn damage_step(&mut self, attacker: Option<CardId>, target: Option<CardId>, new: bool) {
        self.emplace(Kind::DamageStep {
            state: Box::new(DamageStepState {
                backup_phase: 0,
                new_attack: new,
                cards_destroyed_by_battle: None,
                attacker,
                attack_target: target,
            }),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{card_type, Card, CardData};
    use crate::effect::{effect_type, flag, Effect};
    use crate::processor::Status;

    fn field() -> Field {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        f.infos.turn_player = 0;
        f
    }

    fn monster(f: &mut Field, player: u8, seat: u32, atk: i32, def: i32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 18036057 + seat + u32::from(player) * 100,
                type_: card_type::MONSTER,
                level: 4,
                attack: atk,
                defense: def,
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
        // A free seat, not a fixed one: two anchors at the same seat means
        // the second replaces the first and the first effect goes with it.
        let seat = f.players[player as usize]
            .mzone
            .iter()
            .rposition(Option::is_none)
            .unwrap_or(6) as u32;
        let a = monster(f, player, seat, 0, 0);
        let mut e = Effect::new(effect_type::FIELD, code_);
        e.owner = Some(a);
        e.handler = Some(a);
        e.flag[0] = flag::PLAYER_TARGET | flag::ABSOLUTE_TARGET;
        e.range = u16::from(location::MZONE);
        // `s_range` is player 0 outright, `o_range` player 1.
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

    /// Set up an attack and calculate it.
    fn battle(f: &mut Field, attacker: CardId, target: Option<CardId>) -> BattleOutcome {
        f.core.attacker = Some(attacker);
        f.core.attack_target = target;
        f.calculate_battle_damage()
    }

    mod calculate_battle_damage {
        use super::*;

        /// Attack vs attack: the bigger wins, the difference is damage,
        /// and the loser is destroyed.
        #[test]
        fn attack_position_the_bigger_wins() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 2000, 0);
            let x = monster(&mut f, 1, 0, 1200, 0);
            let out = battle(&mut f, a, Some(x));
            assert_eq!(f.core.battle_damage, [0, 800], "the defender's controller");
            assert_eq!(out.reason_card, Some(a));
            assert_eq!(out.destroyed, [false, true], "the target");
        }

        /// And the other way round: a stronger defender destroys the
        /// attacker and damages its controller.
        #[test]
        fn attack_position_a_stronger_defender_wins() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 1200, 0);
            let x = monster(&mut f, 1, 0, 2000, 0);
            let out = battle(&mut f, a, Some(x));
            assert_eq!(f.core.battle_damage, [800, 0]);
            assert_eq!(out.reason_card, Some(x));
            assert_eq!(out.destroyed, [true, false], "the attacker");
        }

        /// **Equal attack destroys both, with no damage** — and a
        /// **0-ATK tie destroys nothing** unless the duel option says so.
        #[test]
        fn an_equal_battle_destroys_both_except_at_zero() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 1500, 0);
            let x = monster(&mut f, 1, 0, 1500, 0);
            let out = battle(&mut f, a, Some(x));
            assert_eq!(f.core.battle_damage, [0, 0], "no damage");
            assert_eq!(out.destroyed, [true, true]);
            assert_eq!(out.reason_card, None, "no damage means no reason card");

            let mut f = field();
            let a = monster(&mut f, 0, 0, 0, 0);
            let x = monster(&mut f, 1, 0, 0, 0);
            let out = battle(&mut f, a, Some(x));
            assert_eq!(out.destroyed, [false, false], "0 vs 0 destroys nothing");

            let mut f = field();
            f.flags |= crate::duel::flags::ZERO_ATK_DESTROYED;
            let a = monster(&mut f, 0, 0, 0, 0);
            let x = monster(&mut f, 1, 0, 0, 0);
            let out = battle(&mut f, a, Some(x));
            assert_eq!(out.destroyed, [true, true], "unless the option is on");
        }

        /// **Attacking a defence-position monster deals no damage**, and
        /// destroys it when the attack is higher.
        #[test]
        fn defence_position_takes_no_damage() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 2000, 0);
            let x = monster(&mut f, 1, 0, 0, 1200);
            f.cards[x].current.position = position::FACEUP_DEFENSE;
            let out = battle(&mut f, a, Some(x));
            assert_eq!(f.core.battle_damage, [0, 0], "no damage without pierce");
            assert_eq!(out.destroyed, [false, true]);
            assert_eq!(out.reason_card, None);
        }

        /// **A stronger defence damages the attacker's controller and
        /// destroys nothing.** The attacker survives — which is the one
        /// asymmetry in the whole function.
        #[test]
        fn a_stronger_defence_damages_but_destroys_nothing() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 1000, 0);
            let x = monster(&mut f, 1, 0, 0, 2500);
            f.cards[x].current.position = position::FACEUP_DEFENSE;
            let out = battle(&mut f, a, Some(x));
            assert_eq!(f.core.battle_damage, [1500, 0], "the attacker's controller");
            assert_eq!(out.destroyed, [false, false], "and nobody is destroyed");
            assert_eq!(out.reason_card, Some(x));
        }

        /// A direct attack: the other player takes the attacker's value.
        #[test]
        fn a_direct_attack_deals_the_attackers_value() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 1800, 0);
            let out = battle(&mut f, a, None);
            assert_eq!(f.core.battle_damage, [0, 1800]);
            assert_eq!(out.reason_card, Some(a));
            assert_eq!(out.destroyed, [false, false]);
        }

        /// **A 0-ATK direct attack does nothing at all.**
        #[test]
        fn a_zero_attack_direct_attack_does_nothing() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 0, 0);
            let out = battle(&mut f, a, None);
            assert_eq!(f.core.battle_damage, [0, 0]);
            assert_eq!(out.reason_card, None);
        }

        /// **`EFFECT_PIERCE` turns the excess into damage**, and each
        /// piercing effect names the side by its *owner*: an effect the
        /// attacker's controller owns damages the opponent.
        #[test]
        fn pierce_deals_the_excess() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 2000, 0);
            let x = monster(&mut f, 1, 0, 0, 1200);
            f.cards[x].current.position = position::FACEUP_DEFENSE;
            single(&mut f, a, code::PIERCE, 0);
            let out = battle(&mut f, a, Some(x));
            assert_eq!(f.core.battle_damage, [0, 800], "the defender's controller");
            assert_eq!(out.destroyed, [false, true]);
        }

        /// **`DOUBLE_DAMAGE` from a piercing effect doubles it.**
        #[test]
        fn pierce_can_double() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 2000, 0);
            let x = monster(&mut f, 1, 0, 0, 1200);
            f.cards[x].current.position = position::FACEUP_DEFENSE;
            single(&mut f, a, code::PIERCE, damage_sentinel::DOUBLE);
            battle(&mut f, a, Some(x));
            assert_eq!(f.core.battle_damage, [0, 1600], "800 doubled");
        }

        /// **`EFFECT_CHANGE_BATTLE_STAT` overrides the stat used**, on
        /// **either** card — they are two separate reads.
        #[test]
        fn change_battle_stat_overrides_on_either_side() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 1000, 0);
            let x = monster(&mut f, 1, 0, 1200, 0);
            single(&mut f, a, code::CHANGE_BATTLE_STAT, 3000);
            let out = battle(&mut f, a, Some(x));
            assert_eq!(f.core.battle_damage, [0, 1800], "3000 - 1200");
            assert_eq!(out.destroyed, [false, true]);

            let mut f = field();
            let a = monster(&mut f, 0, 0, 2000, 0);
            let x = monster(&mut f, 1, 0, 1200, 0);
            single(&mut f, x, code::CHANGE_BATTLE_STAT, 3000);
            let out = battle(&mut f, a, Some(x));
            assert_eq!(
                f.core.battle_damage,
                [1000, 0],
                "3000 - 2000, the other way"
            );
            assert_eq!(out.destroyed, [true, false]);
        }

        /// **`EFFECT_DEFENSE_ATTACK` applies only in defence position.**
        /// An attack-position monster carrying it still uses its ATK.
        #[test]
        fn defense_attack_applies_only_in_defence_position() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 500, 2000);
            // Face-up **attack** position.
            let x = monster(&mut f, 1, 0, 1200, 0);
            single(&mut f, a, code::DEFENSE_ATTACK, 1);
            battle(&mut f, a, Some(x));
            assert_eq!(
                f.core.battle_damage,
                [700, 0],
                "ATK 500 against 1200, not DEF 2000"
            );
        }

        /// **The stale damage from a previous battle is cleared.**
        #[test]
        fn the_previous_damage_is_cleared() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 1500, 0);
            let x = monster(&mut f, 1, 0, 1500, 0);
            f.core.battle_damage = [999, 999];
            battle(&mut f, a, Some(x));
            assert_eq!(f.core.battle_damage, [0, 0], "an equal battle, so none");
        }

        /// **Pierce runs its own redirection and the general block is
        /// skipped** — otherwise a reflection would be applied twice.
        #[test]
        fn pierce_does_not_redirect_twice() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 2000, 0);
            let x = monster(&mut f, 1, 0, 0, 1200);
            f.cards[x].current.position = position::FACEUP_DEFENSE;
            single(&mut f, a, code::PIERCE, 0);
            player_effect(&mut f, code::REFLECT_BATTLE_DAMAGE, 1, 1);
            battle(&mut f, a, Some(x));
            assert_eq!(
                f.core.battle_damage,
                [800, 0],
                "reflected once, back to the attacker"
            );
        }

        /// **`EFFECT_DEFENSE_ATTACK` needs a truthy value.** A
        /// defence-position attacker uses its DEF only when the effect
        /// says so — the effect's presence alone is not enough.
        #[test]
        fn defense_attack_needs_a_truthy_value() {
            for (value, expect_damage) in [(0i64, [0u32, 0]), (1, [0, 800])] {
                let mut f = field();
                let a = monster(&mut f, 0, 0, 500, 2000);
                f.cards[a].current.position = position::FACEUP_DEFENSE;
                let x = monster(&mut f, 1, 0, 1200, 0);
                single(&mut f, a, code::DEFENSE_ATTACK, value);
                battle(&mut f, a, Some(x));
                if expect_damage == [0, 0] {
                    // ATK 500 against ATK 1200: the attacker loses.
                    assert_eq!(f.core.battle_damage, [700, 0], "used ATK, and lost");
                } else {
                    assert_eq!(f.core.battle_damage, expect_damage, "used DEF 2000");
                }
            }
        }

        /// **`EFFECT_NO_BATTLE_DAMAGE` on the card that caused it** stops
        /// the damage.
        #[test]
        fn no_battle_damage_stops_it() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 2000, 0);
            let x = monster(&mut f, 1, 0, 1200, 0);
            single(&mut f, a, code::NO_BATTLE_DAMAGE, 1);
            let out = battle(&mut f, a, Some(x));
            assert_eq!(f.core.battle_damage, [0, 0]);
            assert_eq!(out.reason_card, None, "no damage, so no reason card");
            assert_eq!(out.destroyed, [false, true], "but it still destroyed");
        }

        /// **`EFFECT_REFLECT_BATTLE_DAMAGE` sends it the other way.**
        #[test]
        fn reflect_battle_damage_swaps_the_victim() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 2000, 0);
            let x = monster(&mut f, 1, 0, 1200, 0);
            player_effect(&mut f, code::REFLECT_BATTLE_DAMAGE, 1, 1);
            battle(&mut f, a, Some(x));
            assert_eq!(f.core.battle_damage, [800, 0], "back to the attacker");
        }

        /// **`EFFECT_BOTH_BATTLE_DAMAGE` mirrors it onto both.**
        #[test]
        fn both_battle_damage_hits_everyone() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 2000, 0);
            let x = monster(&mut f, 1, 0, 1200, 0);
            single(&mut f, a, code::BOTH_BATTLE_DAMAGE, 1);
            battle(&mut f, a, Some(x));
            assert_eq!(f.core.battle_damage, [800, 800]);
        }

        /// **`EFFECT_CHANGE_BATTLE_DAMAGE` sets the amount**, and only
        /// where there is already damage — an effect cannot create damage
        /// on a side the battle left at zero.
        ///
        /// The discriminating half is the *second* case: the block has to
        /// run at all, so there must be damage **somewhere**, and the
        /// effect must name the side that has none.
        #[test]
        fn change_battle_damage_cannot_create_damage() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 2000, 0);
            let x = monster(&mut f, 1, 0, 1200, 0);
            player_effect(&mut f, code::CHANGE_BATTLE_DAMAGE, 1, 100);
            battle(&mut f, a, Some(x));
            assert_eq!(f.core.battle_damage, [0, 100], "set where damage existed");

            // Damage lands on player 1; the effect names player 0, who has
            // none. It must stay none.
            let mut f = field();
            let a = monster(&mut f, 0, 0, 2000, 0);
            let x = monster(&mut f, 1, 0, 1200, 0);
            player_effect(&mut f, code::CHANGE_BATTLE_DAMAGE, 0, 100);
            battle(&mut f, a, Some(x));
            assert_eq!(
                f.core.battle_damage,
                [0, 800],
                "player 0 has no damage, so nothing to set"
            );
        }

        /// **The gathered effects are sorted by id**, so the one created
        /// first wins the "last plain value" race regardless of which
        /// list it was gathered from.
        ///
        /// The card's effects are gathered before the players', so a card
        /// effect created *later* than a player effect is out of order
        /// until the sort puts it back.
        #[test]
        fn the_change_effects_are_sorted_by_id() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 2000, 0);
            let x = monster(&mut f, 1, 0, 1200, 0);
            // Created first, so it sorts first and loses the race.
            player_effect(&mut f, code::CHANGE_BATTLE_DAMAGE, 1, 100);
            // Created second, on the card, so gathered first but sorted
            // last — its value is the one that should win.
            single(&mut f, a, code::CHANGE_BATTLE_DAMAGE, 300);
            battle(&mut f, a, Some(x));
            assert_eq!(
                f.core.battle_damage,
                [0, 300],
                "the later-created effect won, so the sort ran"
            );
        }

        /// **`DOUBLE` and `HALF` together cancel out.**
        #[test]
        fn double_and_half_cancel() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 2000, 0);
            let x = monster(&mut f, 1, 0, 1200, 0);
            player_effect(
                &mut f,
                code::CHANGE_BATTLE_DAMAGE,
                1,
                damage_sentinel::DOUBLE,
            );
            player_effect(&mut f, code::CHANGE_BATTLE_DAMAGE, 1, damage_sentinel::HALF);
            battle(&mut f, a, Some(x));
            assert_eq!(f.core.battle_damage, [0, 800], "unchanged");
        }

        /// **A zero stops the walk** — a later effect does not get to put
        /// the damage back.
        #[test]
        fn a_zero_change_stops_the_walk() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 2000, 0);
            let x = monster(&mut f, 1, 0, 1200, 0);
            // Created in this order, so sorted in this order by effect id.
            player_effect(&mut f, code::CHANGE_BATTLE_DAMAGE, 1, 0);
            player_effect(&mut f, code::CHANGE_BATTLE_DAMAGE, 1, 500);
            battle(&mut f, a, Some(x));
            assert_eq!(f.core.battle_damage, [0, 0], "the zero won");
        }

        /// **`EFFECT_BATTLE_DAMAGE_TO_EFFECT` is handed back rather than
        /// applied**, and the redirection block is skipped entirely.
        #[test]
        fn battle_damage_to_effect_is_reported_not_applied() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 2000, 0);
            let x = monster(&mut f, 1, 0, 1200, 0);
            let e = single(&mut f, a, code::BATTLE_DAMAGE_TO_EFFECT, 1);
            player_effect(&mut f, code::REFLECT_BATTLE_DAMAGE, 1, 1);
            let out = battle(&mut f, a, Some(x));
            assert_eq!(out.damage_change, Some(e));
            assert_eq!(
                f.core.battle_damage,
                [0, 800],
                "the reflection never ran — the block was skipped"
            );
        }
    }

    mod attack_all_target_check {
        use super::*;

        /// **A direct attack clears the flag outright**, with no effect
        /// consulted — there was no target to ask about.
        #[test]
        fn a_direct_attack_clears_it() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 1000, 0);
            f.cards[a].attack_all_target = true;
            f.core.attacker = Some(a);
            f.core.attack_target = None;
            f.attack_all_target_check();
            assert!(!f.cards[a].attack_all_target);
        }

        /// **With no `EFFECT_ATTACK_ALL` the flag is left alone**, even
        /// against a target.
        #[test]
        fn without_the_effect_it_is_untouched() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 1000, 0);
            let x = monster(&mut f, 1, 0, 1000, 0);
            f.cards[a].attack_all_target = true;
            f.core.attacker = Some(a);
            f.core.attack_target = Some(x);
            f.attack_all_target_check();
            assert!(f.cards[a].attack_all_target);
        }

        /// **A target the effect refuses clears the flag.**
        #[test]
        fn a_refused_target_clears_it() {
            for (value, kept) in [(1i64, true), (0, false)] {
                let mut f = field();
                let a = monster(&mut f, 0, 0, 1000, 0);
                let x = monster(&mut f, 1, 0, 1000, 0);
                single(&mut f, a, code::ATTACK_ALL, value);
                f.cards[a].attack_all_target = true;
                f.core.attacker = Some(a);
                f.core.attack_target = Some(x);
                f.attack_all_target_check();
                assert_eq!(f.cards[a].attack_all_target, kept, "value {value}");
            }
        }
    }

    mod damage_step_unit {
        use super::*;

        fn run_one(f: &mut Field) -> Status {
            f.process()
        }

        /// Case 0 takes over the attack, announces it, and enters the
        /// Damage Phase.
        #[test]
        fn it_announces_the_attack() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 1800, 0);
            let x = monster(&mut f, 1, 0, 1200, 0);
            f.infos.phase = crate::duel::phases::BATTLE_STEP;
            f.damage_step(Some(a), Some(x), true);
            run_one(&mut f);

            assert_eq!(f.core.attacker, Some(a), "swapped in");
            assert_eq!(f.core.attack_target, Some(x));
            assert_eq!(f.infos.phase, crate::duel::phases::DAMAGE);
            assert_eq!(f.core.effect_damage_step, 1);
            assert_eq!(f.core.pre_field[0], f.cards[a].fieldid_r);
            assert_eq!(f.core.pre_field[1], f.cards[x].fieldid_r);
            assert_eq!(f.cards[a].attacked_count, 1);
            assert_eq!(f.core.attack_state_count[0], 1);
            assert!(f
                .messages
                .iter()
                .any(|m| matches!(m, Message::Attack { .. })));
            assert!(f
                .messages
                .iter()
                .any(|m| matches!(m, Message::DamageStepStart)));
        }

        /// **A face-down target is flipped to the face-up of its own
        /// orientation** — `position >> 1`, not a fixed position.
        #[test]
        fn a_face_down_target_is_flipped_the_right_way_up() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 1800, 0);
            let x = monster(&mut f, 1, 0, 1200, 800);
            f.cards[x].current.position = position::FACEDOWN_DEFENSE;
            f.damage_step(Some(a), Some(x), true);
            for _ in 0..256 {
                if f.cards[x].current.position == position::FACEUP_DEFENSE {
                    break;
                }
                match f.process() {
                    Status::Continue => continue,
                    // The flip runs `ChangePos`, which opens a response
                    // window; decline it and carry on.
                    Status::Awaiting => f.core.returns.set(-1),
                    Status::End => break,
                }
            }
            assert_eq!(
                f.cards[x].current.position,
                position::FACEUP_DEFENSE,
                "defence stays defence"
            );
        }

        /// **A second damage step is refused** unless it is a new attack.
        #[test]
        fn a_nested_damage_step_is_refused() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 1800, 0);
            f.core.effect_damage_step = 1;
            f.damage_step(Some(a), None, false);
            run_one(&mut f);
            assert!(f.core.attacker.is_none(), "took nothing over");
            assert!(
                !f.messages
                    .iter()
                    .any(|m| matches!(m, Message::DamageStepStart)),
                "and announced nothing"
            );
        }

        /// **`new_attack` waives the nested-step guard.** The same
        /// position that was refused above proceeds when the attack is a
        /// genuinely new one rather than a re-entry.
        #[test]
        fn a_new_attack_waives_the_nested_guard() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 1800, 0);
            f.core.effect_damage_step = 1;
            f.damage_step(Some(a), None, true);
            run_one(&mut f);
            assert_eq!(f.core.attacker, Some(a), "took it over anyway");
            assert!(f
                .messages
                .iter()
                .any(|m| matches!(m, Message::DamageStepStart)));
        }

        /// **The attack is swapped, not assigned** — which is what lets a
        /// damage step run inside another one. The outer attack ends up in
        /// the unit's own state, to be given back in case 3.
        #[test]
        fn the_outer_attack_is_swapped_into_the_unit() {
            let mut f = field();
            let outer = monster(&mut f, 0, 0, 1000, 0);
            let inner = monster(&mut f, 0, 1, 1800, 0);
            f.core.attacker = Some(outer);
            f.damage_step(Some(inner), None, true);
            run_one(&mut f);
            assert_eq!(f.core.attacker, Some(inner), "the new attack is live");
            let held = f
                .core
                .units
                .iter()
                .chain(f.core.subunits.iter())
                .find_map(|u| match &u.kind {
                    Kind::DamageStep { state } => Some(state.attacker),
                    _ => None,
                });
            assert_eq!(
                held,
                Some(Some(outer)),
                "and the outer one is held, not discarded"
            );
        }

        /// **A departed *target* skips the announcement too**, not only a
        /// departed attacker.
        #[test]
        fn a_departed_target_skips_the_announcement() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 1800, 0);
            let x = monster(&mut f, 1, 0, 1200, 0);
            f.remove_card(x);
            f.add_card(1, x, location::GRAVE, 0, false);
            f.damage_step(Some(a), Some(x), true);
            run_one(&mut f);
            assert!(
                !f.messages
                    .iter()
                    .any(|m| matches!(m, Message::Attack { .. })),
                "no attack announced"
            );
        }

        /// **The attack is recorded against the target**, in the tally
        /// that "attack each monster once" reads.
        #[test]
        fn the_attack_is_tallied_against_its_target() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 1800, 0);
            let x = monster(&mut f, 1, 0, 1200, 0);
            let fid = f.cards[x].fieldid_r;
            f.damage_step(Some(a), Some(x), true);
            run_one(&mut f);
            assert_eq!(f.cards[a].announced_cards.count(fid, true), 1);

            // And a direct attack is tallied under the player's key.
            let mut f = field();
            let a = monster(&mut f, 0, 0, 1800, 0);
            f.damage_step(Some(a), None, true);
            run_one(&mut f);
            assert_eq!(f.cards[a].announced_cards.count(0, false), 1);
        }

        /// **`attack_all_target_check` runs here**, so a direct attack
        /// spends the blanket permission.
        #[test]
        fn the_attack_all_check_runs() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 1800, 0);
            f.cards[a].attack_all_target = true;
            f.damage_step(Some(a), None, true);
            run_one(&mut f);
            assert!(
                !f.cards[a].attack_all_target,
                "a direct attack clears it, and this is where"
            );
        }

        /// **An attacker that has left the Monster Zone skips to the
        /// end** rather than announcing an attack that cannot happen.
        #[test]
        fn a_departed_attacker_skips_the_announcement() {
            let mut f = field();
            let a = monster(&mut f, 0, 0, 1800, 0);
            f.remove_card(a);
            f.add_card(0, a, location::GRAVE, 0, false);
            f.damage_step(Some(a), None, true);
            run_one(&mut f);
            assert!(
                !f.messages
                    .iter()
                    .any(|m| matches!(m, Message::Attack { .. })),
                "no attack announced"
            );
        }

        /// **Case 3 marks the attack it *restored*, not the one it just
        /// ran.**
        ///
        /// The swap happens first, so `core.attacker` afterwards holds
        /// whatever the unit was carrying — the *outer* attack, if this
        /// damage step was nested inside another. In the ordinary
        /// unnested case the unit carries `None`, so **nothing is marked
        /// at all**, which reads like a bug and is the reference.
        #[test]
        fn the_last_case_marks_the_restored_attack_not_the_finished_one() {
            // Unnested: the unit carries nothing, so nothing is marked.
            let mut f = field();
            let a = monster(&mut f, 0, 0, 1800, 0);
            f.core.attacker = Some(a);
            f.core.effect_damage_step = 1;
            f.infos.phase = crate::duel::phases::DAMAGE;
            f.emplace_at(
                Kind::DamageStep {
                    state: Box::new(DamageStepState {
                        backup_phase: crate::duel::phases::BATTLE_STEP,
                        new_attack: false,
                        cards_destroyed_by_battle: None,
                        attacker: None,
                        attack_target: None,
                    }),
                },
                3,
            );
            run_one(&mut f);
            assert_eq!(f.infos.phase, crate::duel::phases::BATTLE_STEP, "restored");
            assert_eq!(f.core.effect_damage_step, 0);
            assert!(f.core.attacker.is_none(), "swapped back out");
            assert!(
                !f.cards[a].is_status(status::ATTACK_CANCELED),
                "the finished attacker is NOT marked"
            );

            // Nested: the unit carries the outer attack, and *that* is
            // what gets marked.
            let mut f = field();
            let inner = monster(&mut f, 0, 0, 1800, 0);
            let outer = monster(&mut f, 0, 1, 1000, 0);
            let outer_t = monster(&mut f, 1, 0, 1000, 0);
            f.core.attacker = Some(inner);
            f.core.effect_damage_step = 1;
            f.emplace_at(
                Kind::DamageStep {
                    state: Box::new(DamageStepState {
                        backup_phase: crate::duel::phases::BATTLE_STEP,
                        new_attack: false,
                        cards_destroyed_by_battle: None,
                        attacker: Some(outer),
                        attack_target: Some(outer_t),
                    }),
                },
                3,
            );
            run_one(&mut f);
            assert_eq!(f.core.attacker, Some(outer), "the outer attack is back");
            assert!(
                f.cards[outer].is_status(status::ATTACK_CANCELED),
                "and it is the one marked"
            );
            assert!(f.cards[outer_t].is_status(status::ATTACK_CANCELED));
            assert!(
                !f.cards[inner].is_status(status::ATTACK_CANCELED),
                "not the one that just fought"
            );
        }
    }
}

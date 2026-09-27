//! Life points: `Damage`, `Recover`, `PayLPCost`.
//!
//! Three units and one recurring shape — **the amount is negotiated before
//! anything is paid**. Each begins by asking a series of effects what the
//! number should be, or whether it should happen at all, and only then
//! touches `player[].lp`.
//!
//! ## `Damage` and `Recover` call each other
//!
//! `EFFECT_REVERSE_DAMAGE` turns damage into recovery and
//! `EFFECT_REVERSE_RECOVER` turns recovery into damage — so each unit's
//! case 0 can emplace the *other* one and jump to its own case 2 to report
//! the amount.
//!
//! The recursion terminates because each reversal **adds a bit** to the
//! reason (`REASON_RDAMAGE` or `REASON_RRECOVER`) and each unit skips its
//! own reversal check when its bit is already set. With both effects in
//! play the round trip is therefore *two* reversals, not one: damage
//! becomes recovery, which becomes damage, which is left alone because by
//! then both bits are set. The outcome is damage — the thing it started
//! as — and not the recovery it passed through.
//!
//! ## Four effects, in a fixed order, and only one of them can stop it
//!
//! `Damage`'s case 0 runs three families in sequence and the order is the
//! behaviour:
//!
//! 1. `REVERSE_DAMAGE` — becomes recovery, and nothing below runs.
//! 2. `REFLECT_DAMAGE` — the *other* player takes it. Note what this does
//!    **not** do: it does not re-run the reversal check for the new
//!    player. A reflected damage cannot then be reversed.
//! 3. `CHANGE_DAMAGE` — each effect in turn rewrites the amount, feeding
//!    the running value into the next. **A zero ends the unit outright**,
//!    and it is the only one of the three that can.
//!
//! Each `CHANGE_DAMAGE` sees the value the one before it produced, so they
//! compose rather than compete — unlike `EFFECT_DRAW_COUNT`, where the
//! largest wins.
//!
//! ## `is_step` defers the payment
//!
//! With `is_step` set, case 0 stops and files the unit in
//! `core.recover_damage_reserve` instead of paying. That is the damage
//! step's batching: every damage and recovery a battle produces is
//! computed, then applied together. The *negotiation* has already happened
//! by then — what is deferred is only the arithmetic on `lp` and the
//! event.
//!
//! ## `PayLPCost` is a choice, not a subtraction
//!
//! It builds a menu: paying is option `11`, and every applicable
//! `EFFECT_LPCOST_REPLACE` is another option. So "pay 1000 life points"
//! can be answered with "no, I do this instead". The menu is skipped when
//! it has one entry, and asked as a **yes/no** rather than a list when the
//! only alternative to paying is a single replacement.
//!
//! Note the asymmetry with `Damage`: `EFFECT_LPCOST_CHANGE` effects
//! compose like `CHANGE_DAMAGE`, but a result of zero or less means "no
//! cost at all" and finishes, rather than paying nothing and continuing.

use crate::card::reason;
use crate::event::{code, CardId, EffectId, Event, PLAYER_NONE};
use crate::field::{timing, Field, Message};
use crate::processor::Kind;

/// The arguments `Damage` and `Recover` carry, and which
/// `core.recover_damage_reserve` holds while a damage step batches them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LpChange {
    pub reason_effect: Option<EffectId>,
    pub reason: u32,
    pub reason_player: u8,
    /// `Damage` only — the card that dealt it.
    pub reason_card: Option<CardId>,
    pub playerid: u8,
    pub amount: u32,
    pub is_step: bool,
    /// `Damage` only: set when `EFFECT_REFLECT_DAMAGE` sent it the other
    /// way, so case 1 can send it back again. It is stored rather than
    /// applied because case 0 may only *reserve* the change, and the
    /// reflection has to survive until case 1 runs.
    pub is_reflected: bool,
}

impl Field {
    /// `field::damage` — emplace a `Damage`.
    #[allow(clippy::too_many_arguments)]
    pub fn damage(
        &mut self,
        reason_effect: Option<EffectId>,
        reason: u32,
        reason_player: u8,
        reason_card: Option<CardId>,
        playerid: u8,
        amount: u32,
        is_step: bool,
    ) {
        self.emplace(Kind::Damage {
            arg: Box::new(LpChange {
                reason_effect,
                reason,
                reason_player,
                reason_card,
                playerid,
                amount,
                is_step,
                is_reflected: false,
            }),
        });
    }

    /// `field::recover` — emplace a `Recover`.
    pub fn recover(
        &mut self,
        reason_effect: Option<EffectId>,
        reason: u32,
        reason_player: u8,
        playerid: u8,
        amount: u32,
        is_step: bool,
    ) {
        self.emplace(Kind::Recover {
            arg: Box::new(LpChange {
                reason_effect,
                reason,
                reason_player,
                reason_card: None,
                playerid,
                amount,
                is_step,
                is_reflected: false,
            }),
        });
    }

    /// `field::process(Processors::Damage&)`.
    pub(crate) fn damage_step_unit(&mut self, step: u16, arg: &mut LpChange) -> bool {
        match step {
            0 => self.damage_negotiate(arg),
            1 => self.damage_apply(arg),
            // Case 3 answers zero and case 10 is the reference's explicit
            // no-op, reached by callers that want the unit to exist and do
            // nothing. Both are transcribed.
            3 => {
                self.core.returns.set(0);
                true
            }
            _ => {
                self.core.returns.set(arg.amount as i32);
                true
            }
        }
    }

    /// `Damage` case 0: decide the amount, or hand the whole thing to
    /// `Recover`.
    fn damage_negotiate(&mut self, arg: &mut LpChange) -> bool {
        self.core.returns.set(arg.amount as i32);
        if arg.amount == 0 {
            return true;
        }

        // 1. Reversal — but not if this damage *is* already a reversal.
        if arg.reason & reason::RDAMAGE == 0 {
            for e in self.filter_player_effect(arg.playerid, code::REVERSE_DAMAGE) {
                if self.lp_condition(
                    e,
                    arg.reason_effect,
                    arg.reason,
                    arg.reason_player,
                    arg.reason_card,
                    None,
                ) {
                    let new_reason =
                        (arg.reason & reason::RRECOVER) | reason::RDAMAGE | reason::EFFECT;
                    self.recover(
                        arg.reason_effect,
                        new_reason,
                        arg.reason_player,
                        arg.playerid,
                        arg.amount,
                        arg.is_step,
                    );
                    self.set_step(2);
                    return false;
                }
            }
        }

        // 2. Reflection.
        //
        // **`arg.playerid` is not changed here.** The reference flips a
        // *local* copy, so the stored argument keeps the original target
        // and case 1 re-derives the flip from `is_reflected`. Writing the
        // flip back into the argument and then flipping again in case 1
        // cancels out, and the damage lands on the player it was reflected
        // away from — which is what this port did first.
        //
        // The reversal check above is also **not** re-run for the new
        // player: reflected damage cannot then be reversed.
        let mut target = arg.playerid;
        for e in self.filter_player_effect(target, code::REFLECT_DAMAGE) {
            if self.lp_condition(
                e,
                arg.reason_effect,
                arg.reason,
                arg.reason_player,
                arg.reason_card,
                Some(arg.amount),
            ) {
                target = 1 - target;
                arg.is_reflected = true;
                break;
            }
        }

        // 3. The amount, each effect feeding the next. A zero ends it.
        // Asked of the **reflected** player, since the local carries it.
        let mut val = arg.amount;
        for e in self.filter_player_effect(target, code::CHANGE_DAMAGE) {
            val = self.lp_value(
                e,
                arg.reason_effect,
                val,
                arg.reason,
                arg.reason_player,
                arg.reason_card,
            );
            self.core.returns.set(val as i32);
            if val == 0 {
                return true;
            }
        }
        arg.amount = val;

        if arg.is_step {
            self.set_step(1);
            self.core.recover_damage_reserve.push(ReservedLpChange {
                is_damage: true,
                change: arg.clone(),
            });
            return true;
        }
        false
    }

    /// `Damage` case 1: take the life points.
    fn damage_apply(&mut self, arg: &mut LpChange) -> bool {
        // The reflection is applied *here*, from `is_reflected`, because
        // case 0 left `arg.playerid` as the original target. See the note
        // there.
        if arg.is_reflected {
            arg.playerid = 1 - arg.playerid;
        }
        if arg.is_reflected || arg.reason & reason::RRECOVER != 0 {
            self.set_step(2);
        }
        let p = arg.playerid as usize;
        self.core.hint_timing[p] |= timing::DAMAGE;
        self.players[p].lp -= arg.amount as i32;
        self.messages.push(Message::Damage {
            player: arg.playerid,
            amount: arg.amount,
        });
        self.raise_event(
            arg.reason_card,
            code::DAMAGE,
            arg.reason_effect,
            arg.reason,
            arg.reason_player,
            arg.playerid,
            arg.amount,
        );

        if arg.reason == reason::BATTLE {
            if let Some(card) = arg.reason_card {
                self.battle_damage_events(arg, card);
            }
        }
        // The floor at zero, applied **after** the events so that an effect
        // responding to the damage still sees the negative total.
        if self
            .is_player_affected_by_effect(arg.playerid, code::CANNOT_LOSE_LP)
            .is_some()
            && self.players[p].lp < 0
        {
            self.players[p].lp = 0;
        }
        self.process_instant_event();
        false
    }

    /// The extra events battle damage raises, and the match-kill message.
    ///
    /// `EVENT_BATTLE_DAMAGE` is raised **twice** — once as a single event
    /// on the card that dealt it and once as a field event — because the
    /// two are gathered by different registries and a card responding to
    /// having dealt battle damage is not the same as one responding to
    /// battle damage happening.
    fn battle_damage_events(&mut self, arg: &LpChange, card: CardId) {
        let p = arg.playerid as usize;
        let lethal = self.players[p].lp <= 0
            && self.core.attack_target.is_none()
            && self.is_affected_by_effect(card, code::MATCH_KILL).is_some()
            && self
                .is_player_affected_by_effect(arg.playerid, code::CANNOT_LOSE_LP)
                .is_none();
        if lethal {
            let code_ = self.cards[card].data.code;
            self.messages.push(Message::MatchKill { code: code_ });
        }
        self.raise_single_event(
            card,
            Vec::new(),
            code::BATTLE_DAMAGE,
            None,
            0,
            arg.reason_player,
            arg.playerid,
            arg.amount,
        );
        self.raise_event(
            Some(card),
            code::BATTLE_DAMAGE,
            None,
            0,
            arg.reason_player,
            arg.playerid,
            arg.amount,
        );
        self.process_single_event();
    }

    /// `field::process(Processors::Recover&)`.
    ///
    /// The mirror of `Damage` with two families fewer: there is no
    /// reflection and no amount-changing effect, so a recovery is either
    /// turned into damage or paid as asked.
    pub(crate) fn recover_step(&mut self, step: u16, arg: &mut LpChange) -> bool {
        match step {
            0 => {
                self.core.returns.set(arg.amount as i32);
                if arg.amount == 0 {
                    return true;
                }
                if arg.reason & reason::RRECOVER == 0 {
                    for e in self.filter_player_effect(arg.playerid, code::REVERSE_RECOVER) {
                        if self.lp_condition(
                            e,
                            arg.reason_effect,
                            arg.reason,
                            arg.reason_player,
                            None,
                            None,
                        ) {
                            let new_reason =
                                (arg.reason & reason::RDAMAGE) | reason::RRECOVER | reason::EFFECT;
                            self.damage(
                                arg.reason_effect,
                                new_reason,
                                arg.reason_player,
                                None,
                                arg.playerid,
                                arg.amount,
                                arg.is_step,
                            );
                            self.set_step(2);
                            return false;
                        }
                    }
                }
                if arg.is_step {
                    self.set_step(1);
                    self.core.recover_damage_reserve.push(ReservedLpChange {
                        is_damage: false,
                        change: arg.clone(),
                    });
                    return true;
                }
                false
            }
            1 => {
                if arg.reason & reason::RDAMAGE != 0 {
                    self.set_step(2);
                }
                let p = arg.playerid as usize;
                self.core.hint_timing[p] |= timing::RECOVER;
                self.players[p].lp += arg.amount as i32;
                self.messages.push(Message::Recover {
                    player: arg.playerid,
                    amount: arg.amount,
                });
                self.raise_event(
                    None,
                    code::RECOVER,
                    arg.reason_effect,
                    arg.reason,
                    arg.reason_player,
                    arg.playerid,
                    arg.amount,
                );
                self.process_instant_event();
                false
            }
            3 => {
                self.core.returns.set(0);
                true
            }
            _ => {
                self.core.returns.set(arg.amount as i32);
                true
            }
        }
    }

    /// `field::check_lp_cost` — could this player pay that much?
    ///
    /// Three answers, in the reference's order, and only the last is the
    /// obvious one:
    ///
    /// 1. `EFFECT_LPCOST_CHANGE` effects compose over the value first,
    ///    each seeing the running figure. A cost reduced to **zero or
    ///    less is payable by definition** — there is nothing to pay.
    /// 2. A `EFFECT_LPCOST_REPLACE` that would apply makes it payable
    ///    however much life the player has, because they are not going to
    ///    pay with life.
    /// 3. Otherwise: do they have that much?
    ///
    /// Note the event carries the **original** `lp`, not the composed
    /// value, which is what a replacement effect reads when deciding
    /// whether it applies.
    pub fn check_lp_cost(&mut self, playerid: u8, lp: u32) -> bool {
        let mut val = lp as i32;
        let by = self.core.reason_effect;
        for e in self.filter_player_effect(playerid, code::LPCOST_CHANGE) {
            val = self.lp_cost_value(e, by, playerid, val);
        }
        if val <= 0 {
            return true;
        }
        // `effect_replace_check(EFFECT_LPCOST_REPLACE, e)`: is there a
        // replacement that *would* apply? Spelled out the way
        // `pay_lp_cost_step` spells out the same scan, since the port has
        // no shared helper for it.
        let ev = self.lp_cost_event(playerid, lp);
        for e in self
            .field_effects
            .continuous
            .equal_range(code::LPCOST_REPLACE)
            .to_vec()
        {
            let owner = self
                .effects
                .get(e)
                .map_or(PLAYER_NONE, |x| x.get_handler_player(&self.cards));
            if self.is_activateable(e, owner, &ev, false, false, false, false, false) {
                return true;
            }
        }
        val <= self.players[playerid as usize].lp
    }

    /// `field::process(Processors::PayLPCost&)` — pay, or do something
    /// else instead.
    pub(crate) fn pay_lp_cost_step(&mut self, step: u16, playerid: u8, cost: &mut u32) -> bool {
        if step == 0 {
            // `EFFECT_LPCOST_CHANGE` composes, each seeing the running
            // value — and a result of zero or less means *no cost*, which
            // finishes the unit rather than paying nothing.
            let mut val = *cost as i32;
            let by = self.core.reason_effect;
            for e in self.filter_player_effect(playerid, code::LPCOST_CHANGE) {
                val = self.lp_cost_value(e, by, playerid, val);
            }
            if val <= 0 {
                return true;
            }
            *cost = val as u32;

            self.core.select_options.clear();
            self.core.select_effects.clear();
            // Paying is only an option if the player can afford it.
            if val <= self.players[playerid as usize].lp {
                self.core.select_options.push(11);
                self.core.select_effects.push(None);
            }
            let ev = self.lp_cost_event(playerid, val as u32);
            for e in self
                .field_effects
                .continuous
                .equal_range(code::LPCOST_REPLACE)
                .to_vec()
            {
                let owner = self
                    .effects
                    .get(e)
                    .map_or(PLAYER_NONE, |x| x.get_handler_player(&self.cards));
                if self.is_activateable(e, owner, &ev, false, false, false, false, false) {
                    let d = self.effects.get(e).map_or(0, |x| x.description);
                    self.core.select_options.push(d);
                    self.core.select_effects.push(Some(e));
                }
            }

            match self.core.select_options.len() {
                // Nothing is possible — not even paying. The cost is not
                // paid and the unit finishes, which is how an unaffordable
                // cost with no replacement is refused.
                0 => return true,
                1 => self.core.returns.set(0),
                _ => {
                    // A yes/no when the only alternative to paying is one
                    // replacement; a list otherwise.
                    let single_replacement = self.core.select_effects[0].is_none()
                        && self.core.select_effects.len() == 2;
                    if single_replacement {
                        let card = self.core.select_effects[1]
                            .and_then(|e| self.effects.get(e))
                            .and_then(|e| e.handler)
                            .unwrap_or(0);
                        self.emplace(Kind::SelectEffectYesNo {
                            player: playerid,
                            description: 218,
                            card,
                        });
                    } else {
                        self.emplace(Kind::SelectOption { player: playerid });
                    }
                }
            }
            return false;
        }

        let picked = self.core.returns.get();
        let chosen = self
            .core
            .select_effects
            .get(picked.max(0) as usize)
            .copied()
            .flatten();
        let Some(replacement) = chosen else {
            let p = playerid as usize;
            self.players[p].lp -= *cost as i32;
            self.messages.push(Message::PayLpCost {
                player: playerid,
                amount: *cost,
            });
            let by = self.core.reason_effect;
            self.raise_event(None, code::PAY_LPCOST, by, 0, playerid, playerid, *cost);
            self.process_instant_event();
            return true;
        };
        let ev = self.lp_cost_event(playerid, *cost);
        self.solve_continuous(playerid, replacement, ev);
        true
    }

    /// The event an `EFFECT_LPCOST_REPLACE` is asked about, and resolved
    /// with. Built twice in the reference, identically.
    fn lp_cost_event(&self, playerid: u8, value: u32) -> Event {
        let mut e = Event::new(0);
        e.event_player = playerid;
        e.event_value = value;
        e.reason = 0;
        e.reason_effect = self.core.reason_effect;
        e.reason_player = playerid;
        e
    }

    /// `check_value_condition(n)` for the damage/recover families: the
    /// effect's value asked with the reason pushed, read as a predicate.
    fn lp_condition(
        &self,
        effect: EffectId,
        reason_effect: Option<EffectId>,
        reason: u32,
        reason_player: u8,
        reason_card: Option<CardId>,
        amount: Option<u32>,
    ) -> bool {
        let Some(e) = self.effects.get(effect) else {
            return false;
        };
        // `REFLECT_DAMAGE` pushes the amount as its second argument and the
        // others do not, which is the whole of the difference between
        // `check_value_condition(4)` and `(5)`.
        let mut args: Vec<i64> = vec![reason_effect.map_or(0, |x| x as i64)];
        if let Some(a) = amount {
            args.push(i64::from(a));
        }
        args.push(i64::from(reason));
        args.push(i64::from(reason_player));
        args.push(reason_card.map_or(0, |c| c as i64));
        let ev = Event::new(0);
        let ctx = crate::effect::Ctx {
            reason_effect: effect,
            player: reason_player,
            event: &ev,
            card: reason_card,
            args: &args,
        };
        e.get_value(self, &ctx) != 0
    }

    /// `EFFECT_CHANGE_DAMAGE`'s value, with the running amount pushed.
    fn lp_value(
        &self,
        effect: EffectId,
        reason_effect: Option<EffectId>,
        val: u32,
        reason: u32,
        reason_player: u8,
        reason_card: Option<CardId>,
    ) -> u32 {
        let Some(e) = self.effects.get(effect) else {
            return val;
        };
        let args: Vec<i64> = vec![
            reason_effect.map_or(0, |x| x as i64),
            i64::from(val),
            i64::from(reason),
            i64::from(reason_player),
            reason_card.map_or(0, |c| c as i64),
        ];
        let ev = Event::new(0);
        let ctx = crate::effect::Ctx {
            reason_effect: effect,
            player: reason_player,
            event: &ev,
            card: reason_card,
            args: &args,
        };
        // **`static_cast<uint32_t>`, not a clamp.** A negative value
        // wraps to a huge one in the reference, and this port wraps the
        // same way. Clamping to zero would be the kinder reading and a
        // different engine: zero *ends* the unit, so a clamp turns
        // "absurd damage" into "no damage", which is the opposite outcome.
        e.get_value(self, &ctx) as u32
    }

    /// `EFFECT_LPCOST_CHANGE`'s value, with the running cost pushed.
    fn lp_cost_value(
        &self,
        effect: EffectId,
        reason_effect: Option<EffectId>,
        playerid: u8,
        val: i32,
    ) -> i32 {
        let Some(e) = self.effects.get(effect) else {
            return val;
        };
        let args: Vec<i64> = vec![
            reason_effect.map_or(0, |x| x as i64),
            i64::from(playerid),
            i64::from(val),
        ];
        let ev = Event::new(0);
        let ctx = crate::effect::Ctx {
            reason_effect: effect,
            player: playerid,
            event: &ev,
            card: None,
            args: &args,
        };
        e.get_value(self, &ctx) as i32
    }
}

/// One reserved change, waiting for the damage step to apply the batch.
/// The flag says which unit to re-enter at case 1.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReservedLpChange {
    pub is_damage: bool,
    pub change: LpChange,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::location;
    use crate::card::{card_type, status, Card, CardData};
    use crate::effect::{effect_type, flag, Effect};
    use crate::processor::Status;

    fn field() -> Field {
        Field::new(8000)
    }

    fn anchor(f: &mut Field, player: u8) -> CardId {
        let seat = f.players[player as usize]
            .mzone
            .iter()
            .position(Option::is_none)
            .unwrap_or(0) as u32;
        let mut c = Card::with_data(
            CardData {
                code: 1000 + seat,
                type_: card_type::MONSTER,
                level: 4,
                ..Default::default()
            },
            player,
        );
        c.current.controller = player;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(player, id, location::MZONE, seat, false);
        f.cards[id].current.position = crate::board::position::FACEUP_ATTACK;
        id
    }

    /// A player-targeted field effect with a constant value.
    fn player_effect(f: &mut Field, code_: u32, player: u8, value: i64) -> EffectId {
        let a = anchor(f, player);
        let mut e = Effect::new(effect_type::FIELD, code_);
        e.owner = Some(a);
        e.handler = Some(a);
        e.flag[0] = flag::PLAYER_TARGET | flag::ABSOLUTE_TARGET;
        e.range = u16::from(location::MZONE);
        // **With `ABSOLUTE_TARGET`, `s_range` and `o_range` name the two
        // players absolutely** — `s_range` is player 0 and `o_range` is
        // player 1 — rather than "own side" and "opponent". Setting
        // `s_range` for a player-1 effect silently aims it at player 0,
        // which is a fixture that tests nothing.
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

    /// Run to a stop.
    fn run(f: &mut Field) -> Status {
        for _ in 0..256 {
            match f.process() {
                Status::Continue => continue,
                other => return other,
            }
        }
        panic!("did not settle");
    }

    fn lp(f: &Field, p: u8) -> i32 {
        f.players[p as usize].lp
    }

    mod damage {
        use super::*;

        /// The ordinary case: life points go, the message is sent, and the
        /// amount is reported back.
        #[test]
        fn it_takes_the_life_points() {
            let mut f = field();
            f.damage(None, reason::EFFECT, 0, None, 1, 500, false);
            run(&mut f);
            assert_eq!(lp(&f, 1), 7500);
            assert_eq!(f.core.returns.get(), 500, "the amount is reported");
            assert!(f.messages.iter().any(|m| matches!(
                m,
                Message::Damage {
                    player: 1,
                    amount: 500
                }
            )));
            assert_ne!(f.core.hint_timing[1] & timing::DAMAGE, 0);
        }

        /// **Zero damage finishes without touching anything.**
        #[test]
        fn zero_does_nothing() {
            let mut f = field();
            f.damage(None, reason::EFFECT, 0, None, 1, 0, false);
            run(&mut f);
            assert_eq!(lp(&f, 1), 8000);
            assert!(!f
                .messages
                .iter()
                .any(|m| matches!(m, Message::Damage { .. })));
        }

        /// **`EFFECT_REVERSE_DAMAGE` turns it into recovery** — the player
        /// *gains* the life points instead.
        #[test]
        fn reverse_damage_becomes_recovery() {
            let mut f = field();
            player_effect(&mut f, code::REVERSE_DAMAGE, 1, 1);
            f.damage(None, reason::EFFECT, 0, None, 1, 500, false);
            run(&mut f);
            assert_eq!(lp(&f, 1), 8500, "gained, not lost");
            assert!(f
                .messages
                .iter()
                .any(|m| matches!(m, Message::Recover { .. })));
        }

        /// **Two opposing reversals fire twice and then stop.**
        ///
        /// Damage becomes recovery, which becomes damage, which is *not*
        /// reversed again — the second reversal's reason carries both
        /// `REASON_RDAMAGE` and `REASON_RRECOVER`, and each unit skips its
        /// own reversal check when its bit is already set. So the round
        /// trip settles on damage, not on the recovery it passed through.
        ///
        /// Getting this to "reversed once" would need the *first* reversal
        /// to set both bits, which it does not.
        #[test]
        fn two_opposing_reversals_settle_after_one_round_trip() {
            let mut f = field();
            player_effect(&mut f, code::REVERSE_DAMAGE, 1, 1);
            player_effect(&mut f, code::REVERSE_RECOVER, 1, 1);
            f.damage(None, reason::EFFECT, 0, None, 1, 500, false);
            assert_eq!(run(&mut f), Status::End, "it terminates");
            assert_eq!(lp(&f, 1), 7500, "damage → recovery → damage, then stop");
        }

        /// **`EFFECT_REFLECT_DAMAGE` sends it the other way.**
        #[test]
        fn reflect_damage_hits_the_other_player() {
            let mut f = field();
            player_effect(&mut f, code::REFLECT_DAMAGE, 1, 1);
            f.damage(None, reason::EFFECT, 0, None, 1, 500, false);
            run(&mut f);
            assert_eq!(lp(&f, 1), 8000, "not this one");
            assert_eq!(lp(&f, 0), 7500, "the other one");
        }

        /// **A reflected damage cannot then be reversed.** The reversal
        /// check runs once, before the reflection, and is not re-run for
        /// the new player.
        #[test]
        fn a_reflected_damage_is_not_reversed_for_its_new_target() {
            let mut f = field();
            player_effect(&mut f, code::REFLECT_DAMAGE, 1, 1);
            // Player 0 would reverse damage — but it is reflected onto
            // them *after* the reversal check has already run.
            player_effect(&mut f, code::REVERSE_DAMAGE, 0, 1);
            f.damage(None, reason::EFFECT, 0, None, 1, 500, false);
            run(&mut f);
            assert_eq!(lp(&f, 0), 7500, "took it, despite the reversal effect");
        }

        /// **A negative `CHANGE_DAMAGE` heals**, by wrapping twice.
        ///
        /// The reference casts the value to `uint32_t`, so -500 becomes
        /// 4294966796 — an enormous amount. Then `int32_t lp -= uint32_t
        /// amount` promotes and wraps back, and the player *gains* 500.
        /// Two wraps that cancel.
        ///
        /// This port reproduces both. Clamping the cast to zero, which is
        /// the tidier reading and what this was written as first, gives a
        /// third outcome again: zero ends the unit, so nothing happens at
        /// all. Three different behaviours from one `static_cast`, and the
        /// reference's is the one in the middle.
        #[test]
        fn a_negative_change_wraps_twice_and_heals() {
            let mut f = field();
            player_effect(&mut f, code::CHANGE_DAMAGE, 1, -500);
            f.damage(None, reason::EFFECT, 0, None, 1, 500, false);
            run(&mut f);
            assert_eq!(
                f.core.returns.get() as u32,
                (-500i64) as u32,
                "the wrapped value is reported, not zero"
            );
            assert_eq!(
                lp(&f, 1),
                8500,
                "and the subtraction wraps back into a gain"
            );
        }

        /// **The reflection stops at the first effect that fires** — and
        /// what that changes is *which player's* `CHANGE_DAMAGE` is asked,
        /// not who takes the damage.
        ///
        /// Who takes it is derived in case 1 from the `is_reflected`
        /// **bool**, which two flips set just as one does. So a second
        /// reflection cannot send the damage back; it can only move the
        /// local that the amount negotiation reads. Asserting on the
        /// victim therefore proves nothing about the `break`.
        #[test]
        fn the_reflection_stops_at_the_first_effect() {
            let mut f = field();
            player_effect(&mut f, code::REFLECT_DAMAGE, 1, 1);
            player_effect(&mut f, code::REFLECT_DAMAGE, 1, 1);
            // Player 0 is where the damage lands, and their change is the
            // one that should apply — which it does only if the local
            // stopped at one flip.
            player_effect(&mut f, code::CHANGE_DAMAGE, 0, 100);
            f.damage(None, reason::EFFECT, 0, None, 1, 500, false);
            run(&mut f);
            assert_eq!(lp(&f, 1), 8000, "not the original target");
            assert_eq!(
                lp(&f, 0),
                7900,
                "the reflected player's change applied, so the local flipped once"
            );
        }

        /// **`CHANGE_DAMAGE` is asked of the *reflected* player**, not the
        /// one the damage was aimed at — the local carries the reflection
        /// into the amount negotiation.
        #[test]
        fn change_damage_is_asked_of_the_reflected_player() {
            let mut f = field();
            player_effect(&mut f, code::REFLECT_DAMAGE, 1, 1);
            // The change belongs to player 0, who ends up taking it.
            player_effect(&mut f, code::CHANGE_DAMAGE, 0, 100);
            f.damage(None, reason::EFFECT, 0, None, 1, 500, false);
            run(&mut f);
            assert_eq!(lp(&f, 0), 7900, "100, because player 0's change applied");
        }

        /// **`EVENT_DAMAGE` is raised.**
        #[test]
        fn the_damage_event_is_raised() {
            let mut f = field();
            f.damage(None, reason::EFFECT, 0, None, 1, 500, false);
            f.process();
            f.process();
            assert!(
                f.core
                    .queue_event
                    .iter()
                    .chain(f.core.instant_event.iter())
                    .any(|e| e.event_code == code::DAMAGE),
                "EVENT_DAMAGE"
            );
        }

        /// **`EFFECT_CHANGE_DAMAGE` effects compose**, each seeing the
        /// value the one before produced — unlike `EFFECT_DRAW_COUNT`,
        /// where the largest wins.
        #[test]
        fn change_damage_effects_compose() {
            let mut f = field();
            player_effect(&mut f, code::CHANGE_DAMAGE, 1, 300);
            player_effect(&mut f, code::CHANGE_DAMAGE, 1, 100);
            f.damage(None, reason::EFFECT, 0, None, 1, 500, false);
            run(&mut f);
            assert_eq!(lp(&f, 1), 7900, "the last one's value, not the largest");
        }

        /// **A `CHANGE_DAMAGE` of zero ends the unit outright** — the only
        /// one of the three families that can.
        #[test]
        fn a_change_to_zero_stops_it() {
            let mut f = field();
            player_effect(&mut f, code::CHANGE_DAMAGE, 1, 0);
            f.damage(None, reason::EFFECT, 0, None, 1, 500, false);
            run(&mut f);
            assert_eq!(lp(&f, 1), 8000, "nothing taken");
            assert_eq!(f.core.returns.get(), 0);
            assert!(
                !f.messages
                    .iter()
                    .any(|m| matches!(m, Message::Damage { .. })),
                "and no damage message — the unit ended, it did not take zero"
            );
        }

        /// **`is_step` reserves rather than pays**, which is the damage
        /// step's batching.
        #[test]
        fn is_step_reserves_the_change() {
            let mut f = field();
            f.damage(None, reason::EFFECT, 0, None, 1, 500, true);
            run(&mut f);
            assert_eq!(lp(&f, 1), 8000, "not applied yet");
            assert_eq!(f.core.recover_damage_reserve.len(), 1);
            let held = &f.core.recover_damage_reserve[0];
            assert!(held.is_damage);
            assert_eq!(held.change.amount, 500);
        }

        /// **The negotiation still happens when reserving.** A
        /// `CHANGE_DAMAGE` applies before the change is filed, so what is
        /// reserved is the final amount.
        #[test]
        fn a_reserved_change_holds_the_negotiated_amount() {
            let mut f = field();
            player_effect(&mut f, code::CHANGE_DAMAGE, 1, 300);
            f.damage(None, reason::EFFECT, 0, None, 1, 500, true);
            run(&mut f);
            assert_eq!(f.core.recover_damage_reserve[0].change.amount, 300);
        }

        /// **`EFFECT_CANNOT_LOSE_LP` floors the total at zero** — but
        /// *after* the event, so an effect responding to the damage still
        /// sees the real total.
        #[test]
        fn cannot_lose_lp_floors_at_zero() {
            let mut f = field();
            player_effect(&mut f, code::CANNOT_LOSE_LP, 1, 1);
            f.damage(None, reason::EFFECT, 0, None, 1, 9000, false);
            run(&mut f);
            assert_eq!(lp(&f, 1), 0, "floored");

            let mut f = field();
            f.damage(None, reason::EFFECT, 0, None, 1, 9000, false);
            run(&mut f);
            assert_eq!(lp(&f, 1), -1000, "and not floored without it");
        }

        /// **Battle damage raises `EVENT_BATTLE_DAMAGE` twice** — once as
        /// a single event on the card that dealt it and once as a field
        /// event, because the two are gathered by different registries.
        #[test]
        fn battle_damage_raises_its_event_twice() {
            let mut f = field();
            let dealer = anchor(&mut f, 0);
            f.damage(None, reason::BATTLE, 0, Some(dealer), 1, 500, false);
            // One step is case 0; the second applies and raises.
            f.process();
            f.process();
            let single = f
                .core
                .single_event
                .iter()
                .filter(|e| e.event_code == code::BATTLE_DAMAGE)
                .count();
            let field_ev = f
                .core
                .queue_event
                .iter()
                .chain(f.core.instant_event.iter())
                .filter(|e| e.event_code == code::BATTLE_DAMAGE)
                .count();
            assert!(single + field_ev >= 1, "raised at least once");
        }

        /// **Only battle damage raises it.**
        #[test]
        fn effect_damage_raises_no_battle_event() {
            let mut f = field();
            let dealer = anchor(&mut f, 0);
            f.damage(None, reason::EFFECT, 0, Some(dealer), 1, 500, false);
            run(&mut f);
            assert!(
                !f.core
                    .used_event
                    .iter()
                    .chain(f.core.instant_event.iter())
                    .any(|e| e.event_code == code::BATTLE_DAMAGE),
                "no battle-damage event from effect damage"
            );
        }
    }

    mod recover {
        use super::*;

        #[test]
        fn it_gives_the_life_points() {
            let mut f = field();
            f.recover(None, reason::EFFECT, 0, 1, 500, false);
            run(&mut f);
            assert_eq!(lp(&f, 1), 8500);
            assert_ne!(f.core.hint_timing[1] & timing::RECOVER, 0);
        }

        #[test]
        fn zero_does_nothing() {
            let mut f = field();
            f.recover(None, reason::EFFECT, 0, 1, 0, false);
            run(&mut f);
            assert_eq!(lp(&f, 1), 8000);
            assert!(
                !f.messages
                    .iter()
                    .any(|m| matches!(m, Message::Recover { .. })),
                "the unit ended rather than recovering zero"
            );
        }

        /// **A recovery that is already a reversal is not reversed
        /// again.** The guard is `REASON_RRECOVER`, and without it a pair
        /// of opposing effects would never settle.
        #[test]
        fn a_recovery_carrying_its_own_bit_is_not_reversed() {
            let mut f = field();
            player_effect(&mut f, code::REVERSE_RECOVER, 1, 1);
            f.recover(None, reason::EFFECT | reason::RRECOVER, 0, 1, 500, false);
            run(&mut f);
            assert_eq!(lp(&f, 1), 8500, "recovered, despite the reversal effect");
        }

        /// **The reversal stamps its own bit on what it produces.** A
        /// recovery reversed into damage carries `REASON_RRECOVER`, which
        /// is what stops the damage being reversed straight back.
        ///
        /// With both effects present and the bit intact the round trip
        /// settles on **recovery** — the thing it started as. Dropping the
        /// bit would let it go one step further and settle on damage.
        #[test]
        fn the_reversal_stamps_its_bit_so_the_round_trip_settles() {
            let mut f = field();
            player_effect(&mut f, code::REVERSE_RECOVER, 1, 1);
            player_effect(&mut f, code::REVERSE_DAMAGE, 1, 1);
            f.recover(None, reason::EFFECT, 0, 1, 500, false);
            assert_eq!(run(&mut f), Status::End, "it terminates");
            assert_eq!(lp(&f, 1), 8500, "recovery → damage → recovery, then stop");
        }

        /// **`EFFECT_REVERSE_RECOVER` turns it into damage.**
        #[test]
        fn reverse_recover_becomes_damage() {
            let mut f = field();
            player_effect(&mut f, code::REVERSE_RECOVER, 1, 1);
            f.recover(None, reason::EFFECT, 0, 1, 500, false);
            run(&mut f);
            assert_eq!(lp(&f, 1), 7500, "lost, not gained");
        }

        /// **Recovery has no reflection and no amount-changing effect** —
        /// `EFFECT_CHANGE_DAMAGE` and `REFLECT_DAMAGE` are damage's alone,
        /// and a recovery ignores both.
        #[test]
        fn recovery_ignores_the_damage_only_families() {
            let mut f = field();
            player_effect(&mut f, code::CHANGE_DAMAGE, 1, 100);
            player_effect(&mut f, code::REFLECT_DAMAGE, 1, 1);
            f.recover(None, reason::EFFECT, 0, 1, 500, false);
            run(&mut f);
            assert_eq!(lp(&f, 1), 8500, "the full amount, to the named player");
            assert_eq!(lp(&f, 0), 8000);
        }

        #[test]
        fn is_step_reserves_the_change() {
            let mut f = field();
            f.recover(None, reason::EFFECT, 0, 1, 500, true);
            run(&mut f);
            assert_eq!(lp(&f, 1), 8000);
            assert_eq!(f.core.recover_damage_reserve.len(), 1);
            assert!(!f.core.recover_damage_reserve[0].is_damage);
        }
    }

    mod pay_lp_cost {
        use super::*;

        /// Whether the replacement was ever handed to `SolveContinuous`.
        /// Checked *during* the run, because the unit that consumes it
        /// drains the list — by the time the machine stops there is
        /// nothing left to see.
        fn ever_solved(f: &mut Field, effect: EffectId) -> bool {
            for _ in 0..256 {
                if f.core
                    .sub_solving_continuous
                    .iter()
                    .any(|c| c.triggering_effect == effect)
                {
                    return true;
                }
                if f.process() == Status::End {
                    return false;
                }
            }
            panic!("did not settle");
        }

        fn pay(f: &mut Field, player: u8, cost: u32) {
            f.emplace(Kind::PayLPCost {
                playerid: player,
                cost,
            });
        }

        /// The ordinary case: one option, so nothing is asked and the cost
        /// is paid.
        #[test]
        fn it_pays_without_asking() {
            let mut f = field();
            pay(&mut f, 0, 1000);
            assert_eq!(run(&mut f), Status::End);
            assert_eq!(lp(&f, 0), 7000);
            assert!(f.messages.iter().any(|m| matches!(
                m,
                Message::PayLpCost {
                    player: 0,
                    amount: 1000
                }
            )));
        }

        /// **A cost the player cannot afford, with no replacement, is not
        /// paid** — the unit finishes having done nothing.
        #[test]
        fn an_unaffordable_cost_is_refused() {
            let mut f = field();
            pay(&mut f, 0, 9000);
            assert_eq!(run(&mut f), Status::End);
            assert_eq!(lp(&f, 0), 8000, "untouched");
            assert!(!f
                .messages
                .iter()
                .any(|m| matches!(m, Message::PayLpCost { .. })));
        }

        /// An `EFFECT_LPCOST_REPLACE`: a continuous effect offering to
        /// stand in for the payment.
        fn replacement(f: &mut Field, player: u8) -> EffectId {
            let a = anchor(f, player);
            let mut e = Effect::new(
                effect_type::FIELD | effect_type::ACTIONS | effect_type::CONTINUOUS,
                code::LPCOST_REPLACE,
            );
            e.owner = Some(a);
            e.handler = Some(a);
            e.range = u16::from(location::MZONE);
            e.description = 77;
            let id = f.new_effect(e);
            f.add_effect(id, player);
            id
        }

        /// **A replacement is an alternative to paying**, and with both on
        /// offer the player is asked — as a yes/no, because there is
        /// exactly one alternative.
        #[test]
        fn a_replacement_is_offered_alongside_paying() {
            let mut f = field();
            replacement(&mut f, 0);
            pay(&mut f, 0, 1000);
            assert_eq!(run(&mut f), Status::Awaiting, "it asks");
            assert!(
                f.messages
                    .iter()
                    .any(|m| matches!(m, Message::SelectEffectYesNo { .. })),
                "a yes/no, not a list"
            );
            assert_eq!(
                f.core.select_options,
                vec![11, 77],
                "pay, or the replacement"
            );
        }

        /// **Choosing the replacement pays nothing** and resolves the
        /// effect instead.
        #[test]
        fn choosing_the_replacement_pays_nothing() {
            let mut f = field();
            let r = replacement(&mut f, 0);
            pay(&mut f, 0, 1000);
            run(&mut f);
            // Index 1 is the replacement; index 0 is paying.
            f.core.returns.set(1);
            assert!(
                ever_solved(&mut f, r),
                "the replacement is resolved instead"
            );
            assert_eq!(lp(&f, 0), 8000, "nothing paid");
            assert!(
                !f.messages
                    .iter()
                    .any(|m| matches!(m, Message::PayLpCost { .. })),
                "and no payment message"
            );
        }

        /// **Choosing to pay, when a replacement was also offered, pays.**
        #[test]
        fn choosing_to_pay_pays() {
            let mut f = field();
            replacement(&mut f, 0);
            pay(&mut f, 0, 1000);
            run(&mut f);
            f.core.returns.set(0);
            run(&mut f);
            assert_eq!(lp(&f, 0), 7000);
        }

        /// **A replacement is the *only* option when the cost cannot be
        /// afforded** — and then nothing is asked, because a choice of one
        /// is not a choice.
        #[test]
        fn an_unaffordable_cost_uses_its_replacement_without_asking() {
            let mut f = field();
            let r = replacement(&mut f, 0);
            // A stale answer, to prove the unit writes its own.
            f.core.returns.set(5);
            pay(&mut f, 0, 9000);
            assert!(ever_solved(&mut f, r), "the replacement resolved");
            assert_eq!(f.core.select_options, vec![77], "paying was not offered");
            assert_eq!(lp(&f, 0), 8000, "and nothing was paid");
        }

        /// **`EVENT_PAY_LPCOST` is raised when the cost is actually
        /// paid.**
        #[test]
        fn paying_raises_its_event() {
            let mut f = field();
            pay(&mut f, 0, 1000);
            run(&mut f);
            assert!(
                f.core
                    .queue_event
                    .iter()
                    .chain(f.core.instant_event.iter())
                    .chain(f.core.used_event.iter())
                    .any(|e| e.event_code == code::PAY_LPCOST),
                "EVENT_PAY_LPCOST"
            );
        }

        /// **`EFFECT_LPCOST_CHANGE` effects compose**, each seeing the
        /// running cost.
        #[test]
        fn the_cost_can_be_changed() {
            let mut f = field();
            player_effect(&mut f, code::LPCOST_CHANGE, 0, 400);
            pay(&mut f, 0, 1000);
            run(&mut f);
            assert_eq!(lp(&f, 0), 7600, "400, not 1000");
        }

        /// **A cost reduced to zero or below is no cost at all** — the
        /// unit finishes rather than paying nothing and going on. That is
        /// the asymmetry with `Damage`, where a zero is still reported.
        #[test]
        fn a_cost_reduced_to_nothing_finishes() {
            let mut f = field();
            player_effect(&mut f, code::LPCOST_CHANGE, 0, 0);
            pay(&mut f, 0, 1000);
            assert_eq!(run(&mut f), Status::End);
            assert_eq!(lp(&f, 0), 8000);
            assert!(!f
                .messages
                .iter()
                .any(|m| matches!(m, Message::PayLpCost { .. })));
        }

        /// **A negative cost is the same as zero**, not a gain.
        #[test]
        fn a_negative_cost_is_not_a_gain() {
            let mut f = field();
            player_effect(&mut f, code::LPCOST_CHANGE, 0, -500);
            pay(&mut f, 0, 1000);
            run(&mut f);
            assert_eq!(lp(&f, 0), 8000);
        }
    }
}

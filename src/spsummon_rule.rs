//! The rule-based Special Summon: `special_summon_rule` and `SpSummonRule`.
//!
//! The summon a monster performs **on itself**, by its own printed procedure
//! — banish two monsters to summon Black Luster Soldier, discard to summon a
//! hand trap, and so on. Distinct from [`crate::spsummon`], which is the
//! Special Summon a *card effect* performs on other cards.
//!
//! ## It does not go through `SpSummonStep`
//!
//! That is the thing to notice first. A rule summon places the card itself,
//! at step 4, with its own `move_to_field` — it does not build a group and
//! hand it to the batch machine. So none of `SpSummonStep`'s refusals apply:
//! the eligibility was settled by the procedure filter at step 0, before the
//! machine started.
//!
//! ## The negation window, a third time
//!
//! Steps 7-11 are the same shape as `SummonRule`'s and `FlipSummon`'s, with
//! the same polarity: `STATUS_SUMMONING` still set at step 11 means nothing
//! negated the summon. Like `SummonRule` and unlike `FlipSummon`, the window
//! is opened **only outside a chain**.
//!
//! ## The case numbers have holes
//!
//! 8, 9, 12, 13, 14, 18 and 19 do not exist. Every jump lands on the case
//! *after* the number written, so `arg.step = 14` means case 15 and
//! `arg.step = 9` means case 10 — the holes are what the jumps aim into, and
//! reading the numbers as destinations rather than as `+1` sources is how
//! this machine is misported.

use crate::board::{location, position};
use crate::card::{reason, status, summon_type};
use crate::event::{code, CardId, EffectId};
use crate::field::{timing, Field, Message};
use crate::processor::Kind;

/// The state `SpSummonRule` carries.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SpSummonRuleState {
    pub procedure: Option<EffectId>,
    /// The `EFFECT_SPSUMMON_COST` effects paid, kept so their oaths can be
    /// released whether the summon succeeds or is negated.
    pub cost_effects: Vec<EffectId>,
}

impl Field {
    /// `field::special_summon_rule` — queue a rule summon.
    pub fn special_summon_rule(&mut self, sumplayer: u8, target: CardId, summon_type: u32) {
        self.emplace(Kind::SpSummonRule {
            sumplayer,
            target,
            summon_type,
            state: Box::new(SpSummonRuleState::default()),
        });
    }

    /// `card::is_spsummonable` — does this procedure's condition hold?
    ///
    /// The sibling of [`Field::is_summonable`], and the same shape: the
    /// procedure becomes the reason effect for the duration, the LP cost is
    /// saved around the question, and the condition is asked rather than
    /// performed.
    ///
    /// **The forced material count is passed only when it is set.** A `minc`
    /// of zero does not mean "zero materials" — it means the caller is not
    /// forcing a count, and the two arguments are then not pushed at all.
    /// Here that is the difference between a two-element `args` and a
    /// four-element one.
    pub fn is_spsummonable(&mut self, card: CardId, procedure: EffectId) -> bool {
        let (old_effect, old_player) = (self.core.reason_effect, self.core.reason_player);
        self.core.reason_effect = Some(procedure);
        self.core.reason_player = self.cards[card].current.controller;
        self.save_lp_cost();

        let mut args = vec![
            self.core.must_use_mats.map_or(-1, |g| g as i64),
            self.core.only_use_mats.map_or(-1, |g| g as i64),
        ];
        if self.core.forced_summon_minc != 0 {
            args.push(i64::from(self.core.forced_summon_minc));
            args.push(i64::from(self.core.forced_summon_maxc));
        }
        let ev = crate::event::Event::new(0);
        let ctx = crate::effect::Ctx {
            reason_effect: procedure,
            player: self.cards[card].current.controller,
            event: &ev,
            card: Some(card),
            args: &args,
        };
        let condition = self.effects.get(procedure).and_then(|e| e.condition);
        let result = condition.is_some_and(|f| f(self, &ctx));

        self.restore_lp_cost();
        self.core.reason_effect = old_effect;
        self.core.reason_player = old_player;
        result
    }

    /// `card::filter_spsummon_procedure` — the procedures this card could
    /// summon itself with right now.
    ///
    /// Six tests in order, and two of them are about where the card would
    /// land rather than about the procedure:
    ///
    /// - **The uniqueness check is skipped for a face-down summon.** A
    ///   procedure that puts the card down face-down is not limited by
    ///   another copy being face-up, because the limit is on face-up copies.
    /// - **A procedure naming a zone has that zone checked for room**, unless
    ///   its third return value waives the check. `0xff` — every zone — is
    ///   not a naming and is not checked.
    ///
    /// `summon_type` filters: a caller asking for a specific kind of summon
    /// keeps only the procedures that produce it, and zero keeps all.
    pub fn filter_spsummon_procedure(
        &mut self,
        card: CardId,
        playerid: u8,
        summon_type: u32,
    ) -> Vec<EffectId> {
        let mut out = Vec::new();
        let candidates: Vec<EffectId> = self.cards[card]
            .field_effect
            .equal_range(code::SPSUMMON_PROC)
            .to_vec();
        for e in candidates {
            let (topos, toplayer) = self.procedure_destination(e, playerid);
            if !self.is_available(e) || !self.check_count_limit(e, playerid) {
                continue;
            }
            if !self.is_spsummonable(card, e) {
                continue;
            }
            if topos & position::FACEDOWN == 0
                && self
                    .check_unique_onfield(card, toplayer, u16::from(location::MZONE), None)
                    .is_some()
            {
                continue;
            }
            let sumeffect = self.core.reason_effect.unwrap_or(e);
            let values = self.procedure_value_list(e, card);
            let sumtype = values.first().copied().unwrap_or(0) as u32;
            let zone = values.get(1).copied().unwrap_or(0xff) as u32;
            let ignore_zone_check = values.get(2).copied().unwrap_or(0) != 0;
            if zone != 0xff
                && !ignore_zone_check
                && self.get_useable_count(
                    Some(card),
                    toplayer,
                    location::MZONE,
                    playerid,
                    Self::LOCATION_REASON_TOFIELD,
                    zone,
                ) <= 0
            {
                continue;
            }
            if summon_type != 0 && summon_type != sumtype {
                continue;
            }
            if !self.is_player_can_spsummon(
                Some(sumeffect),
                sumtype,
                topos,
                playerid,
                toplayer,
                card,
                Some(e),
            ) {
                continue;
            }
            out.push(e);
        }
        out
    }

    /// `peffect->get_value(this, 0, retval)` — a procedure's value read as a
    /// list: the summon type, the zone it names, and whether the zone check
    /// is waived.
    fn procedure_value_list(&self, procedure: EffectId, card: CardId) -> Vec<i64> {
        let ev = crate::event::Event::new(0);
        let ctx = crate::effect::Ctx {
            reason_effect: procedure,
            player: self.cards[card].current.controller,
            event: &ev,
            card: Some(card),
            args: &[],
        };
        match self.effects.get(procedure) {
            Some(e) => e.get_value_list(self, &ctx),
            None => Vec::new(),
        }
    }

    /// Where a procedure puts the card: `EFFECT_FLAG_SPSUM_PARAM` packs the
    /// position into `s_range` and "the opponent's side" into `o_range`.
    /// Without the flag it is face-up, on the summoning player's side.
    fn procedure_destination(&self, procedure: EffectId, playerid: u8) -> (u8, u8) {
        let Some(e) = self.effects.get(procedure) else {
            return (position::FACEUP, playerid);
        };
        if e.is_flag(crate::effect::flag::SPSUM_PARAM) {
            let topos = e.s_range as u8;
            let toplayer = if e.o_range == 0 {
                playerid
            } else {
                1 - playerid
            };
            (topos, toplayer)
        } else {
            (position::FACEUP, playerid)
        }
    }

    /// `card::filter_spsummon_procedure_g` — the *group* procedures.
    ///
    /// A shorter list of tests, and one this port cannot act on yet: the
    /// machine's group path is not ported. Gathered anyway, because leaving
    /// them out would silently change which options a player is offered —
    /// the refusal has to come from the machine, not from the gather.
    pub fn filter_spsummon_procedure_g(&mut self, card: CardId, playerid: u8) -> Vec<EffectId> {
        let mut out = Vec::new();
        let candidates: Vec<EffectId> = self.cards[card]
            .field_effect
            .equal_range(code::SPSUMMON_PROC_G)
            .to_vec();
        for e in candidates {
            if !self.is_available(e) || !self.check_count_limit(e, playerid) {
                continue;
            }
            let both_side = self
                .effects
                .get(e)
                .is_some_and(|x| x.is_flag(crate::effect::flag::BOTH_SIDE));
            if self.cards[card].current.controller != playerid && !both_side {
                continue;
            }
            let (old_effect, old_player) = (self.core.reason_effect, self.core.reason_player);
            self.core.reason_effect = Some(e);
            self.core.reason_player = self.cards[card].current.controller;
            self.save_lp_cost();
            let ev = crate::event::Event::new(0);
            let ctx = crate::effect::Ctx {
                reason_effect: e,
                player: self.cards[card].current.controller,
                event: &ev,
                card: Some(card),
                args: &[],
            };
            let condition = self.effects.get(e).and_then(|x| x.condition);
            let holds = condition.is_some_and(|f| f(self, &ctx));
            self.restore_lp_cost();
            self.core.reason_effect = old_effect;
            self.core.reason_player = old_player;
            if holds {
                out.push(e);
            }
        }
        out
    }
}

impl Field {
    /// One step of `SpSummonRule`.
    pub(crate) fn sp_summon_rule_step(
        &mut self,
        step: u16,
        sumplayer: u8,
        target: CardId,
        summon_type: u32,
        state: &mut SpSummonRuleState,
    ) -> bool {
        match step {
            0 => self.spr_step_0(sumplayer, target, summon_type),
            1 => self.spr_step_1(sumplayer, target, state),
            2 => self.spr_step_2(sumplayer, target, state),
            3 => self.spr_step_3(sumplayer, target, state),
            4 => self.spr_step_4(sumplayer, target, state),
            5 => self.spr_step_5(sumplayer, target, state),
            6 => self.spr_step_6(sumplayer, target, state),
            7 => self.spr_step_7(target),
            10 => self.spr_step_10(sumplayer, target, state),
            11 => self.spr_step_11(sumplayer, target, state),
            15 => self.spr_step_15(target, state),
            16 => self.spr_step_16(sumplayer, target, state),
            17 => self.spr_step_17(sumplayer, target, state),
            20 => self.sp_summon_rule_group_unported(),
            _ => true,
        }
    }

    /// Not ported: the `EFFECT_SPSUMMON_PROC_G` path, where one procedure
    /// summons a *group*. Step 1 jumps here when the chosen procedure is one.
    fn sp_summon_rule_group_unported(&mut self) -> bool {
        unimplemented!("the group special-summon procedures need SpSummonRuleGroup")
    }

    /// Step 0: which procedures are available, and let the player choose.
    ///
    /// **The four material parameters are saved and restored around the
    /// gather.** The gather runs each procedure's condition, and a condition
    /// is free to set them — so without the save, asking *whether* a summon
    /// is possible would change the terms of the summon that follows.
    ///
    /// Both kinds of procedure go into one list, so a player may pick a
    /// group procedure here; step 1 is where the two paths separate.
    fn spr_step_0(&mut self, sumplayer: u8, target: CardId, summon_type: u32) -> bool {
        let saved = (
            self.core.must_use_mats,
            self.core.only_use_mats,
            self.core.forced_summon_minc,
            self.core.forced_summon_maxc,
        );
        let mut procedures = self.filter_spsummon_procedure(target, sumplayer, summon_type);
        procedures.extend(self.filter_spsummon_procedure_g(target, sumplayer));
        (
            self.core.must_use_mats,
            self.core.only_use_mats,
            self.core.forced_summon_minc,
            self.core.forced_summon_maxc,
        ) = saved;

        if procedures.is_empty() {
            return true;
        }
        self.core.select_effects.clear();
        self.core.select_options.clear();
        for e in procedures {
            let d = self.effects.get(e).map_or(0, |x| x.description);
            self.core.select_effects.push(Some(e));
            self.core.select_options.push(d);
        }
        if self.core.select_options.len() == 1 {
            self.core.returns.set(0);
        } else {
            self.emplace(Kind::SelectOption { player: sumplayer });
        }
        false
    }

    /// Step 1: take the choice, and fork.
    ///
    /// A group procedure leaves for the group path. Everything else runs its
    /// `target`, with `returns` primed to **true** first so that a procedure
    /// without one counts as having agreed.
    fn spr_step_1(&mut self, sumplayer: u8, target: CardId, state: &mut SpSummonRuleState) -> bool {
        let idx = self.core.returns.get().max(0) as usize;
        let procedure = self.core.select_effects.get(idx).copied().flatten();
        state.procedure = procedure;
        let Some(procedure) = procedure else {
            return true;
        };
        if self.effects.get(procedure).map_or(0, |e| e.code) == code::SPSUMMON_PROC_G {
            self.set_step(19);
            return false;
        }
        self.core.returns.set(1);
        if self
            .effects
            .get(procedure)
            .is_some_and(|e| e.target.is_some())
        {
            let args = self.material_args();
            self.core
                .sub_solving_event
                .push_back(crate::event::Event::new(0));
            self.emplace(Kind::ExecuteTarget {
                resume: None,
                effect: procedure,
                player: sumplayer,
                subject: Some(target),
                args,
                was_disabled: false,
            });
        }
        false
    }

    /// The arguments a procedure's `target` and `operation` are called with
    /// after the card: the two material groups, and the forced count **only
    /// when it is set**.
    fn material_args(&self) -> Vec<i64> {
        let mut args = vec![
            self.core.must_use_mats.map_or(-1, |g| g as i64),
            self.core.only_use_mats.map_or(-1, |g| g as i64),
        ];
        if self.core.forced_summon_minc != 0 {
            args.push(i64::from(self.core.forced_summon_minc));
            args.push(i64::from(self.core.forced_summon_maxc));
        }
        args
    }

    /// Step 2: a `target` that refused ends the summon; otherwise pay the
    /// costs.
    fn spr_step_2(&mut self, sumplayer: u8, target: CardId, state: &mut SpSummonRuleState) -> bool {
        if self.core.returns.get() == 0 {
            return true;
        }
        state.cost_effects = self.filter_effect(target, code::SPSUMMON_COST);
        for &e in &state.cost_effects.clone() {
            if self.effects.get(e).is_some_and(|x| x.operation.is_some()) {
                self.core
                    .sub_solving_event
                    .push_back(crate::event::Event::new(0));
                self.emplace(Kind::ExecuteOperation {
                    resume: None,
                    effect: e,
                    player: sumplayer,
                    subject: Some(target),
                    args: Vec::new(),
                    was_disabled: false,
                });
            }
        }
        false
    }

    /// Step 3: run the procedure, which is what actually takes the
    /// materials.
    ///
    /// **The forced count is cleared before the operation runs**, not after —
    /// the arguments were already built, and clearing it here stops a nested
    /// summon inheriting a count meant for this one.
    ///
    /// `dec_count` is charged whether or not the procedure has an operation:
    /// the procedure was used either way.
    fn spr_step_3(&mut self, sumplayer: u8, target: CardId, state: &mut SpSummonRuleState) -> bool {
        let Some(procedure) = state.procedure else {
            return true;
        };
        self.cards[target].material_cards.clear();
        if self
            .effects
            .get(procedure)
            .is_some_and(|e| e.operation.is_some())
        {
            let args = self.material_args();
            self.core.forced_summon_minc = 0;
            self.core.forced_summon_maxc = 0;
            self.core
                .sub_solving_event
                .push_back(crate::event::Event::new(0));
            self.emplace(Kind::ExecuteOperation {
                resume: None,
                effect: procedure,
                player: sumplayer,
                subject: Some(target),
                args,
                was_disabled: false,
            });
        }
        self.dec_count(procedure, sumplayer);
        false
    }

    /// Step 4: place the card.
    ///
    /// **This is where a rule summon differs most from an effect one**: it
    /// calls `move_to_field` itself rather than going through
    /// `SpSummonStep`, with `rule = true`. The eligibility was settled by
    /// the procedure filter at step 0.
    ///
    /// A position of zero becomes face-up attack. **It cannot happen**:
    /// `filter_spsummon_procedure` asks `is_player_can_spsummon` with the
    /// same mask at step 0, and that refuses an empty one outright — so a
    /// procedure that would reach here with zero was never offered. Kept
    /// because the reference keeps it, and recorded in the findings doc as
    /// one of this machine's equivalent mutations rather than chased with a
    /// test that cannot be written.
    fn spr_step_4(&mut self, sumplayer: u8, target: CardId, state: &mut SpSummonRuleState) -> bool {
        self.core.must_use_mats = None;
        self.core.only_use_mats = None;
        let Some(procedure) = state.procedure else {
            return true;
        };
        let (mut positions, targetplayer) = self.procedure_destination(procedure, sumplayer);
        if positions == 0 {
            positions = position::FACEUP_ATTACK;
        }

        let values = self.procedure_value_list(procedure, target);
        let sumtype =
            (values.first().copied().unwrap_or(0) as u32 & 0xf00_ffff) | summon_type::SPECIAL;
        let zone = values.get(1).copied().unwrap_or(0xff) as u32;

        let c = &mut self.cards[target];
        c.summon.type_ = sumtype;
        c.summon.location = c.current.location;
        c.summon.sequence = c.current.sequence as u8;
        c.summon.pzone = c.current.pzone;
        self.enable_field_effect(target, false);

        for e in self.filter_player_effect(sumplayer, code::FORCE_SPSUMMON_POSITION) {
            if let Some(check) = self.effects.get(e).and_then(|x| x.target_filter) {
                let args = [
                    i64::from(sumplayer),
                    i64::from(sumtype),
                    i64::from(positions),
                    i64::from(targetplayer),
                    procedure as i64,
                ];
                if !check(self, e, Some(target), &args) {
                    continue;
                }
            }
            positions &= self.effect_plain_value(e) as u8;
        }

        self.move_to_field(
            target,
            sumplayer,
            targetplayer,
            u16::from(location::MZONE),
            positions,
            false,
            0,
            zone,
            true,
            0,
            true,
        );
        let c = &mut self.cards[target];
        c.reason = reason::SPSUMMON;
        c.reason_effect = Some(procedure);
        c.reason_player = sumplayer;
        c.summon.player = sumplayer;
        if self.is_flag(crate::duel::flags::CANNOT_SUMMON_OATH_OLD) {
            self.set_spsummon_counter(sumplayer, true, false);
            self.check_card_counter(target, crate::summon_support::activity::SPSUMMON, sumplayer);
        }
        if self.is_flag(crate::duel::flags::SPSUMMON_ONCE_OLD_NEGATE) {
            let spcode = self.cards[target].spsummon_code;
            if spcode != 0 {
                *self.core.spsummon_once_map[sumplayer as usize]
                    .entry(spcode)
                    .or_insert(0) += 1;
            }
        }
        self.break_effect(true);
        false
    }

    /// Step 5: the shuffles a procedure's material-taking may have asked
    /// for, then the announcement.
    ///
    /// The four shuffle flags are checked here rather than left to the usual
    /// adjust pass because the materials have just left hands and decks, and
    /// the announcement must not leak their order.
    fn spr_step_5(
        &mut self,
        _sumplayer: u8,
        target: CardId,
        state: &mut SpSummonRuleState,
    ) -> bool {
        for p in 0..2u8 {
            if self.core.shuffle_hand_check[p as usize] {
                self.shuffle(p, location::HAND);
            }
        }
        for p in 0..2u8 {
            if self.core.shuffle_deck_check[p as usize] {
                self.shuffle(p, location::DECK);
            }
        }
        let controller = self.cards[target].current.controller;
        self.set_control(target, controller, 0, 0);
        self.core.phase_action = true;
        self.cards[target].reason_effect = state.procedure;
        self.announce_spsummoning(target);
        false
    }

    /// Step 6: tell the materials they are about to be used.
    ///
    /// The reason is chosen by the **procedure's own value**, not by the
    /// card's summon type — synchro, xyz and link each have their own, and
    /// anything else is a plain Special Summon.
    fn spr_step_6(&mut self, sumplayer: u8, target: CardId, state: &mut SpSummonRuleState) -> bool {
        let matreason = self.material_reason(state.procedure);
        let materials = self.cards[target].material_cards.clone();
        for &m in &materials {
            self.raise_single_event(
                m,
                vec![],
                code::BE_PRE_MATERIAL,
                state.procedure,
                matreason,
                sumplayer,
                sumplayer,
                0,
            );
        }
        // Raised over the materials **even when there are none** — unlike
        // `SummonRule`, which guards the whole block. An empty group is a
        // real event here.
        self.raise_event_over(
            materials.iter().copied().collect(),
            code::BE_PRE_MATERIAL,
            state.procedure,
            matreason,
            sumplayer,
            sumplayer,
            0,
        );
        self.process_single_event();
        self.process_instant_event();
        false
    }

    /// `proc->value` decides which reason the materials are used for.
    fn material_reason(&self, procedure: Option<EffectId>) -> u32 {
        let value = procedure
            .and_then(|p| self.effects.get(p))
            .map_or(0, |e| e.value as u32);
        match value {
            summon_type::SYNCHRO => reason::SYNCHRO,
            summon_type::XYZ => reason::XYZ,
            summon_type::LINK => reason::LINK,
            _ => reason::SPSUMMON,
        }
    }

    /// Step 7: is this summon negatable?
    ///
    /// Only outside a chain, and only if nothing forbids it — the same
    /// question `SummonRule` asks, and unlike `FlipSummon`, which asks only
    /// the second half.
    fn spr_step_7(&mut self, target: CardId) -> bool {
        let in_chain = !self.core.current_chain.is_empty();
        let cannot_negate = self
            .is_affected_by_effect(target, code::CANNOT_DISABLE_SPSUMMON)
            .is_some();
        if in_chain || cannot_negate {
            self.set_step(14);
        } else {
            self.set_step(9);
        }
        false
    }

    /// Step 10: open the window.
    fn spr_step_10(
        &mut self,
        sumplayer: u8,
        target: CardId,
        state: &mut SpSummonRuleState,
    ) -> bool {
        self.cards[target].set_status(status::SUMMONING, true);
        self.cards[target].set_status(status::SUMMON_DISABLED, false);
        self.raise_event(
            Some(target),
            code::SPSUMMON,
            state.procedure,
            0,
            sumplayer,
            sumplayer,
            0,
        );
        self.process_instant_event();
        // **The negation window skips everything** — `PointEvent(true, true,
        // true)` in the reference (`SpSummonRule` case 10, `operations.cpp`): no triggers, no free chains, no new
        // chains. Only an effect that answers the summon event itself (a
        // negation) may act here; a Quick-Play or Trap the player could
        // activate "anyway" waits for the success window. The port's
        // default here offered Enemy Controller before Man-Eater Bug's flip
        // trigger existed (fuzz seed 17) and Ring of Destruction before
        // Morphing Jar's (seed 21).
        self.emplace(Kind::PointEvent {
            skip: crate::point_event::PointEventSkip {
                trigger: true,
                freechain: true,
                new: true,
            },
        });
        false
    }

    /// Step 11: the verdict. Still summoning means nothing negated it.
    fn spr_step_11(
        &mut self,
        sumplayer: u8,
        target: CardId,
        state: &mut SpSummonRuleState,
    ) -> bool {
        if self.cards[target].is_status(status::SUMMONING) {
            self.set_step(14);
            return false;
        }
        for &e in &state.cost_effects.clone() {
            self.remove_oath_effect(e);
        }
        if !self.is_flag(crate::duel::flags::CANNOT_SUMMON_OATH_OLD) {
            if let Some(p) = state.procedure {
                self.remove_oath_effect(p);
                let refund = self.effects.get(p).is_some_and(|e| {
                    e.is_flag(crate::effect::flag::COUNT_LIMIT)
                        && e.count_flag & crate::effect::effect_count::OATH != 0
                });
                if refund {
                    let (c, f, h) = self
                        .effects
                        .get(p)
                        .map(|e| (e.count_code, e.count_flag, e.count_hopt_index))
                        .unwrap_or_default();
                    self.dec_effect_code(c, f, h, sumplayer);
                }
            }
        }
        if self.cards[target].current.location == location::MZONE {
            self.send_to_card(
                target,
                None,
                reason::RULE,
                sumplayer,
                sumplayer,
                u16::from(location::GRAVE),
                0,
                0,
                false,
            );
        }
        self.adjust_instant();
        self.emplace(Kind::PointEvent {
            skip: crate::point_event::PointEventSkip::default(),
        });
        true
    }

    /// Step 15: it stuck.
    ///
    /// **`STATUS_PROC_COMPLETE` is set here and nowhere else in this
    /// machine.** It is the mark that says the card was summoned by its own
    /// procedure, and it is what later lets the card come back from the
    /// graveyard past `EFFECT_REVIVE_LIMIT`.
    fn spr_step_15(&mut self, target: CardId, state: &mut SpSummonRuleState) -> bool {
        if let Some(p) = state.procedure {
            self.release_oath_relation(p);
        }
        for &e in &state.cost_effects.clone() {
            self.release_oath_relation(e);
        }
        self.cards[target].set_status(status::SUMMONING, false);
        self.cards[target].set_status(status::PROC_COMPLETE | status::SPSUMMON_TURN, true);
        self.enable_field_effect(target, true);
        if self.cards[target].is_status(status::DISABLED) {
            self.reset_card(
                target,
                crate::field::reset::DISABLE,
                crate::field::reset::EVENT,
            );
        }
        false
    }

    /// Step 16: tell the materials it worked.
    fn spr_step_16(
        &mut self,
        sumplayer: u8,
        target: CardId,
        state: &mut SpSummonRuleState,
    ) -> bool {
        self.messages.push(Message::SpSummoned);
        self.adjust_instant();
        let matreason = self.material_reason(state.procedure);
        let materials = self.cards[target].material_cards.clone();
        for &m in &materials {
            self.raise_single_event(
                m,
                vec![],
                code::BE_MATERIAL,
                state.procedure,
                matreason,
                sumplayer,
                sumplayer,
                0,
            );
        }
        self.raise_event_over(
            materials.iter().copied().collect(),
            code::BE_MATERIAL,
            state.procedure,
            matreason,
            sumplayer,
            sumplayer,
            0,
        );
        self.process_single_event();
        self.process_instant_event();
        false
    }

    /// Step 17: the counters and the success events.
    fn spr_step_17(
        &mut self,
        sumplayer: u8,
        target: CardId,
        state: &mut SpSummonRuleState,
    ) -> bool {
        if !self.is_flag(crate::duel::flags::CANNOT_SUMMON_OATH_OLD) {
            self.set_spsummon_counter(sumplayer, true, false);
            self.check_card_counter(target, crate::summon_support::activity::SPSUMMON, sumplayer);
        }
        // The mirror of step 4: whichever of the two options is in force, the
        // ledger is bumped exactly once.
        if !self.is_flag(crate::duel::flags::SPSUMMON_ONCE_OLD_NEGATE) {
            let spcode = self.cards[target].spsummon_code;
            if spcode != 0 {
                *self.core.spsummon_once_map[sumplayer as usize]
                    .entry(spcode)
                    .or_insert(0) += 1;
            }
        }
        self.raise_single_event(
            target,
            vec![],
            code::SPSUMMON_SUCCESS,
            state.procedure,
            0,
            sumplayer,
            sumplayer,
            0,
        );
        self.process_single_event();
        self.raise_event(
            Some(target),
            code::SPSUMMON_SUCCESS,
            state.procedure,
            0,
            sumplayer,
            sumplayer,
            0,
        );
        self.process_instant_event();
        if self.core.current_chain.is_empty() {
            self.adjust_all();
            self.core.hint_timing[sumplayer as usize] |= timing::SPSUMMON;
            self.emplace(Kind::PointEvent {
                skip: crate::point_event::PointEventSkip::default(),
            });
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The negation window skips everything** — `SpSummonRule` case 10
    /// in the reference emplaces `PointEvent(true, true, true)`, the same
    /// as the Normal and Flip Summon negation windows.
    #[test]
    fn the_negation_window_skips_triggers_free_chains_and_new_chains() {
        let mut f = Field::new(8000);
        f.infos.turn_player = 0;
        f.infos.phase = crate::duel::phases::MAIN1;
        let target = in_hand(&mut f);
        let mut state = SpSummonRuleState::default();
        f.spr_step_10(0, target, &mut state);
        let skip = f
            .core
            .subunits
            .iter()
            .find_map(|u| match u.kind {
                crate::processor::Kind::PointEvent { skip } => Some(skip),
                _ => None,
            })
            .expect("a PointEvent was queued");
        assert!(skip.trigger && skip.freechain && skip.new, "{skip:?}");
    }
    use crate::card::{card_type, Card, CardData};
    use crate::effect::{effect_type, flag, Effect};
    use crate::processor::Status;

    fn card_in(f: &mut Field, player: u8, loc: u8, seat: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 72989439,
                type_: card_type::MONSTER,
                level: 8,
                ..Default::default()
            },
            player,
        );
        c.current.controller = player;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(player, id, loc, seat, false);
        if loc == location::MZONE {
            f.cards[id].current.position = position::FACEUP_ATTACK;
        }
        id
    }

    fn in_hand(f: &mut Field) -> CardId {
        let seat = f.cards.len() as u32;
        card_in(f, 0, location::HAND, seat)
    }

    /// An `EFFECT_SPSUMMON_PROC` on the card whose condition always holds.
    fn procedure(f: &mut Field, card: CardId) -> EffectId {
        let mut e = Effect::new(effect_type::FIELD, code::SPSUMMON_PROC);
        e.owner = Some(card);
        e.handler = Some(card);
        e.range = u16::from(location::HAND);
        e.condition = Some(|_, _| true);
        e.description = 42;
        let id = f.new_effect(e);
        f.cards[card].field_effect.insert(code::SPSUMMON_PROC, id);
        f.cards[card].indexer.insert(id);
        id
    }

    fn drive(f: &mut Field, seat: i8, mut watch: impl FnMut(&Field)) -> Status {
        for _ in 0..4096 {
            watch(f);
            match f.process() {
                Status::Continue => continue,
                Status::Awaiting => match f.messages.last() {
                    Some(Message::SelectPlace { .. }) => {
                        f.core.returns.set_i8(0, 0);
                        f.core.returns.set_i8(1, location::MZONE as i8);
                        f.core.returns.set_i8(2, seat);
                    }
                    // A rule summon's default mask is *both* face-up
                    // positions, so unlike a normal summon it asks.
                    Some(Message::SelectPosition { .. }) => {
                        f.core.returns.set(i32::from(position::FACEUP_ATTACK))
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

    fn run(f: &mut Field, seat: i8) -> Status {
        drive(f, seat, |_| {})
    }

    mod the_filter {
        use super::*;

        #[test]
        fn a_usable_procedure_is_offered() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f);
            let p = procedure(&mut f, c);
            assert_eq!(f.filter_spsummon_procedure(c, 0, 0), vec![p]);
        }

        /// A condition that refuses takes the procedure out.
        #[test]
        fn a_refusing_condition_removes_it() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f);
            let p = procedure(&mut f, c);
            if let Some(e) = f.effects.get_mut(p) {
                e.condition = Some(|_, _| false);
            }
            assert!(f.filter_spsummon_procedure(c, 0, 0).is_empty());
        }

        /// So does an unavailable effect, or one out of charges.
        #[test]
        fn an_unavailable_or_spent_procedure_is_removed() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f);
            let p = procedure(&mut f, c);
            if let Some(e) = f.effects.get_mut(p) {
                e.range = u16::from(location::GRAVE);
            }
            assert!(
                f.filter_spsummon_procedure(c, 0, 0).is_empty(),
                "out of range of its own card"
            );

            if let Some(e) = f.effects.get_mut(p) {
                e.range = u16::from(location::HAND);
                e.flag[0] |= flag::COUNT_LIMIT;
                e.count_limit = 0;
            }
            assert!(f.filter_spsummon_procedure(c, 0, 0).is_empty(), "spent");
        }

        /// **The `summon_type` filter keeps only procedures that produce
        /// it**, and zero keeps all.
        #[test]
        fn the_summon_type_filter_narrows_the_list() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f);
            let p = procedure(&mut f, c);
            if let Some(e) = f.effects.get_mut(p) {
                e.value = i64::from(summon_type::FUSION);
            }
            assert_eq!(
                f.filter_spsummon_procedure(c, 0, 0),
                vec![p],
                "zero keeps all"
            );
            assert_eq!(
                f.filter_spsummon_procedure(c, 0, summon_type::FUSION),
                vec![p]
            );
            assert!(f
                .filter_spsummon_procedure(c, 0, summon_type::XYZ)
                .is_empty());
        }

        /// **A procedure naming a zone has that zone checked for room** —
        /// unless its third value waives the check. `0xff` is not a naming.
        #[test]
        fn a_named_zone_is_checked_for_room() {
            fn with_values(f: &mut Field, card: CardId, values: &'static [i64]) -> EffectId {
                let p = procedure(f, card);
                if let Some(e) = f.effects.get_mut(p) {
                    e.flag[0] |= flag::FUNC_VALUE;
                    e.value_list_fn = Some(|_, _| Vec::new());
                }
                // A list value is easier to supply through the plain value
                // slot when only the first element matters; here the whole
                // list does, so it is stored on the effect directly.
                let _ = values;
                p
            }
            let mut f = Field::new(8000);
            let c = in_hand(&mut f);
            let p = with_values(&mut f, c, &[]);
            // Name the one occupied zone: no room there.
            card_in(&mut f, 0, location::MZONE, 0);
            if let Some(e) = f.effects.get_mut(p) {
                e.value_list_fn = Some(|_, _| vec![0, 0x1]);
            }
            assert!(
                f.filter_spsummon_procedure(c, 0, 0).is_empty(),
                "the named zone is taken"
            );

            // The same, with the check waived.
            if let Some(e) = f.effects.get_mut(p) {
                e.value_list_fn = Some(|_, _| vec![0, 0x1, 1]);
            }
            assert_eq!(
                f.filter_spsummon_procedure(c, 0, 0),
                vec![p],
                "the third value waives it"
            );

            // And `0xff` names nothing, so nothing is checked.
            if let Some(e) = f.effects.get_mut(p) {
                e.value_list_fn = Some(|_, _| vec![0, 0xff]);
            }
            assert_eq!(f.filter_spsummon_procedure(c, 0, 0), vec![p]);
        }

        /// **`0xff` names no zone, so no room is checked.** A procedure
        /// that names nothing is offered on a full field; one that names a
        /// zone is not.
        #[test]
        fn naming_no_zone_checks_no_room() {
            let mut f = Field::new(8000);
            for seat in 0..5 {
                card_in(&mut f, 0, location::MZONE, seat);
            }
            let c = in_hand(&mut f);
            let p = procedure(&mut f, c);
            assert_eq!(
                f.filter_spsummon_procedure(c, 0, 0),
                vec![p],
                "the default zone is not a naming"
            );
            if let Some(e) = f.effects.get_mut(p) {
                e.flag[0] |= flag::FUNC_VALUE;
                e.value_list_fn = Some(|_, _| vec![0, 0x1f]);
            }
            assert!(
                f.filter_spsummon_procedure(c, 0, 0).is_empty(),
                "a named zone is checked, and there is no room"
            );
        }

        /// **The uniqueness check is skipped for a face-down summon**, since
        /// the limit is on face-up copies.
        #[test]
        fn a_face_down_procedure_skips_the_uniqueness_check() {
            fn board(facedown: bool) -> (Field, CardId, EffectId) {
                let mut f = Field::new(8000);
                // One of the name already face-up, registered as the limiter.
                let onfield = card_in(&mut f, 0, location::MZONE, 0);
                f.cards[onfield].unique_code = 72989439;
                f.cards[onfield].unique_location = u16::from(location::MZONE);
                f.cards[onfield].unique_pos = [1, 0];
                f.cards[onfield].unique_fieldid = 1;
                let mut ue = Effect::new(effect_type::SINGLE, 0);
                ue.owner = Some(onfield);
                ue.handler = Some(onfield);
                let ue = f.new_effect(ue);
                f.cards[onfield].unique_effect = Some(ue);
                f.core.unique_cards[0].push(onfield);

                let c = in_hand(&mut f);
                let p = procedure(&mut f, c);
                if facedown {
                    if let Some(e) = f.effects.get_mut(p) {
                        e.flag[0] |= flag::SPSUM_PARAM;
                        e.s_range = u16::from(position::FACEDOWN_DEFENSE);
                    }
                }
                (f, c, p)
            }
            let (mut f, c, _) = board(false);
            assert!(
                f.filter_spsummon_procedure(c, 0, 0).is_empty(),
                "face-up: the copy on the field blocks it"
            );
            let (mut g, c, p) = board(true);
            assert_eq!(
                g.filter_spsummon_procedure(c, 0, 0),
                vec![p],
                "face-down: the limit does not apply"
            );
        }

        /// The permission layer is consulted, and refuses.
        #[test]
        fn the_permission_layer_can_refuse() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f);
            procedure(&mut f, c);
            let mut e = Effect::new(effect_type::SINGLE, code::CANNOT_SPECIAL_SUMMON);
            e.owner = Some(c);
            e.handler = Some(c);
            let id = f.new_effect(e);
            f.cards[c]
                .single_effect
                .insert(code::CANNOT_SPECIAL_SUMMON, id);
            f.cards[c].indexer.insert(id);
            assert!(f.filter_spsummon_procedure(c, 0, 0).is_empty());
        }

        /// **The forced material count is passed only when it is set.** Zero
        /// means "not forced", not "zero materials".
        #[test]
        fn the_forced_count_is_passed_only_when_set() {
            fn arity(_: &mut Field, ctx: &crate::effect::Ctx) -> bool {
                // Hold only when the two extra arguments are present.
                ctx.args.len() == 4
            }
            let mut f = Field::new(8000);
            let c = in_hand(&mut f);
            let p = procedure(&mut f, c);
            if let Some(e) = f.effects.get_mut(p) {
                e.condition = Some(arity);
            }
            assert!(!f.is_spsummonable(c, p), "unforced: two arguments");
            f.core.forced_summon_minc = 1;
            f.core.forced_summon_maxc = 2;
            assert!(f.is_spsummonable(c, p), "forced: four");
        }
    }

    mod the_machine {
        use super::*;

        #[test]
        fn a_card_with_no_procedure_is_not_summoned() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f);
            f.special_summon_rule(0, c, 0);
            assert_eq!(run(&mut f, 0), Status::End);
            assert_eq!(f.cards[c].current.location, location::HAND);
        }

        /// One procedure is taken without asking, and the card is placed by
        /// this machine rather than through `SpSummonStep`.
        #[test]
        fn a_rule_summon_places_the_card_itself() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f);
            procedure(&mut f, c);
            f.special_summon_rule(0, c, 0);
            assert_eq!(run(&mut f, 2), Status::End);
            assert_eq!(f.cards[c].current.location, location::MZONE);
            assert_eq!(f.cards[c].current.sequence, 2);
            assert_eq!(f.cards[c].current.position, position::FACEUP_ATTACK);
            assert_eq!(f.cards[c].summon.type_, summon_type::SPECIAL);
            assert_eq!(
                f.cards[c].summon.location,
                location::HAND,
                "and it records where it came from"
            );
            assert!(f.cards[c].is_status(status::SPSUMMON_TURN));
            assert!(
                !f.cards[c].is_status(status::SPSUMMON_STEP),
                "the batch machine's marker is never set"
            );
        }

        /// An empty list is refused **before** a question is queued.
        ///
        /// Without the guard the machine reaches `SelectOption` with nothing
        /// to offer, which answers `-1` and emits a bare hint rather than a
        /// question — so the message log is not where this shows. The queue
        /// is.
        #[test]
        fn an_empty_list_queues_no_question() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f);
            f.special_summon_rule(0, c, 0);
            let mut asked = false;
            drive(&mut f, 0, |f| {
                asked |= f
                    .queue()
                    .chain(f.core.subunits.iter())
                    .any(|u| matches!(u.kind, Kind::SelectOption { .. }));
            });
            assert!(!asked, "no procedure, so nothing to choose between");
        }

        /// The procedure is charged, whether or not it has an operation.
        #[test]
        fn the_procedure_is_charged() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f);
            let p = procedure(&mut f, c);
            if let Some(e) = f.effects.get_mut(p) {
                e.flag[0] |= flag::COUNT_LIMIT;
                e.count_limit = 1;
            }
            f.special_summon_rule(0, c, 0);
            run(&mut f, 0);
            assert_eq!(f.effects.get(p).map(|e| e.count_limit), Some(0));
        }

        /// The materials are cleared before the procedure runs, so a
        /// procedure that takes none leaves none behind.
        #[test]
        fn the_materials_are_cleared_first() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f);
            let stale = card_in(&mut f, 0, location::GRAVE, 0);
            f.cards[c].material_cards = std::collections::BTreeSet::from([stale]);
            procedure(&mut f, c);
            f.special_summon_rule(0, c, 0);
            run(&mut f, 1);
            assert!(
                f.cards[c].material_cards.is_empty(),
                "last summon's materials do not carry over"
            );
        }

        /// The summon type keeps only its low bits, and always gains
        /// `SUMMON_TYPE_SPECIAL`.
        #[test]
        fn the_summon_type_is_masked() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f);
            let p = procedure(&mut f, c);
            if let Some(e) = f.effects.get_mut(p) {
                e.flag[0] |= flag::FUNC_VALUE;
                e.value_list_fn = Some(|_, _| vec![0xffff_ffffu32 as i64]);
            }
            f.special_summon_rule(0, c, 0);
            run(&mut f, 0);
            assert_eq!(
                f.cards[c].summon.type_,
                0xf00_ffff | summon_type::SPECIAL,
                "masked, then marked"
            );
        }

        /// `EFFECT_FORCE_SPSUMMON_POSITION` **narrows** the mask, and
        /// narrowing it to one position removes the question.
        #[test]
        fn forcing_a_position_narrows_the_choice_away() {
            let mut f = Field::new(8000);
            let anchor = card_in(&mut f, 0, location::MZONE, 0);
            let mut e = Effect::new(effect_type::FIELD, code::FORCE_SPSUMMON_POSITION);
            e.owner = Some(anchor);
            e.handler = Some(anchor);
            e.value = i64::from(position::FACEUP_DEFENSE);
            e.flag[0] = flag::PLAYER_TARGET | flag::ABSOLUTE_TARGET;
            e.range = u16::from(location::MZONE);
            e.s_range = 1;
            let id = f.new_effect(e);
            f.add_effect(id, 0);

            let c = in_hand(&mut f);
            procedure(&mut f, c);
            f.special_summon_rule(0, c, 0);
            run(&mut f, 1);
            assert_eq!(f.cards[c].current.position, position::FACEUP_DEFENSE);
            assert!(
                !f.messages
                    .iter()
                    .any(|m| matches!(m, Message::SelectPosition { .. })),
                "one position left is no choice"
            );
        }

        /// **The seat is confirmed even when the procedure names one zone.**
        /// `move_to_field`'s `confirm` is the reference's default here too.
        #[test]
        fn the_seat_is_confirmed_even_for_a_named_zone() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f);
            let p = procedure(&mut f, c);
            if let Some(e) = f.effects.get_mut(p) {
                e.flag[0] |= flag::FUNC_VALUE;
                e.value_list_fn = Some(|_, _| vec![0, 1 << 3]);
            }
            f.special_summon_rule(0, c, 0);
            run(&mut f, 3);
            assert_eq!(f.cards[c].current.sequence, 3);
            assert!(
                f.messages
                    .iter()
                    .any(|m| matches!(m, Message::SelectPlace { .. })),
                "the one named seat is still confirmed"
            );
        }

        /// The summon is announced, and the pending shuffles are taken.
        #[test]
        fn the_shuffles_are_taken_and_the_summon_announced() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f);
            // A second card in the hand, so the pile is not empty.
            card_in(&mut f, 0, location::HAND, 1);
            procedure(&mut f, c);
            f.core.shuffle_hand_check[0] = true;
            f.special_summon_rule(0, c, 0);
            run(&mut f, 0);
            assert!(
                !f.core.shuffle_hand_check[0],
                "the pending shuffle was taken"
            );
            assert!(f
                .messages
                .iter()
                .any(|m| matches!(m, Message::SpSummoning { .. })));
        }

        /// **The material reason comes from the procedure's value**, not
        /// from the card.
        #[test]
        fn the_material_reason_comes_from_the_procedure() {
            fn material_reason_of(value: u32) -> u32 {
                let mut f = Field::new(8000);
                let c = in_hand(&mut f);
                let material = card_in(&mut f, 0, location::GRAVE, 0);
                let p = procedure(&mut f, c);
                if let Some(e) = f.effects.get_mut(p) {
                    e.value = i64::from(value);
                    e.operation = Some(|_, _| crate::effect::Yield::Done(0));
                }
                f.cards[c].material_cards = std::collections::BTreeSet::from([material]);
                // The procedure's operation would normally take them; here
                // they are put back so the events have something to reach.
                f.special_summon_rule(0, c, 0);
                let mut seen = 0;
                drive(&mut f, 1, |f| {
                    for e in f.core.instant_event.iter().chain(f.core.queue_event.iter()) {
                        if e.event_code == code::BE_PRE_MATERIAL {
                            seen = e.reason;
                        }
                    }
                });
                seen
            }
            assert_eq!(material_reason_of(summon_type::SYNCHRO), reason::SYNCHRO);
            assert_eq!(material_reason_of(summon_type::XYZ), reason::XYZ);
            assert_eq!(material_reason_of(0), reason::SPSUMMON);
        }

        /// The card's effects are on once the summon has stuck.
        #[test]
        fn the_cards_effects_are_enabled_at_the_end() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f);
            procedure(&mut f, c);
            f.special_summon_rule(0, c, 0);
            let mut seen_off = false;
            drive(&mut f, 0, |f| {
                seen_off |= !f.cards[c].is_status(status::EFFECT_ENABLED);
            });
            assert!(seen_off, "off for the move");
            assert!(f.cards[c].is_status(status::EFFECT_ENABLED), "and on again");
        }

        /// **The once-per-turn ledger is bumped exactly once under either
        /// duel option**, at step 4 or at step 17 — never both.
        #[test]
        fn the_ledger_is_bumped_once_under_either_option() {
            for flags in [
                crate::duel::REFERENCE_CONFIGURATION,
                crate::duel::REFERENCE_CONFIGURATION | crate::duel::flags::SPSUMMON_ONCE_OLD_NEGATE,
            ] {
                let mut f = Field::with_flags(8000, flags);
                let c = in_hand(&mut f);
                f.cards[c].spsummon_code = 777;
                procedure(&mut f, c);
                f.special_summon_rule(0, c, 0);
                run(&mut f, 0);
                assert_eq!(f.cards[c].current.location, location::MZONE);
                assert_eq!(f.core.spsummon_once_map[0].get(&777), Some(&1));
            }
        }

        /// **`STATUS_PROC_COMPLETE` is what a rule summon leaves behind.**
        /// It is the mark that later lets the card past
        /// `EFFECT_REVIVE_LIMIT`.
        #[test]
        fn a_rule_summon_marks_the_procedure_complete() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f);
            procedure(&mut f, c);
            assert!(!f.cards[c].is_status(status::PROC_COMPLETE));
            f.special_summon_rule(0, c, 0);
            run(&mut f, 0);
            assert!(f.cards[c].is_status(status::PROC_COMPLETE));
        }

        /// A `target` that refuses ends the summon.
        #[test]
        fn a_refusing_target_ends_it() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f);
            let p = procedure(&mut f, c);
            if let Some(e) = f.effects.get_mut(p) {
                e.target = Some(|_, _, _, _| crate::effect::Yield::Done(0));
            }
            f.special_summon_rule(0, c, 0);
            run(&mut f, 0);
            assert_eq!(f.cards[c].current.location, location::HAND);
        }

        /// The counters and the success event.
        #[test]
        fn the_counters_and_the_success_event() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f);
            f.cards[c].spsummon_code = 777;
            procedure(&mut f, c);
            f.special_summon_rule(0, c, 0);
            let mut charged = false;
            drive(&mut f, 0, |f| {
                charged |= f.core.hint_timing[0] & timing::SPSUMMON != 0;
            });
            assert_eq!(f.core.spsummon_state_count[0], 1);
            assert_eq!(f.core.spsummon_once_map[0].get(&777), Some(&1));
            assert!(charged);
            assert!(f
                .core
                .instant_event
                .iter()
                .chain(f.core.used_event.iter())
                .any(|e| e.event_code == code::SPSUMMON_SUCCESS));
            assert!(f.messages.iter().any(|m| matches!(m, Message::SpSummoned)));
        }

        /// **`EVENT_SPSUMMON` is raised outside a chain and not inside one**,
        /// which is `SummonRule`'s rule rather than `FlipSummon`'s.
        #[test]
        fn the_window_is_opened_only_outside_a_chain() {
            fn raised(in_chain: bool) -> bool {
                let mut f = Field::new(8000);
                let c = in_hand(&mut f);
                procedure(&mut f, c);
                if in_chain {
                    let anchor = card_in(&mut f, 0, location::MZONE, 0);
                    let mut e = Effect::new(effect_type::SINGLE, code::UPDATE_ATTACK);
                    e.owner = Some(anchor);
                    e.handler = Some(anchor);
                    let e = f.new_effect(e);
                    let mut chain = crate::chain::Chain::new(e, crate::event::Event::new(0));
                    chain.triggering_player = 0;
                    f.core.current_chain.push(chain);
                }
                f.special_summon_rule(0, c, 0);
                run(&mut f, 1);
                assert_eq!(f.cards[c].current.location, location::MZONE, "it happened");
                f.core
                    .instant_event
                    .iter()
                    .chain(f.core.point_event.iter())
                    .chain(f.core.used_event.iter())
                    .any(|e| e.event_code == code::SPSUMMON)
            }
            assert!(raised(false), "outside a chain the window opens");
            assert!(!raised(true), "inside one it does not");
        }

        /// `EFFECT_CANNOT_DISABLE_SPSUMMON` closes it too.
        #[test]
        fn an_undisableable_summon_opens_no_window() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f);
            procedure(&mut f, c);
            let mut e = Effect::new(effect_type::SINGLE, code::CANNOT_DISABLE_SPSUMMON);
            e.owner = Some(c);
            e.handler = Some(c);
            let id = f.new_effect(e);
            f.cards[c]
                .single_effect
                .insert(code::CANNOT_DISABLE_SPSUMMON, id);
            f.cards[c].indexer.insert(id);

            f.special_summon_rule(0, c, 0);
            run(&mut f, 0);
            assert_eq!(f.cards[c].current.location, location::MZONE);
            assert!(!f
                .core
                .instant_event
                .iter()
                .chain(f.core.used_event.iter())
                .any(|e| e.event_code == code::SPSUMMON));
        }

        /// A negated rule summon sends the card to the graveyard.
        #[test]
        fn a_negated_rule_summon_sends_the_card_to_the_graveyard() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f);
            procedure(&mut f, c);
            f.special_summon_rule(0, c, 0);
            for _ in 0..4096 {
                let at_verdict =
                    f.queue().next().is_some_and(|u| {
                        matches!(u.kind, Kind::SpSummonRule { .. }) && u.step == 11
                    }) && f.core.subunits.is_empty();
                if at_verdict {
                    f.cards[c].set_status(status::SUMMONING, false);
                    f.cards[c].set_status(status::SUMMON_DISABLED, true);
                }
                match f.process() {
                    Status::Continue => continue,
                    Status::Awaiting => match f.messages.last() {
                        Some(Message::SelectPlace { .. }) => {
                            f.core.returns.set_i8(0, 0);
                            f.core.returns.set_i8(1, location::MZONE as i8);
                            f.core.returns.set_i8(2, 0);
                        }
                        Some(Message::SelectPosition { .. }) => {
                            f.core.returns.set(i32::from(position::FACEUP_ATTACK))
                        }
                        Some(Message::SelectChain { .. }) => f.core.returns.set(-1),
                        _ => break,
                    },
                    _ => break,
                }
            }
            assert_eq!(f.cards[c].current.location, location::GRAVE);
            assert!(!f.cards[c].is_status(status::PROC_COMPLETE));
            assert_eq!(f.core.spsummon_state_count[0], 0);
        }

        /// **The gather puts back what it borrows.**
        ///
        /// `is_spsummonable` makes the procedure the reason effect for the
        /// duration of the question and restores it afterwards; so does the
        /// group filter. Step 0 additionally saves the four material
        /// parameters around the whole gather.
        ///
        /// That last save cannot be made to matter in this port and is kept
        /// anyway — see the findings doc: the reference needs it because a
        /// Lua condition can write to the field, and here a condition is
        /// `fn(&Field, &Ctx)` and cannot. What *is* observable is the reason
        /// effect, which the same functions borrow in the same way.
        #[test]
        fn the_gather_puts_back_what_it_borrows() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f);
            let p = procedure(&mut f, c);
            let outer = {
                let mut e = Effect::new(effect_type::SINGLE, 0);
                e.owner = Some(c);
                e.handler = Some(c);
                f.new_effect(e)
            };
            f.core.reason_effect = Some(outer);
            f.core.reason_player = 1;
            let group = f.new_group([c]);
            f.core.only_use_mats = Some(group);
            f.core.forced_summon_minc = 3;

            assert!(f.is_spsummonable(c, p));
            assert_eq!(f.core.reason_effect, Some(outer), "put back");
            assert_eq!(f.core.reason_player, 1);

            f.filter_spsummon_procedure(c, 0, 0);
            assert_eq!(f.core.reason_effect, Some(outer));
            assert_eq!(f.core.only_use_mats, Some(group));
            assert_eq!(f.core.forced_summon_minc, 3);
        }

        /// **The forced count is cleared before the procedure's operation
        /// runs**, so a nested summon does not inherit it.
        #[test]
        fn the_forced_count_is_cleared_for_the_operation() {
            fn note(f: &mut Field, _: &crate::effect::Ctx) -> crate::effect::Yield {
                // Recorded through a field the test can read afterwards.
                f.core.forced_summon_maxc = f.core.forced_summon_minc + 100;
                crate::effect::Yield::Done(0)
            }
            let mut f = Field::new(8000);
            let c = in_hand(&mut f);
            let p = procedure(&mut f, c);
            if let Some(e) = f.effects.get_mut(p) {
                e.operation = Some(note);
            }
            f.core.forced_summon_minc = 3;
            f.core.forced_summon_maxc = 4;
            f.special_summon_rule(0, c, 0);
            run(&mut f, 0);
            assert_eq!(
                f.core.forced_summon_maxc, 100,
                "the operation saw a cleared count"
            );
        }

        /// A group procedure reaches the path that is not ported, and says
        /// so rather than doing something else.
        #[test]
        #[should_panic(expected = "SpSummonRuleGroup")]
        fn a_group_procedure_reaches_the_unported_path() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f);
            let mut e = Effect::new(effect_type::FIELD, code::SPSUMMON_PROC_G);
            e.owner = Some(c);
            e.handler = Some(c);
            e.range = u16::from(location::HAND);
            e.condition = Some(|_, _| true);
            let id = f.new_effect(e);
            f.cards[c].field_effect.insert(code::SPSUMMON_PROC_G, id);
            f.cards[c].indexer.insert(id);
            f.special_summon_rule(0, c, 0);
            run(&mut f, 0);
        }
    }
}

//! Normal summoning: `summon` and `SummonRule`.
//!
//! Nineteen steps — the most intricate machine in the port, and the first
//! that can be **interrupted**. A normal summon is negatable, so the machine
//! opens a window, hands control back, and then asks whether it is still
//! happening.
//!
//! ## Three paths through the front half
//!
//! ```text
//!  0 → 1 ─┬─ (Gemini) ──────── 5 ─────────────→ 10 → 11 → …
//!          ├─ (a procedure) → 2 → 4 → 5 → 7 → 8 → 9 → 11 → …
//!          └─ (ordinary)    → 2 → 3 → 5 → 6 → 8 → 9 → 11 → …
//! ```
//!
//! The Gemini path is the surprise: a card **already face-up on the field**
//! can be normal summoned again, which is how a Gemini monster gains its
//! effect. Step 0 recognises it by `current.location == LOCATION_MZONE`;
//! step 1 takes its extra-summon permission and step 5 sends it to 10, with
//! none of the tribute machinery run and no move — the card is already
//! where it is going, so it gains `EFFECT_GEMINI_STATUS` instead.
//!
//! ## `summon_depth` decides who pays
//!
//! A summon performed *inside* another summon's procedure must not spend a
//! second normal summon, nor open its own negation window. The depth counter
//! is incremented at step 0 and decremented at steps 5, 8 and 10 — and
//! **only the outermost** reaching zero goes on to pay.
//!
//! It also disables cancelling: `summon_depth != 0` clears
//! `summon_cancelable`, because backing out of an inner summon would leave
//! the outer one half-done.
//!
//! ## The negation window
//!
//! Steps 13-15 are the interruptible part. The card is marked
//! `STATUS_SUMMONING`, `EVENT_SUMMON` is raised, and a `PointEvent` lets the
//! opponent respond — and because that `PointEvent` is emplaced, it runs
//! before step 15 does. Step 15 then reads the status as the verdict: still
//! set means nothing negated the summon.
//!
//! A summon that *was* negated falls out at step 15: the oaths are released,
//! and the card is sent to the graveyard from the Monster Zone it never
//! legitimately occupied.

use crate::board::{location, position};
use crate::card::{card_type, reason, status, summon_type};
use crate::event::{code, CardId, EffectId};
use crate::field::{reset, timing, Field, Message};
use crate::processor::Kind;
use crate::tribute::Procedures;

/// The state `SummonRule` carries.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SummonRuleState {
    pub procedure: Option<EffectId>,
    pub extra_summon: Option<EffectId>,
    /// A **set**, as the reference's `card_set` is — the release order is
    /// creation order, not the order the player picked them in.
    pub tributes: std::collections::BTreeSet<CardId>,
    pub max_allowed_tributes: u8,
    pub decided: bool,
    /// The `EFFECT_SUMMON_COST` effects that were paid, kept so their oaths
    /// can be released whether the summon succeeds or is negated.
    pub cost_effects: Vec<EffectId>,
}

impl Field {
    /// `field::summon` — queue a normal summon.
    pub fn summon(
        &mut self,
        sumplayer: u8,
        target: CardId,
        procedure: Option<EffectId>,
        ignore_count: bool,
        min_tribute: u8,
        zone: u32,
    ) {
        self.emplace(Kind::SummonRule {
            sumplayer,
            target,
            ignore_count,
            min_tribute,
            zone,
            state: Box::new(SummonRuleState {
                procedure,
                decided: procedure.is_some(),
                ..Default::default()
            }),
        });
    }

    /// `card::is_summonable_card` — is this the kind of monster that can be
    /// normal summoned at all?
    ///
    /// A list of exclusions rather than an inclusion: everything that must
    /// be Special Summoned, plus tokens and trap monsters. The
    /// `EFFECT_UNSUMMONABLE_CARD` check is last and is the only part that
    /// depends on the board.
    pub fn is_summonable_card(&mut self, card: CardId) -> bool {
        let t = self.cards[card].data.type_;
        if t & card_type::MONSTER == 0
            || t & (card_type::RITUAL
                | card_type::SPSUMMON
                | card_type::FUSION
                | card_type::SYNCHRO
                | card_type::XYZ
                | card_type::LINK
                | card_type::TOKEN
                | card_type::TRAPMONSTER)
                != 0
        {
            return false;
        }
        self.is_affected_by_effect(card, code::UNSUMMONABLE_CARD)
            .is_none()
    }
}

impl Field {
    /// One step of `SummonRule`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn summon_rule_step(
        &mut self,
        step: u16,
        sumplayer: u8,
        target: CardId,
        ignore_count: bool,
        min_tribute: &mut u8,
        zone: &mut u32,
        state: &mut SummonRuleState,
    ) -> bool {
        match step {
            0 => self.summon_step_0(sumplayer, target, ignore_count, *min_tribute, *zone, state),
            1 => self.summon_step_1(sumplayer, target, ignore_count, *min_tribute, *zone, state),
            2 => self.summon_step_2(sumplayer, target, min_tribute, zone, state),
            3 => {
                if self.core.returns.get() != 0 {
                    self.core.return_cards.clear();
                } else {
                    let max = state.max_allowed_tributes;
                    let cancelable = self.core.summon_cancelable;
                    self.select_tribute_cards(
                        target, sumplayer, cancelable, 1, max, sumplayer, *zone,
                    );
                }
                self.set_step(4);
                false
            }
            4 => self.summon_step_4(sumplayer, target, *min_tribute, *zone, state),
            5 => self.summon_step_5(sumplayer, target, state),
            6 => self.summon_step_6(sumplayer, target, state),
            7 => self.summon_step_7(sumplayer, target, *min_tribute, *zone, state),
            8 => self.summon_step_8(sumplayer, target, ignore_count, state),
            9 => self.summon_step_9(sumplayer, target, *zone, state),
            10 => self.summon_step_10(sumplayer, target, state),
            11 => self.summon_pay(sumplayer, target, ignore_count, state),
            12 => self.summon_step_12(sumplayer, target, state),
            13 => self.summon_step_13(target),
            14 => self.summon_step_14(sumplayer, target, state),
            15 => self.summon_step_15(sumplayer, target, state),
            16 => self.summon_step_16(target, state),
            17 => self.summon_step_17(sumplayer, target, state),
            18 => self.summon_step_18(sumplayer, target, state),
            _ => true,
        }
    }

    /// Step 0: may this be summoned, and how?
    ///
    /// Three refusals apply to every summon, then the path forks on where
    /// the card **is**.
    ///
    /// **In the Monster Zone** it is a Gemini summon — a face-up monster
    /// being normal summoned again to gain its effect. The conditions are
    /// their own: face-up, a summon still available, `GEMINI_SUMMONABLE`
    /// present, `GEMINI_STATUS` *absent* (it has not already gained it), and
    /// the player able to perform a Gemini summon.
    ///
    /// **Anywhere else** it is an ordinary summon, and the procedure choice
    /// is offered exactly as `MonsterSet` does — including the **empty-list
    /// guard**, which `MonsterSet` lacks. See the note in the findings doc.
    fn summon_step_0(
        &mut self,
        sumplayer: u8,
        target: CardId,
        ignore_count: bool,
        min_tribute: u8,
        zone: u32,
        state: &mut SummonRuleState,
    ) -> bool {
        if !self.is_summonable_card(target) {
            return true;
        }
        if self
            .check_unique_onfield(target, sumplayer, u16::from(location::MZONE), None)
            .is_some()
        {
            return true;
        }
        if self
            .is_affected_by_effect(target, code::CANNOT_SUMMON)
            .is_some()
        {
            return true;
        }

        if self.cards[target].current.location == location::MZONE {
            // The Gemini path.
            if self.cards[target].current.is_position(position::FACEDOWN) {
                return true;
            }
            let no_extra = self.core.extra_summon[sumplayer as usize]
                || self
                    .is_affected_by_effect(target, code::EXTRA_SUMMON_COUNT)
                    .is_none();
            let limit = self.get_summon_count_limit(sumplayer);
            if !ignore_count
                && no_extra
                && self.core.summon_count[sumplayer as usize] as i32 >= limit
            {
                return true;
            }
            if self
                .is_affected_by_effect(target, code::GEMINI_SUMMONABLE)
                .is_none()
            {
                return true;
            }
            // Already gained its effect: it cannot do so again.
            if self
                .is_affected_by_effect(target, code::GEMINI_STATUS)
                .is_some()
            {
                return true;
            }
            if !self.is_player_can_summon(summon_type::GEMINI, sumplayer, Some(target), sumplayer) {
                return true;
            }
        } else {
            let found =
                self.filter_summon_procedure(target, sumplayer, ignore_count, min_tribute, zone);
            if let Some(named) = state.procedure {
                let ok = match &found {
                    Procedures::Forbidden => false,
                    _ => self.check_summon_procedure(
                        target,
                        named,
                        sumplayer,
                        ignore_count,
                        min_tribute,
                        zone,
                    ),
                };
                if !ok {
                    return true;
                }
            } else {
                let (procedures, ordinary) = match found {
                    Procedures::Forbidden => return true,
                    Procedures::Limited(ps) => (ps, false),
                    Procedures::Ordinary {
                        procedures,
                        ordinary,
                    } => (procedures, ordinary),
                };
                self.core.select_effects.clear();
                self.core.select_options.clear();
                if ordinary {
                    self.core.select_effects.push(None);
                    self.core.select_options.push(1);
                }
                for e in procedures {
                    let d = self.effects.get(e).map_or(0, |x| x.description);
                    self.core.select_effects.push(Some(e));
                    self.core.select_options.push(d);
                }
                // The guard `MonsterSet` does not have.
                if self.core.select_options.is_empty() {
                    return true;
                }
                if self.core.select_options.len() == 1 {
                    self.core.returns.set(0);
                } else {
                    self.emplace(Kind::SelectOption { player: sumplayer });
                }
            }
        }

        // An inner summon cannot be cancelled: backing out would leave the
        // outer one half-done.
        if self.core.summon_depth != 0 {
            self.core.summon_cancelable = false;
        }
        self.core.summon_depth += 1;
        self.cards[target].material_cards.clear();
        false
    }

    /// Step 1: which extra-summon permission, if any.
    ///
    /// Identical in shape to `MonsterSet`'s, reading `EXTRA_SUMMON_COUNT`
    /// rather than `EXTRA_SET_COUNT` — except for the Gemini path, which
    /// takes the first permission unchecked and leaves for step 5.
    fn summon_step_1(
        &mut self,
        sumplayer: u8,
        target: CardId,
        ignore_count: bool,
        min_tribute: u8,
        zone: u32,
        state: &mut SummonRuleState,
    ) -> bool {
        let candidates = self.filter_effect(target, code::EXTRA_SUMMON_COUNT);
        // The Gemini path is recognised by re-reading the location, not by a
        // flag set at step 0 — as the reference does, at both of the two
        // places it forks.
        if self.cards[target].current.location == location::MZONE {
            // The Gemini path takes the **first** permission there is,
            // without asking and without checking that it would allow
            // anything — there are no tributes for it to license. Where the
            // ordinary path offers a choice, this one has nothing to choose
            // between.
            state.extra_summon = None;
            if !ignore_count && !self.core.extra_summon[sumplayer as usize] {
                state.extra_summon = candidates.first().copied();
            }
            self.set_step(4);
            return false;
        }
        if !state.decided {
            let idx = self.core.returns.get().max(0) as usize;
            state.procedure = self.core.select_effects.get(idx).copied().flatten();
            state.decided = true;
        }
        self.core.select_effects.clear();
        self.core.select_options.clear();
        let limit = self.get_summon_count_limit(sumplayer);
        if ignore_count || (self.core.summon_count[sumplayer as usize] as i32) < limit {
            self.core.select_effects.push(None);
            self.core.select_options.push(1);
        }
        if !ignore_count && !self.core.extra_summon[sumplayer as usize] {
            for e in candidates {
                if self.extra_summon_count_allows(
                    target,
                    e,
                    state.procedure,
                    sumplayer,
                    min_tribute,
                    zone,
                ) {
                    let d = self.effects.get(e).map_or(0, |x| x.description);
                    self.core.select_effects.push(Some(e));
                    self.core.select_options.push(d);
                }
            }
        }
        if self.core.select_options.len() == 1 {
            self.core.returns.set(0);
        } else {
            self.emplace(Kind::SelectOption { player: sumplayer });
        }
        false
    }

    /// The summon counterpart of `extra_set_count_allows`.
    fn extra_summon_count_allows(
        &mut self,
        target: CardId,
        permission: EffectId,
        procedure: Option<EffectId>,
        sumplayer: u8,
        min_tribute: u8,
        zone: u32,
    ) -> bool {
        let (new_min, mut new_zone, releasable) =
            self.extra_count_params_pub(target, permission, 0x1f001f);
        if let Some(p) = procedure {
            new_zone = self.flip_zone_for_opponent_pub(p, new_zone);
        }
        new_zone &= zone;
        match procedure {
            Some(p) => {
                let min = new_min.max(min_tribute);
                self.is_summonable(target, p, min, new_zone, releasable)
            }
            None => {
                let (mut min, mut max) = self.get_summon_tribute_count(target);
                if !self.is_player_can_summon(
                    summon_type::ADVANCE,
                    sumplayer,
                    Some(target),
                    sumplayer,
                ) {
                    max = 0;
                }
                min = min.max(i32::from(min_tribute));
                if max < min {
                    return false;
                }
                min = min.max(i32::from(new_min));
                let controller = self.cards[target].current.controller;
                self.check_tribute(
                    target,
                    min,
                    max,
                    None,
                    controller,
                    new_zone,
                    releasable,
                    u32::from(position::FACEUP_ATTACK),
                )
            }
        }
    }
}

impl Field {
    /// Step 2: apply the permission, then find the tributes.
    ///
    /// **The jumps are not `MonsterSet`'s**, and the difference is easy to
    /// copy wrongly. Both send a named procedure to step 3 — the one that
    /// reads a yes/no that was never asked and then runs the procedure's
    /// `target`. But where `MonsterSet` also sends its *ordinary* path there,
    /// this one sends it to step 4, past the procedure step it has no
    /// procedure for. `MonsterSet` gets away with passing through because
    /// that step returns immediately when there is no procedure; here the
    /// reference skips it outright.
    ///
    /// The position handed to `get_summon_release_list` is the default
    /// `POS_FACEUP_ATTACK`, where `MonsterSet` passes `POS_FACEDOWN_DEFENSE`
    /// — a tribute pool opened for a face-up summon is not the same pool.
    fn summon_step_2(
        &mut self,
        sumplayer: u8,
        target: CardId,
        min_tribute: &mut u8,
        zone: &mut u32,
        state: &mut SummonRuleState,
    ) -> bool {
        let idx = self.core.returns.get().max(0) as usize;
        state.extra_summon = self.core.select_effects.get(idx).copied().flatten();

        let mut releasable = 0xff00ffu32;
        if let Some(p) = state.extra_summon {
            let (new_min, mut new_zone, r) = self.extra_count_params_pub(target, p, 0x1f001f);
            releasable = r;
            if *min_tribute < new_min {
                *min_tribute = new_min;
            }
            if let Some(proc) = state.procedure {
                new_zone = self.flip_zone_for_opponent_pub(proc, new_zone);
            }
            *zone &= new_zone;
        }
        if state.procedure.is_some() {
            self.set_step(3);
            return false;
        }

        self.core.select_cards.clear();
        let (mut min, max) = self.get_summon_tribute_count(target);
        min = min.max(i32::from(*min_tribute));
        let advance =
            self.is_player_can_summon(summon_type::ADVANCE, sumplayer, Some(target), sumplayer);
        if max == 0 || !advance {
            self.core.return_cards.clear();
            self.set_step(4);
            return false;
        }
        self.core.release_cards.clear();
        self.core.release_cards_ex.clear();
        self.core.release_cards_ex_oneof.clear();
        let (rcount, lists) = self.get_summon_release_list(
            target,
            None,
            false,
            releasable,
            u32::from(position::FACEUP_ATTACK),
        );
        if rcount == 0 {
            self.core.return_cards.clear();
            self.set_step(4);
            return false;
        }
        self.core.release_cards = lists.release.into_iter().collect();
        self.core.release_cards_ex = lists.extra.into_iter().collect();
        self.core.release_cards_ex_oneof = lists.extra_one_of.into_iter().collect();

        let room = self.get_tofield_count(
            Some(target),
            sumplayer,
            location::MZONE,
            sumplayer,
            Field::LOCATION_REASON_TOFIELD,
            *zone,
        );
        let fcount = self.get_mzone_limit(sumplayer, sumplayer, Field::LOCATION_REASON_TOFIELD);
        if min == 0 && room > 0 && fcount > 0 {
            self.emplace(Kind::SelectYesNo {
                player: sumplayer,
                description: 90,
            });
            state.max_allowed_tributes = max as u8;
        } else {
            if min < -fcount + 1 {
                min = -fcount + 1;
            }
            let cancelable = self.core.summon_cancelable;
            self.select_tribute_cards(
                target, sumplayer, cancelable, min as u8, max as u8, sumplayer, *zone,
            );
            self.set_step(4);
        }
        false
    }

    /// Step 4: the procedure's `target`.
    fn summon_step_4(
        &mut self,
        sumplayer: u8,
        target: CardId,
        min_tribute: u8,
        zone: u32,
        state: &mut SummonRuleState,
    ) -> bool {
        self.core.returns.set(1);
        let Some(procedure) = state.procedure else {
            return false;
        };
        if self
            .effects
            .get(procedure)
            .is_some_and(|e| e.target.is_some())
        {
            let args = self.procedure_args(target, min_tribute, zone, state.extra_summon);
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

    /// Step 5: take the tributes, or bail — then pay the summon's cost.
    ///
    /// Three outcomes, and the two failures **decrement the depth** before
    /// returning. Forgetting that leaves the counter permanently raised, and
    /// every later summon in the duel would then believe it was nested and
    /// decline to pay.
    fn summon_step_5(
        &mut self,
        sumplayer: u8,
        target: CardId,
        state: &mut SummonRuleState,
    ) -> bool {
        if self.cards[target].current.location == location::MZONE {
            self.set_step(9);
            return false;
        }
        if state.procedure.is_some() {
            if self.core.returns.get() == 0 {
                self.core.summon_depth -= 1;
                return true;
            }
            self.set_step(6);
        } else {
            if self.core.return_cards.canceled {
                self.core.summon_depth -= 1;
                return true;
            }
            state.tributes = self.core.return_cards.list.iter().copied().collect();
        }
        state.cost_effects = self.filter_effect(target, code::SUMMON_COST);
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

    /// Step 6: the ordinary summon — charge the tribute reductions, release
    /// the tributes, record the summon.
    ///
    /// The charging is the part with no counterpart in `MonsterSet`, and the
    /// **order is a rule**:
    ///
    /// 1. the single largest *uncounted* reduction, which costs nothing;
    /// 2. count-limited reductions **with a target**, spent in order until
    ///    the shortfall is covered;
    /// 3. count-limited reductions **without** one, likewise.
    ///
    /// Targeted ones are spent first because they are the more specific
    /// permission — spending a general one while a specific one applies
    /// wastes it.
    ///
    /// **A reduction can be spent after the shortfall is already covered.**
    /// Neither count-limited loop tests `min > 0` on entry: each tests it
    /// only *after* charging one effect. So a card whose uncounted reduction
    /// alone covers the shortfall still spends one targeted reduction, and
    /// then one untargeted one. The `min > 0` that does guard is the outer
    /// one, tested once before any of this. Faithful, and deliberate here.
    fn summon_step_6(
        &mut self,
        sumplayer: u8,
        target: CardId,
        state: &mut SummonRuleState,
    ) -> bool {
        let level = self.get_level(target) as i32;
        let mut min = match level {
            l if l < 5 => 0,
            l if l < 7 => 1,
            _ => 2,
        };
        min -= state.tributes.len() as i32;
        if min > 0 {
            let effects = self.filter_effect(target, code::DECREASE_TRIBUTE);
            // 1. the largest uncounted reduction
            let mut best = 0i32;
            let mut best_effect = None;
            for &e in &effects {
                let counted = self
                    .effects
                    .get(e)
                    .is_some_and(|x| x.is_flag(crate::effect::flag::COUNT_LIMIT));
                if !counted {
                    let dec = self.effect_value_for_card_pub(e, target) as i32 & 0xffff;
                    if best < dec {
                        best = dec;
                        best_effect = Some(e);
                    }
                }
            }
            if let Some(e) = best_effect {
                min -= best;
                self.hint_card(e);
            }
            // 2 and 3: targeted first, then untargeted. Neither loop is
            // guarded by `min > 0` — see the doc comment.
            for want_target in [true, false] {
                for &e in &effects {
                    let usable = self.effects.get(e).is_some_and(|x| {
                        x.is_flag(crate::effect::flag::COUNT_LIMIT)
                            && x.count_limit > 0
                            && x.target.is_some() == want_target
                    });
                    if !usable {
                        continue;
                    }
                    let dec = self.effect_value_for_card_pub(e, target) as i32 & 0xffff;
                    min -= dec;
                    self.dec_count(e, crate::event::PLAYER_NONE);
                    self.hint_card(e);
                    if min <= 0 {
                        break;
                    }
                }
            }
        }

        self.cards[target].summon.location = location::HAND;
        self.cards[target].summon.type_ = summon_type::NORMAL;
        self.cards[target].summon.pzone = false;
        let tributes = std::mem::take(&mut state.tributes);
        if !tributes.is_empty() {
            for &t in &tributes {
                self.cards[t].reason_card = Some(target);
            }
            self.set_material(target, tributes.clone());
            self.release(
                tributes.iter().copied().collect::<Vec<_>>(),
                None,
                reason::SUMMON | reason::MATERIAL,
                sumplayer,
            );
            self.cards[target].summon.type_ |= summon_type::ADVANCE;
            self.adjust_all();
        }
        self.cards[target].reason_effect = None;
        self.cards[target].reason_player = sumplayer;
        self.set_step(7);
        false
    }

    /// The `MSG_HINT`/`HINT_CARD` an effect's use announces.
    fn hint_card(&mut self, effect: EffectId) {
        let code_ = self
            .effects
            .get(effect)
            .and_then(|e| e.get_handler(&self.cards))
            .map_or(0, |h| self.cards[h].data.code);
        self.messages.push(Message::Hint {
            kind: crate::host_question::hint::CARD,
            player: 0,
            value: u64::from(code_),
        });
    }

    /// Step 7: the procedure's own summon type, then its operation.
    fn summon_step_7(
        &mut self,
        sumplayer: u8,
        target: CardId,
        min_tribute: u8,
        zone: u32,
        state: &mut SummonRuleState,
    ) -> bool {
        let Some(procedure) = state.procedure else {
            return false;
        };
        let value = self.effect_value_for_card_pub(procedure, target) as u32;
        self.cards[target].summon.location = location::HAND;
        self.cards[target].summon.pzone = false;
        self.cards[target].summon.type_ = (value & 0xfff_ffff) | summon_type::NORMAL;
        self.cards[target].reason_effect = Some(procedure);
        self.cards[target].reason_player = sumplayer;
        self.core.returns.set(1);
        if self
            .effects
            .get(procedure)
            .is_some_and(|e| e.operation.is_some())
        {
            let args = self.procedure_args(target, min_tribute, zone, state.extra_summon);
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

    /// Step 8: unwind one level of nesting, and pay if this was the outer
    /// one.
    ///
    /// **An inner summon returns here and goes no further.** It has done its
    /// work; the outer summon's steps 9 onward place the card and open the
    /// window.
    fn summon_step_8(
        &mut self,
        sumplayer: u8,
        target: CardId,
        ignore_count: bool,
        state: &mut SummonRuleState,
    ) -> bool {
        self.core.summon_depth -= 1;
        if self.core.summon_depth != 0 {
            return true;
        }
        self.break_effect(true);
        self.summon_pay(sumplayer, target, ignore_count, state)
    }

    /// Spend the normal summon, or the extra permission. Shared by steps 8
    /// and 11 — the Gemini path reaches it from the other side.
    fn summon_pay(
        &mut self,
        sumplayer: u8,
        target: CardId,
        ignore_count: bool,
        state: &mut SummonRuleState,
    ) -> bool {
        if ignore_count {
            return false;
        }
        match state.extra_summon {
            None => self.core.summon_count[sumplayer as usize] += 1,
            Some(p) => {
                self.core.extra_summon[sumplayer as usize] = true;
                self.hint_card(p);
                if self.effects.get(p).is_some_and(|e| e.operation.is_some()) {
                    self.core
                        .sub_solving_event
                        .push_back(crate::event::Event::new(0));
                    self.emplace(Kind::ExecuteOperation {
                        resume: None,
                        effect: p,
                        player: sumplayer,
                        subject: Some(target),
                        args: Vec::new(),
                        was_disabled: false,
                    });
                }
            }
        }
        false
    }
}

impl Field {
    /// Step 9: choose the position and place the card.
    ///
    /// The default is face-up **attack**, widened to all face-up positions
    /// by the duel option or by either of two effects — the same widening
    /// `is_player_can_summon` does, and for the same reason.
    ///
    /// `EFFECT_FORCE_NORMAL_SUMMON_POSITION` then **narrows** it, and this is
    /// where the narrowing is actually applied: `is_player_can_summon` only
    /// asked whether anything would be left.
    ///
    /// Jumps to step 11, skipping 10 — which is the Gemini path's own.
    fn summon_step_9(
        &mut self,
        sumplayer: u8,
        target: CardId,
        zone: u32,
        state: &mut SummonRuleState,
    ) -> bool {
        let mut targetplayer = sumplayer;
        let mut positions = position::FACEUP_ATTACK;
        if self.is_flag(crate::duel::flags::NORMAL_SUMMON_FACEUP_DEF)
            || self
                .is_player_affected_by_effect(sumplayer, code::NORMAL_SUMMON_FACEUP_DEFENSE)
                .is_some()
            || self
                .is_player_affected_by_effect(sumplayer, code::DEVINE_LIGHT)
                .is_some()
        {
            positions = position::FACEUP;
        }
        if let Some(p) = state.procedure {
            let spsum = self
                .effects
                .get(p)
                .is_some_and(|e| e.is_flag(crate::effect::flag::SPSUM_PARAM));
            if spsum {
                let s_range = self.effects.get(p).map_or(0, |e| e.s_range) as u8;
                positions = s_range & position::FACEUP;
                if self.effects.get(p).is_some_and(|e| e.o_range != 0) {
                    targetplayer = 1 - sumplayer;
                }
            }
        }
        let summon_kind = self.cards[target].summon.type_;
        for e in self.filter_player_effect(sumplayer, code::FORCE_NORMAL_SUMMON_POSITION) {
            if let Some(check) = self.effects.get(e).and_then(|x| x.target_filter) {
                let args = [
                    i64::from(sumplayer),
                    i64::from(summon_kind),
                    i64::from(positions),
                    i64::from(targetplayer),
                ];
                if !check(self, e, Some(target), &args) {
                    continue;
                }
            }
            positions &= self.effect_plain_value(e) as u8;
        }
        self.enable_field_effect(target, false);
        self.move_to_field(
            target,
            sumplayer,
            targetplayer,
            u16::from(location::MZONE),
            positions,
            false,
            0,
            zone,
            false,
            0,
            // `confirm`: the reference's default, and it is `true`.
            true,
        );
        self.set_step(11);
        false
    }

    /// Step 10: the Gemini summon.
    ///
    /// The card does not move — it is already there. What changes is that it
    /// **gains `EFFECT_GEMINI_STATUS`**, which is the whole point: a Gemini
    /// monster summoned a second time becomes an effect monster.
    ///
    /// `summon.location` is set to `LOCATION_MZONE` rather than `HAND`,
    /// because that *is* where it came from — and `summon.sequence` is
    /// recorded, which no other path does.
    fn summon_step_10(
        &mut self,
        sumplayer: u8,
        target: CardId,
        _state: &mut SummonRuleState,
    ) -> bool {
        self.core.summon_depth -= 1;
        if self.core.summon_depth != 0 {
            return true;
        }
        self.enable_field_effect(target, false);
        let seq = self.cards[target].current.sequence as u8;
        let c = &mut self.cards[target];
        c.summon.location = location::MZONE;
        c.summon.pzone = false;
        c.summon.sequence = seq;
        c.summon.type_ |= summon_type::NORMAL;
        c.reason_effect = None;
        c.reason_player = sumplayer;

        let mut e =
            crate::effect::Effect::new(crate::effect::effect_type::SINGLE, code::GEMINI_STATUS);
        e.owner = Some(target);
        e.handler = Some(target);
        e.flag[0] = crate::effect::flag::CANNOT_DISABLE | crate::effect::flag::CLIENT_HINT;
        e.description = 64;
        // `RESET_EVENT + 0x1fe0000`, spelled out as the bits it is — the
        // same decomposition the draw hint uses.
        e.reset_flag = reset::EVENT
            + reset::TURN_SET
            + reset::TOGRAVE
            + reset::REMOVE
            + reset::TEMP_REMOVE
            + reset::TOHAND
            + reset::TODECK
            + reset::LEAVE
            + reset::TOFIELD;
        debug_assert_eq!(
            e.reset_flag - reset::EVENT,
            0x1fe_0000,
            "the decomposition must equal the reference's literal"
        );
        let id = self.new_effect(e);
        self.cards[target]
            .single_effect
            .insert(code::GEMINI_STATUS, id);
        self.cards[target].indexer.insert(id);
        false
    }

    /// Step 12: announce the summon and tell the materials.
    ///
    /// `EVENT_BE_PRE_MATERIAL` is raised for the tributes **before** the
    /// summon is confirmed — they are being used, and effects that respond to
    /// being used as material need the window while the summon is still
    /// pending.
    ///
    /// The activity counters are bumped here **only** under the old oath
    /// option; otherwise they wait for step 18, after the summon has actually
    /// succeeded. That is the difference between counting attempts and
    /// counting successes, and the duel option chooses.
    fn summon_step_12(
        &mut self,
        sumplayer: u8,
        target: CardId,
        state: &mut SummonRuleState,
    ) -> bool {
        let controller = self.cards[target].current.controller;
        self.set_control(target, controller, 0, 0);
        self.core.phase_action = true;
        self.cards[target].reason = reason::SUMMON;
        self.cards[target].summon.player = sumplayer;

        let info = self.get_info_location(target);
        let code_ = self.cards[target].data.code;
        self.messages.push(Message::Summoning {
            code: code_,
            controller: info.controller,
            location: info.location,
            sequence: info.sequence,
            position: info.position,
        });
        if self.is_flag(crate::duel::flags::CANNOT_SUMMON_OATH_OLD) {
            self.bump_summon_counters(sumplayer, target);
        }
        let materials = self.cards[target].material_cards.clone();
        if !materials.is_empty() {
            for &m in &materials {
                self.raise_single_event(
                    m,
                    vec![],
                    code::BE_PRE_MATERIAL,
                    state.procedure,
                    reason::SUMMON,
                    sumplayer,
                    sumplayer,
                    0,
                );
            }
            self.raise_event_over(
                materials.iter().copied().collect(),
                code::BE_PRE_MATERIAL,
                state.procedure,
                reason::SUMMON,
                sumplayer,
                sumplayer,
                0,
            );
        }
        self.process_single_event();
        self.process_instant_event();
        false
    }

    fn bump_summon_counters(&mut self, sumplayer: u8, target: CardId) {
        self.core.summon_state_count[sumplayer as usize] += 1;
        self.core.normalsummon_state_count[sumplayer as usize] += 1;
        self.check_card_counter(target, crate::summon_support::activity::SUMMON, sumplayer);
        self.check_card_counter(
            target,
            crate::summon_support::activity::NORMALSUMMON,
            sumplayer,
        );
    }

    /// Step 13: is this summon negatable?
    ///
    /// Only outside a chain, and only if nothing says it cannot be. Both
    /// answers jump to 15 — the difference is that the negatable path passes
    /// through 14 first, which is what opens the window.
    fn summon_step_13(&mut self, target: CardId) -> bool {
        let in_chain = !self.core.current_chain.is_empty();
        let cannot_negate = self
            .is_affected_by_effect(target, code::CANNOT_DISABLE_SUMMON)
            .is_some();
        if in_chain || cannot_negate {
            self.set_step(15);
        }
        false
    }

    /// Step 14: open the negation window.
    fn summon_step_14(
        &mut self,
        sumplayer: u8,
        target: CardId,
        state: &mut SummonRuleState,
    ) -> bool {
        self.cards[target].set_status(status::SUMMONING, true);
        self.cards[target].set_status(status::SUMMON_DISABLED, false);
        self.raise_event(
            Some(target),
            code::SUMMON,
            state.procedure,
            0,
            sumplayer,
            sumplayer,
            0,
        );
        self.process_instant_event();
        // **The negation window skips everything** — `PointEvent(true, true,
        // true)` in the reference (`SummonRule` case 14, `operations.cpp`): no triggers, no free chains, no new
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

    /// Step 15: was it negated?
    ///
    /// **The polarity is the trap.** `STATUS_SUMMONING` still *set* means
    /// the summon survived — nothing cleared it — and the machine goes on to
    /// 16. The status is cleared by whatever negated the summon, so reaching
    /// past that test means it was negated: the oaths are released, an oath
    /// count code is refunded, and the card goes to the graveyard from a
    /// Monster Zone it never legitimately occupied.
    fn summon_step_15(
        &mut self,
        sumplayer: u8,
        target: CardId,
        state: &mut SummonRuleState,
    ) -> bool {
        if self.cards[target].is_status(status::SUMMONING) {
            // Still summoning: nothing took it away, so the summon stuck.
            // (`arg.step = 15` in the reference is a no-op assignment; the
            // `return FALSE` is what carries it to 16.)
            self.set_step(15);
            return false;
        }
        if let Some(p) = state.procedure {
            if !self.is_flag(crate::duel::flags::CANNOT_SUMMON_OATH_OLD) {
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
        for &e in &state.cost_effects.clone() {
            self.remove_oath_effect(e);
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

    /// Step 16: the summon stuck. Release the oath relations and enable the
    /// card.
    ///
    /// A card that is `STATUS_DISABLED` has that reset here: it arrived
    /// disabled by whatever was on the field, and a successful summon clears
    /// it so the card's own effects can start.
    fn summon_step_16(&mut self, target: CardId, state: &mut SummonRuleState) -> bool {
        if let Some(p) = state.procedure {
            self.release_oath_relation(p);
        }
        for &e in &state.cost_effects.clone() {
            self.release_oath_relation(e);
        }
        self.cards[target].set_status(status::SUMMONING, false);
        self.cards[target].set_status(status::SUMMON_TURN, true);
        self.enable_field_effect(target, true);
        if self.cards[target].is_status(status::DISABLED) {
            self.reset_card(target, reset::DISABLE, reset::EVENT);
        }
        false
    }

    /// Step 17: tell the materials it worked.
    fn summon_step_17(
        &mut self,
        sumplayer: u8,
        target: CardId,
        state: &mut SummonRuleState,
    ) -> bool {
        self.messages.push(Message::Summoned);
        self.adjust_instant();
        let materials = self.cards[target].material_cards.clone();
        if !materials.is_empty() {
            for &m in &materials {
                self.raise_single_event(
                    m,
                    vec![],
                    code::BE_MATERIAL,
                    state.procedure,
                    reason::SUMMON,
                    sumplayer,
                    sumplayer,
                    0,
                );
            }
            self.raise_event_over(
                materials.iter().copied().collect(),
                code::BE_MATERIAL,
                state.procedure,
                reason::SUMMON,
                sumplayer,
                sumplayer,
                0,
            );
        }
        self.process_single_event();
        self.process_instant_event();
        false
    }

    /// Step 18: the summon succeeded.
    ///
    /// The counters are bumped here unless the old oath option already did it
    /// at step 12 — counting *successes* rather than *attempts*.
    fn summon_step_18(
        &mut self,
        sumplayer: u8,
        target: CardId,
        state: &mut SummonRuleState,
    ) -> bool {
        if !self.is_flag(crate::duel::flags::CANNOT_SUMMON_OATH_OLD) {
            self.bump_summon_counters(sumplayer, target);
        }
        self.raise_single_event(
            target,
            vec![],
            code::SUMMON_SUCCESS,
            state.procedure,
            0,
            sumplayer,
            sumplayer,
            0,
        );
        self.process_single_event();
        self.raise_event(
            Some(target),
            code::SUMMON_SUCCESS,
            state.procedure,
            0,
            sumplayer,
            sumplayer,
            0,
        );
        self.process_instant_event();
        if self.core.current_chain.is_empty() {
            self.adjust_all();
            self.core.hint_timing[sumplayer as usize] |= timing::SUMMON;
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

    /// **The negation window skips everything** — `SummonRule` case 14 in
    /// the reference emplaces `PointEvent(true, true, true)`. Fuzz seed 19
    /// found the port asking the wrong player first here, because the
    /// default (all false) ran the full offer region.
    #[test]
    fn the_negation_window_skips_triggers_free_chains_and_new_chains() {
        let mut f = Field::new(8000);
        f.infos.turn_player = 0;
        f.infos.phase = crate::duel::phases::MAIN1;
        let target = in_hand(&mut f, 4);
        let mut state = SummonRuleState::default();
        f.summon_step_14(0, target, &mut state);
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
    use crate::card::{Card, CardData};
    use crate::effect::{effect_type, flag, Effect};
    use crate::processor::Status;

    fn monster(f: &mut Field, level: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 18036057,
                type_: card_type::MONSTER,
                level,
                ..Default::default()
            },
            0,
        );
        c.current.controller = 0;
        c.set_status(status::EFFECT_ENABLED, true);
        f.new_card(c)
    }

    fn in_hand(f: &mut Field, level: u32) -> CardId {
        let id = monster(f, level);
        f.add_card(0, id, location::HAND, 0, false);
        id
    }

    /// A face-up monster already in a Monster Zone.
    ///
    /// `add_card` silently refuses a card that is already placed, so the
    /// card is made unplaced and then placed — a test that skips the
    /// `remove_card` leaves it in the hand and proves nothing.
    fn on_field(f: &mut Field, level: u32, seat: u32) -> CardId {
        let id = monster(f, level);
        f.add_card(0, id, location::MZONE, seat, false);
        f.cards[id].current.position = position::FACEUP_ATTACK;
        assert_eq!(f.cards[id].current.location, location::MZONE);
        id
    }

    /// An effect the card carries itself.
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

    /// A field-wide effect that applies to player 0.
    fn player_effect(f: &mut Field, card: CardId, code_: u32, value: i64) -> EffectId {
        let mut e = Effect::new(effect_type::FIELD, code_);
        e.owner = Some(card);
        e.handler = Some(card);
        e.value = value;
        e.flag[0] = flag::PLAYER_TARGET | flag::ABSOLUTE_TARGET;
        // `range` is where the *card* must be for the effect to work;
        // `s_range`/`o_range` are which player it applies to.
        e.range = u16::from(location::MZONE);
        e.s_range = 1;
        let id = f.new_effect(e);
        f.add_effect(id, 0);
        id
    }

    fn drive(f: &mut Field, seat: i8, position: i32, mut watch: impl FnMut(&Field)) -> Status {
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
                    Some(Message::SelectPosition { .. }) => f.core.returns.set(position),
                    // "Do you want to tribute?" — yes, so the tribute path
                    // is the one the tests exercise.
                    Some(Message::SelectYesNo { .. }) => f.core.returns.set(1),
                    // A response window with nothing in it: decline.
                    Some(Message::SelectChain { .. }) => f.core.returns.set(-1),
                    _ => return Status::Awaiting,
                },
                other => return other,
            }
        }
        panic!("did not settle");
    }

    /// Run until the machine yields on a placement question, or finishes.
    ///
    /// Declines the response windows, because a refused summon opens none
    /// and a successful one opens two — answering them is not what these
    /// tests are about. Everything else is left for the caller to meet.
    fn run(f: &mut Field) -> Status {
        for _ in 0..4096 {
            match f.process() {
                Status::Continue => continue,
                Status::Awaiting => match f.messages.last() {
                    Some(Message::SelectChain { .. }) => f.core.returns.set(-1),
                    _ => return Status::Awaiting,
                },
                other => return other,
            }
        }
        panic!("did not settle");
    }

    fn run_placing(f: &mut Field, seat: i8, position: i32) -> Status {
        drive(f, seat, position, |_| {})
    }

    /// The same, recording the **lowest** `count_limit` each watched
    /// effect reached along the way.
    ///
    /// Reading the residue after the summon no longer works: a card
    /// arriving on the field takes a `RESET_TOFIELD`, and that recharges
    /// every count-limited effect on it (`card::reset_effect_count`). So
    /// the charge has to be seen while it is happening, which is what the
    /// watcher is for.
    fn run_placing_watching(
        f: &mut Field,
        seat: i8,
        position: i32,
        watched: &[EffectId],
    ) -> Vec<u8> {
        let mut low: Vec<u8> = watched
            .iter()
            .map(|&e| f.effects.get(e).map_or(0, |x| x.count_limit))
            .collect();
        let ids = watched.to_vec();
        drive(f, seat, position, |f| {
            for (i, &e) in ids.iter().enumerate() {
                if let Some(x) = f.effects.get(e) {
                    if x.count_limit < low[i] {
                        low[i] = x.count_limit;
                    }
                }
            }
        });
        low
    }

    /// Answer a tribute question by offered index.
    fn take_tributes(f: &mut Field, indices: &[i32]) {
        f.core.returns.set_i32(0, 0);
        f.core.returns.set_i32(1, indices.len() as i32);
        for (i, &idx) in indices.iter().enumerate() {
            f.core.returns.set_i32(i + 2, idx);
        }
    }

    /// A low-level monster arrives face-up in attack, with no question but
    /// the seat.
    #[test]
    fn a_low_level_monster_is_summoned_face_up_in_attack() {
        let mut f = Field::new(8000);
        let c = in_hand(&mut f, 4);
        f.summon(0, c, None, false, 0, 0x1f);
        assert_eq!(run_placing(&mut f, 2, 0x1), Status::End);
        assert_eq!(f.cards[c].current.location, location::MZONE);
        assert_eq!(f.cards[c].current.sequence, 2, "the seat that was chosen");
        assert_eq!(f.cards[c].current.position, position::FACEUP_ATTACK);
        assert!(f.cards[c].is_status(status::SUMMON_TURN));
    }

    /// The summon is recorded as `NORMAL`, from the hand.
    #[test]
    fn the_summon_is_recorded_as_normal_from_the_hand() {
        let mut f = Field::new(8000);
        let c = in_hand(&mut f, 4);
        f.summon(0, c, None, false, 0, 0x1f);
        run_placing(&mut f, 0, 0x1);
        assert_eq!(f.cards[c].summon.type_, summon_type::NORMAL);
        assert_eq!(f.cards[c].summon.location, location::HAND);
        assert_eq!(f.cards[c].summon.player, 0);
    }

    #[test]
    fn summoning_spends_the_normal_summon() {
        let mut f = Field::new(8000);
        let c = in_hand(&mut f, 4);
        f.summon(0, c, None, false, 0, 0x1f);
        run_placing(&mut f, 0, 0x1);
        assert_eq!(f.core.summon_count[0], 1);
        assert_eq!(f.core.summon_count[1], 0, "and not the opponent's");
    }

    #[test]
    fn ignore_count_spends_nothing() {
        let mut f = Field::new(8000);
        let c = in_hand(&mut f, 4);
        f.summon(0, c, None, true, 0, 0x1f);
        run_placing(&mut f, 0, 0x1);
        assert_eq!(f.core.summon_count[0], 0);
        assert!(!f.core.extra_summon[0]);
    }

    /// **Both** activity tallies are bumped, where a set bumps only one.
    ///
    /// `ACTIVITY_SUMMON` counts summons of every kind and
    /// `ACTIVITY_NORMALSUMMON` only the normal ones; a normal summon is both,
    /// and a set is only the latter. The contrast is asserted here rather
    /// than in `MonsterSet`'s tests because it is this machine that made the
    /// distinction observable.
    #[test]
    fn a_summon_bumps_both_tallies_and_a_set_only_one() {
        let mut f = Field::new(8000);
        let c = in_hand(&mut f, 4);
        f.summon(0, c, None, false, 0, 0x1f);
        run_placing(&mut f, 0, 0x1);
        assert_eq!(f.core.summon_state_count[0], 1);
        assert_eq!(f.core.normalsummon_state_count[0], 1);

        let mut g = Field::new(8000);
        let d = in_hand(&mut g, 4);
        g.mset(0, d, None, false, 0, 0x1f);
        run_placing(&mut g, 0, 0x1);
        assert_eq!(g.core.summon_state_count[0], 0, "a set is not a summon");
        assert_eq!(g.core.normalsummon_state_count[0], 1);
    }

    /// **`DUEL_CANNOT_SUMMON_OATH_OLD` moves the tallies, it does not add
    /// to them.** Under the old option they are bumped when the summon is
    /// announced rather than when it succeeds — once either way.
    #[test]
    fn the_old_oath_option_bumps_the_tallies_once() {
        let mut f = Field::with_flags(
            8000,
            crate::duel::REFERENCE_CONFIGURATION | crate::duel::flags::CANNOT_SUMMON_OATH_OLD,
        );
        let c = in_hand(&mut f, 4);
        f.summon(0, c, None, false, 0, 0x1f);
        run_placing(&mut f, 0, 0x1);
        assert_eq!(f.cards[c].current.location, location::MZONE);
        assert_eq!(f.core.summon_state_count[0], 1);
        assert_eq!(f.core.normalsummon_state_count[0], 1);
    }

    /// `TIMING_SUMMON` is charged, and it is charged **while the machine
    /// still has it**: step 18 sets it and the window it then opens spends
    /// it, so a test that looks afterwards sees nothing.
    #[test]
    fn summoning_raises_its_event_and_charges_the_timing() {
        let mut f = Field::new(8000);
        let c = in_hand(&mut f, 4);
        f.summon(0, c, None, false, 0, 0x1f);
        let mut charged = false;
        drive(&mut f, 0, 0x1, |f| {
            charged |= f.core.hint_timing[0] & timing::SUMMON != 0;
        });
        assert!(charged, "the summon timing was opened");
        assert!(f.core.phase_action);
        assert!(
            f.messages
                .iter()
                .any(|m| matches!(m, Message::Summoning { .. })),
            "the summon was announced"
        );
        assert!(
            f.messages.iter().any(|m| matches!(m, Message::Summoned)),
            "and confirmed"
        );
    }

    /// The card's field effects are turned off before it is placed and back
    /// on once the summon has stuck.
    #[test]
    fn field_effects_are_off_while_the_summon_is_pending() {
        let mut f = Field::new(8000);
        let c = in_hand(&mut f, 4);
        assert!(
            f.cards[c].is_status(status::EFFECT_ENABLED),
            "set up enabled, so the clearing is visible"
        );
        f.summon(0, c, None, false, 0, 0x1f);
        let mut seen_off = false;
        drive(&mut f, 0, 0x1, |f| {
            seen_off |= !f.cards[c].is_status(status::EFFECT_ENABLED);
        });
        assert!(seen_off, "disabled for the move");
        assert!(
            f.cards[c].is_status(status::EFFECT_ENABLED),
            "and enabled again once it stuck"
        );
    }

    mod positions {
        use super::*;

        /// The duel option widens the choice to both face-up positions, and
        /// a choice is then actually asked for.
        #[test]
        fn the_duel_option_offers_face_up_defence() {
            let mut f = Field::with_flags(
                8000,
                crate::duel::REFERENCE_CONFIGURATION | crate::duel::flags::NORMAL_SUMMON_FACEUP_DEF,
            );
            let c = in_hand(&mut f, 4);
            f.summon(0, c, None, false, 0, 0x1f);
            run_placing(&mut f, 0, i32::from(position::FACEUP_DEFENSE));
            assert_eq!(f.cards[c].current.position, position::FACEUP_DEFENSE);
            assert!(
                f.messages
                    .iter()
                    .any(|m| matches!(m, Message::SelectPosition { .. })),
                "two positions is a real choice"
            );
        }

        /// `EFFECT_FORCE_NORMAL_SUMMON_POSITION` **narrows** the widened set
        /// back to one, and no choice is asked.
        #[test]
        fn forcing_a_position_narrows_the_choice_away() {
            let mut f = Field::with_flags(
                8000,
                crate::duel::REFERENCE_CONFIGURATION | crate::duel::flags::NORMAL_SUMMON_FACEUP_DEF,
            );
            let c = in_hand(&mut f, 4);
            let anchor = on_field(&mut f, 4, 4);
            player_effect(
                &mut f,
                anchor,
                code::FORCE_NORMAL_SUMMON_POSITION,
                i64::from(position::FACEUP_DEFENSE),
            );
            f.summon(0, c, None, false, 0, 0x1f);
            run_placing(&mut f, 0, 0x1);
            assert_eq!(f.cards[c].current.position, position::FACEUP_DEFENSE);
            assert!(
                !f.messages
                    .iter()
                    .any(|m| matches!(m, Message::SelectPosition { .. })),
                "one position left is no choice"
            );
        }
    }

    mod refusals {
        use super::*;

        /// `EFFECT_UNSUMMONABLE_CARD` and `EFFECT_CANNOT_SUMMON` each refuse.
        #[test]
        fn the_two_prohibitions_each_refuse() {
            for code_ in [code::UNSUMMONABLE_CARD, code::CANNOT_SUMMON] {
                let mut f = Field::new(8000);
                let c = in_hand(&mut f, 4);
                single(&mut f, c, code_, 0);
                f.summon(0, c, None, false, 0, 0x1f);
                run(&mut f);
                assert_eq!(f.core.summon_count[0], 0, "code {code_}");
                assert_eq!(f.cards[c].current.location, location::HAND);
            }
        }

        /// A card that must be Special Summoned is not a summonable card.
        #[test]
        fn the_special_summon_only_types_are_refused() {
            for extra in [
                card_type::RITUAL,
                card_type::SPSUMMON,
                card_type::FUSION,
                card_type::SYNCHRO,
                card_type::XYZ,
                card_type::LINK,
                card_type::TOKEN,
                card_type::TRAPMONSTER,
            ] {
                let mut f = Field::new(8000);
                let c = in_hand(&mut f, 4);
                f.cards[c].data.type_ = card_type::MONSTER | extra;
                assert!(!f.is_summonable_card(c), "type {extra:#x}");
            }
            let mut f = Field::new(8000);
            let c = in_hand(&mut f, 4);
            assert!(f.is_summonable_card(c), "a plain monster is");
        }

        /// **The guard `MonsterSet` does not have.** With the normal summon
        /// already spent there is nothing to offer, and the machine stops
        /// before it raises `summon_depth`.
        ///
        /// The depth is what the assertion turns on: a machine that refused
        /// *later* would have incremented it and left it raised, and every
        /// later summon in the duel would then believe it was nested.
        #[test]
        fn a_spent_normal_summon_refuses_before_the_depth_is_raised() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f, 4);
            f.core.summon_count[0] = 1;
            f.summon(0, c, None, false, 0, 0x1f);
            run(&mut f);
            assert_eq!(f.cards[c].current.location, location::HAND);
            assert_eq!(f.core.summon_depth, 0, "and the depth was never raised");
        }

        /// An inner summon does its work and stops: the outer one places the
        /// card and opens the window.
        ///
        /// The outer summon is stood in for by the depth counter, which is
        /// exactly what an outer summon leaves behind when its procedure
        /// runs.
        #[test]
        fn an_inner_summon_stops_at_step_eight() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f, 4);
            f.core.summon_depth = 1;
            f.summon(0, c, None, false, 0, 0x1f);
            assert_eq!(run(&mut f), Status::End);
            assert_eq!(f.cards[c].current.location, location::HAND, "never placed");
            assert_eq!(f.core.summon_count[0], 0, "and nothing was spent");
            assert_eq!(f.core.summon_depth, 1, "the outer summon's depth is left");
            assert!(
                !f.core.summon_cancelable,
                "an inner summon cannot be backed out of"
            );
        }
    }

    mod tributes {
        use super::*;

        /// A level-7 monster with no tributes available is not summoned.
        #[test]
        fn a_high_level_monster_without_tributes_is_not_summoned() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f, 7);
            f.summon(0, c, None, false, 0, 0x1f);
            run(&mut f);
            assert_eq!(f.cards[c].current.location, location::HAND);
            assert_eq!(f.core.summon_count[0], 0);
        }

        /// With tributes on the field the summon goes ahead, records
        /// `ADVANCE` **alongside** `NORMAL`, and keeps the tributes as its
        /// materials.
        #[test]
        fn a_tribute_summon_records_both_summon_types() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f, 7);
            let t1 = on_field(&mut f, 4, 0);
            let t2 = on_field(&mut f, 4, 1);

            f.summon(0, c, None, false, 0, 0x1f);
            assert_eq!(run(&mut f), Status::Awaiting, "which tributes?");
            take_tributes(&mut f, &[0, 1]);
            run_placing(&mut f, 0, 0x1);

            assert_eq!(
                f.cards[c].summon.type_,
                summon_type::NORMAL | summon_type::ADVANCE,
                "a tribute summon is still a normal summon"
            );
            assert_eq!(
                f.cards[c].material_cards,
                std::collections::BTreeSet::from([t1, t2]),
                "and the tributes are recorded as its materials"
            );
            assert_eq!(f.cards[t1].current.location, location::GRAVE);
            assert_eq!(f.cards[t2].current.location, location::GRAVE);
        }

        /// `EVENT_BE_MATERIAL` reaches each tribute.
        #[test]
        fn the_tributes_are_told_they_were_used() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f, 5);
            let t = on_field(&mut f, 4, 0);
            f.summon(0, c, None, false, 0, 0x1f);
            assert_eq!(run(&mut f), Status::Awaiting);
            take_tributes(&mut f, &[0]);
            run_placing(&mut f, 1, 0x1);
            assert_eq!(
                f.cards[c].material_cards,
                std::collections::BTreeSet::from([t])
            );
            assert!(
                f.core
                    .instant_event
                    .iter()
                    .chain(f.core.used_event.iter())
                    .any(|e| e.event_code == code::BE_MATERIAL),
                "the material event was raised"
            );
        }

        /// An uncounted reduction covers the shortfall and is announced.
        #[test]
        fn an_uncounted_reduction_is_announced() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f, 5);
            single(&mut f, c, code::DECREASE_TRIBUTE, 1);
            f.summon(0, c, None, false, 0, 0x1f);
            run_placing(&mut f, 0, 0x1);
            assert_eq!(f.cards[c].current.location, location::MZONE);
            assert!(
                f.messages.iter().any(|m| matches!(
                    m,
                    Message::Hint {
                        kind: crate::host_question::hint::CARD,
                        ..
                    }
                )),
                "the reduction that paid for it is named"
            );
        }

        /// **A counted reduction is spent even when nothing is left to pay.**
        ///
        /// The uncounted reduction already covers the whole shortfall, and
        /// the count-limited loop still charges one — it tests the shortfall
        /// only *after* spending. Faithful to the reference, and the reason
        /// this test exists is that the natural Rust version guards the loop
        /// and silently diverges.
        #[test]
        fn a_counted_reduction_is_spent_after_the_shortfall_is_covered() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f, 5);
            single(&mut f, c, code::DECREASE_TRIBUTE, 1);
            let counted = single(&mut f, c, code::DECREASE_TRIBUTE, 1);
            if let Some(e) = f.effects.get_mut(counted) {
                e.flag[0] |= flag::COUNT_LIMIT;
                e.count_limit = 1;
                e.target = Some(|_, _, _, _| crate::effect::Yield::Done(1));
            }
            f.summon(0, c, None, false, 0, 0x1f);
            run_placing(&mut f, 0, 0x1);
            assert_eq!(f.cards[c].current.location, location::MZONE);
            assert_eq!(
                f.effects.get(counted).map(|e| e.count_limit),
                Some(0),
                "charged although the shortfall was already covered"
            );
        }
    }

    /// The step machine's jump table, asserted directly.
    ///
    /// Two of these jumps are unobservable from outside — passing through
    /// step 4 with no procedure does nothing — and they are exactly the ones
    /// that were wrong, because `MonsterSet`'s numbers are one lower and the
    /// obvious thing to do is copy them. So they are pinned here rather than
    /// left to a behaviour that does not depend on them.
    mod jumps {
        use super::*;
        use crate::processor::Unit;

        /// Put a `SummonRule` unit on the queue at `step`, run that step, and
        /// report where it said to go next. The processor's own increment is
        /// applied, so the number returned is the case that runs next.
        fn jump_from(f: &mut Field, step: u16, target: CardId, procedure: Option<EffectId>) -> u16 {
            let mut state = SummonRuleState {
                procedure,
                decided: procedure.is_some(),
                ..Default::default()
            };
            let kind = Kind::SummonRule {
                sumplayer: 0,
                target,
                ignore_count: false,
                min_tribute: 0,
                zone: 0x1f,
                state: Box::new(state.clone()),
            };
            f.core.units.push_front(Unit::at(kind, step));
            let (mut mt, mut z) = (0u8, 0x1fu32);
            let done = f.summon_rule_step(step, 0, target, false, &mut mt, &mut z, &mut state);
            assert!(!done, "the step did not finish the machine");
            let next = f.queue().next().unwrap().step + 1;
            f.core.units.pop_front();
            next
        }

        /// A named procedure goes to the step that runs its `target`.
        #[test]
        fn a_procedure_goes_to_step_four() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f, 4);
            let e = single(&mut f, c, code::SUMMON_PROC, 0);
            assert_eq!(jump_from(&mut f, 2, c, Some(e)), 4);
        }

        /// The ordinary path **skips** it — `MonsterSet` passes through.
        #[test]
        fn the_ordinary_path_skips_to_step_five() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f, 4);
            assert_eq!(
                jump_from(&mut f, 2, c, None),
                5,
                "no tributes are possible, so it goes straight on"
            );
        }

        /// And so does the path that has just chosen tributes.
        #[test]
        fn choosing_tributes_also_goes_to_step_five() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f, 7);
            on_field(&mut f, 4, 0);
            on_field(&mut f, 4, 1);
            assert_eq!(jump_from(&mut f, 2, c, None), 5);
        }

        /// Step 3 — the answer to "do you want to tribute?" — joins them.
        #[test]
        fn the_tribute_question_rejoins_at_step_five() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f, 5);
            on_field(&mut f, 4, 0);
            f.core.returns.set(1);
            assert_eq!(jump_from(&mut f, 3, c, None), 5);
        }
    }

    mod extra_summon {
        use super::*;

        /// With the normal summon spent, an `EFFECT_EXTRA_SUMMON_COUNT`
        /// permission is what allows the summon — and it is the permission
        /// that is spent, **not** a second normal summon.
        #[test]
        fn the_permission_is_spent_rather_than_a_second_summon() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f, 4);
            single(&mut f, c, code::EXTRA_SUMMON_COUNT, 0);
            f.core.summon_count[0] = 1;
            f.summon(0, c, None, false, 0, 0x1f);
            run_placing(&mut f, 0, 0x1);
            assert_eq!(f.cards[c].current.location, location::MZONE, "it happened");
            assert!(f.core.extra_summon[0], "the permission was spent");
            assert_eq!(f.core.summon_count[0], 1, "and not a second normal summon");
        }

        /// The Gemini path takes its permission too — without asking, since
        /// there is nothing to choose between.
        #[test]
        fn the_gemini_path_takes_its_permission() {
            let mut f = Field::new(8000);
            let c = on_field(&mut f, 4, 0);
            single(&mut f, c, code::GEMINI_SUMMONABLE, 0);
            single(&mut f, c, code::EXTRA_SUMMON_COUNT, 0);
            f.core.summon_count[0] = 1;
            f.summon(0, c, None, false, 0, 0x1f);
            run_placing(&mut f, 0, 0x1);
            assert!(
                f.is_affected_by_effect(c, code::GEMINI_STATUS).is_some(),
                "the Gemini summon happened"
            );
            assert!(f.core.extra_summon[0], "on the permission");
            assert_eq!(f.core.summon_count[0], 1);
        }
    }

    mod reductions {
        use super::*;

        /// A count-limited reduction.
        ///
        /// A **zero** value is usually what these tests want: a reduction
        /// that does not move the shortfall lets the loop keep spending,
        /// which is what makes *how much shortfall was left* — and therefore
        /// which effects were reached — observable from outside.
        fn counted(f: &mut Field, card: CardId, value: i64, limit: u8, targeted: bool) -> EffectId {
            let e = single(f, card, code::DECREASE_TRIBUTE, value);
            if let Some(x) = f.effects.get_mut(e) {
                x.flag[0] |= flag::COUNT_LIMIT;
                // Both halves, as `Effect.SetCountLimit` sets them: the
                // ceiling is what a recharge restores, and an effect with
                // a limit but no ceiling is one no script could build.
                x.count_limit = limit;
                x.count_limit_max = limit;
                if targeted {
                    x.target = Some(|_, _, _, _| crate::effect::Yield::Done(1));
                }
            }
            e
        }

        fn counted_zero(f: &mut Field, card: CardId) -> EffectId {
            counted(f, card, 0, 1, true)
        }

        /// **The largest uncounted reduction is taken, not the last one.**
        ///
        /// Both reductions are free, so which is taken costs nothing
        /// directly; what it decides is how much shortfall is left for the
        /// count-limited ones to be spent on. Taking the smaller leaves one
        /// tribute still owed, and a second counted reduction is then spent
        /// paying for it.
        #[test]
        fn the_largest_uncounted_reduction_is_the_one_taken() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f, 7);
            // Smallest first and smallest last, so neither "take the first"
            // nor "take the last" happens to agree with "take the largest".
            single(&mut f, c, code::DECREASE_TRIBUTE, 1);
            single(&mut f, c, code::DECREASE_TRIBUTE, 2);
            single(&mut f, c, code::DECREASE_TRIBUTE, 1);
            let first = counted_zero(&mut f, c);
            let second = counted_zero(&mut f, c);

            f.summon(0, c, None, false, 0, 0x1f);
            let low = run_placing_watching(&mut f, 0, 0x1, &[first, second]);
            assert_eq!(f.cards[c].current.location, location::MZONE);
            assert_eq!(low[0], 0, "one is always spent");
            // The **low-water mark**, not the value now: arriving on the
            // field recharges both, so a reading taken afterwards cannot
            // tell an untouched effect from a spent-and-recharged one.
            assert_eq!(
                low[1], 1,
                "and the second only if the smaller reduction was taken"
            );
        }

        /// **Equal reductions keep the first.** `minul < dec` is strictly
        /// less, so a later reduction of the same size does not displace an
        /// earlier one — and since the effects arrive sorted by id, "first"
        /// means the one created first.
        ///
        /// The two are made to belong to different cards so that the hint
        /// names one of them: which effect was taken is otherwise invisible
        /// when both reduce the shortfall by the same amount.
        #[test]
        fn equal_reductions_keep_the_first() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f, 7);
            let other = on_field(&mut f, 4, 0);
            f.cards[other].data.code = 55144522;

            single(&mut f, c, code::DECREASE_TRIBUTE, 2);
            let second = single(&mut f, c, code::DECREASE_TRIBUTE, 2);
            // Indexed on the summoning card, but owned by the other one, so
            // the hint it emits names a different card.
            if let Some(e) = f.effects.get_mut(second) {
                e.handler = Some(other);
            }

            f.summon(0, c, None, false, 0, 0x1f);
            run_placing(&mut f, 1, 0x1);
            let named: Vec<u64> = f
                .messages
                .iter()
                .filter_map(|m| match m {
                    Message::Hint {
                        kind: crate::host_question::hint::CARD,
                        value,
                        ..
                    } => Some(*value),
                    _ => None,
                })
                .collect();
            assert_eq!(
                named,
                vec![18036057],
                "the first of two equal reductions is the one taken"
            );
        }

        /// A reduction whose charges are used up is passed over, not spent
        /// again — and the one behind it is reached because of that.
        #[test]
        fn a_spent_reduction_is_passed_over() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f, 7);
            // Enough to make the summon possible with no tributes at all.
            single(&mut f, c, code::DECREASE_TRIBUTE, 2);
            // Already used up, and large enough that charging it would take
            // the shortfall past zero and stop the loop.
            counted(&mut f, c, 2, 0, true);
            let live = counted_zero(&mut f, c);

            f.summon(0, c, None, false, 0, 0x1f);
            let low = run_placing_watching(&mut f, 0, 0x1, &[live]);
            assert_eq!(f.cards[c].current.location, location::MZONE);
            assert_eq!(
                low[0], 0,
                "the loop reached the one that still had a charge"
            );
        }

        /// **Targeted reductions are spent before untargeted ones.**
        ///
        /// Both loops spend at least one, so the order only shows when one
        /// loop is stopped early: here the untargeted reduction covers the
        /// whole shortfall, so whichever loop runs second spends exactly one
        /// and stops. Running the targeted loop first spends both of its
        /// own; running it second leaves one untouched.
        #[test]
        fn targeted_reductions_are_spent_first() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f, 7);
            let t1 = counted(&mut f, c, 0, 1, true);
            let t2 = counted(&mut f, c, 0, 1, true);
            let u = counted(&mut f, c, 2, 1, false);

            f.summon(0, c, None, false, 0, 0x1f);
            let low = run_placing_watching(&mut f, 0, 0x1, &[t1, t2, u]);
            assert_eq!(f.cards[c].current.location, location::MZONE);
            assert_eq!(low[0], 0);
            assert_eq!(
                low[1], 0,
                "both targeted ones were reached before the untargeted one"
            );
            assert_eq!(low[2], 0);
        }

        /// **The shortfall is by level, and level 6 owes one tribute.**
        ///
        /// With that one tribute paid there is nothing left to reduce, so
        /// nothing is charged. A machine that thought level 6 owed two would
        /// spend a reduction here.
        #[test]
        fn a_level_six_owes_one_tribute() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f, 6);
            on_field(&mut f, 4, 0);
            let counted = counted_zero(&mut f, c);

            f.summon(0, c, None, false, 0, 0x1f);
            assert_eq!(run(&mut f), Status::Awaiting, "tribute or not?");
            // The reduction makes the tribute optional, so the machine asks
            // first; take the tribute.
            f.core.returns.set(1);
            assert_eq!(run(&mut f), Status::Awaiting, "which tribute?");
            take_tributes(&mut f, &[0]);
            run_placing(&mut f, 1, 0x1);

            assert_eq!(f.cards[c].current.location, location::MZONE);
            assert_eq!(
                f.effects.get(counted).map(|e| e.count_limit),
                Some(1),
                "one tribute covers a level six, so nothing was charged"
            );
        }
    }

    mod gemini {
        use super::*;

        fn gemini_on_field(f: &mut Field) -> CardId {
            let c = on_field(f, 4, 0);
            single(f, c, code::GEMINI_SUMMONABLE, 0);
            c
        }

        /// A Gemini summon does not move the card. What it does is give it
        /// `EFFECT_GEMINI_STATUS`.
        #[test]
        fn a_gemini_summon_grants_the_status_without_moving() {
            let mut f = Field::new(8000);
            let c = gemini_on_field(&mut f);
            f.summon(0, c, None, false, 0, 0x1f);
            assert_eq!(
                run(&mut f),
                Status::End,
                "nothing to ask: it is already there"
            );
            assert_eq!(f.cards[c].current.location, location::MZONE);
            assert_eq!(f.cards[c].current.sequence, 0);
            assert!(
                f.is_affected_by_effect(c, code::GEMINI_STATUS).is_some(),
                "it gained its effect"
            );
            assert_eq!(
                f.cards[c].summon.location,
                location::MZONE,
                "summoned from where it stood, not from the hand"
            );
            assert_eq!(f.core.summon_count[0], 1, "and it cost the normal summon");
        }

        /// Twice is refused: the status it gains the first time is the thing
        /// step 0 checks for.
        #[test]
        fn a_gemini_that_already_has_the_status_is_refused() {
            let mut f = Field::new(8000);
            let c = gemini_on_field(&mut f);
            f.summon(0, c, None, false, 0, 0x1f);
            run(&mut f);
            assert_eq!(f.core.summon_count[0], 1);

            f.core.summon_count[0] = 0;
            f.summon(0, c, None, false, 0, 0x1f);
            run(&mut f);
            assert_eq!(f.core.summon_count[0], 0, "the second time is refused");
        }

        /// Without `EFFECT_GEMINI_SUMMONABLE` a monster on the field is not
        /// summonable at all.
        #[test]
        fn an_ordinary_monster_on_the_field_cannot_be_summoned() {
            let mut f = Field::new(8000);
            let c = on_field(&mut f, 4, 0);
            f.summon(0, c, None, false, 0, 0x1f);
            run(&mut f);
            assert_eq!(f.core.summon_count[0], 0);
            assert!(f.is_affected_by_effect(c, code::GEMINI_STATUS).is_none());
        }

        /// A face-down monster is not summonable either, `GEMINI_SUMMONABLE`
        /// or not.
        #[test]
        fn a_face_down_monster_is_refused() {
            let mut f = Field::new(8000);
            let c = gemini_on_field(&mut f);
            f.cards[c].current.position = position::FACEDOWN_DEFENSE;
            f.summon(0, c, None, false, 0, 0x1f);
            run(&mut f);
            assert_eq!(f.core.summon_count[0], 0);
        }
    }

    mod negation {
        use super::*;

        /// Run, negating the summon when the machine reaches the step that
        /// reads the verdict.
        ///
        /// Clearing `STATUS_SUMMONING` is what a negating effect does; there
        /// is no card in the pool yet that does it, so the test does it at
        /// the moment one would have. The wait for `subunits` to be empty is
        /// what puts it *after* the response window rather than before it.
        fn run_negating(f: &mut Field, card: CardId, seat: i8) -> Status {
            for _ in 0..2048 {
                let at_verdict = f
                    .queue()
                    .next()
                    .is_some_and(|u| matches!(u.kind, Kind::SummonRule { .. }) && u.step == 15)
                    && f.core.subunits.is_empty();
                if at_verdict {
                    f.cards[card].set_status(status::SUMMONING, false);
                    f.cards[card].set_status(status::SUMMON_DISABLED, true);
                }
                match f.process() {
                    Status::Continue => continue,
                    Status::Awaiting => match f.messages.last() {
                        Some(Message::SelectPlace { .. }) => {
                            f.core.returns.set_i8(0, 0);
                            f.core.returns.set_i8(1, location::MZONE as i8);
                            f.core.returns.set_i8(2, seat);
                        }
                        Some(Message::SelectChain { .. }) => f.core.returns.set(-1),
                        _ => return Status::Awaiting,
                    },
                    other => return other,
                }
            }
            panic!("did not settle");
        }

        /// A negated summon leaves the card in the graveyard, and it never
        /// counted as a summon.
        #[test]
        fn a_negated_summon_sends_the_card_to_the_graveyard() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f, 4);
            f.summon(0, c, None, false, 0, 0x1f);
            run_negating(&mut f, c, 0);
            assert_eq!(f.cards[c].current.location, location::GRAVE);
            assert!(!f.cards[c].is_status(status::SUMMON_TURN));
            assert_eq!(
                f.core.summon_state_count[0], 0,
                "the tallies are bumped on success, not on the attempt"
            );
            assert_eq!(
                f.core.summon_count[0], 1,
                "but the normal summon was spent before the window opened"
            );
        }

        /// **A summon during a chain's resolution opens no window of its
        /// own.** The chain already has one.
        #[test]
        fn a_summon_inside_a_chain_opens_no_window() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f, 4);
            let anchor = on_field(&mut f, 4, 4);
            let e = single(&mut f, anchor, code::UPDATE_ATTACK, 0);
            let mut chain = crate::chain::Chain::new(e, crate::event::Event::new(0));
            chain.triggering_player = 0;
            f.core.current_chain.push(chain);

            f.summon(0, c, None, false, 0, 0x1f);
            run_placing(&mut f, 0, 0x1);
            assert_eq!(f.cards[c].current.location, location::MZONE, "it happened");
            assert!(
                !f.core
                    .instant_event
                    .iter()
                    .chain(f.core.used_event.iter())
                    .any(|e| e.event_code == code::SUMMON),
                "and no negation window was opened"
            );
        }

        /// `EFFECT_CANNOT_DISABLE_SUMMON` skips the window entirely — no
        /// `EVENT_SUMMON` is ever raised.
        #[test]
        fn a_summon_that_cannot_be_disabled_never_opens_the_window() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f, 4);
            single(&mut f, c, code::CANNOT_DISABLE_SUMMON, 0);
            f.summon(0, c, None, false, 0, 0x1f);
            run_placing(&mut f, 0, 0x1);
            assert_eq!(f.cards[c].current.location, location::MZONE);
            assert!(
                !f.cards[c].is_status(status::SUMMONING),
                "it was never marked as summoning"
            );
            assert!(
                !f.core
                    .instant_event
                    .iter()
                    .chain(f.core.used_event.iter())
                    .any(|e| e.event_code == code::SUMMON),
                "and the window's event was never raised"
            );
            assert_eq!(f.core.summon_state_count[0], 1, "it still succeeded");
        }

        /// The window *is* opened for an ordinary summon — the contrast the
        /// test above needs to mean anything.
        #[test]
        fn an_ordinary_summon_does_open_the_window() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f, 4);
            f.summon(0, c, None, false, 0, 0x1f);
            run_placing(&mut f, 0, 0x1);
            assert!(
                f.core
                    .instant_event
                    .iter()
                    .chain(f.core.used_event.iter())
                    .any(|e| e.event_code == code::SUMMON),
                "EVENT_SUMMON was raised"
            );
        }
    }
}

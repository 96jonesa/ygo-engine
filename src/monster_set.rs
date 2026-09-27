//! Setting a monster: `mset` and `MonsterSet`.
//!
//! Eleven steps, and the shape is a **fork that rejoins**. Steps 0-4 decide
//! *how* the set happens and what it costs; then the ordinary path and the
//! procedure path run separately; then both arrive at step 8 to pay for it
//! and step 9 to put the card down.
//!
//! ```text
//!  0 → 1 → 2 →─┬─ 3 → 4 →─┬─ 5 ─────→ 8 → 9 → 10     (ordinary)
//!              └──────────┴─ 6 → 7 ──→ 8 → 9 → 10     (a procedure)
//! ```
//!
//! Reading the cases in order suggests one sequence; there are two.
//!
//! ## `SelectOption` is asked twice, about different things
//!
//! Step 0 asks **which procedure** to use — the ordinary set, or one of the
//! card's own `SET_PROC` effects. Step 1 asks **which extra-summon
//! permission** to spend, when the player has already used their normal
//! summon. Both use `core.select_effects` as the answer table, and both
//! short-circuit when there is only one option: a choice of one is not a
//! choice.
//!
//! `core.select_effects` holds a **`None` in the first slot** for "the
//! ordinary one", which is why the answer indexes a table of `Option`
//! rather than of effects.
//!
//! ## The tribute question has two shapes
//!
//! Step 2 either asks *whether* to tribute (`SelectYesNo`, when none is
//! required but the field has room) or *which* (`select_tribute_cards`).
//! The first is the "you may tribute for no reason" case a monster needing
//! no tributes still allows.

use crate::board::{location, position};
use crate::card::{card_type, reason, status, summon_type};
use crate::event::{code, CardId, EffectId};
use crate::field::{timing, Field, Message};
use crate::processor::Kind;
use crate::tribute::Procedures;

/// The state `MonsterSet` carries across its steps.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MonsterSetState {
    /// The procedure chosen at step 0, once chosen. `None` means the
    /// ordinary set — which is a real answer, not "not yet decided".
    pub procedure: Option<EffectId>,
    /// The extra-summon permission chosen at step 1, if one was needed.
    pub extra_summon: Option<EffectId>,
    /// A **set**, as the reference's `card_set` is — the release order is
    /// creation order, not the order the player picked them in.
    pub tributes: std::collections::BTreeSet<CardId>,
    /// The maximum the `SelectYesNo` branch will allow if the player says
    /// yes, parked because the question is asked in one step and answered in
    /// another.
    pub max_allowed_tributes: u8,
    /// Whether step 0 has already run its filter. Step 0 is entered once,
    /// but `procedure` being `None` is ambiguous without this.
    pub decided: bool,
}

impl Field {
    /// `field::mset` — queue a monster to be set.
    pub fn mset(
        &mut self,
        setplayer: u8,
        target: CardId,
        procedure: Option<EffectId>,
        ignore_count: bool,
        min_tribute: u8,
        zone: u32,
    ) {
        self.emplace(Kind::MonsterSet {
            setplayer,
            target,
            ignore_count,
            min_tribute,
            zone,
            state: Box::new(MonsterSetState {
                procedure,
                decided: procedure.is_some(),
                ..Default::default()
            }),
        });
    }

    /// One step of `MonsterSet`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn monster_set_step(
        &mut self,
        step: u16,
        setplayer: u8,
        target: CardId,
        ignore_count: bool,
        min_tribute: &mut u8,
        zone: &mut u32,
        state: &mut MonsterSetState,
    ) -> bool {
        match step {
            0 => self.mset_step_0(setplayer, target, ignore_count, *min_tribute, *zone, state),
            1 => self.mset_step_1(setplayer, target, ignore_count, *min_tribute, *zone, state),
            2 => self.mset_step_2(setplayer, target, min_tribute, zone, state),
            3 => {
                // The yes/no answer: yes means "choose tributes", no means
                // "none". The polarity reads backwards because `returns` is
                // the *answer to the question* and the question was "do you
                // decline".
                if self.core.returns.get() != 0 {
                    self.core.return_cards.clear();
                } else {
                    let max = state.max_allowed_tributes;
                    let cancelable = self.core.summon_cancelable;
                    self.select_tribute_cards(
                        target, setplayer, cancelable, 1, max, setplayer, *zone,
                    );
                }
                false
            }
            4 => self.mset_step_4(setplayer, target, state),
            5 => self.mset_step_5(setplayer, target, state),
            6 => self.mset_step_6(setplayer, target, *min_tribute, *zone, state),
            7 => self.mset_step_7(setplayer, target, *min_tribute, *zone, state),
            8 => self.mset_step_8(setplayer, target, ignore_count, state),
            9 => self.mset_step_9(setplayer, target, *zone, state),
            10 => self.mset_step_10(setplayer, target, state),
            _ => true,
        }
    }

    /// Step 0: may this be set at all, and by which procedure?
    ///
    /// Four refusals before anything is asked, and the second is the one that
    /// is easy to forget: **a monster can only be set from the hand.** The
    /// others are `EFFECT_UNSUMMONABLE_CARD`, not being a monster, and
    /// `EFFECT_CANNOT_MSET`.
    ///
    /// With a procedure already named the filter is used only to *validate*
    /// it. Without one, the answers become a `SelectOption` — and the
    /// `Forbidden` case ends the unit, since a limit that nothing satisfies
    /// means the set cannot happen.
    fn mset_step_0(
        &mut self,
        setplayer: u8,
        target: CardId,
        ignore_count: bool,
        min_tribute: u8,
        zone: u32,
        state: &mut MonsterSetState,
    ) -> bool {
        if self
            .is_affected_by_effect(target, code::UNSUMMONABLE_CARD)
            .is_some()
        {
            return true;
        }
        // Only from the hand.
        if self.cards[target].current.location != location::HAND {
            return true;
        }
        if !self.cards[target].data.is_type(card_type::MONSTER) {
            return true;
        }
        if self
            .is_affected_by_effect(target, code::CANNOT_MSET)
            .is_some()
        {
            return true;
        }

        let found = self.filter_set_procedure(target, setplayer, ignore_count, min_tribute, zone);
        if let Some(named) = state.procedure {
            let ok = match &found {
                Procedures::Forbidden => false,
                _ => self.check_set_procedure(
                    target,
                    named,
                    setplayer,
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
                // A limit that nothing satisfies: the set cannot happen.
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
                // The `None` slot: "the ordinary set".
                self.core.select_effects.push(None);
                self.core.select_options.push(1);
            }
            for e in procedures {
                let d = self.effects.get(e).map_or(0, |x| x.description);
                self.core.select_effects.push(Some(e));
                self.core.select_options.push(d);
            }
            if self.core.select_options.is_empty() {
                // Nothing is available: not the ordinary set, and no
                // procedure. The reference reaches `SelectOption` with an
                // empty list here, which answers `-1`, and then indexes
                // `select_effects` with it — reading out of bounds. This
                // ends the unit instead, which is the only defensible
                // reading: a set with no way to perform it does not happen.
                return true;
            }
            if self.core.select_options.len() == 1 {
                self.core.returns.set(0);
            } else {
                self.emplace(Kind::SelectOption { player: setplayer });
            }
        }
        self.cards[target].material_cards.clear();
        false
    }

    /// Step 1: which extra-summon permission, if one is needed?
    ///
    /// The `None` slot here means "no extra permission needed" — offered
    /// only when the player actually has a normal summon left. Each
    /// `EFFECT_EXTRA_SET_COUNT` is then *validated* before being offered:
    /// a permission that would not in fact allow this set is not shown.
    fn mset_step_1(
        &mut self,
        setplayer: u8,
        target: CardId,
        ignore_count: bool,
        min_tribute: u8,
        zone: u32,
        state: &mut MonsterSetState,
    ) -> bool {
        if !state.decided {
            let idx = self.core.returns.get().max(0) as usize;
            state.procedure = self.core.select_effects.get(idx).copied().flatten();
            state.decided = true;
        }
        let candidates = self.filter_effect(target, code::EXTRA_SET_COUNT);
        self.core.select_effects.clear();
        self.core.select_options.clear();

        let limit = self.get_summon_count_limit(setplayer);
        if ignore_count || (self.core.summon_count[setplayer as usize] as i32) < limit {
            self.core.select_effects.push(None);
            self.core.select_options.push(1);
        }
        if !ignore_count && !self.core.extra_summon[setplayer as usize] {
            for e in candidates {
                if self.extra_set_count_allows(
                    target,
                    e,
                    state.procedure,
                    setplayer,
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
            self.emplace(Kind::SelectOption { player: setplayer });
        }
        false
    }

    /// Would this extra-count permission actually allow the set?
    ///
    /// The two branches are the two kinds of set: with a procedure it is the
    /// procedure's own condition that decides; without one it is the
    /// ordinary tribute arithmetic. Note the minimum is raised to whichever
    /// of the two is larger — the permission may demand more tributes than
    /// the monster does.
    fn extra_set_count_allows(
        &mut self,
        target: CardId,
        permission: EffectId,
        procedure: Option<EffectId>,
        setplayer: u8,
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
                let (mut min, mut max) = self.get_set_tribute_count(target);
                if !self.is_player_can_mset(
                    summon_type::ADVANCE,
                    setplayer,
                    Some(target),
                    setplayer,
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
                    u32::from(position::FACEDOWN_DEFENSE),
                )
            }
        }
    }
}

impl Field {
    /// Step 2: apply the chosen permission, then find the tributes.
    ///
    /// The permission's parameters **narrow** what follows: its minimum
    /// raises the unit's, and its zone mask is intersected with the unit's.
    /// Neither replaces — a permission cannot widen what the summon already
    /// allows.
    ///
    /// With a procedure, that is all step 2 does; the procedure finds its own
    /// tributes. Without one, the ordinary arithmetic runs and the question
    /// takes one of two shapes:
    ///
    /// - **no tributes required, and room on the field** — ask *whether* to
    ///   tribute at all, because tributing anyway is legal and sometimes
    ///   wanted;
    /// - **otherwise** — ask *which*, with a minimum raised to at least the
    ///   number the zone limit demands.
    ///
    /// That `-fcount + 1` floor is the same "the player is over the limit"
    /// arithmetic `check_tribute` uses: when the Monster Zone is
    /// over-subscribed the summon needs enough tributes to make room before
    /// it is even possible.
    fn mset_step_2(
        &mut self,
        setplayer: u8,
        target: CardId,
        min_tribute: &mut u8,
        zone: &mut u32,
        state: &mut MonsterSetState,
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
            // The procedure finds its own tributes.
            self.set_step(3);
            return false;
        }

        self.core.select_cards.clear();
        let (mut min, max) = self.get_set_tribute_count(target);
        min = min.max(i32::from(*min_tribute));
        let advance =
            self.is_player_can_mset(summon_type::ADVANCE, setplayer, Some(target), setplayer);
        if max == 0 || !advance {
            self.core.return_cards.clear();
            self.set_step(3);
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
            u32::from(position::FACEDOWN_DEFENSE),
        );
        if rcount == 0 {
            self.core.return_cards.clear();
            self.set_step(3);
            return false;
        }
        self.core.release_cards = lists.release.into_iter().collect();
        self.core.release_cards_ex = lists.extra.into_iter().collect();
        self.core.release_cards_ex_oneof = lists.extra_one_of.into_iter().collect();

        let room = self.get_tofield_count(
            Some(target),
            setplayer,
            location::MZONE,
            setplayer,
            Field::LOCATION_REASON_TOFIELD,
            *zone,
        );
        let fcount = self.get_mzone_limit(setplayer, setplayer, Field::LOCATION_REASON_TOFIELD);
        if min == 0 && room > 0 && fcount > 0 {
            // Tributing is allowed but not required: ask whether to.
            self.emplace(Kind::SelectYesNo {
                player: setplayer,
                description: 90,
            });
            state.max_allowed_tributes = max as u8;
        } else {
            // Over the zone limit: enough tributes to make room, at least.
            if min < -fcount + 1 {
                min = -fcount + 1;
            }
            let cancelable = self.core.summon_cancelable;
            self.select_tribute_cards(
                target, setplayer, cancelable, min as u8, max as u8, setplayer, *zone,
            );
            self.set_step(3);
        }
        false
    }

    /// The arguments a summon procedure's target and operation are called
    /// with: `(min_tribute, zone, releasable, pextra)` after the card.
    ///
    /// `releasable` comes from the extra-summon permission when there is
    /// one, and defaults to `0xff00ff` — every seat on both sides — when
    /// there is not. The reference recomputes it at each of the two call
    /// sites rather than carrying it, and only the third member of the
    /// permission's tuple is read here.
    pub(crate) fn procedure_args(
        &mut self,
        target: CardId,
        min_tribute: u8,
        zone: u32,
        extra: Option<EffectId>,
    ) -> Vec<i64> {
        let releasable = match extra {
            Some(p) => self.extra_count_params_pub(target, p, 0x1f001f).2,
            None => 0xff00ff,
        };
        vec![
            i64::from(min_tribute),
            i64::from(zone),
            i64::from(releasable),
            extra.map_or(-1, |e| e as i64),
        ]
    }

    /// Step 4: take the tributes, and pay the set's own cost.
    ///
    /// A **cancelled** tribute selection ends the whole set — the player
    /// declined, and there is nothing to fall back to.
    ///
    /// `EFFECT_MSET_COST` is run here rather than later because it is a cost
    /// of *setting*, not of the monster arriving: it is paid whether or not
    /// anything else about the set works out.
    fn mset_step_4(&mut self, setplayer: u8, target: CardId, state: &mut MonsterSetState) -> bool {
        if state.procedure.is_some() {
            self.set_step(5);
        } else {
            if self.core.return_cards.canceled {
                return true;
            }
            if !self.core.return_cards.list.is_empty() {
                state.tributes = self.core.return_cards.list.iter().copied().collect();
            }
        }
        for e in self.filter_effect(target, code::MSET_COST) {
            if self.effects.get(e).is_some_and(|x| x.operation.is_some()) {
                self.core
                    .sub_solving_event
                    .push_back(crate::event::Event::new(0));
                self.emplace(Kind::ExecuteOperation {
                    resume: None,
                    effect: e,
                    player: setplayer,
                    subject: None,
                    args: Vec::new(),
                    was_disabled: false,
                });
            }
        }
        false
    }

    /// Step 5: the ordinary set — release the tributes and record the summon.
    ///
    /// `summon.type` starts as `NORMAL` and **gains** `ADVANCE` when there
    /// are tributes, rather than being one or the other. A tribute set is a
    /// normal set, and every rule asking "was this normal summoned" must see
    /// both.
    ///
    /// The tributes are released with `REASON_SUMMON | REASON_MATERIAL`,
    /// which is what exempts them from the "may this effect touch you" test
    /// in `SendTo` — a tribute is a price, not an effect applied to them.
    fn mset_step_5(&mut self, setplayer: u8, target: CardId, state: &mut MonsterSetState) -> bool {
        self.cards[target].summon.location = location::HAND;
        self.cards[target].summon.pzone = false;
        self.cards[target].summon.type_ = summon_type::NORMAL;

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
                setplayer,
            );
            self.cards[target].summon.type_ |= summon_type::ADVANCE;
            self.adjust_all();
        }
        self.cards[target].summon.player = setplayer;
        self.cards[target].reason_effect = None;
        self.cards[target].reason_player = setplayer;
        self.set_step(7);
        false
    }

    /// Step 6: the procedure's `target` — its chance to choose and declare.
    ///
    /// `returns` is primed to **true** first, so a procedure with no target
    /// is treated as having agreed. Only a target that runs can refuse.
    fn mset_step_6(
        &mut self,
        setplayer: u8,
        target: CardId,
        min_tribute: u8,
        zone: u32,
        state: &mut MonsterSetState,
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
                player: setplayer,
                subject: Some(target),
                args,
                was_disabled: false,
            });
        }
        false
    }

    /// Step 7: the procedure refused, or it did not — then run its operation.
    ///
    /// The procedure's own value supplies the summon type, masked to
    /// `0xfffffff` and **ORed with `NORMAL`**: a procedure sets a monster,
    /// however exotic its route, so the normal-set rules still see it.
    ///
    /// `dec_count` is called on the procedure whatever its operation does —
    /// using it is what spends it.
    fn mset_step_7(
        &mut self,
        setplayer: u8,
        target: CardId,
        min_tribute: u8,
        zone: u32,
        state: &mut MonsterSetState,
    ) -> bool {
        if self.core.returns.get() == 0 {
            return true;
        }
        let Some(procedure) = state.procedure else {
            return false;
        };
        let value = self.effect_value_for_card_pub(procedure, target) as u32;
        self.cards[target].summon.type_ = (value & 0xfff_ffff) | summon_type::NORMAL;
        self.cards[target].summon.location = location::HAND;
        self.cards[target].summon.pzone = false;
        self.cards[target].summon.player = setplayer;
        self.cards[target].reason_effect = Some(procedure);
        self.cards[target].reason_player = setplayer;

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
                player: setplayer,
                subject: Some(target),
                args,
                was_disabled: false,
            });
        }
        self.dec_count(procedure, setplayer);
        false
    }

    /// Step 8: pay for the set — a normal summon, or the extra permission.
    ///
    /// The two are exclusive: either the player's normal summon is spent, or
    /// their one extra is. `ignore_count` spends neither, which is what makes
    /// a set that "does not count as your normal summon" possible.
    ///
    /// `break_effect` comes first, and unconditionally: setting a monster is
    /// an action that ends the current chain's window whether or not it costs
    /// anything.
    fn mset_step_8(
        &mut self,
        setplayer: u8,
        target: CardId,
        ignore_count: bool,
        state: &mut MonsterSetState,
    ) -> bool {
        // `break_effect(true)`: the reference's default argument, which
        // also clears `just_sent_cards`.
        self.break_effect(true);
        if ignore_count {
            return false;
        }
        match state.extra_summon {
            None => self.core.summon_count[setplayer as usize] += 1,
            Some(p) => {
                self.core.extra_summon[setplayer as usize] = true;
                let code_ = self
                    .effects
                    .get(p)
                    .and_then(|e| e.get_handler(&self.cards))
                    .map_or(0, |h| self.cards[h].data.code);
                self.messages.push(Message::Hint {
                    kind: crate::host_question::hint::CARD,
                    player: 0,
                    value: u64::from(code_),
                });
                if self.effects.get(p).is_some_and(|e| e.operation.is_some()) {
                    self.core
                        .sub_solving_event
                        .push_back(crate::event::Event::new(0));
                    self.emplace(Kind::ExecuteOperation {
                        resume: None,
                        effect: p,
                        player: setplayer,
                        subject: Some(target),
                        args: Vec::new(),
                        was_disabled: false,
                    });
                }
            }
        }
        false
    }

    /// Step 9: put the card down.
    ///
    /// Face-down defence, unless the procedure named a position — and a
    /// procedure's `s_range` is read as a **position mask** here, narrowed to
    /// the face-down ones. That reuse of `s_range` is the reference's, and it
    /// only applies under `EFFECT_FLAG_SPSUM_PARAM`.
    ///
    /// The field effects are disabled **before** the move: the card is about
    /// to be face-down, and a face-down card applies nothing.
    fn mset_step_9(
        &mut self,
        setplayer: u8,
        target: CardId,
        zone: u32,
        state: &mut MonsterSetState,
    ) -> bool {
        let mut targetplayer = setplayer;
        let mut positions = position::FACEDOWN_DEFENSE;
        if let Some(p) = state.procedure {
            let spsum = self
                .effects
                .get(p)
                .is_some_and(|e| e.is_flag(crate::effect::flag::SPSUM_PARAM));
            if spsum {
                let s_range = self.effects.get(p).map_or(0, |e| e.s_range) as u8;
                positions = s_range & position::FACEDOWN;
                if self.effects.get(p).is_some_and(|e| e.o_range != 0) {
                    targetplayer = 1 - setplayer;
                }
            }
        }
        self.enable_field_effect(target, false);
        self.move_to_field(
            target,
            setplayer,
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
        false
    }

    /// Step 10: record that it happened.
    ///
    /// `set_control` is called with the card's **own** controller — not to
    /// change anything, but because control has to be *established* by an
    /// effect for it to be something later effects can see and undo.
    ///
    /// The `PointEvent` at the end is emplaced **only outside a chain**: a
    /// set during a chain's resolution does not open a response window of its
    /// own, because the chain already has one.
    fn mset_step_10(&mut self, setplayer: u8, target: CardId, state: &mut MonsterSetState) -> bool {
        let controller = self.cards[target].current.controller;
        self.set_control(target, controller, 0, 0);
        self.core.phase_action = true;
        self.core.normalsummon_state_count[setplayer as usize] += 1;
        self.check_card_counter(
            target,
            crate::summon_support::activity::NORMALSUMMON,
            setplayer,
        );
        self.cards[target].set_status(status::SUMMON_TURN, true);

        let info = self.get_info_location(target);
        let code_ = self.cards[target].data.code;
        self.messages.push(Message::Set {
            code: code_,
            controller: info.controller,
            location: info.location,
            sequence: info.sequence,
            position: info.position,
        });
        self.adjust_instant();
        self.raise_event(
            Some(target),
            code::MSET,
            state.procedure,
            0,
            setplayer,
            setplayer,
            0,
        );
        self.process_instant_event();
        if self.core.current_chain.is_empty() {
            self.adjust_all();
            self.core.hint_timing[setplayer as usize] |= timing::MSET;
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
    use crate::card::{Card, CardData};
    use crate::processor::Status;

    fn in_hand(f: &mut Field, level: u32) -> CardId {
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
        let id = f.new_card(c);
        f.add_card(0, id, location::HAND, 0, false);
        id
    }

    /// Run until the machine yields, finishes, **or reaches a unit that is
    /// not ported**.
    ///
    /// `MonsterSet`'s last step calls `adjust_all`, which emplaces `Adjust`
    /// — still an `unimplemented!`. So a completed set ends at that
    /// boundary rather than at an empty queue, and the tests assert the
    /// state the set left behind. When `Adjust` lands these become ordinary
    /// run-to-completion tests with no change to what they assert.
    fn run(f: &mut Field) -> Status {
        for _ in 0..512 {
            if f.queue()
                .next()
                .is_some_and(|u| matches!(u.kind, Kind::Adjust))
                || f.core
                    .subunits
                    .iter()
                    .any(|u| matches!(u.kind, Kind::Adjust))
            {
                return Status::End;
            }
            match f.process() {
                Status::Continue => continue,
                other => return other,
            }
        }
        panic!("did not settle");
    }

    /// Run, answering the seat question `MoveToField` asks at the end.
    ///
    /// With five free Monster Zones there is a real choice, so the set
    /// yields — a test that expects it to run straight through is asserting
    /// that the field is full.
    fn run_placing(f: &mut Field, seat: i8) -> Status {
        let st = run(f);
        if st == Status::Awaiting
            && f.messages
                .iter()
                .any(|m| matches!(m, Message::SelectPlace { .. }))
        {
            f.core.returns.set_i8(0, 0);
            f.core.returns.set_i8(1, location::MZONE as i8);
            f.core.returns.set_i8(2, seat);
            return run(f);
        }
        st
    }

    fn queued(f: &Field) -> bool {
        !f.core.subunits.is_empty() || f.queue().next().is_some()
    }

    /// A low-level monster is set without any question being asked.
    #[test]
    fn a_low_level_monster_is_set_outright() {
        let mut f = Field::new(8000);
        let c = in_hand(&mut f, 4);
        f.mset(0, c, None, false, 0, 0x1f);
        assert_eq!(run_placing(&mut f, 2), Status::End);
        assert_eq!(f.cards[c].current.location, location::MZONE);
        assert_eq!(f.cards[c].current.sequence, 2, "the seat that was chosen");
        assert_eq!(f.cards[c].current.position, position::FACEDOWN_DEFENSE);
        assert!(f.cards[c].is_status(status::SUMMON_TURN));
    }

    /// **The seat is confirmed even when the caller named exactly one.**
    ///
    /// `move_to_field`'s `confirm` argument defaults to *true* in the
    /// reference and a set takes the default; passing `false` would place
    /// the card without asking whenever the zone mask has a single bit.
    #[test]
    fn the_seat_is_confirmed_even_when_only_one_was_asked_for() {
        let mut f = Field::new(8000);
        let c = in_hand(&mut f, 4);
        f.mset(0, c, None, false, 0, 1 << 3);
        assert_eq!(run_placing(&mut f, 3), Status::End);
        assert_eq!(f.cards[c].current.sequence, 3);
        assert!(
            f.messages
                .iter()
                .any(|m| matches!(m, Message::SelectPlace { .. })),
            "the one named seat is still confirmed"
        );
    }

    /// It costs the player their normal summon.
    #[test]
    fn setting_spends_the_normal_summon() {
        let mut f = Field::new(8000);
        let c = in_hand(&mut f, 4);
        f.mset(0, c, None, false, 0, 0x1f);
        run_placing(&mut f, 0);
        assert_eq!(f.core.summon_count[0], 1);
        assert_eq!(f.core.summon_count[1], 0, "and not the opponent's");
    }

    /// `ignore_count` spends neither the normal summon nor the extra.
    #[test]
    fn ignore_count_spends_nothing() {
        let mut f = Field::new(8000);
        let c = in_hand(&mut f, 4);
        f.mset(0, c, None, true, 0, 0x1f);
        run_placing(&mut f, 0);
        assert_eq!(f.core.summon_count[0], 0);
        assert!(!f.core.extra_summon[0]);
    }

    /// The summon is recorded as `NORMAL`, from the hand.
    #[test]
    fn the_summon_is_recorded_as_normal_from_the_hand() {
        let mut f = Field::new(8000);
        let c = in_hand(&mut f, 4);
        f.mset(0, c, None, false, 0, 0x1f);
        run_placing(&mut f, 0);
        assert_eq!(f.cards[c].summon.type_, summon_type::NORMAL);
        assert_eq!(f.cards[c].summon.location, location::HAND);
        assert_eq!(f.cards[c].summon.player, 0);
    }

    /// `EVENT_MSET` is raised, and the timing charged.
    #[test]
    fn setting_raises_its_event() {
        let mut f = Field::new(8000);
        let c = in_hand(&mut f, 4);
        f.mset(0, c, None, false, 0, 0x1f);
        run_placing(&mut f, 0);
        assert!(f
            .core
            .instant_event
            .iter()
            .any(|e| e.event_code == code::MSET));
        assert_ne!(f.core.hint_timing[0] & timing::MSET, 0);
        assert!(f.core.phase_action);
    }

    /// The card's field effects are turned **off** before it is placed.
    ///
    /// It is about to be face-down, and a face-down card applies nothing.
    /// Observable as `STATUS_EFFECT_ENABLED` being cleared — the card starts
    /// the test with it set, so this is a change rather than a default.
    #[test]
    fn field_effects_are_disabled_before_the_card_is_placed() {
        let mut f = Field::new(8000);
        let c = in_hand(&mut f, 4);
        assert!(
            f.cards[c].is_status(status::EFFECT_ENABLED),
            "set up enabled, so the clearing is visible"
        );
        f.mset(0, c, None, false, 0, 0x1f);
        run_placing(&mut f, 0);
        assert!(
            !f.cards[c].is_status(status::EFFECT_ENABLED),
            "a face-down card applies nothing"
        );
    }

    mod refusals {
        use super::*;

        /// **Only from the hand.** A monster already on the field cannot be
        /// set by this machine.
        #[test]
        fn a_monster_not_in_the_hand_is_refused() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f, 4);
            // `add_card` refuses a card that is already placed, so the move
            // has to be made properly or the card stays in the hand and the
            // test proves nothing.
            f.remove_card(c);
            f.add_card(0, c, location::MZONE, 0, false);
            assert_eq!(f.cards[c].current.location, location::MZONE);
            f.mset(0, c, None, false, 0, 0x1f);
            run(&mut f);
            assert_eq!(f.core.summon_count[0], 0, "nothing happened");
            assert!(!f.cards[c].is_status(status::SUMMON_TURN));
        }

        /// Refusals are asserted through `summon_count`, not through the
        /// card's location.
        ///
        /// The driver stops at the seat question, which is *before* the card
        /// moves — so "still in the hand" is true of a **successful** set at
        /// that moment too, and a test asserting it passes either way. What
        /// separates them is that a refused set never reaches step 8 and so
        /// never spends the summon.
        #[test]
        fn a_spell_is_refused() {
            let mut f = Field::new(8000);
            let mut c = Card::with_data(
                CardData {
                    code: 55144522,
                    type_: card_type::SPELL,
                    ..Default::default()
                },
                0,
            );
            c.current.controller = 0;
            c.set_status(status::EFFECT_ENABLED, true);
            let id = f.new_card(c);
            f.add_card(0, id, location::HAND, 0, false);

            f.mset(0, id, None, false, 0, 0x1f);
            run(&mut f);
            assert_eq!(f.core.summon_count[0], 0, "nothing was spent");
            assert_eq!(f.cards[id].current.location, location::HAND);
        }

        /// `EFFECT_UNSUMMONABLE_CARD` and `EFFECT_CANNOT_MSET` each refuse.
        #[test]
        fn the_two_prohibitions_each_refuse() {
            for code_ in [code::UNSUMMONABLE_CARD, code::CANNOT_MSET] {
                let mut f = Field::new(8000);
                let c = in_hand(&mut f, 4);
                let mut e = crate::effect::Effect::new(crate::effect::effect_type::SINGLE, code_);
                e.owner = Some(c);
                e.handler = Some(c);
                let id = f.new_effect(e);
                f.cards[c].single_effect.insert(code_, id);
                f.cards[c].indexer.insert(id);

                f.mset(0, c, None, false, 0, 0x1f);
                run(&mut f);
                assert_eq!(f.core.summon_count[0], 0, "code {code_}");
                assert_eq!(f.cards[c].current.location, location::HAND);
            }
        }

        /// Nothing is even queued for a card that is not a monster in a
        /// hand — the refusal is inside the machine, so a unit *is* queued,
        /// but it ends without acting.
        #[test]
        fn a_refused_set_still_queues_a_unit() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f, 4);
            f.remove_card(c);
            f.add_card(0, c, location::GRAVE, 0, false);
            f.mset(0, c, None, false, 0, 0x1f);
            assert!(queued(&f), "the check is the machine's, not the caller's");
            run(&mut f);
            assert_eq!(f.core.summon_count[0], 0);
        }
    }

    mod tributes {
        use super::*;

        /// A level-7 monster needs two tributes; with none available the set
        /// does not happen.
        #[test]
        fn a_high_level_monster_without_tributes_is_not_set() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f, 7);
            f.mset(0, c, None, false, 0, 0x1f);
            run(&mut f);
            assert_eq!(f.cards[c].current.location, location::HAND);
            assert_eq!(f.core.summon_count[0], 0);
        }

        /// With tributes on the field, the set goes ahead and records
        /// `ADVANCE` **alongside** `NORMAL` — a tribute set is a normal set.
        #[test]
        fn a_tribute_set_records_both_summon_types() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f, 7);
            let t1 = in_hand(&mut f, 4);
            let t2 = in_hand(&mut f, 4);
            for (i, t) in [t1, t2].into_iter().enumerate() {
                // `add_card` refuses a card that is already placed, and does
                // so *silently* — without the removal the tributes stay in
                // the hand and the test asserts nothing.
                f.remove_card(t);
                f.add_card(0, t, location::MZONE, i as u32, false);
                f.cards[t].current.position = position::FACEUP_ATTACK;
                assert_eq!(f.cards[t].current.location, location::MZONE);
            }

            f.mset(0, c, None, false, 0, 0x1f);
            assert_eq!(run(&mut f), Status::Awaiting, "which tributes?");

            // Take both offered tributes.
            let offered = f.core.select_cards.clone();
            assert!(!offered.is_empty(), "the tributes are on offer");
            f.core.returns.set_i32(0, 0);
            f.core.returns.set_i32(1, 2);
            f.core.returns.set_i32(2, 0);
            f.core.returns.set_i32(3, 1);
            run(&mut f);

            // `|= ADVANCE` and `= ADVANCE` cannot be told apart here, and
            // that is the point rather than a gap: ADVANCE *contains*
            // NORMAL (0x11000000 vs 0x10000000), so the OR is a no-op on a
            // value that is already NORMAL. What the assertion pins is that
            // both bits are present, which is what every "was this normal
            // summoned" rule reads.
            assert_eq!(
                f.cards[c].summon.type_,
                summon_type::NORMAL | summon_type::ADVANCE,
                "a tribute set is still a normal set"
            );
            assert_eq!(
                f.cards[c].material_cards,
                std::collections::BTreeSet::from([t1, t2]),
                "and the tributes are recorded as its materials"
            );
        }
    }
}

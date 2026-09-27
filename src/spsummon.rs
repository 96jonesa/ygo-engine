//! The effect-driven Special Summon: `special_summon`, `SpSummonStep` and
//! `SpSummon`.
//!
//! This is the Special Summon a *card effect* performs — `Duel.SpecialSummon`
//! — and it is a different machine from `SpSummonRule`, which is the summon a
//! monster performs on itself by its own printed procedure. Nearly every
//! Special Summon in a duel comes through here.
//!
//! ## A batch, not a card
//!
//! The unit that does the work is `SpSummonStep`, and it handles **one**
//! card. `SpSummon` wraps a group of them: it emplaces one step per card,
//! then does everything that has to happen once — the counters, the
//! `MSG_SPSUMMONED`, the success events — over whatever survived.
//!
//! A card can drop out at `SpSummonStep`, and that is the normal case rather
//! than an error: the revive limit, a `EFFECT_SPSUMMON_CONDITION` that
//! refuses, or simply no room on the field. **The group is the list of
//! survivors**, and `SpSummon` reads it after the steps have run.
//!
//! ## The turn player's cards go first
//!
//! `SpSummon`'s step 0 splits the group by `summon.player` and emplaces the
//! turn player's cards before the opponent's, each half sorted by
//! `card_operation_sort`. Both halves charge `TIMING_SPSUMMON` for their own
//! player. The split is what makes a simultaneous summon of both players'
//! cards deterministic.
//!
//! ## `spsummon_param`
//!
//! Four arguments — the destination player, `nocheck`, `nolimit` and the
//! position mask — are packed onto the *card* rather than carried on the
//! unit, because the unit is entered once per card from a batch. The entry
//! functions pack them; `SpSummonStep` unpacks them.

use crate::board::{location, position};
use crate::card::{card_type, reason, status, summon_type};
use crate::event::{code, CardId, EffectId, PLAYER_NONE};
use crate::field::{timing, Field, GroupId, Message};
use crate::processor::Kind;
use std::collections::BTreeSet;

/// The four arguments `special_summon` parks on the card.
///
/// `nocheck` and `nolimit` are not the same waiver and are easy to swap:
/// **`nolimit` waives the revive limit** (summoning from the graveyard or
/// banishment a card that was never properly summoned), while **`nocheck`
/// waives the summon conditions** — `EFFECT_SPSUMMON_CONDITION` and the
/// monster-type test. A summon can want either without the other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpSummonParam {
    pub playerid: u8,
    pub nocheck: bool,
    pub nolimit: bool,
    pub positions: u8,
}

impl SpSummonParam {
    pub fn pack(self) -> u32 {
        (u32::from(self.playerid) << 24)
            + (u32::from(self.nocheck) << 16)
            + (u32::from(self.nolimit) << 8)
            + u32::from(self.positions)
    }

    pub fn unpack(raw: u32) -> Self {
        Self {
            playerid: ((raw >> 24) & 0xf) as u8,
            nocheck: (raw >> 16) & 0xff != 0,
            nolimit: (raw >> 8) & 0xff != 0,
            positions: (raw & 0xff) as u8,
        }
    }
}

impl Field {
    /// `field::special_summon` — Special Summon a group of cards.
    ///
    /// Everything here happens **before** any card moves: the position mask
    /// is resolved once for the whole group, then each card records where it
    /// is coming from and what it is about to become.
    ///
    /// `temp.reason*` is a saved copy, not a cache. A card that drops out at
    /// `SpSummonStep` has its reason put back from it, which is why the copy
    /// is taken for every card and not only the ones that succeed.
    #[allow(clippy::too_many_arguments)]
    pub fn special_summon(
        &mut self,
        targets: BTreeSet<CardId>,
        sumtype: u32,
        sumplayer: u8,
        playerid: u8,
        nocheck: bool,
        nolimit: bool,
        positions: u8,
        zone: u32,
    ) {
        let positions = self.fold_facedown_under_devine_light(sumplayer, positions);
        let forcing = self.filter_player_effect(sumplayer, code::FORCE_SPSUMMON_POSITION);
        for &card in &targets {
            let pos = self.prepare_spsummon(card, sumtype, sumplayer, &forcing, positions);
            self.cards[card].spsummon_param = SpSummonParam {
                playerid,
                nocheck,
                nolimit,
                positions: pos,
            }
            .pack();
        }
        let group = self.new_group(targets);
        let (reason_effect, reason_player) = (self.core.reason_effect, self.core.reason_player);
        self.emplace(Kind::SpSummon {
            reason_effect,
            reason_player,
            targets: group,
            zone,
        });
    }

    /// `field::special_summon_step` — one card, summoned outside a batch.
    ///
    /// Identical to the loop body above but for **one extra line**: a
    /// face-up summon of a card whose name is already face-up on the field
    /// has `POS_FACEUP` struck out of its mask, so it arrives face-down or
    /// not at all. The batch form does not do this, and the asymmetry is the
    /// reference's.
    #[allow(clippy::too_many_arguments)]
    pub fn special_summon_step(
        &mut self,
        target: CardId,
        sumtype: u32,
        sumplayer: u8,
        playerid: u8,
        nocheck: bool,
        nolimit: bool,
        positions: u8,
        zone: u32,
    ) {
        let mut positions = self.fold_facedown_under_devine_light(sumplayer, positions);
        if positions & position::FACEUP != 0
            && self
                .check_unique_onfield(target, playerid, u16::from(location::MZONE), None)
                .is_some()
        {
            positions &= !position::FACEUP;
        }
        let forcing = self.filter_player_effect(sumplayer, code::FORCE_SPSUMMON_POSITION);
        let pos = self.prepare_spsummon(target, sumtype, sumplayer, &forcing, positions);
        self.cards[target].spsummon_param = SpSummonParam {
            playerid,
            nocheck,
            nolimit,
            positions: pos,
        }
        .pack();
        self.emplace(Kind::SpSummonStep {
            targets: None,
            target,
            zone,
            state: Default::default(),
        });
    }

    /// `field::special_summon_complete` — finish the cards that
    /// `special_summon_step` put onto the field one at a time.
    ///
    /// Enters `SpSummon` at **step 1**, skipping the step that emplaces the
    /// per-card work: it has already been done.
    pub fn special_summon_complete(&mut self, reason_effect: Option<EffectId>, reason_player: u8) {
        let cards = std::mem::take(&mut self.core.special_summoning);
        let group = self.new_group(cards);
        self.emplace_at(
            Kind::SpSummon {
                reason_effect,
                reason_player,
                targets: group,
                zone: 0,
            },
            1,
        );
    }

    /// `EFFECT_DEVINE_LIGHT` folds each face-down position into the same
    /// position, face-up. See the note in `spsummon_permission`.
    fn fold_facedown_under_devine_light(&mut self, sumplayer: u8, positions: u8) -> u8 {
        if positions & position::FACEDOWN != 0
            && self
                .is_player_affected_by_effect(sumplayer, code::DEVINE_LIGHT)
                .is_some()
        {
            return (positions & position::FACEUP) | ((positions & position::FACEDOWN) >> 1);
        }
        positions
    }

    /// Record where a card is coming from and what it is about to become,
    /// and narrow its position by the forcing effects.
    fn prepare_spsummon(
        &mut self,
        card: CardId,
        sumtype: u32,
        sumplayer: u8,
        forcing: &[EffectId],
        positions: u8,
    ) -> u8 {
        let (reason_effect, reason_player) = (self.core.reason_effect, self.core.reason_player);
        let c = &mut self.cards[card];
        c.temp.reason = c.reason;
        c.temp.reason_effect = c.reason_effect;
        c.temp.reason_player = c.reason_player;
        c.summon.type_ = (sumtype & 0xf00_ffff) | summon_type::SPECIAL;
        c.summon.location = c.current.location;
        c.summon.sequence = c.current.sequence as u8;
        c.summon.pzone = c.current.pzone;
        c.summon.player = sumplayer;
        c.reason = reason::SPSUMMON;
        c.reason_effect = reason_effect;
        c.reason_player = reason_player;

        let sumtype_now = self.cards[card].summon.type_;
        let mut pos = positions;
        for &e in forcing {
            if let Some(check) = self.effects.get(e).and_then(|x| x.target_filter) {
                let args = [
                    i64::from(reason_player),
                    i64::from(sumtype_now),
                    i64::from(pos),
                    i64::from(self.cards[card].current.controller),
                    reason_effect.map_or(-1, |x| x as i64),
                ];
                if !check(self, e, Some(card), &args) {
                    continue;
                }
            }
            pos &= self.effect_plain_value(e) as u8;
        }
        pos
    }
}

/// The state `SpSummonStep` carries.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SpSummonStepState {
    pub cost_effects: Vec<EffectId>,
}

impl Field {
    /// One step of `SpSummonStep` — **one card**.
    ///
    /// Six cases and two exits. Case 3 is the success exit; case 5 is the
    /// refusal, reached by `arg.step = 4` from any of the four checks in
    /// case 0, and it is the one that puts the card's reason back.
    pub(crate) fn sp_summon_step_step(
        &mut self,
        step: u16,
        targets: Option<GroupId>,
        target: CardId,
        zone: u32,
        state: &mut SpSummonStepState,
    ) -> bool {
        let param = SpSummonParam::unpack(self.cards[target].spsummon_param);
        match step {
            0 => self.sp_step_0(targets, target, zone, param, state),
            1 => self.sp_step_1(targets, target, zone, param, state),
            2 => {
                self.announce_spsummoning(target);
                false
            }
            3 => {
                self.core.returns.set(1);
                let controller = self.cards[target].current.controller;
                self.set_control(target, controller, 0, 0);
                self.cards[target].set_status(status::SPSUMMON_STEP, true);
                true
            }
            5 => {
                self.core.returns.set(0);
                let c = &mut self.cards[target];
                c.reason = c.temp.reason;
                c.reason_effect = c.temp.reason_effect;
                c.reason_player = c.temp.reason_player;
                if let Some(group) = targets {
                    self.group_mut(group).remove(&target);
                }
                true
            }
            _ => true,
        }
    }

    /// Step 0: the four ways a card drops out, then its cost.
    ///
    /// **The revive limit is the subtle one.** A card with
    /// `EFFECT_REVIVE_LIMIT` that has never been properly summoned may not
    /// be Special Summoned from the graveyard or banishment (`0x38`) at all,
    /// and may not be summoned from the hand or deck (`0x3`) unless the
    /// summon waives the *conditions* as well. So `nolimit` alone opens both
    /// and `nocheck` alone opens only the second — which is why they are two
    /// waivers rather than one.
    ///
    /// **Running out of room sends the card to the graveyard.** Not
    /// silently: a card that was about to be summoned and has nowhere to go
    /// is put on `ss_tograve_set`, which `SpSummon` empties at its step 1.
    /// A card already in the graveyard is left there.
    fn sp_step_0(
        &mut self,
        _targets: Option<GroupId>,
        target: CardId,
        zone: u32,
        param: SpSummonParam,
        state: &mut SpSummonStepState,
    ) -> bool {
        let SpSummonParam {
            playerid,
            nocheck,
            nolimit,
            positions,
        } = param;

        if self
            .is_affected_by_effect(target, code::REVIVE_LIMIT)
            .is_some()
            && !self.cards[target].is_status(status::PROC_COMPLETE)
        {
            let loc = u16::from(self.cards[target].current.location);
            let from_grave_or_banished = loc & 0x38 != 0;
            let from_hand_or_deck = loc & 0x3 != 0;
            if (!nolimit && from_grave_or_banished) || (!nocheck && !nolimit && from_hand_or_deck) {
                self.set_step(4);
                return false;
            }
        }

        let sumtype = self.cards[target].summon.type_ & 0xff00_ffff;
        let summon_player = self.cards[target].summon.player;
        let refused = self.cards[target].current.location == location::MZONE
            || (positions & position::FACEDOWN == 0
                && self
                    .check_unique_onfield(target, playerid, u16::from(location::MZONE), None)
                    .is_some())
            || !self.is_player_can_spsummon(
                self.core.reason_effect,
                sumtype,
                positions,
                summon_player,
                playerid,
                target,
                None,
            )
            || (!nocheck && self.cards[target].data.type_ & card_type::MONSTER == 0);
        if refused {
            self.set_step(4);
            return false;
        }

        let room = self.get_useable_count(
            Some(target),
            playerid,
            location::MZONE,
            summon_player,
            Field::LOCATION_REASON_TOFIELD,
            zone,
        );
        if room <= 0 {
            if self.cards[target].current.location != location::GRAVE {
                self.core.ss_tograve_set.insert(target);
            }
            self.set_step(4);
            return false;
        }

        if !nocheck {
            for e in self.filter_effect(target, code::SPSUMMON_CONDITION) {
                let args = [
                    self.core.reason_effect.map_or(-1, |x| x as i64),
                    i64::from(summon_player),
                    i64::from(sumtype),
                    i64::from(positions),
                    i64::from(playerid),
                ];
                if !self.value_condition_holds(e, target, &args) {
                    self.set_step(4);
                    return false;
                }
            }
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
                    player: summon_player,
                    subject: Some(target),
                    args: Vec::new(),
                    was_disabled: false,
                });
            }
        }
        false
    }

    /// Step 1: put the card onto the field.
    ///
    /// **A card summoned outside a batch registers itself** on
    /// `special_summoning`, which is what `special_summon_complete` later
    /// collects. A card in a batch does not: its group already holds it.
    ///
    /// The Extra Monster Zone arithmetic is skipped entirely without
    /// `DUEL_EMZONE`, which this project masks off — it is the only part of
    /// this machine the reference configuration never runs.
    fn sp_step_1(
        &mut self,
        targets: Option<GroupId>,
        target: CardId,
        zone: u32,
        param: SpSummonParam,
        state: &mut SpSummonStepState,
    ) -> bool {
        for &e in &state.cost_effects.clone() {
            self.release_oath_relation(e);
        }
        if targets.is_none() {
            self.core.special_summoning.insert(target);
        }
        self.enable_field_effect(target, false);
        let summon_player = self.cards[target].summon.player;
        if self.is_flag(crate::duel::flags::CANNOT_SUMMON_OATH_OLD) {
            self.check_card_counter(
                target,
                crate::summon_support::activity::SPSUMMON,
                summon_player,
            );
        }
        let zone = if targets.is_some() && self.is_flag(crate::duel::flags::EMZONE) {
            self.narrow_zone_for_extra(targets, target, param.playerid, zone)
        } else {
            zone
        };
        self.move_to_field(
            target,
            summon_player,
            param.playerid,
            u16::from(location::MZONE),
            param.positions,
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

    /// Not ported: the Extra Monster Zone's share-out between the cards of
    /// one batch. Unreachable without `DUEL_EMZONE`.
    fn narrow_zone_for_extra(
        &mut self,
        _targets: Option<GroupId>,
        _target: CardId,
        _playerid: u8,
        _zone: u32,
    ) -> u32 {
        unimplemented!("the batch's Extra Monster Zone share-out needs DUEL_EMZONE")
    }

    /// `MSG_SPSUMMONING`. A face-down summon **does not name the card** — the
    /// code written is zero, because the opponent may not see what arrived.
    pub(crate) fn announce_spsummoning(&mut self, target: CardId) {
        let info = self.get_info_location(target);
        let code_ = if self.cards[target].current.is_position(position::FACEDOWN) {
            0
        } else {
            self.cards[target].data.code
        };
        self.messages.push(Message::SpSummoning {
            code: code_,
            controller: info.controller,
            location: info.location,
            sequence: info.sequence,
            position: info.position,
        });
    }
}

impl Field {
    /// `effect::check_value_condition` — read an effect's **value** slot as a
    /// predicate.
    ///
    /// The reference reuses the one Lua ref at whatever arity the caller
    /// needs; without a function value it is the constant's truthiness. The
    /// extra parameters ride in `args`.
    fn value_condition_holds(&mut self, effect: EffectId, card: CardId, args: &[i64]) -> bool {
        let Some(e) = self.effects.get(effect) else {
            return true;
        };
        let ev = crate::event::Event::new(0);
        let ctx = crate::effect::Ctx {
            reason_effect: effect,
            player: self.cards[card].summon.player,
            event: &ev,
            card: Some(card),
            args,
        };
        e.check_value_condition(self, &ctx)
    }

    /// One step of `SpSummon` — the **batch**.
    ///
    /// Six cases, and only the first has anything to do with individual
    /// cards: it emplaces one `SpSummonStep` each. Everything after it acts
    /// on whatever the group still holds, because a card that could not be
    /// summoned took itself out of it.
    pub(crate) fn sp_summon_step(
        &mut self,
        step: u16,
        reason_effect: Option<EffectId>,
        reason_player: u8,
        targets: GroupId,
        zone: u32,
    ) -> bool {
        match step {
            0 => self.sp_batch_0(targets, zone),
            1 => {
                if !self.core.ss_tograve_set.is_empty() {
                    let doomed: Vec<CardId> = self.core.ss_tograve_set.iter().copied().collect();
                    self.send_to(
                        doomed,
                        None,
                        reason::RULE,
                        PLAYER_NONE,
                        PLAYER_NONE,
                        u16::from(location::GRAVE),
                        0,
                        position::FACEUP,
                        false,
                    );
                }
                false
            }
            2 => self.sp_batch_2(targets),
            3 => self.sp_batch_3(reason_effect, reason_player, targets),
            4 => {
                let cards: Vec<CardId> = self.group(targets).iter().copied().collect();
                self.raise_event_over(
                    cards,
                    code::SPSUMMON_SUCCESS,
                    reason_effect,
                    0,
                    reason_player,
                    PLAYER_NONE,
                    0,
                );
                self.process_instant_event();
                false
            }
            5 => {
                let cards: Vec<CardId> = self.group(targets).iter().copied().collect();
                self.core.operated_set = cards.to_vec();
                self.core.returns.set(cards.len() as i32);
                true
            }
            _ => true,
        }
    }

    /// Step 0: the turn player's cards first, then the opponent's.
    ///
    /// Each half is sorted by `card_operation_sort` and charges
    /// `TIMING_SPSUMMON` for **its own** player. The split is what makes a
    /// summon of both players' cards at once deterministic, and it is by
    /// `summon.player` rather than by controller — the player performing the
    /// summon, not the one who will own the card.
    fn sp_batch_0(&mut self, targets: GroupId, zone: u32) -> bool {
        let turn_player = self.infos.turn_player;
        let mut own: Vec<CardId> = Vec::new();
        let mut other: Vec<CardId> = Vec::new();
        for &card in self.group(targets) {
            if self.cards[card].summon.player == turn_player {
                own.push(card);
            } else {
                other.push(card);
            }
        }
        for (half, player) in [(&mut own, turn_player), (&mut other, 1 - turn_player)] {
            if half.is_empty() {
                continue;
            }
            if half.len() > 1 {
                self.sort_by_operation(half);
            }
            self.core.hint_timing[player as usize] |= timing::SPSUMMON;
            for &card in half.iter() {
                self.emplace(Kind::SpSummonStep {
                    targets: Some(targets),
                    target: card,
                    zone,
                    state: Default::default(),
                });
            }
        }
        false
    }

    fn sort_by_operation(&mut self, cards: &mut [CardId]) {
        cards.sort_by(|&a, &b| {
            if self.card_operation_sort(a, b) {
                std::cmp::Ordering::Less
            } else if self.card_operation_sort(b, a) {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        });
    }

    /// Step 2: the counters, and the cards' new status.
    ///
    /// **An empty group ends the whole machine** — with `returns` set to
    /// zero, which is what a caller reads to learn that nothing was
    /// summoned.
    ///
    /// The once-per-turn ledger is bumped from a **set** of the codes
    /// present, not once per card: two copies of the same name summoned
    /// together use the allowance once. The per-player summon counter is the
    /// other way about — it is bumped once per *side that summoned
    /// anything*, however many cards that side put down.
    fn sp_batch_2(&mut self, targets: GroupId) -> bool {
        self.core.ss_tograve_set.clear();
        let cards: Vec<CardId> = self.group(targets).iter().copied().collect();
        if cards.is_empty() {
            self.core.returns.set(0);
            self.core.operated_set.clear();
            return true;
        }
        let turn_player = self.infos.turn_player;
        let mut summoned = [false; 2];
        let mut once_codes: [BTreeSet<u32>; 2] = [BTreeSet::new(), BTreeSet::new()];
        for &card in &cards {
            let p = self.cards[card].summon.player;
            summoned[usize::from(p != turn_player)] = true;
            let spcode = self.cards[card].spsummon_code;
            if spcode != 0 {
                once_codes[p as usize].insert(spcode);
            }
        }
        if summoned[0] {
            self.set_spsummon_counter(turn_player, true, false);
        }
        if summoned[1] {
            self.set_spsummon_counter(1 - turn_player, true, false);
        }
        for (plr, codes) in once_codes.into_iter().enumerate() {
            for code_ in codes {
                *self.core.spsummon_once_map[plr].entry(code_).or_insert(0) += 1;
            }
        }
        for &card in &cards {
            self.cards[card].set_status(status::SPSUMMON_STEP, false);
            self.cards[card].set_status(status::SPSUMMON_TURN, true);
            if self.cards[card].current.is_position(position::FACEUP) {
                self.enable_field_effect(card, true);
            }
        }
        self.adjust_instant();
        false
    }

    /// Step 3: `MSG_SPSUMMONED`, then each card's own success event.
    ///
    /// **A face-down card raises no `EVENT_SPSUMMON_SUCCESS` of its own.**
    /// The field-wide one at step 4 covers the whole batch either way; this
    /// is the card's own, and a card nobody can see has nothing to announce.
    ///
    /// The material events carry a reason chosen by the **summon type** —
    /// fusion, ritual, xyz or link — and a summon type with no material
    /// reason raises nothing at all. `STATUS_FUTURE_FUSION` suppresses them
    /// and is cleared here whether or not it did.
    fn sp_batch_3(
        &mut self,
        reason_effect: Option<EffectId>,
        reason_player: u8,
        targets: GroupId,
    ) -> bool {
        self.messages.push(Message::SpSummoned);
        let cards: Vec<CardId> = self.group(targets).iter().copied().collect();
        let batch: Vec<CardId> = cards.clone();
        for &card in &cards {
            let summon_player = self.cards[card].summon.player;
            if !self.is_flag(crate::duel::flags::CANNOT_SUMMON_OATH_OLD) {
                self.check_card_counter(
                    card,
                    crate::summon_support::activity::SPSUMMON,
                    summon_player,
                );
            }
            let (card_reason_effect, card_reason_player) = (
                self.cards[card].reason_effect,
                self.cards[card].reason_player,
            );
            if !self.cards[card].current.is_position(position::FACEDOWN) {
                self.raise_single_event(
                    card,
                    vec![],
                    code::SPSUMMON_SUCCESS,
                    card_reason_effect,
                    0,
                    card_reason_player,
                    summon_player,
                    0,
                );
            }
            let summontype = self.cards[card].summon.type_ & 0xff00_0000;
            let materials = self.cards[card].material_cards.clone();
            let suppressed = self.cards[card].is_status(status::FUTURE_FUSION);
            if summontype != 0 && !materials.is_empty() && !suppressed {
                let matreason = match summontype {
                    summon_type::FUSION => reason::FUSION,
                    summon_type::RITUAL => reason::RITUAL,
                    summon_type::XYZ => reason::XYZ,
                    summon_type::LINK => reason::LINK,
                    _ => 0,
                };
                for &m in &materials {
                    self.raise_single_event(
                        m,
                        batch.clone(),
                        code::BE_MATERIAL,
                        card_reason_effect,
                        matreason,
                        card_reason_player,
                        summon_player,
                        0,
                    );
                }
                self.raise_event_over(
                    materials.iter().copied().collect(),
                    code::BE_MATERIAL,
                    reason_effect,
                    matreason,
                    reason_player,
                    summon_player,
                    0,
                );
            }
            self.cards[card].set_status(status::FUTURE_FUSION, false);
        }
        self.process_single_event();
        self.process_instant_event();
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{Card, CardData};
    use crate::effect::{effect_type, flag, Effect};
    use crate::processor::Status;

    fn card_in(f: &mut Field, player: u8, loc: u8, seat: u32, type_: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 18036057,
                type_,
                level: 4,
                ..Default::default()
            },
            player,
        );
        c.current.controller = player;
        let id = f.new_card(c);
        f.add_card(player, id, loc, seat, false);
        if loc == location::MZONE {
            f.cards[id].current.position = position::FACEUP_ATTACK;
        }
        id
    }

    fn in_grave(f: &mut Field, player: u8) -> CardId {
        let seat = f.cards.len() as u32;
        card_in(f, player, location::GRAVE, seat, card_type::MONSTER)
    }

    fn single(f: &mut Field, card: CardId, code_: u32) -> EffectId {
        let mut e = Effect::new(effect_type::SINGLE, code_);
        e.owner = Some(card);
        e.handler = Some(card);
        let id = f.new_effect(e);
        f.cards[card].single_effect.insert(code_, id);
        f.cards[card].indexer.insert(id);
        id
    }

    /// Step the machine, answering the seat and window questions.
    fn drive(f: &mut Field, seat: i8, mut watch: impl FnMut(&Field)) -> Status {
        for _ in 0..4096 {
            watch(f);
            match f.process() {
                Status::Continue => continue,
                Status::Awaiting => match f.messages.last() {
                    Some(Message::SelectPlace { .. }) => {
                        // A negative seat means "the first free one", which
                        // is what a batch of more than one card needs.
                        let chosen = if seat >= 0 {
                            seat
                        } else {
                            (0..5)
                                .find(|&i| f.players[0].mzone[i].is_none())
                                .unwrap_or(0) as i8
                        };
                        f.core.returns.set_i8(0, 0);
                        f.core.returns.set_i8(1, location::MZONE as i8);
                        f.core.returns.set_i8(2, chosen);
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

    /// Summon one card from the graveyard, face-up attack.
    fn summon_one(f: &mut Field, card: CardId) {
        f.special_summon(
            BTreeSet::from([card]),
            0,
            0,
            0,
            false,
            false,
            position::FACEUP_ATTACK,
            0xff,
        );
    }

    mod parameters {
        use super::*;

        #[test]
        fn the_four_arguments_round_trip() {
            for p in [
                SpSummonParam {
                    playerid: 1,
                    nocheck: true,
                    nolimit: false,
                    positions: position::FACEUP_ATTACK,
                },
                SpSummonParam {
                    playerid: 0,
                    nocheck: false,
                    nolimit: true,
                    positions: position::FACEDOWN_DEFENSE,
                },
            ] {
                assert_eq!(SpSummonParam::unpack(p.pack()), p);
            }
        }

        /// The packing is the reference's, and the shifts are what a test
        /// pins: a player id of 1 lands in the top nibble, not anywhere else.
        #[test]
        fn the_packing_is_the_references() {
            let p = SpSummonParam {
                playerid: 1,
                nocheck: true,
                nolimit: true,
                positions: 0x5,
            };
            assert_eq!(p.pack(), 0x0101_0105);
        }
    }

    mod preparation {
        use super::*;

        /// The card records **where it came from** before it moves, and its
        /// old reason is saved so a refusal can put it back.
        #[test]
        fn the_card_records_where_it_came_from() {
            let mut f = Field::new(8000);
            let c = in_grave(&mut f, 0);
            f.cards[c].reason = reason::DESTROY;
            f.cards[c].reason_player = 1;
            summon_one(&mut f, c);
            assert_eq!(f.cards[c].summon.location, location::GRAVE);
            assert_eq!(f.cards[c].summon.player, 0);
            assert_eq!(
                f.cards[c].summon.type_,
                summon_type::SPECIAL,
                "and what it is becoming"
            );
            assert_eq!(f.cards[c].reason, reason::SPSUMMON);
            assert_eq!(
                f.cards[c].temp.reason,
                reason::DESTROY,
                "the old reason is kept for a refusal to restore"
            );
            assert_eq!(f.cards[c].temp.reason_player, 1);
        }

        /// The summon type keeps only its low bits and always gains
        /// `SUMMON_TYPE_SPECIAL`.
        #[test]
        fn the_summon_type_is_masked_and_marked() {
            let mut f = Field::new(8000);
            let c = in_grave(&mut f, 0);
            f.special_summon(
                BTreeSet::from([c]),
                summon_type::FUSION | 0xf000_0000,
                0,
                0,
                false,
                false,
                position::FACEUP_ATTACK,
                0xff,
            );
            assert_eq!(
                f.cards[c].summon.type_,
                (summon_type::FUSION & 0xf00_ffff) | summon_type::SPECIAL
            );
        }

        /// **The single-card form strikes `POS_FACEUP` out of the mask when
        /// the card's name is already face-up on the field.** The batch form
        /// does not, and the asymmetry is the reference's.
        #[test]
        fn only_the_single_card_form_checks_uniqueness() {
            /// A card carrying the "only one face-up" limit, in the
            /// graveyard and waiting to be summoned.
            fn limited(f: &mut Field) -> CardId {
                let seat = f.cards.len() as u32;
                let c = card_in(f, 0, location::GRAVE, seat, card_type::MONSTER);
                f.cards[c].unique_code = 18036057;
                f.cards[c].unique_location = u16::from(location::MZONE);
                f.cards[c].unique_pos = [1, 0];
                let mut e = Effect::new(effect_type::SINGLE, 0);
                e.owner = Some(c);
                e.handler = Some(c);
                let e = f.new_effect(e);
                f.cards[c].unique_effect = Some(e);
                c
            }
            let mut f = Field::new(8000);
            // One of the name already face-up, registered as the limiter.
            let onfield = card_in(&mut f, 0, location::MZONE, 0, card_type::MONSTER);
            f.cards[onfield].set_status(status::EFFECT_ENABLED, true);
            f.cards[onfield].unique_code = 18036057;
            f.cards[onfield].unique_location = u16::from(location::MZONE);
            f.cards[onfield].unique_pos = [1, 0];
            f.cards[onfield].unique_fieldid = 1;
            let mut e = Effect::new(effect_type::SINGLE, 0);
            e.owner = Some(onfield);
            e.handler = Some(onfield);
            let e = f.new_effect(e);
            f.cards[onfield].unique_effect = Some(e);
            f.core.unique_cards[0].push(onfield);

            let single_form = limited(&mut f);
            f.special_summon_step(
                single_form,
                0,
                0,
                0,
                false,
                false,
                position::FACEUP_ATTACK,
                0xff,
            );
            assert_eq!(
                SpSummonParam::unpack(f.cards[single_form].spsummon_param).positions,
                0,
                "face-up struck out, and nothing else was asked for"
            );

            let batch_form = limited(&mut f);
            f.special_summon(
                BTreeSet::from([batch_form]),
                0,
                0,
                0,
                false,
                false,
                position::FACEUP_ATTACK,
                0xff,
            );
            assert_eq!(
                SpSummonParam::unpack(f.cards[batch_form].spsummon_param).positions,
                position::FACEUP_ATTACK,
                "the batch form does not make the check"
            );
        }
    }

    mod positions {
        use super::*;

        /// **`EFFECT_DEVINE_LIGHT` folds the mask before the summon starts.**
        /// A face-down-defence request becomes face-up defence — the same
        /// position, turned over — rather than being masked away.
        #[test]
        fn devine_light_folds_the_requested_position() {
            let mut f = Field::new(8000);
            let anchor = card_in(&mut f, 0, location::MZONE, 0, card_type::MONSTER);
            f.cards[anchor].set_status(status::EFFECT_ENABLED, true);
            let mut e = Effect::new(effect_type::FIELD, code::DEVINE_LIGHT);
            e.owner = Some(anchor);
            e.handler = Some(anchor);
            e.flag[0] = flag::PLAYER_TARGET | flag::ABSOLUTE_TARGET;
            e.range = u16::from(location::MZONE);
            e.s_range = 1;
            let id = f.new_effect(e);
            f.add_effect(id, 0);

            let c = in_grave(&mut f, 0);
            f.special_summon(
                BTreeSet::from([c]),
                0,
                0,
                0,
                false,
                false,
                position::FACEDOWN_DEFENSE,
                0xff,
            );
            run(&mut f, 1);
            assert_eq!(
                f.cards[c].current.position,
                position::FACEUP_DEFENSE,
                "folded to the same position, face-up"
            );
        }

        /// A face-down card is **not** enabled at the end of the batch.
        #[test]
        fn a_face_down_card_is_left_disabled() {
            let mut f = Field::new(8000);
            let c = in_grave(&mut f, 0);
            f.special_summon(
                BTreeSet::from([c]),
                0,
                0,
                0,
                false,
                false,
                position::FACEDOWN_DEFENSE,
                0xff,
            );
            run(&mut f, 0);
            assert_eq!(f.cards[c].current.position, position::FACEDOWN_DEFENSE);
            assert!(
                !f.cards[c].is_status(status::EFFECT_ENABLED),
                "a face-down card applies nothing"
            );
        }

        /// And a face-up one is.
        #[test]
        fn a_face_up_card_is_enabled() {
            let mut f = Field::new(8000);
            let c = in_grave(&mut f, 0);
            summon_one(&mut f, c);
            run(&mut f, 0);
            assert!(f.cards[c].is_status(status::EFFECT_ENABLED));
        }

        /// **A face-down card raises no `EVENT_SPSUMMON_SUCCESS` of its
        /// own.** The batch-wide one still covers it; this is the card's,
        /// and a card nobody can see has nothing to announce.
        ///
        /// A single event with nothing listening leaves no trace, so the
        /// card is given a `SINGLE | ACTIONS | TRIGGER_O` effect to listen
        /// with — only `process_single_event` can find one of those — and
        /// the chain it becomes is looked for on `new_ochain`, since this
        /// machine opens no window to offer it in.
        fn summon_with_a_listener(f: &mut Field, positions: u8) -> bool {
            let c = in_grave(f, 0);
            let mut e = Effect::new(
                effect_type::SINGLE | effect_type::ACTIONS | effect_type::TRIGGER_O,
                code::SPSUMMON_SUCCESS,
            );
            e.owner = Some(c);
            e.handler = Some(c);
            e.range = u16::from(location::MZONE);
            let id = f.new_effect(e);
            f.cards[c].single_effect.insert(code::SPSUMMON_SUCCESS, id);
            f.cards[c].indexer.insert(id);
            f.special_summon(BTreeSet::from([c]), 0, 0, 0, false, false, positions, 0xff);
            run(f, 0);
            // `SpSummon` opens no window of its own, so the gathered trigger
            // waits on `new_ochain` for whatever window comes next. That is
            // where a test has to look.
            !f.core.new_ochain.is_empty()
        }

        #[test]
        fn only_a_face_up_card_raises_its_own_success_event() {
            let mut f = Field::new(8000);
            assert!(
                summon_with_a_listener(&mut f, position::FACEUP_ATTACK),
                "face-up: the trigger is offered"
            );
            let mut g = Field::new(8000);
            assert!(
                !summon_with_a_listener(&mut g, position::FACEDOWN_DEFENSE),
                "face-down: nothing is announced"
            );
        }
    }

    mod refusals {
        use super::*;

        /// A card already in a Monster Zone cannot be Special Summoned into
        /// one.
        #[test]
        fn a_card_already_on_the_field_is_refused() {
            let mut f = Field::new(8000);
            let c = card_in(&mut f, 0, location::MZONE, 0, card_type::MONSTER);
            summon_one(&mut f, c);
            run(&mut f, 1);
            assert_eq!(f.core.returns.get(), 0, "nothing was summoned");
            assert_eq!(f.cards[c].current.sequence, 0, "and it did not move");
        }

        /// A refused card has its **reason put back** — the saved copy is
        /// what makes that possible.
        #[test]
        fn a_refused_card_has_its_reason_restored() {
            let mut f = Field::new(8000);
            let c = card_in(&mut f, 0, location::MZONE, 0, card_type::MONSTER);
            f.cards[c].reason = reason::DESTROY;
            summon_one(&mut f, c);
            run(&mut f, 1);
            assert_eq!(
                f.cards[c].reason,
                reason::DESTROY,
                "put back, not left as SPSUMMON"
            );
        }

        /// A non-monster is refused unless the summon waives the checks.
        #[test]
        fn a_non_monster_is_refused_unless_nocheck() {
            for (nocheck, want) in [(false, 0), (true, 1)] {
                let mut f = Field::new(8000);
                let c = card_in(&mut f, 0, location::GRAVE, 0, card_type::SPELL);
                f.special_summon(
                    BTreeSet::from([c]),
                    0,
                    0,
                    0,
                    nocheck,
                    false,
                    position::FACEUP_ATTACK,
                    0xff,
                );
                run(&mut f, 0);
                assert_eq!(f.core.returns.get(), want, "nocheck {nocheck}");
            }
        }

        /// **Nowhere to go sends the card to the graveyard** — and a card
        /// already there is left alone.
        #[test]
        fn a_card_with_no_room_is_sent_to_the_graveyard() {
            let mut f = Field::new(8000);
            for seat in 0..5 {
                card_in(&mut f, 0, location::MZONE, seat, card_type::MONSTER);
            }
            let c = card_in(&mut f, 0, location::HAND, 0, card_type::MONSTER);
            summon_one(&mut f, c);
            run(&mut f, 0);
            assert_eq!(f.core.returns.get(), 0);
            assert_eq!(f.cards[c].current.location, location::GRAVE);
        }

        #[test]
        fn a_card_already_in_the_graveyard_is_left_there() {
            let mut f = Field::new(8000);
            for seat in 0..5 {
                card_in(&mut f, 0, location::MZONE, seat, card_type::MONSTER);
            }
            let c = in_grave(&mut f, 0);
            summon_one(&mut f, c);
            // The queue is emptied at step 2, so it has to be watched rather
            // than inspected afterwards.
            let mut queued = false;
            drive(&mut f, 0, |f| {
                queued |= f.core.ss_tograve_set.contains(&c);
            });
            assert!(!queued, "never queued for the graveyard it is already in");
            assert_eq!(f.cards[c].current.location, location::GRAVE);
        }

        /// **`nolimit` and `nocheck` are not the same waiver.**
        ///
        /// A card with `EFFECT_REVIVE_LIMIT` that was never properly
        /// summoned may not come back from the graveyard at all — `nolimit`
        /// opens that, and `nocheck` does not.
        #[test]
        fn the_revive_limit_is_waived_by_nolimit_only() {
            for (nocheck, nolimit, want) in [(false, false, 0), (true, false, 0), (false, true, 1)]
            {
                let mut f = Field::new(8000);
                let c = in_grave(&mut f, 0);
                single(&mut f, c, code::REVIVE_LIMIT);
                f.special_summon(
                    BTreeSet::from([c]),
                    0,
                    0,
                    0,
                    nocheck,
                    nolimit,
                    position::FACEUP_ATTACK,
                    0xff,
                );
                run(&mut f, 0);
                assert_eq!(
                    f.core.returns.get(),
                    want,
                    "nocheck {nocheck} nolimit {nolimit}"
                );
            }
        }

        /// From the **hand**, the same limit is waived by either.
        #[test]
        fn from_the_hand_the_revive_limit_yields_to_either_waiver() {
            for (nocheck, nolimit, want) in [(false, false, 0), (true, false, 1), (false, true, 1)]
            {
                let mut f = Field::new(8000);
                let c = card_in(&mut f, 0, location::HAND, 0, card_type::MONSTER);
                single(&mut f, c, code::REVIVE_LIMIT);
                f.special_summon(
                    BTreeSet::from([c]),
                    0,
                    0,
                    0,
                    nocheck,
                    nolimit,
                    position::FACEUP_ATTACK,
                    0xff,
                );
                run(&mut f, 0);
                assert_eq!(
                    f.core.returns.get(),
                    want,
                    "nocheck {nocheck} nolimit {nolimit}"
                );
            }
        }

        /// A card that has completed a proper summon is past the limit.
        #[test]
        fn a_card_that_was_properly_summoned_is_past_the_limit() {
            let mut f = Field::new(8000);
            let c = in_grave(&mut f, 0);
            single(&mut f, c, code::REVIVE_LIMIT);
            f.cards[c].set_status(status::PROC_COMPLETE, true);
            summon_one(&mut f, c);
            run(&mut f, 0);
            assert_eq!(f.core.returns.get(), 1);
        }

        /// `EFFECT_SPSUMMON_CONDITION` refuses, and `nocheck` skips asking.
        #[test]
        fn a_refusing_condition_drops_the_card_unless_nocheck() {
            for (nocheck, want) in [(false, 0), (true, 1)] {
                let mut f = Field::new(8000);
                let c = in_grave(&mut f, 0);
                let e = single(&mut f, c, code::SPSUMMON_CONDITION);
                if let Some(x) = f.effects.get_mut(e) {
                    x.value = 0;
                }
                f.special_summon(
                    BTreeSet::from([c]),
                    0,
                    0,
                    0,
                    nocheck,
                    false,
                    position::FACEUP_ATTACK,
                    0xff,
                );
                run(&mut f, 0);
                assert_eq!(f.core.returns.get(), want, "nocheck {nocheck}");
            }
        }
    }

    mod ordering {
        use super::*;

        /// **The turn player's cards are emplaced first**, and each half
        /// charges `TIMING_SPSUMMON` for its own player.
        ///
        /// Asserted directly on the queue rather than through a duel,
        /// because `special_summon` gives every card in a batch the same
        /// `summon.player` — the second half is reachable only from the
        /// group summon procedures, which are not ported yet. The rule is
        /// pinned now so that it is not re-derived then.
        #[test]
        fn the_turn_players_cards_are_emplaced_first() {
            let mut f = Field::new(8000);
            f.infos.turn_player = 0;
            // Both are **controlled** by player 0; only the summoning
            // player differs, which is what the split reads.
            let theirs = in_grave(&mut f, 0);
            let mine = in_grave(&mut f, 0);
            f.cards[theirs].summon.player = 1;
            f.cards[mine].summon.player = 0;
            // The group is a set, so `theirs` (created first) sorts ahead of
            // `mine` in it — the split has to reverse that.
            let group = f.new_group([theirs, mine]);

            f.sp_summon_step(0, None, 0, group, 0xff);

            let order: Vec<CardId> = f
                .core
                .subunits
                .iter()
                .filter_map(|u| match u.kind {
                    Kind::SpSummonStep { target, .. } => Some(target),
                    _ => None,
                })
                .collect();
            assert_eq!(order, vec![mine, theirs], "the turn player's card first");
            assert_ne!(f.core.hint_timing[0] & timing::SPSUMMON, 0);
            assert_ne!(
                f.core.hint_timing[1] & timing::SPSUMMON,
                0,
                "and the other half charges its own player"
            );
        }

        /// A half with nothing in it charges nothing.
        #[test]
        fn an_empty_half_charges_no_timing() {
            let mut f = Field::new(8000);
            f.infos.turn_player = 0;
            let mine = in_grave(&mut f, 0);
            f.cards[mine].summon.player = 0;
            let group = f.new_group([mine]);

            f.sp_summon_step(0, None, 0, group, 0xff);
            assert_ne!(f.core.hint_timing[0] & timing::SPSUMMON, 0);
            assert_eq!(
                f.core.hint_timing[1] & timing::SPSUMMON,
                0,
                "the opponent summoned nothing"
            );
        }
    }

    mod success {
        use super::*;

        #[test]
        fn a_card_arrives_in_a_monster_zone() {
            let mut f = Field::new(8000);
            let c = in_grave(&mut f, 0);
            summon_one(&mut f, c);
            assert_eq!(run(&mut f, 2), Status::End);
            assert_eq!(f.cards[c].current.location, location::MZONE);
            assert_eq!(f.cards[c].current.sequence, 2);
            assert_eq!(f.cards[c].current.position, position::FACEUP_ATTACK);
            assert!(f.cards[c].is_status(status::SPSUMMON_TURN));
            assert!(
                !f.cards[c].is_status(status::SPSUMMON_STEP),
                "the per-step marker is cleared at the end"
            );
            assert_eq!(f.core.returns.get(), 1, "one survived");
            assert_eq!(f.core.operated_set, vec![c]);
        }

        /// A batch in which nothing survives says so, and says nothing else.
        ///
        /// `returns` is zero either way — the count of survivors *is* zero —
        /// so what separates a refused batch from a summoned one is that the
        /// refused one never announces itself.
        #[test]
        fn a_batch_that_summons_nothing_announces_nothing() {
            let mut f = Field::new(8000);
            let c = card_in(&mut f, 0, location::MZONE, 0, card_type::MONSTER);
            summon_one(&mut f, c);
            run(&mut f, 1);
            assert_eq!(f.core.returns.get(), 0);
            assert!(
                !f.messages.iter().any(|m| matches!(m, Message::SpSummoned)),
                "nothing was summoned, so nothing is announced"
            );
            assert!(f.core.operated_set.is_empty());
        }

        /// A card summoned **in a batch** does not register itself on
        /// `special_summoning`; the group already holds it. Only the
        /// single-card form does, and only so that
        /// `special_summon_complete` can find it.
        #[test]
        fn a_batched_card_does_not_register_itself() {
            let mut f = Field::new(8000);
            let c = in_grave(&mut f, 0);
            summon_one(&mut f, c);
            let mut registered = false;
            drive(&mut f, 0, |f| {
                registered |= f.core.special_summoning.contains(&c);
            });
            assert_eq!(f.cards[c].current.location, location::MZONE, "it happened");
            assert!(!registered, "and it never went on the list");
        }

        /// `STATUS_FUTURE_FUSION` suppresses the material events, and is
        /// cleared whether or not it did.
        #[test]
        fn future_fusion_suppresses_the_material_events() {
            fn summon_with_material(f: &mut Field, suppressed: bool) -> bool {
                let material = in_grave(f, 0);
                let c = in_grave(f, 0);
                f.cards[c].material_cards = BTreeSet::from([material]);
                if suppressed {
                    f.cards[c].set_status(status::FUTURE_FUSION, true);
                }
                f.special_summon(
                    BTreeSet::from([c]),
                    summon_type::FUSION,
                    0,
                    0,
                    false,
                    false,
                    position::FACEUP_ATTACK,
                    0xff,
                );
                run(f, 0);
                assert!(
                    !f.cards[c].is_status(status::FUTURE_FUSION),
                    "cleared either way"
                );
                f.core
                    .instant_event
                    .iter()
                    .chain(f.core.used_event.iter())
                    .any(|e| e.event_code == code::BE_MATERIAL)
            }
            let mut f = Field::new(8000);
            assert!(summon_with_material(&mut f, false), "ordinarily raised");
            let mut g = Field::new(8000);
            assert!(!summon_with_material(&mut g, true), "and suppressed");
        }

        /// The counters: the per-player tally once, and the once-per-turn
        /// ledger once per **name**.
        #[test]
        fn the_counters_are_bumped_once_each() {
            let mut f = Field::new(8000);
            let a = in_grave(&mut f, 0);
            let b = in_grave(&mut f, 0);
            f.cards[a].spsummon_code = 777;
            f.cards[b].spsummon_code = 777;
            f.special_summon(
                BTreeSet::from([a, b]),
                0,
                0,
                0,
                false,
                false,
                position::FACEUP_ATTACK,
                0xff,
            );
            run(&mut f, -1);
            assert_eq!(f.core.returns.get(), 2, "both arrived");
            assert_eq!(
                f.core.spsummon_state_count[0], 1,
                "once for the side, not once per card"
            );
            assert_eq!(
                f.core.spsummon_once_map[0].get(&777),
                Some(&1),
                "and once for the name"
            );
        }

        /// `MSG_SPSUMMONING` **does not name a face-down card**.
        #[test]
        fn a_face_down_summon_does_not_name_the_card() {
            let mut f = Field::new(8000);
            let c = in_grave(&mut f, 0);
            f.special_summon(
                BTreeSet::from([c]),
                0,
                0,
                0,
                false,
                false,
                position::FACEDOWN_DEFENSE,
                0xff,
            );
            run(&mut f, 0);
            assert_eq!(f.cards[c].current.position, position::FACEDOWN_DEFENSE);
            let named = f.messages.iter().find_map(|m| match m {
                Message::SpSummoning { code, .. } => Some(*code),
                _ => None,
            });
            assert_eq!(named, Some(0), "the name is withheld");
        }

        /// And a face-up one does.
        #[test]
        fn a_face_up_summon_names_the_card() {
            let mut f = Field::new(8000);
            let c = in_grave(&mut f, 0);
            summon_one(&mut f, c);
            run(&mut f, 0);
            let named = f.messages.iter().find_map(|m| match m {
                Message::SpSummoning { code, .. } => Some(*code),
                _ => None,
            });
            assert_eq!(named, Some(18036057));
        }

        /// The success event is raised over the batch, and the timing
        /// charged for the summoning player.
        #[test]
        fn the_success_event_and_timing() {
            let mut f = Field::new(8000);
            let c = in_grave(&mut f, 0);
            summon_one(&mut f, c);
            let mut charged = false;
            drive(&mut f, 0, |f| {
                charged |= f.core.hint_timing[0] & timing::SPSUMMON != 0;
            });
            assert!(charged);
            assert!(f
                .core
                .instant_event
                .iter()
                .chain(f.core.used_event.iter())
                .any(|e| e.event_code == code::SPSUMMON_SUCCESS));
            assert!(f.messages.iter().any(|m| matches!(m, Message::SpSummoned)));
        }

        /// **The single-card form finishes through `special_summon_complete`.**
        /// Until then the card is on `special_summoning` and still carries
        /// `STATUS_SPSUMMON_STEP`.
        #[test]
        fn the_single_card_form_finishes_at_the_complete_call() {
            let mut f = Field::new(8000);
            let c = in_grave(&mut f, 0);
            f.special_summon_step(c, 0, 0, 0, false, false, position::FACEUP_ATTACK, 0xff);
            run(&mut f, 3);
            assert_eq!(f.cards[c].current.location, location::MZONE);
            assert!(
                f.cards[c].is_status(status::SPSUMMON_STEP),
                "not finished yet"
            );
            assert_eq!(f.core.special_summoning, BTreeSet::from([c]));
            assert_eq!(f.core.spsummon_state_count[0], 0);

            f.special_summon_complete(None, 0);
            run(&mut f, 3);
            assert!(!f.cards[c].is_status(status::SPSUMMON_STEP));
            assert!(f.cards[c].is_status(status::SPSUMMON_TURN));
            assert_eq!(f.core.spsummon_state_count[0], 1);
        }

        /// **The seat question is asked even when only one zone is free.**
        ///
        /// `move_to_field`'s `confirm` argument defaults to *true* in the
        /// reference and every summon takes the default; a port passing
        /// `false` would place the card silently. Four call sites had it
        /// wrong and this is the test that can see it.
        #[test]
        fn the_seat_is_confirmed_even_when_only_one_was_asked_for() {
            let mut f = Field::new(8000);
            let c = in_grave(&mut f, 0);
            // A single-bit `zone`: the caller has named exactly one seat, and
            // it is that shape the shortcut tests.
            f.special_summon(
                BTreeSet::from([c]),
                0,
                0,
                0,
                false,
                false,
                position::FACEUP_ATTACK,
                1 << 2,
            );
            run(&mut f, 2);
            assert_eq!(f.cards[c].current.sequence, 2);
            assert!(
                f.messages
                    .iter()
                    .any(|m| matches!(m, Message::SelectPlace { .. })),
                "the one named seat is still confirmed"
            );
        }
    }
}

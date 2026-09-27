//! "May this card do that?" — the five questions the Main Phase asks of every
//! card before offering it.
//!
//! `IdleCommand` walks the hand and the field building five lists, and these
//! are what it walks them with. Nothing here changes the board; each is a
//! question, asked with the LP cost saved around it so that asking never
//! pays.
//!
//! ## `EFFECT_STATUS_SUMMON_SELF`, which is a side effect of asking
//!
//! Three of the five *do* change something: if the effect currently being
//! resolved is the card's own, they mark it `SUMMON_SELF`. That is how an
//! effect that summons its own card is later told apart from one that
//! summons another's — and it is set by the **question**, not by the summon,
//! which is the kind of thing a port drops because it looks like it belongs
//! somewhere else.

use crate::board::{location, position};
use crate::card::{card_type, status, summon_type};
use crate::event::{code, CardId, EffectId};
use crate::field::Field;
use crate::point_event::effect_status;
use crate::tribute::Procedures;

impl Field {
    /// `card::check_cost_condition(ecode, playerid)` — the three-argument
    /// form, which asks without a summon type.
    pub fn check_cost_condition(&mut self, card: CardId, ecode: u32, playerid: u8) -> bool {
        let mut candidates = self.filter_player_effect(playerid, ecode);
        candidates.extend(self.filter_effect(card, ecode));
        for e in candidates {
            let Some(cost) = self.effects.get(e).and_then(|x| x.cost) else {
                continue;
            };
            let args = [i64::from(playerid)];
            let ev = crate::event::Event::new(0);
            let ctx = crate::effect::Ctx {
                reason_effect: e,
                player: playerid,
                event: &ev,
                card: Some(card),
                args: &args,
            };
            if !cost(self, &ctx, false) {
                return false;
            }
        }
        true
    }

    /// `field::is_player_can_sset` — may this player set a card at all?
    ///
    /// The card-aware polarity: a prohibition with no target refuses, and
    /// one whose target *agrees* refuses. Same shape as the special-summon
    /// and flip-summon permissions.
    pub fn is_player_can_sset(&mut self, playerid: u8, card: CardId) -> bool {
        for e in self.filter_player_effect(playerid, code::CANNOT_SSET) {
            let Some(check) = self.effects.get(e).and_then(|x| x.target_filter) else {
                return false;
            };
            if check(self, e, Some(card), &[i64::from(playerid)]) {
                return false;
            }
        }
        true
    }

    /// Mark the effect being resolved as summoning **its own** card.
    fn mark_summon_self(&mut self, card: CardId) {
        let Some(reason) = self.core.reason_effect else {
            return;
        };
        if self
            .effects
            .get(reason)
            .and_then(|e| e.get_handler(&self.cards))
            != Some(card)
        {
            return;
        }
        if let Some(e) = self.effects.get_mut(reason) {
            e.status |= effect_status::SUMMON_SELF;
        }
    }

    /// `card::is_can_be_summoned` — may this be Normal Summoned now?
    ///
    /// Two shapes in one function, forked on where the card is. **In a
    /// Monster Zone it is a Gemini summon**, with its own five conditions;
    /// **in the hand** it is the ordinary one, and the procedure filter
    /// decides.
    ///
    /// A card anywhere else falls through both branches and is **allowed** —
    /// which looks like a gap and is the reference's: this question is asked
    /// by `IdleCommand` only of the hand and the Monster Zones, so the third
    /// case never arises from the caller that matters.
    pub fn is_can_be_summoned(
        &mut self,
        card: CardId,
        playerid: u8,
        ignore_count: bool,
        procedure: Option<EffectId>,
        min_tribute: u8,
        zone: u32,
    ) -> bool {
        if !self.is_summonable_card(card) {
            return false;
        }
        self.mark_summon_self(card);
        if !ignore_count {
            let no_extra = self.core.extra_summon[playerid as usize]
                || self
                    .is_affected_by_effect(card, code::EXTRA_SUMMON_COUNT)
                    .is_none();
            let limit = self.get_summon_count_limit(playerid);
            if no_extra && self.core.summon_count[playerid as usize] as i32 >= limit {
                return false;
            }
        }
        if self.cards[card].is_status(status::FORBIDDEN) {
            return false;
        }

        self.save_lp_cost();
        let allowed =
            self.summon_is_allowed(card, playerid, ignore_count, procedure, min_tribute, zone);
        self.restore_lp_cost();
        allowed
    }

    /// The part of `is_can_be_summoned` the LP cost is saved around.
    fn summon_is_allowed(
        &mut self,
        card: CardId,
        playerid: u8,
        ignore_count: bool,
        procedure: Option<EffectId>,
        min_tribute: u8,
        zone: u32,
    ) -> bool {
        if !self.check_cost_condition(card, code::SUMMON_COST, playerid) {
            return false;
        }
        match self.cards[card].current.location {
            location::MZONE => {
                self.cards[card].current.is_position(position::FACEUP)
                    && self
                        .is_affected_by_effect(card, code::GEMINI_SUMMONABLE)
                        .is_some()
                    && self
                        .is_affected_by_effect(card, code::GEMINI_STATUS)
                        .is_none()
                    && self.is_player_can_summon(
                        summon_type::GEMINI,
                        playerid,
                        Some(card),
                        playerid,
                    )
                    && self
                        .is_affected_by_effect(card, code::CANNOT_SUMMON)
                        .is_none()
            }
            location::HAND => {
                if self
                    .is_affected_by_effect(card, code::CANNOT_SUMMON)
                    .is_some()
                {
                    return false;
                }
                let found =
                    self.filter_summon_procedure(card, playerid, ignore_count, min_tribute, zone);
                match procedure {
                    Some(named) => {
                        !matches!(found, Procedures::Forbidden)
                            && self.check_summon_procedure(
                                card,
                                named,
                                playerid,
                                ignore_count,
                                min_tribute,
                                zone,
                            )
                    }
                    // "No procedure and no ordinary summon" is the refusal.
                    // A `Limited` list that is empty is `Forbidden`, which is
                    // why only two of the three answers refuse here.
                    None => match found {
                        Procedures::Forbidden => false,
                        Procedures::Limited(ps) => !ps.is_empty(),
                        Procedures::Ordinary {
                            procedures,
                            ordinary,
                        } => !procedures.is_empty() || ordinary,
                    },
                }
            }
            _ => true,
        }
    }

    /// `card::is_can_be_special_summoned` — may this card be Special
    /// Summoned by that effect, into that position, on that side?
    ///
    /// The longest of the five, and its order matters: the **revive limit**
    /// is asked before anything about the destination, because a card that
    /// was never properly summoned cannot come back however much room there
    /// is.
    ///
    /// `nolimit` waives the revive limit from everywhere; `nocheck` waives it
    /// only from the hand and the deck, and separately waives the summon
    /// conditions and the monster-type test. The same two waivers
    /// `SpSummonStep` reads, asked one step earlier — and asked here about a
    /// card that has not yet had `spsummon_param` packed onto it, which is
    /// why they are arguments rather than unpacked.
    ///
    /// **A face-up summon of a name already face-up loses `POS_FACEUP`**
    /// rather than being refused outright; it is refused only when nothing
    /// is left of the mask. So such a card can still be Special Summoned
    /// face-down.
    #[allow(clippy::too_many_arguments)]
    pub fn is_can_be_special_summoned(
        &mut self,
        card: CardId,
        reason_effect: Option<EffectId>,
        sumtype: u32,
        sumpos: u8,
        sumplayer: u8,
        toplayer: u8,
        nocheck: bool,
        nolimit: bool,
        zone: u32,
    ) -> bool {
        if reason_effect
            .and_then(|e| self.effects.get(e))
            .and_then(|e| e.get_handler(&self.cards))
            == Some(card)
        {
            if let Some(e) = reason_effect.and_then(|e| self.effects.get_mut(e)) {
                e.status |= effect_status::SUMMON_SELF;
            }
        }
        if self.cards[card].current.location == location::MZONE {
            return false;
        }
        let loc = u16::from(self.cards[card].current.location);
        if !nolimit
            && self
                .is_affected_by_effect(card, code::REVIVE_LIMIT)
                .is_some()
            && !self.cards[card].is_status(status::PROC_COMPLETE)
        {
            let from_grave_removed_or_szone =
                loc & u16::from(location::GRAVE | location::REMOVED | location::SZONE) != 0;
            let from_deck_or_hand = loc & u16::from(location::DECK | location::HAND) != 0;
            if from_grave_removed_or_szone || (!nocheck && from_deck_or_hand) {
                return false;
            }
            let ty = self.cards[card].data.type_;
            if ty & card_type::PENDULUM != 0
                && self.cards[card].current.location == location::EXTRA
                && self.cards[card].current.is_position(position::FACEUP)
            {
                return false;
            }
            if self.cards[card].current.location == location::OVERLAY {
                return false;
            }
        }
        let ty = self.cards[card].data.type_;
        if ty & card_type::PENDULUM != 0
            && self.cards[card].current.location == location::EXTRA
            && self.cards[card].current.is_position(position::FACEUP)
            && matches!(
                sumtype,
                summon_type::FUSION | summon_type::SYNCHRO | summon_type::XYZ
            )
        {
            return false;
        }

        let mut sumpos = sumpos;
        if sumpos & position::FACEDOWN != 0
            && self
                .is_player_affected_by_effect(sumplayer, code::DEVINE_LIGHT)
                .is_some()
        {
            sumpos = (sumpos & position::FACEUP) | ((sumpos & position::FACEDOWN) >> 1);
        }
        if sumpos & position::FACEUP != 0
            && self
                .check_unique_onfield(card, toplayer, u16::from(location::MZONE), None)
                .is_some()
        {
            sumpos &= !position::FACEUP;
            if sumpos == 0 {
                return false;
            }
        }
        let sumtype = sumtype | summon_type::SPECIAL;
        if (sumplayer == 0 || sumplayer == 1)
            && !self.is_player_can_spsummon(
                reason_effect,
                sumtype,
                sumpos,
                sumplayer,
                toplayer,
                card,
                None,
            )
        {
            return false;
        }
        if self
            .is_affected_by_effect(card, code::CANNOT_SPECIAL_SUMMON)
            .is_some()
        {
            return false;
        }
        if self.cards[card].is_status(status::FORBIDDEN) {
            return false;
        }
        if zone != 0xff
            && self.get_useable_count(
                Some(card),
                toplayer,
                location::MZONE,
                sumplayer,
                Self::LOCATION_REASON_TOFIELD,
                zone,
            ) <= 0
        {
            return false;
        }

        self.save_lp_cost();
        let allowed = self.spsummon_is_allowed(
            card,
            reason_effect,
            sumtype,
            sumpos,
            sumplayer,
            toplayer,
            nocheck,
        );
        self.restore_lp_cost();
        allowed
    }

    #[allow(clippy::too_many_arguments)]
    fn spsummon_is_allowed(
        &mut self,
        card: CardId,
        reason_effect: Option<EffectId>,
        sumtype: u32,
        sumpos: u8,
        sumplayer: u8,
        toplayer: u8,
        nocheck: bool,
    ) -> bool {
        if !self.check_cost_condition_typed(card, code::SPSUMMON_COST, sumplayer, sumtype) {
            return false;
        }
        if nocheck {
            return true;
        }
        if self.cards[card].data.type_ & card_type::MONSTER == 0 {
            return false;
        }
        for e in self.filter_effect(card, code::SPSUMMON_CONDITION) {
            let args = [
                reason_effect.map_or(-1, |x| x as i64),
                i64::from(sumplayer),
                i64::from(sumtype),
                i64::from(sumpos),
                i64::from(toplayer),
            ];
            let ev = crate::event::Event::new(0);
            let ctx = crate::effect::Ctx {
                reason_effect: e,
                player: sumplayer,
                event: &ev,
                card: Some(card),
                args: &args,
            };
            let holds = self
                .effects
                .get(e)
                .is_some_and(|x| x.check_value_condition(self, &ctx));
            if !holds {
                return false;
            }
        }
        true
    }

    /// `card::is_setable_mzone` — may this be Set as a monster now?
    ///
    /// The same shape as the summon question with the Gemini branch removed:
    /// **only from the hand**, and against the *set* procedures and the set
    /// count permission.
    pub fn is_setable_mzone(
        &mut self,
        card: CardId,
        playerid: u8,
        ignore_count: bool,
        procedure: Option<EffectId>,
        min_tribute: u8,
        zone: u32,
    ) -> bool {
        if !self.is_summonable_card(card) {
            return false;
        }
        self.mark_summon_self(card);
        if self.cards[card].current.location != location::HAND {
            return false;
        }
        if self.cards[card].is_status(status::FORBIDDEN) {
            return false;
        }
        if self
            .is_affected_by_effect(card, code::CANNOT_MSET)
            .is_some()
        {
            return false;
        }
        if !ignore_count {
            let no_extra = self.core.extra_summon[playerid as usize]
                || self
                    .is_affected_by_effect(card, code::EXTRA_SET_COUNT)
                    .is_none();
            let limit = self.get_summon_count_limit(playerid);
            if no_extra && self.core.summon_count[playerid as usize] as i32 >= limit {
                return false;
            }
        }
        self.save_lp_cost();
        let allowed =
            self.mset_is_allowed(card, playerid, ignore_count, procedure, min_tribute, zone);
        self.restore_lp_cost();
        allowed
    }

    fn mset_is_allowed(
        &mut self,
        card: CardId,
        playerid: u8,
        ignore_count: bool,
        procedure: Option<EffectId>,
        min_tribute: u8,
        zone: u32,
    ) -> bool {
        if !self.check_cost_condition(card, code::MSET_COST, playerid) {
            return false;
        }
        let found = self.filter_set_procedure(card, playerid, ignore_count, min_tribute, zone);
        match procedure {
            Some(named) => {
                !matches!(found, Procedures::Forbidden)
                    && self.check_set_procedure(
                        card,
                        named,
                        playerid,
                        ignore_count,
                        min_tribute,
                        zone,
                    )
            }
            None => match found {
                Procedures::Forbidden => false,
                Procedures::Limited(ps) => !ps.is_empty(),
                Procedures::Ordinary {
                    procedures,
                    ordinary,
                } => !procedures.is_empty() || ordinary,
            },
        }
    }

    /// `card::is_setable_szone` — may this be Set in a Spell/Trap Zone?
    ///
    /// **A Field Spell needs no free seat**, because it replaces whatever is
    /// in the field zone rather than joining a row. A monster needs
    /// `EFFECT_MONSTER_SSET` — that is what a Trap Monster's own set is.
    pub fn is_setable_szone(&mut self, card: CardId, playerid: u8, ignore_field: bool) -> bool {
        let ty = self.cards[card].data.type_;
        if ty & card_type::FIELD == 0 && !ignore_field {
            let controller = self.cards[card].current.controller;
            if self.get_useable_count(
                Some(card),
                playerid,
                location::SZONE,
                controller,
                Self::LOCATION_REASON_TOFIELD,
                0xff,
            ) <= 0
            {
                return false;
            }
        }
        if ty & card_type::MONSTER != 0
            && self
                .is_affected_by_effect(card, code::MONSTER_SSET)
                .is_none()
        {
            return false;
        }
        if self.cards[card].is_status(status::FORBIDDEN) {
            return false;
        }
        if self
            .is_affected_by_effect(card, code::CANNOT_SSET)
            .is_some()
        {
            return false;
        }
        if !self.is_player_can_sset(playerid, card) {
            return false;
        }
        self.save_lp_cost();
        let affordable = self.check_cost_condition(card, code::SSET_COST, playerid);
        self.restore_lp_cost();
        affordable
    }

    /// `card::is_can_be_flip_summoned` — may this be Flip Summoned?
    ///
    /// **A card that was summoned this turn may not be flipped**, and the
    /// duel option that relaxes it is about *who* summoned it: with
    /// `DUEL_CAN_REPOS_IF_NON_SUMPLAYER`, a card whose summoning player is
    /// not its current controller may be. So a monster taken from the
    /// opponent the turn it was summoned can be flipped by its new
    /// controller under that option and not otherwise.
    ///
    /// `announce_count` is the battle system's, and a card that has declared
    /// an attack this turn is out regardless.
    pub fn is_can_be_flip_summoned(&mut self, card: CardId, playerid: u8) -> bool {
        if self.cards[card].is_status(status::FORM_CHANGED) {
            return false;
        }
        let summoned_this_turn = self.cards[card]
            .get_status(status::SUMMON_TURN | status::FLIP_SUMMON_TURN | status::SPSUMMON_TURN);
        if summoned_this_turn
            && (self.cards[card].summon.player == self.cards[card].current.controller
                || !self.is_flag(crate::duel::flags::CAN_REPOS_IF_NON_SUMPLAYER))
        {
            return false;
        }
        if self.cards[card].announce_count > 0 {
            return false;
        }
        if self.cards[card].current.location != location::MZONE {
            return false;
        }
        if !self.cards[card].current.is_position(position::FACEDOWN) {
            return false;
        }
        if self
            .check_unique_onfield(card, playerid, u16::from(location::MZONE), None)
            .is_some()
        {
            return false;
        }
        if !self.is_player_can_flipsummon(playerid, card) {
            return false;
        }
        if self.cards[card].is_status(status::FORBIDDEN) {
            return false;
        }
        if self
            .is_affected_by_effect(card, code::CANNOT_FLIP_SUMMON)
            .is_some()
        {
            return false;
        }
        if self
            .is_affected_by_effect(card, code::CANNOT_CHANGE_POSITION)
            .is_some()
        {
            return false;
        }
        self.save_lp_cost();
        let affordable = self.check_cost_condition(card, code::FLIPSUMMON_COST, playerid);
        self.restore_lp_cost();
        affordable
    }

    /// `card::is_capable_change_position` — may this be turned over by hand?
    ///
    /// The Main Phase's *other* repositioning question, and the one that is
    /// easy to confuse with [`Self::is_can_be_flip_summoned`]. They share
    /// their first two tests verbatim — already changed form this turn, or
    /// summoned this turn — and then diverge completely: this one asks
    /// nothing about location, position, uniqueness or cost, because a
    /// manual position change is not a summon and pays for nothing.
    ///
    /// Which of the two `IdleCommand` asks is decided by the card's current
    /// position, not by the player: a face-up monster is asked this, a
    /// face-down one is asked the flip-summon question, and a face-down
    /// **attack**-position monster is asked both and offered whichever
    /// answer yes.
    ///
    /// The Link test is a rule, not a permission: a Link monster has no
    /// defence position to change to, so there is nothing to ask. It is
    /// dead in this port's card pool, which has no Links, and transcribed
    /// because leaving it out would make the difference from the reference
    /// look deliberate.
    /// `card::is_capable_change_position_by_effect` (`card.cpp:3787`).
    ///
    /// **Not the sibling below.** That one is the *rule* question — may
    /// the player change this monster's position as their once-a-turn
    /// move — and it refuses a monster summoned this turn. This one is
    /// what an **effect** asks, and it refuses only a Link monster, which
    /// has no defence position to turn to. Reading the rule version here
    /// would have Enemy Controller refuse a monster its opponent had just
    /// summoned, which is precisely when it is played.
    ///
    /// The reference takes a `playerid` and does not use it; kept in the
    /// signature so the two read as a pair.
    pub fn is_capable_change_position_by_effect(&self, card: CardId, _playerid: u8) -> bool {
        let data = &self.cards[card].data;
        !(data.is_type(card_type::LINK) && data.is_type(card_type::MONSTER))
    }

    pub fn is_capable_change_position(&mut self, card: CardId, playerid: u8) -> bool {
        if self.cards[card].is_status(status::FORM_CHANGED) {
            return false;
        }
        let summoned_this_turn = self.cards[card]
            .get_status(status::SUMMON_TURN | status::FLIP_SUMMON_TURN | status::SPSUMMON_TURN);
        if summoned_this_turn
            && (self.cards[card].summon.player == self.cards[card].current.controller
                || !self.is_flag(crate::duel::flags::CAN_REPOS_IF_NON_SUMPLAYER))
        {
            return false;
        }
        let ty = self.cards[card].data.type_;
        if ty & card_type::LINK != 0 && ty & card_type::MONSTER != 0 {
            return false;
        }
        if self.cards[card].announce_count > 0 {
            return false;
        }
        if self.cards[card].is_status(status::FORBIDDEN) {
            return false;
        }
        if self
            .is_affected_by_effect(card, code::CANNOT_CHANGE_POSITION)
            .is_some()
        {
            return false;
        }
        self.is_player_affected_by_effect(playerid, code::CANNOT_CHANGE_POSITION)
            .is_none()
    }

    /// `card::is_special_summonable` — has this card a procedure that will
    /// summon it right now?
    ///
    /// The question `IdleCommand` asks of every `EFFECT_SPSUMMON_PROC`
    /// handler, and the answer the reference returns is **the number of
    /// procedures**, not a boolean. Nothing reads the number — every caller
    /// tests it against zero — so it is a `bool` here, and that is the only
    /// departure.
    ///
    /// The cost is asked with the LP stack saved around it, and the
    /// procedure filter runs **inside** the same save: `filter_spsummon_
    /// procedure` calls conditions of its own that may pay, and the stack
    /// has to come back level whether the answer is yes or no.
    ///
    /// Both early refusals are asked **again** downstream, inside
    /// `is_player_can_spsummon`. What the early pair buys is ordering: they
    /// sit above `save_lp_cost`, so a card that cannot be summoned never
    /// runs its summon cost.
    pub fn is_special_summonable(&mut self, card: CardId, playerid: u8, summon_type: u32) -> bool {
        if self.cards[card].data.type_ & card_type::MONSTER == 0 {
            return false;
        }
        if self
            .is_affected_by_effect(card, code::CANNOT_SPECIAL_SUMMON)
            .is_some()
        {
            return false;
        }
        if self.cards[card].is_status(status::FORBIDDEN) {
            return false;
        }
        self.save_lp_cost();
        if !self.check_cost_condition_typed(card, code::SPSUMMON_COST, playerid, summon_type) {
            self.restore_lp_cost();
            return false;
        }
        let procedures = self.filter_spsummon_procedure(card, playerid, summon_type);
        self.restore_lp_cost();
        !procedures.is_empty()
    }

    /// `card::is_capable_attack` — could this monster attack if a Battle
    /// Phase happened?
    ///
    /// Not itself a Main Phase offer. `IdleCommand` asks it only to decide
    /// whether some monster is under `EFFECT_MUST_ATTACK`, which is what
    /// takes the "go to the End Phase" option away — a player who has a
    /// monster obliged to attack may not skip the battle.
    ///
    /// Two prohibitions are **waivable** and one is not: `CANNOT_ATTACK` and
    /// `ATTACK_DISABLED` both yield to `UNSTOPPABLE_ATTACK`, while
    /// `EFFECT_FORBIDDEN` does not. Note that this reads the *effect*
    /// `EFFECT_FORBIDDEN` and not `STATUS_FORBIDDEN`, unlike every other
    /// predicate in this file.
    ///
    /// The final test is about the **turn player**, not about this card's
    /// controller: a player whose Battle Phase is skipped has no attacks,
    /// and it is their skip that is read even when the monster is the
    /// opponent's.
    pub fn is_capable_attack(&mut self, card: CardId) -> bool {
        let attack_position = self.cards[card]
            .current
            .is_position(position::FACEUP_ATTACK);
        let defending_but_allowed = self.cards[card]
            .current
            .is_position(position::FACEUP_DEFENSE)
            && self
                .is_affected_by_effect(card, code::DEFENSE_ATTACK)
                .is_some();
        if !attack_position && !defending_but_allowed {
            return false;
        }
        if self.is_affected_by_effect(card, code::FORBIDDEN).is_some() {
            return false;
        }
        let unstoppable = self
            .is_affected_by_effect(card, code::UNSTOPPABLE_ATTACK)
            .is_some();
        if self
            .is_affected_by_effect(card, code::CANNOT_ATTACK)
            .is_some()
            && !unstoppable
        {
            return false;
        }
        if self
            .is_affected_by_effect(card, code::ATTACK_DISABLED)
            .is_some()
            && !unstoppable
        {
            return false;
        }
        let tp = self.infos.turn_player;
        self.is_player_affected_by_effect(tp, code::SKIP_BP)
            .is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{Card, CardData};
    use crate::effect::{effect_type, flag, Effect};
    use crate::event::PLAYER_NONE;

    fn card_at(f: &mut Field, player: u8, loc: u8, seat: u32, type_: u32, level: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 18036057 + seat,
                type_,
                level,
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
        let seat = f.players[0].hand.len() as u32;
        card_at(f, 0, location::HAND, seat, card_type::MONSTER, 4)
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

    /// A cost that refuses, of the given kind.
    fn refusing_cost(f: &mut Field, card: CardId, code_: u32) {
        let e = single(f, card, code_);
        if let Some(x) = f.effects.get_mut(e) {
            x.cost = Some(|_, _, _| false);
        }
    }

    mod summoning {
        use super::*;

        #[test]
        fn an_ordinary_monster_in_hand_may_be_summoned() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f);
            assert!(f.is_can_be_summoned(c, 0, false, None, 0, 0x1f));
        }

        /// A card that is not summonable at all, and one that is forbidden.
        #[test]
        fn the_card_itself_can_refuse() {
            let mut f = Field::new(8000);
            let spell = card_at(&mut f, 0, location::HAND, 0, card_type::SPELL, 0);
            assert!(!f.is_can_be_summoned(spell, 0, false, None, 0, 0x1f));

            let c = in_hand(&mut f);
            f.cards[c].set_status(status::FORBIDDEN, true);
            assert!(!f.is_can_be_summoned(c, 0, false, None, 0, 0x1f));
        }

        /// **The summon count is spent**, and `ignore_count` waives it.
        #[test]
        fn a_spent_summon_refuses_unless_ignored() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f);
            f.core.summon_count[0] = 1;
            assert!(!f.is_can_be_summoned(c, 0, false, None, 0, 0x1f));
            assert!(f.is_can_be_summoned(c, 0, true, None, 0, 0x1f), "ignored");
        }

        /// An extra-summon permission gets past the spent count.
        #[test]
        fn an_extra_summon_permission_gets_past_it() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f);
            single(&mut f, c, code::EXTRA_SUMMON_COUNT);
            f.core.summon_count[0] = 1;
            assert!(f.is_can_be_summoned(c, 0, false, None, 0, 0x1f));
            f.core.extra_summon[0] = true;
            assert!(
                !f.is_can_be_summoned(c, 0, false, None, 0, 0x1f),
                "but only once"
            );
        }

        /// **The cost is asked and not paid** — the LP cost stack comes back
        /// level either way.
        #[test]
        fn a_refusing_cost_refuses_and_leaves_the_stack_level() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f);
            refusing_cost(&mut f, c, code::SUMMON_COST);
            let before = f.core.lp_cost[0].count;
            assert!(!f.is_can_be_summoned(c, 0, false, None, 0, 0x1f));
            assert_eq!(f.core.lp_cost[0].count, before);
        }

        /// A level-7 monster with no tributes available has no procedure, so
        /// it is refused.
        #[test]
        fn a_high_level_monster_with_no_tributes_is_refused() {
            let mut f = Field::new(8000);
            let c = card_at(&mut f, 0, location::HAND, 0, card_type::MONSTER, 7);
            assert!(!f.is_can_be_summoned(c, 0, false, None, 0, 0x1f));
            card_at(&mut f, 0, location::MZONE, 0, card_type::MONSTER, 4);
            card_at(&mut f, 0, location::MZONE, 1, card_type::MONSTER, 4);
            assert!(
                f.is_can_be_summoned(c, 0, false, None, 0, 0x1f),
                "with tributes it can"
            );
        }

        /// **In a Monster Zone the question is about a Gemini summon.**
        #[test]
        fn a_card_on_the_field_is_asked_the_gemini_question() {
            let mut f = Field::new(8000);
            let c = card_at(&mut f, 0, location::MZONE, 0, card_type::MONSTER, 4);
            assert!(
                !f.is_can_be_summoned(c, 0, false, None, 0, 0x1f),
                "an ordinary monster on the field cannot"
            );
            single(&mut f, c, code::GEMINI_SUMMONABLE);
            assert!(f.is_can_be_summoned(c, 0, false, None, 0, 0x1f));

            // **Face-down is refused**, Gemini or not: the summon it would
            // be is a second one of a monster already face-up.
            f.cards[c].current.position = position::FACEDOWN_DEFENSE;
            assert!(!f.is_can_be_summoned(c, 0, false, None, 0, 0x1f));
            f.cards[c].current.position = position::FACEUP_ATTACK;

            single(&mut f, c, code::GEMINI_STATUS);
            assert!(
                !f.is_can_be_summoned(c, 0, false, None, 0, 0x1f),
                "and not twice"
            );
        }

        /// **`EFFECT_STATUS_SUMMON_SELF` is set by the question**, when the
        /// effect being resolved belongs to the card being asked about.
        #[test]
        fn asking_marks_an_effect_that_summons_its_own_card() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f);
            let e = single(&mut f, c, 0);
            f.core.reason_effect = Some(e);
            assert_eq!(
                f.effects
                    .get(e)
                    .map(|x| x.status & effect_status::SUMMON_SELF),
                Some(0)
            );
            f.is_can_be_summoned(c, 0, false, None, 0, 0x1f);
            assert_ne!(
                f.effects
                    .get(e)
                    .map(|x| x.status & effect_status::SUMMON_SELF),
                Some(0),
                "the effect summons its own card"
            );
        }

        /// And not when it belongs to another card.
        #[test]
        fn another_cards_effect_is_not_marked() {
            let mut f = Field::new(8000);
            let other = card_at(&mut f, 0, location::MZONE, 0, card_type::MONSTER, 4);
            let c = in_hand(&mut f);
            let e = single(&mut f, other, 0);
            f.core.reason_effect = Some(e);
            f.is_can_be_summoned(c, 0, false, None, 0, 0x1f);
            assert_eq!(
                f.effects
                    .get(e)
                    .map(|x| x.status & effect_status::SUMMON_SELF),
                Some(0)
            );
        }
    }

    mod setting_a_monster {
        use super::*;

        #[test]
        fn an_ordinary_monster_in_hand_may_be_set() {
            let mut f = Field::new(8000);
            let c = in_hand(&mut f);
            assert!(f.is_setable_mzone(c, 0, false, None, 0, 0x1f));
        }

        /// **Only from the hand** — unlike the summon question, there is no
        /// second branch.
        #[test]
        fn a_card_on_the_field_may_not_be_set() {
            let mut f = Field::new(8000);
            let c = card_at(&mut f, 0, location::MZONE, 0, card_type::MONSTER, 4);
            assert!(!f.is_setable_mzone(c, 0, false, None, 0, 0x1f));
        }

        /// **It reads the *set* count permission, not the summon one.**
        ///
        /// Two cards rather than one: a card carrying both would be allowed
        /// whichever permission the check reads, and the point is which.
        #[test]
        fn it_reads_the_set_count_permission() {
            let mut f = Field::new(8000);
            f.core.summon_count[0] = 1;

            let wrong = in_hand(&mut f);
            single(&mut f, wrong, code::EXTRA_SUMMON_COUNT);
            assert!(
                !f.is_setable_mzone(wrong, 0, false, None, 0, 0x1f),
                "the summon permission does not buy a set"
            );

            let right = in_hand(&mut f);
            single(&mut f, right, code::EXTRA_SET_COUNT);
            assert!(f.is_setable_mzone(right, 0, false, None, 0, 0x1f));
        }

        #[test]
        fn a_prohibition_and_a_refusing_cost_each_refuse() {
            let mut f = Field::new(8000);
            let a = in_hand(&mut f);
            single(&mut f, a, code::CANNOT_MSET);
            assert!(!f.is_setable_mzone(a, 0, false, None, 0, 0x1f));

            let b = in_hand(&mut f);
            refusing_cost(&mut f, b, code::MSET_COST);
            assert!(!f.is_setable_mzone(b, 0, false, None, 0, 0x1f));
        }
    }

    mod setting_a_spell {
        use super::*;

        #[test]
        fn a_spell_may_be_set_with_room() {
            let mut f = Field::new(8000);
            let c = card_at(&mut f, 0, location::HAND, 0, card_type::SPELL, 0);
            assert!(f.is_setable_szone(c, 0, false));
        }

        /// **A full spell/trap row refuses — but a Field Spell does not need
        /// a seat in it.**
        #[test]
        fn a_field_spell_needs_no_seat() {
            let mut f = Field::new(8000);
            for seat in 0..5 {
                card_at(&mut f, 0, location::SZONE, seat, card_type::SPELL, 0);
            }
            let ordinary = card_at(&mut f, 0, location::HAND, 0, card_type::SPELL, 0);
            assert!(!f.is_setable_szone(ordinary, 0, false));

            let field = card_at(
                &mut f,
                0,
                location::HAND,
                1,
                card_type::SPELL | card_type::FIELD,
                0,
            );
            assert!(f.is_setable_szone(field, 0, false));
            assert!(
                f.is_setable_szone(ordinary, 0, true),
                "and the caller can waive the seat check"
            );
        }

        /// **A monster needs `EFFECT_MONSTER_SSET`** — which is what a Trap
        /// Monster's own set is.
        #[test]
        fn a_monster_needs_permission_to_be_set_there() {
            let mut f = Field::new(8000);
            let c = card_at(&mut f, 0, location::HAND, 0, card_type::MONSTER, 4);
            assert!(!f.is_setable_szone(c, 0, false));
            single(&mut f, c, code::MONSTER_SSET);
            assert!(f.is_setable_szone(c, 0, false));
        }

        /// The player-level prohibition, in both polarities.
        #[test]
        fn the_player_prohibition_refuses_both_ways() {
            let mut f = Field::new(8000);
            let anchor = card_at(&mut f, 0, location::MZONE, 0, card_type::MONSTER, 4);
            let mut e = Effect::new(effect_type::FIELD, code::CANNOT_SSET);
            e.owner = Some(anchor);
            e.handler = Some(anchor);
            e.flag[0] = flag::PLAYER_TARGET | flag::ABSOLUTE_TARGET;
            e.range = u16::from(location::MZONE);
            e.s_range = 1;
            let id = f.new_effect(e);
            f.add_effect(id, 0);

            let c = card_at(&mut f, 0, location::HAND, 0, card_type::SPELL, 0);
            assert!(!f.is_setable_szone(c, 0, false), "no target: unconditional");

            if let Some(x) = f.effects.get_mut(id) {
                x.target_filter = Some(|_, _, _, _| false);
            }
            assert!(f.is_setable_szone(c, 0, false), "the target declined");

            if let Some(x) = f.effects.get_mut(id) {
                x.target_filter = Some(|_, _, _, _| true);
            }
            assert!(!f.is_setable_szone(c, 0, false), "and agreeing refuses");
        }
    }

    mod flipping {
        use super::*;

        fn face_down(f: &mut Field, seat: u32) -> CardId {
            let c = card_at(f, 0, location::MZONE, seat, card_type::MONSTER, 4);
            f.cards[c].current.position = position::FACEDOWN_DEFENSE;
            c
        }

        #[test]
        fn a_face_down_monster_may_be_flipped() {
            let mut f = Field::new(8000);
            let c = face_down(&mut f, 0);
            assert!(f.is_can_be_flip_summoned(c, 0));
        }

        #[test]
        fn a_face_up_one_may_not() {
            let mut f = Field::new(8000);
            let c = card_at(&mut f, 0, location::MZONE, 0, card_type::MONSTER, 4);
            assert!(!f.is_can_be_flip_summoned(c, 0));
        }

        /// **Not the turn it arrived** — and the duel option that relaxes it
        /// is about *who* summoned it, not about time.
        #[test]
        fn not_the_turn_it_arrived() {
            let mut f = Field::new(8000);
            let c = face_down(&mut f, 0);
            f.cards[c].set_status(status::SUMMON_TURN, true);
            f.cards[c].summon.player = 0;
            assert!(!f.is_can_be_flip_summoned(c, 0));

            // Summoned by the other player, under the option that cares.
            let mut g = Field::with_flags(
                8000,
                crate::duel::REFERENCE_CONFIGURATION
                    | crate::duel::flags::CAN_REPOS_IF_NON_SUMPLAYER,
            );
            let c = face_down(&mut g, 0);
            g.cards[c].set_status(status::SUMMON_TURN, true);
            g.cards[c].summon.player = 1;
            assert!(
                g.is_can_be_flip_summoned(c, 0),
                "its summoning player is not its controller"
            );
        }

        /// A card that has declared an attack is out.
        #[test]
        fn a_card_that_attacked_is_out() {
            let mut f = Field::new(8000);
            let c = face_down(&mut f, 0);
            f.cards[c].announce_count = 1;
            assert!(!f.is_can_be_flip_summoned(c, 0));
        }

        /// Each of the three prohibitions refuses on its own.
        #[test]
        fn the_three_prohibitions_each_refuse() {
            for code_ in [
                code::CANNOT_FLIP_SUMMON,
                code::CANNOT_CHANGE_POSITION,
                code::FLIPSUMMON_COST,
            ] {
                let mut f = Field::new(8000);
                let c = face_down(&mut f, 0);
                if code_ == code::FLIPSUMMON_COST {
                    refusing_cost(&mut f, c, code_);
                } else {
                    single(&mut f, c, code_);
                }
                assert!(!f.is_can_be_flip_summoned(c, 0), "code {code_}");
            }
        }

        /// A form change this turn blocks it too.
        #[test]
        fn a_form_change_this_turn_blocks_it() {
            let mut f = Field::new(8000);
            let c = face_down(&mut f, 0);
            f.cards[c].set_status(status::FORM_CHANGED, true);
            assert!(!f.is_can_be_flip_summoned(c, 0));
        }
    }

    mod special_summoning {
        use super::*;

        fn ask(f: &mut Field, card: CardId, by: EffectId, pos: u8) -> bool {
            f.is_can_be_special_summoned(card, Some(by), 0, pos, 0, 0, false, false, 0xff)
        }

        fn source(f: &mut Field) -> EffectId {
            let anchor = card_at(f, 0, location::MZONE, 4, card_type::MONSTER, 4);
            single(f, anchor, 0)
        }

        #[test]
        fn a_monster_in_the_graveyard_may_be_summoned() {
            let mut f = Field::new(8000);
            let by = source(&mut f);
            let c = card_at(&mut f, 0, location::GRAVE, 0, card_type::MONSTER, 4);
            assert!(ask(&mut f, c, by, position::FACEUP_ATTACK));
        }

        /// **A card already in a Monster Zone may not.**
        #[test]
        fn a_card_already_on_the_field_may_not() {
            let mut f = Field::new(8000);
            let by = source(&mut f);
            let c = card_at(&mut f, 0, location::MZONE, 0, card_type::MONSTER, 4);
            assert!(!ask(&mut f, c, by, position::FACEUP_ATTACK));
        }

        /// **The revive limit, and the two waivers.** From the graveyard
        /// only `nolimit` opens it; from the hand either does.
        #[test]
        fn the_revive_limit_and_its_two_waivers() {
            for (loc, nocheck_opens) in [(location::GRAVE, false), (location::HAND, true)] {
                let mut f = Field::new(8000);
                let by = source(&mut f);
                let c = card_at(&mut f, 0, loc, 0, card_type::MONSTER, 4);
                single(&mut f, c, code::REVIVE_LIMIT);
                let plain = f.is_can_be_special_summoned(
                    c,
                    Some(by),
                    0,
                    position::FACEUP_ATTACK,
                    0,
                    0,
                    false,
                    false,
                    0xff,
                );
                assert!(!plain, "loc {loc:#x}");
                let with_nolimit = f.is_can_be_special_summoned(
                    c,
                    Some(by),
                    0,
                    position::FACEUP_ATTACK,
                    0,
                    0,
                    false,
                    true,
                    0xff,
                );
                assert!(with_nolimit, "nolimit always opens it");
                let with_nocheck = f.is_can_be_special_summoned(
                    c,
                    Some(by),
                    0,
                    position::FACEUP_ATTACK,
                    0,
                    0,
                    true,
                    false,
                    0xff,
                );
                assert_eq!(with_nocheck, nocheck_opens, "loc {loc:#x}");
            }
        }

        /// A named zone with no room refuses; `0xff` names none.
        #[test]
        fn a_named_zone_is_checked_for_room() {
            let mut f = Field::new(8000);
            let by = source(&mut f);
            for seat in 0..4 {
                card_at(&mut f, 0, location::MZONE, seat, card_type::MONSTER, 4);
            }
            let c = card_at(&mut f, 0, location::GRAVE, 0, card_type::MONSTER, 4);
            assert!(
                !f.is_can_be_special_summoned(
                    c,
                    Some(by),
                    0,
                    position::FACEUP_ATTACK,
                    0,
                    0,
                    false,
                    false,
                    0x1
                ),
                "seat 0 is taken"
            );
            assert!(
                ask(&mut f, c, by, position::FACEUP_ATTACK),
                "0xff names none"
            );
        }

        /// A non-monster is refused unless the checks are waived.
        #[test]
        fn a_non_monster_needs_nocheck() {
            let mut f = Field::new(8000);
            let by = source(&mut f);
            let c = card_at(&mut f, 0, location::GRAVE, 0, card_type::SPELL, 0);
            assert!(!ask(&mut f, c, by, position::FACEUP_ATTACK));
            assert!(f.is_can_be_special_summoned(
                c,
                Some(by),
                0,
                position::FACEUP_ATTACK,
                0,
                0,
                true,
                false,
                0xff
            ));
        }

        /// **A face-up summon of a name already face-up loses `POS_FACEUP`,
        /// and is refused when nothing is left.**
        #[test]
        fn a_duplicate_name_strikes_the_face_up_positions() {
            let mut f = Field::new(8000);
            let by = source(&mut f);
            let onfield = card_at(&mut f, 0, location::MZONE, 0, card_type::MONSTER, 4);
            f.cards[onfield].unique_code = f.cards[onfield].code;
            f.cards[onfield].unique_location = u16::from(location::MZONE);
            f.cards[onfield].unique_pos = [1, 0];
            f.cards[onfield].unique_fieldid = 1;
            let mut ue = Effect::new(effect_type::SINGLE, 0);
            ue.owner = Some(onfield);
            ue.handler = Some(onfield);
            let ue = f.new_effect(ue);
            f.cards[onfield].unique_effect = Some(ue);
            f.core.unique_cards[0].push(onfield);

            let c = card_at(&mut f, 0, location::GRAVE, 0, card_type::MONSTER, 4);
            f.cards[c].code = f.cards[onfield].code;
            assert!(
                !ask(&mut f, c, by, position::FACEUP_ATTACK),
                "nothing left of the mask"
            );
            assert!(
                ask(&mut f, c, by, position::FACEUP | position::FACEDOWN_DEFENSE),
                "but a face-down position survives"
            );
        }
    }

    /// `PLAYER_NONE` as the summoning player skips the permission layer
    /// entirely — the question is being asked about nobody in particular.
    #[test]
    fn a_playerless_question_skips_the_permission_layer() {
        let mut f = Field::new(8000);
        let anchor = card_at(&mut f, 0, location::MZONE, 4, card_type::MONSTER, 4);
        let by = single(&mut f, anchor, 0);
        let c = card_at(&mut f, 0, location::GRAVE, 0, card_type::MONSTER, 4);
        single(&mut f, c, code::CANNOT_SPECIAL_SUMMON);
        assert!(
            !f.is_can_be_special_summoned(
                c,
                Some(by),
                0,
                position::FACEUP_ATTACK,
                0,
                0,
                false,
                false,
                0xff
            ),
            "asked for a player, the prohibition is read"
        );
        assert!(
            !f.is_can_be_special_summoned(
                c,
                Some(by),
                0,
                position::FACEUP_ATTACK,
                PLAYER_NONE,
                0,
                false,
                false,
                0xff
            ),
            "and the card's own prohibition still refuses"
        );
    }
    mod repositioning {
        use super::*;

        fn faceup(f: &mut Field) -> CardId {
            card_at(f, 0, location::MZONE, 0, card_type::MONSTER, 4)
        }

        #[test]
        fn a_plain_face_up_monster_may_be_turned_over() {
            let mut f = Field::new(8000);
            let c = faceup(&mut f);
            assert!(f.is_capable_change_position(c, 0));
        }

        /// **Once per turn.** `STATUS_FORM_CHANGED` is what stops a player
        /// flipping a monster back and forth.
        #[test]
        fn a_card_already_turned_this_turn_may_not() {
            let mut f = Field::new(8000);
            let c = faceup(&mut f);
            f.cards[c].set_status(status::FORM_CHANGED, true);
            assert!(!f.is_capable_change_position(c, 0));
        }

        /// **Summoned this turn, and the duel option about who summoned
        /// it.** All three summon statuses refuse; under
        /// `CAN_REPOS_IF_NON_SUMPLAYER` a card whose summoning player is
        /// not its controller may still be turned.
        #[test]
        fn summoned_this_turn_refuses_unless_someone_else_summoned_it() {
            for st in [
                status::SUMMON_TURN,
                status::FLIP_SUMMON_TURN,
                status::SPSUMMON_TURN,
            ] {
                let mut f = Field::new(8000);
                let c = faceup(&mut f);
                f.cards[c].set_status(st, true);
                f.cards[c].summon.player = 0;
                assert!(!f.is_capable_change_position(c, 0), "own summon");

                f.cards[c].summon.player = 1;
                assert!(
                    !f.is_capable_change_position(c, 0),
                    "still refused without the duel option"
                );
                f.flags |= crate::duel::flags::CAN_REPOS_IF_NON_SUMPLAYER;
                assert!(
                    f.is_capable_change_position(c, 0),
                    "with the option, allowed"
                );
            }
        }

        /// **A card that has declared an attack this turn may not.** This
        /// is the battle system's counter, read by a Main Phase question.
        #[test]
        fn a_card_that_attacked_may_not() {
            let mut f = Field::new(8000);
            let c = faceup(&mut f);
            f.cards[c].announce_count = 1;
            assert!(!f.is_capable_change_position(c, 0));
        }

        /// A Link monster has no defence position, so the question does not
        /// arise. Both bits are needed — a Link *Spell* is not a thing, but
        /// the reference tests both and so does this.
        #[test]
        fn a_link_monster_may_not_and_both_bits_are_required() {
            let mut f = Field::new(8000);
            let c = card_at(
                &mut f,
                0,
                location::MZONE,
                0,
                card_type::MONSTER | card_type::LINK,
                4,
            );
            assert!(!f.is_capable_change_position(c, 0));

            let mut f = Field::new(8000);
            let c = card_at(&mut f, 0, location::MZONE, 0, card_type::LINK, 4);
            assert!(
                f.is_capable_change_position(c, 0),
                "LINK without MONSTER does not trigger the rule"
            );
        }

        /// The prohibition is read **twice**: on the card and on the
        /// player, and either refuses.
        #[test]
        fn the_prohibition_is_read_on_the_card_and_on_the_player() {
            let mut f = Field::new(8000);
            let c = faceup(&mut f);
            single(&mut f, c, code::CANNOT_CHANGE_POSITION);
            assert!(!f.is_capable_change_position(c, 0), "on the card");

            let mut f = Field::new(8000);
            let c = faceup(&mut f);
            let anchor = card_at(&mut f, 0, location::MZONE, 1, card_type::MONSTER, 4);
            let mut e = Effect::new(effect_type::FIELD, code::CANNOT_CHANGE_POSITION);
            e.owner = Some(anchor);
            e.handler = Some(anchor);
            e.flag[0] = flag::PLAYER_TARGET | flag::ABSOLUTE_TARGET;
            e.range = u16::from(location::MZONE);
            e.s_range = 1;
            let id = f.new_effect(e);
            f.add_effect(id, 0);
            assert!(!f.is_capable_change_position(c, 0), "on the player");
        }

        /// A forbidden card may not.
        #[test]
        fn a_forbidden_card_may_not() {
            let mut f = Field::new(8000);
            let c = faceup(&mut f);
            f.cards[c].set_status(status::FORBIDDEN, true);
            assert!(!f.is_capable_change_position(c, 0));
        }
    }

    mod is_special_summonable {
        use super::*;

        /// A card that **would** be specially summonable: a monster in the
        /// hand with a working `EFFECT_SPSUMMON_PROC`.
        ///
        /// Every refusal below starts from this, because a card with no
        /// procedure is refused anyway — a refusal test built on one passes
        /// whether or not the condition under test is read at all.
        fn summonable(f: &mut Field, type_: u32) -> CardId {
            let c = card_at(f, 0, location::HAND, 0, type_, 4);
            let mut e = Effect::new(effect_type::FIELD, code::SPSUMMON_PROC);
            e.owner = Some(c);
            e.handler = Some(c);
            e.range = u16::from(location::HAND);
            e.condition = Some(|_, _| true);
            let id = f.new_effect(e);
            f.cards[c].field_effect.insert(code::SPSUMMON_PROC, id);
            f.cards[c].indexer.insert(id);
            c
        }

        /// A procedure on the card makes it specially summonable; without
        /// one it is not, however legal it otherwise looks.
        #[test]
        fn it_is_the_procedure_that_decides() {
            let mut f = Field::new(8000);
            let c = card_at(&mut f, 0, location::HAND, 0, card_type::MONSTER, 4);
            assert!(!f.is_special_summonable(c, 0, 0), "no procedure");

            let mut f = Field::new(8000);
            let c = summonable(&mut f, card_type::MONSTER);
            assert!(f.is_special_summonable(c, 0, 0));
        }

        /// A non-monster is refused before anything else is asked — and it
        /// carries a procedure, so nothing else *could* be refusing it.
        #[test]
        fn a_non_monster_is_refused() {
            let mut f = Field::new(8000);
            let c = summonable(&mut f, card_type::SPELL);
            assert!(!f.is_special_summonable(c, 0, 0));
        }

        /// A standing prohibition and a forbidden card both refuse — **and
        /// the prohibition refuses before the cost is asked**.
        ///
        /// The ordering half is what makes this test worth anything for
        /// `CANNOT_SPECIAL_SUMMON`: it is asked again downstream, inside
        /// `is_player_can_spsummon`, so removing the early check still
        /// yields `false`. What the early check buys is that a card which
        /// cannot be summoned never runs its summon cost.
        ///
        /// The `STATUS_FORBIDDEN` half of this cannot be made
        /// discriminating, and the assertion is kept as documentation
        /// rather than as a check: a forbidden card's own effects are
        /// already filtered out by the effect machinery, so its cost is
        /// unreachable whether the early test is there or not.
        #[test]
        fn a_prohibition_and_a_forbidden_card_refuse_before_the_cost() {
            use std::sync::atomic::{AtomicU32, Ordering};
            static ASKED: AtomicU32 = AtomicU32::new(0);

            fn counting_cost(f: &mut Field, card: CardId) {
                let e = single(f, card, code::SPSUMMON_COST);
                if let Some(x) = f.effects.get_mut(e) {
                    x.cost = Some(|_, _, _| {
                        ASKED.fetch_add(1, Ordering::SeqCst);
                        true
                    });
                }
            }

            // The baseline: with neither refusal, the cost *is* asked —
            // twice, once here and once inside the procedure filter's own
            // `is_player_can_spsummon`. The reference asks it twice too;
            // what this test needs is only that it is not zero.
            let mut f = Field::new(8000);
            let c = summonable(&mut f, card_type::MONSTER);
            counting_cost(&mut f, c);
            ASKED.store(0, Ordering::SeqCst);
            assert!(f.is_special_summonable(c, 0, 0));
            assert_eq!(
                ASKED.load(Ordering::SeqCst),
                2,
                "asked when nothing refuses"
            );

            let mut f = Field::new(8000);
            let c = summonable(&mut f, card_type::MONSTER);
            counting_cost(&mut f, c);
            single(&mut f, c, code::CANNOT_SPECIAL_SUMMON);
            ASKED.store(0, Ordering::SeqCst);
            assert!(!f.is_special_summonable(c, 0, 0));
            assert_eq!(
                ASKED.load(Ordering::SeqCst),
                0,
                "the prohibition refuses above the cost"
            );

            let mut f = Field::new(8000);
            let c = summonable(&mut f, card_type::MONSTER);
            counting_cost(&mut f, c);
            f.cards[c].set_status(status::FORBIDDEN, true);
            ASKED.store(0, Ordering::SeqCst);
            assert!(!f.is_special_summonable(c, 0, 0));
            assert_eq!(
                ASKED.load(Ordering::SeqCst),
                0,
                "and so does the forbidden status"
            );
        }

        /// **A refusing cost refuses, and the LP stack comes back level
        /// either way** — the save wraps the procedure filter too, not just
        /// the cost.
        #[test]
        fn a_refusing_cost_refuses_and_leaves_the_stack_level() {
            let mut f = Field::new(8000);
            let c = summonable(&mut f, card_type::MONSTER);
            refusing_cost(&mut f, c, code::SPSUMMON_COST);
            let before = f.core.lp_cost[0].count;
            assert!(!f.is_special_summonable(c, 0, 0));
            assert_eq!(f.core.lp_cost[0].count, before, "refused");

            let mut f = Field::new(8000);
            let c = summonable(&mut f, card_type::MONSTER);
            let before = f.core.lp_cost[0].count;
            assert!(f.is_special_summonable(c, 0, 0));
            assert_eq!(f.core.lp_cost[0].count, before, "allowed past the cost");
        }
    }

    mod is_capable_attack {
        use super::*;

        fn attacker(f: &mut Field) -> CardId {
            card_at(f, 0, location::MZONE, 0, card_type::MONSTER, 4)
        }

        /// Face-up attack position is the ordinary yes; face-down is no.
        #[test]
        fn position_decides_first() {
            let mut f = Field::new(8000);
            let c = attacker(&mut f);
            assert!(f.is_capable_attack(c));

            f.cards[c].current.position = position::FACEDOWN_DEFENSE;
            assert!(!f.is_capable_attack(c));
        }

        /// **Defence position attacks only with `EFFECT_DEFENSE_ATTACK`.**
        #[test]
        fn defence_position_needs_permission() {
            let mut f = Field::new(8000);
            let c = attacker(&mut f);
            f.cards[c].current.position = position::FACEUP_DEFENSE;
            assert!(!f.is_capable_attack(c));

            single(&mut f, c, code::DEFENSE_ATTACK);
            assert!(f.is_capable_attack(c));
        }

        /// **Two prohibitions yield to `UNSTOPPABLE_ATTACK` and one does
        /// not.** That asymmetry is the whole of this predicate's subtlety.
        #[test]
        fn unstoppable_waives_two_of_the_three_refusals() {
            for (code_, waivable) in [
                (code::CANNOT_ATTACK, true),
                (code::ATTACK_DISABLED, true),
                (code::FORBIDDEN, false),
            ] {
                let mut f = Field::new(8000);
                let c = attacker(&mut f);
                single(&mut f, c, code_);
                assert!(!f.is_capable_attack(c), "{code_} refuses");

                single(&mut f, c, code::UNSTOPPABLE_ATTACK);
                assert_eq!(
                    f.is_capable_attack(c),
                    waivable,
                    "{code_} against UNSTOPPABLE_ATTACK"
                );
            }
        }

        /// **The skip is read for the turn player, not for the card's
        /// controller.** Player 1's monster is refused because player 0 —
        /// whose turn it is — has no Battle Phase.
        #[test]
        fn the_battle_phase_skip_is_the_turn_players() {
            let mut f = Field::new(8000);
            let c = card_at(&mut f, 1, location::MZONE, 0, card_type::MONSTER, 4);
            f.infos.turn_player = 0;
            assert!(f.is_capable_attack(c));

            let anchor = card_at(&mut f, 0, location::MZONE, 1, card_type::MONSTER, 4);
            let mut e = Effect::new(effect_type::FIELD, code::SKIP_BP);
            e.owner = Some(anchor);
            e.handler = Some(anchor);
            e.flag[0] = flag::PLAYER_TARGET | flag::ABSOLUTE_TARGET;
            e.range = u16::from(location::MZONE);
            e.s_range = 1;
            let id = f.new_effect(e);
            f.add_effect(id, 0);
            assert!(
                !f.is_capable_attack(c),
                "player 0's skip stops player 1's monster, because it is player 0's turn"
            );
        }
    }
}

//! Putting a card *onto* the field: `move_to_field` and `MoveToField`.
//!
//! The counterpart to [`crate::send_to`], and the division is strict:
//! `send_to` refuses a field destination and this refuses everything else.
//! Between them they are every way a card changes place.
//!
//! Six steps, but only three do anything — 0 chooses the seat, 1 chooses the
//! position, 2 performs the move; 3 and 4 are the two exits that report
//! success and failure.
//!
//! ## The symbolic destinations are normalised away before the unit runs
//!
//! `move_to_field` accepts `LOCATION_MMZONE`, `EMZONE`, `STZONE`, `PZONE`
//! and `FZONE` — none of which is a place a card is stored. Each is rewritten
//! into a real location **plus a zone mask**:
//!
//! | asked for | becomes | with zone |
//! |---|---|---|
//! | `PZONE` | `SZONE` | (the `pzone` flag) |
//! | `FZONE` | `SZONE` | `1 << 5` |
//! | `MMZONE` | `MZONE` | seats 0-4 |
//! | `STZONE` | `SZONE` | seats 0-4 |
//! | `EMZONE` | `MZONE` | seats 5-6 |
//!
//! So the unit itself only ever sees `MZONE` or `SZONE`. A port that kept
//! the symbolic values would have to re-derive the mask at every use.
//!
//! ## `to_field_param` packs four values into one word
//!
//! `(move_player << 24) | (playerid << 16) | (location << 8) | positions`,
//! written by the queueing function and unpacked at the top of every step.
//! The two players are **different questions**: `playerid` is whose side of
//! the field the card is going to, `move_player` is who is being asked to
//! choose the seat. They differ whenever an effect places a card on the
//! opponent's field.
//!
//! ## `ret` is three-valued and each value is a different operation
//!
//! | `ret` | |
//! |---|---|
//! | 0 | an ordinary move to the field |
//! | 1 | a card **returning** from a temporary absence |
//! | 2 | a trap monster going back to its spell/trap seat |
//!
//! `ret == 1` is the one with teeth: it aborts unless the card really did
//! leave temporarily *and by the same effect that is bringing it back*, and
//! if there is no room it sends the card to the graveyard instead of simply
//! failing.

use crate::board::{location, position};
use crate::card::{card_type, reason, status};
use crate::event::{code, CardId, PLAYER_NONE};
use crate::field::{reset, Field, Message};
use crate::host_question::hint;
use crate::processor::Kind;

impl Field {
    /// `field::move_to_field` — queue a card to be placed on the field.
    ///
    /// Three refusals before anything is queued, and the third is the one
    /// that reads oddly: **a card already at the destination under the same
    /// controller does not move.** Not an error and not a no-op unit — no
    /// unit at all, so the caller's `returns` keeps whatever was there.
    #[allow(clippy::too_many_arguments)]
    pub fn move_to_field(
        &mut self,
        target: CardId,
        move_player: u8,
        playerid: u8,
        destination: u16,
        positions: u8,
        enable: bool,
        ret: u8,
        zone: u32,
        rule: bool,
        location_reason: u8,
        confirm: bool,
    ) {
        const PLACEABLE: u16 = location::MZONE as u16
            | location::MMZONE
            | location::EMZONE
            | location::SZONE as u16
            | location::STZONE
            | location::PZONE
            | location::FZONE;
        if destination & PLACEABLE == 0 || positions == 0 {
            return;
        }
        if destination & location::PZONE != 0
            && self.cards[target].current.is_location(location::PZONE)
            && playerid == self.cards[target].current.controller
        {
            return;
        }
        if destination == u16::from(self.cards[target].current.location)
            && playerid == self.cards[target].current.controller
        {
            return;
        }

        // Normalise the symbolic destinations into a real location plus a
        // seat mask. See the module note.
        let (dest, zone, pzone) = match destination {
            d if d == location::PZONE => (location::SZONE, zone, true),
            d if d == location::FZONE => (location::SZONE, 0x1 << 5, false),
            d if d == location::MMZONE => (location::MZONE, 0x1f, false),
            d if d == location::STZONE => (location::SZONE, 0x1f, false),
            d if d == location::EMZONE => (location::MZONE, (0x1 << 5) | (0x1 << 6), false),
            d => ((d & 0xff) as u8, zone, false),
        };
        self.cards[target].to_field_param = (u32::from(move_player) << 24)
            | (u32::from(playerid) << 16)
            | (u32::from(dest) << 8)
            | u32::from(positions);
        self.emplace(Kind::MoveToField {
            target,
            enable,
            ret,
            pzone,
            zone,
            rule,
            location_reason,
            confirm,
        });
    }

    /// `field::get_field_card` — what is sitting in a seat, if anything.
    pub fn get_field_card(&self, playerid: u8, loc: u8, sequence: u32) -> Option<CardId> {
        let p = &self.players[playerid as usize];
        let slots = match loc {
            l if l == location::MZONE => &p.mzone,
            l if l == location::SZONE => &p.szone,
            _ => return None,
        };
        slots.get(sequence as usize).copied().flatten()
    }
}

impl Field {
    /// One step of `MoveToField`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn move_to_field_step(
        &mut self,
        step: u16,
        target: CardId,
        enable: bool,
        ret: u8,
        pzone: bool,
        zone: u32,
        rule: bool,
        location_reason: u8,
        confirm: bool,
    ) -> bool {
        let param = self.cards[target].to_field_param;
        let move_player = ((param >> 24) & 0xff) as u8;
        let playerid = ((param >> 16) & 0xff) as u8;
        let loc = ((param >> 8) & 0xff) as u8;
        let positions = (param & 0xff) as u8;
        match step {
            0 => self.move_to_field_step_0(
                target,
                ret,
                pzone,
                zone,
                rule,
                location_reason,
                confirm,
                move_player,
                playerid,
                loc,
                positions,
            ),
            1 => self.move_to_field_step_1(target, ret, zone, move_player, loc, positions),
            2 => self.move_to_field_step_2(
                target,
                enable,
                ret,
                pzone,
                location_reason,
                move_player,
                playerid,
                loc,
            ),
            3 => {
                self.core.returns.set(1);
                true
            }
            4 => {
                self.core.returns.set(0);
                true
            }
            _ => true,
        }
    }

    /// Step 0: find a seat, asking the host only when there is a choice.
    ///
    /// Three routes out, and they are genuinely different operations:
    ///
    /// - **A Field Spell** displaces whatever occupies the field zone. Which
    ///   *way* it is displaced depends on the duel option:
    ///   `DUEL_1_FACEUP_FIELD` destroys the old one, otherwise it is sent to
    ///   the graveyard. Not the same thing — a destruction is replaceable and
    ///   raises destruction events.
    /// - **A Pendulum card** to a Pendulum Zone builds a mask of the two
    ///   pendulum seats and asks. No usable seat means no move at all.
    /// - **Everything else** counts the free seats and, if there is exactly
    ///   one possible zone and the caller did not insist on confirming, takes
    ///   it without asking.
    ///
    /// The single-seat shortcut is the same principle as the host questions:
    /// where there is no choice, do not manufacture one.
    #[allow(clippy::too_many_arguments)]
    fn move_to_field_step_0(
        &mut self,
        target: CardId,
        ret: u8,
        pzone: bool,
        zone: u32,
        rule: bool,
        location_reason: u8,
        confirm: bool,
        move_player: u8,
        playerid: u8,
        loc: u8,
        positions: u8,
    ) -> bool {
        self.core.returns.set(0);

        // A returning card must really have left temporarily, and by the
        // same effect that is bringing it back. Anything else is a stale
        // return and is abandoned.
        if ret == 1 {
            let temporary = self.cards[target].reason & reason::TEMPORARY != 0;
            let same_owner = match (self.cards[target].reason_effect, self.core.reason_effect) {
                (Some(a), Some(b)) => {
                    self.effects.get(a).and_then(|e| e.owner)
                        == self.effects.get(b).and_then(|e| e.owner)
                }
                _ => false,
            };
            if !temporary || !same_owner {
                return true;
            }
        }

        let is_field_spell = self.cards[target].data.is_type(card_type::FIELD)
            && self.cards[target]
                .data
                .is_type(card_type::SPELL | card_type::TRAP);
        if loc == location::SZONE && zone == (0x1 << 5) && is_field_spell {
            if let Some(occupant) = self.get_field_card(playerid, location::SZONE, 5) {
                let controller = self.cards[occupant].current.controller;
                if self.is_flag(crate::duel::flags::ONE_FACEUP_FIELD) {
                    self.destroy_card(occupant, None, reason::RULE, controller);
                } else {
                    self.send_to_card(
                        occupant,
                        None,
                        reason::RULE,
                        controller,
                        PLAYER_NONE,
                        u16::from(location::GRAVE),
                        0,
                        0,
                        false,
                    );
                }
                self.adjust_all();
            }
            return false;
        }

        if pzone
            && loc == location::SZONE
            && self.cards[target].data.is_type(card_type::PENDULUM)
            && self.is_flag(crate::duel::flags::PZONE)
        {
            let mut flag = 0u32;
            for side in 0..2u32 {
                if self.is_location_useable(playerid as usize, location::PZONE, side)
                    && zone & (1 << side) != 0
                {
                    flag |= 0x1 << (self.pzone_index(side as u8) + 8);
                }
            }
            if flag == 0 {
                return true;
            }
            if move_player != playerid {
                flag <<= 16;
            }
            // The mask SelectPlace wants is of what is *not* available.
            let flag = !flag;
            self.ask_for_place(target, move_player, flag);
            return false;
        }

        let mut flag = 0u32;
        let lreason = if location_reason != 0 {
            u32::from(location_reason)
        } else if self.cards[target].current.location == location::MZONE {
            Field::LOCATION_REASON_CONTROL
        } else {
            Field::LOCATION_REASON_TOFIELD
        };
        let mut ct = self.get_useable_count_with_flag(
            Some(target),
            playerid,
            loc,
            move_player,
            lreason,
            zone,
            &mut flag,
        );

        // The Extra Monster Zones are excluded from the count by default —
        // `get_tofield_count_with_flag` sets their bits after counting — so
        // a caller that explicitly asked for one has to add it back.
        if loc == location::MZONE && zone & 0x60 != 0 && zone != 0xff && !rule {
            for (bit, seat) in [(0x20u32, 5u32), (0x40, 6)] {
                if zone & bit != 0
                    && self.is_location_useable(playerid as usize, u16::from(loc), seat)
                {
                    flag &= !(1 << seat);
                    ct += 1;
                }
            }
        }
        if loc == location::SZONE {
            flag |= !zone;
        }

        // A returning card with nowhere to go is not merely refused: it is
        // sent to the graveyard, and step 3 reports success anyway.
        if ret == 1
            && (ct <= 0
                || self.cards[target].is_status(status::FORBIDDEN)
                || (positions & position::FACEDOWN == 0
                    && self
                        .check_unique_onfield(target, playerid, u16::from(loc), None)
                        .is_some()))
        {
            self.set_step(3);
            let (by, rp) = (self.core.reason_effect, self.core.reason_player);
            self.send_to_card(
                target,
                by,
                reason::RULE,
                rp,
                PLAYER_NONE,
                u16::from(location::GRAVE),
                0,
                0,
                false,
            );
            return false;
        }
        if ct <= 0 || !flag == 0 {
            return true;
        }
        // Exactly one zone asked for, and no insistence on confirming: take
        // it without asking.
        if !confirm && zone & zone.wrapping_sub(1) == 0 {
            for seat in 0..8u32 {
                if (1 << seat) & zone != 0 {
                    self.core.returns.set_i8(2, seat as i8);
                    return false;
                }
            }
        }
        // A trap monster returning to its own seat, in a configuration where
        // it occupied one.
        if !self.is_flag(crate::duel::flags::TRAP_MONSTERS_NOT_USE_ZONE) && ret == 2 {
            let seq = self.cards[target].previous.sequence as i8;
            self.core.returns.set_i8(2, seq);
            return false;
        }

        // Widen the seat mask into SelectPlace's four-row layout. Every bit
        // outside the row being offered is set, meaning unavailable.
        let flag = if move_player == playerid {
            if loc == location::SZONE {
                ((flag & 0xff) << 8) | 0xffff_00ff
            } else {
                (flag & 0xff) | 0xffff_ff00
            }
        } else if loc == location::SZONE {
            ((flag & 0xff) << 24) | 0x00ff_ffff
        } else {
            ((flag & 0xff) << 16) | 0xff00_ffff
        };
        // The seats that never exist: 7 of each monster row, and the two
        // extra monster zones of each.
        let flag = flag | 0xe080_e080;
        self.ask_for_place(target, move_player, flag);
        false
    }

    /// The hint-then-ask pair every seat choice makes.
    fn ask_for_place(&mut self, target: CardId, move_player: u8, flag: u32) {
        let code_ = self.cards[target].data.code;
        self.messages.push(Message::Hint {
            kind: hint::SELECTMSG,
            player: move_player,
            value: u64::from(code_),
        });
        self.emplace(Kind::SelectPlace {
            player: move_player,
            flag,
            count: 1,
            disable_field: false,
        });
    }
}

impl Field {
    /// Step 1: reset what the move invalidates, then choose a position.
    ///
    /// The reset mask is assembled from *both* ends of the move, and the
    /// third term is the one with a real rule in it:
    ///
    /// - arriving on the field adds `RESET_TOFIELD`
    /// - leaving the field adds `RESET_LEAVE`
    /// - moving **within** the field adds `RESET_MSCHANGE` — unless the card
    ///   is a trap monster (`EFFECT_PRE_MONSTER` naming a Trap) or this is a
    ///   trap monster returning to its seat. A trap monster sliding between
    ///   its two homes has not changed what it is.
    ///
    /// The whole block is skipped when the location is unchanged: a card
    /// moving seat within one row resets nothing.
    ///
    /// **The four "this turn" flags are cleared** on a genuine change of
    /// location, or on any return. A card that leaves and comes back may be
    /// summoned again this turn; leaving those set would silently forbid it.
    fn move_to_field_step_1(
        &mut self,
        target: CardId,
        ret: u8,
        zone: u32,
        move_player: u8,
        loc: u8,
        positions: u8,
    ) -> bool {
        let mut seq = self.core.returns.at_i8(2) as u32;
        let is_field_spell = self.cards[target].data.is_type(card_type::FIELD)
            && self.cards[target]
                .data
                .is_type(card_type::SPELL | card_type::TRAP);
        if loc == location::SZONE && zone == (0x1 << 5) && is_field_spell {
            seq = 5;
        }

        let current_loc = self.cards[target].current.location;
        if ret != 1 && loc != current_loc {
            let mut resetflag = 0u32;
            if loc & location::ONFIELD != 0 {
                resetflag |= reset::TOFIELD;
            }
            if current_loc & location::ONFIELD != 0 {
                resetflag |= reset::LEAVE;
            }
            let pre_monster_is_trap = self
                .is_affected_by_effect(target, code::PRE_MONSTER)
                .and_then(|e| self.effects.get(e))
                .is_some_and(|e| e.value as u32 & card_type::TRAP != 0);
            if loc & location::ONFIELD != 0
                && current_loc & location::ONFIELD != 0
                && !pre_monster_is_trap
                && ret != 2
            {
                resetflag |= reset::MSCHANGE;
            }
            self.reset_card(target, resetflag, reset::EVENT);
            self.clear_card_target(target);
        }
        if self.cards[target].current.location & location::ONFIELD == 0 {
            self.cards[target].clear_relate_effect();
        }
        if ret == 1 {
            self.cards[target].reason &= !reason::TEMPORARY;
        }
        if (ret == 0 && loc != current_loc) || ret == 1 {
            for flag in [
                status::SUMMON_TURN,
                status::FLIP_SUMMON_TURN,
                status::SPSUMMON_TURN,
                status::SET_TURN,
                status::FORM_CHANGED,
            ] {
                self.cards[target].set_status(flag, false);
            }
        }
        self.cards[target].temp.sequence = seq;

        // Only a Monster Zone arrival has a position to choose. Everything
        // else takes what it was given, and a Link monster has exactly one
        // legal position so it is not asked either.
        if loc != location::MZONE {
            self.core.returns.set(i32::from(positions));
            return false;
        }
        if self.cards[target].data.is_type(card_type::LINK)
            && self.cards[target].data.is_type(card_type::MONSTER)
        {
            self.core.returns.set(i32::from(position::FACEUP_ATTACK));
            return false;
        }
        let code_ = self.cards[target].data.code;
        self.emplace(Kind::SelectPosition {
            player: move_player,
            code: code_,
            positions,
        });
        false
    }

    /// Step 2: perform the move, then deal with what the card leaves behind.
    ///
    /// Three consequences of *leaving a Monster Zone*, all conditional on
    /// the card not having arrived in one:
    ///
    /// - its equip cards are **destroyed** by the rules, and then unequipped
    ///   — in that order, because `destroy` reads the relationship;
    /// - its Xyz materials are **sent to the graveyard**;
    /// - and a card that was itself an equip is unequipped.
    ///
    /// `EFFECT_MUST_USE_MZONE` is then charged, not consulted: by this point
    /// the seat is chosen, and the loop's only job is to spend the count of
    /// whichever effect forced it. The value is a seat mask, shifted down by
    /// 16 when the effect belongs to the other player — the same two-row
    /// packing the seat masks use everywhere.
    ///
    /// `EFFECT_PRE_MONSTER` is where a trap monster becomes a monster: the
    /// pre-monster marker is reset and replaced with a real
    /// `EFFECT_CHANGE_TYPE`, plus — in a configuration where trap monsters
    /// occupy their spell/trap seat — an `EFFECT_USE_EXTRA_SZONE` that
    /// remembers which seat it came from.
    #[allow(clippy::too_many_arguments)]
    fn move_to_field_step_2(
        &mut self,
        target: CardId,
        enable: bool,
        ret: u8,
        pzone: bool,
        location_reason: u8,
        move_player: u8,
        playerid: u8,
        loc: u8,
    ) -> bool {
        let position = self.core.returns.get() as u8;
        let message = self.open_move_message(target);
        if let Some(host) = self.cards[target].overlay_target {
            self.xyz_remove(host, target);
        }
        let seq = self.cards[target].temp.sequence;
        self.move_card(playerid, target, loc, seq, pzone);
        self.cards[target].current.position = position;
        self.cards[target].set_status(status::LEAVE_CONFIRMED, false);
        self.close_move_message(message, target);

        if self.cards[target].current.location != location::MZONE {
            let equips = self.cards[target].equiping_cards.clone();
            if !equips.is_empty() {
                self.destroy(
                    equips.clone(),
                    None,
                    reason::LOST_TARGET + reason::RULE,
                    PLAYER_NONE,
                    PLAYER_NONE,
                    0,
                    0,
                );
                for equipper in equips {
                    self.unequip(equipper);
                }
            }
            let materials = self.cards[target].xyz_materials.clone();
            if !materials.is_empty() {
                self.send_to(
                    materials,
                    None,
                    reason::LOST_TARGET + reason::RULE,
                    PLAYER_NONE,
                    PLAYER_NONE,
                    u16::from(location::GRAVE),
                    0,
                    position::FACEUP,
                    false,
                );
            }
        }
        if self.cards[target].previous.location == location::SZONE
            && self.cards[target].equiping_target.is_some()
        {
            self.unequip(target);
        }

        if self.cards[target].current.location == location::MZONE {
            self.charge_must_use_mzone(target, move_player, location_reason);
            self.apply_pre_monster(target);
        }

        if enable || (ret == 1 && self.cards[target].current.is_position(position::FACEUP)) {
            self.enable_field_effect(target, true);
        }
        // A returning card that is not a monster cannot stay in a Monster
        // Zone: it goes to the graveyard instead, silently.
        if ret == 1
            && self.cards[target].current.location == location::MZONE
            && !self.cards[target].data.is_type(card_type::MONSTER)
        {
            self.send_to_card(
                target,
                None,
                reason::RULE,
                PLAYER_NONE,
                PLAYER_NONE,
                u16::from(location::GRAVE),
                0,
                0,
                false,
            );
        } else {
            let (by, r, rp) = (
                self.cards[target].reason_effect,
                self.cards[target].reason,
                self.cards[target].reason_player,
            );
            if self.cards[target].previous.location == location::GRAVE {
                self.raise_single_event(
                    target,
                    vec![],
                    code::LEAVE_GRAVE,
                    by,
                    r,
                    move_player,
                    0,
                    0,
                );
                self.raise_event(Some(target), code::LEAVE_GRAVE, by, r, move_player, 0, 0);
            }
            self.raise_single_event(target, vec![], code::MOVE, by, r, rp, 0, 0);
            self.raise_event(Some(target), code::MOVE, by, r, rp, 0, 0);
            self.process_single_event();
            self.process_instant_event();
        }
        self.adjust_disable_check_list();
        false
    }

    /// Spend the count of whichever `EFFECT_MUST_USE_MZONE` forced this seat.
    fn charge_must_use_mzone(&mut self, target: CardId, move_player: u8, location_reason: u8) {
        let lreason = if location_reason != 0 {
            u32::from(location_reason)
        } else {
            Field::LOCATION_REASON_CONTROL
        };
        let controller = self.cards[target].current.controller;
        let seat = self.cards[target].current.sequence;
        let mut effects: Vec<crate::event::EffectId> = Vec::new();
        for p in 0..2u8 {
            effects.extend(self.filter_player_effect(p, code::MUST_USE_MZONE));
        }
        effects.extend(self.filter_effect(target, code::MUST_USE_MZONE));

        for e in effects {
            let spent = self
                .effects
                .get(e)
                .is_some_and(|x| x.is_flag(crate::effect::flag::COUNT_LIMIT) && x.count_limit == 0);
            if spent {
                continue;
            }
            let mut value = self.effect_must_use_value(e, target, controller, move_player, lreason);
            let theirs = self
                .effects
                .get(e)
                .map_or(PLAYER_NONE, |x| x.get_handler_player(&self.cards))
                != controller;
            if theirs {
                value >>= 16;
            }
            if value & (0x1 << seat) != 0 {
                self.dec_count(e, PLAYER_NONE);
            }
        }
    }

    /// `EFFECT_MUST_USE_MZONE`'s value, which defaults to every main seat.
    fn effect_must_use_value(
        &self,
        effect: crate::event::EffectId,
        target: CardId,
        controller: u8,
        move_player: u8,
        lreason: u32,
    ) -> u32 {
        let Some(e) = self.effects.get(effect) else {
            return 0x1f;
        };
        let ev = crate::event::Event::new(0);
        let ctx = crate::effect::Ctx {
            reason_effect: effect,
            player: controller,
            event: &ev,
            card: if e.is_flag(crate::effect::flag::PLAYER_TARGET) {
                None
            } else {
                Some(target)
            },
            args: &[
                i64::from(controller),
                i64::from(move_player),
                i64::from(lreason),
            ],
        };
        e.get_value(self, &ctx) as u32
    }

    /// Turn a card marked `EFFECT_PRE_MONSTER` into an actual monster.
    fn apply_pre_monster(&mut self, target: CardId) {
        let Some(marker) = self.is_affected_by_effect(target, code::PRE_MONSTER) else {
            return;
        };
        let mut type_ = self.effects.get(marker).map_or(0, |e| e.value as u32);
        if type_ & card_type::TRAP != 0 {
            type_ |= card_type::TRAPMONSTER | self.cards[target].data.type_;
        }
        self.reset_card(target, code::PRE_MONSTER, reset::CODE);

        let mut change =
            crate::effect::Effect::new(crate::effect::effect_type::SINGLE, code::CHANGE_TYPE);
        change.owner = Some(target);
        change.handler = Some(target);
        change.flag[0] = crate::effect::flag::CANNOT_DISABLE;
        change.reset_flag = reset::EVENT + 0x1fc_0000;
        change.value = i64::from(card_type::MONSTER | type_);
        let change = self.new_effect(change);
        self.cards[target]
            .single_effect
            .insert(code::CHANGE_TYPE, change);
        self.cards[target].indexer.insert(change);

        if !self.is_flag(crate::duel::flags::TRAP_MONSTERS_NOT_USE_ZONE)
            && type_ & card_type::TRAPMONSTER != 0
        {
            let seat = self.cards[target].previous.sequence;
            let mut extra = crate::effect::Effect::new(
                crate::effect::effect_type::FIELD,
                code::USE_EXTRA_SZONE,
            );
            extra.owner = Some(target);
            extra.handler = Some(target);
            extra.range = u16::from(location::MZONE);
            extra.flag[0] = crate::effect::flag::CANNOT_DISABLE;
            extra.reset_flag = reset::EVENT + 0x1fe_0000;
            extra.value = i64::from(1u32 + (0x10000u32 << seat));
            let extra = self.new_effect(extra);
            self.cards[target]
                .field_effect
                .insert(code::USE_EXTRA_SZONE, extra);
            self.cards[target].indexer.insert(extra);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{Card, CardData};
    use crate::processor::Status;

    fn card(f: &mut Field, player: u8, loc: u8, type_: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 18036057,
                type_,
                ..Default::default()
            },
            player,
        );
        c.current.controller = player;
        c.current.position = position::FACEUP_ATTACK;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        let seat = f.cards.len() as u32 - 1;
        f.add_card(player, id, loc, seat, false);
        id
    }

    /// Place a card, answering any seat/position question with the first
    /// legal option, and report where it ended up.
    fn run(f: &mut Field) -> Status {
        for _ in 0..256 {
            match f.process() {
                Status::Continue => continue,
                other => return other,
            }
        }
        panic!("did not settle");
    }

    fn queued(f: &Field) -> bool {
        !f.core.subunits.is_empty() || f.queue().next().is_some()
    }

    mod queueing {
        use super::*;

        /// `move_to_field` is for the field. A pile destination queues
        /// nothing at all — that is `send_to`.
        #[test]
        fn a_pile_destination_queues_nothing() {
            let mut f = Field::new(8000);
            let c = card(&mut f, 0, location::HAND, card_type::MONSTER);
            f.move_to_field(
                c,
                0,
                0,
                u16::from(location::GRAVE),
                position::FACEUP_ATTACK,
                false,
                0,
                0xff,
                false,
                0,
                false,
            );
            assert!(!queued(&f));
        }

        /// No positions offered is also a refusal.
        #[test]
        fn no_position_queues_nothing() {
            let mut f = Field::new(8000);
            let c = card(&mut f, 0, location::HAND, card_type::MONSTER);
            f.move_to_field(
                c,
                0,
                0,
                u16::from(location::MZONE),
                0,
                false,
                0,
                0xff,
                false,
                0,
                false,
            );
            assert!(!queued(&f));
        }

        /// A card already where it is being sent, under the same controller,
        /// does not move — and no unit is queued at all.
        #[test]
        fn a_card_already_there_queues_nothing() {
            let mut f = Field::new(8000);
            let c = card(&mut f, 0, location::MZONE, card_type::MONSTER);
            f.move_to_field(
                c,
                0,
                0,
                u16::from(location::MZONE),
                position::FACEUP_ATTACK,
                false,
                0,
                0xff,
                false,
                0,
                false,
            );
            assert!(!queued(&f));
        }

        /// The symbolic destinations are rewritten into a real location plus
        /// a seat mask before the unit ever runs.
        #[test]
        fn symbolic_destinations_are_normalised() {
            for (asked, want_loc, want_zone) in [
                (location::MMZONE, location::MZONE, 0x1fu32),
                (location::STZONE, location::SZONE, 0x1f),
                (location::EMZONE, location::MZONE, (1 << 5) | (1 << 6)),
                (location::FZONE, location::SZONE, 1 << 5),
            ] {
                let mut f = Field::new(8000);
                let c = card(&mut f, 0, location::HAND, card_type::MONSTER);
                f.move_to_field(
                    c,
                    0,
                    0,
                    asked,
                    position::FACEUP_ATTACK,
                    false,
                    0,
                    0xff,
                    false,
                    0,
                    false,
                );
                let loc = ((f.cards[c].to_field_param >> 8) & 0xff) as u8;
                assert_eq!(loc, want_loc, "asked for {asked:#x}");
                match f.core.subunits.first().map(|u| &u.kind) {
                    Some(Kind::MoveToField { zone, .. }) => {
                        assert_eq!(*zone, want_zone, "asked for {asked:#x}")
                    }
                    other => panic!("expected MoveToField, got {other:?}"),
                }
            }
        }

        /// The two players are different questions: whose field, and who is
        /// choosing.
        #[test]
        fn the_packed_word_keeps_both_players_apart() {
            let mut f = Field::new(8000);
            let c = card(&mut f, 0, location::HAND, card_type::MONSTER);
            // player 1 chooses a seat on player 0's field
            f.move_to_field(
                c,
                1,
                0,
                u16::from(location::MZONE),
                position::FACEUP_ATTACK,
                false,
                0,
                0xff,
                false,
                0,
                false,
            );
            let p = f.cards[c].to_field_param;
            assert_eq!((p >> 24) & 0xff, 1, "move_player chooses");
            assert_eq!((p >> 16) & 0xff, 0, "playerid owns the field");
            assert_eq!(p & 0xff, u32::from(position::FACEUP_ATTACK));
        }
    }

    mod the_machine {
        use super::*;

        /// A single asked-for seat is taken without a question, the same
        /// principle the host questions follow.
        #[test]
        fn a_single_seat_is_taken_without_asking() {
            let mut f = Field::new(8000);
            let c = card(&mut f, 0, location::HAND, card_type::MONSTER);
            f.move_to_field(
                c,
                0,
                0,
                u16::from(location::MZONE),
                position::FACEUP_ATTACK,
                true,
                0,
                1 << 2,
                false,
                0,
                false,
            );
            assert_eq!(run(&mut f), Status::End, "never yielded");
            assert_eq!(f.cards[c].current.location, location::MZONE);
            assert_eq!(
                f.cards[c].current.sequence, 2,
                "the seat that was asked for"
            );
            // Step 3 is the TRUE exit and step 4 the FALSE one, so a
            // completed move reports 1. The `ret == 1` no-room path jumps to
            // 3, which the increment turns into 4 — reporting 0, because the
            // card went to the graveyard instead of to the field.
            assert_eq!(f.core.returns.get(), 1, "and it reports having moved");
        }

        /// More than one possible seat is a real choice, so the host is
        /// asked.
        #[test]
        fn several_seats_are_asked_about() {
            let mut f = Field::new(8000);
            let c = card(&mut f, 0, location::HAND, card_type::MONSTER);
            f.move_to_field(
                c,
                0,
                0,
                u16::from(location::MZONE),
                position::FACEUP_ATTACK,
                true,
                0,
                0x1f,
                false,
                0,
                false,
            );
            assert_eq!(run(&mut f), Status::Awaiting, "it stopped to ask");

            // player 0, monster zone, seat 3
            f.core.returns.set_i8(0, 0);
            f.core.returns.set_i8(1, location::MZONE as i8);
            f.core.returns.set_i8(2, 3);
            assert_eq!(run(&mut f), Status::End);
            assert_eq!(f.cards[c].current.sequence, 3);
        }

        /// Arriving in a Monster Zone is the only case with a position to
        /// choose; a spell takes what it was given.
        #[test]
        fn a_spell_is_not_asked_which_way_up() {
            let mut f = Field::new(8000);
            let c = card(&mut f, 0, location::HAND, card_type::SPELL);
            f.move_to_field(
                c,
                0,
                0,
                u16::from(location::SZONE),
                position::FACEUP,
                true,
                0,
                1 << 1,
                false,
                0,
                false,
            );
            assert_eq!(run(&mut f), Status::End);
            assert_eq!(f.cards[c].current.location, location::SZONE);
            assert_eq!(f.cards[c].current.sequence, 1);
        }

        /// The "this turn" flags are cleared by a genuine change of
        /// location: a card that left and came back may be summoned again.
        #[test]
        fn a_change_of_location_clears_the_turn_flags() {
            let mut f = Field::new(8000);
            let c = card(&mut f, 0, location::HAND, card_type::MONSTER);
            f.cards[c].set_status(status::SUMMON_TURN, true);
            f.cards[c].set_status(status::SET_TURN, true);

            f.move_to_field(
                c,
                0,
                0,
                u16::from(location::MZONE),
                position::FACEUP_ATTACK,
                true,
                0,
                1 << 0,
                false,
                0,
                false,
            );
            run(&mut f);
            assert!(!f.cards[c].is_status(status::SUMMON_TURN));
            assert!(!f.cards[c].is_status(status::SET_TURN));
        }

        /// A stale return — the card did not leave temporarily — is
        /// abandoned rather than performed.
        #[test]
        fn a_return_without_a_temporary_absence_is_abandoned() {
            let mut f = Field::new(8000);
            let c = card(&mut f, 0, location::REMOVED, card_type::MONSTER);
            f.cards[c].reason = reason::EFFECT; // not TEMPORARY
            f.move_to_field(
                c,
                0,
                0,
                u16::from(location::MZONE),
                position::FACEUP_ATTACK,
                true,
                1,
                0x1f,
                false,
                0,
                false,
            );
            assert_eq!(run(&mut f), Status::End);
            assert_eq!(
                f.cards[c].current.location,
                location::REMOVED,
                "it stayed where it was"
            );
        }

        /// `EVENT_MOVE` is raised for an ordinary arrival.
        #[test]
        fn arriving_raises_a_move_event() {
            let mut f = Field::new(8000);
            let c = card(&mut f, 0, location::HAND, card_type::MONSTER);
            f.move_to_field(
                c,
                0,
                0,
                u16::from(location::MZONE),
                position::FACEUP_ATTACK,
                true,
                0,
                1 << 0,
                false,
                0,
                false,
            );
            run(&mut f);
            assert!(f
                .core
                .instant_event
                .iter()
                .any(|e| e.event_code == code::MOVE));
        }

        /// A card leaving a Monster Zone loses its Xyz materials to the
        /// graveyard.
        #[test]
        fn leaving_a_monster_zone_sheds_its_materials() {
            let mut f = Field::new(8000);
            let xyz = card(&mut f, 0, location::MZONE, card_type::MONSTER);
            let mat = card(&mut f, 0, location::MZONE, card_type::MONSTER);
            f.cards[mat].overlay_target = Some(xyz);
            f.cards[mat].current.location = location::OVERLAY;
            f.cards[mat].current.sequence = 0;
            f.cards[xyz].xyz_materials.push(mat);

            f.move_to_field(
                xyz,
                0,
                0,
                u16::from(location::SZONE),
                position::FACEUP,
                true,
                0,
                1 << 0,
                false,
                0,
                false,
            );
            run(&mut f);
            assert_eq!(f.cards[xyz].current.location, location::SZONE);
            assert_eq!(
                f.cards[mat].current.location,
                location::GRAVE,
                "a material has nothing to sit under any more"
            );
        }
    }
}

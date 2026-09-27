//! The disable-check machinery.
//!
//! `STATUS_DISABLED` and `STATUS_FORBIDDEN` are read all over the engine —
//! by `is_available`, by `is_chain_disablable`, by the trigger gather — but
//! nothing in those places computes them. They are computed here, and only
//! when something asks for a card to be re-checked.
//!
//! That laziness is the design. A continuous effect that disables a monster
//! does not reach in and set a bit; it registers itself, and any card whose
//! answer might have changed is put on `disable_check_set` to be re-asked
//! later. `adjust_disable_check_list` is where the answers are settled.
//!
//! ## The loop guard is not an optimisation
//!
//! A card whose disabled state changes has `STATUS_TO_ENABLE` or
//! `STATUS_TO_DISABLE` set, and a card carrying either is **skipped** by the
//! re-check. Two cards that disable each other would otherwise flip forever:
//! A's change puts B on the list, B's change puts A back on it. The marks
//! are cleared at the end of each card's turn through the loop, so the guard
//! lasts exactly one pass.

use crate::board::location;
use crate::card::{card_type, status};
use crate::effect::{effect_type, flag, Effect};
use crate::event::{code, CardId, EffectId, PLAYER_NONE};
use crate::field::{reset, Field};
use std::collections::BTreeSet;

impl Field {
    /// `effect::is_disable_related` — could this effect change some card's
    /// disabled state?
    pub fn is_disable_related(&self, id: EffectId) -> bool {
        self.effects.get(id).is_some_and(|e| {
            matches!(
                e.code,
                code::IMMUNE_EFFECT | code::DISABLE | code::CANNOT_DISABLE | code::FORBIDDEN
            )
        })
    }

    /// `card::filter_immune_effect` — rebuild the card's immunity list.
    ///
    /// The same five-source shape as `filter_effect`, with two differences
    /// worth noting: it takes *every* `EFFECT_IMMUNE_EFFECT` from the card's
    /// own container without asking whether it is available, and it does not
    /// consult `is_affect_by_effect` — which it could not, since that is the
    /// question immunity answers.
    pub fn filter_immune_effect(&mut self, card: CardId) {
        let mut found = Vec::new();
        for &e in self.cards[card]
            .single_effect
            .equal_range(code::IMMUNE_EFFECT)
        {
            found.push(e);
        }
        // The reference takes these without asking whether they are
        // available, unlike every other five-source walk.
        for equipper in self.cards[card].equiping_cards.clone() {
            for &e in self.cards[equipper]
                .equip_effect
                .equal_range(code::IMMUNE_EFFECT)
            {
                found.push(e);
            }
        }
        for owner in self.cards[card].effect_target_owner.clone() {
            for e in self.cards[owner]
                .target_effect
                .equal_range(code::IMMUNE_EFFECT)
                .to_vec()
            {
                if self.is_target(e, card) {
                    found.push(e);
                }
            }
        }
        for material in self.cards[card].xyz_materials.clone() {
            for e in self.cards[material]
                .xmaterial_effect
                .equal_range(code::IMMUNE_EFFECT)
                .to_vec()
            {
                if self.is_target(e, card) {
                    found.push(e);
                }
            }
        }
        for e in self
            .field_effects
            .aura
            .equal_range(code::IMMUNE_EFFECT)
            .to_vec()
        {
            if self.is_target(e, card) {
                found.push(e);
            }
        }
        self.cards[card].immune_effect = found;
    }

    /// `card::refresh_disable_status` — settle this card's two status bits.
    ///
    /// Immunity is rebuilt first, because both questions below run through
    /// `is_affect_by_effect`, which consults it. `CANNOT_DISABLE` beats
    /// `DISABLE`; there is no such override for `FORBIDDEN`.
    pub fn refresh_disable_status(&mut self, card: CardId) {
        self.filter_immune_effect(card);

        let forbidden = self.is_affected_by_effect(card, code::FORBIDDEN).is_some();
        self.cards[card].set_status(status::FORBIDDEN, forbidden);

        let disabled = self
            .is_affected_by_effect(card, code::CANNOT_DISABLE)
            .is_none()
            && self.is_affected_by_effect(card, code::DISABLE).is_some();
        self.cards[card].set_status(status::DISABLED, disabled);
    }

    /// `field::filter_affected_cards` — every card a field effect reaches.
    ///
    /// Returns nothing for an action effect, a non-field effect, or one
    /// aimed at players: none of those change a card's disabled state, so
    /// none of them puts anything on the check list.
    ///
    /// `s_range` is searched on the handler's own side and `o_range` on the
    /// other, which the reference expresses by swapping both at the end of
    /// the first pass.
    pub fn filter_affected_cards(&self, id: EffectId) -> BTreeSet<CardId> {
        let mut out = BTreeSet::new();
        let Some(e) = self.effects.get(id) else {
            return out;
        };
        if e.is_type(effect_type::ACTIONS)
            || !e.is_type(effect_type::FIELD)
            || e.is_flag(flag::PLAYER_TARGET | flag::SPSUM_PARAM)
        {
            return out;
        }
        let mut side = e.get_handler_player(&self.cards);
        if side == PLAYER_NONE {
            return out;
        }
        let mut range = e.s_range;
        let mut candidates: Vec<CardId> = Vec::new();
        for _ in 0..2 {
            let zones = &self.players[side as usize];
            use crate::board::location as l;
            if range & u16::from(l::MZONE) != 0 {
                candidates.extend(zones.mzone.iter().flatten().copied());
            }
            if range & u16::from(l::SZONE) != 0 {
                candidates.extend(zones.szone.iter().flatten().copied());
            }
            if range & u16::from(l::GRAVE) != 0 {
                candidates.extend(&zones.grave);
            }
            if range & u16::from(l::REMOVED) != 0 {
                candidates.extend(&zones.removed);
            }
            if range & u16::from(l::HAND) != 0 {
                candidates.extend(&zones.hand);
            }
            if range & u16::from(l::DECK) != 0 {
                candidates.extend(&zones.main);
            }
            if range & u16::from(l::EXTRA) != 0 {
                candidates.extend(&zones.extra);
            }
            range = e.o_range;
            side = 1 - side;
        }
        for card in candidates {
            if self.is_target(id, card) {
                out.insert(card);
            }
        }
        out
    }

    /// `field::update_disable_check_list` — put every card an effect reaches
    /// on the list to be re-checked.
    pub fn update_disable_check_list(&mut self, id: EffectId) {
        for card in self.filter_affected_cards(id) {
            self.add_to_disable_check_list(card);
        }
    }

    /// `field::add_to_disable_check_list`.
    pub fn add_to_disable_check_list(&mut self, card: CardId) {
        self.field_effects.disable_check_set.insert(card);
    }

    /// `card::filter_disable_related_cards` — this card's state changed, so
    /// put whatever its own effects reach on the list too.
    ///
    /// Four kinds, and only the first fans out: a field effect reaches every
    /// card in its range, while an equip, target or Xyz-material effect
    /// reaches exactly the one card it is attached to.
    pub fn filter_disable_related_cards(&mut self, card: CardId) {
        for id in self.cards[card].indexer.iter().copied().collect::<Vec<_>>() {
            if !self.is_disable_related(id) {
                continue;
            }
            let Some(e) = self.effects.get(id) else {
                continue;
            };
            let ty = e.effect_type;
            if ty & effect_type::FIELD != 0 {
                self.update_disable_check_list(id);
            } else if ty & effect_type::EQUIP != 0 {
                if let Some(target) = self.cards[card].equiping_target {
                    self.add_to_disable_check_list(target);
                }
            } else if ty & effect_type::TARGET != 0
                && !self.cards[card].effect_target_cards.is_empty()
            {
                for target in self.cards[card].effect_target_cards.clone() {
                    self.add_to_disable_check_list(target);
                }
            } else if ty & effect_type::XMATERIAL != 0 {
                if let Some(target) = self.cards[card].overlay_target {
                    self.add_to_disable_check_list(target);
                }
            }
        }
    }

    /// `field::adjust_disable_check_list` — settle every card that was put
    /// up for re-checking.
    ///
    /// The `TO_ENABLE`/`TO_DISABLE` marks are a loop guard, not bookkeeping:
    /// a card carrying either is skipped, so two cards that disable each
    /// other cannot flip forever. The marks are cleared at the end of each
    /// card's turn through the loop, so the guard lasts exactly one pass.
    ///
    /// The set is drained rather than iterated, because
    /// `filter_disable_related_cards` adds to it while the loop runs — the
    /// reference iterates a `std::set` it is inserting into, which works
    /// there and would not here.
    pub fn adjust_disable_check_list(&mut self) {
        // The set is drained in its own (card-id) order; taking it whole is
        // what lets `filter_disable_related_cards` add to a fresh one meanwhile.
        // Taken whole: what `filter_disable_related_cards` adds while this
        // runs goes on the fresh list and waits for the next adjust, as the
        // reference's iterate-while-inserting does in effect. The taken
        // list's capacity comes back afterwards.
        let mut taken = std::mem::take(&mut self.field_effects.disable_check_set);
        for card in taken.drain_sorted() {
            if !self.cards[card].is_status(status::TO_ENABLE | status::TO_DISABLE) {
                let before = self.cards[card].status & (status::DISABLED | status::FORBIDDEN);
                self.refresh_disable_status(card);
                let after = self.cards[card].status & (status::DISABLED | status::FORBIDDEN);
                if before != after && self.cards[card].is_status(status::EFFECT_ENABLED) {
                    self.filter_disable_related_cards(card);
                    if before != 0 {
                        self.cards[card].set_status(status::TO_ENABLE, true);
                    } else {
                        self.cards[card].set_status(status::TO_DISABLE, true);
                    }
                }
            }
            // A card that has just become disabled loses what its own
            // effects were holding — but only if it is not simultaneously
            // being re-enabled.
            if self.cards[card].is_status(status::DISABLED)
                && self.cards[card].is_status(status::TO_DISABLE)
                && !self.cards[card].is_status(status::TO_ENABLE)
            {
                self.reset_card(card, reset::DISABLE, reset::EVENT);
            }
            self.cards[card].set_status(status::TO_ENABLE | status::TO_DISABLE, false);
        }
        let added = std::mem::replace(&mut self.field_effects.disable_check_set, taken);
        for card in added.into_items() {
            self.field_effects.disable_check_set.insert(card);
        }
    }

    /// `card::reset` for `RESET_EVENT`, which is the only type reached from
    /// here.
    ///
    /// Worth reading the mask arithmetic rather than assuming: `RESET_DISABLE`
    /// is `0x10000`, and it is in **none** of the location masks the other
    /// blocks test (`RESET_TOGRAVE` is `0x40000`, `RESET_TOHAND` `0x200000`,
    /// and so on). So a disable reset skips every one of those blocks and
    /// does exactly three things: drop matching relations, clear the
    /// temporary half of the counters, and remove the effects that say they
    /// reset on it.
    /// `RESET_EVENT | 0xec0000` — the resets the reference gives the
    /// controller-pinning effect `RESET_TURN_SET` creates: leaving the
    /// field by any route. Spelled out from the literal, and pinned.
    pub const TURN_SET_CONTROL_RESETS: u32 =
        reset::LEAVE | reset::TODECK | reset::TOHAND | reset::REMOVE | reset::TOGRAVE;

    pub fn reset_card(&mut self, card: CardId, level: u32, reset_type: u32) {
        if !matches!(
            reset_type,
            reset::EVENT | reset::PHASE | reset::CODE | reset::COPY | reset::CARD
        ) {
            return;
        }
        if reset_type != reset::EVENT {
            // Only the event reset has bookkeeping of its own; the other
            // four are a sweep of the card's effects and nothing else.
            for id in self.cards[card].indexer.iter().copied().collect::<Vec<_>>() {
                if self.effect_resets(id, level, reset_type) {
                    self.remove_card_effect(card, id);
                }
            }
            return;
        }
        self.cards[card].retain_relations(|mask| mask & 0xffff_0000 & level == 0);

        // The reference's six bookkeeping blocks, each gated on its own
        // set of reset bits. They overlap heavily and the *differences*
        // are the content: which of `LEAVE`, `TOFIELD` and `TURN_SET` a
        // block includes is what decides whether a monster that merely
        // changed seats loses its battle tallies.
        const MOVED: u32 = reset::TODECK
            | reset::TOHAND
            | reset::TOGRAVE
            | reset::REMOVE
            | reset::TEMP_REMOVE
            | reset::OVERLAY
            | reset::MSCHANGE;

        if level & MOVED != 0 {
            self.cards[card].clear_relate_effect();
        }

        // Battle bookkeeping: gone once the card has left a zone at all,
        // and **`attack_all_target` is set back to true** — the permissive
        // default, which is why an "attacks all" effect works for a
        // monster that has just arrived.
        if level & (MOVED | reset::LEAVE | reset::TOFIELD) != 0 {
            let c = &mut self.cards[card];
            c.indestructable_effects.clear();
            c.announced_cards.clear();
            c.attacked_cards.clear();
            c.attack_announce_count = 0;
            c.announce_count = 0;
            c.attacked_count = 0;
            c.attack_all_target = true;
        }

        // A turn Set joins the list here: the card counts as new for
        // once-per-turn tallies and for what it has battled.
        if level & (MOVED | reset::LEAVE | reset::TOFIELD | reset::TURN_SET) != 0 {
            self.cards[card].battled_cards.clear();
            self.reset_effect_count(card);
            for e in self.cards[card]
                .field_effect
                .equal_range(code::DISABLE_FIELD)
                .to_vec()
            {
                if let Some(x) = self.effects.get_mut(e) {
                    x.value = 0;
                }
            }
        }

        // Counters go on a *narrower* set: leaving the field by battle
        // (`RESET_LEAVE` alone) does not clear them.
        if level & (MOVED | reset::TOFIELD | reset::TURN_SET) != 0 {
            self.cards[card].counters.clear();
        }

        // The extra-zone grants keep their low half, which is the zone
        // mask; the high half is the "already used" record.
        if level & (MOVED | reset::LEAVE | reset::TOFIELD | reset::TURN_SET | reset::CONTROL) != 0 {
            for code_ in [code::USE_EXTRA_MZONE, code::USE_EXTRA_SZONE] {
                for e in self.cards[card].field_effect.equal_range(code_).to_vec() {
                    if let Some(x) = self.effects.get_mut(e) {
                        x.value &= 0xffff;
                    }
                }
            }
        }

        if level & reset::TOFIELD != 0 {
            self.cards[card].pre_equip_target = None;
        }

        if level & reset::DISABLE != 0 {
            // Counters come in two halves; only the temporary one is lost —
            // and each loss is announced (`MSG_REMOVE_COUNTER` with the
            // temporary count), as the reference's block does. A monster
            // destroyed by battle is disabled by the status refresh before
            // it leaves, so Breaker's Spell Counter goes this way, one
            // message before the move (Goat-deck fuzz seed 4001279 and 187
            // more of 500,000).
            let (controller, location, sequence) = {
                let c = &self.cards[card].current;
                (c.controller, c.location, c.sequence)
            };
            let lost: Vec<(u16, u16)> = self.cards[card]
                .counters
                .iter()
                .filter(|(_, c)| c[1] > 0)
                .map(|(&t, c)| (t, c[1]))
                .collect();
            for (counter_type, count) in lost {
                self.messages.push(crate::field::Message::RemoveCounter {
                    counter_type,
                    controller,
                    location,
                    sequence,
                    count,
                });
                if let Some(c) = self.cards[card].counters.get_mut(&counter_type) {
                    c[1] = 0;
                }
            }
            self.cards[card]
                .counters
                .retain(|_, c| c[0] != 0 || c[1] != 0);
        }

        // **`RESET_TURN_SET` pins the controller.** A card turned
        // face-down loses whatever was *continuously* holding it — an
        // Equip Spell's `EFFECT_SET_CONTROL`, or a conditional one — so
        // the reference re-establishes the controller it has right now
        // with a fresh single effect. (The reference first copies the old
        // effect's id onto it, and `card::add_effect` then overwrites the
        // id with a fresh one regardless — so the pin is *newer* than
        // anything that survives the flip, which is what keeps it winning
        // under `refresh_control_status`'s newest-wins rule. That dead
        // assignment is not transcribed.) Without this block a
        // Snatch-Stolen monster flipped face-down went straight back to
        // its owner (fuzz seed 23). `card::reset`, `card.cpp`, the
        // `RESET_TURN_SET` block.
        if level & reset::TURN_SET != 0 {
            let (_, source) = self.refresh_control_status(card);
            let re_establish = source.and_then(|src| {
                let x = self.effects.get(src)?;
                let continuous_or_conditional = !x.is_type(effect_type::SINGLE)
                    || x.condition.is_some()
                    || x.avail_condition.is_some();
                continuous_or_conditional.then_some(())
            });
            if re_establish.is_some() {
                let mut e = Effect::new(effect_type::SINGLE, code::SET_CONTROL);
                e.owner = Some(card);
                e.handler = Some(card);
                e.value = i64::from(self.cards[card].current.controller);
                e.flag[0] = flag::CANNOT_DISABLE;
                e.reset_flag = reset::EVENT | Self::TURN_SET_CONTROL_RESETS;
                let id = self.new_effect(e);
                self.add_card_effect(card, id);
            }
        }

        for id in self.cards[card].indexer.iter().copied().collect::<Vec<_>>() {
            if self.effect_resets(id, level, reset_type) {
                self.remove_card_effect(card, id);
            }
        }
    }

    /// `card::reset_effect_count` — recharge every count-limited effect
    /// on the card.
    ///
    /// `count_limit_max` is the ceiling and `count_limit` what remains, so
    /// a recharge is an assignment rather than an increment — an effect
    /// used twice in a turn comes back to the same ceiling as one used
    /// once.
    pub fn reset_effect_count(&mut self, card: CardId) {
        for id in self.cards[card].indexer.iter().copied().collect::<Vec<_>>() {
            if let Some(e) = self.effects.get_mut(id) {
                if e.is_flag(flag::COUNT_LIMIT) {
                    e.count_limit = e.count_limit_max;
                }
            }
        }
    }

    /// `effect::reset` for `RESET_EVENT`.
    ///
    /// The middle line is the one to read twice: an effect whose owner is
    /// not its handler — a granted or equipped effect — has `RESET_DISABLE`
    /// **stripped from the level** before the comparison. Disabling the card
    /// an effect sits on does not reset an effect that belongs to another
    /// card, which is the comment above the reference's definition.
    pub fn effect_resets(&mut self, id: EffectId, mut level: u32, reset_type: u32) -> bool {
        let Some(e) = self.effects.get(id) else {
            return false;
        };
        match reset_type {
            reset::EVENT => {
                if e.reset_flag & reset::EVENT == 0 {
                    return false;
                }
                // The reference's one asymmetry here: a *granted* effect —
                // one whose owner is not its handler — is not taken away
                // by the handler being disabled, because it was never the
                // handler's to lose.
                if e.owner != e.handler {
                    level &= !reset::DISABLE;
                }
                level & 0xffff_0000 & e.reset_flag != 0
            }
            // `RESET_CODE` is how a **flag effect** is cleared: the code
            // is the whole address. The two extra tests are what keeps it
            // from taking a real effect that happens to share the number —
            // a flag effect is always a plain single, never an action.
            reset::CODE => {
                e.code == level
                    && e.is_type(effect_type::SINGLE)
                    && !e.is_type(effect_type::ACTIONS)
            }
            // A phase reset is a **countdown**, not a test. It ticks only
            // on a turn the effect's own flags name, and the effect goes
            // when the count reaches zero — which is why `SetReset` with a
            // count of two survives one end phase.
            reset::PHASE => {
                if e.reset_flag & reset::PHASE == 0 {
                    return false;
                }
                let pid = e.get_handler_player(&self.cards);
                let tp = self.infos.turn_player;
                let ours = e.reset_flag & reset::SELF_TURN != 0 && pid == tp;
                let theirs = e.reset_flag & reset::OPPO_TURN != 0 && pid != tp;
                let due = (ours || theirs) && level & 0x3ff & e.reset_flag != 0;
                if due {
                    if let Some(e) = self.effects.get_mut(id) {
                        e.reset_count = e.reset_count.saturating_sub(1);
                    }
                }
                self.effects.get(id).is_some_and(|e| e.reset_count == 0)
            }
            reset::CARD => e
                .get_owner(&self.cards)
                .is_some_and(|o| self.cards[o].data.code == level),
            reset::COPY => u32::from(e.copy_id) == level,
            _ => false,
        }
    }

    /// `card::remove_effect` — take one of a card's own effects away.
    ///
    /// Longer than it looks, because removing an effect has consequences in
    /// four directions and the reference does all four here.
    ///
    /// **Which cards need re-checking** depends on the effect's type, and
    /// the answer is never "the card it was on" except for a single effect.
    /// The reference calls it `check_target`:
    ///
    /// | type | whose status may have changed |
    /// |---|---|
    /// | `SINGLE` | the card itself |
    /// | `EQUIP` | its equip target, if any |
    /// | `TARGET` | the cards it targets, if any |
    /// | `XMATERIAL` | the monster it is under, if any |
    /// | `FIELD` | nobody — it is handled by range instead |
    ///
    /// In each case it is the card that was on the *receiving* end. "If any"
    /// is load-bearing: where the relationship is already gone the set is
    /// **cleared**, not left as the card itself, so nothing is queued at all.
    ///
    /// **A field effect comes off the field conditionally.** Only if it is in
    /// range of this card, or — the special case — if it is a hand trigger
    /// that is not a phase trigger and the card still has a controller. A
    /// port that removed it unconditionally would take effects off the field
    /// that are still live somewhere else.
    ///
    /// **The registry cleanups here are not gated on `FIELD_ONLY`**, unlike
    /// the ones in [`Field::remove_effect`]. That asymmetry is the
    /// reference's and it is deliberate: this is the card's own effect going
    /// away for good, so it leaves every registry, while the field-level
    /// function is also used for effects that are merely leaving the field's
    /// index and may come back.
    pub fn remove_card_effect(&mut self, card: CardId, id: EffectId) {
        let Some(e) = self.effects.get(id) else {
            return;
        };
        let (ty, code_, reset_flag, description) =
            (e.effect_type, e.code, e.reset_flag, e.description);
        let (is_oath, is_count_limited, is_initial, copy_id) = (
            e.is_flag(flag::OATH),
            e.is_flag(flag::COUNT_LIMIT),
            e.is_flag(flag::INITIAL),
            e.copy_id,
        );
        let _ = description;

        // The reference erases by the iterator it stored when adding, so it
        // never has to work out which container the effect is in. An
        // index-based port derives it — but only for `check_target`, since
        // removing from all five containers is cheaper than deriving it and
        // cannot be wrong: `EffectIndex::remove` is a no-op for a code and id
        // it does not hold.
        let c = &mut self.cards[card];
        c.single_effect.remove(code_, id);
        c.field_effect.remove(code_, id);
        c.equip_effect.remove(code_, id);
        c.target_effect.remove(code_, id);
        c.xmaterial_effect.remove(code_, id);

        let check_target: Vec<CardId> = if ty & effect_type::SINGLE != 0 {
            vec![card]
        } else if ty & effect_type::EQUIP != 0 {
            self.cards[card].equiping_target.into_iter().collect()
        } else if ty & effect_type::TARGET != 0 {
            self.cards[card].effect_target_cards.clone()
        } else if ty & effect_type::XMATERIAL != 0 {
            self.cards[card].overlay_target.into_iter().collect()
        } else if ty & effect_type::FIELD != 0 {
            if self.is_available(id) && self.is_disable_related(id) {
                self.update_disable_check_list(id);
            }
            // A hand trigger that is not a phase trigger stays registered
            // while the card has a controller, even out of range.
            let in_range = self
                .effects
                .get(id)
                .is_some_and(|e| e.in_range(&self.cards, &self.cards[card]));
            let hand_trigger = self.effects.get(id).is_some_and(|e| {
                e.range & u16::from(location::HAND) != 0
                    && e.effect_type & effect_type::TRIGGER_O != 0
                    && e.code & code::PHASE == 0
            });
            if in_range || (self.cards[card].current.controller != PLAYER_NONE && hand_trigger) {
                self.remove_effect(id);
            }
            Vec::new()
        } else {
            vec![card]
        };

        if self.cards[card].current.controller != PLAYER_NONE
            && !check_target.is_empty()
            && self.is_disable_related(id)
        {
            for target in check_target {
                self.add_to_disable_check_list(target);
            }
        }

        // A copied effect going away means the card's printed effects have
        // to be registered again. Unreachable until `copy_effect` and the
        // card pool exist, and a panic rather than a no-op for that reason.
        if is_initial && copy_id != 0 && self.cards[card].is_status(status::EFFECT_REPLACED) {
            self.cards[card].set_status(status::EFFECT_REPLACED, false);
            let data_type = self.cards[card].data.type_;
            if data_type & card_type::NORMAL == 0 || data_type & card_type::PENDULUM != 0 {
                self.reinitialize_card_effects(card);
            }
        }

        self.cards[card].indexer.remove(&id);
        if is_oath {
            self.field_effects.oath.remove(&id);
        }
        if reset_flag & reset::PHASE != 0 {
            self.field_effects.pheff.remove(&id);
        }
        if reset_flag & reset::CHAIN != 0 {
            self.field_effects.cheff.remove(&id);
        }
        if is_count_limited {
            self.field_effects.rechargeable.remove(&id);
        }

        // A permit effect going away takes the counters it was permitting
        // with it — *both* halves, because the slot itself is erased rather
        // than either half being decremented.
        if code_ & 0xf_0000 == code::COUNTER_PERMIT && ty & effect_type::SINGLE != 0 {
            let counter_type = (code_ & 0xffff) as u16;
            if let Some(halves) = self.cards[card].counters.remove(&counter_type) {
                let c = &self.cards[card];
                self.messages.push(crate::field::Message::RemoveCounter {
                    counter_type,
                    controller: c.current.controller,
                    location: c.current.location,
                    sequence: c.current.sequence,
                    count: halves[0] + halves[1],
                });
            }
        }

        // A client hint going away is announced, as its arrival was.
        if self
            .effects
            .get(id)
            .is_some_and(|e| e.is_flag(flag::CLIENT_HINT))
        {
            let info = self.get_info_location(card);
            self.messages.push(crate::field::Message::CardHint {
                controller: info.controller,
                location: info.location,
                sequence: info.sequence,
                kind: crate::field::chint::DESC_REMOVE,
                value: description,
            });
        }

        if code_ == code::UNIQUE_CHECK {
            self.remove_unique_card(card);
            let c = &mut self.cards[card];
            c.unique_pos = [0, 0];
            c.unique_code = 0;
        }

        self.core.reseted_effects.insert(id);
    }

    /// `field::remove_unique_card` — drop a card from the unique registry.
    ///
    /// `unique_pos` says *which sides* the uniqueness applies to, so the
    /// card can be registered under either player, or both, and each is
    /// removed independently. A card with no controller was never in the
    /// registry.
    pub fn remove_unique_card(&mut self, card: CardId) {
        let con = self.cards[card].current.controller;
        if con == PLAYER_NONE {
            return;
        }
        let pos = self.cards[card].unique_pos;
        if pos[0] != 0 {
            self.core.unique_cards[con as usize].retain(|&c| c != card);
        }
        if pos[1] != 0 {
            self.core.unique_cards[1 - con as usize].retain(|&c| c != card);
        }
    }

    /// Re-register a card's printed effects after a copied one was removed.
    ///
    /// The reference calls the card's Lua `initial_effect` between two
    /// `STATUS_INITIALIZING` flips. This port has no Lua and no card pool
    /// yet, so it **panics** rather than silently doing nothing — nothing
    /// can reach it, since `copy_effect` is not ported either and no effect
    /// can have a non-zero `copy_id`.
    pub fn reinitialize_card_effects(&mut self, _card: CardId) {
        unimplemented!("re-registering printed effects needs copy_effect and the card pool")
    }
}

#[cfg(test)]
mod tests {
    /// **A disable reset announces the temporary counters it removes.**
    /// The reference's `RESET_DISABLE` block writes `MSG_REMOVE_COUNTER`
    /// with the temporary half's count for every counter that has one and
    /// keeps the permanent half; the port cleared silently.
    #[test]
    fn a_disable_reset_announces_lost_temporary_counters() {
        use crate::card::{Card, CardData};
        use crate::field::{reset, Field, Message};
        let mut f = Field::new(8000);
        let c = f.new_card(Card::with_data(CardData::default(), 0));
        f.add_card(0, c, crate::board::location::MZONE, 2, false);
        f.cards[c].counters.insert(1, [0, 1]); // one temporary Spell Counter
        f.cards[c].counters.insert(2, [2, 0]); // two permanent counters of another kind
        f.reset_card(c, reset::DISABLE, reset::EVENT);
        let removed: Vec<_> = f
            .messages
            .iter()
            .filter_map(|m| match m {
                Message::RemoveCounter {
                    counter_type,
                    controller,
                    location,
                    sequence,
                    count,
                } => Some((*counter_type, *controller, *location, *sequence, *count)),
                _ => None,
            })
            .collect();
        assert_eq!(
            removed,
            vec![(1, 0, crate::board::location::MZONE, 2, 1)],
            "the temporary one, announced where the card sits"
        );
        assert_eq!(f.cards[c].counters.get(&1), None, "and gone");
        assert_eq!(
            f.cards[c].counters.get(&2),
            Some(&[2, 0]),
            "the permanent ones stay, silently"
        );
    }

    use super::*;
    use crate::board::{location, position};
    use crate::card::{card_type, Card, CardData};
    use crate::effect::Effect;

    fn monster(f: &mut Field, controller: u8, seat: usize) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 18036057,
                type_: card_type::MONSTER | card_type::EFFECT,
                ..Default::default()
            },
            controller,
        );
        c.current.controller = controller;
        c.current.position = position::FACEUP_ATTACK;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(controller, id, location::MZONE, seat as u32, false);
        id
    }

    /// A single effect on the card itself: the simplest way to give it a
    /// `DISABLE` or `FORBIDDEN`.
    fn single_on(f: &mut Field, card: CardId, code_: u32) -> EffectId {
        let mut e = Effect::new(effect_type::SINGLE, code_);
        e.owner = Some(card);
        e.handler = Some(card);
        let id = f.new_effect(e);
        f.cards[card].single_effect.insert(code_, id);
        f.cards[card].indexer.insert(id);
        id
    }

    mod refresh {
        use super::*;

        #[test]
        fn a_disable_effect_sets_the_status() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            assert!(!f.cards[c].is_status(status::DISABLED));

            single_on(&mut f, c, code::DISABLE);
            f.refresh_disable_status(c);
            assert!(f.cards[c].is_status(status::DISABLED));
        }

        /// `CANNOT_DISABLE` beats `DISABLE` — and there is no such override
        /// for `FORBIDDEN`, which is the asymmetry worth pinning.
        #[test]
        fn cannot_disable_beats_disable_but_not_forbidden() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            single_on(&mut f, c, code::DISABLE);
            single_on(&mut f, c, code::CANNOT_DISABLE);
            single_on(&mut f, c, code::FORBIDDEN);
            f.refresh_disable_status(c);

            assert!(!f.cards[c].is_status(status::DISABLED), "overridden");
            assert!(
                f.cards[c].is_status(status::FORBIDDEN),
                "forbidden has no override"
            );
        }

        /// Refreshing clears a status that no longer applies, rather than
        /// only ever setting one.
        #[test]
        fn refreshing_clears_a_status_that_no_longer_applies() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            let e = single_on(&mut f, c, code::DISABLE);
            f.refresh_disable_status(c);
            assert!(f.cards[c].is_status(status::DISABLED));

            f.remove_card_effect(c, e);
            f.refresh_disable_status(c);
            assert!(!f.cards[c].is_status(status::DISABLED));
        }

        /// Immunity is rebuilt before the two questions, because both run
        /// through it.
        ///
        /// The second half of this is a rule that is easy to miss: a card's
        /// *own* single effect is immunity-checked only when it carries
        /// `EFFECT_FLAG_SINGLE_RANGE`. Without the flag the walk
        /// short-circuits — `!SINGLE_RANGE || is_affect_by_effect(...)` —
        /// so a card cannot be immune to an effect printed on itself.
        #[test]
        fn immunity_is_rebuilt_first() {
            fn immune_to_everything(
                _: &crate::effect::Effect,
                _: &Field,
                _: &crate::effect::Ctx,
            ) -> i64 {
                1
            }
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            single_on(&mut f, c, code::DISABLE);

            let mut immunity = Effect::new(effect_type::SINGLE, code::IMMUNE_EFFECT);
            immunity.owner = Some(c);
            immunity.handler = Some(c);
            immunity.flag[0] |= flag::FUNC_VALUE;
            immunity.value_fn = Some(immune_to_everything);
            let immunity = f.new_effect(immunity);
            f.cards[c]
                .single_effect
                .insert(code::IMMUNE_EFFECT, immunity);

            assert!(f.cards[c].immune_effect.is_empty(), "not yet gathered");
            f.refresh_disable_status(c);
            assert_eq!(f.cards[c].immune_effect, vec![immunity], "gathered first");
            assert!(
                f.cards[c].is_status(status::DISABLED),
                "a card's own single effect is not immunity-checked without                  SINGLE_RANGE, so the immunity does not apply"
            );

            // With the flag, the walk consults immunity and the disable is
            // shrugged off.
            let disable = f.cards[c].single_effect.equal_range(code::DISABLE)[0];
            f.effects.get_mut(disable).unwrap().flag[0] |= flag::SINGLE_RANGE;
            f.effects.get_mut(disable).unwrap().range = u16::from(location::MZONE);
            f.refresh_disable_status(c);
            assert!(!f.cards[c].is_status(status::DISABLED));
        }
    }

    mod check_list {
        use super::*;

        #[test]
        fn adjusting_settles_the_cards_on_the_list() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            single_on(&mut f, c, code::DISABLE);

            assert!(!f.cards[c].is_status(status::DISABLED), "not yet asked");
            f.add_to_disable_check_list(c);
            f.adjust_disable_check_list();
            assert!(f.cards[c].is_status(status::DISABLED));
            assert!(
                f.field_effects.disable_check_set.is_empty(),
                "and the list is consumed"
            );
        }

        /// The loop guard is **all-bit**: `field.cpp:2101` tests
        /// `is_status(STATUS_TO_ENABLE | STATUS_TO_DISABLE)`, which the
        /// reference defines as every bit set. A card carrying **one** mark
        /// is therefore not skipped — it is refreshed like any other, and
        /// the mark is cleared on the way out. Until 2026-09-14 this port
        /// read the guard as any-bit and this test asserted that reading.
        #[test]
        fn a_single_mark_does_not_guard() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            single_on(&mut f, c, code::DISABLE);

            f.cards[c].set_status(status::TO_DISABLE, true);
            f.add_to_disable_check_list(c);
            f.adjust_disable_check_list();
            assert!(
                f.cards[c].is_status(status::DISABLED),
                "refreshed despite the single mark"
            );
            assert!(
                !f.cards[c].get_status(status::TO_ENABLE | status::TO_DISABLE),
                "and both marks are cleared on the way out"
            );
        }

        /// Both marks together are the guard, and it lasts exactly one
        /// pass: the marks are cleared at the end of the card's turn
        /// through the loop, so asked again it settles.
        #[test]
        fn both_marks_guard_one_pass_and_are_then_cleared() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            single_on(&mut f, c, code::DISABLE);

            f.cards[c].set_status(status::TO_ENABLE | status::TO_DISABLE, true);
            f.add_to_disable_check_list(c);
            f.adjust_disable_check_list();
            assert!(
                !f.cards[c].is_status(status::DISABLED),
                "skipped while carrying both marks"
            );
            assert!(
                !f.cards[c].get_status(status::TO_ENABLE | status::TO_DISABLE),
                "and the marks are cleared on the way out"
            );

            f.add_to_disable_check_list(c);
            f.adjust_disable_check_list();
            assert!(f.cards[c].is_status(status::DISABLED));
        }

        /// A field effect puts every card in its range on the list; an
        /// effect aimed at players puts none, because it cannot change a
        /// card's disabled state.
        #[test]
        fn a_field_effect_reaches_its_whole_range() {
            let mut f = Field::new(8000);
            let source = monster(&mut f, 0, 0);
            let a = monster(&mut f, 0, 1);
            let b = monster(&mut f, 1, 0);

            let mut e = Effect::new(effect_type::FIELD, code::DISABLE);
            e.owner = Some(source);
            e.handler = Some(source);
            e.range = u16::from(location::MZONE);
            e.s_range = u16::from(location::MZONE);
            let e = f.new_effect(e);

            let reached = f.filter_affected_cards(e);
            assert!(reached.contains(&a), "own side is in s_range");
            assert!(reached.contains(&source));
            assert!(!reached.contains(&b), "the other side is not");

            f.effects.get_mut(e).unwrap().o_range = u16::from(location::MZONE);
            assert!(f.filter_affected_cards(e).contains(&b), "now it is");

            f.effects.get_mut(e).unwrap().flag[0] |= flag::PLAYER_TARGET;
            assert!(
                f.filter_affected_cards(e).is_empty(),
                "a player effect reaches no cards"
            );
        }

        /// An action effect reaches nothing either — it has to be activated,
        /// so it cannot be quietly disabling anyone.
        #[test]
        fn an_action_effect_reaches_nothing() {
            let mut f = Field::new(8000);
            let source = monster(&mut f, 0, 0);
            let mut e = Effect::new(effect_type::FIELD | effect_type::ACTIONS, code::DISABLE);
            e.owner = Some(source);
            e.handler = Some(source);
            e.s_range = u16::from(location::MZONE);
            let e = f.new_effect(e);
            assert!(f.filter_affected_cards(e).is_empty());
        }
    }

    mod resets {
        use super::*;

        /// `RESET_DISABLE` is `0x10000`, which is in none of the location
        /// masks — so a disable reset skips all of those blocks. The
        /// temporary half of the counters goes; the permanent half stays.
        #[test]
        fn a_disable_reset_takes_only_the_temporary_counters() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            f.cards[c].counters.insert(0x1, [2, 3]);
            f.cards[c].counters.insert(0x2, [0, 1]);

            f.reset_card(c, reset::DISABLE, reset::EVENT);
            assert_eq!(
                f.cards[c].counters.get(&0x1),
                Some(&[2, 0]),
                "the permanent half survives"
            );
            assert_eq!(
                f.cards[c].counters.get(&0x2),
                None,
                "a counter with nothing left is dropped"
            );
        }

        /// An effect resets only if it says it resets on this event *and*
        /// the level matches its flag.
        #[test]
        fn an_effect_resets_only_when_its_flag_matches() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            let e = single_on(&mut f, c, code::DISABLE);

            assert!(
                !f.effect_resets(e, reset::DISABLE, reset::EVENT),
                "no RESET_EVENT flag at all"
            );

            f.effects.get_mut(e).unwrap().reset_flag = reset::EVENT | reset::TOGRAVE;
            assert!(
                !f.effect_resets(e, reset::DISABLE, reset::EVENT),
                "resets on leaving, not on being disabled"
            );

            f.effects.get_mut(e).unwrap().reset_flag = reset::EVENT | reset::DISABLE;
            assert!(f.effect_resets(e, reset::DISABLE, reset::EVENT));
        }

        /// The one to read twice: an effect whose owner is not its handler
        /// has `RESET_DISABLE` stripped from the level. Disabling the card
        /// an effect *sits on* does not reset an effect that belongs to
        /// another card.
        #[test]
        fn a_granted_effect_survives_its_hosts_disabling() {
            let mut f = Field::new(8000);
            let host = monster(&mut f, 0, 0);
            let granter = monster(&mut f, 0, 1);

            let mut e = Effect::new(effect_type::SINGLE, code::DISABLE);
            e.owner = Some(granter);
            e.handler = Some(host);
            e.reset_flag = reset::EVENT | reset::DISABLE;
            let e = f.new_effect(e);

            assert!(
                !f.effect_resets(e, reset::DISABLE, reset::EVENT),
                "granted: the disable level is stripped"
            );

            f.effects.get_mut(e).unwrap().owner = Some(host);
            assert!(
                f.effect_resets(e, reset::DISABLE, reset::EVENT),
                "its own: it resets"
            );
        }

        /// Becoming disabled removes the card's own resettable effects.
        #[test]
        fn becoming_disabled_takes_the_cards_own_effects() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            single_on(&mut f, c, code::DISABLE);
            let doomed = single_on(&mut f, c, 999);
            f.effects.get_mut(doomed).unwrap().reset_flag = reset::EVENT | reset::DISABLE;

            f.add_to_disable_check_list(c);
            f.adjust_disable_check_list();
            assert!(f.cards[c].is_status(status::DISABLED));
            assert!(
                !f.cards[c].indexer.contains(&doomed),
                "the effect reset with the disabling"
            );
        }
    }

    /// `card::remove_effect`: what removing an effect has to clean up.
    mod remove_card_effect {
        use super::*;
        use crate::effect::{effect_type, flag};
        use crate::field::reset;

        /// Build an effect of an arbitrary type and put it on the card's
        /// matching container plus its indexer.
        fn effect_of(f: &mut Field, card: CardId, ty: u16, code_: u32) -> EffectId {
            let mut e = Effect::new(ty, code_);
            e.owner = Some(card);
            e.handler = Some(card);
            e.effect_owner = f.cards[card].current.controller;
            let id = f.new_effect(e);
            let c = &mut f.cards[card];
            if ty & effect_type::SINGLE != 0 {
                c.single_effect.insert(code_, id);
            } else if ty & effect_type::EQUIP != 0 {
                c.equip_effect.insert(code_, id);
            } else if ty & effect_type::TARGET != 0 {
                c.target_effect.insert(code_, id);
            } else if ty & effect_type::XMATERIAL != 0 {
                c.xmaterial_effect.insert(code_, id);
            } else {
                c.field_effect.insert(code_, id);
            }
            c.indexer.insert(id);
            id
        }

        /// A single effect queues the card it was on.
        #[test]
        fn a_single_effect_queues_its_own_card() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            let e = effect_of(&mut f, c, effect_type::SINGLE, code::DISABLE);
            f.field_effects.disable_check_set.clear();

            f.remove_card_effect(c, e);
            assert!(f.field_effects.disable_check_set.contains(&c));
            assert!(!f.cards[c].indexer.contains(&e), "and leaves the indexer");
        }

        /// An equip effect queues the **target**, not the equip card.
        #[test]
        fn an_equip_effect_queues_the_equip_target() {
            let mut f = Field::new(8000);
            let equip = monster(&mut f, 0, 0);
            let target = monster(&mut f, 0, 1);
            f.cards[equip].equiping_target = Some(target);

            let e = effect_of(&mut f, equip, effect_type::EQUIP, code::DISABLE);
            f.field_effects.disable_check_set.clear();

            f.remove_card_effect(equip, e);
            assert!(
                f.field_effects.disable_check_set.contains(&target),
                "the target is what was being disabled"
            );
            assert!(!f.field_effects.disable_check_set.contains(&equip));
        }

        /// With the relationship already gone the set is **cleared**, not
        /// left as the card itself — so nothing is queued at all. Reading
        /// the "if any" as a fallback to the card is the natural mistake.
        #[test]
        fn an_equip_effect_with_no_target_queues_nobody() {
            let mut f = Field::new(8000);
            let equip = monster(&mut f, 0, 0);
            let e = effect_of(&mut f, equip, effect_type::EQUIP, code::DISABLE);
            f.field_effects.disable_check_set.clear();

            f.remove_card_effect(equip, e);
            assert!(
                f.field_effects.disable_check_set.is_empty(),
                "no target means no queue, not a fallback to the card"
            );
        }

        #[test]
        fn a_target_effect_queues_every_card_it_targets() {
            let mut f = Field::new(8000);
            let owner = monster(&mut f, 0, 0);
            let a = monster(&mut f, 0, 1);
            let b = monster(&mut f, 0, 2);
            f.cards[owner].effect_target_cards = vec![a, b];

            let e = effect_of(&mut f, owner, effect_type::TARGET, code::DISABLE);
            f.field_effects.disable_check_set.clear();

            f.remove_card_effect(owner, e);
            assert!(f.field_effects.disable_check_set.contains(&a));
            assert!(f.field_effects.disable_check_set.contains(&b));
            assert!(!f.field_effects.disable_check_set.contains(&owner));
        }

        #[test]
        fn an_xmaterial_effect_queues_the_monster_above_it() {
            let mut f = Field::new(8000);
            let xyz = monster(&mut f, 0, 0);
            let mat = monster(&mut f, 0, 1);
            f.cards[mat].overlay_target = Some(xyz);

            let e = effect_of(&mut f, mat, effect_type::XMATERIAL, code::DISABLE);
            f.field_effects.disable_check_set.clear();

            f.remove_card_effect(mat, e);
            assert!(f.field_effects.disable_check_set.contains(&xyz));
            assert!(!f.field_effects.disable_check_set.contains(&mat));
        }

        /// An effect that is not disable-related queues nobody, whatever its
        /// type.
        #[test]
        fn an_unrelated_effect_queues_nobody() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            let e = effect_of(&mut f, c, effect_type::SINGLE, code::UPDATE_ATTACK);
            f.field_effects.disable_check_set.clear();

            f.remove_card_effect(c, e);
            assert!(f.field_effects.disable_check_set.is_empty());
        }

        /// The registry cleanups here are **not** gated on `FIELD_ONLY`,
        /// unlike the ones in `field::remove_effect`: this is the card's own
        /// effect going away for good.
        #[test]
        fn the_registries_are_cleaned_without_a_field_only_flag() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            let mut e = Effect::new(effect_type::SINGLE, code::UPDATE_ATTACK);
            e.owner = Some(c);
            e.handler = Some(c);
            e.reset_flag = reset::PHASE | reset::CHAIN;
            e.flag[0] |= flag::OATH | flag::COUNT_LIMIT;
            let id = f.new_effect(e);
            f.cards[c].single_effect.insert(code::UPDATE_ATTACK, id);
            f.cards[c].indexer.insert(id);
            f.field_effects.oath.insert(id, None);
            f.field_effects.pheff.insert(id);
            f.field_effects.cheff.insert(id);
            f.field_effects.rechargeable.insert(id);

            assert!(
                !f.effects
                    .get(id)
                    .is_some_and(|e| e.is_flag(flag::FIELD_ONLY)),
                "no FIELD_ONLY flag — the cleanup must happen anyway"
            );
            f.remove_card_effect(c, id);
            assert!(!f.field_effects.oath.contains_key(&id));
            assert!(!f.field_effects.pheff.contains(&id));
            assert!(!f.field_effects.cheff.contains(&id));
            assert!(!f.field_effects.rechargeable.contains(&id));
            assert!(f.core.reseted_effects.contains(&id));
        }

        /// A permit effect going away erases the whole counter slot — both
        /// halves, not a decrement of either.
        #[test]
        fn a_permit_effect_takes_its_counters_with_it() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            let ct: u16 = 0x12;
            let e = effect_of(
                &mut f,
                c,
                effect_type::SINGLE,
                code::COUNTER_PERMIT + u32::from(ct),
            );
            f.cards[c].counters.insert(ct, [3, 2]);

            f.remove_card_effect(c, e);
            assert!(
                !f.cards[c].counters.contains_key(&ct),
                "the slot goes, both halves with it"
            );
        }

        /// The same code on a *field* effect leaves the counters alone: the
        /// reference requires `EFFECT_TYPE_SINGLE` as well.
        #[test]
        fn a_permit_effect_that_is_not_single_leaves_counters_alone() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            let ct: u16 = 0x12;
            let e = effect_of(
                &mut f,
                c,
                effect_type::FIELD,
                code::COUNTER_PERMIT + u32::from(ct),
            );
            f.cards[c].counters.insert(ct, [3, 2]);

            f.remove_card_effect(c, e);
            assert_eq!(f.cards[c].counters.get(&ct), Some(&[3, 2]));
        }

        #[test]
        fn a_unique_check_effect_deregisters_the_card() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            f.cards[c].unique_pos = [1, 1];
            f.cards[c].unique_code = 18036057;
            f.core.unique_cards[0].push(c);
            f.core.unique_cards[1].push(c);

            let e = effect_of(&mut f, c, effect_type::SINGLE, code::UNIQUE_CHECK);
            f.remove_card_effect(c, e);

            assert!(f.core.unique_cards[0].is_empty());
            assert!(f.core.unique_cards[1].is_empty());
            assert_eq!(f.cards[c].unique_pos, [0, 0]);
            assert_eq!(f.cards[c].unique_code, 0);
        }

        /// `unique_pos` says which sides the uniqueness applies to, and each
        /// is removed independently — a one-sided registration leaves the
        /// other player's list untouched.
        #[test]
        fn only_the_registered_sides_are_removed() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            let other = monster(&mut f, 1, 0);
            f.cards[c].unique_pos = [1, 0];
            f.core.unique_cards[0].push(c);
            f.core.unique_cards[1].push(other);

            f.remove_unique_card(c);
            assert!(f.core.unique_cards[0].is_empty());
            assert_eq!(
                f.core.unique_cards[1],
                vec![other],
                "side 1 was never this card's registration"
            );
        }

        #[test]
        fn a_card_with_no_controller_was_never_registered() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            f.cards[c].unique_pos = [1, 1];
            f.cards[c].current.controller = PLAYER_NONE;
            f.core.unique_cards[0].push(c);

            f.remove_unique_card(c);
            assert_eq!(f.core.unique_cards[0], vec![c], "nothing to remove");
        }
    }
}

#[cfg(test)]
mod enable_tests {
    use super::*;
    use crate::board::{location, position};
    use crate::card::{card_type, Card, CardData};
    use crate::effect::{effect_type, Effect};

    fn on_field(f: &mut Field, loc: u8, seat: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 18036057,
                type_: card_type::MONSTER | card_type::EFFECT,
                ..Default::default()
            },
            0,
        );
        c.current.controller = 0;
        c.current.position = position::FACEUP_ATTACK;
        let id = f.new_card(c);
        f.add_card(0, id, loc, seat, false);
        id
    }

    fn single_on(f: &mut Field, card: CardId, code_: u32) -> EffectId {
        let mut e = Effect::new(effect_type::SINGLE, code_);
        e.owner = Some(card);
        e.handler = Some(card);
        let id = f.new_effect(e);
        f.cards[card].single_effect.insert(code_, id);
        f.cards[card].indexer.insert(id);
        id
    }

    fn effect_on(f: &mut Field, card: CardId, container: &str, range: u16, flags: u32) -> EffectId {
        let mut e = Effect::new(effect_type::SINGLE, 999);
        e.owner = Some(card);
        e.handler = Some(card);
        e.range = range;
        e.flag[0] |= flags;
        let id = f.new_effect(e);
        match container {
            "single" => f.cards[card].single_effect.insert(999, id),
            "field" => f.cards[card].field_effect.insert(999, id),
            "equip" => f.cards[card].equip_effect.insert(999, id),
            _ => f.cards[card].target_effect.insert(999, id),
        }
        f.cards[card].indexer.insert(id);
        id
    }

    /// Nothing happens unless the status actually changes — which matters,
    /// because the body renumbers effect ids.
    #[test]
    fn only_a_real_transition_does_anything() {
        let mut f = Field::new(8000);
        let c = on_field(&mut f, location::MZONE, 0);
        let e = effect_on(&mut f, c, "field", u16::from(location::MZONE), 0);
        let before = f.effects.get(e).unwrap().id.get();

        // Already disabled: disabling again is a no-op.
        f.enable_field_effect(c, false);
        assert_eq!(f.effects.get(e).unwrap().id.get(), before);

        f.enable_field_effect(c, true);
        assert!(f.cards[c].is_status(status::EFFECT_ENABLED));
        let after = f.effects.get(e).unwrap().id.get();
        assert!(after > before, "enabling renumbers");

        // Enabling again changes nothing.
        f.enable_field_effect(c, true);
        assert_eq!(f.effects.get(e).unwrap().id.get(), after);
    }

    /// The renumbering is the point: effect id is what every effect sort
    /// keys on, so a card that settles later has its effects apply later
    /// among equals.
    #[test]
    fn a_card_that_settles_later_sorts_later() {
        let mut f = Field::new(8000);
        let first = on_field(&mut f, location::MZONE, 0);
        let second = on_field(&mut f, location::MZONE, 1);
        let a = effect_on(&mut f, first, "field", u16::from(location::MZONE), 0);
        let b = effect_on(&mut f, second, "field", u16::from(location::MZONE), 0);

        // b was created second, so it already sorts later.
        f.enable_field_effect(second, true);
        f.enable_field_effect(first, true);

        let mut order = vec![a, b];
        f.sort_by_effect_id(&mut order);
        assert_eq!(
            order,
            vec![b, a],
            "the card enabled last sorts last, whatever the creation order"
        );
    }

    /// The four containers are treated differently. A single effect is
    /// renumbered only if it is range-limited *and* in range; an equip
    /// effect only while the card is in a spell/trap zone, with no range
    /// test at all.
    #[test]
    fn the_containers_are_treated_differently() {
        let mut f = Field::new(8000);
        let c = on_field(&mut f, location::MZONE, 0);
        let plain_single = effect_on(&mut f, c, "single", u16::from(location::MZONE), 0);
        let ranged_single = effect_on(
            &mut f,
            c,
            "single",
            u16::from(location::MZONE),
            flag::SINGLE_RANGE,
        );
        let out_of_range = effect_on(&mut f, c, "field", u16::from(location::GRAVE), 0);
        let before: Vec<u32> = [plain_single, ranged_single, out_of_range]
            .iter()
            .map(|&e| f.effects.get(e).unwrap().id.get())
            .collect();

        f.enable_field_effect(c, true);

        assert_eq!(
            f.effects.get(plain_single).unwrap().id.get(),
            before[0],
            "a single effect without SINGLE_RANGE is left alone"
        );
        assert!(
            f.effects.get(ranged_single).unwrap().id.get() > before[1],
            "with the flag and in range, it is renumbered"
        );
        assert_eq!(
            f.effects.get(out_of_range).unwrap().id.get(),
            before[2],
            "a field effect out of range is left alone"
        );
    }

    #[test]
    fn an_equip_effect_is_renumbered_only_in_a_spell_trap_zone() {
        let mut f = Field::new(8000);
        let in_mzone = on_field(&mut f, location::MZONE, 0);
        let e1 = effect_on(&mut f, in_mzone, "equip", 0, 0);
        let before1 = f.effects.get(e1).unwrap().id.get();
        f.enable_field_effect(in_mzone, true);
        assert_eq!(
            f.effects.get(e1).unwrap().id.get(),
            before1,
            "not in a S/T zone"
        );

        let in_szone = on_field(&mut f, location::SZONE, 0);
        let e2 = effect_on(&mut f, in_szone, "equip", 0, 0);
        let before2 = f.effects.get(e2).unwrap().id.get();
        f.enable_field_effect(in_szone, true);
        assert!(
            f.effects.get(e2).unwrap().id.get() > before2,
            "in a S/T zone, and with no range test"
        );
    }

    /// A disabled or forbidden card does not propagate.
    #[test]
    fn a_disabled_card_does_not_propagate() {
        let mut f = Field::new(8000);
        let c = on_field(&mut f, location::MZONE, 0);
        let other = on_field(&mut f, location::MZONE, 1);
        // A field effect of c's that reaches `other`.
        let mut e = Effect::new(effect_type::FIELD, code::DISABLE);
        e.owner = Some(c);
        e.handler = Some(c);
        e.range = u16::from(location::MZONE);
        e.s_range = u16::from(location::MZONE);
        let e = f.new_effect(e);
        f.cards[c].indexer.insert(e);

        // A real FORBIDDEN effect, not just the bit: `enable_field_effect`
        // calls `refresh_disable_status` first, which recomputes the status
        // from the effects present — so setting the bit by hand is undone
        // before it is read. That is the machinery working, and it is why
        // the bit is never the place to set this.
        single_on(&mut f, c, code::FORBIDDEN);
        f.enable_field_effect(c, true);
        assert!(f.cards[c].is_status(status::FORBIDDEN));
        assert!(
            !f.field_effects.disable_check_set.contains(&other),
            "a forbidden card propagates nothing"
        );
    }

    mod oaths {
        use super::*;

        fn oath(f: &mut Field, reason: EffectId) -> EffectId {
            let mut e = Effect::new(effect_type::FIELD, 1234);
            e.flag[0] |= flag::FIELD_ONLY | flag::OATH;
            let id = f.new_effect(e);
            f.field_effects.oath.insert(id, Some(reason));
            f.register_effect(id);
            id
        }

        /// Negating the activation unmakes the promise it paid.
        #[test]
        fn a_negated_activation_takes_its_oath_back() {
            let mut f = Field::new(8000);
            let activation = f.new_effect(Effect::new(effect_type::ACTIVATE, 1));
            let promise = oath(&mut f, activation);

            f.remove_oath_effect(activation);
            assert!(f.field_effects.oath.is_empty());
            assert!(
                !f.field_effects.indexer.contains(&promise),
                "and the effect is out of play"
            );
        }

        /// Releasing keeps the effect and clears only its reason: the
        /// promise still stands and still resets at end of turn, it has
        /// merely stopped being refundable.
        #[test]
        fn releasing_keeps_the_promise_and_drops_the_refund() {
            let mut f = Field::new(8000);
            let activation = f.new_effect(Effect::new(effect_type::ACTIVATE, 1));
            let promise = oath(&mut f, activation);

            f.release_oath_relation(activation);
            assert_eq!(f.field_effects.oath.get(&promise), Some(&None));
            assert!(f.field_effects.indexer.contains(&promise), "still in play");

            f.remove_oath_effect(activation);
            assert!(
                f.field_effects.indexer.contains(&promise),
                "and no longer refundable"
            );
        }

        /// Only the promises of *that* activation are taken back.
        #[test]
        fn another_activations_oath_is_untouched() {
            let mut f = Field::new(8000);
            let a = f.new_effect(Effect::new(effect_type::ACTIVATE, 1));
            let b = f.new_effect(Effect::new(effect_type::ACTIVATE, 2));
            oath(&mut f, a);
            let theirs = oath(&mut f, b);

            f.remove_oath_effect(a);
            assert_eq!(
                f.field_effects.oath.keys().copied().collect::<Vec<_>>(),
                vec![theirs]
            );
        }
    }

    /// `reset_chain` takes the once-per-chain tallies and every effect that
    /// registered as resetting on a chain — which is what `cheff` is for.
    #[test]
    fn resetting_the_chain_takes_the_chain_scoped_effects() {
        let mut f = Field::new(8000);
        let mut e = Effect::new(effect_type::FIELD, 1234);
        e.flag[0] |= flag::FIELD_ONLY;
        e.reset_flag = reset::CHAIN;
        let chain_scoped = f.new_effect(e);
        f.add_effect(chain_scoped, 0);
        assert!(f.field_effects.cheff.contains(&chain_scoped));

        f.add_effect_code(99, crate::effect::effect_count::CHAIN, 0, 0);
        assert_eq!(
            f.get_effect_code(99, crate::effect::effect_count::CHAIN, 0, 0),
            1
        );

        f.reset_chain();
        assert!(!f.field_effects.indexer.contains(&chain_scoped), "gone");
        assert_eq!(
            f.get_effect_code(99, crate::effect::effect_count::CHAIN, 0, 0),
            0,
            "and the once-per-chain tally is clear"
        );
    }

    mod resets {
        use super::super::*;
        use crate::board::{location, position};
        use crate::card::{card_type, Card, CardData};
        use crate::effect::{effect_type, Effect};
        use crate::field::reset;

        fn monster(f: &mut Field, controller: u8, seat: usize) -> CardId {
            let mut c = Card::with_data(
                CardData {
                    code: 18036057,
                    type_: card_type::MONSTER | card_type::EFFECT,
                    ..Default::default()
                },
                controller,
            );
            c.current.controller = controller;
            c.current.position = position::FACEUP_ATTACK;
            c.set_status(status::EFFECT_ENABLED, true);
            let id = f.new_card(c);
            f.add_card(controller, id, location::MZONE, seat as u32, false);
            id
        }

        /// Register `e` on `card` under `code_`, in `single_effect`.
        fn attach(f: &mut Field, card: CardId, e: Effect, code_: u32) -> EffectId {
            let id = f.new_effect(e);
            f.cards[card].single_effect.insert(code_, id);
            f.cards[card].indexer.insert(id);
            id
        }

        /// **A code reset takes a plain single effect with that code, and
        /// nothing else.**
        ///
        /// The two extra tests are what keeps it from taking a real
        /// effect that happens to share the number: a flag effect is
        /// always a plain single, never an action and never a field
        /// effect.
        #[test]
        fn a_code_reset_takes_plain_singles_only() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            const CODE: u32 = 0x1000_0002;

            let mut plain = Effect::new(effect_type::SINGLE, CODE);
            plain.owner = Some(c);
            plain.handler = Some(c);
            let plain = attach(&mut f, c, plain, CODE);

            // `ACTIONS` spelled out: `Effect::new` takes the type
            // verbatim, where `Effect.SetType` derives the bit from the
            // action-shaped types. The reference's test is on the **bit**.
            let mut action = Effect::new(
                effect_type::SINGLE | effect_type::TRIGGER_F | effect_type::ACTIONS,
                CODE,
            );
            action.owner = Some(c);
            action.handler = Some(c);
            let action = attach(&mut f, c, action, CODE);

            let mut field_eff = Effect::new(effect_type::FIELD, CODE);
            field_eff.owner = Some(c);
            field_eff.handler = Some(c);
            let field_eff = attach(&mut f, c, field_eff, CODE);

            let mut other = Effect::new(effect_type::SINGLE, CODE + 1);
            other.owner = Some(c);
            other.handler = Some(c);
            let other = attach(&mut f, c, other, CODE + 1);

            f.reset_card(c, CODE, reset::CODE);
            assert!(
                !f.cards[c].indexer.contains(&plain),
                "the plain single went"
            );
            assert!(f.cards[c].indexer.contains(&action), "an action stayed");
            assert!(
                f.cards[c].indexer.contains(&field_eff),
                "a field effect stayed"
            );
            assert!(
                f.cards[c].indexer.contains(&other),
                "a different code stayed"
            );
        }

        /// **A card reset matches on the owner's printed code**, which is
        /// a card number and not an index.
        #[test]
        fn a_card_reset_matches_the_owners_printed_code() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            let printed = f.cards[c].data.code;
            let mut e = Effect::new(effect_type::SINGLE, 500);
            e.owner = Some(c);
            e.handler = Some(c);
            let id = attach(&mut f, c, e, 500);
            f.reset_card(c, printed + 1, reset::CARD);
            assert!(f.cards[c].indexer.contains(&id), "a different card number");
            f.reset_card(c, printed, reset::CARD);
            assert!(!f.cards[c].indexer.contains(&id));
        }

        /// **A copy reset matches the copy id.**
        #[test]
        fn a_copy_reset_matches_the_copy_id() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            let mut e = Effect::new(effect_type::SINGLE, 500);
            e.owner = Some(c);
            e.handler = Some(c);
            e.copy_id = 7;
            let id = attach(&mut f, c, e, 500);
            f.reset_card(c, 6, reset::COPY);
            assert!(f.cards[c].indexer.contains(&id));
            f.reset_card(c, 7, reset::COPY);
            assert!(!f.cards[c].indexer.contains(&id));
        }

        /// **A phase reset is a countdown, and it ticks only on a turn
        /// the effect's own flags name.**
        ///
        /// Two counts and two turn scopings, because the count and the
        /// turn test are separate: an effect scoped to its owner's turn
        /// is not counted down on the opponent's, and one whose count has
        /// already reached zero goes whichever turn it is.
        #[test]
        fn a_phase_reset_counts_down_on_the_named_turns_only() {
            let end = u32::from(crate::duel::phases::END);

            // Two ticks needed, and both come on the owner's turns.
            let mut f = Field::new(8000);
            f.infos.turn_player = 0;
            let c = monster(&mut f, 0, 0);
            let mut e = Effect::new(effect_type::SINGLE, 500);
            e.owner = Some(c);
            e.handler = Some(c);
            e.reset_flag = reset::PHASE | reset::SELF_TURN | end;
            e.reset_count = 2;
            let id = attach(&mut f, c, e, 500);
            f.reset_card(c, end, reset::PHASE);
            assert!(f.cards[c].indexer.contains(&id), "one tick left");
            f.reset_card(c, end, reset::PHASE);
            assert!(!f.cards[c].indexer.contains(&id), "and now none");

            // The same effect, ticked on the opponent's turn: no tick.
            let mut f = Field::new(8000);
            f.infos.turn_player = 1;
            let c = monster(&mut f, 0, 0);
            let mut e = Effect::new(effect_type::SINGLE, 500);
            e.owner = Some(c);
            e.handler = Some(c);
            e.reset_flag = reset::PHASE | reset::SELF_TURN | end;
            e.reset_count = 1;
            let id = attach(&mut f, c, e, 500);
            f.reset_card(c, end, reset::PHASE);
            assert!(
                f.cards[c].indexer.contains(&id),
                "not the turn it is scoped to"
            );

            // And a different phase does not tick it either.
            let mut f = Field::new(8000);
            f.infos.turn_player = 0;
            let c = monster(&mut f, 0, 0);
            let mut e = Effect::new(effect_type::SINGLE, 500);
            e.owner = Some(c);
            e.handler = Some(c);
            e.reset_flag = reset::PHASE | reset::SELF_TURN | end;
            e.reset_count = 1;
            let id = attach(&mut f, c, e, 500);
            f.reset_card(c, u32::from(crate::duel::phases::STANDBY), reset::PHASE);
            assert!(f.cards[c].indexer.contains(&id), "a different phase");
        }

        /// **Arriving on the field restores the attacks-all target**,
        /// which is the permissive default and the reason an "attacks
        /// every monster" effect works for a monster that has just been
        /// summoned.
        ///
        /// A card starts with it `false` (the field is zero-initialised
        /// in the reference too); the `RESET_TOFIELD` the arrival raises
        /// is what turns it on.
        #[test]
        fn arriving_on_the_field_restores_the_attacks_all_target() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            f.cards[c].attack_all_target = false;
            f.reset_card(c, reset::TOFIELD, reset::EVENT);
            assert!(f.cards[c].attack_all_target);

            // And a reset that is none of the named bits leaves it alone.
            f.cards[c].attack_all_target = false;
            f.reset_card(c, reset::DISABLE, reset::EVENT);
            assert!(!f.cards[c].attack_all_target);
        }

        /// **The battle tallies go when the card leaves a zone**, and a
        /// turn Set is *not* one of the bits that clears them.
        #[test]
        fn the_battle_tallies_go_on_a_move_but_not_on_a_turn_set() {
            let dirty = |f: &mut Field, c: CardId| {
                f.cards[c].attack_announce_count = 3;
                f.cards[c].announce_count = 2;
                f.cards[c].attacked_count = 1;
                f.cards[c].announced_cards.add(Some(c), 7);
            };
            for (level, cleared) in [
                (reset::TOGRAVE, true),
                (reset::LEAVE, true),
                (reset::TOFIELD, true),
                (reset::TURN_SET, false),
                (reset::CONTROL, false),
            ] {
                let mut f = Field::new(8000);
                let c = monster(&mut f, 0, 0);
                dirty(&mut f, c);
                f.reset_card(c, level, reset::EVENT);
                assert_eq!(f.cards[c].announce_count == 0, cleared, "level {level:#x}");
                assert_eq!(f.cards[c].attack_announce_count == 0, cleared);
                assert_eq!(f.cards[c].attacked_count == 0, cleared);
                assert_eq!(
                    f.cards[c].announced_cards.is_empty(),
                    cleared,
                    "and the list of what it announced against"
                );
            }
        }

        /// **A turn Set *does* recharge the count limits and clear what
        /// the card has battled** — a wider set than the battle tallies
        /// above, and the difference is the content.
        #[test]
        fn a_turn_set_recharges_count_limits() {
            for (level, recharged) in [
                (reset::TURN_SET, true),
                (reset::TOFIELD, true),
                (reset::CONTROL, false),
                (reset::DISABLE, false),
            ] {
                let mut f = Field::new(8000);
                let c = monster(&mut f, 0, 0);
                let mut e = Effect::new(effect_type::SINGLE, 500);
                e.owner = Some(c);
                e.handler = Some(c);
                e.flag[0] |= flag::COUNT_LIMIT;
                e.count_limit = 0;
                e.count_limit_max = 2;
                let id = attach(&mut f, c, e, 500);
                f.cards[c].battled_cards.add(Some(c), 9);
                f.reset_card(c, level, reset::EVENT);
                assert_eq!(
                    f.effects.get(id).unwrap().count_limit,
                    if recharged { 2 } else { 0 },
                    "level {level:#x}"
                );
                assert_eq!(
                    f.cards[c].battled_cards.is_empty(),
                    recharged,
                    "and what it battled goes with the same set"
                );
            }
        }

        /// **A recharge restores the ceiling, and only for a
        /// count-limited effect.**
        #[test]
        fn a_recharge_restores_the_ceiling_of_limited_effects_only() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);

            let mut limited = Effect::new(effect_type::SINGLE, 500);
            limited.owner = Some(c);
            limited.handler = Some(c);
            limited.flag[0] |= flag::COUNT_LIMIT;
            limited.count_limit = 0;
            limited.count_limit_max = 3;
            let limited = attach(&mut f, c, limited, 500);

            let mut plain = Effect::new(effect_type::SINGLE, 501);
            plain.owner = Some(c);
            plain.handler = Some(c);
            plain.count_limit = 0;
            plain.count_limit_max = 3;
            let plain = attach(&mut f, c, plain, 501);

            f.reset_effect_count(c);
            assert_eq!(
                f.effects.get(limited).unwrap().count_limit,
                3,
                "restored to the ceiling, not incremented"
            );
            assert_eq!(
                f.effects.get(plain).unwrap().count_limit,
                0,
                "and an effect with no count limit is left alone"
            );
        }

        /// **Counters go on a narrower set than the battle tallies**:
        /// leaving the field by a plain `RESET_LEAVE` keeps them, where a
        /// move to a graveyard does not.
        #[test]
        fn counters_survive_a_plain_leave() {
            for (level, cleared) in [
                (reset::TOGRAVE, true),
                (reset::TOFIELD, true),
                (reset::TURN_SET, true),
                (reset::LEAVE, false),
            ] {
                let mut f = Field::new(8000);
                let c = monster(&mut f, 0, 0);
                f.cards[c].counters.insert(0x1, [2, 0]);
                f.reset_card(c, level, reset::EVENT);
                assert_eq!(f.cards[c].counters.is_empty(), cleared, "level {level:#x}");
            }
        }

        /// **The relate-effect list goes when the card *moves*, and not
        /// when it merely leaves a zone.**
        ///
        /// `RESET_LEAVE` is deliberately absent from that set: a monster
        /// destroyed in battle is still related to the chain that is
        /// resolving over it.
        #[test]
        fn the_relate_effect_list_goes_on_a_move_only() {
            for (level, cleared) in [
                (reset::TOGRAVE, true),
                (reset::TOHAND, true),
                (reset::OVERLAY, true),
                (reset::LEAVE, false),
                (reset::TOFIELD, false),
            ] {
                let mut f = Field::new(8000);
                let c = monster(&mut f, 0, 0);
                f.cards[c].create_chain_relation(0, 11);
                f.reset_card(c, level, reset::EVENT);
                assert_eq!(
                    !f.cards[c].has_chain_relation(0, 11),
                    cleared,
                    "level {level:#x}"
                );
            }
        }

        /// **A `EFFECT_DISABLE_FIELD` value is zeroed** by the same set
        /// that recharges the count limits — the value is a zone mask a
        /// card built while it was somewhere, and it does not travel.
        #[test]
        fn a_disable_field_value_is_zeroed() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            let mut e = Effect::new(effect_type::FIELD, code::DISABLE_FIELD);
            e.owner = Some(c);
            e.handler = Some(c);
            e.value = 0x1f;
            let id = f.new_effect(e);
            f.cards[c].field_effect.insert(code::DISABLE_FIELD, id);
            f.cards[c].indexer.insert(id);
            f.reset_card(c, reset::CONTROL, reset::EVENT);
            assert_eq!(f.effects.get(id).unwrap().value, 0x1f, "not on control");
            f.reset_card(c, reset::TURN_SET, reset::EVENT);
            assert_eq!(f.effects.get(id).unwrap().value, 0);
        }

        /// **The extra-zone grants keep their low half**, which is the
        /// zone mask; the high half is the "already used" record and is
        /// what the reset clears.
        #[test]
        fn an_extra_zone_grant_keeps_its_zone_mask() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            let mut e = Effect::new(effect_type::FIELD, code::USE_EXTRA_MZONE);
            e.owner = Some(c);
            e.handler = Some(c);
            e.value = 0x0003_00a5;
            let id = f.new_effect(e);
            f.cards[c].field_effect.insert(code::USE_EXTRA_MZONE, id);
            f.cards[c].indexer.insert(id);
            f.reset_card(c, reset::CONTROL, reset::EVENT);
            assert_eq!(
                f.effects.get(id).unwrap().value,
                0xa5,
                "the used record went, the zone mask stayed"
            );
        }

        /// **The remembered equip target is dropped on arriving**, and
        /// on nothing else.
        #[test]
        fn the_pre_equip_target_is_dropped_on_arriving() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            let other = monster(&mut f, 0, 1);
            f.cards[c].pre_equip_target = Some(other);
            f.reset_card(c, reset::LEAVE, reset::EVENT);
            assert_eq!(f.cards[c].pre_equip_target, Some(other), "not on leaving");
            f.reset_card(c, reset::TOFIELD, reset::EVENT);
            assert_eq!(f.cards[c].pre_equip_target, None);
        }

        /// **An unknown reset kind does nothing**, rather than falling
        /// through to the event path.
        #[test]
        fn an_unknown_reset_kind_is_ignored() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, 0);
            let mut e = Effect::new(effect_type::SINGLE, 500);
            e.owner = Some(c);
            e.handler = Some(c);
            e.reset_flag = reset::EVENT | reset::TOGRAVE;
            let id = attach(&mut f, c, e, 500);
            f.reset_card(c, reset::TOGRAVE, 0xdead_beef);
            assert!(f.cards[c].indexer.contains(&id));
        }
    }
}

//! Destruction and release: `Destroy`, `Release`, and their replacements.
//!
//! Both machines end the same way — by handing their survivors to
//! [`Field::send_to`] — so most of what they do is decide *who survives*.
//! That is the whole substance of `Destroy`, and it is why its first step is
//! the longest in the port.
//!
//! ## `Destroy` has two entry points
//!
//! | steps | |
//! |---|---|
//! | 0-5 | the ordinary path: an effect or a rule destroys something |
//! | 10-12 | the battle path, entered directly by the battle machinery |
//!
//! They are not variations on each other. The ordinary path resolves
//! protection, offers replacements, raises `EVENT_DESTROY`, and sends the
//! survivors onward. The battle path resolves protection **and stops** —
//! step 12 returns without sending anything, because the battle machinery
//! sends the cards itself once it knows what survived.
//!
//! That is also why the battle path's replacements are emplaced with an
//! explicit step of 10: a replacement that declines has to come back into
//! the battle path, not the ordinary one.
//!
//! ## Protection is four separate questions
//!
//! Step 0 asks them in order, and a card only reaches the next when the
//! previous says nothing:
//!
//! 1. **`is_destructable`** — is it a card that can be destroyed at all?
//! 2. **`check_indestructable_by_effect`** — is something protecting it from
//!    *this particular effect*?
//! 3. **`EFFECT_INDESTRUCTABLE`** — is something protecting it outright?
//! 4. **`EFFECT_INDESTRUCTABLE_COUNT`** — is something protecting it a
//!    limited number of times?
//!
//! Only questions 2-4 charge anything, and a fifth step —
//! `EFFECT_DESTROY_SUBSTITUTE` — is not protection at all: it *replaces* the
//! card with another, which is why the substitute joins the batch rather
//! than the original leaving it quietly.
//!
//! ## The two ways a card leaves the batch are not the same
//!
//! **Indestructible** cards have their reason restored, lose
//! `STATUS_DESTROY_CONFIRMED`, and are erased. Nothing was destroyed.
//!
//! **Substituted** cards have their reason restored and are erased too — but
//! they go into `core.destroy_canceled` and **keep**
//! `STATUS_DESTROY_CONFIRMED` until a later step clears it. That flag is
//! what stops a card being queued for destruction twice while the first
//! destruction is still resolving, and `field::destroy` checks both together:
//! a card that is confirmed *and not* in `destroy_canceled` is dropped from a
//! new batch outright.

use crate::board::{location, position};
use crate::card::{reason, status};
use crate::event::{code, CardId, EffectId, PLAYER_NONE, PLAYER_SELFDES};
use crate::field::{timing, Field, GroupId};
use crate::processor::Kind;
use std::collections::BTreeSet;

impl Field {
    /// `field::destroy` — queue a batch of cards for destruction.
    ///
    /// Three decisions are made per card here.
    ///
    /// **A card already confirmed for destruction is dropped**, unless it is
    /// in `destroy_canceled` — that pair is how a card being destroyed twice
    /// at once is recognised. Without it a card could be queued by two
    /// effects in the same chain and destroyed twice.
    ///
    /// **`PLAYER_SELFDES` suppresses the reason-effect bookkeeping.** A card
    /// destroying itself does not record who did it, because nobody did.
    ///
    /// **The destination is forced.** Anything that is not the hand, deck or
    /// banished pile becomes the graveyard — and for the graveyard and the
    /// banished pile the player is forced to the **owner**, whatever the
    /// caller asked for. A destroyed card goes to its owner's graveyard, not
    /// its controller's.
    #[allow(clippy::too_many_arguments)]
    pub fn destroy(
        &mut self,
        targets: impl IntoIterator<Item = CardId>,
        reason_effect: Option<EffectId>,
        why: u32,
        reason_player: u8,
        playerid: u8,
        destination: u16,
        sequence: u32,
    ) {
        let mut batch: Vec<CardId> = Vec::new();
        for card in targets {
            if self.cards[card].is_status(status::DESTROY_CONFIRMED)
                && !self.core.destroy_canceled.contains(&card)
            {
                continue;
            }
            let c = &mut self.cards[card];
            c.temp.reason = c.reason;
            if reason_player != PLAYER_SELFDES {
                c.temp.reason_effect = c.reason_effect;
                c.temp.reason_player = c.reason_player;
                if reason_effect.is_some() {
                    c.reason_effect = reason_effect;
                }
                c.reason_player = reason_player;
            }
            c.reason = why;

            let mut dest = destination;
            if dest & u16::from(location::HAND | location::DECK | location::REMOVED) == 0 {
                dest = u16::from(location::GRAVE);
            }
            let mut p = playerid;
            if dest != 0 && p == PLAYER_NONE {
                p = c.owner;
            }
            // A destroyed card goes to its OWNER's graveyard or banished
            // pile, whoever controlled it and whoever the caller named.
            if dest & u16::from(location::GRAVE | location::REMOVED) != 0 {
                p = c.owner;
            }
            c.set_status(status::DESTROY_CONFIRMED, true);
            c.sendto_param = crate::card::SendToParam {
                playerid: p,
                position: position::FACEUP,
                location: (dest & 0xff) as u8,
                sequence,
            };
            batch.push(card);
        }
        let group = self.new_group(batch);
        self.emplace(Kind::Destroy {
            targets: group,
            reason_effect,
            reason: why,
            reason_player,
        });
    }

    /// The single-card arity, and the shape almost every caller uses.
    pub fn destroy_card(
        &mut self,
        target: CardId,
        reason_effect: Option<EffectId>,
        why: u32,
        reason_player: u8,
    ) {
        self.destroy(
            [target],
            reason_effect,
            why,
            reason_player,
            PLAYER_NONE,
            0,
            0,
        );
    }

    /// `field::release` — queue a batch of cards to be released.
    ///
    /// Much simpler than `destroy`: there is no protection to resolve here
    /// (that is `is_releasable_by_*`, asked at step 0 of the machine), no
    /// destination to choose — a released card always goes to its **owner's**
    /// graveyard face-up — and no confirmation flag.
    pub fn release(
        &mut self,
        targets: impl IntoIterator<Item = CardId>,
        reason_effect: Option<EffectId>,
        why: u32,
        reason_player: u8,
    ) {
        let batch: Vec<CardId> = targets.into_iter().collect();
        for &card in &batch {
            let c = &mut self.cards[card];
            c.temp.reason = c.reason;
            c.temp.reason_effect = c.reason_effect;
            c.temp.reason_player = c.reason_player;
            c.reason = why;
            c.reason_effect = reason_effect;
            c.reason_player = reason_player;
            c.sendto_param = crate::card::SendToParam {
                playerid: c.owner,
                position: position::FACEUP,
                location: location::GRAVE,
                sequence: 0,
            };
        }
        let group = self.new_group(batch);
        self.emplace(Kind::Release {
            targets: group,
            reason_effect,
            reason: why,
            reason_player,
        });
    }

    /// The body `DestroyReplace` and `ReleaseReplace` share.
    ///
    /// Both begin with the same two exits, in the same order, and the order
    /// matters: **a card that has reached the graveyard or the banished pile
    /// is dropped first**, before the "still in the batch" test. It got there
    /// during this very operation — an earlier replacement sent it — and it
    /// cannot be destroyed or released again.
    ///
    /// `restore` is the difference in what that first exit does:
    /// `DestroyReplace` also clears `STATUS_DESTROY_CONFIRMED`, because the
    /// destruction it was confirmed for is not going to happen.
    fn replace_common(&mut self, targets: GroupId, target: CardId, clear_confirmed: bool) -> bool {
        if self.cards[target].current.location & (location::GRAVE | location::REMOVED) != 0 {
            let c = &mut self.cards[target];
            c.reason = c.temp.reason;
            c.reason_effect = c.temp.reason_effect;
            c.reason_player = c.temp.reason_player;
            if clear_confirmed {
                c.set_status(status::DESTROY_CONFIRMED, false);
            }
            self.group_mut(targets).remove(&target);
            return true;
        }
        !self.group(targets).contains(&target)
    }

    /// `field::process(Processors::DestroyReplace&)`.
    ///
    /// `battle` changes only where a declining replacement comes back to:
    /// step 10, the battle path, rather than the ordinary one.
    pub(crate) fn destroy_replace_step(
        &mut self,
        targets: GroupId,
        target: CardId,
        battle: bool,
    ) -> bool {
        if self.replace_common(targets, target, true) {
            return true;
        }
        self.core.returns.set(0);
        for effect in self.filter_single_continuous_effect(target, code::DESTROY_REPLACE) {
            let kind = Kind::OperationReplace {
                replace_effect: effect,
                targets,
                target: Some(target),
                is_destroy: true,
            };
            if battle {
                self.emplace_at(kind, 10);
            } else {
                self.emplace(kind);
            }
        }
        true
    }

    /// `field::process(Processors::ReleaseReplace&)`.
    ///
    /// The same shape, without the confirmation flag — and with the
    /// `REASON_RULE` guard that `DestroyReplace` does **not** have. A rules
    /// release cannot be replaced; a rules destruction can.
    pub(crate) fn release_replace_step(&mut self, targets: GroupId, target: CardId) -> bool {
        if self.replace_common(targets, target, false) {
            return true;
        }
        if self.cards[target].reason & reason::RULE == 0 {
            self.core.returns.set(0);
            for effect in self.filter_single_continuous_effect(target, code::RELEASE_REPLACE) {
                self.emplace(Kind::OperationReplace {
                    replace_effect: effect,
                    targets,
                    target: Some(target),
                    is_destroy: false,
                });
            }
        }
        true
    }
}

impl Field {
    /// One step of `Destroy`.
    pub(crate) fn destroy_step(
        &mut self,
        step: u16,
        targets: GroupId,
        reason_effect: Option<EffectId>,
        why: u32,
        reason_player: u8,
    ) -> bool {
        match step {
            0 => self.destroy_step_0(targets, reason_player),
            1 | 11 => {
                let battle = step == 11;
                for card in self.group(targets).iter().copied().collect::<Vec<_>>() {
                    self.emplace(Kind::DestroyReplace {
                        targets,
                        target: card,
                        battle,
                    });
                }
                false
            }
            2 => {
                // The substituted cards kept STATUS_DESTROY_CONFIRMED so
                // that nothing could queue them again mid-resolution. That
                // is over now.
                for card in std::mem::take(&mut self.core.destroy_canceled) {
                    self.cards[card].set_status(status::DESTROY_CONFIRMED, false);
                }
                false
            }
            3 => self.destroy_step_3(targets, reason_effect, why, reason_player),
            4 => self.destroy_step_4(targets, reason_effect, why, reason_player),
            5 => {
                // A substitute is not something that was destroyed, so it is
                // excluded from what the operation reports — the count a
                // card reads back is "how many of yours I destroyed", and a
                // stand-in was not one of them.
                let survivors: Vec<CardId> = self
                    .group(targets)
                    .iter()
                    .copied()
                    .filter(|&c| self.cards[c].reason & reason::REPLACE == 0)
                    .collect();
                self.core.returns.set(survivors.len() as i32);
                self.core.operated_set = survivors;
                true
            }
            10 => self.destroy_step_10(targets),
            12 => {
                for card in std::mem::take(&mut self.core.destroy_canceled) {
                    self.cards[card].set_status(status::DESTROY_CONFIRMED, false);
                }
                // The battle path stops here. It does NOT send anything: the
                // battle machinery sends the survivors itself, once it knows
                // which they are.
                true
            }
            _ => true,
        }
    }

    /// Step 0: resolve protection, collect substitutes, then offer the
    /// batch-wide replacements.
    ///
    /// The four protection questions are asked in order and the card leaves
    /// at the first one that answers. Two details decide whether a
    /// protecting effect is *charged*:
    ///
    /// - **A rule or cost destruction skips question 2 entirely.** Being
    ///   destroyed by the rules is not an effect to be protected from.
    /// - **`PLAYER_SELFDES` suppresses the charge**, though not the
    ///   protection: a card that destroys itself and is saved does not spend
    ///   the saver's count.
    fn destroy_step_0(&mut self, targets: GroupId, reason_player: u8) -> bool {
        let mut indestructible: Vec<CardId> = Vec::new();
        let mut chargeable: BTreeSet<EffectId> = BTreeSet::new();
        let mut extra: BTreeSet<CardId> = BTreeSet::new();

        for card in self.group(targets).iter().copied().collect::<Vec<_>>() {
            if !self.is_destructable(card) {
                indestructible.push(card);
                continue;
            }
            let (by, r, rp) = (
                self.cards[card].reason_effect,
                self.cards[card].reason,
                self.cards[card].reason_player,
            );

            // 2. Protected from this particular effect?
            if r & (reason::RULE | reason::COST) == 0 {
                let mut survives = true;
                if by.is_none() || self.is_affect_by_effect(card, by) {
                    if let Some(shield) = self.check_indestructable_by_effect(card, by, rp) {
                        if reason_player != PLAYER_SELFDES {
                            chargeable.insert(shield);
                        }
                        survives = false;
                    }
                } else {
                    // The effect may not touch it at all.
                    survives = false;
                }
                if !survives {
                    indestructible.push(card);
                    continue;
                }
            }

            // 3. Protected outright? The first that says so wins and stops.
            let outright = self
                .filter_effect(card, code::INDESTRUCTABLE)
                .into_iter()
                .find(|&e| self.destruction_condition_pub(e, card, by, r, rp));
            if let Some(shield) = outright {
                if reason_player != PLAYER_SELFDES {
                    chargeable.insert(shield);
                }
                indestructible.push(card);
                continue;
            }

            // 4. Protected a limited number of times? Unlike question 3 this
            //    does NOT stop at the first: every applicable effect is
            //    consulted, so several can be charged for one destruction.
            if self.count_protected(card, by, r, rp, &mut chargeable) {
                indestructible.push(card);
                continue;
            }

            // 5. Substituted — not protection. Something else is destroyed
            //    in its place, and joins the batch below.
            let subs: Vec<EffectId> = self
                .filter_effect(card, code::DESTROY_SUBSTITUTE)
                .into_iter()
                .filter(|&e| self.destruction_condition_pub(e, card, by, r, rp))
                .collect();
            if !subs.is_empty() {
                for e in subs {
                    if let Some(handler) = self.effects.get(e).and_then(|x| x.handler) {
                        extra.insert(handler);
                    }
                }
                self.restore_reason(card);
                // Keeps DESTROY_CONFIRMED: step 2 clears it.
                self.core.destroy_canceled.insert(card);
                self.group_mut(targets).remove(&card);
            }
        }

        for card in indestructible {
            self.restore_reason(card);
            self.cards[card].set_status(status::DESTROY_CONFIRMED, false);
            self.group_mut(targets).remove(&card);
        }
        for rep in extra {
            if self.group(targets).contains(&rep) {
                continue;
            }
            let owner = self.cards[rep].owner;
            let controller = self.cards[rep].current.controller;
            let c = &mut self.cards[rep];
            c.temp.reason = c.reason;
            c.temp.reason_effect = c.reason_effect;
            c.temp.reason_player = c.reason_player;
            c.reason = reason::EFFECT | reason::DESTROY | reason::REPLACE;
            c.reason_effect = None;
            c.reason_player = controller;
            c.sendto_param = crate::card::SendToParam {
                playerid: owner,
                position: position::FACEUP,
                location: location::GRAVE,
                sequence: 0,
            };
            self.group_mut(targets).insert(rep);
        }
        for effect in chargeable {
            self.dec_count(effect, PLAYER_NONE);
        }
        self.operation_replace(code::DESTROY_REPLACE, 5, targets);
        false
    }

    /// Question 4: the counted protections.
    ///
    /// Two shapes share one code, and they keep their tallies in different
    /// places — which is the subtlety:
    ///
    /// - **With `EFFECT_FLAG_COUNT_LIMIT`** the effect counts itself down,
    ///   and a spent one (`count_limit == 0`) is skipped rather than
    ///   refusing.
    /// - **Without it** the effect's *value* says how many times, and the
    ///   tally lives on the **card** (`indestructable_effects`, keyed by
    ///   effect id) — because one such effect may protect several cards
    ///   independently. The counter is incremented first and compared after,
    ///   so a limit of 1 protects once.
    ///
    /// Returns true when the card is protected. Note it does not stop at the
    /// first: every applicable effect is consulted and several may be
    /// charged for a single destruction.
    fn count_protected(
        &mut self,
        card: CardId,
        by: Option<EffectId>,
        r: u32,
        rp: u8,
        chargeable: &mut BTreeSet<EffectId>,
    ) -> bool {
        let mut protected = false;
        for e in self.filter_effect(card, code::INDESTRUCTABLE_COUNT) {
            let counted = self
                .effects
                .get(e)
                .is_some_and(|x| x.is_flag(crate::effect::flag::COUNT_LIMIT));
            if counted {
                if self.effects.get(e).is_some_and(|x| x.count_limit == 0) {
                    continue;
                }
                if self.destruction_condition_pub(e, card, by, r, rp) {
                    chargeable.insert(e);
                    protected = true;
                }
            } else {
                let limit = self.effect_destruction_value(e, card, by, r, rp);
                if limit != 0 {
                    let id = self.effects.get(e).map_or(0, |x| x.id.get());
                    let used = self.cards[card]
                        .indestructable_effects
                        .entry(id)
                        .or_insert(0);
                    *used += 1;
                    if i64::from(*used) <= limit {
                        chargeable.insert(e);
                        protected = true;
                    }
                }
            }
        }
        protected
    }

    /// Step 3: raise `EVENT_DESTROY`.
    ///
    /// An empty batch ends the unit here. Otherwise each surviving card is
    /// walked **in `card_operation_sort` order**, and a card that has
    /// reached the graveyard or banished pile meanwhile is dropped — a
    /// replacement sent it there during step 1.
    ///
    /// `REASON_DESTROY` is added to each card's reason *here*, not in the
    /// queueing function: a card dropped before this point was never
    /// destroyed and must not carry the reason that says it was.
    ///
    /// The hint timing is charged to the controller of the monster an Xyz
    /// material sits under, as everywhere else.
    fn destroy_step_3(
        &mut self,
        targets: GroupId,
        reason_effect: Option<EffectId>,
        why: u32,
        reason_player: u8,
    ) -> bool {
        if self.group(targets).is_empty() {
            self.core.returns.set(0);
            self.core.operated_set.clear();
            return true;
        }
        let mut ordered: Vec<CardId> = self.group(targets).iter().copied().collect();
        if ordered.len() > 1 {
            ordered.sort_by(|&a, &b| {
                if self.card_operation_sort(a, b) {
                    std::cmp::Ordering::Less
                } else if self.card_operation_sort(b, a) {
                    std::cmp::Ordering::Greater
                } else {
                    std::cmp::Ordering::Equal
                }
            });
        }
        for card in ordered {
            if self.cards[card].current.location & (location::GRAVE | location::REMOVED) != 0 {
                self.restore_reason(card);
                self.group_mut(targets).remove(&card);
                continue;
            }
            self.cards[card].reason |= reason::DESTROY;
            let control_player = match self.cards[card].overlay_target {
                Some(t) => self.cards[t].current.controller,
                None => self.cards[card].current.controller,
            };
            self.core.hint_timing[control_player as usize] |= timing::DESTROY;
            let (by, r, rp) = (
                self.cards[card].reason_effect,
                self.cards[card].reason,
                self.cards[card].reason_player,
            );
            self.raise_single_event(card, vec![], code::DESTROY, by, r, rp, 0, 0);
        }
        self.adjust_instant();
        self.process_single_event();
        let all: Vec<CardId> = self.group(targets).iter().copied().collect();
        self.raise_event_over(all, code::DESTROY, reason_effect, why, reason_player, 0, 0);
        self.process_instant_event();
        false
    }

    /// Step 4: hand the survivors to `SendTo`.
    ///
    /// Three things happen that a shorter version would miss.
    ///
    /// **`STATUS_DESTROY_CONFIRMED` is cleared here**, before the send. The
    /// destruction has happened; what follows is only the movement.
    ///
    /// **A destination the card cannot reach falls back to the graveyard.**
    /// A card destroyed and sent to the hand, by something that cannot go to
    /// the hand, is not saved — it goes to the graveyard instead.
    ///
    /// **`SendTo` is emplaced at step 1, not 0.** Step 0 is its own
    /// capability filter and batch-wide replacement offer, and `Destroy` has
    /// just done both itself. Starting at 0 would ask every question twice
    /// and offer every send-replacement a second time.
    fn destroy_step_4(
        &mut self,
        targets: GroupId,
        reason_effect: Option<EffectId>,
        why: u32,
        reason_player: u8,
    ) -> bool {
        let batch: Vec<CardId> = self.group(targets).iter().copied().collect();
        for &card in &batch {
            self.cards[card].set_status(status::DESTROY_CONFIRMED, false);
            let mut dest = self.cards[card].sendto_param.location;
            if dest == 0 {
                dest = location::GRAVE;
            }
            let pos = self.cards[card].sendto_param.position;
            let refused = match dest {
                d if d == location::HAND => !self.is_capable_send_to_hand(card, reason_player),
                d if d == location::DECK => !self.is_capable_send_to_deck(card, reason_player),
                d if d == location::REMOVED => !self.is_removeable(card, reason_player, pos, why),
                _ => false,
            };
            if refused {
                dest = location::GRAVE;
            }
            self.cards[card].sendto_param.location = dest;
        }
        let send = self.new_group(batch);
        self.operation_replace(code::SEND_REPLACE, 5, send);
        self.emplace_at(
            Kind::SendTo {
                targets: send,
                reason_effect,
                reason: why | reason::DESTROY,
                reason_player,
                state: Box::default(),
            },
            1,
        );
        false
    }

    /// Step 10: the battle path's protection check.
    ///
    /// The same four questions as step 0 with three differences, all of
    /// which follow from "the battle already happened":
    ///
    /// - **No `check_indestructable_by_effect`.** Battle destruction is not
    ///   an effect, so an "indestructible by effects" clause says nothing
    ///   about it.
    /// - **`EFFECT_INDESTRUCTABLE` is not charged**, only reported. The
    ///   ordinary path calls `dec_count` on it; this one does not.
    /// - **Substitutes go to `battle_destroy_rep`** rather than joining the
    ///   batch, because the battle machinery has to know what stood in.
    fn destroy_step_10(&mut self, targets: GroupId) -> bool {
        for card in self.group(targets).iter().copied().collect::<Vec<_>>() {
            if !self.is_destructable(card) {
                self.restore_reason(card);
                self.cards[card].set_status(status::DESTROY_CONFIRMED, false);
                self.group_mut(targets).remove(&card);
                continue;
            }
            let (by, r, rp) = (
                self.cards[card].reason_effect,
                self.cards[card].reason,
                self.cards[card].reason_player,
            );
            let outright = self
                .filter_effect(card, code::INDESTRUCTABLE)
                .into_iter()
                .any(|e| self.destruction_condition_pub(e, card, by, r, rp));
            if outright {
                self.restore_reason(card);
                self.cards[card].set_status(status::DESTROY_CONFIRMED, false);
                self.group_mut(targets).remove(&card);
                continue;
            }
            let mut chargeable: BTreeSet<EffectId> = BTreeSet::new();
            if self.count_protected(card, by, r, rp, &mut chargeable) {
                for e in chargeable {
                    self.dec_count(e, PLAYER_NONE);
                }
                self.restore_reason(card);
                self.cards[card].set_status(status::DESTROY_CONFIRMED, false);
                self.group_mut(targets).remove(&card);
                continue;
            }
            let subs: Vec<EffectId> = self
                .filter_effect(card, code::DESTROY_SUBSTITUTE)
                .into_iter()
                .filter(|&e| self.destruction_condition_pub(e, card, by, r, rp))
                .collect();
            if !subs.is_empty() {
                for e in subs {
                    if let Some(handler) = self.effects.get(e).and_then(|x| x.handler) {
                        self.core.battle_destroy_rep.insert(handler);
                    }
                }
                self.restore_reason(card);
                self.core.destroy_canceled.insert(card);
                self.group_mut(targets).remove(&card);
            }
        }
        if !self.group(targets).is_empty() {
            self.operation_replace(code::DESTROY_REPLACE, 12, targets);
        }
        false
    }

    /// One step of `Release`. Five steps, and no protection machinery: the
    /// whole question is asked once, at step 0, by `is_releasable_by_*`.
    pub(crate) fn release_step(
        &mut self,
        step: u16,
        targets: GroupId,
        reason_effect: Option<EffectId>,
        why: u32,
        reason_player: u8,
    ) -> bool {
        match step {
            0 => {
                for card in self.group(targets).iter().copied().collect::<Vec<_>>() {
                    if self.cannot_be_released(card, why, reason_player) {
                        self.restore_reason(card);
                        self.group_mut(targets).remove(&card);
                    }
                }
                if why & reason::RULE != 0 {
                    return false;
                }
                self.operation_replace(code::RELEASE_REPLACE, 5, targets);
                false
            }
            1 => {
                for card in self.group(targets).iter().copied().collect::<Vec<_>>() {
                    self.emplace(Kind::ReleaseReplace {
                        targets,
                        target: card,
                    });
                }
                false
            }
            2 => {
                if self.group(targets).is_empty() {
                    self.core.returns.set(0);
                    self.core.operated_set.clear();
                    return true;
                }
                let mut ordered: Vec<CardId> = self.group(targets).iter().copied().collect();
                if ordered.len() > 1 {
                    ordered.sort_by(|&a, &b| {
                        if self.card_operation_sort(a, b) {
                            std::cmp::Ordering::Less
                        } else if self.card_operation_sort(b, a) {
                            std::cmp::Ordering::Greater
                        } else {
                            std::cmp::Ordering::Equal
                        }
                    });
                }
                for card in ordered {
                    if self.cards[card].current.location & (location::GRAVE | location::REMOVED)
                        != 0
                    {
                        self.restore_reason(card);
                        self.group_mut(targets).remove(&card);
                        continue;
                    }
                    self.cards[card].reason |= reason::RELEASE;
                }
                self.adjust_instant();
                false
            }
            3 => {
                let batch: Vec<CardId> = self.group(targets).iter().copied().collect();
                let send = self.new_group(batch);
                self.operation_replace(code::SEND_REPLACE, 5, send);
                self.emplace_at(
                    Kind::SendTo {
                        targets: send,
                        reason_effect,
                        reason: why | reason::RELEASE,
                        reason_player,
                        state: Box::default(),
                    },
                    1,
                );
                false
            }
            4 => {
                // The counts are charged only now. A release that turned out
                // not to happen — everything dropped at step 0 or replaced
                // at step 1 — does not spend anything, because the machine
                // finished at step 2 and never reached here.
                for effect in std::mem::take(&mut self.core.dec_count_reserve) {
                    self.dec_count(effect, PLAYER_NONE);
                }
                self.core.operated_set = self.group(targets).iter().copied().collect();
                self.core.returns.set(self.core.operated_set.len() as i32);
                true
            }
            _ => true,
        }
    }

    /// `Release`'s step-0 filter.
    ///
    /// A tribute for a summon and a release for anything else ask different
    /// predicates, and the third arm — the one for an ordinary effect —
    /// carries the same cost/summon/rule carve-out `SendTo` has: a release
    /// that is a *price already agreed* does not ask whether the effect may
    /// touch the card.
    fn cannot_be_released(&mut self, card: CardId, why: u32, reason_player: u8) -> bool {
        if self.cards[card].get_status(status::SUMMONING | status::SPSUMMON_STEP) {
            return true;
        }
        if why & reason::SUMMON != 0 {
            let by = self.cards[card].reason_card;
            let Some(by) = by else { return true };
            if !self.is_releasable_by_summon(card, reason_player, by) {
                return true;
            }
        }
        let r = self.cards[card].reason;
        if r & (reason::RULE | reason::SUMMON | reason::COST) == 0 {
            let by = self.cards[card].reason_effect;
            if !self.is_affect_by_effect(card, by)
                || !self.is_releasable_by_nonsummon(card, reason_player, why)
            {
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{card_type, Card, CardData};
    use crate::effect::{effect_type, flag, Effect};
    use crate::processor::Status;

    fn monster(f: &mut Field, player: u8, loc: u8) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 18036057,
                type_: card_type::MONSTER | card_type::EFFECT,
                attack: 1900,
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

    fn printed(f: &mut Field, card: CardId, code_: u32, value: i64) -> EffectId {
        let mut e = Effect::new(effect_type::SINGLE, code_);
        e.owner = Some(card);
        e.handler = Some(card);
        e.value = value;
        let id = f.new_effect(e);
        f.cards[card].single_effect.insert(code_, id);
        f.cards[card].indexer.insert(id);
        id
    }

    fn run(f: &mut Field) -> Status {
        for _ in 0..512 {
            match f.process() {
                Status::Continue => continue,
                other => return other,
            }
        }
        panic!("the machine did not terminate");
    }

    fn raised(f: &Field, ev: u32) -> bool {
        f.core.instant_event.iter().any(|e| e.event_code == ev)
    }

    mod destroy {
        use super::*;

        /// The group of the `Destroy` unit just emplaced.
        ///
        /// `emplace` lands in `core.subunits`; those are spliced into the
        /// real queue at the top of the next `process()`, so a unit queued
        /// but not yet run is **not** visible through `queue()`.
        fn queued_destroy_group(f: &Field) -> GroupId {
            match f.core.subunits.first().map(|u| &u.kind) {
                Some(Kind::Destroy { targets, .. }) => *targets,
                other => panic!("expected a queued Destroy unit, got {other:?}"),
            }
        }

        #[test]
        fn a_monster_is_destroyed_and_reaches_the_graveyard() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, location::MZONE);
            f.destroy_card(c, None, reason::EFFECT, 0);
            run(&mut f);

            assert_eq!(f.cards[c].current.location, location::GRAVE);
            assert!(raised(&f, code::DESTROY), "EVENT_DESTROY");
            assert!(
                raised(&f, code::DESTROYED),
                "and EVENT_DESTROYED from SendTo"
            );
            assert_eq!(f.core.returns.get(), 1);
        }

        /// A destroyed card goes to its **owner's** graveyard, whoever
        /// controlled it — the queueing function forces the player.
        #[test]
        fn a_destroyed_card_goes_to_its_owners_graveyard() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, location::MZONE);
            f.cards[c].owner = 1;
            f.destroy_card(c, None, reason::EFFECT, 0);
            assert_eq!(
                f.cards[c].sendto_param.playerid, 1,
                "the owner, not the controller"
            );
        }

        /// The destination is forced to the graveyard unless it is one of
        /// the three the reference allows.
        #[test]
        fn an_unsupported_destination_becomes_the_graveyard() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, location::MZONE);
            f.destroy(
                [c],
                None,
                reason::EFFECT,
                0,
                PLAYER_NONE,
                u16::from(location::MZONE),
                0,
            );
            assert_eq!(f.cards[c].sendto_param.location, location::GRAVE);
        }

        /// An indestructible card leaves the batch, has its reason put back,
        /// and loses the confirmation flag.
        #[test]
        fn an_indestructible_card_survives_intact() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, location::MZONE);
            f.cards[c].reason = reason::BATTLE;
            printed(&mut f, c, code::INDESTRUCTABLE, 1);

            f.destroy_card(c, None, reason::EFFECT, 0);
            run(&mut f);

            assert_eq!(f.cards[c].current.location, location::MZONE, "it stayed");
            assert_eq!(f.core.returns.get(), 0);
            assert_eq!(f.cards[c].reason, reason::BATTLE, "its reason was put back");
            assert!(!f.cards[c].is_status(status::DESTROY_CONFIRMED));
        }

        /// A card already in the graveyard cannot be destroyed at all.
        #[test]
        fn a_card_in_the_graveyard_is_not_destructable() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, location::GRAVE);
            f.destroy_card(c, None, reason::EFFECT, 0);
            run(&mut f);
            assert_eq!(f.core.returns.get(), 0);
        }

        /// `STATUS_DESTROY_CONFIRMED` stops a card being queued twice while
        /// the first destruction is still resolving.
        #[test]
        fn a_confirmed_card_is_dropped_from_a_second_batch() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, location::MZONE);
            f.cards[c].set_status(status::DESTROY_CONFIRMED, true);
            f.destroy_card(c, None, reason::EFFECT, 0);
            let group = queued_destroy_group(&f);
            assert!(
                f.group(group).is_empty(),
                "already confirmed, so not queued again"
            );
        }

        /// Unless it is in `destroy_canceled` — the pair is what
        /// distinguishes "being destroyed" from "was spared".
        #[test]
        fn a_cancelled_card_may_be_queued_again() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, location::MZONE);
            f.cards[c].set_status(status::DESTROY_CONFIRMED, true);
            f.core.destroy_canceled.insert(c);
            f.destroy_card(c, None, reason::EFFECT, 0);
            let group = queued_destroy_group(&f);
            assert_eq!(f.group(group).len(), 1);
        }

        /// A substitute joins the batch in the original's place, and the
        /// original is spared.
        #[test]
        fn a_substitute_is_destroyed_instead() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, location::MZONE);
            let stand_in = monster(&mut f, 0, location::MZONE);
            // The substitute effect lives on the stand-in, and its handler
            // is what joins the batch.
            let mut e = Effect::new(effect_type::SINGLE, code::DESTROY_SUBSTITUTE);
            e.owner = Some(stand_in);
            e.handler = Some(stand_in);
            e.value = 1;
            let id = f.new_effect(e);
            f.cards[c]
                .single_effect
                .insert(code::DESTROY_SUBSTITUTE, id);
            f.cards[c].indexer.insert(id);

            f.destroy_card(c, None, reason::EFFECT, 0);
            run(&mut f);

            assert_eq!(f.cards[c].current.location, location::MZONE, "spared");
            assert_eq!(
                f.cards[stand_in].current.location,
                location::GRAVE,
                "the stand-in went instead"
            );
        }

        /// A substitute is not something that was destroyed, so it is not
        /// counted in what the operation reports.
        #[test]
        fn a_substitute_is_not_counted_as_destroyed() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, location::MZONE);
            let stand_in = monster(&mut f, 0, location::MZONE);
            let mut e = Effect::new(effect_type::SINGLE, code::DESTROY_SUBSTITUTE);
            e.owner = Some(stand_in);
            e.handler = Some(stand_in);
            e.value = 1;
            let id = f.new_effect(e);
            f.cards[c]
                .single_effect
                .insert(code::DESTROY_SUBSTITUTE, id);
            f.cards[c].indexer.insert(id);

            f.destroy_card(c, None, reason::EFFECT, 0);
            run(&mut f);
            assert_eq!(
                f.core.returns.get(),
                0,
                "nothing the caller asked about was destroyed"
            );
        }

        /// The counted protection: a limit of one saves the card once, and
        /// the tally lives on the card.
        #[test]
        fn a_counted_protection_is_spent_per_card() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, location::MZONE);
            let shield = printed(&mut f, c, code::INDESTRUCTABLE_COUNT, 1);
            let shield_id = f.effects.get(shield).map(|e| e.id.get()).unwrap();

            f.destroy_card(c, None, reason::EFFECT, 0);
            run(&mut f);
            assert_eq!(f.cards[c].current.location, location::MZONE, "saved once");
            assert_eq!(f.cards[c].indestructable_effects.get(&shield_id), Some(&1));

            f.destroy_card(c, None, reason::EFFECT, 0);
            run(&mut f);
            assert_eq!(
                f.cards[c].current.location,
                location::GRAVE,
                "the second time it is not saved"
            );
        }

        /// A count-limited protection that is already spent is skipped
        /// rather than refusing.
        #[test]
        fn a_spent_count_limit_protects_nothing() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, location::MZONE);
            let shield = printed(&mut f, c, code::INDESTRUCTABLE_COUNT, 1);
            if let Some(e) = f.effects.get_mut(shield) {
                e.flag[0] |= flag::COUNT_LIMIT;
                e.count_limit = 0;
            }
            f.destroy_card(c, None, reason::EFFECT, 0);
            run(&mut f);
            assert_eq!(f.cards[c].current.location, location::GRAVE);
        }

        /// The owner override is unconditional, and that is the point:
        /// even when the caller **names a player**, a card destroyed to the
        /// graveyard or the banished pile goes to its owner's.
        #[test]
        fn the_owner_override_beats_an_explicit_player() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, location::MZONE);
            f.cards[c].owner = 1;
            // Player 0 named explicitly — not PLAYER_NONE, so the earlier
            // "default to the owner" branch does not fire.
            f.destroy(
                [c],
                None,
                reason::EFFECT,
                0,
                0,
                u16::from(location::GRAVE),
                0,
            );
            assert_eq!(
                f.cards[c].sendto_param.playerid, 1,
                "the owner wins over the player the caller asked for"
            );
        }

        /// A card that "cannot be sent to the graveyard" is still destroyed
        /// to the graveyard.
        ///
        /// `Destroy` emplaces `SendTo` at **step 1**, skipping its
        /// capability filter — the destruction has already been decided and
        /// is not re-litigated as a movement. Starting `SendTo` at step 0
        /// would let a send-prohibition undo a destruction.
        #[test]
        fn a_send_prohibition_does_not_undo_a_destruction() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, location::MZONE);
            printed(&mut f, c, code::CANNOT_TO_GRAVE, 1);

            f.destroy_card(c, None, reason::EFFECT, 0);
            run(&mut f);
            assert_eq!(
                f.cards[c].current.location,
                location::GRAVE,
                "SendTo starts at step 1, so its filter never runs"
            );
        }

        /// A destination the card cannot reach falls back to the graveyard
        /// rather than saving it.
        #[test]
        fn an_unreachable_destination_falls_back_to_the_graveyard() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, location::MZONE);
            printed(&mut f, c, code::CANNOT_TO_HAND, 1);

            f.destroy(
                [c],
                None,
                reason::EFFECT,
                0,
                PLAYER_NONE,
                u16::from(location::HAND),
                0,
            );
            run(&mut f);
            assert_eq!(
                f.cards[c].current.location,
                location::GRAVE,
                "it is not saved by being unable to reach the hand"
            );
        }

        /// A rule or cost destruction never asks "am I protected from this
        /// effect", because there is no effect to be protected from.
        #[test]
        fn a_rule_destruction_skips_the_by_effect_protection() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, location::MZONE);
            printed(&mut f, c, code::INDESTRUCTABLE_EFFECT, 1);
            // An effect must be named, or `check_indestructable_by_effect`
            // returns early on the null and the test passes for the wrong
            // reason — it is the RULE flag that must do the work here.
            let by = printed(&mut f, c, code::UPDATE_ATTACK, 0);
            f.destroy_card(c, Some(by), reason::RULE, 0);
            run(&mut f);
            assert_eq!(
                f.cards[c].current.location,
                location::GRAVE,
                "an indestructible-by-effect clause cannot answer the rules"
            );
        }
    }

    mod release {
        use super::*;

        #[test]
        fn a_released_monster_reaches_the_graveyard() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, location::MZONE);
            f.release([c], None, reason::COST, 0);
            run(&mut f);

            assert_eq!(f.cards[c].current.location, location::GRAVE);
            assert!(raised(&f, code::RELEASE), "EVENT_RELEASE, from SendTo");
            assert_eq!(f.core.returns.get(), 1);
        }

        /// A released card always goes to its owner's graveyard face-up.
        #[test]
        fn a_release_always_targets_the_owners_graveyard() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, location::MZONE);
            f.cards[c].owner = 1;
            f.release([c], None, reason::COST, 0);
            let p = f.cards[c].sendto_param;
            assert_eq!(p.playerid, 1);
            assert_eq!(p.location, location::GRAVE);
            assert_eq!(p.position, position::FACEUP);
        }

        #[test]
        fn an_unreleasable_card_is_dropped_and_restored() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, location::MZONE);
            f.cards[c].reason = reason::BATTLE;
            printed(&mut f, c, code::UNRELEASABLE_NONSUM, 1);

            f.release([c], None, reason::EFFECT, 0);
            run(&mut f);
            assert_eq!(f.cards[c].current.location, location::MZONE);
            assert_eq!(f.core.returns.get(), 0);
            assert_eq!(f.cards[c].reason, reason::BATTLE);
        }

        /// A Spell or Trap in the hand cannot be released — the rule that
        /// exists only in the non-summon predicate.
        #[test]
        fn a_spell_in_the_hand_cannot_be_released() {
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

            f.release([id], None, reason::EFFECT, 0);
            run(&mut f);
            assert_eq!(f.core.returns.get(), 0);
            assert_eq!(f.cards[id].current.location, location::HAND);
        }

        #[test]
        fn an_empty_batch_reports_nothing() {
            let mut f = Field::new(8000);
            f.release(Vec::<CardId>::new(), None, reason::COST, 0);
            run(&mut f);
            assert_eq!(f.core.returns.get(), 0);
        }

        /// The reserve is only spent when the release actually happens.
        #[test]
        fn a_release_that_does_not_happen_spends_no_counts() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, location::MZONE);
            printed(&mut f, c, code::UNRELEASABLE_NONSUM, 1);
            let spare = monster(&mut f, 0, location::MZONE);
            let counted = printed(&mut f, spare, code::INDESTRUCTABLE_COUNT, 1);
            if let Some(e) = f.effects.get_mut(counted) {
                e.flag[0] |= flag::COUNT_LIMIT;
                e.count_limit = 3;
            }
            f.core.dec_count_reserve.push(counted);

            f.release([c], None, reason::EFFECT, 0);
            run(&mut f);
            assert_eq!(
                f.effects.get(counted).map(|e| e.count_limit),
                Some(3),
                "the machine ended at step 2 and never reached the charge"
            );
        }
    }
}

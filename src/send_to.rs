//! Sending cards somewhere: `SendTo` and `SendToReplace`.
//!
//! The largest step machine in the port so far, and the one most of the rest
//! of the engine funnels into — `Destroy` and `Release` both end by handing
//! their survivors to `SendTo`, and a card reaching the graveyard, the hand,
//! the deck or the banished pile goes through here whatever put it there.
//!
//! Twelve steps, and they divide into four phases:
//!
//! | steps | |
//! |---|---|
//! | 0-1 | drop the cards that cannot go, then offer replacement effects |
//! | 2-4 | raise the pre-leave warning, snapshot, resolve redirects, order |
//! | 5-8 | move the cards, one at a time, looping back to 4 |
//! | 9-11 | reset, raise every event the move earned, and report |
//!
//! ## `sendto_param` is where the destination lives, not the unit
//!
//! The queueing function writes a `sendto_param` onto **each card** and the
//! unit carries only the group. That is not a stylistic choice: steps 3 and 5
//! *rewrite* a card's destination when a redirect applies, and different
//! cards in one batch can end up in different places. A port that put the
//! destination on the unit would send a redirected card to the original
//! destination along with everything else.
//!
//! ## The loop is a step that jumps backwards
//!
//! Steps 5 to 8 handle one card and set `step = 4`, so the machine returns
//! to 5 with the cursor advanced. The reference does this rather than an
//! inner loop because steps 5 and 7 can **yield to the host** for an answer,
//! and a yield has to be able to resume in the middle of the batch.
//!
//! ## The two `0x…` packings
//!
//! `sendto_param` reuses two fields to carry more than they look like:
//!
//! - **`playerid`'s high bits are a flag.** `playerid & 0x7` is the player;
//!   `playerid >> 4` is "an `EFFECT_TO_GRAVE_REDIRECT_CB` applies to this
//!   card". Step 3 sets it, step 5 reads it back.
//! - **A redirect's value carries a sequence.** `redirect >> 16` is the
//!   sequence and `redirect & 0xffff` the location — which is exactly how
//!   `LOCATION_DECKBOT` (`0x10001`) means "the deck, at sequence 1".
//!
//! Both are the reference's, and both are invisible if mis-ported: the card
//! still moves, just to the wrong place in the pile or without its callback.

use crate::board::{location, position};
use crate::card::{card_type, reason, status};
use crate::event::{code, CardId, EffectId, PLAYER_NONE};
use crate::field::{global_flag, reset, timing, Field, GroupId};
use crate::processor::Kind;
use std::collections::BTreeSet;

/// The unit state `SendTo` accumulates across its steps.
///
/// `Processors::SendTo::exargs` in the reference, allocated lazily at step 4
/// there and simply carried here. `cursor` is the reference's `cvit`: an
/// index rather than an iterator, because the vector it walks is stable for
/// the unit's lifetime and an index survives being moved between steps.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SendToState {
    pub leave_field: BTreeSet<CardId>,
    pub leave_grave: BTreeSet<CardId>,
    pub detach: BTreeSet<CardId>,
    pub check_decktop_visibility: [bool; 2],
    /// The batch in processing order, fixed at step 4.
    pub ordered: Vec<CardId>,
    pub cursor: usize,
    /// The `EFFECT_TO_GRAVE_REDIRECT_CB` that applies to the card at the
    /// cursor, if one does.
    pub predirect: Option<EffectId>,
    /// Set once step 4 has built `ordered`, so the backward jumps to step 4
    /// do not rebuild it.
    pub prepared: bool,
}

impl Field {
    /// `field::send_to` — queue a batch of cards to be sent somewhere.
    ///
    /// Nothing moves here. Each card gets a `sendto_param` and the unit is
    /// emplaced; the machine below does the work.
    ///
    /// Four decisions are made per card *before* the machine runs, and each
    /// is easy to miss:
    ///
    /// - **A destination on the field is refused outright.** `send_to` is for
    ///   piles; putting a card onto the field is `move_to_field`.
    /// - **`temp` saves the card's current reason**, so that a card dropped
    ///   at step 0 or by a replacement effect can have it put back. The card
    ///   was never sent, so it must not keep the reason that says it was.
    /// - **A card going from the deck to the hand defaults to the *reason
    ///   player's* hand**, not the owner's, and only when that player already
    ///   controls it. Everything else defaults to the owner. This is what
    ///   makes "add from deck to hand" put the card in the searcher's hand.
    /// - **A card returning from banishment to the graveyard gains
    ///   `REASON_RETURN`**, which is what stops it raising `EVENT_TO_GRAVE`
    ///   at step 10.
    ///
    /// The position rule reads backwards at first: unless the destination is
    /// the banished pile, the card is turned **face-up**, whatever position
    /// was asked for — `ignore` is what lets a caller mean it. A position of
    /// 0 with `ignore` means "keep the position it has".
    #[allow(clippy::too_many_arguments)]
    pub fn send_to(
        &mut self,
        targets: impl IntoIterator<Item = CardId>,
        reason_effect: Option<EffectId>,
        why: u32,
        reason_player: u8,
        playerid: u8,
        destination: u16,
        sequence: u32,
        pos: u8,
        ignore: bool,
    ) {
        if destination & u16::from(location::ONFIELD) != 0 {
            return;
        }
        let targets: Vec<CardId> = targets.into_iter().collect();
        let dest = (destination & 0xff) as u8;
        for &card in &targets {
            let c = &mut self.cards[card];
            c.temp.reason = c.reason;
            c.temp.reason_effect = c.reason_effect;
            c.temp.reason_player = c.reason_player;
            c.reason = why;
            c.reason_effect = reason_effect;
            c.reason_player = reason_player;

            let mut p = playerid;
            // To the hand from the deck with no player named: the reason
            // player's hand, when they already control it.
            if p == PLAYER_NONE
                && dest & location::HAND != 0
                && c.current.location & location::DECK != 0
                && c.current.controller == reason_player
            {
                p = reason_player;
            }
            if p == PLAYER_NONE {
                p = c.owner;
            }
            if dest == location::GRAVE && c.current.location == location::REMOVED {
                c.reason |= reason::RETURN;
            }
            let position = if dest != location::REMOVED && !ignore {
                position::FACEUP
            } else if pos == 0 {
                c.current.position
            } else {
                pos
            };
            c.sendto_param = crate::card::SendToParam {
                playerid: p,
                position,
                location: dest,
                sequence,
            };
        }
        let group = self.new_group(targets);
        self.emplace(Kind::SendTo {
            targets: group,
            reason_effect,
            reason: why,
            reason_player,
            state: Box::default(),
        });
    }

    /// The single-card arity. The reference has it as an overload that
    /// builds a one-card set.
    #[allow(clippy::too_many_arguments)]
    pub fn send_to_card(
        &mut self,
        target: CardId,
        reason_effect: Option<EffectId>,
        why: u32,
        reason_player: u8,
        playerid: u8,
        destination: u16,
        sequence: u32,
        pos: u8,
        ignore: bool,
    ) {
        self.send_to(
            [target],
            reason_effect,
            why,
            reason_player,
            playerid,
            destination,
            sequence,
            pos,
            ignore,
        );
    }

    /// `field::process(Processors::SendToReplace&)` — offer one card's own
    /// replacement effects a chance at the move.
    ///
    /// Not a loop: one card, and it runs to completion in a single step.
    ///
    /// Two early exits that both mean "this card is not going anywhere", and
    /// they are not the same:
    ///
    /// - The card is **no longer in the batch** — a previous replacement
    ///   already removed it. Return; do not restore its reason, because
    ///   whoever removed it did that.
    /// - The card is **already where it was being sent**. Restore its saved
    ///   reason and drop it from the batch: it was never sent, so it must not
    ///   look as though it was.
    ///
    /// Only `filter_single_continuous_effect` is consulted here, not
    /// `filter_effect`. A replacement printed on the card itself gets this
    /// per-card offer; a replacement someone else owns was already handled by
    /// the batch-wide `operation_replace` at step 0.
    pub(crate) fn send_to_replace_step(&mut self, targets: GroupId, target: CardId) -> bool {
        if !self.group(targets).contains(&target) {
            return true;
        }
        let param = self.cards[target].sendto_param;
        if self.cards[target].current.location == param.location
            && self.cards[target].current.controller == param.playerid
        {
            let c = &mut self.cards[target];
            c.reason = c.temp.reason;
            c.reason_effect = c.temp.reason_effect;
            c.reason_player = c.temp.reason_player;
            self.group_mut(targets).remove(&target);
            return true;
        }
        if self.cards[target].reason & reason::RULE == 0 {
            self.core.returns.set(0);
            for effect in self.filter_single_continuous_effect(target, code::SEND_REPLACE) {
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
    /// One step of `SendTo`. Returns true when the unit is finished.
    ///
    /// The `RESTART` idiom the other machines use does not appear: this one
    /// loops by assigning `step = 4` and returning "not finished", which the
    /// processor reads as "run step 5 next". Steps 5-8 therefore all end the
    /// same way.
    pub(crate) fn send_to_step(
        &mut self,
        step: u16,
        targets: GroupId,
        reason_effect: Option<EffectId>,
        why: u32,
        reason_player: u8,
        state: &mut SendToState,
    ) -> bool {
        match step {
            0 => self.send_to_step_0(targets, why),
            1 => {
                for card in self.group(targets).iter().copied().collect::<Vec<_>>() {
                    self.emplace(Kind::SendToReplace {
                        targets,
                        target: card,
                    });
                }
                false
            }
            2 => self.send_to_step_2(targets, reason_effect, why, reason_player),
            3 => self.send_to_step_3(targets),
            4 => self.send_to_step_4(targets, state),
            5 => self.send_to_step_5(state),
            6 => self.send_to_step_6(state),
            7 => {
                // The "crystal beast" redirect: the callback wants the card
                // placed in a spell/trap zone rather than sent, so the host
                // is asked which seat.
                let card = state.ordered[state.cursor];
                self.select_place_for_redirect(card);
                false
            }
            8 => self.send_to_step_8(targets, reason_effect, state),
            9 => self.send_to_step_9(targets, reason_effect, why, reason_player, state),
            10 => self.send_to_step_10(targets, reason_effect, why, reason_player),
            11 => {
                self.core.operated_set = self.group(targets).iter().copied().collect();
                self.core.returns.set(self.group(targets).len() as i32);
                true
            }
            _ => true,
        }
    }

    /// Step 0: drop every card that cannot make the trip, then offer the
    /// batch-wide replacement effects.
    ///
    /// The filter is one long disjunction in the reference and every arm is
    /// load-bearing, but note what wraps it: **a rule-reason send skips the
    /// whole test**. Cards sent by the rules — an equip card whose target
    /// left, a material with nowhere to be — go regardless of what says they
    /// cannot, and `return` before `operation_replace` so nothing may
    /// replace the move either.
    ///
    /// A dropped card has its saved reason **restored**. It was never sent.
    fn send_to_step_0(&mut self, targets: GroupId, why: u32) -> bool {
        if why & reason::RULE == 0 {
            for card in self.group(targets).iter().copied().collect::<Vec<_>>() {
                if self.cannot_make_the_trip(card, why) {
                    let c = &mut self.cards[card];
                    c.reason = c.temp.reason;
                    c.reason_player = c.temp.reason_player;
                    c.reason_effect = c.temp.reason_effect;
                    self.group_mut(targets).remove(&card);
                }
            }
        }
        if why & reason::RULE != 0 {
            return false;
        }
        self.operation_replace(code::SEND_REPLACE, 5, targets);
        false
    }

    /// The step-0 disjunction, as its own predicate so each arm can be read.
    ///
    /// A card is dropped when it is mid-summon, when the effect behind the
    /// move may not touch it, or when the specific destination refuses it.
    ///
    /// The middle arm has a carve-out worth stating: a move made as a
    /// **cost**, as part of a **summon**, or as **material** does not ask
    /// `is_affect_by_effect` at all. Those are prices already agreed, not
    /// effects being applied, so an immunity does not excuse them.
    fn cannot_make_the_trip(&mut self, card: CardId, why: u32) -> bool {
        let dest = self.cards[card].sendto_param.location;
        let pos = self.cards[card].sendto_param.position;
        let by = self.cards[card].reason_effect;
        let card_reason = self.cards[card].reason;
        let rp = self.core.reason_player;

        if self.cards[card].get_status(status::SUMMONING | status::SPSUMMON_STEP) {
            return true;
        }
        if card_reason & (reason::COST | reason::SUMMON | reason::MATERIAL) == 0
            && !self.is_affect_by_effect(card, by)
        {
            return true;
        }
        match dest {
            d if d == location::HAND => !self.is_capable_send_to_hand(card, rp),
            d if d == location::DECK => !self.is_capable_send_to_deck(card, rp),
            d if d == location::REMOVED => !self.is_removeable(card, rp, pos, why),
            d if d == location::GRAVE => !self.is_capable_send_to_grave(card, rp, why),
            d if d == location::EXTRA => !self.is_capable_send_to_extra(card, rp),
            _ => false,
        }
    }

    /// Step 2: the pre-leave warning, and the snapshot.
    ///
    /// An empty batch ends the unit here with a return of 0 — everything was
    /// dropped at step 0 or replaced at step 1.
    ///
    /// `EVENT_LEAVE_FIELD_P` is raised **before anything moves**, which is
    /// what lets an effect respond to a card that is about to leave while it
    /// is still where it was. A card whose summon or activation was negated
    /// does not raise it: it never really arrived, so it cannot leave.
    fn send_to_step_2(
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
        let mut leave_p: BTreeSet<CardId> = BTreeSet::new();
        for card in self.group(targets).iter().copied().collect::<Vec<_>>() {
            let onfield = self.cards[card].current.location & location::ONFIELD != 0;
            let negated =
                self.cards[card].get_status(status::SUMMON_DISABLED | status::ACTIVATE_DISABLED);
            if onfield && !negated {
                let (by, r, rp) = (
                    self.cards[card].reason_effect,
                    self.cards[card].reason,
                    self.cards[card].reason_player,
                );
                self.raise_single_event(card, vec![], code::LEAVE_FIELD_P, by, r, rp, 0, 0);
                leave_p.insert(card);
            }
            if onfield {
                self.snapshot_previous_stats(card);
            }
        }
        if !leave_p.is_empty() {
            let cards: Vec<CardId> = leave_p.into_iter().collect();
            self.raise_event_over(
                cards,
                code::LEAVE_FIELD_P,
                reason_effect,
                why,
                reason_player,
                0,
                0,
            );
        }
        self.process_single_event();
        self.process_instant_event();
        false
    }

    /// Step 3: work out where each card actually ends up.
    ///
    /// Two redirects are consulted, in order, and they are not alternatives:
    /// a card can be redirected on the way out of the field *and* then
    /// redirected again by its new destination's rule.
    ///
    /// - `leave_field_redirect` applies only to a card that is really
    ///   leaving the field (again, not one whose summon was negated).
    /// - `destination_redirect` applies to whatever destination the first
    ///   left it with — and is ignored when it names the location the card
    ///   is already in.
    ///
    /// Both return a packed value: `>> 16` is the sequence, `& 0xffff` the
    /// location. That is how `LOCATION_DECKBOT` says "the deck, at sequence
    /// 1" in one number.
    ///
    /// A leave-field redirect also **clears `REASON_TEMPORARY` and sets
    /// `REASON_REDIRECT`**. The first matters: a card redirected on its way
    /// out is no longer temporarily leaving, so it resets as a real
    /// departure at step 9. And a card redirected to the banished pile is
    /// flipped face-up — banishing face-down is a different operation, so
    /// the two face-down positions are mapped to their face-up counterparts
    /// rather than left alone.
    fn send_to_step_3(&mut self, targets: GroupId) -> bool {
        for card in self.group(targets).iter().copied().collect::<Vec<_>>() {
            self.enable_field_effect(card, false);
        }
        self.adjust_disable_check_list();
        for card in self.group(targets).iter().copied().collect::<Vec<_>>() {
            let mut dest = u32::from(self.cards[card].sendto_param.location);
            let check_cb = dest & u32::from(location::GRAVE) != 0
                && self
                    .is_affected_by_effect(card, code::TO_GRAVE_REDIRECT_CB)
                    .is_some();

            let onfield = self.cards[card].current.location & location::ONFIELD != 0;
            let negated =
                self.cards[card].get_status(status::SUMMON_DISABLED | status::ACTIVATE_DISABLED);
            if onfield && !negated {
                let r = self.cards[card].reason;
                let packed = self.leave_field_redirect(card, r);
                if packed != 0 {
                    let (loc, seq) = (packed & 0xffff, packed >> 16);
                    let c = &mut self.cards[card];
                    c.reason &= !reason::TEMPORARY;
                    c.reason |= reason::REDIRECT;
                    c.sendto_param.location = loc as u8;
                    c.sendto_param.sequence = seq;
                    dest = loc;
                    if loc == u32::from(location::REMOVED) {
                        let p = c.sendto_param.position;
                        if p & position::FACEDOWN_ATTACK != 0 {
                            c.sendto_param.position =
                                (p & !position::FACEDOWN_ATTACK) | position::FACEUP_ATTACK;
                        }
                        let p = c.sendto_param.position;
                        if p & position::FACEDOWN_DEFENSE != 0 {
                            c.sendto_param.position =
                                (p & !position::FACEDOWN_DEFENSE) | position::FACEUP_DEFENSE;
                        }
                    }
                }
            }

            let r = self.cards[card].reason;
            let packed = self.destination_redirect(card, dest as u8, r);
            if packed != 0 {
                let (loc, seq) = (packed & 0xffff, packed >> 16);
                if u32::from(self.cards[card].current.location) != loc {
                    let c = &mut self.cards[card];
                    c.reason |= reason::REDIRECT;
                    c.sendto_param.location = loc as u8;
                    c.sendto_param.sequence = seq;
                }
            }
            if check_cb {
                // The flag lives in the high bits of `playerid`; step 5
                // reads it back out.
                self.cards[card].sendto_param.playerid |= 1 << 4;
            }
        }
        false
    }

    /// Step 4: fix the processing order, once.
    ///
    /// Steps 5-8 jump back here after each card, so the guard matters: the
    /// reference allocates its `exargs` here and would rebuild the vector on
    /// every lap without one. `prepared` is that guard.
    fn send_to_step_4(&mut self, targets: GroupId, state: &mut SendToState) -> bool {
        if state.prepared {
            return false;
        }
        let mut ordered: Vec<CardId> = self.group(targets).iter().copied().collect();
        if ordered.len() > 1 {
            // The reference sorts with `card_operation_sort` as the
            // comparator. Rust wants a total order, and the predicate is a
            // strict weak ordering, so it is lifted into one here.
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
        state.ordered = ordered;
        state.cursor = 0;
        state.prepared = true;
        false
    }
}

impl Field {
    /// Step 5: take the next card, or leave the loop.
    ///
    /// The cursor reaching the end jumps to step 9 — which is why the
    /// machine's later steps are numbered as they are.
    ///
    /// **A token does not move; it stops existing.** The whole rest of the
    /// journey is skipped: the token's previous state is recorded, it is
    /// reset and removed from the board, counted as having left the field,
    /// and the loop goes straight back to the next card. Routing a token
    /// through `move_card` would put it in a graveyard, where no token
    /// belongs.
    ///
    /// Otherwise, if an `EFFECT_TO_GRAVE_REDIRECT_CB` applies *and* there is
    /// a spell/trap zone free, the controller is asked whether to use it.
    /// Both halves matter: the offer is not made when there is nowhere for
    /// the card to go.
    fn send_to_step_5(&mut self, state: &mut SendToState) -> bool {
        if state.cursor >= state.ordered.len() {
            // `set_step(8)`, not 9: the processor's increment means the next
            // case to run is n + 1. The reference writes `arg.step = 8` here
            // for exactly that reason. Setting 9 skips step 9 entirely —
            // which silently drops every `EVENT_LEAVE_FIELD`, since that is
            // the step that raises it, and leaves the cards un-reset.
            self.set_step(8);
            return false;
        }
        let card = state.ordered[state.cursor];
        state.predirect = None;
        if self.cards[card].sendto_param.playerid >> 4 != 0 {
            state.predirect = self.is_affected_by_effect(card, code::TO_GRAVE_REDIRECT_CB);
        }
        self.enable_field_effect(card, false);

        if self.cards[card].data.is_type(card_type::TOKEN) {
            // A token leaving play: the reference writes the whole message
            // here, with an empty `loc_info` for where it goes.
            let previous = self.get_info_location(card);
            self.messages.push(crate::field::Message::Move {
                code: self.cards[card].data.code,
                previous,
                current: crate::leave_field::LocInfo::default(),
                reason: self.cards[card].reason,
            });
            if !self.core.current_chain.is_empty() {
                self.core.just_sent_cards.insert(card);
            }
            let c = &mut self.cards[card];
            c.previous = c.current;
            c.reason &= !reason::TEMPORARY;
            let fid = self.next_field_id_raw();
            let c = &mut self.cards[card];
            c.fieldid = fid;
            c.fieldid_r = fid;
            self.reset_card(card, reset::LEAVE, reset::EVENT);
            self.cards[card].clear_relate_effect();
            self.remove_card(card);
            state.leave_field.insert(card);
            state.cursor += 1;
            self.set_step(4);
            self.cards[card].set_status(status::LEAVE_CONFIRMED, false);
            return false;
        }

        let controller = self.cards[card].current.controller;
        let room = self.get_useable_count(
            Some(card),
            controller,
            location::SZONE,
            controller,
            Field::LOCATION_REASON_TOFIELD,
            0xff,
        ) > 0;
        if state.predirect.is_some() && room {
            self.emplace(Kind::SelectEffectYesNo {
                player: controller,
                description: 97,
                card,
            });
        } else {
            self.core.returns.set(0);
        }
        false
    }

    /// Step 6: the move itself.
    ///
    /// Reached with the answer from step 5 in `returns`. A **yes** means the
    /// callback redirect was accepted, so this step returns immediately and
    /// the machine goes on to steps 7-8 instead.
    ///
    /// Four things happen in an order that matters.
    ///
    /// **The hint timing is charged to the controller of the monster an Xyz
    /// material sits under**, not to the material's own controller — which
    /// for a material is `PLAYER_NONE` by the time this runs.
    ///
    /// **A card sent to the hand or deck loses `STATUS_PROC_COMPLETE`.** It
    /// may be summoned again, and the flag says its summon procedure was
    /// already carried out.
    ///
    /// **The move is skipped entirely when the card is already there** —
    /// same controller and same location. The bookkeeping below it still
    /// runs, which is the point of not returning early.
    ///
    /// **A negated summon erases `previous.location`.** The card never
    /// legitimately occupied the zone, so nothing may later ask where it
    /// came from. That branch is checked *before* the ordinary
    /// left-the-field branch, so a negated summon does not raise a leave
    /// event either.
    fn send_to_step_6(&mut self, state: &mut SendToState) -> bool {
        if self.core.returns.get() != 0 {
            return false;
        }
        let card = state.ordered[state.cursor];
        let oloc = self.cards[card].current.location;
        let param = self.cards[card].sendto_param;
        let playerid = param.playerid & 0x7;
        let dest = param.location;
        let control_player = match self.cards[card].overlay_target {
            Some(t) => self.cards[t].current.controller,
            None => self.cards[card].current.controller,
        };

        let t = match dest {
            d if d == location::GRAVE => Some(timing::TOGRAVE),
            d if d == location::HAND => {
                self.cards[card].set_status(status::PROC_COMPLETE, false);
                Some(timing::TOHAND)
            }
            d if d == location::DECK => {
                self.cards[card].set_status(status::PROC_COMPLETE, false);
                Some(timing::TODECK)
            }
            d if d == location::REMOVED => Some(timing::REMOVE),
            _ => None,
        };
        if let Some(t) = t {
            self.core.hint_timing[control_player as usize] |= t;
        }

        if self.cards[card].current.controller != playerid
            || self.cards[card].current.location != dest
        {
            // `MSG_MOVE`: every card that changes controller or location on
            // a send announces itself, halves either side of the move.
            let message = self.open_move_message(card);
            if let Some(host) = self.cards[card].overlay_target {
                state.detach.insert(host);
                self.xyz_remove(host, card);
            }
            self.move_card(playerid, card, dest, param.sequence, false);
            // Re-read, do not reuse `param`. `move_card` calls `add_card`,
            // and some destinations **rewrite** `sendto_param.position` on
            // the way in: the hand forces face-down unless the card is
            // public, the deck forces face-down, the extra deck forces
            // face-up defence. The reference assigns from the card's field
            // after the move for exactly that reason, and `param` is a copy
            // taken before it.
            //
            // Reusing the snapshot leaves a card returned to the hand
            // face-up there, which the Adjust step then treats as a hand
            // that needs shuffling — a second shuffle the reference never
            // does, and one that rolls the duel's generator and reorders
            // the hand. Nothing in the pool could reach it until a card
            // put something back into a hand.
            self.cards[card].current.position = self.cards[card].sendto_param.position;
            self.close_move_message(message, card);
        }
        if self.cards[card].current.location == location::DECK
            && (self.core.deck_reversed
                || self.cards[card].current.position == position::FACEUP_DEFENSE)
        {
            state.check_decktop_visibility[control_player as usize] = true;
        }
        self.cards[card].set_status(status::LEAVE_CONFIRMED, false);

        if self.cards[card].get_status(status::SUMMON_DISABLED | status::ACTIVATE_DISABLED) {
            self.cards[card].set_status(status::SUMMON_DISABLED | status::ACTIVATE_DISABLED, false);
            self.cards[card].previous.location = 0;
        } else if oloc & location::ONFIELD != 0 {
            self.reset_card(card, reset::LEAVE, reset::EVENT);
            state.leave_field.insert(card);
        } else if oloc == location::GRAVE {
            state.leave_grave.insert(card);
        }
        if self.cards[card].previous.location == location::OVERLAY {
            self.cards[card].previous.controller = control_player;
        }
        state.cursor += 1;
        self.set_step(4);
        false
    }

    /// Step 8: the callback redirect's move, into a spell/trap zone.
    ///
    /// Step 7 asked the host for a seat; this places the card there face-up
    /// and then runs the redirect effect's own operation.
    ///
    /// The reset mask is `RESET_LEAVE + RESET_MSCHANGE` rather than step 6's
    /// `RESET_LEAVE`: the card is changing what it *is* as well as where it
    /// is, and `clear_card_target` is called here and not there for the same
    /// reason.
    fn send_to_step_8(
        &mut self,
        targets: GroupId,
        reason_effect: Option<EffectId>,
        state: &mut SendToState,
    ) -> bool {
        let card = state.ordered[state.cursor];
        let oloc = self.cards[card].current.location;
        // `returns.at<int8_t>(2)` in the reference, not the leading i32:
        // `SelectPlace` answers in three `i8` slots — player, location,
        // sequence — and the sequence is the third. Reading the whole i32
        // here gives all three bytes packed together, which is a plausible
        // sequence number and always the wrong one.
        let seq = self.core.returns.at_i8(2) as u32;
        let message = self.open_move_message(card);
        if let Some(host) = self.cards[card].overlay_target {
            state.detach.insert(host);
            self.xyz_remove(host, card);
        }
        let controller = self.cards[card].current.controller;
        self.move_card(controller, card, location::SZONE, seq, false);
        self.cards[card].current.position = position::FACEUP;
        self.close_move_message(message, card);
        self.cards[card].set_status(status::LEAVE_CONFIRMED, false);

        if self.cards[card].get_status(status::SUMMON_DISABLED | status::ACTIVATE_DISABLED) {
            self.cards[card].set_status(status::SUMMON_DISABLED | status::ACTIVATE_DISABLED, false);
            self.cards[card].previous.location = 0;
        } else if oloc & location::ONFIELD != 0 {
            self.reset_card(card, reset::LEAVE + reset::MSCHANGE, reset::EVENT);
            self.clear_card_target(card);
            state.leave_field.insert(card);
        }
        if let Some(predirect) = state.predirect {
            let has_operation = self
                .effects
                .get(predirect)
                .is_some_and(|e| e.operation.is_some());
            if has_operation {
                let controller = self.cards[card].current.controller;
                let mut e = crate::event::Event::new(0);
                e.event_cards = self.group(targets).iter().copied().collect();
                e.event_player = controller;
                e.event_value = 0;
                e.reason = self.cards[card].reason;
                e.reason_effect = reason_effect;
                e.reason_player = controller;
                self.core.sub_solving_event.push_back(e);
                self.emplace(Kind::ExecuteOperation {
                    resume: None,
                    effect: predirect,
                    player: controller,
                    subject: None,
                    args: Vec::new(),
                    was_disabled: false,
                });
            }
        }
        state.cursor += 1;
        self.set_step(4);
        false
    }
}

impl Field {
    /// Step 9: reset each card for where it has arrived, then raise the
    /// leaving events.
    ///
    /// The reset level is chosen by the **new** location, and the four are
    /// not interchangeable: `RESET_TOHAND`, `RESET_TODECK` (which the extra
    /// deck shares), `RESET_TOGRAVE`, and for the banished pile a choice
    /// between `RESET_REMOVE` and `RESET_TEMP_REMOVE` depending on whether
    /// the card is coming back. An effect that lasts "while this is on the
    /// field" is cancelled by any of them; one that lasts "until it returns"
    /// must not be cancelled by the temporary form.
    ///
    /// A **token** is judged by where it was *being sent*, not where it is —
    /// it is nowhere, having been removed at step 5.
    ///
    /// Tokens are also the exception to re-enabling field effects: every
    /// other card gets `enable_field_effect(true)` here, a token does not.
    /// It no longer exists, and re-enabling would register effects for a
    /// card that is not on the board.
    fn send_to_step_9(
        &mut self,
        targets: GroupId,
        reason_effect: Option<EffectId>,
        why: u32,
        reason_player: u8,
        state: &mut SendToState,
    ) -> bool {
        for card in self.group(targets).iter().copied().collect::<Vec<_>>() {
            let is_token = self.cards[card].data.is_type(card_type::TOKEN);
            if !is_token {
                self.enable_field_effect(card, true);
            }
            let nloc = self.cards[card].current.location;
            if nloc == location::HAND {
                self.reset_card(card, reset::TOHAND, reset::EVENT);
            }
            if nloc == location::DECK || nloc == location::EXTRA {
                self.reset_card(card, reset::TODECK, reset::EVENT);
            }
            if nloc == location::GRAVE {
                self.reset_card(card, reset::TOGRAVE, reset::EVENT);
            }
            let banished = nloc == location::REMOVED
                || (is_token && self.cards[card].sendto_param.location == location::REMOVED);
            if banished {
                let level = if self.cards[card].reason & reason::TEMPORARY != 0 {
                    reset::TEMP_REMOVE
                } else {
                    reset::REMOVE
                };
                self.reset_card(card, level, reset::EVENT);
            }
            self.refresh_disable_status(card);
        }

        for &card in &state.leave_field.clone() {
            let (by, r, rp) = (
                self.cards[card].reason_effect,
                self.cards[card].reason,
                self.cards[card].reason_player,
            );
            self.raise_single_event(card, vec![], code::LEAVE_FIELD, by, r, rp, 0, 0);
        }
        for &card in &state.leave_grave.clone() {
            let (by, r, rp) = (
                self.cards[card].reason_effect,
                self.cards[card].reason,
                self.cards[card].reason_player,
            );
            self.raise_single_event(card, vec![], code::LEAVE_GRAVE, by, r, rp, 0, 0);
        }
        // A detach event is only raised for a host still in a Monster Zone,
        // and only when a card in the duel is watching for one.
        let detach_wanted = self.core.global_flag & global_flag::DETACH_EVENT != 0;
        if detach_wanted && !state.detach.is_empty() {
            for &host in &state.detach.clone() {
                if self.cards[host].current.location & location::MZONE != 0 {
                    self.raise_single_event(
                        host,
                        vec![],
                        code::DETACH_MATERIAL,
                        reason_effect,
                        why,
                        reason_player,
                        0,
                        0,
                    );
                }
            }
        }
        self.process_single_event();

        let leave_field: Vec<CardId> = std::mem::take(&mut state.leave_field).into_iter().collect();
        if !leave_field.is_empty() {
            self.raise_event_over(
                leave_field,
                code::LEAVE_FIELD,
                reason_effect,
                why,
                reason_player,
                0,
                0,
            );
        }
        let leave_grave: Vec<CardId> = std::mem::take(&mut state.leave_grave).into_iter().collect();
        if !leave_grave.is_empty() {
            self.raise_event_over(
                leave_grave,
                code::LEAVE_GRAVE,
                reason_effect,
                why,
                reason_player,
                0,
                0,
            );
        }
        let detach: Vec<CardId> = std::mem::take(&mut state.detach).into_iter().collect();
        if detach_wanted && !detach.is_empty() {
            self.raise_event_over(
                detach,
                code::DETACH_MATERIAL,
                reason_effect,
                why,
                reason_player,
                0,
                0,
            );
        }
        self.process_instant_event();
        self.adjust_instant();
        false
    }

    /// Step 10: cut the ties, then raise everything the move earned.
    ///
    /// Seven events, and a card can qualify for several at once — a
    /// released monster that was also discarded raises both, plus
    /// `EVENT_MOVE`, which every card in the batch raises unconditionally.
    ///
    /// Three of the seven are decided by the **reason** rather than the
    /// destination (`DISCARD`, `RELEASE`, `DESTROYED`), and the destroy one
    /// excludes battle: a monster destroyed in combat raises its destruction
    /// event from the battle machinery, not here.
    ///
    /// `EVENT_TO_GRAVE` has the one exclusion that is easy to miss: a card
    /// **returning** from the banished pile does not raise it, which is what
    /// `REASON_RETURN` was set for back in the queueing function.
    ///
    /// Two consequences are queued at the very end, after the events:
    /// equip cards left with nothing to equip are **destroyed by the rules**,
    /// and Xyz materials left under nothing are **sent to the graveyard** —
    /// the recursive call this whole machine exists to make.
    fn send_to_step_10(
        &mut self,
        targets: GroupId,
        reason_effect: Option<EffectId>,
        why: u32,
        reason_player: u8,
    ) -> bool {
        let mut tohand = Vec::new();
        let mut todeck = Vec::new();
        let mut tograve = Vec::new();
        let mut removed = Vec::new();
        let mut discarded = Vec::new();
        let mut released = Vec::new();
        let mut destroyed = Vec::new();
        let mut equipings = Vec::new();
        let mut overlays = Vec::new();
        let in_chain = !self.core.current_chain.is_empty();

        for card in self.group(targets).iter().copied().collect::<Vec<_>>() {
            let nloc = self.cards[card].current.location;
            if self.cards[card].equiping_target.is_some() {
                self.unequip(card);
            }
            for equipper in self.cards[card].equiping_cards.clone() {
                self.unequip(equipper);
                if self.cards[equipper].current.location == location::SZONE {
                    equipings.push(equipper);
                }
            }
            self.clear_card_target(card);

            let (by, r, rp) = (
                self.cards[card].reason_effect,
                self.cards[card].reason,
                self.cards[card].reason_player,
            );
            if nloc == location::HAND {
                if self.cards[card].owner != self.cards[card].current.controller {
                    self.add_borrowed_card_hint(card);
                }
                if in_chain {
                    self.add_resolving_hand_hint(card);
                }
                tohand.push(card);
                self.raise_single_event(card, vec![], code::TO_HAND, by, r, rp, 0, 0);
            }
            if nloc == location::DECK || nloc == location::EXTRA {
                todeck.push(card);
                self.raise_single_event(card, vec![], code::TO_DECK, by, r, rp, 0, 0);
            }
            if nloc == location::GRAVE && r & reason::RETURN == 0 {
                tograve.push(card);
                self.raise_single_event(card, vec![], code::TO_GRAVE, by, r, rp, 0, 0);
            }
            let banished = nloc == location::REMOVED
                || (self.cards[card].data.is_type(card_type::TOKEN)
                    && self.cards[card].sendto_param.location == location::REMOVED);
            if banished {
                removed.push(card);
                self.raise_single_event(card, vec![], code::REMOVE, by, r, rp, 0, 0);
            }
            if r & reason::DISCARD != 0 {
                discarded.push(card);
                self.raise_single_event(card, vec![], code::DISCARD, by, r, rp, 0, 0);
            }
            if r & reason::RELEASE != 0 {
                released.push(card);
                self.raise_single_event(card, vec![], code::RELEASE, by, r, rp, 0, 0);
            }
            // Non-battle destruction only.
            if r & reason::DESTROY != 0 && r & reason::BATTLE == 0 {
                destroyed.push(card);
                self.raise_single_event(card, vec![], code::DESTROYED, by, r, rp, 0, 0);
            }
            overlays.extend(self.cards[card].xyz_materials.iter().copied());
            self.raise_single_event(card, vec![], code::MOVE, by, r, rp, 0, 0);
        }

        for (cards, ev) in [
            (tohand, code::TO_HAND),
            (todeck, code::TO_DECK),
            (tograve, code::TO_GRAVE),
            (removed, code::REMOVE),
            (discarded, code::DISCARD),
            (released, code::RELEASE),
            (destroyed, code::DESTROYED),
        ] {
            if !cards.is_empty() {
                self.raise_event_over(cards, ev, reason_effect, why, reason_player, 0, 0);
            }
        }
        let all: Vec<CardId> = self.group(targets).iter().copied().collect();
        self.raise_event_over(all, code::MOVE, reason_effect, why, reason_player, 0, 0);
        self.process_single_event();
        self.process_instant_event();

        if !equipings.is_empty() {
            self.destroy(
                equipings,
                None,
                reason::RULE + reason::LOST_TARGET,
                PLAYER_NONE,
                PLAYER_NONE,
                0,
                0,
            );
        }
        if !overlays.is_empty() {
            self.send_to(
                overlays,
                None,
                reason::RULE + reason::LOST_TARGET,
                PLAYER_NONE,
                PLAYER_NONE,
                u16::from(location::GRAVE),
                0,
                position::FACEUP,
                false,
            );
        }
        self.adjust_instant();
        false
    }

    /// The marker put on a card added to a hand *by a resolving effect*.
    ///
    /// Distinct from [`Field::add_borrowed_card_hint`] and they stack: a card
    /// can be both borrowed and freshly added. Description 225, and a reset
    /// mask that is spelled out rather than inherited — it is **not** the
    /// `0x1fe0000` block, because it adds `RESET_CHAIN`: the hint stops
    /// meaning anything once the chain that put the card there has finished.
    pub(crate) fn add_resolving_hand_hint(&mut self, card: CardId) {
        use crate::effect::{effect_type, flag, Effect};
        let mut hint = Effect::new(effect_type::SINGLE, 0);
        hint.owner = Some(card);
        hint.handler = Some(card);
        hint.flag[0] = flag::CANNOT_DISABLE | flag::CLIENT_HINT;
        hint.description = 225;
        hint.reset_flag = reset::EVENT
            | reset::TOFIELD
            | reset::LEAVE
            | reset::TODECK
            | reset::TOHAND
            | reset::TEMP_REMOVE
            | reset::REMOVE
            | reset::TOGRAVE
            | reset::TURN_SET
            | reset::CHAIN;
        // `pcard->add_effect(deffect)`: through the registration, not by
        // hand — the hint has `RESET_CHAIN`, and only `add_card_effect`
        // puts it on the chain-reset list that later removes it (and
        // writes the client's `MSG_CARD_HINT`).
        let hint = self.new_effect(hint);
        self.add_card_effect(card, hint);
    }
}

/// Subsystems `SendTo` reaches that are not ported yet.
///
/// Each **panics** rather than doing nothing, following the convention the
/// rest of the crate uses: nothing can reach them — no duel starts, so no
/// unit runs — and a panic cannot be mistaken for correct behaviour the way
/// a silent no-op can.
impl Field {
    /// Ask the host which spell/trap seat the callback redirect should use.
    ///
    /// `Processors::SelectPlace` plus the `MSG_HINT` that precedes it. The
    /// zone mask the reference builds is
    /// `((flag << 8) & 0xff00) | 0xffffe0ff` — the spell/trap row of the
    /// controller's side, with the seats already in use masked out.
    pub fn select_place_for_redirect(&mut self, card: CardId) {
        let controller = self.cards[card].current.controller;
        let mut flag = 0u32;
        self.get_useable_count_with_flag(
            Some(card),
            controller,
            location::SZONE,
            controller,
            Field::LOCATION_REASON_TOFIELD,
            0xff,
            &mut flag,
        );
        // `((flag << 8) & 0xff00) | 0xffffe0ff` — every bit set (unavailable)
        // except the eight spell/trap seats of this player's row, which the
        // shifted `flag` then narrows to the ones actually free.
        let flag = ((flag << 8) & 0xff00) | 0xffff_e0ff;
        let code = self.cards[card].data.code;
        self.messages.push(crate::field::Message::Hint {
            kind: crate::host_question::hint::SELECTMSG,
            player: controller,
            value: u64::from(code),
        });
        self.emplace(Kind::SelectPlace {
            player: controller,
            flag,
            count: 1,
            disable_field: false,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{Card, CardData};
    use crate::effect::{effect_type, Effect};
    use crate::processor::Status;

    fn monster(f: &mut Field, player: u8, loc: u8, seat: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 18036057,
                type_: card_type::MONSTER | card_type::EFFECT,
                attack: 1900,
                defense: 1400,
                level: 5,
                ..Default::default()
            },
            player,
        );
        c.current.controller = player;
        c.current.position = position::FACEUP_ATTACK;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(player, id, loc, seat, false);
        id
    }

    /// Run the queued unit to completion, or until it wants an answer.
    ///
    /// The step cap is a guard, not a count: `SendTo` loops by jumping back
    /// to step 4, so the number of `process` calls depends on the batch size
    /// and is not something a test should assert on.
    fn run(f: &mut Field) -> Status {
        for _ in 0..256 {
            match f.process() {
                Status::Continue => continue,
                other => return other,
            }
        }
        panic!("SendTo did not terminate");
    }

    fn send_one_to(f: &mut Field, card: CardId, dest: u8, why: u32) {
        f.send_to_card(
            card,
            None,
            why,
            0,
            PLAYER_NONE,
            u16::from(dest),
            0,
            0,
            false,
        );
    }

    mod send_to_queueing {
        use super::*;

        /// `send_to` is for piles. Putting a card on the field is
        /// `move_to_field`, and asking for one here does nothing at all —
        /// not even a queued unit.
        #[test]
        fn a_destination_on_the_field_is_refused() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, location::MZONE, 0);
            f.send_to_card(
                c,
                None,
                reason::EFFECT,
                0,
                PLAYER_NONE,
                u16::from(location::MZONE),
                0,
                0,
                false,
            );
            // `emplace` lands in `core.subunits`, not `units` — those are
            // spliced in at the top of the next `process()`. Asserting on
            // `queue()` here would pass whether or not anything was queued.
            assert!(f.core.subunits.is_empty(), "nothing was queued");
        }

        /// The saved reason is what lets a dropped card be put back exactly
        /// as it was.
        #[test]
        fn the_previous_reason_is_saved_before_being_overwritten() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, location::MZONE, 0);
            f.cards[c].reason = reason::BATTLE;
            f.cards[c].reason_player = 1;

            send_one_to(&mut f, c, location::GRAVE, reason::EFFECT);
            assert_eq!(f.cards[c].reason, reason::EFFECT, "the new reason applies");
            assert_eq!(
                f.cards[c].temp.reason,
                reason::BATTLE,
                "the old one is kept"
            );
            assert_eq!(f.cards[c].temp.reason_player, 1);
        }

        /// Deck to hand with no player named goes to the *reason player's*
        /// hand — which is what makes a search put the card in the
        /// searcher's hand rather than the owner's.
        #[test]
        fn a_search_puts_the_card_in_the_searchers_hand() {
            let mut f = Field::new(8000);
            // Owned by player 1 but sitting in player 1's deck, controlled
            // by player 1; the reason player is 1.
            let c = monster(&mut f, 1, location::DECK, 0);
            f.send_to_card(
                c,
                None,
                reason::EFFECT,
                1,
                PLAYER_NONE,
                u16::from(location::HAND),
                0,
                0,
                false,
            );
            assert_eq!(f.cards[c].sendto_param.playerid, 1);
        }

        #[test]
        fn otherwise_a_card_goes_to_its_owner() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, location::MZONE, 0);
            f.cards[c].owner = 1;
            send_one_to(&mut f, c, location::GRAVE, reason::EFFECT);
            assert_eq!(f.cards[c].sendto_param.playerid, 1, "the owner's graveyard");
        }

        /// Coming back from the banished pile to the graveyard is a
        /// *return*, and step 10 reads that flag to suppress
        /// `EVENT_TO_GRAVE`.
        #[test]
        fn returning_from_banishment_is_flagged() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, location::REMOVED, 0);
            send_one_to(&mut f, c, location::GRAVE, reason::EFFECT);
            assert!(f.cards[c].reason & reason::RETURN != 0);

            let mut g = Field::new(8000);
            let d = monster(&mut g, 0, location::MZONE, 0);
            send_one_to(&mut g, d, location::GRAVE, reason::EFFECT);
            assert_eq!(
                g.cards[d].reason & reason::RETURN,
                0,
                "an ordinary trip to the graveyard is not a return"
            );
        }

        /// Everything but a banish is turned face-up, whatever was asked
        /// for — `ignore` is how a caller means the position it named.
        #[test]
        fn a_position_is_overridden_unless_the_card_is_banished() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, location::MZONE, 0);
            f.send_to_card(
                c,
                None,
                reason::EFFECT,
                0,
                PLAYER_NONE,
                u16::from(location::GRAVE),
                0,
                position::FACEDOWN_DEFENSE,
                false,
            );
            assert_eq!(f.cards[c].sendto_param.position, position::FACEUP);

            let mut g = Field::new(8000);
            let d = monster(&mut g, 0, location::MZONE, 0);
            g.send_to_card(
                d,
                None,
                reason::EFFECT,
                0,
                PLAYER_NONE,
                u16::from(location::REMOVED),
                0,
                position::FACEDOWN_DEFENSE,
                false,
            );
            assert_eq!(
                g.cards[d].sendto_param.position,
                position::FACEDOWN_DEFENSE,
                "a banish keeps the position it was given"
            );
        }
    }

    mod the_machine {
        use super::*;

        #[test]
        fn a_monster_reaches_the_graveyard_and_reports_one() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, location::MZONE, 0);
            send_one_to(&mut f, c, location::GRAVE, reason::EFFECT);
            run(&mut f);

            assert_eq!(f.cards[c].current.location, location::GRAVE);
            assert_eq!(f.core.returns.get(), 1, "one card was sent");
            assert_eq!(f.core.operated_set, vec![c]);
        }

        /// Step 2 snapshots what the card was before it moves, which is the
        /// whole reason the statistics layer exists.
        #[test]
        fn what_the_card_was_is_recorded_on_the_way_out() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, location::MZONE, 0);
            send_one_to(&mut f, c, location::GRAVE, reason::EFFECT);
            run(&mut f);

            let was = &f.cards[c].previous_stats;
            assert_eq!(was.code, 18036057);
            assert_eq!(was.attack, 1900);
            assert_eq!(was.level, 5);
            assert_eq!(
                f.cards[c].previous.location,
                location::MZONE,
                "and where it was"
            );
        }

        /// An empty batch finishes at step 2 with a return of 0.
        #[test]
        fn an_empty_batch_reports_nothing_sent() {
            let mut f = Field::new(8000);
            f.send_to(
                Vec::<CardId>::new(),
                None,
                reason::EFFECT,
                0,
                PLAYER_NONE,
                u16::from(location::GRAVE),
                0,
                0,
                false,
            );
            run(&mut f);
            assert_eq!(f.core.returns.get(), 0);
            assert!(f.core.operated_set.is_empty());
        }

        /// A card that may not be sent is dropped at step 0 **and has its
        /// reason put back** — it was never sent.
        #[test]
        fn a_card_that_cannot_go_is_dropped_and_restored() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, location::MZONE, 0);
            f.cards[c].reason = reason::BATTLE;

            let mut e = Effect::new(effect_type::SINGLE, code::CANNOT_TO_GRAVE);
            e.owner = Some(c);
            e.handler = Some(c);
            let id = f.new_effect(e);
            f.cards[c].single_effect.insert(code::CANNOT_TO_GRAVE, id);
            f.cards[c].indexer.insert(id);

            send_one_to(&mut f, c, location::GRAVE, reason::EFFECT);
            run(&mut f);

            assert_eq!(f.cards[c].current.location, location::MZONE, "it stayed");
            assert_eq!(f.core.returns.get(), 0);
            assert_eq!(
                f.cards[c].reason,
                reason::BATTLE,
                "its reason was put back, not left as EFFECT"
            );
        }

        /// A rule send skips the whole step-0 filter: the same prohibition
        /// does not stop it.
        #[test]
        fn a_rule_send_ignores_what_forbids_it() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, location::MZONE, 0);
            let mut e = Effect::new(effect_type::SINGLE, code::CANNOT_TO_GRAVE);
            e.owner = Some(c);
            e.handler = Some(c);
            let id = f.new_effect(e);
            f.cards[c].single_effect.insert(code::CANNOT_TO_GRAVE, id);
            f.cards[c].indexer.insert(id);

            send_one_to(&mut f, c, location::GRAVE, reason::RULE);
            run(&mut f);
            assert_eq!(
                f.cards[c].current.location,
                location::GRAVE,
                "the rules do not ask permission"
            );
        }

        /// A card already where it is being sent is dropped by
        /// `SendToReplace`, with its reason restored.
        #[test]
        fn a_card_already_at_its_destination_is_dropped() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, location::GRAVE, 0);
            f.cards[c].reason = reason::BATTLE;
            send_one_to(&mut f, c, location::GRAVE, reason::EFFECT);
            run(&mut f);
            assert_eq!(f.core.returns.get(), 0, "it was already there");
            assert_eq!(f.cards[c].reason, reason::BATTLE);
        }

        /// A token stops existing rather than arriving anywhere.
        #[test]
        fn a_token_is_removed_rather_than_moved() {
            let mut f = Field::new(8000);
            let mut c = Card::with_data(
                CardData {
                    code: 73915051,
                    type_: card_type::MONSTER | card_type::TOKEN,
                    ..Default::default()
                },
                0,
            );
            c.current.controller = 0;
            c.current.position = position::FACEUP_ATTACK;
            c.set_status(status::EFFECT_ENABLED, true);
            let t = f.new_card(c);
            f.add_card(0, t, location::MZONE, 0, false);

            send_one_to(&mut f, t, location::GRAVE, reason::EFFECT);
            run(&mut f);

            assert_ne!(
                f.cards[t].current.location,
                location::GRAVE,
                "no token belongs in a graveyard"
            );
            assert_eq!(f.cards[t].previous.location, location::MZONE);
        }

        /// The batch is ordered before it is processed, and a graveyard is
        /// taken topmost-first.
        #[test]
        fn a_batch_is_processed_in_sorted_order() {
            let mut f = Field::new(8000);
            let lo = monster(&mut f, 0, location::GRAVE, 0);
            let hi = monster(&mut f, 0, location::GRAVE, 1);
            f.cards[lo].current.sequence = 0;
            f.cards[hi].current.sequence = 1;

            f.send_to(
                [lo, hi],
                None,
                reason::EFFECT,
                0,
                PLAYER_NONE,
                u16::from(location::REMOVED),
                0,
                0,
                false,
            );
            // Step through to just past the ordering step and read it.
            for _ in 0..8 {
                if let Some(Kind::SendTo { state, .. }) = f.queue().next().map(|u| &u.kind) {
                    if state.prepared {
                        assert_eq!(
                            state.ordered,
                            vec![hi, lo],
                            "a pile is taken from the top: higher sequence first"
                        );
                        return;
                    }
                }
                f.process();
            }
            panic!("the ordering step was never reached");
        }

        /// Both cards arrive, and the batch reports two.
        #[test]
        fn a_batch_sends_every_card() {
            let mut f = Field::new(8000);
            let a = monster(&mut f, 0, location::MZONE, 0);
            let b = monster(&mut f, 0, location::MZONE, 1);
            f.send_to(
                [a, b],
                None,
                reason::EFFECT,
                0,
                PLAYER_NONE,
                u16::from(location::GRAVE),
                0,
                0,
                false,
            );
            run(&mut f);
            assert_eq!(f.cards[a].current.location, location::GRAVE);
            assert_eq!(f.cards[b].current.location, location::GRAVE);
            assert_eq!(f.core.returns.get(), 2);
        }

        /// A card sent to the hand loses `STATUS_PROC_COMPLETE`: it may be
        /// summoned again.
        #[test]
        fn going_to_the_hand_clears_the_summon_procedure_flag() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, location::MZONE, 0);
            f.cards[c].set_status(status::PROC_COMPLETE, true);
            send_one_to(&mut f, c, location::HAND, reason::EFFECT);
            run(&mut f);
            assert_eq!(f.cards[c].current.location, location::HAND);
            assert!(!f.cards[c].is_status(status::PROC_COMPLETE));
        }

        /// The events a move raises are the observable half of steps 9 and
        /// 10. They do not stay in `queue_event`: those steps each call
        /// `process_instant_event` themselves, which drains it, so by the
        /// time the unit finishes the events have landed in `instant_event`.
        fn raised(f: &Field, ev: u32) -> bool {
            f.core.instant_event.iter().any(|e| e.event_code == ev)
        }

        /// An ordinary trip to the graveyard raises `EVENT_TO_GRAVE`.
        #[test]
        fn arriving_in_the_graveyard_raises_the_event() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, location::MZONE, 0);
            send_one_to(&mut f, c, location::GRAVE, reason::EFFECT);
            run(&mut f);
            assert!(raised(&f, code::TO_GRAVE));
            assert!(raised(&f, code::MOVE), "and EVENT_MOVE, always");
        }

        /// But a card **returning** from the banished pile does not — which
        /// is the entire reason `REASON_RETURN` is set at queueing time.
        /// Without this the flag would be set and never read.
        #[test]
        fn returning_from_banishment_raises_no_to_grave_event() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, location::REMOVED, 0);
            send_one_to(&mut f, c, location::GRAVE, reason::EFFECT);
            run(&mut f);

            assert_eq!(f.cards[c].current.location, location::GRAVE, "it arrived");
            assert!(
                !raised(&f, code::TO_GRAVE),
                "a return is not an arrival in the graveyard"
            );
            assert!(raised(&f, code::MOVE), "it still moved");
        }

        /// A card leaving the field raises the pre-leave warning *and* the
        /// real one.
        #[test]
        fn leaving_the_field_raises_both_leave_events() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, location::MZONE, 0);
            send_one_to(&mut f, c, location::GRAVE, reason::EFFECT);
            run(&mut f);
            assert!(
                raised(&f, code::LEAVE_FIELD_P),
                "the warning, before it moved"
            );
            assert!(raised(&f, code::LEAVE_FIELD));
        }

        /// A card whose summon was negated never really arrived, so it
        /// cannot leave: neither leave event is raised for it, and its
        /// `previous.location` is erased so nothing can ask where it came
        /// from.
        #[test]
        fn a_negated_summon_does_not_leave_the_field() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, location::MZONE, 0);
            f.cards[c].set_status(status::SUMMON_DISABLED, true);
            send_one_to(&mut f, c, location::GRAVE, reason::EFFECT);
            run(&mut f);

            assert_eq!(
                f.cards[c].current.location,
                location::GRAVE,
                "it still goes"
            );
            assert!(
                !raised(&f, code::LEAVE_FIELD_P),
                "no warning: it was never really there"
            );
            assert!(!raised(&f, code::LEAVE_FIELD));
            assert_eq!(
                f.cards[c].previous.location, 0,
                "and nothing may ask where it came from"
            );
        }

        /// Three of the seven events at step 10 are decided by the *reason*,
        /// not the destination — and a destroy in battle is excluded, since
        /// the battle machinery raises that one.
        #[test]
        fn the_reason_decides_three_of_the_events() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, location::MZONE, 0);
            send_one_to(&mut f, c, location::GRAVE, reason::EFFECT | reason::DISCARD);
            run(&mut f);
            assert!(raised(&f, code::DISCARD));

            let mut g = Field::new(8000);
            let d = monster(&mut g, 0, location::MZONE, 0);
            send_one_to(&mut g, d, location::GRAVE, reason::DESTROY | reason::BATTLE);
            run(&mut g);
            assert!(
                !raised(&g, code::DESTROYED),
                "a battle destruction is the battle machinery's event to raise"
            );
        }

        /// The hint timings are what a client highlights; a trip to the
        /// graveyard charges `TIMING_TOGRAVE` to the controller.
        #[test]
        fn the_destination_charges_a_hint_timing() {
            let mut f = Field::new(8000);
            let c = monster(&mut f, 0, location::MZONE, 0);
            send_one_to(&mut f, c, location::GRAVE, reason::EFFECT);
            run(&mut f);
            assert!(f.core.hint_timing[0] & timing::TOGRAVE != 0);
        }
    }

    /// **A destination that rewrites the position wins over the one the
    /// caller asked for.**
    ///
    /// `add_card` overrules `sendto_param.position` on the way in — the
    /// hand forces face-down unless the card is public — and the move must
    /// read the field back rather than the copy it took beforehand. The
    /// copy still says what the caller asked for.
    ///
    /// The cost of getting this wrong is not a cosmetic position: a
    /// face-up card sitting in a hand is exactly what the Adjust step
    /// shuffles a hand *for*, so the duel shuffles a second time, rolls
    /// its generator, and reorders the hand. Nothing could reach it until
    /// a card put something back into a hand (Magician of Faith).
    #[test]
    fn a_card_sent_to_the_hand_lands_face_down_however_it_was_asked_for() {
        let mut f = Field::new(8000);
        let c = monster(&mut f, 0, location::GRAVE, 0);
        f.send_to(
            vec![c],
            None,
            reason::EFFECT,
            0,
            PLAYER_NONE,
            u16::from(location::HAND),
            0,
            // The caller asks for face-up, as `Duel.SendtoHand` always does.
            position::FACEUP,
            false,
        );
        run(&mut f);
        assert_eq!(f.cards[c].current.location, location::HAND);
        assert_eq!(
            f.cards[c].current.position,
            position::FACEDOWN,
            "the hand overrules the caller"
        );
        // And so the hand does not then look like one needing a shuffle.
        let public = f.is_affected_by_effect(c, crate::event::code::PUBLIC);
        assert!(public.is_none(), "not a public card");
        assert!(
            !f.cards[c].current.is_faceup(),
            "a face-up card in a hand is what Adjust shuffles for"
        );
    }
}

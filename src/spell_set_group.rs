//! Setting several Spells or Traps at once.
//!
//! `field::process(Processors::SpellSetGroup&)`. The batch form of
//! `SpellSet`, and not a loop around it: the cards are placed in two
//! separate passes, and the seats are shuffled afterwards so the opponent
//! cannot tell which is which.
//!
//! ## Two loops, both backwards
//!
//! | from | `arg.step` | runs next | what it repeats |
//! |---|---|---|---|
//! | 2, more to place | `0` | **case 1** | choosing a seat per card |
//! | 2, done | — (returns false) | case 3 | |
//! | 4, more to move | `2` | **case 3** | moving one card per pass |
//! | 4, done | — (returns false) | case 5 | |
//!
//! Both are the increment rule, and both write a number *two* below the
//! case they reach because they are jumping backwards. Reading either as
//! "go to case 0" or "go to case 2" gets the wrong case and, in the first
//! loop, re-runs the filtering that built the batch.
//!
//! ## Choosing the seats and filling them are separate passes
//!
//! Cases 1-2 ask for every card's seat *before* any card moves, recording
//! them in `set_group_seq` and marking `set_group_used_zones`. Cases 3-4
//! then move them. The separation is what lets case 6 shuffle: the seats
//! are known as a set before anything occupies them.
//!
//! ## The placement mask is built from the far end
//!
//! Case 1 takes `get_useable_count`'s forbidden-zone flag, adds the seats
//! this batch has already claimed, and then **shifts it into the half the
//! asked player is not looking at**, filling everything else with ones:
//!
//! ```text
//! setplayer == toplayer:  ((flag & 0xff) << 8)  | 0xffff00ff
//! otherwise:              ((flag & 0xff) << 24) | 0xffffff
//! ```
//!
//! then `| 0xe080e080` to forbid the zones that are not Spell & Trap seats
//! at all. A set bit is forbidden, so the `0xffff00ff` and `0xffffff`
//! halves are "everything except the row we mean".
//!
//! ## The shuffle skips Field Spells but still consumes their index
//!
//! Case 6 walks `operated_set` with a counter into `set_group_seq`, and
//! `continue`s past a Field Spell **after** incrementing. So a Field Spell
//! in the batch shifts every later card's seat by one. That is the
//! reference's behaviour and it looks like a bug; it is reproduced.
//!
//! The count announced is also reduced by one when a Field Spell is
//! present, since it is not among the seats being shuffled.

use crate::board::{location, position};
use crate::card::{card_type, status};
use crate::event::{code, CardId, EffectId};
use crate::field::{timing, Field, Message};
use crate::processor::Kind;

impl Field {
    /// Queue a group set.
    pub fn spell_set_group(
        &mut self,
        setplayer: u8,
        toplayer: u8,
        targets: Vec<CardId>,
        confirm: bool,
        reason_effect: Option<EffectId>,
    ) {
        self.emplace(Kind::SpellSetGroup {
            setplayer,
            toplayer,
            targets,
            confirm,
            reason_effect,
            set_cards: Vec::new(),
        });
    }

    /// One step of `SpellSetGroup`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn spell_set_group_step(
        &mut self,
        step: u16,
        setplayer: u8,
        toplayer: u8,
        targets: &[CardId],
        confirm: bool,
        reason_effect: Option<EffectId>,
        set_cards: &mut Vec<CardId>,
    ) -> bool {
        match step {
            0 => self.ssg_filter(setplayer, toplayer, targets, set_cards),
            1 => self.ssg_ask_seat(setplayer, toplayer, set_cards),
            2 => self.ssg_record_seat(set_cards),
            3 => self.ssg_move_one(setplayer, toplayer),
            4 => self.ssg_mark_one(),
            5 => {
                if confirm {
                    self.messages.push(Message::ConfirmCards {
                        player: toplayer,
                        codes: self
                            .core
                            .set_group_set
                            .iter()
                            .map(|&c| self.cards[c].data.code)
                            .collect(),
                    });
                    let shown: Vec<(u8, CardId)> = self
                        .core
                        .set_group_set
                        .iter()
                        .map(|&c| (toplayer, c))
                        .collect();
                    self.core.revealed.extend(shown);
                }
                false
            }
            6 => self.ssg_shuffle(toplayer),
            7 => self.ssg_announce(setplayer, reason_effect),
            8 => {
                self.core.returns.set(self.core.operated_set.len() as i32);
                true
            }
            _ => true,
        }
    }

    /// Case 0: which of the offered cards can actually be set, and pay
    /// whatever setting them costs.
    fn ssg_filter(
        &mut self,
        setplayer: u8,
        toplayer: u8,
        targets: &[CardId],
        set_cards: &mut Vec<CardId>,
    ) -> bool {
        self.core.operated_set.clear();
        for &target in targets {
            let ty = self.cards[target].data.type_;
            let no_room = ty & card_type::FIELD == 0
                && self.get_useable_count(
                    Some(target),
                    toplayer,
                    location::SZONE,
                    setplayer,
                    Field::LOCATION_REASON_TOFIELD,
                    0xff,
                ) <= 0;
            let monster_refused = ty & card_type::MONSTER != 0
                && self
                    .is_affected_by_effect(target, code::MONSTER_SSET)
                    .is_none();
            if no_room
                || monster_refused
                || self.cards[target].current.location == location::SZONE
                || !self.is_player_can_sset(setplayer, target)
                || self
                    .is_affected_by_effect(target, code::CANNOT_SSET)
                    .is_some()
            {
                continue;
            }
            if !set_cards.contains(&target) {
                set_cards.push(target);
            }
        }
        if set_cards.is_empty() {
            self.core.returns.set(0);
            return true;
        }

        for &card in set_cards.iter() {
            for e in self.filter_effect(card, code::SSET_COST) {
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
        }
        self.core.set_group_pre_set.clear();
        self.core.set_group_set.clear();
        self.core.set_group_used_zones = 0;
        self.core.phase_action = true;
        false
    }

    /// Case 1: ask where the next card goes — unless it is a Field Spell,
    /// which has only one place to be.
    fn ssg_ask_seat(&mut self, setplayer: u8, toplayer: u8, set_cards: &[CardId]) -> bool {
        let Some(&target) = set_cards.first() else {
            return true;
        };
        if self.cards[target].data.type_ & card_type::FIELD != 0 {
            // Seat 5 is the Field Zone. Written straight into the answer
            // slot `SelectPlace` would have used, so case 2 reads it the
            // same way either path took.
            self.core.returns.set_i8(2, 5);
            return false;
        }
        let mut flag = 0u32;
        self.get_useable_count_with_flag(
            Some(target),
            toplayer,
            location::SZONE,
            setplayer,
            Field::LOCATION_REASON_TOFIELD,
            0xff,
            &mut flag,
        );
        flag |= self.core.set_group_used_zones;
        // Shifted into the half the asked player is *not* looking at, with
        // everything else set — a set bit is forbidden. See the module
        // note; getting the halves the wrong way round offers the
        // opponent's row.
        flag = if setplayer == toplayer {
            ((flag & 0xff) << 8) | 0xffff_00ff
        } else {
            ((flag & 0xff) << 24) | 0x00ff_ffff
        };
        // Everything that is not a Spell & Trap seat.
        flag |= 0xe080_e080;
        self.messages.push(Message::Hint {
            kind: crate::host_question::hint::SELECTMSG,
            player: setplayer,
            value: u64::from(self.cards[target].data.code),
        });
        self.emplace(Kind::SelectPlace {
            player: setplayer,
            flag,
            count: 1,
            disable_field: false,
        });
        false
    }

    /// Case 2: record the seat, and go round again if more remain.
    fn ssg_record_seat(&mut self, set_cards: &mut Vec<CardId>) -> bool {
        let Some(&target) = set_cards.first() else {
            return true;
        };
        let seq = self.core.returns.at_i8(2).max(0) as u32;
        let i = self.core.set_group_pre_set.len().min(6);
        self.core.set_group_seq[i] = seq;
        self.core.set_group_pre_set.push(target);
        self.core.set_group_used_zones |= 1 << seq;
        set_cards.remove(0);
        if !set_cards.is_empty() {
            // 0, not 1: the increment lands on case 1, asking for the next
            // card's seat. See the module note.
            self.set_step(0);
        }
        false
    }

    /// Case 3: move one card into the seat chosen for it.
    fn ssg_move_one(&mut self, setplayer: u8, toplayer: u8) -> bool {
        let Some(&target) = self.core.set_group_pre_set.first() else {
            return true;
        };
        self.enable_field_effect(target, false);
        let zone = if self.cards[target].data.type_ & card_type::FIELD != 0 {
            1 << 5
        } else {
            // **The lowest claimed seat, consumed as it is used.** The
            // cards are not moved in the order their seats were chosen —
            // the mask is walked from the bottom — which is why case 6 has
            // to put them right afterwards.
            let mut found = 0;
            for i in 0..7u32 {
                let bit = 1 << i;
                if self.core.set_group_used_zones & bit != 0 {
                    self.core.set_group_used_zones &= !bit;
                    found = bit;
                    break;
                }
            }
            found
        };
        self.move_to_field(
            target,
            setplayer,
            toplayer,
            u16::from(location::SZONE),
            position::FACEDOWN,
            false,
            0,
            zone,
            false,
            0,
            false,
        );
        false
    }

    /// Case 4: mark it set, grant a Trap Monster its borrowed type, and
    /// go round again if more remain.
    fn ssg_mark_one(&mut self) -> bool {
        let Some(&target) = self.core.set_group_pre_set.first() else {
            return true;
        };
        self.cards[target].set_status(status::SET_TURN, true);
        if self.cards[target].data.type_ & card_type::MONSTER != 0 {
            self.grant_set_monster_type(target);
        }
        self.messages.push(Message::Set {
            code: self.cards[target].data.code,
            controller: self.cards[target].current.controller,
            location: self.cards[target].current.location,
            sequence: self.cards[target].current.sequence,
            position: self.cards[target].current.position,
        });
        self.core.set_group_set.push(target);
        self.core.set_group_pre_set.remove(0);
        if !self.core.set_group_pre_set.is_empty() {
            // 2, not 3: the increment lands on case 3, moving the next.
            self.set_step(2);
        }
        false
    }

    /// Case 6: shuffle the set cards among their seats.
    fn ssg_shuffle(&mut self, toplayer: u8) -> bool {
        self.core.operated_set = self.core.set_group_set.clone();
        let mut ct = self.core.operated_set.len() as u8;
        if self.core.set_group_used_zones & (1 << 5) != 0 {
            // The Field Spell is not among the seats being shuffled.
            ct -= 1;
        }
        if ct <= 1 {
            return false;
        }
        self.messages.push(Message::ShuffleSetCard {
            location: location::SZONE,
            count: ct,
        });
        // **The index advances for every card, including the Field Spell
        // that is skipped.** So a Field Spell in the batch shifts every
        // later card's seat by one. The reference's, and it reads like a
        // slip.
        let cards = self.core.operated_set.clone();
        for (i, card) in cards.into_iter().enumerate() {
            let seq = self.core.set_group_seq[i.min(6)];
            if self.cards[card].data.type_ & card_type::FIELD != 0 {
                continue;
            }
            let s = seq as usize;
            if s < self.players[toplayer as usize].szone.len() {
                self.players[toplayer as usize].szone[s] = Some(card);
                self.cards[card].current.sequence = seq;
            }
        }
        false
    }

    /// Case 7: the window on having set them.
    fn ssg_announce(&mut self, setplayer: u8, reason_effect: Option<EffectId>) -> bool {
        self.adjust_instant();
        let cards = self.core.operated_set.clone();
        self.raise_event_over(cards, code::SSET, reason_effect, 0, setplayer, setplayer, 0);
        self.process_instant_event();
        if self.core.current_chain.is_empty() {
            self.adjust_all();
            self.core.hint_timing[setplayer as usize] |= timing::SSET;
            // `PointEvent(false, false, false)` — none of the three skips.
            self.emplace(Kind::PointEvent {
                skip: crate::point_event::PointEventSkip::default(),
            });
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{Card, CardData};
    use crate::duel::phases;
    use crate::effect::{effect_type, flag, Effect};
    use crate::field::reset;
    use crate::processor::Status;

    fn field() -> Field {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        f.infos.turn_player = 0;
        f.infos.phase = phases::MAIN1;
        f
    }

    fn in_hand(f: &mut Field, player: u8, type_: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 20000 + f.cards.len() as u32,
                type_,
                ..Default::default()
            },
            player,
        );
        c.current.controller = player;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        let seat = f.players[player as usize].hand.len() as u32;
        f.add_card(player, id, location::HAND, seat, false);
        id
    }

    /// Run on, answering the placement questions by taking the lowest
    /// seat the mask leaves free.
    fn run(f: &mut Field) -> Status {
        for _ in 0..2048 {
            match f.process() {
                Status::Continue => continue,
                Status::Awaiting => match f.messages.last() {
                    Some(Message::SelectPlace { player, flag, .. }) => {
                        let (p, flag) = (*player, *flag);
                        let seq = (0..8u32)
                            .find(|s| flag & (1 << (s + 8)) == 0)
                            .expect("a free spell seat");
                        f.core.returns.set_i8(0, p as i8);
                        f.core.returns.set_i8(1, location::SZONE as i8);
                        f.core.returns.set_i8(2, seq as i8);
                    }
                    // Case 7 opens a real response window outside a
                    // chain; decline it.
                    Some(Message::SelectChain { .. }) => f.core.returns.set(-1),
                    other => panic!("unexpected question: {other:?}"),
                },
                other => return other,
            }
        }
        panic!("did not settle");
    }

    fn set_count(f: &Field, player: u8) -> usize {
        f.players[player as usize]
            .szone
            .iter()
            .filter(|s| s.is_some())
            .count()
    }

    /// **A batch of ordinary Spells is set, all of them.**
    #[test]
    fn a_batch_of_spells_is_set() {
        let mut f = field();
        let a = in_hand(&mut f, 0, card_type::SPELL);
        let b = in_hand(&mut f, 0, card_type::SPELL);
        let c = in_hand(&mut f, 0, card_type::SPELL);
        f.core.returns.set(-99);
        f.spell_set_group(0, 0, vec![a, b, c], false, None);
        run(&mut f);
        assert_eq!(set_count(&f, 0), 3, "all three reached the row");
        assert_eq!(f.core.returns.get(), 3, "and the count is reported");
        for id in [a, b, c] {
            assert_eq!(f.cards[id].current.location, location::SZONE);
            assert!(f.cards[id].current.is_position(position::FACEDOWN));
            assert!(f.cards[id].is_status(status::SET_TURN));
        }
    }

    /// **A card already in the Spell & Trap row is filtered out**, and so
    /// is a monster with no permission to be set there.
    #[test]
    fn the_ineligible_are_filtered_out() {
        let mut f = field();
        let ok = in_hand(&mut f, 0, card_type::SPELL);
        let monster = in_hand(&mut f, 0, card_type::MONSTER);
        let already = in_hand(&mut f, 0, card_type::SPELL);
        f.remove_card(already);
        f.add_card(0, already, location::SZONE, 0, false);

        f.spell_set_group(0, 0, vec![ok, monster, already], false, None);
        run(&mut f);
        assert_eq!(f.cards[ok].current.location, location::SZONE, "set");
        assert_eq!(
            f.cards[monster].current.location,
            location::HAND,
            "a monster without MONSTER_SSET is refused"
        );
        assert_eq!(f.core.returns.get(), 1, "one card, not three");
    }

    /// **Nothing eligible finishes at once with zero.** Poisoned first, so
    /// the assertion cannot pass on an untouched `returns`.
    #[test]
    fn an_empty_batch_reports_zero() {
        let mut f = field();
        let monster = in_hand(&mut f, 0, card_type::MONSTER);
        f.core.returns.set(-99);
        f.spell_set_group(0, 0, vec![monster], false, None);
        assert_eq!(f.process(), Status::Continue);
        assert_eq!(f.core.returns.get(), 0);
        assert_eq!(set_count(&f, 0), 0);
    }

    /// **A monster WITH `MONSTER_SSET` is set, and borrows the type.**
    /// The same helper `SpellSet` uses, which is why it is shared.
    #[test]
    fn a_permitted_monster_is_set_and_borrows_its_type() {
        let mut f = field();
        let m = in_hand(&mut f, 0, card_type::MONSTER);
        let mut e = Effect::new(effect_type::SINGLE, code::MONSTER_SSET);
        e.owner = Some(m);
        e.handler = Some(m);
        e.value = i64::from(card_type::TRAP | card_type::CONTINUOUS);
        let id = f.new_effect(e);
        f.cards[m].single_effect.insert(code::MONSTER_SSET, id);
        f.cards[m].indexer.insert(id);

        f.spell_set_group(0, 0, vec![m], false, None);
        run(&mut f);
        assert_eq!(f.cards[m].current.location, location::SZONE);
        let granted = *f.cards[m]
            .single_effect
            .equal_range(code::CHANGE_TYPE)
            .first()
            .expect("a borrowed type");
        assert_eq!(
            f.effects.get(granted).unwrap().value,
            i64::from(card_type::TRAP | card_type::CONTINUOUS)
        );
        // `RESET_EVENT + 0x1fe0000`: LEAVE included, unlike the equip
        // borrow's `0x17e0000`.
        assert_eq!(
            f.effects.get(granted).unwrap().reset_flag - reset::EVENT,
            0x1fe_0000
        );
    }

    /// **Each card gets its own seat.** The mask carries the seats this
    /// batch has already claimed, so no two land in the same one — which
    /// a single-card batch could never show.
    #[test]
    fn no_two_cards_are_offered_the_same_seat() {
        let mut f = field();
        let cards: Vec<CardId> = (0..4)
            .map(|_| in_hand(&mut f, 0, card_type::SPELL))
            .collect();
        f.spell_set_group(0, 0, cards.clone(), false, None);
        run(&mut f);
        let mut seats: Vec<u32> = cards.iter().map(|&c| f.cards[c].current.sequence).collect();
        seats.sort_unstable();
        seats.dedup();
        assert_eq!(seats.len(), 4, "four distinct seats");
        assert_eq!(set_count(&f, 0), 4);
    }

    /// **A Field Spell is not asked where to go** — it has one place —
    /// and it does not consume a Spell & Trap seat.
    #[test]
    fn a_field_spell_takes_its_own_zone() {
        let mut f = field();
        let field_spell = in_hand(&mut f, 0, card_type::SPELL | card_type::FIELD);
        f.spell_set_group(0, 0, vec![field_spell], false, None);
        run(&mut f);
        assert_eq!(f.cards[field_spell].current.location, location::SZONE);
        assert_eq!(
            f.cards[field_spell].current.sequence, 5,
            "the Field Zone, not a Spell & Trap seat"
        );
    }

    /// **The shuffle message is withheld for a single card.** There is
    /// nothing to hide when only one was set.
    #[test]
    fn one_card_is_not_shuffled() {
        let mut f = field();
        let a = in_hand(&mut f, 0, card_type::SPELL);
        f.spell_set_group(0, 0, vec![a], false, None);
        run(&mut f);
        assert!(
            !f.messages
                .iter()
                .any(|m| matches!(m, Message::ShuffleSetCard { .. })),
            "no shuffle announced"
        );
    }

    /// And two or more are shuffled, with the count announced.
    #[test]
    fn a_batch_is_shuffled_with_its_count() {
        let mut f = field();
        let a = in_hand(&mut f, 0, card_type::SPELL);
        let b = in_hand(&mut f, 0, card_type::SPELL);
        f.spell_set_group(0, 0, vec![a, b], false, None);
        run(&mut f);
        let shuffled = f.messages.iter().find_map(|m| match m {
            Message::ShuffleSetCard { location, count } => Some((*location, *count)),
            _ => None,
        });
        assert_eq!(shuffled, Some((location::SZONE, 2)));
    }

    /// **`confirm` shows the opponent what was set**, and without it
    /// nothing is revealed.
    #[test]
    fn confirm_reveals_the_batch() {
        for confirm in [false, true] {
            let mut f = field();
            let a = in_hand(&mut f, 0, card_type::SPELL);
            let b = in_hand(&mut f, 0, card_type::SPELL);
            f.spell_set_group(0, 0, vec![a, b], confirm, None);
            run(&mut f);
            let revealed = f
                .messages
                .iter()
                .any(|m| matches!(m, Message::ConfirmCards { .. }));
            assert_eq!(revealed, confirm, "confirm = {confirm}");
        }
    }

    /// **The set-window timing reaches the question the window asks.**
    ///
    /// Checked *at* the chain question rather than afterwards: the window
    /// consumes `hint_timing` on its way through, so a check at the end
    /// finds zero whether the timing was ever set or not.
    #[test]
    fn setting_opens_the_window() {
        let mut f = field();
        let a = in_hand(&mut f, 0, card_type::SPELL);
        f.spell_set_group(0, 0, vec![a], false, None);

        let mut seen = None;
        for _ in 0..2048 {
            match f.process() {
                Status::Continue => continue,
                Status::Awaiting => match f.messages.last() {
                    Some(Message::SelectPlace { player, flag, .. }) => {
                        let (p, flag) = (*player, *flag);
                        let seq = (0..8u32)
                            .find(|s| flag & (1 << (s + 8)) == 0)
                            .expect("a free spell seat");
                        f.core.returns.set_i8(0, p as i8);
                        f.core.returns.set_i8(1, location::SZONE as i8);
                        f.core.returns.set_i8(2, seq as i8);
                    }
                    Some(Message::SelectChain { hint_timing, .. }) => {
                        seen = seen.or(Some(*hint_timing));
                        f.core.returns.set(-1);
                    }
                    other => panic!("unexpected question: {other:?}"),
                },
                _ => break,
            }
        }
        let timing_at_window = seen.expect("a window was opened");
        assert_ne!(
            timing_at_window & timing::SSET,
            0,
            "the set timing reached the window"
        );
        assert!(f.core.phase_action, "a group set is a phase action");
    }

    /// **The non-seat zones are forbidden in the offer.** `0xe080e080`
    /// marks everything that is not a Spell & Trap seat, on both sides.
    /// Without it the question offers places a set card cannot go, and
    /// a host that picks the lowest legal seat never notices.
    #[test]
    fn the_offer_forbids_everything_that_is_not_a_spell_seat() {
        let mut f = field();
        let a = in_hand(&mut f, 0, card_type::SPELL);
        f.spell_set_group(0, 0, vec![a], false, None);
        let mut asked = None;
        for _ in 0..64 {
            if f.process() != Status::Continue {
                if let Some(Message::SelectPlace { flag, .. }) = f.messages.last() {
                    asked = Some(*flag);
                }
                break;
            }
        }
        let flag = asked.expect("a placement question");
        assert_eq!(
            flag & 0xe080_e080,
            0xe080_e080,
            "every non-seat bit is forbidden"
        );
    }

    /// **A Field Spell is not asked at all.** It has one place to go, so
    /// the unit writes the answer itself. Removing the shortcut still
    /// lands the card in the Field Zone — case 3 decides that from the
    /// card's type — so only the *absence of the question* shows it.
    #[test]
    fn a_field_spell_is_never_asked_where_to_go() {
        let mut f = field();
        let fs = in_hand(&mut f, 0, card_type::SPELL | card_type::FIELD);
        f.spell_set_group(0, 0, vec![fs], false, None);
        let mut asked_place = false;
        for _ in 0..2048 {
            match f.process() {
                Status::Continue => continue,
                Status::Awaiting => match f.messages.last() {
                    Some(Message::SelectPlace { .. }) => {
                        asked_place = true;
                        break;
                    }
                    Some(Message::SelectChain { .. }) => f.core.returns.set(-1),
                    other => panic!("unexpected question: {other:?}"),
                },
                _ => break,
            }
        }
        assert!(!asked_place, "no placement question for a Field Spell");
        assert_eq!(f.cards[fs].current.sequence, 5);
    }

    /// **The player-level prohibition is read.** Distinct from the
    /// per-card one: this is `is_player_can_sset`, and a card-targeted
    /// `CANNOT_SSET` does not exercise it.
    #[test]
    fn a_player_level_prohibition_refuses_the_set() {
        let mut f = field();
        let a = in_hand(&mut f, 0, card_type::SPELL);
        // A player-target `CANNOT_SSET` with no `target_filter` refuses
        // outright — the reference's shape for a blanket ban.
        let anchor = in_hand(&mut f, 0, card_type::SPELL);
        let mut e = Effect::new(effect_type::FIELD, code::CANNOT_SSET);
        e.owner = Some(anchor);
        e.handler = Some(anchor);
        e.flag[0] = flag::PLAYER_TARGET | flag::ABSOLUTE_TARGET;
        e.range = u16::from(location::HAND);
        e.s_range = 1;
        let id = f.new_effect(e);
        f.add_effect(id, 0);

        f.core.returns.set(-99);
        f.spell_set_group(0, 0, vec![a], false, None);
        run(&mut f);
        assert_eq!(f.core.returns.get(), 0, "refused");
        assert_eq!(f.cards[a].current.location, location::HAND);
    }

    /// **A full Spell & Trap row stops the set.** Nothing else in the
    /// suite fills it, so the room check is otherwise never exercised.
    #[test]
    fn a_full_row_refuses_the_set() {
        let mut f = field();
        let seats = f.players[0].szone.len().min(5);
        for seat in 0..seats {
            let filler = in_hand(&mut f, 0, card_type::SPELL);
            f.remove_card(filler);
            f.add_card(0, filler, location::SZONE, seat as u32, false);
        }
        let a = in_hand(&mut f, 0, card_type::SPELL);
        f.core.returns.set(-99);
        f.spell_set_group(0, 0, vec![a], false, None);
        run(&mut f);
        assert_eq!(f.core.returns.get(), 0, "no room");
        assert_eq!(f.cards[a].current.location, location::HAND);
    }

    /// **The shuffle count excludes a Field Spell**, which is not among
    /// the seats being shuffled. A batch without one cannot show this.
    #[test]
    fn the_shuffle_count_leaves_out_the_field_spell() {
        let mut f = field();
        let fs = in_hand(&mut f, 0, card_type::SPELL | card_type::FIELD);
        let a = in_hand(&mut f, 0, card_type::SPELL);
        let b = in_hand(&mut f, 0, card_type::SPELL);
        f.spell_set_group(0, 0, vec![fs, a, b], false, None);
        run(&mut f);
        let shuffled = f.messages.iter().find_map(|m| match m {
            Message::ShuffleSetCard { count, .. } => Some(*count),
            _ => None,
        });
        assert_eq!(shuffled, Some(2), "three set, two shuffled");
        assert_eq!(f.core.returns.get(), 3, "but all three were set");
    }

    /// **The prohibition is read per card.** One refused card does not
    /// stop the rest.
    #[test]
    fn a_prohibited_card_does_not_block_the_others() {
        let mut f = field();
        let ok = in_hand(&mut f, 0, card_type::SPELL);
        let barred = in_hand(&mut f, 0, card_type::SPELL);
        let mut e = Effect::new(effect_type::SINGLE, code::CANNOT_SSET);
        e.owner = Some(barred);
        e.handler = Some(barred);
        e.flag[0] = flag::CANNOT_DISABLE;
        let id = f.new_effect(e);
        f.cards[barred].single_effect.insert(code::CANNOT_SSET, id);
        f.cards[barred].indexer.insert(id);

        f.spell_set_group(0, 0, vec![ok, barred], false, None);
        run(&mut f);
        assert_eq!(f.cards[ok].current.location, location::SZONE);
        assert_eq!(f.cards[barred].current.location, location::HAND);
        assert_eq!(f.core.returns.get(), 1);
    }
}

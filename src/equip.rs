//! Attaching an equip card to a monster.
//!
//! `field::process(Processors::Equip&)` and `card::equip`.
//!
//! ## The jump table, and the case that looks like a typo
//!
//! | from | `arg.step` | runs next | outcome |
//! |---|---|---|---|
//! | 0, refused outright | — (returns true) | nothing | `returns` false |
//! | 0, sent to the graveyard | `2` | **case 3** | `returns` false |
//! | 0, otherwise | — (returns false) | case 1 | |
//! | 1, `is_step` | — (returns true) | nothing | `returns` true |
//! | 1, otherwise | — (returns false) | case 2 | `returns` true |
//!
//! `arg.step = 2` reaches case **3** by the increment rule, and case 3 is
//! the *failure* case — `returns` false. Case 2 is the success case,
//! reached only by falling out of case 1.
//!
//! Writing `set_step(3)` there would reach case 4, fall through the
//! switch, and return with `returns` still false from case 0's first
//! line — **the same observable result**. So that particular slip is an
//! equivalent mutation here, and would stay equivalent only for as long as
//! nothing else is added past case 3.
//!
//! ## Three ways to fail, and only one of them is quiet
//!
//! The equip is refused outright when the equip card is unaffected by the
//! reason effect, or when it would equip to itself. Otherwise a missing or
//! face-down target, or no room in the Spell & Trap row, sends the equip
//! card **to the graveyard** — except under
//! `DUEL_EQUIP_NOT_SENT_IF_MISSING_TARGET` with the card already in the
//! Monster Zone, which refuses instead.
//!
//! ## A borrowed type, granted on the way
//!
//! A card equipping that is not printed `TYPE_EQUIP` is given the type it
//! needs, and *which* effect depends on what it is: a Trap gains
//! `EFFECT_ADD_TYPE` (keeping its Trap-ness), while everything else gets
//! `EFFECT_CHANGE_TYPE` to a composed value that names Union or Token
//! explicitly when it is one. Reading only the last branch grants a plain
//! Equip Spell and loses the rest.

use crate::board::{location, position};
use crate::card::{card_type, reason};
use crate::effect::{effect_type, flag, Effect};
use crate::event::{code, CardId, PLAYER_NONE};
use crate::field::{reset, timing, Field, Message};
use crate::processor::Kind;

impl Field {
    /// `card::equip` — attach `card` to `target`.
    ///
    /// Refuses silently if the card is already equipping something, which
    /// is why `Equip`'s case 0 unequips first rather than relying on this.
    pub fn equip_card(&mut self, card: CardId, target: CardId, send_msg: bool) {
        if self.cards[card].equiping_target.is_some() {
            return;
        }
        self.cards[target].equiping_cards.push(card);
        self.cards[card].equiping_target = Some(target);
        let related: Vec<_> = self.cards[card]
            .equip_effect
            .iter()
            .map(|&(_, id)| id)
            .collect();
        if related.iter().any(|&id| self.is_disable_related(id)) {
            self.add_to_disable_check_list(target);
        }
        if send_msg {
            self.messages.push(Message::Equip {
                equip: self.get_info_location(card),
                target: self.get_info_location(target),
            });
        }
    }

    /// Queue an equip.
    pub fn equip(
        &mut self,
        equip_player: u8,
        equip_card: CardId,
        target: CardId,
        faceup: bool,
        is_step: bool,
    ) {
        self.emplace(Kind::Equip {
            equip_player,
            equip_card,
            target,
            faceup,
            is_step,
        });
    }

    /// One step of `Equip`.
    pub(crate) fn equip_step(
        &mut self,
        step: u16,
        equip_player: u8,
        equip_card: CardId,
        target: CardId,
        faceup: bool,
        is_step: bool,
    ) -> bool {
        match step {
            0 => self.equip_place(equip_player, equip_card, target, faceup),
            1 => self.equip_attach(equip_card, target, is_step),
            2 => {
                self.core.returns.set(1);
                true
            }
            3 => {
                self.core.returns.set(0);
                true
            }
            _ => true,
        }
    }

    /// Case 0: get the equip card into the Spell & Trap row, or give up.
    fn equip_place(
        &mut self,
        equip_player: u8,
        equip_card: CardId,
        target: CardId,
        faceup: bool,
    ) -> bool {
        // **Failure is the default.** Every early exit leaves this in
        // place, which is why case 3 setting it again is redundant.
        self.core.returns.set(0);
        let by = self.core.reason_effect;
        if !self.is_affect_by_effect(equip_card, by) {
            return true;
        }
        if equip_card == target {
            return true;
        }

        let mut to_grave = false;
        let t = &self.cards[target].current;
        if t.location != location::MZONE || t.is_position(position::FACEDOWN) {
            // The one configuration where a missing target refuses rather
            // than sending the equip card away.
            if self.is_flag(crate::duel::flags::EQUIP_NOT_SENT_IF_MISSING_TARGET)
                && self.cards[equip_card].current.location == location::MZONE
            {
                return true;
            }
            to_grave = true;
        }
        if self.cards[equip_card].current.location != location::SZONE {
            self.refresh_location_info_instant();
            if self.get_useable_count(
                Some(equip_card),
                equip_player,
                location::SZONE,
                equip_player,
                Field::LOCATION_REASON_TOFIELD,
                0xff,
            ) <= 0
            {
                to_grave = true;
            }
        }

        if to_grave {
            if self.cards[equip_card].current.location != location::GRAVE {
                self.send_to(
                    [equip_card],
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
            // 2, not 3: the increment lands on case 3, the failure case.
            self.set_step(2);
            return false;
        }

        if let Some(old) = self.cards[equip_card].equiping_target {
            // Already equipping something else: detach first, and let
            // case 1 attach to the new target.
            self.cards[equip_card]
                .effect_target_cards
                .retain(|&c| c != old);
            self.cards[old]
                .effect_target_owner
                .retain(|&c| c != equip_card);
            self.unequip(equip_card);
            self.enable_field_effect(equip_card, false);
            return false;
        }
        if self.cards[equip_card].current.location == location::SZONE {
            if faceup
                && self.cards[equip_card]
                    .current
                    .is_position(position::FACEDOWN)
            {
                // The reference's single-card overload sets
                // `position_param` straight to `npos`; the port's takes
                // the four targets, so the same value goes in all four.
                let up = position::FACEUP;
                self.change_position([equip_card], None, equip_player, up, up, up, up, 0, false);
            }
            return false;
        }

        self.enable_field_effect(equip_card, false);
        self.cards[equip_card].reason_player = equip_player;
        let pos = if faceup || self.cards[equip_card].current.is_position(position::FACEUP) {
            position::FACEUP
        } else {
            position::FACEDOWN
        };
        self.move_to_field(
            equip_card,
            equip_player,
            equip_player,
            u16::from(location::SZONE),
            pos,
            false,
            0,
            0xff,
            false,
            0,
            true,
        );
        false
    }

    /// Case 1: attach, grant the borrowed type, and open the window.
    fn equip_attach(&mut self, equip_card: CardId, target: CardId, is_step: bool) -> bool {
        self.equip_card(equip_card, target, true);
        if self.cards[equip_card].data.type_ & card_type::EQUIP == 0 {
            self.grant_equip_type(equip_card);
        }
        // The ties, set **directly**: the reference's `Equip` inserts
        // into the two containers itself rather than calling
        // `add_card_target`, so no `MSG_CARD_TARGET` is written and no
        // disable check is queued. An equip is announced by `MSG_EQUIP`.
        if !self.cards[equip_card].effect_target_cards.contains(&target) {
            self.cards[equip_card].effect_target_cards.push(target);
        }
        if !self.cards[target].effect_target_owner.contains(&equip_card) {
            self.cards[target].effect_target_owner.push(equip_card);
        }

        if is_step {
            // A step inside a larger operation reports success and leaves
            // the window to whoever is driving it.
            self.core.equiping_cards.push(equip_card);
            self.core.returns.set(1);
            return true;
        }

        if self.cards[equip_card].current.is_position(position::FACEUP) {
            self.enable_field_effect(equip_card, true);
        }
        self.adjust_disable_check_list();
        let by = self.core.reason_effect;
        let rp = self.core.reason_player;
        self.raise_single_event(
            target,
            vec![equip_card],
            code::EQUIP,
            by,
            0,
            rp,
            PLAYER_NONE,
            0,
        );
        self.raise_event_over(vec![equip_card], code::EQUIP, by, 0, rp, PLAYER_NONE, 0);
        // The timing belongs to whoever **controls** the target — and for
        // an Xyz material that is the controller of the monster it is
        // under, not of the material itself.
        let owner = match self.cards[target].overlay_target {
            Some(t) => self.cards[t].current.controller,
            None => self.cards[target].current.controller,
        };
        self.core.hint_timing[owner as usize] |= timing::EQUIP;
        self.process_single_event();
        self.process_instant_event();
        false
    }

    /// The type an equipping card borrows, which is four different
    /// effects depending on what it already is.
    fn grant_equip_type(&mut self, equip_card: CardId) {
        let printed = self.cards[equip_card].data.type_;
        let (code_, value) =
            if self.get_type(equip_card, None, 0, PLAYER_NONE) & card_type::TRAP != 0 {
                // A Trap Monster **adds** Equip and stays a Trap.
                (code::ADD_TYPE, card_type::EQUIP)
            } else if printed & card_type::UNION != 0 {
                (
                    code::CHANGE_TYPE,
                    card_type::EQUIP + card_type::SPELL + card_type::UNION,
                )
            } else if printed & card_type::TOKEN != 0 {
                (
                    code::CHANGE_TYPE,
                    card_type::EQUIP + card_type::SPELL + card_type::TOKEN,
                )
            } else {
                (code::CHANGE_TYPE, card_type::EQUIP + card_type::SPELL)
            };
        let mut e = Effect::new(effect_type::SINGLE, code_);
        e.owner = Some(equip_card);
        e.handler = Some(equip_card);
        e.value = i64::from(value);
        e.flag[0] = flag::CANNOT_DISABLE;
        // `RESET_EVENT + 0x17e0000`, decoded rather than remembered:
        // `TURN_SET | TOGRAVE | REMOVE | TEMP_REMOVE | TOHAND | TODECK |
        // TOFIELD`. It **includes** `TOFIELD` and **excludes** `LEAVE`,
        // which is the opposite of what the shape of the number suggests
        // — the borrowed type survives leaving the field but not arriving
        // on it. (Contrast `0x1fe0000`, the same block plus `LEAVE`, used
        // by the hand hints in `SendTo`.)
        e.reset_flag = reset::EVENT
            | reset::TURN_SET
            | reset::TOGRAVE
            | reset::REMOVE
            | reset::TEMP_REMOVE
            | reset::TOHAND
            | reset::TODECK
            | reset::TOFIELD;
        let id = self.new_effect(e);
        self.cards[equip_card].single_effect.insert(code_, id);
        self.cards[equip_card].indexer.insert(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{status, Card, CardData};
    use crate::duel::phases;
    use crate::processor::Status;

    fn field() -> Field {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        f.infos.turn_player = 0;
        f.infos.phase = phases::MAIN1;
        f
    }

    fn monster(f: &mut Field, player: u8) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 40000,
                type_: card_type::MONSTER,
                level: 4,
                attack: 1000,
                ..Default::default()
            },
            player,
        );
        c.current.controller = player;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        let seat = f.players[player as usize]
            .mzone
            .iter()
            .position(Option::is_none)
            .expect("a free seat") as u32;
        f.add_card(player, id, location::MZONE, seat, false);
        f.cards[id].current.position = position::FACEUP_ATTACK;
        id
    }

    /// A spell of `type_`, in `player`'s hand unless placed elsewhere.
    fn spell(f: &mut Field, player: u8, type_: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 41000,
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

    /// Run on, answering the one question an equip asks: `move_to_field`
    /// wants to know which Spell & Trap seat. Anything else is a setup
    /// error.
    fn run(f: &mut Field) -> Status {
        for _ in 0..512 {
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
                    other => panic!("unexpected question: {other:?}"),
                },
                other => return other,
            }
        }
        panic!("did not settle");
    }

    /// **The borrowed type's reset mask, decoded.**
    ///
    /// `RESET_EVENT + 0x17e0000`. The number's shape suggests the
    /// leaving-the-field block, and it is not: it **includes**
    /// `RESET_TOFIELD` and **excludes** `RESET_LEAVE`. Pinned as a literal
    /// because nothing else would fail if it drifted — a wrong mask
    /// produces an effect that resets at slightly the wrong moments, which
    /// is invisible until a specific card cares.
    #[test]
    fn the_borrowed_types_reset_mask_is_the_references() {
        let expected = reset::TURN_SET
            | reset::TOGRAVE
            | reset::REMOVE
            | reset::TEMP_REMOVE
            | reset::TOHAND
            | reset::TODECK
            | reset::TOFIELD;
        assert_eq!(expected, 0x017e_0000, "the reference's literal");
        assert_eq!(expected & reset::LEAVE, 0, "LEAVE is NOT in it");
        assert_ne!(expected & reset::TOFIELD, 0, "TOFIELD is");

        let mut f = field();
        let m = monster(&mut f, 0);
        let s = spell(&mut f, 0, card_type::SPELL);
        f.equip(0, s, m, true, false);
        run(&mut f);
        let granted = *f.cards[s]
            .single_effect
            .equal_range(code::CHANGE_TYPE)
            .first()
            .expect("a borrowed type was granted");
        assert_eq!(
            f.effects.get(granted).unwrap().reset_flag,
            reset::EVENT | expected
        );
    }

    /// **Which effect is granted depends on what the card already is.**
    /// A Trap *adds* Equip and stays a Trap; everything else *changes*
    /// type, naming Union or Token when it is one.
    #[test]
    fn the_borrowed_type_differs_by_what_the_card_is() {
        let cases = [
            (
                card_type::SPELL,
                code::CHANGE_TYPE,
                card_type::EQUIP + card_type::SPELL,
            ),
            (
                card_type::SPELL | card_type::UNION,
                code::CHANGE_TYPE,
                card_type::EQUIP + card_type::SPELL + card_type::UNION,
            ),
            (card_type::TRAP, code::ADD_TYPE, card_type::EQUIP),
        ];
        for (printed, expect_code, expect_value) in cases {
            let mut f = field();
            let m = monster(&mut f, 0);
            let s = spell(&mut f, 0, printed);
            f.equip(0, s, m, true, false);
            run(&mut f);
            let id = *f.cards[s]
                .single_effect
                .equal_range(expect_code)
                .first()
                .unwrap_or_else(|| panic!("printed {printed:#x}: no effect under {expect_code}"));
            assert_eq!(
                f.effects.get(id).unwrap().value,
                i64::from(expect_value),
                "printed {printed:#x}"
            );
        }
    }

    /// A card printed `TYPE_EQUIP` borrows nothing — it already is one.
    #[test]
    fn a_printed_equip_spell_borrows_nothing() {
        let mut f = field();
        let m = monster(&mut f, 0);
        let s = spell(&mut f, 0, card_type::SPELL | card_type::EQUIP);
        f.equip(0, s, m, true, false);
        run(&mut f);
        assert!(f.cards[s]
            .single_effect
            .equal_range(code::CHANGE_TYPE)
            .is_empty());
        assert!(f.cards[s]
            .single_effect
            .equal_range(code::ADD_TYPE)
            .is_empty());
        assert_eq!(f.cards[s].equiping_target, Some(m), "and it is attached");
    }

    /// **Equipping to itself is refused.**
    ///
    /// The card has to be one that would *otherwise* be a legal target of
    /// itself — a face-up monster in the Monster Zone. A spell sitting in
    /// the hand is refused anyway for not being a target at all, so it
    /// reports the same failure with the self-check deleted and proves
    /// nothing.
    #[test]
    fn a_card_cannot_equip_to_itself() {
        let mut f = field();
        let m = monster(&mut f, 0);
        f.core.returns.set(-99);
        f.equip(0, m, m, true, false);
        run(&mut f);
        assert_eq!(f.core.returns.get(), 0, "refused");
        assert_eq!(f.cards[m].equiping_target, None, "and not attached");
        assert_eq!(
            f.cards[m].current.location,
            location::MZONE,
            "and left where it was"
        );
    }

    /// **A face-down target sends the equip card to the graveyard**, and
    /// the unit reports failure.
    #[test]
    fn a_face_down_target_sends_the_equip_card_away() {
        let mut f = field();
        let m = monster(&mut f, 0);
        f.cards[m].current.position = position::FACEDOWN_DEFENSE;
        let s = spell(&mut f, 0, card_type::SPELL);
        f.core.returns.set(-99);
        f.equip(0, s, m, true, false);
        run(&mut f);
        assert_eq!(f.cards[s].current.location, location::GRAVE);
        assert_eq!(f.cards[m].equiping_cards, Vec::<CardId>::new());
        assert_eq!(f.core.returns.get(), 0, "reported as a failure");
    }

    /// A target that is not in the Monster Zone at all, likewise.
    #[test]
    fn a_target_off_the_field_sends_the_equip_card_away() {
        let mut f = field();
        let m = monster(&mut f, 0);
        f.remove_card(m);
        f.add_card(0, m, location::GRAVE, 0, false);
        let s = spell(&mut f, 0, card_type::SPELL);
        f.equip(0, s, m, true, false);
        run(&mut f);
        assert_eq!(f.cards[s].current.location, location::GRAVE);
        assert_eq!(f.core.returns.get(), 0);
    }

    /// **A successful equip reports success and links both ways.**
    #[test]
    fn a_successful_equip_links_both_directions() {
        let mut f = field();
        let m = monster(&mut f, 0);
        let s = spell(&mut f, 0, card_type::SPELL);
        f.core.returns.set(-99);
        f.equip(0, s, m, true, false);
        run(&mut f);
        assert_eq!(f.core.returns.get(), 1, "success");
        assert_eq!(f.cards[s].equiping_target, Some(m));
        assert!(f.cards[m].equiping_cards.contains(&s));
        assert!(f.cards[s].effect_target_cards.contains(&m));
        assert!(f.cards[m].effect_target_owner.contains(&s));
        assert_eq!(f.cards[s].current.location, location::SZONE);
    }

    /// **`is_step` stops before the window** and records the card for
    /// whoever is driving the larger operation.
    #[test]
    fn a_step_equip_defers_the_window() {
        let mut f = field();
        let m = monster(&mut f, 0);
        let s = spell(&mut f, 0, card_type::SPELL);
        f.equip(0, s, m, true, true);
        run(&mut f);
        assert_eq!(f.core.returns.get(), 1);
        assert!(f.core.equiping_cards.contains(&s));
        assert!(
            !f.messages
                .iter()
                .any(|m| matches!(m, Message::Hint { value: 0, .. })),
            "no window was opened here"
        );
        // The ordinary path does record the timing; this one does not.
        assert_eq!(f.core.hint_timing[0] & timing::EQUIP, 0);
    }

    /// And the ordinary path does open it, against the target's
    /// controller rather than the equipper's.
    #[test]
    fn the_window_belongs_to_the_targets_controller() {
        let mut f = field();
        let m = monster(&mut f, 1);
        let s = spell(&mut f, 0, card_type::SPELL);
        f.equip(0, s, m, true, false);
        run(&mut f);
        assert_ne!(f.core.hint_timing[1] & timing::EQUIP, 0, "player 1's");
        assert_eq!(f.core.hint_timing[0] & timing::EQUIP, 0, "not player 0's");
    }

    /// **Re-equipping detaches from the old target first.**
    #[test]
    fn equipping_again_moves_it_off_the_old_target() {
        let mut f = field();
        let first = monster(&mut f, 0);
        let second = monster(&mut f, 0);
        let s = spell(&mut f, 0, card_type::SPELL);
        f.equip(0, s, first, true, false);
        run(&mut f);
        assert_eq!(f.cards[s].equiping_target, Some(first));

        f.equip(0, s, second, true, false);
        run(&mut f);
        assert_eq!(f.cards[s].equiping_target, Some(second), "moved");
        assert!(
            !f.cards[first].equiping_cards.contains(&s),
            "and the old target no longer holds it"
        );
        assert!(!f.cards[first].effect_target_owner.contains(&s));
    }
}

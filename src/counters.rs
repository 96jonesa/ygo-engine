//! Counters on a card.
//!
//! `card::add_counter`, `remove_counter`, `get_counter`,
//! `is_can_add_counter`. Small, and with three things in it that are easy to
//! get backwards.
//!
//! ## Counters come in two halves
//!
//! Every counter type is stored as `[permanent, temporary]`. A counter
//! placed on a card that needs no permission goes in the first; one placed
//! under a permitting effect goes in the second, and is lost when the card
//! is disabled (`card::reset` with `RESET_DISABLE` clears index 1 and leaves
//! index 0). `get_counter` adds them.
//!
//! `remove_counter` spends the **temporary** half first and only then eats
//! into the permanent one — so disabling a card and removing a counter take
//! from the same place, and a card whose permitted counters were already
//! spent loses real ones next.
//!
//! ## Two bits live in the counter *type*, not in a table
//!
//! `COUNTER_WITHOUT_PERMIT` (`0x1000`) and `COUNTER_NEED_ENABLE` (`0x2000`)
//! are flags packed into the type word a caller passes, so one argument
//! carries both "which counter" and "how it is governed".
//!
//! The stored key is `countertype & ~COUNTER_NEED_ENABLE` — and note which
//! flag that masks. `NEED_ENABLE` is removed, so the same counter placed
//! with and without it shares a slot. **`WITHOUT_PERMIT` is kept**, so the
//! same counter placed with and without *that* lives in two separate slots,
//! with separate limits, queried by separate keys.
//!
//! That asymmetry is easy to miss and easy to "fix" into masking both. It
//! also means a caller must be consistent: `get_counter(SPELL_COUNTER)` does
//! not see counters placed as `SPELL_COUNTER | WITHOUT_PERMIT`. The limit
//! lookup uses the same key, so the two slots are limited independently.
//!
//! ## `EFFECT_COUNTER_PERMIT` and `_LIMIT` are code *ranges*
//!
//! They are `0x10000` and `0x20000`, and the counter type is **added** to
//! them to make the effect code looked up. That is why `is_can_be_forbidden`
//! masks a code with `0xf0000` before comparing — a permit effect's code is
//! not one number but sixty-five thousand of them.

use crate::board::{location, position};
use crate::card::status;
use crate::event::{code, CardId};
use crate::field::{Field, Message};

/// The counter types the script library names
/// (`card_counter_constants.lua`), transcribed whole.
///
/// Eleven, of which this pool uses one. The rest are here because a table
/// taken in pieces invites inventing the rest — and because several carry
/// [`counter::WITHOUT_PERMIT`] (`0x1000`) in the type word, which is the
/// kind of detail that is wrong-by-omission if a card writes its own.
///
/// Pinned against the library by `tools/check_constants.py`.
pub mod counter_type {
    pub const A: u16 = 0x100e;
    pub const BUSHIDO: u16 = 0x3;
    pub const EC: u16 = 0x217;
    pub const FEATHER: u16 = 0x10;
    pub const FOG: u16 = 0x1019;
    pub const KAIJU: u16 = 0x37;
    pub const PREDATOR: u16 = 0x1041;
    pub const RESONANCE: u16 = 0x211;
    pub const SIGNAL: u16 = 0x1148;
    pub const SPELL: u16 = 0x1;
    pub const VENOM: u16 = 0x1009;
}

/// Flags packed into a counter type word.
pub mod counter {
    /// This counter needs no permitting effect. Its counters go in the
    /// permanent half.
    pub const WITHOUT_PERMIT: u16 = 0x1000;
    /// This counter is lost when the card is disabled.
    pub const NEED_ENABLE: u16 = 0x2000;
}

impl Field {
    /// `card::get_counter` — both halves together.
    pub fn get_counter(&self, card: CardId, counter_type: u16) -> u16 {
        self.cards[card]
            .counters
            .get(&counter_type)
            .map_or(0, |c| c[0] + c[1])
    }

    /// `card::is_can_add_counter`.
    ///
    /// `loc` non-zero means "pretend the card is there" — the question asked
    /// before a card has arrived somewhere, where the on-field and face-up
    /// tests would otherwise refuse it.
    pub fn is_can_add_counter(
        &mut self,
        card: CardId,
        playerid: u8,
        counter_type: u16,
        count: u16,
        singly: bool,
        loc: u16,
    ) -> bool {
        if count > 0 {
            if !self.is_player_can_place_counter(playerid, card, counter_type, count) {
                return false;
            }
            if loc == 0
                && (!self.cards[card]
                    .current
                    .is_location(u16::from(location::ONFIELD))
                    || !self.cards[card].current.is_faceup())
            {
                return false;
            }
            if counter_type & counter::NEED_ENABLE != 0
                && self.cards[card].is_status(status::DISABLED)
            {
                return false;
            }
        }

        // A counter that needs no permission skips the permit search
        // entirely — `check` starts true rather than being proven.
        let mut check = counter_type & counter::WITHOUT_PERMIT != 0;
        if !check {
            // The reference masks with `0xffff` here, which is a no-op on
            // a `uint16_t` — kept as a comment rather than as dead code.
            let permit_code = code::COUNTER_PERMIT + u32::from(counter_type);
            for id in self.filter_effect(card, permit_code) {
                let Some(e) = self.effects.get(id) else {
                    continue;
                };
                let range = e.value as u16;
                let filter = e.target_filter;
                if loc != 0 {
                    check = loc & range != 0;
                } else if self.cards[card]
                    .current
                    .is_location(u16::from(location::ONFIELD))
                {
                    let passes = filter.is_none_or(|f| f(self, id, Some(card), &[]));
                    check = self.cards[card].current.is_location(range)
                        && self.cards[card].current.is_faceup()
                        && passes;
                } else {
                    // Off the field the permit's range is not consulted at
                    // all: the counter is allowed.
                    check = true;
                }
                if check {
                    break;
                }
            }
        }
        if !check {
            return false;
        }

        let stored = counter_type & !counter::NEED_ENABLE;
        let current = self.get_counter(card, stored);
        let limit = self.counter_limit(card, stored);
        // `singly` asks about placing exactly one, whatever the caller said.
        let count = if singly { 1 } else { count };
        !matches!(limit, Some(limit)
            if u32::from(current) + u32::from(count) > u32::from(limit))
    }

    /// The lowest limit any effect places on this counter type, if any.
    fn counter_limit(&mut self, card: CardId, stored_type: u16) -> Option<u16> {
        let limit_code = code::COUNTER_LIMIT + u32::from(stored_type);
        let mut limit: Option<u16> = None;
        for id in self.filter_effect(card, limit_code) {
            let v = self.effect_value_for_card_pub(id, card) as u16;
            limit = Some(limit.map_or(v, |l| l.min(v)));
        }
        limit
    }

    /// `card::add_counter`.
    pub fn add_counter(
        &mut self,
        card: CardId,
        playerid: u8,
        counter_type: u16,
        count: u16,
        singly: bool,
    ) -> bool {
        if !self.is_can_add_counter(card, playerid, counter_type, count, singly, 0) {
            return false;
        }
        let stored = counter_type & !counter::NEED_ENABLE;
        self.cards[card].counters.entry(stored).or_insert([0, 0]);

        // `singly` here means something different from `singly` in the
        // permission check: there it asked about one counter, here it caps
        // the batch at whatever room is left under the limit.
        let mut placed = count;
        if singly {
            if let Some(limit) = self.counter_limit(card, stored) {
                let room = limit.saturating_sub(self.get_counter(card, stored));
                placed = placed.min(room);
            }
        }

        // Which half it goes in: permanent only when it needs no permit
        // *and* does not need the card enabled.
        let half = if counter_type & counter::WITHOUT_PERMIT != 0
            && counter_type & counter::NEED_ENABLE == 0
        {
            0
        } else {
            1
        };
        if let Some(c) = self.cards[card].counters.get_mut(&stored) {
            c[half] += placed;
        }

        let c = &self.cards[card];
        self.messages.push(Message::AddCounter {
            counter_type: stored,
            controller: c.current.controller,
            location: c.current.location,
            sequence: c.current.sequence,
            count: placed,
        });
        // The event carries the *unmasked* type, so a card watching for a
        // particular counter sees the flags the placer used.
        self.raise_single_event(
            card,
            vec![],
            code::ADD_COUNTER + u32::from(counter_type),
            self.core.reason_effect,
            crate::card::reason::EFFECT,
            playerid,
            playerid,
            u32::from(placed),
        );
        self.process_single_event();
        true
    }

    /// `card::remove_counter` — the temporary half first.
    pub fn remove_counter(&mut self, card: CardId, counter_type: u16, count: u16) -> bool {
        let Some(&existing) = self.cards[card].counters.get(&counter_type) else {
            return false;
        };
        if existing[1] <= count {
            let remains = count - existing[1];
            if existing[0] <= remains {
                self.cards[card].counters.remove(&counter_type);
            } else if let Some(c) = self.cards[card].counters.get_mut(&counter_type) {
                c[1] = 0;
                c[0] -= remains;
            }
        } else if let Some(c) = self.cards[card].counters.get_mut(&counter_type) {
            c[1] -= count;
        }

        let c = &self.cards[card];
        self.messages.push(Message::RemoveCounter {
            counter_type,
            controller: c.current.controller,
            location: c.current.location,
            sequence: c.current.sequence,
            count,
        });
        true
    }

    /// `field::is_player_can_place_counter` — the same prohibition shape as
    /// the capability predicates.
    pub fn is_player_can_place_counter(
        &mut self,
        playerid: u8,
        card: CardId,
        counter_type: u16,
        count: u16,
    ) -> bool {
        let _ = (counter_type, count);
        for id in self.filter_player_effect(playerid, code::CANNOT_PLACE_COUNTER) {
            let Some(e) = self.effects.get(id) else {
                continue;
            };
            let Some(target) = e.target_filter else {
                return false;
            };
            if target(self, id, Some(card), &[]) {
                return false;
            }
        }
        true
    }

    /// Whether a card is in a position to keep counters at all — the test
    /// `is_can_add_counter` makes when no location is supplied.
    pub fn can_hold_counters(&self, card: CardId) -> bool {
        self.cards[card]
            .current
            .is_location(u16::from(location::ONFIELD))
            && self.cards[card].current.position & position::FACEUP != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{card_type, Card, CardData};
    use crate::effect::{effect_type, Effect};
    use crate::event::EffectId;

    const SPELL_COUNTER: u16 = 0x1;

    fn on_field(f: &mut Field) -> CardId {
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
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        let seat = f.players[0]
            .mzone
            .iter()
            .position(Option::is_none)
            .expect("a free seat") as u32;
        f.add_card(0, id, location::MZONE, seat, false);
        id
    }

    fn permit(f: &mut Field, card: CardId, counter_type: u16, range: u16) -> EffectId {
        let mut e = Effect::new(
            effect_type::SINGLE,
            code::COUNTER_PERMIT + u32::from(counter_type),
        );
        e.owner = Some(card);
        e.handler = Some(card);
        e.value = i64::from(range);
        let id = f.new_effect(e);
        f.cards[card]
            .single_effect
            .insert(code::COUNTER_PERMIT + u32::from(counter_type), id);
        f.cards[card].indexer.insert(id);
        id
    }

    /// Without a permitting effect, a counter that needs one cannot go on.
    #[test]
    fn a_counter_needs_permission_unless_it_says_otherwise() {
        let mut f = Field::new(8000);
        let c = on_field(&mut f);
        assert!(!f.add_counter(c, 0, SPELL_COUNTER, 1, false));

        permit(&mut f, c, SPELL_COUNTER, u16::from(location::MZONE));
        assert!(f.add_counter(c, 0, SPELL_COUNTER, 1, false));
        assert_eq!(f.get_counter(c, SPELL_COUNTER), 1);

        // Or it can say otherwise.
        let d = on_field(&mut f);
        assert!(f.add_counter(d, 0, SPELL_COUNTER | counter::WITHOUT_PERMIT, 1, false));
    }

    /// A permitted counter is temporary and is lost when the card is
    /// disabled; a `WITHOUT_PERMIT` one is permanent and survives.
    #[test]
    fn the_two_halves_are_kept_apart() {
        use crate::field::reset;
        const FREE: u16 = SPELL_COUNTER | counter::WITHOUT_PERMIT;

        let mut f = Field::new(8000);
        let c = on_field(&mut f);
        permit(&mut f, c, SPELL_COUNTER, u16::from(location::MZONE));
        f.add_counter(c, 0, SPELL_COUNTER, 2, false);
        f.add_counter(c, 0, FREE, 3, false);

        // Two slots, because the stored key keeps WITHOUT_PERMIT.
        assert_eq!(f.cards[c].counters.get(&SPELL_COUNTER), Some(&[0, 2]));
        assert_eq!(f.cards[c].counters.get(&FREE), Some(&[3, 0]));

        f.reset_card(c, reset::DISABLE, reset::EVENT);
        assert_eq!(
            f.get_counter(c, SPELL_COUNTER),
            0,
            "the permitted ones are lost"
        );
        assert_eq!(f.get_counter(c, FREE), 3, "the free ones survive");
    }

    /// The mask removes `NEED_ENABLE` and **keeps** `WITHOUT_PERMIT`, so the
    /// first pair share a slot and the second pair do not. Masking both —
    /// the natural tidy-up — merges two slots that the reference keeps
    /// apart, each with its own limit.
    #[test]
    fn only_need_enable_is_masked_out_of_the_key() {
        const FREE: u16 = SPELL_COUNTER | counter::WITHOUT_PERMIT;

        let mut f = Field::new(8000);
        let c = on_field(&mut f);
        f.add_counter(c, 0, FREE, 1, false);
        f.add_counter(c, 0, FREE | counter::NEED_ENABLE, 1, false);
        assert_eq!(f.cards[c].counters.len(), 1, "NEED_ENABLE is masked out");
        assert_eq!(f.get_counter(c, FREE), 2);
        assert_eq!(
            f.get_counter(c, SPELL_COUNTER),
            0,
            "and the un-flagged key sees none of them"
        );

        // The other flag makes a genuinely separate slot.
        let d = on_field(&mut f);
        permit(&mut f, d, SPELL_COUNTER, u16::from(location::MZONE));
        f.add_counter(d, 0, SPELL_COUNTER, 1, false);
        f.add_counter(d, 0, FREE, 1, false);
        assert_eq!(f.cards[d].counters.len(), 2, "WITHOUT_PERMIT is not");
    }

    /// A counter needing the card enabled cannot go on a disabled card.
    #[test]
    fn a_disabled_card_refuses_a_counter_that_needs_enabling() {
        let mut f = Field::new(8000);
        let c = on_field(&mut f);
        f.cards[c].set_status(status::DISABLED, true);
        assert!(!f.add_counter(
            c,
            0,
            SPELL_COUNTER | counter::WITHOUT_PERMIT | counter::NEED_ENABLE,
            1,
            false
        ));
        assert!(
            f.add_counter(c, 0, SPELL_COUNTER | counter::WITHOUT_PERMIT, 1, false),
            "but one that does not need it is fine"
        );
    }

    /// Removing spends the temporary half first, then eats into the
    /// permanent one.
    #[test]
    fn removing_spends_the_temporary_half_first() {
        let mut f = Field::new(8000);
        let c = on_field(&mut f);
        // Both halves of *one* slot: a permitted counter and, by hand, some
        // permanent ones under the same key.
        permit(&mut f, c, SPELL_COUNTER, u16::from(location::MZONE));
        f.add_counter(c, 0, SPELL_COUNTER, 2, false);
        f.cards[c].counters.get_mut(&SPELL_COUNTER).unwrap()[0] = 3;

        f.remove_counter(c, SPELL_COUNTER, 1);
        assert_eq!(
            f.cards[c].counters.get(&SPELL_COUNTER),
            Some(&[3, 1]),
            "the temporary half went first"
        );

        f.remove_counter(c, SPELL_COUNTER, 2);
        assert_eq!(
            f.cards[c].counters.get(&SPELL_COUNTER),
            Some(&[2, 0]),
            "then it ate into the permanent one"
        );

        f.remove_counter(c, SPELL_COUNTER, 5);
        assert_eq!(
            f.cards[c].counters.get(&SPELL_COUNTER),
            None,
            "and an emptied slot is dropped"
        );
    }

    /// Removing a counter type the card does not have is a no-op that
    /// answers false.
    #[test]
    fn removing_an_absent_counter_answers_false() {
        let mut f = Field::new(8000);
        let c = on_field(&mut f);
        assert!(!f.remove_counter(c, SPELL_COUNTER, 1));
    }

    /// A limit effect caps the total, and the lowest of several wins.
    #[test]
    fn a_limit_caps_the_total() {
        // Registered under the *stored* key, which keeps WITHOUT_PERMIT.
        const FREE: u16 = SPELL_COUNTER | counter::WITHOUT_PERMIT;
        let mut f = Field::new(8000);
        let c = on_field(&mut f);
        for cap in [5i64, 3] {
            let mut e = Effect::new(effect_type::SINGLE, code::COUNTER_LIMIT + u32::from(FREE));
            e.owner = Some(c);
            e.handler = Some(c);
            e.value = cap;
            let id = f.new_effect(e);
            f.cards[c]
                .single_effect
                .insert(code::COUNTER_LIMIT + u32::from(FREE), id);
            f.cards[c].indexer.insert(id);
        }

        assert!(f.add_counter(c, 0, FREE, 3, false));
        assert!(
            !f.add_counter(c, 0, FREE, 1, false),
            "the lower of the two limits binds"
        );
    }

    /// `singly` caps a batch at the room left rather than refusing it.
    #[test]
    fn singly_fills_the_remaining_room() {
        const FREE: u16 = SPELL_COUNTER | counter::WITHOUT_PERMIT;
        let mut f = Field::new(8000);
        let c = on_field(&mut f);
        let mut e = Effect::new(effect_type::SINGLE, code::COUNTER_LIMIT + u32::from(FREE));
        e.owner = Some(c);
        e.handler = Some(c);
        e.value = 3;
        let id = f.new_effect(e);
        f.cards[c]
            .single_effect
            .insert(code::COUNTER_LIMIT + u32::from(FREE), id);
        f.cards[c].indexer.insert(id);

        f.add_counter(c, 0, FREE, 2, false);
        assert!(f.add_counter(c, 0, FREE, 5, true));
        assert_eq!(
            f.get_counter(c, FREE),
            3,
            "capped at the limit rather than refused"
        );
    }

    /// A card off the field, or face-down, holds no counters.
    #[test]
    fn a_hidden_card_holds_no_counters() {
        let mut f = Field::new(8000);
        let c = on_field(&mut f);
        assert!(f.can_hold_counters(c));
        assert!(f.add_counter(c, 0, SPELL_COUNTER | counter::WITHOUT_PERMIT, 1, false));

        f.cards[c].current.position = position::FACEDOWN_DEFENSE;
        assert!(!f.can_hold_counters(c));
        assert!(!f.add_counter(c, 0, SPELL_COUNTER | counter::WITHOUT_PERMIT, 1, false));
    }

    /// Placing a counter raises an event carrying the **unmasked** type, so
    /// a card watching for a particular counter sees the flags the placer
    /// used.
    #[test]
    fn the_event_carries_the_unmasked_type() {
        let mut f = Field::new(8000);
        let c = on_field(&mut f);
        let placed = SPELL_COUNTER | counter::WITHOUT_PERMIT;
        f.add_counter(c, 0, placed, 1, false);
        let stored = placed & !counter::NEED_ENABLE;
        // The gather consumed it, so look at what moved on.
        assert!(
            f.core.instant_event.is_empty() && f.core.single_event.is_empty(),
            "the single event was processed"
        );
        assert!(
            f.messages
                .iter()
                .any(|m| matches!(m, Message::AddCounter { counter_type, .. }
                              if *counter_type == stored)),
            "but the message carries the stored key"
        );
    }
}

//! Leaving where you were: redirects, and the ties that must be cut.
//!
//! Six helpers that `SendTo` calls, gathered because they answer the same
//! question from different sides — *what has to happen before this card can
//! be somewhere else?* Two of them compute where it actually ends up, and
//! four sever a relationship that would otherwise outlive the move.
//!
//! In the reference these are `card` methods. Every one of them reaches
//! `pduel->game_field`, so here they are [`Field`] methods taking a
//! [`CardId`], the same shape `remove_card_effect` already has.
//!
//! ## The redirects return a location that is not a location
//!
//! `LOCATION_DECKBOT` is `0x10001` and `LOCATION_DECKSHF` is `0x20001` —
//! both of which *contain* `LOCATION_DECK` (`0x01`) in their low bit. They
//! are not zones. They are "the deck, and here is how to put it back", and
//! the extra bit is the instruction.
//!
//! Two consequences the reference relies on and a port can lose:
//!
//! - They do not fit in the `u8` a card's `location` field is, which is why
//!   these functions return `u32` and why the constants live in their own
//!   module rather than beside `location::DECK`.
//! - `redirects & LOCATION_DECKBOT` is **true for a plain deck redirect**,
//!   because they share that low bit. The reference therefore tests
//!   `(redirects & LOCATION_DECKBOT) == LOCATION_DECKBOT` — an equality,
//!   not a membership test. Written as the usual `!= 0` it reports "put it
//!   on the bottom" for every ordinary return-to-deck.
//!
//! ## The two redirects differ in more than which effect they read
//!
//! `leave_field_redirect` **accumulates** into `redirects` and then picks by
//! a fixed precedence — banish, then deck, then hand — so two effects that
//! both apply are resolved by rank rather than by order. `destination_redirect`
//! **returns the first** effect that passes, so there the order of
//! `filter_effect` decides. That asymmetry is the reference's, and the
//! comment above the precedence block ("the ruling ... can't be confirmed
//! for now") says it is a guess upstream too.

use crate::board::{location, redirect};
use crate::card::card_type;
use crate::effect::Ctx;
use crate::event::{code, CardId, EffectId, Event, PLAYER_NONE};
use crate::field::Field;

/// Where a card is, as the reference reports it to the host. `loc_info`.
///
/// Not the same thing as a [`crate::board::Loc`]: for an Xyz material the
/// four slots describe the *monster it is under*, with the material's own
/// sequence displaced into the `position` slot. See
/// [`Field::get_info_location`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LocInfo {
    pub controller: u8,
    pub location: u8,
    pub sequence: u32,
    pub position: u8,
}

impl Field {
    /// `card::get_info_location`.
    ///
    /// For an ordinary card this is just its `current`. For an Xyz material
    /// it is a different shape entirely, and the reuse of the same four
    /// slots is what makes it easy to mis-port:
    ///
    /// | slot | ordinary card | Xyz material |
    /// |---|---|---|
    /// | controller | its own | the monster's |
    /// | location | its own | the monster's, `+ OVERLAY` |
    /// | sequence | its own | **the monster's** |
    /// | position | its own | **the material's own sequence** |
    ///
    /// The material's sequence occupies the position slot because a material
    /// has no position, and the host needs to know which of the monster's
    /// materials this is.
    /// Open an `MSG_MOVE`. The reference allocates the message **before**
    /// the move and fills its second half after it; keeping the index does
    /// the same here, so the message sits in the stream where the
    /// reference's does even if the move itself writes others.
    pub(crate) fn open_move_message(&mut self, card: CardId) -> usize {
        let previous = self.get_info_location(card);
        self.messages.push(crate::field::Message::Move {
            code: self.cards[card].data.code,
            previous,
            current: LocInfo::default(),
            reason: 0,
        });
        self.messages.len() - 1
    }

    /// Fill the second half of an opened `MSG_MOVE`: where the card is now,
    /// and its current reason.
    pub(crate) fn close_move_message(&mut self, at: usize, card: CardId) {
        let now = self.get_info_location(card);
        let reason = self.cards[card].reason;
        if let Some(crate::field::Message::Move {
            current, reason: r, ..
        }) = self.messages.get_mut(at)
        {
            *current = now;
            *r = reason;
        }
    }

    pub fn get_info_location(&self, card: CardId) -> LocInfo {
        let c = &self.cards[card];
        match c.overlay_target {
            Some(t) => {
                let t = &self.cards[t];
                LocInfo {
                    controller: t.current.controller,
                    // The reference masks this with `0xff`. That mask is a
                    // no-op *here* and not there: its expression is computed
                    // at a wider type and narrowed into a `uint8_t` field,
                    // while both operands are already `u8` in this port.
                    location: t.current.location | location::OVERLAY,
                    sequence: t.current.sequence,
                    position: c.current.sequence as u8,
                }
            }
            None => LocInfo {
                controller: c.current.controller,
                location: c.current.location,
                sequence: c.current.sequence,
                position: c.current.position,
            },
        }
    }

    /// The body both redirects share: run one `EFFECT_*_REDIRECT` effect's
    /// value and decide whether the destination it names is permitted.
    ///
    /// The four arms are tried in the reference's order, and each pairs a
    /// destination bit with the prohibition that blocks it and the player
    /// check that can refuse it. `leave_field_redirect` omits the graveyard
    /// arm — a card leaving the field is already heading there by default,
    /// so there is nothing to redirect it *to*.
    fn redirect_value(
        &mut self,
        card: CardId,
        effect: EffectId,
        reason: u32,
        grave_arm: bool,
    ) -> u32 {
        let redirect = {
            let ev = Event::new(0);
            let ctx = Ctx {
                reason_effect: effect,
                player: self.cards[card].current.controller,
                event: &ev,
                card: Some(card),
                args: &[i64::from(reason)],
            };
            let Some(e) = self.effects.get(effect) else {
                return 0;
            };
            e.get_value(self, &ctx) as u32
        };
        let player = self
            .effects
            .get(effect)
            .map_or(PLAYER_NONE, |e| e.get_handler_player(&self.cards));

        if redirect & u32::from(location::HAND) != 0
            && self
                .is_affected_by_effect(card, code::CANNOT_TO_HAND)
                .is_none()
            && self.is_player_can_send_to_hand(player, card)
        {
            return redirect;
        }
        if redirect & u32::from(location::DECK) != 0
            && self
                .is_affected_by_effect(card, code::CANNOT_TO_DECK)
                .is_none()
            && self.is_player_can_send_to_deck(player, card)
        {
            return redirect;
        }
        if redirect & u32::from(location::REMOVED) != 0
            && self
                .is_affected_by_effect(card, code::CANNOT_REMOVE)
                .is_none()
            && self.is_player_can_remove(player, card, crate::card::reason::EFFECT)
        {
            return redirect;
        }
        if grave_arm
            && redirect & u32::from(location::GRAVE) != 0
            && self
                .is_affected_by_effect(card, code::CANNOT_TO_GRAVE)
                .is_none()
            && self.is_player_can_send_to_grave(player, card, crate::card::reason::EFFECT)
        {
            return redirect;
        }
        0
    }

    /// `card::leave_field_redirect` — where does this card go instead of the
    /// graveyard, now that it is leaving the field?
    ///
    /// Tokens never redirect: they cease to exist, so there is nowhere to
    /// send them.
    ///
    /// Note the two structural differences from [`Field::destination_redirect`],
    /// both deliberate: the results are **accumulated** across every
    /// applicable effect and then ranked, and the *else-if* chain inside the
    /// loop means one effect contributes at most one destination — the first
    /// of hand/deck/banish that it names and that is permitted.
    pub fn leave_field_redirect(&mut self, card: CardId, reason: u32) -> u32 {
        if self.cards[card].data.is_type(card_type::TOKEN) {
            return 0;
        }
        let mut redirects = 0u32;
        for effect in self.filter_effect(card, code::LEAVE_FIELD_REDIRECT) {
            redirects |= self.redirect_value(card, effect, reason, false);
        }
        if redirects & u32::from(location::REMOVED) != 0 {
            return u32::from(location::REMOVED);
        }
        // The ruling for the priority of the following redirects can't be
        // confirmed for now — the reference says so, and this is its guess.
        if redirects & u32::from(location::DECK) != 0 {
            // Equality, not membership: both of these contain LOCATION_DECK.
            if (redirects & redirect::DECKBOT) == redirect::DECKBOT {
                return redirect::DECKBOT;
            }
            if (redirects & redirect::DECKSHF) == redirect::DECKSHF {
                return redirect::DECKSHF;
            }
            return u32::from(location::DECK);
        }
        if redirects & u32::from(location::HAND) != 0 {
            return u32::from(location::HAND);
        }
        0
    }

    /// `card::destination_redirect` — this card is being sent *there*; does
    /// something send it somewhere else instead?
    ///
    /// One effect per destination, and an unrecognised destination redirects
    /// nowhere. Unlike [`Field::leave_field_redirect`] this returns the first
    /// effect that passes rather than ranking them, and it has the graveyard
    /// arm the other lacks.
    pub fn destination_redirect(&mut self, card: CardId, destination: u8, reason: u32) -> u32 {
        if self.cards[card].data.is_type(card_type::TOKEN) {
            return 0;
        }
        let which = match destination {
            location::HAND => code::TO_HAND_REDIRECT,
            location::DECK => code::TO_DECK_REDIRECT,
            location::GRAVE => code::TO_GRAVE_REDIRECT,
            location::REMOVED => code::REMOVE_REDIRECT,
            _ => return 0,
        };
        for effect in self.filter_effect(card, which) {
            let r = self.redirect_value(card, effect, reason, true);
            if r != 0 {
                return r;
            }
        }
        0
    }

    /// `card::unequip` — break the equip relationship from the equip card's
    /// side.
    ///
    /// The target is remembered in `pre_equip_target` rather than simply
    /// dropped. That is not bookkeeping: an equip card that is destroyed
    /// still has effects that ask what it *was* equipped to, and by the time
    /// they run the live pointer is gone.
    ///
    /// The disable check is queued against the **target**, not against the
    /// card being unequipped — the target is what was being disabled by the
    /// equip's effects, so the target is what has to be re-evaluated.
    pub fn unequip(&mut self, card: CardId) {
        let Some(target) = self.cards[card].equiping_target else {
            return;
        };
        let related: Vec<EffectId> = self.cards[card]
            .equip_effect
            .iter()
            .map(|&(_, id)| id)
            .collect();
        if related.iter().any(|&id| self.is_disable_related(id)) {
            self.add_to_disable_check_list(target);
        }
        self.cards[target].equiping_cards.retain(|&c| c != card);
        self.cards[card].pre_equip_target = Some(target);
        self.cards[card].equiping_target = None;
    }

    /// `card::xyz_remove` — take one material out from under an Xyz monster.
    ///
    /// `target` is the monster; `mat` the material. The guard is the
    /// reference's: a card that is not under *this* monster is not removed
    /// from it, which matters because the caller often has a set of cards
    /// and only some of them are materials.
    ///
    /// Three things happen that a shorter version would miss:
    ///
    /// - The material's `current` is stashed in `previous` and then cleared
    ///   to nowhere (`PLAYER_NONE`, location 0). It is not yet anywhere
    ///   else — `SendTo` puts it somewhere afterwards — and a great many
    ///   rules read `previous` in exactly that window.
    /// - The remaining materials are **resequenced**, because a material's
    ///   sequence is its index in the stack and the erase left a hole.
    /// - The material's *field*-type `xmaterial_effect`s are removed from the
    ///   field. Only the field ones: the single ones travel with the card.
    pub fn xyz_remove(&mut self, target: CardId, mat: CardId) {
        if self.cards[mat].overlay_target != Some(target) {
            return;
        }
        let seq = self.cards[mat].current.sequence as usize;
        self.cards[target].xyz_materials.remove(seq);
        if !self.core.current_chain.is_empty() {
            self.core.just_sent_cards.insert(mat);
        }

        let c = &mut self.cards[mat];
        c.previous.controller = c.current.controller;
        c.previous.location = c.current.location;
        c.previous.sequence = c.current.sequence;
        c.previous.pzone = c.current.pzone;
        c.current.controller = PLAYER_NONE;
        c.current.location = 0;
        c.current.sequence = 0;
        c.pre_overlay_target = c.overlay_target.take();

        let rest = self.cards[target].xyz_materials.clone();
        for (i, &m) in rest.iter().enumerate() {
            self.cards[m].current.sequence = i as u32;
        }

        let xmat: Vec<EffectId> = self.cards[mat]
            .xmaterial_effect
            .iter()
            .map(|&(_, id)| id)
            .collect();
        for id in xmat {
            if self
                .effects
                .get(id)
                .is_some_and(|e| e.effect_type & crate::effect::effect_type::FIELD != 0)
            {
                self.remove_effect(id);
            }
        }
    }

    /// `card::add_card_target` — record that this card's effects target
    /// `other`.
    ///
    /// Two directions and a message. The disable-check entry goes on
    /// **`other`**, the card on the receiving end, because that is whose
    /// status a targeting effect can change — the same asymmetry
    /// [`Self::clear_card_target`] undoes.
    pub fn add_card_target(&mut self, card: CardId, other: CardId) {
        if !self.cards[card].effect_target_cards.contains(&other) {
            self.cards[card].effect_target_cards.push(other);
        }
        if !self.cards[other].effect_target_owner.contains(&card) {
            self.cards[other].effect_target_owner.push(card);
        }
        let targeting: Vec<EffectId> = self.cards[card]
            .target_effect
            .iter()
            .map(|&(_, id)| id)
            .collect();
        if targeting.iter().any(|&id| self.is_disable_related(id)) {
            self.add_to_disable_check_list(other);
        }
        self.messages.push(crate::field::Message::CardTarget {
            owner: self.get_info_location(card),
            target: self.get_info_location(other),
        });
    }

    /// `card::clear_card_target` — cut every effect-target tie this card has,
    /// in both directions.
    ///
    /// A card can be on either end of a targeting relationship and the two
    /// ends are stored separately, so this walks both:
    ///
    /// - `effect_target_owner`: cards whose effects target *this* one. Drop
    ///   this card from their lists.
    /// - `effect_target_cards`: cards *this* one's effects target. Drop this
    ///   card from their owner lists — and additionally remove from each of
    ///   them any single effect this card owns with `EFFECT_FLAG_OWNER_RELATE`,
    ///   because such an effect only exists for as long as the relationship
    ///   does.
    ///
    /// The two disable checks are queued against different cards, which is
    /// the detail to get right: for the first loop the card whose status may
    /// change is **this** one, for the second it is the **other**. In both
    /// cases it is the card that was on the receiving end of the effect.
    pub fn clear_card_target(&mut self, card: CardId) {
        let owners = std::mem::take(&mut self.cards[card].effect_target_owner);
        for owner in owners {
            self.cards[owner].effect_target_cards.retain(|&c| c != card);
            let tgt: Vec<EffectId> = self.cards[owner]
                .target_effect
                .iter()
                .map(|&(_, id)| id)
                .collect();
            if tgt.iter().any(|&id| self.is_disable_related(id)) {
                self.add_to_disable_check_list(card);
            }
        }

        let targets = std::mem::take(&mut self.cards[card].effect_target_cards);
        let own_target: Vec<EffectId> = self.cards[card]
            .target_effect
            .iter()
            .map(|&(_, id)| id)
            .collect();
        let own_related = own_target.iter().any(|&id| self.is_disable_related(id));
        for target in targets {
            self.cards[target]
                .effect_target_owner
                .retain(|&c| c != card);
            if own_related {
                self.add_to_disable_check_list(target);
            }
            let singles: Vec<EffectId> = self.cards[target]
                .single_effect
                .iter()
                .map(|&(_, id)| id)
                .collect();
            for id in singles {
                let owned_and_relate = self.effects.get(id).is_some_and(|e| {
                    e.owner == Some(card) && e.is_flag(crate::effect::flag::OWNER_RELATE)
                });
                if owned_and_relate {
                    self.remove_card_effect(target, id);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::{position, Loc};
    use crate::card::{status, Card, CardData};
    use crate::effect::{effect_type, flag, Effect};

    fn card_in(f: &mut Field, loc: u8, type_: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 18036057,
                type_,
                ..Default::default()
            },
            0,
        );
        c.current.controller = 0;
        c.current.position = position::FACEUP_ATTACK;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        // Each caller gets its own seat: `add_card` refuses a taken one
        // *silently*, and a card left in nowhere answers stat questions with
        // its printed values rather than failing.
        let seat = f.cards.len() as u32 - 1;
        f.add_card(0, id, loc, seat, false);
        id
    }

    /// A redirect printed on the card itself, naming `value` as its
    /// destination.
    fn redirect_on(f: &mut Field, card: CardId, code_: u32, value: i64) -> EffectId {
        let mut e = Effect::new(effect_type::SINGLE, code_);
        e.owner = Some(card);
        e.handler = Some(card);
        e.effect_owner = 0;
        e.value = value;
        let id = f.new_effect(e);
        f.cards[card].single_effect.insert(code_, id);
        f.cards[card].indexer.insert(id);
        id
    }

    mod get_info_location {
        use super::*;

        #[test]
        fn an_ordinary_card_reports_its_own_place() {
            let mut f = Field::new(8000);
            let c = card_in(&mut f, location::MZONE, crate::card::card_type::MONSTER);
            let info = f.get_info_location(c);
            assert_eq!(info.controller, 0);
            assert_eq!(info.location, location::MZONE);
            assert_eq!(info.position, position::FACEUP_ATTACK);
        }

        /// The slot reuse that is easy to mis-port: a material reports the
        /// *monster's* sequence, and puts its own into the position slot.
        #[test]
        fn a_material_reports_the_monster_and_displaces_its_sequence() {
            let mut f = Field::new(8000);
            let xyz = card_in(&mut f, location::MZONE, crate::card::card_type::MONSTER);
            let mat = card_in(&mut f, location::GRAVE, crate::card::card_type::MONSTER);
            f.cards[xyz].current.controller = 1;
            f.cards[xyz].current.sequence = 3;
            f.cards[mat].overlay_target = Some(xyz);
            f.cards[mat].current.sequence = 2;

            let info = f.get_info_location(mat);
            assert_eq!(info.controller, 1, "the monster's controller");
            assert_eq!(
                info.location,
                location::MZONE | location::OVERLAY,
                "the monster's location, flagged as an overlay"
            );
            assert_eq!(
                info.sequence, 3,
                "the monster's sequence, not the material's"
            );
            assert_eq!(info.position, 2, "the material's own sequence lives here");
        }
    }

    mod leave_field_redirect {
        use super::*;

        #[test]
        fn a_token_never_redirects() {
            let mut f = Field::new(8000);
            let c = card_in(
                &mut f,
                location::MZONE,
                crate::card::card_type::MONSTER | crate::card::card_type::TOKEN,
            );
            redirect_on(
                &mut f,
                c,
                code::LEAVE_FIELD_REDIRECT,
                i64::from(location::HAND),
            );
            assert_eq!(f.leave_field_redirect(c, 0), 0, "tokens cease to exist");
        }

        #[test]
        fn nothing_applicable_redirects_nowhere() {
            let mut f = Field::new(8000);
            let c = card_in(&mut f, location::MZONE, crate::card::card_type::MONSTER);
            assert_eq!(f.leave_field_redirect(c, 0), 0);
        }

        #[test]
        fn a_hand_redirect_sends_it_to_the_hand() {
            let mut f = Field::new(8000);
            let c = card_in(&mut f, location::MZONE, crate::card::card_type::MONSTER);
            redirect_on(
                &mut f,
                c,
                code::LEAVE_FIELD_REDIRECT,
                i64::from(location::HAND),
            );
            assert_eq!(f.leave_field_redirect(c, 0), u32::from(location::HAND));
        }

        /// Banish outranks everything, whatever order the effects registered
        /// in. The accumulate-then-rank shape is what makes this true, and a
        /// first-match port would return whichever registered first.
        #[test]
        fn banish_outranks_hand_and_deck() {
            let mut f = Field::new(8000);
            let c = card_in(&mut f, location::MZONE, crate::card::card_type::MONSTER);
            redirect_on(
                &mut f,
                c,
                code::LEAVE_FIELD_REDIRECT,
                i64::from(location::HAND),
            );
            redirect_on(
                &mut f,
                c,
                code::LEAVE_FIELD_REDIRECT,
                i64::from(location::REMOVED),
            );
            assert_eq!(
                f.leave_field_redirect(c, 0),
                u32::from(location::REMOVED),
                "banish ranks above hand regardless of registration order"
            );
        }

        /// The bit-sharing trap, pinned. `DECKBOT` contains `DECK`, so a
        /// membership test would report "bottom of deck" for this plain
        /// return-to-deck.
        #[test]
        fn a_plain_deck_redirect_is_not_a_bottom_redirect() {
            let mut f = Field::new(8000);
            let c = card_in(&mut f, location::MZONE, crate::card::card_type::MONSTER);
            redirect_on(
                &mut f,
                c,
                code::LEAVE_FIELD_REDIRECT,
                i64::from(location::DECK),
            );
            assert_eq!(f.leave_field_redirect(c, 0), u32::from(location::DECK));
            assert_ne!(f.leave_field_redirect(c, 0), redirect::DECKBOT);
            assert_eq!(
                u32::from(location::DECK) & redirect::DECKBOT,
                u32::from(location::DECK),
                "they really do share the low bit — the equality test is load-bearing"
            );
        }

        #[test]
        fn a_bottom_redirect_survives_the_equality_test() {
            let mut f = Field::new(8000);
            let c = card_in(&mut f, location::MZONE, crate::card::card_type::MONSTER);
            redirect_on(
                &mut f,
                c,
                code::LEAVE_FIELD_REDIRECT,
                redirect::DECKBOT as i64,
            );
            assert_eq!(f.leave_field_redirect(c, 0), redirect::DECKBOT);
        }

        /// A prohibition on the destination makes the redirect not apply, so
        /// the card falls through to its default.
        #[test]
        fn a_prohibited_destination_does_not_redirect() {
            let mut f = Field::new(8000);
            let c = card_in(&mut f, location::MZONE, crate::card::card_type::MONSTER);
            redirect_on(
                &mut f,
                c,
                code::LEAVE_FIELD_REDIRECT,
                i64::from(location::HAND),
            );
            let mut e = Effect::new(effect_type::SINGLE, code::CANNOT_TO_HAND);
            e.owner = Some(c);
            e.handler = Some(c);
            let id = f.new_effect(e);
            f.cards[c].single_effect.insert(code::CANNOT_TO_HAND, id);
            f.cards[c].indexer.insert(id);

            assert_eq!(f.leave_field_redirect(c, 0), 0, "the hand is closed to it");
        }
    }

    mod destination_redirect {
        use super::*;

        #[test]
        fn an_unrecognised_destination_redirects_nowhere() {
            let mut f = Field::new(8000);
            let c = card_in(&mut f, location::MZONE, crate::card::card_type::MONSTER);
            redirect_on(&mut f, c, code::TO_HAND_REDIRECT, i64::from(location::DECK));
            assert_eq!(
                f.destination_redirect(c, location::MZONE, 0),
                0,
                "only the four pile destinations have a redirect effect"
            );
        }

        #[test]
        fn the_destination_picks_which_effect_is_read() {
            let mut f = Field::new(8000);
            let c = card_in(&mut f, location::MZONE, crate::card::card_type::MONSTER);
            redirect_on(&mut f, c, code::TO_HAND_REDIRECT, i64::from(location::DECK));
            assert_eq!(
                f.destination_redirect(c, location::HAND, 0),
                u32::from(location::DECK),
                "heading to the hand, the hand redirect applies"
            );
            assert_eq!(
                f.destination_redirect(c, location::GRAVE, 0),
                0,
                "heading to the graveyard, it does not"
            );
        }

        /// The arm `leave_field_redirect` does not have.
        #[test]
        fn the_graveyard_is_a_destination_here() {
            let mut f = Field::new(8000);
            let c = card_in(&mut f, location::MZONE, crate::card::card_type::MONSTER);
            redirect_on(&mut f, c, code::REMOVE_REDIRECT, i64::from(location::GRAVE));
            assert_eq!(
                f.destination_redirect(c, location::REMOVED, 0),
                u32::from(location::GRAVE)
            );
        }
    }

    mod unequip {
        use super::*;

        #[test]
        fn the_target_is_remembered_not_dropped() {
            let mut f = Field::new(8000);
            let equip = card_in(&mut f, location::SZONE, crate::card::card_type::SPELL);
            let target = card_in(&mut f, location::MZONE, crate::card::card_type::MONSTER);
            f.cards[equip].equiping_target = Some(target);
            f.cards[target].equiping_cards.push(equip);

            f.unequip(equip);
            assert_eq!(f.cards[equip].equiping_target, None);
            assert_eq!(
                f.cards[equip].pre_equip_target,
                Some(target),
                "effects that ask what it was equipped to still need an answer"
            );
            assert!(f.cards[target].equiping_cards.is_empty());
        }

        #[test]
        fn unequipping_nothing_is_a_no_op() {
            let mut f = Field::new(8000);
            let equip = card_in(&mut f, location::SZONE, crate::card::card_type::SPELL);
            f.unequip(equip);
            assert_eq!(f.cards[equip].pre_equip_target, None);
        }

        /// The disable check goes on the **target**: it is what the equip's
        /// effects were acting on.
        #[test]
        fn a_disable_related_equip_queues_the_target() {
            let mut f = Field::new(8000);
            let equip = card_in(&mut f, location::SZONE, crate::card::card_type::SPELL);
            let target = card_in(&mut f, location::MZONE, crate::card::card_type::MONSTER);
            f.cards[equip].equiping_target = Some(target);
            f.cards[target].equiping_cards.push(equip);

            let mut e = Effect::new(effect_type::EQUIP, code::DISABLE);
            e.owner = Some(equip);
            e.handler = Some(equip);
            let id = f.new_effect(e);
            f.cards[equip].equip_effect.insert(code::DISABLE, id);

            f.unequip(equip);
            assert!(
                f.field_effects.disable_check_set.contains(&target),
                "the target is what was being disabled"
            );
            assert!(!f.field_effects.disable_check_set.contains(&equip));
        }
    }

    mod xyz_remove {
        use super::*;

        fn stack(f: &mut Field, n: usize) -> (CardId, Vec<CardId>) {
            let xyz = card_in(f, location::MZONE, crate::card::card_type::MONSTER);
            let mut mats = Vec::new();
            for i in 0..n {
                let m = card_in(f, location::GRAVE, crate::card::card_type::MONSTER);
                f.cards[m].overlay_target = Some(xyz);
                f.cards[m].current = Loc {
                    controller: 0,
                    location: location::OVERLAY,
                    sequence: i as u32,
                    position: position::FACEUP_ATTACK,
                    pzone: false,
                };
                f.cards[xyz].xyz_materials.push(m);
                mats.push(m);
            }
            (xyz, mats)
        }

        #[test]
        fn a_card_under_a_different_monster_is_not_removed() {
            let mut f = Field::new(8000);
            let (xyz, mats) = stack(&mut f, 1);
            let other = card_in(&mut f, location::MZONE, crate::card::card_type::MONSTER);
            f.xyz_remove(other, mats[0]);
            assert_eq!(
                f.cards[xyz].xyz_materials.len(),
                1,
                "it is not under `other`, so `other` does not lose it"
            );
        }

        /// The material ends up nowhere, not somewhere else — and `previous`
        /// is what the rules read in that window.
        #[test]
        fn the_material_lands_nowhere_with_its_past_recorded() {
            let mut f = Field::new(8000);
            let (xyz, mats) = stack(&mut f, 1);
            f.xyz_remove(xyz, mats[0]);

            let m = &f.cards[mats[0]];
            assert_eq!(m.current.controller, PLAYER_NONE);
            assert_eq!(m.current.location, 0);
            assert_eq!(m.current.sequence, 0);
            assert_eq!(m.previous.location, location::OVERLAY);
            assert_eq!(m.pre_overlay_target, Some(xyz));
            assert_eq!(m.overlay_target, None);
        }

        /// The erase leaves a hole, and a material's sequence *is* its index.
        #[test]
        fn the_remaining_materials_are_resequenced() {
            let mut f = Field::new(8000);
            let (xyz, mats) = stack(&mut f, 3);
            f.xyz_remove(xyz, mats[0]);

            assert_eq!(f.cards[xyz].xyz_materials, vec![mats[1], mats[2]]);
            assert_eq!(f.cards[mats[1]].current.sequence, 0);
            assert_eq!(f.cards[mats[2]].current.sequence, 1);
        }

        /// Only while a chain is running: `just_sent_cards` exists so that
        /// cards that left during one chain are recognised as having left
        /// together.
        #[test]
        fn it_joins_just_sent_cards_only_during_a_chain() {
            let mut f = Field::new(8000);
            let (xyz, mats) = stack(&mut f, 1);
            f.xyz_remove(xyz, mats[0]);
            assert!(f.core.just_sent_cards.is_empty(), "no chain running");

            let mut g = Field::new(8000);
            let (xyz, mats) = stack(&mut g, 1);
            g.core
                .current_chain
                .push(crate::chain::Chain::new(0, Event::new(code::CHAINING)));
            g.xyz_remove(xyz, mats[0]);
            assert!(g.core.just_sent_cards.contains(&mats[0]));
        }

        /// Field-type material effects come off the field; single ones travel
        /// with the card.
        ///
        /// What is being observed is **deregistration**, not destruction.
        /// `field::remove_effect` takes the effect out of the field's index —
        /// the effect object itself survives, in the reference as here, which
        /// is why the assertion is about the aura and not about the arena.
        #[test]
        fn only_field_type_material_effects_are_removed() {
            let mut f = Field::new(8000);
            let (xyz, mats) = stack(&mut f, 1);
            let m = mats[0];

            let mut fe = Effect::new(effect_type::XMATERIAL | effect_type::FIELD, code::DISABLE);
            fe.owner = Some(m);
            fe.handler = Some(m);
            let field_id = f.new_effect(fe);
            f.cards[m].xmaterial_effect.insert(code::DISABLE, field_id);
            f.field_effects.indexer.insert(field_id);
            f.register_effect(field_id);

            let mut se = Effect::new(
                effect_type::XMATERIAL | effect_type::SINGLE,
                code::UPDATE_ATTACK,
            );
            se.owner = Some(m);
            se.handler = Some(m);
            let single_id = f.new_effect(se);
            f.cards[m]
                .xmaterial_effect
                .insert(code::UPDATE_ATTACK, single_id);
            f.field_effects.indexer.insert(single_id);

            f.xyz_remove(xyz, m);
            assert!(
                !f.field_effects.indexer.contains(&field_id),
                "the field one is deregistered"
            );
            assert!(
                f.field_effects.indexer.contains(&single_id),
                "the single one travels with the card"
            );
            assert!(
                f.effects.get(field_id).is_some(),
                "deregistered, not destroyed — the reference keeps the object too"
            );
        }
    }

    mod clear_card_target {
        use super::*;

        /// Both directions are stored separately, and both have to be cut.
        #[test]
        fn both_ends_of_the_relationship_are_cut() {
            let mut f = Field::new(8000);
            let a = card_in(&mut f, location::MZONE, crate::card::card_type::MONSTER);
            let b = card_in(&mut f, location::MZONE, crate::card::card_type::MONSTER);
            let c = card_in(&mut f, location::MZONE, crate::card::card_type::MONSTER);

            // `a` targets `c`; `b` targets `a`.
            f.cards[a].effect_target_cards.push(c);
            f.cards[c].effect_target_owner.push(a);
            f.cards[b].effect_target_cards.push(a);
            f.cards[a].effect_target_owner.push(b);

            f.clear_card_target(a);
            assert!(f.cards[a].effect_target_cards.is_empty());
            assert!(f.cards[a].effect_target_owner.is_empty());
            assert!(
                f.cards[c].effect_target_owner.is_empty(),
                "the card `a` targeted no longer names it"
            );
            assert!(
                f.cards[b].effect_target_cards.is_empty(),
                "the card that targeted `a` no longer names it"
            );
        }

        /// An owner-relate effect exists only for as long as the targeting
        /// relationship does.
        #[test]
        fn owner_relate_effects_on_the_target_are_removed() {
            let mut f = Field::new(8000);
            let a = card_in(&mut f, location::MZONE, crate::card::card_type::MONSTER);
            let t = card_in(&mut f, location::MZONE, crate::card::card_type::MONSTER);
            f.cards[a].effect_target_cards.push(t);
            f.cards[t].effect_target_owner.push(a);

            // Owned by `a`, sitting on `t`, flagged owner-relate.
            let mut e = Effect::new(effect_type::SINGLE, code::UPDATE_ATTACK);
            e.owner = Some(a);
            e.handler = Some(t);
            e.flag[0] |= flag::OWNER_RELATE;
            let related = f.new_effect(e);
            f.cards[t]
                .single_effect
                .insert(code::UPDATE_ATTACK, related);
            f.cards[t].indexer.insert(related);

            // Owned by `t` itself — not the relationship's, so it stays.
            let mut own = Effect::new(effect_type::SINGLE, code::UPDATE_DEFENSE);
            own.owner = Some(t);
            own.handler = Some(t);
            let own = f.new_effect(own);
            f.cards[t].single_effect.insert(code::UPDATE_DEFENSE, own);
            f.cards[t].indexer.insert(own);

            f.clear_card_target(a);
            assert!(
                f.cards[t]
                    .single_effect
                    .equal_range(code::UPDATE_ATTACK)
                    .is_empty(),
                "the relationship's effect goes with it"
            );
            assert!(
                !f.cards[t]
                    .single_effect
                    .equal_range(code::UPDATE_DEFENSE)
                    .is_empty(),
                "the card's own effect stays"
            );
        }

        /// An effect owned by `a` but *without* the flag survives: the flag is
        /// what ties it to the relationship, not the ownership.
        #[test]
        fn ownership_alone_does_not_remove_it() {
            let mut f = Field::new(8000);
            let a = card_in(&mut f, location::MZONE, crate::card::card_type::MONSTER);
            let t = card_in(&mut f, location::MZONE, crate::card::card_type::MONSTER);
            f.cards[a].effect_target_cards.push(t);
            f.cards[t].effect_target_owner.push(a);

            let mut e = Effect::new(effect_type::SINGLE, code::UPDATE_ATTACK);
            e.owner = Some(a);
            e.handler = Some(t);
            let id = f.new_effect(e);
            f.cards[t].single_effect.insert(code::UPDATE_ATTACK, id);
            f.cards[t].indexer.insert(id);

            f.clear_card_target(a);
            assert!(
                f.effects.get(id).is_some(),
                "owned by `a`, but not flagged owner-relate"
            );
        }
    }

    mod add_card_target {
        use super::*;
        use crate::board::location;

        /// Both directions are recorded, and the message names both
        /// seats.
        #[test]
        fn it_records_the_tie_both_ways_and_announces_it() {
            let mut f = Field::new(8000);
            let owner = card_in(&mut f, location::SZONE, crate::card::card_type::TRAP);
            let target = card_in(&mut f, location::MZONE, crate::card::card_type::MONSTER);
            f.messages.clear();
            f.add_card_target(owner, target);
            assert_eq!(f.cards[owner].effect_target_cards, vec![target]);
            assert_eq!(f.cards[target].effect_target_owner, vec![owner]);
            match f.messages.as_slice() {
                [crate::field::Message::CardTarget {
                    owner: o,
                    target: t,
                }] => {
                    assert_eq!(o.location, location::SZONE);
                    assert_eq!(t.location, location::MZONE);
                }
                other => panic!("expected one CardTarget: {other:?}"),
            }
            // Recording it twice changes nothing but writes the message
            // again, which is the reference's behaviour: the containers
            // are sets, the message stream is not.
            f.add_card_target(owner, target);
            assert_eq!(f.cards[owner].effect_target_cards, vec![target]);
            assert_eq!(f.messages.len(), 2);
        }

        /// **The disable check goes on the card being targeted**, not on
        /// the one doing the targeting — it is the target whose status a
        /// disable-related effect can change.
        #[test]
        fn a_disable_related_targeting_effect_queues_a_check_on_the_target() {
            let mut f = Field::new(8000);
            let owner = card_in(&mut f, location::SZONE, crate::card::card_type::TRAP);
            let target = card_in(&mut f, location::MZONE, crate::card::card_type::MONSTER);
            // Without a disable-related targeting effect: no check at all.
            f.add_card_target(owner, target);
            assert!(f.field_effects.disable_check_set.is_empty());

            let mut f = Field::new(8000);
            let owner = card_in(&mut f, location::SZONE, crate::card::card_type::TRAP);
            let target = card_in(&mut f, location::MZONE, crate::card::card_type::MONSTER);
            let mut e = Effect::new(effect_type::SINGLE, code::DISABLE);
            e.owner = Some(owner);
            e.handler = Some(owner);
            let id = f.new_effect(e);
            f.cards[owner].target_effect.insert(code::DISABLE, id);
            f.add_card_target(owner, target);
            assert!(
                f.field_effects.disable_check_set.contains(&target),
                "the target is checked"
            );
            assert!(
                !f.field_effects.disable_check_set.contains(&owner),
                "and the owner is not"
            );
        }
    }
}

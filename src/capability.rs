//! Can this card go there?
//!
//! The `is_capable_send_to_*` family and `is_removeable`. `SendTo`'s first
//! step asks one of these per card and drops the ones that answer no, so
//! they are the gate between "the effect said to move these" and "these
//! actually move".
//!
//! ## A prohibition with no target forbids everything
//!
//! Every one of these ends in a `field::is_player_can_*`, and those share a
//! shape worth stating once:
//!
//! ```text
//! for each EFFECT_CANNOT_X the player has:
//!     if the effect has no target  -> forbidden, unconditionally
//!     else run the target; if it says so -> forbidden
//! ```
//!
//! So a blanket "cards cannot be sent to the graveyard" is expressed as an
//! effect with **no target function at all**, and a conditional one by an
//! effect whose target picks out what it covers. Reading a missing target as
//! "no restriction" — the natural default — inverts the blanket case
//! exactly.

use crate::board::location;
use crate::card::{card_type, reason, status};
use crate::effect::Ctx;
use crate::event::{code, CardId, EffectId, Event};
use crate::field::Field;

impl Field {
    /// The shape every `is_player_can_*` shares.
    ///
    /// `args` are the integers the reference pushes after the effect and the
    /// card, which differ between members of the family — `is_player_can_remove`
    /// pushes a reason *and* the acting effect where the others push less.
    fn no_prohibition(
        &mut self,
        playerid: u8,
        prohibition: u32,
        card: CardId,
        args: &[i64],
    ) -> bool {
        for id in self.filter_player_effect(playerid, prohibition) {
            let Some(e) = self.effects.get(id) else {
                continue;
            };
            // No target: the prohibition is unconditional.
            let Some(target) = e.target_filter else {
                return false;
            };
            if target(self, id, Some(card), args) {
                return false;
            }
        }
        true
    }

    /// `field::is_player_can_discard_hand`.
    ///
    /// Two things this does **not** share with the rest of the family.
    /// It refuses outright for a card that is not in a hand — the others
    /// are asked about cards already in the right place. And the
    /// arguments it pushes are `(effect, card, peffect, reason)`, where
    /// `peffect` is the effect *asking*, not the prohibition; the
    /// no-prohibition helper pushes the prohibition itself first, so the
    /// acting effect rides in `args`.
    pub fn is_player_can_discard_hand(
        &mut self,
        playerid: u8,
        card: CardId,
        by: Option<EffectId>,
        reason: u32,
    ) -> bool {
        if self.cards[card].current.location != location::HAND {
            return false;
        }
        let acting = by.map_or(0i64, |e| e as i64);
        self.no_prohibition(
            playerid,
            code::CANNOT_DISCARD_HAND,
            card,
            &[acting, i64::from(reason)],
        )
    }

    /// `field::is_player_can_send_to_grave`.
    pub fn is_player_can_send_to_grave(&mut self, playerid: u8, card: CardId, reason: u32) -> bool {
        self.no_prohibition(playerid, code::CANNOT_TO_GRAVE, card, &[i64::from(reason)])
    }

    /// `field::is_player_can_send_to_hand`.
    pub fn is_player_can_send_to_hand(&mut self, playerid: u8, card: CardId) -> bool {
        self.no_prohibition(playerid, code::CANNOT_TO_HAND, card, &[])
    }

    /// `field::is_player_can_send_to_deck`.
    pub fn is_player_can_send_to_deck(&mut self, playerid: u8, card: CardId) -> bool {
        self.no_prohibition(playerid, code::CANNOT_TO_DECK, card, &[])
    }

    /// `field::is_player_can_remove`.
    pub fn is_player_can_remove(&mut self, playerid: u8, card: CardId, reason: u32) -> bool {
        self.no_prohibition(playerid, code::CANNOT_REMOVE, card, &[i64::from(reason)])
    }

    /// `card::is_capable_send_to_grave`.
    ///
    /// The shortest of the family: nothing about the card's own location
    /// matters, because anywhere can reach the graveyard.
    pub fn is_capable_send_to_grave(&mut self, card: CardId, playerid: u8, reason: u32) -> bool {
        if self
            .is_affected_by_effect(card, code::CANNOT_TO_GRAVE)
            .is_some()
        {
            return false;
        }
        self.is_player_can_send_to_grave(playerid, card, reason)
    }

    /// `card::is_capable_cost_to_grave` — could this card be sent to a
    /// graveyard **as a cost**?
    ///
    /// Not the same question as `is_capable_send_to_grave`, and the
    /// difference is the tail: the card's destination is worked out
    /// through the two redirects, and if anything would divert it
    /// somewhere other than a graveyard the answer is no. A cost has to
    /// land where it said it would.
    ///
    /// `sendto_param.location` is written and restored around the
    /// redirect calls, because those read it — the reference does the
    /// same, and it is the reason this cannot simply ask the redirects.
    pub fn is_capable_cost_to_grave(&mut self, card: CardId, playerid: u8) -> bool {
        if self.cards[card].data.is_type(card_type::TOKEN) {
            return false;
        }
        if self.cards[card].data.is_type(card_type::PENDULUM)
            && self.cards[card]
                .current
                .is_location(u16::from(location::ONFIELD))
            && self.is_capable_send_to_extra(card, playerid)
        {
            return false;
        }
        if self.cards[card].current.location == location::GRAVE {
            return false;
        }
        if self
            .is_affected_by_effect(card, code::CANNOT_USE_AS_COST)
            .is_some()
        {
            return false;
        }
        if self
            .is_affected_by_effect(card, code::CANNOT_TO_GRAVE_AS_COST)
            .is_some()
        {
            return false;
        }
        if !self.is_capable_send_to_grave(card, playerid, reason::COST) {
            return false;
        }
        let saved = self.cards[card].sendto_param;
        self.cards[card].sendto_param.location = location::GRAVE;
        let mut dest = location::GRAVE;
        if self.cards[card]
            .current
            .is_location(u16::from(location::ONFIELD))
        {
            let redirect = (self.leave_field_redirect(card, reason::COST) & 0xffff) as u8;
            if redirect != 0 {
                dest = redirect;
            }
        }
        let redirect = (self.destination_redirect(card, dest, reason::COST) & 0xffff) as u8;
        if redirect != 0 {
            dest = redirect;
        }
        self.cards[card].sendto_param = saved;
        dest == location::GRAVE
    }

    /// `card::is_removeable_as_cost`.
    ///
    /// The sibling of [`Field::is_capable_cost_to_grave`], and the same
    /// save-and-restore around the redirects — but with a twist that one
    /// does not have. The reference keeps a `redirchk` flag recording
    /// **whether any redirect fired at all**, and refuses a face-down
    /// banish when one did:
    ///
    /// ```text
    /// if(dest != LOCATION_REMOVED || (redirchk && (pos & POS_FACEDOWN)))
    /// ```
    ///
    /// A redirect that lands the card in the banished pile anyway still
    /// counts. Reading the tail as `dest != LOCATION_REMOVED` alone —
    /// which is what the grave sibling does — silently allows a face-down
    /// banish the reference refuses.
    pub fn is_removeable_as_cost(&mut self, card: CardId, playerid: u8, pos: u8) -> bool {
        if self.cards[card].current.location == location::REMOVED {
            return false;
        }
        if self
            .is_affected_by_effect(card, code::CANNOT_USE_AS_COST)
            .is_some()
        {
            return false;
        }
        if !self.is_removeable(card, playerid, pos, reason::COST) {
            return false;
        }
        let mut redirchk = false;
        let saved = self.cards[card].sendto_param;
        self.cards[card].sendto_param.location = location::REMOVED;
        let mut dest = location::REMOVED;
        if self.cards[card]
            .current
            .is_location(u16::from(location::ONFIELD))
        {
            let redirect = (self.leave_field_redirect(card, reason::COST) & 0xffff) as u8;
            if redirect != 0 {
                redirchk = true;
                dest = redirect;
            }
        }
        let redirect = (self.destination_redirect(card, dest, reason::COST) & 0xffff) as u8;
        if redirect != 0 {
            redirchk = true;
            dest = redirect;
        }
        self.cards[card].sendto_param = saved;
        dest == location::REMOVED && !(redirchk && pos & crate::board::position::FACEDOWN != 0)
    }

    /// `card::is_capable_send_to_hand`.
    ///
    /// Note the fourth test: an extra-deck monster can only be sent to the
    /// hand if it could be sent to the **deck**. That is not a typo — such a
    /// card in the hand would be illegal, so the reference asks the question
    /// about where it would actually end up.
    pub fn is_capable_send_to_hand(&mut self, card: CardId, playerid: u8) -> bool {
        if self.cards[card].is_status(status::LEAVE_CONFIRMED) {
            return false;
        }
        let extra_types = self.extra_deck_types();
        let is_extra = self.cards[card].is_extra_deck_monster(extra_types);
        if self.cards[card].current.location == crate::board::location::EXTRA && is_extra {
            return false;
        }
        if self
            .is_affected_by_effect(card, code::CANNOT_TO_HAND)
            .is_some()
        {
            return false;
        }
        if is_extra && !self.is_capable_send_to_deck(card, playerid) {
            return false;
        }
        self.is_player_can_send_to_hand(playerid, card)
    }

    /// `card::is_capable_send_to_deck`.
    pub fn is_capable_send_to_deck(&mut self, card: CardId, playerid: u8) -> bool {
        if self.cards[card].is_status(status::LEAVE_CONFIRMED) {
            return false;
        }
        let extra_types = self.extra_deck_types();
        if self.cards[card].current.location == crate::board::location::EXTRA
            && self.cards[card].is_extra_deck_monster(extra_types)
        {
            return false;
        }
        if self
            .is_affected_by_effect(card, code::CANNOT_TO_DECK)
            .is_some()
        {
            return false;
        }
        self.is_player_can_send_to_deck(playerid, card)
    }

    /// `card::is_capable_send_to_extra`.
    ///
    /// Only an extra-deck monster or a Pendulum belongs there at all — and
    /// the prohibition it consults is `CANNOT_TO_DECK`, not a separate one.
    /// The extra deck is a deck for this purpose.
    pub fn is_capable_send_to_extra(&mut self, card: CardId, playerid: u8) -> bool {
        let extra_types = self.extra_deck_types();
        if !self.cards[card].is_extra_deck_monster(extra_types)
            && !self.cards[card].data.is_type(card_type::PENDULUM)
        {
            return false;
        }
        if self
            .is_affected_by_effect(card, code::CANNOT_TO_DECK)
            .is_some()
        {
            return false;
        }
        self.is_player_can_send_to_deck(playerid, card)
    }

    /// `card::is_removeable`.
    ///
    /// The last test is the odd one: a **token** cannot be banished
    /// face-down. A token that left the field ceases to exist, so there
    /// would be nothing face-down to banish.
    pub fn is_removeable(&mut self, card: CardId, playerid: u8, pos: u8, reason: u32) -> bool {
        if !self.is_player_can_remove(playerid, card, reason) {
            return false;
        }
        if self
            .is_affected_by_effect(card, code::CANNOT_REMOVE)
            .is_some()
        {
            return false;
        }
        if self.cards[card].data.is_type(card_type::TOKEN)
            && pos & crate::board::position::FACEDOWN != 0
        {
            return false;
        }
        true
    }

    /// Which prohibition a destination is governed by — the mapping
    /// `SendTo`'s first step makes.
    pub fn capability_for(
        &mut self,
        card: CardId,
        playerid: u8,
        dest: u8,
        pos: u8,
        reason: u32,
    ) -> bool {
        use crate::board::location as l;
        match dest {
            l::HAND => self.is_capable_send_to_hand(card, playerid),
            l::DECK => self.is_capable_send_to_deck(card, playerid),
            l::REMOVED => self.is_removeable(card, playerid, pos, reason),
            l::GRAVE => self.is_capable_send_to_grave(card, playerid, reason),
            l::EXTRA => self.is_capable_send_to_extra(card, playerid),
            _ => true,
        }
    }

    /// Effects granting an unconditional prohibition, for tests and for
    /// reading: an effect with no `target_filter` forbids outright.
    pub fn prohibition_is_unconditional(&self, id: EffectId) -> bool {
        self.effects
            .get(id)
            .is_some_and(|e| e.target_filter.is_none())
    }

    /// Run one effect's value as a yes/no with the three arguments the
    /// destruction predicates all push: the effect behind the move, the
    /// reason, and the player.
    ///
    /// The reference pushes these before every `check_value_condition(3)` in
    /// `Destroy`, and the order is the order it pushes them in. Dropping or
    /// reordering them is invisible — the value function simply sees
    /// different arguments and a card silently stops protecting itself.
    pub(crate) fn destruction_condition_pub(
        &self,
        effect: EffectId,
        card: CardId,
        by: Option<EffectId>,
        reason: u32,
        playerid: u8,
    ) -> bool {
        let Some(e) = self.effects.get(effect) else {
            return false;
        };
        let ev = Event::new(0);
        let ctx = Ctx {
            reason_effect: by.unwrap_or(effect),
            player: playerid,
            event: &ev,
            card: Some(card),
            args: &[
                by.map_or(0, |b| b as i64),
                i64::from(reason),
                i64::from(playerid),
            ],
        };
        e.check_value_condition(self, &ctx)
    }

    /// The same three arguments, read as a *number* rather than a yes/no.
    ///
    /// `EFFECT_INDESTRUCTABLE_COUNT` without `EFFECT_FLAG_COUNT_LIMIT` uses
    /// its value to say **how many times**, where every other destruction
    /// predicate reads the same field as a condition. Same call, different
    /// question — which is why the reference has `get_value(3)` here and
    /// `check_value_condition(3)` everywhere else.
    pub(crate) fn effect_destruction_value(
        &self,
        effect: EffectId,
        card: CardId,
        by: Option<EffectId>,
        reason: u32,
        playerid: u8,
    ) -> i64 {
        let Some(e) = self.effects.get(effect) else {
            return 0;
        };
        let ev = Event::new(0);
        let ctx = Ctx {
            reason_effect: by.unwrap_or(effect),
            player: playerid,
            event: &ev,
            card: Some(card),
            args: &[
                by.map_or(0, |b| b as i64),
                i64::from(reason),
                i64::from(playerid),
            ],
        };
        e.get_value(self, &ctx)
    }

    /// `card::is_destructable` — can this card be destroyed *at all*?
    ///
    /// Nothing to do with effects: an Xyz material is not a card on the
    /// board, and a card already in the graveyard or banished has nothing
    /// left to destroy. Everything else is destructible, and every reason a
    /// particular destruction fails is asked separately.
    pub fn is_destructable(&self, card: CardId) -> bool {
        if self.cards[card].overlay_target.is_some() {
            return false;
        }
        self.cards[card].current.location & (location::GRAVE | location::REMOVED) == 0
    }

    /// `card::check_indestructable_by_effect` — which effect, if any, is
    /// protecting this card from *that* effect.
    ///
    /// Returns the protecting effect rather than a yes/no, because the
    /// caller has to charge its count. **No effect behind the destruction
    /// means no protection to find**: a destruction with no `reason_effect`
    /// — the rules destroying something — cannot be answered by an
    /// "indestructible by effects" clause, so the reference returns early
    /// rather than running the filters against a null.
    pub fn check_indestructable_by_effect(
        &self,
        card: CardId,
        by: Option<EffectId>,
        playerid: u8,
    ) -> Option<EffectId> {
        by?;
        self.filter_effect(card, code::INDESTRUCTABLE_EFFECT)
            .into_iter()
            .find(|&e| self.destruction_condition_pub(e, card, by, 0, playerid))
    }

    /// `card::is_releasable_by_summon` — may this be tributed for a summon?
    ///
    /// Two prohibitions, aimed in opposite directions, and both are needed:
    /// `EFFECT_UNRELEASABLE_SUM` on **this** card asks "may I be tributed
    /// for that monster", and `EFFECT_TRIBUTE_LIMIT` on **the summoning
    /// monster** asks "may I tribute that card". A port that checked only
    /// one would let half the tribute restrictions through.
    pub fn is_releasable_by_summon(&mut self, card: CardId, playerid: u8, by: CardId) -> bool {
        if self.cards[card].is_status(status::SUMMONING) {
            return false;
        }
        if self.cards[card].overlay_target.is_some() {
            return false;
        }
        if self.cards[card].current.location & (location::GRAVE | location::REMOVED) != 0 {
            return false;
        }
        if !self.is_player_can_release(playerid, card, reason::SUMMON) {
            return false;
        }
        if self
            .is_affected_by_effect_against(card, code::UNRELEASABLE_SUM, by)
            .is_some()
        {
            return false;
        }
        self.is_affected_by_effect_against(by, code::TRIBUTE_LIMIT, card)
            .is_none()
    }

    /// `card::is_releasable_by_nonsummon` — may this be released for
    /// anything else?
    ///
    /// The same opening as the summon form, plus one rule that only applies
    /// here: **a Spell or Trap in the hand cannot be released.** Monsters in
    /// the hand can (that is how a hand tribute works); a Spell or Trap in
    /// the hand has nothing to release.
    pub fn is_releasable_by_nonsummon(&mut self, card: CardId, playerid: u8, reason_: u32) -> bool {
        if self.cards[card].is_status(status::SUMMONING) {
            return false;
        }
        if self.cards[card].overlay_target.is_some() {
            return false;
        }
        if self.cards[card].current.location & (location::GRAVE | location::REMOVED) != 0 {
            return false;
        }
        if self.cards[card].current.location == location::HAND
            && self.cards[card]
                .data
                .is_type(card_type::SPELL | card_type::TRAP)
        {
            return false;
        }
        if !self.is_player_can_release(playerid, card, reason_) {
            return false;
        }
        self.is_affected_by_effect(card, code::UNRELEASABLE_NONSUM)
            .is_none()
    }

    /// `field::is_player_can_release`.
    pub fn is_player_can_release(&mut self, playerid: u8, card: CardId, reason_: u32) -> bool {
        self.no_prohibition(
            playerid,
            code::CANNOT_RELEASE,
            card,
            &[i64::from(playerid), i64::from(reason_)],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::{location, position};
    use crate::card::{Card, CardData};
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
        let id = f.new_card(c);
        f.add_card(0, id, loc, 0, false);
        id
    }

    /// A player-wide prohibition, with or without a target.
    fn prohibition(
        f: &mut Field,
        code_: u32,
        target: Option<crate::effect::TargetFilter>,
    ) -> EffectId {
        let mut source = Card::new(1, 0);
        source.current.controller = 0;
        source.current.location = location::MZONE;
        source.current.position = position::FACEUP_ATTACK;
        source.set_status(status::EFFECT_ENABLED, true);
        let source = f.new_card(source);

        let mut e = Effect::new(effect_type::FIELD, code_);
        e.owner = Some(source);
        e.handler = Some(source);
        e.effect_owner = 0;
        e.flag[0] |= flag::PLAYER_TARGET;
        e.range = u16::from(location::MZONE);
        e.s_range = u16::from(location::MZONE);
        e.target_filter = target;
        let id = f.new_effect(e);
        f.field_effects.aura.insert(code_, id);
        f.field_effects.indexer.insert(id);
        id
    }

    /// The rule that inverts if you read a missing target as "no
    /// restriction": a prohibition **without** a target forbids everything.
    #[test]
    fn a_prohibition_without_a_target_forbids_everything() {
        fn never(_: &Field, _: EffectId, _: Option<CardId>, _: &[i64]) -> bool {
            false
        }

        let mut f = Field::new(8000);
        let c = card_in(&mut f, location::MZONE, card_type::MONSTER);
        assert!(f.is_capable_send_to_grave(c, 0, 0), "nothing forbids it");

        // With a target that declines, the card is still allowed.
        let mut g = Field::new(8000);
        let c2 = card_in(&mut g, location::MZONE, card_type::MONSTER);
        prohibition(&mut g, code::CANNOT_TO_GRAVE, Some(never));
        assert!(
            g.is_capable_send_to_grave(c2, 0, 0),
            "a conditional prohibition that says no does not forbid"
        );

        // With no target at all, it forbids outright.
        let mut h = Field::new(8000);
        let c3 = card_in(&mut h, location::MZONE, card_type::MONSTER);
        let p = prohibition(&mut h, code::CANNOT_TO_GRAVE, None);
        assert!(h.prohibition_is_unconditional(p));
        assert!(
            !h.is_capable_send_to_grave(c3, 0, 0),
            "an unconditional prohibition forbids"
        );
    }

    /// A prohibition on the card itself is a different route to the same no.
    #[test]
    fn a_prohibition_on_the_card_forbids_too() {
        let mut f = Field::new(8000);
        let c = card_in(&mut f, location::MZONE, card_type::MONSTER);
        let mut e = Effect::new(effect_type::SINGLE, code::CANNOT_TO_GRAVE);
        e.owner = Some(c);
        e.handler = Some(c);
        let e = f.new_effect(e);
        f.cards[c].single_effect.insert(code::CANNOT_TO_GRAVE, e);
        assert!(!f.is_capable_send_to_grave(c, 0, 0));
    }

    /// A card already confirmed to be leaving cannot be sent to the hand or
    /// deck — it is on its way somewhere else.
    #[test]
    fn a_card_confirmed_to_leave_goes_nowhere_else() {
        let mut f = Field::new(8000);
        let c = card_in(&mut f, location::MZONE, card_type::MONSTER);
        assert!(f.is_capable_send_to_hand(c, 0));
        f.cards[c].set_status(status::LEAVE_CONFIRMED, true);
        assert!(!f.is_capable_send_to_hand(c, 0));
        assert!(!f.is_capable_send_to_deck(c, 0));
        assert!(
            f.is_capable_send_to_grave(c, 0, 0),
            "but the graveyard is still open"
        );
    }

    /// An extra-deck monster in the extra deck stays there.
    #[test]
    fn an_extra_deck_monster_in_the_extra_deck_stays() {
        let mut f = Field::new(8000);
        let c = card_in(
            &mut f,
            location::EXTRA,
            card_type::MONSTER | card_type::FUSION,
        );
        assert_eq!(f.cards[c].current.location, location::EXTRA);
        assert!(!f.is_capable_send_to_hand(c, 0));
        assert!(!f.is_capable_send_to_deck(c, 0));
    }

    /// Only an extra-deck monster or a Pendulum belongs in the extra deck —
    /// and the prohibition it consults is `CANNOT_TO_DECK`, because the
    /// extra deck is a deck for this purpose.
    #[test]
    fn the_extra_deck_is_a_deck_for_prohibitions() {
        let mut f = Field::new(8000);
        let ordinary = card_in(&mut f, location::MZONE, card_type::MONSTER);
        assert!(!f.is_capable_send_to_extra(ordinary, 0));

        let mut g = Field::new(8000);
        let fusion = card_in(
            &mut g,
            location::MZONE,
            card_type::MONSTER | card_type::FUSION,
        );
        assert!(g.is_capable_send_to_extra(fusion, 0));
        prohibition(&mut g, code::CANNOT_TO_DECK, None);
        assert!(
            !g.is_capable_send_to_extra(fusion, 0),
            "a deck prohibition covers the extra deck"
        );
    }

    /// A token cannot be banished face-down: it would cease to exist on the
    /// way, leaving nothing to banish.
    #[test]
    fn a_token_cannot_be_banished_face_down() {
        let mut f = Field::new(8000);
        let token = card_in(
            &mut f,
            location::MZONE,
            card_type::MONSTER | card_type::TOKEN,
        );
        assert!(f.is_removeable(token, 0, position::FACEUP_ATTACK, 0));
        assert!(!f.is_removeable(token, 0, position::FACEDOWN_DEFENSE, 0));

        let ordinary = card_in(&mut f, location::MZONE, card_type::MONSTER);
        assert!(
            f.is_removeable(ordinary, 0, position::FACEDOWN_DEFENSE, 0),
            "an ordinary card can be"
        );
    }

    /// The destination-to-prohibition mapping `SendTo` makes.
    #[test]
    fn each_destination_asks_its_own_question() {
        let mut f = Field::new(8000);
        let c = card_in(&mut f, location::MZONE, card_type::MONSTER);
        prohibition(&mut f, code::CANNOT_TO_HAND, None);

        assert!(!f.capability_for(c, 0, location::HAND, 0, 0));
        assert!(
            f.capability_for(c, 0, location::GRAVE, 0, 0),
            "a hand prohibition does not cover the graveyard"
        );
        assert!(
            f.capability_for(c, 0, location::MZONE, 0, 0),
            "a destination with no capability question is allowed"
        );
    }

    /// The predicates `Destroy` and `Release` gate on.
    mod destruction_and_release {
        use super::*;
        use crate::board::position;
        use crate::effect::{effect_type, Effect};

        fn on_field(f: &mut Field, player: u8, loc: u8, type_: u32) -> CardId {
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

        /// An effect on the card whose value is a constant.
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

        mod is_destructable {
            use super::*;

            #[test]
            fn an_ordinary_card_on_the_field_is() {
                let mut f = Field::new(8000);
                let c = on_field(&mut f, 0, location::MZONE, card_type::MONSTER);
                assert!(f.is_destructable(c));
            }

            /// A material is not a card on the board.
            ///
            /// The material is deliberately placed in the **overlay**
            /// location, not the graveyard. Written the obvious way — with
            /// the material sitting in the graveyard — the *location* test
            /// answers first and the overlay test is never reached, so the
            /// test passes whether or not the overlay check exists. A
            /// mutation that deleted that check went unnoticed until this
            /// was fixed.
            #[test]
            fn an_xyz_material_is_not() {
                let mut f = Field::new(8000);
                let xyz = on_field(&mut f, 0, location::MZONE, card_type::MONSTER);
                let mat = on_field(&mut f, 0, location::MZONE, card_type::MONSTER);
                f.cards[mat].overlay_target = Some(xyz);
                f.cards[mat].current.location = location::OVERLAY;
                assert!(
                    f.cards[mat].current.location & (location::GRAVE | location::REMOVED) == 0,
                    "the location test must not be what answers here"
                );
                assert!(!f.is_destructable(mat));
            }

            /// Nothing left to destroy.
            #[test]
            fn a_card_already_gone_is_not() {
                let mut f = Field::new(8000);
                for loc in [location::GRAVE, location::REMOVED] {
                    let c = on_field(&mut f, 0, loc, card_type::MONSTER);
                    assert!(!f.is_destructable(c), "location {loc:#x}");
                }
            }
        }

        mod check_indestructable_by_effect {
            use super::*;

            /// No effect behind the destruction means no protection to find:
            /// an "indestructible by effects" clause cannot answer the rules.
            #[test]
            fn a_destruction_with_no_effect_behind_it_finds_no_protection() {
                let mut f = Field::new(8000);
                let c = on_field(&mut f, 0, location::MZONE, card_type::MONSTER);
                printed(&mut f, c, code::INDESTRUCTABLE_EFFECT, 1);
                assert!(
                    f.check_indestructable_by_effect(c, None, 0).is_none(),
                    "the rules are not an effect to be protected from"
                );
            }

            #[test]
            fn a_protecting_effect_is_returned_not_just_reported() {
                let mut f = Field::new(8000);
                let c = on_field(&mut f, 0, location::MZONE, card_type::MONSTER);
                let shield = printed(&mut f, c, code::INDESTRUCTABLE_EFFECT, 1);
                let by = printed(&mut f, c, code::UPDATE_ATTACK, 0);
                assert_eq!(
                    f.check_indestructable_by_effect(c, Some(by), 0),
                    Some(shield),
                    "the caller needs the effect itself, to charge its count"
                );
            }

            /// A condition that declines protects nothing.
            #[test]
            fn a_condition_of_zero_does_not_protect() {
                let mut f = Field::new(8000);
                let c = on_field(&mut f, 0, location::MZONE, card_type::MONSTER);
                printed(&mut f, c, code::INDESTRUCTABLE_EFFECT, 0);
                let by = printed(&mut f, c, code::UPDATE_ATTACK, 0);
                assert!(f.check_indestructable_by_effect(c, Some(by), 0).is_none());
            }
        }

        mod is_releasable_by_nonsummon {
            use super::*;

            #[test]
            fn a_monster_on_the_field_is_releasable() {
                let mut f = Field::new(8000);
                let c = on_field(&mut f, 0, location::MZONE, card_type::MONSTER);
                assert!(f.is_releasable_by_nonsummon(c, 0, reason::COST));
            }

            /// A monster in the hand can be released — that is how a hand
            /// tribute works.
            #[test]
            fn a_monster_in_the_hand_is_releasable() {
                let mut f = Field::new(8000);
                let c = on_field(&mut f, 0, location::HAND, card_type::MONSTER);
                assert!(f.is_releasable_by_nonsummon(c, 0, reason::COST));
            }

            /// But a Spell or Trap in the hand is not: the rule that exists
            /// only in this form of the predicate.
            #[test]
            fn a_spell_or_trap_in_the_hand_is_not() {
                let mut f = Field::new(8000);
                for t in [card_type::SPELL, card_type::TRAP] {
                    let c = on_field(&mut f, 0, location::HAND, t);
                    assert!(
                        !f.is_releasable_by_nonsummon(c, 0, reason::COST),
                        "type {t:#x} in the hand"
                    );
                }
            }

            #[test]
            fn a_card_being_summoned_is_not() {
                let mut f = Field::new(8000);
                let c = on_field(&mut f, 0, location::MZONE, card_type::MONSTER);
                f.cards[c].set_status(status::SUMMONING, true);
                assert!(!f.is_releasable_by_nonsummon(c, 0, reason::COST));
            }

            #[test]
            fn an_unreleasable_effect_forbids_it() {
                let mut f = Field::new(8000);
                let c = on_field(&mut f, 0, location::MZONE, card_type::MONSTER);
                printed(&mut f, c, code::UNRELEASABLE_NONSUM, 1);
                assert!(!f.is_releasable_by_nonsummon(c, 0, reason::COST));
            }
        }

        mod is_releasable_by_summon {
            use super::*;

            #[test]
            fn an_ordinary_tribute_is_allowed() {
                let mut f = Field::new(8000);
                let tribute = on_field(&mut f, 0, location::MZONE, card_type::MONSTER);
                let summoning = on_field(&mut f, 0, location::HAND, card_type::MONSTER);
                assert!(f.is_releasable_by_summon(tribute, 0, summoning));
            }

            /// The two prohibitions point in opposite directions, and both
            /// are consulted. First: "I may not be tributed for that".
            #[test]
            fn the_tribute_may_refuse_the_summoner() {
                let mut f = Field::new(8000);
                let tribute = on_field(&mut f, 0, location::MZONE, card_type::MONSTER);
                let summoning = on_field(&mut f, 0, location::HAND, card_type::MONSTER);
                printed(&mut f, tribute, code::UNRELEASABLE_SUM, 1);
                assert!(!f.is_releasable_by_summon(tribute, 0, summoning));
            }

            /// And: "I may not tribute that". Checking only the first would
            /// let half the tribute restrictions through.
            #[test]
            fn the_summoner_may_refuse_the_tribute() {
                let mut f = Field::new(8000);
                let tribute = on_field(&mut f, 0, location::MZONE, card_type::MONSTER);
                let summoning = on_field(&mut f, 0, location::HAND, card_type::MONSTER);
                printed(&mut f, summoning, code::TRIBUTE_LIMIT, 1);
                assert!(
                    !f.is_releasable_by_summon(tribute, 0, summoning),
                    "the limit is printed on the summoning monster, not the tribute"
                );
            }

            /// A value of zero means the restriction does not apply to this
            /// pairing — which is the whole point of asking the value about a
            /// target rather than merely finding the effect.
            #[test]
            fn a_restriction_that_declines_this_pairing_allows_it() {
                let mut f = Field::new(8000);
                let tribute = on_field(&mut f, 0, location::MZONE, card_type::MONSTER);
                let summoning = on_field(&mut f, 0, location::HAND, card_type::MONSTER);
                printed(&mut f, summoning, code::TRIBUTE_LIMIT, 0);
                assert!(f.is_releasable_by_summon(tribute, 0, summoning));
            }
        }
    }

    mod cost_to_grave {
        use super::*;

        fn single_on(f: &mut Field, card: CardId, code_: u32) -> EffectId {
            let mut e = Effect::new(effect_type::SINGLE, code_);
            e.owner = Some(card);
            e.handler = Some(card);
            let id = f.new_effect(e);
            f.cards[card].single_effect.insert(code_, id);
            f.cards[card].indexer.insert(id);
            id
        }

        /// **The plain case, and the three refusals that are about the
        /// card itself**: a token, a card already in a graveyard, and one
        /// under either cost prohibition.
        #[test]
        fn it_refuses_a_token_a_buried_card_and_the_two_prohibitions() {
            let mut f = Field::new(8000);
            let c = card_in(&mut f, location::MZONE, card_type::MONSTER);
            assert!(f.is_capable_cost_to_grave(c, 0), "the positive sibling");

            let mut f = Field::new(8000);
            let tok = card_in(
                &mut f,
                location::MZONE,
                card_type::MONSTER | card_type::TOKEN,
            );
            assert!(!f.is_capable_cost_to_grave(tok, 0), "a token");

            let mut f = Field::new(8000);
            let buried = card_in(&mut f, location::GRAVE, card_type::MONSTER);
            assert!(!f.is_capable_cost_to_grave(buried, 0), "already there");

            for code_ in [code::CANNOT_USE_AS_COST, code::CANNOT_TO_GRAVE_AS_COST] {
                let mut f = Field::new(8000);
                let c = card_in(&mut f, location::MZONE, card_type::MONSTER);
                single_on(&mut f, c, code_);
                assert!(!f.is_capable_cost_to_grave(c, 0), "under {code_}");
            }
        }

        /// **The ordinary send has to be possible too**, which is a
        /// different prohibition from the cost-specific ones.
        #[test]
        fn it_also_asks_whether_the_card_could_be_sent_at_all() {
            let mut f = Field::new(8000);
            let c = card_in(&mut f, location::MZONE, card_type::MONSTER);
            single_on(&mut f, c, code::CANNOT_TO_GRAVE);
            assert!(!f.is_capable_cost_to_grave(c, 0));
        }

        /// **A card that would be redirected elsewhere cannot pay**, and
        /// the check leaves `sendto_param` as it found it.
        ///
        /// A cost has to land where it said it would: an effect sending
        /// the card to the banished pile instead means the graveyard is
        /// not where it ends up, so the cost is not payable.
        #[test]
        fn a_redirected_card_cannot_pay_and_the_parameter_is_restored() {
            let mut f = Field::new(8000);
            let c = card_in(&mut f, location::MZONE, card_type::MONSTER);
            let before = f.cards[c].sendto_param;
            assert!(f.is_capable_cost_to_grave(c, 0), "the positive sibling");
            assert_eq!(f.cards[c].sendto_param, before, "restored");

            let mut e = Effect::new(effect_type::SINGLE, code::LEAVE_FIELD_REDIRECT);
            e.owner = Some(c);
            e.handler = Some(c);
            e.value = i64::from(location::REMOVED);
            let id = f.new_effect(e);
            f.cards[c]
                .single_effect
                .insert(code::LEAVE_FIELD_REDIRECT, id);
            f.cards[c].indexer.insert(id);
            let before = f.cards[c].sendto_param;
            assert!(
                !f.is_capable_cost_to_grave(c, 0),
                "it would be banished instead"
            );
            assert_eq!(f.cards[c].sendto_param, before, "still restored");
        }
    }

    mod removeable_as_cost {
        use super::*;

        fn single_on(f: &mut Field, card: CardId, code_: u32) -> EffectId {
            let mut e = Effect::new(effect_type::SINGLE, code_);
            e.owner = Some(card);
            e.handler = Some(card);
            let id = f.new_effect(e);
            f.cards[card].single_effect.insert(code_, id);
            f.cards[card].indexer.insert(id);
            id
        }

        fn redirect_on(f: &mut Field, card: CardId, code_: u32, dest: u8) {
            let mut e = Effect::new(effect_type::SINGLE, code_);
            e.owner = Some(card);
            e.handler = Some(card);
            e.value = i64::from(dest);
            let id = f.new_effect(e);
            f.cards[card].single_effect.insert(code_, id);
            f.cards[card].indexer.insert(id);
        }

        /// The positive sibling, and the two refusals about the card
        /// itself.
        #[test]
        fn it_refuses_a_banished_card_and_the_cost_prohibition() {
            let mut f = Field::new(8000);
            let c = card_in(&mut f, location::MZONE, card_type::MONSTER);
            assert!(f.is_removeable_as_cost(c, 0, position::FACEUP));

            let mut f = Field::new(8000);
            let gone = card_in(&mut f, location::REMOVED, card_type::MONSTER);
            assert!(!f.is_removeable_as_cost(gone, 0, position::FACEUP));

            let mut f = Field::new(8000);
            let c = card_in(&mut f, location::MZONE, card_type::MONSTER);
            single_on(&mut f, c, code::CANNOT_USE_AS_COST);
            assert!(!f.is_removeable_as_cost(c, 0, position::FACEUP));
        }

        /// The ordinary banish has to be possible too — a different
        /// prohibition from the cost-specific one.
        #[test]
        fn it_also_asks_whether_the_card_could_be_banished_at_all() {
            let mut f = Field::new(8000);
            let c = card_in(&mut f, location::MZONE, card_type::MONSTER);
            single_on(&mut f, c, code::CANNOT_REMOVE);
            assert!(!f.is_removeable_as_cost(c, 0, position::FACEUP));
        }

        /// A card that would land somewhere else cannot pay, and
        /// `sendto_param` is left as it was found.
        #[test]
        fn a_redirected_card_cannot_pay_and_the_parameter_is_restored() {
            let mut f = Field::new(8000);
            let c = card_in(&mut f, location::MZONE, card_type::MONSTER);
            let before = f.cards[c].sendto_param;
            assert!(f.is_removeable_as_cost(c, 0, position::FACEUP));
            assert_eq!(f.cards[c].sendto_param, before, "restored");

            // Not the graveyard: `leave_field_redirect` only ever answers
            // REMOVED, DECK or HAND, because leaving the field for the
            // graveyard is the default rather than a diversion.
            redirect_on(&mut f, c, code::LEAVE_FIELD_REDIRECT, location::HAND);
            let before = f.cards[c].sendto_param;
            assert!(
                !f.is_removeable_as_cost(c, 0, position::FACEUP),
                "it would go back to the hand instead"
            );
            assert_eq!(f.cards[c].sendto_param, before, "still restored");
        }

        /// **The twist the graveyard sibling does not have.** A redirect
        /// that lands the card in the banished pile *anyway* still counts
        /// as a redirect, and a face-down banish is refused when one
        /// fired — `redirchk && (pos & POS_FACEDOWN)`.
        ///
        /// Reading the tail as `dest != LOCATION_REMOVED` alone lets this
        /// through, and nothing else in the function would notice.
        #[test]
        fn a_redirect_to_the_banished_pile_still_blocks_a_face_down_cost() {
            let mut f = Field::new(8000);
            let c = card_in(&mut f, location::MZONE, card_type::MONSTER);
            assert!(
                f.is_removeable_as_cost(c, 0, position::FACEDOWN),
                "face-down is fine with no redirect at all"
            );
            redirect_on(&mut f, c, code::LEAVE_FIELD_REDIRECT, location::REMOVED);
            assert!(
                f.is_removeable_as_cost(c, 0, position::FACEUP),
                "face-up is unaffected: it still ends up banished"
            );
            assert!(
                !f.is_removeable_as_cost(c, 0, position::FACEDOWN),
                "but face-down is refused once a redirect has fired"
            );
        }
    }
}

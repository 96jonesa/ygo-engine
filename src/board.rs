//! The board: where a card is, and which way up.
//!
//! Translated from ocgcore's `LOCATION_*` and `POS_*` constants and the
//! positional half of `card_state`. The numeric values are the reference's
//! own: they are bit flags, tested with `&` throughout the engine, and a
//! port that renumbered them would have to rewrite every such test.

/// Bit flags naming a zone. A card is in exactly one, but queries combine
/// them — `LOCATION_ONFIELD` is the obvious case.
pub mod location {
    pub const DECK: u8 = 0x01;
    pub const HAND: u8 = 0x02;
    pub const MZONE: u8 = 0x04;
    pub const SZONE: u8 = 0x08;
    pub const GRAVE: u8 = 0x10;
    pub const REMOVED: u8 = 0x20;
    pub const EXTRA: u8 = 0x40;
    pub const OVERLAY: u8 = 0x80;
    /// The two zones a card can be "on the field" in.
    pub const ONFIELD: u8 = MZONE | SZONE;

    /// The *symbolic* locations, which the reference keeps separate for a
    /// reason worth stating: they are not values a card's `location` field
    /// ever holds. A card in the field zone has `location == SZONE` and
    /// `sequence == 5`; `FZONE` is a question you can ask about it, not a
    /// place it is stored. `Loc::is_location` is where that derivation
    /// lives, and it is why the query takes a `u16` while the field is a
    /// `u8`.
    pub const FZONE: u16 = 0x100;
    pub const PZONE: u16 = 0x200;
    pub const STZONE: u16 = 0x400;
    pub const MMZONE: u16 = 0x800;
    pub const EMZONE: u16 = 0x1000;

    /// The first sequence that is an Extra Monster Zone, and the seat the
    /// field zone occupies among the spell/trap zones.
    pub const EXTRA_ZONE_START: u32 = 5;
}

/// Destinations that are a zone *plus an instruction*.
///
/// These are not `location::*` values and deliberately do not live beside
/// them. Both contain `location::DECK` (`0x01`) in their low bit — they mean
/// "the deck, and here is where in it" — so they are wider than the `u8` a
/// card's `location` field is, and a membership test against either is true
/// for an ordinary return to the deck. The reference compares them for
/// **equality**; see `leave_field_redirect`.
pub mod redirect {
    /// `LOCATION_DECKBOT` — return to the bottom of the deck.
    pub const DECKBOT: u32 = 0x10001;
    /// `LOCATION_DECKSHF` — return to the deck, then shuffle it.
    pub const DECKSHF: u32 = 0x20001;
}

/// Battle position, also bit flags: queries ask "face-up?" or "attack?"
/// rather than testing equality.
pub mod position {
    pub const FACEUP_ATTACK: u8 = 0x1;
    pub const FACEDOWN_ATTACK: u8 = 0x2;
    pub const FACEUP_DEFENSE: u8 = 0x4;
    pub const FACEDOWN_DEFENSE: u8 = 0x8;

    pub const FACEUP: u8 = FACEUP_ATTACK | FACEUP_DEFENSE;
    pub const FACEDOWN: u8 = FACEDOWN_ATTACK | FACEDOWN_DEFENSE;
    pub const ATTACK: u8 = FACEUP_ATTACK | FACEDOWN_ATTACK;
    pub const DEFENSE: u8 = FACEUP_DEFENSE | FACEDOWN_DEFENSE;
}

/// A card's position in the world: which player's zone, which zone, which
/// slot, and which way up.
///
/// ocgcore keeps this inside `card_state` alongside the card's statistics,
/// and every card carries two of them — `current` and `previous` — because
/// a great many rules ask where a card *was*. Only the positional half is
/// modelled here; the statistics arrive with the card model.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Loc {
    /// The player whose zone this is. Not necessarily the owner: control
    /// changes hands, ownership does not.
    pub controller: u8,
    /// A `location::*` flag.
    pub location: u8,
    /// The slot within the zone. Meaningful for MZONE and SZONE; an index
    /// into the pile elsewhere.
    pub sequence: u32,
    /// A `position::*` flag.
    pub position: u8,
    /// Set when this spell/trap seat is a Pendulum Zone. The reference
    /// carries the same flag on `card_state`, because — unlike the field
    /// zone — a Pendulum Zone is not identifiable from the sequence alone.
    pub pzone: bool,
}

impl Loc {
    /// Is this card in any of the zones named by `loc`?
    ///
    /// The mask form is what the engine actually uses:
    /// `is_location(ONFIELD)` rather than two equality tests.
    ///
    /// The symbolic locations are *derived* here rather than stored, exactly
    /// as the reference derives them: a card in the field zone has
    /// `location == SZONE` and `sequence == 5`, and asking it for `FZONE`
    /// is a question about that pair. Storing them instead would mean two
    /// places that can disagree about where a card is.
    pub fn is_location(&self, loc: u16) -> bool {
        if u16::from(self.location) & (loc & 0xff) != 0 {
            return true;
        }
        let in_mzone = self.location == location::MZONE;
        let in_szone = self.location == location::SZONE;
        let extra = self.sequence >= location::EXTRA_ZONE_START;
        (loc & location::EMZONE != 0 && in_mzone && extra)
            || (loc & location::MMZONE != 0 && in_mzone && !extra)
            || (loc & location::STZONE != 0 && in_szone && !extra)
            || (loc & location::FZONE != 0
                && in_szone
                && self.sequence == location::EXTRA_ZONE_START)
            || (loc & location::PZONE != 0 && in_szone && self.pzone)
    }

    pub fn is_position(&self, pos: u8) -> bool {
        self.position & pos != 0
    }

    pub fn is_faceup(&self) -> bool {
        self.is_position(position::FACEUP)
    }

    pub fn is_onfield(&self) -> bool {
        self.is_location(u16::from(location::ONFIELD))
    }
}

/// One player's zones.
///
/// The sizes are ocgcore's: seven monster zones and eight spell/trap zones,
/// which are wider than the five-and-five a Goat-era board uses. The extra
/// slots are the Extra Monster Zones and the field/pendulum zones, and they
/// are modelled because the reference models them — the duel *flags* decide
/// which are usable, not the array size. Under this project's configuration
/// (`DUEL_MODE_MR5 & ~DUEL_EMZONE`) the Extra Monster Zones are off.
#[derive(Clone, Debug)]
pub struct PlayerZones {
    pub lp: i32,
    /// Which field slots are occupied, as a bitfield: bits 0..=6 are the
    /// monster zones, bits 8..=15 the spell/trap zones. `used_location` in
    /// the reference, and the thing zone-counting actually reads — the slot
    /// arrays are the contents, this is the occupancy.
    ///
    /// Two representations of the same fact is a smell, and the reference
    /// keeps both deliberately: a card can occupy a zone it is not *in*
    /// (a Link monster's zone, a token mid-summon), so the bitfield is the
    /// authority on whether a seat is free.
    pub used_location: u32,
    /// Slots made unusable by an effect, in the same layout.
    pub disabled_location: u32,
    /// How many face-up Pendulum monsters sit at the *end* of the Extra
    /// Deck. They are kept there so the rest of the deck stays a pile, and
    /// the count is what tells the two apart.
    pub extra_p_count: usize,
    pub mzone: Vec<Option<usize>>,
    pub szone: Vec<Option<usize>>,
    /// How many cards this player draws to open, and how many each turn.
    /// Both come from the duel's team configuration rather than from the
    /// rules, which is why they are per-player state and not constants.
    pub start_count: i32,
    pub draw_count: i32,
    /// Relay duels hand the deck to the next player on this flag.
    /// `DUEL_RELAY` is off in this port's configuration, so it is never
    /// set — transcribed because `RefreshRelay` reads it.
    pub recharge: bool,
    pub main: Vec<usize>,
    pub hand: Vec<usize>,
    pub grave: Vec<usize>,
    pub removed: Vec<usize>,
    pub extra: Vec<usize>,
}

impl PlayerZones {
    pub const MZONE_SLOTS: usize = 7;
    pub const SZONE_SLOTS: usize = 8;

    pub fn new(starting_lp: i32) -> Self {
        Self {
            lp: starting_lp,
            used_location: 0,
            disabled_location: 0,
            extra_p_count: 0,
            // The reference takes both from the duel's team configuration.
            // Five and one are the ordinary values; a caller setting up a
            // duel overwrites them.
            start_count: 5,
            draw_count: 1,
            recharge: false,
            mzone: vec![None; Self::MZONE_SLOTS],
            szone: vec![None; Self::SZONE_SLOTS],
            main: Vec::new(),
            hand: Vec::new(),
            grave: Vec::new(),
            removed: Vec::new(),
            extra: Vec::new(),
        }
    }

    /// The pile for a location, where that location is a pile rather than a
    /// set of slots. `None` for MZONE and SZONE, which are addressed by
    /// sequence.
    pub fn pile(&self, loc: u8) -> Option<&Vec<usize>> {
        match loc {
            location::DECK => Some(&self.main),
            location::HAND => Some(&self.hand),
            location::GRAVE => Some(&self.grave),
            location::REMOVED => Some(&self.removed),
            location::EXTRA => Some(&self.extra),
            _ => None,
        }
    }

    /// Every slot that is occupied or disabled, in one mask — which is how
    /// the reference reads them: `disabled_location | used_location`.
    pub fn blocked(&self) -> u32 {
        self.disabled_location | self.used_location
    }

    pub fn pile_mut(&mut self, loc: u8) -> Option<&mut Vec<usize>> {
        match loc {
            location::DECK => Some(&mut self.main),
            location::HAND => Some(&mut self.hand),
            location::GRAVE => Some(&mut self.grave),
            location::REMOVED => Some(&mut self.removed),
            location::EXTRA => Some(&mut self.extra),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The values are the reference's, and the engine tests them as masks.
    #[test]
    fn locations_are_the_references_bit_flags() {
        assert_eq!(location::DECK, 0x01);
        assert_eq!(location::MZONE, 0x04);
        assert_eq!(location::SZONE, 0x08);
        assert_eq!(location::OVERLAY, 0x80);
        assert_eq!(location::ONFIELD, 0x0c, "MZONE | SZONE");
    }

    #[test]
    fn a_card_on_the_field_matches_the_onfield_mask() {
        let m = Loc {
            location: location::MZONE,
            ..Default::default()
        };
        assert!(m.is_onfield());
        assert!(m.is_location(u16::from(location::MZONE)));
        assert!(!m.is_location(u16::from(location::SZONE)));

        let g = Loc {
            location: location::GRAVE,
            ..Default::default()
        };
        assert!(!g.is_onfield());
    }

    /// Positions are masks too: "face-up" spans attack and defence.
    #[test]
    fn face_up_spans_both_attack_and_defence() {
        for pos in [position::FACEUP_ATTACK, position::FACEUP_DEFENSE] {
            let c = Loc {
                position: pos,
                ..Default::default()
            };
            assert!(c.is_faceup(), "{pos:#x} should be face-up");
        }
        for pos in [position::FACEDOWN_ATTACK, position::FACEDOWN_DEFENSE] {
            let c = Loc {
                position: pos,
                ..Default::default()
            };
            assert!(!c.is_faceup(), "{pos:#x} should not be face-up");
        }
    }

    /// Seven and eight, as the reference has them — the duel flags decide
    /// which slots are usable, not the array size.
    #[test]
    fn the_board_is_the_references_width() {
        let z = PlayerZones::new(8000);
        assert_eq!(z.mzone.len(), 7);
        assert_eq!(z.szone.len(), 8);
        assert_eq!(z.lp, 8000);
    }

    /// The symbolic locations are derived from location and sequence, not
    /// stored. This is the test that would have failed for the whole life of
    /// a `u8`-only query, silently answering "no" to every one of them.
    mod symbolic_locations {
        use super::*;

        #[test]
        fn the_field_zone_is_the_sixth_spell_trap_seat() {
            let fzone = Loc {
                location: location::SZONE,
                sequence: 5,
                ..Default::default()
            };
            assert!(fzone.is_location(location::FZONE));
            assert!(!fzone.is_location(location::STZONE), "not a normal seat");
            assert!(
                fzone.is_location(u16::from(location::SZONE)),
                "and still a spell/trap zone"
            );

            let normal = Loc {
                location: location::SZONE,
                sequence: 2,
                ..Default::default()
            };
            assert!(normal.is_location(location::STZONE));
            assert!(!normal.is_location(location::FZONE));
        }

        /// Unlike the field zone, a Pendulum Zone is not identifiable from
        /// the sequence, so the reference carries a flag and so does this.
        #[test]
        fn a_pendulum_zone_needs_its_flag() {
            let mut l = Loc {
                location: location::SZONE,
                sequence: 0,
                ..Default::default()
            };
            assert!(!l.is_location(location::PZONE));
            l.pzone = true;
            assert!(l.is_location(location::PZONE));
        }

        /// The monster zones split at sequence 5: below is a Main Monster
        /// Zone, at or above is an Extra Monster Zone. Under this project's
        /// configuration the latter are switched off by the duel flags, not
        /// by the board — so the query still answers.
        #[test]
        fn monster_zones_split_at_the_extra_zones() {
            let main = Loc {
                location: location::MZONE,
                sequence: 4,
                ..Default::default()
            };
            assert!(main.is_location(location::MMZONE));
            assert!(!main.is_location(location::EMZONE));

            let extra = Loc {
                location: location::MZONE,
                sequence: 5,
                ..Default::default()
            };
            assert!(extra.is_location(location::EMZONE));
            assert!(!extra.is_location(location::MMZONE));
        }

        /// A card off the field is in none of them, whatever its sequence.
        #[test]
        fn nothing_off_the_field_is_in_a_symbolic_zone() {
            let g = Loc {
                location: location::GRAVE,
                sequence: 5,
                ..Default::default()
            };
            for loc in [
                location::FZONE,
                location::PZONE,
                location::STZONE,
                location::MMZONE,
                location::EMZONE,
            ] {
                assert!(!g.is_location(loc), "{loc:#x} should not match a grave");
            }
        }
    }

    #[test]
    fn piles_are_addressed_by_location_and_slots_are_not() {
        let mut z = PlayerZones::new(8000);
        z.pile_mut(location::HAND).unwrap().push(3);
        assert_eq!(z.pile(location::HAND).unwrap(), &vec![3]);
        assert!(z.pile(location::MZONE).is_none(), "a zone, not a pile");
        assert!(z.pile(location::SZONE).is_none(), "a zone, not a pile");
    }
}

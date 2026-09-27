//! Duel options: the flags that decide which rules are in force.
//!
//! `DUEL_*` in the reference, transcribed whole. These are not a settings
//! bag — the machinery branches on them constantly, and the same code path
//! produces different rulings depending on which are set. Which ones this
//! project runs under is therefore part of the specification, not part of
//! the test harness, and is pinned as [`REFERENCE_CONFIGURATION`].

/// The flag table, exactly as the reference numbers it. 64 bits: the table
/// outgrew 32 and the reference widened it.
pub mod flags {
    pub const TEST_MODE: u64 = 0x01;
    pub const ATTACK_FIRST_TURN: u64 = 0x02;
    pub const USE_TRAPS_IN_NEW_CHAIN: u64 = 0x04;
    pub const SIX_STEP_BATTLE_STEP: u64 = 0x08;
    /// Insertion order is draw order. What lets a differential harness
    /// replay a known game instead of fighting the RNG.
    pub const PSEUDO_SHUFFLE: u64 = 0x10;
    pub const TRIGGER_WHEN_PRIVATE_KNOWLEDGE: u64 = 0x20;
    pub const SIMPLE_AI: u64 = 0x40;
    pub const RELAY: u64 = 0x80;
    pub const OCG_OBSOLETE_IGNITION: u64 = 0x100;
    pub const FIRST_TURN_DRAW: u64 = 0x200;
    pub const ONE_FACEUP_FIELD: u64 = 0x400;
    pub const PZONE: u64 = 0x800;
    pub const SEPARATE_PZONE: u64 = 0x1000;
    pub const EMZONE: u64 = 0x2000;
    pub const FSX_MMZONE: u64 = 0x4000;
    pub const TRAP_MONSTERS_NOT_USE_ZONE: u64 = 0x8000;
    pub const RETURN_TO_DECK_TRIGGERS: u64 = 0x10000;
    pub const TRIGGER_ONLY_IN_LOCATION: u64 = 0x20000;
    pub const SPSUMMON_ONCE_OLD_NEGATE: u64 = 0x40000;
    pub const CANNOT_SUMMON_OATH_OLD: u64 = 0x80000;
    pub const NO_STANDBY_PHASE: u64 = 0x100000;
    pub const NO_MAIN_PHASE_2: u64 = 0x200000;
    pub const THREE_COLUMNS_FIELD: u64 = 0x400000;
    pub const DRAW_UNTIL_5: u64 = 0x800000;
    pub const NO_HAND_LIMIT: u64 = 0x1000000;
    pub const UNLIMITED_SUMMONS: u64 = 0x2000000;
    pub const INVERTED_QUICK_PRIORITY: u64 = 0x4000000;
    pub const EQUIP_NOT_SENT_IF_MISSING_TARGET: u64 = 0x8000000;
    pub const ZERO_ATK_DESTROYED: u64 = 0x10000000;
    pub const STORE_ATTACK_REPLAYS: u64 = 0x20000000;
    pub const SINGLE_CHAIN_IN_DAMAGE_SUBSTEP: u64 = 0x40000000;
    pub const CAN_REPOS_IF_NON_SUMPLAYER: u64 = 0x80000000;
    pub const TCG_SEGOC_NONPUBLIC: u64 = 0x100000000;
    pub const TCG_SEGOC_FIRSTTRIGGER: u64 = 0x200000000;
    pub const TCG_FAST_EFFECT_IGNITION: u64 = 0x400000000;
    pub const EXTRA_DECK_RITUAL: u64 = 0x800000000;
    pub const NORMAL_SUMMON_FACEUP_DEF: u64 = 0x1000000000;
}

/// The named rule sets. Transcribed rather than reconstructed, including
/// the ones this project does not use — a mode defined by hand from a
/// half-remembered list is the same failure as a mistyped constant.
pub mod modes {
    use super::flags::*;

    pub const SPEED: u64 = THREE_COLUMNS_FIELD
        | NO_MAIN_PHASE_2
        | TRAP_MONSTERS_NOT_USE_ZONE
        | TRIGGER_ONLY_IN_LOCATION;

    pub const MR1: u64 = OCG_OBSOLETE_IGNITION
        | FIRST_TURN_DRAW
        | ONE_FACEUP_FIELD
        | SPSUMMON_ONCE_OLD_NEGATE
        | RETURN_TO_DECK_TRIGGERS
        | CANNOT_SUMMON_OATH_OLD;

    /// The reference's own Goat-format mode. **This project does not use
    /// it** — see [`super::REFERENCE_CONFIGURATION`] — and it is here so
    /// that the difference is visible rather than forgotten.
    pub const GOAT: u64 = MR1
        | TCG_FAST_EFFECT_IGNITION
        | USE_TRAPS_IN_NEW_CHAIN
        | SIX_STEP_BATTLE_STEP
        | TRIGGER_WHEN_PRIVATE_KNOWLEDGE
        | EQUIP_NOT_SENT_IF_MISSING_TARGET
        | ZERO_ATK_DESTROYED
        | STORE_ATTACK_REPLAYS
        | SINGLE_CHAIN_IN_DAMAGE_SUBSTEP
        | CAN_REPOS_IF_NON_SUMPLAYER
        | TCG_SEGOC_NONPUBLIC
        | TCG_SEGOC_FIRSTTRIGGER;

    pub const MR2: u64 = FIRST_TURN_DRAW
        | ONE_FACEUP_FIELD
        | SPSUMMON_ONCE_OLD_NEGATE
        | RETURN_TO_DECK_TRIGGERS
        | CANNOT_SUMMON_OATH_OLD;

    pub const MR3: u64 = PZONE
        | SEPARATE_PZONE
        | SPSUMMON_ONCE_OLD_NEGATE
        | RETURN_TO_DECK_TRIGGERS
        | CANNOT_SUMMON_OATH_OLD;

    pub const MR4: u64 = PZONE
        | EMZONE
        | SPSUMMON_ONCE_OLD_NEGATE
        | RETURN_TO_DECK_TRIGGERS
        | CANNOT_SUMMON_OATH_OLD;

    pub const MR5: u64 =
        PZONE | EMZONE | FSX_MMZONE | TRAP_MONSTERS_NOT_USE_ZONE | TRIGGER_ONLY_IN_LOCATION;
}

/// The phases of a turn, as the reference numbers them. Bit flags, because
/// queries ask "in the Battle Phase?" across several of them.
pub mod phases {
    pub const DRAW: u16 = 0x01;
    pub const STANDBY: u16 = 0x02;
    pub const MAIN1: u16 = 0x04;
    pub const BATTLE_START: u16 = 0x08;
    pub const BATTLE_STEP: u16 = 0x10;
    pub const DAMAGE: u16 = 0x20;
    pub const DAMAGE_CAL: u16 = 0x40;
    pub const BATTLE: u16 = 0x80;
    pub const MAIN2: u16 = 0x100;
    pub const END: u16 = 0x200;
}

/// The configuration both engines in this repository are compared under:
/// `(DUEL_MODE_MR5 & ~DUEL_EMZONE) | DUEL_PSEUDO_SHUFFLE`.
///
/// Two consequences of it are worth having in view, because they are
/// properties of the *configuration* rather than of the engine:
///
/// - Masking `EMZONE` off is a deliberate choice for the Goat-era board:
///   five Monster Zones, and a Fusion Monster occupies a main one.
/// - MR5 excludes `FIRST_TURN_DRAW` and `ATTACK_FIRST_TURN`, so there is no
///   draw and no Battle Phase on the first player's first turn.
///
/// Note that this is *not* the reference's own [`modes::GOAT`]. That mode
/// carries a dozen further flags — `TCG_SEGOC_NONPUBLIC`,
/// `SINGLE_CHAIN_IN_DAMAGE_SUBSTEP`, `USE_TRAPS_IN_NEW_CHAIN` among them —
/// each of which changes rulings. Anything comparing this port against a
/// Goat-format ruling from elsewhere has to account for that difference.
pub const REFERENCE_CONFIGURATION: u64 = (modes::MR5 & !flags::EMZONE) | flags::PSEUDO_SHUFFLE;

#[cfg(test)]
mod tests {
    use super::*;

    /// Pinned literals across the table.
    #[test]
    fn the_flags_are_the_references() {
        assert_eq!(flags::TEST_MODE, 0x01);
        assert_eq!(flags::PSEUDO_SHUFFLE, 0x10);
        assert_eq!(flags::PZONE, 0x800);
        assert_eq!(flags::EMZONE, 0x2000);
        assert_eq!(flags::THREE_COLUMNS_FIELD, 0x400000);
        assert_eq!(
            flags::NORMAL_SUMMON_FACEUP_DEF,
            0x1000000000,
            "the table outgrew 32 bits, which is why a flag set is a u64"
        );
    }

    #[test]
    fn mr5_is_the_references_composition() {
        assert_eq!(
            modes::MR5,
            flags::PZONE
                | flags::EMZONE
                | flags::FSX_MMZONE
                | flags::TRAP_MONSTERS_NOT_USE_ZONE
                | flags::TRIGGER_ONLY_IN_LOCATION
        );
    }

    /// The configuration is a specification, so it is pinned.
    #[test]
    fn the_reference_configuration_is_mr5_without_the_extra_zones() {
        let c = REFERENCE_CONFIGURATION;
        assert_eq!(c & flags::EMZONE, 0, "masked off for a Goat-era board");
        assert_ne!(c & flags::PZONE, 0);
        assert_ne!(c & flags::FSX_MMZONE, 0);
        assert_ne!(c & flags::TRAP_MONSTERS_NOT_USE_ZONE, 0);
        assert_ne!(c & flags::TRIGGER_ONLY_IN_LOCATION, 0);
        assert_ne!(c & flags::PSEUDO_SHUFFLE, 0);

        assert_eq!(c & flags::FIRST_TURN_DRAW, 0, "no draw on the first turn");
        assert_eq!(c & flags::ATTACK_FIRST_TURN, 0, "and no Battle Phase");
    }

    /// This project's configuration is not the reference's Goat mode, and
    /// the difference is large. Stated as a test so nobody has to take it
    /// on trust.
    #[test]
    fn the_reference_configuration_is_not_the_references_goat_mode() {
        assert_ne!(REFERENCE_CONFIGURATION, modes::GOAT);
        for flag in [
            flags::TCG_SEGOC_NONPUBLIC,
            flags::TCG_SEGOC_FIRSTTRIGGER,
            flags::SINGLE_CHAIN_IN_DAMAGE_SUBSTEP,
            flags::USE_TRAPS_IN_NEW_CHAIN,
            flags::SIX_STEP_BATTLE_STEP,
        ] {
            assert_ne!(modes::GOAT & flag, 0, "GOAT carries {flag:#x}");
            assert_eq!(
                REFERENCE_CONFIGURATION & flag,
                0,
                "and this project's configuration does not"
            );
        }
    }
}

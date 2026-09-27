//! The card pool: each card's script, translated.
//!
//! A card script in the reference is a Lua file defining
//! `initial_effect(c)`, which the core calls once when the card is created,
//! with `STATUS_INITIALIZING` set on the card for the duration. Here each
//! card is a Rust module exposing the same function, and this registry is
//! how a printed code finds it.
//!
//! ## Everything a card calls goes through `script_api`
//!
//! That is the one rule of this directory, and it is what
//! `docs/script-library.md` §7 decided: a translated card sees the
//! **script-facing** semantics — the ones ocgcore's Lua library installs
//! over the core's exports — and never the core's own methods. The two
//! differ (`Card.IsRelateToEffect` is the live example), and the port keeps
//! them apart by making this directory call only `script_api`.
//!
//! A grep enforces it, the way `cargo tree` enforces the no-engine
//! dependency: no module under `cards/` may name a `Field` method that is
//! not re-exported through `script_api`.

use crate::event::CardId;
use crate::field::Field;

pub mod airknight_parshath;
pub mod asura_priest;
pub mod aux_release_cost;
pub mod aux_select_unselect;
pub mod bls_envoy;
pub mod book_of_moon;
pub mod breaker;
pub mod call_of_the_haunted;
pub mod chaos_sorcerer;
pub mod creature_swap;
pub mod dark_balter;
pub mod dark_hole;
pub mod dark_mimic_lv1;
pub mod dd_warrior_lady;
pub mod dekoichi;
pub mod delinquent_duo;
pub mod dust_tornado;
pub mod enemy_controller;
pub mod fiend_skull_dragon;
pub mod gatling_dragon;
pub mod graceful_charity;
pub mod heavy_storm;
pub mod jinzo;
pub mod king_dragun;
pub mod magical_merchant;
pub mod magician_of_faith;
pub mod man_eater_bug;
pub mod metamorphosis;
pub mod mirror_force;
pub mod morphing_jar;
pub mod mystic_tomato;
pub mod mystical_space_typhoon;
pub mod nobleman_of_crossout;
pub mod pot_of_greed;
pub mod premature_burial;
pub mod proc_equip;
pub mod proc_fusion;
pub mod proc_spirit;
pub mod reaper_nightmare;
pub mod reinforcement_of_the_army;
pub mod ring_of_destruction;
pub mod ryu_senshi;
pub mod sakuretsu_armor;
pub mod sangan;
pub mod scapegoat;
pub mod sinister_serpent;
pub mod snatch_steal;
pub mod thousand_eyes;
pub mod threatening_roar;
pub mod torrential_tribute;
pub mod trap_dustshoot;
pub mod trap_hole;
pub mod tribe_infecting_virus;
pub mod tsukuyomi;
pub mod widespread_ruin;

/// Every card in the pool, one code each — the deck `tools/fuzz.py --deck
/// pool` deals and `examples/bench.rs` times. Built from the modules'
/// own `CODE` constants so it cannot drift from them; a test pins its
/// length to the module list above.
pub const POOL: [u32; 50] = [
    airknight_parshath::CODE,
    asura_priest::CODE,
    bls_envoy::CODE,
    book_of_moon::CODE,
    breaker::CODE,
    call_of_the_haunted::CODE,
    chaos_sorcerer::CODE,
    creature_swap::CODE,
    dark_balter::CODE,
    dark_hole::CODE,
    dark_mimic_lv1::CODE,
    dd_warrior_lady::CODE,
    dekoichi::CODE,
    delinquent_duo::CODE,
    dust_tornado::CODE,
    enemy_controller::CODE,
    fiend_skull_dragon::CODE,
    gatling_dragon::CODE,
    graceful_charity::CODE,
    heavy_storm::CODE,
    jinzo::CODE,
    king_dragun::CODE,
    magical_merchant::CODE,
    magician_of_faith::CODE,
    man_eater_bug::CODE,
    metamorphosis::CODE,
    mirror_force::CODE,
    morphing_jar::CODE,
    mystic_tomato::CODE,
    mystical_space_typhoon::CODE,
    nobleman_of_crossout::CODE,
    pot_of_greed::CODE,
    premature_burial::CODE,
    reaper_nightmare::CODE,
    reinforcement_of_the_army::CODE,
    ring_of_destruction::CODE,
    ryu_senshi::CODE,
    sakuretsu_armor::CODE,
    sangan::CODE,
    scapegoat::CODE,
    sinister_serpent::CODE,
    snatch_steal::CODE,
    thousand_eyes::CODE,
    threatening_roar::CODE,
    torrential_tribute::CODE,
    trap_dustshoot::CODE,
    trap_hole::CODE,
    tribe_infecting_virus::CODE,
    tsukuyomi::CODE,
    widespread_ruin::CODE,
];

/// A card's `initial_effect`.
pub type InitialEffect = fn(&mut Field, CardId);

/// The translated pool, by printed code.
/// The printed data of the translated cards, transcribed from the oracle's
/// table (`tools/oracle/carddata.py`) so the harness gives both engines
/// the same card.
///
/// A Spell or Trap needs only its type; a monster needs the whole line,
/// and every field of it is load-bearing — level decides what may summon
/// it, attack and defence decide battles, attribute and race are read by
/// half the pool's filters.
pub fn card_data(code: u32) -> Option<crate::card::CardData> {
    use crate::card::{attribute, card_type as ct, race};
    let plain = |code, type_| crate::card::CardData {
        code,
        type_,
        ..Default::default()
    };
    match code {
        morphing_jar::CODE => Some(crate::card::CardData {
            code,
            type_: ct::MONSTER | ct::EFFECT | ct::FLIP,
            level: 2,
            attack: 700,
            defense: 600,
            attribute: attribute::EARTH,
            race: race::ROCK,
            ..Default::default()
        }),
        bls_envoy::CODE => Some(crate::card::CardData {
            code,
            // **`TYPE_SPSUMMON` is on the printed line**, and it is what
            // stops the idle command offering a two-tribute Normal
            // Summon. Chaos Sorcerer's data does *not* carry it — that
            // card relies on `EnableReviveLimit`'s
            // `EFFECT_UNSUMMONABLE_CARD` alone.
            type_: ct::MONSTER | ct::EFFECT | ct::SPSUMMON,
            level: 8,
            attack: 3000,
            defense: 2500,
            attribute: attribute::LIGHT,
            race: race::WARRIOR,
            ..Default::default()
        }),
        enemy_controller::CODE => Some(plain(code, ct::SPELL | ct::QUICKPLAY)),
        scapegoat::CODE => Some(plain(code, ct::SPELL | ct::QUICKPLAY)),
        metamorphosis::CODE => Some(plain(code, ct::SPELL)),
        king_dragun::CODE => Some(crate::card::CardData {
            code,
            type_: ct::MONSTER | ct::FUSION | ct::EFFECT,
            level: 7,
            attack: 2400,
            defense: 1100,
            attribute: attribute::DARK,
            race: race::DRAGON,
            ..Default::default()
        }),
        thousand_eyes::CODE => Some(crate::card::CardData {
            code,
            type_: ct::MONSTER | ct::FUSION | ct::EFFECT,
            level: 1,
            attack: 0,
            defense: 0,
            attribute: attribute::DARK,
            race: race::SPELLCASTER,
            ..Default::default()
        }),
        reaper_nightmare::CODE => Some(crate::card::CardData {
            code,
            type_: ct::MONSTER | ct::FUSION | ct::EFFECT,
            level: 5,
            attack: 800,
            defense: 600,
            attribute: attribute::DARK,
            race: race::ZOMBIE,
            ..Default::default()
        }),
        ryu_senshi::CODE => Some(crate::card::CardData {
            code,
            type_: ct::MONSTER | ct::FUSION | ct::EFFECT,
            level: 6,
            attack: 2000,
            defense: 1200,
            attribute: attribute::EARTH,
            race: race::WARRIOR,
            ..Default::default()
        }),
        dark_balter::CODE => Some(crate::card::CardData {
            code,
            type_: ct::MONSTER | ct::FUSION | ct::EFFECT,
            level: 5,
            attack: 2000,
            defense: 1200,
            attribute: attribute::DARK,
            race: race::FIEND,
            ..Default::default()
        }),
        fiend_skull_dragon::CODE => Some(crate::card::CardData {
            code,
            type_: ct::MONSTER | ct::FUSION | ct::EFFECT,
            level: 5,
            attack: 2000,
            defense: 1200,
            attribute: attribute::WIND,
            race: race::DRAGON,
            ..Default::default()
        }),
        gatling_dragon::CODE => Some(crate::card::CardData {
            code,
            type_: ct::MONSTER | ct::FUSION | ct::EFFECT,
            level: 8,
            attack: 2600,
            defense: 1200,
            attribute: attribute::DARK,
            race: race::MACHINE,
            ..Default::default()
        }),
        // **The Sheep Tokens have printed lines but no script.** ocgcore
        // reads them out of the card database like any other card, so
        // they belong in this table rather than in the card that makes
        // them.
        c if scapegoat::token_data(c).is_some() => scapegoat::token_data(c),
        chaos_sorcerer::CODE => Some(crate::card::CardData {
            code,
            type_: ct::MONSTER | ct::EFFECT,
            level: 6,
            attack: 2300,
            defense: 2000,
            attribute: attribute::DARK,
            race: race::SPELLCASTER,
            ..Default::default()
        }),
        dekoichi::CODE => Some(crate::card::CardData {
            code,
            type_: ct::MONSTER | ct::EFFECT | ct::FLIP,
            level: 4,
            attack: 1400,
            defense: 1000,
            attribute: attribute::DARK,
            race: race::MACHINE,
            ..Default::default()
        }),
        sinister_serpent::CODE => Some(crate::card::CardData {
            code,
            type_: ct::MONSTER | ct::EFFECT,
            level: 1,
            attack: 300,
            defense: 250,
            attribute: attribute::WATER,
            race: race::REPTILE,
            ..Default::default()
        }),
        tribe_infecting_virus::CODE => Some(crate::card::CardData {
            code,
            type_: ct::MONSTER | ct::EFFECT,
            level: 4,
            attack: 1600,
            defense: 1000,
            attribute: attribute::WATER,
            race: race::AQUA,
            ..Default::default()
        }),
        magical_merchant::CODE => Some(crate::card::CardData {
            code,
            type_: ct::MONSTER | ct::EFFECT | ct::FLIP,
            level: 1,
            attack: 200,
            defense: 700,
            attribute: attribute::LIGHT,
            race: race::INSECT,
            ..Default::default()
        }),
        mystic_tomato::CODE => Some(crate::card::CardData {
            code,
            type_: ct::MONSTER | ct::EFFECT,
            level: 4,
            attack: 1400,
            defense: 1100,
            attribute: attribute::DARK,
            race: race::PLANT,
            ..Default::default()
        }),
        sangan::CODE => Some(crate::card::CardData {
            code,
            type_: ct::MONSTER | ct::EFFECT,
            level: 3,
            attack: 1000,
            defense: 600,
            attribute: attribute::DARK,
            race: race::FIEND,
            ..Default::default()
        }),
        magician_of_faith::CODE => Some(crate::card::CardData {
            code,
            type_: ct::MONSTER | ct::EFFECT | ct::FLIP,
            level: 1,
            attack: 300,
            defense: 400,
            attribute: attribute::LIGHT,
            race: race::SPELLCASTER,
            ..Default::default()
        }),
        man_eater_bug::CODE => Some(crate::card::CardData {
            code,
            type_: ct::MONSTER | ct::EFFECT | ct::FLIP,
            level: 2,
            attack: 450,
            defense: 600,
            attribute: attribute::EARTH,
            race: race::INSECT,
            ..Default::default()
        }),
        dd_warrior_lady::CODE => Some(crate::card::CardData {
            code,
            type_: ct::MONSTER | ct::EFFECT,
            level: 4,
            attack: 1500,
            defense: 1600,
            attribute: attribute::LIGHT,
            race: race::WARRIOR,
            ..Default::default()
        }),
        airknight_parshath::CODE => Some(crate::card::CardData {
            code,
            type_: ct::MONSTER | ct::EFFECT,
            level: 5,
            attack: 1900,
            defense: 1400,
            attribute: attribute::LIGHT,
            race: race::FAIRY,
            ..Default::default()
        }),
        dark_mimic_lv1::CODE => Some(crate::card::CardData {
            code,
            type_: ct::MONSTER | ct::EFFECT | ct::FLIP,
            level: 1,
            attack: 100,
            defense: 1000,
            attribute: attribute::DARK,
            race: race::FIEND,
            ..Default::default()
        }),
        tsukuyomi::CODE => Some(crate::card::CardData {
            code,
            type_: ct::MONSTER | ct::EFFECT | ct::SPIRIT,
            level: 4,
            attack: 1100,
            defense: 1400,
            attribute: attribute::DARK,
            race: race::SPELLCASTER,
            ..Default::default()
        }),
        asura_priest::CODE => Some(crate::card::CardData {
            code,
            type_: ct::MONSTER | ct::EFFECT | ct::SPIRIT,
            level: 4,
            attack: 1700,
            defense: 1200,
            attribute: attribute::LIGHT,
            race: race::FAIRY,
            ..Default::default()
        }),
        jinzo::CODE => Some(crate::card::CardData {
            code,
            type_: ct::MONSTER | ct::EFFECT,
            level: 6,
            attack: 2400,
            defense: 1500,
            attribute: attribute::DARK,
            race: race::MACHINE,
            ..Default::default()
        }),
        breaker::CODE => Some(crate::card::CardData {
            code,
            type_: ct::MONSTER | ct::EFFECT,
            level: 4,
            attack: 1600,
            defense: 1000,
            attribute: attribute::DARK,
            race: race::SPELLCASTER,
            ..Default::default()
        }),
        pot_of_greed::CODE
        | dark_hole::CODE
        | heavy_storm::CODE
        | graceful_charity::CODE
        | reinforcement_of_the_army::CODE
        | nobleman_of_crossout::CODE
        | delinquent_duo::CODE
        | creature_swap::CODE => Some(plain(code, crate::card::card_type::SPELL)),
        // An **Equip** Spell (`carddata.py`). The subtype is what makes it
        // stay on the field attached to something rather than resolving
        // and going to the graveyard.
        premature_burial::CODE | snatch_steal::CODE => Some(plain(
            code,
            crate::card::card_type::SPELL | crate::card::card_type::EQUIP,
        )),
        // A Quick-Play, not a plain Spell (`carddata.py`). The subtype
        // decides when it may be activated, so it is load-bearing.
        mystical_space_typhoon::CODE | book_of_moon::CODE => Some(plain(
            code,
            crate::card::card_type::SPELL | crate::card::card_type::QUICKPLAY,
        )),
        torrential_tribute::CODE
        | mirror_force::CODE
        | threatening_roar::CODE
        | trap_hole::CODE
        | sakuretsu_armor::CODE
        | widespread_ruin::CODE
        | dust_tornado::CODE
        | trap_dustshoot::CODE
        | ring_of_destruction::CODE => Some(plain(code, crate::card::card_type::TRAP)),
        // A **Continuous** Trap (`carddata.py`): it stays on the field
        // after resolving, which is what its three tie-keeping effects
        // are for.
        call_of_the_haunted::CODE => Some(plain(
            code,
            crate::card::card_type::TRAP | crate::card::card_type::CONTINUOUS,
        )),
        _ => None,
    }
}

pub fn initial_effect_for(code: u32) -> Option<InitialEffect> {
    match code {
        pot_of_greed::CODE => Some(pot_of_greed::initial_effect),
        dark_hole::CODE => Some(dark_hole::initial_effect),
        torrential_tribute::CODE => Some(torrential_tribute::initial_effect),
        heavy_storm::CODE => Some(heavy_storm::initial_effect),
        mirror_force::CODE => Some(mirror_force::initial_effect),
        threatening_roar::CODE => Some(threatening_roar::initial_effect),
        airknight_parshath::CODE => Some(airknight_parshath::initial_effect),
        dd_warrior_lady::CODE => Some(dd_warrior_lady::initial_effect),
        trap_hole::CODE => Some(trap_hole::initial_effect),
        sakuretsu_armor::CODE => Some(sakuretsu_armor::initial_effect),
        graceful_charity::CODE => Some(graceful_charity::initial_effect),
        mystical_space_typhoon::CODE => Some(mystical_space_typhoon::initial_effect),
        man_eater_bug::CODE => Some(man_eater_bug::initial_effect),
        dekoichi::CODE => Some(dekoichi::initial_effect),
        book_of_moon::CODE => Some(book_of_moon::initial_effect),
        morphing_jar::CODE => Some(morphing_jar::initial_effect),
        widespread_ruin::CODE => Some(widespread_ruin::initial_effect),
        dust_tornado::CODE => Some(dust_tornado::initial_effect),
        magician_of_faith::CODE => Some(magician_of_faith::initial_effect),
        reinforcement_of_the_army::CODE => Some(reinforcement_of_the_army::initial_effect),
        sangan::CODE => Some(sangan::initial_effect),
        mystic_tomato::CODE => Some(mystic_tomato::initial_effect),
        nobleman_of_crossout::CODE => Some(nobleman_of_crossout::initial_effect),
        delinquent_duo::CODE => Some(delinquent_duo::initial_effect),
        trap_dustshoot::CODE => Some(trap_dustshoot::initial_effect),
        magical_merchant::CODE => Some(magical_merchant::initial_effect),
        tribe_infecting_virus::CODE => Some(tribe_infecting_virus::initial_effect),
        sinister_serpent::CODE => Some(sinister_serpent::initial_effect),
        ring_of_destruction::CODE => Some(ring_of_destruction::initial_effect),
        creature_swap::CODE => Some(creature_swap::initial_effect),
        premature_burial::CODE => Some(premature_burial::initial_effect),
        call_of_the_haunted::CODE => Some(call_of_the_haunted::initial_effect),
        breaker::CODE => Some(breaker::initial_effect),
        jinzo::CODE => Some(jinzo::initial_effect),
        asura_priest::CODE => Some(asura_priest::initial_effect),
        tsukuyomi::CODE => Some(tsukuyomi::initial_effect),
        dark_mimic_lv1::CODE => Some(dark_mimic_lv1::initial_effect),
        snatch_steal::CODE => Some(snatch_steal::initial_effect),
        chaos_sorcerer::CODE => Some(chaos_sorcerer::initial_effect),
        enemy_controller::CODE => Some(enemy_controller::initial_effect),
        scapegoat::CODE => Some(scapegoat::initial_effect),
        gatling_dragon::CODE => Some(gatling_dragon::initial_effect),
        fiend_skull_dragon::CODE => Some(fiend_skull_dragon::initial_effect),
        dark_balter::CODE => Some(dark_balter::initial_effect),
        ryu_senshi::CODE => Some(ryu_senshi::initial_effect),
        reaper_nightmare::CODE => Some(reaper_nightmare::initial_effect),
        thousand_eyes::CODE => Some(thousand_eyes::initial_effect),
        king_dragun::CODE => Some(king_dragun::initial_effect),
        metamorphosis::CODE => Some(metamorphosis::initial_effect),
        bls_envoy::CODE => Some(bls_envoy::initial_effect),
        _ => None,
    }
}

impl Field {
    /// Run a card's script, if it has one — `card::initial_effect` in the
    /// reference, minus the Lua.
    ///
    /// `STATUS_INITIALIZING` is set for the duration and cleared after:
    /// `add_card_effect` reads it to stamp `EFFECT_FLAG_INITIAL` on every
    /// effect registered from inside, which is how the core later tells a
    /// printed effect from a granted one.
    pub fn initialize_card(&mut self, card: CardId) {
        let code = self.cards[card].data.code;
        let Some(init) = initial_effect_for(code) else {
            return;
        };
        self.cards[card].set_status(crate::card::status::INITIALIZING, true);
        init(self, card);
        self.cards[card].set_status(crate::card::status::INITIALIZING, false);
    }
}

#[cfg(test)]
mod discipline {
    /// **A card module calls only `script_api`.** The port keeps the
    /// core's own methods and the script-facing ones apart — the library
    /// layer replaced some of the latter, and a card that reached past
    /// the API would see the wrong semantics. Enforced here the way the
    /// no-engine dependency is enforced by `cargo tree`: mechanically.
    #[test]
    fn card_modules_touch_the_field_only_through_the_api() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/cards");
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.file_name().is_some_and(|n| n == "mod.rs") {
                continue;
            }
            let src = std::fs::read_to_string(&path).unwrap();
            // Only the card's own code is held to the rule; its tests may
            // set a board up however they like.
            let body = src.split("#[cfg(test)]").next().unwrap_or("");
            for (i, line) in body.lines().enumerate() {
                let code = line.split("//").next().unwrap_or("");
                for forbidden in [
                    "f.core",
                    "f.cards[",
                    "f.players",
                    "f.effects",
                    "Field::",
                    "f.emplace",
                ] {
                    assert!(
                        !code.contains(forbidden),
                        "{}:{}: a card reaches past script_api ({forbidden})",
                        path.display(),
                        i + 1
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod pool_tests {
    use super::*;

    /// **`POOL` is the module list.** One entry per card module (the
    /// `aux_*` and `proc_*` library modules are not cards), no
    /// duplicates, and every entry has printed data and an initial
    /// effect. A card added without a `POOL` entry fails here.
    #[test]
    fn the_pool_is_every_card_module_once() {
        let modules = include_str!("mod.rs")
            .lines()
            .filter_map(|l| l.strip_prefix("pub mod ")?.strip_suffix(';'))
            .filter(|m| !m.starts_with("aux_") && !m.starts_with("proc_"))
            .count();
        assert_eq!(POOL.len(), modules, "one entry per card module");
        let mut seen = std::collections::HashSet::new();
        for code in POOL {
            assert!(seen.insert(code), "{code} is listed twice");
            assert!(card_data(code).is_some(), "{code} has no printed data");
            assert!(initial_effect_for(code).is_some(), "{code} has no script");
        }
    }
}

#[cfg(test)]
mod card_data_tests {
    use super::*;
    use crate::card::card_type;

    /// **The printed types match the oracle's table** (`carddata.py`):
    /// the two Spells and the Trap, and nothing for an untranslated code.
    #[test]
    fn the_printed_data_matches_the_oracles_table() {
        assert_eq!(
            card_data(pot_of_greed::CODE).unwrap().type_,
            card_type::SPELL
        );
        assert_eq!(card_data(dark_hole::CODE).unwrap().type_, card_type::SPELL);
        assert_eq!(
            card_data(graceful_charity::CODE).unwrap().type_,
            card_type::SPELL
        );
        assert_eq!(
            card_data(mystical_space_typhoon::CODE).unwrap().type_,
            card_type::SPELL | card_type::QUICKPLAY,
            "a Quick-Play, which decides when it may be activated"
        );
        assert_eq!(
            card_data(book_of_moon::CODE).unwrap().type_,
            card_type::SPELL | card_type::QUICKPLAY,
            "a Quick-Play too"
        );
        assert_eq!(
            card_data(heavy_storm::CODE).unwrap().type_,
            card_type::SPELL
        );
        assert_eq!(
            card_data(mirror_force::CODE).unwrap().type_,
            card_type::TRAP
        );
        assert_eq!(
            card_data(threatening_roar::CODE).unwrap().type_,
            card_type::TRAP
        );
        assert_eq!(card_data(trap_hole::CODE).unwrap().type_, card_type::TRAP);
        assert_eq!(
            card_data(sakuretsu_armor::CODE).unwrap().type_,
            card_type::TRAP
        );
        // The first monster: the whole printed line, not just a type.
        let ak = card_data(airknight_parshath::CODE).unwrap();
        assert_eq!(ak.type_, card_type::MONSTER | card_type::EFFECT);
        assert_eq!((ak.level, ak.attack, ak.defense), (5, 1900, 1400));
        assert_eq!(ak.attribute, crate::card::attribute::LIGHT);
        assert_eq!(ak.race, crate::card::race::FAIRY);
        let dd = card_data(dd_warrior_lady::CODE).unwrap();
        assert_eq!(dd.type_, card_type::MONSTER | card_type::EFFECT);
        assert_eq!((dd.level, dd.attack, dd.defense), (4, 1500, 1600));
        assert_eq!(dd.attribute, crate::card::attribute::LIGHT);
        assert_eq!(dd.race, crate::card::race::WARRIOR);
        let meb = card_data(man_eater_bug::CODE).unwrap();
        assert_eq!(
            meb.type_,
            card_type::MONSTER | card_type::EFFECT | card_type::FLIP,
            "a Flip monster, which is what makes its effect a flip effect"
        );
        assert_eq!((meb.level, meb.attack, meb.defense), (2, 450, 600));
        assert_eq!(meb.attribute, crate::card::attribute::EARTH);
        assert_eq!(meb.race, crate::card::race::INSECT);
        let dek = card_data(dekoichi::CODE).unwrap();
        assert_eq!(
            dek.type_,
            card_type::MONSTER | card_type::EFFECT | card_type::FLIP
        );
        assert_eq!((dek.level, dek.attack, dek.defense), (4, 1400, 1000));
        assert_eq!(dek.attribute, crate::card::attribute::DARK);
        assert_eq!(dek.race, crate::card::race::MACHINE);
        let ss = card_data(sinister_serpent::CODE).unwrap();
        assert_eq!(ss.type_, card_type::MONSTER | card_type::EFFECT);
        assert_eq!((ss.level, ss.attack, ss.defense), (1, 300, 250));
        assert_eq!(ss.attribute, crate::card::attribute::WATER);
        assert_eq!(ss.race, crate::card::race::REPTILE);
        let tiv = card_data(tribe_infecting_virus::CODE).unwrap();
        assert_eq!(tiv.type_, card_type::MONSTER | card_type::EFFECT);
        assert_eq!((tiv.level, tiv.attack, tiv.defense), (4, 1600, 1000));
        assert_eq!(tiv.attribute, crate::card::attribute::WATER);
        assert_eq!(tiv.race, crate::card::race::AQUA);
        let mm = card_data(magical_merchant::CODE).unwrap();
        assert_eq!(
            mm.type_,
            card_type::MONSTER | card_type::EFFECT | card_type::FLIP
        );
        assert_eq!((mm.level, mm.attack, mm.defense), (1, 200, 700));
        assert_eq!(mm.attribute, crate::card::attribute::LIGHT);
        assert_eq!(mm.race, crate::card::race::INSECT);
        let mt = card_data(mystic_tomato::CODE).unwrap();
        assert_eq!(mt.type_, card_type::MONSTER | card_type::EFFECT);
        assert_eq!((mt.level, mt.attack, mt.defense), (4, 1400, 1100));
        assert_eq!(mt.attribute, crate::card::attribute::DARK);
        assert_eq!(mt.race, crate::card::race::PLANT);
        let san = card_data(sangan::CODE).unwrap();
        assert_eq!(san.type_, card_type::MONSTER | card_type::EFFECT);
        assert_eq!((san.level, san.attack, san.defense), (3, 1000, 600));
        assert_eq!(san.attribute, crate::card::attribute::DARK);
        assert_eq!(san.race, crate::card::race::FIEND);
        let mof = card_data(magician_of_faith::CODE).unwrap();
        assert_eq!(
            mof.type_,
            card_type::MONSTER | card_type::EFFECT | card_type::FLIP
        );
        assert_eq!((mof.level, mof.attack, mof.defense), (1, 300, 400));
        assert_eq!(mof.attribute, crate::card::attribute::LIGHT);
        assert_eq!(mof.race, crate::card::race::SPELLCASTER);
        let mj = card_data(morphing_jar::CODE).unwrap();
        assert_eq!(
            mj.type_,
            card_type::MONSTER | card_type::EFFECT | card_type::FLIP
        );
        assert_eq!((mj.level, mj.attack, mj.defense), (2, 700, 600));
        assert_eq!(mj.race, crate::card::race::ROCK);
        assert_eq!(
            card_data(torrential_tribute::CODE).unwrap().type_,
            card_type::TRAP
        );
        assert_eq!(
            card_data(widespread_ruin::CODE).unwrap().type_,
            card_type::TRAP
        );
        // The plain Spells, pinned the same way. Reinforcement of the
        // Army's pin was missed when it was ported and is added here.
        assert_eq!(
            card_data(reinforcement_of_the_army::CODE).unwrap().type_,
            card_type::SPELL
        );
        assert_eq!(
            card_data(nobleman_of_crossout::CODE).unwrap().type_,
            card_type::SPELL
        );
        assert_eq!(
            card_data(delinquent_duo::CODE).unwrap().type_,
            card_type::SPELL
        );
        assert_eq!(
            card_data(trap_dustshoot::CODE).unwrap().type_,
            card_type::TRAP
        );
        assert_eq!(
            card_data(ring_of_destruction::CODE).unwrap().type_,
            card_type::TRAP
        );
        assert_eq!(
            card_data(creature_swap::CODE).unwrap().type_,
            card_type::SPELL
        );
        // An **Equip** Spell, not a plain one: the subtype is what makes
        // it stay on the field attached to something.
        assert_eq!(
            card_data(premature_burial::CODE).unwrap().type_,
            card_type::SPELL | card_type::EQUIP
        );
        assert_eq!(
            card_data(call_of_the_haunted::CODE).unwrap().type_,
            card_type::TRAP | card_type::CONTINUOUS
        );
        assert_eq!(
            card_data(snatch_steal::CODE).unwrap().type_,
            card_type::SPELL | card_type::EQUIP
        );
        // Breaker's whole printed line, because three of the five decide
        // things: the level gates the tribute, the attack is what the
        // counter adds to, and the race is what a Spellcaster support
        // card would read.
        // `TYPE_FLIP` is load-bearing: it is what `Nobleman of Crossout`
        // reads to decide whether to sweep both decks.
        let mimic = card_data(dark_mimic_lv1::CODE).unwrap();
        assert_eq!(
            mimic.type_,
            card_type::MONSTER | card_type::EFFECT | card_type::FLIP
        );
        assert_eq!(mimic.level, 1);
        assert_eq!(mimic.attack, 100);
        assert_eq!(mimic.defense, 1000);
        assert_eq!(mimic.attribute, crate::card::attribute::DARK);
        assert_eq!(mimic.race, crate::card::race::FIEND);
        let tsuku = card_data(tsukuyomi::CODE).unwrap();
        assert_eq!(
            tsuku.type_,
            card_type::MONSTER | card_type::EFFECT | card_type::SPIRIT
        );
        assert_eq!(tsuku.level, 4);
        assert_eq!(tsuku.attack, 1100);
        assert_eq!(tsuku.defense, 1400);
        assert_eq!(tsuku.attribute, crate::card::attribute::DARK);
        assert_eq!(tsuku.race, crate::card::race::SPELLCASTER);
        // The pool's first **spirit**, and `TYPE_SPIRIT` is load-bearing
        // rather than decorative: it is what the rules read to stop the
        // card being Special Summoned or Set from the field.
        let asura = card_data(asura_priest::CODE).unwrap();
        assert_eq!(
            asura.type_,
            card_type::MONSTER | card_type::EFFECT | card_type::SPIRIT
        );
        assert_eq!(asura.level, 4);
        assert_eq!(asura.attack, 1700);
        assert_eq!(asura.defense, 1200);
        assert_eq!(asura.attribute, crate::card::attribute::LIGHT);
        assert_eq!(asura.race, crate::card::race::FAIRY);
        // Jinzo's level is the thing that decides: it needs two
        // tributes, which is most of what makes it a slow card.
        let jinzo = card_data(jinzo::CODE).unwrap();
        assert_eq!(jinzo.type_, card_type::MONSTER | card_type::EFFECT);
        assert_eq!(jinzo.level, 6);
        assert_eq!(jinzo.attack, 2400);
        assert_eq!(jinzo.defense, 1500);
        assert_eq!(jinzo.attribute, crate::card::attribute::DARK);
        assert_eq!(jinzo.race, crate::card::race::MACHINE);
        let breaker = card_data(breaker::CODE).unwrap();
        assert_eq!(breaker.type_, card_type::MONSTER | card_type::EFFECT);
        assert_eq!(breaker.level, 4);
        assert_eq!(breaker.attack, 1600);
        assert_eq!(breaker.defense, 1000);
        assert_eq!(breaker.attribute, crate::card::attribute::DARK);
        assert_eq!(breaker.race, crate::card::race::SPELLCASTER);
        assert_eq!(
            card_data(dust_tornado::CODE).unwrap().type_,
            card_type::TRAP
        );
        assert!(
            card_data(5_053_103).is_none(),
            "the vanilla is the example's, not the table's"
        );
    }
}

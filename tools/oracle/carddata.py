"""Real card data for the scoped pool, in ocgcore's card_data terms.

Race/attribute are the printed TCG values;
they do not affect any interaction in this pool but are recorded correctly.
"""

from __future__ import annotations

from dataclasses import dataclass

from tools.oracle import protocol as p


@dataclass(frozen=True)
class OcgCard:
    code: int
    type: int
    level: int
    attack: int
    defense: int
    attribute: int
    race: int


_NORMAL = p.TYPE_MONSTER | p.TYPE_NORMAL

CARDS: dict[int, OcgCard] = {
    c.code: c
    for c in (
        OcgCard(13039848, _NORMAL, 3, 1300, 2000, p.ATTRIBUTE_EARTH, p.RACE_ROCK),
        OcgCard(15025844, _NORMAL, 4, 800, 2000, p.ATTRIBUTE_LIGHT, p.RACE_SPELLCASTER),
        OcgCard(43096270, _NORMAL, 4, 2000, 100, p.ATTRIBUTE_LIGHT, p.RACE_DRAGON),
        OcgCard(69247929, _NORMAL, 4, 2000, 100, p.ATTRIBUTE_EARTH, p.RACE_BEASTWARRIOR),
        OcgCard(97590747, _NORMAL, 4, 1800, 1000, p.ATTRIBUTE_DARK, p.RACE_FIEND),
        OcgCard(5053103, _NORMAL, 4, 1700, 1000, p.ATTRIBUTE_EARTH, p.RACE_BEASTWARRIOR),
        OcgCard(67284908, _NORMAL, 5, 0, 3000, p.ATTRIBUTE_EARTH, p.RACE_ROCK),
        OcgCard(70781052, _NORMAL, 6, 2500, 1200, p.ATTRIBUTE_DARK, p.RACE_FIEND),
        OcgCard(6631034, _NORMAL, 6, 2600, 1700, p.ATTRIBUTE_WATER, p.RACE_DINOSAUR),
        OcgCard(46986414, _NORMAL, 7, 2500, 2100, p.ATTRIBUTE_DARK, p.RACE_SPELLCASTER),
        OcgCard(89631139, _NORMAL, 8, 3000, 2500, p.ATTRIBUTE_LIGHT, p.RACE_DRAGON),
        OcgCard(
            54652250,
            p.TYPE_MONSTER | p.TYPE_EFFECT | p.TYPE_FLIP,
            2,
            450,
            600,
            p.ATTRIBUTE_EARTH,
            p.RACE_INSECT,
        ),
        OcgCard(14087893, p.TYPE_SPELL | p.TYPE_QUICKPLAY, 0, 0, 0, 0, 0),
        OcgCard(53129443, p.TYPE_SPELL, 0, 0, 0, 0, 0),
        OcgCard(55144522, p.TYPE_SPELL, 0, 0, 0, 0, 0),
        OcgCard(56120475, p.TYPE_TRAP, 0, 0, 0, 0, 0),
        OcgCard(4206964, p.TYPE_TRAP, 0, 0, 0, 0, 0),
        OcgCard(5318639, p.TYPE_SPELL | p.TYPE_QUICKPLAY, 0, 0, 0, 0, 0),
        OcgCard(19613556, p.TYPE_SPELL, 0, 0, 0, 0, 0),
        OcgCard(44095762, p.TYPE_TRAP, 0, 0, 0, 0, 0),
        OcgCard(53582587, p.TYPE_TRAP, 0, 0, 0, 0, 0),
        OcgCard(36361633, p.TYPE_TRAP, 0, 0, 0, 0, 0),
        OcgCard(
            87621407,
            p.TYPE_MONSTER | p.TYPE_EFFECT | p.TYPE_FLIP,
            4,
            1400,
            1000,
            p.ATTRIBUTE_DARK,
            p.RACE_MACHINE,
        ),
        OcgCard(
            31560081,
            p.TYPE_MONSTER | p.TYPE_EFFECT | p.TYPE_FLIP,
            1,
            300,
            400,
            p.ATTRIBUTE_LIGHT,
            p.RACE_SPELLCASTER,
        ),
        OcgCard(  # Magical Merchant
            32362575,
            p.TYPE_MONSTER | p.TYPE_EFFECT | p.TYPE_FLIP,
            1,
            200,
            700,
            p.ATTRIBUTE_LIGHT,
            p.RACE_INSECT,
        ),
        OcgCard(
            18036057,
            p.TYPE_MONSTER | p.TYPE_EFFECT,
            5,
            1900,
            1400,
            p.ATTRIBUTE_LIGHT,
            p.RACE_FAIRY,
        ),
        OcgCard(
            7572887,
            p.TYPE_MONSTER | p.TYPE_EFFECT,
            4,
            1500,
            1600,
            p.ATTRIBUTE_LIGHT,
            p.RACE_WARRIOR,
        ),
        OcgCard(79571449, p.TYPE_SPELL, 0, 0, 0, 0, 0),
        OcgCard(60082869, p.TYPE_TRAP, 0, 0, 0, 0, 0),
        OcgCard(77754944, p.TYPE_TRAP, 0, 0, 0, 0, 0),
        OcgCard(
            33508719,
            p.TYPE_MONSTER | p.TYPE_EFFECT | p.TYPE_FLIP,
            2,
            700,
            600,
            p.ATTRIBUTE_EARTH,
            p.RACE_ROCK,
        ),
        OcgCard(
            26202165,
            p.TYPE_MONSTER | p.TYPE_EFFECT,
            3,
            1000,
            600,
            p.ATTRIBUTE_DARK,
            p.RACE_FIEND,
        ),
        OcgCard(
            83011278,
            p.TYPE_MONSTER | p.TYPE_EFFECT,
            4,
            1400,
            1100,
            p.ATTRIBUTE_DARK,
            p.RACE_PLANT,
        ),
        OcgCard(
            2134346,
            p.TYPE_MONSTER | p.TYPE_EFFECT | p.TYPE_SPIRIT,
            4,
            1700,
            1200,
            p.ATTRIBUTE_LIGHT,
            p.RACE_FAIRY,
        ),
        OcgCard(
            34853266,
            p.TYPE_MONSTER | p.TYPE_EFFECT | p.TYPE_SPIRIT,
            4,
            1100,
            1400,
            p.ATTRIBUTE_DARK,
            p.RACE_SPELLCASTER,
        ),
        OcgCard(
            74713516,
            p.TYPE_MONSTER | p.TYPE_EFFECT | p.TYPE_FLIP,
            1,
            100,
            1000,
            p.ATTRIBUTE_DARK,
            p.RACE_FIEND,
        ),
        OcgCard(44763025, p.TYPE_SPELL, 0, 0, 0, 0, 0),
        OcgCard(64697231, p.TYPE_TRAP, 0, 0, 0, 0, 0),
        OcgCard(71044499, p.TYPE_SPELL, 0, 0, 0, 0, 0),
        OcgCard(70828912, p.TYPE_SPELL | p.TYPE_EQUIP, 0, 0, 0, 0, 0),
        OcgCard(45986603, p.TYPE_SPELL | p.TYPE_EQUIP, 0, 0, 0, 0, 0),
        OcgCard(
            71413901,
            p.TYPE_MONSTER | p.TYPE_EFFECT,
            4,
            1600,
            1000,
            p.ATTRIBUTE_DARK,
            p.RACE_SPELLCASTER,
        ),
        OcgCard(
            33184167, p.TYPE_MONSTER | p.TYPE_EFFECT, 4, 1600, 1000, p.ATTRIBUTE_WATER, p.RACE_AQUA
        ),
        OcgCard(83555666, p.TYPE_TRAP, 0, 0, 0, 0, 0),
        OcgCard(
            8131171,
            p.TYPE_MONSTER | p.TYPE_EFFECT,
            1,
            300,
            250,
            p.ATTRIBUTE_WATER,
            p.RACE_REPTILE,
        ),
        OcgCard(97077563, p.TYPE_TRAP | p.TYPE_CONTINUOUS, 0, 0, 0, 0, 0),
        OcgCard(73915051, p.TYPE_SPELL | p.TYPE_QUICKPLAY, 0, 0, 0, 0, 0),
        OcgCard(46411259, p.TYPE_SPELL, 0, 0, 0, 0, 0),
        OcgCard(
            72989439,
            # Cannot be Normal Summoned/Set: the cdb's type carries
            # TYPE_SPSUMMON, without which ocgcore's idle command offered a
            # two-tribute Normal Summon (blind triage).
            p.TYPE_MONSTER | p.TYPE_EFFECT | p.TYPE_SPSUMMON,
            8,
            3000,
            2500,
            p.ATTRIBUTE_LIGHT,
            p.RACE_WARRIOR,
        ),
        OcgCard(
            9596126,
            p.TYPE_MONSTER | p.TYPE_EFFECT,
            6,
            2300,
            2000,
            p.ATTRIBUTE_DARK,
            p.RACE_SPELLCASTER,
        ),
        OcgCard(
            63519819,
            p.TYPE_MONSTER | p.TYPE_FUSION | p.TYPE_EFFECT,
            1,
            0,
            0,
            p.ATTRIBUTE_DARK,
            p.RACE_SPELLCASTER,
        ),
        # Batch 4 (wave 4d): the negation fusions.
        OcgCard(
            80071763,
            p.TYPE_MONSTER | p.TYPE_FUSION | p.TYPE_EFFECT,
            5,
            2000,
            1200,
            p.ATTRIBUTE_DARK,
            p.RACE_FIEND,
        ),
        OcgCard(
            49868263,
            p.TYPE_MONSTER | p.TYPE_FUSION | p.TYPE_EFFECT,
            6,
            2000,
            1200,
            p.ATTRIBUTE_EARTH,
            p.RACE_WARRIOR,
        ),
        OcgCard(
            66235877,
            p.TYPE_MONSTER | p.TYPE_FUSION | p.TYPE_EFFECT,
            5,
            2000,
            1200,
            p.ATTRIBUTE_WIND,
            p.RACE_DRAGON,
        ),
        # Batch 4 (wave 4e): the dragon package.
        OcgCard(
            13756293,
            p.TYPE_MONSTER | p.TYPE_FUSION | p.TYPE_EFFECT,
            7,
            2400,
            1100,
            p.ATTRIBUTE_DARK,
            p.RACE_DRAGON,
        ),
        OcgCard(
            87751584,
            p.TYPE_MONSTER | p.TYPE_FUSION | p.TYPE_EFFECT,
            8,
            2600,
            1200,
            p.ATTRIBUTE_DARK,
            p.RACE_MACHINE,
        ),
        OcgCard(
            85684223,
            p.TYPE_MONSTER | p.TYPE_FUSION | p.TYPE_EFFECT,
            5,
            800,
            600,
            p.ATTRIBUTE_DARK,
            p.RACE_ZOMBIE,
        ),
        # Batch 4 (wave 4f): the control tech.
        OcgCard(98045062, p.TYPE_SPELL | p.TYPE_QUICKPLAY, 0, 0, 0, 0, 0),
        OcgCard(31036355, p.TYPE_SPELL, 0, 0, 0, 0, 0),
        # Goat batch 5, wave 5a.
        OcgCard(
            77585513,
            p.TYPE_MONSTER | p.TYPE_EFFECT,
            6,
            2400,
            1500,
            p.ATTRIBUTE_DARK,
            p.RACE_MACHINE,
        ),
        OcgCard(32807846, p.TYPE_SPELL, 0, 0, 0, 0, 0),
        # Sheep Tokens: ocgcore's Scapegoat creates codes 73915052..55 through
        # the card reader (no script).
        *(
            OcgCard(
                73915051 + i,
                p.TYPE_MONSTER | p.TYPE_NORMAL | p.TYPE_TOKEN,
                1,
                0,
                0,
                p.ATTRIBUTE_EARTH,
                p.RACE_BEAST,
            )
            for i in (1, 2, 3, 4)
        ),
    )
}

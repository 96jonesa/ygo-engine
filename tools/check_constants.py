#!/usr/bin/env python3
"""Diff the port's transcribed constant tables against ocgcore's headers.

The rule this enforces is in ``docs/processor-loop.md``: transcribe constant
tables from the reference, transcribe them *whole*, and pin literals in a
test. The rule was written after three tables were wrong, and a fourth
(``EFFECT_TYPE_``) then turned out to have been transcribed as far as
``GRANT`` and stopped — so "I followed the rule" is not evidence, and this
is what evidence looks like.

It reports a table that is missing an entry, or that has one whose value
differs. It cannot run in CI: ocgcore's checkout is gitignored (``build.sh``
fetches it), and the port crate deliberately has no build-time dependency on
it. Run it by hand after ``tools/oracle/build.sh``, and whenever a constant
table is touched.

    python3 tools/check_constants.py
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "tools/oracle/build/src/ygopro-core"
LUA = ROOT / "tools/oracle/build/cardscripts"
RS = ROOT / "src"

# (C prefix, header, rust file, rust module)
TABLES = [
    ("STATUS_", "ocgapi_constants.h", "card.rs", "status"),
    ("REASON_", "ocgapi_constants.h", "card.rs", "reason"),
    ("TYPE_", "ocgapi_constants.h", "card.rs", "card_type"),
    ("ATTRIBUTE_", "ocgapi_constants.h", "card.rs", "attribute"),
    ("RACE_", "ocgapi_constants.h", "card.rs", "race"),
    ("EFFECT_TYPE_", "effect_constants.h", "effect.rs", "effect_type"),
    ("EFFECT_FLAG_", "effect_constants.h", "effect.rs", "flag"),
    ("EFFECT_FLAG2_", "effect_constants.h", "effect.rs", "flag2"),
    ("EFFECT_COUNT_CODE_", "effect_constants.h", "effect.rs", "effect_count"),
    ("DUEL_", "ocgapi_constants.h", "duel.rs", "flags"),
    ("LOCATION_", "ocgapi_constants.h", "board.rs", "location"),
    ("POS_", "ocgapi_constants.h", "board.rs", "position"),
    ("TIMING_", "field.h", "field.rs", "timing"),
    ("RESET_", "effect_constants.h", "field.rs", "reset"),
    ("CHAIN_", "field.h", "chain.rs", "chain_flag"),
    ("GLOBALFLAG_", "field.h", "field.rs", "global_flag"),
    ("PHASE_", "ocgapi_constants.h", "duel.rs", "phases"),
]

# Tables that live in the script library rather than in a C header. The
# card-facing constants are Lua's, and a card that names one names it as the
# script does.
#
# (Lua prefix, lua file, rust file, rust module)
LUA_TABLES = [
    ("HINTMSG_", "constant.lua", "host_question.rs", "hintmsg"),
    ("COUNTER_", "card_counter_constants.lua", "counters.rs", "counter_type"),
]

# Script-library tables whose entries are **expressions** over other
# constants rather than plain integers. Same idea as LUA_TABLES, but the
# reference side has to be evaluated: the point of pinning a composite is
# that a wrong constituent is invisible in the result.
#
# (Lua prefix, lua file, rust file, rust module)
LUA_COMPOSITES = [
    ("RESETS_", "constant.lua", "field.rs", "resets"),
    ("TIMINGS_", "constant.lua", "field.rs", "timings"),
]


def lua_constants(filename: str, prefix: str) -> dict[str, int]:
    """Every `PREFIX_NAME = <int>` assignment in a script-library file.

    Decimal **or** hex: the hint table is written in decimal and the
    counter table in hex, and a reader that takes only one silently
    returns nothing for the other — which `check_tables` then reports as a
    clean zero-entry pass. See the guard there.
    """
    text = (LUA / filename).read_text()
    return {
        m.group(1): int(m.group(2), 0)
        for m in re.finditer(
            r"^" + prefix + r"(\w+)\s*=\s*(0x[0-9a-fA-F]+|\d+)\s*$", text, re.M
        )
    }


def lua_composites(filename: str, prefix: str) -> dict[str, int]:
    """Every `PREFIX_NAME = <bitwise expression>` in a script-library file.

    The right-hand sides are `|`, `&`, `~` over names defined earlier in
    the same file, which is also valid Python once the names are bound —
    Lua's `~` is a 32-bit complement and Python's is arbitrary-precision,
    but every use here is `a & ~b` with `a` non-negative, where the two
    agree.

    Entries are evaluated **in file order** so a composite may name an
    earlier composite, which `RESETS_STANDARD_DISABLE` does.
    """
    text = (LUA / filename).read_text()
    env: dict[str, int] = {}
    for m in re.finditer(r"^(\w+)\s*=\s*([\w|&~() ]+?)\s*(?:--.*)?$", text, re.M):
        name, expr = m.group(1), m.group(2)
        try:
            env[name] = int(expr, 0)
            continue
        except ValueError:
            pass
        try:
            env[name] = eval(expr, {"__builtins__": {}}, env)
        except (NameError, SyntaxError, TypeError):
            continue
    return {
        name[len(prefix):]: value
        for name, value in env.items()
        if name.startswith(prefix) and isinstance(value, int)
    }


# Where a C name is not a legal Rust identifier, or reads badly as one.
ALIASES = {
    "1_FACEUP_FIELD": "ONE_FACEUP_FIELD",
    "1ST_TURN_DRAW": "FIRST_TURN_DRAW",
    "6_STEP_BATLLE_STEP": "SIX_STEP_BATTLE_STEP",
    "3_COLUMNS_FIELD": "THREE_COLUMNS_FIELD",
    "0_ATK_DESTROYED": "ZERO_ATK_DESTROYED",
}

# Composites, not table entries: they are defined from other entries and the
# crate asserts their composition in its own tests instead.
SKIP = re.compile(r"^(MODE_|MR\d|ALL$|MAX$)")


def all_reference_constants() -> dict[str, tuple[int, str]]:
    """Every #define and enum constant in every header, by name.

    Used for the tables that are not a single prefix in a single header:
    the `code` module mixes `EVENT_*` and `EFFECT_*`, the symbolic locations
    live in `common.h` rather than `ocgapi_constants.h`, and a handful of
    constants are one-offs.
    """
    found: dict[str, tuple[int, str]] = {}
    for header in sorted(SRC.glob("*.h")):
        text = header.read_text()
        patterns = [
            # The trailing `(?://.*)?` matters: several of ocgcore's
            # `#define`s carry an explanatory comment on the same line
            # (`#define LOCATION_DECKBOT 0x10001  //Return to deck bottom`).
            # Anchoring at `$` without it makes every such constant invisible
            # to this lookup, and an unchecked constant is exactly what this
            # tool exists to prevent.
            r"^[^/\n]*#define\s+(\w+)\s+(0x[0-9a-fA-F]+|\d+)\s*(?://.*)?$",
            r"^\s*(\w+)\s*=\s*(0x[0-9a-fA-F]+|\d+)\s*,",
        ]
        for pattern in patterns:
            for m in re.finditer(pattern, text, re.M):
                found.setdefault(m.group(1), (parse_int(m.group(2)), header.name))
    return found


def parse_int(text: str) -> int:
    text = text.strip()
    return int(text, 16) if text.lower().startswith("0x") else int(text, 10)


def c_constants(header: str, prefix: str) -> dict[str, int]:
    text = (SRC / header).read_text()
    found: dict[str, int] = {}
    # `^[^/\n]*` keeps a commented-out definition from matching. ocgcore
    # has several — `//#define CHAIN_DECK_EFFECT`, `//#define
    # EFFECT_STATUS_ACTIVATED` — and reporting them as "not ported" would
    # train the reader to ignore the tool's output, which is worse than not
    # having it.
    patterns = [
        r"^[^/\n]*#define\s+" + prefix + r"(\w+)\s+(0x[0-9a-fA-F]+|\d+)",
        r"^\s*" + prefix + r"(\w+)\s*=\s*(0x[0-9a-fA-F]+|\d+)\s*,",
    ]
    for pattern in patterns:
        for m in re.finditer(pattern, text, re.M):
            if not SKIP.match(m.group(1)):
                found[m.group(1)] = int(m.group(2), 0)
    return found


def rust_constants(filename: str, module: str) -> dict[str, int]:
    text = (RS / filename).read_text()
    m = re.search(r"pub mod " + module + r" \{(.*?)\n\}", text, re.S)
    if not m:
        return {}
    # `[0-9a-fA-F_]` and the `_` strip: Rust allows `0x1000_0000`, and a
    # pattern that stops at the underscore reads that as `0x1000` — then
    # reports a WRONG VALUE against a constant that is in fact right, or
    # worse, agrees with a reference value that happens to match the prefix.
    return {
        c.group(1): int(c.group(2).replace("_", ""), 0)
        for c in re.finditer(
            r"pub const (\w+): u\d+ = (0x[0-9a-fA-F_]+|[\d_]+)", m.group(1)
        )
    }


def rust_expr_constants(filename: str, module: str) -> dict[str, int]:
    """Like `rust_constants`, but for a module whose entries are bitwise
    **expressions** over other constants.

    `reset::TOFIELD | reset::LEAVE` is not a literal, so the plain reader
    sees nothing at all and reports every entry as missing — which looks
    like a real gap and is not. This one resolves `path::NAME` against the
    module the path names and then evaluates, so the composite is pinned
    to the same standard as a literal.
    """
    text = (RS / filename).read_text()
    m = re.search(r"pub mod " + module + r" \{(.*?)\n\}", text, re.S)
    if not m:
        return {}
    env: dict[str, int] = {}
    out: dict[str, int] = {}
    for c in re.finditer(
        r"pub const (\w+): u\d+ =\s*([^;]+);", m.group(1), re.S
    ):
        name, expr = c.group(1), " ".join(c.group(2).split())
        # `reset::TOFIELD` -> its value; `phases::END as u32` -> its value.
        def resolve(match: re.Match[str]) -> str:
            mod, const = match.group(1), match.group(2)
            for rust_file in ("field.rs", "duel.rs", "card.rs", "effect.rs", "board.rs"):
                table = rust_constants(rust_file, mod)
                if const in table:
                    return str(table[const])
            raise KeyError(f"{mod}::{const}")
        try:
            expr = re.sub(r"(\w+)::(\w+)", resolve, expr)
        except KeyError:
            continue
        # Rust spells bitwise complement `!`, Python `~`; and `as u32`
        # is a cast the expression does not need once it is integers.
        expr = expr.replace(" as u32", "").replace("!", "~")
        try:
            out[name] = eval(expr, {"__builtins__": {}}, env) & 0xFFFF_FFFF
        except (NameError, SyntaxError, TypeError):
            continue
        env[name] = out[name]
    return out


# Modules whose names map to more than one reference prefix, or to a prefix
# that is not the module's own. The tuple is tried in order.
MIXED = [
    ("event.rs", "code", lambda n: (f"EVENT_{n}", f"EFFECT_{n}")),
    ("card.rs", "assume", lambda n: (f"ASSUME_{n}",)),
    ("card.rs", "summon_type", lambda n: (f"SUMMON_TYPE_{n}",)),
    ("point_event.rs", "effect_status", lambda n: (f"EFFECT_STATUS_{n}",)),
    # `location` is checked by prefix too, but only against
    # ocgapi_constants.h; the symbolic ones are in common.h.
    ("board.rs", "location", lambda n: (f"LOCATION_{n}",)),
    # `redirect` holds the two `LOCATION_*` values that are a zone plus an
    # instruction (`DECKBOT`, `DECKSHF`). They are kept out of `location`
    # because they do not fit its `u8`, but they are the reference's
    # `LOCATION_` names and are checked as such.
    ("board.rs", "redirect", lambda n: (f"LOCATION_{n}",)),
    # The host-question hint kinds. Transcribed whole when `AnnounceRace`
    # needed one of them, so the whole table is worth pinning.
    ("host_question.rs", "hint", lambda n: (f"HINT_{n}",)),
]

# The single name that exists under BOTH the EVENT_ and EFFECT_ prefixes in
# the whole reference. `code`'s lookup tries EVENT_ first, so without this it
# would compare the effect constant against the event's value and call it
# wrong. Listed rather than special-cased in the lookup so that a second
# collision appearing upstream is a visible addition here.
COLLISIONS = {
    ("code", "ATTACK_DISABLED"): "EFFECT_ATTACK_DISABLED",
    ("code", "ATTACK_DISABLED_EVENT"): "EVENT_ATTACK_DISABLED",
}

# One-off constants that are not part of any table.
# A **card number** the library uses as an effect code, pinned against the
# literal in the script that uses it. There is no named constant on either
# side to compare, and a wrong digit here fails silently: the filter simply
# never finds the card and the branch never fires.
LUA_LITERALS = [
    (
        "cards_specific_functions.lua",
        "Auxiliary.SpElimFilter",
        r"Duel\.IsPlayerAffectedByEffect\(c:GetControler\(\),(\d+)\)",
        "script_api.rs",
        "SPIRIT_ELIMINATION",
    ),
    (
        "cards_specific_functions.lua",
        "Auxiliary.DoubleSnareValidity",
        r"eff:SetCode\((\d+)\)",
        "script_api.rs",
        "CARD_DOUBLE_SNARE",
    ),
    # A **Life Point cost**, which a wrong digit would hide behind a
    # perfectly working payment.
    (
        "c80071763.lua",
        "s.initial_effect",
        r"Cost\.PayLP\((\d+)\)",
        "cards/dark_balter.rs",
        "COST_LP",
    ),
    # **Fusion material card numbers.** Nothing in this pool performs a
    # Fusion Summon, so the lists are never read and a mistyped digit
    # stays self-consistent forever — exactly the shape that has to be
    # pinned rather than tested.
    (
        "c80071763.lua",
        "s.initial_effect",
        r"Fusion\.AddProcMix\(c,false,false,(\d+),\d+\)",
        "cards/dark_balter.rs",
        "MATERIAL_A",
    ),
    (
        "c80071763.lua",
        "s.initial_effect",
        r"Fusion\.AddProcMix\(c,false,false,\d+,(\d+)\)",
        "cards/dark_balter.rs",
        "MATERIAL_B",
    ),
    (
        "c66235877.lua",
        "s.initial_effect",
        r"Fusion\.AddProcMix\(c,false,false,(\d+),\d+\)",
        "cards/fiend_skull_dragon.rs",
        "MATERIAL_A",
    ),
    (
        "c66235877.lua",
        "s.initial_effect",
        r"Fusion\.AddProcMix\(c,false,false,\d+,(\d+)\)",
        "cards/fiend_skull_dragon.rs",
        "MATERIAL_B",
    ),
    (
        "c49868263.lua",
        "s.initial_effect",
        r"Cost\.PayLP\((\d+)\)",
        "cards/ryu_senshi.rs",
        "COST_LP",
    ),
    (
        "c49868263.lua",
        "s.initial_effect",
        r"Fusion\.AddProcMix\(c,false,false,(\d+),\d+\)",
        "cards/ryu_senshi.rs",
        "MATERIAL_A",
    ),
    (
        "c49868263.lua",
        "s.initial_effect",
        r"Fusion\.AddProcMix\(c,false,false,\d+,(\d+)\)",
        "cards/ryu_senshi.rs",
        "MATERIAL_B",
    ),
    # Reaper on the Nightmare passes `true,true` where the rest pass
    # `false,false`, so its pattern is its own.
    (
        "c85684223.lua",
        "s.initial_effect",
        r"Fusion\.AddProcMix\(c,true,true,(\d+),\d+\)",
        "cards/reaper_nightmare.rs",
        "MATERIAL_A",
    ),
    (
        "c85684223.lua",
        "s.initial_effect",
        r"Fusion\.AddProcMix\(c,true,true,\d+,(\d+)\)",
        "cards/reaper_nightmare.rs",
        "MATERIAL_B",
    ),
    (
        "c63519819.lua",
        "s.initial_effect",
        r"Fusion\.AddProcMix\(c,true,true,(\d+),\d+\)",
        "cards/thousand_eyes.rs",
        "MATERIAL_A",
    ),
    (
        "c63519819.lua",
        "s.initial_effect",
        r"Fusion\.AddProcMix\(c,true,true,\d+,(\d+)\)",
        "cards/thousand_eyes.rs",
        "MATERIAL_B",
    ),
    (
        "c13756293.lua",
        "s.initial_effect",
        r"Fusion\.AddProcMix\(c,true,true,(\d+),\d+\)",
        "cards/king_dragun.rs",
        "MATERIAL_A",
    ),
    (
        "c13756293.lua",
        "s.initial_effect",
        r"Fusion\.AddProcMix\(c,true,true,\d+,(\d+)\)",
        "cards/king_dragun.rs",
        "MATERIAL_B",
    ),
    # The library's "Eyes Restrict" marker: `e1:SetCode(89785779)`, with
    # `e2`'s code an expression over it that the pattern leaves alone.
    (
        "cards_specific_functions.lua",
        "Auxiliary.AddEREquipLimit",
        r"e1:SetCode\((\d+)\)",
        "cards/thousand_eyes.rs",
        "ER_MARKER",
    ),
]

# Effect names the port **assumes no pool card uses**, and the
# approximation each assumption licenses. An approximation whose premise
# quietly stops holding is invisible: the code stays self-consistent and
# the divergence surfaces as behaviour that never fires.
ABSENT_FROM_POOL = [
    (
        ("EFFECT_ADD_TYPE", "EFFECT_REMOVE_TYPE", "EFFECT_CHANGE_TYPE"),
        "script_api::target_is_trap reads the PRINTED type, where "
        "Card.IsTrap reads the effective one",
    ),
    (
        ("SEQ_DECKTOP", "SEQ_DECKBOTTOM"),
        "game.rs models a look at the deck's top as a uniform chance node: "
        "every placement into a deck shuffles, so nothing knows the order",
    ),
    (
        ("EFFECT_EXTRA_RELEASE", "EFFECT_EXTRA_RELEASE_NONSUM"),
        "the release-as-cost library collapses: no must-include pool, no "
        "single-use bookkeeping, no second group to split off",
    ),
    (
        ("SUMMON_TYPE_FUSION", "Duel.SetFusionMaterial", "EFFECT_FUSION_MATERIAL"),
        "nothing in this pool performs a Fusion Summon, so the material "
        "procedure Fusion.AddProcMix registers is never asked its question "
        "and its condition and operation are left unported",
    ),
    (
        ("EFFECT_ADD_RACE", "EFFECT_REMOVE_RACE", "EFFECT_CHANGE_RACE"),
        "script_api::is_race_readonly reads the PRINTED race, where "
        "Card.IsRace reads the effective one",
    ),
    (
        ("89785779",),
        "the Eyes Restrict marker's value and operation are called only by "
        "other cards' scripts; Thousand-Eyes Restrict registers both and "
        "no pool card exercises them",
    ),
    (
        ("cost_lp_paid",),
        "Cost.PayLP writes the amount into the chain data and no pool "
        "card reads it back, so the ports of Cost.PayLP drop the write",
    ),
]

# Individual **named** Lua constants the port carries, pinned against the
# library file that defines them. `card_counter_constants.lua` holds 157
# card numbers; transcribing the table whole would be noise, so the ones
# the port actually names are pinned one at a time instead.
LUA_NAMED = [
    ("card_counter_constants.lua", "CARD_BLUEEYES_SPIRIT", "card.rs", "CARD_BLUEEYES_SPIRIT"),
    ("card_counter_constants.lua", "CARD_MARINE_DOLPHIN", "card.rs", "CARD_MARINE_DOLPHIN"),
    ("card_counter_constants.lua", "CARD_TWINKLE_MOSS", "card.rs", "CARD_TWINKLE_MOSS"),
]

SINGLETONS = [
    ("event.rs", "PLAYER_NONE", "PLAYER_NONE"),
    ("event.rs", "PLAYER_ALL", "PLAYER_ALL"),
    ("event.rs", "PLAYER_SELFDES", "PLAYER_SELFDES"),
    ("field.rs", "NO_FLIP_EFFECT", "NO_FLIP_EFFECT"),
    ("field.rs", "LOCATION_REASON_TOFIELD", "LOCATION_REASON_TOFIELD"),
    ("card.rs", "CARD_MARINE_DOLPHIN", "CARD_MARINE_DOLPHIN"),
    ("card.rs", "CARD_TWINKLE_MOSS", "CARD_TWINKLE_MOSS"),
]

# Values that are derived from the reference's *expressions* rather than
# from a named constant, so a name lookup cannot check them. Each is listed
# with where the expression appears, and is verified by reading rather than
# by this tool.
DERIVED = {
    "PHASE_MASK": "the literal 0xfffff000u in processor.cpp's phase-code tests",
    "EXTRA_ZONE_START": "the literal 5 in card.h's is_location and field.cpp",
}


def free_constant(filename: str, name: str) -> int | None:
    text = (RS / filename).read_text()
    m = re.search(r"(?:pub )?const " + name + r"\s*:\s*\w+\s*=\s*([^;]+);", text)
    if not m:
        return None
    value = m.group(1).strip()
    try:
        return parse_int(value)
    except ValueError:
        return None


def lua_function_body(filename: str, function: str) -> str:
    """The text of one Lua function, so a literal inside it is not
    confused with the same shape of call elsewhere in the file."""
    text = (LUA / filename).read_text()
    start = text.find(f"function {function}(")
    if start < 0:
        return ""
    end = text.find("\nfunction ", start + 1)
    return text[start:] if end < 0 else text[start:end]


def pool_scripts() -> list[tuple[int, str]]:
    """Every pool card that has a script, as (code, source)."""
    text = (ROOT / "tools/oracle/carddata.py").read_text()
    codes = sorted({int(c) for c in re.findall(r"OcgCard\(\s*(\d+),", text)})
    out = []
    for code in codes:
        path = LUA / f"c{code}.lua"
        if path.is_file():
            out.append((code, path.read_text()))
    return out


def check_lua_named() -> int:
    problems = 0
    for filename, lua_name, rust_file, rust_name in LUA_NAMED:
        text = (LUA / filename).read_text()
        m = re.search(rf"^{lua_name}\s*=\s*(\d+)", text, re.M)
        ours = free_constant(rust_file, rust_name)
        if not m or ours is None:
            print(f"GAP  {lua_name:<28} could not compare")
            problems += 1
        elif int(m.group(1)) != ours:
            print(f"GAP  {lua_name:<28} reference {m.group(1)}, ours {ours}")
            problems += 1
        else:
            print(f"ok   {lua_name:<28} {ours}")
    return problems


def check_absent_from_pool() -> int:
    scripts = pool_scripts()
    if not scripts:
        print("GAP  pool scripts        read nothing from carddata.py")
        return 1
    problems = 0
    for names, why in ABSENT_FROM_POOL:
        hits = sorted(
            {code for code, src in scripts for n in names if n in src}
        )
        label = names[0] if len(names) == 1 else names[0] + " family"
        if hits:
            print(f"GAP  {label:<28} used by {hits} — {why}")
            problems += len(hits)
        else:
            print(f"ok   {label:<28} absent from {len(scripts)} pool scripts")
    return problems


def check_lua_literals() -> int:
    problems = 0
    for filename, function, pattern, rust_file, name in LUA_LITERALS:
        text = lua_function_body(filename, function)
        found = {int(m) for m in re.findall(pattern, text)}
        ours = free_constant(rust_file, name)
        if len(found) != 1 or ours is None:
            print(f"GAP  {name:<20} read {sorted(found)} from {function}, ours {ours}")
            problems += 1
        elif ours != found.pop():
            # re-read for the message; the set was consumed above
            ref = {int(m) for m in re.findall(pattern, text)}.pop()
            print(f"GAP  {name:<20} reference {ref}, ours {ours}")
            problems += 1
        else:
            print(f"ok   {name:<20} {ours}")
    return problems


def check_mixed(reference: dict[str, tuple[int, str]]) -> int:
    problems = 0
    for filename, module, candidates in MIXED:
        ours = rust_constants(filename, module)
        missing, wrong = [], []
        for name, value in sorted(ours.items()):
            if name in DERIVED:
                continue
            forced = COLLISIONS.get((module, name))
            names = (forced,) if forced else candidates(name)
            hit = next((reference[c] for c in names if c in reference), None)
            if hit is None:
                missing.append(f"{name} = {value:#x}")
            elif hit[0] != value:
                wrong.append(f"{name}: reference {hit[0]:#x} ({hit[1]}), ours {value:#x}")
        ok = not missing and not wrong
        problems += len(missing) + len(wrong)
        print(f"{'ok  ' if ok else 'GAP '}{module:<20} {len(ours):3d} entries")
        for line in wrong:
            print(f"       WRONG VALUE  {line}")
        for line in missing:
            print(f"       no reference {line}")
    return problems


def check_singletons(reference: dict[str, tuple[int, str]]) -> int:
    problems = 0
    for filename, name, ref_name in SINGLETONS:
        ours = free_constant(filename, name)
        hit = reference.get(ref_name)
        if ours is None or hit is None:
            print(f"GAP  {name:<20} could not compare")
            problems += 1
        elif ours != hit[0]:
            print(f"GAP  {name:<20} reference {hit[0]:#x}, ours {ours:#x}")
            problems += 1
        else:
            print(f"ok   {name:<20} {ours:#x}")
    return problems


def main() -> int:
    if not SRC.is_dir():
        print(f"no ocgcore checkout at {SRC}", file=sys.stderr)
        print("run tools/oracle/build.sh first", file=sys.stderr)
        return 2

    problems = 0
    for prefix, header, filename, module in TABLES:
        reference = c_constants(header, prefix)
        ours = rust_constants(filename, module)
        missing, wrong = [], []
        for name, value in sorted(reference.items()):
            got = ours.get(ALIASES.get(name, name))
            if got is None:
                missing.append(f"{name} = {value:#x}")
            elif got != value:
                wrong.append(f"{name}: reference {value:#x}, ours {got:#x}")

        ok = not missing and not wrong
        problems += len(missing) + len(wrong)
        print(f"{'ok  ' if ok else 'GAP '}{prefix:<20} {len(reference):3d} entries")
        for line in wrong:
            print(f"       WRONG VALUE  {line}")
        for line in missing:
            print(f"       not ported   {line}")

    for prefix, filename, rust_file, module in LUA_COMPOSITES:
        reference = lua_composites(filename, prefix)
        ours = rust_expr_constants(rust_file, module)
        if not reference:
            print(f"GAP {prefix:<20}   0 entries (lua) — read nothing from {filename}")
            problems += 1
            continue
        missing, wrong = [], []
        for name, value in sorted(reference.items()):
            got = ours.get(ALIASES.get(name, name))
            if got is None:
                missing.append(f"{name} = {value:#x}")
            elif got != value:
                wrong.append(f"{name}: reference {value:#x}, ours {got:#x}")
        ok = not missing and not wrong
        problems += len(missing) + len(wrong)
        print(f"{'ok  ' if ok else 'GAP '}{prefix:<20} {len(reference):3d} entries (lua)")
        for line in wrong:
            print(f"       WRONG VALUE  {line}")
        for line in missing:
            print(f"       not ported   {line}")

    for prefix, filename, rust_file, module in LUA_TABLES:
        reference = lua_constants(filename, prefix)
        ours = rust_constants(rust_file, module)
        # A table that matched nothing is a broken reader, not a clean
        # result: every entry would be reported as agreeing.
        if not reference:
            print(f"GAP {prefix:<20}   0 entries (lua) — read nothing from {filename}")
            problems += 1
            continue
        missing, wrong = [], []
        for name, value in sorted(reference.items()):
            got = ours.get(ALIASES.get(name, name))
            if got is None:
                missing.append(f"{name} = {value}")
            elif got != value:
                wrong.append(f"{name}: reference {value}, ours {got}")
        ok = not missing and not wrong
        problems += len(missing) + len(wrong)
        print(f"{'ok  ' if ok else 'GAP '}{prefix:<20} {len(reference):3d} entries (lua)")
        for line in wrong:
            print(f"       WRONG VALUE  {line}")
        for line in missing:
            print(f"       not ported   {line}")

    reference = all_reference_constants()
    print()
    problems += check_mixed(reference)
    print()
    problems += check_singletons(reference)
    print()
    problems += check_lua_literals()
    print()
    problems += check_lua_named()
    print()
    problems += check_absent_from_pool()
    print()
    for name, where in DERIVED.items():
        print(f"--   {name:<20} derived from {where}")

    if problems:
        print(f"\n{problems} problem(s). A wrong constant is invisible: the code")
        print("around it stays consistent with whatever number it was given.")
    return 1 if problems else 0


if __name__ == "__main__":
    raise SystemExit(main())

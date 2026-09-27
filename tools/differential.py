#!/usr/bin/env python3
"""Run one duel on both engines and diff the traces.

The port and ocgcore are two different programs. To compare them, each
renders a duel into the **same** line-based form (see `src/trace.rs` for
the format and, more importantly, for what is deliberately left out), and
the comparison is a diff of those lines.

    tools/differential.py              # the default vanilla duel
    tools/differential.py --context 6  # more lines around a break
    tools/differential.py --dump both  # print both traces in full

Run it from the repository root with Python 3.10 or newer (standard
library only): the oracle FFI and its card database live under
`tools/oracle`.

## The policy is the hard part

A differential run only means something if both engines are asked the same
questions *and answer them the same way*. The port's answers come from
`driver::PassPolicy`; the function below is that policy written a second
time against ocgcore's wire protocol. The two must agree, so every rule
here names the `Policy` method it mirrors. A divergence caused by the two
policies drifting apart looks exactly like an engine bug, which is why
this is the part to keep honest.
"""

from __future__ import annotations

import argparse
import struct
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

from tools.oracle import protocol as p  # noqa: E402
from tools.oracle.core import OcgDuel, oracle_available  # noqa: E402

PORT = ROOT

# The duel both sides build. A code the reference's card database knows,
# with the statline the port is handed verbatim — see `Driver::deck_card`.
DECK_CODE = 5_053_103
# The translated cards, placed at the **top** of each deck by `--top` (a
# `CODE:N,CODE:N` list, bottom-to-top) so the play policy meets them first.
# `--pots N` is the older sugar for `--top 55144522:N`.
POT_OF_GREED = 55_144_522

# The random state, named for BOTH engines rather than left to their
# defaults — which differ. Nothing in a vanilla duel draws from the
# generator (`DUEL_PSEUDO_SHUFFLE` orders the piles by insertion), so the
# mismatch is invisible right up until the first `TossCoin`, at which point
# every coin disagrees and reads as an engine bug.
SEED = (1, 2, 3, 4)
# Message ids the protocol module does not name (ocgapi_constants.h).
MSG_SHUFFLE_EXTRA = 39
MSG_REVERSE_DECK = 37
MSG_DECK_TOP = 38
MSG_SWAP = 55
MSG_FIELD_DISABLED = 56
MSG_CHAIN_DISABLED = 76
MSG_CARD_SELECTED = 80
MSG_BECOME_TARGET = 83
MSG_CARD_TARGET = 96
MSG_ADD_COUNTER = 101
MSG_REMOVE_COUNTER = 102
MSG_ATTACK_DISABLED = 112
MSG_MISSED_EFFECT = 120
MSG_CARD_HINT = 160
MSG_MATCH_KILL = 170
MSG_SHUFFLE_SET_CARD = 36

# `OCG_DuelProcess` returns these; the port's `Status` has the same three.
STATUS_END, STATUS_AWAITING, STATUS_CONTINUE = 0, 1, 2

# The port's trace binary, when a caller has built it once rather than
# paying `cargo run` per game (the fuzz runner does). Set by `main` or by
# `fuzz.py`; None means `cargo run --quiet --example trace`.
PORT_BIN: Path | None = None

MASK64 = (1 << 64) - 1


class SplitMix64:
    """`driver::RandomPolicy`'s generator, the same ten lines.

    Both sides pin the first three outputs for seed 0 (see `selfcheck`);
    a generator that drifted on one side would desynchronise every random
    game after the first draw and read as an engine bug.
    """

    def __init__(self, seed: int) -> None:
        self.state = seed & MASK64

    def next_u64(self) -> int:
        self.state = (self.state + 0x9E3779B97F4A7C15) & MASK64
        z = self.state
        z = ((z ^ (z >> 30)) * 0xBF58476D1CE4E5B9) & MASK64
        z = ((z ^ (z >> 27)) * 0x94D049BB133111EB) & MASK64
        return z ^ (z >> 31)

    def below(self, n: int) -> int:
        """One draw, reduced to `0..n`; `n == 0` draws anyway and answers 0."""
        r = self.next_u64()
        return r % n if n else 0


class Mirror:
    """What answers ocgcore's questions: one of the port's three policies.

    `play` selects between the two scripted policies exactly as before;
    `rng` is set for `random:N` and every branch of `ask_and_answer` that
    consults it follows the draw protocol written on the matching
    `RandomPolicy` method.
    """

    def __init__(self, spec: str) -> None:
        self.spec = spec
        self.play = spec == "play"
        self.rng: SplitMix64 | None = None
        if spec.startswith("random:"):
            self.rng = SplitMix64(int(spec.removeprefix("random:")))
        elif spec not in ("pass", "play"):
            raise SystemExit(f"unknown policy {spec!r}: expected pass, play or random:N")


def selfcheck() -> None:
    """The generator pin — `driver::random_policy_tests::splitmix64_is_pinned`."""
    r = SplitMix64(0)
    got = (r.next_u64(), r.next_u64(), r.next_u64())
    want = (0xE220A8397B1DCDAF, 0x6E789E6AA1B965F4, 0x06C45D188009454F)
    if got != want:
        raise SystemExit(f"SplitMix64 drifted from the port's: {got!r}")


# Messages the trace renders but `protocol.parse_message` returns raw, with
# the layout transcribed from ocgcore rather than remembered.
MSG_RECOVER = 92
MSG_PAY_LPCOST = 100
MSG_EQUIP = 93


def _loc(r: p._Reader) -> tuple[int, int, int]:
    """`loc_info` — controller, location, sequence, position (card.h)."""
    con, loc, seq, _pos = r.u8(), r.u8(), r.u32(), r.u32()
    return con, loc, seq


def cut_at_win(lines: list[str]) -> list[str]:
    """End both traces at the first `evt win` line.

    Neither engine stops itself: ocgcore keeps processing after `MSG_WIN`
    (a host ends the duel), and the port's driver stops after the *step*
    that announced it — so past the win, the two sides' batches are
    different lengths of the same nothing. The win is the last fact either
    side reports.
    """
    for i, line in enumerate(lines):
        if line.startswith("evt win "):
            return [*lines[: i + 1], "evt stop win " + line.removeprefix("evt win ")]
    return lines


UNORDERED_BURST = ("evt cardhint ", "evt removecounter ")


def canonical(lines: list[str]) -> list[str]:
    """Sort each maximal run of lines the reference emits in **hash order**.

    ocgcore's phase and chain reset lists (`field_effect::pheff`, `cheff`)
    are `std::unordered_set<effect*>`: the order in which effects reset at a
    phase end is the order of their pointers' hashes, which no port can
    reproduce and which two runs of the reference itself need not share.
    The messages a reset writes — a client hint's removal, a counter permit's
    counters — are therefore compared as a set: consecutive lines of those
    kinds are sorted on both sides. A lone line is unchanged.
    """
    out: list[str] = []
    run: list[str] = []
    for line in lines:
        if line.startswith(UNORDERED_BURST):
            run.append(line)
            continue
        out.extend(sorted(run))
        run = []
        out.append(line)
    out.extend(sorted(run))
    return out


def event_line(mid: int, raw: bytes) -> str | None:
    """Render one `InfoMsg` as a trace event, or `None` if exempt.

    The mirror of `trace::line_for`'s event half. Anything not listed is an
    exemption there too.
    """
    r = p._Reader(raw[1:])
    if mid == p.MSG_NEW_TURN:
        return f"evt turn p{r.u8()}"
    if mid == p.MSG_NEW_PHASE:
        (phase,) = struct.unpack_from("<H", raw, 1)
        return f"evt phase {phase:#x}"
    if mid == p.MSG_DAMAGE:
        return f"evt damage p{r.u8()} {r.u32()}"
    if mid == MSG_RECOVER:
        return f"evt recover p{r.u8()} {r.u32()}"
    if mid == MSG_PAY_LPCOST:
        return f"evt paylp p{r.u8()} {r.u32()}"
    if mid == p.MSG_ATTACK:
        a, t = _loc(r), _loc(r)
        return f"evt attack {a[0]}:{a[1]}:{a[2]} -> {t[0]}:{t[1]}:{t[2]}"
    if mid == p.MSG_BATTLE:
        _loc(r)
        aatk, _adef, adestroyed = r.u32(), r.u32(), r.u8()
        _loc(r)
        tatk, _tdef, tdestroyed = r.u32(), r.u32(), r.u8()
        return f"evt battle {aatk}/{adestroyed} {tatk}/{tdestroyed}"
    if mid == p.MSG_SUMMONING:
        return f"evt summon {r.u32()}"
    if mid == p.MSG_SPSUMMONING:
        return f"evt spsummon {r.u32()}"
    if mid == p.MSG_FLIPSUMMONING:
        return f"evt flipsummon {r.u32()}"
    if mid == p.MSG_SET:
        return f"evt set {r.u32()}"
    if mid == p.MSG_SHUFFLE_HAND:
        # The hand order after a shuffle, card for card. `DUEL_PSEUDO_SHUFFLE`
        # does **not** cover the hand (`field::shuffle` skips only the deck),
        # so this is a real roll of the duel's generator and the resulting
        # order is what every later hand index refers to. Rendering only the
        # count would compare nothing: two engines that disagree about the
        # order agree about the size.
        player = r.u8()
        n = r.u32()
        codes = ",".join(str(r.u32()) for _ in range(n))
        return f"evt shufflehand p{player} [{codes}]"
    if mid == MSG_EQUIP:
        # Both sides as locations, mirroring `trace.rs`: an attachment is
        # board state nothing else in the trace reports, and a seat tells
        # two copies of one equip card apart where a code does not.
        ec, el, es = _loc(r)
        tc, tl, ts = _loc(r)
        return f"evt equip p{ec} l{el} s{es} -> p{tc} l{tl} s{ts}"
    if mid == p.MSG_DAMAGE_STEP_START:
        return "evt damagestep start"
    if mid == p.MSG_DAMAGE_STEP_END:
        return "evt damagestep end"
    # ---- The widened half (2026-09-17), mirroring `trace.rs` arm for arm.
    # Layouts are transcribed from the reference's `new_message` sites
    # (`libduel.cpp`, `processor.cpp`, `card.cpp`, `field.cpp`, `operations.cpp`).
    if mid == p.MSG_HINT:
        return f"evt hint k{r.u8()} p{r.u8()} {r.u64()}"
    if mid == p.MSG_CHAINING:
        code = r.u32()
        _loc(r)  # the handler's location; the port's message carries the triggering one
        tc, tl, ts = r.u8(), r.u8(), r.u32()
        desc, n = r.u64(), r.u32()
        return f"evt chaining {code} t{tc}:{tl}:{ts} d{desc} n{n}"
    if mid == p.MSG_CHAINED:
        return f"evt chained n{r.u8()}"
    if mid == p.MSG_CHAIN_SOLVING:
        return f"evt chainsolving n{r.u8()}"
    if mid == p.MSG_CHAIN_SOLVED:
        return f"evt chainsolved n{r.u8()}"
    if mid == MSG_CHAIN_DISABLED:
        return f"evt chaindisabled n{r.u8()}"
    if mid == p.MSG_CHAIN_END:
        return "evt chainend"
    if mid == p.MSG_SUMMONED:
        return "evt summoned"
    if mid == p.MSG_SPSUMMONED:
        return "evt spsummoned"
    if mid == p.MSG_FLIPSUMMONED:
        return "evt flipsummoned"
    if mid == p.MSG_CONFIRM_CARDS:
        player, n = r.u8(), r.u32()
        names: list[str] = []
        for _ in range(n):
            names.append(str(r.u32()))
            r.u8(), r.u8(), r.u32()
        return f"evt confirm p{player} [{','.join(names)}]"
    if mid == p.MSG_SHUFFLE_DECK:
        return f"evt shuffledeck p{r.u8()}"
    if mid == MSG_SHUFFLE_EXTRA:
        player, n = r.u8(), r.u32()
        return f"evt shuffleextra p{player} [{','.join(str(r.u32()) for _ in range(n))}]"
    if mid == MSG_DECK_TOP:
        return f"evt decktop p{r.u8()} s{r.u32()} {r.u32()} pos{r.u32()}"
    if mid == MSG_REVERSE_DECK:
        return "evt reversedeck"
    if mid == MSG_ATTACK_DISABLED:
        return "evt attackdisabled"
    if mid == MSG_FIELD_DISABLED:
        return f"evt fielddisabled {r.u32():#x}"
    if mid in (MSG_ADD_COUNTER, MSG_REMOVE_COUNTER):
        kind = "addcounter" if mid == MSG_ADD_COUNTER else "removecounter"
        ct, c, loc, seq, n = r.u16(), r.u8(), r.u8(), r.u8(), r.u16()
        return f"evt {kind} t{ct} p{c} l{loc} s{seq} n{n}"
    if mid == MSG_MATCH_KILL:
        return f"evt matchkill {r.u32()}"
    if mid == MSG_SWAP:
        first = r.u32()
        _loc(r)
        second = r.u32()
        return f"evt swap {first} {second}"
    if mid == MSG_CARD_HINT:
        c, loc, seq = _loc(r)
        return f"evt cardhint p{c} l{loc} s{seq} k{r.u8()} {r.u64()}"
    if mid == MSG_CARD_TARGET:
        oc, ol, oseq = _loc(r)
        tc, tl, tseq = _loc(r)
        return f"evt cardtarget p{oc} l{ol} s{oseq} -> p{tc} l{tl} s{tseq}"
    if mid == MSG_CARD_SELECTED:
        n = r.u32()
        locs = []
        for _ in range(n):
            c, loc, seq = _loc(r)
            locs.append(f"p{c} l{loc} s{seq}")
        return f"evt cardselected [{','.join(locs)}]"
    if mid == MSG_MISSED_EFFECT:
        _loc(r)
        return f"evt missed {r.u32()}"
    if mid == MSG_BECOME_TARGET:
        return f"evt becometarget n{r.u32()}"
    if mid == MSG_SHUFFLE_SET_CARD:
        return f"evt shuffleset l{r.u8()} n{r.u8()}"
    if mid == p.MSG_RANDOM_SELECTED:
        player, n = r.u8(), r.u32()
        locs = []
        for _ in range(n):
            c, loc, seq = _loc(r)
            locs.append(f"p{c} l{loc} s{seq}")
        return f"evt randomselected p{player} [{','.join(locs)}]"
    return None


def idle_command(msg: p.IdleCmd, play: bool, turn_id: int) -> int:
    """`Policy::idle`, as the two Rust policies implement it.

    Command numbers are `SelectIdleCmd`'s: 0 summon, 1 special summon by
    a card's own procedure, 3 mset, 4 sset, 5 activate, 6 to the Battle
    Phase, 7 to the End Phase (playerop.cpp's validation lists them
    all).

    `turn_id` mirrors the engine's `infos.turn_id`, which is incremented
    immediately before `MSG_NEW_TURN` is written — so counting those
    messages gives exactly the number the Rust policy reads.
    """
    if not play:
        # PassPolicy — out of the phase at once.
        return 7 if msg.to_ep else 6
    # PlayPolicy — activate, then Set a Spell/Trap, then the monster,
    # then leave. A normal summon and a monster Set are the same
    # once-a-turn move, so the policy alternates: preferring either one
    # always would mean the other never happens, and never Setting a
    # monster means never producing a face-down, so no flip effect is
    # ever reached.
    if msg.activatable:
        return 5
    # A rule summon costs the turn nothing, so it goes above every
    # once-a-turn move — but below activation, so a monster already on
    # the field still uses its effect.
    if msg.spsummonable:
        return 1
    if msg.ssetable:
        return 4
    if msg.msetable and turn_id % 2 == 1:
        return 3
    if msg.summonable:
        return 0
    if msg.msetable:
        return 3
    if msg.to_bp:
        return 6
    return 7 if msg.to_ep else 6


def battle_command(msg: p.BattleCmd, play: bool) -> int:
    """`Policy::battle`. 1 attack, 2 to Main 2, 3 to the End Phase."""
    if play and msg.attackable:
        return 1
    return 2 if msg.to_m2 else 3


def random_cards(rng: SplitMix64, n: int, min_: int, max_: int) -> list[int]:
    """`RandomPolicy::cards`: one draw for how many, then a partial
    Fisher-Yates with one draw per pick, reported ascending."""
    lo = min(min_, n)
    hi = max(min(max_, n), lo)
    k = lo + rng.below(hi - lo + 1)
    idx = list(range(n))
    for i in range(k):
        j = i + rng.below(n - i)
        idx[i], idx[j] = idx[j], idx[i]
    return sorted(idx[:k])


def ask_and_answer(msg: object, mirror: Mirror, turn_id: int = 0) -> tuple[str, str, bytes]:
    """The `ask`/`ans` pair for one request, and the bytes to respond with.

    Each branch names the `driver::Policy` method it mirrors; keeping them
    labelled is what stops the two policies drifting. The random branches
    follow the draw protocol written on the `RandomPolicy` method of the
    same name.
    """
    play, rng = mirror.play, mirror.rng
    if isinstance(msg, p.IdleCmd):
        n = (
            len(msg.summonable)
            + len(msg.spsummonable)
            + len(msg.repositionable)
            + len(msg.msetable)
            + len(msg.ssetable)
            + len(msg.activatable)
        )
        if rng is not None:
            # RandomPolicy::idle — one draw over the flat list of moves.
            flat: list[tuple[int, int]] = []
            for kind, entries in (
                (0, msg.summonable),
                (1, msg.spsummonable),
                (2, msg.repositionable),
                (3, msg.msetable),
                (4, msg.ssetable),
                (5, msg.activatable),
            ):
                flat.extend((kind, i) for i in range(len(entries)))
            if msg.to_bp:
                flat.append((6, 0))
            if msg.to_ep:
                flat.append((7, 0))
            kind, index = flat[rng.below(len(flat))] if flat else (6, 0)
        else:
            kind, index = idle_command(msg, play, turn_id), 0
        lists = " ".join(
            f"{tag}:{','.join(str(e.code) for e in entries)}"
            for tag, entries in (
                ("s", msg.summonable),
                ("sp", msg.spsummonable),
                ("r", msg.repositionable),
                ("ms", msg.msetable),
                ("ss", msg.ssetable),
                ("a", msg.activatable),
            )
        )
        return (
            f"ask idle p{msg.player} n{n} [{lists}]",
            f"ans idle {kind} {index}",
            p.respond_index_pair(kind, index),
        )
    if isinstance(msg, p.BattleCmd):
        n = len(msg.activatable) + len(msg.attackable)
        if rng is not None:
            # RandomPolicy::battle — attacks, activations, then the exits.
            flat = [(1, i) for i in range(len(msg.attackable))]
            flat += [(0, i) for i in range(len(msg.activatable))]
            if msg.to_m2:
                flat.append((2, 0))
            if msg.to_ep:
                flat.append((3, 0))
            kind, index = flat[rng.below(len(flat))] if flat else (3, 0)
        else:
            kind, index = battle_command(msg, play), 0
        lists = (
            f"a:{','.join(str(e.code) for e in msg.activatable)}"
            f" at:{','.join(str(e.code) for e in msg.attackable)}"
        )
        return (
            f"ask battle p{msg.player} n{n} [{lists}]",
            f"ans battle {kind} {index}",
            p.respond_index_pair(kind, index),
        )
    if isinstance(msg, p.SelectChain):
        # Policy::chain — PlayPolicy activates the first thing offered,
        # PassPolicy declines unless forced. RandomPolicy: one draw over
        # the chains plus one for declining, unless forced.
        count = len(msg.chains)
        if rng is not None:
            if count == 0:
                pick = 0 if msg.forced else -1
            elif msg.forced:
                pick = rng.below(count)
            else:
                r = rng.below(count + 1)
                pick = r if r < count else -1
        else:
            pick = 0 if msg.forced or (play and msg.chains) else -1
        codes = ",".join(str(c.code) for c in msg.chains)
        detail = codes
        if __import__("os").environ.get("DIFF_CHAIN_DEBUG"):
            # The port renders the same fields inside the bracket; keep the
            # two strings identical so the differential compares them.
            detail = (
                f"{codes}] spe={msg.spe_count} forced={int(msg.forced)} "
                f"t={msg.timing:#x}/{msg.timing_other:#x} ["
            )
        return (
            f"ask chain p{msg.player} n{count}" + (f" [{detail}]" if detail else ""),
            f"ans chain {pick if pick >= 0 else '-'}",
            p.respond_int(pick),
        )
    if isinstance(msg, p.SelectCard):
        # Policy::cards — the first `min`, the smallest legal answer; or
        # RandomPolicy::cards.
        if rng is not None:
            picks = random_cards(rng, len(msg.cards), msg.min, msg.max)
        else:
            picks = list(range(msg.min))
        return (
            f"ask card p{msg.player} n{len(msg.cards)}",
            "ans card " + ",".join(str(i) for i in picks),
            p.respond_cards(picks),
        )
    if isinstance(msg, p.SelectTribute):
        # The TAGGED encoding, the one SelectCard uses: the reference
        # decodes this with parse_response_cards. And `min` counts release
        # value, not cards, so take offers in order until their release
        # values reach it, never more than `max` cards.
        picks = []
        value = 0
        for i, entry in enumerate(msg.cards):
            if value >= msg.min or len(picks) >= msg.max:
                break
            picks.append(i)
            value += max(entry.extra, 1)
        return (
            f"ask card p{msg.player} n{len(msg.cards)}",
            "ans card " + ",".join(str(i) for i in picks),
            p.respond_cards(picks),
        )
    if isinstance(msg, p.SelectUnselect):
        # Policy::unselect. An index *toggles*: a card already chosen is
        # offered back, so answering 0 forever picks and un-picks the same
        # card. The scripted policies finish as soon as finishing is
        # legal; RandomPolicy draws over the list plus one for stopping
        # when stopping is legal.
        total = len(msg.cards)
        if rng is not None:
            if msg.finishable or msg.cancelable:
                r = rng.below(total + 1)
                pick = r if r < total else -1
            else:
                pick = rng.below(total)
        else:
            pick = -1 if msg.finishable else 0
        if pick < 0:
            return (
                f"ask card p{msg.player} n{total}",
                "ans unselect -",
                p.respond_int(-1),
            )
        return (
            f"ask card p{msg.player} n{total}",
            f"ans unselect {pick}",
            p.respond_unselect(pick),
        )
    if isinstance(msg, p.SelectYesNo):
        # `MSG_SELECT_EFFECTYN` parses to the same type but names the card
        # asking, and a plain yes/no leaves that code zero — which is how
        # the two are told apart here.
        if msg.code:
            # Policy::effect_yes_no — the play policy takes optional
            # triggers, or they are never exercised. RandomPolicy: one
            # draw, odd is yes.
            yes = (rng.next_u64() % 2 == 1) if rng is not None else play
            return (
                f"ask effectyesno p{msg.player} n2",
                f"ans yesno {'y' if yes else 'n'}",
                p.respond_int(1 if yes else 0),
            )
        # Policy::yes_no — the play policy says yes, against the trait's
        # default. A bare yes/no is the "you may also do X" half of a
        # card, and a declining policy never traces it. The passing
        # policy keeps the safe default. RandomPolicy: one draw, odd is yes.
        yes = (rng.next_u64() % 2 == 1) if rng is not None else play
        return (
            f"ask yesno p{msg.player} n2",
            f"ans yesno {'y' if yes else 'n'}",
            p.respond_int(1 if yes else 0),
        )
    if isinstance(msg, p.SelectOption):
        # Policy::option — PlayPolicy alternates between the first and
        # last branch by turn, so a card offering two effects has both
        # taken; PassPolicy always answers first.
        if rng is not None:
            pick = rng.below(len(msg.options))
        else:
            pick = len(msg.options) - 1 if (play and turn_id % 2 == 1) else 0
        return (
            f"ask option p{msg.player} n{len(msg.options)}",
            f"ans option {pick}",
            p.respond_int(pick),
        )
    if isinstance(msg, p.SelectPosition):
        # Policy::position — the lowest bit offered; RandomPolicy draws
        # over the offered bits.
        bits = [b for b in (1, 2, 4, 8) if msg.positions & b]
        pos = bits[rng.below(len(bits))] if (rng is not None and bits) else next(iter(bits), 1)
        return (
            f"ask position p{msg.player} n{bin(msg.positions).count('1')}",
            f"ans position {pos:#x}",
            p.respond_int(pos),
        )
    if isinstance(msg, p.AnnounceRace):
        # Policy::announce_race — the lowest `count` bits that are actually
        # offered. Taking the lowest bits of the whole word instead names
        # races the prompt never offered, and the engine retries rather
        # than correcting it.
        picked = 0
        left = msg.count
        for bit in range(64):
            if left == 0:
                break
            if msg.available & (1 << bit):
                picked |= 1 << bit
                left -= 1
        return (
            f"ask race p{msg.player} n{bin(msg.available).count('1')}",
            f"ans race {picked:#x}",
            p.respond_u64(picked),
        )
    if isinstance(msg, p.SelectPlace):
        # Policy::place — the first free monster seat on the asked player's
        # own side. The bit layout is `SelectPlace`'s response check read
        # backwards (playerop.cpp): a seat is `1 << sequence`, shifted 8
        # more for the Spell/Trap row and 16 more for the *other* player's
        # side, and a set bit means forbidden. Seven monster seats, not
        # eight.
        #
        # `protocol.lowest_allowed_place` is not used here: it shifts on the
        # absolute player id, where the reference shifts on whether the
        # named side is the asked player's own.
        # The row is whichever the mask leaves open: monster seats are bits
        # 0-6, Spell & Trap seats bits 8-12 (a set bit is forbidden).
        mz = next((s for s in range(7) if not msg.flag & (1 << s)), None)
        if mz is not None:
            loc, seq = p.LOCATION_MZONE, mz
        else:
            loc = p.LOCATION_SZONE
            seq = next(s for s in range(8) if not msg.flag & (1 << (s + 8)))
        return (
            f"ask place p{msg.player} n{msg.count}",
            f"ans place p{msg.player} l{loc} s{seq}",
            p.respond_place(msg.player, loc, seq),
        )
    raise SystemExit(f"the mirror policy has no answer for {msg!r}")


def build_deck(deck_size: int, top: list[tuple[int, int]]) -> list[int]:
    """The decklist both engines are given: vanillas, then the `--top` cards.

    The last listed card is the top of the deck, so the groups are laid down
    in the order given and the final group is drawn first.
    """
    placed = sum(n for _, n in top)
    if placed > deck_size:
        raise SystemExit(f"--top places {placed} cards in a deck of {deck_size}")
    deck = [DECK_CODE] * (deck_size - placed)
    for code, n in top:
        deck += [code] * n
    return deck


def parse_top(spec: str) -> list[tuple[int, int]]:
    """`CODE:N,CODE:N` → `[(code, n), ...]`; the empty string is no cards."""
    out: list[tuple[int, int]] = []
    for item in filter(None, spec.split(",")):
        code, _, n = item.partition(":")
        out.append((int(code), int(n or "1")))
    return out


def oracle_trace(
    deck_size: int, max_steps: int, policy: str, top: list[tuple[int, int]]
) -> list[str]:
    """Play the duel on ocgcore, rendering the same trace."""
    mirror = Mirror(policy)
    deck = build_deck(deck_size, top)
    duel = OcgDuel((deck, deck), seed=SEED)
    lines: list[str] = []
    # `infos.turn_id`, counted the only way a host can: the engine bumps
    # it immediately before writing MSG_NEW_TURN.
    turn_id = 0
    try:
        for _ in range(max_steps):
            status = duel.process()
            pending = None
            for raw in duel.messages():
                msg = p.parse_message(raw)
                if isinstance(msg, p.REQUEST_TYPES):
                    ask, ans, response = ask_and_answer(msg, mirror, turn_id)
                    lines.append(ask)
                    pending = (ans, response)
                elif isinstance(msg, p.DrawMsg):
                    lines.append(f"evt draw p{msg.player} n{len(msg.codes)}")
                elif isinstance(msg, p.WinMsg):
                    lines.append(f"evt win p{msg.player} r{msg.reason}")
                elif isinstance(msg, p.MoveMsg):
                    a, b = msg.prev, msg.cur
                    was = f"{a.controller}:{a.location}:{a.sequence}:{a.position}"
                    now = f"{b.controller}:{b.location}:{b.sequence}:{b.position}"
                    lines.append(f"evt move {msg.code} {was} -> {now} r{msg.reason:#x}")
                elif isinstance(msg, p.ConfirmDecktopMsg):
                    codes = ",".join(str(c) for c in msg.codes)
                    lines.append(f"evt confirmtop p{msg.player} [{codes}]")
                elif isinstance(msg, p.CoinMsg):
                    # **The one place both engines draw from their
                    # generators.** Under `DUEL_PSEUDO_SHUFFLE` nothing
                    # else in a duel does, so until a card tossed a coin
                    # the two seeds could disagree invisibly. Rendered
                    # face by face rather than as a count of heads: two
                    # engines that disagree about the order agree about
                    # the total.
                    faces = "".join("h" if r else "t" for r in msg.results)
                    lines.append(f"evt coin p{msg.player} {faces}")
                elif isinstance(msg, p.InfoMsg):
                    if msg.msg_id == p.MSG_NEW_TURN:
                        turn_id += 1
                    line = event_line(msg.msg_id, msg.raw)
                    if line:
                        lines.append(line)
            if pending:
                lines.append(pending[0])
                duel.respond(pending[1])
            # The reference does not stop itself either: the host watches
            # for the win, exactly as `Driver::run` does.
            if any(line.startswith("evt win ") for line in lines[-64:]):
                return canonical(cut_at_win(lines))
            if status == STATUS_END:
                lines.append("evt stop finished")
                return canonical(lines)
    finally:
        duel.destroy()
    lines.append("evt stop outofsteps")
    return canonical(lines)


def port_trace(policy: str, deck_size: int, max_steps: int, top: str) -> list[str]:
    """Play the duel on the port, via the `trace` example.

    A subprocess because the port has no Python binding and is not getting
    one — a line-based trace on stdout is the whole seam.
    """
    env = dict(__import__("os").environ)
    if PORT_BIN is not None:
        argv = [str(PORT_BIN)]
    else:
        argv = ["cargo", "run", "--quiet", "--example", "trace", "--"]
    argv += [policy, str(deck_size), str(max_steps), ",".join(str(s) for s in SEED), top]
    r = subprocess.run(
        argv,
        cwd=PORT,
        capture_output=True,
        text=True,
        env=env,
        check=False,
    )
    if r.returncode != 0:
        raise SystemExit(f"the port's trace example failed:\n{r.stderr}")
    lines = r.stdout.strip().split("\n")
    if any(line.startswith("evt win ") for line in lines):
        # The port already stops after the step that announced the win; the cut
        # drops that step's later lines, as the oracle side drops its batch's.
        lines = cut_at_win([line for line in lines if not line.startswith("evt stop ")])
    return canonical(lines)


def report(port: list[str], oracle: list[str], context: int) -> int:
    """Print the first divergence with context. Returns an exit code."""
    # Not strict: a length difference is one of the things this reports,
    # and it is handled below rather than raised here.
    port, oracle = canonical(port), canonical(oracle)
    for i, (a, b) in enumerate(zip(port, oracle, strict=False)):
        if a != b:
            lo = max(0, i - context)
            print(f"diverged at line {i + 1} of {len(port)} / {len(oracle)}\n")
            for j in range(lo, i):
                print(f"  {j + 1:5}  {port[j]}")
            print(f"\n  port    {a}\n  oracle  {b}\n")
            for j in range(i + 1, min(i + 1 + context, min(len(port), len(oracle)))):
                print(f"  {j + 1:5}  port={port[j]!r} oracle={oracle[j]!r}")
            return 1
    if len(port) != len(oracle):
        longer, name = (port, "port") if len(port) > len(oracle) else (oracle, "oracle")
        n = min(len(port), len(oracle))
        print(f"agreed for {n} lines, then {name} continued:\n")
        for line in longer[n : n + context]:
            print(f"  {line}")
        return 1
    print(f"identical: {len(port)} lines")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument(
        "--policy",
        default="play",
        help="pass leaves every phase at once; play summons and attacks (more coverage); "
        "random:N chooses uniformly among legal answers from seed N (the fuzzing policy)",
    )
    ap.add_argument("--deck-size", type=int, default=40)
    ap.add_argument("--pots", type=int, default=0, help="Pot of Greed copies on top of each deck")
    ap.add_argument(
        "--top", default="", help="CODE:N,CODE:N — cards on top of each deck, bottom-to-top"
    )
    ap.add_argument("--max-steps", type=int, default=500_000)
    ap.add_argument("--context", type=int, default=10, help="lines of context around a break")
    ap.add_argument("--dump", choices=["port", "oracle", "both"], help="print traces and stop")
    args = ap.parse_args()

    if not oracle_available():
        print("ocgcore is not built: run tools/oracle/build.sh", file=sys.stderr)
        return 2
    selfcheck()

    top_spec = args.top
    if args.pots:
        top_spec = ",".join(filter(None, [top_spec, f"{POT_OF_GREED}:{args.pots}"]))
    port = port_trace(args.policy, args.deck_size, args.max_steps, top_spec)
    top = parse_top(top_spec)
    oracle = oracle_trace(args.deck_size, args.max_steps, args.policy, top)
    if args.dump:
        for name, lines in (("port", port), ("oracle", oracle)):
            if args.dump in (name, "both"):
                print(f"===== {name} ({len(lines)} lines) =====")
                print("\n".join(lines))
        return 0
    return report(port, oracle, args.context)


if __name__ == "__main__":
    raise SystemExit(main())

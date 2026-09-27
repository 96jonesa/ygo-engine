#!/usr/bin/env python3
"""Fuzz the port against ocgcore: many random duels, one seed each, diffed.

    tools/fuzz.py --games 200 --seed 0 --workers 0
    tools/fuzz.py --games 50 --deck pool --deck-size 40
    tools/fuzz.py --games 20 --top 63519819:4,46411259:5,73915051:5

Run it from the repository root with Python 3.10 or newer, like
`differential.py`, which this drives: each game is `differential.py
--policy random:<seed>` — the port's `RandomPolicy` and the mirror in
`differential.py` draw from the same generator, so both engines make the
same choices and the first line they disagree on is a real divergence.

## What varies between games, and what does not

The seed varies the **choices** (every question a player is asked) and,
with `--deck pool`, the **deck**: a seeded shuffle of the whole card pool,
laid down identically on both engines. The engines' own generators stay
at the harness's fixed seed — under `DUEL_PSEUDO_SHUFFLE` nothing but a
coin toss reads them, and the coin's faces are part of the trace.

## Reporting

One flushed progress line per finished game on stderr (done/total,
percent, elapsed, ETA), the divergences with their first differing line
on stdout, a summary, and a nonzero exit if any divergence is not on the
allowlist. `--workers N` runs games on a process pool; 0 means every CPU.

The allowlist is a text file of first-divergence signatures, one per
line, exactly as this tool prints them (`port='…' oracle='…'`); blank
lines and `#` comments are ignored. A known divergence is reported but
does not fail the run.
"""

from __future__ import annotations

import argparse
import multiprocessing as mp
import os
import random
import re
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import differential as d

from tools.oracle.core import oracle_available

ROOT = d.ROOT
PORT = d.PORT
CARDS = PORT / "src" / "cards"


def pool_codes() -> list[int]:
    """Every card the port knows, read off the card modules — so the
    preset grows with the pool rather than being a list to maintain."""
    codes: list[int] = []
    for path in sorted(CARDS.glob("*.rs")):
        m = re.search(r"pub const CODE: u32 = ([0-9_]+);", path.read_text())
        if m:
            codes.append(int(m.group(1).replace("_", "")))
    return codes


def deck_spec(preset: str, top: str, deck_size: int, seed: int) -> str:
    """The `--top` string for one game.

    `shuffled`: the `--top` multiset in a seeded order per game.
    `pool`: a seeded shuffle of the whole pool, `deck_size` of it (the
    rest is vanilla filler if the pool is smaller). `vanilla`: the fixed
    `--top` alone. The string is what both engines receive, so the deck
    is identical by construction.
    """
    if preset == "vanilla":
        return top
    rng = random.Random(seed)
    if preset == "shuffled":
        # A fixed decklist with multiples, dealt in a new order each game:
        # under DUEL_PSEUDO_SHUFFLE neither engine shuffles, so the order
        # the spec lists is the order the deck is in.
        cards = [
            code
            for item in top.split(",")
            if item
            for code, n in [item.split(":")]
            for _ in range(int(n))
        ]
        rng.shuffle(cards)
        return ",".join(f"{c}:1" for c in cards)
    codes = pool_codes()
    rng.shuffle(codes)
    chosen = codes[:deck_size]
    spec = ",".join(f"{c}:1" for c in chosen)
    return ",".join(filter(None, [top, spec]))


@dataclass
class Game:
    seed: int
    spec: str
    lines: int
    first_diff: int | None  # 1-based line, None when identical
    signature: str  # "port='…' oracle='…'" or "" when identical
    stop: str  # the port's stop line
    error: str  # non-empty when a side failed outright


def play_one(seed: int, spec: str, deck_size: int, max_steps: int) -> Game:
    policy = f"random:{seed}"
    try:
        port = d.port_trace(policy, deck_size, max_steps, spec)
    except SystemExit as e:  # the trace example failed
        return Game(seed, spec, 0, None, "", "", f"port: {e}")
    try:
        oracle = d.oracle_trace(deck_size, max_steps, policy, d.parse_top(spec))
    except Exception as e:
        return Game(seed, spec, len(port), None, "", "", f"oracle: {e!r}")
    stop = next((ln for ln in reversed(port) if ln.startswith("evt stop")), "")
    for i, (a, b) in enumerate(zip(port, oracle, strict=False), start=1):
        if a != b:
            return Game(seed, spec, len(port), i, f"port={a!r} oracle={b!r}", stop, "")
    if len(port) != len(oracle):
        i = min(len(port), len(oracle)) + 1
        a = port[i - 1] if i <= len(port) else "<end>"
        b = oracle[i - 1] if i <= len(oracle) else "<end>"
        return Game(seed, spec, len(port), i, f"port={a!r} oracle={b!r}", stop, "")
    return Game(seed, spec, len(port), None, "", stop, "")


def _task(args: tuple[int, str, int, int]) -> Game:
    seed, spec, deck_size, max_steps = args
    return play_one(seed, spec, deck_size, max_steps)


def build_port() -> Path:
    """Build the trace example once, in release, and return the binary.

    `PORT_BIN` names a prebuilt one instead — `tests/oracle_fuzz.rs` sets
    it to the binary of the build under test, which under `cargo mutants`
    is a mutant's.
    """
    if prebuilt := os.environ.get("PORT_BIN"):
        return Path(prebuilt)
    env = dict(os.environ)
    subprocess.run(
        ["cargo", "build", "--release", "--quiet", "--example", "trace"],
        cwd=PORT,
        env=env,
        check=True,
    )
    return PORT / "target" / "release" / "examples" / "trace"


def load_allowlist(path: Path | None) -> set[str]:
    if path is None or not path.is_file():
        return set()
    return {
        ln.strip()
        for ln in path.read_text().splitlines()
        if ln.strip() and not ln.lstrip().startswith("#")
    }


def progress(done: int, total: int, start: float, game: Game) -> str:
    elapsed = time.monotonic() - start
    eta = (elapsed / done) * (total - done) if done else 0.0
    verdict = "error" if game.error else ("DIVERGE" if game.first_diff else "identical")
    return (
        f"[{done}/{total}] {100 * done / total:5.1f}% {elapsed:6.0f}s eta {eta:5.0f}s "
        f"seed={game.seed} {verdict} lines={game.lines}"
    )


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("--games", type=int, default=20)
    ap.add_argument("--seed", type=int, default=0, help="first seed; games use seed..seed+games-1")
    ap.add_argument("--workers", type=int, default=1, help="process pool size; 0 means every CPU")
    ap.add_argument(
        "--deck",
        choices=["pool", "vanilla", "shuffled"],
        default="pool",
        help="pool: a seeded shuffle of the whole pool; vanilla: the fixed --top; "
        "shuffled: the --top multiset (CODE:N,...) in a fresh seeded order per game",
    )
    ap.add_argument("--deck-size", type=int, default=40)
    ap.add_argument("--top", default="", help="CODE:N,CODE:N laid under the preset (bottom-to-top)")
    ap.add_argument("--max-steps", type=int, default=500_000)
    ap.add_argument("--allowlist", type=Path, default=None)
    ap.add_argument(
        "--context", type=int, default=0, help="print a repro command with this much context"
    )
    args = ap.parse_args()

    if not oracle_available():
        print("ocgcore is not built: run tools/oracle/build.sh", file=sys.stderr)
        return 2
    d.selfcheck()
    d.PORT_BIN = build_port()

    seeds = list(range(args.seed, args.seed + args.games))
    tasks = [
        (s, deck_spec(args.deck, args.top, args.deck_size, s), args.deck_size, args.max_steps)
        for s in seeds
    ]
    workers = args.workers if args.workers > 0 else (os.cpu_count() or 1)
    allow = load_allowlist(args.allowlist)

    start = time.monotonic()
    games: list[Game] = []
    if workers == 1:
        for t in tasks:
            games.append(_task(t))
            print(progress(len(games), len(tasks), start, games[-1]), file=sys.stderr, flush=True)
    else:
        with mp.get_context("fork").Pool(workers) as pool:
            for game in pool.imap_unordered(_task, tasks):
                games.append(game)
                print(progress(len(games), len(tasks), start, game), file=sys.stderr, flush=True)
    games.sort(key=lambda g: g.seed)

    diverged = [g for g in games if g.first_diff is not None]
    errors = [g for g in games if g.error]
    unlisted = [g for g in diverged if g.signature not in allow]
    for g in errors:
        print(f"seed {g.seed}: ERROR {g.error}\n  --top {g.spec}")
    for g in diverged:
        tag = "known" if g.signature in allow else "NEW"
        print(
            f"seed {g.seed}: diverged at line {g.first_diff} of {g.lines} [{tag}]\n"
            f"  {g.signature}\n  --top {g.spec}"
        )
        if args.context and tag == "NEW":
            print(
                f"  repro: tools/differential.py --policy random:{g.seed} "
                f"--deck-size {args.deck_size} --top {g.spec} --context {args.context}"
            )
    total_lines = sum(g.lines for g in games)
    print(
        f"\n{len(games)} games, {total_lines} trace lines, {time.monotonic() - start:.0f}s: "
        f"{len(games) - len(diverged) - len(errors)} identical, {len(diverged)} diverged "
        f"({len(unlisted)} new), {len(errors)} errors"
    )
    return 1 if unlisted or errors else 0


if __name__ == "__main__":
    raise SystemExit(main())

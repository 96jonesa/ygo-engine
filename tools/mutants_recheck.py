"""Re-test a named list of mutants with a longer oracle fuzz.

    python3 tools/mutants_recheck.py NAMES [--output DIR]
        [--jobs N] [--games N] [--timeout SECS] [--profile release|dev]

`NAMES` holds one cargo-mutants mutant name per line (as `cargo mutants
--list` prints them). Each becomes an anchored `--re` pattern, so exactly
those mutants run; "delete field" mutants are excluded because cargo-mutants
27 lets them through any `--re` filter. The oracle test plays `--games`
random pool-deck games per mutant (`PORT_ORACLE_FUZZ`).
"""

from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PORT = ROOT


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("names", type=Path)
    ap.add_argument("--output", type=Path, default=PORT / "mutants.out.recheck")
    ap.add_argument("--jobs", type=int, default=4)
    ap.add_argument("--games", type=int, default=200)
    ap.add_argument("--timeout", type=int, default=300)
    ap.add_argument("--profile", default="release")
    a = ap.parse_args()
    names = [ln.rstrip("\n") for ln in a.names.read_text().splitlines() if ln.strip()]
    names = [n for n in names if ": delete field " not in n]
    cmd = ["cargo", "mutants", "--jobs", str(a.jobs), "--timeout", str(a.timeout), "-e", "cards"]
    if a.profile != "dev":
        cmd += ["--profile", a.profile]
    cmd += ["-E", "delete field"]
    for n in names:
        cmd += ["--re", "^" + re.escape(n) + "$"]
    cmd += ["--output", str(a.output), "--", "--lib", "--test", "oracle_fuzz"]
    env = {
        **os.environ,
        "PORT_ORACLE_FUZZ": str(a.games),
        "PORT_ORACLE_REPO": os.environ.get("PORT_ORACLE_REPO", str(ROOT)),
        "CARGO_PROFILE_DEV_DEBUG": "0",
    }
    print(
        f"{len(names)} mutants, {a.games} games each, {a.jobs} jobs, profile {a.profile}",
        file=sys.stderr,
        flush=True,
    )
    return subprocess.run(cmd, cwd=PORT, env=env, check=False).returncode


if __name__ == "__main__":
    sys.exit(main())

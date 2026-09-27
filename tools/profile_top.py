"""Rank the port's functions by self, inclusive and allocator time.

    samply record --save-only -o bench.profile.json target/release/examples/bench ...
    python3 tools/profile_top.py bench.profile.json target/release/examples/bench

Reads a `samply --save-only` profile (Firefox Profiler format), symbolicates
the bench binary's frames in one `atos` batch (macOS, arm64; build with
`CARGO_PROFILE_RELEASE_DEBUG=1` so the symbols are there), demangles through
`rustfilt`, and prints three rankings: self time, inclusive time of the
port's own functions, and allocator samples attributed to the nearest port
caller — the last is what decides an optimisation step.
"""

from __future__ import annotations

import collections
import gzip
import json
import os
import re
import subprocess
import sys
from pathlib import Path
from typing import Any

BASE = 0x1_0000_0000  # a macOS arm64 executable's load address
PORT_FRAME = re.compile(r"Field>::|^driver::|^cards::|^<card::")


def load(profile: Path) -> dict[str, Any]:
    raw = profile.read_bytes()
    text = gzip.decompress(raw) if raw[:2] == b"\x1f\x8b" else raw
    data: dict[str, Any] = json.loads(text)
    return data


def symbolicate(binary: str, addrs: list[int]) -> dict[int, str]:
    """`atos` in batches; a v0-mangled name comes back without its `_R`."""
    out: dict[int, str] = {}
    for start in range(0, len(addrs), 400):
        part = addrs[start : start + 400]
        lines = subprocess.run(
            ["atos", "-o", binary, "-arch", "arm64", "-l", hex(BASE)]
            + [hex(BASE + a) for a in part],
            capture_output=True,
            text=True,
            check=False,
        ).stdout.splitlines()
        for a, line in zip(part, lines, strict=False):
            name = re.sub(r"\s*\(in [^)]*\)(\s*\+\s*\d+)?(\s*\([^)]*\))?\s*$", "", line.strip())
            name = re.sub(r"^_?\$?", "", name)
            if name.startswith(("RNv", "RIN", "RNC", "RC", "RM", "RX")):
                name = "_" + name
            out[a] = name
    return out


def demangle(names: list[str]) -> dict[str, str]:
    rustfilt = os.path.expanduser("~/.cargo/bin/rustfilt")
    if not os.path.exists(rustfilt):
        return {}
    done = subprocess.run(
        [rustfilt], input="\n".join(names), capture_output=True, text=True, check=False
    ).stdout.splitlines()
    return dict(zip(names, done, strict=False)) if len(done) == len(names) else {}


def short(name: str, demangled: dict[str, str]) -> str:
    n = demangled.get(name, name)
    n = re.sub(r"::h[0-9a-f]{16}$", "", n)
    n = re.sub(r"<([^<>]*) as [^<>]*>", r"\1", n)
    n = n.replace("ygo_engine::", "").replace("alloc::collections::btree::map::", "btree::")
    n = n.replace("alloc::vec::", "vec::").replace("core::", "")
    return n[:90]


def main() -> int:
    profile, binary = Path(sys.argv[1]), sys.argv[2]
    data = load(profile)
    thread = max(data["threads"], key=lambda th: th["samples"]["length"])
    strings = data["shared"]["stringArray"] if "shared" in data else thread["stringArray"]
    del strings  # names come from the binary, not the profile's string table
    libs = data["libs"]
    frames, funcs, stacks_t, resources = (
        thread["frameTable"],
        thread["funcTable"],
        thread["stackTable"],
        thread["resourceTable"],
    )
    frame_lib: list[str] = []
    for i in range(frames["length"]):
        res = funcs["resource"][frames["func"][i]]
        frame_lib.append(libs[resources["lib"][res]]["name"] if res >= 0 else "?")
    addrs = sorted(
        {
            frames["address"][i]
            for i in range(frames["length"])
            if frame_lib[i] == "bench" and frames["address"][i] >= 0
        }
    )
    sym = symbolicate(binary, addrs)
    demangled = demangle(sorted(set(sym.values())))

    def frame_name(i: int) -> str:
        if frame_lib[i] == "bench":
            return short(
                sym.get(frames["address"][i], f"bench+{frames['address'][i]:#x}"), demangled
            )
        return f"[{frame_lib[i]}]"

    self_t: collections.Counter[str] = collections.Counter()
    incl_t: collections.Counter[str] = collections.Counter()
    alloc_by: collections.Counter[str] = collections.Counter()
    total = 0
    alloc_total = 0
    stacks = thread["samples"]["stack"]
    weights = thread["samples"].get("weight") or [1] * len(stacks)
    for s, w in zip(stacks, weights, strict=False):
        if s is None:
            continue
        w = w or 1
        total += w
        leaf = frame_name(stacks_t["frame"][s])
        self_t[leaf] += w
        seen: set[str] = set()
        cur = s
        while cur is not None and cur >= 0:
            nm = frame_name(stacks_t["frame"][cur])
            if nm not in seen:
                incl_t[nm] += w
                seen.add(nm)
            cur = stacks_t["prefix"][cur]
        is_alloc = (
            leaf.startswith("[libsystem_malloc")
            or "__rdl_alloc" in leaf
            or leaf.startswith("DYLD-STUB$$free")
        )
        if is_alloc:
            alloc_total += w
            owner = "?"
            cur = stacks_t["prefix"][s]
            while cur is not None and cur >= 0:
                nm = frame_name(stacks_t["frame"][cur])
                if PORT_FRAME.search(nm):
                    owner = nm
                    break
                cur = stacks_t["prefix"][cur]
            alloc_by[owner] += w

    print(f"samples: {total}  (symbolicated {len(sym)} of {len(addrs)} bench addresses)")
    print("\n== self time (top 32) ==")
    for n, c in self_t.most_common(32):
        print(f"{100 * c / total:5.1f}%  {n}")
    print("\n== inclusive time, port functions only (top 40) ==")
    shown = 0
    for n, c in incl_t.most_common(200):
        if PORT_FRAME.search(n):
            print(f"{100 * c / total:5.1f}%  {n}")
            shown += 1
            if shown == 40:
                break
    share = 100 * alloc_total / max(total, 1)
    print(f"\n== allocator samples by nearest port caller ({share:.1f}% of all samples) ==")
    for n, c in alloc_by.most_common(22):
        print(f"{100 * c / max(alloc_total, 1):5.1f}%  {n}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

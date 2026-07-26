#!/usr/bin/env python3
"""Per-crate line-coverage floor over an lcov tracefile.

The coverage-unit CI job gates the workspace aggregate; this asserts the
per-crate invariant PRODUCTION-READINESS.md claims ("every crate >= 92%"),
so a small crate cannot erode while the aggregate stays green (#17).

Usage: per-crate-coverage.py <lcov-file> <floor-percent>

Run from the repository root: the expected crate set is enumerated from
the crates/ directory on disk, and a crate that is MISSING from the
tracefile (or present with zero measured lines) fails the gate — no
evidence is not passing evidence, and a crate forgotten from the
coverage job's -p list must not slip past a gate that claims "every
crate". Crate = first path component after crates/ in each SF: record;
records outside crates/ are ignored. Exits 1 listing every crate below
the floor, unmeasured, or missing.
"""

import os
import sys
from collections import defaultdict


def main() -> int:
    if len(sys.argv) != 3:
        print(__doc__, file=sys.stderr)
        return 2
    lcov_path, floor = sys.argv[1], float(sys.argv[2])

    expected = sorted(
        entry.name
        for entry in os.scandir("crates")
        if entry.is_dir() and not entry.name.startswith(".")
    )
    if not expected:
        print(
            "no crate directories found under crates/ — run from the repo root",
            file=sys.stderr,
        )
        return 2

    found = defaultdict(int)
    hit = defaultdict(int)
    crate = None
    with open(lcov_path, encoding="utf-8") as lcov:
        for line in lcov:
            line = line.strip()
            if line.startswith("SF:"):
                path = line[3:].replace("\\", "/")
                crate = None
                if "crates/" in path:
                    tail = path.split("crates/", 1)[1]
                    crate = tail.split("/", 1)[0]
            elif line.startswith("LF:") and crate:
                found[crate] += int(line[3:])
            elif line.startswith("LH:") and crate:
                hit[crate] += int(line[3:])

    failed = []
    for name in expected:
        if found[name] == 0:
            state = "0 measured lines" if name in found else "MISSING from tracefile"
            print(f"{name:<20}    --      ({state})  <-- UNMEASURED")
            failed.append(name)
            continue
        pct = 100.0 * hit[name] / found[name]
        marker = "" if pct >= floor else "  <-- BELOW FLOOR"
        print(f"{name:<20} {pct:6.2f}%  ({hit[name]}/{found[name]} lines){marker}")
        if pct < floor:
            failed.append(name)

    for name in sorted(set(found) - set(expected)):
        print(f"{name:<20} (in tracefile but not under crates/ — ignored)")

    if failed:
        print(f"\nper-crate gate ({floor}% floor) failed for: {', '.join(failed)}")
        return 1
    print(f"\nall {len(expected)} crates measured and meet the {floor}% floor")
    return 0


if __name__ == "__main__":
    sys.exit(main())

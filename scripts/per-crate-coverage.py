#!/usr/bin/env python3
"""Per-crate line-coverage floor over an lcov tracefile.

The coverage-unit CI job gates the workspace aggregate; this asserts the
per-crate invariant PRODUCTION-READINESS.md claims ("every crate >= 92%"),
so a small crate cannot erode while the aggregate stays green (#17).

Usage: per-crate-coverage.py <lcov-file> <floor-percent>

Crate = first path component after crates/ in each SF: record. Files
outside crates/ (if any appear) are ignored. Exits 1 listing every crate
below the floor.
"""

import sys
from collections import defaultdict


def main() -> int:
    if len(sys.argv) != 3:
        print(__doc__, file=sys.stderr)
        return 2
    lcov_path, floor = sys.argv[1], float(sys.argv[2])

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

    if not found:
        print(f"no crates/ records found in {lcov_path}", file=sys.stderr)
        return 1

    failed = []
    for name in sorted(found):
        pct = 100.0 * hit[name] / found[name] if found[name] else 100.0
        marker = "" if pct >= floor else "  <-- BELOW FLOOR"
        print(f"{name:<20} {pct:6.2f}%  ({hit[name]}/{found[name]} lines){marker}")
        if pct < floor:
            failed.append(name)

    if failed:
        print(f"\nper-crate floor {floor}% violated by: {', '.join(failed)}")
        return 1
    print(f"\nall crates meet the {floor}% floor")
    return 0


if __name__ == "__main__":
    sys.exit(main())

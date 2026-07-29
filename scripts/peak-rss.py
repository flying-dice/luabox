#!/usr/bin/env python3
"""Print the peak resident set size, in MiB, of a command run to completion.

Used by scripts/perf-gate.sh's memory leg (decisions/07 accepted a ~1.9x RSS
trade on the 100-kLOC corpus; this is what turns that number into a gate).

Why not `/usr/bin/time -v`: GNU time is present on GitHub's ubuntu runners but
not in every container the gate is run in, and BSD `time` on macOS spells its
flags differently again. `wait4(2)`'s rusage is the thing GNU time itself
reports, and Python's stdlib exposes it directly — one mechanism, no packages,
identical on Linux and macOS. The interpreter is already a CI dependency
(scripts/per-crate-coverage.py runs in the coverage job).

The child's own stdout/stderr are discarded: this measures the toolchain, not
the terminal, exactly as the timed legs do.

Usage: peak-rss.py <command> [args...]
Prints one integer (MiB) on stdout; exits nonzero if the command could not be
run. The command's own exit status is deliberately ignored — `fmt --check` and
friends legitimately fail on the synthetic corpus, and only the memory matters.
"""

import resource
import subprocess
import sys


def main() -> int:
    if len(sys.argv) < 2:
        print("usage: peak-rss.py <command> [args...]", file=sys.stderr)
        return 2

    try:
        subprocess.run(
            sys.argv[1:],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
        )
    except OSError as error:
        print(f"peak-rss.py: cannot run {sys.argv[1]}: {error}", file=sys.stderr)
        return 1

    # This process waits for exactly one child, so RUSAGE_CHILDREN's high-water
    # mark is that child's. `ru_maxrss` is KiB on Linux and bytes on macOS —
    # the one portability wart in the whole mechanism.
    peak = resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss
    mib = peak / (1024 * 1024) if sys.platform == "darwin" else peak / 1024
    print(int(mib))
    return 0


if __name__ == "__main__":
    sys.exit(main())

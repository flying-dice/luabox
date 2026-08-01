#!/bin/bash
# LB0510 shape matrix: for every program in scripts/tests/lb0510-matrix/, the
# lint verdict and the RUNTIME verdict are both re-derived and compared to the
# committed expectations in that directory's expected.tsv.
#
# Why this exists. `metatable-without-index` is a rule about what a program
# does when it runs, and its accuracy has been argued four review rounds
# running from a shape matrix that lived only in board prose and doc comments.
# Prose cannot be re-run. This can:
#
#   lint column     `luabox lint` over a one-file project, counting LB0510
#                   diagnostics. A count, not a flag, so one-finding-per-
#                   setmetatable-site is pinned too.
#   runtime column  `lua5.4 <program>`; a non-zero exit is `crash`, zero is
#                   `ok`. This is the ground truth the rule's false-positive
#                   and false-negative claims are about.
#
# The two columns are NOT asserted equal, and must not be: the no-metafield
# arm deliberately over-approximates (it fires on a construction whose methods
# this file never calls, which does not crash). What the script enforces is
# that both columns match what is written down — so a change in either
# direction shows up as a diff against a claim, and the claims in
# docs/03-reference/02-limitations.md stop being unfalsifiable.
#
# No lua5.4 on PATH means the runtime column SKIPs, loudly, and the lint
# column still runs. CI's differential job installs 5.1-5.4, so both columns
# run there.
set -u

here="$(cd "$(dirname "$0")" && pwd)"
matrix="$here/lb0510-matrix"
expected="$matrix/expected.tsv"
luabox="${LUABOX:-$here/../../target/release/luabox}"
lua="${LUA54:-lua5.4}"

if [ ! -x "$luabox" ]; then
    echo "error: no luabox binary at $luabox (build with: cargo build --release --bin luabox, or set LUABOX)" >&2
    exit 1
fi
if [ ! -f "$expected" ]; then
    echo "error: no expectations at $expected" >&2
    exit 1
fi

runtime_column=1
if ! command -v "$lua" >/dev/null 2>&1; then
    runtime_column=0
    echo "SKIP  runtime column: no $lua on PATH — the crash/ok half of every"
    echo "SKIP  claim below is UNVERIFIED in this run. Install lua5.4 (or set"
    echo "SKIP  LUA54) to check it; CI's differential job always does."
    echo
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/src"
printf '[package]\nname = "lb0510mx"\nversion = "0.1.0"\nedition = "5.4"\n' \
    > "$work/luabox.toml"

fails=0
rows=0
declare -A seen=()

printf '%-42s %-18s %s\n' "SHAPE" "LINT (findings)" "RUNTIME"
printf '%-42s %-18s %s\n' "-----" "---------------" "-------"

while IFS=$'\t' read -r program want_findings want_runtime note; do
    case "${program:-}" in ''|'#'*) continue ;; esac
    rows=$((rows + 1))
    seen["$program"]=1
    prog="$matrix/$program.lua"
    if [ ! -f "$prog" ]; then
        echo "FAIL  $program: expected.tsv names it, but $prog does not exist" >&2
        fails=$((fails + 1))
        continue
    fi

    # --- lint column -----------------------------------------------------
    cp "$prog" "$work/src/main.lua"
    out="$( (cd "$work" && "$luabox" lint) 2>&1 )"
    got_findings="$(printf '%s\n' "$out" | grep -c 'LB0510')"

    # --- runtime column --------------------------------------------------
    got_runtime=skipped
    if [ "$runtime_column" -eq 1 ]; then
        # Exit status is the measurement; the program's own output is noise.
        if "$lua" "$prog" >/dev/null 2>&1; then
            got_runtime=ok
        else
            got_runtime=crash
        fi
    fi

    printf '%-42s %-18s %s\n' "$program" "$got_findings" "$got_runtime"

    if [ "$got_findings" != "$want_findings" ]; then
        echo "FAIL  $program: lint emitted $got_findings LB0510 finding(s), expected $want_findings" >&2
        echo "      ($note)" >&2
        printf '%s\n' "$out" | sed 's/^/      | /' >&2
        fails=$((fails + 1))
    fi
    if [ "$runtime_column" -eq 1 ] && [ "$got_runtime" != "$want_runtime" ]; then
        echo "FAIL  $program: $lua says $got_runtime, expected $want_runtime" >&2
        echo "      ($note)" >&2
        echo "      The runtime column is ground truth — if the interpreter" >&2
        echo "      disagrees with expected.tsv, the CLAIM is what is wrong." >&2
        fails=$((fails + 1))
    fi
done < "$expected"

# A program with no row is a shape somebody added and nobody pinned.
for prog in "$matrix"/*.lua; do
    base="$(basename "$prog" .lua)"
    if [ -z "${seen[$base]:-}" ]; then
        echo "FAIL  $base.lua has no row in expected.tsv (every shape must be pinned)" >&2
        fails=$((fails + 1))
    fi
done

echo
if [ "$rows" -eq 0 ]; then
    echo "lb0510 matrix: NO ROWS RAN (empty expected.tsv?)" >&2
    exit 1
fi
if [ "$fails" -gt 0 ]; then
    echo "lb0510 matrix: $fails disagreement(s) across $rows shape(s)" >&2
    exit 1
fi
if [ "$runtime_column" -eq 1 ]; then
    echo "lb0510 matrix: all $rows shapes match, lint AND runtime ($($lua -v 2>&1 | head -n 1))"
else
    echo "lb0510 matrix: all $rows shapes match on the lint column; runtime column SKIPPED"
fi

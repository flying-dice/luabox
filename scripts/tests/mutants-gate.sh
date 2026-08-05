#!/bin/bash
# Mutation gate (#58): run cargo-mutants over luabox-types and compare the
# surviving ("missed") mutants against the committed expectations in
# scripts/tests/mutants-allowlist.txt.
#
# Why this exists. The wave-21 lesson from PR #55: a fixture that cannot
# fail against the defect it names converts absence-of-signal into false
# confidence — the reviewer found three latent bugs in code that had
# "passing tests". A mutant that survives is exactly such a fixture gap:
# the code changed behaviour and no test noticed.
#
# The allowlist is the same discipline as every other committed-expectation
# file in scripts/tests/: a survivor either gets a test that kills it, or a
# reviewed line here saying why it is acceptable (defensive arm, display
# nicety, semantically-equal rewrite). What the gate enforces is that the
# SET matches what is written down — a NEW survivor fails the job, and a
# survivor that a new test kills shows up as a stale allowlist line to
# prune (reported, not failed: killing a waived mutant is progress).
#
# Scope: the FILES env var (comma-separated, default the merge-seam
# neighbourhood where the #46 family lived). The scheduled CI job pins the
# same set explicitly rather than widening it — widening waits on #60, whose
# finding is that check.rs's fallback is largely shadowed by inference, so
# auditing it before that cleanup would allowlist noise rather than kill it.
# See .github/workflows/mutants.yml. Not per-PR either way: a full run costs
# tens of minutes, which is why it rides a schedule instead of the merge path.
set -u

here="$(cd "$(dirname "$0")" && pwd)"
repo="$here/../.."
allowlist="$here/mutants-allowlist.txt"
files="${FILES:-crates/luabox-types/src/env.rs,crates/luabox-types/src/defs.rs,crates/luabox-types/src/generics.rs,crates/luabox-types/src/infer/reify.rs}"

if ! command -v cargo-mutants >/dev/null 2>&1; then
    echo "error: cargo-mutants not installed (cargo install cargo-mutants --locked)" >&2
    exit 1
fi
if [ ! -f "$allowlist" ]; then
    echo "error: no allowlist at $allowlist" >&2
    exit 1
fi

out_dir="${MUTANTS_OUT:-$(mktemp -d)}"
file_args=()
IFS=',' read -ra parts <<<"$files"
for f in "${parts[@]}"; do
    file_args+=(--file "$f")
done

echo "mutants-gate: cargo mutants -p luabox-types ${file_args[*]} (this takes a while)"
# Exit 3 = missed mutants, exit 4 = only timeouts. Both are judged below by
# the allowlist comparison rather than by the exit code; anything else is a
# real failure.
(cd "$repo" && cargo mutants -p luabox-types "${file_args[@]}" -o "$out_dir")
status=$?
case "$status" in
0 | 3 | 4) ;;
*)
    echo "error: cargo-mutants failed with exit $status" >&2
    exit "$status"
    ;;
esac

missed="$out_dir/mutants.out/missed.txt"
[ -f "$missed" ] || missed=/dev/null
# A TIMED-OUT mutant is not a killed one: no test proved it dead, the run just
# stopped waiting. Judging `missed.txt` alone would let a loaded runner — where
# every survivor happens to time out — print "0 survivors" and exit 0, which is
# the exact failure mode this gate exists to catch (an absence of signal read
# as coverage). So timeouts are judged by the same allowlist: an already-
# reviewed line that flaps missed -> timeout stays reviewed, a NEW timed-out
# mutant is an unjudged one and fails.
timeout="$out_dir/mutants.out/timeout.txt"
[ -f "$timeout" ] || timeout=/dev/null

# Column 1 of the allowlist is the exact cargo-mutants missed line; the
# reviewed reason rides after a tab and is stripped for comparison.
waived="$(mktemp)"
grep -v '^#' "$allowlist" | cut -f1 | grep . >"$waived" || true

fails=0
new=0
while IFS= read -r line; do
    [ -n "$line" ] || continue
    if ! grep -qxF "$line" "$waived"; then
        echo "FAIL  new surviving mutant (no test kills it, no allowlist line): $line" >&2
        fails=$((fails + 1))
        new=$((new + 1))
    fi
done <"$missed"

new_timeouts=0
while IFS= read -r line; do
    [ -n "$line" ] || continue
    if grep -qxF "$line" "$waived"; then
        echo "NOTE  allowlist line timed out this run rather than surviving outright: $line"
    else
        echo "FAIL  new timed-out mutant (no test killed it — the run stopped waiting): $line" >&2
        fails=$((fails + 1))
        new_timeouts=$((new_timeouts + 1))
    fi
done <"$timeout"

# Stale = the allowlist line neither survived NOR timed out, i.e. a test now
# kills it. A line that flapped to the timeout list is still unkilled, so it is
# not stale and must not be pruned.
stale=0
while IFS= read -r line; do
    [ -n "$line" ] || continue
    if ! grep -qxF "$line" "$missed" && ! grep -qxF "$line" "$timeout"; then
        echo "NOTE  allowlist line no longer survives (a test now kills it — prune it): $line"
        stale=$((stale + 1))
    fi
done <"$waived"
rm -f "$waived"

# `grep -c` prints its count and exits 1 when that count is zero, so the
# non-zero status is swallowed rather than answered with a second "0".
count_lines() { grep -c . "$1" 2>/dev/null || true; }
total="$(count_lines "$missed")"
timed_out="$(count_lines "$timeout")"
echo
echo "mutants-gate: $total survivor(s), $timed_out timed out, $new new, $new_timeouts new timed out, $stale stale allowlist line(s)"
if [ "$fails" -gt 0 ]; then
    echo "mutants-gate: FAILED — kill each new survivor with a test or add a reviewed allowlist line"
    if [ "$new_timeouts" -gt 0 ]; then
        echo "mutants-gate:   a timed-out mutant is unkilled, not killed — give it a faster test,"
        echo "mutants-gate:   raise --timeout, or waive it with a reviewed reason like any survivor"
    fi
    exit 1
fi
echo "mutants-gate: OK ($total survivor(s) and $timed_out timeout(s) all reviewed)"

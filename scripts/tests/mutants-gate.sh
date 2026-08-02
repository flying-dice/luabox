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
# neighbourhood where the #46 family lived). The scheduled CI job widens it;
# see .github/workflows/mutants.yml. Not per-PR: a full run costs tens of
# minutes, which is why it rides a schedule instead of the merge path.
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
# Exit 3 = missed mutants (judged by the allowlist comparison below), exit 4
# = only timeouts. Anything else is a real failure. A mutant on the
# timeout/missed boundary can flap between runs on a loaded machine; the
# allowlist judges MISSED lines only, so a flap toward timeout reads as a
# stale allowlist NOTE, never a failure.
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

stale=0
while IFS= read -r line; do
    [ -n "$line" ] || continue
    if ! grep -qxF "$line" "$missed"; then
        echo "NOTE  allowlist line no longer survives (a test now kills it — prune it): $line"
        stale=$((stale + 1))
    fi
done <"$waived"
rm -f "$waived"

total="$(grep -c . "$missed" 2>/dev/null || echo 0)"
echo
echo "mutants-gate: $total survivor(s), $new new, $stale stale allowlist line(s)"
if [ "$fails" -gt 0 ]; then
    echo "mutants-gate: FAILED — kill each new survivor with a test or add a reviewed allowlist line"
    exit 1
fi
echo "mutants-gate: OK (survivors all reviewed)"

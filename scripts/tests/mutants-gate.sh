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
# A gate that can only report "reviewed" is the same failure mode one level
# up, so three ways of auditing NOTHING are failures here, not silence:
#   - a scoped file that does not exist (a rename or module move leaves the
#     hardcoded path matching nothing; cargo-mutants accepts such a --file
#     and generates no mutants, exit 0);
#   - a run that generated no mutants at all, whatever the reason;
#   - an allowlist every one of whose lines went stale in a single run,
#     which is what an audit pointed somewhere else looks like.
# `luals-differential.sh` enforces the same rule on the same class of gap:
# an unclaimed case is coverage that silently is not.
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
# ALLOWLIST is a seam for mutants-gate-selftest.sh, which drives this gate
# against fixture expectations and a stub cargo-mutants; CI and every human
# run take the default.
allowlist="${ALLOWLIST:-$here/mutants-allowlist.txt}"
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
# The scope is a hardcoded path string here and in the workflow, and neither
# is updated by a rename. cargo-mutants does not object to a --file matching
# nothing — it lists nothing and exits 0 — so the audit would run empty and
# the job would pass. Check the paths before spending the run.
for f in "${parts[@]}"; do
    if [ ! -f "$repo/$f" ]; then
        echo "error: scoped file does not exist: $f" >&2
        echo "error:   FILES / .github/workflows/mutants.yml still name a path this repo does not have;" >&2
        echo "error:   a rename or module move empties the audit without failing it. Re-point the scope." >&2
        exit 1
    fi
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

results="$out_dir/mutants.out"
outcome_file() { [ -f "$results/$1" ] && echo "$results/$1" || echo /dev/null; }
missed="$(outcome_file missed.txt)"
# A TIMED-OUT mutant is not a killed one: no test proved it dead, the run just
# stopped waiting. Judging `missed.txt` alone would let a loaded runner — where
# every survivor happens to time out — print "0 survivors" and exit 0, which is
# the exact failure mode this gate exists to catch (an absence of signal read
# as coverage). So timeouts are judged by the same allowlist: an already-
# reviewed line that flaps missed -> timeout stays reviewed, a NEW timed-out
# mutant is an unjudged one and fails.
timeout="$(outcome_file timeout.txt)"
caught="$(outcome_file caught.txt)"
unviable="$(outcome_file unviable.txt)"

# `grep -c` prints its count and exits 1 when that count is zero, so the
# non-zero status is swallowed rather than answered with a second "0".
count_lines() { grep -c . "$1" 2>/dev/null || true; }
generated=$(($(count_lines "$caught") + $(count_lines "$missed") + $(count_lines "$timeout") + $(count_lines "$unviable")))
if [ "$generated" -eq 0 ]; then
    echo
    echo "mutants-gate: FAILED — the run generated 0 mutants, so nothing was audited" >&2
    echo "mutants-gate:   an empty run is not a clean one. Check the scope ($files) and that" >&2
    echo "mutants-gate:   cargo-mutants can build the package." >&2
    exit 1
fi

# Column 1 of the allowlist is the cargo-mutants line as first seen; the
# reviewed reason rides after a tab and is stripped for comparison.
waived="$(mktemp)"
grep -v '^#' "$allowlist" | cut -f1 | grep . >"$waived" || true
waived_total="$(count_lines "$waived")"

# The comparison is keyed on (file, mutant text) with `line:col` INFORMATIONAL,
# because an edit above a waived mutant shifts its position while the mutant
# itself is unchanged. Keying on the whole line made a shift indistinguishable
# from a real survivor at the moment of judgement — the reviewer diffed texts
# by eye and a mixed batch (4 shifts + 1 genuinely new mutant, 2026-08-05)
# looks exactly like a clean shift batch in the counts. Here the pairing is
# measured: a live mutant whose (file, text) matches an unclaimed waived line
# is a SHIFT, and only a live mutant with no such match at all is NEW.
# Duplicate (file, text) pairs exist (two `replace > with >=` mutants in one
# function), so the match is a multiset: same-position pairs claim each other
# first, then the leftovers pair up as shifts, and whatever is still unpaired
# is new (live side) or stale (allowlist side).
classified="$(mktemp)"
{
    awk '{ print "W\t" $0 }' "$waived"
    awk '{ print "M\t" $0 }' "$missed"
    awk '{ print "T\t" $0 }' "$timeout"
} | awk -F'\t' '
function key(s) {
    if (match(s, /:[0-9]+:[0-9]+: /)) return substr(s, 1, RSTART - 1) SUBSEP substr(s, RSTART + RLENGTH)
    return s SUBSEP ""
}
function pos(s) {
    if (match(s, /:[0-9]+:[0-9]+: /)) return substr(s, RSTART + 1, RLENGTH - 3)
    return "?"
}
{
    k = key($2)
    keys[k] = 1
    if ($1 == "W") { wline[k, ++w[k]] = $2; wpos[k, w[k]] = pos($2) }
    else           { lline[k, ++l[k]] = $2; lpos[k, l[k]] = pos($2); lkind[k, l[k]] = $1 }
}
END {
    for (k in keys) {
        nw = w[k] + 0; nl = l[k] + 0
        # Pass 1: identical position claims its own waived line.
        for (i = 1; i <= nl; i++) {
            for (j = 1; j <= nw; j++) {
                if (!wtaken[k, j] && wpos[k, j] == lpos[k, i]) {
                    wtaken[k, j] = 1; ltaken[k, i] = 1
                    if (lkind[k, i] == "T") print "TIMEOUT\t" lline[k, i]
                    break
                }
            }
        }
        # Pass 2: same mutant, moved — measured as a pairing, not eyeballed.
        for (i = 1; i <= nl; i++) {
            if (ltaken[k, i]) continue
            for (j = 1; j <= nw; j++) {
                if (wtaken[k, j]) continue
                wtaken[k, j] = 1; ltaken[k, i] = 1
                print "SHIFT\t" wline[k, j] "\t" lline[k, i]
                if (lkind[k, i] == "T") print "TIMEOUT\t" lline[k, i]
                break
            }
        }
        # Whatever is left is genuinely new, or genuinely killed.
        for (i = 1; i <= nl; i++) if (!ltaken[k, i]) print "NEW\t" lkind[k, i] "\t" lline[k, i]
        for (j = 1; j <= nw; j++) if (!wtaken[k, j]) print "STALE\t" wline[k, j]
    }
}
' | sort >"$classified"
# `for (k in keys)` walks awk's hash in an arbitrary order, so the sort is what
# makes two runs over the same data print the same report — a gate whose output
# reshuffles between runs is one nobody can diff.
rm -f "$waived"

fails=0
new=0
new_timeouts=0
shifts=0
stale=0
repin="$(mktemp)"
while IFS=$'\t' read -r kind a b; do
    case "$kind" in
    NEW)
        if [ "$a" = "T" ]; then
            echo "FAIL  new timed-out mutant (no test killed it — the run stopped waiting): $b" >&2
            new_timeouts=$((new_timeouts + 1))
        else
            echo "FAIL  new surviving mutant (no test kills it, no allowlist line): $b" >&2
            new=$((new + 1))
        fi
        fails=$((fails + 1))
        ;;
    SHIFT)
        echo "NOTE  waived mutant moved (same file and mutation, new position — re-pin, do not re-review):"
        echo "        was: $a"
        echo "        now: $b"
        printf '%s\t%s\n' "$a" "$b" >>"$repin"
        shifts=$((shifts + 1))
        ;;
    TIMEOUT)
        echo "NOTE  allowlist line timed out this run rather than surviving outright: $a"
        ;;
    STALE)
        # Stale = the allowlist line neither survived NOR timed out anywhere,
        # i.e. a test now kills it. A line that only moved is a SHIFT above and
        # is still unkilled, so it is not stale and must not be pruned.
        echo "NOTE  allowlist line no longer survives (a test now kills it — prune it): $a"
        stale=$((stale + 1))
        ;;
    esac
done <"$classified"
rm -f "$classified"

# Every reviewed line going stale at once is not 17 test wins in one week; it
# is an audit that ran somewhere else — the shape a scope drift takes when the
# paths still exist (a module split, a package rename) and the file check above
# cannot see it.
if [ "$waived_total" -gt 0 ] && [ "$stale" -eq "$waived_total" ]; then
    echo
    echo "mutants-gate: FAILED — every one of the $waived_total reviewed lines went stale in one run" >&2
    echo "mutants-gate:   that is what an audit pointed at the wrong scope looks like. Confirm the" >&2
    echo "mutants-gate:   scope ($files) before pruning anything." >&2
    fails=$((fails + 1))
fi

total="$(count_lines "$missed")"
timed_out="$(count_lines "$timeout")"
echo
echo "mutants-gate: $generated mutant(s) generated, $total survivor(s), $timed_out timed out, $new new, $new_timeouts new timed out, $shifts shifted, $stale stale allowlist line(s)"
if [ "$shifts" -gt 0 ]; then
    echo "mutants-gate: re-pin the $shifts shifted line(s) — replace column 1, reasons unchanged:"
    while IFS=$'\t' read -r was now; do
        echo "     old: $was"
        echo "     new: $now"
    done <"$repin"
fi
rm -f "$repin"
if [ "$fails" -gt 0 ]; then
    echo "mutants-gate: FAILED — kill each new survivor with a test or add a reviewed allowlist line"
    if [ "$new_timeouts" -gt 0 ]; then
        echo "mutants-gate:   a timed-out mutant is unkilled, not killed — give it a faster test,"
        echo "mutants-gate:   raise --timeout, or waive it with a reviewed reason like any survivor"
    fi
    exit 1
fi
echo "mutants-gate: OK ($total survivor(s) and $timed_out timeout(s) all reviewed, $generated generated)"

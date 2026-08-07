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
# neighbourhood where the #46 family lived). The scheduled CI job sets no
# FILES at all and deliberately relies on this default — see the "No FILES="
# comment on the `mutants` job in .github/workflows/mutants.yml, which
# retired the job's own explicit pin because two hand-maintained copies of
# the same scope were free to drift apart with nothing to notice (F4).
# Widening the scope waits on #60, whose finding is that check.rs's fallback
# is largely shadowed by inference, so auditing it before that cleanup would
# allowlist noise rather than kill it. Not per-PR either way: a full run
# costs tens of minutes, which is why it rides a schedule instead of the
# merge path.
set -u

here="$(cd "$(dirname "$0")" && pwd)"
repo="$here/../.."
# ALLOWLIST is a seam for mutants-gate-selftest.sh, which drives this gate
# against fixture expectations and a stub cargo-mutants; CI and every human
# run take the default.
allowlist="${ALLOWLIST:-$here/mutants-allowlist.txt}"
# default_files is the scope of record — see SCOPE_OF_RECORD below — as well
# as FILES' own default, from the one place both are written down.
default_files="crates/luabox-types/src/env.rs,crates/luabox-types/src/defs.rs,crates/luabox-types/src/generics.rs,crates/luabox-types/src/infer/reify.rs"
files="${FILES:-$default_files}"

if ! command -v cargo-mutants >/dev/null 2>&1; then
    echo "error: cargo-mutants not installed (cargo install cargo-mutants --locked)" >&2
    exit 1
fi
if [ ! -f "$allowlist" ]; then
    echo "error: no allowlist at $allowlist" >&2
    exit 1
fi
# Small scratch files created below (waived/classified/repin); an early exit
# anywhere past this point must not leave them behind. This does NOT touch
# out_dir — that is the actual report (mutants.out), which stays reachable
# on disk on every exit path; see the echo right after out_dir is resolved.
trap 'rm -f "${waived:-}" "${classified:-}" "${repin:-}" 2>/dev/null' EXIT

out_dir="${MUTANTS_OUT:-$(mktemp -d)}"
# cargo-mutants runs inside `(cd "$repo" && ...)` below, but out_dir is read
# back out here against the CALLER's cwd — a relative MUTANTS_OUT would be
# written under $repo and looked for somewhere else entirely, which reads as
# "the run generated 0 mutants" only after paying for the whole run.
# luals-differential.sh:52-55 resolves LUABOX the same way for the same
# reason; mktemp -d is already absolute, so this is a no-op in the common
# (unset MUTANTS_OUT) case.
case "$out_dir" in
/*) ;;
*) out_dir="$(cd "$(dirname "$out_dir")" && pwd)/$(basename "$out_dir")" ;;
esac
echo "mutants-gate: report directory: $out_dir"
file_args=()
IFS=',' read -ra parts <<<"$files"
# The scope is a hardcoded path string here — the one owner now that the
# workflow's own FILES pin has been retired (see the Scope note above) —
# and it is not updated by a rename. cargo-mutants does not object to a
# --file matching nothing — it lists nothing and exits 0 — so the audit
# would run empty and the job would pass. Check the paths before spending
# the run.
for f in "${parts[@]}"; do
    if [ ! -f "$repo/$f" ]; then
        echo "error: scoped file does not exist: $f" >&2
        echo "error:   FILES (or this script's default, if FILES is unset) still names a path this repo does not have;" >&2
        echo "error:   a rename or module move empties the audit without failing it. Re-point the scope." >&2
        exit 1
    fi
    file_args+=(--file "$f")
done

# The check above only catches a scope naming a path that does not exist. It
# says nothing about a scope that was narrowed while every path named still
# exists. Two different things can be narrowed away from, so two checks:
#
# missing_from_scope echoes, space-separated, which lines of $2 (newline-
# separated paths) are absent from $parts (the FILES actually being audited
# this run) — empty means fully covered.
missing_from_scope() {
    local list="$1" out="" candidate f found
    while IFS= read -r candidate; do
        [ -n "$candidate" ] || continue
        found=0
        for f in "${parts[@]}"; do
            [ "$f" = "$candidate" ] && found=1 && break
        done
        [ "$found" -eq 1 ] || out="$out $candidate"
    done <<<"$list"
    printf '%s' "$out"
}

# Check 1: FILES must be a superset of the SCOPE OF RECORD — the documented,
# canonical audit surface (default_files above, the scope the scheduled job
# in .github/workflows/mutants.yml inherits by leaving FILES unset). This is
# the check
# missing before #58 review round 4 (R18/R19): the allowlist-only check
# below asks "does this run re-measure every waiver on record", which a
# narrowing can satisfy by accident when every current waiver happens to sit
# in one file — all 17 lines in mutants-allowlist.txt are in env.rs today, so
# FILES=crates/luabox-types/src/env.rs alone passed that check while auditing
# a quarter of the four-file surface this file's own header and CI both
# claim, giving 0 new / 0 stale / exit 0 for the three files left out
# entirely. The allowlist is a fact about what has been REVIEWED, not a
# definition of what the gate is FOR; the scope of record is that
# definition, and it does not shrink just because every current waiver
# happens to fit inside a narrower FILES. SCOPE_OF_RECORD is a seam for
# mutants-gate-selftest.sh, exactly like ALLOWLIST above — every other run
# takes the default, which is default_files itself, so a plain human run and
# CI (which sets no FILES and so also takes that default) both compare
# against the one scope this file and the workflow agree on.
scope_of_record="${SCOPE_OF_RECORD:-$default_files}"
missing_scope_of_record="$(missing_from_scope "$(printf '%s' "$scope_of_record" | tr ',' '\n')")"
if [ -n "$missing_scope_of_record" ]; then
    echo "error: FILES ($files) narrows the audit below the scope of record:$missing_scope_of_record" >&2
    echo "error:   the scope of record is $scope_of_record (SCOPE_OF_RECORD, default: the same list FILES" >&2
    echo "error:   defaults to). Every current waiver fitting inside a narrower FILES does not make that" >&2
    echo "error:   narrower run complete — widen FILES to cover the scope of record." >&2
    exit 1
fi

# Check 2: FILES must also be a superset of the files the allowlist actually
# references (a superset, not equal: FILES may legitimately audit more than
# is waived yet, e.g. a file with no survivors at all, or the scope of
# record growing before the allowlist catches up). This is what catches a
# waiver left behind by a rename that scope_of_record's fixed literal cannot
# see on its own — the two checks are independent, not a fallback for each
# other.
waived_files="$(grep -v '^#' "$allowlist" | cut -f1 | grep . | cut -d: -f1 | sort -u)" || true
if [ -n "$waived_files" ]; then
    missing_scope="$(missing_from_scope "$waived_files")"
    if [ -n "$missing_scope" ]; then
        echo "error: the allowlist waives mutants in a file this run's scope does not audit:$missing_scope" >&2
        echo "error:   FILES ($files) must be a superset of every file mutants-allowlist.txt references, or" >&2
        echo "error:   those waivers are recorded but not re-measured this run. Widen FILES or narrow the allowlist." >&2
        exit 1
    fi
fi

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
# cargo-mutants writes all four outcome files unconditionally whenever a run
# reaches classification — mutants-gate-selftest.sh's stub replicates exactly
# that (every case creates all four, even the ones with zero entries).
# EMPTY caught.txt is a fact: "0 mutants caught this run", the ordinary shape
# of a run whose scope has few or no defensive-equivalent survivors. A
# MISSING caught.txt is a different fact: cargo-mutants failed to write it, or
# a future release renamed/dropped it. Pass 0 below has no other source of
# evidence that a waived mutant was genuinely killed rather than merely
# shifted (R18) — reading a missing file as "nothing caught" would silently
# disable that evidence and hand every killed-and-shifted mutant back to
# Pass 2 as an ordinary shift, degrading straight back to the round-4 defect
# with no warning at all. That is a fourth way to audit NOTHING (see the
# header above, which names three) — fail loudly, same as the other three,
# rather than substituting /dev/null and continuing.
for outcome in caught missed timeout unviable; do
    if [ ! -f "$results/$outcome.txt" ]; then
        echo
        echo "mutants-gate: FAILED — cargo-mutants did not write $results/$outcome.txt" >&2
        echo "mutants-gate:   a missing outcome file is not the same fact as one with zero lines in it —" >&2
        echo "mutants-gate:   the gate cannot tell 'nothing $outcome this run' from 'the classification" >&2
        echo "mutants-gate:   this run depends on silently did not happen'. Check the installed cargo-mutants" >&2
        echo "mutants-gate:   version against what it writes to -o." >&2
        exit 1
    fi
done
missed="$results/missed.txt"
# A TIMED-OUT mutant is not a killed one: no test proved it dead, the run just
# stopped waiting. Judging `missed.txt` alone would let a loaded runner — where
# every survivor happens to time out — print "0 survivors" and exit 0, which is
# the exact failure mode this gate exists to catch (an absence of signal read
# as coverage). So timeouts are judged by the same allowlist: an already-
# reviewed line that flaps missed -> timeout stays reviewed, a NEW timed-out
# mutant is an unjudged one and fails.
timeout="$results/timeout.txt"
caught="$results/caught.txt"
unviable="$results/unviable.txt"

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
# measured in three passes, each one only allowed to conclude what it can
# actually prove — anything a pass cannot prove is left for the next one,
# and what nothing can prove is NEW (fails) rather than a guess dressed as a
# NOTE (#58 review round 3, F1/F2):
#
#   Pass 1 (runs first): identical position claims its own waived line —
#   nothing moved, so this is the strongest possible evidence and gets first
#   claim on both sides before either of the passes below infers anything.
#   Pass 0 (runs second, over what Pass 1 left unclaimed): a waived line
#   whose (file, mutation text) KEY — position dropped — matches a caught.txt
#   entry this run is PROVEN killed, wherever it now sits. Position is
#   deliberately NOT part of this match: an edit can shift a waived mutant's
#   line AND kill it in the same run (#58 review round 4, F1/R18), and a
#   full-line match — position included — misses exactly that case, because
#   the kill shows up in caught.txt at the mutant's NEW position while the
#   allowlist still names the old one. Running this after Pass 1 (rather
#   than before, as an earlier revision did) matters: Pass 1 first removes
#   any waived line that is still alive, unmoved, at its own exact position,
#   so Pass 0 only ever proposes STALE for a waived line that is confirmed
#   ABSENT from this run's live results at its original spot — it cannot
#   steal a waived line out from under a survivor that is simply sitting
#   still. This is also the case Pass 2 used to get wrong on its own: a
#   waived mutant gets a new test and is genuinely caught in the same run a
#   DIFFERENT, never-reviewed mutant with the same mutation text survives
#   somewhere else in the file. Position-blind text matching alone cannot
#   tell "moved and killed" from "one killed, an unrelated one appeared"
#   apart — both leave one residual waived line and one residual live line
#   in the same key — so this is resolved with independent evidence
#   (caught.txt) rather than guessed from the residual counts. When more
#   than one waived line in a key is unclaimed and caught.txt has fewer
#   matches than that, the lowest-position waived line(s) are retired first
#   (same rank-pairing discipline as Pass 2, for a reproducible report) —
#   the rest stay open for Pass 2. A line retired here is STALE immediately
#   and never offered to Pass 2 as a shift candidate.
#   Pass 2: same mutant text, moved, with no caught.txt evidence either way
#   — still surviving, just not where the allowlist says. The lines left in
#   a key after Pass 1 and Pass 0 are sorted by position on each side and
#   paired by rank
#   (smallest live position with smallest residual waived position, and so
#   on) — the only pairing that preserves relative order, which is what an
#   edit shifting a block of code produces. Pairing "whichever unclaimed
#   waived line comes first" instead (the old rule) could cross two
#   reviewed reasons onto each other's positions with nothing in the output
#   to reveal it (F2, reproduced: 308<->323 and 311<->320, provably crossed
#   against the columns the gate had already parsed). Only
#   min(residual waived, residual live) pairs are made per key; a count
#   imbalance is never forced into a pairing just to make the totals
#   balance — the excess is genuinely new or genuinely stale.
classified="$(mktemp)"
{
    awk '{ print "W\t" $0 }' "$waived"
    awk '{ print "M\t" $0 }' "$missed"
    awk '{ print "T\t" $0 }' "$timeout"
    awk '{ print "C\t" $0 }' "$caught"
} | awk -F'\t' '
function key(s) {
    if (match(s, /:[0-9]+:[0-9]+: /)) return substr(s, 1, RSTART - 1) SUBSEP substr(s, RSTART + RLENGTH)
    return s SUBSEP ""
}
function pos(s) {
    if (match(s, /:[0-9]+:[0-9]+: /)) return substr(s, RSTART + 1, RLENGTH - 3)
    return "?"
}
# A sortable position: pos() returns "line:col"; zero-pad both so plain
# string comparison orders them numerically.
function sortpos(p,    parts, n) {
    n = split(p, parts, ":")
    if (n != 2) return p
    return sprintf("%010d:%010d", parts[1], parts[2])
}
{
    # Caught entries are keyed the same position-blind way as everything
    # else — a count per key is all Pass 0 needs, position plays no part in
    # proving a kill (see the comment above this pipeline).
    if ($1 == "C") { kc = key($2); cn[kc]++; next }
    k = key($2)
    keys[k] = 1
    if ($1 == "W") { wline[k, ++w[k]] = $2; wpos[k, w[k]] = pos($2) }
    else           { lline[k, ++l[k]] = $2; lpos[k, l[k]] = pos($2); lkind[k, l[k]] = $1 }
}
END {
    for (k in keys) {
        nw = w[k] + 0; nl = l[k] + 0
        # Pass 1 (runs first): identical position claims its own waived
        # line — still alive, unmoved. This must run before Pass 0 so a
        # survivor sitting still at its reviewed spot is never up for grabs
        # as caught-elsewhere evidence for a DIFFERENT key-mate.
        for (i = 1; i <= nl; i++) {
            for (j = 1; j <= nw; j++) {
                if (!wtaken[k, j] && wpos[k, j] == lpos[k, i]) {
                    wtaken[k, j] = 1; ltaken[k, i] = 1
                    if (lkind[k, i] == "T") print "TIMEOUT\t" lline[k, i]
                    break
                }
            }
        }
        # Pass 0 (runs second, over what Pass 1 left unclaimed): a caught
        # entry sharing this key proves a kill, position-blind, so a waived
        # line that moved AND was killed in the same run still retires as
        # STALE instead of falling through to Pass 2 and masquerading as a
        # still-open shift. Retire lowest-position-rank waived lines first
        # when there are more unclaimed waived lines than caught entries to
        # prove them with — the rest stay open for Pass 2.
        #
        # That "retire lowest-position-rank first" rule is a REPRODUCIBLE
        # pick, not a PROVEN one, exactly when rw0 > nc: nc caught entries
        # prove that many of the waived lines sharing this key are dead, but
        # not WHICH ones — the cargo-mutants report does not correlate a caught
        # mutant back to which reviewed candidate it was. When rw0 == nc
        # every remaining candidate retires regardless of pick order, so the
        # CONCLUSION is sound even though the individual pairing is still
        # arbitrary; when rw0 > nc the pick determines which specific line
        # the report calls dead and which stays open, and got there by
        # position rank alone. `ambiguous` records exactly this case for the
        # STALE line printed below, which reads it back and hedges instead
        # of stating an unproven pick as fact (#58 review round 5, N34).
        nc = cn[k] + 0
        if (nc > 0) {
            rw0 = 0
            for (j = 1; j <= nw; j++) if (!wtaken[k, j]) { rw0++; ridx0[rw0] = j; rkey0[rw0] = sortpos(wpos[k, j]) SUBSEP j }
            for (a = 1; a <= rw0; a++)
                for (b = a + 1; b <= rw0; b++)
                    if (rkey0[b] < rkey0[a]) { t = rkey0[a]; rkey0[a] = rkey0[b]; rkey0[b] = t; t = ridx0[a]; ridx0[a] = ridx0[b]; ridx0[b] = t }
            take0 = (nc < rw0) ? nc : rw0
            ambiguous = (rw0 > nc)
            for (a = 1; a <= take0; a++) {
                j = ridx0[a]
                wtaken[k, j] = 1
                if (ambiguous) print "STALE\t" wline[k, j] "\tTIE"
                else           print "STALE\t" wline[k, j]
            }
            delete ridx0; delete rkey0
        }
        # Pass 2: same mutant, moved, no caught.txt evidence either way —
        # a measured, order-preserving pairing over what Pass 1 and Pass 0
        # left, not a first-unclaimed-wins guess.
        rw = 0
        for (j = 1; j <= nw; j++) if (!wtaken[k, j]) { rw++; ridx[rw] = j; rkey[rw] = sortpos(wpos[k, j]) SUBSEP j }
        rl = 0
        for (i = 1; i <= nl; i++) if (!ltaken[k, i]) { rl++; lidx[rl] = i; lkey2[rl] = sortpos(lpos[k, i]) SUBSEP i }
        # Selection sort: n is a handful of mutants sharing identical
        # mutation text, and awk has no builtin sort-with-comparator.
        for (a = 1; a <= rw; a++)
            for (b = a + 1; b <= rw; b++)
                if (rkey[b] < rkey[a]) { t = rkey[a]; rkey[a] = rkey[b]; rkey[b] = t; t = ridx[a]; ridx[a] = ridx[b]; ridx[b] = t }
        for (a = 1; a <= rl; a++)
            for (b = a + 1; b <= rl; b++)
                if (lkey2[b] < lkey2[a]) { t = lkey2[a]; lkey2[a] = lkey2[b]; lkey2[b] = t; t = lidx[a]; lidx[a] = lidx[b]; lidx[b] = t }
        pairs = (rw < rl) ? rw : rl
        for (a = 1; a <= pairs; a++) {
            j = ridx[a]; i = lidx[a]
            wtaken[k, j] = 1; ltaken[k, i] = 1
            print "SHIFT\t" wline[k, j] "\t" lline[k, i]
            if (lkind[k, i] == "T") print "TIMEOUT\t" lline[k, i]
        }
        delete ridx; delete rkey; delete lidx; delete lkey2
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
        #
        # $b == TIE (set by the classifier's Pass 0, above) means THIS line
        # was picked by position rank among several sharing the same
        # mutation text, with fewer caught.txt entries than candidates —
        # some line in that group is proven dead, this one specifically is
        # not (N34). Say so instead of stating the pick as fact.
        if [ "$b" = "TIE" ]; then
            echo "NOTE  allowlist line PROBABLY no longer survives — proven dead is one of several reviewed lines sharing this exact mutation text; WHICH one is a position-rank pick, not individual proof. Verify by hand before pruning: $a"
        else
            echo "NOTE  allowlist line no longer survives (a test now kills it — prune it): $a"
        fi
        stale=$((stale + 1))
        ;;
    esac
done <"$classified"
rm -f "$classified"

# Every reviewed line going stale at once is not 17 test wins in one week; it
# is an audit that ran somewhere else — the shape a scope drift takes when the
# paths still exist (a module split, a package rename) and the file check above
# cannot see it.
#
# Deliberately only TOTAL staleness fails (#58 review round 3, F16). Partial
# staleness — some waived lines killed, others still reviewed survivors — is
# ordinary test progress and is reported as NOTEs above, not failed; punishing
# it would make writing the kill-test for one waived mutant a reason the job
# turns red. The scope-coverage check above (F4) closes the sharpest version
# of the residual worry this asymmetry raises: a FILES narrowing that drops a
# waived line's file outright now fails before the run even starts, so it
# cannot masquerade as partial staleness. What it does NOT close: an in-scope
# file that is internally split into new modules while FILES still names the
# old path — the moved mutants simply stop being generated at their old
# text+position and their waived lines go stale exactly like a genuine kill
# would. That case is still live and is a judgement call for whoever reviews
# the stale list, not something this gate can distinguish mechanically without
# a generated-count baseline to compare against (which would need its own
# provenance — see mutants-allowlist.txt's header).
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

#!/bin/bash
# Self-test for the mutation gate (#58) — the gate on the gate.
#
# Why. mutants-gate.sh exists to catch fixtures that cannot fail, and its own
# failure mode is exactly that: every discriminator it computes is one
# forgotten `exit 1` away from being a decoration. Three rounds of review
# found precisely such gaps (a timeout run that printed OK; a zero-mutant run
# that printed "all reviewed"; a never-reviewed survivor classified SHIFT
# because it shared mutation text with a waived line that had genuinely been
# killed), and all three were invisible to the suite because either the gate
# had no suite, or a case asserted an exit code and a string that a second,
# unrelated code path also produces. This file is audited the same way it
# audits the gate: every case below was proven, by deleting the gate line(s)
# it names and re-running this file, to fail when that line is gone.
#
# A real cargo-mutants run costs tens of minutes, so the outcomes are staged
# instead: a stub `cargo-mutants` on PATH writes the mutants.out/*.txt files
# each case needs and exits with the code the real tool would. What is under
# test is the gate's judgement, which is what reviews keep finding wrong.
# cargo-mutants itself is out of scope here — and IS pinned, contrary to what
# an earlier revision of this comment claimed in the OTHER direction:
# .github/workflows/mutants.yml installs it as `tool: cargo-mutants@27.1.0`
# (checked at this head; re-grep `tool: cargo-mutants@` there before trusting
# this sentence again, since nothing here re-derives it). Pinning removes the
# "changes under CI on its own schedule" version of the risk, but not the
# "someone bumps the pin" version: the allowlist keys on cargo-mutants'
# verbatim wording (mutants-allowlist.txt's column 1), so a future version
# bump that renames a mutation ("replace > with >=" becoming something else)
# would turn every waived line stale and every live mutant new in the same
# run — a real gap, on a much narrower trigger than "any time", with no
# fixture here or anywhere else that can exercise it without actually
# swapping the installed binary.
#
# Every case asserts an exit code AND at least one discriminating string that
# only the code path under test can produce — several also assert a string
# must be ABSENT (`!needle`), because a second, independent failure can mask
# a deleted line just as effectively as a missing assertion can. A gate that
# fails for the wrong reason does not pass this file. Run it in seconds:
#
#   bash scripts/tests/mutants-gate-selftest.sh
set -u

here="$(cd "$(dirname "$0")" && pwd)"
gate="$here/mutants-gate.sh"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
orig_path="$PATH"

# shellcheck source=selftest-lib.sh
source "$here/selftest-lib.sh"

# A stub cargo-mutants: writes the outcome files staged in $STUB_OUTCOMES
# (one `name:line` per line, name in caught|missed|timeout|unviable) into the
# -o directory, then exits $STUB_EXIT. `cargo mutants` dispatches to whatever
# `cargo-mutants` is first on PATH, so this substitutes for the real tool.
#
# Every kind's file is created unconditionally (even with zero lines) UNLESS
# its name appears in STUB_SKIP_OUTCOME_FILES (comma-separated) — that is the
# one seam this stub has for staging N32: a real cargo-mutants run that never
# writes one of the four files at all, which is a different fact from writing
# it empty (see mutants-gate.sh's outcome-file existence check).
#
# STUB_EXIT is the OTHER half of the staging, and every case below picks it to
# match the outcomes it stages, from the mapping MEASURED against cargo-mutants
# 27.1.0 (2026-08-09, four probe runs — the same measurement mutants-gate.sh's
# `case "$status"` comment records):
#   0 — nothing missed, nothing timed out (all caught / all unviable / none
#       generated at all).
#   2 — at least one MISSED mutant and no timeouts.
#   3 — at least one TIMEOUT, whether or not anything was also missed.
#   4 — the unmutated baseline failed; nothing was measured, and all four
#       outcome files are written EMPTY.
# A case staging a missed mutant under exit 3 (or vice versa) is a fixture the
# real tool would never produce, so it cannot pin the gate's judgement of the
# real tool — the pre-2026-08-09 revision of this file did exactly that for
# every case, against a `case "$status"` arm of `0|3|4` in which every value
# was wrong. Keep the code and the staged outcomes consistent when adding a
# case; the arms are load-bearing (an exit the gate does not accept never
# reaches classification at all).
mkdir -p "$work/bin"
cat >"$work/bin/cargo-mutants" <<'STUB'
#!/bin/bash
out=""
prev=""
for arg in "$@"; do
    [ "$prev" = "-o" ] && out="$arg"
    prev="$arg"
done
# The REAL invocation, one argument per line, for the flag assertions below
# (round 12 review R12-5): those used to grep the gate's own echoed command
# string, which is a *second* rendering of the arguments and can agree with
# the claim while the array actually handed to this process does not. What
# the tool receives is what matters, and this is the only place that sees it.
if [ -n "${STUB_ARGV_OUT:-}" ]; then
    printf '%s\n' "$@" >"$STUB_ARGV_OUT"
fi
mkdir -p "$out/mutants.out"
skip=",${STUB_SKIP_OUTCOME_FILES:-},"
for kind in caught missed timeout unviable; do
    case "$skip" in
    *",$kind,"*) ;;
    *) : >"$out/mutants.out/$kind.txt" ;;
    esac
done
if [ -n "${STUB_OUTCOMES:-}" ] && [ -f "$STUB_OUTCOMES" ]; then
    while IFS= read -r entry; do
        [ -n "$entry" ] || continue
        printf '%s\n' "${entry#*:}" >>"$out/mutants.out/${entry%%:*}.txt"
    done <"$STUB_OUTCOMES"
fi
exit "${STUB_EXIT:-0}"
STUB
chmod +x "$work/bin/cargo-mutants"
PATH="$work/bin:$PATH"
export PATH

# Two waived mutants sharing one (file, text) key — the multiset case that
# makes position-blind matching wrong — plus one that is unique.
allowlist="$work/allowlist.txt"
cat >"$allowlist" <<'LIST'
# fixture allowlist
crates/luabox-types/src/env.rs:298:30: replace > with >= in TypeEnv::build_from_items	[defensive] one
crates/luabox-types/src/env.rs:301:27: replace > with >= in TypeEnv::build_from_items	[defensive] two
crates/luabox-types/src/env.rs:1333:9: replace TypeEnv::is_class -> bool with true	[equivalent] three
LIST

# An all-comment allowlist: zero waived lines, a real shape (not a fixture
# artifact) — a scope with no reviewed survivors yet, or one just re-pinned
# down to nothing. Distinct from "missing", which is a different failure.
comment_only_allowlist="$work/comment-only-allowlist.txt"
cat >"$comment_only_allowlist" <<'LIST'
# nothing waived yet in this scope
LIST

# Real files, so the scope check passes for every case that is not about it.
scope="crates/luabox-types/src/env.rs"

# run <name> <exit-expected> <outcomes-file> <stub-exit> <needle>...
# A needle prefixed with `!` must be ABSENT from the log rather than present
# — a second, unrelated failure path can supply the same "expected" string a
# deleted line was supposed to produce, and a needle can only prove presence.
# FILES_OVERRIDE / ALLOWLIST_OVERRIDE / SCOPE_OF_RECORD_OVERRIDE /
# IN_DIFF_OVERRIDE (set by the caller as temporary variable assignments on
# the `run` call itself) substitute for the fixture
# scope/allowlist/scope-of-record/diff-bound without hand-rolling a second
# invocation of the gate. SCOPE_OF_RECORD defaults to
# $scope (the same single file FILES defaults to), matching the gate's own
# "SCOPE_OF_RECORD defaults to the same list FILES defaults to" — so every
# case below that does not name the R19 guard gets FILES == SCOPE_OF_RECORD
# and never trips it by accident.
run() {
    local name="$1" want_exit="$2" outcomes="$3" stub_exit="$4"
    shift 4
    local log="$work/$name.log"
    local files="${FILES_OVERRIDE:-$scope}"
    local list="${ALLOWLIST_OVERRIDE:-$allowlist}"
    local record="${SCOPE_OF_RECORD_OVERRIDE:-$scope}"
    # IN_DIFF_OVERRIDE stages the diff-bounded mode `mutants-pr` runs in. It
    # is passed as the empty string when unset, which is exactly what the gate
    # reads for "not set" (`in_diff="${IN_DIFF:-}"`), so every case that does
    # not name it gets the ordinary full-scope shape.
    local diff="${IN_DIFF_OVERRIDE:-}"
    STUB_OUTCOMES="$outcomes" STUB_EXIT="$stub_exit" \
        FILES="$files" ALLOWLIST="$list" SCOPE_OF_RECORD="$record" MUTANTS_OUT="$work/out-$name" \
        IN_DIFF="$diff" \
        bash "$gate" >"$log" 2>&1
    local got=$?
    assert_exit "$name" "$want_exit" "$got" "$log" "$@"
}

steady="$work/steady.txt"
cat >"$steady" <<'OUT'
missed:crates/luabox-types/src/env.rs:298:30: replace > with >= in TypeEnv::build_from_items
missed:crates/luabox-types/src/env.rs:301:27: replace > with >= in TypeEnv::build_from_items
missed:crates/luabox-types/src/env.rs:1333:9: replace TypeEnv::is_class -> bool with true
caught:crates/luabox-types/src/env.rs:400:1: replace TypeEnv::noop with ()
OUT
# "0 shifted"/"0 new" pin Pass 1 (exact-position matching) specifically:
# without it every waived line here would fall through to Pass 2's
# same-key pairing and get reported as a moved mutant even though nothing
# moved — a permanent false "re-pin" on a run where nothing changed.
# "report directory:" (F15) pins that the report's location is ALWAYS
# echoed — on this, the ordinary successful-run path, and (by the same
# unconditional line, before any of the early-exit checks) on every other
# path too, so an out_dir nobody remembered to print is never unreachable.
run steady_state 0 "$steady" 2 "3 survivor(s)" "all reviewed" "0 shifted" "0 new" "report directory:"

# The round-1 gap: a run where every survivor timed out. Unkilled is unkilled.
flap="$work/flap.txt"
cat >"$flap" <<'OUT'
timeout:crates/luabox-types/src/env.rs:298:30: replace > with >= in TypeEnv::build_from_items
missed:crates/luabox-types/src/env.rs:301:27: replace > with >= in TypeEnv::build_from_items
missed:crates/luabox-types/src/env.rs:1333:9: replace TypeEnv::is_class -> bool with true
OUT
run waived_line_flaps_to_timeout 0 "$flap" 3 "timed out this run rather than surviving" "0 shifted"

new_timeout="$work/new-timeout.txt"
cat >"$new_timeout" <<'OUT'
timeout:crates/luabox-types/src/env.rs:900:5: replace TypeEnv::brand_new with ()
missed:crates/luabox-types/src/env.rs:298:30: replace > with >= in TypeEnv::build_from_items
missed:crates/luabox-types/src/env.rs:301:27: replace > with >= in TypeEnv::build_from_items
missed:crates/luabox-types/src/env.rs:1333:9: replace TypeEnv::is_class -> bool with true
OUT
# "1 new timed out" pins the summary-line increment (:198 in the old
# numbering) separately from the per-mutant FAIL echo, and the "unkilled,
# not killed" hint pins the remediation text at :252-255 — both survive a
# gate that still fails for an unrelated reason unless asserted by name.
run new_timeout_fails 1 "$new_timeout" 3 \
    "new timed-out mutant" "brand_new" "1 new timed out" "unkilled, not killed"

new_survivor="$work/new.txt"
cat >"$new_survivor" <<'OUT'
missed:crates/luabox-types/src/env.rs:900:5: replace TypeEnv::brand_new with ()
missed:crates/luabox-types/src/env.rs:298:30: replace > with >= in TypeEnv::build_from_items
missed:crates/luabox-types/src/env.rs:301:27: replace > with >= in TypeEnv::build_from_items
missed:crates/luabox-types/src/env.rs:1333:9: replace TypeEnv::is_class -> bool with true
OUT
run new_survivor_fails 1 "$new_survivor" 2 "new surviving mutant" "brand_new" "1 new"

# The round-2 finding: a shift is a MEASURED pairing, not a reader's guess.
# All three waived mutants move; nothing is new; the run passes and prints
# what to re-pin. "was:"/"now:" is the per-mutant NOTE; "old:"/"new:" is the
# separate re-pin summary block fed by the same record — F9 found both
# assertable independently of the "3 shifted"/"re-pin" counts, which a
# second code path can also produce with the record itself deleted.
shifted="$work/shifted.txt"
cat >"$shifted" <<'OUT'
missed:crates/luabox-types/src/env.rs:309:30: replace > with >= in TypeEnv::build_from_items
missed:crates/luabox-types/src/env.rs:312:27: replace > with >= in TypeEnv::build_from_items
missed:crates/luabox-types/src/env.rs:1344:9: replace TypeEnv::is_class -> bool with true
OUT
run shifted_batch_is_not_new 0 "$shifted" 2 "3 shifted" "0 new" "re-pin the 3 shifted" \
    "was: crates/luabox-types/src/env.rs:298:30" "now: crates/luabox-types/src/env.rs:309:30" \
    "old: crates/luabox-types/src/env.rs:298:30" "new: crates/luabox-types/src/env.rs:309:30"

# #58 review round 6, M26: N34 hedged the STALE side of exactly this
# ambiguity ("PROBABLY ... a position-rank pick, not individual proof") but
# left the SHIFT side printing "re-pin, do not re-review" unhedged for the
# mirror-image guess — live in the committed allowlist today, where
# env.rs:759:30 and :762:27 share one mutation text. Two waived lines
# sharing a key (no caught.txt evidence either way) both move to two live
# survivors sharing the same key: the SET of matches is proven (both keys
# match), but WHICH reviewed reason belongs at WHICH new position is a
# position-rank pick among interchangeable candidates — swap the two waived
# lines' reasons and the report would look identical. Both SHIFT lines here
# must carry the hedge, not the confident "re-pin, do not re-review" wording
# shifted_batch_is_not_new above legitimately keeps for its own unambiguous
# pairs (checked directly: mutants-gate.sh's `shift_ambig = (pairs > 1) ?
# "TIE" : ""` line neutered to `shift_ambig = ""` reproduces the pre-fix
# unhedged wording on this exact fixture).
multi_shift="$work/multi-shift.txt"
cat >"$multi_shift" <<'OUT'
missed:crates/luabox-types/src/env.rs:500:1: replace TypeEnv::is_class -> bool with true
missed:crates/luabox-types/src/env.rs:600:1: replace TypeEnv::is_class -> bool with true
OUT
multi_shift_allowlist="$work/multi-shift-allowlist.txt"
cat >"$multi_shift_allowlist" <<'LIST'
crates/luabox-types/src/env.rs:10:1: replace TypeEnv::is_class -> bool with true	[equivalent] first reason
crates/luabox-types/src/env.rs:15:1: replace TypeEnv::is_class -> bool with true	[equivalent] second reason
LIST
ALLOWLIST_OVERRIDE="$multi_shift_allowlist" \
    run shift_pairing_is_hedged_when_ambiguous 0 "$multi_shift" 2 \
    "2 shifted" \
    "PROBABLY moved" "position-rank pick, not individual proof" \
    "was: crates/luabox-types/src/env.rs:10:1" "now: crates/luabox-types/src/env.rs:500:1" \
    "was: crates/luabox-types/src/env.rs:15:1" "now: crates/luabox-types/src/env.rs:600:1" \
    "!waived mutant moved (same file and mutation, new position — re-pin, do not re-review)"

# The mixed batch that defeated the old rule: two shifts and one genuinely
# new mutant in the same run. Counts alone cannot separate them; the key can.
mixed="$work/mixed.txt"
cat >"$mixed" <<'OUT'
missed:crates/luabox-types/src/env.rs:309:30: replace > with >= in TypeEnv::build_from_items
missed:crates/luabox-types/src/env.rs:312:27: replace > with >= in TypeEnv::build_from_items
missed:crates/luabox-types/src/env.rs:1344:9: replace TypeEnv::is_class -> bool with true
missed:crates/luabox-types/src/env.rs:915:5: replace TypeEnv::brand_new with ()
OUT
run mixed_shift_and_new_batch 1 "$mixed" 2 "3 shifted" "1 new" "brand_new"

# `sort` (mutants-gate.sh, end of the classification pipeline) is what makes
# the report's line order reproducible run over run, independent of the
# order awk's `for (k in keys)` happens to walk its hash table in. NEW ("N")
# sorts before SHIFT ("S"), so in a batch containing both, the NEW line
# should always print first. Disclosed honestly: this was checked by
# deleting `| sort` (mutants-gate.sh) and re-running — for THIS fixture's
# two keys, gawk's unsorted hash-iteration order happened to already emit
# NEW before SHIFT, so the assertion below still passes without `sort` and
# does not prove the line is load-bearing. It is kept as a real structural
# check (it would catch a classifier that emitted the wrong KIND, not just
# the wrong order) and as documentation of the guarantee `sort` provides,
# not as a substitute for one.
mixed_log="$work/mixed_shift_and_new_batch.log"
new_at="$(grep -n 'new surviving mutant' "$mixed_log" | head -1 | cut -d: -f1)"
shift_at="$(grep -n 'waived mutant moved' "$mixed_log" | head -1 | cut -d: -f1)"
if [ -n "$new_at" ] && [ -n "$shift_at" ] && [ "$new_at" -lt "$shift_at" ]; then
    echo "PASS  mixed_batch_output_is_sorted (NEW at line $new_at, before SHIFT at line $shift_at)"
    pass=$((pass + 1))
else
    echo "FAIL  mixed_batch_output_is_sorted: expected the NEW line before any SHIFT line" >&2
    sed 's/^/        /' "$mixed_log" >&2
    fail=$((fail + 1))
fi

# A test that kills a waived mutant is progress, not a failure.
killed="$work/killed.txt"
cat >"$killed" <<'OUT'
missed:crates/luabox-types/src/env.rs:298:30: replace > with >= in TypeEnv::build_from_items
missed:crates/luabox-types/src/env.rs:301:27: replace > with >= in TypeEnv::build_from_items
caught:crates/luabox-types/src/env.rs:1333:9: replace TypeEnv::is_class -> bool with true
OUT
run one_killed_waiver_is_stale_not_failure 0 "$killed" 2 "prune it" "1 stale"

# F1's exact shape: a waived mutant is PROVEN caught (its own line appears
# verbatim in caught.txt, at its own position) in the same run a different,
# never-reviewed mutant with the same mutation text survives elsewhere. The
# proof must retire the waived line as stale outright — it must not be
# handed to Pass 2 as a shift candidate for the new survivor, which is what
# silently cleared the new survivor before this fix (0 new, 1 shifted, OK,
# exit 0, and nobody had reviewed the mutant at 315:19).
proven_kill="$work/proven-kill.txt"
cat >"$proven_kill" <<'OUT'
caught:crates/luabox-types/src/env.rs:298:30: replace > with >= in TypeEnv::build_from_items
missed:crates/luabox-types/src/env.rs:301:27: replace > with >= in TypeEnv::build_from_items
missed:crates/luabox-types/src/env.rs:315:19: replace > with >= in TypeEnv::build_from_items
missed:crates/luabox-types/src/env.rs:1333:9: replace TypeEnv::is_class -> bool with true
OUT
run f1_caught_proof_beats_shared_key 1 "$proven_kill" 2 \
    "new surviving mutant" "315:19" "1 new" "0 shifted" "1 stale" \
    "!was: crates/luabox-types/src/env.rs:298:30"

# R18 (#58 review round 4): F1's proof above only fires when the killed
# mutant did NOT move — it matches the waived line's FULL TEXT, position
# included, against caught.txt verbatim. An edit can shift a waived mutant's
# line AND kill it in the same run: the kill then shows up in caught.txt at
# the mutant's NEW position (915:19) while the allowlist still names the
# old one (298:30), so the old full-line check never finds it, the waived
# line falls through to Pass 2, and Pass 2 pairs it — by position rank, the
# only evidence it has — against whatever else shares its key. Reproduced
# here: a genuinely new, never-reviewed survivor (305:11) shares the same
# key, and the old code paired 298:30 with IT instead ("0 new, 1 shifted,
# OK, exit 0" — the exact shape the review reproduced), silently dropping
# the real survivor and misreporting the true kill as merely a pending
# re-pin. The fix must retire 298:30 as STALE on caught.txt's key-level
# evidence (proven killed, wherever it now sits) and report 305:11 as NEW.
killed_and_shifted="$work/killed-and-shifted.txt"
cat >"$killed_and_shifted" <<'OUT'
missed:crates/luabox-types/src/env.rs:301:27: replace > with >= in TypeEnv::build_from_items
missed:crates/luabox-types/src/env.rs:1333:9: replace TypeEnv::is_class -> bool with true
caught:crates/luabox-types/src/env.rs:915:19: replace > with >= in TypeEnv::build_from_items
missed:crates/luabox-types/src/env.rs:305:11: replace > with >= in TypeEnv::build_from_items
OUT
run r18_killed_and_shifted_beats_shared_key 1 "$killed_and_shifted" 2 \
    "new surviving mutant" "305:11" "1 new" "0 shifted" "1 stale" "prune it" \
    "!was: crates/luabox-types/src/env.rs:298:30" \
    "!now: crates/luabox-types/src/env.rs:305:11"

# N32 (#58 review round 5): R18's whole fix, above, rests on caught.txt as
# the ONLY evidence that separates "waived mutant genuinely killed" from
# "waived mutant just moved". The old outcome_file() substituted /dev/null
# for ANY of the four outcome files that did not exist, with no warning —
# indistinguishable from that file existing and legitimately having zero
# lines. So a cargo-mutants release that stops writing caught.txt (or any
# release that fails to write it for some other reason) would silently
# degrade Pass 0 straight back to the round-4 defect: the R18 fixture above
# would go back to reporting a genuine kill as a mere SHIFT. STUB_OUTCOMES
# here stages nothing but missed lines — no "caught:" entry — because the
# stub's append (`>>`) would otherwise recreate caught.txt itself and defeat
# the case; STUB_SKIP_OUTCOME_FILES is the only way this file can leave
# caught.txt genuinely absent rather than empty. "!all reviewed" proves the
# run did not fall through to classification and print OK for a state its
# own Pass 0 could not actually evidence.
steady_no_caught="$work/steady-no-caught.txt"
cat >"$steady_no_caught" <<'OUT'
missed:crates/luabox-types/src/env.rs:298:30: replace > with >= in TypeEnv::build_from_items
missed:crates/luabox-types/src/env.rs:301:27: replace > with >= in TypeEnv::build_from_items
missed:crates/luabox-types/src/env.rs:1333:9: replace TypeEnv::is_class -> bool with true
OUT
export STUB_SKIP_OUTCOME_FILES="caught"
run caught_file_missing_fails_loudly 1 "$steady_no_caught" 2 \
    "did not write" "caught.txt" "!all reviewed"
unset STUB_SKIP_OUTCOME_FILES

# F2: two waived mutants at different positions both shift in the same run.
# Pairing must be nearest-position, not "whichever unclaimed waived line
# comes first in the allowlist" — the old rule paired 308<->323 and
# 311<->320, provably crossed against the very columns it had just parsed.
# 308 is 12 lines from 320 and 15 from 323; 311 is 12 from 323 and 9 from
# 320 — sorted-rank pairing (ascending waived position against ascending
# live position) is the only assignment that keeps relative order, and is
# asserted here by checking each "was:"/"now:" pair lands together.
crossed_allowlist="$work/crossed-allowlist.txt"
cat >"$crossed_allowlist" <<'LIST'
crates/luabox-types/src/env.rs:308:30: replace > with >= in TypeEnv::build_from_items	[defensive] reason-A
crates/luabox-types/src/env.rs:311:27: replace > with >= in TypeEnv::build_from_items	[defensive] reason-B
LIST
crossed="$work/crossed.txt"
cat >"$crossed" <<'OUT'
missed:crates/luabox-types/src/env.rs:323:27: replace > with >= in TypeEnv::build_from_items
missed:crates/luabox-types/src/env.rs:320:30: replace > with >= in TypeEnv::build_from_items
OUT
crossed_log="$work/pass2_pairing.log"
STUB_OUTCOMES="$crossed" STUB_EXIT=2 \
    FILES="$scope" ALLOWLIST="$crossed_allowlist" SCOPE_OF_RECORD="$scope" MUTANTS_OUT="$work/out-pass2-pairing" \
    bash "$gate" >"$crossed_log" 2>&1
crossed_exit=$?
if [ "$crossed_exit" = 0 ] \
    && grep -A1 -F "was: crates/luabox-types/src/env.rs:308:30" "$crossed_log" | grep -qF "now: crates/luabox-types/src/env.rs:320:30" \
    && grep -A1 -F "was: crates/luabox-types/src/env.rs:311:27" "$crossed_log" | grep -qF "now: crates/luabox-types/src/env.rs:323:27"; then
    echo "PASS  pass2_pairing_is_nearest_not_crossed (exit $crossed_exit)"
    pass=$((pass + 1))
else
    echo "FAIL  pass2_pairing_is_nearest_not_crossed: expected 308->320 and 311->323 paired, not crossed" >&2
    sed 's/^/        /' "$crossed_log" >&2
    fail=$((fail + 1))
fi

# R20 (#58 review round 4): the fixture above lists its waivers in ASCENDING
# position order in the allowlist file, so it exercises only the LIVE-side
# rank sort — deleting the waived-side sort (mutants-gate.sh, the `rkey`
# selection-sort block) leaves this fixture 21/21 green because sorting an
# already-sorted array is a no-op. Reversed here: reason-B (311) is written
# BEFORE reason-A (308) in the allowlist file, while the live positions
# (320, 323) stay ascending — so a pairing that used the waived lines in
# file-encounter order instead of position order would cross 311<->320 and
# 308<->323, reproducing F2's original bug on this axis specifically.
reversed_allowlist="$work/reversed-allowlist.txt"
cat >"$reversed_allowlist" <<'LIST'
crates/luabox-types/src/env.rs:311:27: replace > with >= in TypeEnv::build_from_items	[defensive] reason-B
crates/luabox-types/src/env.rs:308:30: replace > with >= in TypeEnv::build_from_items	[defensive] reason-A
LIST
reversed="$work/reversed.txt"
cat >"$reversed" <<'OUT'
missed:crates/luabox-types/src/env.rs:320:30: replace > with >= in TypeEnv::build_from_items
missed:crates/luabox-types/src/env.rs:323:27: replace > with >= in TypeEnv::build_from_items
OUT
reversed_log="$work/pass2_waived_side_sort.log"
STUB_OUTCOMES="$reversed" STUB_EXIT=2 \
    FILES="$scope" ALLOWLIST="$reversed_allowlist" SCOPE_OF_RECORD="$scope" MUTANTS_OUT="$work/out-waived-sort" \
    bash "$gate" >"$reversed_log" 2>&1
reversed_exit=$?
if [ "$reversed_exit" = 0 ] \
    && grep -A1 -F "was: crates/luabox-types/src/env.rs:308:30" "$reversed_log" | grep -qF "now: crates/luabox-types/src/env.rs:320:30" \
    && grep -A1 -F "was: crates/luabox-types/src/env.rs:311:27" "$reversed_log" | grep -qF "now: crates/luabox-types/src/env.rs:323:27"; then
    echo "PASS  pass2_pairing_sorts_the_waived_side_too (exit $reversed_exit)"
    pass=$((pass + 1))
else
    echo "FAIL  pass2_pairing_sorts_the_waived_side_too: expected 308->320 and 311->323 paired despite the allowlist file listing 311 before 308" >&2
    sed 's/^/        /' "$reversed_log" >&2
    fail=$((fail + 1))
fi

# R20 (#58 review round 4): `k = key($2)` is the entire (file, mutation)
# keying the gate's headline claim rests on — every other case above still
# passes without it, because their fixtures' positions happen to already
# sort in an order that looks key-correct. Isolate it: two DIFFERENT
# mutation texts at CLOSE positions (100, 105) whose true (keyed) shift
# partners sit at positions that invert the naive position-only ranking —
# text A's partner (900) is numerically FARTHER than text B's partner (106)
# even though text A's waived line sits at the LOWER position (100 < 105).
# Correct, key-aware pairing is 100<->900 (both text A) and 105<->106 (both
# text B). Positional-only pairing (what a collapsed, empty key produces —
# every row falling into one bucket) sorts strictly by position on both
# sides regardless of text and pairs 100<->106 and 105<->900 instead,
# crossing the two mutation texts onto each other exactly as F2 originally
# crossed two reasons — provably wrong here because the paired "now:" text
# would not even be the same mutation as the "was:" line.
key_allowlist="$work/key-allowlist.txt"
cat >"$key_allowlist" <<'LIST'
crates/luabox-types/src/env.rs:100:1: replace > with >= in TypeEnv::build_from_items	[defensive] text-A
crates/luabox-types/src/env.rs:105:1: replace TypeEnv::is_class -> bool with true	[equivalent] text-B
LIST
key_outcomes="$work/key-outcomes.txt"
cat >"$key_outcomes" <<'OUT'
missed:crates/luabox-types/src/env.rs:106:1: replace TypeEnv::is_class -> bool with true
missed:crates/luabox-types/src/env.rs:900:1: replace > with >= in TypeEnv::build_from_items
OUT
key_log="$work/key_is_load_bearing.log"
STUB_OUTCOMES="$key_outcomes" STUB_EXIT=2 \
    FILES="$scope" ALLOWLIST="$key_allowlist" SCOPE_OF_RECORD="$scope" MUTANTS_OUT="$work/out-key-load-bearing" \
    bash "$gate" >"$key_log" 2>&1
key_exit=$?
if [ "$key_exit" = 0 ] \
    && grep -A1 -F "was: crates/luabox-types/src/env.rs:100:1" "$key_log" | grep -qF "now: crates/luabox-types/src/env.rs:900:1" \
    && grep -A1 -F "was: crates/luabox-types/src/env.rs:105:1" "$key_log" | grep -qF "now: crates/luabox-types/src/env.rs:106:1"; then
    echo "PASS  pairing_is_keyed_on_mutation_text_not_position_alone (exit $key_exit)"
    pass=$((pass + 1))
else
    echo "FAIL  pairing_is_keyed_on_mutation_text_not_position_alone: expected 100(text-A)->900(text-A) and 105(text-B)->106(text-B), not paired by raw position" >&2
    sed 's/^/        /' "$key_log" >&2
    fail=$((fail + 1))
fi

# A SHIFT and a TIMEOUT are not mutually exclusive: a waived mutant can move
# AND flap to timeout in the same run. Pass 2 must still mark it TIMEOUT
# (:172 in the old numbering), not just SHIFT — the two live mutants that
# were not it (301) fall out unpaired and go stale.
shift_timeout="$work/shift-timeout.txt"
cat >"$shift_timeout" <<'OUT'
timeout:crates/luabox-types/src/env.rs:315:19: replace > with >= in TypeEnv::build_from_items
missed:crates/luabox-types/src/env.rs:1333:9: replace TypeEnv::is_class -> bool with true
OUT
run shift_and_timeout_together 0 "$shift_timeout" 3 \
    "waived mutant moved" "timed out this run rather than surviving" \
    "old: crates/luabox-types/src/env.rs:298:30" "now: crates/luabox-types/src/env.rs:315:19" "1 stale"

# Round-2 finding 1, direct: a run that tested nothing must not report
# "all reviewed". Same shape as an audit whose scope quietly stopped
# matching — and the two must not be confusable: without the target `exit 1`
# the run falls through to classification, every waived line has nothing to
# match, and the SEPARATE whole-allowlist-stale check fires instead, still
# exiting 1. The `!went stale in one run` needle is what tells them apart.
empty="$work/empty.txt"
: >"$empty"
run zero_mutants_fails 1 "$empty" 0 "generated 0 mutants" "nothing was audited" "!went stale in one run"

# …and the shape the file check cannot see: mutants generated, but not one
# of them is anything the allowlist knows — the scope moved, not the tests.
# The fixture stages nothing that could independently fail the run (no
# missed/timeout at all), so `!new surviving mutant` proves the failure is
# this guard and not a decoy survivor shadowing it.
elsewhere="$work/elsewhere.txt"
cat >"$elsewhere" <<'OUT'
caught:crates/luabox-types/src/other.rs:10:1: replace other::f with ()
OUT
run whole_allowlist_stale_fails 1 "$elsewhere" 0 "went stale in one run" "wrong scope" "!new surviving mutant"

# The cheapest way for the audit to go quiet: a rename nobody propagated.
# FILES_OVERRIDE substitutes the scope for this one call; run() falls back
# to $scope otherwise, so this is the seam F14 asked for — one call site,
# not a hand-rolled invocation with its own grep and its own message. The
# two `!` needles are load-bearing, not decorative (#58 review round 4,
# R20): the "scoped file does not exist" echo alone does not prove the
# target `exit 1` fired — it is printed BEFORE the deleted line, so it
# survives either way. Without the target exit, `file_args` still picks up
# the nonexistent path unconditionally after the `fi`, and FILES (this
# fake single file) narrows below both SCOPE_OF_RECORD (default: $scope, a
# DIFFERENT single file) and the default allowlist's env.rs waivers — either
# downstream check fires its own exit 1 with its own message, reproducing
# this case's exit code and its one positive needle without the line under
# test ever running.
FILES_OVERRIDE="crates/luabox-types/src/renamed_by_a_refactor.rs" \
    run missing_scope_file_fails 1 "$steady" 2 "scoped file does not exist" \
    "!narrows the audit below the scope of record" \
    "!waives mutants in a file this run's scope does not audit"

# The allowlist itself can go missing (a bad ALLOWLIST override, a checkout
# that dropped the file) — distinct from "every line in it went stale".
# `!report directory:` is load-bearing, not decorative (#58 review round 4,
# R20): the "no allowlist at" echo alone does not prove the target `exit 1`
# fired — without it, $steady's own missed set has nothing waived to match
# (the allowlist read below fails silently) and every one of its 3 survivors
# becomes NEW on its own, which ALSO exits 1 and ALSO leaves "no allowlist
# at" sitting in the log from the echo that ran just before the deleted
# line — a second, unrelated failure reproducing both assertions this case
# had. "report directory:" is only ever printed after the allowlist check
# passes, so its absence is the one thing that specifically proves the exit
# happened there and cargo-mutants was never reached.
ALLOWLIST_OVERRIDE="$work/does-not-exist.txt" \
    run missing_allowlist_fails 1 "$steady" 2 "no allowlist at" "!report directory:" "!new surviving mutant"

# F4: a scope naming a file that EXISTS but excludes a file the allowlist
# waives mutants in must fail before spending the run — not silently audit a
# fraction of the claimed surface and report 0 new / 0 stale / exit 0. Real
# file, wrong one: distinct from missing_scope_file_fails (F14 shape) above.
# SCOPE_OF_RECORD_OVERRIDE matches FILES_OVERRIDE here so THIS narrowing
# clears the R19 scope-of-record check (below) cleanly and the run reaches
# the allowlist-narrowing check this case actually targets — the
# scope-of-record narrowing itself is exercised by its own case below.
FILES_OVERRIDE="crates/luabox-types/src/defs.rs" \
    SCOPE_OF_RECORD_OVERRIDE="crates/luabox-types/src/defs.rs" \
    run scope_excludes_a_waived_file_fails 1 "$steady" 2 \
    "the allowlist waives mutants in a file this run's scope does not audit" \
    "crates/luabox-types/src/env.rs"

# R19 (#58 review round 4): the check above compares FILES against the
# ALLOWLIST's own referenced files, which all 17 real waivers happen to sit
# inside env.rs today — so FILES=crates/luabox-types/src/env.rs alone
# satisfies it while auditing a quarter of the four-file scope this file's
# header and .github/workflows/mutants.yml both claim, giving 0 new / 0
# stale / exit 0 for the three files never even attempted. Reproduced here
# with SCOPE_OF_RECORD_OVERRIDE standing in for production's default (the
# full four-file default_files) while FILES stays at the fixture's usual
# single file — exactly the shape a scope pinned narrower than the
# documented surface takes, regardless of what the allowlist references.
FILES_OVERRIDE="crates/luabox-types/src/env.rs" \
    SCOPE_OF_RECORD_OVERRIDE="crates/luabox-types/src/env.rs,crates/luabox-types/src/defs.rs,crates/luabox-types/src/generics.rs,crates/luabox-types/src/infer/reify.rs" \
    run scope_of_record_narrowing_fails 1 "$steady" 2 \
    "narrows the audit below the scope of record" \
    "crates/luabox-types/src/defs.rs" \
    "!the allowlist waives mutants in a file this run's scope does not audit"

# F3: MUTANTS_OUT may be given relative to the caller's cwd. cargo-mutants
# runs inside `(cd "$repo" && ...)`, so an unresolved relative path would be
# written under $repo and read back from wherever this script happened to be
# invoked — misreporting a path bug as "the run generated 0 mutants" only
# after paying for the whole run. Prove the resolution happens BEFORE that
# read: invoke from a throwaway cwd, with MUTANTS_OUT relative to it, and
# confirm both the echoed report path and the actual mutants.out directory
# land there — not under $repo, and not a false "0 mutants" failure.
relout_dir="$work/relout-caller-cwd"
mkdir -p "$relout_dir"
relout_log="$work/relative-mutants-out.log"
(
    cd "$relout_dir" || exit 1
    STUB_OUTCOMES="$steady" STUB_EXIT=2 \
        FILES="$scope" ALLOWLIST="$allowlist" SCOPE_OF_RECORD="$scope" MUTANTS_OUT="relout" \
        bash "$gate" >"$relout_log" 2>&1
)
relout_exit=$?
if [ "$relout_exit" = 0 ] \
    && grep -qF "report directory: $relout_dir/relout" "$relout_log" \
    && [ -d "$relout_dir/relout/mutants.out" ] \
    && ! grep -qF "generated 0 mutants" "$relout_log"; then
    echo "PASS  relative_mutants_out_resolves_against_callers_cwd (exit $relout_exit)"
    pass=$((pass + 1))
else
    echo "FAIL  relative_mutants_out_resolves_against_callers_cwd: expected the report under $relout_dir/relout" >&2
    sed 's/^/        /' "$relout_log" >&2
    fail=$((fail + 1))
fi

# An all-comment allowlist (waived_total == 0) with survivors must not read
# as "every reviewed line went stale" — 0 stale == 0 waived_total is true by
# arithmetic, and the guard's `waived_total -gt 0` conjunct is what stops
# that from firing on a scope that simply has nothing waived yet.
clean_for_comment_only="$work/clean-for-comment-only.txt"
cat >"$clean_for_comment_only" <<'OUT'
caught:crates/luabox-types/src/other.rs:1:1: replace other::a with ()
OUT
ALLOWLIST_OVERRIDE="$comment_only_allowlist" \
    run all_comment_allowlist_with_clean_run_is_ok 0 "$clean_for_comment_only" 0 \
    "OK" "0 stale" "!went stale in one run"

# `generated` (:105 old numbering) sums all four outcome kinds. An allowlist
# with nothing waived means every missed/timeout mutant is unreviewed and
# fails the run regardless — which isolates the arithmetic from the
# shift/stale machinery and lets one fixture cover all four kinds, including
# `unviable`, which nothing else here stages at all.
counts="$work/counts.txt"
cat >"$counts" <<'OUT'
caught:crates/luabox-types/src/other.rs:1:1: replace other::a with ()
missed:crates/luabox-types/src/other.rs:2:1: replace other::b with ()
timeout:crates/luabox-types/src/other.rs:3:1: replace other::c with ()
unviable:crates/luabox-types/src/other.rs:4:1: replace other::d with ()
OUT
ALLOWLIST_OVERRIDE="$comment_only_allowlist" \
    run generated_count_includes_every_outcome_kind 1 "$counts" 3 "4 mutant(s) generated"

# The whole `command -v cargo-mutants` check (:48-51 old numbering) — PATH
# scrubbed of every `.cargo` component so the stub cannot be found either,
# not just reverted to $orig_path (which, on a dev box, has the real tool).
scrubbed_path="$(printf '%s' "$orig_path" | tr ':' '\n' | grep -v '\.cargo' | paste -sd: -)"
cargo_absent_log="$work/cargo_absent.log"
PATH="$scrubbed_path" FILES="$scope" ALLOWLIST="$allowlist" SCOPE_OF_RECORD="$scope" MUTANTS_OUT="$work/out-cargo-absent" \
    bash "$gate" >"$cargo_absent_log" 2>&1
cargo_absent_exit=$?
if [ "$cargo_absent_exit" = 1 ] && grep -qF -- "cargo-mutants not installed" "$cargo_absent_log"; then
    echo "PASS  cargo_mutants_absent_fails (exit $cargo_absent_exit)"
    pass=$((pass + 1))
else
    echo "FAIL  cargo_mutants_absent_fails: expected exit 1 naming cargo-mutants missing, got exit $cargo_absent_exit" >&2
    sed 's/^/        /' "$cargo_absent_log" >&2
    fail=$((fail + 1))
fi

# The `case "$status"` catch-all arm and its propagation: a real crash (OOM,
# a panic, a killed process) is none of the four measured shapes (0/2/3
# judgeable, 4 baseline-failed), and the gate's own exit code must be the
# propagated status, not a fixed 1 — 137 (128+SIGKILL) is what an OOM-killed
# cargo-mutants would report, and is far enough from every other exit code in
# this file that a gate which always exits 1 on error cannot pass this by
# accident.
run status_137_propagates 137 "$steady" 137 "cargo-mutants failed with exit 137"

# The 4 arm specifically (2026-08-09): cargo-mutants exits 4 when the
# UNMUTATED baseline fails to build or test — and still writes all four
# outcome files, all EMPTY (measured, probe D). Under the old `0|3|4` arm that
# fell straight through to the generated==0 branch, which in IN_DIFF mode
# prints "OK — the diff touches nothing mutable" and exits 0: a tree whose
# tests do not even compile reading as a green PR gate. Both modes are pinned
# here because only the IN_DIFF one had the exit-0 sink; the non-IN_DIFF one
# would have failed anyway, but with the WRONG diagnosis ("the run generated 0
# mutants" points at the scope, not at the baseline), which is why the
# `!generated 0 mutants` needle rides both. Proven by restoring `0 | 2 | 3 | 4)`
# in mutants-gate.sh: the IN_DIFF case flips to exit 0 and the non-IN_DIFF
# case to the 0-mutants message, and both needles below miss.
run baseline_failure_fails_loudly 1 "$empty" 4 \
    "baseline build/test failed, nothing was measured" \
    "the UNMUTATED tree does not build" \
    "!generated 0 mutants" "!all reviewed"

in_diff_file="$work/pr.diff"
cat >"$in_diff_file" <<'DIFF'
--- a/crates/luabox-types/src/env.rs
+++ b/crates/luabox-types/src/env.rs
@@ -298,7 +298,7 @@
-    if a > b {
+    if a >= b {
DIFF
IN_DIFF_OVERRIDE="$in_diff_file" \
    run baseline_failure_fails_loudly_in_diff_mode 1 "$empty" 4 \
    "baseline build/test failed, nothing was measured" \
    "!touches nothing mutable in scope" "!generated 0 mutants"

# N33 (#58 review round 5): five load-bearing lines the earlier cases above
# never exercised, found by deleting each of mutants-gate.sh's 232 non-
# comment lines one at a time and re-running this file — those five left it
# 25/25 green. Every case below was checked directly against mutants-gate.sh
# with the named line deleted or neutered before being written the way it
# is now: each such run reproduced the failure this comment describes, then
# the line was restored.

# :52 default_files — FILES' own default AND the scope of record R19 (see
# scope_of_record_narrowing_fails above) is measured against. Every case
# above sets FILES explicitly (via $scope or an override), so none of them
# ever let the gate's actual default resolve — a truncation or typo in the
# four-file literal would sit there undetected. Invoked directly, not
# through run(), with FILES and SCOPE_OF_RECORD both left unset so the real
# default is what the printed command line has to name. Deleting the line
# does not silently narrow the scope, either — this repo's `set -u` turns
# the now-unbound $default_files into a hard crash the moment `files=` reads
# it (confirmed: "line 52: default_files: unbound variable", not a quiet
# empty scope), which is a stronger failure than a truncation would produce
# but still one this case's needles catch, since none of the four paths
# would be in the log.
default_files_log="$work/default_files.log"
STUB_OUTCOMES="$empty" STUB_EXIT=0 \
    ALLOWLIST="$comment_only_allowlist" MUTANTS_OUT="$work/out-default-files" \
    bash "$gate" >"$default_files_log" 2>&1
if grep -qF -- "--file crates/luabox-types/src/env.rs" "$default_files_log" \
    && grep -qF -- "--file crates/luabox-types/src/defs.rs" "$default_files_log" \
    && grep -qF -- "--file crates/luabox-types/src/generics.rs" "$default_files_log" \
    && grep -qF -- "--file crates/luabox-types/src/infer/reify.rs" "$default_files_log"; then
    echo "PASS  default_files_covers_all_four_scope_files"
    pass=$((pass + 1))
else
    echo "FAIL  default_files_covers_all_four_scope_files: expected all four --file args in the log" >&2
    sed 's/^/        /' "$default_files_log" >&2
    fail=$((fail + 1))
fi

# :97 file_args+=(--file "$f") — nothing above ever asserted the printed
# `cargo mutants` command line actually carries a --file argument; deleting
# it silently widens every case in this file to `cargo mutants -p
# luabox-types` over the WHOLE crate without failing a single one, because
# the stub ignores every argument except -o. Reuses the ordinary $scope
# fixture and pins the flag for it by name.
#
# Asserted against the argv the stub actually RECEIVED, not against the
# gate's echoed command string (round 12 review R12-5, applied to both flag
# cases): the echo is a second rendering of the same intent, and a gate that
# printed `--file …` while invoking cargo-mutants without it would satisfy a
# grep of the log. `argv_has <file> <flag> <value>` matches the flag and its
# value as ADJACENT lines, so a flag whose value drifted onto another
# argument cannot pass either.
argv_has() {
    local argv="$1" flag="$2" value="$3"
    grep -A1 -x -F -- "$flag" "$argv" 2>/dev/null | grep -qx -F -- "$value"
}

file_args_log="$work/file_args.log"
file_args_argv="$work/file_args.argv"
STUB_OUTCOMES="$steady" STUB_EXIT=2 STUB_ARGV_OUT="$file_args_argv" \
    FILES="$scope" ALLOWLIST="$allowlist" SCOPE_OF_RECORD="$scope" MUTANTS_OUT="$work/out-file-args" \
    bash "$gate" >"$file_args_log" 2>&1
if argv_has "$file_args_argv" --file crates/luabox-types/src/env.rs; then
    echo "PASS  file_args_scopes_cargo_mutants_to_files"
    pass=$((pass + 1))
else
    echo "FAIL  file_args_scopes_cargo_mutants_to_files: expected --file crates/luabox-types/src/env.rs in the argv cargo-mutants was invoked with" >&2
    sed 's/^/        argv: /' "$file_args_argv" >&2 2>/dev/null || true
    sed 's/^/        /' "$file_args_log" >&2
    fail=$((fail + 1))
fi

# timeout_args=(--timeout "$mutant_timeout") — round 11 review R11-3. Without
# an explicit --timeout, cargo-mutants derives max(baseline x 5, 20s), which
# races the 5/10/20/30s in-test `recv_timeout` bounds several luabox-types
# tests use to turn a dead budget counter into a fast panic: below a ~6s
# baseline the 30s bound loses, its mutant is classified TIMEOUT instead of
# CAUGHT, and the gate fails on a NEW timeout for a test that would have
# killed it.
#
# Asserted against the argv cargo-mutants was really invoked with (round 12
# review R12-5). This case used to grep the gate's own echoed command line,
# which is a SECOND rendering of the same intent: drop `"${timeout_args[@]}"`
# from the `cargo mutants` call while leaving the `echo` above it untouched
# — the exact echo-vs-invocation drift this repo flags elsewhere — and the
# old assertion stayed green while every mutant ran under cargo-mutants'
# derived timeout again. Proven by making exactly that edit and re-running:
# this case, and only this case, fails.
timeout_args_log="$work/timeout_args.log"
timeout_args_argv="$work/timeout_args.argv"
STUB_OUTCOMES="$steady" STUB_EXIT=2 STUB_ARGV_OUT="$timeout_args_argv" \
    FILES="$scope" ALLOWLIST="$allowlist" SCOPE_OF_RECORD="$scope" MUTANTS_OUT="$work/out-timeout-args" \
    bash "$gate" >"$timeout_args_log" 2>&1
if argv_has "$timeout_args_argv" --timeout 90; then
    echo "PASS  timeout_args_bounds_each_mutant_above_every_in_test_bound"
    pass=$((pass + 1))
else
    echo "FAIL  timeout_args_bounds_each_mutant_above_every_in_test_bound: expected --timeout 90 in the argv cargo-mutants was invoked with" >&2
    sed 's/^/        argv: /' "$timeout_args_argv" >&2 2>/dev/null || true
    sed 's/^/        /' "$timeout_args_log" >&2
    fail=$((fail + 1))
fi

# sortpos() (the zero-padding awk function feeding both selection sorts) —
# every fixture above uses positions of equal digit width, so lexicographic
# and numeric order coincide and the function's actual job (making "100"
# sort after "9", not before it) can never be exercised. Two waived lines at
# 9:1 and 100:1, two live mutants at 20:1 and 95:1, all four sharing one
# mutation text: correct (numeric) ascending-rank pairing is 9<->20 and
# 100<->95. Confirmed by neutering sortpos to `return p` (no padding) and
# re-running this exact fixture: the pairing crosses to 100<->20 and 9<->95
# — lexicographically "100:1" sorts before "9:1" because '1' < '9' as the
# first character — which is what these needles would catch.
sortpos_allowlist="$work/sortpos-allowlist.txt"
cat >"$sortpos_allowlist" <<'LIST'
crates/luabox-types/src/env.rs:9:1: replace TypeEnv::is_class -> bool with true	[equivalent] narrow
crates/luabox-types/src/env.rs:100:1: replace TypeEnv::is_class -> bool with true	[equivalent] wide
LIST
sortpos_outcomes="$work/sortpos-outcomes.txt"
cat >"$sortpos_outcomes" <<'OUT'
missed:crates/luabox-types/src/env.rs:20:1: replace TypeEnv::is_class -> bool with true
missed:crates/luabox-types/src/env.rs:95:1: replace TypeEnv::is_class -> bool with true
OUT
sortpos_log="$work/sortpos_zero_pads_numerically.log"
STUB_OUTCOMES="$sortpos_outcomes" STUB_EXIT=2 \
    FILES="$scope" ALLOWLIST="$sortpos_allowlist" SCOPE_OF_RECORD="$scope" MUTANTS_OUT="$work/out-sortpos" \
    bash "$gate" >"$sortpos_log" 2>&1
sortpos_exit=$?
if [ "$sortpos_exit" = 0 ] \
    && grep -A1 -F "was: crates/luabox-types/src/env.rs:9:1" "$sortpos_log" | grep -qF "now: crates/luabox-types/src/env.rs:20:1" \
    && grep -A1 -F "was: crates/luabox-types/src/env.rs:100:1" "$sortpos_log" | grep -qF "now: crates/luabox-types/src/env.rs:95:1"; then
    echo "PASS  sortpos_zero_pads_numerically (exit $sortpos_exit)"
    pass=$((pass + 1))
else
    echo "FAIL  sortpos_zero_pads_numerically: expected 9->20 and 100->95 paired by numeric, not lexicographic, position" >&2
    sed 's/^/        /' "$sortpos_log" >&2
    fail=$((fail + 1))
fi

# Pass 0's tie-break sort and take0 = min(nc, rw0) — when a key has more
# unclaimed waived lines than caught.txt entries to prove them with, this is
# the arithmetic and ordering deciding WHICH waived line(s) retire as STALE
# on caught-entry evidence and which are left open for Pass 2. Two waived
# lines at 50:1 (low) and 60:1 (high), one caught entry (so nc=1, rw0=2,
# take0=1) and one live survivor at 70:1: correct behaviour retires ONLY the
# lower-position line (50) as stale and leaves 60 open, which Pass 2 then
# pairs with the live 70:1 as a shift. Confirmed by changing take0 to `rw0`
# (retire every unclaimed line, ignoring how much caught.txt evidence there
# actually is): both waived lines go stale, the live 70:1 has nothing left
# to pair with and becomes a NEW survivor, and the whole run flips to
# FAILED — every needle below would miss. The allowlist strips reason text
# before comparison (column 1 only), so this is asserted on POSITION, not
# on the [equivalent] label, which never reaches the gate's output at all.
#
# This is also N34's exact shape (#58 review round 5), and doubles as its
# regression case: rw0 (2) > nc (1) here, so which specific line the report
# retires is a position-rank pick, not a proof that THAT line (rather than
# 60:1) is the one nc's single caught entry actually killed — the old
# wording ("a test now kills it — prune it") stated the pick as fact
# regardless. "PROBABLY" is the hedge that must appear on 50:1's line now;
# the un-ambiguous sibling cases above (f1_caught_proof_beats_shared_key,
# r18_killed_and_shifted_beats_shared_key — both rw0 == nc, no leftover
# candidate, nothing to hedge) keep asserting the confident "prune it"
# wording, so this file pins both sides of the distinction.
take0_allowlist="$work/take0-allowlist.txt"
cat >"$take0_allowlist" <<'LIST'
crates/luabox-types/src/env.rs:50:1: replace TypeEnv::is_class -> bool with true	[equivalent] lower position, must retire first
crates/luabox-types/src/env.rs:60:1: replace TypeEnv::is_class -> bool with true	[equivalent] higher position, must stay open for Pass 2
LIST
take0_outcomes="$work/take0-outcomes.txt"
cat >"$take0_outcomes" <<'OUT'
caught:crates/luabox-types/src/env.rs:999:1: replace TypeEnv::is_class -> bool with true
missed:crates/luabox-types/src/env.rs:70:1: replace TypeEnv::is_class -> bool with true
OUT
ALLOWLIST_OVERRIDE="$take0_allowlist" \
    run pass0_retires_lowest_position_first 0 "$take0_outcomes" 2 \
    "1 stale" \
    "PROBABLY no longer survives" "position-rank pick, not individual proof" \
    "crates/luabox-types/src/env.rs:50:1" \
    "was: crates/luabox-types/src/env.rs:60:1" "now: crates/luabox-types/src/env.rs:70:1" \
    "!prune it): crates/luabox-types/src/env.rs:50:1" \
    "!prune it): crates/luabox-types/src/env.rs:60:1" \
    "!was: crates/luabox-types/src/env.rs:50:1"

# #58 review round 6, M25: the case directly above lists its two waived
# lines in ASCENDING position order in the allowlist FILE (50 before 60),
# which is also their encounter order — so it never isolates Pass 0's own
# tie-break sort (mutants-gate.sh's `rkey0`/`ridx0` selection-sort block)
# from the plain encounter order the W lines arrive in: deleting that sort
# left this file 31/31 green, because sorting an already-sorted array is a
# no-op (confirmed directly against mutants-gate.sh with the sort block
# removed and this fixture — see below — re-run against it). Reversed here:
# the allowlist FILE lists the HIGH position (80) first and the LOW
# position (20) second — encounter order is [80, 60]... no: [80, 20] —
# while nc=1 (one caught entry) still proves exactly one of the two dead.
# Correct (sorted) behaviour retires the LOWER position (20) as STALE
# regardless of file order and leaves 80 open for Pass 2 to pair with the
# live survivor at 90. Confirmed by deleting the same `rkey0`/`ridx0` sort
# block mutants-gate.sh's pass0_retires_lowest_position_first case above
# checks: WITH the sort this fixture prints "STALE ...:20:1" (PROBABLY) and
# "was: ...:80:1 / now: ...:90:1"; WITHOUT it, encounter order (not
# position) decides, and the whole pairing flips — "STALE ...:80:1" and
# "was: ...:20:1 / now: ...:90:1" instead: two different reviewed reasons
# re-pinned onto the same live mutant depending on whether the sort runs,
# exit 0 either way, exactly what the review found unproven by the
# ascending-order case alone.
descending_allowlist="$work/descending-allowlist.txt"
cat >"$descending_allowlist" <<'LIST'
crates/luabox-types/src/env.rs:80:1: replace TypeEnv::is_class -> bool with true	[equivalent] higher position, listed FIRST in the file
crates/luabox-types/src/env.rs:20:1: replace TypeEnv::is_class -> bool with true	[equivalent] lower position, listed SECOND in the file
LIST
descending_outcomes="$work/descending-outcomes.txt"
cat >"$descending_outcomes" <<'OUT'
caught:crates/luabox-types/src/env.rs:999:1: replace TypeEnv::is_class -> bool with true
missed:crates/luabox-types/src/env.rs:90:1: replace TypeEnv::is_class -> bool with true
OUT
ALLOWLIST_OVERRIDE="$descending_allowlist" \
    run pass0_tie_break_sort_is_load_bearing_on_descending_allowlist 0 "$descending_outcomes" 2 \
    "1 stale" \
    "PROBABLY no longer survives" "position-rank pick, not individual proof" \
    "crates/luabox-types/src/env.rs:20:1" \
    "was: crates/luabox-types/src/env.rs:80:1" "now: crates/luabox-types/src/env.rs:90:1" \
    "!prune it): crates/luabox-types/src/env.rs:20:1" \
    "!prune it): crates/luabox-types/src/env.rs:80:1" \
    "!was: crates/luabox-types/src/env.rs:20:1"

# `| sort` at the end of the classification pipeline — mixed_batch_output_is_
# sorted above already discloses that its own 2-key fixture happens to come
# out in sorted order from gawk's unsorted hash iteration anyway, so it does
# not prove the line load-bearing. This fixture was found empirically (run
# with `| sort` stripped, try candidate multi-key batches, keep the one that
# actually differs): four keys spanning all four kinds — NEW (900), SHIFT
# (fn_b, 20->25), STALE (fn_a, 10) and STALE (fn_c, 30), TIMEOUT (fn_d, 40)
# — whose natural gawk iteration order this run measured as STALE(fn_c),
# SHIFT(fn_b), STALE(fn_a), NEW, TIMEOUT — provably NOT the sorted order
# (NEW, SHIFT, STALE(fn_a=10), STALE(fn_c=30), TIMEOUT(fn_d)) `| sort`
# produces and this case asserts.
sort_allowlist="$work/sort-allowlist.txt"
cat >"$sort_allowlist" <<'LIST'
crates/luabox-types/src/env.rs:10:1: replace fn_a with ()	[equivalent] a
crates/luabox-types/src/env.rs:20:1: replace fn_b with ()	[equivalent] b
crates/luabox-types/src/env.rs:30:1: replace fn_c with ()	[equivalent] c
crates/luabox-types/src/env.rs:40:1: replace fn_d with ()	[equivalent] d
LIST
sort_outcomes="$work/sort-outcomes.txt"
cat >"$sort_outcomes" <<'OUT'
missed:crates/luabox-types/src/env.rs:900:1: replace fn_new with ()
missed:crates/luabox-types/src/env.rs:25:1: replace fn_b with ()
caught:crates/luabox-types/src/env.rs:30:1: replace fn_c with ()
timeout:crates/luabox-types/src/env.rs:40:1: replace fn_d with ()
OUT
sort_log="$work/pipeline_output_is_sorted.log"
STUB_OUTCOMES="$sort_outcomes" STUB_EXIT=3 \
    FILES="$scope" ALLOWLIST="$sort_allowlist" SCOPE_OF_RECORD="$scope" MUTANTS_OUT="$work/out-sort" \
    bash "$gate" >"$sort_log" 2>&1
new_line="$(grep -n 'new surviving mutant' "$sort_log" | head -1 | cut -d: -f1)"
shift_line="$(grep -n 'waived mutant moved' "$sort_log" | head -1 | cut -d: -f1)"
stale_a_line="$(grep -n 'env.rs:10:1: replace fn_a' "$sort_log" | grep 'prune it' | head -1 | cut -d: -f1)"
stale_c_line="$(grep -n 'env.rs:30:1: replace fn_c' "$sort_log" | grep 'prune it' | head -1 | cut -d: -f1)"
timeout_line="$(grep -n 'timed out this run rather than surviving' "$sort_log" | head -1 | cut -d: -f1)"
if [ -n "$new_line" ] && [ -n "$shift_line" ] && [ -n "$stale_a_line" ] && [ -n "$stale_c_line" ] && [ -n "$timeout_line" ] \
    && [ "$new_line" -lt "$shift_line" ] && [ "$shift_line" -lt "$stale_a_line" ] \
    && [ "$stale_a_line" -lt "$stale_c_line" ] && [ "$stale_c_line" -lt "$timeout_line" ]; then
    echo "PASS  pipeline_output_is_sorted_across_four_kinds (NEW $new_line < SHIFT $shift_line < STALE(a) $stale_a_line < STALE(c) $stale_c_line < TIMEOUT $timeout_line)"
    pass=$((pass + 1))
else
    echo "FAIL  pipeline_output_is_sorted_across_four_kinds: expected NEW < SHIFT < STALE(fn_a) < STALE(fn_c) < TIMEOUT" >&2
    sed 's/^/        /' "$sort_log" >&2
    fail=$((fail + 1))
fi

# ---------------------------------------------------------------------------
# IN_DIFF mode (2026-08-09). `mutants-pr` (.github/workflows/mutants.yml) is
# the only BLOCKING mutation job on the merge path, and it is the only caller
# that sets IN_DIFF — yet until this block not one case in this file ever set
# it, so all four of the branches the flag exists to switch shipped with zero
# coverage. Three of them are exit-0 paths, which is the worst possible
# combination: a bug in any of them is a green PR gate.
# ---------------------------------------------------------------------------

# The guard on the flag's own input. A CI job that computes the diff path
# wrongly (an empty base ref, a path relative to the wrong cwd) must fail
# here, not pass the nonexistent path to cargo-mutants — which accepts
# `--in-diff` against a file it cannot read by erroring out with a code this
# gate would then propagate as an opaque crash. `!bounded to a diff` is the
# load-bearing needle: the "does not exist" echo sits BEFORE the target exit,
# so it survives the line's deletion; the command-line echo that names the
# diff only ever runs after it.
IN_DIFF_OVERRIDE="$work/no-such.diff" \
    run in_diff_missing_file_fails 1 "$steady" 2 \
    "IN_DIFF names a diff file that does not exist" \
    "!bounded to a diff" "!all reviewed"

# The exit-0 path the baseline_failure case above shares a sink with, in its
# LEGITIMATE form: a PR whose diff genuinely touches nothing mutable in scope.
# This must stay exit 0 (a docs-only PR cannot be made to fail the mutation
# gate), and must NOT print the full-scope "generated 0 mutants" failure —
# which is what the branch would collapse to if the `[ -n "$in_diff" ]` test
# guarding it were deleted.
IN_DIFF_OVERRIDE="$in_diff_file" \
    run in_diff_zero_generated_is_ok 0 "$empty" 0 \
    "touches nothing mutable in scope" "nothing to audit this run" \
    "!generated 0 mutants" "!nothing was audited"

# A waived line with no live evidence in a diff-bounded run is NOT proven
# dead: the run never attempted it. Reporting it as "a test now kills it —
# prune it" would walk a reviewer into deleting a reviewed waiver for a
# survivor that is still alive and simply outside the PR's hunks. The fixture
# stages 298:30 and 301:27 live and 1333:9 absent with NO caught.txt entry, so
# the residual line has no evidence of any kind — the exact shape the diff
# bound produces. Both wordings are asserted, the per-line NOTE and the
# summary's stale_label, because they are two independent lines
# (mutants-gate.sh's STALE case and its `stale_label` assignment) and either
# can rot without the other.
in_diff_stale="$work/in-diff-stale.txt"
cat >"$in_diff_stale" <<'OUT'
missed:crates/luabox-types/src/env.rs:298:30: replace > with >= in TypeEnv::build_from_items
missed:crates/luabox-types/src/env.rs:301:27: replace > with >= in TypeEnv::build_from_items
OUT
IN_DIFF_OVERRIDE="$in_diff_file" \
    run in_diff_untouched_waiver_is_not_measured_not_pruned 0 "$in_diff_stale" 2 \
    "not touched by this diff-bounded run" "do not prune from this alone" \
    "not measured, not pruned" \
    "!prune it" "!1 stale allowlist line(s)"

# The total-staleness FAIL is the gate's scope-drift alarm, and it is
# DELIBERATELY skipped under IN_DIFF: a small diff touching none of the waived
# lines makes 100% staleness the ordinary outcome, not evidence of a
# misaimed audit. Same fixture as whole_allowlist_stale_fails above, which
# pins the alarm firing at full scope — this case pins it silent under the
# flag, so the pair discriminates the `[ -z "$in_diff" ]` conjunct
# specifically rather than the guard as a whole. Delete that conjunct from
# mutants-gate.sh and this case goes RED (exit 1, "went stale in one run"),
# while whole_allowlist_stale_fails stays green: every mutants-pr run over a
# diff that misses the allowlist would fail the PR.
IN_DIFF_OVERRIDE="$in_diff_file" \
    run in_diff_skips_total_staleness_fail 0 "$elsewhere" 0 \
    "not measured, not pruned" "OK" \
    "!went stale in one run" "!wrong scope"

# ---------------------------------------------------------------------------
# selftest-lib.sh's own report guard (decisions/12 §2: a guard with no
# discriminating case is a decoration). `selftest_report` returns
# `fail -eq 0 && pass -gt 0`; the second conjunct is what stops a suite whose
# cases all silently vanished — a corpus wiring break, a helper rename — from
# exiting 0 on "0 passed, 0 failed". Nothing in this directory could exercise
# it, because every suite that sources the library runs at least one case
# before reporting. A child bash that sources the library and reports having
# run NOTHING can, and costs one process.
#
# Both halves are asserted so the case discriminates the conjunct rather than
# the function: with `[ "$pass" -gt 0 ]` deleted the zero-case run exits 0 and
# the first assertion goes RED; the one-case control stays green either way
# and proves the empty-suite failure is not simply "selftest_report always
# fails".
# ---------------------------------------------------------------------------
lib_zero_log="$work/selftest_lib_zero_cases.log"
bash -c 'set -u; source "$1"; selftest_report "empty-suite"' _ "$here/selftest-lib.sh" \
    >"$lib_zero_log" 2>&1
lib_zero_exit=$?
assert_exit selftest_report_fails_a_suite_that_ran_no_cases 1 "$lib_zero_exit" "$lib_zero_log" \
    "empty-suite: 0 passed, 0 failed"

lib_one_log="$work/selftest_lib_one_case.log"
bash -c 'set -u; source "$1"; check control ok ok; selftest_report "one-case-suite"' _ "$here/selftest-lib.sh" \
    >"$lib_one_log" 2>&1
lib_one_exit=$?
assert_exit selftest_report_passes_a_suite_that_ran_one_case 0 "$lib_one_exit" "$lib_one_log" \
    "one-case-suite: 1 passed, 0 failed"

selftest_report "mutants-gate-selftest"

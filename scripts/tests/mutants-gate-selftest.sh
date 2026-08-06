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
# cargo-mutants itself is out of scope here — but is NOT "pinned", whatever
# an earlier version of this comment claimed: mutants.yml installs it as
# `tool: cargo-mutants` with no `@x.y.z`, so install-action resolves whatever
# the latest release is at run time. The allowlist keys on cargo-mutants'
# verbatim wording (mutants-allowlist.txt's column 1), so an upstream rename
# of a mutation ("replace > with >=" becoming something else) would turn
# every waived line stale and every live mutant new in the same run — a real
# gap, with no fixture here or anywhere else that can exercise it without
# actually swapping the installed binary.
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

# A stub cargo-mutants: writes the outcome files staged in $STUB_OUTCOMES
# (one `name:line` per line, name in caught|missed|timeout|unviable) into the
# -o directory, then exits $STUB_EXIT. `cargo mutants` dispatches to whatever
# `cargo-mutants` is first on PATH, so this substitutes for the real tool.
mkdir -p "$work/bin"
cat >"$work/bin/cargo-mutants" <<'STUB'
#!/bin/bash
out=""
prev=""
for arg in "$@"; do
    [ "$prev" = "-o" ] && out="$arg"
    prev="$arg"
done
mkdir -p "$out/mutants.out"
: >"$out/mutants.out/caught.txt"
: >"$out/mutants.out/missed.txt"
: >"$out/mutants.out/timeout.txt"
: >"$out/mutants.out/unviable.txt"
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

pass=0
fail=0

# run <name> <exit-expected> <outcomes-file> <stub-exit> <needle>...
# A needle prefixed with `!` must be ABSENT from the log rather than present
# — a second, unrelated failure path can supply the same "expected" string a
# deleted line was supposed to produce, and a needle can only prove presence.
# FILES_OVERRIDE / ALLOWLIST_OVERRIDE (set by the caller as temporary
# variable assignments on the `run` call itself) substitute for the fixture
# scope/allowlist without hand-rolling a second invocation of the gate.
run() {
    local name="$1" want_exit="$2" outcomes="$3" stub_exit="$4"
    shift 4
    local log="$work/$name.log"
    local files="${FILES_OVERRIDE:-$scope}"
    local list="${ALLOWLIST_OVERRIDE:-$allowlist}"
    STUB_OUTCOMES="$outcomes" STUB_EXIT="$stub_exit" \
        FILES="$files" ALLOWLIST="$list" MUTANTS_OUT="$work/out-$name" \
        bash "$gate" >"$log" 2>&1
    local got=$?
    local ok=1
    local report=""
    [ "$got" = "$want_exit" ] || {
        ok=0
        report="$report expected exit $want_exit"
    }
    local needle
    for needle in "$@"; do
        case "$needle" in
        !*)
            if grep -qF -- "${needle#!}" "$log"; then
                ok=0
                report="$report present-but-should-be-absent:[${needle#!}]"
            fi
            ;;
        *)
            if ! grep -qF -- "$needle" "$log"; then
                ok=0
                report="$report missing:[$needle]"
            fi
            ;;
        esac
    done
    if [ "$ok" = 1 ]; then
        echo "PASS  $name (exit $got)"
        pass=$((pass + 1))
    else
        echo "FAIL  $name: got exit $got.$report" >&2
        sed 's/^/        /' "$log" >&2
        fail=$((fail + 1))
    fi
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
run steady_state 0 "$steady" 3 "3 survivor(s)" "all reviewed" "0 shifted" "0 new" "report directory:"

# The round-1 gap: a run where every survivor timed out. Unkilled is unkilled.
flap="$work/flap.txt"
cat >"$flap" <<'OUT'
timeout:crates/luabox-types/src/env.rs:298:30: replace > with >= in TypeEnv::build_from_items
missed:crates/luabox-types/src/env.rs:301:27: replace > with >= in TypeEnv::build_from_items
missed:crates/luabox-types/src/env.rs:1333:9: replace TypeEnv::is_class -> bool with true
OUT
run waived_line_flaps_to_timeout 0 "$flap" 4 "timed out this run rather than surviving" "0 shifted"

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
run new_timeout_fails 1 "$new_timeout" 4 \
    "new timed-out mutant" "brand_new" "1 new timed out" "unkilled, not killed"

new_survivor="$work/new.txt"
cat >"$new_survivor" <<'OUT'
missed:crates/luabox-types/src/env.rs:900:5: replace TypeEnv::brand_new with ()
missed:crates/luabox-types/src/env.rs:298:30: replace > with >= in TypeEnv::build_from_items
missed:crates/luabox-types/src/env.rs:301:27: replace > with >= in TypeEnv::build_from_items
missed:crates/luabox-types/src/env.rs:1333:9: replace TypeEnv::is_class -> bool with true
OUT
run new_survivor_fails 1 "$new_survivor" 3 "new surviving mutant" "brand_new" "1 new"

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
run shifted_batch_is_not_new 0 "$shifted" 3 "3 shifted" "0 new" "re-pin the 3 shifted" \
    "was: crates/luabox-types/src/env.rs:298:30" "now: crates/luabox-types/src/env.rs:309:30" \
    "old: crates/luabox-types/src/env.rs:298:30" "new: crates/luabox-types/src/env.rs:309:30"

# The mixed batch that defeated the old rule: two shifts and one genuinely
# new mutant in the same run. Counts alone cannot separate them; the key can.
mixed="$work/mixed.txt"
cat >"$mixed" <<'OUT'
missed:crates/luabox-types/src/env.rs:309:30: replace > with >= in TypeEnv::build_from_items
missed:crates/luabox-types/src/env.rs:312:27: replace > with >= in TypeEnv::build_from_items
missed:crates/luabox-types/src/env.rs:1344:9: replace TypeEnv::is_class -> bool with true
missed:crates/luabox-types/src/env.rs:915:5: replace TypeEnv::brand_new with ()
OUT
run mixed_shift_and_new_batch 1 "$mixed" 3 "3 shifted" "1 new" "brand_new"

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
run one_killed_waiver_is_stale_not_failure 0 "$killed" 3 "prune it" "1 stale"

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
run f1_caught_proof_beats_shared_key 1 "$proven_kill" 3 \
    "new surviving mutant" "315:19" "1 new" "0 shifted" "1 stale" \
    "!was: crates/luabox-types/src/env.rs:298:30"

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
STUB_OUTCOMES="$crossed" STUB_EXIT=3 \
    FILES="$scope" ALLOWLIST="$crossed_allowlist" MUTANTS_OUT="$work/out-pass2-pairing" \
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
# not a hand-rolled invocation with its own grep and its own message.
FILES_OVERRIDE="crates/luabox-types/src/renamed_by_a_refactor.rs" \
    run missing_scope_file_fails 1 "$steady" 3 "scoped file does not exist"

# The allowlist itself can go missing (a bad ALLOWLIST override, a checkout
# that dropped the file) — distinct from "every line in it went stale".
ALLOWLIST_OVERRIDE="$work/does-not-exist.txt" \
    run missing_allowlist_fails 1 "$steady" 3 "no allowlist at"

# F4: a scope naming a file that EXISTS but excludes a file the allowlist
# waives mutants in must fail before spending the run — not silently audit a
# fraction of the claimed surface and report 0 new / 0 stale / exit 0. Real
# file, wrong one: distinct from missing_scope_file_fails (F14 shape) above.
FILES_OVERRIDE="crates/luabox-types/src/defs.rs" \
    run scope_excludes_a_waived_file_fails 1 "$steady" 3 \
    "the allowlist waives mutants in a file this run's scope does not audit" \
    "crates/luabox-types/src/env.rs"

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
    STUB_OUTCOMES="$steady" STUB_EXIT=3 \
        FILES="$scope" ALLOWLIST="$allowlist" MUTANTS_OUT="relout" \
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
PATH="$scrubbed_path" FILES="$scope" ALLOWLIST="$allowlist" MUTANTS_OUT="$work/out-cargo-absent" \
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

# The `case "$status"` arms and the `:83-84` propagation: a real crash (OOM,
# a panic, a killed process) is neither 3 (missed) nor 4 (timeout-only), and
# the gate's own exit code must be the propagated status, not a fixed 1 —
# 137 (128+SIGKILL) is what an OOM-killed cargo-mutants would report, and is
# far enough from every other exit code in this file that a gate which
# always exits 1 on error cannot pass this by accident.
run status_137_propagates 137 "$steady" 137 "cargo-mutants failed with exit 137"

echo
echo "mutants-gate-selftest: $pass passed, $fail failed"
[ "$fail" -eq 0 ]

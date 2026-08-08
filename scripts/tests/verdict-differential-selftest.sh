#!/bin/bash
# Self-test for the verdict regression oracle — the gate on the gate, same
# discipline as luals-differential-selftest.sh and mutants-gate-selftest.sh.
#
# Why. A gate that cannot fail is worse than no gate: it reads green while
# measuring nothing, and PR #61's five review rounds each found exactly that
# shape in some OTHER script before this one existed (an ignored `cp`
# failure, a bare clean/diag check that can't tell one code from another, a
# self-test that existed but that CI never ran). Every case below was
# PROVEN, by deleting or neutering the line(s) in verdict-differential.sh it
# names and re-running this file, to fail when that line is gone — the
# table is reported at the end of the PR description, not asserted in prose.
#
# luabox is STUBBED — a content-driven script that reads
# `-- STUB-LUABOX-DIAG: <CODE>` marker comments out of the fixture .lua
# files and reports exactly those codes, so a fixture pins an exact, known
# code set without a real typecheck. What is under test is the DRIVER's
# judgement, not the checker — same division of labour as
# luals-differential-selftest.sh. Each fixture corpus is a few files under
# $work, pointed at via VERDICT_CORPUS (the same mechanism
# verdict-differential.sh's LUALS_CORPUS-alike gives this file).
#
# Every case asserts an exit code AND at least one discriminating string
# that only the code path under test can produce — several also assert a
# string must be ABSENT (`!needle`), because a second, unrelated failure can
# supply the same "expected" exit code or substring a deleted line was
# supposed to produce. Run it in seconds:
#
#   bash scripts/tests/verdict-differential-selftest.sh
set -u

here="$(cd "$(dirname "$0")" && pwd)"
gate="$here/verdict-differential.sh"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# Round 6 review M49: the pass/fail counters, the exit-plus-`!needle`
# assertion and M35's "measured nothing" guard live in one place now.
# shellcheck source=scripts/tests/selftest-lib.sh
. "$here/selftest-lib.sh"

# --- stub tool ---------------------------------------------------------
mkdir -p "$work/bin"

stub_luabox="$work/bin/luabox"
cat >"$stub_luabox" <<'STUB'
#!/bin/bash
# stub luabox: understands only `check --format json`, invoked from inside
# the differential script's per-case temp project. Reports exactly the
# codes named by a `-- STUB-LUABOX-DIAG: <CODE>` marker comment on its own
# line in any *.lua file under ./src (case file or copied dep) OR ./defs
# (an ambient package copied by a `.defs` sidecar, M33) — scanning both is
# what lets a self-test case PROVE a `.defs` sidecar's file actually landed
# on disk, not just that the driver didn't crash.
#
# A `-- STUB-LUABOX-BREAK-JSON` marker instead makes the stub emit output
# that is not valid JSON at all — standing in for a real tool bug rather
# than a well-formed "0 diagnostics" or "N diagnostics" answer.
#
# A `-- STUB-LUABOX-EXIT: <N>` marker overrides the exit code the stub
# would otherwise use (0 for no codes, 1 for any) — M31: the real
# discriminator this gate was missing was luabox's own exit status, so the
# self-test needs to be able to decouple "printed diagnostics" from "exited
# nonzero" in both directions (empty output + nonzero exit; diagnostics +
# an exit code other than the default 1) rather than the stub's normal
# behaviour always keeping the two in lockstep.
#
# The manifest itself (M60) is asserted a different way — see
# write_case_manifest in verdict-differential.sh, which echoes what it
# wrote to ITS OWN stdout per case, independent of whether this stub (or
# the real luabox) ever opens luabox.toml at all.
if [ "${1:-}" != "check" ]; then
    echo "stub-luabox: unsupported invocation: $*" >&2
    exit 2
fi
codes=()
break_json=0
exit_override=""
shopt -s nullglob
for f in src/*.lua defs/*.lua; do
    if grep -qF -- '-- STUB-LUABOX-BREAK-JSON' "$f"; then
        break_json=1
    fi
    while IFS= read -r code; do
        codes+=("$code")
    done < <(grep -oE -- '-- STUB-LUABOX-DIAG: [A-Za-z0-9]+' "$f" | awk '{print $3}')
    m="$(grep -oE -- '-- STUB-LUABOX-EXIT: [0-9]+' "$f" | awk '{print $3}' | tail -1)"
    [ -n "$m" ] && exit_override="$m"
done
shopt -u nullglob
if [ "$break_json" = 1 ]; then
    printf 'not-json: stub-luabox produced a malformed response\n'
    echo "check: stub forced a malformed response" >&2
    exit "${exit_override:-0}"
fi
if [ "${#codes[@]}" -eq 0 ]; then
    printf '[]\n'
    echo "check: 0 errors, 0 warnings in 1 files" >&2
    exit "${exit_override:-0}"
fi
out="["
sep=""
for c in "${codes[@]}"; do
    out="$out$sep{\"code\":\"$c\",\"severity\":\"error\",\"message\":\"stub\"}"
    sep=","
done
printf '%s]\n' "$out"
echo "check: ${#codes[@]} errors, 0 warnings in 1 files" >&2
echo "Error: check failed with ${#codes[@]} error(s)" >&2
exit "${exit_override:-1}"
STUB
chmod +x "$stub_luabox"

# --- helpers -----------------------------------------------------------
newcorpus() {
    local d="$work/corpus-$1"
    mkdir -p "$d"
    printf '%s' "$d"
}

# assert <name> <want-exit> <got-exit> <log> <needle>...
# This file's local name for selftest-lib.sh's `assert_exit`, kept so the
# ~40 call sites below read as they always have.
assert() { assert_exit "$@"; }

# run <name> <want-exit> <corpus-dir> <needle>...
# Standard invocation: stub luabox, real (missing) VERDICT_PRINT.
run() {
    local name="$1" want_exit="$2" corpus_dir="$3"
    shift 3
    local log="$work/$name.log"
    LUABOX="$stub_luabox" VERDICT_CORPUS="$corpus_dir" \
        bash "$gate" >"$log" 2>&1
    local got=$?
    assert "$name" "$want_exit" "$got" "$log" "$@"
}

# ============================================================================
# 1: the luabox binary existence guard.
# ============================================================================
log="$work/luabox_binary_missing_fails.log"
LUABOX="$work/bin/does-not-exist-luabox" VERDICT_CORPUS="$(newcorpus dummy1)" \
    bash "$gate" >"$log" 2>&1
got=$?
assert luabox_binary_missing_fails 1 "$got" "$log" \
    "error: no luabox binary at"

# ============================================================================
# 2: the corpus directory existence guard.
# ============================================================================
log="$work/corpus_dir_missing_fails.log"
LUABOX="$stub_luabox" VERDICT_CORPUS="$work/does-not-exist-corpus" \
    bash "$gate" >"$log" 2>&1
got=$?
assert corpus_dir_missing_fails 1 "$got" "$log" \
    "error: no corpus directory at"

# ============================================================================
# 3: the expected.tsv existence guard.
# ============================================================================
no_expected_corpus="$(newcorpus no-expected)"
cat >"$no_expected_corpus/only_case.lua" <<'LUA'
-- no expected.tsv in this corpus directory at all
LUA
log="$work/expected_tsv_missing_fails.log"
LUABOX="$stub_luabox" VERDICT_CORPUS="$no_expected_corpus" \
    bash "$gate" >"$log" 2>&1
got=$?
assert expected_tsv_missing_fails 1 "$got" "$log" \
    "error: no expectations at"

# ============================================================================
# 4: an expected.tsv with zero case rows (header/comments only) is refused,
# not read as "0 failures across 0 rows".
# ============================================================================
empty_rows_corpus="$(newcorpus empty-rows)"
printf '# case\tverdict\tcodes\tnote\n' >"$empty_rows_corpus/expected.tsv"
run zero_rows_refused 1 "$empty_rows_corpus" \
    "NO ROWS MEASURED"

# Same principle, but the corpus directory itself is empty (no .lua files
# either) — must not read as vacuously green.
truly_empty_corpus="$(newcorpus truly-empty)"
printf '# case\tverdict\tcodes\tnote\n' >"$truly_empty_corpus/expected.tsv"
run truly_empty_corpus_refused 1 "$truly_empty_corpus" \
    "NO ROWS MEASURED"

# ============================================================================
# 5: python3 is a declared, checked dependency.
# ============================================================================
python3_corpus="$(newcorpus python3-check)"
cat >"$python3_corpus/only_case.lua" <<'LUA'
-- STUB-LUABOX-DIAG: LB0300
LUA
printf '# case\tverdict\tcodes\tnote\nonly_case\tdiag\tLB0300\tcontrol\n' \
    >"$python3_corpus/expected.tsv"

scrubbed="$work/bin-no-python"
mkdir -p "$scrubbed"
for b in bash grep sed awk cp rm mkdir cat basename dirname mktemp printf \
    sort uniq paste tr command true false head; do
    p="$(command -v "$b" 2>/dev/null)"
    [ -n "$p" ] && ln -sf "$p" "$scrubbed/$b"
done
log="$work/python3_absent_fails_loudly.log"
PATH="$scrubbed" LUABOX="$stub_luabox" VERDICT_CORPUS="$python3_corpus" \
    bash "$gate" >"$log" 2>&1
got=$?
assert python3_absent_fails_loudly 1 "$got" "$log" \
    "python3 not found on PATH — required to parse"

# ============================================================================
# 6: a relative LUABOX resolves against the CALLER's cwd, not the per-case
# temp project cwd the gate later `cd`s into.
# ============================================================================
rel_corpus="$(newcorpus relative-luabox)"
cat >"$rel_corpus/only_case.lua" <<'LUA'
-- STUB-LUABOX-DIAG: LB0300
LUA
printf '# case\tverdict\tcodes\tnote\nonly_case\tdiag\tLB0300\tcontrol\n' \
    >"$rel_corpus/expected.tsv"

rel_wd="$work/rel-cwd"
mkdir -p "$rel_wd"
cp "$stub_luabox" "$rel_wd/rel-stub-luabox"
log="$work/relative_luabox_resolved_against_cwd.log"
(
    cd "$rel_wd" && LUABOX="./rel-stub-luabox" VERDICT_CORPUS="$rel_corpus" \
        bash "$gate" >"$log" 2>&1
)
got=$?
assert relative_luabox_resolved_against_cwd 0 "$got" "$log" \
    "match expected.tsv" \
    "!could not parse 'luabox check --format json' output as JSON"

# ============================================================================
# 7-10: `.deps` handling.
# ============================================================================

# 7: a case whose .deps sidecar names a file that is not in the corpus.
deps_missing_corpus="$(newcorpus deps-missing)"
cat >"$deps_missing_corpus/carrier.lua" <<'LUA'
-- nothing to flag on its own
LUA
printf 'nonexistent_module.lua\n' >"$deps_missing_corpus/carrier.deps"
printf '# case\tverdict\tcodes\tnote\ncarrier\tclean\t-\tcontrol\n' \
    >"$deps_missing_corpus/expected.tsv"
run deps_copy_failure_fails 1 "$deps_missing_corpus" \
    "could not copy dependency 'nonexistent_module.lua'"

# 8: a case whose .deps sidecar tries to escape the corpus directory.
deps_unsafe_corpus="$(newcorpus deps-unsafe)"
cat >"$deps_unsafe_corpus/carrier.lua" <<'LUA'
-- nothing to flag on its own
LUA
printf '../../../etc/hostname\n' >"$deps_unsafe_corpus/carrier.deps"
printf '# case\tverdict\tcodes\tnote\ncarrier\tclean\t-\tcontrol\n' \
    >"$deps_unsafe_corpus/expected.tsv"
run deps_traversal_guard_rejects_unsafe_name 1 "$deps_unsafe_corpus" \
    "unsafe dependency name '../../../etc/hostname'" \
    "!could not copy dependency"

# 9: the .deps sidecar's one line has no trailing newline.
deps_noeol_corpus="$(newcorpus deps-noeol)"
cat >"$deps_noeol_corpus/helper.lua" <<'LUA'
-- STUB-LUABOX-DIAG: LB0306
LUA
cat >"$deps_noeol_corpus/carrier.lua" <<'LUA'
-- carrier itself is clean; the diagnostic can only come from helper.lua
LUA
printf 'helper.lua' >"$deps_noeol_corpus/carrier.deps"  # deliberately no \n
printf '# case\tverdict\tcodes\tnote\ncarrier\tdiag\tLB0306\tthe dep is copied even without a trailing newline\n' \
    >"$deps_noeol_corpus/expected.tsv"
run deps_final_line_without_newline_is_copied 0 "$deps_noeol_corpus" \
    "carrier                                       diag     LB0306"

# ============================================================================
# 10: a `.deps` sidecar naming MULTIPLE dependencies (one per line) — every
# line is copied, not just the first. copy_case_deps's `while read` loop was
# always structurally generic over N lines, but nothing before this case
# ever proved it against more than one; the real corpus started using
# multi-line `.deps` for the first time seeding the class-merge-precedence
# cells (e.g. field_dup_cross_file_first_processed_wins names two files,
# `_a.lua` and `_b.lua`), so this is no longer a hypothetical shape. Two
# distinct diagnostic markers, one per dependency, both must show up in the
# measured code set — proves BOTH lines were read and copied, not just the
# first with the rest silently dropped.
# ============================================================================
deps_multi_corpus="$(newcorpus deps-multi)"
cat >"$deps_multi_corpus/dep_one.lua" <<'LUA'
-- STUB-LUABOX-DIAG: LB0300
LUA
cat >"$deps_multi_corpus/dep_two.lua" <<'LUA'
-- STUB-LUABOX-DIAG: LB0306
LUA
cat >"$deps_multi_corpus/carrier.lua" <<'LUA'
-- carrier itself is clean; both diagnostics come from its dependencies
LUA
printf 'dep_one.lua\ndep_two.lua\n' >"$deps_multi_corpus/carrier.deps"
printf '# case\tverdict\tcodes\tnote\ncarrier\tdiag\tLB0300,LB0306\tboth deps in a multi-line .deps sidecar are copied\n' \
    >"$deps_multi_corpus/expected.tsv"
run deps_multiple_dependencies_all_copied 0 "$deps_multi_corpus" \
    "match expected.tsv" \
    "!luabox codes are"

# ============================================================================
# 11: the exact diagnostic CODE SET is asserted, not just clean/diag.
# ============================================================================
codes_corpus="$(newcorpus codes-mismatch)"
cat >"$codes_corpus/only_case.lua" <<'LUA'
-- STUB-LUABOX-DIAG: LB0300
LUA
# Both readings are "diag" — only the code differs, so a bare clean/diag
# check would pass this and hide the mismatch.
printf '# case\tverdict\tcodes\tnote\nonly_case\tdiag\tLB0301\tcontrol\n' \
    >"$codes_corpus/expected.tsv"
run codes_mismatch_fails 1 "$codes_corpus" \
    "luabox codes are 'LB0300', expected.tsv says 'LB0301'" \
    "!luabox is 'clean'"

# ============================================================================
# 12: a verdict mismatch (clean vs diag) fails, independent of the codes
# comparison above.
# ============================================================================
verdict_corpus="$(newcorpus verdict-mismatch)"
cat >"$verdict_corpus/only_case.lua" <<'LUA'
-- STUB-LUABOX-DIAG: LB0300
LUA
printf '# case\tverdict\tcodes\tnote\nonly_case\tclean\t-\tcontrol\n' \
    >"$verdict_corpus/expected.tsv"
run verdict_mismatch_fails 1 "$verdict_corpus" \
    "luabox is 'diag', expected.tsv says 'clean'"

# ============================================================================
# 13: a row with an empty note column is refused — every row, not only a
# "diverging" one (there is no divergence axis in this gate).
# ============================================================================
empty_note_corpus="$(newcorpus empty-note)"
cat >"$empty_note_corpus/only_case.lua" <<'LUA'
-- nothing flagged
LUA
printf '# case\tverdict\tcodes\tnote\nonly_case\tclean\t-\t\n' \
    >"$empty_note_corpus/expected.tsv"
run empty_note_fails 1 "$empty_note_corpus" \
    "note column is empty"

# ============================================================================
# 14: the unclaimed-corpus sweep — a *.lua file with no row in expected.tsv
# at all (and not named by any .deps sidecar) must fail, not pass by
# omission.
# ============================================================================
unclaimed_corpus="$(newcorpus unclaimed)"
cat >"$unclaimed_corpus/orphan.lua" <<'LUA'
-- present on disk, never mentioned in expected.tsv
LUA
printf '# case\tverdict\tcodes\tnote\n' >"$unclaimed_corpus/expected.tsv"
run unclaimed_corpus_file_fails 1 "$unclaimed_corpus" \
    "orphan.lua exists in the corpus but expected.tsv has no row for it"

# ============================================================================
# 15: the row-level src existence guard — expected.tsv names a case whose
# .lua file does not exist on disk. want_verdict is "diag" (a value a
# silently-empty $work/src would NOT produce: an absent cp source leaves the
# stub seeing zero files, i.e. "clean"), so this case only passes for the
# RIGHT reason.
# ============================================================================
ghost_corpus="$(newcorpus ghost-case)"
printf '# case\tverdict\tcodes\tnote\nghost\tdiag\tLB0300\tcontrol\n' \
    >"$ghost_corpus/expected.tsv"
run src_missing_for_named_case_fails 1 "$ghost_corpus" \
    "ghost: expected.tsv names it, but" \
    "does not exist"

# ============================================================================
# 16: luabox's own JSON-parse-failure handler — the stub emits a malformed
# response instead of a diagnostics array.
# ============================================================================
break_json_corpus="$(newcorpus break-json)"
cat >"$break_json_corpus/only_case.lua" <<'LUA'
-- STUB-LUABOX-BREAK-JSON
LUA
printf '# case\tverdict\tcodes\tnote\nonly_case\tclean\t-\tcontrol\n' \
    >"$break_json_corpus/expected.tsv"
run luabox_json_parse_failure_fails 1 "$break_json_corpus" \
    "could not parse 'luabox check --format json' output as JSON"

# ============================================================================
# 17: per-row state does not leak between cases — row 1 has a `.deps`
# dependency carrying a diagnostic marker; row 2 has NO deps and must come
# back clean. Without the cleanup, row 2 inherits row 1's dependency file
# still sitting in the shared $work/src and reports its diagnostic as its
# own.
# ============================================================================
leak_corpus="$(newcorpus stale-dep-leak)"
cat >"$leak_corpus/case_with_dep.lua" <<'LUA'
local h = require("helper")
LUA
cat >"$leak_corpus/helper.lua" <<'LUA'
-- STUB-LUABOX-DIAG: LB0306
LUA
printf 'helper.lua\n' >"$leak_corpus/case_with_dep.deps"
cat >"$leak_corpus/case_clean.lua" <<'LUA'
-- no deps of its own; must not see the previous row's dependency file
LUA
printf '# case\tverdict\tcodes\tnote\ncase_with_dep\tdiag\tLB0306\tluabox-only marker\ncase_clean\tclean\t-\tcontrol\n' \
    >"$leak_corpus/expected.tsv"
run stale_dep_file_cleared_between_cases 0 "$leak_corpus" \
    "match expected.tsv" \
    "!case_clean: luabox is 'diag', expected.tsv says 'clean'"

# ============================================================================
# 18-20: VERDICT_PRINT (regenerate) mode.
# ============================================================================

# 18: VERDICT_PRINT prints the MEASURED verdict, not a comparison — an
# expected.tsv that is flatly wrong must still exit 0 and print what was
# actually measured, proving print mode never compares.
print_corpus="$(newcorpus print-mode)"
cat >"$print_corpus/only_case.lua" <<'LUA'
-- STUB-LUABOX-DIAG: LB0300
LUA
printf '# case\tverdict\tcodes\tnote\nonly_case\tclean\t-\texisting note text\n' \
    >"$print_corpus/expected.tsv"
log="$work/print_mode_prints_measured_not_comparison.log"
LUABOX="$stub_luabox" VERDICT_CORPUS="$print_corpus" VERDICT_PRINT=1 \
    bash "$gate" >"$log" 2>&1
got=$?
assert print_mode_prints_measured_not_comparison 0 "$got" "$log" \
    "only_case	diag	LB0300	existing note text" \
    "!expected.tsv says"

# 19: VERDICT_PRINT skips a file that is only ever named by another case's
# .deps sidecar — it is a support module, not a case of its own.
print_deps_corpus="$(newcorpus print-mode-deps)"
cat >"$print_deps_corpus/carrier.lua" <<'LUA'
local h = require("helper")
LUA
cat >"$print_deps_corpus/helper.lua" <<'LUA'
-- STUB-LUABOX-DIAG: LB0306
LUA
printf 'helper.lua\n' >"$print_deps_corpus/carrier.deps"
printf '# case\tverdict\tcodes\tnote\ncarrier\tdiag\tLB0306\tcontrol\n' \
    >"$print_deps_corpus/expected.tsv"
log="$work/print_mode_skips_dep_only_files.log"
LUABOX="$stub_luabox" VERDICT_CORPUS="$print_deps_corpus" VERDICT_PRINT=1 \
    bash "$gate" >"$log" 2>&1
got=$?
assert print_mode_skips_dep_only_files 0 "$got" "$log" \
    "carrier	diag	LB0306	control" \
    "!helper	"

# 20: VERDICT_PRINT emits a TODO placeholder note for a case with no
# existing row at all — the reminder that a brand new row still needs a
# human sentence before it can be pasted into expected.tsv.
print_new_corpus="$(newcorpus print-mode-new)"
cat >"$print_new_corpus/brand_new_case.lua" <<'LUA'
-- nothing flagged
LUA
printf '# case\tverdict\tcodes\tnote\n' >"$print_new_corpus/expected.tsv"
log="$work/print_mode_todo_for_new_case.log"
LUABOX="$stub_luabox" VERDICT_CORPUS="$print_new_corpus" VERDICT_PRINT=1 \
    bash "$gate" >"$log" 2>&1
got=$?
assert print_mode_todo_for_new_case 0 "$got" "$log" \
    "brand_new_case	clean	-	TODO: brand_new_case"

# ============================================================================
# 21-24: M31 — luabox's own exit status is now inspected, not ignored.
# ============================================================================

# 21: the exact bug reported — a binary that fails outright but prints an
# empty diagnostics array (`[]`) used to be recorded as a clean, PASSING
# verdict. `-- STUB-LUABOX-EXIT: 2` forces the stub to exit nonzero while
# still reporting zero codes (no `-- STUB-LUABOX-DIAG` marker at all).
exit_empty_corpus="$(newcorpus exit-nonzero-empty)"
cat >"$exit_empty_corpus/only_case.lua" <<'LUA'
-- STUB-LUABOX-EXIT: 2
LUA
printf '# case\tverdict\tcodes\tnote\nonly_case\tclean\t-\tcontrol\n' \
    >"$exit_empty_corpus/expected.tsv"
run luabox_nonzero_exit_with_empty_json_fails 1 "$exit_empty_corpus" \
    "luabox exited 2 but printed an empty diagnostics array"

# 22: a hang gets the same treatment via `timeout`, but reported as a
# timeout specifically (rc=124), not a generic nonzero exit — a hang and a
# crash want different next steps. Uses its own stub (sleeps past
# VERDICT_CASE_TIMEOUT) rather than the shared marker-driven one.
hang_stub="$work/bin/hang-luabox"
cat >"$hang_stub" <<'STUB'
#!/bin/bash
if [ "${1:-}" != "check" ]; then
    echo "hang-stub: unsupported invocation: $*" >&2
    exit 2
fi
sleep 5
printf '[]\n'
exit 0
STUB
chmod +x "$hang_stub"
hang_corpus="$(newcorpus hang)"
cat >"$hang_corpus/only_case.lua" <<'LUA'
-- would be clean, if the stub ever returned
LUA
printf '# case\tverdict\tcodes\tnote\nonly_case\tclean\t-\tcontrol\n' \
    >"$hang_corpus/expected.tsv"
log="$work/luabox_timeout_fails.log"
LUABOX="$hang_stub" VERDICT_CORPUS="$hang_corpus" VERDICT_CASE_TIMEOUT=1 \
    bash "$gate" >"$log" 2>&1
got=$?
assert luabox_timeout_fails 1 "$got" "$log" \
    "luabox timed out after 1s (VERDICT_CASE_TIMEOUT)"

# 23: (M31's second half) a nonzero exit code that is NOT the stub's
# default 1, paired with REAL diagnostics that match expected.tsv, must
# still PASS — the fix must not overfit to "exit must be exactly 1 for a
# diag row"; any nonzero exit alongside genuine diagnostics is legitimate
# (luabox's own non-strict/warn mode can do exactly this).
exit_diag_corpus="$(newcorpus exit-nonzero-with-diag)"
cat >"$exit_diag_corpus/only_case.lua" <<'LUA'
-- STUB-LUABOX-DIAG: LB0300
-- STUB-LUABOX-EXIT: 3
LUA
printf '# case\tverdict\tcodes\tnote\nonly_case\tdiag\tLB0300\tcontrol\n' \
    >"$exit_diag_corpus/expected.tsv"
run luabox_nonzero_exit_with_diagnostics_still_matches 0 "$exit_diag_corpus" \
    "match expected.tsv"

# 24: the expected clean/zero-exit case — no markers at all, default stub
# behaviour (exit 0, empty codes). Control for 21-23: proves the new exit-
# status check does not itself misfire on the ordinary, correct case.
exit_zero_clean_corpus="$(newcorpus exit-zero-clean)"
cat >"$exit_zero_clean_corpus/only_case.lua" <<'LUA'
-- nothing flagged, default exit 0
LUA
printf '# case\tverdict\tcodes\tnote\nonly_case\tclean\t-\tcontrol\n' \
    >"$exit_zero_clean_corpus/expected.tsv"
run luabox_zero_exit_clean_control_passes 0 "$exit_zero_clean_corpus" \
    "match expected.tsv"

# ============================================================================
# 25-26: M60/M33 — the per-case manifest is now assertable, not silently
# droppable, and a case can override it.
# ============================================================================

# 25: the DEFAULT manifest's `strict = true` line survives into what the
# stub actually saw. Before M60, this line was written by an unprotected
# printf nothing ever read back — deleting it left every case in this file
# green. The stub now dumps luabox.toml verbatim; assert the line is really
# in there.
default_manifest_corpus="$(newcorpus default-manifest)"
cat >"$default_manifest_corpus/only_case.lua" <<'LUA'
-- STUB-LUABOX-DIAG: LB0300
LUA
printf '# case\tverdict\tcodes\tnote\nonly_case\tdiag\tLB0300\tcontrol\n' \
    >"$default_manifest_corpus/expected.tsv"
run manifest_strict_true_survives_in_default_manifest 0 "$default_manifest_corpus" \
    "strict = true"

# 26: a case shipping its own `<case>.toml` sidecar gets THAT manifest
# instead of the default — M33's escape hatch. Content deliberately omits
# "strict = true" so the two assertions (present / absent) can only both
# pass if the override, not the default, was actually written.
override_corpus="$(newcorpus manifest-override)"
cat >"$override_corpus/only_case.lua" <<'LUA'
-- verdict is irrelevant to this assertion; only the manifest dump is
LUA
printf 'strict = false  # override marker: the default manifest string is absent here\n' \
    >"$override_corpus/only_case.toml"
printf '# case\tverdict\tcodes\tnote\nonly_case\tclean\t-\tcontrol\n' \
    >"$override_corpus/expected.tsv"
run manifest_override_sidecar_is_honored 0 "$override_corpus" \
    "strict = false" \
    "!strict = true"

# ============================================================================
# 27: M33 — the `.defs` sidecar actually copies its file(s) into $work/defs,
# where a real `[types] defs` manifest entry would resolve them from. The
# stub scans defs/*.lua the same way it scans src/*.lua (see the stub's own
# header), so a diagnostic marker only surfacing correctly here proves the
# file made it onto disk under the right name, not merely that the driver
# didn't crash.
# ============================================================================
defs_corpus="$(newcorpus defs-sidecar)"
cat >"$defs_corpus/carrier.lua" <<'LUA'
-- carrier itself flags nothing; the diagnostic can only come from the
-- ambient defs file named by carrier.defs
LUA
cat >"$defs_corpus/ambient.d.lua" <<'LUA'
-- STUB-LUABOX-DIAG: LB0305
LUA
printf 'ambient.d.lua\n' >"$defs_corpus/carrier.defs"
printf '# case\tverdict\tcodes\tnote\ncarrier\tdiag\tLB0305\tcontrol\n' \
    >"$defs_corpus/expected.tsv"
run defs_sidecar_copies_ambient_file 0 "$defs_corpus" \
    "match expected.tsv" \
    "!luabox codes are"

# ============================================================================
# 28: M61 — a blank line inside (and trailing) a `.deps` sidecar is skipped,
# not treated as a dependency name (which `cp` would then fail on) nor as
# the end of the file. Two real entries with a blank line BETWEEN them plus
# one AFTER: both dependencies' diagnostics must still surface.
# ============================================================================
deps_blank_line_corpus="$(newcorpus deps-blank-line)"
cat >"$deps_blank_line_corpus/dep_one.lua" <<'LUA'
-- STUB-LUABOX-DIAG: LB0300
LUA
cat >"$deps_blank_line_corpus/dep_two.lua" <<'LUA'
-- STUB-LUABOX-DIAG: LB0306
LUA
cat >"$deps_blank_line_corpus/carrier.lua" <<'LUA'
-- carrier itself is clean; both diagnostics come from its dependencies
LUA
printf 'dep_one.lua\n\ndep_two.lua\n\n' >"$deps_blank_line_corpus/carrier.deps"
printf '# case\tverdict\tcodes\tnote\ncarrier\tdiag\tLB0300,LB0306\tblank lines in .deps (between entries and trailing) are skipped, not fatal\n' \
    >"$deps_blank_line_corpus/expected.tsv"
run deps_blank_line_is_skipped 0 "$deps_blank_line_corpus" \
    "match expected.tsv" \
    "!luabox codes are"

selftest_report verdict-differential-selftest

#!/bin/bash
# Self-test for the luals parity differential (#57) — the gate on the gate.
#
# Why. luals-differential.sh exists to catch a parity claim that cannot
# fail, and round 3 review found it had exactly that shape in eight places:
# two columns silently measuring different file sets (F17), an ignored `cp`
# failure that let four rows verify nothing (F18), a "not clean" check that
# cannot tell an intended LB0306 from a regressed LB0300 (F19), a SKIP path
# with no way to make it required (F23), an undeclared python3 dependency
# whose failure mode named the wrong cause (F24), three tool-failure shapes
# collapsed into one indistinguishable message (F26), an unvalidated `.deps`
# entry that could copy outside the corpus (F27), and a "justified
# divergence" rule that was prose, not code (F28) — and every one of them
# was invisible because this file did not exist. This file is audited the
# same way mutants-gate-selftest.sh audits its own gate: every case below
# was proven, by deleting or neutering the line(s) in luals-differential.sh
# it names and re-running this file, to fail when that line is gone.
#
# luabox and lua-language-server are both STUBBED — content-driven scripts
# that read `-- STUB-LUABOX-DIAG: <CODE>` / `-- STUB-LUALS-DIAG: <code>`
# marker comments out of the fixture .lua files and report exactly those, so
# a fixture pins an exact, known code set without a real typecheck or a real
# multi-second luals startup. What is under test is the DRIVER's judgement,
# not either tool — same division of labour as the sibling file's cargo-mutants
# stub. Each fixture corpus is a few files under $work, pointed at via
# LUALS_CORPUS (added to luals-differential.sh for exactly this reason).
#
# Every case asserts an exit code AND at least one discriminating string
# that only the code path under test can produce — several also assert a
# string must be ABSENT (`!needle`), because a second, unrelated failure can
# supply the same "expected" exit code or substring a deleted line was
# supposed to produce. Run it in seconds:
#
#   bash scripts/tests/luals-differential-selftest.sh
set -u

here="$(cd "$(dirname "$0")" && pwd)"
gate="$here/luals-differential.sh"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# Round 6 review M49: the pass/fail counters, the exit-plus-`!needle`
# assertion and M35's "measured nothing" guard live in one place now.
# shellcheck source=scripts/tests/selftest-lib.sh
. "$here/selftest-lib.sh"

# --- stub tools ------------------------------------------------------------
mkdir -p "$work/bin"

stub_luabox="$work/bin/luabox"
cat >"$stub_luabox" <<'STUB'
#!/bin/bash
# stub luabox: understands only `check --format json`, invoked from inside
# the differential script's per-case temp project. Reports exactly the
# codes named by a `-- STUB-LUABOX-DIAG: <CODE>` marker comment on its own
# line in any *.lua file under ./src (case file or copied dep) — the same
# file set the real luabox column typechecks as one project.
#
# A `-- STUB-LUABOX-BREAK-JSON` marker instead makes the stub emit output
# that is not valid JSON at all — standing in for a real tool bug (a panic
# mid-write, a truncated pipe) rather than a well-formed "0 diagnostics" or
# "N diagnostics" answer. This exists to drive
# luals-differential.sh:311-318, the `codes_rc != 0` handler.
if [ "${1:-}" != "check" ]; then
    echo "stub-luabox: unsupported invocation: $*" >&2
    exit 2
fi
codes=()
break_json=0
shopt -s nullglob
for f in src/*.lua; do
    if grep -qF -- '-- STUB-LUABOX-BREAK-JSON' "$f"; then
        break_json=1
    fi
    while IFS= read -r code; do
        codes+=("$code")
    done < <(grep -oE -- '-- STUB-LUABOX-DIAG: [A-Za-z0-9]+' "$f" | awk '{print $3}')
done
shopt -u nullglob
if [ "$break_json" = 1 ]; then
    printf 'not-json: stub-luabox produced a malformed response\n'
    echo "check: stub forced a malformed response" >&2
    exit 0
fi
if [ "${#codes[@]}" -eq 0 ]; then
    printf '[]\n'
    echo "check: 0 errors, 0 warnings in 1 files" >&2
    exit 0
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
exit 1
STUB
chmod +x "$stub_luabox"

stub_luals="$work/bin/lua-language-server"
cat >"$stub_luals" <<'STUB'
#!/bin/bash
# stub lua-language-server: understands only `--check <dir> --checklevel=X
# --check_out_path=<path>`. Writes a check.json exactly like a real --check
# run would: keyed by file:// URI, one array per flagged file, and — because
# a real 0-problem run writes nothing, which luals-differential.sh's
# `[ -f "$luals_json" ]` depends on — nothing at all when nothing is
# flagged. Flags a file by scanning it for `-- STUB-LUALS-DIAG: <code>`.
# STUB_LUALS_FAIL_EXIT (+ STUB_LUALS_FAIL_MSG) forces a tool failure instead,
# for the "three failure modes, one message" case (F26).
dir=""
out_path=""
prev=""
for arg in "$@"; do
    case "$prev" in
    --check) dir="$arg" ;;
    esac
    case "$arg" in
    --check_out_path=*) out_path="${arg#--check_out_path=}" ;;
    esac
    prev="$arg"
done
if [ -n "${STUB_LUALS_FAIL_EXIT:-}" ]; then
    echo "${STUB_LUALS_FAIL_MSG:-stub-luals: forced failure}" >&2
    exit "$STUB_LUALS_FAIL_EXIT"
fi
json="{"
sep=""
found=0
shopt -s nullglob
for f in "$dir"/*.lua; do
    hits=""
    hsep=""
    while IFS= read -r code; do
        hits="$hits$hsep{\"code\":\"$code\",\"message\":\"stub\",\"severity\":2}"
        hsep=","
        found=1
    done < <(grep -oE -- '-- STUB-LUALS-DIAG: [a-z0-9-]+' "$f" | awk '{print $3}')
    if [ -n "$hits" ]; then
        abs="$(cd "$(dirname "$f")" && pwd)/$(basename "$f")"
        json="$json${sep}\"file://$abs\":[$hits]"
        sep=","
    fi
done
shopt -u nullglob
json="$json}"
if [ "$found" = 1 ]; then
    printf '%s' "$json" >"$out_path"
fi
exit 0
STUB
chmod +x "$stub_luals"

# --- helpers -----------------------------------------------------------
newcorpus() {
    local d="$work/corpus-$1"
    mkdir -p "$d"
    printf '%s' "$d"
}

# assert <name> <want-exit> <got-exit> <log> <needle>...
# This file's local name for selftest-lib.sh's `assert_exit`, kept so the
# call sites below read as they always have.
assert() { assert_exit "$@"; }

# run <name> <want-exit> <corpus-dir> <needle>...
# Standard invocation: stub luabox + stub luals, real (missing) LUALS_REQUIRED.
run() {
    local name="$1" want_exit="$2" corpus_dir="$3"
    shift 3
    local log="$work/$name.log"
    LUABOX="$stub_luabox" LUALS="$stub_luals" LUALS_CORPUS="$corpus_dir" \
        bash "$gate" >"$log" 2>&1
    local got=$?
    assert "$name" "$want_exit" "$got" "$log" "$@"
}

# ============================================================================
# 1-2: the SKIP branch (F23) — loud, and does not stop the luabox column.
# ============================================================================

skip_corpus="$(newcorpus skip)"
cat >"$skip_corpus/only_case.lua" <<'LUA'
-- STUB-LUABOX-DIAG: LB0300
LUA
printf '# case\tluabox\tcodes\tluals\tnote\nonly_case\tdiag\tLB0300\tdiag\tcontrol\n' \
    >"$skip_corpus/expected.tsv"

log="$work/skip_prints_loudly.log"
LUABOX="$stub_luabox" LUALS="lua-language-server-does-not-exist-selftest" \
    LUALS_CORPUS="$skip_corpus" bash "$gate" >"$log" 2>&1
got=$?
assert skip_prints_loudly 0 "$got" "$log" \
    "SKIP  luals column: no lua-language-server-does-not-exist-selftest on PATH" \
    "luals column SKIPPED"

# Same SKIP condition, but the fixture's expected codes are WRONG — proving
# the luabox column still actually re-derives and compares, rather than the
# whole per-row body having become conditional on luals being present too.
skip_codes_corpus="$(newcorpus skip-codes)"
cat >"$skip_codes_corpus/only_case.lua" <<'LUA'
-- STUB-LUABOX-DIAG: LB0300
LUA
printf '# case\tluabox\tcodes\tluals\tnote\nonly_case\tdiag\tLB9999\tdiag\tcontrol\n' \
    >"$skip_codes_corpus/expected.tsv"

log="$work/luabox_enforced_during_skip.log"
LUABOX="$stub_luabox" LUALS="lua-language-server-does-not-exist-selftest" \
    LUALS_CORPUS="$skip_codes_corpus" bash "$gate" >"$log" 2>&1
got=$?
assert luabox_enforced_during_skip 1 "$got" "$log" \
    "SKIP  luals column: no" \
    "luabox codes are 'LB0300', expected.tsv says 'LB9999'"

# ============================================================================
# 3: LUALS_REQUIRED=1 turns a missing luals into a hard failure (F23).
# ============================================================================

log="$work/luals_required_missing_fails.log"
LUABOX="$stub_luabox" LUALS="lua-language-server-does-not-exist-selftest" \
    LUALS_REQUIRED=1 LUALS_CORPUS="$skip_corpus" bash "$gate" >"$log" 2>&1
got=$?
assert luals_required_missing_fails 1 "$got" "$log" \
    "LUALS_REQUIRED=1 and no lua-language-server-does-not-exist-selftest on PATH" \
    "!SKIP  luals column"

# ============================================================================
# 4: a relative LUABOX resolves against the CALLER's cwd, not the per-case
# temp project cwd the gate later `cd`s into (round-2 fix for #57).
# ============================================================================

rel_corpus="$(newcorpus relative-luabox)"
cat >"$rel_corpus/only_case.lua" <<'LUA'
-- STUB-LUABOX-DIAG: LB0300
LUA
printf '# case\tluabox\tcodes\tluals\tnote\nonly_case\tdiag\tLB0300\tclean\tcontrol\n' \
    >"$rel_corpus/expected.tsv"

rel_wd="$work/rel-cwd"
mkdir -p "$rel_wd"
cp "$stub_luabox" "$rel_wd/rel-stub-luabox"
log="$work/relative_luabox_resolved_against_cwd.log"
(
    cd "$rel_wd" && LUABOX="./rel-stub-luabox" LUALS="$stub_luals" \
        LUALS_CORPUS="$rel_corpus" bash "$gate" >"$log" 2>&1
)
got=$?
assert relative_luabox_resolved_against_cwd 0 "$got" "$log" \
    "both columns match" \
    "!could not parse 'luabox check --format json' output as JSON"

# ============================================================================
# 5-7: `.deps` handling (F17, F18, F27).
# ============================================================================

# 5: a case whose .deps sidecar names a file that is not in the corpus.
deps_missing_corpus="$(newcorpus deps-missing)"
cat >"$deps_missing_corpus/carrier.lua" <<'LUA'
-- nothing to flag on its own
LUA
printf 'nonexistent_module.lua\n' >"$deps_missing_corpus/carrier.deps"
printf '# case\tluabox\tcodes\tluals\tnote\ncarrier\tclean\t-\tclean\tcontrol\n' \
    >"$deps_missing_corpus/expected.tsv"
run deps_copy_failure_fails 1 "$deps_missing_corpus" \
    "could not copy dependency 'nonexistent_module.lua'"

# 6: a case whose .deps sidecar tries to escape the corpus directory.
deps_unsafe_corpus="$(newcorpus deps-unsafe)"
cat >"$deps_unsafe_corpus/carrier.lua" <<'LUA'
-- nothing to flag on its own
LUA
printf '../../../etc/hostname\n' >"$deps_unsafe_corpus/carrier.deps"
printf '# case\tluabox\tcodes\tluals\tnote\ncarrier\tclean\t-\tclean\tcontrol\n' \
    >"$deps_unsafe_corpus/expected.tsv"
run deps_traversal_guard_rejects_unsafe_name 1 "$deps_unsafe_corpus" \
    "unsafe dependency name '../../../etc/hostname'" \
    "!could not copy dependency"

# 7: the .deps sidecar's one line has no trailing newline — a real file
# dropped off the read loop's last iteration without `|| [ -n "$dep" ]`.
deps_noeol_corpus="$(newcorpus deps-noeol)"
cat >"$deps_noeol_corpus/helper.lua" <<'LUA'
-- STUB-LUABOX-DIAG: LB0306
LUA
cat >"$deps_noeol_corpus/carrier.lua" <<'LUA'
-- carrier itself is clean; the diagnostic can only come from helper.lua
LUA
printf 'helper.lua' >"$deps_noeol_corpus/carrier.deps"  # deliberately no \n
printf '# case\tluabox\tcodes\tluals\tnote\ncarrier\tdiag\tLB0306\tclean\tthe dep is copied even without a trailing newline\n' \
    >"$deps_noeol_corpus/expected.tsv"
run deps_final_line_without_newline_is_copied 0 "$deps_noeol_corpus" \
    "carrier                                  diag       LB0306"

# ============================================================================
# 8: the exact diagnostic CODE SET is asserted, not just clean/diag (F19).
# ============================================================================

codes_corpus="$(newcorpus codes-mismatch)"
cat >"$codes_corpus/only_case.lua" <<'LUA'
-- STUB-LUABOX-DIAG: LB0300
LUA
# Both readings are "diag" — only the code differs, so a bare clean/diag
# check (the pre-#57-fix state) would pass this and hide the mismatch.
printf '# case\tluabox\tcodes\tluals\tnote\nonly_case\tdiag\tLB0301\tclean\tcontrol\n' \
    >"$codes_corpus/expected.tsv"
run codes_mismatch_fails 1 "$codes_corpus" \
    "luabox codes are 'LB0300', expected.tsv says 'LB0301'" \
    "!luabox is 'clean'"

# ============================================================================
# 9: a DIVERGE row (luabox != luals) with no note is refused (F28).
# ============================================================================

note_corpus="$(newcorpus empty-note)"
cat >"$note_corpus/only_case.lua" <<'LUA'
-- nothing flagged by either stub
LUA
printf '# case\tluabox\tcodes\tluals\tnote\nonly_case\tclean\t-\tdiag\t\n' \
    >"$note_corpus/expected.tsv"
run empty_note_on_diverging_row_fails 1 "$note_corpus" \
    "luabox (clean) and luals (diag) disagree but the note column is empty"

# ============================================================================
# 10-11: class names unique per FILE, not merely per declaration (F28).
# ============================================================================

dup_class_corpus="$(newcorpus dup-class)"
cat >"$dup_class_corpus/a.lua" <<'LUA'
---@class Shared
---@field x number
LUA
cat >"$dup_class_corpus/b.lua" <<'LUA'
---@class Shared
---@field y string
LUA
printf '# case\tluabox\tcodes\tluals\tnote\na\tclean\t-\tclean\tcontrol\nb\tclean\t-\tclean\tcontrol\n' \
    >"$dup_class_corpus/expected.tsv"
run duplicate_class_name_across_files_fails 1 "$dup_class_corpus" \
    "class 'Shared' is declared in more than one corpus file (a.lua,b.lua)"

# The intra-file shape (the SAME class declared twice in the SAME file) is
# the intentional merge fixtures' whole point (duplicate_class_union in the
# real corpus) and must NOT trip the same guard.
same_file_corpus="$(newcorpus same-file-dup)"
cat >"$same_file_corpus/merged.lua" <<'LUA'
---@class Merged
---@field x number

---@class Merged
---@field y string
LUA
printf '# case\tluabox\tcodes\tluals\tnote\nmerged\tclean\t-\tclean\tunion of two declarations, one file\n' \
    >"$same_file_corpus/expected.tsv"
run same_file_duplicate_class_is_not_a_conflict 0 "$same_file_corpus" \
    "both columns match" \
    "!is declared in more than one corpus file"

# ============================================================================
# 12: luals is judged over the SAME file set luabox typechecks — a case's
# own file is clean, but its .deps dependency is what luals flags (F17).
# ============================================================================

dep_union_corpus="$(newcorpus dep-union)"
cat >"$dep_union_corpus/generic_mod.lua" <<'LUA'
-- luals-only hit: a shape luals cannot parse, in the DEPENDENCY, not the case
-- STUB-LUALS-DIAG: undefined-doc-name
LUA
cat >"$dep_union_corpus/consumer.lua" <<'LUA'
-- the case file itself carries no marker of its own
local m = require("generic_mod")
LUA
printf 'generic_mod.lua\n' >"$dep_union_corpus/consumer.deps"
printf '# case\tluabox\tcodes\tluals\tnote\nconsumer\tclean\t-\tdiag\tluals flags the dependency, not the case file\n' \
    >"$dep_union_corpus/expected.tsv"
run luals_flags_via_dep_file 0 "$dep_union_corpus" \
    "consumer                                 clean      -                        diag"

# ============================================================================
# 13: python3 is a declared, checked dependency (F24).
# ============================================================================

# A curated PATH with every tool the gate needs EXCEPT python3 (and no
# `.cargo`/python component at all, not merely a reordering that still finds
# the real one further down).
scrubbed="$work/bin-no-python"
mkdir -p "$scrubbed"
for b in bash grep sed awk cp rm mkdir cat basename dirname mktemp printf \
    sort uniq paste tr command true false head; do
    p="$(command -v "$b" 2>/dev/null)"
    [ -n "$p" ] && ln -sf "$p" "$scrubbed/$b"
done
log="$work/python3_absent_fails_loudly.log"
PATH="$scrubbed" LUABOX="$stub_luabox" LUALS="$stub_luals" \
    LUALS_CORPUS="$skip_corpus" bash "$gate" >"$log" 2>&1
got=$?
assert python3_absent_fails_loudly 1 "$got" "$log" \
    "python3 not found on PATH — required to parse"

# ============================================================================
# 14: a luals tool failure shows its REAL output, not one indistinguishable
# line (F26) — three different underlying causes must not read alike.
# ============================================================================

log="$work/luals_tool_failure_shows_real_output.log"
LUABOX="$stub_luabox" LUALS="$stub_luals" LUALS_CORPUS="$skip_corpus" \
    STUB_LUALS_FAIL_EXIT=3 \
    STUB_LUALS_FAIL_MSG="stub: workspace load failed: permission denied on /fixture/root" \
    bash "$gate" >"$log" 2>&1
got=$?
assert luals_tool_failure_shows_real_output 1 "$got" "$log" \
    "lua-language-server --check failed to run (exit 3)" \
    "workspace load failed: permission denied on /fixture/root"

# ============================================================================
# round-4 (R24): the half of the gate F25/round-3 left unasserted — the
# actual COMPARISON logic, the two data-integrity sweeps that run outside the
# per-row loop, and the per-invocation state that must not leak between rows.
# Every case below was proven the same way as 1-14: delete the exact line(s)
# it names from luals-differential.sh and re-run this file — PASS must
# become FAIL.
# ============================================================================

# 15: the luals-column comparison itself (:340-348). luabox and luals both
# read "clean" in expected.tsv (so the divergence-note rule at :251 never
# fires), but the stub luals tool actually flags the case — the comparison
# block is what is supposed to catch that and fail the row.
luals_wrong_expectation_corpus="$(newcorpus luals-wrong-expectation)"
cat >"$luals_wrong_expectation_corpus/only_case.lua" <<'LUA'
-- luabox stub reports nothing; luals stub does — expected.tsv wrongly
-- claims luals is clean too.
-- STUB-LUALS-DIAG: undefined-field
LUA
printf '# case\tluabox\tcodes\tluals\tnote\nonly_case\tclean\t-\tclean\tcontrol\n' \
    >"$luals_wrong_expectation_corpus/expected.tsv"
run luals_column_mismatch_fails 1 "$luals_wrong_expectation_corpus" \
    "luals is 'diag', expected.tsv says 'clean'"

# 16: the unclaimed-corpus sweep (:357-368) — a *.lua file with no row in
# expected.tsv at all (and not named by any .deps sidecar) must fail, not
# pass by omission.
unclaimed_corpus="$(newcorpus unclaimed)"
cat >"$unclaimed_corpus/orphan.lua" <<'LUA'
-- present on disk, never mentioned in expected.tsv
LUA
printf '# case\tluabox\tcodes\tluals\tnote\n' >"$unclaimed_corpus/expected.tsv"
run unclaimed_corpus_file_fails 1 "$unclaimed_corpus" \
    "orphan.lua exists in the corpus but expected.tsv has no row for it"

# 17: the row-level src existence guard (:243-247) — expected.tsv names a
# case whose .lua file does not exist on disk. want_luabox is set to a value
# ("diag") that a silently-empty $work/src would NOT produce (an absent cp
# source leaves the stub luabox seeing zero files, i.e. "clean"), so this
# case only passes for the RIGHT reason: the specific "does not exist"
# message, not an incidental code-mismatch on the fallback path.
ghost_corpus="$(newcorpus ghost-case)"
printf '# case\tluabox\tcodes\tluals\tnote\nghost\tdiag\tLB0300\tdiag\tcontrol\n' \
    >"$ghost_corpus/expected.tsv"
run src_missing_for_named_case_fails 1 "$ghost_corpus" \
    "ghost: expected.tsv names it, but" \
    "does not exist"

# 18: luabox's own JSON-parse-failure handler (:311-318) — the stub tool
# emits a malformed response instead of a diagnostics array.
break_json_corpus="$(newcorpus break-json)"
cat >"$break_json_corpus/only_case.lua" <<'LUA'
-- STUB-LUABOX-BREAK-JSON
LUA
printf '# case\tluabox\tcodes\tluals\tnote\nonly_case\tclean\t-\tclean\tcontrol\n' \
    >"$break_json_corpus/expected.tsv"
run luabox_json_parse_failure_fails 1 "$break_json_corpus" \
    "could not parse 'luabox check --format json' output as JSON"

# 19: the parity-code positive filter (:176 sets the code universe, :188
# applies it) — luals flags a REAL diagnostic code that is simply not in
# $parity_codes (style/unused-local territory). It must not count toward the
# case's diag verdict; expected.tsv says clean and must stay clean.
non_parity_corpus="$(newcorpus non-parity-code)"
cat >"$non_parity_corpus/only_case.lua" <<'LUA'
-- luals reports a real but out-of-scope code; the parity filter must drop it
-- STUB-LUALS-DIAG: unused-local
LUA
printf '# case\tluabox\tcodes\tluals\tnote\nonly_case\tclean\t-\tclean\ta non-parity-set luals code must not flip the verdict\n' \
    >"$non_parity_corpus/expected.tsv"
run non_parity_luals_code_does_not_count 0 "$non_parity_corpus" \
    "both columns match" \
    "!only_case                                clean      -                        diag"

# 20: per-row state does not leak (:257's `rm -f "$work/src"/*.lua`) — row 1
# has a `.deps` dependency carrying a diagnostic marker; row 2 has NO deps
# and must come back clean. Without the cleanup, row 2 inherits row 1's
# dependency file still sitting in the shared $work/src and reports its
# diagnostic as its own.
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
printf '# case\tluabox\tcodes\tluals\tnote\ncase_with_dep\tdiag\tLB0306\tclean\tluabox-only marker, luals stub does not mirror it\ncase_clean\tclean\t-\tclean\tcontrol\n' \
    >"$leak_corpus/expected.tsv"
run stale_dep_file_cleared_between_cases 0 "$leak_corpus" \
    "both columns match" \
    "!case_clean: luabox is 'diag', expected.tsv says 'clean'"

# 21: the luabox binary existence guard (:86-89) — fires before any corpus
# or expected.tsv is even read.
log="$work/luabox_binary_missing_fails.log"
LUABOX="$work/bin/does-not-exist-luabox" LUALS="$stub_luals" \
    LUALS_CORPUS="$skip_corpus" bash "$gate" >"$log" 2>&1
got=$?
assert luabox_binary_missing_fails 1 "$got" "$log" \
    "error: no luabox binary at"

# 22: the expected.tsv existence guard (:99-102).
no_expected_corpus="$(newcorpus no-expected-tsv)"
cat >"$no_expected_corpus/only_case.lua" <<'LUA'
-- no expected.tsv in this corpus directory at all
LUA
log="$work/expected_tsv_missing_fails.log"
LUABOX="$stub_luabox" LUALS="$stub_luals" LUALS_CORPUS="$no_expected_corpus" \
    bash "$gate" >"$log" 2>&1
got=$?
assert expected_tsv_missing_fails 1 "$got" "$log" \
    "error: no expectations at"

selftest_report luals-differential-selftest

#!/bin/bash
# Self-test for scripts/perf-gate-lib.sh, the pure-function seam
# scripts/perf-gate.sh's RETAINED-TYPEENV REGRESSION GATE leg (and its
# other corpus-manifest writers) are built on.
#
# Why this exists, and what it does NOT cover. Every other gate in this repo
# with real judgement in it (scripts/tests/mutants-gate.sh,
# scripts/tests/luals-differential.sh) has a self-test that stages outcomes
# and proves the judgement fails the right way; scripts/perf-gate.sh had
# none — "the leg's ability to fail is a manual claim in a comment" (#58
# review round 5, N41). A full self-test in that same shape would need a
# release build, a generated corpus and a real `luabox` invocation for every
# case, which is exactly the tens-of-seconds-per-case cost
# scripts/tests/mutants-gate-selftest.sh avoids by staging cargo-mutants'
# OUTPUT rather than running it — so this file applies the same split:
# perf-gate-lib.sh holds every piece of judgement that does not require a
# real binary (budget arithmetic, "was the corpus actually generated" and
# the manifest writer three call sites used to duplicate by hand), and this
# file drives THAT in milliseconds. It does not, and cannot, prove the
# timing/RSS budgets themselves are well-calibrated, or that
# scripts/perf-gate.sh's `cargo build` / corpus-generation / `luabox check`
# sequence still wires these functions together correctly end to end —
# only running scripts/perf-gate.sh for real proves that, the same way
# gate-selftest jobs elsewhere do not replace their full-audit siblings.
#
# Every case below was checked directly against perf-gate-lib.sh: the named
# line(s) removed or neutered, this file re-run, the failure confirmed, the
# line restored.
#
#   bash scripts/tests/perf-gate-selftest.sh
set -u

here="$(cd "$(dirname "$0")" && pwd)"
repo="$here/../.."
lib="$repo/scripts/perf-gate-lib.sh"

# shellcheck source=../perf-gate-lib.sh
source "$lib"

pass=0
fail=0

check() {
    local name="$1" got="$2" want="$3"
    if [ "$got" = "$want" ]; then
        echo "PASS  $name"
        pass=$((pass + 1))
    else
        echo "FAIL  $name: got [$got], want [$want]" >&2
        fail=$((fail + 1))
    fi
}

check_true() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "PASS  $name"
        pass=$((pass + 1))
    else
        echo "FAIL  $name: expected success, command failed" >&2
        fail=$((fail + 1))
    fi
}

check_false() {
    local name="$1"
    shift
    if ! "$@" >/dev/null 2>&1; then
        echo "PASS  $name"
        pass=$((pass + 1))
    else
        echo "FAIL  $name: expected failure, command succeeded" >&2
        fail=$((fail + 1))
    fi
}

# --- scale_budget_ms --------------------------------------------------------
# The float multiply every timed leg's budget goes through — pinned at the
# exact factor values this repo actually uses: 1.0 (default), the CI
# multiplier (4.0, ci.yml's perf-gates job), and this box's own multiplier
# (5.0, needed here because this machine is CPU-starved for the pre-existing
# timing legs — see scripts/perf-gate.sh's Env: block).
check "scale_budget_ms default factor" "$(scale_budget_ms 1000 1.0)" "1000"
check "scale_budget_ms CI factor (4.0)" "$(scale_budget_ms 1000 4.0)" "4000"
check "scale_budget_ms this box's factor (5.0)" "$(scale_budget_ms 4000 5.0)" "20000"
# The two cases above (1.5 * 50 = 75.0, 1.999 * 1000 = 1999.0) land on exact
# integers in double precision — truncation and round-to-nearest print the
# identical digit, so neither one actually pins "%d" (truncate) over "%.0f"
# (round-to-nearest); verified by swapping printf's format and watching all
# five cases stay green. A case that discriminates the two needs a product
# whose fractional part clears .5 by a comfortable margin (so IEEE-754
# imprecision near the boundary itself can't flip the result either way):
# 100 * 1.678 = 167.8 (%d -> 167, %.0f -> 168) and 1000 * 1.9999 = 1999.9
# (%d -> 1999, %.0f -> 2000) — both checked directly against the %.0f
# mutation described above and confirmed to fail under it.
check "scale_budget_ms truncates, does not round" "$(scale_budget_ms 100 1.678)" "167"
check "scale_budget_ms truncates a fractional result down" "$(scale_budget_ms 1000 1.9999)" "1999"

# --- assert_lua_file_count --------------------------------------------------
# N38's exact fix: the leg this backs (RETAINED-TYPEENV REGRESSION GATE)
# used to print "PASS ... 11 MiB < 100 MiB" against an empty corpus with
# nothing catching it — reproduced here directly rather than through a full
# perf-gate.sh run.
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

mkdir -p "$work/full"
for i in 1 2 3; do : >"$work/full/mod_$i.lua"; done
check_true "assert_lua_file_count passes on a matching corpus" \
    assert_lua_file_count "$work/full" 3

mkdir -p "$work/empty"
check_false "assert_lua_file_count fails on an empty corpus (N38's exact shape)" \
    assert_lua_file_count "$work/empty" 500

mkdir -p "$work/short"
for i in 1 2; do : >"$work/short/mod_$i.lua"; done
check_false "assert_lua_file_count fails on a truncated corpus" \
    assert_lua_file_count "$work/short" 3

# The error message must name what it found and what it expected — a
# self-test that only checks the exit code would pass just as happily if
# this collapsed to a bare `return 1` with no diagnostic at all.
msg="$(assert_lua_file_count "$work/short" 3 2>&1 >/dev/null)"
case "$msg" in
*"has 2 .lua file(s), expected 3"*) echo "PASS  assert_lua_file_count reports the actual and expected counts"; pass=$((pass + 1)) ;;
*)
    echo "FAIL  assert_lua_file_count reports the actual and expected counts: got [$msg]" >&2
    fail=$((fail + 1))
    ;;
esac

# Non-.lua files in the same directory must not be counted — a corpus
# generator that also drops its own luabox.toml (every one of them does)
# must not inflate the count and mask a truncated .lua run.
mkdir -p "$work/mixed"
for i in 1 2 3; do : >"$work/mixed/mod_$i.lua"; done
: >"$work/mixed/luabox.toml"
: >"$work/mixed/README.md"
check_true "assert_lua_file_count ignores non-.lua files in the same directory" \
    assert_lua_file_count "$work/mixed" 3

# --- write_perf_manifest ----------------------------------------------------
# N42: three call sites in perf-gate.sh (and two in perf-gate.ps1's
# New-PerfManifest) used to hand-copy this heredoc with one bit of drift
# possible per copy — the package name, and whether [types] is present.
strict_out="$work/strict.toml"
write_perf_manifest "$strict_out" "perf-gate-corpus" "true"
if grep -qF 'name = "perf-gate-corpus"' "$strict_out" \
    && grep -qF '[types]' "$strict_out" \
    && grep -qF 'strict = true' "$strict_out" \
    && tail -1 "$strict_out" | grep -qF '[dependencies]'; then
    echo "PASS  write_perf_manifest (strict) has the right name, [types] block and trailing [dependencies]"
    pass=$((pass + 1))
else
    echo "FAIL  write_perf_manifest (strict): unexpected content" >&2
    sed 's/^/        /' "$strict_out" >&2
    fail=$((fail + 1))
fi

lenient_out="$work/lenient.toml"
write_perf_manifest "$lenient_out" "perf-gate-diagnostics" "false"
if grep -qF 'name = "perf-gate-diagnostics"' "$lenient_out" \
    && ! grep -qF '[types]' "$lenient_out" \
    && ! grep -qF 'strict = true' "$lenient_out" \
    && tail -1 "$lenient_out" | grep -qF '[dependencies]'; then
    echo "PASS  write_perf_manifest (non-strict) omits [types] entirely"
    pass=$((pass + 1))
else
    echo "FAIL  write_perf_manifest (non-strict): unexpected content" >&2
    sed 's/^/        /' "$lenient_out" >&2
    fail=$((fail + 1))
fi

# The generated manifest must be parseable TOML, not merely visually
# plausible — catches a stray quote or an unescaped brace the string-based
# assertions above would not. `tomllib` is stdlib-only from Python 3.11;
# SKIP rather than FAIL below that, the same "a missing tool is not evidence
# of a regression" rule scripts/perf-gate.sh's own peak-RSS leg follows for
# a missing python3 entirely.
if python3 -c 'import tomllib' >/dev/null 2>&1; then
    if python3 - "$strict_out" <<'PYEOF'
import sys
import tomllib
with open(sys.argv[1], "rb") as f:
    data = tomllib.load(f)
assert data["package"]["name"] == "perf-gate-corpus"
assert data["types"]["strict"] is True
PYEOF
    then
        echo "PASS  write_perf_manifest (strict) output parses as valid TOML"
        pass=$((pass + 1))
    else
        echo "FAIL  write_perf_manifest (strict) output does not parse as valid TOML" >&2
        fail=$((fail + 1))
    fi
else
    echo "SKIP  write_perf_manifest (strict) TOML-validity check — no python3 tomllib (needs 3.11+)"
fi

# --- wiring: the lib is not just defined, it is USED ------------------------
# A self-test that only exercises perf-gate-lib.sh in isolation would not
# notice someone deleting the call site in perf-gate.sh itself while leaving
# the lib file behind — the exact "reported, not failed" gap this repo's
# other self-tests close with their own version of this check (e.g.
# mutants-gate-selftest.sh's cargo-mutants pin cross-check).
gate="$repo/scripts/perf-gate.sh"
if grep -qF 'source "$repo_root/scripts/perf-gate-lib.sh"' "$gate" \
    && grep -qF 'assert_lua_file_count "$retained_env_dir"' "$gate" \
    && grep -qF 'RAYON_NUM_THREADS=4' "$gate" \
    && grep -qF 'write_perf_manifest' "$gate"; then
    echo "PASS  perf-gate.sh sources and actually calls every perf-gate-lib.sh function this file tests"
    pass=$((pass + 1))
else
    echo "FAIL  perf-gate.sh does not wire in perf-gate-lib.sh the way this file assumes" >&2
    fail=$((fail + 1))
fi

echo
echo "perf-gate-selftest: $pass passed, $fail failed"
[ "$fail" -eq 0 ]

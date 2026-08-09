#!/usr/bin/env bash
# Pure helper functions shared by scripts/perf-gate.sh and its self-test
# (scripts/tests/perf-gate-selftest.sh). Kept free of `cargo build`, corpus
# generation and any real `luabox` invocation so the self-test can source
# this file directly and exercise the logic in milliseconds — the same
# split scripts/tests/mutants-gate.sh's awk classification pipeline and
# scripts/tests/luals-differential.sh's driver already rely on: the
# expensive step (a mutation run; a release build) is not what a self-test
# needs to prove wrong, the JUDGEMENT around it is (#58 review round 5,
# N41). Sourcing this file only defines functions — nothing here has a
# side effect on its own.
#
# `set -euo pipefail` is deliberately NOT set here: this file is sourced by
# a caller (perf-gate.sh) that already sets it, and a sourced file
# re-asserting shell options can surprise a caller relying on a narrower
# scope. Every function below fails closed (checks its own inputs) rather
# than leaning on the caller's `set -e`.

# scale_budget_ms <base_ms> <factor> [ceiling_ms] — LUABOX_PERF_FACTOR's
# float multiply, optionally capped.
#
# Plain `awk` (POSIX, present on every CI/dev box this repo targets) does the
# float arithmetic; everything downstream stays integer ms.
#
# The optional third argument closes a sensitivity hole the round 6 budget
# rebase opened (local merge-gate finding). `LUABOX_PERF_FACTOR` exists to
# absorb slow, shared CI hardware, so it multiplies the WHOLE budget — but a
# base budget already carries ~3x headroom over a measured baseline by this
# file's own convention, and multiplying headroom by headroom compounds.
# Concretely, for the `check (warm)` leg against its ~2.5 s measured
# baseline:
#
#   base   CI ceiling (x4)   sensitivity
#   1000              4000   1.6x   <- before the rebase
#   7500             30000    12x   <- after, unbounded by anything
#
# A 12x ceiling is not a gate. The cap keeps a leg's CI budget within a
# stated multiple of what it actually costs, so raising a base to stop a
# flaky local FAIL cannot silently blind CI as a side effect. A leg passing
# no ceiling is unchanged.
#
# "No ceiling" has TWO spellings, and both mean the same thing on both
# readers (#58 review round 8, F15a). An omitted/empty third argument is
# bash's natural one. A ceiling of 0 is the other, and it used to mean the
# OPPOSITE here: `[[ -n "0" ]]` is true, so 0 clamped every budget to 0 and
# failed every timed leg, while perf-gate-lib.ps1's ConvertTo-ScaledBudgetMs
# has documented 0 as its "no cap" DEFAULT since it was written — one shared
# perf-gate-budgets.env value, two readers, opposite behaviour, and no
# selftest case on the bash side to notice. `-gt 0` is the fix; a negative
# ceiling reads as "no cap" too, since no scaled budget can sit under one.
scale_budget_ms() {
  local scaled
  scaled="$(awk -v b="$1" -v f="$2" 'BEGIN { printf "%d", b * f }')"
  local ceiling="${3:-}"
  if [[ -n "$ceiling" && "$ceiling" -gt 0 && "$scaled" -gt "$ceiling" ]]; then
    scaled="$ceiling"
  fi
  printf '%s' "$scaled"
}

# read_perf_budgets <path> — validate scripts/perf-gate-budgets.env, then
# `source` it, so every KEY it defines lands as a shell variable in the
# caller's scope exactly as the bare `source` perf-gate.sh used to do.
#
# The validation is the point (#58 review round 8, F15c). perf-gate-lib.ps1's
# Read-PerfBudgets has always hard-rejected any line that is not
# `KEY=INTEGER`, on the stated grounds that "a budgets file that half-parses
# is not a fact about the budgets". The bash side had no such pass: `source`
# executes the file, so `CHECK_BUDGET_BASE_MS=75O0` (letter O) is a perfectly
# valid assignment that every downstream `[[ "$check_ms" -lt "$budget" ]]`
# then coerces to 0 — every timed leg FAILING with no explanation — and
# `RSS_BUDGET_MIB=$(rm -rf ~)` is a perfectly valid command substitution. The
# file is repo-controlled, so the second shape is robustness rather than a
# live attack path; the first is a plain typo away. One rule, both readers,
# same regex (`^[A-Z_][A-Z0-9_]*=-?[0-9]+$`), same fail-loudly verdict.
#
# Deliberately still `source` after validating, rather than parsing the
# KEY/VALUE pairs into variables by hand: the file's own header promises it
# is valid bash as-is, and a second, hand-rolled assignment mechanism here is
# one more thing that can disagree with the PowerShell reader.
read_perf_budgets() {
  local path="$1" line trimmed lineno=0
  if [[ ! -f "$path" ]]; then
    echo "error: read_perf_budgets: no budgets file at $path" >&2
    return 1
  fi
  while IFS= read -r line || [[ -n "$line" ]]; do
    lineno=$((lineno + 1))
    # Same two skips as Read-PerfBudgets: blank (after trimming) and comment.
    trimmed="${line#"${line%%[![:space:]]*}"}"
    trimmed="${trimmed%"${trimmed##*[![:space:]]}"}"
    [[ -z "$trimmed" || "$trimmed" == '#'* ]] && continue
    if [[ ! "$trimmed" =~ ^[A-Z_][A-Z0-9_]*=-?[0-9]+$ ]]; then
      echo "error: read_perf_budgets: malformed line ${lineno} in ${path}: ${line}" >&2
      echo "error:   every non-comment line must be KEY=INTEGER — no quoting, no spaces around '=';" >&2
      echo "error:   this file is sourced as shell, so anything else either sets a budget to a value" >&2
      echo "error:   the comparisons below coerce to 0 or runs as a command" >&2
      return 1
    fi
  done <"$path"
  # shellcheck disable=SC1090
  source "$path"
}

# assert_lua_file_count <dir> <expected> — N38: a peak-RSS (or wall-time)
# PASS printed against a corpus that was not actually generated the way the
# budget assumes — an empty directory, a truncated loop, a path a rename
# left stale — is not evidence of anything; it is the same "auditing
# NOTHING" gap scripts/tests/mutants-gate.sh's zero-mutants check and
# scripts/tests/luals-differential.sh's unclaimed-case check both close on
# their own inputs. Counts top-level *.lua files only (this repo's
# generated perf corpora are flat, one file per module) and fails loudly on
# a mismatch rather than silently measuring whatever is actually there.
assert_lua_file_count() {
  local dir="$1" expected="$2" got
  got="$(find "$dir" -maxdepth 1 -name '*.lua' 2>/dev/null | wc -l | tr -d ' ')"
  if [[ "$got" -ne "$expected" ]]; then
    echo "error: corpus at $dir has ${got} .lua file(s), expected ${expected} — the generation step" >&2
    echo "error:   did not run the way this leg's budget assumes; the measurement below would not" >&2
    echo "error:   be about the corpus its own PASS/FAIL line claims" >&2
    return 1
  fi
  return 0
}

# write_perf_manifest <path> <package-name> <strict:true|false> — every
# generated corpus in perf-gate.sh needs a luabox.toml differing only in
# `name` and whether `[types] strict = true` is present; three near-
# identical heredocs (N42) is exactly the kind of duplication that lets one
# copy drift from the others silently. One writer, one place to read the
# shape from.
write_perf_manifest() {
  local path="$1" name="$2" strict="$3"
  {
    printf '[package]\nname = "%s"\nversion = "0.0.0"\nedition = "5.4"\n\n' "$name"
    printf '[build]\ntarget = "5.4"\nout = "dist"\n\n'
    if [[ "$strict" == "true" ]]; then
      printf '[types]\nstrict = true\n\n'
    fi
    printf '[dependencies]\n'
  } >"$path"
}

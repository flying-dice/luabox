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

# scale_budget_ms <base_ms> <factor> — LUABOX_PERF_FACTOR's float multiply.
# Plain `awk` (POSIX, present on every CI/dev box this repo targets) does
# the float arithmetic; everything downstream stays integer ms.
scale_budget_ms() {
  awk -v b="$1" -v f="$2" 'BEGIN { printf "%d", b * f }'
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

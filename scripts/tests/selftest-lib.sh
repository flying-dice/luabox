#!/bin/bash
# Shared assertion harness for scripts/tests/*-selftest.sh (#58 review round
# 6, M49): the same ~42-line block — pass/fail counters, an exit-code-plus-
# needle assertion, a `!needle`-must-be-absent form — was hand-copied
# byte-for-byte into luals-differential-selftest.sh and
# verdict-differential-selftest.sh, re-inlined a third time in
# mutants-gate-selftest.sh's own run(), and perf-gate-selftest.sh invented a
# FIFTH, weaker vocabulary with no `!needle` support at all — one gate held
# to a lower bar than the three it was modelled on. One copy, sourced by
# every self-test in this directory, closes that.
#
# `set -u` is deliberately NOT set here, for the same reason
# scripts/perf-gate-lib.sh doesn't set `set -euo pipefail`: this file is
# sourced by a caller that already sets its own shell options, and a sourced
# file re-asserting them can surprise a caller relying on a narrower scope.
#
# Every function below is a judgement primitive with no side effect beyond
# incrementing $pass/$fail and printing PASS/FAIL — the corpus, the stub
# binaries and the gate invocation itself stay in each self-test file, where
# the domain knowledge of what is being tested actually lives.

pass=0
fail=0

# assert_exit <name> <want-exit> <got-exit> <log> [needle...]
# The exit-code-plus-needle check every self-test in this directory already
# hand-rolled under the name `assert` (luals-differential-selftest.sh,
# verdict-differential-selftest.sh) or inlined directly in `run()`
# (mutants-gate-selftest.sh). A needle prefixed with `!` must be ABSENT from
# the log rather than present — a second, unrelated failure path can supply
# the same "expected" string a deleted line was supposed to produce, and a
# bare needle can only prove presence, never that the RIGHT code path fired.
assert_exit() {
    local name="$1" want_exit="$2" got_exit="$3" log="$4"
    shift 4
    local ok=1 report=""
    [ "$got_exit" = "$want_exit" ] || {
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
        echo "PASS  $name (exit $got_exit)"
        pass=$((pass + 1))
    else
        echo "FAIL  $name: got exit $got_exit.$report" >&2
        sed 's/^/        /' "$log" >&2
        fail=$((fail + 1))
    fi
}

# check <name> <got> <want> — plain string-equality assertion, for pure
# functions with a return VALUE rather than an exit-code-and-log shape
# (perf-gate-lib.sh's scale_budget_ms and friends).
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

# check_true <name> <cmd...> — the command must exit 0.
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

# check_false <name> <cmd...> — the command must exit nonzero.
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

# selftest_report <suite-name> — the one line every self-test in this
# directory ends on. M35 (#58 review round 6): "$fail" -eq 0 alone reads a
# suite whose cases all silently vanished (a corpus/helper wiring break
# above this point, or every case erroring out before it ever calls
# assert_exit/check) as "0 passed, 0 failed" — a clean exit 0, the exact
# "cannot fail by measuring nothing" shape decisions/12 is titled after, one
# level up inside the self-test itself. verdict-differential-selftest.sh
# already carried this guard by hand; mutants-gate-selftest.sh's own
# "$fail" -eq 0 line did not (it happens to be unreachable in practice
# because that file's very first case always runs, but the guard should not
# depend on a file's case ORDER to be true) — both now route through here.
selftest_report() {
    local suite="$1"
    echo
    echo "$suite: $pass passed, $fail failed"
    [ "$fail" -eq 0 ] && [ "$pass" -gt 0 ]
}

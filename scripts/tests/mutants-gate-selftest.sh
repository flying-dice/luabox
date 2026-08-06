#!/bin/bash
# Self-test for the mutation gate (#58) — the gate on the gate.
#
# Why. mutants-gate.sh exists to catch fixtures that cannot fail, and its own
# failure mode is exactly that: every discriminator it computes is one
# forgotten `exit 1` away from being a decoration. Two rounds of review found
# precisely such gaps (a timeout run that printed OK; a zero-mutant run that
# printed "all reviewed"), and both were invisible to the suite because the
# gate had no suite. This file is it.
#
# A real cargo-mutants run costs tens of minutes, so the outcomes are staged
# instead: a stub `cargo-mutants` on PATH writes the mutants.out/*.txt files
# each case needs and exits with the code the real tool would. What is under
# test is the gate's judgement, which is what reviews keep finding wrong —
# not cargo-mutants, which is pinned and has its own tests.
#
# Every case asserts an exit code AND a discriminating string, so a gate that
# fails for the wrong reason does not pass this file. Run it in seconds:
#
#   bash scripts/tests/mutants-gate-selftest.sh
set -u

here="$(cd "$(dirname "$0")" && pwd)"
gate="$here/mutants-gate.sh"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

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

# Real files, so the scope check passes for every case that is not about it.
scope="crates/luabox-types/src/env.rs"

pass=0
fail=0

# run <name> <exit-expected> <outcomes-file> <stub-exit> <must-contain>...
run() {
    local name="$1" want_exit="$2" outcomes="$3" stub_exit="$4"
    shift 4
    local log="$work/$name.log"
    STUB_OUTCOMES="$outcomes" STUB_EXIT="$stub_exit" \
        FILES="$scope" ALLOWLIST="$allowlist" MUTANTS_OUT="$work/out-$name" \
        bash "$gate" >"$log" 2>&1
    local got=$?
    local ok=1
    [ "$got" = "$want_exit" ] || ok=0
    local needle
    for needle in "$@"; do
        grep -qF -- "$needle" "$log" || ok=0
    done
    if [ "$ok" = 1 ]; then
        echo "PASS  $name (exit $got)"
        pass=$((pass + 1))
    else
        echo "FAIL  $name: expected exit $want_exit and $# marker(s), got exit $got" >&2
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
run steady_state 0 "$steady" 3 "3 survivor(s)" "all reviewed"

# The round-1 gap: a run where every survivor timed out. Unkilled is unkilled.
flap="$work/flap.txt"
cat >"$flap" <<'OUT'
timeout:crates/luabox-types/src/env.rs:298:30: replace > with >= in TypeEnv::build_from_items
missed:crates/luabox-types/src/env.rs:301:27: replace > with >= in TypeEnv::build_from_items
missed:crates/luabox-types/src/env.rs:1333:9: replace TypeEnv::is_class -> bool with true
OUT
run waived_line_flaps_to_timeout 0 "$flap" 4 "timed out this run rather than surviving"

new_timeout="$work/new-timeout.txt"
cat >"$new_timeout" <<'OUT'
timeout:crates/luabox-types/src/env.rs:900:5: replace TypeEnv::brand_new with ()
missed:crates/luabox-types/src/env.rs:298:30: replace > with >= in TypeEnv::build_from_items
missed:crates/luabox-types/src/env.rs:301:27: replace > with >= in TypeEnv::build_from_items
missed:crates/luabox-types/src/env.rs:1333:9: replace TypeEnv::is_class -> bool with true
OUT
run new_timeout_fails 1 "$new_timeout" 4 "new timed-out mutant" "brand_new"

new_survivor="$work/new.txt"
cat >"$new_survivor" <<'OUT'
missed:crates/luabox-types/src/env.rs:900:5: replace TypeEnv::brand_new with ()
missed:crates/luabox-types/src/env.rs:298:30: replace > with >= in TypeEnv::build_from_items
missed:crates/luabox-types/src/env.rs:301:27: replace > with >= in TypeEnv::build_from_items
missed:crates/luabox-types/src/env.rs:1333:9: replace TypeEnv::is_class -> bool with true
OUT
run new_survivor_fails 1 "$new_survivor" 3 "new surviving mutant" "brand_new"

# The round-2 finding: a shift is a MEASURED pairing, not a reader's guess.
# All three waived mutants move; nothing is new; the run passes and prints
# what to re-pin.
shifted="$work/shifted.txt"
cat >"$shifted" <<'OUT'
missed:crates/luabox-types/src/env.rs:309:30: replace > with >= in TypeEnv::build_from_items
missed:crates/luabox-types/src/env.rs:312:27: replace > with >= in TypeEnv::build_from_items
missed:crates/luabox-types/src/env.rs:1344:9: replace TypeEnv::is_class -> bool with true
OUT
run shifted_batch_is_not_new 0 "$shifted" 3 "3 shifted" "0 new" "re-pin the 3 shifted"

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

# A test that kills a waived mutant is progress, not a failure.
killed="$work/killed.txt"
cat >"$killed" <<'OUT'
missed:crates/luabox-types/src/env.rs:298:30: replace > with >= in TypeEnv::build_from_items
missed:crates/luabox-types/src/env.rs:301:27: replace > with >= in TypeEnv::build_from_items
caught:crates/luabox-types/src/env.rs:1333:9: replace TypeEnv::is_class -> bool with true
OUT
run one_killed_waiver_is_stale_not_failure 0 "$killed" 3 "prune it" "1 stale"

# Round-2 finding 1, direct: a run that tested nothing must not report
# "all reviewed". Same shape as an audit whose scope quietly stopped matching.
empty="$work/empty.txt"
: >"$empty"
run zero_mutants_fails 1 "$empty" 0 "generated 0 mutants" "nothing was audited"

# …and the shape the file check cannot see: mutants generated, but not one of
# them is anything the allowlist knows. 17-of-17 stale is a moved audit.
elsewhere="$work/elsewhere.txt"
cat >"$elsewhere" <<'OUT'
caught:crates/luabox-types/src/other.rs:10:1: replace other::f with ()
missed:crates/luabox-types/src/other.rs:20:1: replace other::g with ()
OUT
run whole_allowlist_stale_fails 1 "$elsewhere" 3 "went stale in one run" "wrong scope"

# The cheapest way for the audit to go quiet: a rename nobody propagated.
missing_log="$work/missing.log"
STUB_OUTCOMES="$steady" STUB_EXIT=3 \
    FILES="crates/luabox-types/src/renamed_by_a_refactor.rs" ALLOWLIST="$allowlist" \
    MUTANTS_OUT="$work/out-missing" bash "$gate" >"$missing_log" 2>&1
missing_exit=$?
if [ "$missing_exit" = 1 ] && grep -qF "scoped file does not exist" "$missing_log"; then
    echo "PASS  missing_scope_file_fails (exit 1)"
    pass=$((pass + 1))
else
    echo "FAIL  missing_scope_file_fails: expected exit 1 naming the path, got exit $missing_exit" >&2
    sed 's/^/        /' "$missing_log" >&2
    fail=$((fail + 1))
fi

echo
echo "mutants-gate-selftest: $pass passed, $fail failed"
[ "$fail" -eq 0 ]

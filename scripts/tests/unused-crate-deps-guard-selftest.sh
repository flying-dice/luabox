#!/bin/bash
# Self-test for scripts/tests/unused-crate-deps-guard.sh (#82) — the gate on
# the gate, same discipline as luals-differential-selftest.sh and
# verdict-differential-selftest.sh.
#
# The gate under test is a thin wrapper over a real compiler lint. Most cases
# use a real two-crate fixture; S10 stubs Cargo narrowly to reach the otherwise
# defensive zero-target branch. `$work` holds a 2-crate
# cargo workspace — `fixture-leaf` (a trivial lib) and `fixture-user` (a lib
# that path-depends on it) — regenerated per case by make_fixture, whose
# second argument is which manifest table fixture-user declares the
# dependency under: `dependencies` (the #81-shaped bug: a normal dep
# fixture-user's lib never touches, reached only from `#[cfg(test)]`) or
# `dev-dependencies` (the fixed shape, same test-only reach). Every case
# below runs the REAL guard script against one of these, through
# UNUSED_CRATE_DEPS_REPO_DIR — if the guard is edited to stop catching the
# bug shape, S2 goes red; nothing here is duplicated logic for the guard to
# drift away from.
#
# S7 reads .gitlab-ci.yml directly (the runner-cache-sweep-selftest.sh
# precedent for checking CI wiring, not just script behaviour) and asserts
# the job this guard is wired into is stage `check`, `needs:` its own
# self-test, and is unconditionally blocking (no job-local `rules:`
# override, `allow_failure`, or `when: manual`) — otherwise the mechanism
# S1-S6/S8-S10 prove correct would be wired to nothing that can fail a
# pipeline. DISCLOSED, not pinned (decisions/12 clause 2): S7 is a literal
# text slice of THIS repo's `.gitlab-ci.yml`, not GitLab's own merged/
# `extends`-resolved config — it cannot see a `rules:`/`allow_failure`
# escape hatch added to `.rust` or `.rules-ci` instead of to this job
# directly (every job extending those would inherit it silently). Closing
# that needs either the full `extends`-chain resolution
# runner-cache-sweep-selftest.sh's own part 2 already builds for cache-key
# expansion (a second consumer would justify lifting it out), or a
# `glab ci lint`/GitLab merged-config API call this task has no network
# access to make — left for a maintainer with that access, not built here.
#
# The cases exercise the real guard, several of them via a MUTATED copy of
# it (not a hand-typed cargo invocation the real script could drift away
# from): S4 removes `--lib`, S8 removes `--locked`, S9 downgrades `-D` to
# `-W` — each proving the guard fails closed on the exact edit it warns
# against in its own header, not merely that cargo behaves a certain way
# in isolation. S2 pins the dedicated structured-diagnostic branch, and S10
# pins the no-target refusal.
#
# Run it in seconds (no full-workspace build — the fixture is two
# three-line crates):
#
#   bash scripts/tests/unused-crate-deps-guard-selftest.sh
set -u

here="$(cd "$(dirname "$0")" && pwd)"
repo_root="$(cd "$here/../.." && pwd)"
guard="$here/unused-crate-deps-guard.sh"
ci_yml="$repo_root/.gitlab-ci.yml"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# shellcheck source=scripts/tests/selftest-lib.sh
. "$here/selftest-lib.sh"

# make_fixture <dir> <leaf-section>
# <leaf-section> is literally `dependencies` or `dev-dependencies`: which
# manifest table fixture-user declares fixture-leaf under. fixture-user's
# lib reaches fixture-leaf ONLY from a #[cfg(test)] module either way — the
# one variable under test is which table the dependency sits in, exactly
# the shape 4fc7ad3 changed in crates/luabox-lint/Cargo.toml.
make_fixture() {
    local dir="$1" leaf_section="$2"
    rm -rf "$dir"
    mkdir -p "$dir/leaf/src" "$dir/user/src"
    cat >"$dir/Cargo.toml" <<EOF
[workspace]
resolver = "3"
members = ["leaf", "user"]
EOF
    cat >"$dir/leaf/Cargo.toml" <<'EOF'
[package]
name = "fixture-leaf"
version = "0.1.0"
edition = "2021"
EOF
    cat >"$dir/leaf/src/lib.rs" <<'EOF'
pub fn leaf_value() -> i32 {
    1
}
EOF
    cat >"$dir/leaf/src/main.rs" <<'EOF'
fn main() {
    let _ = fixture_leaf::leaf_value();
}
EOF
    cat >"$dir/user/Cargo.toml" <<EOF
[package]
name = "fixture-user"
version = "0.1.0"
edition = "2021"

[$leaf_section]
fixture-leaf = { path = "../leaf" }
EOF
    cat >"$dir/user/src/lib.rs" <<'EOF'
pub fn user_value() -> i32 {
    2
}

#[cfg(test)]
mod tests {
    #[test]
    fn uses_leaf_only_here() {
        assert_eq!(fixture_leaf::leaf_value(), 1);
    }
}
EOF
    # Cargo.lock committed so --locked (the guard's own flag) does not
    # itself fail the fixture before the lint ever runs.
    (cd "$dir" && cargo generate-lockfile >/dev/null 2>&1)
}

# ============================================================================
# S1: fixed shape (dev-dependency, test-only reach) passes.
# ============================================================================
fixed_dir="$work/fixed"
make_fixture "$fixed_dir" "dev-dependencies"
log="$work/s1.log"
UNUSED_CRATE_DEPS_REPO_DIR="$fixed_dir" bash "$guard" >"$log" 2>&1
got=$?
assert_exit S1_fixed_shape_passes 0 "$got" "$log"

# ============================================================================
# S2: bug shape (normal dependency, test-only reach) fails, and names the
# crate the way rustc's own diagnostic does — not a guessed string.
# ============================================================================
bug_dir="$work/bug"
make_fixture "$bug_dir" "dependencies"
log="$work/s2.log"
UNUSED_CRATE_DEPS_REPO_DIR="$bug_dir" bash "$guard" >"$log" 2>&1
got=$?
assert_exit S2_bug_shape_fails 1 "$got" "$log" \
    "a [dependencies] entry above is unused" \
    "extern crate \`fixture_leaf\` is unused in crate \`fixture_user\`"

# ============================================================================
# S3: a normal dependency used by production code remains green.
# ============================================================================
prod_dir="$work/production"
make_fixture "$prod_dir" "dependencies"
cat >"$prod_dir/user/src/lib.rs" <<'EOF'
pub fn user_value() -> i32 {
    fixture_leaf::leaf_value() + 1
}
EOF
log="$work/s3.log"
UNUSED_CRATE_DEPS_REPO_DIR="$prod_dir" bash "$guard" >"$log" 2>&1
got=$?
assert_exit S3_production_dependency_passes 0 "$got" "$log"

# ============================================================================
# S4: removing --lib still leaves leaf's bin target for Cargo to check, but
# the guard must reject the missing lib artifacts rather than accept a
# successful partial audit.
# ============================================================================
mutant_guard="$work/guard-without-lib.sh"
sed 's/cargo check --workspace --lib --bins/cargo check --workspace --bins/' \
    "$guard" >"$mutant_guard"
chmod +x "$mutant_guard"
log="$work/s4.log"
UNUSED_CRATE_DEPS_REPO_DIR="$fixed_dir" bash "$mutant_guard" >"$log" 2>&1
got=$?
assert_exit S4_missing_lib_scope_fails_closed 1 "$got" "$log" \
    "did not visit every intended lib/bin"

# ============================================================================
# S5: an unrelated compiler error is not misreported as an unused dependency.
# ============================================================================
broken_dir="$work/broken"
make_fixture "$broken_dir" "dev-dependencies"
printf '%s\n' 'this is not rust' >>"$broken_dir/user/src/lib.rs"
log="$work/s5.log"
UNUSED_CRATE_DEPS_REPO_DIR="$broken_dir" bash "$guard" >"$log" 2>&1
got=$?
assert_exit S5_unrelated_compile_failure_is_honest 1 "$got" "$log" \
    "NOT an unused-dependency finding"

# ============================================================================
# S6: --locked is load-bearing: removing the committed lockfile makes the
# audit inconclusive rather than silently regenerating it.
# ============================================================================
locked_dir="$work/locked"
make_fixture "$locked_dir" "dev-dependencies"
rm "$locked_dir/Cargo.lock"
log="$work/s6.log"
UNUSED_CRATE_DEPS_REPO_DIR="$locked_dir" bash "$guard" >"$log" 2>&1
got=$?
assert_exit S6_missing_lock_fails_closed 1 "$got" "$log" \
    "NOT an unused-dependency finding"

# ============================================================================
# S7: the job this guard is wired into, in .gitlab-ci.yml, is stage `check`
# and carries no allow_failure/manual escape — the config-level half of
# "blocking", independent of whatever the script above proves about the
# script's own behaviour.
# ============================================================================
if [ ! -f "$ci_yml" ]; then
    echo "FAIL  S7_ci_job_is_check_stage_and_blocking: no CI config at $ci_yml" >&2
    fail=$((fail + 1))
else
    # Slice from the job's own top-level key to the next top-level key
    # (a line starting in column 0) — the same "one job's block" cut
    # runner-cache-sweep-selftest.sh makes, narrowed to one named job
    # instead of a generic parse because there is exactly one job to check.
    # Stops at the job's OWN trailing blank line (every job in this file is
    # blank-line-separated from the next job's leading comment — verified
    # above for `check:`) as well as at the next literal top-level key, so a
    # mutation is judged against this job's lines alone, not however much
    # unrelated comment text happens to sit before the next real job.
    job_block="$(awk '
        /^unused-crate-deps:/ { grab=1 }
        grab && (/^$/ || (/^[^[:space:]#]/ && !/^unused-crate-deps:/)) { exit }
        grab { print }
    ' "$ci_yml")"
    if [ -z "$job_block" ]; then
        echo "FAIL  S7_ci_job_is_check_stage_and_blocking: no 'unused-crate-deps:' job in $ci_yml" >&2
        fail=$((fail + 1))
    else
        ok=1
        report=""
        echo "$job_block" | grep -q 'stage: check' || {
            ok=0
            report="$report missing:[stage: check]"
        }
        echo "$job_block" | grep -q 'unused-crate-deps-guard\.sh' || {
            ok=0
            report="$report missing:[calls unused-crate-deps-guard.sh]"
        }
        echo "$job_block" | grep -q 'unused-crate-deps-guard-selftest' || {
            ok=0
            report="$report missing:[needs selftest]"
        }
        echo "$job_block" | grep -q 'rules:' && {
            ok=0
            report="$report present-but-should-be-absent:[local rules]"
        }
        echo "$job_block" | grep -q 'allow_failure' && {
            ok=0
            report="$report present-but-should-be-absent:[allow_failure]"
        }
        echo "$job_block" | grep -q 'when: manual' && {
            ok=0
            report="$report present-but-should-be-absent:[when: manual]"
        }
        if [ "$ok" = 1 ]; then
            echo "PASS  S7_ci_job_is_check_stage_and_blocking"
            pass=$((pass + 1))
        else
            echo "FAIL  S7_ci_job_is_check_stage_and_blocking:$report" >&2
            echo "$job_block" | sed 's/^/        /' >&2
            fail=$((fail + 1))
        fi
    fi
fi

# ============================================================================
# S8: --locked is load-bearing specifically on the `cargo check` line, not
# just present. A mutant with it stripped from THAT line (the `cargo
# metadata` line right below it in the guard keeps its own `--locked` — it
# only needs *a* self-consistent lock by the time it runs, not proof the
# committed one was untouched) regenerates the lock S6 deleted and passes
# silently, on the exact fixture S6 proves the real guard refuses.
# ============================================================================
mutant_guard_locked="$work/guard-without-locked.sh"
sed 's/cargo check --workspace --lib --bins --locked/cargo check --workspace --lib --bins/' \
    "$guard" >"$mutant_guard_locked"
chmod +x "$mutant_guard_locked"
log="$work/s8.log"
UNUSED_CRATE_DEPS_REPO_DIR="$locked_dir" bash "$mutant_guard_locked" >"$log" 2>&1
got=$?
assert_exit S8_locked_flag_is_load_bearing 0 "$got" "$log"

# ============================================================================
# S9: -D (not -W) is load-bearing. A mutant with the RUSTFLAGS level
# downgraded to warn, run against S2's bug-shaped fixture, still compiles
# fixture-user cleanly (a warning does not fail the build, so both cargo's
# own exit code AND the target-visitation check above see a normal,
# fully-visited, zero-warning-as-far-as-exit-code-goes run) — the real,
# would-be-silent version of the bug decisions/12 requires this case to
# demonstrate rather than merely assert in prose.
# ============================================================================
mutant_guard_warn="$work/guard-warn-instead-of-deny.sh"
sed 's/-D unused_crate_dependencies/-W unused_crate_dependencies/' \
    "$guard" >"$mutant_guard_warn"
chmod +x "$mutant_guard_warn"
log="$work/s9.log"
UNUSED_CRATE_DEPS_REPO_DIR="$bug_dir" bash "$mutant_guard_warn" >"$log" 2>&1
got=$?
assert_exit S9_deny_not_warn_is_load_bearing 0 "$got" "$log"

# ============================================================================
# S10: Cargo must not be able to report success without the guard measuring
# at least one intended target. A narrow stub supplies valid empty metadata
# and an empty successful check stream; the parser's explicit no-target
# refusal is the only thing that can make this case red.
# ============================================================================
stub_bin="$work/no-target-bin"
mkdir -p "$stub_bin"
cat >"$stub_bin/cargo" <<'EOF'
#!/bin/sh
case " $* " in
    *" metadata "*)
        printf '%s\n' '{"packages":[],"workspace_members":[]}'
        ;;
esac
exit 0
EOF
chmod +x "$stub_bin/cargo"
log="$work/s10.log"
PATH="$stub_bin:$PATH" UNUSED_CRATE_DEPS_REPO_DIR="$fixed_dir" \
    bash "$guard" >"$log" 2>&1
got=$?
assert_exit S10_zero_targets_fail_closed 1 "$got" "$log" \
    "no workspace lib/bin targets found"

selftest_report unused-crate-deps-guard-selftest

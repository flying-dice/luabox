#!/bin/bash
# ===========================================================================
# Self-test for scripts/ops/runner-cache-sweep.sh — the one `rm -rf` in this
# repo, run hourly from cron against directories a live GitLab runner is
# writing into. Decision 12 says a gate whose failure mode is silence needs a
# test that can watch it fail; that applies with more force to a deleter than
# to a measurer, because its silent failure mode deletes work in progress.
#
# Shape: fixture directories under one mktemp -d, aged with `touch -d @<epoch>`
# so no case has to wait for a clock, and the REAL script run against them
# through its env seams (CACHE_DIR, BUILDS_DIR, LOCK_FILE, MAX_AGE_MIN,
# CACHE_CAP_GIB/CACHE_CAP_KIB, DOCKER_PS). Nothing here stubs the sweep's own
# logic — the only stand-ins are for what a container cannot provide: `logger`
# (no syslog; the shim captures the calls verbatim so `-p user.err` is
# assertable), `docker ps` (no daemon), and one deliberately broken `du`.
#
# Part 2 has nothing to do with the filesystem: it parses .gitlab-ci.yml and
# asserts every cache key keeps at least one `pull-push` writer. That is the
# other half of the eviction contract — a key no job ever writes is a key
# whose archive ages out under the sweep's window while jobs still read it,
# and the failure arrives as a slow cold-cache job, not as an error.
#
# Verified by mutation, decision 12 clause 2. Each guard in the sweep was
# deleted or neutered in a copy, the suite re-run against that copy through
# RUNNER_CACHE_SWEEP_BIN, and the cases it claims to pin confirmed red
# (2026-09-06, findutils 4.10, bash 5.2). Measured:
#   dir assertions deleted            -> S7, S8, S8b
#   du_total coerces empty output to 0 -> S9, S24
#   numeric-slot assertion deleted    -> S10, S20
#   fuzz-corpus age exemption deleted -> S13, S14
#   run-level busy-slot skip deleted  -> S5, S22
#   docker-unavailable fallback = 0   -> S6, S6b, S23
#   flock acquisition dropped         -> S15, S15b
#   parent-directory guard deleted    -> S16
#   cap loop stops subtracting        -> S11b, S12, S12b
#   cap drains strictly oldest-first  -> S12, S12b
#   failed freshness scan reads stale -> S21
#   per-checkout occupancy re-probe deleted, or ignored -> S22, S23
#   every failure exits 2 (no partial code) -> S24, S26
#   over-cap-with-no-candidates check deleted -> S25, S26
#   sizes always reported in MiB      -> S18b, S25
# And one against .gitlab-ci.yml itself, through RUNNER_CACHE_SWEEP_CI_YML:
#   `examples` takes its own `target` tree `policy: pull` -> P1, P1b. That
#   mutation left this suite GREEN until the writer check expanded keys per
#   job, which is exactly what round 2's N-3 measured.
# Every mutation above was rejected by at least one case, none by a case that
# was merely counting log lines. Re-run one with:
#   cp scripts/ops/runner-cache-sweep.sh /tmp/mutant.sh && $EDITOR /tmp/mutant.sh
#   RUNNER_CACHE_SWEEP_BIN=/tmp/mutant.sh bash scripts/tests/runner-cache-sweep-selftest.sh
#   RUNNER_CACHE_SWEEP_CI_YML=/tmp/mutant-ci.yml bash scripts/tests/runner-cache-sweep-selftest.sh
#
#   bash scripts/tests/runner-cache-sweep-selftest.sh
# ===========================================================================
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
sweep="${RUNNER_CACHE_SWEEP_BIN:-$repo/scripts/ops/runner-cache-sweep.sh}"
ci_yml="${RUNNER_CACHE_SWEEP_CI_YML:-$repo/.gitlab-ci.yml}"

# shellcheck source=selftest-lib.sh
source "$here/selftest-lib.sh"

if [ ! -f "$sweep" ]; then
    echo "runner-cache-sweep-selftest: no sweep script at $sweep" >&2
    exit 1
fi
if [ ! -f "$ci_yml" ]; then
    echo "runner-cache-sweep-selftest: no CI config at $ci_yml" >&2
    exit 1
fi

work="$(mktemp -d)"
trap 'rm -rf -- "$work"' EXIT

# ---------------------------------------------------------------------------
# Stand-ins for what a build container has no way to provide.
# ---------------------------------------------------------------------------
shim="$work/shim"
mkdir -p "$shim"

cat >"$shim/logger" <<'SHIM'
#!/bin/bash
# The host's syslog channel is not observable from here, so record the call.
printf '%s\n' "$*" >>"${SELFTEST_LOGGER_LOG:-/dev/null}"
SHIM

cat >"$shim/docker-idle" <<'SHIM'
#!/bin/bash
# `docker ps` with no job containers running.
exit 0
SHIM

cat >"$shim/docker-slot1" <<'SHIM'
#!/bin/bash
# One job running in concurrency slot 1, named as the runner names them.
echo "runner-zAbCd-project-40-concurrent-1-a1b2c3d4-build"
echo "gitlab-runner"
SHIM

cat >"$shim/docker-becomes-busy" <<'SHIM'
#!/bin/bash
# Idle on the FIRST call (the run-level probe), and running a job in slot 0 on
# every call after it: a job that started while the sweep was walking the tree.
# The counter lives in a file because each call is a separate process.
count_file="${SELFTEST_DOCKER_COUNT:?}"
n="$(cat "$count_file" 2>/dev/null || echo 0)"
printf '%s\n' "$((n + 1))" >"$count_file"
if [ "$n" -ge 1 ]; then
    echo "runner-zAbCd-project-40-concurrent-0-a1b2c3d4-build"
fi
SHIM

cat >"$shim/docker-dies-mid-run" <<'SHIM'
#!/bin/bash
# Answers the run-level probe (nothing running) and then stops answering: the
# daemon went away, or the client did, while the sweep was mid-tree.
count_file="${SELFTEST_DOCKER_COUNT:?}"
n="$(cat "$count_file" 2>/dev/null || echo 0)"
printf '%s\n' "$((n + 1))" >"$count_file"
if [ "$n" -ge 1 ]; then
    echo "Cannot connect to the Docker daemon" >&2
    exit 1
fi
SHIM

cat >"$shim/docker-broken" <<'SHIM'
#!/bin/bash
echo "Cannot connect to the Docker daemon" >&2
exit 1
SHIM

mkdir -p "$work/shim-find"
cat >"$work/shim-find/find" <<SHIM
#!/bin/bash
# Fault injector. The sweep's freshness probe is the only find call carrying
# \`-quit\`; fail that one and pass every other call through to the real binary.
# Simulates an unreadable or half-vanished tree under one checkout.
for arg in "\$@"; do
    if [ "\$arg" = -quit ]; then
        echo "find: fault injected" >&2
        exit 1
    fi
done
exec $(command -v find) "\$@"
SHIM

mkdir -p "$work/shim-du"
cat >"$work/shim-du/du" <<'SHIM'
#!/bin/bash
echo "du: /: Permission denied" >&2
exit 1
SHIM

mkdir -p "$work/shim-du-late"
cat >"$work/shim-du-late/du" <<SHIM
#!/bin/bash
# Answers the opening measurement and fails every one after it: the mount goes
# away mid-run, once the age pass has already removed archives.
count_file="\${SELFTEST_DU_COUNT:?}"
n="\$(cat "\$count_file" 2>/dev/null || echo 0)"
printf '%s\n' "\$((n + 1))" >"\$count_file"
if [ "\$n" -ge 1 ]; then
    echo "du: /: Input/output error" >&2
    exit 1
fi
exec $(command -v du) "\$@"
SHIM

chmod +x "$shim"/* "$work/shim-du/du" "$work/shim-du-late/du" "$work/shim-find/find"

# A PATH holding everything the sweep needs EXCEPT docker, so the
# docker-not-installed branch is exercised the way CI actually meets it (the
# rust image has no docker binary) rather than by whatever happens to be on a
# developer's box. A sweep that grows a new external dependency turns S6 red
# with a "command not found" in its log; that is the intended alarm.
nodocker="$work/bin-nodocker"
mkdir -p "$nodocker"
for tool in bash flock mktemp du awk find sort dirname rm; do
    ln -s "$(command -v "$tool")" "$nodocker/$tool"
done
ln -s "$shim/logger" "$nodocker/logger"

# ---------------------------------------------------------------------------
# Fixtures. Build the tree, then age it wholesale, then re-age the few paths a
# case wants fresh: `touch` on a file never moves its directory's mtime, so the
# order of the last two steps does not matter — the order of the FIRST two
# does (creating a file does move the directory's mtime).
# ---------------------------------------------------------------------------
# Ages are relative to the moment the fixture is built, not to the start of
# the suite: S1 sits one minute either side of the window, and a suite slow
# enough to drift a minute would flip it.
at() { printf '@%s\n' "$(($(date +%s) - $1 * 60))"; }

mkfixture() { # mkfixture <name> -> prints the fixture root
    local d="$work/fx/$1"
    mkdir -p "$d/cache" "$d/builds"
    printf '%s\n' "$d"
}

mkfile() { # mkfile <path> [kib]
    mkdir -p -- "$(dirname -- "$1")"
    if [ "${2:-0}" -gt 0 ]; then
        head -c "$(($2 * 1024))" /dev/zero >"$1"
    else
        : >"$1"
    fi
}

age_tree() { # age_tree <root> <minutes>
    find "$1" -depth -exec touch -d "$(at "$2")" -- {} +
}

age_path() { # age_path <path> <minutes>
    touch -d "$(at "$2")" -- "$1"
}

# ---------------------------------------------------------------------------
# Running the real script.
# ---------------------------------------------------------------------------
sweep_log=""
sweep_syslog=""
sweep_rc=0

run_sweep() { # run_sweep <case> <fixture> [VAR=value...]
    local case_name="$1" fx="$2"
    shift 2
    sweep_log="$work/$case_name.log"
    sweep_syslog="$work/$case_name.syslog"
    : >"$sweep_syslog"
    : >"$work/$case_name.dockercount"
    : >"$work/$case_name.ducount"
    sweep_rc=0
    env \
        PATH="$shim:$PATH" \
        SELFTEST_LOGGER_LOG="$sweep_syslog" \
        SELFTEST_DOCKER_COUNT="$work/$case_name.dockercount" \
        SELFTEST_DU_COUNT="$work/$case_name.ducount" \
        CACHE_DIR="$fx/cache" \
        BUILDS_DIR="$fx/builds" \
        LOCK_FILE="$fx/lock" \
        MAX_AGE_MIN=1440 \
        CACHE_CAP_GIB=200 \
        DOCKER_PS="$shim/docker-idle" \
        "$@" \
        bash "$sweep" >"$sweep_log" 2>&1 || sweep_rc=$?
    # The syslog capture is the channel cron actually reads; fold it into the
    # log the assertions run against so a case can pin either one.
    sed 's/^/syslog: /' "$sweep_syslog" >>"$sweep_log"
}

survives() { check_true "$1" test -e "$2"; }
deleted() { check_false "$1" test -e "$2"; }

echo "=== Part 1 — the sweep against fixture trees ==="

# --- S1: the age boundary --------------------------------------------------
# Measured, not assumed (GNU findutils 4.10, 2026-09-06): `-mmin +1440` matches
# an archive last touched 1440 minutes ago or more, and `-mmin -1440` matches
# one touched 1439 ago or less — find rounds the sub-minute remainder up, so
# the rule on the tin ("untouched for 24h") is the rule on the disk. A window
# that quietly slipped a minute either way should cost a red case.
fx="$(mkfixture s1)"
ns="$fx/cache/flying-dice/luabox"
mkfile "$ns/cargo-home-protected/cache.zip"
mkfile "$ns/target-check-protected/cache.zip"
mkfile "$ns/target-check-non_protected/cache.zip"
mkfile "$ns/luals-3.13.5-protected/cache.zip"
age_tree "$fx" 4320
age_path "$ns/cargo-home-protected/cache.zip" 1439
age_path "$ns/target-check-protected/cache.zip" 1440
age_path "$ns/target-check-non_protected/cache.zip" 1441
run_sweep s1 "$fx"
assert_exit S1_age_boundary_sweep_succeeds 0 "$sweep_rc" "$sweep_log" \
    "cache: age summary: removed 3"
survives "S1 archive untouched 1439min survives" "$ns/cargo-home-protected/cache.zip"
deleted "S1 archive untouched 1440min is deleted" "$ns/target-check-protected/cache.zip"
deleted "S1 archive untouched 1441min is deleted" "$ns/target-check-non_protected/cache.zip"
deleted "S1 archive untouched 3 days is deleted" "$ns/luals-3.13.5-protected/cache.zip"

# --- S2: only archives are cache candidates --------------------------------
fx="$(mkfixture s2)"
ns="$fx/cache/flying-dice/luabox"
mkfile "$ns/cargo-home-protected/cache.zip"
mkfile "$ns/cargo-home-protected/cache.tar"
mkfile "$ns/cargo-home-protected/cache.zip.tmp"
mkfile "$ns/README"
age_tree "$fx" 4320
run_sweep s2 "$fx"
assert_exit S2_non_archive_files_are_not_candidates 0 "$sweep_rc" "$sweep_log" \
    "cache: age summary: removed 1"
deleted "S2 the .zip is deleted" "$ns/cargo-home-protected/cache.zip"
survives "S2 a .tar beside it is untouched" "$ns/cargo-home-protected/cache.tar"
survives "S2 a half-written .zip.tmp is untouched" "$ns/cargo-home-protected/cache.zip.tmp"
survives "S2 a non-archive file is untouched" "$ns/README"

# --- S3: a stale checkout at depth 4 ---------------------------------------
fx="$(mkfixture s3)"
mkfile "$fx/builds/zAbCd/0/flying-dice/luabox/target/debug/build.o" 8
mkfile "$fx/builds/zAbCd/0/flying-dice/luabox/.git/HEAD"
age_tree "$fx" 4320
run_sweep s3 "$fx"
assert_exit S3_stale_checkout_removed 0 "$sweep_rc" "$sweep_log" \
    "builds: summary: removed 1" \
    "builds: removed $fx/builds/zAbCd/0/flying-dice/luabox"
deleted "S3 the stale checkout is gone" "$fx/builds/zAbCd/0/flying-dice/luabox"
survives "S3 the slot directory itself survives" "$fx/builds/zAbCd/0"

# --- S4: one fresh file three levels down keeps the checkout ---------------
fx="$(mkfixture s4)"
co="$fx/builds/zAbCd/0/flying-dice/luabox"
mkfile "$co/target/debug/deps/liblua.rlib" 8
mkfile "$co/.git/index.lock"
age_tree "$fx" 4320
age_path "$co/target/debug/deps/liblua.rlib" 3
run_sweep s4 "$fx"
assert_exit S4_live_checkout_survives 0 "$sweep_rc" "$sweep_log" \
    "builds: summary: removed 0" "in-use 1"
survives "S4 a checkout with one fresh file 3 levels deep survives" "$co"
survives "S4 and its stale siblings survive with it" "$co/.git/index.lock"

# --- S5: a busy slot is not swept, an idle one beside it is ----------------
fx="$(mkfixture s5)"
busy="$fx/builds/zAbCd/1/flying-dice/luabox"
idle="$fx/builds/zAbCd/0/flying-dice/luabox"
mkfile "$busy/target/debug/build.o" 8
mkfile "$idle/target/debug/build.o" 8
age_tree "$fx" 4320
run_sweep s5 "$fx" DOCKER_PS="$shim/docker-slot1"
assert_exit S5_busy_slot_is_skipped 0 "$sweep_rc" "$sweep_log" \
    "builds: slot(s) 1 busy" "builds: summary: removed 1" "busy slots 1"
survives "S5 a stale checkout in a RUNNING slot is untouched" "$busy"
deleted "S5 the idle slot beside it is still swept" "$idle"

# --- S6: no docker, no deletions -------------------------------------------
# The degrade has to be explicit and it has to be safe: log it, and treat every
# slot as busy. Failing the sweep instead would stop the cache half too;
# passing silently would delete live checkouts on any host without a docker
# client.
fx="$(mkfixture s6)"
co="$fx/builds/zAbCd/0/flying-dice/luabox"
mkfile "$co/target/debug/build.o" 8
mkfile "$fx/cache/flying-dice/luabox/cargo-home-protected/cache.zip"
age_tree "$fx" 4320
run_sweep s6 "$fx" PATH="$nodocker" DOCKER_PS=""
assert_exit S6_docker_absent_treats_every_slot_as_busy 0 "$sweep_rc" "$sweep_log" \
    "builds: docker not found — treating every slot as busy" \
    "builds: summary: removed 0" \
    "!command not found"
survives "S6 no checkout is removed without a slot-occupancy probe" "$co"
deleted "S6 the cache half still runs" "$fx/cache/flying-dice/luabox/cargo-home-protected/cache.zip"

run_sweep s6b "$fx" DOCKER_PS="$shim/docker-broken"
assert_exit S6b_docker_ps_failure_treats_every_slot_as_busy 0 "$sweep_rc" "$sweep_log" \
    "failed — treating every slot as busy" "builds: summary: removed 0"
survives "S6b a failing docker ps removes no checkout" "$co"

# --- S7/S8: a missing bind mount is a refusal, not a clean sweep -----------
fx="$(mkfixture s7)"
mkfile "$fx/builds/zAbCd/0/flying-dice/luabox/target/build.o"
age_tree "$fx" 4320
rm -rf "$fx/cache"
run_sweep s7 "$fx"
assert_exit S7_missing_cache_dir_refuses 2 "$sweep_rc" "$sweep_log" \
    "REFUSED: CACHE_DIR is not a directory" \
    "syslog: -p user.err -t runner-cache-sweep" \
    "!net " "!builds: summary"
survives "S7 nothing is deleted when a mount is missing" "$fx/builds/zAbCd/0/flying-dice/luabox"

fx="$(mkfixture s8)"
mkfile "$fx/cache/flying-dice/luabox/cargo-home-protected/cache.zip"
age_tree "$fx" 4320
rm -rf "$fx/builds"
run_sweep s8 "$fx"
assert_exit S8_missing_builds_dir_refuses 2 "$sweep_rc" "$sweep_log" \
    "REFUSED: BUILDS_DIR is not a directory" \
    "syslog: -p user.err -t runner-cache-sweep"
survives "S8 the cache half is not swept either" "$fx/cache/flying-dice/luabox/cargo-home-protected/cache.zip"

fx="$(mkfixture s8b)"
mkfile "$fx/cache/flying-dice/luabox/cargo-home-protected/cache.zip"
age_tree "$fx" 4320
rm -rf "$fx/builds"
: >"$fx/builds"
run_sweep s8b "$fx"
assert_exit S8b_builds_dir_is_a_file_refuses 2 "$sweep_rc" "$sweep_log" \
    "REFUSED: BUILDS_DIR is not a directory"

# --- S9: du failing is a refusal -------------------------------------------
# The original read a failed `du` as zero and reported `freed 0 MiB` in a green
# run — the precise shape decisions/12 exists to forbid.
fx="$(mkfixture s9)"
mkfile "$fx/cache/flying-dice/luabox/cargo-home-protected/cache.zip"
mkfile "$fx/builds/zAbCd/0/flying-dice/luabox/target/build.o"
age_tree "$fx" 4320
run_sweep s9 "$fx" PATH="$work/shim-du:$shim:$PATH"
assert_exit S9_du_failure_before_any_deletion_refuses_with_2 2 "$sweep_rc" "$sweep_log" \
    "REFUSED: du failed" \
    "!PARTIALLY SWEPT" \
    "syslog: -p user.err -t runner-cache-sweep" \
    "!net " "!cache: age summary"
survives "S9 nothing is deleted when the measurement fails" \
    "$fx/cache/flying-dice/luabox/cargo-home-protected/cache.zip"

# --- S10/S20: the layout is asserted, not assumed --------------------------
# Pre-bind-mount hosts had <builds>/<namespace>/<project>. The old sweep's
# depth-4 scan matched nothing there: it evicted nothing and said so in the
# language of a successful run.
fx="$(mkfixture s10)"
mkfile "$fx/builds/flying-dice/luabox/target/build.o"
mkfile "$fx/cache/flying-dice/luabox/cargo-home-protected/cache.zip"
age_tree "$fx" 4320
run_sweep s10 "$fx"
assert_exit S10_depth2_layout_refuses 2 "$sweep_rc" "$sweep_log" \
    "REFUSED: builds layout: expected a numeric concurrency slot" \
    "syslog: -p user.err -t runner-cache-sweep"
survives "S10 a depth-2 tree is reported, never swept blind" "$fx/builds/flying-dice/luabox"
survives "S10 and the cache half does not run either" \
    "$fx/cache/flying-dice/luabox/cargo-home-protected/cache.zip"

fx="$(mkfixture s20)"
mkfile "$fx/builds/zAbCd/0/flying-dice/luabox/target/build.o"
mkfile "$fx/builds/zAbCd/.stray/leftover"
age_tree "$fx" 4320
run_sweep s20 "$fx"
assert_exit S20_unexpected_entry_under_a_token_refuses 2 "$sweep_rc" "$sweep_log" \
    "REFUSED: builds layout: expected a numeric concurrency slot"
survives "S20 an unexpected entry stops the sweep instead of being ignored" \
    "$fx/builds/zAbCd/0/flying-dice/luabox"

fx="$(mkfixture s20b)"
mkfile "$fx/builds/zAbCd/0/flying-dice/luabox/target/build.o"
: >"$fx/builds/loose-file"
age_tree "$fx" 4320
run_sweep s20b "$fx"
assert_exit S20b_loose_file_under_builds_refuses 2 "$sweep_rc" "$sweep_log" \
    "REFUSED: builds layout: expected runner-token directories"

# --- S11: the cap, and the arithmetic inside it ----------------------------
# Sizes are in KiB via CACHE_CAP_KIB so the drain runs against a five-megabyte
# fixture instead of a two-hundred-gigabyte one. Everything here is FRESH, so
# the age pass cannot be what removed it.
fx="$(mkfixture s11)"
ns="$fx/cache/flying-dice/luabox"
mkfile "$ns/target-check-protected/cache.zip" 256
mkfile "$ns/target-examples-protected/cache.zip" 256
mkfile "$ns/target-coverage-unit-protected/cache.zip" 256
mkfile "$ns/cargo-home-protected/cache.zip" 256
mkfile "$ns/luals-3.13.5-protected/cache.zip" 256
mkfile "$fx/builds/zAbCd/0/flying-dice/luabox/.git/HEAD"
age_tree "$fx" 30
age_path "$ns/target-check-protected/cache.zip" 300
age_path "$ns/target-examples-protected/cache.zip" 240
age_path "$ns/target-coverage-unit-protected/cache.zip" 180
age_path "$ns/cargo-home-protected/cache.zip" 120
age_path "$ns/luals-3.13.5-protected/cache.zip" 60
run_sweep s11 "$fx"
assert_exit S11a_under_the_cap_the_step_still_reports 0 "$sweep_rc" "$sweep_log" \
    "cache: cap summary: removed 0" "(cap 200GiB)"
survives "S11a nothing is removed under a 200GiB cap" "$ns/target-check-protected/cache.zip"

run_sweep s11b "$fx" CACHE_CAP_KIB=650
assert_exit S11b_cap_drains_oldest_first_and_stops 0 "$sweep_rc" "$sweep_log" \
    "cache: cap summary: removed 3" "(cap 650KiB)"
deleted "S11b the oldest archive goes first" "$ns/target-check-protected/cache.zip"
deleted "S11b then the second oldest" "$ns/target-examples-protected/cache.zip"
deleted "S11b then the third" "$ns/target-coverage-unit-protected/cache.zip"
survives "S11b the drain stops as soon as the tree is under the cap" "$ns/cargo-home-protected/cache.zip"
survives "S11b so the newest archives are kept" "$ns/luals-3.13.5-protected/cache.zip"
check_false "S11b the cap never touches the builds tree" \
    grep -qF "builds: removed" "$sweep_log"

# --- S12: the corpus is drained last, not oldest-first ---------------------
# The corpus here is the OLDEST file in the tree, so a plain oldest-first drain
# eats it first. It is accumulated state; a rebuildable target tree is not.
fx="$(mkfixture s12)"
ns="$fx/cache/flying-dice/luabox"
mkfile "$ns/fuzz-corpus-lua_parse-protected/cache.zip" 256
mkfile "$ns/target-check-protected/cache.zip" 256
mkfile "$ns/cargo-home-protected/cache.zip" 256
age_tree "$fx" 30
age_path "$ns/fuzz-corpus-lua_parse-protected/cache.zip" 600
age_path "$ns/target-check-protected/cache.zip" 300
age_path "$ns/cargo-home-protected/cache.zip" 120
run_sweep s12 "$fx" CACHE_CAP_KIB=600
assert_exit S12_cap_drains_rebuildable_archives_before_the_corpus 0 "$sweep_rc" "$sweep_log" \
    "cache: cap summary: removed 1" \
    "cache: cap removed $ns/target-check-protected/cache.zip" \
    "!cache: cap removed $ns/fuzz-corpus-lua_parse-protected/cache.zip"
survives "S12 the oldest file in the tree survives because it is a corpus" \
    "$ns/fuzz-corpus-lua_parse-protected/cache.zip"
deleted "S12 the rebuildable tree went instead" "$ns/target-check-protected/cache.zip"

run_sweep s12b "$fx" CACHE_CAP_KIB=1
assert_exit S12b_the_corpus_is_still_subject_to_the_cap 0 "$sweep_rc" "$sweep_log" \
    "cache: cap removed $ns/fuzz-corpus-lua_parse-protected/cache.zip"
deleted "S12b under enough pressure the corpus goes too" \
    "$ns/fuzz-corpus-lua_parse-protected/cache.zip"

# --- S13: the corpus is exempt from the age rule ---------------------------
fx="$(mkfixture s13)"
ns="$fx/cache/flying-dice/luabox"
mkfile "$ns/fuzz-corpus-lua_parse-protected/cache.zip"
mkfile "$ns/fuzz-corpus-luacats-non_protected/cache.zip"
mkfile "$ns/fuzz-target-lua_parse-protected/cache.zip"
age_tree "$fx" 20160
run_sweep s13 "$fx"
assert_exit S13_corpus_archives_are_exempt_from_the_age_rule 0 "$sweep_rc" "$sweep_log" \
    "cache: age summary: removed 1, exempt 2"
survives "S13 a 14-day-old corpus survives the 24h rule" "$ns/fuzz-corpus-lua_parse-protected/cache.zip"
survives "S13 including its non_protected copy" "$ns/fuzz-corpus-luacats-non_protected/cache.zip"
deleted "S13 the fuzz BUILD tree beside it is not exempt" "$ns/fuzz-target-lua_parse-protected/cache.zip"

# --- S14: spaces in every path segment -------------------------------------
fx="$(mkfixture "s14 with spaces")"
ns="$fx/cache/flying dice/lua box"
mkfile "$ns/target-check protected/cache file.zip"
mkfile "$ns/fuzz-corpus-lua parse-protected/cache file.zip"
co="$fx/builds/zAbCd token/0/flying dice/lua box"
mkfile "$co/target/debug/build one.o"
age_tree "$fx" 4320
run_sweep s14 "$fx"
assert_exit S14_spaces_in_every_path_segment 0 "$sweep_rc" "$sweep_log" \
    "cache: age summary: removed 1, exempt 1" \
    "builds: summary: removed 1"
deleted "S14 a spaced archive path is deleted correctly" "$ns/target-check protected/cache file.zip"
survives "S14 a spaced corpus path is exempted correctly" "$ns/fuzz-corpus-lua parse-protected/cache file.zip"
deleted "S14 a spaced checkout path is deleted correctly" "$co"
survives "S14 the spaced slot directory survives" "$fx/builds/zAbCd token/0"

# --- S15: two sweeps never overlap -----------------------------------------
fx="$(mkfixture s15)"
mkfile "$fx/cache/flying-dice/luabox/cargo-home-protected/cache.zip"
mkfile "$fx/builds/zAbCd/0/flying-dice/luabox/target/build.o"
age_tree "$fx" 4320
: >"$fx/lock"
exec 8>>"$fx/lock"
flock -n 8
run_sweep s15 "$fx"
exec 8>&-
assert_exit S15_a_second_sweep_skips_while_one_holds_the_lock 0 "$sweep_rc" "$sweep_log" \
    "skipped: another sweep holds" \
    "!cache: age summary" "!builds: summary" "!net "
survives "S15 the locked-out run deletes no archive" \
    "$fx/cache/flying-dice/luabox/cargo-home-protected/cache.zip"
survives "S15 and no checkout" "$fx/builds/zAbCd/0/flying-dice/luabox"
run_sweep s15b "$fx"
assert_exit S15b_the_lock_is_released_for_the_next_run 0 "$sweep_rc" "$sweep_log" \
    "cache: age summary: removed 1"

# --- S16: an archive whose key directory is being written ------------------
fx="$(mkfixture s16)"
ns="$fx/cache/flying-dice/luabox"
mkfile "$ns/target-check-protected/cache.zip"
mkfile "$ns/cargo-home-protected/cache.zip"
age_tree "$fx" 4320
age_path "$ns/target-check-protected" 2
run_sweep s16 "$fx"
assert_exit S16_a_fresh_key_directory_protects_its_archive 0 "$sweep_rc" "$sweep_log" \
    "cache: age summary: removed 1, exempt 0, in-use 1"
survives "S16 an archive under a just-touched key directory survives" "$ns/target-check-protected/cache.zip"
deleted "S16 the archive beside it, whose directory is stale, goes" "$ns/cargo-home-protected/cache.zip"

# --- S17: empty directories, but only old ones -----------------------------
fx="$(mkfixture s17)"
mkdir -p "$fx/cache/flying-dice/luabox/stale-key-protected"
mkdir -p "$fx/cache/flying-dice/luabox/fresh-key-protected"
mkfile "$fx/builds/zAbCd/0/flying-dice/luabox/.git/HEAD"
age_tree "$fx" 4320
age_path "$fx/cache/flying-dice/luabox/fresh-key-protected" 2
run_sweep s17 "$fx"
assert_exit S17_empty_directory_sweep_respects_the_window 0 "$sweep_rc" "$sweep_log"
deleted "S17 an empty key directory older than the window is removed" \
    "$fx/cache/flying-dice/luabox/stale-key-protected"
survives "S17 an empty key directory a runner just created is not" \
    "$fx/cache/flying-dice/luabox/fresh-key-protected"

# --- S18: the summary line is a signed delta over a real measurement -------
# The magnitude is deliberately NOT pinned: `du -sm` rounds each tree up to a
# whole MiB, so the exact figure depends on the filesystem under the fixture.
# What is pinned is the shape — a sign, and a number that goes DOWN when the
# sweep releases space. `freed`, the word this replaces, could not.
fx="$(mkfixture s18)"
mkfile "$fx/cache/flying-dice/luabox/cargo-home-protected/cache.zip" 2048
mkfile "$fx/builds/zAbCd/0/flying-dice/luabox/target/build.o" 64
age_tree "$fx" 4320
run_sweep s18 "$fx"
assert_exit S18_the_run_reports_a_signed_delta 0 "$sweep_rc" "$sweep_log" "!freed"
check_true "S18 the summary is a signed delta, negative when space is released" \
    grep -qE '^net -[0-9]+ (KiB|MiB); cache\+builds now [0-9]+ (KiB|MiB)$' "$sweep_log"
check_true "S18 the delta reaches syslog, not only stdout" \
    grep -qE 'net -[0-9]+ (KiB|MiB)' "$sweep_syslog"

fx="$(mkfixture s18b)"
mkfile "$fx/cache/flying-dice/luabox/cargo-home-protected/cache.zip" 512
mkfile "$fx/builds/zAbCd/0/flying-dice/luabox/.git/HEAD"
age_tree "$fx" 30
run_sweep s18b "$fx"
assert_exit S18b_a_run_that_removes_nothing_reports_no_change 0 "$sweep_rc" "$sweep_log" \
    "net +0 KiB"

# --- S22: a slot that goes busy DURING the sweep ---------------------------
# The run-level occupancy probe is minutes old by the time a multi-GB tree
# finishes scanning, and a job that has started but not yet written a file is
# invisible to the freshness probes — the runner, however, has already marked
# the slot busy. So occupancy is re-read per checkout, as the last thing before
# `rm -rf`. Note the summary: the run-level probe saw slot 0 IDLE (busy slots
# 0), which is what makes this case discriminate the re-probe rather than the
# run-level skip.
fx="$(mkfixture s22)"
co="$fx/builds/zAbCd/0/flying-dice/luabox"
mkfile "$co/target/debug/build.o" 8
age_tree "$fx" 4320
run_sweep s22 "$fx" DOCKER_PS="$shim/docker-becomes-busy"
assert_exit S22_a_slot_that_goes_busy_mid_sweep_keeps_its_checkout 0 "$sweep_rc" "$sweep_log" \
    "builds: slot 0 became busy during the sweep — kept $co" \
    "builds: summary: removed 0, busy slots 0, in-use 0, busy on re-probe 1" \
    "!builds: removed $co"
survives "S22 the checkout a job just claimed is kept" "$co"

# --- S23: the re-probe cannot answer ---------------------------------------
# Same degrade as at run level, applied at the point it matters most: if the
# occupancy question cannot be answered immediately before `rm -rf`, the answer
# is busy.
fx="$(mkfixture s23)"
co="$fx/builds/zAbCd/0/flying-dice/luabox"
mkfile "$co/target/debug/build.o" 8
age_tree "$fx" 4320
run_sweep s23 "$fx" DOCKER_PS="$shim/docker-dies-mid-run"
assert_exit S23_a_re_probe_that_cannot_answer_counts_as_busy 0 "$sweep_rc" "$sweep_log" \
    "builds: slot-occupancy re-probe unavailable — kept $co" \
    "builds: summary: removed 0, busy slots 0, in-use 0, busy on re-probe 1"
survives "S23 the checkout is kept when the probe stops answering" "$co"

# --- S21: a freshness scan that ERRORS is not evidence of staleness --------
# The probe is what stands between `rm -rf` and a live checkout. If it fails —
# an I/O error, a half-unmounted tree, a directory the sweep cannot read — the
# answer is "I do not know", and the only safe reading of that is "leave it".
# Reading a failed scan as "nothing fresh here" is how a deleter eats a live
# workspace on the one day the filesystem is already unhappy.
fx="$(mkfixture s21)"
mkfile "$fx/cache/flying-dice/luabox/cargo-home-protected/cache.zip"
mkfile "$fx/builds/zAbCd/0/flying-dice/luabox/target/build.o"
age_tree "$fx" 4320
run_sweep s21 "$fx" PATH="$work/shim-find:$shim:$PATH"
assert_exit S21_a_failed_freshness_scan_deletes_nothing 0 "$sweep_rc" "$sweep_log" \
    "builds: summary: removed 0" "cache: age summary: removed 0"
survives "S21 a checkout whose scan errored is kept" "$fx/builds/zAbCd/0/flying-dice/luabox"
survives "S21 an archive whose re-stat errored is kept" \
    "$fx/cache/flying-dice/luabox/cargo-home-protected/cache.zip"

# --- S24: a failure AFTER the first deletion is exit 3, not exit 2 ---------
# "REFUSED: nothing was deleted" was false the moment the age pass had removed
# an archive and a later `du` failed — the operator reads it, believes the tree
# is untouched, and looks in the wrong place. Two codes, and the boundary is
# the first deletion.
fx="$(mkfixture s24)"
zip="$fx/cache/flying-dice/luabox/cargo-home-protected/cache.zip"
mkfile "$zip"
mkfile "$fx/builds/zAbCd/0/flying-dice/luabox/target/build.o"
age_tree "$fx" 4320
run_sweep s24 "$fx" PATH="$work/shim-du-late:$shim:$PATH"
assert_exit S24_a_failure_after_a_deletion_exits_3 3 "$sweep_rc" "$sweep_log" \
    "PARTIALLY SWEPT after 1 removal(s): du failed" \
    "cache: age removed $zip" \
    "syslog: -p user.err -t runner-cache-sweep" \
    "!REFUSED"
deleted "S24 the removal it reports really happened" "$zip"
survives "S24 and it stopped there, before the builds half" \
    "$fx/builds/zAbCd/0/flying-dice/luabox"

# --- S25: over the cap with nothing the glob can drain ---------------------
# A runner that writes `cache.zst` (or any rename of the archive) leaves this
# sweep with no candidates. Reporting `removed 0, tree now ~0 MiB` and exiting
# 0 over a full disk is #85's original signature; it is an alarm now.
fx="$(mkfixture s25)"
zst="$fx/cache/flying-dice/luabox/cargo-home-protected/cache.zst"
mkfile "$zst" 64
mkfile "$fx/builds/zAbCd/0/flying-dice/luabox/.git/HEAD"
age_tree "$fx" 4320
run_sweep s25 "$fx" CACHE_CAP_KIB=1
assert_exit S25_over_cap_with_no_candidates_refuses 2 "$sweep_rc" "$sweep_log" \
    "REFUSED: cache is" "over the 1KiB cap" "no '*.zip' archive matched" \
    "syslog: -p user.err -t runner-cache-sweep" \
    "!cap summary: removed 0"
survives "S25 the archive it cannot drain is left alone" "$zst"
check_true "S25 the size that triggered it is legible, not rounded to ~0 MiB" \
    grep -qE 'cache is [0-9]+ KiB, over the 1KiB cap' "$sweep_log"

# --- S26: the same alarm, after the age pass has already removed something --
fx="$(mkfixture s26)"
zip="$fx/cache/flying-dice/luabox/target-check-protected/cache.zip"
zst="$fx/cache/flying-dice/luabox/cargo-home-protected/cache.zst"
mkfile "$zip"
mkfile "$zst" 64
mkfile "$fx/builds/zAbCd/0/flying-dice/luabox/.git/HEAD"
age_tree "$fx" 4320
run_sweep s26 "$fx" CACHE_CAP_KIB=1
assert_exit S26_over_cap_with_no_candidates_after_a_deletion_exits_3 3 "$sweep_rc" "$sweep_log" \
    "PARTIALLY SWEPT after 1 removal(s): cache is" \
    "cache: age removed $zip" \
    "!REFUSED"
deleted "S26 the age pass's removal stands" "$zip"
survives "S26 the archive the glob cannot see is untouched" "$zst"

echo
echo "=== Part 2 — every cache key keeps a pull-push writer, per JOB ==="

# Finding 9 / N-3. The sweep evicts on mtime, and GitLab touches a local cache
# archive only when a job WRITES it: `policy: pull` extracts the zip without
# rewriting it. A key every job takes read-only therefore ages out under the
# window while jobs are still reading it, and the symptom is a job that
# silently rebuilds from cold, not an error. The rule this asserts is the
# pipeline half of the eviction contract: at least one writer per key.
#
# The keys are expanded PER JOB before anything is counted. Aggregating on the
# unexpanded string was the round-2 defect: `target-$CI_JOB_NAME_SLUG` is one
# string in the file and nine different archives on disk, so a single
# pull-push job covered all nine and `examples` could be flipped read-only with
# the suite still green. Expansion needs the job set, so this reads the job
# set: top-level jobs, their `extends` chains (last parent wins, the job's own
# `cache:` wins over all), `variables:` from the global block and every level
# of the chain, `parallel: matrix:` legs, and `CI_JOB_NAME_SLUG` via GitLab's
# own slug rule.
#
# Read straight from the YAML rather than from `glab ci config compile`: the
# self-test runs in a build container with no glab, no token and no network,
# and a committed snapshot of the compiled config would go stale silently —
# the exact failure mode decisions/12 names. The cost is a parser, so it is
# written to REFUSE anything it cannot resolve — an unknown cache entry shape,
# an unknown policy, a dangling or nested alias, an `extends` target that does
# not exist, a variable it cannot expand, a matrix leg over more than one
# variable, a matrix job keyed on the job slug, a top-level `default: cache:`,
# or a file with no jobs or no keys at all — rather than to skip it and pass.
# Every one of those refusals is pinned by a fixture below.
#
# Known limit, stated rather than hidden: occurrences are counted wherever a
# `cache:` block is reachable from a job, including through hidden templates
# like `.rust`. That is where the pipeline's one cargo-home writer is declared.

cp_reset() {
    unset CP_KEY CP_POLICY CP_MERGE BLK_SEEN BLK_CACHE BLK_HAS_CACHE BLK_EXTENDS \
        BLK_VARS BLK_MATRIX GLOBAL_VARS EXP_VARS KEY_READERS KEY_WRITERS BLK_ORDER
    declare -gA CP_KEY=() CP_POLICY=() CP_MERGE=() BLK_SEEN=() BLK_CACHE=() \
        BLK_HAS_CACHE=() BLK_EXTENDS=() BLK_VARS=() BLK_MATRIX=() GLOBAL_VARS=() \
        EXP_VARS=() KEY_READERS=() KEY_WRITERS=()
    declare -ga BLK_ORDER=()
}

# An unquoted YAML scalar ends at an inline ` #` comment; a quoted one ends at
# its closing quote. Round 2 caught this the expensive way: LUALS_VERSION's
# trailing comment became part of the cache key.
cp_strip_comment() { # cp_strip_comment <raw value>
    local v="$1"
    case "$v" in
    '"'* | "'"*)
        printf '%s' "$v"
        return 0
        ;;
    esac
    case "$v" in
    *' #'*) v="${v%% #*}" ;;
    esac
    printf '%s' "${v%"${v##*[![:space:]]}"}"
}

cp_scalar() { # cp_scalar <raw value> — comment stripped, one layer of quotes
    local v
    v="$(cp_strip_comment "$1")"
    case "$v" in
    '"'*)
        v="${v#\"}"
        v="${v%%\"*}"
        ;;
    "'"*)
        v="${v#\'}"
        v="${v%%\'*}"
        ;;
    esac
    printf '%s' "$v"
}

# GitLab's own slug rule (Gitlab::Utils.slugify): downcase, every character
# outside [a-z0-9] becomes '-', truncate to 63, then strip leading/trailing
# '-'. Not squeezed: two adjacent non-alphanumerics stay two dashes.
cp_slugify() { # cp_slugify <job name>
    local s="${1,,}" out="" i c
    for ((i = 0; i < ${#s}; i++)); do
        c="${s:i:1}"
        case "$c" in
        [a-z0-9]) out="$out$c" ;;
        *) out="$out-" ;;
        esac
    done
    out="${out:0:63}"
    while [ "${out#-}" != "$out" ]; do out="${out#-}"; done
    while [ "${out%-}" != "$out" ]; do out="${out%-}"; done
    printf '%s' "$out"
}

# $NAME and ${NAME} against EXP_VARS. No eval, and an unknown name is a
# refusal: a key this check cannot name is a key it cannot verify.
cp_expand() { # cp_expand <text> -> CP_EXPANDED
    local rest="$1" out="" name c
    while [ -n "$rest" ]; do
        case "$rest" in
        *'$'*)
            out="$out${rest%%\$*}"
            rest="${rest#*\$}"
            name=""
            if [ "${rest:0:1}" = "{" ]; then
                case "$rest" in
                *'}'*)
                    name="${rest%%\}*}"
                    name="${name#\{}"
                    rest="${rest#*\}}"
                    ;;
                *)
                    echo "PARSE ERROR: unterminated \${ in '$1'"
                    return 1
                    ;;
                esac
            else
                while [ -n "$rest" ]; do
                    c="${rest:0:1}"
                    case "$c" in
                    [A-Za-z0-9_]) name="$name$c" && rest="${rest:1}" ;;
                    *) break ;;
                    esac
                done
            fi
            if [ -z "$name" ]; then
                echo "PARSE ERROR: a bare \$ in '$1'"
                return 1
            fi
            if [ -z "${EXP_VARS[$name]+set}" ]; then
                echo "PARSE ERROR: cannot expand \$$name in '$1' — no variable, matrix value or predefined slug defines it"
                return 1
            fi
            out="$out${EXP_VARS[$name]}"
            ;;
        *)
            out="$out$rest"
            rest=""
            ;;
        esac
    done
    CP_EXPANDED="$out"
}

# The helpers below are called from inside cp_parse and read its locals
# ($top, $have_pend, $pend_key, $pend_policy) through bash's dynamic scope.
cp_add_entry() { # cp_add_entry <raw key> <policy>
    case "$2" in
    '' | pull | push | pull-push) ;;
    *)
        echo "PARSE ERROR: unknown cache policy '$2' for key '$1'"
        return 1
        ;;
    esac
    BLK_CACHE[$top]="${BLK_CACHE[$top]:-}$1"$'\t'"$2"$'\n'
}

cp_flush_pending() {
    if [ "$have_pend" = 1 ]; then
        have_pend=0
        cp_add_entry "$pend_key" "$pend_policy" || return 1
    fi
    return 0
}

cp_add_alias() { # cp_add_alias <anchor>
    local anchor="$1" key policy merge
    key="${CP_KEY[$anchor]:-}"
    policy="${CP_POLICY[$anchor]:-}"
    merge="${CP_MERGE[$anchor]:-}"
    if [ -n "$merge" ]; then
        if [ -z "${CP_KEY[$merge]:-}${CP_POLICY[$merge]:-}" ]; then
            echo "PARSE ERROR: alias *$anchor merges *$merge, which defines no cache key"
            return 1
        fi
        if [ -n "${CP_MERGE[$merge]:-}" ]; then
            echo "PARSE ERROR: alias *$anchor merges *$merge, which itself merges — not resolved here"
            return 1
        fi
        [ -n "$key" ] || key="${CP_KEY[$merge]:-}"
        [ -n "$policy" ] || policy="${CP_POLICY[$merge]:-}"
    fi
    if [ -z "$key" ]; then
        echo "PARSE ERROR: alias *$anchor resolves to no cache key"
        return 1
    fi
    cp_add_entry "$key" "$policy"
}

cp_split_list() { # cp_split_list <"[a, b]" or scalar> — one value per line
    local v="$1" item
    case "$v" in
    \[*\])
        v="${v#\[}"
        v="${v%\]}"
        local IFS=,
        for item in $v; do
            item="${item#"${item%%[![:space:]]*}"}"
            item="${item%"${item##*[![:space:]]}"}"
            [ -n "$item" ] || continue
            cp_scalar "$item"
            printf '\n'
        done
        ;;
    *)
        cp_scalar "$v"
        printf '\n'
        ;;
    esac
}

# Non-job top-level keys. Everything else that does not start with '.' is a job.
cp_is_job() { # cp_is_job <top-level name>
    case "$1" in
    .*) return 1 ;;
    stages | variables | workflow | include | default | image | services | cache | before_script | after_script) return 1 ;;
    esac
    return 0
}

cp_parse() { # cp_parse <yaml>
    local yml="$1" line lineno=0 ws indent item field value name
    local top="" anchor="" section="" pend_key="" pend_policy="" have_pend=0
    while IFS= read -r line || [ -n "$line" ]; do
        lineno=$((lineno + 1))
        line="${line%$'\r'}"
        if [[ "$line" =~ ^[[:space:]]*(#.*)?$ ]]; then
            continue
        fi
        case "$line" in
        *$'\t'*)
            if [[ "$line" =~ ^$'\t' ]]; then
                echo "PARSE ERROR ($yml:$lineno): tab indentation"
                return 1
            fi
            ;;
        esac

        if [[ "$line" =~ ^[^[:space:]] ]]; then
            cp_flush_pending || return 1
            section=""
            anchor=""
            if [[ "$line" =~ ^([A-Za-z0-9._-]+):[[:space:]]*(\&([A-Za-z0-9_-]+)[[:space:]]*)?$ ]]; then
                top="${BASH_REMATCH[1]}"
                anchor="${BASH_REMATCH[3]:-}"
            elif [[ "$line" =~ ^([A-Za-z0-9._-]+):[[:space:]] ]]; then
                top="${BASH_REMATCH[1]}"
            else
                echo "PARSE ERROR ($yml:$lineno): unrecognised top-level line"
                return 1
            fi
            if [ -z "${BLK_SEEN[$top]:-}" ]; then
                BLK_SEEN[$top]=1
                BLK_ORDER+=("$top")
            fi
            continue
        fi

        ws="${line%%[! ]*}"
        indent=${#ws}
        case "$indent" in
        2)
            if [ "$top" = variables ]; then
                if [[ "$line" =~ ^[[:space:]]{2}([A-Za-z_][A-Za-z0-9_]*):[[:space:]]*(.*)$ ]]; then
                    GLOBAL_VARS[${BASH_REMATCH[1]}]="$(cp_scalar "${BASH_REMATCH[2]}")"
                fi
                continue
            fi
            cp_flush_pending || return 1
            if [[ "$line" =~ ^[[:space:]]{2}\<\<:[[:space:]]*\*([A-Za-z0-9_-]+) ]]; then
                [ -z "$anchor" ] || CP_MERGE[$anchor]="${BASH_REMATCH[1]}"
                continue
            fi
            if [[ ! "$line" =~ ^[[:space:]]{2}([A-Za-z_][A-Za-z0-9_]*):[[:space:]]*(.*)$ ]]; then
                section=""
                continue # a list item under an anchor block (.changes-*), etc.
            fi
            field="${BASH_REMATCH[1]}"
            value="$(cp_strip_comment "${BASH_REMATCH[2]}")"
            section=""
            case "$field" in
            cache)
                if [ -n "$value" ]; then
                    echo "PARSE ERROR ($yml:$lineno): inline cache value not understood"
                    return 1
                fi
                section=cache
                BLK_HAS_CACHE[$top]=1
                ;;
            key) [ -z "$anchor" ] || CP_KEY[$anchor]="$value" ;;
            policy) [ -z "$anchor" ] || CP_POLICY[$anchor]="$value" ;;
            variables) section=variables ;;
            parallel) section=parallel ;;
            extends)
                if [ -z "$value" ]; then
                    section=extends
                else
                    while IFS= read -r item; do
                        [ -n "$item" ] || continue
                        BLK_EXTENDS[$top]="${BLK_EXTENDS[$top]:-} $item"
                    done <<<"$(cp_split_list "$value")"
                fi
                ;;
            *) section=other ;;
            esac
            ;;
        4)
            case "$section" in
            cache)
                if [[ ! "$line" =~ ^[[:space:]]{4}-[[:space:]]*(.*)$ ]]; then
                    echo "PARSE ERROR ($yml:$lineno): unrecognised line inside a cache block"
                    return 1
                fi
                item="${BASH_REMATCH[1]}"
                cp_flush_pending || return 1
                if [[ "$item" =~ ^\*([A-Za-z0-9_-]+)[[:space:]]*$ ]]; then
                    cp_add_alias "${BASH_REMATCH[1]}" || return 1
                elif [[ "$item" =~ ^key:[[:space:]]*(.+)$ ]]; then
                    pend_key="$(cp_scalar "${BASH_REMATCH[1]}")"
                    pend_policy=""
                    have_pend=1
                else
                    echo "PARSE ERROR ($yml:$lineno): unrecognised cache entry '$item'"
                    return 1
                fi
                ;;
            variables)
                if [[ "$line" =~ ^[[:space:]]{4}([A-Za-z_][A-Za-z0-9_]*):[[:space:]]*(.*)$ ]]; then
                    BLK_VARS[$top]="${BLK_VARS[$top]:-}${BASH_REMATCH[1]}"$'\t'"$(cp_scalar "${BASH_REMATCH[2]}")"$'\n'
                fi
                ;;
            extends)
                if [[ "$line" =~ ^[[:space:]]{4}-[[:space:]]*(.+)$ ]]; then
                    BLK_EXTENDS[$top]="${BLK_EXTENDS[$top]:-} $(cp_scalar "${BASH_REMATCH[1]}")"
                fi
                ;;
            parallel)
                if [[ "$line" =~ ^[[:space:]]{4}matrix:[[:space:]]*$ ]]; then
                    section=matrix
                fi
                ;;
            esac
            ;;
        6)
            case "$section" in
            cache)
                if [[ ! "$line" =~ ^[[:space:]]{6}([A-Za-z_]+):[[:space:]]*(.*)$ ]]; then
                    echo "PARSE ERROR ($yml:$lineno): unrecognised line inside a cache entry"
                    return 1
                fi
                field="${BASH_REMATCH[1]}"
                value="$(cp_strip_comment "${BASH_REMATCH[2]}")"
                case "$field" in
                policy)
                    if [ "$have_pend" != 1 ]; then
                        echo "PARSE ERROR ($yml:$lineno): policy: outside a cache entry"
                        return 1
                    fi
                    pend_policy="$value"
                    ;;
                paths | when | untracked | unprotect | fallback_keys) ;;
                *)
                    echo "PARSE ERROR ($yml:$lineno): unrecognised cache field '$field'"
                    return 1
                    ;;
                esac
                ;;
            matrix)
                if [[ "$line" =~ ^[[:space:]]{6}-[[:space:]]*([A-Za-z_][A-Za-z0-9_]*):[[:space:]]*(.+)$ ]]; then
                    name="${BASH_REMATCH[1]}"
                    while IFS= read -r item; do
                        [ -n "$item" ] || continue
                        BLK_MATRIX[$top]="${BLK_MATRIX[$top]:-}$name"$'\t'"$item"$'\n'
                    done <<<"$(cp_split_list "$(cp_strip_comment "${BASH_REMATCH[2]}")")"
                else
                    echo "PARSE ERROR ($yml:$lineno): unrecognised matrix leg"
                    return 1
                fi
                ;;
            esac
            ;;
        8)
            if [ "$section" = matrix ]; then
                echo "PARSE ERROR ($yml:$lineno): a matrix leg over more than one variable is not expanded by this check"
                return 1
            fi
            ;;
        *)
            if [ "$section" = cache ]; then
                echo "PARSE ERROR ($yml:$lineno): unrecognised line inside a cache block"
                return 1
            fi
            ;;
        esac
    done <"$yml"
    cp_flush_pending || return 1
    return 0
}

# The effective cache of a job: its own `cache:` if it declares one (an array
# is replaced, not merged), otherwise the nearest one up the `extends` chain,
# last-listed parent first — GitLab's own precedence.
# NOTE: this one reports on STDERR. It is called inside a command substitution
# (its stdout IS the entry list), so an error printed on stdout would be
# captured as data and vanish — which is how a refusal message went missing
# once already.
cp_cache_of() { # cp_cache_of <name> <depth>
    local name="$1" depth="$2" parent out
    if [ "$depth" -gt 10 ]; then
        echo "PARSE ERROR: extends chain deeper than 10 at '$name'" >&2
        return 1
    fi
    if [ -n "${BLK_HAS_CACHE[$name]:-}" ]; then
        printf '%s' "${BLK_CACHE[$name]:-}"
        return 0
    fi
    local -a parents=()
    read -r -a parents <<<"${BLK_EXTENDS[$name]:-}"
    local i
    for ((i = ${#parents[@]} - 1; i >= 0; i--)); do
        parent="${parents[$i]}"
        if [ -z "${BLK_SEEN[$parent]:-}" ]; then
            echo "PARSE ERROR: '$name' extends '$parent', which is not defined in this file" >&2
            return 1
        fi
        out="$(cp_cache_of "$parent" "$((depth + 1))")" || return 1
        if [ -n "$out" ]; then
            printf '%s' "$out"
            return 0
        fi
    done
    return 0
}

# Variables merge the other way round: ancestors first, in order, then the
# job's own. Mutates EXP_VARS, so it must not run in a subshell.
cp_vars_of() { # cp_vars_of <name> <depth>
    local name="$1" depth="$2" parent k v
    if [ "$depth" -gt 10 ]; then
        echo "PARSE ERROR: extends chain deeper than 10 at '$name'"
        return 1
    fi
    local -a parents=()
    read -r -a parents <<<"${BLK_EXTENDS[$name]:-}"
    for parent in "${parents[@]}"; do
        if [ -z "${BLK_SEEN[$parent]:-}" ]; then
            echo "PARSE ERROR: '$name' extends '$parent', which is not defined in this file"
            return 1
        fi
        cp_vars_of "$parent" "$((depth + 1))" || return 1
    done
    while IFS=$'\t' read -r k v; do
        [ -n "$k" ] || continue
        EXP_VARS[$k]="$v"
    done <<<"${BLK_VARS[$name]:-}"
    return 0
}

cache_policy_report() { # cache_policy_report <yaml>
    local yml="$1"
    cp_reset
    cp_parse "$yml" || return 1

    if [ -n "${BLK_HAS_CACHE[default]:-}" ]; then
        echo "PARSE ERROR: a top-level 'default: cache:' applies to every job and is not resolved by this check"
        return 1
    fi

    local job entries rawkey policy key leg mvar mval leg_desc k jobs=0
    local -a legs=()
    for job in "${BLK_ORDER[@]}"; do
        cp_is_job "$job" || continue
        jobs=$((jobs + 1))
        entries="$(cp_cache_of "$job" 0)" || return 1
        [ -n "$entries" ] || continue

        unset EXP_VARS
        declare -gA EXP_VARS=()
        for k in "${!GLOBAL_VARS[@]}"; do
            EXP_VARS[$k]="${GLOBAL_VARS[$k]}"
        done
        cp_vars_of "$job" 0 || return 1
        EXP_VARS[CI_JOB_NAME_SLUG]="$(cp_slugify "$job")"

        legs=()
        if [ -n "${BLK_MATRIX[$job]:-}" ]; then
            while IFS=$'\t' read -r mvar mval; do
                [ -n "$mvar" ] || continue
                legs+=("$mvar"$'\t'"$mval")
            done <<<"${BLK_MATRIX[$job]}"
        fi
        [ "${#legs[@]}" -gt 0 ] || legs=("")

        for leg in "${legs[@]}"; do
            leg_desc=""
            if [ -n "$leg" ]; then
                mvar="${leg%%$'\t'*}"
                mval="${leg#*$'\t'}"
                EXP_VARS[$mvar]="$mval"
                leg_desc=" [$mvar=$mval]"
            fi
            while IFS=$'\t' read -r rawkey policy; do
                [ -n "$rawkey" ] || continue
                if [ -n "$leg" ] && [ "${rawkey#*CI_JOB_NAME_SLUG}" != "$rawkey" ]; then
                    echo "PARSE ERROR: job '$job' has a parallel matrix and a key on \$CI_JOB_NAME_SLUG; this check cannot reproduce the runner's slug for a matrix leg"
                    return 1
                fi
                cp_expand "$rawkey" || return 1
                key="$CP_EXPANDED"
                KEY_READERS[$key]=$((${KEY_READERS[$key]:-0} + 1))
                if [ "$policy" != pull ]; then
                    KEY_WRITERS[$key]=$((${KEY_WRITERS[$key]:-0} + 1))
                fi
                echo "JOB $job$leg_desc -> $key ${policy:-pull-push}"
            done <<<"$entries"
        done
    done

    if [ "$jobs" -eq 0 ]; then
        echo "ERROR: no jobs found in $yml — this check measured nothing"
        return 1
    fi
    if [ "${#KEY_READERS[@]}" -eq 0 ]; then
        echo "ERROR: no cache keys found in $yml — this check measured nothing"
        return 1
    fi
    local -a keys=()
    mapfile -t keys < <(printf '%s\n' "${!KEY_READERS[@]}" | sort)
    local violations=0
    for k in "${keys[@]}"; do
        echo "KEY $k readers=${KEY_READERS[$k]} writers=${KEY_WRITERS[$k]:-0}"
        if [ "${KEY_WRITERS[$k]:-0}" -eq 0 ]; then
            echo "VIOLATION: cache key '$k' has no pull-push writer — the sweep evicts it while jobs still read it"
            violations=$((violations + 1))
        fi
    done
    echo "checked ${#keys[@]} cache key(s) across $jobs job(s), $violations violation(s)"
    [ "$violations" -eq 0 ]
}

run_policy_check() { # run_policy_check <case> <yaml>
    sweep_log="$work/$1.log"
    sweep_rc=0
    (cache_policy_report "$2") >"$sweep_log" 2>&1 || sweep_rc=$?
}

run_policy_check P1 "$ci_yml"
assert_exit P1_every_expanded_cache_key_in_the_real_config_has_a_writer 0 "$sweep_rc" "$sweep_log" \
    "KEY cargo-home" \
    "KEY target-check readers=1 writers=1" \
    "KEY target-examples readers=1 writers=1" \
    "KEY luals-3.13.5" \
    "KEY pwsh-7.4.6" \
    "KEY fuzz-cargo-home" \
    "KEY fuzz-target-lua_parse" \
    "KEY fuzz-corpus-luacats" \
    ", 0 violation(s)" \
    "!VIOLATION" \
    '!$CI_JOB_NAME_SLUG' \
    '!$FUZZ_TARGET'
# The JOB lines are the part that round 2 proved was missing: the key is
# expanded per job, so `examples` reading cargo-home and WRITING its own target
# tree are two separate facts, and either can fail on its own.
assert_exit P1b_the_report_is_per_job_not_per_key_string 0 "$sweep_rc" "$sweep_log" \
    "JOB examples -> cargo-home pull" \
    "JOB examples -> target-examples pull-push" \
    "JOB check -> target-check pull-push" \
    "JOB fuzz [FUZZ_TARGET=luacats] -> fuzz-corpus-luacats pull-push" \
    "JOB luals-parity -> luals-3.13.5 pull-push"

fixtures="$work/yaml"
mkdir -p "$fixtures"

cat >"$fixtures/no-writer.yml" <<'YML'
.cache-thing: &cache-thing
  key: shared-thing
  paths:
    - thing

.cache-thing-ro: &cache-thing-ro
  <<: *cache-thing
  policy: pull

job-a:
  cache:
    - *cache-thing-ro
  script:
    - echo a

job-b:
  cache:
    - *cache-thing-ro
YML
run_policy_check P2 "$fixtures/no-writer.yml"
assert_exit P2_a_key_every_job_pulls_is_a_violation 1 "$sweep_rc" "$sweep_log" \
    "VIOLATION: cache key 'shared-thing' has no pull-push writer" \
    "KEY shared-thing readers=2 writers=0"

cat >"$fixtures/one-writer.yml" <<'YML'
.cache-thing: &cache-thing
  key: shared-thing
  paths:
    - thing

.cache-thing-ro: &cache-thing-ro
  <<: *cache-thing
  policy: pull

writer:
  cache:
    - *cache-thing
  script:
    - echo w

reader:
  cache:
    - *cache-thing-ro
YML
run_policy_check P3 "$fixtures/one-writer.yml"
assert_exit P3_one_writer_among_readers_satisfies_the_rule 0 "$sweep_rc" "$sweep_log" \
    "KEY shared-thing readers=2 writers=1" "!VIOLATION"

cat >"$fixtures/inline-pull.yml" <<'YML'
job-a:
  cache:
    - key: lonely
      paths:
        - x
      policy: pull
YML
run_policy_check P4 "$fixtures/inline-pull.yml"
assert_exit P4_an_inline_pull_only_key_is_a_violation 1 "$sweep_rc" "$sweep_log" \
    "VIOLATION: cache key 'lonely' has no pull-push writer"

cat >"$fixtures/unknown-shape.yml" <<'YML'
job-a:
  cache:
    - !reference [.tpl, cache]
YML
run_policy_check P5 "$fixtures/unknown-shape.yml"
assert_exit P5_an_unparsed_cache_shape_refuses 1 "$sweep_rc" "$sweep_log" \
    "PARSE ERROR" "unrecognised cache entry" "!VIOLATION"

cat >"$fixtures/unknown-policy.yml" <<'YML'
job-a:
  cache:
    - key: k
      policy: shove
YML
run_policy_check P6 "$fixtures/unknown-policy.yml"
assert_exit P6_an_unknown_policy_refuses 1 "$sweep_rc" "$sweep_log" \
    "PARSE ERROR: unknown cache policy 'shove'"

cat >"$fixtures/dangling-alias.yml" <<'YML'
job-a:
  cache:
    - *nothing-defines-this
YML
run_policy_check P7 "$fixtures/dangling-alias.yml"
assert_exit P7_a_dangling_alias_refuses 1 "$sweep_rc" "$sweep_log" \
    "PARSE ERROR: alias *nothing-defines-this resolves to no cache key"

cat >"$fixtures/no-cache.yml" <<'YML'
job-a:
  script:
    - echo a
YML
run_policy_check P8 "$fixtures/no-cache.yml"
assert_exit P8_a_config_with_no_cache_keys_refuses_to_pass 1 "$sweep_rc" "$sweep_log" \
    "ERROR: no cache keys found"

cat >"$fixtures/per-job-blind-spot.yml" <<'YML'
.build: &build-cache
  key: target-$CI_JOB_NAME_SLUG
  paths:
    - target

.rust:
  cache:
    - *build-cache

alpha:
  extends: .rust
  script:
    - echo alpha

beta:
  extends: .rust
  cache:
    - key: target-$CI_JOB_NAME_SLUG
      paths:
        - target
      policy: pull
YML
run_policy_check P9 "$fixtures/per-job-blind-spot.yml"
assert_exit P9_a_per_job_key_read_only_in_one_job_is_a_violation 1 "$sweep_rc" "$sweep_log" \
    "JOB alpha -> target-alpha pull-push" \
    "JOB beta -> target-beta pull" \
    "KEY target-alpha readers=1 writers=1" \
    "VIOLATION: cache key 'target-beta' has no pull-push writer"

cat >"$fixtures/matrix.yml" <<'YML'
variables:
  TOOL_VERSION: "1.2.3"

legs:
  parallel:
    matrix:
      - LEG: [one, two]
  cache:
    - key: corpus-$LEG
      paths:
        - corpus
    - key: tool-${TOOL_VERSION}
      paths:
        - .tool
YML
run_policy_check P10 "$fixtures/matrix.yml"
assert_exit P10_matrix_legs_and_global_variables_expand 0 "$sweep_rc" "$sweep_log" \
    "JOB legs [LEG=one] -> corpus-one pull-push" \
    "JOB legs [LEG=two] -> corpus-two pull-push" \
    "KEY tool-1.2.3 readers=2 writers=2" \
    "!VIOLATION"

cat >"$fixtures/unknown-variable.yml" <<'YML'
job-a:
  cache:
    - key: target-$NOBODY_DEFINES_THIS
      paths:
        - target
YML
run_policy_check P11 "$fixtures/unknown-variable.yml"
assert_exit P11_a_key_that_cannot_be_expanded_refuses 1 "$sweep_rc" "$sweep_log" \
    "PARSE ERROR: cannot expand \$NOBODY_DEFINES_THIS" "!VIOLATION"

cat >"$fixtures/matrix-slug.yml" <<'YML'
job-a:
  parallel:
    matrix:
      - LEG: [one, two]
  cache:
    - key: target-$CI_JOB_NAME_SLUG
      paths:
        - target
YML
run_policy_check P12 "$fixtures/matrix-slug.yml"
assert_exit P12_a_matrix_job_keyed_on_the_slug_refuses 1 "$sweep_rc" "$sweep_log" \
    "cannot reproduce the runner's slug for a matrix leg"

cat >"$fixtures/matrix-two-vars.yml" <<'YML'
job-a:
  parallel:
    matrix:
      - LEG: [one, two]
        OTHER: [x, y]
  cache:
    - key: k-$LEG-$OTHER
      paths:
        - target
YML
run_policy_check P13 "$fixtures/matrix-two-vars.yml"
assert_exit P13_a_multi_variable_matrix_leg_refuses 1 "$sweep_rc" "$sweep_log" \
    "a matrix leg over more than one variable is not expanded"

cat >"$fixtures/missing-template.yml" <<'YML'
job-a:
  extends: .not-defined-anywhere
  script:
    - echo a
YML
run_policy_check P14 "$fixtures/missing-template.yml"
assert_exit P14_an_extends_target_that_does_not_exist_refuses 1 "$sweep_rc" "$sweep_log" \
    "extends '.not-defined-anywhere', which is not defined"

cat >"$fixtures/default-cache.yml" <<'YML'
default:
  cache:
    - key: everything
      paths:
        - target

job-a:
  script:
    - echo a
YML
run_policy_check P15 "$fixtures/default-cache.yml"
assert_exit P15_a_top_level_default_cache_refuses 1 "$sweep_rc" "$sweep_log" \
    "top-level 'default: cache:' applies to every job"

cat >"$fixtures/templates-only.yml" <<'YML'
.tpl:
  cache:
    - key: k
      paths:
        - target
YML
run_policy_check P16 "$fixtures/templates-only.yml"
assert_exit P16_a_file_with_no_jobs_refuses_to_pass 1 "$sweep_rc" "$sweep_log" \
    "ERROR: no jobs found"

report_status=0
selftest_report "runner-cache-sweep-selftest" || report_status=$?
exit "$report_status"

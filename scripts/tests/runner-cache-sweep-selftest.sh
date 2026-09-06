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
#   du_total coerces empty output to 0 -> S9
#   numeric-slot assertion deleted    -> S10, S20
#   fuzz-corpus age exemption deleted -> S13, S14
#   busy-slot skip deleted            -> S5
#   docker-unavailable fallback = 0   -> S6, S6b
#   flock acquisition dropped         -> S15, S15b
#   parent-directory guard deleted    -> S16
#   cap loop stops subtracting        -> S11b, S12, S12b
#   cap drains strictly oldest-first  -> S12, S12b
#   failed freshness scan reads stale -> S21
# Every mutation above was rejected by at least one case, none by a case that
# was merely counting log lines. Re-run one with:
#   cp scripts/ops/runner-cache-sweep.sh /tmp/mutant.sh && $EDITOR /tmp/mutant.sh
#   RUNNER_CACHE_SWEEP_BIN=/tmp/mutant.sh bash scripts/tests/runner-cache-sweep-selftest.sh
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

chmod +x "$shim"/* "$work/shim-du/du" "$work/shim-find/find"

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
    sweep_rc=0
    env \
        PATH="$shim:$PATH" \
        SELFTEST_LOGGER_LOG="$sweep_syslog" \
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
assert_exit S9_du_failure_refuses 2 "$sweep_rc" "$sweep_log" \
    "REFUSED: du failed" \
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
    grep -qE '^net -[0-9]+ MiB; cache\+builds now [0-9]+ MiB$' "$sweep_log"
check_true "S18 the delta reaches syslog, not only stdout" \
    grep -qE 'net -[0-9]+ MiB' "$sweep_syslog"

fx="$(mkfixture s18b)"
mkfile "$fx/cache/flying-dice/luabox/cargo-home-protected/cache.zip" 512
mkfile "$fx/builds/zAbCd/0/flying-dice/luabox/.git/HEAD"
age_tree "$fx" 30
run_sweep s18b "$fx"
assert_exit S18b_a_run_that_removes_nothing_reports_no_change 0 "$sweep_rc" "$sweep_log" \
    "net +0 MiB"

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

echo
echo "=== Part 2 — every cache key keeps a pull-push writer ==="

# Finding 9. The sweep evicts on mtime, and GitLab touches a local cache
# archive only when a job WRITES it: `policy: pull` reads the zip out of the
# store without rewriting it. A key every job takes read-only therefore ages
# out under the window while jobs are still reading it, and the symptom is a
# job that silently rebuilds from cold, not an error. The rule this asserts is
# the pipeline half of the eviction contract: at least one writer per key.
#
# Read straight from the YAML rather than from `glab ci config compile`: the
# self-test runs in a build container with no glab, no token and no network,
# and a committed snapshot of the compiled config would go stale silently —
# the exact failure mode decisions/12 names. The cost is a parser, so it is
# written to REFUSE anything it does not understand (unknown cache entry
# shape, unknown policy, dangling or nested alias, no keys at all) rather than
# to skip it, and the fixtures below pin that refusal.
#
# Known limit, stated rather than hidden: occurrences are counted wherever a
# `cache:` block appears, including hidden templates like `.rust`. That is
# where the pipeline's one cargo-home writer is declared, so a template no job
# extends could in principle satisfy the rule on paper.

cp_reset() {
    unset CP_KEY CP_POLICY CP_MERGE CP_READERS CP_WRITERS
    declare -gA CP_KEY=() CP_POLICY=() CP_MERGE=() CP_READERS=() CP_WRITERS=()
}

cp_record() { # cp_record <key> <policy>
    local key="$1" policy="$2"
    case "$policy" in
    '' | pull | push | pull-push) ;;
    *)
        echo "PARSE ERROR: unknown cache policy '$policy' for key '$key'"
        return 1
        ;;
    esac
    CP_READERS[$key]=$((${CP_READERS[$key]:-0} + 1))
    if [ "$policy" != pull ]; then
        CP_WRITERS[$key]=$((${CP_WRITERS[$key]:-0} + 1))
    fi
}

cp_record_alias() { # cp_record_alias <anchor>
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
    cp_record "$key" "$policy"
}

cache_policy_report() { # cache_policy_report <yaml>
    local yml="$1" line item field value lineno=0
    local anchor="" in_cache=0 have_pend=0 pend_key="" pend_policy=""
    cp_reset
    while IFS= read -r line || [ -n "$line" ]; do
        lineno=$((lineno + 1))
        line="${line%$'\r'}"
        if [[ "$line" =~ ^[[:space:]]*(#.*)?$ ]]; then
            continue
        fi
        if [[ "$line" =~ ^[[:space:]]{8}-[[:space:]] ]]; then
            continue # a `paths:` entry of an inline cache item
        elif [[ "$line" =~ ^[[:space:]]{6}([A-Za-z_]+):[[:space:]]*(.*)$ ]]; then
            field="${BASH_REMATCH[1]}"
            value="${BASH_REMATCH[2]}"
            if [ "$in_cache" = 1 ]; then
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
            fi
            continue
        elif [[ "$line" =~ ^[[:space:]]{4}-[[:space:]]*(.*)$ ]]; then
            # Every match below clobbers BASH_REMATCH, so take the item text
            # out of it before running another one.
            item="${BASH_REMATCH[1]}"
            if [ "$in_cache" = 1 ]; then
                if [ "$have_pend" = 1 ]; then
                    cp_record "$pend_key" "$pend_policy" || return 1
                    have_pend=0
                fi
                if [[ "$item" =~ ^\*([A-Za-z0-9_-]+)[[:space:]]*$ ]]; then
                    cp_record_alias "${BASH_REMATCH[1]}" || return 1
                elif [[ "$item" =~ ^key:[[:space:]]*(.+)$ ]]; then
                    pend_key="${BASH_REMATCH[1]}"
                    pend_policy=""
                    have_pend=1
                else
                    echo "PARSE ERROR ($yml:$lineno): unrecognised cache entry '$item'"
                    return 1
                fi
            fi
            continue
        elif [[ "$line" =~ ^[[:space:]]{2}([A-Za-z_]+):[[:space:]]*(.*)$ ]]; then
            field="${BASH_REMATCH[1]}"
            value="${BASH_REMATCH[2]}"
            if [ "$have_pend" = 1 ]; then
                cp_record "$pend_key" "$pend_policy" || return 1
                have_pend=0
            fi
            in_cache=0
            case "$field" in
            cache)
                if [ -n "$value" ]; then
                    echo "PARSE ERROR ($yml:$lineno): inline cache value not understood"
                    return 1
                fi
                in_cache=1
                ;;
            key) [ -z "$anchor" ] || CP_KEY[$anchor]="$value" ;;
            policy) [ -z "$anchor" ] || CP_POLICY[$anchor]="$value" ;;
            esac
            continue
        elif [[ "$line" =~ ^[[:space:]]{2}\<\<:[[:space:]]*\*([A-Za-z0-9_-]+) ]]; then
            [ -z "$anchor" ] || CP_MERGE[$anchor]="${BASH_REMATCH[1]}"
            continue
        elif [[ "$line" =~ ^[^[:space:]] ]]; then
            if [ "$have_pend" = 1 ]; then
                cp_record "$pend_key" "$pend_policy" || return 1
                have_pend=0
            fi
            in_cache=0
            anchor=""
            if [[ "$line" =~ ^[A-Za-z0-9._-]+:[[:space:]]*\&([A-Za-z0-9_-]+)[[:space:]]*$ ]]; then
                anchor="${BASH_REMATCH[1]}"
            fi
            continue
        fi
        if [ "$in_cache" = 1 ]; then
            echo "PARSE ERROR ($yml:$lineno): unrecognised line inside a cache block"
            return 1
        fi
    done <"$yml"
    if [ "$have_pend" = 1 ]; then
        cp_record "$pend_key" "$pend_policy" || return 1
    fi

    if [ "${#CP_READERS[@]}" -eq 0 ]; then
        echo "ERROR: no cache keys found in $yml — this check measured nothing"
        return 1
    fi
    local -a keys=()
    mapfile -t keys < <(printf '%s\n' "${!CP_READERS[@]}" | sort)
    local k violations=0
    for k in "${keys[@]}"; do
        echo "KEY $k readers=${CP_READERS[$k]} writers=${CP_WRITERS[$k]:-0}"
        if [ "${CP_WRITERS[$k]:-0}" -eq 0 ]; then
            echo "VIOLATION: cache key '$k' has no pull-push writer — the sweep evicts it while jobs still read it"
            violations=$((violations + 1))
        fi
    done
    echo "checked ${#keys[@]} cache key(s), $violations violation(s)"
    [ "$violations" -eq 0 ]
}

run_policy_check() { # run_policy_check <case> <yaml>
    sweep_log="$work/$1.log"
    sweep_rc=0
    (cache_policy_report "$2") >"$sweep_log" 2>&1 || sweep_rc=$?
}

run_policy_check P1 "$ci_yml"
assert_exit P1_every_cache_key_in_the_real_config_has_a_writer 0 "$sweep_rc" "$sweep_log" \
    "KEY cargo-home" \
    'KEY target-$CI_JOB_NAME_SLUG' \
    'KEY luals-$LUALS_VERSION' \
    'KEY pwsh-$PWSH_VERSION' \
    "KEY fuzz-cargo-home" \
    'KEY fuzz-target-$FUZZ_TARGET' \
    'KEY fuzz-corpus-$FUZZ_TARGET' \
    "!VIOLATION"

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

report_status=0
selftest_report "runner-cache-sweep-selftest" || report_status=$?
exit "$report_status"

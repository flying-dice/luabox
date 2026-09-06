#!/bin/bash
# ===========================================================================
# runner-cache-sweep — eviction for the GitLab runner's host-bound cache and
# build directories (config.toml [runners.docker] volumes binds on /mnt/cache).
#
# WHY IT EXISTS. GitLab's local cache never deletes anything: every key writes
# an archive and leaves the previous ones. Every key in .gitlab-ci.yml is fixed
# (decision 15), so the steady state is bounded — but a key that falls out of
# use (a renamed job, a bumped `luals` pin, an abandoned fuzz target) is
# collected by nothing. This is that collector, plus a size backstop.
#
# WHAT IT DELETES (owner's rule, 2026-09-06):
#   <cache>   archives untouched for MAX_AGE_MIN (default 24 h); then, as a
#             backstop, the oldest archives until the tree is under
#             CACHE_CAP_GIB.
#   <builds>  checkouts (<token>/<slot>/<namespace>/<project>) in which no file
#             changed inside the window.
#
# ON-DISK SHAPE. The runner stores one archive per key at
#   <cache>/<namespace>/<project>/<key>-protected/cache.zip
# (`-non_protected` on unprotected refs). The suffix is part of the DIRECTORY
# name, so one cache key is up to TWO archives on disk, one per ref class —
# job 29041's trace says `Checking cache for cargo-home-protected`. The unit
# this script deletes is therefore the FILE, and the key it belongs to is the
# name of its PARENT DIRECTORY; that is how the exemption below is matched.
#
# EXEMPTION. `fuzz-corpus-*` archives are exempt from the age rule. That corpus
# is accumulated state, not a rebuildable tree, and this instance has NO
# pipeline schedule (checked 2026-09-06: `pipeline_schedules` returns an empty
# list), so `fuzz` runs only on an MR touching its `changes:` paths or on a
# manual run — days apart. A 24 h rule would hand the fuzzer an empty corpus
# every time. The exemption is from the AGE rule only: corpus archives are
# still evicted by the size cap, and are drained LAST there, after every
# rebuildable archive.
#
# SAFETY. This is `rm -rf` running from cron against directories a live runner
# is writing into. Each guard below is load-bearing and is pinned by
# scripts/tests/runner-cache-sweep-selftest.sh:
#   - Refuses (`logger -p user.err`, exit 2 or 3 per the exit contract below)
#     when CACHE_DIR or BUILDS_DIR is not a directory, when ANY `du` or `find`
#     fails — the empty-directory pass included, which used to be the one
#     exception — or when <builds> is not the depth-4 layout it assumes. A gone
#     bind mount used to look exactly like a clean sweep: `du` printed nothing,
#     empty was coerced to 0, and the log line read `freed 0 MiB` in a healthy
#     green.
#   - Refuses when the cache tree is STILL over the cap once the drain ends,
#     for any reason: nothing matched the archive glob (`cache.zst` after a
#     runner upgrade), the candidates vanished, a removal failed. The bound is
#     the thing that matters, so the alarm is on the outcome rather than on one
#     of its causes.
#   - Reports every failed `rm` at `user.err` by path, counts them, and exits
#     non-zero at the end of the run. They used to reach stderr only, which
#     cron sends to /dev/null.
#   - `flock -n`: the hourly cron run and a manual run never overlap.
#   - Skips any slot whose job container is running. The runner names them
#     `runner-<token>-project-<id>-concurrent-<slot>-<hash>-build`, so the slot
#     is read out of `docker ps`. If docker is unavailable or `docker ps`
#     fails, EVERY slot is treated as busy and no checkout is removed — the
#     one direction of that failure that cannot delete a live checkout.
#   - Re-checks freshness immediately before every `rm`, and re-reads slot
#     occupancy per checkout as the last thing before `rm -rf`. The full-tree
#     scan that decides a checkout is stale takes seconds on a multi-GB target
#     dir, and a job can start inside that window (reproduced: a checkout
#     deleted while a job was writing `.git/index.lock`). The freshness probe
#     alone cannot see a job that has started but not yet written; the
#     occupancy re-probe can, because the runner marks the slot busy for the
#     whole job. A re-probe that cannot answer counts as busy.
#   - Never deletes an archive whose key directory was touched inside the
#     window (the runner creates that directory before writing the archive),
#     and never deletes an empty directory younger than the window.
#
# ASSUMPTIONS, asserted rather than hoped for: <builds>/<token>/<slot> with a
# NUMERIC slot; checkouts two levels below a slot (<namespace>/<project>). A
# nested subgroup would put the checkout deeper, and this sweeps the subgroup
# directory instead — coarser, still guarded by the same freshness checks.
#
# Every deletion is logged by PATH, not just counted: a sweep that ate
# something it should not have is otherwise unreconstructable an hour later.
#
# Env overrides (CACHE_DIR, BUILDS_DIR, LOCK_FILE, MAX_AGE_MIN, CACHE_CAP_GIB,
# CACHE_CAP_KIB, DOCKER_PS) exist for the self-test; the host runs it with none
# of them set.
# Invoked as `/bin/bash <path>` because it lives on vfat /boot, which cannot
# carry an exec bit. Runs hourly from
# /boot/config/plugins/dynamix/docker-runner-cache-sweep.cron; the channel is
# syslog (`logger`), because cron discards stdout and stderr.
#
#   exit 0 — swept, or skipped because another sweep holds the lock
#   exit 2 — REFUSED: the failure happened before the first deletion, so the
#            trees are exactly as this run found them
#   exit 3 — PARTIALLY SWEPT: the failure happened after at least one deletion.
#            The trees have changed; every removal is in syslog by path, and
#            the message says how many and what went wrong. This is the case
#            "exit 2, nothing was deleted" used to claim, wrongly, whenever a
#            `du` or `find` failed mid-run.
# ===========================================================================
set -euo pipefail
shopt -s nullglob dotglob

LOG_TAG=runner-cache-sweep
CACHE_DIR="${CACHE_DIR:-/mnt/cache/appdata/gitlab-runner/cache}"
BUILDS_DIR="${BUILDS_DIR:-/mnt/cache/appdata/gitlab-runner/builds}"
LOCK_FILE="${LOCK_FILE:-/var/run/runner-cache-sweep.lock}"
MAX_AGE_MIN="${MAX_AGE_MIN:-1440}"
CACHE_CAP_GIB="${CACHE_CAP_GIB:-200}"
# The same cap expressed in KiB. Unset on the host; it is the self-test's
# seam, so the drain loop's arithmetic can be exercised against kilobyte
# fixtures instead of two hundred gigabytes of them.
CACHE_CAP_KIB="${CACHE_CAP_KIB:-}"
# A command printing one running container name per line. Unset on the host,
# where `docker ps` is used directly.
DOCKER_PS="${DOCKER_PS:-}"

# syslog is the channel that survives cron; stdout/stderr are for a human
# running this by hand and for the self-test. `logger` failing (absent, or no
# /dev/log) must never take the sweep down with it.
log() {
    logger -t "$LOG_TAG" -- "$1" 2>/dev/null || true
    printf '%s\n' "$1"
}
err() {
    logger -p user.err -t "$LOG_TAG" -- "$1" 2>/dev/null || true
    printf '%s\n' "$1" >&2
}
# Nothing has been deleted yet -> exit 2, and the tree is exactly as the sweep
# found it. Something has -> exit 3, because "refused, nothing deleted" would
# be a lie and an operator reading it would look in the wrong place. Every
# deletion is already logged by path above the failure.
deletions=0
# A failed `rm` used to reach stderr only, which cron sends to /dev/null: the
# sweep reported success while the disk kept filling. Counted here, reported at
# user.err by path, and fatal at the end of the run.
failures=0
die() {
    if [ "$deletions" -gt 0 ]; then
        err "PARTIALLY SWEPT after $deletions removal(s): $1"
        exit 3
    fi
    err "REFUSED: $1"
    exit 2
}

# du, with every "measured nothing" path turned into a failure. `du -sm` on a
# vanished mount prints nothing and exits nonzero; summing that to 0 is the
# defect this wrapper exists to make impossible.
du_total() { # du_total <-m|-k> <path>...
    local unit="$1" out total
    shift
    out="$(du -s "$unit" -- "$@" 2>/dev/null)" || return 1
    [ -n "$out" ] || return 1
    total="$(printf '%s\n' "$out" | awk '{ s += $1; n += 1 } END { if (n == 0) exit 1; print s }')" || return 1
    [ -n "$total" ] || return 1
    printf '%s\n' "$total"
}

# Sizes are reported in the unit that makes them legible. `~0 MiB` is what the
# cap step printed while sitting over a 650 KiB cap — a number that reads as
# "nothing here" when the truth was "over the limit and unable to act".
human_kib() { # human_kib <kib>
    if [ "$1" -ge 1024 ]; then
        printf '%s MiB' "$(($1 / 1024))"
    else
        printf '%s KiB' "$1"
    fi
}

signed_kib() { # signed_kib <delta-kib>
    if [ "$1" -lt 0 ]; then
        printf -- '-%s' "$(human_kib "$((0 - $1))")"
    else
        printf '+%s' "$(human_kib "$1")"
    fi
}

# True when <path> must NOT be deleted: something under it changed inside the
# window, OR the scan itself failed. A scan that errored is not evidence of
# staleness, and the safe reading of "I do not know" is "leave it alone".
# `-quit` stops at the first hit, so a live tree costs one hit, not a walk.
must_keep() { # must_keep <path>
    local out
    out="$(find "$1" -mmin -"$MAX_AGE_MIN" -print -quit 2>/dev/null)" || return 0
    [ -n "$out" ]
}

# ---------------------------------------------------------------------------
# Preconditions. Everything that can refuse, refuses BEFORE anything is
# deleted — decision 15's "check this first" applied to the sweep itself.
# ---------------------------------------------------------------------------
if ! command -v flock >/dev/null 2>&1; then
    die "flock not found; refusing to sweep without a lock"
fi
if ! : >>"$LOCK_FILE" 2>/dev/null; then
    die "cannot open the lock file: $LOCK_FILE"
fi
exec 9>>"$LOCK_FILE"
if ! flock -n 9; then
    log "skipped: another sweep holds $LOCK_FILE"
    exit 0
fi

[ -d "$CACHE_DIR" ] || die "CACHE_DIR is not a directory: $CACHE_DIR (bind mount gone?)"
[ -d "$BUILDS_DIR" ] || die "BUILDS_DIR is not a directory: $BUILDS_DIR (bind mount gone?)"

work="$(mktemp -d)" || die "cannot create a work directory"
trap 'rm -rf -- "$work"' EXIT

# <builds>/<token>/<slot>/... — every entry under a token directory must be a
# numeric slot. The pre-bind layout was <builds>/<namespace>/<project>, whose
# trees this sweep would silently never reach: it reported nothing and evicted
# nothing while the disk filled.
assert_builds_layout() {
    local token slot name
    for token in "$BUILDS_DIR"/*; do
        if [ ! -d "$token" ]; then
            die "builds layout: expected runner-token directories under $BUILDS_DIR, found: $token"
        fi
        for slot in "$token"/*; do
            if [ ! -d "$slot" ]; then
                die "builds layout: expected <token>/<slot> directories, found a non-directory: $slot"
            fi
            name="${slot##*/}"
            case "$name" in
            '' | *[!0-9]*)
                die "builds layout: expected a numeric concurrency slot under $token, found: $slot"
                ;;
            esac
        done
    done
}
assert_builds_layout

before="$(du_total -k "$CACHE_DIR" "$BUILDS_DIR")" ||
    die "du failed for $CACHE_DIR / $BUILDS_DIR"

# ---------------------------------------------------------------------------
# Which concurrency slots are running a job right now.
# ---------------------------------------------------------------------------
busy_slots=""
all_slots_busy=0

# probe_busy_slots <announce 0|1> — re-reads slot occupancy from scratch. It
# is called once at the top of the run (announce=1, so the picture is in the
# log) and again immediately before every `rm -rf` (announce=0, or an hourly
# sweep would repeat the same two lines once per checkout).
probe_busy_slots() {
    local announce="$1"
    local names name
    busy_slots=""
    all_slots_busy=0
    if [ -n "$DOCKER_PS" ]; then
        if ! names="$("$DOCKER_PS" 2>/dev/null)"; then
            if [ "$announce" = 1 ]; then
                log "builds: DOCKER_PS ($DOCKER_PS) failed — treating every slot as busy"
            fi
            all_slots_busy=1
            return 0
        fi
    elif ! command -v docker >/dev/null 2>&1; then
        if [ "$announce" = 1 ]; then
            log "builds: docker not found — treating every slot as busy"
        fi
        all_slots_busy=1
        return 0
    elif ! names="$(docker ps --format '{{.Names}}' 2>/dev/null)"; then
        if [ "$announce" = 1 ]; then
            log "builds: 'docker ps' failed — treating every slot as busy"
        fi
        all_slots_busy=1
        return 0
    fi
    # runner-<token>-project-<id>-concurrent-<slot>-<hash>-build
    local rest slot
    local -a seen=()
    while IFS= read -r name; do
        [ -n "$name" ] || continue
        case "$name" in
        *-concurrent-*)
            rest="${name#*-concurrent-}"
            slot="${rest%%-*}"
            case "$slot" in
            '' | *[!0-9]*) continue ;;
            esac
            busy_slots="${busy_slots} ${slot} "
            seen+=("$slot")
            ;;
        esac
    done <<<"$names"
    if [ "$announce" = 1 ] && [ "${#seen[@]}" -gt 0 ]; then
        log "builds: slot(s) ${seen[*]} busy"
    fi
    return 0
}

slot_is_busy() { # slot_is_busy <slot>
    if [ "$all_slots_busy" = 1 ]; then
        return 0
    fi
    case "$busy_slots" in
    *" $1 "*) return 0 ;;
    esac
    return 1
}

probe_busy_slots 1

# ---------------------------------------------------------------------------
# 1. cache archives untouched for MAX_AGE_MIN
# ---------------------------------------------------------------------------
sweep_cache_age() {
    local list="$work/cache-age" f parent key
    local removed=0 exempt=0 skipped=0
    local -a candidates=()
    if ! find "$CACHE_DIR" -type f -name '*.zip' -mmin +"$MAX_AGE_MIN" -print0 >"$list"; then
        die "find failed under $CACHE_DIR"
    fi
    mapfile -d '' -t candidates <"$list"
    for f in "${candidates[@]}"; do
        parent="$(dirname -- "$f")"
        key="${parent##*/}"
        case "$key" in
        fuzz-corpus-*)
            exempt=$((exempt + 1))
            continue
            ;;
        esac
        # The runner touches the key directory when it writes a new archive
        # into it, so a fresh directory means a job is mid-upload.
        if must_keep "$parent"; then
            skipped=$((skipped + 1))
            continue
        fi
        # Re-stat the archive itself immediately before removing it.
        if [ -z "$(find "$f" -maxdepth 0 -mmin +"$MAX_AGE_MIN" -print -quit 2>/dev/null)" ]; then
            skipped=$((skipped + 1))
            continue
        fi
        if rm -f -- "$f"; then
            removed=$((removed + 1))
            deletions=$((deletions + 1))
            log "cache: age removed $f"
        else
            failures=$((failures + 1))
            err "cache: FAILED to remove $f"
        fi
    done
    log "cache: age summary: removed $removed, exempt $exempt, in-use $skipped (window ${MAX_AGE_MIN}min)"
}

# ---------------------------------------------------------------------------
# 2. size cap, oldest first. Rebuildable archives drain before fuzz corpora.
# ---------------------------------------------------------------------------
sweep_cache_cap() {
    local list="$work/cache-cap" row f parent key fkb
    local cap_kb cap_label
    if [ -n "$CACHE_CAP_KIB" ]; then
        cap_kb="$CACHE_CAP_KIB"
        cap_label="${CACHE_CAP_KIB}KiB"
    else
        cap_kb=$((CACHE_CAP_GIB * 1024 * 1024))
        cap_label="${CACHE_CAP_GIB}GiB"
    fi
    local size_kb removed=0
    local -a rows=() plain=() corpus=() ordered=()
    size_kb="$(du_total -k "$CACHE_DIR")" || die "du failed for $CACHE_DIR"
    if ! find "$CACHE_DIR" -type f -name '*.zip' -printf '%T@\t%p\0' >"$list"; then
        die "find failed under $CACHE_DIR"
    fi
    sort -z -n "$list" >"$list.sorted" || die "sort failed for $list"
    mapfile -d '' -t rows <"$list.sorted"
    for row in "${rows[@]}"; do
        f="${row#*$'\t'}"
        parent="$(dirname -- "$f")"
        key="${parent##*/}"
        case "$key" in
        fuzz-corpus-*) corpus+=("$f") ;;
        *) plain+=("$f") ;;
        esac
    done
    ordered=("${plain[@]}" "${corpus[@]}")
    for f in "${ordered[@]}"; do
        [ "$size_kb" -gt "$cap_kb" ] || break
        # Deleted by the age pass or by a runner between the scan and here.
        [ -e "$f" ] || continue
        fkb="$(du_total -k "$f")" || die "du failed for $f"
        if rm -f -- "$f"; then
            size_kb=$((size_kb - fkb))
            removed=$((removed + 1))
            deletions=$((deletions + 1))
            log "cache: cap removed $f"
        else
            failures=$((failures + 1))
            err "cache: FAILED to remove $f"
        fi
    done
    # Logged on every run, including the runs where it removes nothing: a cap
    # step that only speaks when it acts is one nobody can see is wired up.
    # The size is an estimate — the measurement is taken once and each removed
    # file subtracted from it, rather than re-walking the whole tree per
    # deletion; the honest number is in the net line at the end.
    log "cache: cap summary: removed $removed, tree now ~$(human_kib "$size_kb") (cap $cap_label)"
    # The alarm is on the OUTCOME, not on one of its causes. Whatever the
    # reason — nothing matched the archive glob, the candidates vanished under
    # us, a removal failed — a tree still over the cap when the drain ends
    # means the sweep cannot enforce the only bound this disk has, and saying
    # `removed 1, tree now ~684 KiB` and exiting 0 is #85's original signature:
    # a green run over a full disk. The empty-candidate case keeps its own
    # reason text because it names the likeliest cause (`cache.zst` after a
    # runner upgrade, a changed cache layout).
    if [ "$size_kb" -gt "$cap_kb" ]; then
        if [ "${#ordered[@]}" -eq 0 ]; then
            die "cache is $(human_kib "$size_kb"), over the $cap_label cap, and no '*.zip' archive matched under $CACHE_DIR — check the runner's archive naming/layout"
        fi
        die "cache is $(human_kib "$size_kb"), over the $cap_label cap, after draining $removed of ${#ordered[@]} archive(s) — the rest could not be removed or no longer exist"
    fi
}

# Empty directories older than the window. An empty directory a runner created
# seconds ago is a job about to clone into it, so the window applies here too.
#
# `-delete` BEFORE `-print0`, deliberately: find only runs the print when the
# delete returned true, so the count is directories actually removed. The other
# order counts directories it failed to remove, which is the count feeding the
# exit contract.
#
# A failure here dies like every other `find` failure. It used to log at info
# and carry on, which made the header's "refuses when `du` or `find` fails" a
# claim with an exception in it — and the one exception was the pass that
# removes things.
sweep_empty_dirs() { # sweep_empty_dirs <root> <label>
    local root="$1" label="$2" list="$work/empty-dirs"
    local -a gone=()
    if ! find "$root" -mindepth 1 -type d -empty -mmin +"$MAX_AGE_MIN" -delete -print0 >"$list"; then
        # Count what it managed before the failure, so the exit code is right.
        if [ -s "$list" ]; then
            mapfile -d '' -t gone <"$list"
            deletions=$((deletions + ${#gone[@]}))
        fi
        die "empty-directory sweep failed under $root"
    fi
    [ -s "$list" ] || return 0
    mapfile -d '' -t gone <"$list"
    deletions=$((deletions + ${#gone[@]}))
    log "$label: removed ${#gone[@]} empty directory(ies)"
}

# ---------------------------------------------------------------------------
# 3. build checkouts: <builds>/<token>/<slot>/<namespace>/<project>
# ---------------------------------------------------------------------------
sweep_builds() {
    local list="$work/builds" token slot name d
    local removed=0 busy=0 in_use=0 busy_late=0
    local -a checkouts=()
    for token in "$BUILDS_DIR"/*; do
        for slot in "$token"/*; do
            name="${slot##*/}"
            if slot_is_busy "$name"; then
                busy=$((busy + 1))
                continue
            fi
            if ! find "$slot" -mindepth 2 -maxdepth 2 -type d -print0 >"$list"; then
                die "find failed under $slot"
            fi
            checkouts=()
            mapfile -d '' -t checkouts <"$list"
            for d in "${checkouts[@]}"; do
                # A checkout is live if ANY file in it changed inside the
                # window. This walk is the expensive part of the run, so it
                # happens once; the occupancy re-probe below is what makes the
                # decision current, and it costs milliseconds.
                if must_keep "$d"; then
                    in_use=$((in_use + 1))
                    continue
                fi
                # Last thing before the delete: re-read slot occupancy. The
                # run-level probe is minutes old by the time a multi-GB tree
                # finishes scanning, and the runner marks a slot busy for the
                # WHOLE job — so this catches a job that started during the
                # scan even before it has written a file the freshness probes
                # above could see. A probe that cannot answer (docker gone,
                # daemon down) counts as busy, exactly as at run level.
                probe_busy_slots 0
                if slot_is_busy "$name"; then
                    busy_late=$((busy_late + 1))
                    if [ "$all_slots_busy" = 1 ]; then
                        log "builds: slot-occupancy re-probe unavailable — kept $d"
                    else
                        log "builds: slot $name became busy during the sweep — kept $d"
                    fi
                    continue
                fi
                if rm -rf -- "$d"; then
                    removed=$((removed + 1))
                    deletions=$((deletions + 1))
                    log "builds: removed $d"
                else
                    failures=$((failures + 1))
                    err "builds: FAILED to remove $d"
                fi
            done
            # Empty directories, but only ones older than the window: an empty
            # directory a runner created seconds ago is a job about to clone
            # into it.
            sweep_empty_dirs "$slot" builds
        done
    done
    log "builds: summary: removed $removed, busy slots $busy, in-use $in_use, busy on re-probe $busy_late (window ${MAX_AGE_MIN}min)"
}

sweep_cache_age
sweep_cache_cap
sweep_empty_dirs "$CACHE_DIR" cache
sweep_builds

after="$(du_total -k "$CACHE_DIR" "$BUILDS_DIR")" ||
    die "du failed for $CACHE_DIR / $BUILDS_DIR"
# Signed, because this number can legitimately be positive: a job writing
# during the sweep grows the trees, and "freed -1200 MiB" was a lie in both
# directions.
printf -v summary 'net %s; cache+builds now %s' \
    "$(signed_kib "$((after - before))")" "$(human_kib "$after")"
log "$summary"

# Last, so the measurement above is in syslog either way: a run that could not
# remove something it decided to remove has not done its job, whatever the
# totals say.
if [ "$failures" -gt 0 ]; then
    die "$failures removal(s) failed — see the FAILED lines above"
fi

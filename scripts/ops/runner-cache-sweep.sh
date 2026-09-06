#!/bin/bash
# Eviction for the GitLab runner's host-bound cache and build directories
# (config.toml [runners.docker] volumes binds on /mnt/cache). GitLab's local
# cache never deletes: every key writes a zip and leaves the previous one.
# Owner's rule (2026-09-06): anything untouched for 24h is deleted. A 60 GiB
# cap on /cache is the backstop, oldest first. Runs hourly from
# /boot/config/plugins/dynamix/docker-runner-cache-sweep.cron.
set -u
LOG_TAG="runner-cache-sweep"
log() { logger -t "$LOG_TAG" "$1"; echo "$1"; }
CACHE_DIR=/mnt/cache/appdata/gitlab-runner/cache
BUILDS_DIR=/mnt/cache/appdata/gitlab-runner/builds
MAX_AGE_MIN=1440
CACHE_CAP_GIB=60

before=$(du -sm "$CACHE_DIR" "$BUILDS_DIR" 2>/dev/null | awk '{s+=$1} END {print s+0}')

# 1. cache archives untouched for MAX_AGE_MIN
n=$(find "$CACHE_DIR" -type f -name '*.zip' -mmin +"$MAX_AGE_MIN" -print -delete 2>/dev/null | wc -l)
log "cache: removed $n archive(s) untouched for ${MAX_AGE_MIN}min"
find "$CACHE_DIR" -mindepth 1 -type d -empty -delete 2>/dev/null

# 2. size cap: oldest first until under CACHE_CAP_GIB
cap_kb=$((CACHE_CAP_GIB * 1024 * 1024)); removed=0
while [ "$(du -sk "$CACHE_DIR" 2>/dev/null | cut -f1)" -gt "$cap_kb" ]; do
  oldest=$(find "$CACHE_DIR" -type f -name '*.zip' -printf '%T@ %p\n' 2>/dev/null | sort -n | head -1 | cut -d' ' -f2-)
  [ -z "$oldest" ] && break
  rm -f -- "$oldest" && removed=$((removed+1))
done
[ "$removed" -gt 0 ] && log "cache: removed $removed archive(s) to stay under ${CACHE_CAP_GIB}GiB"

# 3. build checkouts: <builds>/<runner-short-token>/<slot>/<namespace>/<project>
#    A checkout is live if ANY file in it changed inside the window.
n=0
while IFS= read -r d; do
  if [ -z "$(find "$d" -mmin -"$MAX_AGE_MIN" -print -quit 2>/dev/null)" ]; then
    rm -rf -- "$d" && n=$((n+1))
  fi
done < <(find "$BUILDS_DIR" -mindepth 4 -maxdepth 4 -type d 2>/dev/null)
log "builds: removed $n checkout(s) untouched for ${MAX_AGE_MIN}min"
find "$BUILDS_DIR" -mindepth 1 -type d -empty -delete 2>/dev/null

after=$(du -sm "$CACHE_DIR" "$BUILDS_DIR" 2>/dev/null | awk '{s+=$1} END {print s+0}')
log "freed $((before - after)) MiB; cache+builds now ${after} MiB"

#!/usr/bin/env bash
# Exercise scripts/install.sh's draft-release path against a mock GitHub API.
#
# The draft path (LUABOX_DRAFT_INSTALL=1) had nothing standing behind it: it
# needs a draft release, a token that can see it, and asset endpoints that
# redirect to storage, so the first thing that ever ran it was a real release —
# the most expensive place to find a bug in it. This runs the *real* installer,
# unmodified, against scripts/tests/mock-release-api.py.
#
# What each leg proves:
#   1. curl, happy path      — the paginated release walk reaches page 2, the
#                              asset-id redirect is followed WITHOUT forwarding
#                              Authorization (the mock's storage answers 400 if
#                              it ever sees one), the checksum verifies, and the
#                              installed binary runs.
#   2. wget, happy path      — the same, with curl hidden from PATH. wget
#                              forwards --header across redirects, so this is
#                              the leg that proves the hand-rolled redirect hop
#                              in install.sh actually drops the token.
#   3. unknown tag           — the walk exhausts the listing and the install
#                              fails, rather than installing nothing quietly.
#   4. no jq                 — the opt-in refuses immediately, naming jq.
#
# Usage: scripts/tests/draft-install-mock.sh
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo_root"

TAG="v9.9.9"

for tool in python3 jq tar; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "draft-install-mock: SKIP — no $tool on PATH" >&2
        exit 0
    fi
done

work="$(mktemp -d)"
mock_pid=""
cleanup() {
    [ -n "$mock_pid" ] && kill "$mock_pid" 2>/dev/null || true
    rm -rf "$work"
}
trap cleanup EXIT

# ── The release's assets ─────────────────────────────────────────────────────
# `luabox-<target>.tar.gz` holding an executable named exactly `luabox` (what
# install.sh moves into place), plus the SHA256SUMS it verifies against. The
# fake binary is a shell script: the installer only chmods and moves it, and
# the assertion below only runs `--version`.
target="$(uname -s)-$(uname -m)"
case "$(uname -s)" in
    Linux) target="$(uname -m)-unknown-linux-gnu" ;;
    Darwin) target="aarch64-apple-darwin" ;;
    *) echo "draft-install-mock: SKIP — unsupported OS $(uname -s)" >&2; exit 0 ;;
esac
archive="luabox-${target}.tar.gz"

stage="$work/stage"
mkdir -p "$stage"
cat > "$stage/luabox" <<'FAKE'
#!/bin/sh
echo "luabox 9.9.9 (mock asset)"
FAKE
chmod +x "$stage/luabox"

assets="$work/assets"
mkdir -p "$assets"
tar -czf "$assets/$archive" -C "$stage" luabox
( cd "$assets" && sha256sum "$archive" > SHA256SUMS )

# ── The mock ────────────────────────────────────────────────────────────────
# It prints its base URL once both listeners are bound, so reading that line is
# the readiness handshake — no sleep-and-hope.
mock_out="$work/mock.out"
python3 scripts/tests/mock-release-api.py --tag "$TAG" --asset-dir "$assets" \
    > "$mock_out" 2>"$work/mock.err" &
mock_pid=$!

api_base=""
for _ in $(seq 1 100); do
    if [ -s "$mock_out" ]; then
        api_base="$(sed -n 's/^API_BASE=//p' "$mock_out" | head -1)"
        [ -n "$api_base" ] && break
    fi
    sleep 0.1
done
if [ -z "$api_base" ]; then
    echo "draft-install-mock: FAIL — the mock never reported a base URL" >&2
    cat "$work/mock.err" >&2
    exit 1
fi
echo "draft-install-mock: mock API at $api_base"

fail=0

# A PATH with curl removed, to force install.sh onto its wget fallback. A
# symlink farm rather than a wrapper: `command -v curl` has to genuinely find
# nothing, and shadowing cannot remove an entry.
wget_only_path="$work/wget-only"
mkdir -p "$wget_only_path"
for dir in /usr/local/bin /usr/bin /bin; do
    [ -d "$dir" ] || continue
    for tool in "$dir"/*; do
        name="$(basename "$tool")"
        [ "$name" = "curl" ] && continue
        [ -e "$wget_only_path/$name" ] || ln -s "$tool" "$wget_only_path/$name" 2>/dev/null || true
    done
done

# Run install.sh on the draft path and report. $1 names the leg, $2 the PATH to
# run under, $3 the version to ask for, $4 whether it must succeed (yes/no).
run_install() {
    local leg="$1" path="$2" version="$3" must_succeed="$4"
    local dir="$work/install-$leg"
    rm -rf "$dir"
    local log="$work/$leg.log"
    local status=0
    env -i \
        HOME="$work/home-$leg" \
        PATH="$path" \
        LUABOX_DRAFT_INSTALL=1 \
        GITHUB_TOKEN=fake-token \
        LUABOX_API_BASE="$api_base" \
        LUABOX_VERSION="$version" \
        LUABOX_INSTALL_DIR="$dir" \
        bash scripts/install.sh > "$log" 2>&1 || status=$?

    if [ "$must_succeed" = "yes" ]; then
        if [ "$status" -ne 0 ]; then
            echo "FAIL $leg: install exited $status"
            sed 's/^/    /' "$log"
            fail=1
            return
        fi
        if ! output="$("$dir/luabox" --version 2>&1)"; then
            echo "FAIL $leg: the installed binary did not run"
            fail=1
            return
        fi
        case "$output" in
            *"9.9.9"*) echo "PASS $leg: installed and ran ($output)" ;;
            *) echo "FAIL $leg: installed binary said '$output'"; fail=1 ;;
        esac
    else
        if [ "$status" -eq 0 ]; then
            echo "FAIL $leg: install succeeded where it had to fail"
            sed 's/^/    /' "$log"
            fail=1
        else
            echo "PASS $leg: refused, exit $status"
        fi
    fi
}

echo
run_install "curl" "$PATH" "$TAG" yes
run_install "wget" "$wget_only_path" "$TAG" yes
run_install "unknown-tag" "$PATH" "v0.0.0-does-not-exist" no

# ── jq fail-fast ────────────────────────────────────────────────────────────
# The opt-in must refuse in seconds, naming jq, rather than several API
# round-trips later inside the asset download.
no_jq_path="$work/no-jq"
mkdir -p "$no_jq_path"
for dir in /usr/local/bin /usr/bin /bin; do
    [ -d "$dir" ] || continue
    for tool in "$dir"/*; do
        name="$(basename "$tool")"
        [ "$name" = "jq" ] && continue
        [ -e "$no_jq_path/$name" ] || ln -s "$tool" "$no_jq_path/$name" 2>/dev/null || true
    done
done
status=0
env -i HOME="$work/home-nojq" PATH="$no_jq_path" \
    LUABOX_DRAFT_INSTALL=1 GITHUB_TOKEN=fake-token \
    LUABOX_API_BASE="$api_base" LUABOX_VERSION="$TAG" \
    LUABOX_INSTALL_DIR="$work/install-nojq" \
    bash scripts/install.sh > "$work/nojq.log" 2>&1 || status=$?
if [ "$status" -ne 0 ] && grep -q "needs 'jq'" "$work/nojq.log"; then
    echo "PASS no-jq: refused up front, naming jq"
else
    echo "FAIL no-jq: exit $status"
    sed 's/^/    /' "$work/nojq.log"
    fail=1
fi

echo
if [ "$fail" -eq 0 ]; then
    echo "draft-install-mock: ALL LEGS PASSED"
else
    echo "draft-install-mock: LEGS FAILED"
fi
exit "$fail"

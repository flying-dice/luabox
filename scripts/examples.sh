#!/usr/bin/env bash
# Keep the examples green. For every project under examples/ this runs the
# core gate (check, fmt --check, lint) plus per-example extras (build tree +
# bundle, .love packaging). luabox is a static toolchain — nothing here needs
# a Lua interpreter. Exits non-zero on the first real failure.
#
# Usage: bash scripts/examples.sh
# Honours $LUABOX (path to the luabox binary); defaults to target/release/luabox.
set -u

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
examples="$repo_root/examples"

LUABOX="${LUABOX:-$repo_root/target/release/luabox}"
if [ ! -x "$LUABOX" ] && [ -x "$LUABOX.exe" ]; then
    LUABOX="$LUABOX.exe"
fi
if [ ! -x "$LUABOX" ]; then
    echo "error: luabox binary not found at '$LUABOX' — run 'cargo build --release'" >&2
    exit 1
fi
fails=0
pass() { echo "    ok   $1"; }
fail() { echo "    FAIL $1" >&2; fails=$((fails + 1)); }

# run <label> -- <command...>
run() {
    local label="$1"; shift
    [ "$1" = "--" ] && shift
    if "$@" >/tmp/lb_ex_out 2>&1; then
        pass "$label"
    else
        fail "$label"
        sed 's/^/         | /' /tmp/lb_ex_out >&2
    fi
}

gate() {
    local dir="$1"
    run "check"        -- "$LUABOX" check
    run "fmt --check"  -- "$LUABOX" fmt --check
    run "lint"         -- "$LUABOX" lint
}

section() { echo; echo "== $1 =="; }

# 1. hello-luabox --------------------------------------------------------------
section "hello-luabox"
cd "$examples/hello-luabox"
gate .

# 2. geometry ------------------------------------------------------------------
section "geometry"
cd "$examples/geometry"
gate .

# 3. renderer (path dep — cross-package types, read in place) -----------------
section "renderer"
cd "$examples/renderer"
gate .

# 4. legacy-inifile ------------------------------------------------------------
section "legacy-inifile"
cd "$examples/legacy-inifile"
gate .

# 5. timemachine (build tree + bundle) ----------------------------------------
section "timemachine"
cd "$examples/timemachine"
gate .
# Config bundles to dist/timemachine.lua (minified, with a .map); --no-bundle
# forces the mirrored tree emit under dist/src/ instead.
run "build --no-bundle" -- "$LUABOX" build --no-bundle
run "build"             -- "$LUABOX" build

# 6. love-asteroids-lite (bundle a .love and check its contents) --------------
section "love-asteroids-lite"
cd "$examples/love-asteroids-lite"
gate .
# `[build] mode = "love"` makes a bare `luabox build` package the .love.
run "build (.love via mode=love)" -- "$LUABOX" build
if unzip -l dist/asteroids-lite.love >/tmp/lb_ex_out 2>&1 \
    && grep -q "main.lua" /tmp/lb_ex_out && grep -q "conf.lua" /tmp/lb_ex_out; then
    pass ".love contains main.lua + conf.lua"
else
    fail ".love contains main.lua + conf.lua"; sed 's/^/         | /' /tmp/lb_ex_out >&2
fi

# 7. workspace (check fans out; gate a member standalone) ---------------------
section "workspace"
cd "$examples/workspace"
gate .
cd "$examples/workspace/packages/core"
run "check (core member)" -- "$LUABOX" check

echo
if [ "$fails" -eq 0 ]; then
    echo "examples: ALL GREEN"
    exit 0
else
    echo "examples: $fails step(s) FAILED"
    exit 1
fi

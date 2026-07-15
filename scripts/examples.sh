#!/usr/bin/env bash
# Keep the examples green. For every project under examples/ this runs the
# core gate (check, fmt --check, lint) plus per-example extras (install,
# build, bundle, .love packaging, and run steps where a Lua runtime is
# present). Exits non-zero on the first real failure.
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
# Put the binary on PATH so `[tasks]` that call `luabox` resolve.
export PATH="$(dirname "$LUABOX"):$PATH"

# Find a Lua interpreter for run steps. All locally-run example output
# (including timemachine's lowered bundle) is Lua 5.1-compatible, so a single
# interpreter set via LUABOX_LUA drives every edition deterministically.
# We store the bare interpreter *name* (not the resolved path): luabox and the
# shell both resolve it via PATH, and on Windows/Git-Bash that lets `.exe` be
# appended — a resolved extensionless path would not open.
LUA=""
for cand in lua lua5.4 lua54 lua5.3 lua5.1 lua51 luajit; do
    if command -v "$cand" >/dev/null 2>&1; then
        LUA="$cand"
        break
    fi
done
if [ -n "$LUA" ]; then
    export LUABOX_LUA="$LUA"
    echo "==> using Lua runtime: $LUA ($(command -v "$LUA"))"
else
    echo "==> no Lua runtime on PATH — run steps will be skipped (not a failure)"
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

# 3. renderer (path dep — install first) --------------------------------------
section "renderer"
cd "$examples/renderer"
run "install" -- "$LUABOX" install
gate .
if [ -n "$LUA" ]; then
    if "$LUABOX" run src/main.lua >/tmp/lb_ex_out 2>&1 && grep -q "area = 16" /tmp/lb_ex_out; then
        pass "run (draws a square)"
    else
        fail "run (draws a square)"; sed 's/^/         | /' /tmp/lb_ex_out >&2
    fi
fi

# 4. legacy-inifile ------------------------------------------------------------
section "legacy-inifile"
cd "$examples/legacy-inifile"
gate .

# 5. timemachine (build + bundle + run lowered output on Lua 5.1) -------------
section "timemachine"
cd "$examples/timemachine"
gate .
run "build"                       -- "$LUABOX" build
run "bundle --minify --sourcemap" -- "$LUABOX" bundle --minify --sourcemap
if [ -n "$LUA" ]; then
    if "$LUA" dist/timemachine.lua >/tmp/lb_ex_out 2>&1 && grep -q "sum(1..5) = 15" /tmp/lb_ex_out; then
        pass "run lowered bundle on Lua 5.1"
    else
        fail "run lowered bundle on Lua 5.1"; sed 's/^/         | /' /tmp/lb_ex_out >&2
    fi
else
    echo "    skip lowered-run (no Lua runtime)"
fi

# 6. love-asteroids-lite (bundle a .love and check its contents) --------------
section "love-asteroids-lite"
cd "$examples/love-asteroids-lite"
gate .
run "bundle --mode love" -- "$LUABOX" bundle --mode love
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

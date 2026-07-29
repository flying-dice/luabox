#!/usr/bin/env bash
# Keep the examples green. For every project under examples/ this runs the
# core gate (check, fmt --check, lint) plus per-example extras (build tree +
# bundle, .love packaging), and then *executes* the timemachine bundle on
# lua5.1 to prove the compiler's output actually runs. Exits non-zero on the
# first real failure.
#
# luabox itself never runs Lua — it is a static toolchain. This script does,
# because a compiler that emits a file nobody ever executes is only checked
# against its own opinion of the file. The interpreter is a property of the
# harness, not of the product: with no lua5.1 on PATH the run step SKIPs
# loudly and the script stays green. `unzip` — used only to look inside the
# packaged .love — is treated the same way.
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

# The interpreter the built bundle is executed under. `[build] target` for
# timemachine is 5.1, so lua5.1 is the one runtime that proves the lowering:
# anything newer would accept 5.4 constructs the lowering is supposed to have
# removed. Absent (typical on a dev box), the run step SKIPs.
LUA51=""
for cand in lua5.1 lua51; do
    if command -v "$cand" >/dev/null 2>&1; then
        LUA51="$cand"
        break
    fi
done
if [ -n "$LUA51" ]; then
    echo "==> executing built output under: $LUA51 ($(command -v "$LUA51"))"
else
    echo "==> SKIP: no lua5.1 on PATH — the built bundle will not be executed"
    echo "    (install lua5.1 to run this check locally; CI always runs it)"
fi

# The archive inspector used to look inside the packaged .love. Like lua5.1
# it belongs to the harness, not to luabox — luabox builds the .love either
# way — so with no unzip on PATH the inspection step SKIPs and the gate still
# passes (examples/README.md says so; this is what makes that true).
UNZIP=""
if command -v unzip >/dev/null 2>&1; then
    UNZIP="unzip"
fi
if [ -n "$UNZIP" ]; then
    echo "==> inspecting packaged archives with: $UNZIP ($(command -v "$UNZIP"))"
else
    echo "==> SKIP: no unzip on PATH — the packaged .love will not be inspected"
    echo "    (install unzip to run this check locally; CI always runs it)"
fi

fails=0
pass() { echo "    ok   $1"; }
skip() { echo "    SKIP $1"; }
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
    run "check"        -- "$LUABOX" check
    run "fmt --check"  -- "$LUABOX" fmt --check
    run "lint"         -- "$LUABOX" lint
}

section() { echo; echo "== $1 =="; }

# 1. hello-luabox --------------------------------------------------------------
section "hello-luabox"
cd "$examples/hello-luabox" || { echo "error: missing example dir: $examples/hello-luabox" >&2; exit 1; }
gate

# 2. geometry ------------------------------------------------------------------
section "geometry"
cd "$examples/geometry" || { echo "error: missing example dir: $examples/geometry" >&2; exit 1; }
gate

# 3. renderer (path dep — cross-package types, read in place) -----------------
section "renderer"
cd "$examples/renderer" || { echo "error: missing example dir: $examples/renderer" >&2; exit 1; }
gate

# 4. legacy-inifile ------------------------------------------------------------
section "legacy-inifile"
cd "$examples/legacy-inifile" || { echo "error: missing example dir: $examples/legacy-inifile" >&2; exit 1; }
gate

# 5. timemachine (build tree + bundle + run the lowered output) ---------------
section "timemachine"
cd "$examples/timemachine" || { echo "error: missing example dir: $examples/timemachine" >&2; exit 1; }
gate
# Config bundles to dist/timemachine.lua (minified, with a .map); --no-bundle
# forces the mirrored tree emit under dist/src/ instead.
run "build --no-bundle" -- "$LUABOX" build --no-bundle
run "build"             -- "$LUABOX" build
# Execute what we just compiled: a 5.4 source lowered to a minified 5.1
# bundle must still print the right answer under a real 5.1 interpreter.
if [ -n "$LUA51" ]; then
    if "$LUA51" dist/timemachine.lua >/tmp/lb_ex_out 2>&1 \
        && grep -q "sum(1..5) = 15" /tmp/lb_ex_out; then
        pass "run lowered bundle on Lua 5.1"
    else
        fail "run lowered bundle on Lua 5.1"; sed 's/^/         | /' /tmp/lb_ex_out >&2
    fi
else
    skip "run lowered bundle on Lua 5.1 (no lua5.1 on PATH)"
fi

# 6. love-asteroids-lite (bundle a .love and check its contents) --------------
section "love-asteroids-lite"
cd "$examples/love-asteroids-lite" || { echo "error: missing example dir: $examples/love-asteroids-lite" >&2; exit 1; }
gate
# `[build] mode = "love"` makes a bare `luabox build` package the .love.
run "build (.love via mode=love)" -- "$LUABOX" build
if [ -n "$UNZIP" ]; then
    if "$UNZIP" -l dist/asteroids-lite.love >/tmp/lb_ex_out 2>&1 \
        && grep -q "main.lua" /tmp/lb_ex_out && grep -q "conf.lua" /tmp/lb_ex_out; then
        pass ".love contains main.lua + conf.lua"
    else
        fail ".love contains main.lua + conf.lua"; sed 's/^/         | /' /tmp/lb_ex_out >&2
    fi
else
    skip ".love contains main.lua + conf.lua (no unzip on PATH)"
fi

# 7. workspace (check fans out; gate a member standalone) ---------------------
section "workspace"
cd "$examples/workspace" || { echo "error: missing example dir: $examples/workspace" >&2; exit 1; }
gate
cd "$examples/workspace/packages/core" || { echo "error: missing example dir: $examples/workspace/packages/core" >&2; exit 1; }
run "check (core member)" -- "$LUABOX" check

echo
if [ "$fails" -eq 0 ]; then
    echo "examples: ALL GREEN"
    exit 0
else
    echo "examples: $fails step(s) FAILED"
    exit 1
fi

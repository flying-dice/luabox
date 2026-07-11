# luabox examples

Real, runnable projects that show the whole toolchain in action. Every one
passes `luabox check`, `luabox fmt --check`, and `luabox lint` cleanly against
the real binary — and the runnable ones actually run.

Work through them top to bottom; each introduces one new idea.

| # | Example | Edition → target | Demonstrates |
|---|---------|------------------|--------------|
| 1 | [hello-luabox](hello-luabox/) | 5.4 | The 60-second tour: `init` → `check` → `fmt` → `lint` → `test` → `run`, one annotated function, `[tasks]` |
| 2 | [geometry](geometry/) | 5.4 | **LuaCATS classes + a `.d.lua` def package**: `---@class`/`---@field` inheritance, `---@alias`, `---@enum`, a generic-class attempt — and an honest account of what luabox does and doesn't verify today |
| 3 | [renderer](renderer/) | 5.1 | **Consuming types across a package boundary** (path dependency), `luabox install` — and the finding that plain LuaCATS types don't actually cross that boundary today (vendored stopgap) |
| 4 | [legacy-inifile](legacy-inifile/) | 5.1 | **Pure LuaCATS** (`---@class`/`---@param`/`---@return`), warn mode, `[lint]` allowlist + `---@luabox-ignore` |
| 5 | [timemachine](timemachine/) | 5.4 → **5.1** | **Cross-version lowering**: write 5.4 (`goto`, bitops, `<close>`, `//`), `build` + `bundle` to 5.1, run the output on stock Lua 5.1 |
| 6 | [love-asteroids-lite](love-asteroids-lite/) | 5.1 | **LÖVE skeleton**: typing a framework via a `defs` package, `bundle --mode love` → a `.love` archive |
| 7 | [workspace](workspace/) | 5.1 | **Monorepo**: `[workspace]` members, path deps between packages, checking across the tree |

## A learning path

1. **Start at `hello-luabox`** to feel the core loop — the commands you'll use
   every day.
2. **`geometry` then `renderer`** are a pair: the first types a library with
   plain LuaCATS (`---@class`/`---@field`/`---@alias`/`---@enum`) plus a
   `.d.lua` def package, the second *attempts* to consume those types across a
   dependency edge. Read both READMEs in full — they document, with exact
   `luabox check` output, what LuaCATS actually verifies today (classes,
   fields, aliases, enums, literal sealing), what it silently lets through
   (inheritance/conformance, arbitrary field access, cross-package type
   sharing), and what's outright broken (generic classes and generic
   functions).
3. **`legacy-inifile`** is a second, simpler pure-LuaCATS library — no
   inheritance, no generics, just `---@class`/`---@param`/`---@return` in
   warn mode. It shows luabox meeting existing code where it is.
4. **`timemachine`** is the `tsc`-style payoff: modern syntax lowered to run on
   an old runtime, verified by actually running on Lua 5.1.
5. **`love-asteroids-lite`** and **`workspace`** show two real-world shapes of
   project: a game bundled for a framework (and the `---@meta` pattern for
   typing a framework/native-style global), and a multi-package monorepo
   (which also carries the set's one `---@generic` **function** example,
   alongside `geometry`'s generic **class** attempt).

## Running the examples

Each directory has its own `README.md` with the exact commands to try. With
`luabox` on your PATH:

```sh
cd examples/hello-luabox
luabox check && luabox fmt --check && luabox lint && luabox test
```

`luabox test` and `luabox run` need a Lua interpreter on your PATH (or
`LUABOX_LUA` set). Examples 1–7 that run locally do so on **Lua 5.1**;
`timemachine` writes 5.4 and *ships* 5.1, so its lowered output also runs on
Lua 5.1.

## Keeping them green

`scripts/examples.ps1` (Windows) and `scripts/examples.sh` (Linux/macOS) run
the full gate — check, fmt, lint, plus per-example extras (install, build,
bundle, `.love` packaging, and tests where a matching runtime exists). CI runs
the bash script on every push.

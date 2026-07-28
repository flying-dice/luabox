# luabox examples

Real projects that show the whole toolchain in action. Every one passes
`luabox check`, `luabox fmt --check`, and `luabox lint` cleanly against the
real binary; the runnable ones run on a stock interpreter of their target
dialect (luabox itself never executes Lua).

Work through them top to bottom; each introduces one new idea.

| # | Example | Edition → target | Demonstrates |
|---|---------|------------------|--------------|
| 1 | [hello-luabox](hello-luabox/) | 5.4 | The 60-second tour: `init` → `check` → `fmt` → `lint`, one annotated function |
| 2 | [geometry](geometry/) | 5.4 | **LuaCATS classes + a `.d.lua` def package**: `---@class`/`---@field` inheritance, enforced `: Interface` conformance, `---@alias`, `---@enum`, generic classes, and fully-typed `require()` in the test file |
| 3 | [renderer](renderer/) | 5.1 | **Consuming types across a package boundary** (path dependency, `[types] defs`): the dependency's classes are visible and *checked* in the consumer — a missing `Drawable` member is an error here |
| 4 | [legacy-inifile](legacy-inifile/) | 5.1 | **Pure LuaCATS** (`---@class`/`---@param`/`---@return`), warn mode, `[lint]` allowlist + `---@luabox-ignore` |
| 5 | [timemachine](timemachine/) | 5.4 → **5.1** | **Cross-version lowering**: write 5.4 (`goto`, bitops, `<close>`, `//`), `build` + `bundle` to 5.1, and the output runs on stock Lua 5.1 |
| 6 | [love-asteroids-lite](love-asteroids-lite/) | 5.1 | **LÖVE skeleton**: typing a framework via a `defs` package, `bundle --mode love` → a `.love` archive |
| 7 | [workspace](workspace/) | 5.1 | **Monorepo**: nested packages, path deps between them, checking across the tree in one pass |

## A learning path

1. **Start at `hello-luabox`** to feel the core loop — the commands you'll use
   every day.
2. **`geometry` then `renderer`** are a pair: the first types a library with
   plain LuaCATS (`---@class`/`---@field`/`---@alias`/`---@enum`, generics,
   enforced `: Interface` conformance) plus a `.d.lua` def package, the second
   consumes those types across a dependency edge via `[types] defs`. Read both
   READMEs in full — they document, with exact `luabox check` output, what
   luabox verifies: conformance and undefined-field errors in the defining
   package, and the same errors reported *in the consumer* across the package
   boundary.
3. **`legacy-inifile`** is a second, simpler pure-LuaCATS library — no
   inheritance, no generics, just `---@class`/`---@param`/`---@return` in
   warn mode. It shows luabox meeting existing code where it is.
4. **`timemachine`** is the `tsc`-style payoff: modern syntax lowered to run on
   an old runtime — point a stock Lua 5.1 at `dist/timemachine.lua` to see it.
5. **`love-asteroids-lite`** and **`workspace`** show two real-world shapes of
   project: a game bundled for a framework (and the `---@meta` pattern for
   typing a framework/native-style global), and a multi-package monorepo
   (which also carries a `---@generic` **function** example, alongside
   `geometry`'s generic **class**).

## Running the examples

Each directory has its own `README.md` with the exact commands to try. With
`luabox` on your PATH:

```sh
cd examples/hello-luabox
luabox check && luabox fmt --check && luabox lint
```

luabox is a static toolchain — it checks, formats, lints and builds, but never
launches an interpreter. To *run* an example, hand its sources (or its built
output) to your own Lua: every runnable example here targets **Lua 5.1**, and
`timemachine` writes 5.4 while *shipping* 5.1, so its lowered bundle runs on
5.1 too.

## Keeping them green

`scripts/examples.ps1` (Windows) and `scripts/examples.sh` (Linux/macOS) run
the full gate — check, fmt, lint, plus per-example extras (build, bundle,
`.love` packaging). One of those steps *does* want a Lua 5.1 interpreter:
both scripts execute the `timemachine` bundle they just built, because a
compiler whose output is never run is only checked against its own opinion of
it. `unzip` is wanted the same way, to inspect the `.love` archive. Those
tools belong to the harness, not to luabox — with neither on `PATH` the step
SKIPs loudly and the gate still passes, so a dev box needs nothing installed.
Linux CI installs both, so there the step always runs. CI runs the bash script
on every push.

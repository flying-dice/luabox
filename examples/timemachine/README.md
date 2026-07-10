# timemachine

The cross-version money demo. Write modern **Lua 5.4**, ship **Lua 5.1** —
luabox lowers the delta the way `tsc` targets an older JavaScript. This is the
`target` half of the toolchain (SPEC.md §2.1).

```toml
[package]
edition = "5.4"     # the dialect you write

[build]
target = "5.1"      # the dialect you ship
```

`src/main.lua` deliberately uses four features that do not exist in Lua 5.1:

| Feature (edition 5.4) | Lowered to 5.1 |
|---|---|
| Integer division `a // b` | `math.floor(a / b)` |
| Bitwise ops `& \| ~ << >>` | a tree-shaken `__luabox_rt` bitshim |
| `<close>` to-be-closed vars | a `pcall` scope wrapper calling `__close` |
| `goto` / labels | a `repeat … until` back-edge |

## Build it

```sh
luabox check                          # typechecks as 5.4
luabox build                          # lowers 5.4 → 5.1 into dist/src/
luabox bundle --minify --sourcemap    # single lowered+minified file + .lua.map
```

`build` mirrors your source tree into `dist/`; `bundle` inlines the require
graph into one file at `dist/timemachine.lua`. Both lower to the 5.1 target.

## The payoff: run the 5.1 output on real Lua 5.1

```sh
luabox bundle
lua dist/timemachine.lua
```

```
midpoint(0, 9) = 4
READ?  true
WRITE? true
EXEC?  false
sum(1..5) = 15
[close] run
```

That output is produced by **Lua 5.1** running code we wrote in 5.4 syntax.

## What the lowering looks like

`goto`/labels become an ordinary back-edge loop:

```lua
-- source (5.4)                  -- emitted (5.1)
::again::                        repeat
i = i + 1                          i = i + 1
sum = sum + i                      sum = sum + i
if i < 5 then goto again end     until not (i < 5)
```

Bitops and `<close>` route through a single injected `__luabox_rt` module that
is **tree-shaken** — only the helpers you actually use are emitted, and the
prelude is hoisted exactly once across the whole bundle. `7 // 2` becomes
`math.floor(7 / 2)` inline, no runtime needed.

Not every construct is lowerable: an *irreducible* `goto` (jumping into the
middle of a loop) is a hard `error[LB0601]`, and `ffi` use can't be
polyfilled. luabox proves each lowering rule preserves semantics rather than
guessing.

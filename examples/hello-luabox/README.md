# hello-luabox

The 60-second tour. A tiny binary project (`edition = "5.4"`) with one
annotated function and a busted-style test file — enough to feel every core
command in the toolchain.

```
hello-luabox/
├── luabox.toml         # manifest: package, build target, strict types
├── src/main.lua        # one ---@param/---@return annotated function
└── tests/greet_test.lua  # busted-style; run with your environment's test tooling
```

## The workflow, one command at a time

Run these from this directory (with `luabox` on your PATH).

```sh
luabox check        # typecheck: annotations honoured, 0 errors
luabox fmt --check  # canonical formatting (drop --check to rewrite in place)
luabox lint         # type-informed lint rules, 0 warnings
```

luabox is a *static* toolchain: it never runs your program. Execute
`src/main.lua` with whatever Lua interpreter you deploy on.

## What each piece demonstrates

- **Annotations drive `check`.** `greet` is annotated `---@param name string`
  / `---@return string`. Because `[types] strict = true`, a mismatched call
  such as `greet(42)` would be a hard `error[LB0300]`; without `strict` it
  degrades to a warning. Try editing the call and re-running `luabox check`.
- **Tests belong to your deployment environment.** `tests/greet_test.lua` is
  written busted-style (`describe`/`it`/`assert.equal`); run it with your
  environment's own tooling (e.g. [busted](https://lunarmodules.github.io/busted/)).
  luabox still recognises `*_test.lua`/`tests/` files: `check` and `lint`
  cover them, and `lint` allows the busted globals there.

## Where to go next

`../geometry` introduces LuaCATS classes and a `.d.lua` def package. See
`../README.md` for the full learning path.

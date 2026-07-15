# hello-luabox

The 60-second tour. A tiny binary project (`edition = "5.4"`) with one
annotated function, a `[tasks]` table, and a busted-style test file — enough
to feel every core command in the toolchain.

```
hello-luabox/
├── luabox.toml         # manifest: package, build target, strict types, tasks
├── src/main.lua        # one ---@param/---@return annotated function
└── tests/greet_test.lua  # busted-style; run with your environment's test tooling
```

## The workflow, one command at a time

Run these from this directory (with `luabox` on your PATH).

```sh
luabox check        # typecheck: annotations honoured, 0 errors
luabox fmt --check  # canonical formatting (drop --check to rewrite in place)
luabox lint         # type-informed lint rules, 0 warnings
luabox run start    # runs the `start` task → prints the greeting
```

`luabox run start` resolves `start` from `[tasks]` in the manifest. You can
also run the script directly: `luabox run src/main.lua`.

## What each piece demonstrates

- **Annotations drive `check`.** `greet` is annotated `---@param name string`
  / `---@return string`. Because `[types] strict = true`, a mismatched call
  such as `greet(42)` would be a hard `error[LB0300]`; without `strict` it
  degrades to a warning. Try editing the call and re-running `luabox check`.
- **`[tasks]` are your scripts.** `start` is a single command; `ci` is an
  array (`luabox check`, `luabox lint`, `luabox fmt --check`) that stops at
  the first failure — run the whole gate with `luabox run ci`.
- **Tests belong to your deployment environment.** `tests/greet_test.lua` is
  written busted-style (`describe`/`it`/`assert.equal`); run it with your
  environment's own tooling (e.g. [busted](https://lunarmodules.github.io/busted/)).
  luabox still recognises `*_test.lua`/`tests/` files: `check` and `lint`
  cover them, and `lint` allows the busted globals there.

## Where to go next

`../geometry` introduces LuaCATS classes and a `.d.lua` def package. See
`../README.md` for the full learning path.

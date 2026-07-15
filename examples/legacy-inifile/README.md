# legacy-inifile

A pure-**LuaCATS** library. This is what an existing, idiomatic Lua 5.1
codebase looks like when luabox checks it — proving the "annotated Lua checks
day one" promise. Contrast it with `../geometry`, which pushes LuaCATS
further — class inheritance, conformance, and generics.

```
legacy-inifile/
├── luabox.toml            # strict = false (warn mode), [lint] globals allowlist
├── src/inifile.lua        # ---@class / ---@field / ---@param / ---@return
└── tests/inifile_test.lua # busted-style describe/it/assert
```

## LuaCATS-only, warn mode

The whole API is described with stock LuaLS annotations that any editor
already understands:

- `---@class IniFile` + `---@field sections table<string, table<string, string>>`
- `---@param` / `---@return` on `parse`, `get`, and `section_names`

`[types] strict = false` puts type checking in **warn mode**: mismatches are
reported as warnings (exit zero) rather than hard errors. It is the gentle
on-ramp for adopting luabox on an existing project — get to a clean report,
then flip `strict = true` when you're ready for CI-grade enforcement.

```sh
luabox check        # 0 errors, 0 warnings — annotations line up
luabox fmt --check
luabox lint         # 0 errors, 0 warnings
```

## Two ways to silence a lint

This example deliberately shows both suppression mechanisms:

1. **Manifest allowlist.** The library exports itself as a global
   (`inifile = M`) the way old Lua modules did. Rather than fight that one
   deliberate global write, `[lint] globals = ["inifile"]` allows it
   project-wide.

2. **Inline ignore with a mandatory reason.** A reserved, not-yet-used local
   carries `---@luabox-ignore unused-local reserved for the planned
   duplicate-key mode`. The reason is required — a bare `---@luabox-ignore` is
   itself a diagnostic (`LB0500`).

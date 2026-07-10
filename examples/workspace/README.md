# workspace

A monorepo: several packages under one tree, sharing a lockfile and checked
together. The `[workspace]` table lists members by glob.

```
workspace/
├── luabox.toml                     # [workspace] members = ["packages/*"]
└── packages/
    ├── core/                       # a shared library
    │   ├── luabox.toml
    │   ├── src/core.lua
    │   └── tests/core_test.lua
    └── cli-tool/                   # depends on core (path dependency)
        ├── luabox.toml             # [dependencies] core = { path = "../core" }
        └── src/main.lua
```

## Checking across members

The root manifest carries a `[workspace]` table. Running `luabox check` from
the workspace root walks **every member's** sources in one pass:

```sh
luabox check        # 0 errors across core + cli-tool
luabox fmt --check  # formats the whole tree
luabox lint
```

`cli-tool` declares a path dependency on `core`
(`core = { path = "../core" }`), so the two packages form a small dependency
graph inside the workspace.

## Working in a single member

Each package is a normal luabox project. `cd` into one to run its tests or
its tasks:

```sh
cd packages/core     && luabox test
cd packages/cli-tool && luabox run start   # prints "2 + 3 = 5"
```

## A generic-attempt, for completeness

`core.first_or` carries a `---@generic T` annotation — included so the
example set has at least one real `---@generic` **function** (distinct from
`../geometry`'s generic **class** attempt). It's a live demonstration of the
same underlying gap: `T` doesn't actually flow through to the return type
(it lowers to `unknown`), so nothing here relies on it for real type safety.
See `../geometry/README.md` for the exact `luabox check` error text this
produces once you try to pin the result to a concrete type.

## Why a workspace

- **One lockfile, one resolution.** Members share dependency versions.
- **Path deps between members** without publishing — edit `core`, and
  `cli-tool` sees the change immediately.
- **Fan-out commands** — check or format the entire repo from the root.

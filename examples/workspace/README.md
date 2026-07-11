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

## A real `---@generic` function

`core.first_or` carries a `---@generic T` annotation — the example set's
`---@generic` **function** (distinct from `../geometry`'s generic **class**).
It now works (#84): `T` is inferred from the argument types at each call site
and flows through to the return type, so `first_or({ 1, 2 }, 0)` types as
`number` and `first_or(names, "?")` as `string`. Within any file that can see
its signature, pinning the result to a concrete `---@type` checks, and a
`default` whose type disagrees with the list element is a real `luabox check`
error. Matched to lua-language-server's semantics — see
`../geometry/README.md` for the full generics writeup and error text.

(Cross-*package* signature sharing is a separate epic (#108): a generic
called from another package still resolves to `unknown` until that lands.
Generic inference itself is complete.)

## Why a workspace

- **One lockfile, one resolution.** Members share dependency versions.
- **Path deps between members** without publishing — edit `core`, and
  `cli-tool` sees the change immediately.
- **Fan-out commands** — check or format the entire repo from the root.

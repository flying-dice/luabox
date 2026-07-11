# Known limitations (0.1)

luabox 0.1 checks stock LuaCATS more strictly than lua-language-server, but it
is early software. This page lists the gaps a real user is likely to hit in the
first week — each one verified against the shipping binary — so nothing here is
a surprise. It is deliberately short: small parser trivia is left out so the
handful of things that actually matter stay visible.

Where an item has a tracking issue, it is linked.

## Type system

### `---@alias` names are not visible across files (#110)

An `---@alias` is resolvable in the **same file** that declares it, and in an
ambient definition package listed under `[types] defs`. It is **not** carried
across a `require()` boundary by name: referencing an alias declared in another
module produces `LB0305: unknown type name`.

```lua
-- ids.lua
---@alias Id string

-- main.lua
local _ids = require("ids")
---@param x Id            -- LB0305: unknown type name `Id`
local function use(x) end
```

`---@class` and `---@enum` names **are** workspace-global and cross files
normally — alias is the exception. Values whose type happens to be an alias
still flow through `require()` fine; it is only the *name* that does not
resolve cross-file. Put shared aliases in a `[types] defs` package for now.

### LuaCATS tags that parse but are not yet enforced

luabox parses the full LuaCATS tag vocabulary (so annotated code and definition
packages load unchanged), but a few tags do not yet influence checking. They
are accepted and ignored rather than rejected:

| Tag | Status in `check` / `lint` |
|---|---|
| `---@operator` | Parsed; operator-overload result types are not applied during inference. |
| `---@nodiscard` | Parsed; discarding the annotated return is not diagnosed. |
| `---@deprecated` | Parsed; using a deprecated symbol is not diagnosed (it is surfaced by `luabox doc`). |
| `---@async` | Parsed; no async/await checking. |
| `---@vararg` (legacy standalone form) | Parsed; the legacy standalone tag is not wired to inference (the `---@param ...` form is the modern spelling). |
| `---@version`, `---@source`, `---@see`, `---@package` | Metadata; ignored by checking (some surface in `luabox doc`). |

Tags that **are** enforced today: `---@class` (incl. `: Parent` conformance),
`---@field`, `---@param`, `---@return`, `---@type`, `---@alias` (same-file /
defs), `---@generic`, `---@enum`, `---@overload`, `---@cast`, `---@meta`,
`---@diagnostic` (lint suppression), and inline `--[[@as T]]`.

## Tooling

### `luabox test --coverage` is not implemented (#100)

The test runner works; the `--coverage` flag is gated and exits with a clear
message rather than reporting bogus numbers:

```
$ luabox test --coverage
Error: --coverage is not implemented yet (SPEC.md §11); track progress at the project backlog
```

### Dependencies: no hosted registry in 0.1 (#101)

`[dependencies]` entries in `luabox.toml` may be:

- a **path** dependency — `pkg = { path = "../pkg" }`
- a **git** dependency — `pkg = { git = "…", rev|tag|branch = "…" }`
- a **workspace** dependency — `pkg = { workspace = true }`
- a **version requirement** — `pkg = "1.2"` — resolved against a registry you
  point at yourself.

There is **no hosted, first-party registry** in 0.1. A registry is any root you
choose — a plain directory, a `file://` URL, or an `https://` base (read-only
for install) — selected via the `LUABOX_REGISTRY` environment variable.
Adding a version-requirement dependency with no registry configured fails with
setup guidance rather than silently doing nothing:

```
$ luabox add somelib@1.2
Error: cannot add `somelib` as a registry dependency: no registry is
configured. Set LUABOX_REGISTRY to your registry's location …
```

### Editor extensions are not on marketplaces yet (#102)

The VS Code, JetBrains, Neovim, and Zed integrations are packaged but not yet
published to the VS Code Marketplace, Open VSX, the JetBrains Marketplace, or
the Zed registry. Install from the built `.vsix` / plugin `.zip` / dev-extension
as described in each editor's README under [`editors/`](editors/).

### Prebuilt binaries arrive with the first tagged release (#95)

The install scripts ([`scripts/install.sh`](scripts/install.sh),
[`scripts/install.ps1`](scripts/install.ps1)) download a prebuilt binary from
the latest GitLab release. Until the first `v*` tag is published there are no
release assets, so the scripts detect this and exit with a pointer to the
build-from-source fallback (`cargo install --git … luabox-cli`). They do **not**
build from source themselves — they fetch a release binary or tell you how to
build one.

## Stability expectation for 0.x

The **annotation surface is stable**: it is stock LuaCATS — the same
`---@`-comment dialect lua-language-server reads — and luabox does not add its
own competing type-file format. Existing annotated code keeps working.

What may still move during 0.x is **luabox's own diagnostic behavior**: the set
of rules, their severities, and the `LBnnnn` diagnostic codes may be tuned as
strictness is refined. Pin a toolchain version if you need byte-for-byte stable
diagnostics in CI.

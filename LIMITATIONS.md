# Known limitations (0.1)

luabox 0.1 checks stock LuaCATS more strictly than lua-language-server, but it
is early software. This page lists the gaps a real user is likely to hit in the
first week — each one verified against the shipping binary — so nothing here is
a surprise. It is deliberately short: small parser trivia is left out so the
handful of things that actually matter stay visible.

Where an item has a tracking issue, it is linked.

## Type system

### `---@alias` cross-file edge cases (#110)

`---@class`, `---@enum`, and `---@alias` names are all workspace-global: an
alias declared in any project file is nameable and enforced from every other
file (luals parity), so no `require()` and no `[types] defs` package is needed
to share an alias by name. A same-name `---@alias` declared in more than one
project file (or shadowing a `[types] defs` alias) now warns as
`duplicate-doc-alias` (`LB0310`) at the losing site, matching luals, while the
deterministic first-wins winner is unchanged. One residual behavior still
differs from lua-language-server and is worth knowing:

- **A cyclic alias collapses to `unknown` instead of being reported.** A
  self- or mutually-referential alias (`---@alias A B` / `---@alias B A`,
  across files or within one) terminates safely — the recursive edge lowers to
  `unknown` — but luabox emits no diagnostic for the cycle, where luals flags
  it. This matches luabox's existing same-file cyclic-alias behavior.

### LuaCATS tags that parse but are not yet enforced

luabox parses the full LuaCATS tag vocabulary (so annotated code and definition
packages load unchanged), but a few tags do not yet influence checking. They
are accepted and ignored rather than rejected:

| Tag | Status in `check` / `lint` |
|---|---|
| `---@async` | Parsed; no async/await checking. |
| `---@vararg` (legacy standalone form) | Parsed; the legacy standalone tag is not wired to inference (the `---@param ...` form is the modern spelling). |
| `---@version`, `---@source`, `---@see` | Metadata; ignored by checking (some surface in `luabox doc`). |

Tags that **are** enforced today: `---@class` (incl. `: Parent` conformance),
`---@field` (incl. `duplicate-doc-field`), `---@param`, `---@return`,
`---@type`, `---@alias` (same-file, defs, and cross-file by name, incl.
`duplicate-doc-alias`), `---@generic`, `---@enum`, `---@overload`, `---@cast`,
`---@meta`, `---@deprecated` (use sites diagnosed, luals `deprecated`),
`---@nodiscard` (discarded returns diagnosed, luals `discard-returns`),
`---@operator` (overload result types applied during inference, luals parity),
`---@private` / `---@protected` / `---@package` (member visibility enforced,
luals `invisible` — `LB0312`, both the `---@field <scope>` modifier and the
standalone tag on a `function Class:method` doc block),
`---@diagnostic` (lint + checker suppression), and inline `--[[@as T]]`.

One edge of member visibility (`LB0312`) is deliberately conservative. Whether
an access is "inside the class" is judged from the enclosing **carrier method**
(`function Class:method` / `function Class.fn`), matching luals's environment
rule; and `---@package` scopes to the file that declares the member's **class**
(so a class split across files treats every declaring file as in-package). Where
the receiver's class cannot be resolved to a single `---@class` — a union, or a
plain inferred table — no `invisible` is raised, keeping false positives out at
the cost of a few false negatives. The standalone tag is associated to its class
through the `function Carrier:method` path; a visibility tag on a bare
`Carrier.method = function() … end` assignment is not yet wired.

A `:` method call whose receiver resolves (through inference) to a single
declared `---@class` is now argument-checked against the method's signature and
flags a `---@deprecated` method at the call site (#118), the same as a
dotted/free call. Resolution is deliberately conservative: when the receiver is
not a single declared class — an unknown/`any`/union receiver, a plain inferred
table, an unannotated method, or an unresolved metatable — the `:` call is left
unchecked (no false positives). One `---@deprecated` edge remains uncovered: a
class type used purely as an annotation (not through a value use site).

One edge of `---@operator` is out of scope: **`---@operator call` (making a
value callable) is parsed but not applied.** The overload mechanism types
binary/unary operator *expressions* (`a + b`, `-v`, `#v`); making a class
value itself callable would hook the call-evaluation path instead and does not
fall out of the same mechanism, so it is deferred. Every other operator luals
supports (`add`, `sub`, `mul`, `div`, `mod`, `pow`, `idiv`, `concat`, `band`,
`bor`, `bxor`, `shl`, `shr`, `unm`, `bnot`, `len`) applies, including
right-operand dispatch and overload selection by parameter type.

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

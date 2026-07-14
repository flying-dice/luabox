# Known limitations (0.1)

luabox 0.1 checks stock LuaCATS more strictly than lua-language-server, but it
is early software. This page lists the gaps a real user is likely to hit in the
first week — each one verified against the shipping binary — so nothing here is
a surprise. It is deliberately short: small parser trivia is left out so the
handful of things that actually matter stay visible.

Where an item has a tracking issue, it is linked.

## Type system

### `---@alias` cross-file semantics (shipped — #110 closed; edge-case notes)

`---@class`, `---@enum`, and `---@alias` names are all workspace-global: an
alias declared in any project file is nameable and enforced from every other
file (luals parity), so no `require()` and no `[types] defs` package is needed
to share an alias by name. A same-name `---@alias` declared in more than one
project file (or shadowing a `[types] defs` alias) warns as
`duplicate-doc-alias` (`LB0310`) at the losing site, matching luals, while the
deterministic first-wins winner is unchanged. A self- or mutually-referential
alias (`---@alias A B` / `---@alias B A`, across files or within one, or a
bare `---@alias A A`) is reported as `LB0314`, at the alias's own declaration
— once per checked file that references it, however many places in that file
do — the recursive edge itself still terminates safely, lowering to
`unknown` rather than recursing, matching luals' `cyclic-alias` diagnostic.

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

Every operator luals supports applies. Binary/unary operator *expressions*
(`add`, `sub`, `mul`, `div`, `mod`, `pow`, `idiv`, `concat`, `band`, `bor`,
`bxor`, `shl`, `shr`, `unm`, `bnot`, `len`) are typed on the operator-expression
path, including right-operand dispatch and overload selection by parameter type.
`---@operator call` hooks the call-evaluation path instead (#122): a value whose
type resolves to a single declared `---@class` (through inheritance) declaring a
`call` overload is itself callable — `obj(arg)` checks the argument against the
operator's input type and takes its declared result type, flowing into
assignments, returns, and further checks. A no-input `call: R` operator accepts
any arguments; multiple `call` overloads select by argument type. Resolution is
conservative: an unknown/`any`/union callee or a plain table (no declared class,
or a class with no `call` operator) is left exactly as before — no synthesized
signature and no new diagnostic.

### Bidirectional / contextual typing (#120)

A function *literal* written where a `fun(...)` type is expected now takes that
expected type's parameter types for its own parameters, so its body checks with
no per-parameter annotation — the canonical bidirectional win (like typing a
callback's parameters from the callback type). Two positions are covered:

- **call argument** — `higher(function(w) ... end)` where `higher` declares
  `---@param cb fun(w: Widget)` types `w` as `Widget` inside the lambda, so a
  bad field read (`w.nofield`) is flagged (`LB0306`) and misusing `w` where a
  concrete type is expected behaves as if `w` had that type; and
- **`---@type` assignment** — `---@type fun(x: number): number` on a
  `local f = function(x) ... end` types `x` as `number`.

Conservative by construction: with no expected function type — an unannotated
callee/target, an `unknown`/`any` expected type, or a non-function expected type
— the parameters stay `unknown` exactly as before and no new diagnostic arises.
An explicit `---@param`/inline annotation on the lambda's parameter wins over
the contextual type (annotations are authoritative, SPEC §3).

Deferred (follow-ups, not yet done): propagating an expected type *into* a table
literal to infer the literal's own type (field-by-field checking of a table
literal against a `---@class` parameter already works); contextual typing of
`return` expressions beyond `---@return` checking; nested/transitive
propagation through multiple call layers; and overload-driven expected types and
generic callback inference (a generic callee is deliberately skipped — its
callback parameters carry unbound placeholders that are never guessed).

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

The VS Code integration is packaged but not yet published to the VS Code
Marketplace or Open VSX. Install from the built `.vsix` as described in its
README under [`editors/vscode/`](editors/vscode/). (JetBrains, Neovim, and
Zed integrations were removed for now — any LSP client can still be pointed
at `luabox lsp` manually.)

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

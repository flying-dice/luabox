# Known limitations (0.2)

luabox 0.2 checks stock LuaCATS more strictly than lua-language-server, but it
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

### LuaCATS tags: the full vocabulary is enforced

Every LuaCATS tag now influences checking, navigation, or docs — nothing is
parsed-but-ignored: `---@class` (incl. `: Parent` conformance), `---@field`
(incl. `duplicate-doc-field`), `---@param`, `---@return`, `---@type`,
`---@alias` (same-file, defs, and cross-file by name, incl.
`duplicate-doc-alias`), `---@generic`, `---@enum`, `---@overload`, `---@cast`,
`---@meta`, `---@deprecated` (use sites diagnosed, luals `deprecated`),
`---@nodiscard` (discarded returns diagnosed, luals `discard-returns`),
`---@operator` (overload result types applied during inference, luals parity),
`---@private` / `---@protected` / `---@package` (member visibility enforced,
luals `invisible` — `LB0312`, via the `---@field <scope>` modifier, the
standalone tag on a `function Class:method` block, and on a bare
`Carrier.method = function() … end` assignment), `---@diagnostic` (lint +
checker suppression), inline `--[[@as T]]`, `---@vararg` (the legacy spelling
of `---@param ... T`; both on one block union, matching luals), `---@async`
(calls from non-async functions warn as luals `await-in-sync`, `LB0316`;
top-level calls are fine — the main chunk is an async context),
`---@version` (a symbol whose version set excludes the project `edition`
warns at use sites as luals does, riding the `deprecated` diagnostic —
`>5.2`/`JIT`/comma lists, and 5.1 implies LuaJIT), `---@source`
(goto-definition redirects to the annotated location), and `---@see`
(rendered in hover and as linked "See also" sections in `luabox doc`).

Deliberate parity boundaries (luals behaves the same way): async-ness never
*propagates* (only an explicit `---@async` tag counts, matching luals's
default `awaitPropagate = false`), and using a `---@deprecated` class purely
as a type annotation is not flagged — luals's `deprecated` diagnostic also
fires only on value/call use sites.

One edge of member visibility (`LB0312`) is deliberately conservative. Whether
an access is "inside the class" is judged from the enclosing **carrier method**
(`function Class:method` / `function Class.fn`), matching luals's environment
rule; and `---@package` scopes to the file that declares the member's **class**
(so a class split across files treats every declaring file as in-package). Where
the receiver's class cannot be resolved to a single `---@class` — a union, or a
plain inferred table — no `invisible` is raised, keeping false positives out at
the cost of a few false negatives.

A `:` method call whose receiver resolves (through inference) to a single
declared `---@class` is now argument-checked against the method's signature and
flags a `---@deprecated` method at the call site (#118), the same as a
dotted/free call. Resolution is deliberately conservative: when the receiver is
not a single declared class — an unknown/`any`/union receiver, a plain inferred
table, an unannotated method, or an unresolved metatable — the `:` call is left
unchecked (no false positives). A `---@deprecated` class used purely as a type
annotation (not through a value use site) is not flagged — deliberate luals
parity; its `deprecated` diagnostic also fires only on value/call use sites.

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

### `goto`/label/`break` legality is not diagnosed (#44)

Three programs every reference Lua rejects at load time pass `luabox check`
and `luabox lint` clean: a `goto` with no visible matching label, a label
declared twice in the same scope, and `break` outside any loop. Dialect
legality itself is enforced (`goto` under `edition = "5.1"` reports
`LB0010`/`LB0001`) — the gap is specifically label/loop *resolution*
legality. Your interpreter still rejects these at load time; luabox just
doesn't pre-empt it yet. HIR already resolves each `goto` to its label (or
none), so this is a planned diagnostic, tracked as
[#44](https://github.com/flying-dice/luabox/issues/44).

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

The expected type now also propagates *into* literals and through nested
layers, matching luals (`script/vm/compiler.lua`, which lazily compiles a node
against its expected type):

- **into a table literal** — an expected `---@class` (at a `---@type` local, a
  `---@param` argument, or a `---@return` position) types a function-valued
  field's lambda from the field's declared `fun(...)`, so a bad field read
  inside it is flagged; a nested table-literal field takes its declared class
  type. The field-by-field literal diagnostics against the class are unchanged;
- **`return` position** — a `---@return fun(...)` contextually types the
  returned function literal's parameters the same way `---@type` does, and a
  `---@return <Class>` types a returned table literal's fields; and
- **nested/transitive** — an expected `fun(a: A): fun(b: B)` types both the
  outer and the returned inner lambda's parameters.

Still deferred (follow-ups, not yet done): overload-driven expected types and
generic callback inference (a generic callee is deliberately skipped — its
callback parameters carry unbound placeholders that are never guessed). A layer
with no expected type — e.g. a lambda passed to an *unannotated* parameter — is
never typed: propagation follows the expected-type structure and never invents
one.

## Tooling

### Parser nesting and expression-size limits

**Anything reference Lua accepts, luabox parses.** The parser bounds its own
recursion (nesting limit 220, against reference Lua's ~197 — `LUAI_MAXCCALLS`
is 200) and the height of the trees it builds (512), so pathological input
degrades into a diagnostic instead of a stack overflow. Both limits sit above
what `lua5.1`–`lua5.4` and `luac` accept, so no program a reference
implementation compiles is rejected here for being too deeply nested. Past
them you get one `nesting limit exceeded` or `expression too complex` — never
a crash, and the tree stays lossless.

The one place the two disagree is a *flat* operator chain — `a + a + … + a`,
`"a" .. "a" .. …` — which reference Lua parses iteratively at any length.
luabox builds one tree node per operator, so 511 terms is the longest chain
that parses: at 512 you get `expression too complex`. Machine-generated
sources are the only realistic way to reach that; hand-written Lua does not.

### Human diagnostics window very long source lines

`--format human` prints roughly 200 characters of a source line around each
label, with `...` marking whichever side continues, rather than the whole
line. This only shows up on lines longer than that — minified or generated
sources — where printing the line in full made the *report* larger than the
file it described (10 000 findings on a 377 kB single-line file produced
3.5 GB of output). Column numbers are never windowed: `file:line:col` names
the true column here and in every machine format, and `--format json`,
`sarif`, `github` and `gitlab` carry no source text at all, so nothing is
lost to a tool reading them.

Reporting is otherwise linear in the number of findings. The one cost that
still scales with a *single* diagnostic is its own span: a label covering a
megabyte counts a megabyte of characters to size its caret run, once.

### Dependency management and execution are non-goals, not gaps

luabox neither manages dependencies nor runs Lua. There is no resolver, no
lockfile, no registry client, no publish or sign-in path, and no managed
interpreter — those are **deliberate v1 non-goals**, not gaps waiting to be
filled, and nothing here is planned for a later 0.x. The decision record is
in [DIRECTION.md](../../DIRECTION.md#v1-scope-cut-accepted-2026-07-26)
(flying-dice/luabox#10, #11).

In practice: you materialize a rock tree yourself and luabox reads it.

```sh
luarocks install --tree lua_modules penlight
luabox check
```

What that tree buys you is `require` resolution and bundling: both the
luarocks layout (`lua_modules/share/lua/<X.Y>/…`, `<X.Y>` from your `[build]
target`, `5.1` for `luajit`) and the flat `lua_modules/<name>/` layout are on
the module path, and `lua_modules/` is never walked as project source.

**Cross-package *types* are narrower than that, and this is the sharp edge.**
A dependency's LuaCATS definitions reach your use sites only when all three
hold: (a) a `[dependencies]`/`[dev-dependencies]` entry names the package;
(b) a `luabox.toml` for it exists at `lua_modules/<name>/luabox.toml` (or at
the `path` you gave) and sets `[types] defs`; (c) the `defs/` directory it
names is present. A luarocks tree has no per-package
`luabox.toml`, so a plain `luarocks install --tree lua_modules penlight`
gives you resolution and bundling but leaves penlight `unknown` to the
typechecker. Typed third-party code today therefore means either a
flat-layout package that ships a `luabox.toml`, or definitions you write into
your own project's `defs/` and list in your own `[types] defs`. Teaching
luabox to read a rockspec's own definition files is not planned for 0.x.

Your `*.rockspec` and luarocks own everything else (adding, updating,
publishing), and you run your program with whatever Lua you already have.

### `build --mode love` requires an external zip tool

A `.love` file is a zip archive and luabox carries no zip implementation, so
`[build] mode = "love"` is the one build step that shells out to a tool it
does not ship: `zip`, else `python3 -m zipfile`, else `python -m zipfile` on
Linux/macOS; the System32 `bsdtar`, else PowerShell's `Compress-Archive`, on
Windows. With none of them on `PATH` the build fails loudly, naming every
tool it tried — it never writes a partial archive. This does not weaken the
"never spawns an interpreter" claim: the spawned tool archives the output
luabox just emitted and executes no Lua. `mode = "nvim-plugin"` and every
other command need nothing external. The other two commands that leave the
process are `upgrade` and `doc --open`; see
[DIRECTION.md](../../DIRECTION.md#north-star).

### Editor extensions are not on marketplaces yet (#102)

The editor integrations live in their own repos and ship installable
artifacts from their own releases — the VS Code `.vsix` from
[flying-dice/luabox-vscode](https://github.com/flying-dice/luabox-vscode)
(install via `code --install-extension`), the JetBrains plugin `.zip` from
[flying-dice/luabox-jetbrains](https://github.com/flying-dice/luabox-jetbrains)
(install from disk) — but neither is published to its marketplace yet
(VS Code Marketplace / Open VSX / JetBrains Marketplace). Those uploads are
the only residual steps and are pending publisher accounts/tokens this repo
doesn't hold. Any other editor can point its LSP client at `luabox lsp`.

### Prebuilt binaries (#95 — shipped)

Prebuilt binaries ship as of v0.1.0. Every `v*` tag publishes a
[GitHub release](https://github.com/flying-dice/luabox/releases) with binaries
for Linux x86_64, macOS Apple Silicon, and Windows x86_64 (plus `SHA256SUMS`).
The release is created as a **draft** and stays one until, on all three OSes,
the shipped install script has installed that draft's binary and the full
black-box acceptance + LSP acceptance suites have passed against the
*installed* executable; only then does it become a published, `latest` release
(see [RELEASING.md](../../docs/02-guides/01-releasing.md)). The install scripts
([`scripts/install.sh`](../../scripts/install.sh),
[`scripts/install.ps1`](../../scripts/install.ps1)) download the binary for your
platform from the latest release; they do **not** build from source — if you
want that, use `cargo install --git https://github.com/flying-dice/luabox luabox-cli`.
The remaining gap is only reach: not yet on crates.io, Homebrew, or other
package managers.

## Stability expectation for 0.x

The **annotation surface is stable**: it is stock LuaCATS — the same
`---@`-comment dialect lua-language-server reads — and luabox does not add its
own competing type-file format. Existing annotated code keeps working.

What may still move during 0.x is **luabox's own diagnostic behavior**: the set
of rules, their severities, and the `LBnnnn` diagnostic codes may be tuned as
strictness is refined. Pin a toolchain version if you need byte-for-byte stable
diagnostics in CI.

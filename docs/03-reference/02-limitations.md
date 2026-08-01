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
`---@meta` (a definition file's `---@field` declarations *and* its
carrier-style `function Class:method()` definitions both join the class
surface), `---@deprecated` (use sites diagnosed, luals `deprecated`),
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
*argument*-unchecked (no false positives). The callee's own use-site tags carry
no such risk, so `---@deprecated`/`---@async`/`---@version` on a
`function Class:method()` reach `obj:method()` for **every** resolved receiver
(#33) — including a plain prototype with no `---@class`, a method that is both
`---@field`-declared and defined, a `---@class` carrier with no
`C.__index = C` link (luals folds carrier attachments into the class off the
carrier binding, with no metatable reasoning), and a carrier-style member
defined in a `---@meta` defs file (#39). A `---@deprecated` class used purely
as a type annotation (not through a value use site) is not flagged —
deliberate luals parity; its `deprecated` diagnostic also fires only on
value/call use sites.

Two boundaries here are real and deliberate. A receiver that cannot resolve to
a single declared class at all — an `any`/unknown parameter, a union, a table
built behind an unresolved metatable — surfaces nothing: the method it names is
not known, so there are no tags to report. And a *dotted* call reached through a
value expression (`w.helper(…)` where `w` is an instance) is not
argument-checked; dotted callees resolve by name, so only `M.helper(…)` on the
module table itself is. Both cost false negatives, never false positives.

### The `__index`-less carrier: member *resolution* falls through

The `C.__index = C`-less carrier is not only a tag-propagation case. **Member
resolution itself falls through**: a value derived from `setmetatable({}, C)`
resolves `C`'s carrier-attached members *as if `__index` were set*, so `c:m()`
type-checks, takes `m`'s declared signature and return type, and reports
nothing — no `LB0306` — even though no metatable link exists at runtime. This
is the deliberate luals-parity behaviour of #33 (luals folds carrier
attachments into the class off the carrier binding, with no metatable
reasoning), and it is the one place where that parity and the runtime disagree
outright:

```lua
---@class Counter
---@field n integer
local Counter = {}
function Counter:value() return self.n end

local c = setmetatable({ n = 1 }, Counter)
print(c:value())   -- luabox check: exit 0
                   -- lua5.4:       attempt to call a nil value (method 'value')
```

Adding `Counter.__index = Counter` makes the program run. The fall-through is
scoped: it only *adds* resolutions off a resolved carrier — `c:nonexistent()`
on the same value still reports `LB0306` — so a genuinely undefined member is
not hidden by it.

Note that the "false negatives, never false positives" line above is about
**unresolved** receivers (an `any`/union/unknown receiver surfaces nothing
because there is no member to speak about). It does **not** describe this
case: here the receiver resolves fine and the finding the runtime would
justify is suppressed on purpose.

The checker keeps parity — that is what makes annotations portable — and the
runtime gap is covered by the `metatable-without-index` lint (`LB0510`,
suspicious tier), which fires on `setmetatable(t, C)` where `C` is an in-file
`---@class` carrier whose `__index` is never assigned. That rule is
**luabox-specific**: luals ships no equivalent diagnostic, so
`[lint] metatable-without-index = "allow"` restores exact luals behaviour.

#### What that lint does *not* cover

`LB0510` is a partial cover, not a closure of the gap, and both of its bounds
are worth stating plainly.

**It is in-file only.** The carrier, the `setmetatable` call and any `__index`
write must all be in the file being linted. The common shape

```lua
-- src/klass.lua
---@class Klass
local Klass = {}
function Klass:value() return 1 end
return Klass

-- src/main.lua
local Klass = require("klass")
local k = setmetatable({}, Klass)
print(k:value())   -- crashes; `luabox check` and `luabox lint` are both silent
```

is not reported: the `setmetatable` argument resolves to a `require` result,
not to an in-file `---@class` carrier, and the rule stays silent on anything it
cannot see whole. The same holds for a carrier whose `__index` is written
inside a function that is never called, in a branch that never runs, through a
local alias (`local mt = C`), or with a computed key — those are deliberate
*suppressions*, and they cost false negatives rather than false positives. A
carrier that declares some other metafield (`__call`, `__tostring`, `__add`,
`__mode`) is likewise silent: it is an operator metatable, not a broken class.

**It is lint-only.** `LB0510` never affects `luabox check`'s exit code; it
appears in `luabox lint` (and in the editor, on the lint channel). Wiring it
into `check` would put a heuristic behind the command CI gates on, which is
exactly the split the `check`/`lint` separation exists to keep.

Cross-file carrier analysis is not a tightening of this rule but a different
pass — one with the project's require graph in hand — and it is not shipped.
The reliable defence remains `C.__index = C`.

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

### `goto`/label/`break` legality (shipped — #44 closed; one rule left out)

The three programs every reference Lua rejects at load time are now errors in
`check`, `lint` and the editor: a `goto` with no visible matching label
(`LB0020`, with a did-you-mean nudge for a near-miss name), a label already
defined in scope (`LB0021`), and `break` with no enclosing loop in the same
function (`LB0022`). The verdicts were built against `luac5.4 -p` and
`luac5.1 -p` on a 53-program matrix and agree with them cell for cell, with
the one exception below. Duplicate-label scope follows each edition's own
rule — Lua 5.4 rejects a nested label shadowing an outer one, 5.2/5.3/LuaJIT
do not — so luabox never rejects what your `edition`'s compiler accepts.

**Left out on purpose:** reference Lua also rejects a forward `goto` that
jumps *into* the scope of a local (`goto skip local x = 1 ::skip::` →
`jumps into the scope of local 'x'`). That rule counts active locals at the
jump and at the label, with a special case for a label that is the last void
statement of its block; `;` is erased on lowering, so the HIR cannot express
"void statement" and the check would be a re-implementation of the reference
parser rather than a reading of the resolution luabox already has. luabox
accepts such a program and your interpreter still rejects it at load time.
This is an under-approximation only — no legal program is ever rejected for
it.

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
  `local f = function(x) ... end` types `x` as `number`, and equally on the
  assignment spelling, `M.f = function(x) ... end` (#38).

Conservative by construction: with no expected function type — an unannotated
callee/target, an `unknown`/`any` expected type, or a non-function expected type
— the parameters stay `unknown` exactly as before and no new diagnostic arises.
An explicit `---@param`/inline annotation on the lambda's parameter wins over
the contextual type (annotations are authoritative, SPEC §3).

On an assignment the `---@type` is authoritative for the *value* as well, not
just for the literal's parameters: it supplies the signature call sites are
checked against, and the block's use-site tags
(`---@deprecated`/`---@async`/`---@nodiscard`/`---@version`), which `fun(...)`
syntax has nowhere to write, ride along with it (#38). `---@type A, B` is
positional exactly as on a `local`, so a lone annotation over `a, b = f, g`
declares `a` only, and `b` keeps its inferred type. A declared signature that
disagrees with the literal's own parameter list — extra or missing parameters —
is **not** diagnosed: luals has no such rule (`---@type` simply covers the
value's type), so the declaration governs and luabox stays silent. A `---@type`
over anything other than a function literal on an assignment is unchanged;
that slot's enforcement lives on the `---@type` local path.

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

What that tree buys you is `require` resolution, bundling **and types**: both
the luarocks layout (`lua_modules/share/lua/<X.Y>/…`, `<X.Y>` from your
`[build] target`, `5.1` for `luajit`) and the flat `lua_modules/<name>/` layout
are on the module path, `lua_modules/` is never walked as project source, and a
luarocks tree's installed sources are read for their LuaCATS surfaces (#30,
[decisions/09](../../decisions/09-rock-tree-type-harvest.md)).

**Cross-package types from a bare tree used to be the sharp edge. It is
gone.** A rock's `---@class`/`---@enum`/`---@alias` declarations and each
module's `require`-export type are harvested from
`lua_modules/share/lua/<X.Y>/**.lua` with **no manifest declaration of any
kind** — no `[dependencies]` entry, no per-package `luabox.toml`, no `[types]
defs`. Surfaces only: a vendored body is never typechecked (a type error
inside a rock produces nothing), and a rock source that does not parse is
skipped in silence, named in the LSP log and nowhere else. The editor and CI
harvest the same tree, so they agree.

What that leaves, stated plainly:

- **A rock with no LuaCATS annotations gives you nothing** and stays `unknown`
  to the typechecker, exactly as before. Deliberate, not pending: an
  un-annotated module's export is a table of `unknown`s whose shape is whatever
  its top-level assignments happen to reveal, so harvesting it could only turn
  dynamic module construction into `undefined-field` noise about code you did
  not write. A source with no `---@` anywhere is skipped before it is parsed.
- **A library whose API is a *global*** rather than a module return — LÖVE,
  Neovim, OpenResty — still wants a `defs/` package. The harvest contributes
  type declarations and export types, not ambient globals.
- **Argument checking at a rock function's call site** does not happen:
  `local m = require("rock"); m.f("wrong")` is unchecked. The axis is the
  **module boundary**, not field access — measured: a table-field call in the
  *same* file IS argument-checked (LB0300), a call to anything reached via
  `require` is not, your own modules included. Fields survive the boundary
  (LB0306 fires cross-module) and a rock's `---@return` types flow, so
  misusing a *result* is caught; a rock's `---@param` is **not enforced at
  cross-module call sites**. Pre-existing, not a harvest limitation — tracked
  as [#46](https://github.com/flying-dice/luabox/issues/46), with the
  measured table on the issue. An explicit `---@type` at the call site
  restores checking today.
- **The flat `lua_modules/<name>/` layout is not harvested.** It keeps its
  existing route: a `[dependencies]`/`[dev-dependencies]` entry naming the
  package, a `luabox.toml` for it at `lua_modules/<name>/luabox.toml` (or at
  the `path` you gave) with `[types] defs`, and the `defs/` directory it names.
- **A name collision between two rocks resolves silently**, first-wins in path
  order. You declared neither side and cannot edit vendored code, so there is
  no warning to act on — unlike a collision between two `[types] defs`
  packages, which still warns (`LB0307`/`LB0310`).

Writing the definitions yourself remains the escape hatch, and it is a real
one: a name your `defs/` package — or your own source — declares wins over a
rock's **outright**, not merged, so a wrong or incomplete annotation upstream
is something you can correct locally. Reading a *rockspec* for definition files
it points at is still not planned for 0.x; nothing needs it now.

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

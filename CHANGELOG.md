# Changelog

All notable changes to this project are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versioning follows [SemVer](https://semver.org/), with the 0.x caveats
spelled out in [RELEASING.md](docs/02-guides/01-releasing.md#semver-policy-for-0x).

## [Unreleased]

**Breaking: `luabox check` rejects Lua it previously accepted.** The #56
export-seam change below is a narrowing, not a superset — a clean project
today can fail in CI after upgrading, with no code change of its own. Read
the #56 entry in full before upgrading a project you run in CI; the
"opt-out and migration" note under it is the fastest path to green if you
hit a new failure.

This block rides **0.2.0**, itself a breaking minor under the
[0.x policy](docs/02-guides/01-releasing.md#semver-policy-for-0x) for the v1
scope cut, so no further bump is forced — but it may not ship as a patch,
and that rule is now written into the policy rather than left to judgement.

### Changed

- **A `---@class` module export crosses `require` as the class, in both
  spellings** (#56). A returned *carrier* (`---@class Point` over
  `local P = {}`) used to cross the boundary as the structural table it
  happens to be — empty, since `---@field` lines declare members without
  assigning them — so consumers got leniency: `p.x` arrived `unknown`,
  `p.nope` was accepted, and a `---@field`-declared function was never
  argument-checked. The export position now reifies the carrier as the
  class it carries, the workspace-global identity luals resolves a require
  to, making the carrier and instance spellings symmetric: members typed,
  `p.nope` an `LB0306`, `---@field`-declared functions argument-checked.
  Strictly narrowing for code that leaned on the old leniency — that
  leniency was a disclosed limitation, and the shape it hid is exactly the
  one the wave-25 review flagged. Inside the declaring file nothing
  changes; only what crosses `require` does.

  **One narrowing reaches code that works at runtime**, and it is worth
  naming: a member attached to the carrier under a *computed* key
  (`for _, n in ipairs(names) do H[n] = … end`) is declared nowhere, so
  reading it through a `require` is now `LB0306`. Declaring the key space —
  `---@field [string] fun(): string` on the class — makes it clean again.
  Not a unilateral tightening: lua-language-server reports `undefined-field`
  on the same read, and both verdicts are now corpus rows of the #57 parity
  gate (`dynamic_key_carrier_require`, `dynamic_key_carrier_indexer`).

  **A second runtime-working shape narrows the same way, and is not covered
  by the computed-key fix above**: a carrier that borrows its members
  through an undeclared `__index` — `local T = setmetatable({}, { __index =
  Proto })` where `Proto` is a plain table nothing declares — reads
  `t.hello` through a `require` as `LB0306` now too, for the identical
  reason: nothing declares `hello`. lua-language-server agrees here as well
  (`undefined-field`, corpus row `metatable_index_carrier_require`).
  Declare the member on the class, or make `Proto` a class the carrier
  names as a parent (`---@class Thing : Proto`).

  Statically visible attachments — dotted functions, colon methods, data
  fields, table-literal carriers, members assigned from a `require` — are
  measured unaffected. The two shapes above (computed key, undeclared
  `__index`) are the ones that are not.

  **One class shape is excepted**, and the exception is the mechanism's
  price rather than an oversight: a carrier whose members still mention an
  **unbound type parameter** — `---@class Box<T>`, or a class inheriting a
  field from a generic parent it named without arguments (`: Base`) —
  crosses as the monomorphised template instead of as the class, because
  `Ty::Named` carries no type arguments and a class name that means `T` in
  the consumer names a variable it cannot produce. A template is structural,
  so it enforces no member list: `b.nope` on a generic carrier stays clean
  where the same read on a plain class is `LB0306`. Both directions are
  pinned as fixtures, and `---@type Box<number>` on the binding types its
  *declared* members correctly — it does not add enforcement: an
  *undeclared* member on a bound generic reference is still `LB0300 "found
  unknown"`, never `LB0306`, for the same reason. See
  [limitations](docs/03-reference/02-limitations.md#a-class-module-export-closed-in-both-spellings-54--56).

  **The narrowing also reaches vendored luarocks surfaces.** A rock's own
  harvested types go through this same export reification, so `luabox
  check` can newly emit `LB0306` in *your* file because an upstream rock
  exports a dynamic-key `---@class` carrier — a shape you did not write and
  cannot edit in place. The fix is still available, just indirect: declare
  the rock's class yourself, indexer included, in a `[types] defs` package
  (`[types] defs = ["defs"]`, a `defs/<rock>.lua` file); your project's own
  declaration of a name wins outright over a rock's, not merged with it (see
  [Using dependencies](README.md#using-dependencies) in README).

  **No per-diagnostic opt-out exists for `LB0306`, and none was added.**
  `[lint]` has per-rule `allow`/`warn`/`deny`; the checker's own `LB03xx`
  family has no equivalent severity control in `luabox.toml`. Two blunter
  hatches exist today and are not new: `---@diagnostic disable:
  undefined-field` (per line or per file) suppresses this one code exactly;
  `[types] strict = false` downgrades every type diagnostic — not `LB0306`
  alone — from error to warning project-wide, and `check`'s exit code is
  nonzero only on an error, so this is CI-green again but blunter than the
  one code you meant to waive: it silences every other type mismatch along
  with it. There is no staged, per-module migration between the two
  hatches — a project with many affected carrier modules fixes every call
  site in one pass or takes the whole-project downgrade. There is also no
  flag that recovers the *old* verdicts while keeping strict mode on: the
  only way to keep this PR's behavior from reaching your CI is not
  upgrading past it — pin the `luabox` binary version.

### Added

- **Hover and completion resolve class members through the checker's
  ambient environment** (#56). The editor surfaces were built on a per-file
  view, so a class declared in another file — named via `---@type`, or
  arriving through a `require` — had no member hover and no member
  completion even while diagnostics enforced those members. Both surfaces
  now read the same merged defs + workspace-global + rocks environment the
  diagnostics pipeline checks against (`Ambient::class_members`), so what
  the editor offers is what `luabox check` enforces — the last row of the
  `#54` limitations table, closed from both ends.

- **A lua-language-server parity gate** (#57). "Matches luals" is now
  measured, not claimed: `scripts/tests/luals-differential.sh` re-derives
  both tools' verdicts over a corpus of parity-sensitive shapes and diffs
  them against committed two-column expectations, where an intentional
  divergence is a justified row rather than a hidden allowlist. CI pins
  luals 3.13.5 by sha256. The gate's first run resolved the one guessed
  bound from #49 by measurement: luals requires an omitted *non-trailing*
  nil-admitting parameter exactly as luabox does.

- **Property coverage for duplicate generic-class declarations** (#59). The
  positional-unification rule that #46 needed at three separate merge seams
  now has a single owner (`class_param_unification`), and a proptest
  generator covers the duplicate-declaration shape space — parameter
  spellings × field placement × declaration order × same/cross-file — with
  behavioural probes in both directions, so the next shape in that family
  is found by a generator instead of a reviewer.

- **`luabox lint --format json|sarif|github|gitlab`.** `lint` had no
  `--format` at all, so `luabox lint --format json` exited 2 — clap rejecting
  an unknown argument, the "you invoked me wrong" code rather than a verdict —
  while `check` carried the full surface. `lint` now takes the same closed set
  through the same renderer; the two commands produce the same diagnostic
  values, so a second rendering path for them could only be a way to disagree.
  Lint's exit-code semantics are untouched (SPEC.md §9): a warn-tier finding
  still exits 0. That is precisely why the machine formats carry severity
  faithfully — a CI consumer that wants to gate on warnings reads the severity
  back out of the report and decides for itself, rather than having luabox
  decide for it. A `[lint]` deny escalation moves both together: reported as an
  error, and exit 1. A clean project emits a well-formed *empty* document, as
  `check` already did, so a consumer that unconditionally parses stdout does
  not break on the happy path (refs #53).

### Fixed

- **A type-mismatch remedy suffix was introduced and withdrawn inside this
  same block.** Round 3 review F72 had `check_slot` append `" (add `---@type
  <expected>` to check it)"` to a `LB0300`/`LB0304` message whenever the
  found type was `unknown`. Round 4 review R12 withdrew it: `slot` there is
  the mismatching *expression* — a call argument or a `return` value — never
  the binding `---@type` actually attaches to, so the note pointed at a site
  the reader cannot annotate; and it fired for return-type mismatches too,
  where `---@type` is not even the applicable tag (`---@return` is). Net
  effect on this release: none — the suffix never reached a tagged version,
  only intermediate builds of this same Unreleased block. Noted here because
  diagnostic text is observable output a downstream tool can grep, and this
  one changed twice before anyone outside this PR could see either version.

- **Indexer precedence now follows one measured rule per seam, and a
  multi-parent merge no longer flips verdicts.** Two distinct rules govern
  a same-key `---@field [K] V` indexer, and this release makes both explicit
  after an intermediate build of this block collapsed them into one:

  - **Two unrelated parents** (`---@class C : P1, P2`, each declaring the
    key independently) resolve **first-listed-wins**, matching `develop`.
    An intermediate build of this block made it last-wins, which turned a
    read `develop` accepts into a false reject, and the reversed parent list
    from a rejection into silence. Both directions are measured against a
    binary built at the merge base and pinned.
  - **One ancestor reached twice with different type arguments** (a generic
    diamond) resolves **last-listed-wins**, matching the rule its `---@field`
    members already followed, so a class's fields and its indexers cannot
    disagree about which edge won.

  Separately, a same-key indexer re-declared on one class name across two
  `---@class` blocks now **dedups first-wins** at both merge seams (in-file
  and cross-file), matching the `LB0311` first-wins rule for a duplicate
  `---@field`. Previously both seams simply appended, so one merged class
  could carry the same key twice and resolve it by scan order.

- **Inlay hints and hover no longer lose a require'd carrier's member types.**
  Introduced and fixed inside this same block: when the export seam started
  reifying against the merged project ambient, the *display*-mode queries
  (`module_export`, `binding_types`) were left reading the defs-only ambient,
  so a binding whose class is declared in another file silently rendered
  `unknown` in the editor while `luabox check` typed it correctly — the exact
  editor/CI divergence #56 exists to close. Both queries now merge project
  types, pinned by a cross-file fixture.

- **A member covered only by a declared indexer now hovers.** `---@field
  [string] fun(): string` on a class made `h.anything` legal to the checker
  but gave the editor nothing to show; hover now falls back to the member's
  erased type instead of declining, matching the leniency rule the checker
  already applied.

- **A generic `---@class` carrier no longer exports its unbound type
  parameter.** `---@class Box<T>` crossing a `require` handed the consumer
  members typed `T` — a type variable it can neither name nor produce, so
  `expected number, found T` pointed at no action. The export now carries the
  same monomorphised template a bare `Box` reference lowers to: the unbound
  parameter reads `unknown`, and `---@type Box<number>` on the binding — the
  annotation that does bind it — types the members. The verdict is unchanged
  either way; the diagnostic is what changed. Two parity-corpus rows now cover
  a generic class crossing `require` in both directions.

  The parameter need not be the carrier's own: a class inheriting `---@field
  item U` from `---@class Base<U>` leaked `U` the same way. One rule now owns
  the seam — every type parameter in scope for the resolved shape is
  substituted, and the export crosses as the class name only when that
  substitution changes nothing.

- **A `---@class Sub : Base<number>` binds its parent's type parameter.** The
  arguments on a parent reference were dropped when the declaration was
  lowered — only the parent's *name* was kept — so an inherited `---@field
  item U` stayed `U` no matter what the child bound it to: `Sub.item` typed
  as an unbound parameter, and `: Base<number>` reported `LB0300` against its
  own declared parent (`missing member item of type U`). Parent arguments are
  now lowered and bound where the class members merge, at every level of the
  chain (`---@class Mid<M> : Slot<M>` passes its own parameter up), so
  `Sub.item` is `number`, the conformance obligation is stated in the child's
  vocabulary, and the class keeps the #56 export identity because it has
  nothing unbound left. A parent named *without* arguments (`: Base`) still
  leaves its parameters free, as a bare reference always has. Reference-site
  monomorphisation (`Cell<number>`) stays shallow by design (#84).

- **A generic `---@class`'s type parameters are scoped to the declaration that
  writes them.** Two declarations of one generic class may spell the parameter
  differently — `---@class Boxed<T>` with `---@field value T` beside
  `---@class Boxed<U>` with `---@field other U` — and luals resolves each
  declaration's field bodies against its own list. luabox handed the *first*
  non-empty list to every declaration of the name, so a renamed duplicate's
  own, valid annotation was reported as `LB0305` "unknown type name". Each
  declaration now lowers against its own parameters and the shapes are unified
  **positionally** as they merge: slot 0 is one type variable however the two
  spell it, so `Boxed<string>` makes both `value` and `other` `string`. Three
  merge points needed the rule and all three carry it — the instantiation
  templates, the class definition that crosses the `require` boundary, and the
  workspace-global class two files build. A declaration naming *another*
  declaration's parameter is still `LB0305`; the scoping cuts both ways, as it
  does in luals (refs #49).
- **A trailing parameter whose type admits `nil` is optional for arity.**
  `---@param b number|nil` and `---@param b? number` say the same thing about
  what may reach `b`, and Lua supplies `nil` for every argument the caller left
  off — so omitting it is the call the annotation permits, which is what luals
  concludes. Only the `?` spelling was counted, so `f(1)` against
  `f(a: number, b: number|nil)` reported `LB0301`. The rule lands on every
  arity path at once (direct, method, overload selection, cross-module).
  Deliberately narrow in two directions, both documented: only a *trailing* run
  is optional, and "admits `nil`" means the type says `nil` — `any` and
  `unknown` decline to constrain the parameter rather than declaring it
  omittable, so they stay required (refs #46).
- **A string receiver's members resolve through the `string` library.** Every
  string in a Lua state shares one metatable whose `__index` is the `string`
  table, so `s:upper()` *is* `string.upper(s)`. The free-function spelling
  typed; the receiver-method spelling produced `unknown`, which surfaced as
  `LB0300` "found `unknown`" wherever the result was used — for every string
  method. Now `s:upper()` is `string`, `s:byte()` is `integer`,
  `s:match(p)` is `string|nil`, and the arguments are checked with the receiver
  bound (`s:rep("three")` is `LB0300`, `s:sub()` is `LB0301`). A member the
  library does not declare is `LB0306` in both the `:` and `.` spellings, as it
  is at runtime, and a project that writes `function string.trim(s)` gets
  `s:trim()` — also as at runtime (refs #46).

### Documentation

- **The `---@class` module-export edge is written out where the other
  editor/CI edges are.** The disclosure lived only in `README.md`, and it was
  inaccurate: it described a class *carrier* module as hovering "as the class
  name" with CI "still checking" its members. Measured at the time, the
  carrier spelling hovered as the structural table the carrier is, and CI did
  not enforce it either — `p.x` crossed the boundary as `unknown` and `p.nope`
  was accepted. The class *instance* spelling was the one where the editor was
  narrower than CI. Both rows are now in `docs/03-reference/02-limitations.md`
  as a measured table, pinned by fixtures on both sides, and the `requires.rs`
  doc comment that made the same claim is corrected to match (refs #54).

  **Superseded within this same Unreleased block**: the #56 entry above closes
  that edge — a carrier export now crosses as the class and CI does enforce
  its members, so the measurement recorded here describes the behaviour this
  release changes, not the behaviour it ships. The limitations table it points
  at has been flipped to match.

- **Hover and completion on a `require` binding now use the type pass's
  answer.** `local m = require("mod")` hovered `unknown` while `luabox check`
  and the server's own diagnostics resolved the module's export type for that
  very binding — two `require` resolvers, and the editor asked the one that had
  never heard of cross-file modules (the per-file LuaCATS harvest). There is
  one now: `luabox-lsp`'s `requires::RequireExports`, the map the type pass
  already threads into `check_file_with_requires` — project modules from the
  database, rock modules from the vendored-tree harvest, in the precedence
  path-keyed resolution gives `luabox check`. Hover on the binding renders the
  module's export type, hover on a member (`m.helper`) renders that member's
  type qualified by the module, and `.`/`:` completion offers the module's
  exported members (`:` only the function-typed ones). Rock requires resolve
  the same way. Types render as the checker holds them, literals included: a
  module field inferred as `1` shows `1`, not `integer` — widening it for
  display would be the editor disagreeing with CI, which is the whole class of
  defect. An explicit `---@type` still wins over the module export. What stays
  `unknown` is unchanged and deliberate, and now pinned as such: a dynamic
  `require(name)`, and `require("a") or require("b")`, name no module
  statically, and the type pass does not resolve them either. One
  narrower-than-CI case is documented rather than papered over: when a module's
  export is a `---@class` *instance*, the binding hovers as the class name but
  its fields live in the declaring file, out of the editor's per-file reach, so
  its members get no hover or completion. README's "hover and completion agree
  with CI" is scoped to match (refs #54).
- **Calls to `require`d functions are argument-checked.** A function reached
  across a module boundary flowed its *type* into the consumer — a required
  function's `---@return` typed the value you bound — but its `---@param`
  annotations were enforced nowhere, so `local m = require("mod");
  m.f("wrong")` reported nothing while the identical call written in the same
  file reported `LB0300`. Every `require`d function in every project was
  unchecked at its call sites, rocks included, which bounded what a vendored
  tree's harvested types could actually catch. The checker resolved a callee's
  signature only through registries keyed by *name* — a local binding, a
  dotted name in the ambient map — and a required module's members appear in
  neither; nothing in the consumer file declares them. It now falls back to
  the callee's resolved *type*, which inference had already computed, and
  hands it to the same argument-checking path a same-file call takes. So
  `LB0300` and `LB0301` read identically on both sides of the boundary, and
  overloads, generics, `---@vararg` and optional parameters behave there
  exactly as they do within a file. Covered: `return M` module tables, a
  module whose export *is* a function, nested tables, colon- and dot-calls on
  exported classes, and vendored rocks.

  Conservatism is inherited rather than re-decided: an **unannotated**
  exported function is still not argument-checked, because an unannotated
  same-file function is not either. That required carrying provenance —
  reification erases the difference between a written `---@param` list and one
  read off an unannotated body, and checking calls against the latter would
  invent arity errors about code that makes no claim — so a function type now
  records whether a human wrote its signature. Two smaller gaps fell out of
  the same work: a `---@param` block above `return function(...) end` now
  binds to that function (a single-function module could not carry a signature
  at all before), and a `---@type fun(...)` over a name is authoritative at
  its call sites. Dynamic `require` paths, `---@field`-only class members and
  functions re-exported through a second `require` remain out, and are
  enumerated in [the limitations
  reference](docs/03-reference/02-limitations.md) (refs #46).
- **The project source walk no longer follows a symlink cycle.** `src/loop ->
  <root>` made `layout::walk` re-collect every source once per level until the
  kernel's symlink budget ran out — 41 copies of one `src/main.lua` on Linux,
  and 41 diagnostics for one mistake, terminating by `ELOOP` rather than by
  design. The walk now tests `entry.file_type()` (`is_real_dir`) instead of
  `Path::is_dir()`, the same guard the sibling rock and defs walks already
  carried: symlinked directories are not descended, symlinked *files* are
  still project source, and `walk`'s `LayoutError` propagation is unchanged
  (refs #51).
- **The GitLab Code Quality report no longer emits unusable locations or
  colliding fingerprints.** Two defects made the format lossy in a pipeline.
  An unspanned project-level finding (`LB1001` an unrecognised edition,
  `LB1002` an unresolvable `[types] defs` package, `LB1004` an unknown
  `[lint]` key) reported `location.path: ""` with `begin: 0`, which GitLab's
  parser rejects — the report parsed as JSON and annotated nothing. Those
  findings now take a stable synthetic path decided by code family: the
  manifest block (`LB1xxx`) reports `luabox.toml`, the file it is actually
  about, and anything else genuinely fileless reports the project root, both
  on line 1. Separately, the fingerprint hashed only code + file + byte
  range, so two *distinct* diagnostics over one range — the parser emits
  "unexpected token" and "expected an identifier" about the same token —
  hashed identically, and GitLab keeps one issue per fingerprint: the second
  finding silently vanished. The message is now hashed in. Fingerprints stay
  stable across runs for unchanged findings, which is their purpose; a
  *reworded* message deliberately re-keys its findings, the correct half of
  that trade to lose. JSON, SARIF and GitHub Actions were swept for both
  defect classes and have neither — SARIF omits `result.locations` entirely
  (the specified way to say "no location"), GitHub Actions drops the `file=`
  property, JSON carries no location field at all, and no other format emits
  a fingerprint (refs #52).
- **A `---@type` above an assignment is applied, not silently dropped.**
  `---@type string` above `M.a = 1` used to declare nothing and diagnose
  nothing: the annotation was consumed only for `local` statements, so it
  looked accepted and rotted. It now declares the assigned slot and checks the
  initializer against it (`LB0300` on the value), exactly as on a `local`.
  luals binds a doc block to the statement rather than to the target's syntax,
  so every spelling gets it: a table field (`M.a`), a bracket index with a
  literal key (`M["a"]`), a nested field (`M.a.b`, annotating the innermost
  slot — the one being assigned), a global (`G = 1`), and a plain name. Reads
  of the target see the declared type. `---@type A, B` stays positional as on a
  `local`, so a lone annotation over `M.a, M.b = x, y` declares `M.a` only.
  Where a `---@field` also declares the member the two do not compete: the
  `---@field` governs the class surface, the `---@type` governs the assignment
  it sits above. One consequence worth knowing: `---@type <Class>` over
  `G = {}` now reports its missing members on the spot (`LB0302`) — the
  build-it-up-later deferral is a property of the `local X = {}` carrier
  spelling, and `---@class` is the carrier spelling that does collect members
  attached to a global later.
- **Two `---@class` declarations for one name in the same file union instead of
  the second wiping the first.** Across files they already unioned; within one
  file the second declaration replaced the class and every member the first had
  contributed vanished — a merge rule that depended on the file boundary, which
  luals has no notion of. Parents, `---@field`s, `---@operator`s, visibility
  and carrier attachments now merge from every declaration, in one file, across
  files, and in `---@meta` definition files alike, and a class carried by two
  different tables collects the members of both. A same-name **field** declared
  twice keeps the **first** declaration and warns at the loser as
  `duplicate-doc-field` (`LB0311`) — which is what that warning's own note has
  always said, so the stored type and the message now agree. luals unions the
  two types instead; the divergence, and why a stable winner plus a warning
  beats a silent widening, is written up in
  [Known limitations](docs/03-reference/02-limitations.md). A project file's own
  `---@class` still *replaces* a same-named stdlib/`[types] defs` class whole —
  that escape hatch is unchanged. The generic monomorphisation template merges
  the same way: a `---@class Name<T>` declared twice keeps both declarations'
  fields, and a *bare* re-declaration that only adds members now reaches the
  template instead of being skipped for carrying no `<T>`. That also removes a
  false `LB0305` — a duplicate that renamed the parameter left the template
  saying `U` while the surviving field body said `T`, and reported the first
  declaration's own annotation as an unknown type name.
- **A `---@class` carried by a global collects its members.** `---@class Global`
  over `Glob = {}` tagged the class but never gathered anything attached to it,
  so `function Glob:size()` was invisible and every `g:size()` through the class
  reported `LB0306` on valid luals code. The carrier maps were keyed on local
  bindings, which a free global name does not have. All the member spellings now
  land — `function Glob:m()`, `function Glob.fn()`, `Glob.const = v`,
  `Glob.fn = function() end` — with their declared signatures, in-file, across
  files, and in `---@meta` definition files. Precedence is unchanged and now
  covers globals coherently: the carrier *variable* beats a class of the same
  name in either declaration order, and a variable carried twice answers with
  its most recent carrier, the way Lua resolves the name.
- **`luabox check --watch` now reruns when the vendored rock tree changes.**
  Since the rock type harvest landed, `check` reads
  `lua_modules/share/lua/<X.Y>/**.lua` — but the watcher still filtered all of
  `lua_modules/` out, so `luarocks install --tree lua_modules <rock>` (the
  exact workflow the harvest exists for) left a running watcher printing a
  stale verdict until some project file was touched. The versioned rock tree
  is now relevant (`layout::is_rock_source`); flat `lua_modules/` layouts,
  rockspecs, and nested trees inside workspace members still are not, because
  the rerun still does not read them.
- **The bundle loader gate names the line, not just the file.** A vendored
  rock that cannot load on the ship target (`not loadable under target 5.4:
  label `a` is already defined`) reported a path and a message with no
  position — and the rock path is the only one that reaches this branch,
  since the check gate never walks `lua_modules/`. The error now carries
  `at line N` from the finding's own span.

- **`LB0510` prunes dead code symmetrically, and its bounds are now a list
  rather than a count.** The prune was one-sided in two places. It dropped the
  `then` of a literal-false `if` and never the `else` of a literal-**true**
  one, so `if true then … else c:reset() end` counted a call no execution
  performs; and it read no loop header and no early return at all. Arms are
  now walked in order and everything after the first literal-truthy one is
  dead with it — later `elseif` conditions, their blocks, and the `else`. Two
  more shapes are the same literal question and are answered with it: a
  numeric `for` whose written header runs zero times (`for _ = 1, 0`,
  `for _ = 1, 10, -1`) and statements after one that leaves the block
  (`do return end`, `break`). A zero step is deliberately not decided: Lua
  raises `'for' step is zero` evaluating the header, and a program that never
  gets that far is not dead code.

  Separately, the ternary. `cond and ctor or other` parses as
  `(cond and ctor) or other`, and `and` seeded its right operand
  unconditionally — no literal was consulted on that path at all — so a
  literal-false `cond`, under which Lua never evaluates the construction,
  still seeded it. Truthiness is now read where the source writes it out, and
  folds through nested `and`/`or`, which is what makes the ternary answerable:
  `false and ctor` is the falsy left operand, `false or ctor` *is* the
  construction (a closed false negative), and a literal `cond` selects one of
  `ctor`/`other`. An undecided `cond` still seeds the `ctor` side, the same
  trade `x and ctor` already took.

  One more miss closed: `local m = c.reset; m(c)` is `c.reset(c)` with a name
  in between — the read goes through the missing `__index`, yields `nil`, and
  the call fails. The read alone is still not a use, and a read off the
  *carrier* is a plain table read no metatable serves.

  Four false positives and one false negative are **disclosed rather than
  fixed**, each with a matrix twin: flow-insensitive derivation (`c`
  reassigned before use), two `setmetatable` calls on one table (the second
  replaces it, so the superseded site is a finding against a lookup that
  works), the insert-last-wins name-to-body map in both directions, and a
  guard that is a name rather than a literal — in its `if`, ternary and
  numeric-`for` spellings. Deciding any of them needs constant propagation,
  and a partial one that decided `and`/`or` but not `if` would be worse than
  none.

  [LIMITATIONS](docs/03-reference/02-limitations.md) loses "one approximation
  remains" — the claim four consecutive review rounds found overclaiming — for
  an enumerated, two-direction bounds section written against the code and
  then checked back against it. The shape matrix grew from 94 programs to 119.

- **The language server no longer exits 1 when a session ends during the
  *second* progress-create window.** The previous round's fix aborted the wait
  on a `shutdown` or `exit`, which is right but not sufficient: `run` opens
  two create windows before the message loop (the startup rock harvest, then
  the workspace index), and a queued config reload opens more from inside it.
  Window 1 aborted on the `shutdown` and correctly left the `exit` on the
  channel; window 2 then drained that `exit` onto the pending queue, and the
  shutdown handshake read an empty channel for 30 s before failing — the same
  wait-then-exit-1 the round before had closed, reached by one more window.

  A session-ender is now **sticky**: the next create is not sent at all, so no
  window opens, and nothing is announced to a client that has asked to leave
  (the abort path used to return the token, so a shutting-down client still
  received `begin`/`report`/`end` plus another create). The shutdown handshake
  is the server's own, looking in the pending queue before the channel and
  answering a repeated `shutdown` instead of failing the session on it. The
  30 s bound is unchanged.

- **A `window/workDoneProgress/create` the server will not wait for is no
  longer sent.** After one create went unanswered the server skipped the
  *wait* on the next but still sent it, so a client's late error answer landed
  in the message loop's discard arm and `$/progress` went out under a token it
  had just refused — the never-answers optimisation stepped around the refusal
  check the whole mechanism exists for. One timeout now mutes progress for the
  session: no further create is sent, so there is no token to report under and
  no traffic to a client that is not listening. The token that timed out keeps
  the degraded path, which is the behaviour this mechanism replaced.

- **`LB0510` counts uses where the file can *reach* them.** The previous round
  closed the reported instances and left the mechanism, so the same defect
  came back in new shapes. `observed()` iterated receivers across every body
  in the file with no reachability test at all: a `local function boom()
  return c:reset() end` that nothing invokes, beside `print(type(boom))`,
  warned about a lookup no execution performs — and so did
  `if false then c:reset() end`.

  Uses are now counted only in bodies this file **enters** — the chunk, plus
  a fixpoint over the in-file call graph across the edges the pass already
  tracked (a named function, a field of a named table, a function expression
  the call site writes out, the `__call` dispatch on a derived value, and the
  colon method an instance call reaches) — and only in statements no
  **literal** condition prunes. A closure that escapes is not reached, and a
  guard that is a name is not decided; both are false-negative-shaped and both
  are disclosed with fixtures. Round 7's last disclosed approximation, a
  dead-branch `self:m()` inside a `__call`, closes as a side effect.

  Three more shapes in the same neighbourhood, each measured against
  `lua5.4` before being written down:

  - **the statement-form constructor.** `setmetatable` mutates its first
    argument and returns it; only the result was tracked, so the idiomatic
    `local t = {}; setmetatable(t, Cache); t:reset()` was silent on a crash.
    The argument is now linked too, alias-rooted, which also makes the same
    shape inside `Cache.new()` flow into factory detection.
  - **`and`/`or` follow the operand actually evaluated**, which is what the
    doc claimed and the code did not: `Or` seeded both operands. A
    construction is a table and always truthy, so `ctor or x` *is* the
    construction and `x or ctor` is undecidable; `x and ctor` is followed
    because a falsy `x` fails the same call just as hard, and `ctor and x`
    evaluates to `x`.
  - **module factories and dot dispatch.** Functions attached to a table were
    keyed by binding, so a *global* module table (`M = {}; function M.new()`)
    resolved to no body at all. And `c.reset(c)` is the same lookup as
    `c:reset()` with the same failure, so it now counts — while a bare field
    read, a metafield read, and a key naming nothing on the carrier do not.

  The shape matrix grew from 48 programs to 94, and the disclosed-miss list
  in [LIMITATIONS](docs/03-reference/02-limitations.md) from eight entries to
  eleven. Its preface no longer claims a committed fixture for all of them:
  ten have one, and `require` structurally cannot, because that bound is
  cross-file and the matrix runs one file at a time.

- **A `shutdown` sent while a progress token is being created no longer exits
  1.** `Connection::handle_shutdown` answers the request and then reads the
  *channel* for the `exit` that follows; it cannot see the queue
  `await_progress_create` drains into. So an `exit` that arrived during the
  wait was invisible to it — the server answered the shutdown, waited out
  `handle_shutdown`'s own 30 s bound for a notification it was already
  holding, and exited 1. A session without the progress capability exited 0,
  which is what makes it a regression; VS Code and Neovim surface it as
  abnormal termination, and it reproduced on the config-reload path as well
  as at startup.

  The wait now **aborts** the moment it drains a `shutdown` request or an
  `exit` notification: the message goes on the queue in arrival order, the
  loop drains it into the ordinary handshake, and that handshake finds the
  `exit` where it expects it. `Connection`'s contract is untouched. Waiting
  out the remaining 250 ms for a token nobody will use was pointless anyway.

- **An error answer to `window/workDoneProgress/create` is a refusal, not a
  success.** The wait matched on the response id and never looked at
  `response.error`, so a client replying `-32601` still received the `begin`,
  the per-file `report`s and the `end` under a token it had just declined —
  the exact traffic the wait was added to prevent. A refused token now takes
  the `progress: false` path: no `begin`, no `report`, no `end`.

- **`LB0510` counts calls the file *makes*, not colon calls it *contains*.**
  The reached-a-method gate had a syntactic hole: `self` inside any function
  attached to the carrier was treated as an instance, so a `self:m()` written
  anywhere in any attached body counted as a use whether or not the file ever
  entered that body. Two measured false positives. The committed `__call`
  fixture with its last line changed from `print(f())` to `print(type(f))`
  produced byte-identical lint output against opposite `lua5.4` verdicts; and
  `Cache.__mode = "k"` beside `function Cache:reset() self:clear() end` that
  nothing invokes warned on a program that runs to completion — the same shape
  the previous round closed, reopened by one added line.

  A method is now reached one of two ways: a **colon call on a derived value**
  (`c:m()`), or a **plain call on a derived value** (`c()`) when the carrier's
  `__call` metamethod is a body in this file whose own receiver takes a colon
  call — the chain `c()` → `C.__call(self)` → `self:m()`, which does crash.
  Only the `__call` body is asked, not every attached body: a `__call` that
  reaches no method, beside a `C:reset()` nothing invokes, runs fine. A colon
  method is silent unless something reaches it on an instance, which is a
  colon call on a derived value and was already tracked.

- **`LB0510` sees instances bound by assignment, by a global, or past the end
  of an initialiser list.** Value bindings were seeded from `local` statements
  only, and assignments were read for `C.field = …` targets alone, so
  `local c` / `c = setmetatable({}, Cache)` / `c:reset()` was silent on a
  crash. So were `g = setmetatable({}, Cache); g:m()`, the constructor pattern
  through a global `function make() … end`, and
  `local c = setmetatable({}, Cache) or fallback`. All now fire.

  Separately, all three passes paired names against initialisers with
  `names.iter().zip(init)`, which **drops** every name past the end of the
  list: `local n, c = make()` where `make` returns `1, setmetatable({}, Cache)`
  left `c` invisible and handed `n` the derivation that belongs to it. Names
  and values are now adjusted the way Lua adjusts them, tracking which result
  slot feeds which name, and factory return slots are tracked to match.

  What is still missed is now named shape by shape in
  [LIMITATIONS](docs/03-reference/02-limitations.md) — table field, parameter,
  generic-`for` variable, method-call factory, `...` slot, depth-two
  constructor, a `__call` reaching its method through a nested closure, and
  the pre-existing `require` bound — each with a committed fixture and its
  `lua5.4` verdict, so closing one is a deliberate act rather than a surprise.

  The shape matrix grew from 16 programs to 48. Every shape that can be
  written invoked and uninvoked is now committed **both** ways: the round-7
  false positive existed because only the invoked half had ever been written
  down, so the harness could not see that two files with opposite runtime
  verdicts were producing identical lint output.

- **`LB0510` no longer warns on a carrier whose methods nobody calls.** The
  operator-table gate was *structural* where it needed to be *behavioural*: a
  carrier that declared a lookup-irrelevant metafield **and** a colon method
  was read as a class, whether or not anything ever invoked that method on an
  instance. Eight measured shapes warned and ran fine under `lua5.4` —
  `Cache.__mode = "k"` beside `function Cache:reset()`, a `__newindex` guard
  beside `Guard:reject()`, an `__lt` comparator beside `Sorter:cmp()`.

  On a carrier with a metafield the rule now fires only on an **observed
  instance-side use**: a colon call landing on a value derived from
  `setmetatable(_, C)`. Four derivations are followed — the construction
  method-called on the spot, a local bound to it (and aliases of that local),
  the constructor pattern (`local c = Counter.new(); c:value()`), and `self`
  inside a function attached to the carrier, which is how a `__call` factory
  doing `return self:build()` still fires. Deriving the value is also what
  keeps one class's calls from settling another's: a `:get()` on a `Store`
  instance says nothing about a `Cache` that happens to declare a `get` too.

  A carrier with **no** metafield is unchanged — still structural, still
  firing on the construction alone. That region is pinned by four rounds of
  measurement, and gating it behaviourally would reopen the false-negative
  axis the previous round closed.

  The shape matrix behind all of this is now committed and runnable rather
  than kept in prose: `scripts/tests/lb0510-matrix/` holds one program per
  shape with its expected finding count, and `scripts/tests/lb0510-matrix.sh`
  re-derives both the lint verdict and — where `lua5.4` is on `PATH` — what
  the program actually does when executed. It is blocking in the differential
  workflow, the job with real interpreters on the runner.

- **`LB0510` fires again on the canonical carrier class.** The previous
  round's two false-positive fixes each over-reached, and between them they
  silenced the shape `metatable-without-index` exists for. A single
  `function Counter.__tostring(c)` line beside a colon method disabled the
  rule, so the idiomatic Vector2 tutorial class — a dot constructor,
  `:length()`, `__tostring`, `__add` — crashed on `v:length()` in silence;
  and a bare `local mt = Counter` with nothing written through it disabled it
  too, as did an alias inside a dead branch or an unrelated nested function.
  Re-measured against `lua5.4` over a 32-shape matrix: 19 shapes crash at
  runtime, the rule fired on 5.

  Another metafield now buys silence only on a carrier with **no instance
  methods** — a colon-declared `function C:m()` is what instance lookup, and
  so `__index`, is needed for — and an alias suppresses only when something
  is actually written *through* it, with carrier identity propagated along
  alias chains so a write through any link counts. After: 17 of the 19
  crashing shapes are reported, with zero false positives across the 13 that
  run clean. The two that stay quiet are the documented conservative bounds
  (a write in a dead branch or a never-called function), consistent with the
  same writes made directly on the carrier.

- **Every bundle mode refused to notice a chunk the ship target cannot
  load.** `luabox build`'s residual control-flow validation lived on the
  tree-mode path only; `bundle = true`, `mode = "love"` and
  `mode = "nvim-plugin"` all route through the bundler, which validated parse
  errors and dialect legality but never control-flow legality. A 5.2 project
  shipping `::a:: do ::a:: end` to 5.4 therefore wrote the illegal chunk into
  `dist/main.lua` — and into the `.love` archive — and exited **0**, while
  tree mode exited 1 with no output. The bundler now judges it too, so all
  four emit shapes agree.

- **A manifest-declared ship target reaches the legality passes.**
  `[build] target` fed `require` resolution and the rock harvest but nothing
  that judged the source, so a project *declaring* `target = "5.4"` passed
  `luabox check` on a program `luabox check --target 5.4` rejects, then built
  an artifact that cannot load. The control-flow pass now runs for the
  manifest target as well; `--target` still overrides it. The target's
  *dialect* legality is deliberately not asked of a manifest target — a
  declared target says the project is lowered there, and reporting `LB0011`
  on every `//` in a 5.3 project shipping 5.1 would fail `check` for using
  the feature `[build] target` exists to provide. An explicit `--target` is
  the literal "would this source be legal there?" question and still asks
  both.

- **A duplicate label reported for two dialects no longer contradicts
  itself.** The edition and target legality runs were merged on (code,
  primary span) with the first one winning, but two `LB0021`s at the same
  span are not the same verdict: 5.4's `checkrepeated` searches every open
  block where 5.2's searches only the current one, so the first-definition
  site is dialect-dependent. Edition 5.2 with `--target 5.4` over

  ```lua
  ::a::
  do
    ::a::
    ::a::
  end
  ```

  named line 4 as a duplicate of line 3, then line 3 as a duplicate of line
  1 — line 3 reported as both — and printed them out of source order. The
  ship target's verdict now wins a construct both reject, a finding only the
  target rejects says so in a note, and the merged set is rendered in source
  order.

- **`luabox build`'s duplicate-label report carries a span.** It was
  reconstructed from the lowered text, whose ranges do not index the source,
  so it shipped with no labels at all while `check --target` gave full spans
  for the identical defect. The build gate now judges the source at the ship
  target, so the finding underlines the duplicate and points at the first
  definition. The spanless residual pass over the lowered output stays as the
  belt-and-braces catch for anything lowering itself introduces.

- **`metatable-without-index` (`LB0510`) no longer warns on correct code.**
  Two shapes were false positives. An *operator metatable* — a carrier
  declaring `__call`, `__tostring`, `__add` or `__mode` and no `__index` —
  is a metatable whose purpose is not instance lookup; there is no
  `t:method()` to fail, and the rule is now silent when the carrier declares
  any metafield other than `__index`. An *aliased* `__index` write
  (`local mt = Counter; mt.__index = mt`) settles the same table the rule
  cannot follow, so a local alias of a carrier now suppresses, the same
  conservative trade as a computed key. In the other direction,
  `rawset(C, "n", 0)` no longer over-suppresses: only a literal `__index` or
  an unreadable key settles the carrier. The finding's note stopped asserting
  a crash at call sites that may not exist.


- **`--target` now reaches the control-flow legality pass, and `luabox build`
  will not emit a tree the target cannot load.** `--target` means "would this
  source be legal there?", but the `LB0020`-`LB0022` pass ran for the project
  `edition` only. Duplicate-label scope is the one control-flow rule that
  differs by edition — 5.4's `checkrepeated` searches every open block where
  5.2/5.3/LuaJIT search only the current one — so `edition = "5.2"` with
  `::a:: do ::a:: end` and `luabox check --target 5.4` reported **0 errors**
  for a chunk `luac5.4 -p` refuses to load. It is now `LB0021`, exit 1, and
  the finding is reported once when both the edition and the target flag the
  same span, exactly as dialect legality already deduplicated.

  `luabox build`'s check gate does not judge the target's *dialect* legality
  on purpose (lowering is what handles constructs the target's parser
  rejects), but nothing lowers a shadowed label away — so the same program
  built with `--target 5.4` silently emitted an unloadable file and exited 0.
  The gate now runs the target's control-flow pass against the source, and
  the residual validation of each lowered file judges it again over the
  lowered text. Emission stays per file, as it always has (tsc/esbuild
  semantics): the file that fails is not written and the exit code is
  nonzero, while files that lowered cleanly are still emitted — and because
  `build` never cleans `dist/`, a failing rebuild leaves the previous run's
  output in place. `luabox lint` and the language server have no target flag
  and are unaffected.
- **A `---@class` in a defs file no longer steals another class's carrier
  variable.** `---@class Wrapper` over `local Animal = {}` binds the *local*
  `Animal` to `Wrapper`, so `function Animal:speak()` is `Wrapper`'s method.
  A later `---@class Animal` (carried by some other variable) overwrote that
  binding with its own name-is-its-own-carrier alias, and `speak` folded onto
  the wrong class — swapping the two class blocks flipped the verdict, and
  the ordinary project-source path, which resolves the binding, disagreed
  with the defs path on the identical body. Carrier-variable bindings now
  take precedence over name aliases explicitly and in one ordered pass, so
  both orderings agree with each other and with project source (#39's goal).

- **`luabox check` and the language server no longer disagree about which
  file a colliding rock module name means.** A vendored tree can hold both
  `pl.lua` and `pl/init.lua`, and both answer to `require("pl")`. The rock
  walk sorted a `Vec<PathBuf>`, whose `Ord` is component-wise: it ranks the
  bare component `pl` below `pl.lua` and so put the **directory** first,
  inverting the order `require` actually resolves in. The harvest is
  first-wins per module name, so the editor (name-keyed) called `pl` the
  `init.lua` while `luabox check` (path-keyed, through
  `resolve_candidates`, which tries the flat `<rel>.lua` first) called it
  `pl.lua` — the same source got opposite verdicts in CI and in the editor.

  The walk now sorts by the paths' raw bytes, which reproduces candidate
  order on every platform (`.` = 0x2E sorts below both `/` and `\`). Pinned
  from both ends: a `collect_rock_sources` unit test over a tree that
  actually contains the colliding pair, and one repro fixture asserted
  through `luabox check` and through the server.

- **`goto`/label/`break` legality is diagnosed**
  ([#44](https://github.com/flying-dice/luabox/issues/44)) — three programs
  every reference Lua refuses to *load* used to pass `luabox check` and
  `luabox lint` clean. They are now errors, in `check`, in `lint` and in the
  editor:
  - `LB0020` — a `goto` naming no visible label (`goto nowhere`). The label
    name is underlined, and a near-miss visible label becomes a
    ``did you mean `continue`?`` nudge.
  - `LB0021` — a label already defined in scope (`::a:: ::a::`), pointing at
    the second declaration with the first one labelled as context.
  - `LB0022` — `break` with no enclosing loop in the same function, including
    the case people actually hit: `break` inside a closure *defined* in a
    loop, where the loop sits on the other side of a function boundary.

  Dialect legality already covered `goto` under `edition = "5.1"`
  (`LB0010`); the gap was label/loop *resolution* legality, which the HIR had
  been resolving all along without judging. The rules are read off reference
  Lua's own (`lparser.c`'s `undefgoto`/`checkrepeated`) and the verdicts were
  built differentially against `luac5.4 -p` and `luac5.1 -p` over a
  53-program matrix — every legal `goto` shape (forward, backward, outward,
  the `::continue::` idiom in each loop kind), every loop kind's `break`, and
  the same label name in sibling or nested-function scopes are left alone.
  Duplicate-label scope follows each edition's own rule: 5.4 rejects a nested
  label that shadows an outer one, 5.2/5.3/LuaJIT do not.

  One reference rule is deliberately left out — a forward `goto` that jumps
  into the scope of a local — because the HIR erases the void statements the
  rule turns on. It is an under-approximation only (no legal program is
  rejected for it) and is recorded in
  [the limitations page](docs/03-reference/02-limitations.md).
- **`---@type fun(…)` on an assignment now types the function it annotates**
  ([#38](https://github.com/flying-dice/luabox/issues/38)). `---@deprecated` +
  `---@type fun(self: C, n: integer)` above `C.m = function(self, n) end`
  reached nothing: neither the declared signature nor the block's tags landed
  on the assigned value, so `o:m(…)` was unchecked and the deprecation never
  surfaced. `Carrier.m = function(…) end` is the assignment spelling of a
  function definition — luals binds a doc block to the function value there
  exactly as it does above `function Carrier.m()` — so a doc block now attaches
  either way it is written: an explicit `---@type fun(…)` is authoritative for
  the value (SPEC §3) and supplies parameters, returns, overloads and generics,
  while the block's use-site tags
  (`---@deprecated`/`---@async`/`---@nodiscard`/`---@version`), which `fun(…)`
  syntax cannot express, ride along with it. The literal's own parameters are
  typed from the declared signature, the same bidirectional rule `---@type` on
  a `local` follows. `---@type A, B` stays positional, so a lone annotation
  over `a, b = f, g` declares `a` only. A declared signature that disagrees
  with the literal's parameter list is not itself a diagnostic — luals has no
  such rule, and the declaration simply governs.
- **A `---@class` carrier with no `C.__index = C` line no longer loses its
  methods** ([#33](https://github.com/flying-dice/luabox/issues/33)). The
  canonical luals shape — `---@class C`, `local C = {}`, `function C:m()`, and
  no runtime metatable link — reported `LB0306` (undefined field) at every
  `o:m()` and dropped the method's `---@deprecated`/`---@async` tags with it.
  An instance's shape reached its carrier only through an explicit `__index`,
  a runtime-fidelity requirement luals does not make: it folds carrier
  attachments into the class off the carrier binding. The fall-through only
  *adds* resolutions, so a genuinely undefined field is still reported and
  argument checking stays exactly as conservative as before.
- **Carrier-style members in a `---@meta` defs file are folded into the class**
  ([#39](https://github.com/flying-dice/luabox/issues/39)). `function
  Class:method()` (and `function Class.fn()`) inside a definition package
  reported `LB0306` at every use site: a checked project file gets its carrier
  attachments folded in by inference, but a defs file is never inferred, so
  they reached nothing. They are now harvested syntactically — signature,
  returns, and use-site tags — exactly as luals treats a library file, with a
  same-name `---@field` staying authoritative on type while inheriting the
  attachment's tags. An attachment with no doc block still joins the surface,
  at a fully permissive signature, so nothing is silently dropped.

### Added

- **`metatable-without-index` (`LB0510`, suspicious) — the runtime half of the
  `---@class` carrier trade.** `luabox check` resolves `c:m()` through a
  `---@class` carrier even when the metatable chain has no `__index`; that is
  deliberate luals parity (#33) and it stays. But `setmetatable({}, Counter)`
  followed by `c:value()` is `attempt to call a nil value (method 'value')` in
  every reference Lua, and luabox had stopped saying so. The new lint says it
  instead: it fires on `setmetatable(t, C)` where `C` is a `---@class` carrier
  declared in the same file and nothing anywhere assigns `C.__index`, and it
  names the one-line fix (`C.__index = C`).

  It is deliberately conservative — a global carrier, one reached through
  `require`, a table literal, a call result, a computed field write
  (`C[k] = v`), a `rawset(C, …)`, or a reassignment of `C` all leave it
  silent — and `---@meta` definition files are exempt. Suppressible as
  `---@luabox-ignore metatable-without-index <reason>` and configurable as any
  `[lint]` rule; `metatable-without-index = "allow"` restores exact luals
  behaviour.

  **Parity status: luabox-specific.** luals ships no equivalent diagnostic —
  it has nothing that reasons about metatable wiring — so this is a
  deliberate, opt-out-able addition on top of parity, not a divergence in the
  checker. `luabox explain LB0510` says all of this, and the trade is recorded
  in [the limitations page](docs/03-reference/02-limitations.md).

### Internal (contributors)

- **The `differential` job's path filter covers the whole lint crate.** The
  LB0510 runtime gate (the only job with a real Lua on the runner) filtered
  exactly one lint file — the rule itself — while the rule also reads
  `facts.rs` and `context.rs`, so an edit to either landed green with the
  runtime column unrun, which is what the filter's own comment promises
  cannot happen. `crates/luabox-lint/**` now, so the rule can grow a
  dependency on a sibling module without anyone remembering the filter.

- **The server waits for the `window/workDoneProgress/create` response before
  reporting under the token.** It sent the create and the token's `begin` back
  to back — 0.1 ms apart on the wire, with no response in between — and the
  message loop discarded every response, so the answer was never read at all.
  LSP puts the token in the client's hands: a `$/progress` under a token the
  client has not acknowledged is a notification it may drop, and a dropped
  `begin` announces the pause to nobody. `create_progress_token` now waits, so
  every caller's `begin` follows the response by construction. Client messages
  arriving during the wait — `initialized`, a `didOpen` for a restored buffer —
  are queued and drained by the loop before anything new, in arrival order.
  Bounded at 250 ms, so a client that answers nothing gets the previous
  behaviour rather than a server that stops serving it.

  **It is not free, and "no worse than what it replaced" is true of the
  protocol and false of the latency.** A client that never answers a create
  pays the full bound on every token: two at startup, one per config reload.
  Measured over the same workspace, startup-to-usable went from a 5842 ms
  median to 6282 ms — 2 × 250 ms before the editor is usable, plus 250 ms on
  each reload. A client that answers (every real editor does) pays a
  sub-millisecond round trip and none of this. The silent case is now halved
  — after one create goes unanswered the server stops waiting for the rest of
  the session — but it is not zero, and a client that answers *slowly* still
  costs whatever it costs.

- **`diagnostics::convert` derives its `source` instead of taking one.** All
  three non-test call sites passed exactly `source_for(diag.code)` — an
  invariant held by convention across two modules, and the shape of both
  source-mismatch bugs the previous rounds found. The parameter is gone;
  behaviour is byte-identical, and a caller can no longer disagree with the
  code because there is nothing left to pass.

- **The pinned-stack isolation test refuses to run under `RUST_MIN_STACK`.**
  The variable raises every thread's default stack, so an environment setting
  it to 8 MiB or more would let an *unpinned* rayon worker survive the
  calibrated recursion — two of the three deletion modes would stop failing
  and the test would pass while proving nothing. It now asserts the variable
  is unset, loudly, rather than overriding it.

- **Every server-created progress token and its `create` request id are now
  unique.** Both were derived from the token *name*, a compile-time constant,
  so three config reloads sent three `window/workDoneProgress/create` requests
  sharing one id and one token. JSON-RPC requires ids to be unique among
  outstanding requests and LSP requires server-generated tokens to be unique;
  a client tracking outstanding requests by id saw the second `create` collide
  with the first. A per-session counter now feeds both, so a reload announces
  itself as `luabox/reload-2`, `luabox/reload-3`, …. The startup tokens fire
  once each but get the same treatment.

- **The work-done capability gate moved inside `begin_progress`.** It returned
  a token unconditionally and relied on its one call site being guarded —
  which is what the reload path getting a second, unguarded call site looked
  like last round. It now returns `Option<ProgressToken>` like its titled
  sibling, so the gate is a property of the function.

- **Every published diagnostic's `source` is derived from its code.** Two
  publishers — the type pass and the parse/dialect helper — bypassed
  `source_for` and hardcoded the toolchain source. Harmless today, since
  neither can emit an `LB05xx`, and precisely the "right for the codes that
  exist now" shape as the code-action bug fixed last round. Both are routed
  through the helper; behaviour is byte-identical, and a new test asserts the
  invariant over the published stream rather than over the helper.

- **`pin_worker_stacks` has a test that fails when it is deleted.** The old
  suite (idempotence, plus cross-crate constant equality) passed with the pin
  gone, the `.stack_size` dropped, or both constants lowered together, and the
  call site carried a comment claiming no such test could exist. That holds
  only for parser-driven recursion, which `MAX_DEPTH` caps below 2 MiB; a
  *synthetic* recursion is under no such cap. `luabox-lsp`'s new
  `tests/pinned_stack.rs` reaches the pin through `run` — the production call
  site — then recurses ~8 MiB on a global-pool worker: comfortably past
  rayon's 2 MiB default and comfortably inside the pinned 16 MiB. It is the
  only test in its binary, because `build_global` succeeds once per process.
  All three failure modes were checked by hand and each aborts the binary.

- **`LB0510`'s alias resolution is linear again.** `Carriers::build` calls
  `Aliases::root` once per indexed write, and `root` walked the whole
  `local b = a; local a = C` chain on every call — quadratic in chain depth
  times writes. Measured on a generated file with an 8000-link chain and 8000
  writes through its deepest link, lint went from 1.27 s to 0.08 s; at 32000
  (a 1.36 MB file) from 22.85 s to 0.38 s, which is the flat control's time.
  Every alias is now resolved to its root once at build time, with path
  compression, so `root` is a single lookup. The self-loop guard and the step
  bound are kept: a chain is acyclic by construction, but that is a property
  of the lowerer rather than of this function's input.

- **Work-done progress is gated on the client, and the startup pause has a
  token.** `window.workDoneProgress` was read once and handed to the
  bootstrap index alone, so the config reload announced itself to clients
  that never advertised the capability; the flag now lives on the server and
  gates every `$/progress` it sends. The synchronous startup rock harvest —
  a stretch of protocol silence a client could not attribute to anything —
  is now wrapped in its own token, which required running it after the
  `Server` is constructed so it can reach the same helper the reload uses.
  The two tokens carry distinct ids so the pauses are distinguishable.

- **Three stale claims in comments and help text now match the code.**
  `build`'s residual-validation comment said the check gate runs edition
  legality only (it has also run the ship target's control-flow pass since
  the previous round) and implied the residual arms were unreachable; it now
  describes what they actually serve — a finding *lowering itself*
  introduces, plus three enumerated paths the gate cannot see (a module under
  `lua_modules/`, `--out` pointed at a source directory, and `LB0014`/`15`/`16`
  in tree mode), none of which writes an artifact. `build`'s module doc gains
  the same correction and names the bundle-path twin. `check --help`'s
  `--target` no longer says "*also* validate dialect legality": the manifest
  target drives the control-flow axis with no flag, and the flag asks both.
  `SPEC.md` §5 states which passes `[build] target` drives per command.

- **The rendered diagnostic stream is source-ordered across legality axes.**
  Dialect legality and control-flow legality were each sorted and then
  concatenated in pass order, so an `LB0021` at line 13 could render after an
  `LB0013` at line 29. They are now sorted together, with a stable tie-break
  that keeps the parser's verdict ahead of the loader's at the same span.

- **The language server's startup number is reproducible.**
  `scripts/lsp-startup-bench.sh` (plus its stdio client
  `scripts/lsp-startup-bench.py` and `gen-corpus --rock-tree`) regenerates the
  corpus, drives the real protocol and reports median/min, so the
  `harvest_rock_tree` figure can be re-measured instead of quoted. It is not
  a CI gate. Three claims around it were wrong and are fixed: two comments
  disagreed on the same measurement (654 ms vs 545 ms); the harvest was
  described as happening "once, at startup" when `reload_config` re-runs it
  on the main loop for every `workspace/didChangeConfiguration` and every
  watched `luabox.toml` edit — that reload is now wrapped in a work-done
  progress token so the pause is visible rather than looking like a hang; and
  the rayon pool was pinned only by the CLI entry point, so a library caller
  of `luabox_lsp::run`/`run_stdio` harvested on rayon's 2 MiB default worker
  stacks. Both public entry points now pin, best effort, and a test drives a
  195-deep source over 32 rock modules through the *unpinned* path.

- **A repeated carrier variable in a defs file follows the binding.** Within
  the lexical rank, `carrier_var_classes` kept the first carrier for a
  repeated variable name — but `function M:m()` names the binding in scope,
  which Lua resolves to the last `local M`. The project-source inference path
  resolved the binding and said "last"; the defs path said "first", so the
  two disagreed on every repeated-carrier shape, which is exactly the parity
  #39 exists to hold. Lexical-over-nominal ranking is a separate axis and is
  unchanged.

- **The control-flow differential enforces its own matrix invariant.**
  `scripts/tests/control-flow-differential.sh` asserted in prose that its
  target column's pinned edition parses every matrix program, with nothing
  checking it; a future program using a 5.3+ construct would have made that
  column measure two things at once and failed in the direction the script
  calls never-OK, blaming luabox for a defect in the matrix. A preflight now
  fails loudly, naming the program.

- **`LB0510`'s bounds are documented.** The limitations reference now states
  plainly that the rule is in-file only — the cross-file `require`d-carrier
  shape crashes at runtime with `check` and `lint` both silent — and that it
  is lint-only, never affecting `check`'s exit code. Cross-file carrier
  analysis is a different pass and is deliberately not attempted.

- **Two tests that could not fail, and one that was missing.** The `goto`
  half of `control_flow`'s 5.1 guard is now exercised by a hand-lowered file
  that genuinely contains a `Stmt::Goto` (the recovered 5.1 parse may contain
  none, so the old assertion held whether or not the guard existed); the lint
  quick-fix *pairing* — not just its precondition — is asserted against the
  diagnostic the server actually published, over a mixed diagnostic set.

- **`luabox_hir::validate::control_flow`'s 5.1 comment now matches the
  measurement.** It claimed `goto` under `edition = "5.1"` is "already
  reported as `LB0010`"; it is in fact two `LB0001` parse errors — `goto` is
  not in the 5.1 grammar — and since every caller skips this pass on a dirty
  parse, the `goto` guard is unreachable from any front-end path. The label
  guard *is* reachable and load-bearing (`::a::` parses under 5.1 and is
  `LB0010`). Both guards are documented per their real reachability, the
  `goto` one kept as defence in depth because the function is `pub` over an
  already-lowered file, and the asymmetry is pinned by a test.

- **The lint code band has an authority instead of a magic decade.** The
  language server decided whether a finding was a lint rule — and therefore
  whether its quick-fix matcher would look at it — with
  `diag.code.number() / 100 == 5`, and nothing asserted that every lint rule
  actually lives in `LB0500`-`LB0599`. `luabox_diag::Code::is_lint` now owns
  the band, with the contract spelled out in its doc comment, and the
  invariant is asserted where it cannot rot: `luabox-lint` checks every
  registered rule's code against it (that is the load-bearing test —
  `luabox-diag` sits below the rule registry and cannot see it), and the
  registry checks the band is densely allocated from `LB0500`.
- **The language server's startup rock harvest is parallel, and measured.**
  `luabox check` parallelized the identical workload after a measured
  1.98 s → 0.55 s; the LSP kept the sequential form and shipped no number.
  It now rides the same rayon pool (the global one `real_main` pins to a
  16 MiB worker stack), through the same `harvest_file` + `RockSurfaces::fold`
  split, so the result is byte-identical — the fold is what fixes precedence.

  Measured by `scripts/lsp-startup-bench.sh`, which reproduces the whole
  measurement end to end: it generates the corpus (`gen-corpus --rock-tree`:
  50 files, 102,813 lines of `---@class` Lua under
  `lua_modules/share/lua/5.4/`), drives the real stdio protocol, and times
  `initialize` to the **first** `publishDiagnostics`. On one 4 vCPU
  virtualized box, 7 runs each: **2751 ms → 760 ms median** (2673 ms → 718 ms
  min), 3.6x. The baseline is the same binary forced to one rayon worker, not
  a pre-fix build, so the comparison is of the parallelism and of nothing else
  in a commit range. The same project with no rock tree publishes in 11 ms, so
  the harvest is effectively the whole wait. The sequential row is single-host
  and steal-sensitive — an independent 4 vCPU box did not reproduce it — so
  the harness, not the constant, is what makes the claim checkable. The single
  source for the numbers, and for the host they were taken on, is the code
  comment at `harvest_rock_tree`.

## [0.2.0] - 2026-07-29

**The v1 scope cut — every item below is a breaking change.** luabox is now
a purely static toolchain: *it consumes a rock tree, it does not produce
one, and it never spawns an interpreter.* Dependency management and
execution are deliberate non-goals, not gaps — the decision record is in
[DIRECTION.md](DIRECTION.md#v1-scope-cut-accepted-2026-07-26)
([#10](https://github.com/flying-dice/luabox/issues/10),
[#11](https://github.com/flying-dice/luabox/issues/11)). The unreleased
dependency-management wave (luarocks registry, `publish`, url tarball deps,
rockspec editing) is retracted with it — none of it ever reached a release,
so it appears in no version entry.

### Added

- **A bare `luarocks install --tree lua_modules <rock>` now gives you the
  rock's *types*, with no configuration at all**
  ([#30](https://github.com/flying-dice/luabox/issues/30)). This was the
  documented sharp edge: cross-package definitions needed a `[dependencies]`
  entry, a per-package `lua_modules/<name>/luabox.toml` with `[types] defs`,
  and the `defs/` directory it named — none of which a luarocks tree has, so a
  rock stayed `unknown` to the typechecker. The rocks were never actually
  untyped: LuaCATS is the ecosystem's annotation dialect, and a rock that
  documents itself for lua-language-server has already written the signatures.
  `luabox check` and the LSP now read them where they sit, in the installed
  sources under `lua_modules/share/lua/<X.Y>/`:
  - a rock's `---@class`, `---@enum` and `---@alias` declarations become
    nameable and enforced in your code — `---@type rock.Thing` resolves (no
    more `LB0305`) and its fields are checked;
  - each rock module's `require`-export type joins the cross-file registry, so
    `local m = require("rock")` carries the module's annotated return types and
    misusing a rock-typed value is reported **at your use site** — an
    undeclared field read on a rock class is an `LB0306` in *your* file. No
    diagnostic ever names a vendored file.

  **Surfaces only; bodies are never checked.** Vendored code remains
  unchecked: the harvest returns a type surface and no findings, so a type
  error inside a rock produces nothing, and a rock source that does not parse
  is skipped whole — named in the LSP log pane, silent under `check`, never a
  project diagnostic. A source with no `---@` anywhere is skipped before it is
  parsed, so an un-annotated, dynamically-built module table cannot become
  `undefined-field` noise about code you did not write.

  **Explicit beats implicit.** A class, enum or alias name declared by your
  `[types] defs` or by any of your own source files wins over a rock's
  **outright** — replaced, not merged — which is what makes writing your own
  definitions a real escape hatch for a wrong annotation upstream. Among rocks
  the rule is silent first-wins in path order: you declared neither side of a
  rock-vs-rock clash and cannot edit vendored code, so there is no
  `LB0307`/`LB0310` to act on. Editor and CI harvest the same version
  directory (`[build] target`, `5.1` for `luajit`), so they agree. The flat
  `lua_modules/<name>/` layout keeps its existing `[dependencies]` + `[types]
  defs` route unchanged, and a `[dependencies]` entry alongside a rock tree
  neither breaks nor double-counts the harvest. What still needs definitions of
  your own — an unannotated rock, a global-API library, argument checking at a
  module field's call site — is spelled out in
  [docs/03-reference/02-limitations.md](docs/03-reference/02-limitations.md);
  the design record is
  [decisions/09](decisions/09-rock-tree-type-harvest.md).
- **`luabox schema` — the manifest contract, published as a JSON Schema.**
  The binary now carries a complete draft 2020-12 JSON Schema for
  `luabox.toml` and prints it to stdout, so editors, validators and LLM
  coding assistants can read the whole contract:
  `luabox schema > luabox.schema.json`. It covers every table, key, default,
  closed vocabulary (`edition`, `build.target`, `build.mode`, lint tiers and
  levels) and every mutually-exclusive dependency form, each with a prose
  description. The schema describes the manifest's *data model*: you write
  TOML, tooling maps it to JSON with the standard mapping and validates that.
- **The manifest contract is declared once, and both the parser and the
  published schema are built from it.** `luabox.toml` used to be described
  twice — by the hand-rolled parser's key allow-lists and by a hand-authored
  JSON Schema — with a parity suite standing between them to catch the drift.
  There is now a single declarative table in `luabox-manifest`: each table of
  the manifest names its keys once, with the value type, whether the key is
  required, the default the parser applies, and the prose an outside reader
  needs. `Manifest::parse` builds its allow-lists and its did-you-mean
  candidates from that table, and `schema/luabox.schema.json` is *generated*
  from it. A key that exists for one and not the other is no longer a test
  failure; it cannot be written down. Manifest error messages, the published
  schema and `luabox schema`'s output are unchanged.

  What a key table cannot say stays hand-written — and stays guarded by
  tests: the four-branch `oneOf` that makes the dependency source forms
  mutually exclusive, the semver and package-name patterns, and the `[lint]`
  open rule-id mapping. Every `examples/*/luabox.toml` in the repository plus
  a curated valid/invalid fixture corpus still runs through **both** the
  schema validator and the parser, and the two must return the same verdict,
  which is what holds those fragments to the parser's hand-coded cross-key
  rules. Adding an example project extends the corpus automatically. The
  generated schema file stays checked in, because its `$id` is a URL editors
  point at and the binary prints the file rather than rendering it at
  runtime; a test fails when the file drifts from the contract and says how
  to regenerate it
  (`LUABOX_BLESS=1 cargo test -p luabox-manifest schema_file_is_current`).

### Removed

- **`[tasks]`, `[workspace]`, and `{ workspace = true }` dependencies**
  ([#18](https://github.com/flying-dice/luabox/issues/18)) — these manifest
  tables only ever served the removed `run` command and the parked solver,
  and had been parse-but-inert since the scope cut. They are now an
  unknown-table error carrying the valid set, the did-you-mean nudge, and —
  for these two names specifically — `— removed in 0.2.0, see CHANGELOG.md`,
  so a manifest brought over from 0.1.4 says what happened rather than
  reading as a typo. Monorepo trees
  are unaffected: the source walk checks nested packages without any
  manifest declaration.
- **`luabox add` / `remove` / `install` / `update` / `vendor`**
  ([#10](https://github.com/flying-dice/luabox/issues/10)) — dependency
  resolution and installation are gone: the PubGrub solver, the
  git/url/http/luarocks providers, `luabox.lock`, the comment-preserving
  rockspec editor, and the hard-link installs into `lua_modules/`. A
  `luabox.lock` left in a project is now ignored, and
  `LUABOX_LUAROCKS_MIRROR` is unrecognized.
- **`luabox search` / `outdated`**
  ([#10](https://github.com/flying-dice/luabox/issues/10)) — the
  luarocks.org discovery reads and the GitHub-release probing, along with
  their frozen `{"results":[…]}` / `{"dependencies":[…]}` JSON contracts.
  Editors consuming those contracts lose them with no replacement.
- **`luabox publish`**
  ([#10](https://github.com/flying-dice/luabox/issues/10)) — the
  rockspec upload proxy, its offline gates, and `LUABOX_LUAROCKS_URL`.
  Publish with `luarocks upload <rockspec>` instead.
- **`luabox login` / `logout` / `whoami`**
  ([#10](https://github.com/flying-dice/luabox/issues/10)) — the GitHub
  OAuth device flow, the OS-keychain storage of the GitHub token and the
  luarocks.org API key, and the `LUABOX_GITHUB_TOKEN` / `GITHUB_TOKEN` /
  `LUABOX_LUAROCKS_API_KEY` precedence chain. luabox now stores no
  credential and makes no authenticated request — the editor extensions'
  "Sign in with GitHub" flow no longer has a backing command.
- **`luabox run`** ([#11](https://github.com/flying-dice/luabox/issues/11))
  — luabox never spawns an interpreter and never executes your code.
  `[tasks]` entries, the toolchain-first
  `PATH` resolution (`node_modules/.bin` semantics), and the
  `luabox run luarocks -- install <rock>` escape hatch all go with it.
- **`luabox toolchain`**
  ([#11](https://github.com/flying-dice/luabox/issues/11)) — installing,
  pinning, and listing managed Lua runtimes, the built-in toolchain index,
  and the luarocks provisioning (and generated `LUAROCKS_CONFIG`) that came
  with `toolchain install`. Bring your own interpreter; luabox acquires
  nothing.
- **The `luabox-store` crate** — the content-addressed store and its
  locking existed only to back installs.
- **The resolving half of `luabox-resolve`** — solver, providers, lockfile,
  semver ranges, luarocks bridge and solver reporting. The crate slims to
  the manifest/project/dialect model the frontend commands actually use.

### Kept — the seam

- The **`luabox.toml` manifest model** (`[package]`, `[lint]`, `[build]`,
  `[types]`) that every frontend command reads.
- The **`lua_modules/` read path**: `require` resolution, bundling and
  cross-package type checking still work over a rock tree, provided you
  materialize it. (Types need more than the tree — see *Fixed* below.)
- Everything static: `new`/`init`, `check`, `lint`, `fmt`, `build` (+ the
  bundler and its `love`/`nvim-plugin` modes), `unmap`, `doc`, `lsp`,
  `explain`, `upgrade`, and `--watch`.

### Fixed

- **A misspelled `[lint]` key is no longer silently inert** — `unused-locl =
  "allow"` did nothing and said nothing, because rule ids live in
  `luabox-lint` and the dependency-free manifest parser cannot check them.
  The check now runs where the config is consumed: `luabox lint` reports
  `LB1004` (a warning — the exit code is unchanged) naming the key, and
  `luabox lsp` logs it via `window/logMessage`. The did-you-mean nudge spans
  rule ids *and* tier names, so a mistyped tier — which reaches the config as
  a rule-id override, indistinguishable from one — says ``did you mean
  `pedantic`?``.
- **Invalid `--format` and `--mode` values are now rejected by the CLI
  parser itself** — exit 2 with clap's `[possible values: …]` listing,
  matching every other malformed invocation, instead of exit 1 from deep
  inside the command. `--edition`/`--target` deliberately keep their
  domain-level path so `LB1001` stays a machine-readable diagnostic.
- **A failure no longer dumps a stack backtrace when `RUST_BACKTRACE` is
  set.** `main` returned a `Result`, so every `Error:` was rendered by
  `anyhow`'s `Debug` — which appends the captured frames whenever that
  variable is exported for something else entirely. Release binaries are
  stripped, so the dump arrived as pages of `<unknown>` burying the one line
  that named the problem. luabox now renders the error and its `Caused by:`
  chain itself. Exit codes are unchanged: 0 on success, 1 on a command that
  ran and failed, 2 on a malformed invocation.
- **`[tasks]` and `[workspace]` say they were removed, not just that they are
  unknown** ([#18](https://github.com/flying-dice/luabox/issues/18)). Both are
  gone (see *Removed*), but a manifest upgraded from 0.1.4 still carries them,
  and the generic unknown-table error sent readers looking for a misspelling.
  The error for exactly these two names now ends `— removed in 0.2.0, see
  CHANGELOG.md`; every other unknown table is unaffected.
- **`---@deprecated` and `---@async` on a method carrier now reach `obj:method()`
  call sites** ([#33](https://github.com/flying-dice/luabox/issues/33)). Two
  carrier shapes swallowed the tags. A plain prototype table (`local P = {}` +
  `P.__index = P`, no `---@class`) published no method signature at all, because
  publication was gated on a declared-class receiver — a gate that belongs to
  *argument* checking, not to tags the author wrote on the method itself; the
  gate now governs only argument checking, and `LB0308`/`LB0316` fire for any
  resolved receiver while a structurally-resolved call stays free of
  manufactured arity findings. A method that is both `---@field`-declared and
  defined lost them too: the declaration shadows the carrier, and `fun(...)`
  syntax has nowhere to write a tag, so the declaration now inherits the
  carrier's `---@deprecated`/`---@async`/`---@version` while still governing
  parameters and returns — same-file and across the project surface.
- **`luabox doc` refuses to generate while parse errors exist**
  ([#24](https://github.com/flying-dice/luabox/issues/24)) — a file that
  does not parse has no trustworthy harvest. One rule: project sources
  and project defs gate (rendering the `LB0001` diagnostics refused
  over); a *dependency's* broken def is skipped with a stderr warning
  and never partially harvested — vendored text cannot brick the
  command. Type errors never gate: docs for imperfect code are still
  docs.
- **`pkg = { version = "1.0" }` now parses as the bare-string form spelled
  longhand** ([#23](https://github.com/flying-dice/luabox/issues/23)). The
  valid-key list always named `version`, but a version-only table was
  rejected with "must specify one of `git`, `path`, or `url`" — the two
  rules disagreed. A lone git reference or `sha256` still errors, now
  naming the missing source.
- **`---@source` redirects no longer vanish for a lone statement**
  ([#14](https://github.com/flying-dice/luabox/issues/14)). When the
  annotated statement was the only one in its block — a one-statement file,
  function body, or `do … end` — the enclosing block node shared its text
  range and was matched first, so goto-definition silently jumped to the
  local declaration instead of the annotated location. The target is now
  resolved to the *statement* at that range.
- **`lua_modules/` is no longer walked as project source.** `check`, `lint`,
  `fmt` and `build` skip any directory named `lua_modules`, at every depth,
  the same way they skip dot-directories and the build output directory. A
  vendored rock tree is whatever luarocks put there; typechecking it against
  *your* project's strictness failed on any rock that is not trivially typed
  — and took `luabox build` down with it, since `build` refuses to emit while
  `check` reports errors. Summaries now count first-party files only.
- **`require` resolves through a real luarocks tree.** Resolution (and so
  bundling, `check`'s cross-file types, and the LSP's goto-definition) now
  searches `lua_modules/share/lua/<X.Y>/a/b/c.lua` and
  `…/a/b/c/init.lua` — the layout `luarocks install --tree lua_modules`
  actually writes — where `<X.Y>` is the build target's version directory
  (`luajit` maps to `5.1`, as luarocks itself does). The flat
  `lua_modules/<name>/` layout is still searched first, so nothing that
  resolved before resolves elsewhere now. Compiled C modules under
  `lua_modules/lib/lua/<X.Y>/` cannot be inlined into a text bundle and stay
  runtime `require`s, exactly like any other unresolved name.
- **Unterminated long brackets are reported instead of silently accepted**
  ([#15](https://github.com/flying-dice/luabox/issues/15)). `x = [[abc` and
  `--[[ abc` used to lex as a complete string / comment running to
  end-of-file with no diagnostic, and the string then decoded to `ab` — a
  closing bracket's worth of bytes stripped that the lexer never saw. Both
  now produce `LB0001` (`unterminated long string` / `unterminated long
  comment`) spanning the whole unclosed run, matching how unterminated short
  strings have always been treated, and no truncated literal reaches the
  HIR. Unterminated short strings now report `unterminated string` rather
  than the generic `expected expression`. Files that relied on the old
  silence now fail `check`; `fmt` returns them unchanged, as it does for any
  input that does not parse.
- **The editor no longer indexes vendored `lua_modules/` trees.** The LSP's
  workspace index had its own copy of the source walk, and that copy still
  descended into the rock tree `luarocks install --tree lua_modules`
  materializes — so workspace symbols, goto-definition and rename saw
  thousands of vendored symbols that `luabox check` had already stopped
  looking at. Both now run the one walk (`luabox-manifest`'s), which skips
  `lua_modules/` at every depth and visits entries in sorted order.
  **Behaviour change:** symbols that live only inside `lua_modules/` no
  longer appear in workspace symbol search or goto results — put the types
  you need in a `defs/` package and list it in `[types] defs`, exactly as
  `check` requires. `--watch` stops rerunning for `lua_modules/` writes for
  the same reason: the command it reruns would not read those files.


- **`--watch` stops rerunning once your edit has settled.** After the first
  change, `luabox check --watch` never went quiet again: it re-ran the command
  every debounce window, forever, on a project nobody was touching (measured:
  148 reruns over 30 s of idle after one edit). Nothing had changed — the
  watcher was reacting to *itself*. `notify`'s inotify backend also subscribes
  to `OPEN`, `CLOSE_NOWRITE` and `ATTRIB`, so every rerun's own **reads** of
  your `*.lua` files and `luabox.toml` came back as filesystem events and
  triggered the next rerun. Watch now acts only on events that describe a
  change — creations, removals, renames, content and metadata writes, plus a
  writer closing a file — and ignores the access events a read produces. That
  filter is the whole fix, and it is enough on its own: on every platform
  luabox ships a binary for, a *read* is not reported as a change at all
  (inotify classifies it as an access; neither FSEvents nor
  `ReadDirectoryChangesW` reports it), so nothing a run does can feed the next
  one. An `mtime`-only `touch` still reruns, and one edit still costs one
  rerun.
- **`--watch` no longer throws away a save made moments after the previous
  one.** The fix above originally shipped with a second, belt-and-braces half:
  a 200 ms sweep after every rerun that received filesystem events and
  discarded them, so that a rerun could not react to its own activity. It
  could not tell a rerun's own noise from your editor's, so a save landing in
  that window was discarded outright — and nothing ever went back for it.
  `check --watch` sat there reporting `watch: ok` over a tree you had just
  broken, indefinitely, and `fmt --watch` silently skipped formatting the file
  you had just saved. Two saves ~0.3 s apart reproduced it every time, and an
  IDE "save all" spreading five files ~120 ms apart hit it on every use. The
  sweep is gone: every edit gets its rerun, at any spacing, and one edit still
  settles into silence afterwards.
- **`luabox fmt` and `luabox lint --fix` can no longer destroy the source they
  rewrite.** Both replaced a file by truncating it and then writing it back —
  so a write that failed part-way left a fragment where your source had been,
  and luabox, the only process that still held the bytes, then exited. It was
  not recoverable. Reproduced with `ulimit -f 8`: a 97,780-byte source came
  back as 8,192 bytes. A full disk, a quota, or a filesystem going read-only
  mid-run did the same. Every rewrite of a file *you* wrote now stages the
  complete new content in a sibling temp file, flushes it to disk, and renames
  it over the target — an atomic replace, so a concurrent reader sees either
  all the old bytes or all the new ones, and a failure at any point leaves the
  original untouched. Permissions are preserved (an executable script stays
  executable), and a symlinked source is still rewritten *through* the link
  rather than having the link replaced by a regular file. One thing is given up
  deliberately: a **hardlinked** source now gets a new inode, so another name
  for the old file keeps pointing at the old content. Losing the file outright
  was worse. Files luabox creates rather than replaces are unaffected —
  `init`/`new` scaffolding, and everything `build`/`doc` write into their own
  output directory.
- **`luabox check | head` no longer crashes — and no longer lies about what it
  found.** Piping any luabox report into a reader that stops early — `head`,
  `grep -q`, a pager you quit — used to kill the process: Rust's `println!`
  panics when a write fails, a closed pipe makes every write fail, and any
  report larger than the pipe buffer (64 kB) is guaranteed to still be writing
  when the reader leaves. The result was a raw Rust panic and a backtrace on
  stderr, ending in `SIGABRT` (exit status 134) on release builds, in all five
  `--format`s. A CI job with `set -o pipefail` and a routine `| head` or
  `| grep -q` went red for it.

  The first fix for that traded one failure for a quieter, worse one: a
  departed reader exited **0**, unconditionally. So
  `set -o pipefail; luabox check | head -1` over a tree with thousands of
  errors *succeeded*, and a CI gate reported green over a broken tree — silent,
  and wrong in the safe-looking direction. What happens now is neither: **a
  departed reader costs you the output and nothing else.** luabox stops
  writing, does not panic, and exits with the verdict the run actually reached
  — 1 when `check`, `lint` or `fmt --check` found problems, 0 when they did
  not, and 0 for commands like `build` and `schema` whose report precedes a
  success nothing later can revoke. The truncated report is the only loss, and
  losing it is silent, the way `head` users expect. That applies to stderr as
  well as stdout, so `luabox check 2>&1 | head` behaves too. A run whose output
  is read in full is unchanged, and a genuine write failure (a full disk on
  `luabox schema > luabox.schema.json`) is still an error, reported as one line
  on stderr and exit 1 rather than a panic.
  `luabox lsp` gets the opposite policy for the same defect: its two stderr
  log lines used to abort the whole language server the moment a client had
  closed the log pipe (an editor restart, a torn-down output pane) while the
  user's `luabox.toml` happened to be mid-edit and invalid — a long-running
  server must *survive* a dead log pipe, so it now drops the message and
  keeps serving instead of exiting at all.
- **Diagnostics on very long lines are fast to report, and readable.** A file
  with one enormous line — minified or generated source — made reporting
  quadratic all over again, because a label's *column* was counted by walking
  characters from the start of its line. On a 377 kB single-line file with
  10 000 findings, `check` took 71 s in the human format (0.5 s as JSON), and
  the SARIF, GitHub and GitLab renderers ~2.2-2.5 s. Columns are now resolved
  by binary search like lines, so every format lands within ~1.3x of JSON on
  that input (human 0.5 s, SARIF 0.7 s), and the cost doubles when the finding
  count doubles instead of quadrupling. The human renderer also **windows**
  long source lines rustc-style, printing ~200 characters around the label with
  `...` markers rather than the whole line plus a column-wide indent — the same
  input used to produce 3.5 GB of output, and now produces 4.6 MB. Column
  numbers are unaffected in every format, and a line short enough to print
  whole is still printed whole, byte for byte.
- **`check --format gitlab` reports the line each diagnostic is actually on.**
  The GitLab Code Quality renderer discarded the source lookup it was handed
  and wrote `location.lines.begin: 1` for every finding. GitLab places a
  finding on the merge-request diff by that line and drops it when the line is
  not part of the diff, so the report parsed, looked plausible, and annotated
  nothing. It now resolves the primary label's real 1-based line through the
  same lookup SARIF's `startLine` already used; a file the lookup cannot
  supply still falls back to line 1, and a diagnostic with no label at all
  still reports the empty path and line 0. Fingerprints are unchanged — they
  hash the code, file and byte range, never the rendered line — so existing
  findings keep their identity and history in GitLab rather than all
  reappearing as new.
- **`luabox lsp` survives a malformed message instead of dying on it.** Any
  request or notification whose params did not deserialize became an error
  that propagated out of the message loop and killed the process with exit 1
  — leaving the request the editor was blocked on unanswered, and every open
  buffer without diagnostics, hover or completion until the client noticed the
  pipe had closed. A hover with no `position`, a `didOpen` missing its
  `languageId`, a `formatting` with no `options`, and — the one real clients
  actually emit — a `file://` URI containing an unencoded space were all
  fatal. A malformed **request** is now answered with the protocol's own
  `-32602 InvalidParams`, naming the method and what failed to decode, and a
  malformed **notification**, which has no id to answer, is reported on
  `window/logMessage` and dropped; either way the server keeps serving. A
  malformed `initialize` remains terminal — there is no workspace to serve —
  but the client is now told so on the id it is blocked on. Genuinely fatal
  conditions stay fatal: a closed stdin or a dead connection still ends the
  loop.
- **`luabox lsp` exits 1 on `exit` without a prior `shutdown`.** The LSP spec
  reserves exit code 0 for the ordered `shutdown`/`exit` handshake and asks
  for 1 when a client sends `exit` on its own. The lone notification was
  ignored outright, so the server lingered until its stdin closed and then
  exited 0.
- **A manifest key is never silently inert.** `rev`, `tag` and `branch` pin a
  *git* checkout, so alongside a `path` or a `url` source they described
  nothing — and were quietly dropped by both the parser and the published
  JSON Schema. `{ path = "…", rev = "…" }`, `{ path = "…", tag = "…" }`,
  `{ path = "…", branch = "…" }` and the same three next to a `url` source
  are now errors naming the source that *was* found ("has a git reference key
  but a `path` source"), batched with every other manifest error like the
  long-standing `sha256`-without-`url` and git-reference-without-`git` rules.
  The schema's `path source` and `url source` branches exclude the three keys
  by the same mechanism they already used for `git`/`url`/`sha256`, so an
  editor flags them before `luabox check` does.
- **`lint` and `check` no longer slow down as a file collects diagnostics.**
  Every finding resolved its line number by counting newlines from byte 0, so
  the cost of reporting was O(diagnostics × file size): a single 100-kLOC file
  with 32 k findings took over three minutes to lint, while the same file with
  one finding took 0.35 s. Each file now builds one line table and
  binary-searches it — 20 k suppressed findings in one file went from 15.7 s to
  0.35 s for `lint`, and from 4.3 s to 0.66 s for `check`. **Reporting** those
  findings paid the same price again, and worse: every renderer resolved each
  label by scanning the file from byte 0 for its line and column, walked it a
  second time for that line's text, and — because the source lookup hands back
  an owned `String`, read off disk by the CLI — *cloned the whole file* per
  label while doing it. Human, SARIF, GitHub Actions and GitLab output were all
  quadratic in the finding count, which left `--format json`, the one format
  that renders nothing, as the only fast way to report 32 k diagnostics from
  one 2.8 MB file: 1.4 s, against 85 s for the same run in human form. Every
  renderer now fetches each distinct file once per run and answers every label
  against one line table, byte-for-byte identically to before: on that file,
  `check` 85 s → 1.4 s (60×), `--format sarif` 82 s → 1.8 s (45×),
  `--format github` 63 s → 1.3 s (47×) and `--format gitlab` 54 s → 1.5 s
  (36×) — every one of them now within 1.3× of the `--format json` floor on the
  same input — and, on a 1.7 MB file carrying 32 k lint findings, `lint` 41 s →
  0.35 s (116×). The perf gate was structurally blind to all of this (its
  corpus reports `0 errors, 0 warnings`), so it gained a diagnostics-heavy
  fourth gate, now in two variants: findings suppressed, which times the
  bookkeeping and would have failed its budget by 13× against the old code, and
  findings rendered, which times the renderers and would have failed its budget
  by 15× (`lint`) and 6× (`check`).
- **Valid Lua with a `#!` shebang is accepted, in every edition.** Reference
  Lua has skipped a leading `#` line since 5.0 (`skipcomment`), so an
  executable script was ordinary source everywhere except here, where
  `check`/`lint`/`build` rejected it with `unexpected '!'`. The first line of a
  file that starts with `#` is now lexed as trivia, like a comment, and `fmt`
  reproduces it byte-exact and stays idempotent. A `#` anywhere below byte 0
  is still the length operator, and still an error — matching reference Lua
  exactly.

  **Bundling handles it too.** A bundle splices every module's text into one
  file, so a module's `#!` line would land in the middle of it — where `#` *is*
  the length operator, which made `luabox build --bundle` fail its own reparse
  with `internal bundler error` on any project whose entry or any required
  module was an executable script (`--minify` instead dropped the line
  silently). The prefix is now cut from every module as it is spliced, using
  the lexer's own rule, and the **entry's** `#!` line is re-emitted at byte 0
  of the bundle — plain and minified alike — so a bundled program stays an
  executable program. A dependency's shebang is dropped: it only ever meant
  "run *this* file".
- **A UTF-8 byte-order mark is accepted where reference Lua accepts it.** Lua
  gained `skipBOM` in 5.2, and LuaJIT has it too, so a BOM'd file compiles
  there and was rejected here in every edition with `unexpected '\u{feff}'`.
  The mark is now skipped as trivia under 5.2/5.3/5.4/LuaJIT (and preserved
  byte-exact by `fmt`); under 5.1, which really does reject it, the diagnostic
  now names it — "file starts with a UTF-8 byte-order mark, which Lua 5.1
  rejects — save the file without a BOM" — instead of echoing an invisible
  codepoint. A bundle strips the mark from every module it inlines and never
  emits one of its own: the bundle is a *new* file, and a `target = "5.1"`
  bundle carrying a mark would not load at all.
- **The parser accepts everything reference Lua accepts.** The nesting budget
  was 100, well under the ~197 levels of tables/parens/calls/`if`s that
  `lua5.4` compiles, and a right-associative operator chain spent one level
  *per term*, so a 100-term `"a" .. "a" .. …` was rejected while the same-length
  `+` chain was fine. Right-associative chains (`..`, `^`) are now consumed
  iteratively, at constant depth and with the same limits as `+` (10 000 terms
  parse identically to `+`), and the nesting budget is 220 — above every
  reference implementation, with measured stack headroom for a debug build on
  a default 2 MiB thread stack. See
  [LIMITATIONS.md](docs/03-reference/02-limitations.md#parser-nesting-and-expression-size-limits).

### Internal (contributors)

- **`luabox check` reads and parses each file once per run.** The cross-file
  surface pre-pass and the per-file check were two independent parallel walks
  of the source set, so every file was read twice, parsed twice, harvested
  twice and lowered three times. They now share one set of per-file records.
  `luabox-types` grew `FileArtifacts` (a file's harvest + lowering) and the
  `module_surface_with_artifacts` / `check_file_with_artifacts` entry points
  that take one; `module_surface`, `check_file_with_requires` and
  `module_requires` are unchanged wrappers, so the LSP and any other consumer
  need not care. No diagnostic, ordering or summary changes. It is not free:
  reading and parsing once means the per-file artifacts are *retained* across
  both passes instead of being dropped and rebuilt, and peak RSS on the
  100-kLOC reference corpus went from 64 MiB to 123 MiB (~1.9×). That is the
  deliberate trade — memory for I/O and CPU — and the number is here so
  nobody has to rediscover it from a profiler. **That number is now gated.**
  Accepting it was one thing; leaving it unenforced was another — `check`
  could have grown to 500 MiB on the same input with every CI gate still
  green. `scripts/perf-gate.sh` (and `perf-gate.ps1`) gained a peak-RSS leg on
  that corpus, budget 300 MiB, measured through `wait4(2)`'s rusage
  (`scripts/peak-rss.py`; `Process.PeakWorkingSet64` on Windows). It is
  deliberately *not* scaled by `LUABOX_PERF_FACTOR` — a slow machine runs the
  same allocations, it just takes longer over them — and has its own
  `LUABOX_RSS_BUDGET_MIB` override for when the budget itself is renegotiated.
- **The two things that stood behind the parser's depth limit and the
  watcher's debounce are now assertions rather than claims.** `MAX_DEPTH`
  (220) had a headroom proof for **parsing** only, in a 2 MiB thread —
  everything downstream (lowering, inference, the formatter, the bundler, the
  minifier, the renderers) walked the same 2.2×-deeper trees unmeasured. A new
  workspace test (`crates/luabox-cli/tests/deep_pipeline.rs`) drives the real
  binary — `check`, `fmt`, `fmt --check`, `build --bundle --minify
  --sourcemap` — over each construct at the depth reference Lua accepts, which
  covers the *main* thread's stack and, because it is an ordinary workspace
  test, runs on Linux, macOS **and Windows** in CI, where that stack is 1 MiB.
  Separately, `watch.rs`'s `partition_batches` model claimed agreement with
  the live `next_batch` loop and nothing checked it; both now consume one table
  of timed cases and a test requires identical batching on every one (verified
  by mutation: a model drifted to a sliding window fails it).
- **`install.sh`'s draft-release path is exercised on every push instead of
  first by a real tag.** Nothing ran that code until a `v*` push reached
  `release.yml`'s verify job — the most expensive place to find a bug in it.
  CI's new `draft-install-mock` job runs the real installer against a
  python3-stdlib mock of the release API (`scripts/tests/mock-release-api.py`),
  reached through a new CI-only `LUABOX_API_BASE` override (mirrored in
  `install.ps1`): the paginated release walk with the tag deliberately on page
  2, the asset-id 302 to a second host with the `Authorization` header asserted
  **absent**, a real `tar.gz` + `SHA256SUMS` that must verify and run, and the
  negative case. The script also gained a `wget` fallback throughout — it was
  curl-only, and the draft path bypassed even the shared download helper — with
  the cross-host token drop hand-rolled for `wget`, which forwards headers
  across redirects where `curl -L` does not. Its `jq` requirement moved to the
  `LUABOX_DRAFT_INSTALL=1` opt-in itself, so a runner without `jq` fails in
  seconds with one message rather than several API round-trips later, and
  `release.yml` asserts `jq --version` before it can get that far.
- **Releases are gated on the full e2e suite running against the *installed*
  binary.** `release.yml` now creates the release as a true **draft**, and on
  Linux, macOS and Windows it downloads the shipped install script *from that
  draft*, installs the draft's binary with it, and runs the whole black-box
  cucumber spec (`acceptance` + `lsp_acceptance`) against that installed
  executable. Only once all three legs pass does the release go
  `--draft=false --latest`; a public-URL install and `luabox upgrade` smoke
  runs afterwards, since neither can see a draft. The suites pick their binary
  at runtime from `LUABOX_E2E_BIN` (falling back to the cargo-built one), and
  `scripts/install.{sh,ps1}` gained a CI-only path — explicit
  `LUABOX_DRAFT_INSTALL=1` opt-in plus `GITHUB_TOKEN` — that resolves a draft
  release through the GitHub API; without the opt-in their behaviour is
  unchanged. See [RELEASING.md](docs/02-guides/01-releasing.md).
- **`luabox-resolve` is now `luabox-manifest`.** The crate lost its resolving
  half in this release (see *Removed*) and the name outlived it. It also
  absorbs project *layout* — root discovery, the first-party source walk and
  `[types] defs` resolution — which `luabox-cli` and `luabox-lsp` had each
  grown a separate, and separately drifting, copy of. Not published to any
  registry, so no downstream rename is needed; imports move from
  `luabox_resolve::manifest::*` to `luabox_manifest::model::*`, with the
  layout API under `luabox_manifest::layout`.

### Migration

Materialize the tree with luarocks directly, then point luabox at it:

```sh
luarocks install --tree lua_modules penlight
luabox check          # penlight is requirable and bundlable
```

Declare dependencies in your `*.rockspec` by hand (or with `luarocks`), and
publish with `luarocks upload`. Note what the tree does and does not give
you: `require` resolution and bundling come free, but a rock's *types* still
need a `[dependencies]` entry plus a `lua_modules/<name>/luabox.toml` with
`[types] defs` — which a luarocks tree does not have. Write the LuaCATS
definitions into your own `defs/` and list them in your `[types] defs`; see
[README](README.md#using-dependencies) and
[LIMITATIONS.md](docs/03-reference/02-limitations.md#dependency-management-and-execution-are-non-goals-not-gaps).

## [0.1.4] - 2026-07-14

### Added

- `luabox login [--format text|json]` — sign in to GitHub through the browser
  via the OAuth 2.0 Device Authorization Grant (RFC 8628). No scope is
  requested (an unscoped token already lifts the API rate limit; least
  privilege). `luabox` prints a `user_code` and verification URL, best-effort
  opens your browser, polls until you authorize, then stores the token
  **encrypted at rest in the OS keychain** (macOS Keychain, Windows Credential
  Manager, Linux Secret Service). `--format json` emits newline-delimited
  events (`prompt`, then `success`/`error`) for the editor extensions'
  "Sign in with GitHub" buttons to consume. This **supersedes pasting a
  Personal Access Token** into `LUABOX_GITHUB_TOKEN` — though that env var
  still works and still takes precedence.
- `luabox logout` — delete the stored token from the OS keychain (idempotent).
- `luabox whoami [--format text|json]` — report the signed-in GitHub login and
  where its token came from (`keychain`/`env`), or "not signed in" (always
  exits 0).
- `luabox search`/`outdated` (and `update`'s re-pin) now transparently use a
  keychain-stored token after `luabox login`, with no env var set. Token
  precedence is `LUABOX_GITHUB_TOKEN` → `GITHUB_TOKEN` → keychain → anonymous
  (env wins so CI and one-off overrides are always honored). A keychain that
  cannot be reached (headless/CI boxes with no secret service) degrades
  gracefully: `login` points you at `LUABOX_GITHUB_TOKEN` instead of crashing,
  and token lookup silently falls through to the env vars.

## [0.1.3] - 2026-07-14

### Added

- `luabox search [QUERY] [--format json|text]` — discover luabox packages on
  GitHub. luabox has no hosted registry (SPEC.md §6): a **package** is a public
  GitHub repo carrying the topic `luabox` **and** a root `luabox.toml`. Search
  finds candidates by topic, filters to those with a root manifest (excluding
  the toolchain/editor repos, which carry the topic but ship no manifest),
  reads each `[package] name`, and reports the latest release tag to pin. The
  `--format json` output is a stable contract the editor GUIs consume.
- `luabox outdated [--format json|text]` — report each dependency against the
  latest GitHub release of its repo. A tag-pinned git dependency is flagged
  outdated when a newer release tag exists; non-git deps and rev/branch pins
  are listed without a false "outdated" verdict. Always exits 0 (a report, not
  a gate). Also emits a stable `--format json` contract.
- `luabox update <name>` now **re-pins** a tag-pinned git dependency to its
  GitHub repo's latest release tag (comment-preserving `luabox.toml` surgery)
  before re-resolving; `luabox update` with no name re-pins every tag-pinned
  git dependency. A dependency pinned by `rev`/`branch` is left untouched (its
  pin kind is never switched silently) with a note.

  Together these give editors an npm-like dependency UX — discover, see
  what's outdated, and update with one click — over GitHub-as-registry,
  addressing the discovery half of #137 without a hosted registry. GitHub
  requests honor `LUABOX_GITHUB_TOKEN` (else `GITHUB_TOKEN`) as a bearer token,
  raising the anonymous 60 req/hr search limit to 5000/hr; everything degrades
  gracefully without one.

## [0.1.2] - 2026-07-14

### Changed

- The VS Code extension moved to its own repository,
  [flying-dice/luabox-vscode](https://github.com/flying-dice/luabox-vscode)
  (full history preserved), releasing its `.vsix` independently; a JetBrains
  plugin now lives at
  [flying-dice/luabox-jetbrains](https://github.com/flying-dice/luabox-jetbrains).
  This repo's releases carry the CLI binaries, `SHA256SUMS`, and the install
  scripts (six assets); the release gate's vsix checks moved to the
  extension repo's own pipeline.

## [0.1.1] - 2026-07-14

### Added

- `luabox upgrade [VERSION]` — replace the running binary with a GitHub
  release build: resolves the latest tag (or installs the given one),
  downloads the platform asset, verifies it against the release's
  `SHA256SUMS`, and self-replaces in place (on Windows via the
  rename-aside dance, since a running executable cannot be overwritten).
  The release pipeline's smoke gate now exercises the upgrade on all
  three OSes before a release goes `latest`.

## [0.1.0] - 2026-07-14

The first public release: the full command surface works end to end against
real Lua sources, driven by an executable spec of cucumber scenarios. Alpha
quality — see the caveats below and [BACKLOG.md](docs/04-project/01-backlog.md) for what remains
open post-launch.

### Toolchain

One static binary, one crate per bounded context (SPEC.md §16):

- `init` / `new` — scaffold a project (`--lib`/`--bin`, `--edition`).
- `check` — typecheck: LuaCATS annotations, rich table/OOP
  inference, dialect legality against `--target`, `--watch`,
  `--format json|sarif|github|gitlab`.
- `lint` — 8 type-informed rules, `---@luabox-ignore`, `--fix`.
- `fmt` — canonical formatter for `.lua`, `--check`/`--watch`.
- `build` — lower `edition → target` (goto, bitops, `<close>`/`<const>`,
  `_ENV`, integer/float semantics) with tree-shaken polyfills.
- `bundle` — single-file bundle, `--minify`, `--sourcemap` + `unmap`,
  `--mode love|nvim-plugin`.
- `test` / `bench` — **deprecated at 0.1**: luabox is a toolchain, not a
  runtime, and code coupled to its deployment environment (LÖVE, Neovim,
  OpenResty, …) cannot be faithfully executed on a bare interpreter. Both
  still work for what they can run but warn on every invocation and are
  slated for removal; `--coverage` errors out and will not be implemented.
- `run` — `[tasks]` entries or scripts via the resolved runtime.
- `add` / `remove` / `install` / `update` / `vendor` — PubGrub resolver,
  `luabox.lock`, content-addressed store with hard-link installs;
  path/git/`luarocks/*` dependencies plus writable `file://`/directory
  registries (see "Dependencies & registries" below — hosted registry is
  post-0.1).
- `publish` / `audit` — registry publish with yank; advisory-DB audit.
- `toolchain` — install/pin/list managed Lua runtimes.
- `lsp` — language server (see "LSP & editor integrations" below).
- `doc` — static docs generated from annotations.
- `explain LBnnnn` — rustc-style diagnostic pages.

### Type checking

Types come from full LuaCATS annotation support (`---@class`, `---@field`,
`---@param`, `---@return`, `---@generic`, `---@alias`, `---@enum`,
`---@meta` definition packages) — the one and only type format. Rich table
inference is unconditional: tables never degrade to a bare `table` type,
per-field shapes are inferred from constructors and subsequent assignments,
and idiomatic `setmetatable`/`__index` OOP resolves without annotations.

The direction (see [DIRECTION.md](DIRECTION.md), decided 2026-07-11) is
**LuaCATS-native strict checking**: luabox verifies what lua-language-server
declares but trusts — real generics, cross-package type sharing, `---@class`
conformance, undefined-global detection. All of those parity/strictness
items landed and were probe-verified (#84, #90, #103, #107, #108), followed
by a checker-deepening wave: workspace-global `---@alias` with cyclic-alias
diagnosis (LB0314, #110/#123), alias parity — nested literal unquoting and
generic aliases (#116, #117) — `:`-method-call receiver resolution through
class shapes (#118), unmatched overloaded calls reported against the
closest overload (#119), contextual (bidirectional) typing of
function-literal parameters (#120), union exhaustiveness for `if`/`elseif`
chains (LB0315, #121), `---@operator call` (#122), generic-arity checking
for generic `---@class<T>` (LB0313, #124), member visibility
`---@private`/`---@protected`/`---@package` (LB0312, incl. bare
`Carrier.method = fn` assignment carriers), `---@operator`
overloads in inference, and `deprecated`/`discard-returns`/duplicate-doc
diagnostics (luals parity).

With that, the **full LuaCATS tag vocabulary is enforced** — the last
parsed-but-ignored tags now check: legacy `---@vararg` (wired to inference,
unioning with `---@param ...` per luals), `---@async` (luals `await-in-sync`,
LB0316; the main chunk counts as async), `---@version` (edition gating at
use sites riding the `deprecated` diagnostic, with luals's `>`/`<`/`JIT`
grammar and 5.1⇒LuaJIT rule), `---@source` (goto-definition redirect), and
`---@see` (hover + docgen "See also"). Contextual typing also deepened:
expected types flow into table literals, `return` positions, and nested
function-literal layers (luals `compileNode` parity).

### Dialects & lowering

Parse, typecheck, lint, and format Lua 5.1, 5.2, 5.3, 5.4, and LuaJIT.
`luabox build --target` lowers the dialect you write (`edition`) down to
the one you ship (`target`) — 5.4 → 5.3 → 5.2 → 5.1 — restructuring
`goto`/labels, shimming bitops/integer-division, rewriting `<close>`/
`<const>` scope-exits, and translating `_ENV`, with tree-shaken polyfills
injected only where used. Luau is explicitly out of scope.

### LSP & editor integrations

`luabox lsp` (stdio) is a full-featured language server over a
salsa-incremental database shared with `check`/`lint`/`fmt`: diagnostics
(type + lint) with quick-fixes and autofixes, completion with auto-require
import (#134), hover, goto definition/type-definition/implementation
(#132), find-references (#125), rename with prepareRename (#126),
document & workspace symbols (#131), signature help (#127), type-driven
code actions (#129), call hierarchy (#130), document highlight, folding
and selection ranges (#133), inlay hints, semantic tokens, document and
range formatting, plus protocol maturity — incremental sync, config
reload, file watching, and progress reporting (#135). `require`
resolution is single-sourced across `check`, `bundle`, and the LSP, so
goto-definition on a `require(...)` lands on the same module the checker
and bundler resolve.

One editor integration wraps it: VS Code (`editors/vscode/`), a
first-class TypeScript extension. (Neovim, JetBrains, and Zed
integrations were removed for now — any LSP client can be pointed at
`luabox lsp` manually.)

### Reliability

Restriction-class clippy lints (`unwrap`/`expect`/`panic`/`string_slice`)
are enforced on production code, and the panics they surfaced are fixed:
UTF-8-boundary slicing in the `add` spec parser and in docgen, unbounded
JSON nesting (now depth-limited), and integer overflows in
`---@version` arithmetic and the content-addressed store. Malformed input
now yields a diagnostic rather than aborting. Alongside this, a
clean-code/idiomatic-Rust drawdown consolidated duplicated logic —
project discovery, the Lua file walker, manifest parsing, the
diagnostics-render epilogue, and require resolution — behind single
shared helpers, and replaced ad-hoc `anyhow`/`String` errors with typed
error enums in the store and bundle crates.

### Release machinery

LICENSE (MIT), CI on GitHub Actions (`.github/workflows/ci.yml`) mirrored by
an internal GitLab pipeline for check/test, one-line install scripts for
Linux/macOS/Windows, and the release process this changelog is part of (see
RELEASING.md).

### Distribution

Shipped as [GitHub releases](https://github.com/flying-dice/luabox/releases):
each `v*` tag builds prebuilt binaries (Linux x86_64, macOS Apple Silicon,
Windows x86_64) and the VS Code `.vsix`, publishes them with `SHA256SUMS` and
the one-line installers as release assets, then **smoke-installs on all three
OSes before marking the release `latest`** — a release that fails any smoke
install does not go live. Marketplace/Open VSX publishing of the `.vsix`
remains a manual, credential-gated follow-up (#102).

### Known limitations

- Not yet published to any package registry (crates.io, Homebrew, etc.);
  install a tagged release binary or build from source.
- No hosted first-party dependency registry; `LUABOX_REGISTRY` must point
  at a writable directory or `file://` root.
- `luabox test --coverage` is not implemented.

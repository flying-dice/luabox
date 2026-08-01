# Changelog

All notable changes to this project are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versioning follows [SemVer](https://semver.org/), with the 0.x caveats
spelled out in [RELEASING.md](docs/02-guides/01-releasing.md#semver-policy-for-0x).

## [Unreleased]

### Fixed

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

  `luabox build` keeps its edition-only check gate on purpose (lowering is
  what handles constructs the target rejects), but nothing lowers a shadowed
  label away — so the same program built with `--target 5.4` silently emitted
  an unloadable file and exited 0. The residual validation of each lowered
  file now judges control-flow legality under the target too, and refuses to
  write anything when it fails. `luabox lint` and the language server have no
  target flag and are unaffected.
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

  Measured on a penlight-scale annotated tree (50 files, ~103 kLOC of
  `---@class` Lua under `lua_modules/share/lua/5.4/`), driving the real stdio
  protocol and timing `initialize` to the **first** `publishDiagnostics`, 4
  cores, 7 runs: **2270 ms → 654 ms median** (2222 ms → 574 ms min), a 3.5x
  cut. The same project with no rock tree publishes in 8 ms, so the harvest
  was effectively the whole wait. The number is now in the code comment at
  `harvest_rock_tree`, along with the judgment that the harvest stays on the
  startup path: an asynchronous republish would trade the remaining ~650 ms
  for a window in which rock-typed code is diagnosed against an empty rock
  layer, flashing `LB0305`/`LB0306` and then clearing them.

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

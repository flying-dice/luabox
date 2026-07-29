# Type-system direction (decision record)

Status: **accepted** (2026-07-11). Supersedes the earlier `.luab` shape DSL
direction (SHAPES-V2, removed under #109).

## North star

**LuaCATS is the one type format; luabox checks it more strictly than
lua-language-server; luabox-specific keyword extensions come only after
launching at feature parity.**

Three commitments, in order:

1. **Interop first.** The annotation format is LuaCATS (`---@class`,
   `---@field`, `---@alias`, `---@enum`, `---@param`, `---@return`,
   `---@generic`, `---@meta` def packages). Existing annotated codebases,
   community definition packages, and luals-compatible tooling all work
   unchanged. There is no competing luabox file format.
2. **Stricter.** luabox's edge is that it *verifies* what luals declares but
   trusts — real conformance, real generics, cross-package checking — on the
   same format. "Match/exceed luals."
3. **Keywords later.** Any luabox-specific syntax (new keywords, extensions)
   ships **only after** going live at feature parity, as extensions *on the
   LuaCATS base* — never as a second file type.

## What this drops

The **`.luab` shape DSL is dropped/parked.** It is precisely the "new
keywords" that commitment 3 defers. Keeping it now would ship the very
two-format problem this direction exists to avoid. Its *engines* are not
wasted: the monomorphization (`subst_ty`/`instantiate`) and structural
conformance / function-subtyping machinery are re-pointed onto the LuaCATS
front-end (see the launch gate). `.luab` is removed once the LuaCATS path
reaches parity — the removal is a *consequence* of parity, not a precondition
for starting the work.

## Launch gate: feature parity + strictness

"Feature parity" is testable: **everything luals verifies, plus luabox's
strictness.** Measured against the gap map the example conversion produced
(commit `a01684b`; all verified missing today). Two tracks, both gating
launch:

### Parity (match luals so interop is real)
- **[#84] generics** — `---@generic` functions and generic `---@class<T>`
  (both broken today; reuse `.luab`'s monomorphization engine).
- **[#108] cross-package type sharing** — a dependency's types visible and
  checked in a consumer (LuaCATS has none today; `.luab` had it).

### Strictness (exceed luals — the whole point)
- **[#107] `---@class` conformance** — a `: Interface` carrier must provide
  the interface's methods with compatible signatures, `__index`-aware so
  inheritance isn't wrongly flagged (not enforced today; re-point the
  deferred-conformance + function-subtyping engines off `---@type`/`.luab`).
- **[#103] undefined-global** — flag typo'd/unknown global reads (no rule
  today; the parsed `---@diagnostic disable: undefined-global` is a no-op).
- Retained from the SHAPES-V2 work, re-pointed onto LuaCATS: function
  signature subtyping, carrier `self`-inference, member-naming diagnostics,
  declaration-site labels, literal freshness.

Smaller correctness items surfaced along the way: **[#105]** def scalar
fields untyped, **[#106]** call return not propagated to unannotated locals.

## Sequencing

1. Build the parity + strictness items on the LuaCATS front-end (reusing the
   `.luab` engines).
2. Land the release machinery (LICENSE, install, CI, quickstart — see the
   Initial-public-release milestone).
3. **Go live** at feature parity + strictness.
4. Remove `.luab` (subsystem, tree-sitter grammar, editor `.luab` support,
   SHAPES-V2.md) once nothing depends on it. **Done (#109): the `.luab`
   subsystem was removed.**
5. **Post-launch:** add luabox keyword extensions on the LuaCATS base.

## Non-goals (for now)

- A second type file format.
- New keywords before launch.
- Nominal/`.luab`-style structural declarations as the authoring surface.

---

# v1 scope cut (accepted 2026-07-26)

Status: **accepted** (2026-07-26), owner decision. Scopes down — but does
not reverse — the "luarocks.org is the registry" pivot (#2). Tracked as
flying-dice/luabox#10 (dependency management), #11 (`run`/`toolchain`), #12
(docs) and #13 (gates); the evidence is the quality baseline in
[PRODUCTION-READINESS.md](docs/04-project/02-production-readiness.md).

## North star

**luabox consumes a rock tree; it does not produce one — and it never spawns
an interpreter.**

v1 is a purely static toolchain: parse, typecheck, lint, format, lower,
bundle, document, and serve LSP. Three things are out, and these are the
load-bearing claims — the ones a user can rely on and a reviewer should hold
us to:

1. **It never spawns an interpreter.** Your Lua is read, never run.
2. **It never fetches or resolves packages.** No solver, no lockfile, no
   registry client, no downloads on your project's behalf.
3. **It holds no credential.** No login, no token store, no keychain entry.

Three commands do start a child process, and none of them weakens any of the
three claims:

- **`upgrade`** replaces the running binary with a GitHub release —
  anonymously, via `curl` and `tar`, on explicit request. It is the toolchain
  updating itself, not the toolchain acting on your project.
- **`doc --open`** hands the `index.html` it just generated to the platform's
  browser opener (`xdg-open`/`open`/`start`).
- **`build --mode love`** shells out to a zip tool to package the `.love`
  archive — `zip`, else `python3 -m zipfile`, else `python -m zipfile` on
  Unix; the System32 `bsdtar`, else PowerShell's `Compress-Archive`, on
  Windows. It archives the build output luabox just produced, executes no
  Lua, and when no tool is present it fails loudly naming the ones it tried
  rather than emitting a half-made archive.

Stating it as "spawns no process at all" was the tidier sentence and the false
one; the three claims above are what actually holds.

## What this cuts

- **[#10] dependency management** — `add`/`remove`/`install`/`update`/
  `vendor`, `search`/`outdated`, `publish`, `login`/`logout`/`whoami`, and
  behind them the PubGrub solver, the git/url/http/luarocks providers,
  `luabox.lock`, the rockspec editor, the GitHub device flow, the OS
  keychain, and the whole `luabox-store` CAS crate.
- **[#11] execution** — `run` and `toolchain` (interpreter *and* luarocks
  provisioning). luabox acquires no runtime and runs no user code; the earlier
  "nvm/rustup for Lua" framing is withdrawn with them.

## Why

The entanglement was favorable — there was a clean amputation line, and the
core never crossed it. `luabox-lint`, `luabox-lsp` and the frontend commands
consume only the `luabox.toml` manifest model plus a *materialized*
`lua_modules/` tree; they never touched the solver, the providers, the
store, or the luarocks bridge.

The numbers said which side was ready. The core — parser, formatter, linter,
checker, LSP — sits at **92–95% line coverage**, clean under pedantic clippy,
driven by a spec-first acceptance suite. The dependency layer was ~17% of
the workspace carrying the *worst* coverage in it (`deps_cmd` 68.6%,
`outdated_cmd` 60.1%, `keychain.rs` 40.6%, the providers 71–78%), the entire
credential surface, and all of the live-network coupling — including the
suite's only scenario that could fail for environmental rather than product
reasons. Finishing it competed directly with nailing the core.

Deleting beats hiding. A feature flag or a hidden-command quarantine would
have kept every cost the cut exists to shed — build/test/clippy time, the
credential surface, the drag on every refactor that touches shared types —
while only appearing to shed them. Git history makes deletion as reversible
as a flag in practice; this repo has done exactly that before (the `.luab`
subsystem, #109).

## What the seam keeps

Cross-package types are not collateral damage. What survives is the *read*
side:

- the `luabox.toml` manifest model (`[package]`, `[lint]`, build config),
  used by every frontend command — `luabox-resolve` slims to
  manifest/project/dialect;
- the **`lua_modules/` read path**, so `require` resolution and
  cross-package type checking keep working over a tree the user materializes
  themselves: `luarocks install --tree lua_modules <rock>`, then `luabox
  check`.

Users keep the whole ecosystem; luabox stops being the thing that fetches
it.

**Decision (2026-07-29): the manifest contract is single-sourced from a
declarative table, not from serde.** One const table in `luabox-manifest`
(`contract.rs`) drives both the parser's key allowlists/did-you-mean and the
generated JSON Schema that `luabox schema` prints. A serde-derive route was
evaluated and rejected: no serde-based engine delivers the trio the parser
guarantees — every error batch-collected in one pass (`eserde` gets this
far), a byte span per error for rustc-style rendering (`toml_edit` spans;
serde derives erase them), and did-you-mean across keys *and* enum values
plus the removed-table nudges and cross-key dependency rules. Revisit only
if a serde-compatible engine grows span support and suggestion hooks.

## What still stands

The **luarocks.org-as-registry direction (#2) is unchanged** — only its
*scope* is parked. If dependency management returns after v1 it returns on
luarocks.org, with the rockspec as the package manifest, never on a
first-party registry. Registry UX (#137 and neighbours) is post-v1 by the
same token.

## Non-goals (v1)

- Resolving, installing, vendoring, or publishing packages.
- Credential storage, sign-in flows, and authenticated requests.
- Acquiring, pinning, or spawning a Lua interpreter (or a luarocks). (`upgrade`
  fetching luabox's *own* release, `doc --open` launching a browser, and
  `build --mode love` packaging via a zip tool are the three deliberate
  exceptions — see the north star above.)

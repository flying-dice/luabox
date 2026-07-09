# luabox — Unified Lua Toolchain

**Name:** `luabox`. CLI binary: `luabox`, alias `lb`.
**One-line:** cargo + rustup + rust-analyzer + bun, for Lua. Not a runtime. Ever.

---

## 1. Vision

- One static binary. Zero-install-friction (curl | sh, brew, scoop, mise). Written in Rust.
- Bun's ethos: one tool, instant startup, batteries included, obscene speed.
- Rust's ethos: correctness-first, one blessed workflow, first-class diagnostics, stability guarantees.
- LuaLS/sumneko annotation dialect is the **type-system lingua franca** — no new annotation syntax invented.
- Runtimes are pluggable externals (`lua5.1`, `lua5.4`, `luajit`, `lune`, love2d, Roblox, OpenResty, Neovim). Luabox compiles/checks/bundles *for* them, never *is* them.

### Non-goals

- No interpreter/VM. No REPL beyond delegating to a configured runtime.
- No new type annotation language (consume LuaLS + Luau native types; emit both).
- No LuaRocks replacement-by-fiat — interop first, supersede by being better.

---

## 2. Supported dialects & targets

| Dialect | Parse | Typecheck | Lint | Format | Downgrade from | Downgrade to |
|---|---|---|---|---|---|---|
| Lua 5.1 | ✓ | ✓ | ✓ | ✓ | — (floor) | — |
| Lua 5.2 | ✓ | ✓ | ✓ | ✓ | ✓ | 5.1 |
| Lua 5.3 | ✓ | ✓ | ✓ | ✓ | ✓ | 5.2, 5.1 |
| Lua 5.4 | ✓ | ✓ | ✓ | ✓ | ✓ | 5.3, 5.2, 5.1 |
| LuaJIT (5.1+ext) | ✓ | ✓ | ✓ | ✓ | ✓ | 5.1 |
| Luau | ✓ | ✓ (native types) | ✓ | ✓ | ✓ | 5.1–5.4, LuaJIT |

### 2.1 `target` — TSC-style emit lowering

`edition` = dialect you write. `target` = dialect you ship. Compiler lowers the delta.

Lowering matrix (representative, not exhaustive):

| Feature | Source | Lowering strategy |
|---|---|---|
| `goto`/labels (5.2+) | →5.1 | control-flow restructure (loop/flag rewrite); error if irreducible |
| Integer division `//`, bitops `& \| ~ << >>` (5.3+) | →5.2/5.1 | `math.floor(a/b)`, `bit32`/`bit` shim, polyfill lib injection |
| `<close>`/`<const>` (5.4) | →5.3- | scope-exit rewrite via pcall wrapper / plain local + const-check at compile time |
| `_ENV` (5.2+) | →5.1 | `setfenv`/`getfenv` rewrite |
| Luau type syntax | →any | full erasure (types already checked upstream) |
| Luau `continue` | →5.x | loop restructure |
| Luau compound assign `+=` | →5.x | expand to `a = a + b` |
| Luau string interpolation `` `x{y}` `` | →5.x | `string.format` rewrite |
| Luau `if-then-else` expressions | →5.x | `and/or` chain or IIFE (semantics-preserving; no falsy-nil bug) |
| Integer/float semantics (5.3+) | →5.1/JIT | diagnostic tiers: warn on observable divergence, error on proven divergence |

- Polyfills: tree-shaken, single injected `__luabox_rt` module, deduplicated across bundle. Zero-cost when unused.
- Non-lowerable constructs (e.g. true `<close>` semantics under error in 5.1 coroutines): hard diagnostic with explanation + escape hatch annotation `---@luabox-allow lossy-lowering`.
- Prior art acknowledged: darklua (Luau→Lua transforms). Luabox subsumes, adds semantics-preservation proofs per rule.

---

## 3. Type system

- **Source of truth:** LuaLS annotations (`---@class`, `---@field`, `---@param`, `---@return`, `---@generic`, `---@alias`, `---@overload`, `---@type`, `---@cast`, `---@enum`, `---@meta`). Full dialect compatibility — existing annotated codebases check day one.
- Luau files: native syntax types. Internal type IR unifies both; cross-calling between annotated Lua and Luau is fully checked.
- **Definition packages:** `@types/*`-style. `*.d.lua` files (`---@meta` modules) distributed via registry. Runtime API defs shipped for: 5.1–5.4 stdlib, LuaJIT ext, Luau, LÖVE, Neovim, OpenResty, Roblox.
- Strictness ladder (per-package, per-file override): `none` → `warn` → `strict` (untyped = `unknown`, not `any`) — migration path for legacy code.
- Inference: bidirectional, flow-sensitive narrowing (`if type(x) == "string"`), literal types, generics with constraints. Match/exceed Luau checker on Luau code; match/exceed LuaLS on annotated Lua.
- **Rich table inference — hard requirement.** Tables never degrade to a bare `table` type. The IR models table *shapes* structurally, and inference maintains them without annotations:
  - Per-field shapes from table constructors and subsequent assignments (`t.x = 1` extends/refines the shape; sealed vs unsealed per strictness level).
  - Array part vs hash part vs mixed distinguished; element types for `t[i]`, and `pairs`/`ipairs`/`next` iteration typed from the shape.
  - Metatable semantics: `setmetatable`/`__index` chains (table and function forms) resolve field lookup, so idiomatic OOP (`Class.__index = Class`, `:` methods, inheritance chains) types correctly without annotations.
  - Literal-keyed indexing narrows (`t["x"]` ≡ `t.x`); dynamic keys fall back to indexer types, not `any`.
  - Inferred shapes unify with declared `---@class`/`---@field` and Luau table types: missing/excess-field diagnostics per strictness level, width subtyping for function arguments.
- `luabox check` = standalone typecheck, CI-grade, machine-readable output (JSON, SARIF, GitHub/GitLab annotations).

---

## 4. `luabox` CLI surface

```
luabox init [--lib|--bin] [--edition luau|5.4|...]   scaffold in cwd
luabox new <name>                                     scaffold new dir
luabox add <pkg>[@version] [--dev]                    dep management
luabox remove <pkg>
luabox install                                        resolve + fetch (lockfile-driven)
luabox update [pkg]
luabox check [--target <t>]                           typecheck
luabox lint [--fix]                                   clippy analog
luabox fmt [--check]                                  canonical formatter
luabox build [--target <t>] [--out dir]               lower + emit
luabox bundle [--minify] [--sourcemap]                single-file emit
luabox test [pattern] [--watch] [--coverage]          built-in runner
luabox bench                                          built-in benchmarks
luabox run <script|task>                              run via configured runtime / tasks
luabox doc [--open]                                   docs from annotations
luabox publish                                        registry publish
luabox lsp                                            stdio LSP server
luabox toolchain [install|pin|list]                   runtime version mgmt (rustup analog)
luabox vendor                                         vendor deps into tree
luabox audit                                          advisory DB check
luabox explain LB0xxx                                 rustc-style diagnostic docs
```

Bun-style ergonomics: every command cold-starts < 50 ms; watch mode on check/test/build; `luabox run` resolves package tasks then `$PATH`.

---

## 5. Project manifest — `luabox.toml`

```toml
[package]
name = "my-lib"
version = "1.2.0"
edition = "luau"            # dialect you write
description = ""
license = "MIT"

[build]
target = "5.1"              # dialect you ship (tsc target)
out = "dist"

[types]
strict = true
defs = ["love2d"]           # ambient definition packages

[dependencies]
penlight = "1.14"
promise = { git = "https://…", rev = "abc123" }

[dev-dependencies]
busted-compat = "1.0"

[tasks]                     # bun/npm-scripts analog, cargo alias analog
start = "luabox run src/main.lua"
ci = ["luabox check", "luabox lint", "luabox test"]

[workspace]                 # monorepo, cargo-style
members = ["packages/*"]
```

- **Lockfile:** `luabox.lock` — content-addressed, hashes every artifact, deterministic resolution. Text-based (bun learned this the hard way — no binary lockfile).
- Workspaces: shared lockfile, path deps, `luabox check --workspace`, task fan-out.

---

## 6. Package manager

- **Resolution:** full semver, PubGrub solver (cargo-quality error messages on conflicts).
- **Registry:** new first-party registry (static-CDN index, git-index protocol like crates.io sparse index) **plus** transparent LuaRocks bridge — `luarocks/penlight` resolves via rockspec translation. Adoption requires the existing corpus.
- **Store:** global content-addressed cache (`~/.luabox/store`), hard-link/reflink into projects — bun/pnpm model. Install speed is a feature.
- Dep kinds: registry, git (rev/tag/branch), path, workspace.
- Per-dialect compatibility metadata: packages declare `lua-versions = ["5.1", "luau"]`; resolver refuses incompatible graphs *or* auto-selects downgrade-built artifacts (packages published with pre-lowered variants).
- `luabox publish`: builds, checks, runs tests, verifies annotations coverage on public API, signs (sigstore), pushes. Yank support, no deletion (crates.io rule).
- `luabox audit`: advisory database, RUSTSEC analog.
- Binary deps (C modules): **declared, not built** — luabox resolves prebuilt artifacts per runtime/platform from registry; falls back to luarocks build path with loud warning. Luabox is not a C build system.

---

## 7. Bundler

- Input: entry module(s). Output: single `.lua` file per entry, target-lowered.
- Require-graph resolution: static `require("x")` inlined into module map with lazy init (preserves load-order semantics and cycles exactly as multi-file); dynamic requires diagnosed, overridable allowlist.
- Tree-shaking: module-level always; statement-level for provably side-effect-free module bodies (annotation `---@luabox-pure` opt-in for aggressive mode).
- Minify: identifier mangling (scope-aware), whitespace, string/number folding. Property names never mangled (Lua tables are the API).
- Source maps: custom `.lua.map` format + LSP support for mapped stack traces; `luabox unmap <traceback>` decodes production errors.
- Targets/profiles: `dev` (readable, asserts kept) / `release` (minified, asserts stripped via `---@luabox-assert` markers).
- Embedding modes: plain chunk, LÖVE fused, Roblox model file (`.rbxm` via emitted module tree), Neovim plugin layout.

---

## 8. Language server — `luabox lsp` (rust-analyzer mirror)

Architecture:

- Salsa-style incremental computation database. Every query memoized, fine-grained invalidation. No full re-analysis on keystroke.
- Lossless syntax tree (rowan-style green/red trees) — comments/whitespace preserved; same tree feeds formatter, linter, fixes, refactors. **One parser for the whole toolchain.**
- Error-resilient parsing: broken code still gets completion/hover.
- VFS layered over disk + editor overlays; workspace-wide symbol index built in background, memory-mapped persistent cache.

Feature parity checklist (rust-analyzer baseline):

- Completion: context-aware, type-driven, auto-require insertion (import-on-completion), postfix snippets (`x.if`, `x.pcall`, `x.for`).
- Hover: rendered types + docs, layout of `---@class` fields.
- Go-to def/type-def/impl (metatable `__index` chains resolved), find-references, rename (workspace-wide, string-require-aware).
- Inlay hints: parameter names, inferred types, implicit `self`.
- Semantic tokens, document/workspace symbols, folding, selection ranges.
- Code actions: add annotation from inferred type, extract function/local, inline, convert `.`↔`:` call with self-check, generate `---@class` from table literal, sort requires, add missing fields.
- Diagnostics: streamed, typecheck + lint unified, quick-fixes attached.
- Call hierarchy, signature help, on-type formatting.
- `--stdio` + TCP; first-class VS Code extension; Neovim builtin-LSP config shipped; JetBrains via LSP API.

---

## 9. Linter (clippy analog)

- Rule tiers: `correctness` (deny), `suspicious`, `perf`, `style`, `pedantic` (opt-in) — clippy taxonomy.
- Representative rules: shadowing, unused (vars/params/requires), global reads/writes (`strict-globals`), `nil`-index proven paths, string concat in loop (`table.concat` hint), `pairs` on array (5.x perf), truthiness footguns (`x == false` vs `not x`), missing `local`, dialect-portability lints (`gotcha: # on sparse table`).
- All rules typed-informed — linter runs on the same analysis DB as the checker. No regex lints.
- `--fix`: machine-applicable fixes via the lossless tree.
- Config in `luabox.toml [lint]`; per-line `---@luabox-ignore rule-id reason` (reason mandatory — rust's `#[allow]` regret fixed).
- Prior art: selene, luacheck. Delta: type-aware, autofix, one analysis pass.

## 10. Formatter

- StyLua-compatible default style; canonical, few knobs (rustfmt philosophy: max ~6 options — line width, indent, quote style, call-parens, trailing tables comma, end-of-line).
- Range formatting, format-on-save via LSP, `fmt --check` for CI, stable output guarantee (idempotent, version-pinned in lockfile so CI and dev never fight).

## 11. Test runner & bench

- Bun-test-style: built-in, zero-config discovery (`*_test.lua`, `*.test.lua`, `tests/`).
- API: busted-compatible shim (`describe/it/assert`) + native flat API (`test("name", fn)`).
- Executes on the **configured runtime** for the target matrix: `luabox test --matrix` runs suite against 5.1/5.4/luajit in parallel — the killer feature for library authors promising cross-version support.
- Watch mode, filtering, snapshot tests, coverage (via source-map-aware instrumentation, since we own the emit), JUnit/JSON reporters.
- `luabox bench`: criterion-style statistical benchmarking across runtimes.

## 12. Toolchain manager (rustup analog)

- `luabox toolchain install 5.4.6` / `luajit-2.1` / `luau-0.xxx` — fetches prebuilt runtime binaries per platform into `~/.luabox/toolchains`.
- `luabox.toml` pins runtime for `run`/`test`; `luabox-toolchain.toml` override file (rust-toolchain.toml analog).
- Not a runtime — an acquirer of runtimes. Distinction preserved.

## 13. Docs

- `luabox doc`: static site from LuaLS annotations + markdown in doc comments. rustdoc quality bar: search, cross-linked types, examples as tested code blocks (`luabox test --doc`).
- Registry auto-hosts docs per published version (docs.rs analog).

---

## 14. Diagnostics culture

- Every error has a code (`LB0421`), a `luabox explain` page, a span-rich rendering with labels/suggestions. Rustc's empathy, verbatim.
- Machine formats: JSON, SARIF, GitHub Actions, GitLab Code Quality.

## 15. Stability & governance

- Toolchain semver: breaking lints/format changes only on major; `edition`-style opt-in for behavior changes.
- MSRV analog: `min-luabox-version` in manifest, resolver-respected.
- RFC process for language-facing decisions (annotation extensions go upstream to LuaLS first — we follow, not fork).

---

## 16. Architecture

Cargo workspace, one crate per bounded context. Crates communicate through published traits/types at the boundary only — no reaching into a sibling's internals, no shared mutable state, no "utils" dumping ground. Dependency direction is acyclic and enforced (`cargo-deny` bans + CI check on the crate graph).

```
crates/
  luabox-syntax      lossless parser, all dialects, one grammar w/ feature flags
  luabox-hir         desugared IR, name resolution
  luabox-types       unified type IR (LuaLS ⊕ Luau), inference engine
  luabox-db          salsa incremental database (shared: check/lint/lsp/fmt)
  luabox-lower       target lowering + polyfill injection (the tsc bit)
  luabox-bundle      require-graph, tree-shake, minify, sourcemaps
  luabox-resolve     PubGrub solver, registry + luarocks bridge
  luabox-store       CAS cache, fetch, verify
  luabox-lsp         server over luabox-db
  luabox-test        runner, matrix orchestration, coverage
  luabox-cli         thin frontend
```

- Single analysis database backs check, lint, LSP, fmt, doc — computed once, used everywhere. This is the rust-analyzer lesson; bolt-on tools (luacheck+stylua+LuaLS as separate binaries) each re-parse the world.
- Parallelism: rayon per-module; incremental across runs via on-disk salsa cache.

Bounded contexts (DDD terms — each crate owns its domain language):

| Context | Crates | Owns | Boundary contract |
|---|---|---|---|
| Syntax | `luabox-syntax` | grammar, lossless trees, dialect feature-gating | tree types + parse API; nothing above knows token details |
| Semantics | `luabox-hir`, `luabox-types`, `luabox-db` | name resolution, type IR, inference, incremental queries | query interface (salsa DB traits) |
| Emit | `luabox-lower`, `luabox-bundle` | lowering rules, polyfills, require-graph, sourcemaps | takes checked HIR in, bytes out; cannot influence checking |
| Distribution | `luabox-resolve`, `luabox-store`, registry client | manifests, solver, lockfile, CAS, luarocks bridge | package graph API; knows nothing of syntax/types |
| Execution | `luabox-test`, toolchain mgr | runtime acquisition, matrix orchestration, coverage | runtime handle abstraction; only context allowed to spawn runtimes |
| Frontend | `luabox-cli`, `luabox-lsp` | UX, protocol, rendering diagnostics | consumes all contexts; owns none of their logic |

### 16.1 Implementation — Rust

- Single `luabox` bin crate; context crates published for embedders (e.g. third-party editors linking `luabox-syntax`).
- Key dependencies: `rowan` (lossless syntax trees), `salsa` (incremental queries), `pubgrub` (resolver), `tower-lsp` or hand-rolled `lsp-server` (rust-analyzer uses the latter — follow it, less magic), `rayon`, `notify` (watch), `clap` (CLI), `serde`/`toml_edit` (manifest round-tripping — `luabox add` must preserve comments).
- Release profile: LTO fat, `codegen-units=1`, panic=abort, stripped. Musl static build for Linux; universal binary macOS; MSVC Windows. One artifact per platform, no dynamic deps.
- Perf gates in CI: cold start < 50 ms, `check` on 100-kLOC corpus < 1 s warm, LSP keystroke-to-diagnostics < 100 ms p95. Regressions block merge — speed is a spec'd feature, not an aspiration.
- Fuzzing: parser + lowering under `cargo-fuzz`; lowering rules additionally verified by differential execution (run source and lowered output on real runtimes, compare) in CI.

### 16.2 Testing strategy

Test pyramid, strictly layered:

- **Unit** — per crate, `#[cfg(test)]`, boundary-internal. Fast, no I/O.
- **Acceptance — cucumber (`cucumber-rs`)** — the primary spec-level layer. Every user-facing behaviour in this document maps to a `.feature` file; Gherkin scenarios ARE the executable spec. Organised per bounded context under `tests/features/<context>/`:
  ```gherkin
  Feature: Target lowering — goto
    Scenario: goto lowered to 5.1
      Given a project with edition "5.4" and target "5.1"
      And a source file using goto/labels
      When I run "luabox build"
      Then the emitted code contains no goto statements
      And differential execution on lua5.4 and lua5.1 produces identical output
  ```
  Step definitions live with the frontend context; they drive the real CLI binary against temp-dir fixture projects — black-box, no internal API shortcuts. New feature = feature file first (BDD, not test-after).
- **Differential execution** — lowering correctness against real runtimes (see above); wired in as cucumber `Then` steps where scenario-relevant, plus a standalone corpus sweep.
- **Fuzz + property** — `cargo-fuzz` (parser, lowering), `proptest` (resolver invariants: lockfile determinism, solver idempotence; formatter idempotence: `fmt(fmt(x)) == fmt(x)`).
- **Perf gates** — CI-blocking benchmarks (§16.1).

Gherkin discipline: scenarios declarative (behaviour, not CLI flags for their own sake), one behaviour per scenario, `Scenario Outline` for the dialect × target matrix — the lowering table in §2.1 becomes one outline with an examples table per row.

---

## 17. Prior art / positioning

| Tool | Covers | Luabox delta |
|---|---|---|
| LuaRocks | packages | lockfiles, CAS store, solver, speed; bridged not fought |
| lux | packages (Rust rewrite) | luabox = whole toolchain, not PM only |
| LuaLS | LSP/types | incremental salsa core, unified with lint/fmt/build |
| selene / luacheck | lint | type-aware rules, autofix |
| StyLua | fmt | same tree as analysis, style-compatible |
| darklua | Luau lowering | full 5.x matrix, semantics-preservation guarantees |
| busted | test | zero-config, runtime matrix, coverage via own emit |
| aftman / rokit / hererocks | toolchains | integrated, manifest-pinned |
| tsc | target lowering | the model, applied to Lua dialect lattice |
| cargo / rustup / rust-analyzer / clippy | everything | the blueprint |
| bun | DX, speed, one-binary | the temperament |

---

## 18. Phasing

1. **P0 — Core:** parser (all dialects), luabox.toml, `init/fmt/check` (LuaLS-annotation subset), CLI skeleton. Formatter ships first — cheapest trust-builder.
2. **P1 — Types & LSP:** full inference, salsa DB, `luabox lsp` with completion/hover/diagnostics/goto.
3. **P2 — Packages:** resolver, store, lockfile, luarocks bridge, `add/install/publish`, registry MVP.
4. **P3 — Build:** lowering matrix, bundler, sourcemaps, `build/bundle`.
5. **P4 — Runner-adjacent:** test matrix, bench, toolchain manager, coverage.
6. **P5 — Ecosystem:** doc hosting, audit DB, editor extensions polish, Roblox/LÖVE/Neovim embedding modes.

Each phase independently useful. No phase depends on registry adoption — luarocks bridge de-risks the cold-start.

---

## 19. Open questions

- Luau type system vs LuaLS annotations: divergent semantics (Luau is sound-ish/strict; LuaLS is permissive). Unified IR must pick a bias per strictness level — proposal: strict mode = Luau semantics, warn mode = LuaLS semantics.
- 5.3+ integer/float observable divergence when targeting 5.1: how loud by default? Proposal: error in `strict`, warn otherwise.
- Registry namespace policy (flat vs scoped `@org/pkg`): proposal scoped — flat namespaces are a crates.io regret.
- C-module story beyond prebuilt artifacts: out of scope until P5+; document loudly.
- Name collision check — verify `luabox` unclaimed on crates.io, GitHub, luarocks, npm before any public artifact.

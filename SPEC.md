# luabox — Unified Lua Toolchain (spec rev 3)

**Name:** `luabox`. CLI binary: `luabox`, alias `lb`.
**One-line:** cargo + rustup + rust-analyzer + bun, for Lua. Not a runtime. Ever.

The `.lb` shape DSL is specified in [SHAPES.md](SHAPES.md). Until the cucumber feature files
exist for a behaviour, this text is the sole source of truth; from then on the feature files
govern.

## 1. Vision

- One static binary. Zero-install-friction (curl | sh, brew, scoop, mise). Written in Rust.
- Bun's ethos: one tool, instant startup, batteries included, obscene speed.
- Rust's ethos: correctness-first, one blessed workflow, first-class diagnostics, stability guarantees.
- Type system: `.lb` shape DSL (Rust struct/trait style, [SHAPES.md](SHAPES.md)) layered over untyped Lua, with full LuaCATS (LuaLS/sumneko) annotation compatibility — both front-ends, one IR.
- Runtimes are pluggable externals (`lua5.1`, `lua5.4`, `luajit`, love2d, OpenResty, Neovim). Luabox compiles/checks/bundles *for* them, never *is* them.

### Non-goals

- No interpreter/VM. No REPL beyond delegating to a configured runtime.
- Full LuaCATS (`---@class` etc.) support is non-negotiable — existing annotated codebases check day one. The `.lb` shape DSL is an additional analyser-only front-end into the same type IR, never a replacement.
- **Luau: explicitly out of scope.** Alternative typed paradigm with its own owner and toolchain (Roblox, luau-lsp). Luabox's typed story is `.lb` shapes over untyped Lua. Scope decision, not an oversight.
- No LuaRocks replacement-by-fiat — interop first, supersede by being better.

## 2. Supported dialects & targets

| Dialect | Parse | Typecheck | Lint | Format | Downgrade from | Downgrade to |
|---|---|---|---|---|---|---|
| Lua 5.1 | ✓ | ✓ | ✓ | ✓ | — (floor) | — |
| Lua 5.2 | ✓ | ✓ | ✓ | ✓ | ✓ | 5.1 |
| Lua 5.3 | ✓ | ✓ | ✓ | ✓ | ✓ | 5.2, 5.1 |
| Lua 5.4 | ✓ | ✓ | ✓ | ✓ | ✓ | 5.3, 5.2, 5.1 |
| LuaJIT (5.1+ext) | ✓ | ✓ | ✓ | ✓ | ✓ | 5.1 |

Luau: out of scope (§1). No parse, no check, no lowering.

### 2.1 `target` — TSC-style emit lowering

`edition` = dialect you write. `target` = dialect you ship. Compiler lowers the delta.

| Feature | Source | Lowering strategy |
|---|---|---|
| `goto`/labels (5.2+) | →5.1 | control-flow restructure (loop/flag rewrite); error if irreducible |
| Integer division `//`, bitops `& \| ~ << >>` (5.3+) | →5.2/5.1 | `math.floor(a/b)`, `bit32`/`bit` shim, polyfill lib injection |
| `<close>`/`<const>` (5.4) | →5.3- | scope-exit rewrite via pcall wrapper / plain local + const-check at compile time |
| `_ENV` (5.2+) | →5.1 | `setfenv`/`getfenv` rewrite |
| Integer/float semantics (5.3+) | →5.1/JIT | diagnostic tiers: warn on observable divergence, error on proven divergence |
| LuaJIT extensions (`bit.*`, ffi absence) | →5.1 | `bit` shim where polyfillable; `ffi` use = hard diagnostic, not lowerable |

- Polyfills: tree-shaken, single injected `__luabox_rt` module, deduplicated across bundle. Zero-cost when unused.
- Non-lowerable constructs (e.g. true `<close>` semantics under error in 5.1 coroutines): hard diagnostic + escape hatch `---@luabox-allow lossy-lowering`.
- Prior art: darklua (dialect transforms, Luau-centric). Luabox covers the 5.x lattice with semantics-preservation proofs per rule.

## 3. Type system

- **Source of truth:** LuaLS annotations (`---@class`, `---@field`, `---@param`, `---@return`, `---@generic`, `---@alias`, `---@overload`, `---@type`, `---@cast`, `---@enum`, `---@meta`). Full dialect compatibility.
- **Definition packages:** `@types/*`-style. `*.d.lua` files (`---@meta` modules) distributed via registry. Runtime API defs shipped for: 5.1–5.4 stdlib, LuaJIT ext, LÖVE, Neovim, OpenResty.
- Strictness ladder (per-package, per-file override): `none` → `warn` → `strict` (untyped = `unknown`, not `any`).
- Inference: bidirectional, flow-sensitive narrowing (`if type(x) == "string"`), literal types, generics with constraints. Match/exceed LuaLS on annotated Lua; `.lb` shapes add the rigor LuaLS lacks.
- **Rich table inference — hard requirement.** Tables never degrade to a bare `table` type. The IR models table *shapes* structurally, and inference maintains them without annotations:
  - Per-field shapes from table constructors and subsequent assignments (`t.x = 1` extends/refines the shape; sealed vs unsealed per strictness level).
  - Array part vs hash part vs mixed distinguished; element types for `t[i]`, and `pairs`/`ipairs`/`next` iteration typed from the shape.
  - Metatable semantics: `setmetatable`/`__index` chains (table and function forms) resolve field lookup, so idiomatic OOP (`Class.__index = Class`, `:` methods, inheritance chains) types correctly without annotations.
  - Literal-keyed indexing narrows (`t["x"]` ≡ `t.x`); dynamic keys fall back to indexer types, not `any`.
  - Inferred shapes unify with declared `---@class`/`---@field` and `.lb` struct types: missing/excess-field diagnostics per strictness level, width subtyping for function arguments.
- `luabox check` = standalone typecheck, CI-grade, machine-readable output (JSON, SARIF, GitHub/GitLab annotations).
- Shapes: see [SHAPES.md](SHAPES.md). Analyser-only, sealed semantics, coexists with LuaCATS in one IR.

## 4. CLI surface

```
luabox init [--lib|--bin] [--edition 5.4|5.1|...]    scaffold in cwd
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

Every command cold-starts < 50 ms; watch mode on check/test/build; `luabox run` resolves package tasks then `$PATH`.

## 5. Project manifest — `luabox.toml`

```toml
[package]
name = "my-lib"
version = "1.2.0"
edition = "5.4"             # dialect you write
license = "MIT"

[build]
target = "5.1"              # dialect you ship (tsc target)
out = "dist"

[types]
strict = true
defs = ["love2d"]           # ambient definition packages
shape-paths = ["shapes/"]   # .lb search dirs (SHAPES.md §6)

[dependencies]
penlight = "1.14"
promise = { git = "https://…", rev = "abc123" }

[dev-dependencies]
busted-compat = "1.0"

[tasks]
start = "luabox run src/main.lua"
ci = ["luabox check", "luabox lint", "luabox test"]

[workspace]
members = ["packages/*"]
```

- **Lockfile:** `luabox.lock` — content-addressed, hashes every artifact, deterministic, text-based.
- Workspaces: shared lockfile, path deps, `--workspace` flags, task fan-out.

## 6. Package manager

- **Resolution:** full semver, PubGrub solver (cargo-quality conflict messages).
- **Registry:** first-party (static-CDN sparse index) **plus** transparent LuaRocks bridge via rockspec translation.
- **Store:** global content-addressed cache (`~/.luabox/store`), hard-link/reflink into projects (bun/pnpm model).
- Dep kinds: registry, git (rev/tag/branch), path, workspace.
- Packages declare `lua-versions = ["5.1", "5.4"]`; resolver refuses incompatible graphs or selects pre-lowered published variants.
- `luabox publish`: builds, checks, tests, verifies annotation coverage on public API, signs (sigstore). Yank, no deletion.
- `luabox audit`: RUSTSEC-analog advisory DB.
- C modules: **declared, not built** — prebuilt artifacts per runtime/platform; luarocks build fallback with loud warning. Luabox is not a C build system.

## 7. Bundler

- Entry module(s) → single target-lowered `.lua` per entry.
- Static `require` graph inlined with lazy init (preserves load order + cycles); dynamic requires diagnosed, allowlist override.
- Tree-shaking: module-level always; statement-level for provably pure module bodies (`---@luabox-pure` opt-in).
- Minify: scope-aware identifier mangling, whitespace, constant folding. Property names never mangled.
- Source maps: `.lua.map` + LSP mapped stack traces; `luabox unmap <traceback>`.
- Profiles: `dev` (readable, asserts kept) / `release` (minified, `---@luabox-assert` stripped).
- Embedding modes: plain chunk, LÖVE fused, Neovim plugin layout.

## 8. Language server — `luabox lsp` (rust-analyzer mirror)

- Salsa incremental DB; memoized queries, fine-grained invalidation.
- Lossless rowan trees (comments preserved) — one parser feeds fmt/lint/fixes/refactors.
- Error-resilient parsing; VFS over disk + editor overlays; background workspace index, mmap persistent cache.
- Features: type-driven completion + auto-require import, postfix snippets; hover with rendered types/docs; goto def/type-def/impl (metatable `__index` resolved); find-refs; workspace rename (string-require-aware); inlay hints; semantic tokens; code actions (annotate from inference, extract/inline, `.`↔`:` convert, generate `---@class` from literal, sort requires, add missing fields); streamed diagnostics with quick-fixes; call hierarchy; signature help; on-type formatting.
- `--stdio` + TCP; first-class VS Code extension; Neovim builtin-LSP config; JetBrains via LSP API.

## 9. Linter (clippy analog)

- Tiers: `correctness` (deny), `suspicious`, `perf`, `style`, `pedantic` (opt-in).
- Representative rules: shadowing, unused, global reads/writes, proven nil-index, concat-in-loop, `pairs`-on-array, truthiness footguns, missing `local`, dialect-portability (`#` on sparse table).
- All rules type-informed on the shared analysis DB. No regex lints. `--fix` via lossless tree.
- Config in `luabox.toml [lint]`; `---@luabox-ignore rule-id reason` (reason mandatory).

## 10. Formatter

- StyLua-compatible default; max ~6 options (width, indent, quotes, call-parens, trailing comma, EOL).
- Range formatting, format-on-save, `--check` for CI, idempotent, version-pinned in lockfile.

## 11. Test runner & bench

- Zero-config discovery (`*_test.lua`, `*.test.lua`, `tests/`); busted-compatible shim + native flat API.
- `luabox test --matrix`: one suite against 5.1/5.4/luajit in parallel — killer feature for cross-version library authors.
- Watch, filtering, snapshots, coverage (source-map-aware instrumentation), JUnit/JSON reporters.
- `luabox bench`: criterion-style statistical benchmarking across runtimes.

## 12. Toolchain manager (rustup analog)

- `luabox toolchain install 5.4.6` / `luajit-2.1` — prebuilt runtimes into `~/.luabox/toolchains`.
- Manifest pins runtime for run/test; `luabox-toolchain.toml` override. An acquirer of runtimes, never a runtime.

## 13. Docs

- `luabox doc`: static site from annotations + shape declarations; search, cross-linked types, doc examples run under `luabox test --doc`. Registry auto-hosts per version (docs.rs analog).

## 14. Diagnostics culture

- Every error coded (`LB0421`), `luabox explain` page, span-rich rendering with labels/suggestions. Machine formats: JSON, SARIF, GitHub Actions, GitLab Code Quality.
- Diagnostic block `LB2xxx` reserved for shapes (SHAPES.md §5).

## 15. Stability & governance

- Toolchain semver; breaking lint/format changes on major only; edition-style opt-ins.
- `min-luabox-version` in manifest, resolver-respected.
- RFC process for language-facing decisions; LuaCATS extensions proposed upstream to LuaLS first.

## 16. Architecture

Cargo workspace, one crate per bounded context. Boundary-only communication (published traits/types), no shared mutable state, no utils dumping ground. Acyclic dep graph enforced (`cargo-deny` + CI check).

```
crates/
  luabox-syntax      lossless parser: Lua dialects (feature-flagged) + .lb shape grammar
  luabox-hir         desugared IR, name resolution
  luabox-types       unified type IR (LuaCATS ⊕ shapes), inference engine
  luabox-db          salsa incremental database (shared: check/lint/lsp/fmt)
  luabox-lower       target lowering + polyfill injection (the tsc bit)
  luabox-bundle      require-graph, tree-shake, minify, sourcemaps
  luabox-resolve     PubGrub solver, registry + luarocks bridge
  luabox-store       CAS cache, fetch, verify
  luabox-lsp         server over luabox-db
  luabox-test        runner, matrix orchestration, coverage
  luabox-cli         thin frontend
```

| Context | Crates | Owns | Boundary contract |
|---|---|---|---|
| Syntax | `luabox-syntax` | grammars (Lua + shape), lossless trees, dialect gating | tree types + parse API |
| Semantics | `luabox-hir`, `luabox-types`, `luabox-db` | name resolution, type IR, inference, incremental queries | salsa DB traits |
| Emit | `luabox-lower`, `luabox-bundle` | lowering, polyfills, require-graph, sourcemaps | checked HIR in, bytes out; shape-blind |
| Distribution | `luabox-resolve`, `luabox-store` | manifests, solver, lockfile, CAS, luarocks bridge | package graph API; never parses syntax |
| Execution | `luabox-test`, toolchain mgr | runtime acquisition, matrix, coverage | runtime handle; only context spawning runtimes |
| Frontend | `luabox-cli`, `luabox-lsp` | UX, protocol, diagnostics rendering | consumes all, owns none |

### 16.1 Implementation — Rust

- Key deps: `rowan`, `salsa`, `pubgrub`, `lsp-server` (rust-analyzer's choice over tower-lsp), `rayon`, `notify`, `clap`, `serde`/`toml_edit` (comment-preserving manifest edits).
- Release: fat LTO, `codegen-units=1`, panic=abort, stripped; musl static Linux, universal macOS, MSVC Windows.
- CI perf gates (merge-blocking): cold start < 50 ms; `check` 100-kLOC warm < 1 s; LSP keystroke-to-diagnostics < 100 ms p95.
- Fuzzing: parser + lowering under `cargo-fuzz`; lowering verified by differential execution against real runtimes in CI.

### 16.2 Testing strategy

- **Unit** — per crate, boundary-internal, no I/O.
- **Acceptance — cucumber (`cucumber-rs`)** — primary layer. Every user-facing behaviour maps to a `.feature` file under `tests/features/<context>/`; Gherkin scenarios ARE the executable spec. Step definitions drive the real CLI binary against temp-dir fixtures — black-box. Feature file first, then implementation.
- **Differential execution** — lowered output vs source on real runtimes, as cucumber `Then` steps + corpus sweep.
- **Fuzz + property** — `proptest`: lockfile determinism, solver idempotence, `fmt(fmt(x)) == fmt(x)`.
- **Perf gates** — §16.1.
- Discipline: declarative scenarios, one behaviour each; `Scenario Outline` for the dialect × target matrix (§2.1 table = examples tables).

## 17. Prior art / positioning

| Tool | Covers | Luabox delta |
|---|---|---|
| LuaRocks | packages | lockfiles, CAS store, solver, speed; bridged not fought |
| lux | packages (Rust rewrite) | whole toolchain, not PM only |
| LuaLS | LSP/types | incremental salsa core, unified with lint/fmt/build |
| selene / luacheck | lint | type-aware rules, autofix |
| StyLua | fmt | same tree as analysis, style-compatible |
| darklua | dialect transforms (Luau-centric) | full 5.x matrix, semantics-preservation guarantees |
| Luau / luau-lsp | typed Lua paradigm | deliberately not competed with — luabox types untyped Lua via `.lb` |
| busted | test | zero-config, runtime matrix, own-emit coverage |
| aftman / rokit / hererocks | toolchains | integrated, manifest-pinned |
| tsc | target lowering | the model, applied to the Lua dialect lattice |
| cargo / rustup / rust-analyzer / clippy | everything | the blueprint |
| bun | DX, speed, one-binary | the temperament |

## 18. Phasing

1. **P0 — Core:** Lua parser (all dialects) + `.lb` grammar module, `luabox.toml`, `init/fmt/check` (LuaCATS subset), CLI skeleton. Formatter ships first.
2. **P1 — Types & LSP:** full inference, salsa DB, shape checking (sealed + coherence), cross-front-end interop, LSP completion/hover/diagnostics/goto.
3. **P2 — Packages:** resolver, store, lockfile, luarocks bridge, `add/install/publish`, registry MVP, `.lb` in package artifacts.
4. **P3 — Build:** lowering matrix, bundler, sourcemaps.
5. **P4 — Runner-adjacent:** test matrix, bench, toolchain manager, coverage, LSP shape polish.
6. **P5 — Ecosystem:** doc hosting, audit DB, editor extensions, LÖVE/Neovim embedding.

## 19. Open questions (escalate, don't guess)

- Strictness semantics for un-shaped LuaCATS code in `strict` mode. Proposal: sealed-leaning warnings; shapes always hard errors.
- Integer/float divergence loudness targeting 5.1. Proposal: error in `strict`, warn otherwise.
- Registry namespaces flat vs scoped `@org/pkg`. Proposal: scoped.
- C-module story beyond prebuilt artifacts: out of scope until P5+.
- Verify `luabox` unclaimed (crates.io, GitHub, luarocks, npm) before any public artifact.
  **Checked 2026-07 (ticket #10):** free on crates.io/npm/PyPI/Homebrew — claim placeholder
  crates before any public artifact. **Taken on LuaRocks** (active terminal library by
  Sylviettee/SovietKitsune) — the bare name is unavailable in exactly the ecosystem the §6
  bridge targets; strengthens the scoped-namespace proposal. `github.com/luabox` handle is
  squatted (dormant since 2018) — pick a fallback org. **`lb` alias:** collides with Debian
  `live-build`'s `/usr/bin/lb` and is squatted on npm/PyPI/crates.io — recommend shipping it
  as a documented shell alias, not an installed binary. Decisions pending: LuaRocks name
  strategy, `lb` shipping mode.

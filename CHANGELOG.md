# Changelog

All notable changes to this project are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versioning follows [SemVer](https://semver.org/), with the 0.x caveats
spelled out in [RELEASING.md](RELEASING.md#semver-policy-for-0x).

## [Unreleased]

Nothing yet — changes land here between releases.

## [0.1.0] - drafted, unreleased

The first public release: the full command surface works end to end against
real Lua sources, driven by an executable spec of cucumber scenarios. Alpha
quality — see the caveats below and [BACKLOG.md](BACKLOG.md) for what's
still open before this tag is actually cut.

### Toolchain

One static binary, one crate per bounded context (SPEC.md §16):

- `init` / `new` — scaffold a project (`--lib`/`--bin`, `--edition`).
- `check` — typecheck: LuaCATS annotations + `.luab` shapes, rich table/OOP
  inference, dialect legality against `--target`, `--watch`,
  `--format json|sarif|github|gitlab`.
- `lint` — 8 type-informed rules, `---@luabox-ignore`, `--fix`.
- `fmt` — canonical formatter for `.lua` + `.luab`, `--check`/`--watch`.
- `build` — lower `edition → target` (goto, bitops, `<close>`/`<const>`,
  `_ENV`, integer/float semantics) with tree-shaken polyfills.
- `bundle` — single-file bundle, `--minify`, `--sourcemap` + `unmap`,
  `--mode love|nvim-plugin`.
- `test` / `bench` — zero-config, busted-compatible runner, `--matrix`
  across runtimes; criterion-lite benchmarks. `--coverage` is accepted but
  not implemented yet and errors out rather than silently no-opping (see
  BACKLOG.md #100).
- `run` — `[tasks]` entries or scripts via the resolved runtime.
- `add` / `remove` / `install` / `update` / `vendor` — PubGrub resolver,
  `luabox.lock`, content-addressed store with hard-link installs;
  path/git/`luarocks/*` dependencies plus writable `file://`/directory
  registries (see "Dependencies & registries" below — hosted registry is
  post-0.1).
- `publish` / `audit` — registry publish with yank; advisory-DB audit.
- `toolchain` — install/pin/list managed Lua runtimes.
- `lsp` — language server: diagnostics, hover, goto, completion, symbols.
- `doc` — static docs generated from annotations + shapes.
- `explain LBnnnn` — rustc-style diagnostic pages.

### Type checking

Types come from full LuaCATS annotation support (`---@class`, `---@field`,
`---@param`, `---@return`, `---@generic`, `---@alias`, `---@enum`,
`---@meta` definition packages) plus `.luab` shape modules — an
analyser-only, TypeScript-adjacent `type` layer over untyped Lua — sharing
one type IR. Rich table inference is unconditional: tables never degrade to
a bare `table` type, shapes are inferred from constructors and subsequent
assignments, and idiomatic `setmetatable`/`__index` OOP resolves without
annotations.

The accepted forward direction (see [DIRECTION.md](DIRECTION.md), decided
2026-07-11) is **LuaCATS-native strict checking**: LuaCATS becomes the one
type format, luabox verifies it more strictly than lua-language-server
(real generics, cross-package type sharing, `---@class` conformance,
undefined-global detection), and the `.luab` shape DSL is parked/dropped
once that front-end reaches feature parity. That migration is in progress,
not finished — see the parity/strictness items tracked in BACKLOG.md
(#84, #90, #103, #107, #108) and the `.luab` removal that follows
(#109). This release still ships both front-ends.

### Dialects & lowering

Parse, typecheck, lint, and format Lua 5.1, 5.2, 5.3, 5.4, and LuaJIT.
`luabox build --target` lowers the dialect you write (`edition`) down to
the one you ship (`target`) — 5.4 → 5.3 → 5.2 → 5.1 — restructuring
`goto`/labels, shimming bitops/integer-division, rewriting `<close>`/
`<const>` scope-exits, and translating `_ENV`, with tree-shaken polyfills
injected only where used. Luau is explicitly out of scope.

### LSP & editor integrations

`luabox lsp` (stdio) provides diagnostics, hover, goto-definition,
completion, and document symbols over a salsa-incremental database shared
with `check`/`lint`/`fmt`. Four editor integrations wrap it:

- VS Code (`editors/vscode/`) — first-class TypeScript extension.
- Neovim (`editors/nvim/`) — native LSP config (0.11+) with lspconfig
  fallback.
- JetBrains (`editors/jetbrains/`) — Gradle/Kotlin plugin on the native LSP
  API (2024.2+), LSP4IJ documented for Community editions.
- Zed (`editors/zed/`) — Rust/WASM extension with its own tree-sitter
  grammar for `.luab`.

### Release machinery

LICENSE (MIT), GitLab CI on the canonical remote (check/test/release
stages), one-line install scripts for Linux/macOS/Windows, and the release
process this changelog is part of (see RELEASING.md).

### Known limitations

- Not yet published to any package registry (crates.io, Homebrew, etc.);
  install a tagged release binary or build from source.
- No hosted first-party dependency registry; `LUABOX_REGISTRY` must point
  at a writable directory or `file://` root.
- `luabox test --coverage` is not implemented.
- The LuaCATS-strictness launch gate (see above) is still in progress.

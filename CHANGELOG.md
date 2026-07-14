# Changelog

All notable changes to this project are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versioning follows [SemVer](https://semver.org/), with the 0.x caveats
spelled out in [RELEASING.md](RELEASING.md#semver-policy-for-0x).

## [Unreleased]

Nothing yet — changes land here between releases.

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
quality — see the caveats below and [BACKLOG.md](BACKLOG.md) for what remains
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

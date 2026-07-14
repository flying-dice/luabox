# luabox

**A cargo-style toolchain for Lua.** One static binary that gives Lua the
workflow Rust developers expect — `check`, `lint`, `fmt`, `run`,
`build`, `doc`, `publish` — with a type checker that speaks stock
[LuaCATS](https://luals.github.io/wiki/annotations/) annotations (the
lua-language-server dialect) but *verifies* what luals only trusts:
`---@class` conformance, `---@generic` generics, and types shared across
modules and packages. Works with Lua 5.1–5.4 and LuaJIT. **Not a runtime.**

## Quickstart

Install the binary (see [Install](#install) below), then:

```sh
luabox new hello
cd hello
```

`luabox new` scaffolds a project — a `luabox.toml` manifest, a `.gitignore`,
and `src/main.lua`:

```
Created binary project `hello` (edition 5.4)
```

Write an annotated function in `src/main.lua`. The `---@param` / `---@return`
comments are ordinary LuaCATS — the same annotations lua-language-server reads:

```lua
---@param name string
---@return string
local function greet(name)
    return "Hello, " .. name .. "!"
end

print(greet("world"))
print(greet(42))
```

Typecheck it. luabox catches the `42` — a plain Lua editor would not:

```
$ luabox check
error[LB0300]: type mismatch: expected `string`, found `42`
  --> src/main.lua:8:13
  |
8 | print(greet(42))
  |             ^^ expected `string`

check: 1 errors, 0 warnings in 1 files
```

Fix line 8 to `print(greet("luabox"))` and it passes:

```
$ luabox check
check: 0 errors, 0 warnings in 1 files
```

Run it, format it, lint it:

```
$ luabox run src/main.lua
Hello, world!
Hello, luabox!

$ luabox fmt
formatted 1 files (0 changed)

$ luabox lint
lint: 0 errors, 0 warnings in 1 files
```

That is the whole loop — one binary for check, run, format, and lint, no build
config. `run` executes on a Lua runtime found on `PATH` or installed via
`luabox toolchain`. See [`examples/`](examples/) for larger, real projects —
a LÖVE game, a multi-package workspace, and a 5.4-to-5.1 cross-version lowering
demo.

## Install

Prebuilt binaries and the VS Code `.vsix` are attached to every tagged
[GitHub release](https://github.com/flying-dice/luabox/releases) (`v*`, built
by [`.github/workflows/release.yml`](.github/workflows/release.yml) — see
[RELEASING.md](RELEASING.md)). The one-line installers fetch the latest one:

```sh
# Linux / macOS
curl -fsSL https://raw.githubusercontent.com/flying-dice/luabox/main/scripts/install.sh | bash
```

```powershell
# Windows
irm https://raw.githubusercontent.com/flying-dice/luabox/main/scripts/install.ps1 | iex
```

Each release ships prebuilt binaries for **Linux x86_64**, **macOS Apple
Silicon**, and **Windows x86_64**, plus the VS Code extension `.vsix`, with
`SHA256SUMS` alongside them.

To build from source instead — or to track the tip of `main`:

```sh
cargo install --git https://github.com/flying-dice/luabox luabox-cli
# or, from a checkout:
cargo build --release            # target/release/luabox
```

## Why luabox

Lua's ecosystem already has rich type annotations — LuaCATS, as read by
lua-language-server. luabox's edge is that it treats those annotations as
**claims to be verified**, not hints to be trusted, on the exact same format:

- **`---@class` conformance.** A `---@class Dog : Animal` must actually provide
  `Animal`'s fields and methods with compatible signatures — missing members
  are errors, not silent gaps.
- **Real generics.** `---@generic T` functions and generic classes are
  monomorphized: `id(42)` returns `integer`, and flowing that into a `string`
  slot is caught.
- **Cross-module and cross-package types.** A `---@class` declared in one file
  is checked at every use site across the workspace; a dependency's types
  (shared via `[types] defs`) are visible and checked in the consumer.
- **Undefined globals and fields.** Typo'd globals and unknown fields on
  declared classes are flagged.

All on stock LuaCATS — there is no second, luabox-specific type file format.
See [DIRECTION.md](DIRECTION.md) for the governing decision record and
[SPEC.md](SPEC.md) for the full design.

## Editor setup

The VS Code extension ([`editors/vscode/`](editors/vscode/)) wraps the
`luabox lsp` stdio language server (diagnostics with quick-fixes, completion
with auto-require imports, hover, goto definition/type-definition/
implementation, find-references, rename, document & workspace symbols,
signature help, call hierarchy, inlay hints, semantic tokens, formatting,
folding and selection ranges; `.lua` files). It is a TypeScript extension:
`npm install && npm run compile`, then `npx @vscode/vsce package` for a
`.vsix`.

It resolves the `luabox` binary from `PATH` (overridable via settings) and
launches it as `luabox lsp`. It is not on the Marketplace yet
([#102](LIMITATIONS.md#editor-extensions-are-not-on-marketplaces-yet-102)) —
install the `.vsix` attached to each GitHub release (or build your own per its
README). Other editors (Neovim,
JetBrains, Zed) can point any LSP client at `luabox lsp`; dedicated
integrations may return later.

## Limitations

luabox 0.1 is alpha software — `test`/`bench` are deprecated (luabox is a
toolchain, not a runtime), there is no hosted package registry, and the VS Code
extension is not yet on the Marketplace/Open VSX (the `.vsix` ships as a release
asset). The full LuaCATS tag vocabulary is enforced. Every remaining gap is
documented honestly in
[**LIMITATIONS.md**](LIMITATIONS.md). Read it before you rely on luabox for
anything load-bearing.

---

## Commands

| | |
|---|---|
| `init` / `new` | scaffold a project (`--lib`, `--edition 5.1..5.4\|luajit`) |
| `check` | typecheck: LuaCATS + rich inference, dialect legality, `--target`, `--watch`, `--format json\|sarif\|github\|gitlab` |
| `fmt` | canonical formatter for `.lua` (`--check`, `--watch`) |
| `lint` | type-informed rules, `---@luabox-ignore`, per-rule `[lint]` levels |
| `build` | lower `edition → target` (goto, bitops, `<close>`, `_ENV`, …) with tree-shaken polyfills |
| `bundle` | single-file bundle, `--minify`, `--sourcemap` + `unmap`, `--mode love\|nvim-plugin` |
| `test` / `bench` | **deprecated** — luabox is a toolchain, not a runtime; code coupled to its deployment environment (LÖVE, Neovim, OpenResty, …) can't be faithfully executed on a bare interpreter. Both warn and are slated for removal; use the environment's own tooling |
| `add` / `remove` / `install` / `update` / `vendor` | PubGrub resolver, `luabox.lock`, CAS store with hard-link installs; path/git/workspace/registry deps |
| `publish` / `audit` | registry publish with yank; advisory-DB audit |
| `run` | `[tasks]` entries or scripts via the resolved runtime |
| `toolchain` | install/pin/list managed Lua runtimes |
| `upgrade` | self-update from GitHub releases (`luabox upgrade` for latest, or a specific `v0.1.1`), checksum-verified |
| `lsp` | language server: diagnostics + quick-fixes, completion (auto-require), hover, goto def/type/impl, references, rename, symbols, signature help, call hierarchy, inlay hints, semantic tokens, formatting |
| `doc` | static docs from annotations |
| `explain LBnnnn` | rustc-style diagnostic pages |

## Dependencies & registries in 0.1

0.1 ships dependency resolution without a hosted registry. Supported
dependency kinds today are **path**, **git** (`rev`/`tag`/`branch`),
**workspace**, and **version-requirement** dependencies resolved against a
registry you point at. There is no first-party hosted default
([#101](LIMITATIONS.md#dependencies-no-hosted-registry-in-01-101)).

A registry is any writable root: a plain directory or a `file://` URL. Point
`luabox add`/`install`/`update` and `luabox publish` at one by setting
`LUABOX_REGISTRY` to that root. `https://` registries are supported for
resolving/installing but are read-only in this MVP — `luabox publish`
requires a directory or `file://` root it can write to.

```sh
export LUABOX_REGISTRY=file:///path/to/a/registry   # or a plain directory
luabox add somelib@1.2                              # resolves against it
luabox publish                                      # publishes to it
```

Without `LUABOX_REGISTRY` set, registry-kind specs (`luabox add pkg@1.0`) and
`luabox publish` fail with setup guidance rather than silently doing nothing.

## Project layout (for contributors)

Cargo workspace, one crate per bounded context (SPEC.md §16):

| Crate | Owns |
|---|---|
| `luabox-syntax` | lossless parser: Lua dialects + LuaCATS annotations |
| `luabox-hir` | desugared IR, name resolution |
| `luabox-types` | LuaCATS type IR, inference |
| `luabox-db` | incremental query database |
| `luabox-lower` | target lowering + polyfills |
| `luabox-bundle` | require-graph, tree-shake, minify, sourcemaps |
| `luabox-resolve` | PubGrub solver, registry + luarocks bridge |
| `luabox-store` | content-addressed cache |
| `luabox-lsp` | language server |
| `luabox-test` | test runner, runtime matrix |
| `luabox-cli` | the `luabox` binary |

```sh
cargo build
cargo test --workspace          # unit + cucumber acceptance tests
cargo fmt --all --check
cargo clippy --workspace --all-targets
```

Acceptance tests are Gherkin feature files under
`crates/luabox-cli/tests/features/` driving the real binary against temp-dir
fixture projects — the executable spec (SPEC.md §16.2).

## Status

**0.1.0** — released 2026-07-14, the full command surface works end to end.
Alpha quality: the executable spec drives the real binary through cucumber
scenarios, perf gates block CI, and lowering is verified by differential
execution against real runtimes in CI. Prebuilt binaries and the VS Code
`.vsix` are attached to each [GitHub release](https://github.com/flying-dice/luabox/releases);
not yet published to a package registry (crates.io, Homebrew, etc.). Luau is
explicitly out of scope. See [LIMITATIONS.md](LIMITATIONS.md) for known gaps.

## License

MIT — see [LICENSE](LICENSE).

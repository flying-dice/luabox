# luabox

**A cargo-style toolchain for Lua.** One static binary that gives Lua the
workflow Rust developers expect — `check`, `lint`, `fmt`, `test`, `run`,
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
config. `run` and `test` execute on a Lua runtime found on `PATH` or installed
via `luabox toolchain`. See [`examples/`](examples/) for larger, real projects —
a LÖVE game, a multi-package workspace, and a 5.4-to-5.1 cross-version lowering
demo.

## Install

Prebuilt binaries are attached to tagged GitLab releases (`v*`, built by
`.gitlab-ci.yml`'s `release` stage — see [RELEASING.md](RELEASING.md)). The
install scripts fetch the latest one:

```sh
# Linux / macOS
curl -fsSL https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/raw/main/scripts/install.sh | bash
```

```powershell
# Windows
irm https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/raw/main/scripts/install.ps1 | iex
```

Prebuilt binaries land with the first tagged release (#95). Until then — or if
you just want the tip of `main` — the scripts point you at building from
source:

```sh
cargo install --git ssh://git@gitlab.beluga-sirius.ts.net/flying-dice/luabox.git luabox-cli
# or, from a checkout:
cargo build --release            # target/release/luabox
```

Both scripts fetch from the GitLab releases API and fail with a clear message
(rather than silently doing nothing) if no release has been tagged yet.

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

Editor integrations wrap the `luabox lsp` stdio language server (diagnostics,
hover, goto-definition, completion, document symbols; `.lua` files). They live
under [`editors/`](editors/):

| Editor | Path | Notes |
|---|---|---|
| VS Code | [`editors/vscode/`](editors/vscode/) | First-class TypeScript extension. `npm install && npm run compile`, then `npx @vscode/vsce package` for a `.vsix`. |
| Neovim | [`editors/nvim/`](editors/nvim/) | `require("luabox").setup()` (Neovim 0.11+ native LSP; lspconfig fallback included). |
| JetBrains | [`editors/jetbrains/`](editors/jetbrains/) | Gradle/Kotlin plugin using the native LSP API (IntelliJ IDEA Ultimate 2024.2+). `./gradlew buildPlugin`, then install-from-disk. LSP4IJ route documented for Community editions. |
| Zed | [`editors/zed/`](editors/zed/) | Rust/WASM extension; registers the server for Lua. `cargo build --target wasm32-wasip2 --release`, then install as a dev extension. |

All of them resolve the `luabox` binary from `PATH` (overridable per editor)
and launch it as `luabox lsp`. None are on their marketplaces yet
([#102](LIMITATIONS.md#editor-extensions-are-not-on-marketplaces-yet-102)) —
install from the built `.vsix` / plugin `.zip` / dev-extension per each
editor's README.

## Limitations

luabox 0.1 is alpha software with real gaps — `---@alias` names are not visible
across files, `test --coverage` is not implemented, there is no hosted package
registry, and a few LuaCATS tags parse but are not yet enforced. Every one is
documented honestly, with its tracking issue, in
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
| `test` / `bench` | zero-config runner (busted-compatible), `--matrix` across runtimes; criterion-lite bench |
| `add` / `remove` / `install` / `update` / `vendor` | PubGrub resolver, `luabox.lock`, CAS store with hard-link installs; path/git/workspace/registry deps |
| `publish` / `audit` | registry publish with yank; advisory-DB audit |
| `run` | `[tasks]` entries or scripts via the resolved runtime |
| `toolchain` | install/pin/list managed Lua runtimes |
| `lsp` | language server: diagnostics, hover, goto, completion, symbols |
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

**0.1.0** — the full command surface works end to end. Alpha quality: the
executable spec drives the real binary through cucumber scenarios, perf gates
block CI, and lowering is verified by differential execution against real
runtimes in CI. Not yet published to any package registry (crates.io, Homebrew,
etc.) — install a prebuilt binary or build from source. Luau is explicitly out
of scope. See [LIMITATIONS.md](LIMITATIONS.md) for known gaps.

## License

MIT — see [LICENSE](LICENSE).

# luabox

**A cargo-style toolchain for Lua.** One static binary that gives Lua the
workflow Rust developers expect — `check`, `lint`, `fmt`, `run`,
`build`, `doc` — with a type checker that speaks stock
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

Prebuilt binaries are attached to every tagged
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
Silicon**, and **Windows x86_64**, with `SHA256SUMS` alongside them. Already
installed? `luabox upgrade` self-updates from the latest release.

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

Editor integrations live in their own repos and release independently:

| Editor | Repo | Install |
|---|---|---|
| VS Code | [flying-dice/luabox-vscode](https://github.com/flying-dice/luabox-vscode) | `.vsix` from that repo's releases → `code --install-extension` |
| JetBrains | [flying-dice/luabox-jetbrains](https://github.com/flying-dice/luabox-jetbrains) | plugin `.zip` from that repo's releases → install from disk |

Both wrap the `luabox lsp` stdio language server (diagnostics with
quick-fixes, completion with auto-require imports, hover, goto
definition/type-definition/implementation, find-references, rename, document
& workspace symbols, signature help, call hierarchy, inlay hints, semantic
tokens, formatting, folding and selection ranges; `.lua` files), resolving
the `luabox` binary from `PATH` (overridable in settings). Neither is on its
marketplace yet
([#102](LIMITATIONS.md#editor-extensions-are-not-on-marketplaces-yet-102)).
Any other editor can point its LSP client at `luabox lsp`.

## Limitations

luabox 0.1 is alpha software — there is no hosted package registry, and the
editor extensions are not yet on their marketplaces (each ships installable
artifacts from its own repo's releases). The full LuaCATS tag vocabulary is
enforced.
Every remaining gap is
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
| `add` / `remove` / `install` / `update` / `vendor` | PubGrub resolver, `luabox.lock`, CAS store with hard-link installs. Registry deps (luarocks.org) live in the rockspec; `add`/`remove` manage `path`/`git`/`workspace` sources in `luabox.toml`. `update <name>` re-pins a git dep to its repo's latest release tag |
| `search` / `outdated` | discover luabox packages on GitHub (topic `luabox` + a root `luabox.toml`) and report git deps behind their latest release; `--format json\|text` |
| `login` / `logout` / `whoami` | sign in to GitHub via the browser (OAuth device flow), storing the token encrypted in the OS keychain; sign out; show the signed-in identity. `login`/`whoami` take `--format json\|text` |
| `run` | `[tasks]` entries or scripts via the resolved runtime |
| `toolchain` | install/pin/list managed Lua runtimes |
| `upgrade` | self-update from GitHub releases (`luabox upgrade` for latest, or a specific `v0.1.1`), checksum-verified |
| `lsp` | language server: diagnostics + quick-fixes, completion (auto-require), hover, goto def/type/impl, references, rename, symbols, signature help, call hierarchy, inlay hints, semantic tokens, formatting |
| `doc` | static docs from annotations |
| `explain LBnnnn` | rustc-style diagnostic pages |

## Dependencies & registries

luabox follows the pnpm/bun model:
[**luarocks.org is the registry**](https://luarocks.org), and the
**rockspec is the package manifest**
([flying-dice/luabox#2](https://github.com/flying-dice/luabox/issues/2)).

- Your project's `*.rockspec` owns its **name**, **version**, and **registry
  dependencies** — its `dependencies` (and `test_dependencies`) are bare rock
  names in LuaRocks constraint syntax (`"lpeg >= 1.0"`), resolved against
  luarocks.org. `luabox init`/`new` scaffold one for you.
- `luabox.toml` is **tool configuration** (edition, build, types, tasks) plus
  the **source** dependencies a rockspec cannot express — `path`, `git`
  (`rev`/`tag`/`branch`), and `workspace` entries. A version-requirement entry
  in `luabox.toml` is an error that points you at the rockspec.

```toml
# hello-0.1.0-1.rockspec — the package manifest
package = "hello"
version = "0.1.0-1"
dependencies = { "lua >= 5.4", "lpeg >= 1.0" }   # resolved from luarocks.org
```

```toml
# luabox.toml — tool config + git/path sources
[package]
edition = "5.4"

[dependencies]
mylib = { path = "../mylib" }                     # a source dependency
```

`luabox install`/`update` resolve the merged graph (rockspec registry deps +
luabox.toml source deps) with the PubGrub solver, write `luabox.lock`, and
hard-link packages into `lua_modules/`. C-module rocks are out of scope and
rejected with a clear error — luabox is not a C build system. There is no
first-party registry, and `LUABOX_REGISTRY` is gone; set
`LUABOX_LUAROCKS_MIRROR` to a local mirror directory for hermetic/offline
resolves.

### Discovering & managing dependencies

With no hosted registry, **GitHub is the discovery surface**. A luabox
*package* is any public GitHub repo that carries the topic `luabox` **and** a
root `luabox.toml`; installing one is a git dependency pinned to its latest
release tag.

```sh
luabox search json          # public repos: topic:luabox + a root luabox.toml
luabox add cool-lib --git https://github.com/owner/cool-lib --tag v1.2.0
luabox outdated             # which git deps are behind their latest release?
luabox update cool-lib      # re-pin cool-lib to its repo's latest release tag
```

`search` and `outdated` take `--format json|text` (`text` is the default;
editors pass `json` for a stable contract). `outdated` always exits 0 — it is a
report. `update <name>` re-pins a **tag**-pinned git dependency to the latest
release tag of its GitHub repo; a `rev`/`branch` pin is left untouched.

GitHub requests honor an authentication token (see **Authentication** below),
sent as `Authorization: Bearer …`, which raises the anonymous 60 req/hr search
limit to 5000/hr. Everything works without a token, just against the lower
anonymous limit.

### Authentication

Sign in to GitHub through the browser — no Personal Access Token to paste:

```sh
luabox login        # opens the browser, prints a device code, stores a token
luabox whoami       # -> your GitHub login (and where the token came from)
luabox logout       # removes the stored token
```

`luabox login` runs the OAuth 2.0 **device flow**: it shows a short `user_code`
and a verification URL (and best-effort opens your browser), you approve there,
and the resulting token is stored **encrypted at rest in your OS keychain**
(macOS Keychain, Windows Credential Manager, Linux Secret Service). No scope is
requested — an unscoped token already lifts the rate limit (least privilege).
`luabox search`/`outdated`/`update` then use it automatically.

`login` and `whoami` accept `--format json` (newline-delimited events for
`login`; one object for `whoami`) — that is what the editor extensions'
"Sign in with GitHub" buttons drive.

Token precedence for every GitHub request is:

```
LUABOX_GITHUB_TOKEN  →  GITHUB_TOKEN  →  keychain (from `luabox login`)  →  anonymous
```

Environment variables win over the keychain, so CI and one-off overrides are
always honored. On a headless box with no keychain (common in CI), `luabox
login` can't store the token and tells you to set `LUABOX_GITHUB_TOKEN`
instead; nothing crashes.

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
| `luabox-resolve` | PubGrub solver, luarocks.org bridge, rockspec + `luabox.toml` manifests |
| `luabox-store` | content-addressed cache |
| `luabox-lsp` | language server |
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
execution against real runtimes in CI. Prebuilt binaries are attached to each
[GitHub release](https://github.com/flying-dice/luabox/releases); editor
extensions release from their own repos;
not yet published to a package registry (crates.io, Homebrew, etc.). Luau is
explicitly out of scope. See [LIMITATIONS.md](LIMITATIONS.md) for known gaps.

## License

MIT — see [LICENSE](LICENSE).

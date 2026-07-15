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
| `build` | one tsc/esbuild-style emit driven by `[build]`: lower `edition → target` (goto, bitops, `<close>`, `_ENV`, …) with tree-shaken polyfills; `bundle = true` inlines the require graph into one file per `entry` (`--minify`, `--sourcemap`); `mode = love\|nvim-plugin` packages a `.love` / Neovim plugin. Flags (`--target`/`--out`/`--outfile`/`--entry`/`--bundle`/`--no-bundle`/`--sourcemap`/`--minify`/`--mode`) override config |
| `unmap` | decode a production traceback back to source lines via the `<bundle>.map` that `build --sourcemap` writes next to the bundle |
| `add` / `remove` / `install` / `update` / `vendor` | PubGrub resolver, `luabox.lock`, CAS store with hard-link installs. `add <rock>` / `remove <rock>` edit the rockspec's `dependencies` (`--dev` → `test_dependencies`) comment-preservingly; `--path`/`--git` manage source deps in `luabox.toml`. `update <name>` re-pins a git dep to its repo's latest release tag |
| `publish` | upload the authored rockspec to luarocks.org (`--dry-run` to preview). Gates on a valid canonically-named rockspec, a green `check`, and pure-Lua-only; needs an API key (`login --luarocks`) |
| `search` / `outdated` | search luarocks.org (the registry) for rocks by name, and report dependencies behind their latest version (registry rocks vs. luarocks.org, git deps vs. their repo's latest GitHub release); `--format json\|text` |
| `login` / `logout` / `whoami` | sign in to GitHub via the browser (OAuth device flow), storing the token encrypted in the OS keychain; sign out; show the signed-in identity. Authenticates git-source operations (`outdated`/`update` release probing) — registry reads are anonymous. `login --luarocks` stores a luarocks.org API key for `publish`. `login`/`whoami` take `--format json\|text` |
| `run` | `[tasks]` entries or scripts via the resolved runtime |
| `toolchain` | install/pin/list managed Lua runtimes |
| `upgrade` | self-update from GitHub releases (`luabox upgrade` for latest, or a specific `v0.1.1`), checksum-verified |
| `lsp` | language server: diagnostics + quick-fixes, completion (auto-require), hover, goto def/type/impl, references, rename, symbols, signature help, call hierarchy, inlay hints, semantic tokens, formatting |
| `doc` | static docs from annotations |
| `explain LBnnnn` | rustc-style diagnostic pages |

### Shipping a bundle: crash-to-source with `unmap`

A minified bundle's tracebacks name the *bundle's* lines, not your source.
Enable source maps at build time and keep the `.map` around to decode
production crashes back to their real file and line:

```toml
[build]
bundle    = true
outfile   = "dist/game.lua"
minify    = true
sourcemap = true          # writes dist/game.lua.map next to the bundle
```

```sh
luabox build              # emits dist/game.lua + dist/game.lua.map
# ship dist/game.lua to players; keep dist/game.lua.map in your build artifacts
```

When a player pastes a traceback like `dist/game.lua:842: attempt to index a
nil value`, pipe it back through `unmap` (map is read from `<bundle>.map` next
to the bundle):

```sh
echo 'dist/game.lua:842: attempt to index a nil value' | luabox unmap dist/game.lua
# → src/player.lua:10: attempt to index a nil value
```

The traceback can come from stdin (above) or as trailing arguments. The map is
recorded only at build time, so `sourcemap = true` is what makes this possible
— there is no way to reconstruct it after the fact.

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
mylib = { path = "../mylib" }                     # a path source dependency
gitlib = { git = "https://github.com/owner/gitlib", tag = "v1.2.0" }
tarball = { url = "https://example.com/pkg.tar.gz", sha256 = "…" }  # bun-style, pinned by digest
```

The `url` source is a bun-style http(s) (or `file://`/local) tarball pinned by
its SHA-256: the digest is captured once at `luabox add --url` time and verified
before extraction on every install after, so a corrupt or tampered download
installs nothing.

`luabox install`/`update` resolve the merged graph (rockspec registry deps +
luabox.toml source deps) with the PubGrub solver, write `luabox.lock`, and
hard-link packages into `lua_modules/`. C-module rocks are out of scope and
rejected with a clear error — luabox is not a C build system. There is no
first-party registry, and `LUABOX_REGISTRY` is gone; set
`LUABOX_LUAROCKS_MIRROR` to a local mirror directory for hermetic/offline
resolves.

**Dialect compatibility.** Every dependency has a **family set** of the Lua
dialects it supports — never a range (a range implies an order LuaJIT breaks: it
is 5.1-plus-extensions, not a point between 5.1 and 5.2). A registry rock's set
is translated from its rockspec `lua` constraint (`lua >= 5.1, < 5.4` →
`{5.1, 5.2, 5.3}`, plus `luajit` whenever 5.1 is admitted); a path/git package's
set is its `luabox.toml` `[package] lua-versions`; an absent set means all
dialects. A dependency is accepted for your **`[build] target`** (default
`edition`) when the target is in its set, **or** its own edition is *lowerable*
to the target — luabox lowers dependency sources alongside yours at build. When
neither holds, resolution fails with the `explain`-able
[`LB1003`](https://github.com/flying-dice/luabox/issues/5) (`luabox explain
LB1003`). Luau is fenced off: it has no lowering path to a PUC target.

### Discovering & managing dependencies

**luarocks.org is the discovery surface** for registry dependencies. `luabox
search` reads luarocks.org's root manifest and matches your query as a
case-insensitive substring of rock names — an anonymous registry read, no
GitHub, no token. Git dependencies remain a public GitHub repo pinned to a
release tag.

```sh
luabox search penlight      # rocks on luarocks.org whose name contains "penlight"
luabox search               # the first 50 rocks by name (the registry is large)
luabox add penlight         # add the latest penlight to the rockspec + install
luabox add penlight@1.14    # a lower bound: writes "penlight >= 1.14"
luabox add busted --dev     # a test-only dep → the rockspec's test_dependencies
luabox remove penlight      # delete the entry from the rockspec + re-sync
luabox add cool-lib --git https://github.com/owner/cool-lib --tag v1.2.0
luabox add tarball --url https://example.com/pkg.tar.gz  # captures & pins its sha256
luabox outdated             # which deps are behind their latest version?
luabox update cool-lib      # re-pin cool-lib to its repo's latest release tag
```

`luabox add <rock>` edits your **rockspec** the way `pnpm add` edits
`package.json`: it resolves the rock on luarocks.org, splices one entry into the
`dependencies` table (or `test_dependencies` with `--dev`), and re-installs.
The edit is surgical — indentation, quote style, comments, and the `lua >= X.Y`
pin are all preserved, and only the touched line changes:

```diff
 dependencies = {
    "lua >= 5.4",
+   "penlight >= 1.14.0",
 }
```

An explicit `add penlight@1.14` writes `>= 1.14` (`@=1.14` for an exact `== 1.14`);
a bare `add penlight` looks up and pins `>= <latest>`. `remove <rock>` deletes
exactly that entry. A `--path`/`--git` add still edits `luabox.toml`, since those
source dependencies a rockspec cannot express.

`search` and `outdated` take `--format json|text` (`text` is the default;
editors pass `json` for a stable contract — `{"results":[…]}` and
`{"dependencies":[…]}` respectively). `search` reports each rock's highest
translated semver and its version count. `outdated` compares each **registry**
rock's locked version against luarocks.org's highest, and each **git** dep's tag
against its GitHub repo's latest release; it always exits 0 (a report, not a
gate). `update <name>` re-pins a **tag**-pinned git dependency to the latest
release tag of its GitHub repo; a `rev`/`branch` pin is left untouched, and
registry deps are re-resolved within their rockspec constraints.

Set `LUABOX_LUAROCKS_MIRROR` to a local mirror directory to run `search` (and
resolution) offline/hermetically. The git-source paths (`outdated`'s release
probing, `update`'s re-pin) honor a GitHub token (see **Authentication**),
sent as `Authorization: Bearer …`, which raises the anonymous 60 req/hr limit to
5000/hr; they work without one, just against the lower anonymous limit.

### Authentication

Sign in to GitHub through the browser — no Personal Access Token to paste:

```sh
luabox login        # opens the browser, prints a device code, stores a token
luabox whoami       # -> your GitHub login (and where the token came from)
luabox logout       # removes the stored GitHub token (and any luarocks.org API key)
```

`luabox login` runs the OAuth 2.0 **device flow**: it shows a short `user_code`
and a verification URL (and best-effort opens your browser), you approve there,
and the resulting token is stored **encrypted at rest in your OS keychain**
(macOS Keychain, Windows Credential Manager, Linux Secret Service). No scope is
requested — an unscoped token already lifts the rate limit (least privilege).
luabox's git-source operations — `luabox outdated`'s release probing and `luabox
update`'s re-pin — then use it automatically. (`luabox search` is an anonymous
luarocks.org read and never consults it.)

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

## Publishing

luabox follows the pnpm/bun model: **[luarocks.org](https://luarocks.org) is
the registry**, and your **rockspec is the package manifest**. `luabox publish`
is a thin proxy that uploads the rockspec you authored (`luabox init`/`new`
scaffold one; `luabox add`/`remove` edit it) to luarocks.org *verbatim* — it
compiles and generates nothing.

```sh
luabox login --luarocks     # once: paste your luarocks.org API key (stored in the keychain)
luabox publish --dry-run    # preview: prints the rockspec + upload target, no network
luabox publish              # upload the authored rockspec to luarocks.org
```

Get an API key from <https://luarocks.org/settings/api-keys>. `luabox login
--luarocks` reads it from stdin and stores it **encrypted at rest in your OS
keychain**; `LUABOX_LUAROCKS_API_KEY` overrides it (CI/one-off). The key is
**never** logged — it is redacted from every echoed command and error.

Before uploading, `publish` gates entirely offline: your project must have a
single root `*.rockspec` that parses and carries `package`, `version`, and a
`source.url`; its filename must be the canonical `<package>-<version>.rockspec`;
`luabox check` must be green; and the rock must be **pure-Lua** (`build.type =
builtin`, no C sources — luabox is a toolchain, not a C build system). A
duplicate version or a server-side validation error surfaces luarocks.org's own
message. Point `publish` at a different server with `LUABOX_LUAROCKS_URL`.

Consumers install your published rock with plain `luarocks install <name>` — no
luabox required on their side.

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

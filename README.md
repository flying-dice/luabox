# luabox

**A cargo-style toolchain for Lua.** One static binary that gives Lua the
workflow Rust developers expect — `check`, `lint`, `fmt`, `build`, `doc`,
`lsp` — with a type checker that speaks stock
[LuaCATS](https://luals.github.io/wiki/annotations/) annotations (the
lua-language-server dialect) but *verifies* what luals only trusts:
`---@class` conformance, `---@generic` generics, and types shared across
modules and packages. Works with Lua 5.1–5.4 and LuaJIT. It is a purely
static toolchain: it **never spawns an interpreter**, and it **consumes** a
rock tree rather than producing one — you materialize `lua_modules/` with
luarocks and luabox reads it. **Not a runtime, not a package manager.**

## Quickstart

Install the binary (see [Install](#install) below), then:

```sh
luabox new hello
cd hello
```

`luabox new` scaffolds a project — a `luabox.toml` manifest, a
`hello-0.1.0-1.rockspec`, a `.gitignore`, and `src/main.lua`:

```
Created binary project `hello` (edition 5.4)
```

The two manifests split by ownership, and neither is generated from the
other. `luabox.toml` is luabox's own configuration — `edition`, `[build]`,
`[types]`, `[lint]`. The rockspec is the *ecosystem's* manifest, the one
luarocks reads: your package's name, version, and dependencies live there,
so anyone can `luarocks install` your project without luabox. luabox scaffolds
it and then leaves it alone — it neither edits nor resolves it.

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

Format it, lint it:

```
$ luabox fmt
formatted 1 files (0 changed)

$ luabox lint
lint: 0 errors, 0 warnings in 1 files
```

That is the whole loop — one binary for check, format, and lint, no build
config, and no interpreter anywhere: luabox reads your sources, it never
executes them. Run the program with whatever Lua you already have. See
[`examples/`](examples/) for larger, real projects —
a LÖVE game, a multi-package workspace, and a 5.4-to-5.1 cross-version lowering
demo.

## Install

Prebuilt binaries are attached to every tagged
[GitHub release](https://github.com/flying-dice/luabox/releases) (`v*`, built
by [`.github/workflows/release.yml`](.github/workflows/release.yml) — see
[RELEASING.md](docs/02-guides/01-releasing.md)). The one-line installers fetch the latest one:

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
[SPEC.md](docs/03-reference/01-spec.md) for the full design.

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
([#102](docs/03-reference/02-limitations.md#editor-extensions-are-not-on-marketplaces-yet-102)).
Any other editor can point its LSP client at `luabox lsp`.

## Limitations

luabox is alpha software — dependency management and running code are
deliberately out of scope (see
[DIRECTION.md](DIRECTION.md#v1-scope-cut-accepted-2026-07-26)), and the
editor extensions are not yet on their marketplaces (each ships installable
artifacts from its own repo's releases). The full LuaCATS tag vocabulary is
enforced.
Every remaining gap is
documented honestly in
[**LIMITATIONS.md**](docs/03-reference/02-limitations.md). Read it before you rely on luabox for
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
| `upgrade` | self-update from GitHub releases (`luabox upgrade` for latest, or a specific `v0.1.1`), checksum-verified |
| `lsp` | language server: diagnostics + quick-fixes, completion (auto-require), hover, goto def/type/impl, references, rename, symbols, signature help, call hierarchy, inlay hints, semantic tokens, formatting |
| `doc` | static docs from annotations |
| `explain LBnnnn` | rustc-style diagnostic pages |
| `schema` | print the JSON Schema (draft 2020-12) for `luabox.toml` — point an editor, a validator or an LLM at the whole manifest contract |

> **`build --mode love` needs an external zip tool on `PATH`.** A `.love`
> file *is* a zip archive and luabox carries no zip implementation, so
> packaging is the one build step that shells out: `zip`, else
> `python3 -m zipfile`, else `python -m zipfile` on Linux/macOS; the
> System32 `bsdtar`, else PowerShell's `Compress-Archive`, on Windows. With
> none of them available the build fails loudly, naming every tool it tried
> — it never leaves a partial `.love` behind. It archives the output luabox
> just emitted; no Lua is executed. No other build step spawns anything,
> `mode = "nvim-plugin"` included — across the whole CLI the only commands
> that start a child process are this packaging step, `upgrade`, and
> `doc --open` (see [DIRECTION.md](DIRECTION.md)).

### Coding assistance for `luabox.toml`: `luabox schema`

`luabox.toml` is described in full by a JSON Schema (draft 2020-12) that the
binary carries — every table, every key, every default, every closed
vocabulary (`edition`, `build.target`, `build.mode`, lint levels) and every
mutually-exclusive dependency form, each with a prose description. Write it
out and point your tooling at it:

```sh
luabox schema > luabox.schema.json
```

Editors and validators get completion and inline errors for the manifest; an
LLM handed that one file can write a correct `luabox.toml` without guessing.
The schema describes the manifest's *data model* — you write TOML, tooling
maps it to JSON with the standard mapping and validates that.

It cannot go stale: the schema is *generated* from the same declarative key
table `luabox` validates manifests against, so its key sets, enums and
required fields are the parser's own rather than a copy of them. What that
table cannot express — above all the mutually exclusive dependency source
forms — is held in place by running every example manifest plus a curated
valid/invalid corpus through both the schema and `luabox` itself, demanding
the same verdict from each.

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

## Using dependencies

luabox **consumes** a rock tree; it does not produce one. Materialize a
project-local `lua_modules/` with luarocks yourself, and luabox reads it:

```sh
luarocks install --tree lua_modules penlight
luabox check                # penlight is requirable, bundlable, and skipped as source
```

Anything under `lua_modules/` is on the module path for `require` resolution
and bundling. Both dependency layouts are searched: the luarocks tree
(`lua_modules/share/lua/<X.Y>/pl/tablex.lua`, where `<X.Y>` is your `[build]
target` — `5.1` for `luajit`) and the flat `lua_modules/<name>/` layout a
hand-vendored or sibling package has. A compiled C module
(`lua_modules/lib/lua/<X.Y>/*.so`) cannot be inlined into a text bundle, so
`require` calls naming one are left as runtime `require`s — ship the `.so`
alongside your bundle. `lua_modules/` is never walked as project source:
`check`, `lint`, `fmt` and `build` skip it whatever it contains.

**Getting a dependency's *types* needs more than installing it.** All three
must hold, or the rock is `unknown` to the typechecker:

1. a `[dependencies]` (or `[dev-dependencies]`) entry naming the package —
   the table drives nothing else, but it is the list that gets searched;
2. a `luabox.toml` for that package with a `[types] defs` key, at
   `lua_modules/<name>/luabox.toml` (or, for a `path` dependency, at the
   path you gave) — i.e. the **flat** layout, since a luarocks tree has no
   per-package `luabox.toml`;
3. the `defs/` directory it names, holding the `*.d.lua` files.

So a plain `luarocks install --tree lua_modules penlight` gives you require
resolution and bundling, but **not** penlight's types. To type it today,
write the LuaCATS definitions into your own project's `defs/` and list them
in your `[types] defs` — the same route as any third-party library.

Your `*.rockspec` and luarocks own dependency management — there is no
solver, no lockfile, and no registry client in luabox
([DIRECTION.md](DIRECTION.md#v1-scope-cut-accepted-2026-07-26)).

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
| `luabox-manifest` | `luabox.toml` manifest model + project layout: discovery, source walk, `[types] defs` |
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

**0.2.0** — unreleased; the v1 scope cut (see
[DIRECTION.md](DIRECTION.md#v1-scope-cut-accepted-2026-07-26)). Last
released: 0.1.4 (2026-07-14). The kept command surface works end to end.
Alpha quality: the executable spec drives the real binary through cucumber
scenarios, perf gates block CI, and lowering is verified by differential
execution against real runtimes in CI. Prebuilt binaries are attached to each
[GitHub release](https://github.com/flying-dice/luabox/releases); editor
extensions release from their own repos;
not yet published to a package registry (crates.io, Homebrew, etc.). Luau is
explicitly out of scope. See [LIMITATIONS.md](docs/03-reference/02-limitations.md) for known gaps.

## License

MIT — see [LICENSE](LICENSE).

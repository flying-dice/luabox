# luabox

**cargo + rustup + rust-analyzer + bun, for Lua. Not a runtime. Ever.**

One static binary that is the package manager, typechecker, linter, formatter,
bundler, test runner, LSP server, and toolchain manager for every Lua dialect
(5.1–5.4, LuaJIT). Types come from full LuaCATS annotation support plus
`.luab` shape modules — TypeScript-adjacent `type` declarations checked
structurally over untyped Lua, analyser-only. See [SPEC.md](SPEC.md) and
[SHAPES-V2.md](SHAPES-V2.md) for the full design. Luau is explicitly out of
scope.

## Status

**0.1.0** — the full command surface works end to end. Alpha quality: the
executable spec is 167 cucumber scenarios driving the real binary, perf gates
(cold start < 50 ms, `check` on 100 kLOC < 1 s warm) block CI, and lowering is
verified by differential execution against real runtimes in CI. Not yet
published to any package registry (crates.io, Homebrew, etc.) — install a
prebuilt binary or build from source.

## Install

Prebuilt binaries are attached to tagged GitLab releases
(`v*`, built by `.gitlab-ci.yml`'s `release` stage — see
[RELEASING.md](RELEASING.md)):

```sh
# Linux / macOS
curl -fsSL https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/raw/main/scripts/install.sh | bash
```

```powershell
# Windows
irm https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/raw/main/scripts/install.ps1 | iex
```

Both scripts fetch the latest release from the GitLab releases API and fail
with a clear message (rather than silently doing nothing) if no release has
been tagged yet.

Until a release exists, or if you just want the tip of `main`, build from
source:

```sh
cargo install --git ssh://git@gitlab.beluga-sirius.ts.net/flying-dice/luabox.git luabox-cli
# or, from a checkout:
cargo build --release            # target/release/luabox
```

## Commands

| | |
|---|---|
| `init` / `new` | scaffold a project (`--lib`, `--edition 5.1..5.4\|luajit`) |
| `check` | typecheck: LuaCATS + `.luab` shapes + rich inference, dialect legality, `--target`, `--watch`, `--format json\|sarif\|github\|gitlab` |
| `fmt` | canonical formatter for `.lua` + `.luab` (`--check`, `--watch`) |
| `lint` | 8 type-informed rules, `---@luabox-ignore`, `--fix` |
| `build` | lower `edition → target` (goto, bitops, `<close>`, `_ENV`, …) with tree-shaken polyfills |
| `bundle` | single-file bundle, `--minify`, `--sourcemap` + `unmap`, `--mode love\|nvim-plugin` |
| `test` / `bench` | zero-config runner (busted-compatible), `--matrix` across runtimes; criterion-lite bench |
| `add` / `remove` / `install` / `update` / `vendor` | PubGrub resolver, `luabox.lock`, CAS store with hard-link installs; path/git/registry/`luarocks/*` deps |
| `publish` / `audit` | sparse-index registry publish with yank; advisory-DB audit |
| `run` | `[tasks]` entries or scripts via the resolved runtime |
| `toolchain` | install/pin/list managed Lua runtimes |
| `lsp` | language server: diagnostics, hover, goto, completion, symbols |
| `doc` | static docs from annotations + shapes |
| `explain LBnnnn` | rustc-style diagnostic pages |

## Layout

Cargo workspace, one crate per bounded context (SPEC.md §16):

| Crate | Owns |
|---|---|
| `luabox-syntax` | lossless parser: Lua dialects + `.luab` shape grammar |
| `luabox-hir` | desugared IR, name resolution |
| `luabox-types` | unified type IR (LuaCATS ⊕ shapes), inference |
| `luabox-db` | incremental query database |
| `luabox-lower` | target lowering + polyfills |
| `luabox-bundle` | require-graph, tree-shake, minify, sourcemaps |
| `luabox-resolve` | PubGrub solver, registry + luarocks bridge |
| `luabox-store` | content-addressed cache |
| `luabox-lsp` | language server |
| `luabox-test` | test runner, runtime matrix |
| `luabox-cli` | the `luabox` binary |

## Development

```sh
cargo build
cargo test --workspace          # unit + cucumber acceptance tests
cargo fmt --all --check
cargo clippy --workspace --all-targets
```

Acceptance tests are Gherkin feature files under
`crates/luabox-cli/tests/features/` driving the real binary against temp-dir
fixture projects — the executable spec (SPEC.md §16.2).

## Dependencies & registries in 0.1

0.1 ships dependency resolution without a hosted registry. Supported
dependency kinds today: path, git (`rev`/`tag`/`branch`), and the LuaRocks
bridge (`luarocks/*` specs). Registry-kind dependencies also work, but only
against a registry *you* point at — there is no first-party hosted default
yet (that's post-0.1; SPEC.md §6).

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

Without `LUABOX_REGISTRY` set, registry-kind specs (`luabox add pkg@1.0`)
and `luabox publish` fail with setup guidance rather than silently doing
nothing. A hosted, first-party registry is planned but not part of the 0.1
scope.

## Editor setup

Editor integrations wrap the `luabox lsp` stdio language server (diagnostics,
hover, goto-definition, completion, document symbols; `.lua` and `.luab` files).
They live under [`editors/`](editors/):

| Editor | Path | Notes |
|---|---|---|
| VS Code | [`editors/vscode/`](editors/vscode/) | First-class TypeScript extension; ships a `.luab` shape grammar. `npm install && npm run compile`, then `npx @vscode/vsce package` for a `.vsix`. |
| Neovim | [`editors/nvim/`](editors/nvim/) | `require("luabox").setup()` (Neovim 0.11+ native LSP; lspconfig fallback included). Adds `.luab` filetype detection. |
| JetBrains | [`editors/jetbrains/`](editors/jetbrains/) | Gradle/Kotlin plugin using the native LSP API (IntelliJ IDEA Ultimate 2024.2+). `./gradlew buildPlugin`, then install-from-disk. LSP4IJ route documented for Community editions. |
| Zed | [`editors/zed/`](editors/zed/) | Rust/WASM extension; registers the server for Lua + `.luab` and ships a tree-sitter grammar ([`tree-sitter-luab/`](tree-sitter-luab/)). `cargo build --target wasm32-wasip2 --release`, then install as a dev extension. |

All three resolve the `luabox` binary from `PATH` (overridable per editor) and
launch it as `luabox lsp`. Build the binary first with `cargo build --release`.

## License

MIT — see [LICENSE](LICENSE).

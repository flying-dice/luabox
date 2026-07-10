# luabox

**cargo + rustup + rust-analyzer + bun, for Lua. Not a runtime. Ever.**

One static binary that is the package manager, typechecker, linter, formatter,
bundler, test runner, LSP server, and toolchain manager for every Lua dialect
(5.1–5.4, LuaJIT). Types come from full LuaCATS annotation support plus the
`.lb` shape DSL — Rust struct/trait declarations checked over untyped Lua,
analyser-only. See [SPEC.md](SPEC.md) and [SHAPES.md](SHAPES.md) for the full
design. Luau is explicitly out of scope.

## Status

**0.1.0** — the full command surface works end to end. Alpha quality: the
executable spec is 167 cucumber scenarios driving the real binary, perf gates
(cold start < 50 ms, `check` on 100 kLOC < 1 s warm) block CI, and lowering is
verified by differential execution against real runtimes in CI. Not yet
published to any registry; build from source.

```sh
cargo build --release            # target/release/luabox
```

## Commands

| | |
|---|---|
| `init` / `new` | scaffold a project (`--lib`, `--edition 5.1..5.4\|luajit`) |
| `check` | typecheck: LuaCATS + `.lb` shapes + rich inference, dialect legality, `--target`, `--watch`, `--format json\|sarif\|github\|gitlab` |
| `fmt` | canonical formatter for `.lua` + `.lb` (`--check`, `--watch`) |
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
| `luabox-syntax` | lossless parser: Lua dialects + `.lb` shape grammar |
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

## Editor setup

Editor integrations wrap the `luabox lsp` stdio language server (diagnostics,
hover, goto-definition, completion, document symbols; `.lua` and `.lb` files).
They live under [`editors/`](editors/):

| Editor | Path | Notes |
|---|---|---|
| VS Code | [`editors/vscode/`](editors/vscode/) | First-class TypeScript extension; ships a `.lb` shape grammar. `npm install && npm run compile`, then `npx @vscode/vsce package` for a `.vsix`. |
| Neovim | [`editors/nvim/`](editors/nvim/) | `require("luabox").setup()` (Neovim 0.11+ native LSP; lspconfig fallback included). Adds `.lb` filetype detection. |
| JetBrains | [`editors/jetbrains.md`](editors/jetbrains.md) | Via LSP4IJ (all editions) or the native LSP API (Ultimate/plugin authors). |

All three resolve the `luabox` binary from `PATH` (overridable per editor) and
launch it as `luabox lsp`. Build the binary first with `cargo build --release`.

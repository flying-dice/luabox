# luabox

**cargo + rustup + rust-analyzer + bun, for Lua. Not a runtime. Ever.**

One static binary that is the package manager, typechecker, linter, formatter,
bundler, test runner, LSP server, and toolchain manager for every Lua dialect
(5.1–5.4, LuaJIT, Luau). See [SPEC.md](SPEC.md) for the full design.

## Status

Pre-alpha — P0 (parser, `luabox.toml`, `init`/`fmt`/`check`, CLI skeleton) in
progress. Nothing here is usable yet.

## Layout

Cargo workspace, one crate per bounded context (SPEC.md §16):

| Crate | Owns |
|---|---|
| `luabox-syntax` | lossless parser, all dialects |
| `luabox-hir` | desugared IR, name resolution |
| `luabox-types` | unified type IR (LuaLS ⊕ Luau), inference |
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

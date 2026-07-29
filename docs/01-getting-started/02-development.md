# Development

Everything is gated in CI; this page tells you how to run the same gates
locally. [.github/workflows/ci.yml](../../.github/workflows/ci.yml) is
authoritative for the exact floors and invocations.

## Toolchain

Rust, pinned by `rust-toolchain.toml` - install via rustup and the pin does the
rest. No Lua interpreter is required to build or test luabox itself; the
examples harness optionally uses `lua5.1` and `unzip` and SKIPs loudly without
them ([scripts/examples.sh](../../scripts/examples.sh)).

## The gates

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings   # pedantic set, warnings are errors
cargo test --workspace                                   # unit + cucumber e2e suites
bash scripts/perf-gate.sh                                # perf budgets (SPEC S16.1)
bash scripts/examples.sh                                 # every example, full gate
```

Coverage runs as two isolated suites (unit and black-box e2e) with
`cargo-llvm-cov`; per-crate floors are enforced by
[scripts/per-crate-coverage.py](../../scripts/per-crate-coverage.py). See the
`coverage-unit` and `coverage-e2e` jobs in ci.yml for the exact package lists
and floors.

## Conventions

- Restriction lints deny `unwrap`/`expect`/`panic` in non-test code.
- The manifest contract is single-sourced from
  `crates/luabox-manifest/src/contract.rs`; if you change it, regenerate the
  schema with `LUABOX_BLESS=1 cargo test -p luabox-manifest schema_file_is_current`.
- Diagnostics carry registered `LBnnnn` codes with `luabox explain` pages;
  every code is reachability-tested.

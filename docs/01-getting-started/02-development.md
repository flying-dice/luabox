# Development

Everything is gated in CI; this page tells you how to run the same gates
locally. [.gitlab-ci.yml](../../.gitlab-ci.yml) on `origin` is the pipeline
of record for the exact floors and invocations (decision 14);
[.github/workflows/ci.yml](../../.github/workflows/ci.yml) on the GitHub
mirror keeps the same floors and adds the macOS/Windows matrix the GitLab
runners cannot cover yet.

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

Two comparison gates run against external ground truth and need extra
binaries locally (CI always has them):

```sh
bash scripts/tests/luals-differential.sh   # type-checker parity vs lua-language-server (#57)
bash scripts/tests/lb0510-matrix.sh        # LB0510 lint verdicts vs real lua5.4 runtime
```

The luals gate compares `luabox check` and a **pinned** lua-language-server
(version in `.github/workflows/luals-parity.yml`) over the corpus in
`scripts/tests/luals-differential/`, against the committed expectations in
that directory's `expected.tsv` — intentional divergences are justified rows
there, not a hidden allowlist, and a diverging row with no justification is
refused by the driver. The luabox column asserts the **exact set of `LB`
codes**, not merely "something fired": a row naming `LB0306` fails if the
case starts reporting `LB0300` instead. Both columns are measured over the
same file set — the case plus every module its `.deps` sidecar names — since
luals sees the corpus as one workspace and it is easy to compare two
different things by accident.

Without a local lua-language-server (or `LUALS=/path/to/bin`) its column
SKIPs loudly and the luabox column still runs; `LUALS_REQUIRED=1` turns that
SKIP into a failure, which is what CI sets, because a job that measures
nothing must not report success. `LUABOX=` may be relative or absolute; the
driver resolves it before running each case from a temp project directory.

Mutation testing was removed from this repo (2026-08-09, operator decision):
the audits cost hours per run for defect yield the team judged below the
time spent, and the regression tests those audits produced remain in the
ordinary suites. Do not reintroduce the machinery.

## Writing fixtures that can fail

The wave-21 lesson (PR #55): the original duplicate-class fixture used the
one shape where the defect was invisible, so the test passed while the bug
shipped. Three rules keep that from recurring:

- **Red first.** A fixture that pins a bug must fail against the pre-fix
  binary; commit it red (with the measured count in the message), then the
  fix that turns it green — the branch history is the proof the test can
  fail.
- **One variable per control.** A control scenario must differ from its
  positive case in exactly one respect; a control that duplicates the
  positive shape validates nothing.
- **Probe both directions.** For type assertions, pair the accepting probe
  with a rejecting one (`want_string(x)` clean AND `want_number(x)` firing)
  — leniency bugs pass every accepting probe.

## Conventions

- Restriction lints deny `unwrap`/`expect`/`panic` in non-test code.
- The manifest contract is single-sourced from
  `crates/luabox-manifest/src/contract.rs`; if you change it, regenerate the
  schema with `LUABOX_BLESS=1 cargo test -p luabox-manifest schema_file_is_current`.
- Diagnostics carry registered `LBnnnn` codes with `luabox explain` pages;
  every code is reachability-tested.

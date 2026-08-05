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

Two comparison gates run against external ground truth and need extra
binaries locally (CI always has them):

```sh
bash scripts/tests/luals-differential.sh   # type-checker parity vs lua-language-server (#57)
bash scripts/tests/lb0510-matrix.sh        # LB0510 lint verdicts vs real lua5.4 runtime
```

The luals gate compares `luabox check` and a **pinned** lua-language-server
(version in `.github/workflows/luals-parity.yml`) over the corpus in
`scripts/tests/luals-differential/`, against the committed two-column
expectations in that directory's `expected.tsv` — intentional divergences are
justified rows there, not a hidden allowlist. Without a local
lua-language-server (or `LUALS=/path/to/bin`) its column SKIPs loudly and the
luabox column still runs. `LUABOX=` may be relative or absolute; the driver
resolves it before running each case from a temp project directory.

A third comparison gate runs on a weekly schedule rather than per-PR:
mutation testing over `luabox-types` (`scripts/tests/mutants-gate.sh`, CI
job `mutants`). Survivors diff against the committed allowlist in
`scripts/tests/mutants-allowlist.txt` — a new survivor is a fixture gap and
fails the job; a reviewed survivor is a justified line there. A **timed-out**
mutant is judged the same way: no test killed it, the run just stopped
waiting, so a new one fails the job rather than disappearing from the count.
Its scope is the merge-seam neighbourhood, pinned in both the script default
and the workflow; widening it to `check.rs` waits on #60.

## Writing fixtures that can fail

The wave-21 lesson (PR #55): the original duplicate-class fixture used the
one shape where the defect was invisible, so the test passed while the bug
shipped. Three rules keep that from recurring, and the mutation gate above
audits the result:

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

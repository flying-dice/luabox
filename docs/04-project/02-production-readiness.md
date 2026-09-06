# Production-readiness assessment (2026-07-25)

> **Dated record.** Figures below are as measured at each section's date;
> `.github/workflows/ci.yml` is authoritative for the current coverage
> floors, and the suites have grown since sign-off. As of the post-release
> backlog burn-down (2026-07-26, #19–#32): ~2,045 workspace tests,
> 669 CLI + 180 LSP black-box scenarios, **e2e 85.7% (floor 83)**,
> **unit 96.9% (floor 95, per-crate 92 enforced)**.

A point-in-time quality baseline for the workspace, plus the scope
evaluation it feeds (below). Numbers were measured on `main` at
`8b6efb5` with the pinned 1.92.0 toolchain; the coverage gate added to
CI keeps the headline number honest from here on.

## Quality baseline

| Gate | Result |
|---|---|
| `cargo fmt --all --check` | clean |
| `cargo clippy --workspace --all-targets` (pedantic + unwrap/panic restriction lints) | clean, zero warnings |
| `cargo test --workspace` | 1407 passed, 0 real failures across 44 test binaries |
| Cucumber acceptance | 23 features, 187 scenarios: 184 pass, 2 skip, 1 `@network` scenario (see below) |
| Line coverage (`cargo llvm-cov`) | **88.0%** lines, 87.4% functions, 86.4% regions |

The single failing test was the `@network` acceptance scenario, which
fetched a live GitHub tarball and broke in any environment where
github.com is filtered — a hermeticity bug, not a product bug. It is now
opt-in (`LUABOX_NETWORK_TESTS=1`) and probes both hosts it needs before
running, so every default `cargo test` run is hermetic. This was the
only live-network test in the suite.

## Coverage by crate

| Crate | Lines | Missed | Coverage |
|---|---:|---:|---:|
| luabox-diag | 481 | 21 | 95.6% |
| luabox-syntax (parser/formatter) | 4,733 | 265 | 94.4% |
| luabox-lower | 1,472 | 84 | 94.3% |
| luabox-lint | 1,155 | 78 | 93.2% |
| luabox-hir | 1,016 | 74 | 92.7% |
| luabox-types (checker) | 10,035 | 820 | 91.8% |
| luabox-lsp | 5,587 | 594 | 89.4% |
| luabox-bundle | 656 | 75 | 88.6% |
| luabox-db | 525 | 81 | 84.6% |
| luabox-resolve | 5,684 | 859 | 84.9% |
| luabox-store | 978 | 210 | 78.5% |
| luabox-cli | 6,720 | 1,505 | 77.6% |

The shape matters more than the total: **the core — parser, formatter,
linter, checker, LSP — sits at 92–95%.** The weak tail is concentrated
in dependency management and its network/auth plumbing: `deps_cmd`
(68.6%), `outdated_cmd` (60.1%), `github.rs` (62.3%), `auth_cmd`
(70.1%), `keychain.rs` (40.6%), `luabox-store` (78.5%), and the resolve
providers (71–78%). (`lsp_cmd.rs` at 0% is a 9-line stdio wrapper; the
server behind it is tested in `luabox-lsp`.)

## CI posture

Already present and blocking: fmt/clippy/test on three OSes, perf gates,
an examples gate driving the real binary, differential execution against
real interpreters, weekly parser fuzzing, and a release pipeline that
holds every release as a draft until the full black-box e2e suite has
passed against the *installed* artefact on all three OSes. Added by this
assessment: line-coverage floors in CI
(`cargo-llvm-cov`, an lcov artifact uploaded per run). A single 85% floor
was proposed here; what shipped is stricter and split in two, so neither
style of testing can coast on the other's numbers — **unit** (every
in-process test, cucumber binaries excluded) with a floor of **95**, and
**e2e** (the black-box cucumber suites driving the real binary) with a
floor of **72**. Raise a floor as its number rises; never lower one to
make a PR pass.

---

# Scope evaluation: park dependency management (ACCEPTED 2026-07-26)

Status: **accepted — owner decision, 2026-07-26.** Scope went further
than proposed: `run`/`toolchain` are also stripped, making v1 a pure
static toolchain (luabox never spawns an interpreter). The decided v1
surface:

**Strip:** `add`/`remove`/`install`/`update`/`vendor`,
`search`/`outdated`, `publish`/`auth`/keychain/GitHub device-flow,
solver + providers + lockfile + CAS store, `run`/`toolchain`.

**Keep:** `check`/`lint`/`fmt`/`lsp`/`explain`, `build` + bundler,
`doc`, `self-update`, `new`/`init`, `--watch`, and the `lua_modules/`
read path (cross-package types over a user-materialized tree).

The rationale and execution plan below stand; the decision record
moves to DIRECTION.md when the cut lands.

## The question

Dependency management (registry resolution, install, publishing) is the
gnarliest surface in the product. Should it be rolled back until the
low-hanging fruit — parser, formatter, linter, checker, LSP — is nailed?

## What the map shows

The entanglement is favorable; there is a clean amputation line.

**The core never touches the gnarly parts.** `luabox-lint`, `luabox-lsp`
and the frontend commands consume only `luabox_resolve::manifest` (the
`luabox.toml` model) plus, for cross-package types and `run`'s module
paths, a *materialized* `lua_modules/` tree — they never invoke the
solver, providers, store, or luarocks bridge.

**The gnarly parts are self-contained,** and they are where the risk
concentrates:

- `luabox-resolve`: pubgrub solver, luarocks bridge + rockspec
  editing, git/url/http providers (curl shell-outs), lockfile —
  ~6.6k LOC.
- `luabox-store`: CAS store, locking — ~1.9k LOC.
- `luabox-cli`: `add`/`remove`/`install`/`update`/`vendor`/`search`/
  `outdated`/`publish`, GitHub device-flow auth, keychain — ~3.2k LOC.
- 6 of 23 feature files (`distribution/*`, `frontend/deps.feature`),
  and the suite's only live-network scenario.

That is **~11.7k LOC (~17% of the workspace)** carrying: all of the
external-ecosystem semantics (luarocks constraint grammar, rockspec
round-tripping), all shell-outs to `curl`/`git` for fetching, the
entire credential/keychain security surface, and the lowest coverage
in the workspace. Every recent gnarl — including the one test that can
fail for environmental reasons — lives inside this boundary.

## Options

**A. Amputate (delete, keep the seam).** Delete solver/providers/
luarocks/lockfile/store and the dependency CLI commands; keep
`manifest`, `project`, `dialect`, and the read-only `lua_modules/`
consumption path. The story is one sentence: *luabox consumes a rock
tree; it does not produce one.* Users point luarocks at
`lua_modules/` themselves; `check`/`run`/cross-package types keep
working. All code stays recoverable in git history (the repo has done
exactly this before — the `.luab` subsystem, #109).

**B. Quarantine.** Keep the code compiled and tested but hide the
commands (feature flag / hidden in help) and freeze feature work.
Keeps optionality warm, but retains ~17% of build/test/clippy time,
the auth surface, and the maintenance drag on every refactor that
touches shared types — the opposite of focusing.

**C. Freeze in docs only.** Zero diff; declares priorities but reduces
no burden and the surface keeps rotting in place.

## Recommendation

**Option A.** The delta between A and B is exactly the burden we are
trying to shed, and git history makes A as reversible as B in
practice. The core's numbers (92–95% coverage, clean pedantic clippy,
spec-first acceptance suite) say the focus bet is sound; the
dependency layer's numbers say it is the part that is not ready, and
finishing it now competes directly with nailing the core.

What A explicitly keeps, because the core needs it:
- `luabox.toml` manifest parsing/model (`[package]`, `[lint]`, build
  config) — used by every frontend command.
- The `lua_modules/` read path in `check`/`run` — cross-package types
  and module resolution over a tree the user materializes with
  luarocks directly.
- `toolchain` (interpreter + luarocks provisioning) — it is the
  bring-your-own-luarocks story's other half.
- `publish`-adjacent nothing: auth, keychain, github.rs all go.

## Execution plan (if accepted)

1. Decision record in DIRECTION.md (supersedes the "luarocks.org is the
   registry" pivot's *scope*, not its direction — the registry choice
   stands for when this returns).
2. Delete `luabox-store`, the resolve solver/provider/luarocks/lockfile
   modules, and the dependency CLI commands + their feature files;
   `luabox-resolve` slims to manifest/project/dialect (candidate rename:
   `luabox-manifest`).
3. README/LIMITATIONS rewrite: "Dependencies" section becomes the
   bring-your-own-luarocks recipe (~10 lines).
4. CHANGELOG entry + minor version bump; the removals are breaking.
5. Re-run the full gate suite; coverage floor likely rises to ~90 —
   raise the CI floor accordingly.

---

# 0.2.0 sign-off (2026-07-26)

The accepted scope cut is executed, verified, and enforced. Tracked as
issues #10–#13 (GitLab `origin`; decision 14); latent bugs surfaced by the coverage waves filed
as #14 and #15.

## Final state

| Gate | Result |
|---|---|
| `cargo fmt --all --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` (pedantic + restriction lints) | clean |
| Full test suite | **2,012 tests, 0 failures** |
| E2E (cucumber, black-box, run alone) | 348 CLI scenarios + 82 LSP-over-stdio scenarios, all green; **74.6% line coverage**, CI floor 72 |
| Unit (all in-process tests, run alone) | ~950 new tests this wave; **96.9% line coverage**, every crate ≥ 92%, CI floor 95 |

The two suites are measured and gated **independently** in CI
(`coverage-unit`, `coverage-e2e`): social/black-box scenarios pin the
user-visible contract while mockist/in-process tests pin each crate's
behaviour, doubling up on the same code from both directions.

## Honest residue (why the numbers are not 100)

- **Unit (96.9%)**: catalogued in the test waves — defensive arms behind
  validated invariants, `cfg(windows)`-gated zip paths on Linux CI, the
  live-network upgrade path, real-stdio server bootstrap, and
  assert-message operands inside passing tests. These are genuinely
  unreachable in-process on this platform, not untested behaviour.
- **E2E (74.6%)**: a black-box suite can only enter through the CLI; the
  remaining lines are internals the binary reaches only on inputs the
  frontend rejects earlier, plus the same OS/network-gated paths. Every
  user-facing command, diagnostic family, LSP method, and exit code is
  scenario-covered.

## Versioning note

The owner asked for this to be "version 0.1.0 at the refined scope".
Tags v0.1.0–v0.1.4 are already published with release binaries the
install scripts resolve, so re-issuing 0.1.0 would rewrite shipped
history. 0.2.0 is that release in substance: the first version at the
nailed-down scope, with the confidence bar raised.

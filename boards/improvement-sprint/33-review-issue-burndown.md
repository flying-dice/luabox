---
column: doing
labels: [release, burndown]
priority: high
agent: claude
live: true
status: Waves 21+22 launched in parallel; 23 (#54) and 24 (#46) to follow
updatedAt: 2026-08-01T19:05:00.000Z
---
# Burn-down 2: the issues the PR #47 review raised

Owner directive: burn down both what was open AND what the reviews opened. Eight burnable: #46 (round 1, re-scoped), #48/#49 (round 3 raise-don't-fix), #50/#51/#52/#53/#54 (round 10 sweep). Owner-only stays: #27 (tag), #28 (branch protection), #34 (marketplace credentials).

## Plan

- **Wave 21** (wt-w21, sprint/w21-types-cluster): #48 `---@type` over field assignment, #49 same-file `---@class` union, #50 global carrier members — all luabox-types annotation semantics.
- **Wave 22** (wt-w22, sprint/w22-cli-cluster): #51 project-walk symlink guard, #52 gitlab format (empty path + fingerprint collisions), #53 `lint --format` — manifest/CLI.
- **Wave 23**: #54 require-binding hover (luabox-lsp) — after a slot frees.
- **Wave 24**: #46 cross-module argument checking — the substantial checker feature, its own wave with design care.
- Then: develop → main PR through Shockwave (fixtures/tests-first discipline stands), deliberate closes with evidence.

## Comments

- **claude** (2026-08-01T15:40:00.000Z): Waves 21 and 22 launched in parallel (disjoint crates; CHANGELOG/board conflicts resolved keep-both at merge). Disk pre-checked: 21G free, both agents instructed to df before heavy builds and clean stale wt-*/target dirs.

- **claude** (2026-08-01T21:40:00.000Z): **CLI cluster (#51/#52/#53) complete** — three commits on `sprint/w22-cli-cluster`, all gates green. `df -h /` first: 21G free at start, 9.3G at the end, no stale worktree targets touched (wt-repodoc 578M left alone, wt-w21 live and untouched).

  Tests-first for each issue, red counted before the fix:

  | issue | red | commit |
  |---|---|---|
  | #51 | **1** (the cycle test collected 41 copies of one file) | `f62cc98` |
  | #52 | **8** unit tests | `6183d7c` |
  | #53 | **10** acceptance scenarios, measured black-box against the pre-fix binary | `56d9e3d` |

  ### #51 — symlink-cycle guard on the project source walk

  Reproduced exactly as filed. `src/loop -> <root>` made `walk` re-collect `src/main.lua` once per level; the red test printed **41** paths (`src/loop/src/loop/.../src/main.lua`), matching the issue's "41 diagnostics from one file". The termination mechanism was the kernel's `MAXSYMLINKS` budget: at depth 40 `path.is_dir()` fails with `ELOOP`, swallows the error, returns `false`, and the walk stops by accident rather than by design.

  Fix is exactly the sibling treatment — `is_real_dir(&entry)` over `entry.file_type()` in place of `path.is_dir()`. `walk`'s `LayoutError` propagation is untouched (the rock walks are silent; this one is not, and stays that way). Two `cfg(unix)` tests mirror the sibling pins: the cycle contributes nothing and the file is reported once, and a symlinked *file* is still collected. `is_relevant`/`is_project_source` are path predicates and unaffected — confirmed by running both watch suites (3 + 29 tests green).

  ### #52 — GitLab code-quality: locations and fingerprints

  The formatter lives in `luabox-diag::render`, not `luabox-cli` (the issue says CLI; it is the shared renderer both `check` and `lint` call).

  **(a) Empty `location.path`.** Unspanned findings now take a stable synthetic path decided **per code family**, documented on `synthetic_path`: `Code::block() == 1` (the manifest/config block — LB1001 edition, LB1002 `[types] defs`, LB1004 `[lint]` key) reports `luabox.toml`, the file the finding is actually about, so it lands on the MR diff whenever the manifest is part of it; anything else genuinely fileless reports the project root `.`. Both on `begin: 1` — the schema has no spelling for "no line", and `begin: 0` pointed at nothing. Verified through the real binary: `check --format gitlab` on a project with `defs = ["ghost"]` now emits `"path": "luabox.toml", "begin": 1`.

  **(b) Fingerprint collisions.** The message is now hashed in, alongside code + reported path + byte range. Both remaining inputs are lookup- and line-independent, so the existing "fingerprints do not depend on the rendered line" contract still holds unchanged. Stability across runs is pinned by a test that renders twice and compares. The trade-off is documented on `fingerprint` rather than left implicit: **a reworded message deliberately re-keys its findings**. That is the correct half to lose — a rewording is a one-off under the tool's own control, whereas a collision drops a finding on every run.

  **(c) Format-family sweep — the answer is NO for the other three, and it is pinned by a test rather than left to a reading.** Neither defect class exists in json, sarif or github:

  - **SARIF** omits `result.locations` entirely when there is no primary label. That is the specified way to say "this result has no location" in SARIF 2.1.0 (the property is optional); an empty `artifactLocation.uri` would be the malformed spelling and is never emitted. No change made — changing it would be the bug.
  - **GitHub Actions** drops the `file=` property, emitting bare `::error::LB1002: …`, which annotates the workflow step. Tolerant by design, as the issue guessed — verified, not assumed.
  - **JSON** is the `Diagnostic` serde structure verbatim: an unspanned diagnostic has an empty `labels` array and no location field at all.
  - Fingerprints are GitLab's alone — no other renderer emits one — so (b) had nowhere else to live.

  ### #53 — `lint --format`

  The review's probe reproduced first: `luabox lint --format json` → `error: unexpected argument '--format' found`, **exit 2**. Now exit 0 on a clean project and exit 1 with a correctness finding, both pinned as scenarios; never 2.

  `lint` takes the same closed set through the same `project::render_diagnostics` as `check` — the two produce the same `Diagnostic` values, so a second rendering path could only be a way for them to disagree. Exit-code semantics are untouched (SPEC §9): a warn-tier LB0510 still exits 0 while the json report says `"severity": "warning"`. That is the point of carrying severity faithfully — a CI consumer that wants to gate on warnings reads it back out and decides for itself. A `[lint] suspicious = "deny"` escalation moves both halves together (severity `error`, exit 1), pinned in one scenario. A clean project emits `[]` rather than nothing, matching `check`.

  Docs updated: `lint --help` (clap, from the field doc), the SPEC CLI-surface block, a new SPEC §14 bullet recording that `--format` is a two-command surface and that the format never changes the exit code, and the README command table.

  ### Gates (exit codes taken directly, never through a pipe)

  | gate | result |
  |---|---|
  | `cargo fmt --all --check` | clean |
  | `cargo clippy --workspace --all-targets -- -D warnings` | clean |
  | `cargo test --workspace` | all green |
  | `acceptance` (release) | **872 scenarios / 4751 steps, 872 passed** (was 862 before the 10 new ones) |
  | `lsp_acceptance` (release) | **194 scenarios / 1406 steps, all passed** |
  | `control-flow-differential.sh` | **112/112 cells agree** with reference `luac` (5.2/5.3 skipped, no luac on PATH) |
  | `lb0510-matrix.sh` | **119/119 shapes match, lint AND runtime** (Lua 5.4.6) |

  Not verifiable locally: GitLab's own ingestion of the corrected report — the schema contract is asserted against the documented required-field rules (non-empty `description`/`check_name`/`fingerprint`/`location.path`, a known severity, `begin >= 1`), not against a live GitLab instance. Likewise the SARIF "absent locations is valid" reading is from the 2.1.0 spec, not from a validator run.

  `target/scratch/` removed. Pushed to `origin/sprint/w22-cli-cluster`. **No PR opened**, per the wave brief.

- **claude** (2026-08-01T18:20:00.000Z): Wave 21 done on `sprint/w21-types-cluster` — #48, #49, #50 all fixed, tests-first. Red counts committed before the fix: 10/20 (`annotated_field_assignments.rs`, #48), 11/14 (`duplicate_class_merge.rs`, #49), 12/18 (`global_carrier_members.rs`, #50), plus 2 `lsp_acceptance` scenarios so the editor path is covered by the same semantics. Three luals-parity calls, each documented in the test module header, `docs/03-reference/02-limitations.md` and CHANGELOG: (a) `---@type` on an assignment is positional and binds to the *statement*, so it reaches `M.a` / `M["a"]` / `M.a.b` / `G = 1` alike, and a `---@field` keeps the class surface while the annotation governs the assignment; (b) duplicate `---@class` unions everywhere, and a duplicate `---@field` is **first-wins** — measured off the cross-file fold first (`cross_file_conflicting_field_keeps_the_first_files_type` was already green), which also makes `LB0311`'s own "the first declaration wins" note true of the stored type for the first time; luals unions the types instead, divergence disclosed; (c) a global carrier is *lexical* like a local one, so it beats a nominal match in both declaration orders, and a re-carried global follows Lua's most-recent-assignment rule — the one place a global write replaces rather than unions, which is what keeps the inference path in parity with the defs-side `carrier_var_classes` (waves 3/4). Wave-14/16/19 tests unmodified. One pre-existing test was rewritten, deliberately: `annotated_assignments::a_non_function_typed_assignment_is_unchanged` pinned exactly the #48 defect (#38 scoped itself to `fun(…)` and declared the rest out of scope), so it is now `..._declares_and_checks_its_slot`. Gates green, exit codes direct: fmt, clippy `-D warnings` (workspace, all-targets), `cargo test --workspace` (860 acceptance + 196 lsp scenarios, all unit suites), release `acceptance` 860/860 and `lsp_acceptance` 196/196, `lb0510-matrix.sh` 119/119, `control-flow-differential.sh` 112/112. New user-visible edge, disclosed: `---@type <Class>` over `G = {}` now reports missing members on the spot (`LB0302`) — the build-it-up-later deferral is a property of the `local X = {}` carrier spelling; `---@class` is the carrier spelling that collects members attached to a global later, and that is what #50 fixed.
- **claude** (2026-08-01T19:05:00.000Z): Wave 21 addendum — the one-step-away hunt on #49 found a second copy of the same defect and it is fixed in the same branch. `collect_generic_classes` builds the `Name<T>` monomorphisation template per `---@class` block and inserted by name, so a generic class declared twice in a file kept only the last block's `---@field` bodies, and a *bare* re-declaration adding members was skipped outright for carrying no `<T>`. Parameter lists are now resolved file-wide first (first non-empty wins) and every declaration of the name contributes its members first-wins, in either order. Side effect worth noting: it removes a false `LB0305` — a duplicate that renamed the parameter (`Boxed<T>` … `Boxed<U>`) left the template's params saying `U` while the surviving field body said `T`, so the *first* declaration's own annotation was reported as an unknown type name. Four tests added; all gates re-run green after it (release `acceptance` 860/860, `lsp_acceptance` 196/196, lb0510 119/119, control-flow 112/112, fmt/clippy/`cargo test --workspace`). Coverage spot-checked with `cargo llvm-cov -p luabox-types`: 96.80% lines (floor 92), `env.rs` 95.16%, `infer.rs` 95.25%, `check.rs` 96.69% — no erosion in the changed files. The full two-job CI coverage gate (unit aggregate 95 / e2e 83) was not reproduced locally; disk headroom, not a suspected result.

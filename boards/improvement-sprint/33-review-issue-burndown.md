---
column: doing
labels: [release, burndown]
priority: high
agent: claude
live: true
status: Wave 22 (#51/#52/#53) and wave 23 (#54) done; 21 in flight, 24 (#46) to follow
updatedAt: 2026-08-01T23:55:00.000Z
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

- **claude** (2026-08-01T23:55:00.000Z): **#54 — require-binding hover complete** — three commits on `sprint/w23-hover-require` (off `develop@8a540a3`), all gates green, no PR. `df -h /` first: 13G free at start, 9.9G at the low point; no `wt-*/target` deleted (wt-w21 live and untouched throughout, and `crates/luabox-types` never opened for edit).

  Tests-first, red counted before the fix: **15 red** — 7 in `cargo test -p luabox-lsp --lib`, 8 in the new `crates/luabox-cli/tests/features/lsp/hover-require.feature` (a NEW feature file, not an edit to `hover.feature`/`completion.feature`, so the parallel agent has nothing to conflict with). Commit `39f660b`.

  ### The defect, and why it was two things pretending to be one

  `local m = require("mod")` hovered `unknown`. The reviewer measured hover; the same binding typed correctly in the problems pane. Both were true, because there were **two** `require` resolvers:

  - `diagnostics::diagnostics` built a `module → Ty` map inline (db `require_exports` for project modules per #85, `RockSurfaces::by_module` merged beneath for rocks per #30) and threaded it into `check_file_with_requires`;
  - `hover`/`completion` went through `FileSema::binding_type`, which is the per-file LuaCATS annotation harvest and has never known anything about cross-file modules. No annotation → `None` → the literal string `"unknown"`.

  So the fix was not a lookup in the hover provider. It is `crates/luabox-lsp/src/requires.rs` — `RequireExports` — and the diagnostics pipeline was moved onto it *first*, as a pure extraction with no behaviour change, so that "one source of truth" is a fact about the code rather than a claim in a commit message. Commit `43774ee` then routes hover and completion through the same object.

  ### Audit of the adjacent surfaces, as asked

  | surface | before | after |
  |---|---|---|
  | hover on a project-module require | `unknown` | the module's export type |
  | hover on a rock require (harvested tree) | `unknown` | the harvested export type |
  | hover on a **field** of a require binding (`m.helper`) | **no hover at all** (declined) | `(field) mod.helper: fun(…)`, qualified by the *module* |
  | **completion** on a require binding (`m.`) | **nothing offered** — same defect, previously unmeasured | the module's exported members; `m:` only the function-typed ones |
  | goto-definition on the binding | already correct (its `local` declaration) | unchanged, now **pinned** |
  | goto-definition on the require *string* | already correct (the module file) | unchanged, now **pinned** |

  So the reviewer's "completion was not reliably measured and I am not claiming anything" resolves to: it had the same defect, it is fixed through the same shared source, and it is pinned both as unit tests and as acceptance scenarios. Goto-definition was already right on both halves and is now pinned so it stays right.

  ### Decisions worth recording

  - **Types render as the checker holds them, literals included.** A module field inferred as `1` shows `1`, not `integer`. Widening for display (what inlay hints do, deliberately) would be prettier and would be *the editor disagreeing with CI* — the exact class of defect #54 is. Cost a red: the first fixture asserted `other.version: string` and the truth was `other.version: "1.0"`; the fixture was wrong, not the code.
  - **`require_module_of` matches by identity, not containment.** The `local`'s initialiser in the matching position must *be* the require call. That is what makes `require("a") or require("b")` and `require("mod").sub` decline rather than guess, and it makes shadowing free (the lookup is keyed on the binding's own declaration range, so two `local m`s get their own modules).
  - **`or`-chained require: decided as `unknown`, documented.** Which branch runs is a runtime fact; naming either module's type would be a coin flip presented as a type. Pinned as *correct*, with the reason in the module docs, the feature file and the CHANGELOG — not left as an unexplained gap.
  - **Dynamic `require(name)`: `unknown` BY DESIGN**, pinned. The type pass does not resolve it either, so the two still agree.
  - **Explicit beats implicit.** An `---@type` on the binding still wins over the module export, matching the rest of the toolchain.

  ### README

  Re-read against reality rather than deleted. "(the editor sees the same surfaces, so hover and completion agree with CI)" was true of diagnostics and false of hover — half of what #54 said. It now states the specific thing that holds (binding, member, completion; project modules and rocks) and names its two edges: the by-design `unknown`s, and **one place the editor is genuinely narrower than CI** — a module whose export is a `---@class` *instance* is `Ty::Named`, so the binding hovers as the class name but the class's `---@field`s are declared in the module's own file, out of reach of the per-file view these surfaces are built on. Its members get no hover or completion while `luabox check` still checks them. Measured, not assumed (a throwaway probe confirmed `Named("Point")`), and pinned by `a_class_instance_module_hovers_as_the_class_but_has_no_member_hover` so the README and the test cannot drift apart. Closing that would mean giving the editor surfaces the ambient environment — a bigger change than #54, and not this wave's.

  ### Gates (exit codes taken directly, never through a pipe)

  | gate | result |
  |---|---|
  | `cargo fmt --all --check` | clean |
  | `cargo clippy --workspace --all-targets -- -D warnings` | clean |
  | `cargo test --workspace` | **2497 passed**, 0 failed |
  | `acceptance` (release) | **872 scenarios / 4751 steps, all passed** |
  | `lsp_acceptance` (release) | **208 scenarios / 1516 steps, all passed** (was 194/1406 — +14 from `hover-require.feature`) |
  | `cargo build --release` | ok |
  | `control-flow-differential.sh` | **112/112 cells agree** with reference `luac` (5.2/5.3 skipped, no luac on PATH) |
  | `lb0510-matrix.sh` | **119/119 shapes match, lint AND runtime** (Lua 5.4.6) |

  Not verifiable locally: nothing behavioural — the whole issue is reproducible in-tree and every claim above is pinned by a test. The only unverified reading is that a real editor renders the wider table-shaped hover acceptably; the LSP contract (a markdown code block) is unchanged, only its contents are longer.

  `target/scratch/` removed. Pushed to `origin/sprint/w23-hover-require`. **No PR opened**, per the wave brief.

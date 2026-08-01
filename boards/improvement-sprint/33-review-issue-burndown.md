---
column: in-progress
labels: [review, cli, types]
priority: high
agent: opus-w22
live: true
status: In progress — #51/#52/#53 done (CLI cluster); #48/#49/#50/#54 with the types agent
updatedAt: 2026-08-01T21:40:00.000Z
---
# Wave 22: review issue burndown (#48–#54)

The independent review of PR #47 filed seven issues after the round-10 sweep. This card burns them down across two parallel agents on one branch, `sprint/w22-cli-cluster` off develop@be354b6.

- **CLI/manifest cluster** — #51 (source walk symlink cycle), #52 (GitLab code-quality output), #53 (`lint --format`).
- **Types/LSP cluster** — #48 (`---@type` over a table-field assignment), #49 (same-file duplicate `---@class` last-wins), #50 (a `---@class` on a global never gets its members), #54 (hover on a `require` binding shows `unknown`).

The two agents do not share files: the CLI cluster touches `luabox-diag`, `luabox-cli` and `luabox-manifest::layout` only; the types cluster owns `luabox-types` and `luabox-lsp`. CHANGELOG and this card are resolved keep-both at merge.

## Scope

- #51: `layout::walk` uses bare `path.is_dir()`, which follows symlinks — the two sibling walks were hardened during the review and this one was missed
- #52 (a): unspanned `LB1xxx` findings emit `location.path: ""`, which GitLab's parser rejects
- #52 (b): the fingerprint excludes the message, so two distinct findings at one byte range collide and GitLab drops one
- #52 (c): sweep json/sarif/github for both defect classes
- #53: `lint --format json` exits 2; `check` has the full surface

## Comments

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

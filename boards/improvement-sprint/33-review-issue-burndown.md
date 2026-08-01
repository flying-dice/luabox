---
column: doing
labels: [release, burndown]
priority: high
agent: claude
live: true
status: All four waves landed on develop; opening develop -> main PR
updatedAt: 2026-08-01T22:30:00.000Z
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

- **claude** (2026-08-01T19:55:00.000Z): Wave 21 merged to develop (`6696328`; CHANGELOG and this card resolved keep-both). Spot-checks green on the merged head, exit codes direct: fmt 0, clippy 0, `cargo test --workspace` 0, release `acceptance` **872/872** (4751 steps), release `lsp_acceptance` **196/196** (1417 steps — wave 21's two new scenarios on top of wave 22's baseline), `control-flow-differential.sh` **112/112**, `lb0510-matrix.sh` **119/119** lint AND runtime (Lua 5.4.6). Pushed. wt-w21 removed (18G free after); wave 24 (#46 cross-module argument checking) launched in the freed slot on `sprint/w24-cross-module-args` off `6696328` — luabox-types only, disjoint from wave 23's luabox-lsp surface, shared `diagnostics.feature` appends at end-of-file only. Six of eight burnable issues now fixed on develop (#48 #49 #50 #51 #52 #53); #54 and #46 in flight.

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

- **claude** (2026-08-01T20:35:00.000Z): Wave 23 merged to develop (`4e3e88b`; this card resolved keep-both, CHANGELOG merged clean). Spot-checks green on the merged head, exit codes direct: fmt 0, clippy 0, `cargo test --workspace` 0, release `acceptance` **872/872** (4751 steps), release `lsp_acceptance` **210/210** (1527 steps — wave 23's 14 scenarios and wave 21's 2 now both on the merged head), `control-flow-differential.sh` **112/112**, `lb0510-matrix.sh` **119/119** lint AND runtime (Lua 5.4.6). Pushed. Disk incident averted at merge time: 3.7G free when the wave-23 notification arrived (wave 24 building concurrently); freed `wt-w23/target` (7.4G) before running gates, 12G free after; wt-w23 worktree now fully removed. Seven of eight burnable issues fixed on develop (#48 #49 #50 #51 #52 #53 #54); #46 (wave 24) is the last one in flight.

- **claude** (2026-08-01T22:30:00.000Z): **Wave 24 — #46 cross-module argument checking** complete on `sprint/w24-cross-module-args`, tests-first, all gates green. `df -h /` before every heavy build: 21G at start, 9.2G at the end; no `wt-*` sibling touched (wt-w23 left alone).

  **Measured before designing.** The issue's five-row table reproduced exactly on `develop` first (rows 3 and 4 `none`; rows 1/2/5 already `LB0300`), and the same 23-shape harness re-run after the fix gives the before/after below. Eleven shapes flipped from unchecked to checked; every negative stayed silent, which was the point of measuring them up front.

  | shape | before | after |
  |---|---|---|
  | row 1 same-file direct | LB0300 | LB0300 |
  | row 2 same-file table field | LB0300 | LB0300 |
  | **row 3 cross-module direct** | none | **LB0300** |
  | **row 4 cross-module table field** | none | **LB0300** |
  | row 5 cross-module + call-site `---@type` | LB0300 | LB0300 |
  | correct args cross-module | none | none |
  | unannotated exported fn | none | none |
  | optional param omitted | none | none |
  | varargs, extra args | none | none |
  | dynamic require path | none | none |
  | wrong arity, too few | none | **LB0301** |
  | wrong arity, too many | none | **LB0301** |
  | aliased require binding | none | **LB0300** |
  | function stored then called | none | **LB0300** |
  | nested table export | none | **LB0300** |
  | colon-method on exported class | LB0300 | LB0300 |
  | dot-call of a method | none | **LB0300** |
  | rock function (lua_modules harvest) | none | **LB0300** |
  | rock function, correct args | none | none |
  | module via `init.lua` | none | **LB0300** |
  | `require("m").f(...)` inline | none | **LB0300** |
  | cross-module call as a nested argument | none | **LB0300** |
  | re-export of a *required* function | none | none (disclosed) |

  **A fallback, not a second mechanism.** `Checker::callee_sig` resolved a callee only through registries keyed by *name* — a local binding, a dotted name in the ambient/defs map — and a required module's members are in neither, because nothing in the consumer file declares them. Inference had already resolved the callee to its `fun(...)`; nobody asked it. `callee_sig` now falls back to that resolved type and hands it to the existing `check_arg_slots`, so arity, `---@param` types, overload selection, generics, `---@vararg` and optionals are the same code on both sides of the boundary.

  **Conservatism needed provenance.** An unannotated exported function must not be argument-checked, because an unannotated *same-file* function is not — measured on `develop` before any design. Same-file is safe because such a function is registered nowhere; `reconcile_params` separately makes *partially* annotated parameters optional `unknown`, so partial annotation cannot manufacture arity errors either. Reification erases the difference between a written `---@param` list and one read off a body (both become a `FunctionTy` with `unknown` params), so `FunctionTy::declared` now records whether a human wrote the signature — set at the annotation-lowering sites, explicitly `false` in `reify_func`'s synthesized branch. The failure direction is "stops checking", never "false positive".

  **luals-parity calls.** (a) Only *written* signatures check calls; luals does not arity-check an unannotated function either, and this is what keeps the negative column above empty. (b) `---@type fun(...)` is authoritative at call sites (SPEC §3; luals reports `redundant-parameter`) — the rule the issue's own row 5 already assumed. (c) A `---@param` block above `return function(...) end` now binds to that function: without it a single-function module could not carry a signature *at all*, in its own file as much as in a consumer, which is what made row 3 unreachable rather than merely unchecked.

  **One pre-existing test rewritten, deliberately.** `annotated_assignments::a_typed_assignment_over_a_non_literal_right_hand_side_is_unchanged` asserted that `---@type fun(a: integer)` over `M.f = other`, then `M.f(1, 2)`, reports nothing — pinning exactly this defect (a callee known only through its type going unchecked). Now `..._is_checked_from_its_type`, asserting `LB0301`. No other wave 14/16/19/21 test changed.

  **Two neighbourhood shapes disclosed rather than fixed**, each with a fixture pinning the behaviour, plus the rewritten #46 bullet in `docs/03-reference/02-limitations.md` (bound removed, what now works stated, remaining edges enumerated): a member declared only as a `---@field` on an exported class (the *member* does not reach the consumer, not its signature), and a function re-exported through a second `require` (a module's own requires are deliberately unresolved — that is what keeps the registry acyclic and cycles tolerable). Exporting a class carrier as `Ty::Named` was tried for the first and **reverted**: it produced false `LB0306`/`LB0300` on valid code in `cross_file_require::require_of_class_module_resolves_inherited_method`. False positives on correct code are not a trade this work may make.

  **Two false trails, killed by measurement rather than assumed.** A `luabox check` pinning a core during a gate run looked like an exponential blowup from resolving the callee type twice per level; a scaling fixture (nested calls through a required module's function, depths 6→30) came back flat at ~32ms on *both* the suspect and the fixed build, so the theory was wrong — the processes were `broken_pipe`'s deliberately large fixtures (1400 files, 2000 findings) in a debug build under `--workspace` parallelism, slow by design. Separately, a workspace run reporting 872 scenarios instead of 894 turned out to be a shell-precedence mistake (`cd X && … &` swallowed the `cd`), which had run cargo against `/home/user/luabox` instead of the worktree; re-run with absolute paths. The single-resolution change was kept on its own merits and its comment corrected to claim only what was measured.

  Gates, exit codes taken directly (never through a pipe):

  | gate | exit | result |
  |---|---|---|
  | `cargo fmt --all --check` | 0 | clean |
  | `cargo clippy --workspace --all-targets -- -D warnings` | 0 | clean |
  | `cargo test --workspace` | 0 | all suites green |
  | release `acceptance` | 0 | **894 scenarios / 4882 steps, all passed** (872 baseline + 22 new) |
  | release `lsp_acceptance` | 0 | **196 scenarios / 1417 steps, all passed** (baseline held; no lsp files touched) |
  | `lb0510-matrix.sh` | 0 | **119/119 shapes match, lint AND runtime** (Lua 5.4.6) |
  | `control-flow-differential.sh` | 0 | **112/112 cells agree** with reference `luac` (5.2/5.3 skipped, no luac) |

  Red counts committed before the fix: **13 red / 21** in `crates/luabox-types/tests/cross_module_args.rs` and **8 red / 18** new scenarios in `crates/luabox-cli/tests/features/frontend/cross-module-args.feature` (suite 872 → 890 at that commit; 894 once the disclosure fixtures were added), carried by `74c400c`. Fix `e5380a6`, follow-up refactor `330971b`. Not verified locally: nothing — every gate in the brief ran here. **No PR opened**, per the wave brief.

- **claude** (2026-08-01T22:45:00.000Z): Wave 24 merged to develop (`ddd690b`; CHANGELOG and this card resolved keep-both). Full gate suite on the merged head, exit codes direct: fmt 0, clippy 0, `cargo test --workspace` 0, release `acceptance` **894/894** (4882 steps), release `lsp_acceptance` **210/210** (1527 steps), `control-flow-differential.sh` **112/112**, `lb0510-matrix.sh` **119/119** lint AND runtime (Lua 5.4.6). **All eight burnable issues are now fixed on develop** (#46 #48 #49 #50 #51 #52 #53 #54). Opening develop -> main PR 'Review-raised issue burn-down' next; closes stay deliberate post-merge with evidence, per policy.

- **claude** (2026-08-01T23:59:00.000Z): Wave 25 — Shockwave review of PR #55 came back **FAIL** with three findings; all three are fixed on `sprint/w25-shockwave-r1` (9 commits, pushed, **no PR** per brief). Every intermediate commit was verified to build individually, since Shockwave builds them.

  **Finding 1 (blocker, from wave 21's `be12148`) — generic `---@class` type parameters were not scoped to their declaration.** `collect_generic_classes` recorded the first non-empty parameter list per name and handed *that one* to every declaration, so a renamed duplicate's own valid annotation resolved against names never in scope for it. Adopted the reviewer's fix shape verbatim — lower each declaration against its own list, unify **positionally** when merging. Reviewer's repros re-measured: `Boxed<T>`/`Boxed<U>` both with field bodies was `error[LB0305]: unknown type name 'U'`, exit 1 → **exit 0**; bare-first `Boxed<U>` then `Boxed<T>` was `LB0305 'T'`, exit 1 → **exit 0**. The neighborhood sweep found a **third broken variant the review did not name**: the two declarations in *different files* (`Boxed<T>` in `a.lua`, `Boxed<U>` in `b.lua`) leaked `error[LB0300]: expected 'string', found 'U'` at the consumer → now exit 0. Three merge points needed the rule and all three carry it (instantiation templates; the class definition that crosses `require`; the workspace-global class two files build). Two supporting corrections fell out: `lower_class_template` attributed fields to a declaration by class *name*, which cannot tell two same-name declarations in one doc block apart, so each got the other's fields — ownership is positional now, which is what its own doc comment already claimed; and `collect_generic_classes` no longer contributes diagnostics, since `absorb_block` lowers the same field bodies again and is the sole reporter (it was double-reporting a genuinely unknown name). The reviewer's meta-point is answered: every new fixture has field bodies on **both** declarations under different parameter names, plus the bare-first variant and a functional `Boxed<string>` monomorphisation check in both directions. Wave 21's `the_first_declarations_type_parameters_are_not_renamed_by_a_duplicate` passes **unmodified**. Red: **8 red / 29** in `duplicate_class_merge.rs` and **4 red / 5** new scenarios (`5bf8757`), plus **1 red** cross-file scenario (`d50b7a4`); fix `a9bc917`.

  **Finding 2b — trailing nil-admitting parameter is optional for arity.** `f(1)` against `---@param b number|nil` was `LB0301`, exit 1 → **exit 0**. Landed in `FunctionTy::required_params`, the single place all four arity paths go through (direct, method, overload selection, cross-module), so it reached every one at once. Two boundaries drawn deliberately and pinned by passing controls in the same red batch: **only a trailing run** is optional (a caller cannot skip a middle argument in Lua without writing `nil`, and **luals's exact non-trailing behaviour could not be verified — no lua-language-server in this environment**, so the conservative direction was taken and documented); and "admits nil" means the type *says* nil — new `Ty::admits_nil_explicitly`, the narrow half of `admits_nil`, so `any`/`unknown` stay required. Red: **6 red / 29** in `call_sites.rs`, **2 red** scenarios (`5940042`); one fixture corrected in `588ec7e` after measuring that its body `w * (scale or 1)` was red for a *second, pre-existing* reason (arithmetic over the union an `or` produces infers `unknown` — reproduced on the unmodified tree with `---@param s number`, so unrelated to this branch and left alone); fix `9c7f0a0`.

  **Finding 2a — FIXED, not disclosed.** The brief said prefer the fix, measure first, decide on evidence. Prototyped it before writing a line of doc: it is **one arm in `lookup_ty_field`** reusing the dotted-function registry the free-function spelling already goes through — the ambient-environment architecture is untouched, which was the stated bar for disclosing instead. Prototype was green across the entire workspace with no existing fixture rewritten, so the fix shipped. Reviewer's repro (5 × `LB0300` "found `unknown`", exit 1) → **exit 0**; `s:upper()` is `string`, `s:byte()` `integer`, `s:match(p)` `string|nil` (luals: `string?`). Parity came free because it is Lua's own semantics — every string shares one metatable whose `__index` is the `string` table — so `s:nope()` is `LB0306` in both the `:` and `.` spellings (runtime: "attempt to call a nil value (method 'nope')"), and a project writing `function string.trim(s)` gets `s:trim()`. Arguments are checked too, with the receiver bound: the library spells `self` as an ordinary leading parameter, which `check_method_call` (which strips by the *name* `self`) cannot recognise, so it is dropped in `eval_method_call` where the receiver is still known — `s:rep("three")` is `LB0300`, `s:upper(1,2,3)`/`s:sub()` are `LB0301`. Edge left and documented: a `string|nil` receiver stays lenient, the pre-existing rule for union receivers generally. Red: **13 red / 43** in `method_calls.rs`, **4 red** scenarios (`7a9afc4`); fix `ff789a5`.

  **Finding 3 — the doc claim was wrong, and measurement is what showed it.** Probed the LSP directly rather than trusting the prose. For a `---@class Point` **carrier** module (`local P = {}`), the binding hovers `local p: {  }` — a *structural table*, **not** the class name — `p.x` hover is null, completion omits `x`, and `luabox check` is **lenient**: `p.x` crosses as `unknown` and `p.nope` is *accepted*. So the old README claim was wrong twice for that spelling: not the class name, and CI does not "still check" the members. The claim was only ever true of the class **instance** spelling (`---@type Point` on the returned local), where the binding does hover `local p: Point` and CI *does* fully enforce (`p.x` is `number`, `p.nope` is `LB0306`) — that is the genuine editor-narrower-than-CI row. Both rows are now a measured table in `docs/03-reference/02-limitations.md`, `requires.rs`'s doc comment is corrected, and README points at the limitations page. Hover behaviour itself deliberately unchanged. Fixtures pin every row on both sides: **+4** `lsp/hover-require.feature`, **+2** `frontend/require.feature`. These pin measured behaviour rather than driving a fix, so they are green on arrival — stated plainly in `9882372` rather than dressed up as a red-to-green cycle. The remaining findings-2 edges are recorded on the same page, and #49 gains the type-parameter scoping rule.

  **Gates on the branch head, exit codes direct (`$?`), all 0:** `cargo fmt --all --check` 0 · `cargo clippy --workspace --all-targets -- -D warnings` 0 · `cargo test --workspace` 0 · release `acceptance` 0 — **910/910** scenarios (4973 steps; baseline 894, +16 new) · release `lsp_acceptance` 0 — **214/214** (1556 steps; baseline 210, +4 new) · `lb0510-matrix.sh` 0 — **119/119** lint AND runtime (Lua 5.4.6) · `control-flow-differential.sh` 0 — **112/112**. No test from waves 14/16/19/21/23/24 was modified. Not verified locally: luals's non-trailing nil-admitting arity behaviour (no `lua-language-server` binary available) — handled conservatively and documented as such. **No PR opened**, per the wave brief.

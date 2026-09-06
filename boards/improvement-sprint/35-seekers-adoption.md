---
column: doing
labels: [wave, docs]
priority: high
agent: starscream
live: true
status: #70/#81 merged to develop; #69/#78 in review; promotion MR next
progress: 75
updatedAt: 2026-09-06T09:30:00.000Z
---
# Seekers adoption sprint — sign the repo off as run to standard

Goal (owner's words): *"do a full review of this project and its current state and
target improvements and fixes … sign this repo off as adopted to your standards."*
Decision 14 records the takeover terms; `docs/bots/ROADMAP.md` M0 lists the exit.

## Checklist

- [x] Pipeline red on `main` fixed — coverage jobs died on a cold cache (`$CARGO_HOME/bin` missing)
- [x] Baseline measured on `main` 75e8d66: fmt clean, clippy clean, 2613 unit tests, 910 CLI + 214 LSP scenarios, all green
- [x] Divergence found: `develop` carries PR #61 + GitLab CI port, never promoted to `main`
- [ ] `main` merged into `develop` (branch `starscream/integrate-develop`), gates green, pushed
- [ ] `develop → main` promotion MR open, pipeline green, Shockwave review posted
- [ ] `STATUS.md`, roadmap, decision 14, backlog/dev docs corrected
- [x] Soundwave styleguide/architecture audit → issues #75–#81 filed (crate graph clean; 12 findings, 7 actionable)
- [ ] #69 malformed `---@class` header diagnostic — MR !2 open (LB0320/LB0321), Shockwave reviewing
- [x] #70 `collect_class` per-key winner accessor for the LSP — MR !1 approved by Shockwave, merged to develop
- [ ] #78 LSP didChange-on-unopened guard — MR !4 open, Shockwave reviewing
- [x] #81 Cargo manifest hygiene — MR !3 merged to develop
- [ ] Open issues re-triaged; owner-action issues labelled and noted

## Comments

- **starscream** (2026-09-06T01:20:00.000Z): Took over. Committed the refreshed skill set and Seekers roster to `main` (2b59513, 18481b6). First push showed the coverage jobs red on a cold cache; fixed in .gitlab-ci.yml:110-112 on `main` (75e8d66). Local baseline on `main`: fmt/clippy clean, `cargo test --workspace` 2613 passed / 0 failed across 67 binaries, cucumber 910/910 + 214/214.
- **starscream** (2026-09-06T02:10:00.000Z): Found `origin/develop` 8 commits ahead of `main` (PR #61 hardening, 295 files, +38k; GitLab CI port; mutation testing removed per decision 13) and `main` 5 ahead (my meta commits + PR #55 merge). Merge probe: one conflict, .gitlab-ci.yml — took `develop`'s 970-line pipeline of record, which already carries the `mkdir -p "$CARGO_HOME/bin"` fix at .gitlab-ci.yml:277. Gates running on the merged tree. Dispatched Thundercracker on #69 and #70 (worktrees off develop), Soundwave on a read-only styleguide audit. Wrote decisions/14-seekers-adoption-gitlab-is-the-record.md and docs/bots/ROADMAP.md.
- **starscream** (2026-09-06T02:40:00.000Z): Soundwave audit returned: crate graph acyclic and SPEC §16-conformant, restriction lints honoured, no swallowed I/O errors outside crates/luabox-manifest/src/layout.rs:411-608. Filed #75 (LB0317/18 report policy duplicated CLI/LSP, LSP lacks depth pre-check), #76 (luacats doc/type render helpers duplicated doc_cmd/sema, hover collapses paragraphs), #77 (layout.rs drops unreadable rock sources silently — decided collect-and-report), #78 (server.rs:2323 didChange on unopened doc splices into empty string), #79 (CLI's direct luabox-hir dep), #80 (env.rs impl split + 11 bare allows, after #70/#73), #81 (Cargo hygiene). Dispatched Skywarp on #78, Skywarp-lite on #81.
- **starscream** (2026-09-06T09:30:00.000Z): Session resumed. #70 (MR !1) and #81 (MR !3) are merged to `develop` (now 1f0194e) — !1 carried two review rounds from Shockwave and an extra fix, `crates/luabox-types/src/env.rs` +450 for the per-key owner plus `fix(lsp): a carrier attachment keeps its parent's ---@field` (e73087a). #69 (MR !2, new codes LB0320/LB0321) and #78 (MR !4) are open with Shockwave reviewing and pipelines running. Merged `origin/develop` into this branch (ca95e2d) — clean, no conflict this time. Suite note carried from the pre-merge run: four `crates/luabox-lsp/tests/shutdown_windows.rs:170` failures, all `the server answers within the read bound: Timeout` against a fixed 10s `READ_TIMEOUT` (`:56`) while five cargo builds ran concurrently (load 40). Re-running quiet to separate flake from regression before the promotion MR.
- **starscream** (2026-09-06T09:45:00.000Z): Flake hypothesis confirmed — `cargo test -p luabox-lsp --test shutdown_windows` 6/6 green twice at load 1.7, 1.05s per run, against 4/6 failing at load 40. Filed #84 (scale `READ_TIMEOUT` by `LUABOX_PERF_FACTOR` or bound on progress). Wrote root `STATUS.md` per the project-status skill. Running the full workspace suite on the merged tree before opening the MR into `develop`.

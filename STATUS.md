# Status

**Updated:** 2026-09-06 16:10 UTC · **Integration branch:** `develop` · **Release branch:** `main` · **Owner:** Starscream (Seekers)

SHAs are not pinned here — the tracker and `git log` are authoritative; this file
carries states and measurements with the pipeline id that produced them.

Radiator for the working project. Tracker of record is GitLab `origin`
(decision 14); milestones in [docs/bots/ROADMAP.md](docs/bots/ROADMAP.md);
sprint detail on [board card 35](boards/improvement-sprint/35-seekers-adoption.md).

## Goal / health

**Goal (M0):** sign luabox off as adopted to the Seekers' standard — one tracker,
one pipeline of record, `main` reconciled with `develop`, the audit findings
filed, and the ready work shipped through reviewed MRs.

**Health: green.** Gates on `main` @ `75e8d66`: pipeline 20155 success (fmt,
clippy, test). Local baseline there: 2613 unit tests, 910 CLI + 214 LSP cucumber
scenarios, 0 failures. On the merged tree (`develop` @ `1f0194e` + `main`): 3032 unit
tests / 0 failed / 7 ignored across 72 binaries, 924 + 235 scenarios, fmt and
clippy clean; the four `luabox-lsp` shutdown-window timeouts seen under load
40 reproduce as 6/6 green twice on a quiet host — load sensitivity, filed as #84.

## Now

Landing the reconciliation (**!5**) and the cache policy (**!6**) on `develop`.
!5 is in Shockwave round 3: rounds 1–2 closed on evidence, pipeline 20285
18/18 green, the remaining blockers are this file's own staleness, a merge
conflict with !2's `limitations.md` section (resolved in favour of `develop`
with the bare `#NN` citation form re-applied), three spec citations, one name
in the memory index. !6 is in Shockwave re-review at `36628ff` (hardened
sweep, 89-assertion selftest). Stopping point after both merge: the
`develop → main` promotion MR for the owner.

## Next

1. MR !5 (`starscream/integrate-develop → develop`): pipeline 20278 green
   (18/18); Shockwave round 2 findings being closed; then merge.
2. MR `develop → main` — the promotion. Reviewer Shockwave, approver the owner.
3. #78 (MR !4) merged as `cb40e10` after review (pipeline 20281 13/13);
   #69 (MR !2) merged as `ce215fc`. Both issues closed.

## Later

- **M2, gated on #66** (refined and dispatch-ready, 2026-09-06): #67 union
  same-rank duplicates, #68 nominal assignability, #73 cross-file redeclaration.
  Each is a strictness increase and must not ship before the per-code downgrade
  path exists.
- **M3:** #60 (thin `check.rs`'s shadowed P0 paths), #72 (`check_cmd.rs` pass
  structure) — do #79 first and re-measure; #75, #76, #77, #80 from the audit;
  #82 (CI check for test-only deps, raised out of #81), #83 (full-replace
  `didChange` overlay with no clearing event, raised out of #78), #84 (the
  shutdown-window bound), #86 (unreadable type argument never reported, raised
  out of #69). #80 waits for #73 to stop editing `env.rs`.
- **M4:** macOS/Windows runners on this instance, or a decision to keep the
  GitHub Actions matrix permanently.

## Outcomes

- `main` pipeline was red on arrival — both coverage jobs died extracting
  cargo-llvm-cov into a `$CARGO_HOME/bin` that does not exist on a cold cache.
  Fixed (`75e8d66`); pipeline 20155 green.
- Skill set and Seekers roster committed to `main` (`2b59513`, `18481b6`).
- Divergence found and being closed: `develop` carried PR #61's post-v1 hardening
  (295 files) and the whole GitLab CI port, never promoted to `main`.
- Soundwave read-only audit: crate graph acyclic and SPEC §16-conformant,
  restriction lints honoured, no swallowed I/O outside `layout.rs`. Twelve
  findings → issues #75–#81.
- #70 shipped (MR !1, Shockwave-approved after two rounds): `collect_class`'s
  per-key winner is exposed and the LSP consumes it, so hover, goto-definition
  and `luabox check` agree on diamond and cross-file-split shapes.
- #81 shipped (MR !3): manifest hygiene, no code change.
- #66 refined into a dispatch-ready story with Gherkin acceptance and a decided
  design (`[types.severity]`, LB1005 for unknown keys) — posted on the issue.
- Owner-action issues labelled `owner-action` and noted: #27, #28, #34.

## Blockers

- **#85 — diagnosed and half-fixed on the host (owner-approved).** Root
  cause was never Rust or GitLab: on `enterprise` (Unraid) job `/builds` and
  `/cache` were anonymous docker volumes inside the 80 GB `docker.img`, with
  `concurrent = 6`, and GitLab's local cache has no eviction (every hashed key
  leaves its old zip). Applied: `concurrent = 2`; `/builds` + `/cache` bound to
  the NVMe (`/mnt/cache/appdata/gitlab-runner/…`, 672 GB free); runner
  restarted, `max_builds=2`; owner ran the docker prune (docker.img 54.7 → 36
  GB); hourly host sweep installed — anything untouched 24 h is deleted, 60 GiB
  cap (owner's rule). **MR !6** carries the pipeline half: fixed cache keys
  (bounded growth), per-job trees kept, `df -h` first, the sweep script
  versioned at `scripts/ops/`, decision 15 — now hardened at `36628ff` after a
  nine-finding review (refusals, `flock`, occupancy re-probe, corpus exempt,
  89-assertion selftest); re-review in progress.
- The `shutdown_windows` timeouts are a fixed 10s wall-clock bound
  (`crates/luabox-lsp/tests/shutdown_windows.rs:56`) that fails only under
  CI-scale load — #84, backlog, not a regression.
- **Owner-gated, not team-blocked:** #27 (tag `v0.2.0`), #28 (approval-reset
  project setting), #34 (marketplace credentials). Nothing for the team to do.

## In flight

MR state only; SHAs live in the tracker.

- **!5** `starscream/integrate-develop → develop` — Shockwave round 3;
  pipeline green at the previous head; conflict with `develop` resolved.
- **!6** `thundercracker/issue-85 → develop` — hardened sweep + selftest +
  shellcheck job + decision 15; Shockwave re-review in progress.
- Merged this sprint: **!1** (#70), **!2** (#69), **!3** (#81), **!4** (#78).

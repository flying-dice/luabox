# Status

**Updated:** 2026-09-06 16:56 UTC (commit time) · **Integration branch:** `develop` · **Release branch:** `main` · **Owner:** Starscream (Seekers)

Rewritten from the tracker at every push. Commit SHAs appear only as the
identity of a measurement (a pipeline's head); states are read from GitLab
at the time stamped above. Tracker of record: GitLab `origin` (decision 14).
Milestones: [docs/bots/ROADMAP.md](docs/bots/ROADMAP.md). Sprint journal:
[board card 35](boards/improvement-sprint/35-seekers-adoption.md).

## Goal / health

**Goal (M0):** sign luabox off as adopted to the Seekers' standard — one tracker,
one pipeline of record, `main` reconciled with `develop`, the audit findings
filed, and the ready work shipped through reviewed MRs.

**Health.**
- `develop`: green — pipeline 20286 on the tree that merged !1–!4.
- `main`: **unproven at its tip.** Its last green pipeline (20262) ran five
  docs/memory-only commits ago; the pipelines for the later commits were
  cancelled to keep the two-slot runner on the MRs. The promotion MR's own
  pipeline is the proof that matters.
- !5 (this branch): pipeline 20293 green, 18/18 — `check` trace: 3074 passed /
  0 failed / 6 ignored over 74 binaries; 924 + 235 cucumber scenarios.
- !6: pipeline 20294 running at the round-3 fix; the previous head's pipeline
  (20291) was 18/18 with shellcheck clean.

## Now

Two MRs left before the promotion, both in Shockwave re-review:
- **!5** `starscream/integrate-develop → develop` — round 5 returned with the
  radiator (this file) as the only open finding; this commit is the fix:
  every section rewritten from the tracker, not patched.
- **!6** `thundercracker/issue-85 → develop` — round 4 in progress against the
  round-3 fix (cap alarm on the outcome, failed `rm` fatal, empty-dir `find`
  through `die`, `include:`/global `cache:` refused, 118 selftest assertions).

## Next

1. !5: Shockwave round 6 on this head → merge.
2. !6: Shockwave round 4 → merge.
3. `develop → main` promotion MR (its pipeline proves `main`'s tree; owner
   approves) → #27 tag `v0.2.0`.

## Later

- **M2, gated on #66** (refined and dispatch-ready): #67 union same-rank
  duplicates, #68 nominal assignability, #73 cross-file redeclaration. Each is
  a strictness increase and must not ship before the per-code downgrade path.
- **M3:** #60 (thin `check.rs`'s shadowed P0 paths), #72 (`check_cmd.rs` pass
  structure) — do #79 first and re-measure; #75, #76, #77, #80 from the audit;
  #82 (dependency-hygiene CI check), #83 (overlay with no clearing event), #84
  (shutdown-window test bound), #86 (unreadable type argument never reported),
  #87 (prose-wrap gate), #88 (`luabox explain` GitHub URLs). #80 waits for #73.
- **M4:** macOS/Windows runners on this instance, or a decision to keep the
  GitHub Actions matrix permanently.

## Outcomes

- `main` pipeline was red on arrival — coverage jobs died extracting
  cargo-llvm-cov into a `$CARGO_HOME/bin` that does not exist on a cold cache.
  Fixed; pipeline 20155 green.
- Skill set, Seekers roster and agent memory (no personal data) committed.
- Divergence closed: `develop` carried PR #61's post-v1 hardening (295 files)
  and the GitLab CI port, never promoted; !5 carries the reconciliation.
- Soundwave read-only audit: crate graph acyclic and SPEC §16-conformant,
  restriction lints honoured, no swallowed I/O outside `layout.rs`. Twelve
  findings → issues #75–#81.
- Shipped to `develop` through reviewed MRs: #70 (!1), #69 (!2: LB0320/LB0321
  at the declaration), #81 (!3), #78 (!4: no invented document on `didChange`).
- #85 diagnosed on the host: job workspaces and caches lived inside the 80 GB
  `docker.img` with `concurrent = 6`. Applied: `concurrent = 2`, `/builds` +
  `/cache` bound to the NVMe, docker pruned (54.7 → 36 GB), hourly sweep with
  the owner's 24 h rule (cron disabled until !6's hardened script lands).
- #66 refined into a dispatch-ready story with Gherkin acceptance and a decided
  design (`[types.severity]`, LB1005 for unknown keys).
- Owner-action issues labelled `owner-action` and noted: #27, #28, #34, #85.
- Approval rule set by the owner: Shockwave (Starscream-dispatched) approves
  into `develop`, Starscream merges; the owner approves `develop → main` and
  pushes tags (decision 14 clause 3).

## Blockers

- None team-side. `main`'s tip is unproven by pipeline (see Health) until the
  promotion MR runs.
- Owner-gated, not team-blocked: #27 (tag `v0.2.0`), #28 (approval-reset
  project setting), #34 (marketplace credentials), #85's host leftovers
  (runner `Restart=unless-stopped`, unused images, empty `[runners.cache]`
  block, re-enable the sweep cron once !6 merges).

## In flight

- **!5** — Shockwave round 6 pending on this head.
- **!6** — Shockwave round 4 in progress (pipeline 20294).
- Merged this sprint: **!1** (#70), **!2** (#69), **!3** (#81), **!4** (#78).

# Status

**Updated:** 2026-09-06 17:46 UTC (commit time) · **Integration branch:** `develop` · **Release branch:** `main` · **Owner role:** lead (vanilla roster; decision 16)

This is the historical 2026-09-06 tracker snapshot, not current branch health.
Current roster/memory policy is [decision 16](decisions/16-vanilla-installer-owned-harnesses.md).
Persona names and completed-process descriptions below describe that snapshot.
Consult GitLab for live work; this policy correction does not refresh ticket or
pipeline status. The snapshot is deliberately coarse: open MRs
are listed by number only — their pipelines, review rounds and heads live on
the MR and go stale within minutes here. Branch health names the last green
pipeline on the branch itself. Tracker of record: GitLab `origin` (decision 14).
Milestones: [docs/bots/ROADMAP.md](docs/bots/ROADMAP.md). Sprint journal:
[board card 35](boards/improvement-sprint/35-seekers-adoption.md).

## Goal / health

**Goal (M0):** sign luabox off as adopted to the Seekers' standard — one tracker,
one pipeline of record, `main` reconciled with `develop`, the audit findings
filed, and the ready work shipped through reviewed MRs.

**Health.**
- `develop`: green — last green pipeline 20286 on its tip at the time of this
  commit.
- `main`: **unproven at its tip** — its last green pipeline (20280) is three
  memory-only commits behind; the promotion MR's pipeline is the proof.
- Open MRs: their pipeline state is on the MR, not here.

## Now

Two MRs left before the promotion, both in Shockwave review rounds
(read the MR threads for the current round and pipeline):
- **!5** `starscream/integrate-develop → develop` — the reconciliation and this
  sprint's planning artefacts; the harness parity gate.
- **!6** `thundercracker/issue-85 → develop` — fixed cache keys, the hardened
  host sweep with its selftest, decision 15.

## Next

1. !5 and !6: Shockwave approves → Starscream merges.
2. `develop → main` promotion MR (its pipeline proves `main`'s tree; owner
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

- Open: **!5**, **!6** (state on the MRs).
- Merged this sprint: **!1** (#70), **!2** (#69), **!3** (#81), **!4** (#78).

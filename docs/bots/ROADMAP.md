# Roadmap

Owner: lead (vanilla flying-dice/bots roster; decision 16). Tracker: GitLab issues on `origin` (decision 14).
Sprint state: root `STATUS.md` + the current card on `boards/improvement-sprint/`.

The milestone/ticket list below is the 2026-09-06 planning snapshot, not a live
tracker refresh. Consult GitLab for completion and release status. Current role
and harness policy follows [decision 16](../../decisions/16-vanilla-installer-owned-harnesses.md).

## M0 — Adoption (2026-09-06 →)

Sign the repo off against the project quality gates (decision 14).

- `main` reconciled with `develop` (PR #61 hardening + GitLab CI port promoted).
- GitLab pipeline green on `develop` and `main`; coverage jobs fixed.
- `STATUS.md`, this roadmap, decision 14, sprint card 35 in place; backlog and
  development docs name GitLab as the record.
- Styleguide/architecture audit (architect) → findings filed as issues.
- Two implementation-ready issues shipped via MR: #69 (malformed `---@class`
  header diagnostic), #70 (`collect_class` per-key winner for the LSP).
- Exit: `develop → main` promotion MR open, awaiting owner approval.

## M1 — v0.2.0 release (owner-gated)

- #27 tag `v0.2.0` — fires `release.yml` (binaries, SHA256SUMS, install
  scripts, smoke-gated go-live). Prerequisite: CHANGELOG `[Unreleased]` → `0.2.0`.
- #34 marketplace publication of editor extensions (credentials only the owner holds).
- #28 approval-reset setting on the GitLab project.

## M2 — Strictness with an escape hatch

Every strictness increase needs a per-code downgrade path first.

- #66 per-code severity for `LB03xx` in `[types]` (reuses `[lint]`'s
  `allow|warn|deny` vocabulary; luals name aliases).
- Then, gated on #66: #67 union same-rank duplicate declarations (luals parity);
  #68 nominal class-to-class assignability (tightening half); #73 cross-file
  `---@class` redeclaration — one project-wide winner or a diagnostic.

## M3 — Checker single-owner cleanup and cost structure

- #60 thin `check.rs` P0 paths shadowed by inference; inference becomes the
  single owner of expression typing.
- #72 restructure `check_cmd.rs`'s three passes so the N=500 win holds with a
  generic class in `[types] defs`.
- Architecture audit findings from M0 that are structural rather than local:
  #75, #76, #77, #79, #80 — and the follow-ups the sprint's MRs raised:
  #82 (dependency-hygiene CI check), #83 (overlay clearing), #84 (test bound).

## M4 — Platform coverage on GitLab

- macOS/Windows runners (or a decision to keep the GitHub matrix permanently).
- `perf-gates`/`examples` stability on the shared runner (decision 12 posture).

## Not planned

Dependency management, runtimes, registry UX — parked post-v1 per
`DIRECTION.md` and decision 02. Nothing here reopens them.

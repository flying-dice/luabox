---
status: Accepted
date: 2026-08-07
---
# Decision 12 — Every quality gate has a self-test, fails when it measures nothing, and expensive audits ride a schedule while their self-test rides the merge path

## Context

This repo runs several gates that are not ordinary test suites: mutation testing over the merge seams, a lua-language-server parity differential, a perf/memory gate, and a verdict oracle. Each computes a judgement and reports pass or fail.

Across five review rounds, every one of them was found reporting success for a run that proved nothing: a mutation run whose only survivor timed out printed `OK`; a run that tested zero mutants printed "all reviewed"; a never-reviewed survivor was classified as a moved one and exited 0; the parity driver's luals column skipped silently when the binary was absent; the perf gate reported a memory pass over an empty corpus; the differential's expectation comparison could be deleted with its self-test still green. In each case the discriminator existed and simply never failed.

The pattern is that a gate is code, its failure mode is silence, and nothing was checking it. A gate nobody has watched fail is indistinguishable from a gate that cannot fail.

Separately, the mutation audit costs roughly forty minutes for four files. Running it on every push to an open PR is the merge-path cost it exists to avoid, but excluding it from `pull_request` means nothing in CI re-measures the allowlist's claim against a PR head before merge.

## Decision

1. **Every gate has a self-test** (`scripts/tests/*-selftest.sh`), driven by stubbed binaries so it runs in seconds without a build.
2. **Every self-test case is verified by mutation**: delete or neuter the line in the gate it claims to pin, confirm the suite fails, restore. A case that still passes with its target gone is a defect, not a test. Cases that cannot be made to discriminate are disclosed in the file rather than counted as coverage.
3. **A gate fails loudly when it measures nothing.** Concretely: a scoped path that does not exist, a run that generated no work, an expectation file with no rows, a corpus case with no committed row, a missing tool, an allowlist every line of which went stale at once. Absence of signal is never read as a pass.
4. **The self-test runs on the merge path; the expensive audit rides a schedule.** For mutation testing: `gate-selftest` runs per-PR in seconds and gates the audit job, which runs on schedule, on push to the default branch, and on manual dispatch. `workflow_dispatch` is the documented way to confirm a specific head on demand.
5. **The cost of (4) is stated where the claim is made.** The allowlist header says plainly that its measurement is a manual run, that `pull_request` structurally cannot re-verify it, and how to make CI verify it for a given head. Cadence traded for honesty about what CI did and did not check.

## Consequences

Four gates now carry self-tests totalling 88 cases, each verified by neutering the line it pins. Adding a gate means adding its self-test; the friction is the mechanism.

A reviewer challenged (4) twice, correctly, on the grounds that a header claiming "measured against this head" has nothing in CI able to confirm it pre-merge. The resolution was to make the claim honest rather than to pay forty minutes per push. If the audit ever becomes cheap enough to run per-PR, revisit — the trade is cost, not principle.

Generalisation: any check whose output is a judgement rather than a value needs a test that the judgement can come out negative. This applies to lint configurations, CI conditionals, and release gates as much as to these four.

> **2026-08-09:** the mutation gate this decision repeatedly cites was removed wholesale — see decision 13. The discipline here still governs the remaining gates.

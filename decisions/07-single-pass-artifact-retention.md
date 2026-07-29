---
status: Accepted
date: 2026-07-29
---
# Decision 07 — `check` retains per-file artifacts across its two passes

## Context

`luabox check` needs a cross-file surface pre-pass before the per-file check
pass. It used to re-read, re-parse and re-lower every file in each pass. Sprint
wave 4 (issue #42, commit 290b1c2) measured the alternatives on the 100-kLOC
perf corpus: a probe showed the syntax trees are only ~12 MiB of the retention
cost while HIR + harvested annotations are ~47 MiB — so a re-parse fallback
would keep most of the memory *and* surrender the read/parse saving.

## Decision

Fuse the passes: one parallel pass builds per-file artifacts (`FileArtifacts`
in crates/luabox-types/src/lib.rs) that both passes consume, retaining them
for the run's duration.

## Consequences

−5% CPU median and one read/parse per file, at ~1.9× peak RSS (64 → 123 MiB
on the 100-kLOC corpus) — memory now scales with project size held, not
streamed. Diagnostics proven byte-identical across every example project.
Recorded in CHANGELOG 0.2.0-unreleased notes; the wrapper APIs
(`module_surface`, `check_file_with_requires`) kept the LSP and luabox-db
unchanged.

**Now gated (wave 12).** The accepted 123 MiB had no ceiling: `check` could
have grown to 500 MiB on the same input with every gate still green. `check`'s
peak RSS on this corpus is now a CI-blocking leg of `scripts/perf-gate.sh`
(mirrored in `perf-gate.ps1`), budget 300 MiB — ~2.4× the accepted number,
wide enough to ignore allocator noise and narrow enough to catch another whole
regime of retention. Override with `LUABOX_RSS_BUDGET_MIB`; deliberately not
scaled by `LUABOX_PERF_FACTOR`.

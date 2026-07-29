---
column: done
labels: [wave, perf]
priority: med
updatedAt: 2026-07-29T05:20:00.000Z
---
# Wave 4 — single-pass check pipeline (#42)

`luabox check` read/parsed every file twice and lowered three times; fused into one artifact pass (crates/luabox-cli/src/check_cmd.rs, `FileArtifacts` in crates/luabox-types/src/lib.rs). Merged to develop in commit 290b1c2; byte-identical diagnostics proven across all 7 examples. Tracked as GitHub issue #42.

---
column: done
labels: [review-round]
priority: high
updatedAt: 2026-07-29T05:50:00.000Z
---
# Round-4 independent adversarial review (sprint exit gate)

Fresh reviewer over develop after waves 8a/8b/8c/9. Zero findings ends the sprint; any finding becomes the next wave. Mandatory gates + 60 min self-directed probing. Umbrella: GitHub issue #43.

## Comments

- **claude** (2026-07-29T05:50:00.000Z): Round-4 verdict: FAIL, 5 findings (2 MAJOR / 1 MEDIUM / 2 LOW). F1 wave-8b regression: bundler never learned shebang/BOM are file-prefix trivia - check passes, build --bundle dies with internal bundler error (crates/luabox-bundle/src/lib.rs:270-296). F2 pre-existing: --watch enters a permanent rerun loop after the first edit - notify OPEN events from the rerun's own reads re-trigger it (crates/luabox-cli/src/watch.rs:90-128). F3: wave-8c linearity incomplete - line_col walks chars per column, quadratic again on single-line files (crates/luabox-diag/src/line_index.rs:59-77), human renderer also emits the whole line per label (5.6 GB for 377 KB input). F4 ci.yml perf-gate comment stale. F5 LIMITATIONS off-by-one (511 terms parse, not 512). Everything else held: all gates green, waves 8a/9 clean under attack, 62/62 manifest sweep, 17/17 LSP assertions. Findings become wave 10.
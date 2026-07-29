---
column: doing
labels: [wave, review-round]
priority: high
agent: opus-w10
live: true
status: agent brief being written
updatedAt: 2026-07-29T05:50:00.000Z
---
# Wave 10 — round-4 findings (bundler prefix trivia, --watch loop, column linearity)

F1: bundle must strip/relocate SHEBANG/BOM trivia when splicing modules (crates/luabox-bundle/src/lib.rs:270-296) and stop silently dropping the entry shebang under --minify. F2: filter notify event kinds and drain self-triggered events in the --watch loop (crates/luabox-cli/src/watch.rs:90-128). F3: O(1)-amortized column lookup + rustc-style long-line truncation in the human renderer (crates/luabox-diag/src/line_index.rs, render.rs). F4: ci.yml gate comment. F5: docs/03-reference/02-limitations.md off-by-one.

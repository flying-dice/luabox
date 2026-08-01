---
column: doing
labels: [wave, review-round]
priority: high
agent: opus-w14
live: true
status: launching
updatedAt: 2026-08-01T00:25:00.000Z
---
# Wave 14 — Shockwave round-2 threads on PR #47

1 (bug): --target must reach the control-flow legality pass (crates/luabox-cli/src/check_cmd.rs pass 3) + target scenarios in control-flow.feature. 3 (question->fix): keep luals parity AND the runtime truth - new lint rule for setmetatable(t, C) where C.__index is never assigned; answer the thread with the decision. 4: replace the /100==5 magic decade with a luabox-diag band query + registry-asserted invariant. 5: measure LSP initialize->first-diagnostics on a big annotated rock tree; parallelize the LSP harvest (rayon) or move it off the critical path; ship the number. Path-filter issue already fixed directly (3f3ada0).

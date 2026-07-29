---
column: doing
labels: [wave]
priority: high
agent: opus-13a
live: true
status: launching
updatedAt: 2026-07-29T19:11:00.000Z
---
# Wave 13a — goto/label/break legality diagnostics (#44)

Owner pre-release burn-down. Unresolved goto, duplicate label, break outside a loop must diagnose like reference Lua rejects them. HIR already resolves goto targets (crates/luabox-hir/src/hir.rs:185).

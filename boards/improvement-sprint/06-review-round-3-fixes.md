---
column: done
labels: [review-round, perf]
priority: high
updatedAt: 2026-07-29T05:20:00.000Z
---
# Waves 8a/8b/8c — round-3 review findings (9/9 fixed)

Third review found 4 MAJOR product bugs in unreached corners: gitlab format line numbers (crates/luabox-diag/src/render.rs), LSP death on malformed requests (crates/luabox-lsp/src/server.rs), two independent quadratics (crates/luabox-syntax/src/line_index.rs, crates/luabox-diag/src/line_index.rs), shebang/BOM/nesting-limit parser gaps (crates/luabox-syntax/src/lua/lexer.rs). All merged: 3925201 (8a), 9493c04 (8b), 586f303 (8c); 5 MINOR doc fixes in a74167b.

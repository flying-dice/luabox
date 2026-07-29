---
column: done
labels: [wave, docs]
priority: med
updatedAt: 2026-07-29T05:20:00.000Z
---
# Wave 9 — manifest contract single-sourced (#41)

One const table (crates/luabox-manifest/src/contract.rs) now drives both the parser allowlists and the generated JSON schema; checked-in schema is a blessed artifact (`LUABOX_BLESS=1`). Owner-approved design over serde derive — decision recorded in DIRECTION.md and decisions/05. Merged in da546f6.

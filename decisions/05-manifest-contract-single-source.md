---
status: Accepted
date: 2026-07-29
---
# Decision 05 — The manifest contract is single-sourced from a declarative table, not serde

## Context

Wave 6 shipped a hand-authored JSON Schema pinned to the hand-rolled parser by a parity suite. The owner asked whether the structs could be the single source. A serde route was evaluated and rejected: no serde-based engine provides batch errors + per-error byte spans + did-you-mean over keys and enum values (eserde gets one of three). Full note: DIRECTION.md, sprint wave 9, issue #41.

## Decision

One const table (crates/luabox-manifest/src/contract.rs) drives the parser's allowlists and the generated schema; the checked-in schema file is a blessed artifact (LUABOX_BLESS=1 regenerates); cross-key rules stay hand-coded on both sides, guarded by the valid/invalid corpus.

## Consequences

Schema drift is now unrepresentable for keys/enums/descriptions and corpus-caught for the rest. Revisit serde only if an engine grows span support and suggestion hooks.

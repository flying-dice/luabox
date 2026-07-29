---
status: Accepted
date: 2026-07-26
---
# Decision 03 — Two isolated coverage suites with CI floors

## Context

The owner asked for social AND mockist confidence doubled up: black-box e2e coverage measured separately from unit coverage so each style independently guarantees behaviour. Enforced since the coverage waves (.github/workflows/ci.yml, scripts/per-crate-coverage.py, issue #17).

## Decision

CI runs coverage twice: unit (everything in-process, floor 95 aggregate + 92 per-crate) and e2e (cucumber suites driving the real binary, floor 83). Floors only ratchet up.

## Consequences

Every crate stays honestly tested in both styles; a PR that erodes either suite fails CI. The exact floors live in ci.yml - it is authoritative, not this record.

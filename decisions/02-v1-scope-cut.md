---
status: Accepted
date: 2026-07-26
---
# Decision 02 — v1 consumes a rock tree; it never produces one and never spawns an interpreter

## Context

Dependency management (solver, installers, publish, credentials) was the gnarliest, least-finished surface. The owner directed a scope rollback to nail the static core first. Full record: DIRECTION.md#v1-scope-cut-accepted-2026-07-26, GitHub issues #10/#11.

## Decision

Cut resolution/installation/publishing/execution from v1. luabox reads a lua_modules/ tree the user materializes with luarocks; three deliberate child-process exceptions (upgrade, doc --open, build --mode love packaging).

## Consequences

0.2.0 removed nine commands and two crates (CHANGELOG.md). The read seam (require resolution, cross-package types, bundling over a rock tree) stays and is e2e-tested. Registry direction is parked, not reversed.

---
status: Accepted
date: 2026-07-14
---
# Decision 01 — A static toolchain that verifies stock LuaCATS

## Context

Lua's ecosystem already annotates code with LuaCATS (the lua-language-server dialect), but luals trusts annotations rather than verifying them. The repo's first commits (2026-07-14, git log) already carry this shape: checker, linter, formatter, bundler, LSP in one binary. Reconstructed from DIRECTION.md's north star - the reasoning there is authoritative.

## Decision

Build a cargo-style, single-binary static toolchain whose type checker treats stock LuaCATS annotations as claims to be verified - no luabox-specific type format.

## Consequences

Interop with the luals ecosystem is free; strictness beyond luals is the differentiator. The toolchain must never need a Lua runtime to do its job (see Decision 02).

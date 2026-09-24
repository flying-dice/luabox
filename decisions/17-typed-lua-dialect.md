---
status: Proposed
date: 2026-09-24
---
# Decision 17 — Typed Lua: typed source with C-style headers that erases to Lua

## Context

Decision 01 made stock LuaCATS the one type format, checked more strictly
than lua-language-server. In practice, the checker's hardest problems come
from inferred types, and the payoff over a strict lua-language-server
configuration is small: the same annotations, checked a little harder.
Comment annotations also stay optional and invisible in a way that makes
"statically typed Lua" hard to deliver.

## Decision

Define a small typed dialect of Lua, specified in
[docs/05-typed-lua/](../docs/05-typed-lua/01-overview.md):

1. `.luac` source files with types after names (`local x: number`,
   `function f(p: Point): number`, `typedef Point = {…}`), always strictly
   checked, compiled by replacing types with spaces so output keeps every
   line and column. Every Lua program parses unchanged.
2. `.luah` headers as the interface of every shared module, including plain
   Lua and native modules, brought in with `#include`; globals declared
   `extern`.
3. Types are written or taken one step from a typed value; nothing is
   inferred from usage. Tables are fixed at construction. No classes,
   operator overloading, macros or runtime support.

## Consequences

- If accepted, this supersedes decision 01 and the LuaCATS north star in
  DIRECTION.md. The LuaCATS checker is not extended further.
- The toolchain work follows the spec: parser, checker, erasure compiler,
  header resolution, then LSP. `examples/typed-lua/` holds hand-written
  compiler output that becomes the compiler's golden tests.
- Existing plain Lua keeps working unchanged: it is used through headers.
- `typedef` becomes a reserved word; nothing else in Lua changes meaning.

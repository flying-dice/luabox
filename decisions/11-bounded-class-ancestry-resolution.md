---
status: Accepted
date: 2026-08-07
---
# Decision 11 — Class-ancestry resolution is bounded and reports a diagnostic, rather than growing the stack

## Context

Resolving a class walks its ancestor chain recursively, one native stack frame per link, so the survivable depth is a property of the thread the walk happens to run on. Measured on the round-5 tree, in the debug profile CI runs: ~7,900-7,999 links on a 16 MiB pinned thread and ~990-999 on an un-pinned default-stack thread — both pinned by the bisection `env.rs`'s `MAX_ANCESTRY_DEPTH` doc comment describes. A release build survives roughly an order of magnitude more, measured once during the investigation but deliberately not quoted as a bound here: no committed test measures it, and the limit is derived from the smallest floor rather than the most comfortable one — the last being what an embedder calling `luabox_lsp::run_stdio` directly gets, since that path pins nothing. Past the limit the process aborts: no diagnostic, no file name, no line.

Two remedies were considered and rejected. Raising the CLI's `PINNED_STACK_BYTES` breaks the invariant that the CLI and the LSP share one stack budget (`main.rs` has a test asserting they match) and, more importantly, only relocates the cliff — recursion depth is user-controlled and unbounded, so any fixed stack has a depth that defeats it. A syntactic pre-check in the CLI was added first and is retained, but it guards one entry point; the LSP request path and an embedder are untouched by it.

Silently truncating the ancestry was rejected outright: a checker that returns an incomplete shape and says nothing is confidently wrong, which is worse than aborting. The first implementation of the erasure did exactly that in one place — resolving a truncated ancestry made a genuinely-missing required member read as satisfied, because `Ty::Unknown::admits_nil()` is true — and the fix was to separate the gating shape from the display shape.

## Decision

`DiamondGuard` refuses to recurse past `MAX_ANCESTRY_DEPTH` (200) and records the class in a ledger, which `check::run` drains into `LB0317` naming the file and the class, with a note that members above the limit are absent from the resolved shape. Because both `collect_class` and `collect_operators` funnel through the same guard, every caller is protected regardless of entry point.

The limit is derived from the **smallest** measured floor, not the largest: ~990 on the un-pinned thread, with a ~5x margin, matching this codebase's existing margin convention. The CLI's syntactic pre-check imports the same constant rather than carrying its own, so the two cannot drift — an earlier revision had them at 200 and 2,000, and a 400-link chain therefore produced a truncated shape plus cascading false `LB0306`s while the CLI's own message told the user 2,000 was acceptable.

## Consequences

A pathological hierarchy produces one actionable diagnostic instead of a crash, on every entry point. The cost is a hard ceiling on legitimate depth: 200 links is far beyond hand-written Lua but reachable by a generator emitting one class per row, and such a project will need to flatten its hierarchy or declare the members it reads nearer the leaf.

`LB0317` is registered in the explain registry, so `luabox explain LB0317` states why the limit exists and what to do about it.

Generalisation: a recursive walk over user-controlled structure needs an explicit bound with a reported diagnostic. Relying on the stack makes the failure depend on which thread the caller happened to use, and makes it an abort rather than a message.

---
name: clean-code-review
description: Multi-agent clean code audit — each principle gets its own agent
---

## Proportionality gate

Check diff size first:
- **> 50 lines changed OR > 3 files touched** → full sub-agent audit below.
- **Otherwise** → self-scan inline. One pass, same principles, no sub-agents. Tag violations you find.

## Agents (full audit only)

Launch parallel sub-agents, each scanning files touched by the current change + immediate surroundings.

1. **SRP** — functions doing two jobs, classes with multiple reasons to change, mixed I/O and logic.
2. **DRY** — copy-pasted blocks, duplicated constants, near-identical functions, repeated conditionals.
3. **Naming** — unclear/misleading names, generic names (manager/handler/processor), no intent revealed.
4. **Coupling** — concrete deps constructed inline, shared mutable state.
5. **Dead code** — unused functions, unreachable branches, commented-out code, stale imports.
6. **KISS** — unnecessary complexity, over-engineered abstractions, premature generalisation. 5-whys each finding. Can't justify it → violation.
7. **BOUNDARY** — contract-not-implementation coupling across module/system seams.
    - Consumer imports a concrete type from another module's internals instead of its
      published face (client, facade, package root, __all__).
    - Signatures, return types, or DI wiring name a concrete impl where the published
      contract belongs — so swapping the impl forces edits in the consumer.
    - Remote/service dep constructed against a specific node/version/impl rather than
      routed through its gateway/client abstraction.
    - Foreign bounded-context model used raw across the seam, no translation (missing ACL).
    - Dependency points toward the more-volatile side (stable code depending on volatile concretes).
8. **PANIC-SAFETY** — production paths that abort the process on reachable input.
    - `.unwrap()` / `.expect()` on a `Result`/`Option` whose `Err`/`None` is reachable
      from real input (external data, FFI, IO, parsing, user files, env, locks).
    - Slice/index `xs[i]` / `src[a..b]` without a bounds guard; integer arithmetic that
      can overflow; `panic!` / `unreachable!` / `todo!` / `unimplemented!` on a live path.
    - Every error path handled: propagate a `Result`, fall back to a safe value, or guard
      the precondition. Blast radius scales severity — a panic in the in-DCS bridge
      crashes the sim; in the IDE host, the app; engines that claim totality must never
      panic on any input.

## BOUNDARY gate (inverse of KISS)
Fire ONLY where a real seam exists: a cross-system or cross-context call, a trust/security
perimeter, or a swap that is actual or credibly imminent. Each finding must name the decision
the boundary lets change independently. Can't name one → don't flag — and flagging it anyway
is itself a KISS violation.

## PANIC-SAFETY gate
Fire ONLY when the `Err`/`None`/out-of-bounds is REACHABLE from real input. An unwrap that is
provably safe by an invariant established just above it (immediately after the matching insert,
on a compile-time constant, on a value just `is_some()`-checked) is NOT a violation — but the
invariant should be named in a comment. Test code (`#[cfg(test)]` modules, `tests/`, fixtures,
mocks) is EXEMPT: `unwrap`/`expect` there is idiomatic and documents the test's assumptions.
Name the reachable input that triggers each flagged panic; can't name one → it's safe, don't flag.

## Each agent

- Reports: file, line range, description, severity (0–1).
- Ignores: test boilerplate, framework-mandated patterns, pre-existing issues outside the diff.

## Consolidation

For each violation scoring **> 0.5**:

```
// TODO: clean-code - <0-1 score> - <SRP|DRY|NAMING|COUPLING|DEAD|KISS|BOUNDARY|PANIC>: <description>
```

Add at the violation site. Violations you introduced this session scoring > 0.5 → fix immediately.
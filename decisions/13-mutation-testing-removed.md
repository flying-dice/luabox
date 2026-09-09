# 13. Mutation testing is removed — the tests stay, the machinery goes

Date: 2026-08-09. Operator decision (Jonathan).

## Decision

Mutation testing is removed from this repository wholesale: the gate script
(`mutants-gate.sh`), its self-test, the reviewed survivor allowlist, and both
CI jobs (the diff-bounded per-PR run and the scheduled full audit).

## Why

Time cost above defect yield. Measured on this infrastructure: a full audit
ran ~2 hours serialized on the single untagged runner; the diff-bounded PR
job cost ~25–30 minutes on every push and repeatedly became the long pole of
the review loop. The audits did find real fixture gaps during PR #61's review
rounds — that record stands in the history — but each finding also cost a
full pipeline cycle to confirm killed, and the operator judges the practice
too slow for practical benefit at this team's scale.

## What survives

Every regression test a mutation run ever produced stays in the ordinary
suites — arm-isolating tests, bounded-time pins, ledger-fold tests. The tests
were the value. Decision 12's discipline (gates are self-tested; a gate that
can measure nothing must fail) continues to govern the remaining gates
(perf, verdict-differential, luals-differential).

## Consequences

- A fixture-that-cannot-fail now has to be caught by review and the red-first
  rule (docs/01-getting-started/02-development.md) rather than by an audit.
- The `[gap]`-class coverage debt the old allowlist documented is no longer
  tracked anywhere mechanical.
- Do not reintroduce the machinery without a fresh operator decision and a
  measured time budget.

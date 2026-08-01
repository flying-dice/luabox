---
column: doing
labels: [release, review]
priority: high
agent: opus-w16
live: true
status: Wave 16 launched: LB0510 regression + 9 accuracy items
updatedAt: 2026-08-01T05:30:00.000Z
---
# Wave 16: Shockwave round-5 threads on PR #47

Round 5 @ 4659cb5: 10 of 15 round-4 threads CLOSED on measurement (the whole target-legality axis closed by 324-row enumeration, zero broken artifacts). Verdict FAIL, 10 open: 2 bugs, 8 issues. refs #47

The blocking one: **LB0510 regressed** — wave 15's false-positive fixes over-suppressed. One added `__tostring` on a class with instance methods silences the rule while the program still crashes (measured across 28 shapes: 25 crash, rule fires on 5, 20 false negatives, 14 new). The alias arm settles on a bare `local mt = Counter` with NO write at all — naming the carrier twice anywhere disables the rule file-wide.

## Scope

- LB0510 (bug, blocking): operator suppression only when carrier has no instance methods; alias arm requires an actual write through the alias; 28-shape matrix as tests
- infer.rs #4 (carried): closes by dependency when LB0510 is fixed
- M (partial): "unreachable from every CLI path" comment is false — three reaching paths measured; narrow the comment
- I residuals: bench.sh stale 3.84x (Q), pin_worker_stacks untested, host disclosure on the sequential row
- N: startup harvest has no progress token but the rationale claims one — wrap it (connection exists in Server::new) and gate on capability
- O: reload progress token ungated on client capability — store the flag on Server, gate like bootstrap
- P: LIMITATIONS overstates (alias "write" not required by code; "is an operator metatable" asserts what the rule can't know) — rewrite with the LB0510 fix
- R: check --help contradicts manifest-target behavior; spec §2.1 line
- S: build_cmd.rs module doc stale (gate + residual twin unnamed)
- Non-blocking bundle: K preflight widen to LB000[0-9] + header line; E global source-order sort; nvim-plugin/manifest-tree/flag-override/68-row scenarios; server.rs:1272 LINT_SOURCE conditional one-liner

## Comments

- **claude** (2026-08-01T05:30:00.000Z): Worktree /home/user/wt-w16, branch sprint/w16-shockwave-round5 off develop@4659cb5. Opus agent launched with the round-5 review + all six inline threads.

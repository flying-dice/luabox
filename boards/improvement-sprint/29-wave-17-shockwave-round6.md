---
column: doing
labels: [release, review]
priority: high
agent: opus-w17
live: true
status: Wave 17 launched: LB0510 behavioral gate + 5 issues
updatedAt: 2026-08-01T07:45:00.000Z
---
# Wave 17: Shockwave round-6 threads on PR #47

Round 6 @ 460aa2e: 7 of 10 round-5 threads CLOSED (alias arm, infer #4, M, O, P, Q, S; N folded closed into I with a 96.1%-tokened startup window measured). Verdict FAIL, 6 open: 1 bug, 5 issues. refs #47

The bug: LB0510's third swing — the fire condition is structural (carrier has a colon method) where it needs to be behavioral (a method actually reached on an instance). 8 false positives in the metafield + never-invoked-method region (Cache __mode + :reset(), Guard __newindex + :reject(), Sorter __lt + :cmp() firing twice). The FN axis is done and stays done.

## Scope

- LB0510: gate the fire on an observed instance-side use when a non-index metafield is present; commit the shape matrix as a runnable harness; fix the LIMITATIONS causal clause (:191-194) and the ":189-191 nothing is ever looked up" overstatement
- SPEC :134-137: false claim that build --target turns on dialect legality (check_gate hardcodes control_flow_only; the flag is a lowering flag on build)
- T: reload progress request id + token are compile-time constants — three reloads share one id/token; monotonic counter into both
- U: Aliases::root is O(writes x chain depth) — 15s on a 1.2MB pathological file; path-compress or memoise at build
- V: diagnostics.rs:42 "every publisher goes through this" — two bypass source_for; route or narrow
- I residual: pin_worker_stacks still not isolated (deleting the call/.stack_size()/lowering both constants fails nothing)
- O note: move the capability gate inside begin_progress (second caller would reintroduce)

## Comments

- **claude** (2026-08-01T07:45:00.000Z): Worktree /home/user/wt-w17, branch sprint/w17-shockwave-round6 off develop@460aa2e. Opus agent launched with the round-6 review + T/U/V inline threads.

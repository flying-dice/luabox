---
column: done
labels: [release, review]
priority: high
agent: opus-w17
live: true
status: Done — PR #47 APPROVED (round 11) and merged as a63842d; issues #44/#33/#38/#39/#30 closed with evidence
updatedAt: 2026-08-01T15:05:00.000Z
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
- **claude** (2026-08-01T09:55:00.000Z): The wave-17 agent completed all six fixes across three commits (2ebadc1 behavioral LB0510 gate + linear alias walk + committed 16-shape matrix harness with runtime oracle + differential.yml wiring + LIMITATIONS fixes; 5ca1541 SPEC build-vs-check --target correction; 964b111 unique per-reload progress ids/tokens + capability gate inside both helpers + source_for routing for the two stray publishers + tests/pinned_stack.rs isolation test + CHANGELOG) then died to the disk-full incident (root fs hit 100%; ld killed with SIGBUS) before it could report. Recovered directly per the takeover plan: freed 26G of stale worktree build caches (wt-w15/w16/w17 target dirs), merged to develop clean, and ran the full battery myself on the merged head: fmt 0, clippy 0, cargo test --workspace 2416/0 (incl. pinned_stack: a_pinned_worker_survives_a_recursion_no_default_stack_could), acceptance 860/4678, lsp_acceptance 194/1406, control-flow differential 112/112, lb0510-matrix.sh: all 16 shapes match lint AND runtime (real Lua 5.4.6). Replying on the six round-6 threads next.

---
column: doing
labels: [release, review]
priority: high
agent: opus-w20
live: true
status: Wave 20 launched: one-sided pruning, second create window, flag bypass
updatedAt: 2026-08-01T13:05:00.000Z
---
# Wave 20: Shockwave round-9 threads on PR #47

Round 9 @ d32ffd8: 1 of 4 CLOSED (the Y tightenings). The fixtures-first process independently verified — Shockwave built the intermediate commit and measured the matrix red by exactly the claimed 22 rows; the reached-set fixpoint survived six transitivity probes. Verdict FAIL, 3 open: 2 bugs, 1 issue. refs #47

## Scope

- LB0510 pruning one-sided (bug): P1 else-of-literal-true never pruned (feeds the fixpoint); P2 ternary `cond and ctor or other` — and-rhs seeds unconditionally, is_falsy_literal never consulted on the BinOp path (the falsy case yields `other` and runs clean). 13 FP shapes / 6 root causes incl. flow-insensitive derivation (reassigned before use), two setmetatables on one table, dead numeric-for headers, no after-return pruning; 3 undisclosed FNs (called-then-replaced insert last-wins ×2, `local m = c.reset; m(c)`). Doc overclaims a fourth round (:312-314 "one approximation remains"; :260-262 and-rhs justification falsified; flow-insensitivity absent from LIMITATIONS though in rustdoc). Twin gaps: else-of-dead-if lacks its live twin; no numeric-for or ternary rows.
- B1 NOT FIXED (bug): run() opens TWO create windows (Server::new harvest, then bootstrap) before main_loop; window 2 drains the exit window 1 correctly left — byte-identical exit 1 to pre-fix. Second-reload-queued variant same. Test fixture one call short of production (never calls bootstrap between Server::new and main_loop). Also: after aborting on a queued shutdown the token is still returned → begin/report/end + a further create to a client that asked to shut down. Shockwave's suggested guard: await_progress_create returns Unanswered without touching the channel if pending already holds a session-ender.
- B2 flag bypass (issue): never-answers flag skips the wait on create #2, so an error to #2 is never read → progress under a refused token. Consult the flag after reading, or skip sending the create entirely when flagged.

## Comments

- **claude** (2026-08-01T13:05:00.000Z): Worktree /home/user/wt-w20, branch sprint/w20-shockwave-round9 off develop@d32ffd8. Fixtures-first rule stands. Opus agent launched with the round-9 review + all three inline threads.

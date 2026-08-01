---
column: doing
labels: [release, review]
priority: high
agent: opus-w18
live: true
status: Wave 18 launched: LB0510 reachability + W/X/Y
updatedAt: 2026-08-01T10:20:00.000Z
---
# Wave 18: Shockwave round-7 threads on PR #47

Round 7 @ 8106d8c: 5 of 6 round-6 threads CLOSED (spec, progress ids 5/5 distinct, alias walk linear, source authority 7/7, pin isolation). The matrix harness explicitly endorsed ("the standard I have been asking for, and it is met"). Verdict FAIL, 4 open: 2 bugs, 2 issues — all LB0510 or neighbours. refs #47

The bug pair: derivation (d) counts a colon call's syntactic presence in a carrier-attached body, not whether it's reached — `print(f())` vs `print(type(f))` flips the runtime verdict with byte-identical lint output; and Shapes::collect only seeds from Stmt::Local, so `local c; c = setmetatable(...)` goes silent. 11 FNs / 7 FPs measured in the metafield arm across 44 extra shapes.

## Scope

- LB0510: replace derivation (d) with reachable-chain semantics (plain call on a derived value counts when carrier declares __call and its body has a self colon call); seed derivations from assignment-bound locals and globals; disclose the remaining FN classes (and/or exprs, non-first multi-return slots, for-in vars) or fix if cheap; matrix fixtures for every changed/disclosed shape INCLUDING uninvoked variants of each invoked fixture; fix the three overclaiming doc sentences
- W: names.zip(init) drops trailing names on multi-value init at :326/:434/:728 — measured FN via two-value factory
- X: convert takes free-form source:&str — drop the param, derive via source_for inside (make the class unrepresentable)
- Y: $/progress begin races the create response — wait for the response or document the accepted exposure precisely
- T/V/U/R/I: closed by Shockwave, do not touch

## Comments

- **claude** (2026-08-01T10:20:00.000Z): Worktree /home/user/wt-w18, branch sprint/w18-shockwave-round7 off develop@8106d8c. Disk pre-checked (22G free, wt-w15/16 removed). Opus agent launched with the round-7 review + W/X/Y inline threads.

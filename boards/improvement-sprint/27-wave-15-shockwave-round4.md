---
column: doing
labels: [release, review]
priority: high
agent: opus-w15
live: true
status: Wave 15 launched: 13 open threads (4 PARTIAL, 9 new)
updatedAt: 2026-08-01T02:40:00.000Z
---
# Wave 15: Shockwave round-4 threads on PR #47

Round 4 @ 70a6b3a: A/C/5 CLOSED on measurement; 1/3/4/B PARTIAL; new D/E/F/G (bugs), H/I/J/K/L/M (issues). Verdict FAIL, 13 open. refs #47

Priority by blast radius (Shockwave's ranking): D (bundle modes ship unloadable artifacts, exit 0 — release-blocking), F/G (LB0510 cries wolf on operator-only metatables + misses cross-file shape), E (--target dedup mis-attributes first-defined-here, out-of-order output), I (LSP perf number unfalsifiable; stop-short premise contradicted by 946ms config-reload stall).

## Scope

- D: control_flow residual in luabox-bundle (~6 lines at lib.rs:357, HIR in hand) + bundle-mode acceptance scenarios
- E: dedup key includes secondary labels / prefer target verdict on disagreement; sort by span before render
- F: LB0510 silent on metafield-only carriers and aliased carriers
- G: LIMITATIONS states in-file-only + lint-only; fix rawset literal-key over-suppression
- H: [build] target feeds check passes when no CLI flag
- I: committed harness under scripts/, reconciled numbers (654 vs 545 contradiction), corrected premise comment (reload_config path), rayon pool pinned/documented for library entry points
- J: carrier tie-break last-wins (Lua binding semantics) not first-wins
- K: differential target column edition assumption enforced
- L: CHANGELOG partial-emit wording corrected
- M: build residual pass prefers spanned source-level findings

## Comments

- **claude** (2026-08-01T02:40:00.000Z): Worktree /home/user/wt-w15, branch sprint/w15-shockwave-round4 off develop@70a6b3a. Opus agent launched with full repro set from the round-4 review + inline threads.

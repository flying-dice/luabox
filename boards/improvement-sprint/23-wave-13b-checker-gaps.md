---
column: doing
labels: [wave]
priority: high
agent: opus-13b
live: true
status: launching
updatedAt: 2026-07-29T22:21:00.000Z
---
# Wave 13b — checker gaps #33 / #38 / #39

Owner pre-release burn-down. #38: ---@type fun(...) on an assignment must apply the declared signature and tags. #33: complete the method-tag surfacing residue across remaining carrier shapes. #39: carrier-style methods in ---@meta defs files fold into the class surface (or diagnose as unsupported).

## Comments

- **claude** (2026-07-29T22:21:00.000Z): Landed on the agent's behalf - the platform incident (six 5xx terminations) kept killing opus-13b during final verification, so its written work was verified and committed by the orchestrator. The diff (1400 insertions, 12 files) implements all three: #38 ---@type fun(...) on assignments applies signature+tags (crates/luabox-types/src/infer.rs, tests/annotated_assignments.rs, 8 tests); #33 residue across carrier shapes (tests/method_calls.rs, 118 lines); #39 defs carrier-member fold (crates/luabox-types/src/env.rs carrier_var_classes shared with the #115 pass, tests/defs_carrier_members.rs), plus 14 new acceptance scenarios across 5 feature files, 1 LSP scenario, CHANGELOG entries for all three, and LIMITATIONS updates. Verified by me in the worktree: 61 suites green, 754/754 acceptance, 188/188 LSP, clippy -D warnings rc=0, fmt rc=0. Honest caveat: the agent's probe tables and luals-parity narratives were lost to the incident - the evidence here is the test suite itself, which encodes the before/after per shape.

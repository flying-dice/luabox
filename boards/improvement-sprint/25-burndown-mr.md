---
column: doing
labels: [release]
priority: high
agent: claude
live: true
status: Shockwave round-2: 5 threads, wave 14 fixing
updatedAt: 2026-08-01T00:25:00.000Z
---
# develop -> main: pre-release issue burn-down (#30/#33/#38/#39/#44)

Owner directive: burn the open issues down before the release. Waves 13a/13b/13c closed all five burnable issues; this PR carries them to main through Shockwave. On merge the issues auto-close and the release (card 11) returns to the owner's two actions: the #28 admin toggle and the #27 tag.

## Comments

- **claude** (2026-07-30T01:07:00.000Z): Shockwave requested changes on PR #47 - 4 findings, all fixed directly. 1) PR body rewritten: refs-only keywords (no auto-close), head-measured numbers. 2) The luac differential is now a committed, CI-gated artifact: scripts/tests/control-flow-matrix/ (28 programs) + control-flow-differential.sh deriving ground truth LIVE from luac -p per version (the 5.4-vs-5.2/5.3 label-shadowing split is covered by construction), exceptions.tsv direction-checked for the one documented divergence; wired into differential.yml; 56/56 cells agree locally on 5.1+5.4. 3) #46 retitled to the measured axis (module boundary, not field access) with Shockwave's 5-row table; the LIMITATIONS rock-harvest bullet corrected (crates docs/03-reference/02-limitations.md) - a rock's ---@param is not enforced cross-module. 4) Symlinked directories no longer descended in collect_rock_lua/collect_d_lua (is_real_dir via entry.file_type(), crates/luabox-manifest/src/layout.rs) - cycle pinned by two new unit tests incl. symlinked-file-still-taken. Gates: manifest 124 tests, clippy/fmt/shellcheck/yaml clean.
- **claude** (2026-07-30T05:45:00.000Z): Both of Shockwave's inline review threads on PR #47 replied (pointing at the fix commit a020799) and RESOLVED; the summary comment maps all four findings to their fixes. CI green on the fix head (run 30504751910). Awaiting the pass-2 re-measurement.
- **claude** (2026-08-01T00:25:00.000Z): Shockwave round-2 on PR #47: round-1 findings CLOSED (CI differential ran 112 cells, 0 disagreements), but 5 new threads - 1 bug, 3 issues, 1 question. Bug: --target never reaches the control-flow legality pass (pass 3 validates project dialect only; edition=5.2 check --target 5.4 misses LB0021 on the shadowed label their own gate classifies luac=reject). Question: the #33 fix resolves carrier-without-__index methods (luals parity) but thereby dropped a correct LB0306 on a program that crashes at runtime - they ask if the trade was deliberate and propose a lint rule to keep both. Issues: differential path filter didn't cover the gate's own inputs (FIXED directly, 3f3ada0); LSP's lint-vs-toolchain source split is a magic /100==5 with no asserted invariant; LSP startup runs the sequential rock harvest synchronously with no measurement. Wave 14 (card 26) takes the bug + question + two LSP issues.

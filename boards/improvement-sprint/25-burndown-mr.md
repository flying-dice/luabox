---
column: doing
labels: [release]
priority: high
agent: claude
live: true
status: Shockwave round-1 findings fixed, awaiting re-review
updatedAt: 2026-07-30T05:45:00.000Z
---
# develop -> main: pre-release issue burn-down (#30/#33/#38/#39/#44)

Owner directive: burn the open issues down before the release. Waves 13a/13b/13c closed all five burnable issues; this PR carries them to main through Shockwave. On merge the issues auto-close and the release (card 11) returns to the owner's two actions: the #28 admin toggle and the #27 tag.

## Comments

- **claude** (2026-07-30T01:07:00.000Z): Shockwave requested changes on PR #47 - 4 findings, all fixed directly. 1) PR body rewritten: refs-only keywords (no auto-close), head-measured numbers. 2) The luac differential is now a committed, CI-gated artifact: scripts/tests/control-flow-matrix/ (28 programs) + control-flow-differential.sh deriving ground truth LIVE from luac -p per version (the 5.4-vs-5.2/5.3 label-shadowing split is covered by construction), exceptions.tsv direction-checked for the one documented divergence; wired into differential.yml; 56/56 cells agree locally on 5.1+5.4. 3) #46 retitled to the measured axis (module boundary, not field access) with Shockwave's 5-row table; the LIMITATIONS rock-harvest bullet corrected (crates docs/03-reference/02-limitations.md) - a rock's ---@param is not enforced cross-module. 4) Symlinked directories no longer descended in collect_rock_lua/collect_d_lua (is_real_dir via entry.file_type(), crates/luabox-manifest/src/layout.rs) - cycle pinned by two new unit tests incl. symlinked-file-still-taken. Gates: manifest 124 tests, clippy/fmt/shellcheck/yaml clean.
- **claude** (2026-07-30T05:45:00.000Z): Both of Shockwave's inline review threads on PR #47 replied (pointing at the fix commit a020799) and RESOLVED; the summary comment maps all four findings to their fixes. CI green on the fix head (run 30504751910). Awaiting the pass-2 re-measurement.

---
column: doing
labels: [wave]
priority: high
agent: claude
live: true
status: "#59 + #57 done — implementing #56 (LSP ambient environment)"
progress: 50
updatedAt: 2026-08-02T02:00:00.000Z
---
# Post-v1 hardening: issues #56–#59

The four follow-ups identified after the PR #55 burn-down, filed as GitHub
issues #56 (LSP ambient environment for hover/completion), #57
(lua-language-server differential CI oracle), #58 (fixture audit for shapes
that cannot fail), and #59 (unify duplicate-declaration merge seams + property
tests).

## Checklist

- [x] #59 — one shared positional-unification function across the three merge seams, proptest coverage
- [ ] #56 — hover/completion resolve through the ambient environment, limitation rows flipped
- [x] #57 — luals differential CI job with divergence allowlist
- [ ] #58 — mutation pass over luabox-types, weak fixtures strengthened, scheduled CI job

## Comments

- **claude** (2026-08-02T00:00:00.000Z): Card opened. Issues #56–#59 filed with the evidence from the Shockwave review rounds. Working on branch claude/production-readiness-assessment-9natxz based on develop head 210700e. Order: #59 first (foundation in luabox-types), then #56, #57, #58.
- **claude** (2026-08-02T02:00:00.000Z): #57 done. Parity is now a measured gate in the lb0510-matrix style: corpus scripts/tests/luals-differential/ (11 cases), two-column expectations in scripts/tests/luals-differential/expected.tsv:1 (divergences are justified rows, not an allowlist), driver scripts/tests/luals-differential.sh:1 (luals column SKIPs loudly without a binary), CI job .github/workflows/luals-parity.yml:1 pinning luals 3.13.5 by sha256. Measured 10/11 parity; the one divergence is luals 3.13.5 lacking generic-class `<T>` support entirely. The #49 residual is RESOLVED by measurement: luals reports missing-parameter for an omitted non-trailing `number|nil` param, so the conservative trailing-only bound matches luals exactly — docs/03-reference/02-limitations.md:729 updated from "could not be verified" to the measurement, and docs/01-getting-started/02-development.md:30 documents the local run. Harness verified green end-to-end locally with the real luals binary, and the SKIP path verified too.
- **claude** (2026-08-02T01:00:00.000Z): #59 done. The unification rule now has one owner: crates/luabox-types/src/env.rs:1791 (`class_param_unification`), called from all three seams — the templates at crates/luabox-types/src/env.rs:1723, the in-file fold at crates/luabox-types/src/env.rs:781, the workspace-global fold at crates/luabox-types/src/env.rs:409. Property suite crates/luabox-types/tests/duplicate_class_merge_property.rs:1 covers the shape space (param spellings × field placement × order × same/cross-file) with two-directional behavioural probes; a run surfaced that strict mode rejects `unknown` at call sites (LB0300 `found unknown`), so the surplus-parameter pin asserts the measured strict-side behaviour, not the leniency I first assumed. Gates: duplicate_class_merge 29/29, property 4/4, workspace green, acceptance 910/910, lsp_acceptance 214/214, clippy/fmt clean.

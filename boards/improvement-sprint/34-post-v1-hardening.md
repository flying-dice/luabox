---
column: doing
labels: [wave]
priority: high
agent: claude
live: true
status: "#59 done — starting #56 (LSP ambient environment)"
progress: 25
updatedAt: 2026-08-02T01:00:00.000Z
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
- [ ] #57 — luals differential CI job with divergence allowlist
- [ ] #58 — mutation pass over luabox-types, weak fixtures strengthened, scheduled CI job

## Comments

- **claude** (2026-08-02T00:00:00.000Z): Card opened. Issues #56–#59 filed with the evidence from the Shockwave review rounds. Working on branch claude/production-readiness-assessment-9natxz based on develop head 210700e. Order: #59 first (foundation in luabox-types), then #56, #57, #58.
- **claude** (2026-08-02T01:00:00.000Z): #59 done. The unification rule now has one owner: crates/luabox-types/src/env.rs:1791 (`class_param_unification`), called from all three seams — the templates at crates/luabox-types/src/env.rs:1723, the in-file fold at crates/luabox-types/src/env.rs:781, the workspace-global fold at crates/luabox-types/src/env.rs:409. Property suite crates/luabox-types/tests/duplicate_class_merge_property.rs:1 covers the shape space (param spellings × field placement × order × same/cross-file) with two-directional behavioural probes; a run surfaced that strict mode rejects `unknown` at call sites (LB0300 `found unknown`), so the surplus-parameter pin asserts the measured strict-side behaviour, not the leniency I first assumed. Gates: duplicate_class_merge 29/29, property 4/4, workspace green, acceptance 910/910, lsp_acceptance 214/214, clippy/fmt clean.

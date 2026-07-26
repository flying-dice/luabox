# Backlog

The backlog lives as issues on
[GitLab](https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/issues)
(labels: `release`, `blocker`, `icebox`, `shapes-v2`). This file is an
index only — the issues carry the user stories and acceptance criteria.

## v1 scope cut (2026-07-26)

Dependency management and interpreter execution were cut from v1 by owner
decision — luabox consumes a rock tree, it does not produce one, and it
never spawns an interpreter. The decision record is in
[DIRECTION.md](DIRECTION.md#v1-scope-cut-accepted-2026-07-26); the wave is
tracked on GitHub as flying-dice/luabox#10 (strip
`add`/`remove`/`install`/`update`/`vendor`, `search`/`outdated`,
`publish`/auth, the solver/providers/lockfile and `luabox-store`),
flying-dice/luabox#11 (strip `run`/`toolchain`),
flying-dice/luabox#12 (docs) and flying-dice/luabox#13 (gate verification
+ coverage-floor ratchet). Every registry-, credential- or runtime-shaped
item below is **parked post-v1** — not scheduled, not a known gap.

## Initial public release (milestone)

**The launch gate is complete.** Everything in
[DIRECTION.md](DIRECTION.md)'s parity + strictness tracks landed and was
probe-verified: generics (#84), cross-package type sharing (#108),
`---@class` conformance (#107), undefined-global (#103), undefined-field
strictness (#90), plus cross-file `require` typing with workspace-global
classes (#85). The `.luab` subsystem is removed (#109). Release
machinery landed: LICENSE (#92), CI config (#94), CHANGELOG + release
process (#97), coverage gated honestly (#100), registry story decided
(#101), def scalar fields (#105), call-return propagation (#106),
editor packaging (#102, publish steps residual). `shapes-v2` is merged
to `main` and pushed (#93); the end-user README quickstart (#96) and
LIMITATIONS.md (#99) shipped.

Two follow-up waves also landed and closed (2026-07-13/14): the checker
deepening pass — workspace-global `---@alias` incl. cyclic diagnosis
(#110, #123), alias parity (#116, #117), `:`-call receiver resolution
(#118), closest-overload reporting (#119), contextual typing (#120),
union exhaustiveness LB0315 (#121), `---@operator call` (#122), generic
class arity (#124) — and the LSP feature build-out: find-references,
rename, workspace symbols, signature help, goto type-def/impl, lint
diagnostics + autofixes, code actions, call hierarchy, document
highlight/folding/selection ranges, auto-require import completion, and
protocol maturity (#125–#135). The VS Code status bar item (#138) lands
in v0.1.0.

### Still open

- [#102](https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/issues/102)
  Distribute the editor integrations — VS Code and JetBrains now live in
  their own repos ([luabox-vscode](https://github.com/flying-dice/luabox-vscode),
  [luabox-jetbrains](https://github.com/flying-dice/luabox-jetbrains)) with
  their own release pipelines shipping installable artifacts. Open only for
  the residual credential-gated marketplace uploads (VS Code Marketplace /
  Open VSX / JetBrains Marketplace), which need publisher accounts/tokens
  this environment doesn't hold.

### Closed by this release

- [#95](https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/issues/95)
  End-user installation: prebuilt binaries — done. The v0.1.0 `v*` tag
  exercised the GitHub release pipeline end to end; prebuilt binaries for
  Linux/macOS/Windows now ship as smoke-gated release assets and the install
  scripts resolve them.

## Post-launch

- [#136](https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/issues/136)
  Test runner: execute doc examples as tests (`luabox test --doc`) —
  **moot**: `luabox test` was removed (flying-dice/luabox#1; toolchain, not
  a runtime); close on GitLab.
- [#137](https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/issues/137)
  Registry UX: `luabox search` + `login`/auth — **parked post-v1** by the
  scope cut above; those commands no longer exist. The luarocks.org
  registry *direction* stands for whenever dependency management returns.

_Everything else is closed. #83/#88/#89/#98/#104 were closed as
inconsistent with the north star; #85/#86/#87/#90/#91 graduated from the
icebox and shipped. See DIRECTION.md for the decision record._

# Backlog

The backlog lives as issues on
[GitLab](https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/issues)
(labels: `release`, `blocker`, `icebox`, `shapes-v2`). This file is an
index only — the issues carry the user stories and acceptance criteria.

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
protocol maturity (#125–#135).

### Still open

- [#95](https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/issues/95)
  End-user installation: prebuilt binaries — needs the first `v*` tag to
  exercise the CI release stage; install scripts are in place.
- [#102](https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/issues/102)
  Distribute the editor integrations — reduced to VS Code only for now;
  packaging done; open for the credential-gated publish steps
  (Marketplace/Open VSX tokens, release attachments).

## Post-launch

- [#136](https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/issues/136)
  Test runner: execute doc examples as tests (`luabox test --doc`).
- [#137](https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/issues/137)
  Registry UX: `luabox search` + `login`/auth — needs the hosted-registry
  work (adjacent to #95).

_Everything else is closed. #83/#88/#89/#98/#104 were closed as
inconsistent with the north star; #85/#86/#87/#90/#91 graduated from the
icebox and shipped. See DIRECTION.md for the decision record._

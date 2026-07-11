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
editor packaging (#102, publish steps residual).

### Still open

- [#93](https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/issues/93)
  Merge `shapes-v2` to `main` and push — the final step of this pass.
- [#95](https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/issues/95)
  End-user installation: prebuilt binaries — needs the first `v*` tag to
  exercise the CI release stage; install scripts are in place.
- [#96](https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/issues/96)
  README end-user quickstart — in progress.
- [#99](https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/issues/99)
  Known limitations (0.1) documented honestly — in progress.
- [#102](https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/issues/102)
  Distribute the editor integrations — packaging done; open for the
  credential-gated publish steps (Marketplace/Open VSX tokens, JetBrains
  signing, Zed registry mirror, release attachments).

## Post-launch

- [#110](https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/issues/110)
  Cross-file `---@alias` naming (classes/enums are workspace-global
  since #85; aliases still need defs or same-file).

_Everything else is closed. #83/#88/#89/#98/#104 were closed as
inconsistent with the north star; #85/#86/#87/#90/#91 graduated from the
icebox and shipped. See DIRECTION.md for the decision record._

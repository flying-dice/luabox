# Backlog

The backlog lives as issues on
[GitLab](https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/issues)
(labels: `release`, `blocker`, `icebox`, `shapes-v2`). This file is an
index only — the issues carry the user stories and acceptance criteria.

## Initial public release (milestone)

The toolchain is feature-complete; these are the release-machinery gaps
between "code-complete on a branch" and "a stranger can install and trust
it." **Blockers** must land before the first public release.

**Feature-parity + strictness (launch gate)** — the LuaCATS front-end must
verify everything luals does (parity, so interop is real) plus luabox's
strictness (the edge), before launch. See [DIRECTION.md](DIRECTION.md). All
verified missing today by the example conversion (commit `a01684b`).

Parity (match luals):

- [#84](https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/issues/84)
  Real generic type variables + generic classes in LuaCATS
- [#108](https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/issues/108)
  Cross-package LuaCATS type sharing

Strictness (exceed luals):

- [#107](https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/issues/107)
  Enforce `---@class` conformance (`: Interface`, `__index`-aware)
- [#103](https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/issues/103)
  `undefined-global` diagnostic (typo'd/unknown global reads)
- [#90](https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/issues/90)
  Strictness for un-shaped LuaCATS code (SPEC §19)

`.luab` removal (a *consequence* of reaching the gate, not a precondition):

- [#109](https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/issues/109)
  `.luab` subsystem removal (absorbs #83/#88/#89/#98 and the `.luab` bits of
  #91/#102)

Blockers:

- [#92](https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/issues/92)
  LICENSE file (MIT) at the repo root
- [#93](https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/issues/93)
  Merge `shapes-v2` to `main` and push (14 commits unpushed)
- [#94](https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/issues/94)
  CI on the canonical GitLab remote (only GitHub Actions exists today)
- [#95](https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/issues/95)
  End-user installation: prebuilt binaries + one-line install

Release-needed:

- [#96](https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/issues/96)
  README end-user quickstart
- [#97](https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/issues/97)
  CHANGELOG and version/tag/release process
- [#98](https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/issues/98)
  Rename `SHAPES-V2.md` → `SHAPES.md` (retire the migration name)
- [#99](https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/issues/99)
  Known limitations (0.1) documented honestly
- [#100](https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/issues/100)
  `luabox test --coverage`: implement or gate honestly
- [#101](https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/issues/101)
  Registry story for 0.1 (path/git/luarocks/`file://` vs hosted)
- [#102](https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/issues/102)
  Distribute the editor integrations
- [#105](https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/issues/105)
  Def-declared scalar fields on global tables aren't typed
- [#106](https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/issues/106)
  Call return type not propagated to an unannotated local (`local p = f()`)

## Ready

_None open — the SHAPES-V2 checker-quality and LSP waves are landed
(#77–#82 closed)._

## Icebox (post-launch)

- [#85](https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/issues/85)
  Cross-file `require` resolution
- [#86](https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/issues/86)
  Overload-aware call results and tuple types (LuaCATS parity polish)
- [#87](https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/issues/87)
  Docgen: list `---@class` implementors of an interface (post-#107)
- [#91](https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/issues/91)
  watch.rs env-flakiness (make robust or gate)

_Closed as inconsistent with the north star: #83 (folded into #108), #88,
#89, #98, #104. See DIRECTION.md._

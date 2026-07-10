# Backlog

The backlog lives as issues on
[GitLab](https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/issues)
(labels: `release`, `blocker`, `icebox`, `shapes-v2`). This file is an
index only — the issues carry the user stories and acceptance criteria.

## Initial public release (milestone)

The toolchain is feature-complete; these are the release-machinery gaps
between "code-complete on a branch" and "a stranger can install and trust
it." **Blockers** must land before the first public release.

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

## Ready

_None open — the SHAPES-V2 checker-quality and LSP waves are landed
(#77–#82 closed)._

## Icebox

- [#83](https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/issues/83)
  `[types] rename` for colliding dependency namespaces
- [#84](https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/issues/84)
  Real generic type variables in LuaCATS
- [#85](https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/issues/85)
  Cross-file `require` resolution
- [#86](https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/issues/86)
  Overload-aware call results and tuple types
- [#87](https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/issues/87)
  Docs: conformers listing on type pages
- [#88](https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/issues/88)
  tree-sitter parity for levelled long brackets
- [#89](https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/issues/89)
  v1 → v2 shapes migration tooling (`luabox fix`)
- [#90](https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/issues/90)
  Strictness for un-shaped LuaCATS code (SPEC §19)
- [#91](https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/issues/91)
  SHAPES-V2 housekeeping (cache key, Zed rev, watch.rs, completion gating)

# Overview

luabox is a cargo-style static toolchain for Lua: one binary for `check`,
`lint`, `fmt`, `build`, `doc`, and `lsp`, with a type checker that verifies
stock [LuaCATS](https://luals.github.io/wiki/annotations/) annotations.
It never spawns an interpreter and consumes a `lua_modules/` tree rather than
producing one.

Start with the [README](../../README.md) - it is the front door, with the
quickstart and install one-liners. From there:

- [The specification](../03-reference/01-spec.md) - the full design, section by
  section. Code comments cite it as `SPEC.md SN`.
- [Known limitations](../03-reference/02-limitations.md) - every gap a user is
  likely to hit, verified against the shipping binary.
- [Releasing](../02-guides/01-releasing.md) - how a tag becomes a verified,
  published release.
- [DIRECTION.md](../../DIRECTION.md) - the governing decision record (kept at
  the repo root; summarized in `decisions/`).
- [examples/](../../examples/README.md) - seven real projects, each passing the
  full gate in CI.

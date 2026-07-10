# Type-system direction (decision record)

Status: **accepted** (2026-07-11). Supersedes the direction of
[SHAPES-V2.md](SHAPES-V2.md) (the `.luab` shape DSL).

## North star

**LuaCATS is the one type format; luabox checks it more strictly than
lua-language-server; luabox-specific keyword extensions come only after
launching at feature parity.**

Three commitments, in order:

1. **Interop first.** The annotation format is LuaCATS (`---@class`,
   `---@field`, `---@alias`, `---@enum`, `---@param`, `---@return`,
   `---@generic`, `---@meta` def packages). Existing annotated codebases,
   community definition packages, and luals-compatible tooling all work
   unchanged. There is no competing luabox file format.
2. **Stricter.** luabox's edge is that it *verifies* what luals declares but
   trusts — real conformance, real generics, cross-package checking — on the
   same format. "Match/exceed luals."
3. **Keywords later.** Any luabox-specific syntax (new keywords, extensions)
   ships **only after** going live at feature parity, as extensions *on the
   LuaCATS base* — never as a second file type.

## What this drops

The **`.luab` shape DSL is dropped/parked.** It is precisely the "new
keywords" that commitment 3 defers. Keeping it now would ship the very
two-format problem this direction exists to avoid. Its *engines* are not
wasted: the monomorphization (`subst_ty`/`instantiate`) and structural
conformance / function-subtyping machinery are re-pointed onto the LuaCATS
front-end (see the launch gate). `.luab` is removed once the LuaCATS path
reaches parity — the removal is a *consequence* of parity, not a precondition
for starting the work.

## Launch gate: feature parity + strictness

"Feature parity" is testable: **everything luals verifies, plus luabox's
strictness.** Measured against the gap map the example conversion produced
(commit `a01684b`; all verified missing today). Two tracks, both gating
launch:

### Parity (match luals so interop is real)
- **[#84] generics** — `---@generic` functions and generic `---@class<T>`
  (both broken today; reuse `.luab`'s monomorphization engine).
- **[#108] cross-package type sharing** — a dependency's types visible and
  checked in a consumer (LuaCATS has none today; `.luab` had it).

### Strictness (exceed luals — the whole point)
- **[#107] `---@class` conformance** — a `: Interface` carrier must provide
  the interface's methods with compatible signatures, `__index`-aware so
  inheritance isn't wrongly flagged (not enforced today; re-point the
  deferred-conformance + function-subtyping engines off `---@type`/`.luab`).
- **[#103] undefined-global** — flag typo'd/unknown global reads (no rule
  today; the parsed `---@diagnostic disable: undefined-global` is a no-op).
- Retained from the SHAPES-V2 work, re-pointed onto LuaCATS: function
  signature subtyping, carrier `self`-inference, member-naming diagnostics,
  declaration-site labels, literal freshness.

Smaller correctness items surfaced along the way: **[#105]** def scalar
fields untyped, **[#106]** call return not propagated to unannotated locals.

## Sequencing

1. Build the parity + strictness items on the LuaCATS front-end (reusing the
   `.luab` engines).
2. Land the release machinery (LICENSE, install, CI, quickstart — see the
   Initial-public-release milestone).
3. **Go live** at feature parity + strictness.
4. Remove `.luab` (subsystem, tree-sitter grammar, editor `.luab` support,
   SHAPES-V2.md) once nothing depends on it.
5. **Post-launch:** add luabox keyword extensions on the LuaCATS base.

## Non-goals (for now)

- A second type file format.
- New keywords before launch.
- Nominal/`.luab`-style structural declarations as the authoring surface.

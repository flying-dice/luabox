---
status: Accepted
date: 2026-07-29
---
# Decision 09 — A bare luarocks tree yields types: harvest the rock's own annotations

## Context

`luarocks install --tree lua_modules <rock>` bought `require` resolution and
bundling but not types. Cross-package definitions reached a use site only when
all three of a `[dependencies]` entry, a `lua_modules/<pkg>/luabox.toml` with
`[types] defs`, and the `defs/` directory it names were present (#108). A
luarocks tree has none of them, so a rock stayed `unknown` to the checker;
README, docs/03-reference/01-spec.md §5 and docs/03-reference/02-limitations.md
all documented this as *the sharp edge* (flying-dice/luabox#30).

The rocks were never actually untyped. LuaCATS is the ecosystem's annotation
dialect, and a rock that documents itself for lua-language-server has already
written the signatures — they sit in the installed sources under
`lua_modules/share/lua/<X.Y>/`, which luabox reads for bundling and walks past
for checking. The one design question #30 flags is the strictness interaction:
vendored code is UNCHECKED (a #16 review finding: walking `lua_modules/` made
`check` typecheck rock sources against the project's strictness and took `build`
down with it), yet its *signatures* should be VISIBLE.

## Decision

**Harvest rule.** When a project has a luarocks tree for the version directory
its `[build] target` resolves against (`lua_modules/share/lua/<X.Y>/`, `5.1` for
LuaJIT), every `*.lua` under it is read in path-sorted order and reduced to its
*surface*: the `---@class`/`---@enum`/`---@alias` declarations it makes, and the
type a `require` of it evaluates to. Nothing else. Zero manifest declaration is
required — that is the point. A source with no `---@` anywhere is skipped before
it is parsed: its export would be a dynamically-shaped table of `unknown`s,
which could only manufacture `undefined-field` noise about code the user did not
write, so an unannotated rock stays exactly as it was — requirable, bundlable,
`unknown` to the checker.

Scope guard: the harvest requires the versioned `share/lua/<X.Y>/` layout. The
flat `lua_modules/<name>/` layout keeps its existing `[dependencies]` +
`[types] defs` path, unchanged. An explicit `[dependencies]` entry alongside a
rock tree neither breaks nor double-harvests: the entry contributes defs (from a
`luabox.toml` a rock tree does not have, so usually none) and the harvest
contributes surfaces, and the merge below de-duplicates by name.

**Strictness interaction: signatures visible, bodies never checked, failures
never gate.** The harvest runs `module_surface`, which by construction returns a
surface and no diagnostics — there is no channel through which a rock body's
type error can become project output, and no rock file is ever added to the
checked source set. A rock source that does not parse is skipped whole (a
recovered tree is a guess, and a guessed surface is worse than none), noted at
debug level in the LSP log and silent under `check`. A rock's annotations are
resolved *ambient-relaxed*: parsed with the richest dialect so nothing is
rejected for looking newer than the tree's version directory, exactly as `.d.lua`
definition files are parsed.

**Precedence — explicit beats implicit.** Winner order is: dialect stdlib and
`[types] defs` (project-local, then each direct dependency's, as #108 already
ordered them), then the project's own source files, then the harvested rocks.
Rock surfaces merge **whole-declaration first-wins**
(`Ambient::with_rock_types`): a class, enum or alias name already claimed by any
of the above is left exactly as it is, and only unclaimed names are inserted.
This is deliberately *not* the member-wise union `Ambient::with_project_types`
performs for project files (luals parity: two declarations in code you wrote are
two halves of one intent). A rock's declaration is not the user's, so when a
project declares `mylib.Point` itself it is correcting or replacing what the rock
says, and unioning the rock's fields back in would defeat the escape hatch.

Among harvested rocks the order is the deterministic path sort and the rule is
**silent** first-wins — no `LB0307`/`LB0310`. This diverges from `[types] defs`
collisions, which warn, and the reason is that a warning must be actionable: the
user declared neither side of a rock-vs-rock name clash, cannot edit vendored
code, and did not ask for either surface. Warning about it would make a
zero-configuration feature noisy on installation. The escape hatch is the answer
where it matters: declare the name yourself and you win outright.

Implementation: `luabox_manifest::layout::{RockSource, collect_rock_sources}`
(the path-only tree walk, `luabox_bundle::rocks_version_dir` shared with
resolution so harvest and `require` cannot look in different directories);
`luabox_types::rocks::{RockModule, RockSurfaces, harvest}` (the surface
reduction) and `Ambient::with_rock_types` (the precedence merge);
`luabox-cli/src/check_cmd.rs` (`harvest_rocks`, the path-keyed export registry)
and `luabox-lsp/src/server.rs` + `diagnostics.rs` (the same surfaces in the
editor, module-name-keyed because the database only holds project files).

## Consequences

A bare `luarocks install --tree lua_modules <rock>` now types the rock, with no
`luabox.toml` edit at all: its `---@class` names resolve (no more `LB0305`) and
are structurally enforced, `local m = require("rock")` carries the module's
export type so annotated return types flow into the consumer, and misusing a
rock-typed value — an undeclared field read, a class where a string is wanted —
is diagnosed **in the consumer**, at the consumer's strictness. The same
surfaces reach the editor, so hover, completion and published diagnostics agree
with CI.

What still needs explicit defs, stated honestly in
docs/03-reference/02-limitations.md: a rock with no LuaCATS annotations (nothing
to harvest); a rock whose API is a *global* rather than a module return (the
harvest contributes declarations and export types, not ambient globals — a
`love`-style framework still wants a def package); and argument checking at a
rock function's call site, which is a pre-existing checker gap for calls through
any table/class field, not a harvest limitation — a def-declared global API
(`geometry.point(…)`) is param-checked, a module field call is not, whether the
module is a rock or one of your own files.

Cost is proportional to the *annotated* portion of the rock tree and is paid
once per `check` run (parse + harvest + surface inference per annotated rock
file, no check pass). A project with no rock tree pays one failed `is_dir`, so
the clean-corpus perf legs are unmoved. The per-file reduction is pure and
independent, so `check` runs it on the rayon pool it already has and folds the
results in path order — the fold, not the compute order, is what fixes
precedence, so the parallel and sequential (LSP-startup) forms agree by
construction. Measured on a synthetic 100-kLOC, 50-file, *fully* annotated tree
in front of a one-file project (4 cores): 1.98 s sequential → 0.55 s parallel.
A realistically sized annotated rock is a fraction of that, and an unannotated
one costs a read plus a substring scan.

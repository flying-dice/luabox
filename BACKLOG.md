# Backlog

Open items after the SHAPES-V2 pivot (branch `shapes-v2`), in user-story
format. Statuses: **in flight** (being implemented right now), **ready**
(specced, unstarted), **icebox** (deliberately parked — pick up only with a
reason). Remove a story when its acceptance criteria are verified green.

---

## In flight

### Typed `self` in shape carriers

**As a** library author writing the idiomatic v2 carrier (`local Circle = {}`
… `---@return geometry.Circle` constructor returning `setmetatable(lit,
Circle)`), **I want** `self` inside `Circle:method()` to type as
`geometry.Circle`, **so that** `self.radius` is checked against the declared
field types without any extra annotation.

- [ ] `self.radius` misuse in a carrier method is diagnosed in strict mode.
- [ ] The geometry example's carriers pass strict with zero diagnostics.
- [ ] Explicit `---@param self T` and explicit `---@class` both take
      precedence over the inferred tie.
- Ref: SHAPES-V2.md "Lua-side consumption"; mechanism mirrors
  `declared_targets` in `crates/luabox-types/src/env.rs`.

## Ready — queued for the current effort

### Generic arity errors at annotation sites

**As a** Lua developer writing `---@type geometry.Pair<number, string>`,
**I want** the wrong argument count reported at the annotation, **so that**
mistakes don't silently collapse to `unknown`.

- [ ] Wrong arity and args-on-non-generic produce a diagnostic with the
      annotation's span (mirror the `unknown_names` → LB0305 plumbing).
- Ref: `TODO(P1)` in `crates/luabox-types/src/lower.rs` (`lower_named`,
  scratch-Vec drop).

### "Did you mean `geometry.Point`?" on unknown type names

**As a** developer typing a bare short name (`---@type Point`), **I want**
LB0305 to suggest the fully-qualified shape name when one matches, **so
that** the FQ-only rule teaches instead of stonewalling.

- [ ] LB0305 carries a note naming candidate FQ types whose last segment
      matches.

### Declaration-site labels on conformance errors

**As a** developer reading a `type mismatch: expected geometry.Shape`
error, **I want** a secondary label pointing at the `.luab` declaration,
**so that** I can jump to the contract I failed.

- [ ] LB0300/LB0302-family diagnostics against shape types carry a
      secondary span at the declaring `.luab` item (`TypeShape` already
      holds file + range; thread it into the env).
- Ref: SHAPES-V2.md "assignability errors … point at the `.luab`
  declaration".

### Diagnose the lenient `.luab` edges

**As a** shape author, **I want** malformed type expressions rejected in
the `.luab` file, **so that** mistakes don't silently degrade to `unknown`.

- [ ] A multi-return paren list `(A, B)` outside return position is an
      error (today the first member is silently taken).
- [ ] An intersection member that isn't an object/table type (e.g.
      `number & string`) is an error (today the whole intersection
      collapses to `unknown`).
- Ref: `crates/luabox-types/src/shape/raw.rs` (`convert_ty` Paren arm),
  `scope.rs` (`lower_intersection`).

### Pin sealed-key semantics beyond literals

**As a** developer with `---@type geometry.Point` on a local, **I want**
later unknown-key writes/reads (`p.z = 1`, `print(p.z)`) diagnosed like
v1's LB2002 did, **so that** sealing means sealed, not literal-only.

- [ ] Tests pin the current behavior for post-literal reads and writes on
      sealed shape-typed values; implement enforcement if absent.

## Icebox — parked deliberately

### `[types] rename` for colliding dependency namespaces

**As a** consumer of two dependencies that publish the same package name,
**I want** a manifest-level rename (`[types] rename = { geo = "geometry" }`),
**so that** both surfaces are addressable without either package changing.
Parked: the collision requires two deps with identical package names —
rare; revisit on first real report. (`---@import` stays a non-goal.)

### Real generic type variables in LuaCATS

**As a** LuaCATS user, **I want** `---@generic T` to be a real type
variable with constraint solving (today it lowers to `unknown`), **so
that** generic functions check instead of degrade. Pre-existing `TODO(P1)`
in `lower.rs`; shapes inherit whatever lands here.

### Cross-file `require` resolution

**As a** developer, **I want** `require("circle")` results typed from the
required module's annotations, **so that** conformance assertions work in
test files, not just the defining file. Pre-existing P1
(`crates/luabox-types/src/lib.rs` doc comment); it's why the geometry
conformance assertion lives in `circle.lua` rather than the test.

### Overload-aware call results and tuple types

**As a** LuaCATS user, **I want** calls to pick the matching overload's
return type, and real tuple types, **so that** overloaded stdlib APIs and
tuple annotations check precisely. Pre-existing `TODO(P1)`s
(`ty.rs::FunctionTy::overloads`, `lower.rs::Tuple`).

### Docs: conformers listing on type pages

**As a** documentation reader on `type.geometry.Shape.html`, **I want** a
"conformers" section derived from positional-use sites, **so that** the
v1 implementors listing has a v2 equivalent. Dropped with the impl
registry; needs a use-site index in `luabox doc`.

### tree-sitter parity for levelled long brackets

**As a** Zed/Helix user, **I want** `--[=[ ]=]` comments highlighted as
blocks and `----` demoted to a plain comment, **so that** editor
highlighting matches the rowan lexer exactly. Needs an external scanner;
cosmetic (documented deviation in `tree-sitter-luab/README.md`).

### v1 → v2 migration tooling

**As a** v1 shapes user, **I want** `luabox fix` (or similar) to rewrite
`struct`/`trait`/`impl` files and `---@use`/`---@struct`/`---@impl` tags
to v2, **so that** migration is mechanical. Today: hard manifest error on
`[types] shapes`, unknown-tag warnings, no rewriter.

### Strictness for un-shaped LuaCATS code (SPEC §19)

**As a** strict-mode user, **I want** the sealed-leaning-warnings proposal
for plain LuaCATS classes decided and implemented, **so that** `strict`
has one coherent story across both front-ends. Open question in SPEC.md
§19 (v2 note recorded there).

### Housekeeping

- **Scope cache key**: `ShapeStore::package_scope` caches by `shape_paths`
  only; include dependencies in the key before any caller varies deps
  under one store (`store.rs`).
- **Zed grammar rev**: the pinned rev only resolves once `shapes-v2` is
  pushed to the GitLab remote (`editors/zed/extension.toml`).
- **`watch.rs`**: `check_watch_reruns_on_file_change` fails in some
  environments (pre-dates v2); make it robust or gate it.
- **LSP `.luab` completion gating**: completions in `.luab` are offered at
  any cursor position; gate to type positions when a reliable heuristic
  exists (noted judgment call, `crates/luabox-lsp/src/luab.rs`).

# SHAPES v2 — TypeScript-adjacent shape modules

Status: **accepted, not yet implemented** (2026-07-10)
Supersedes: [SHAPES.md](SHAPES.md) upon implementation.

## Motivation

The v1 DSL (`struct` / `trait` / `impl` / `type`, nominal conformance, `---@use`
/ `---@struct` / `---@impl` binding tags) requires developers to state facts the
analyzer can derive, in more than one place:

- `impl Shape for Circle;` in the `.luab` is a bare set-membership assertion,
  unverified at its own site; its verification is anchored to a *different*
  statement (`---@impl Shape for Circle`) in a different file. Omitting either
  half produces confusing, asymmetric failures.
- `---@use` imports names the analyzer could resolve itself.
- `---@struct` tags the *carrier table* with the *instance type* — a category
  error the design papered over.

v2 replaces nominal, declared conformance with **structural, positional
conformance**, collapses the DSL to a single keyword, and reduces the Lua-side
vocabulary to **zero new tags**: standard LuaCATS positions do all consumption.

## The `.luab` grammar

One item form. Optional `export` modifier, optional generics.

```
module       := item*
item         := doc? "export"? "type" NAME generics? "=" type_expr
generics     := "<" NAME ("," NAME)* ">"
type_expr    := union
union        := intersection ("|" intersection)*
intersection := postfix ("&" postfix)*
postfix      := primary "?"?                        -- `?` = `| nil`
primary      := qualified type_args?                -- geometry.Point, Pair<T>
              | object
              | fn_type
              | "(" type_expr ")"
qualified    := NAME ("." NAME)*
object       := "{" (member ("," member)* ","?)? "}"
member       := NAME "?"? ":" type_expr             -- field
              | NAME "(" params? ")" (":" type_expr)?  -- method
params       := param ("," param)*
param        := "self" | NAME "?"? ":" type_expr
```

Deleted from v1: `struct`, `trait`, `impl`, `use` items; supertrait clauses
(`trait Drawable: Shape` becomes intersection). Example, full v1 spec module
in v2 syntax:

```typescript
type Point = { x: number, y: number, label?: string }
type Circle = { radius: number }
type Pair<T> = { first: T, second: T }

export type Shape = {
    area(self): number,
    perimeter(self): number,
}

export type Drawable = Shape & {
    draw(self, surface: Surface),
}
```

`self` as a first parameter marks a method member and drives `:`-vs-`.`
receiver checking, exactly as v1 trait fns did. Method bodies remain illegal —
`.luab` stays analyser-only, never on the require path, never in build output.

## Namespacing: fully qualified, path-derived

- A module's namespace is derived from its path under `[types] shape-paths`:
  `shapes/geometry.luab` declares into `geometry.*`,
  `shapes/love/graphics.luab` into `love.graphics.*`. Authors never write
  their own prefix.
- **Every reference outside the declaring module is fully qualified**
  (`geometry.Point`, `love.graphics.Canvas`). No short forms, no ambiguity
  resolution. This matches the established luals definition-library
  convention, so luabox types and LuaCATS `---@class` types share one
  coherent naming scheme.
- Inside the declaring `.luab`, sibling references are short (`Shape`, not
  `geometry.Shape`) — the file *is* the namespace. Generic parameters are
  lexically local.

## Scope: ambient, no imports

`---@use` is removed. The package's type scope is built once per package:
every module under `shape-paths`, plus dependencies' exported surfaces
(below). Any standard annotation position may name any type in scope.

- **Collisions:** two modules declaring the same fully-qualified name is a
  package-level duplicate-declaration error at both `.luab` sites. No silent
  merging (luals's merge behavior is explicitly rejected).
- **Zero-cost invariant restated:** the package scope is built once and
  shared; a file that never names a shape type pays a hash-map miss, not
  scope construction. (v1's per-tag gate no longer applies since there are
  no tags to gate on.)

## Visibility and publication

Two levels, gating only the package boundary:

- `type X = ...` — package-internal. Ambient within the package under its
  FQ name; invisible to dependents.
- `export type X = ...` — additionally part of the published type surface.

The manifest declares a TS-style entrypoint, replacing `[types] shapes = [...]`:

```toml
[types]
strict = true
shape-paths = ["shapes"]
entry = "shapes/init.luab"    # like package.json "types": "./index.d.ts"
```

- A dependent's view of the package is **exactly the entrypoint's exports**,
  mounted under the **package name** as root namespace: `geometry.Point`
  addresses `export type Point` in geometry's entrypoint. Internal module
  paths are not addressable from outside.
- **Re-export is a plain alias** (structural typing makes alias and re-export
  indistinguishable): `export type A = sub.folder.A` flattens internal
  structure into the curated surface. The RHS may be anything resolvable in
  package scope, including a dependency's type
  (`export type Canvas = love.graphics.Canvas`) — facade packages are legal.
- Packages without an `entry` export nothing; `export` is then a no-op.

## Lua-side consumption: zero new tags

`---@use`, `---@struct`, `---@impl` are removed. Nothing replaces them.
Types are consumed through standard LuaCATS positions only: `---@type`,
`---@param`, `---@return`, `---@field`.

- **Conformance is positional.** A value is a `geometry.Shape` at exactly the
  places one is demanded. There is no assertion tag; a developer wanting a
  local check writes `---@type geometry.Shape` on a binding — the general
  mechanism covers the special case. v1's LB2003 impl-completeness diagnostic
  is subsumed by assignability errors, which must name the missing/mismatched
  members and point at both the value's origin and the `.luab` declaration.
- **Sealed checking survives as literal freshness:** a table literal flowing
  into an annotated position errors on fields the target type doesn't declare
  (v1 LB2007-family, re-anchored).
- **`self` typing:** inferred — `setmetatable({...}, Carrier)` flowing
  through a `---@return geometry.Circle` ties the carrier to the instance
  type, so `self` in `Carrier:method()` types as `geometry.Circle`. Explicit
  fallback: `---@param self geometry.Circle` (already-standard LuaCATS).

Idiomatic carrier after the pivot:

```lua
local Circle = {}
Circle.__index = Circle

function Circle:area()
    return math.pi * self.radius ^ 2
end

function Circle:perimeter()
    return 2 * math.pi * self.radius
end

---@param radius number
---@return geometry.Circle
function Circle.new(radius)
    return setmetatable({ radius = radius }, Circle)
end

return Circle
```

## What is deleted (not replaced)

- `.luab` items: `struct`, `trait`, `impl ...;`, `use`; supertrait clauses.
- Tags: `---@use`, `---@struct`, `---@impl`.
- Machinery: the conformance registry (`scope.impls`, `conforms()`,
  `lb_impl()`), per-file `---@use` resolution and its three-tier lookup
  (including the sibling-`.luab` tier), the supertrait coherence pass, the
  `[types] shapes` export list, `DepShapeExport` in its current form.
- Nominal instance branding. Structurally identical types are
  interchangeable; this is accepted (TS stance). Metatable identity remains
  available to *inference* but confers no nominal type identity.

## Migration plan (staged)

1. **Syntax** — `luabox-syntax/src/shape`: new lexer/parser/AST/formatter for
   the single-item grammar; delete struct/trait/impl/use kinds.
2. **Types** — `luabox-types`: path-derived FQ lowering; ambient package
   scope with duplicate diagnostics; delete conformance registry and
   supertrait pass; positional structural checks + literal freshness at
   annotation positions; `self` inference; remove `---@use`/`---@struct`/
   `---@impl` tag handling from the LuaCATS front-end.
3. **Manifest & CLI** — `[types] entry`, dependency export surfaces from
   entrypoints; kill `shapes = [...]`.
4. **Diagnostics** — registry rework: retire LB2003/LB2005-as-import-error;
   respec LB2005 as duplicate declaration; assignability messages per above.
5. **Docgen** — `trait.*.html` / `struct.*.html` pages become `type.*.html`;
   implementors listings derive from positional-use sites or are dropped.
6. **Grammar & editors** — tree-sitter-luab rewrite; re-pin in Zed/JetBrains/
   VS Code extensions.
7. **Examples & docs** — geometry + renderer examples, SHAPES.md retirement,
   README.

Each stage lands green (fmt, clippy, full test suite) before the next starts.

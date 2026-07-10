# luabox shape spec — `.lb` declarations (spec rev 3)

**One-line:** Rust struct/trait syntax in separate `.lb` files; plain Lua binds via `---@` tags; checked with Rust temperament. Analyser-only — no def emit, ever.

Companion to [SPEC.md](SPEC.md).

## 1. Scope & invariants

In: `.lb` grammar; binding tags `---@use`/`---@struct`/`---@impl`; sealed checking; `.lb` package distribution; LSP/fmt/lint over shape files.
Out: any runtime behaviour; `.d.lua`/`.d.luau` emit; macros/derive; consuming `.d.ts`/`.d.tl`; replacing LuaCATS.

**Invariants (violate = spec bug):**
1. Shapes never affect runtime output — check-time only; emitted Lua byte-identical with or without them.
2. Shapes lower into the unified type IR. One checker. No parallel type system.
3. `*.lb` never on require path, never in build/bundle output.
4. No opt-in = zero cost, zero diagnostics.

**Accepted trade-off:** shape types visible to luabox consumers only; stock LuaLS sees untyped API.

## 2. File format

- Extension `.lb` — invisible to Lua tooling by construction; no exclude config needed.
- UTF-8. Not Lua. Own grammar/parser in `luabox-syntax` (rowan, lossless).
- Comments `//`, `/* */`; doc comments `///` surface in hover and `luabox doc`.
- One file = one shape module; module name = file stem (`geometry.lb` → `geometry`).

## 3. Grammar

```ebnf
file        := item*
item        := struct | trait | impl | alias | use

struct      := doc* "struct" IDENT generics? "{" field* open? "}"
field       := doc* IDENT ":" type ","?
open        := ".."                          ; open shape — extra keys allowed

trait       := doc* "trait" IDENT generics? supertraits? "{" trait_item* "}"
supertraits := ":" IDENT ("+" IDENT)*
trait_item  := doc* "fn" IDENT "(" params? ")" ("->" ret)? ";"
param       := ("self") | (IDENT ":" type)
ret         := type ("," type)*              ; multi-return

impl        := "impl" IDENT generics? "for" IDENT ";"   ; conformance assertion only
alias       := "type" IDENT generics? "=" type ";"
use         := "use" path ";"

generics    := "<" IDENT (":" IDENT ("+" IDENT)*)? ("," ...)* ">"
type        := IDENT generics_args? | type "?" | type "|" type
             | "Vec" "<" type ">" | "HashMap" "<" type "," type ">"
             | "fn" "(" params? ")" ("->" ret)? | "(" type ")"
```

No bodies, no expressions. Parser rejects bodies: "implementations live in .lua — bind with ---@impl".

Example:

```rust
/// 2D geometry primitives.
struct Point { x: number, y: number, label: string? }

trait Shape {
    fn area(self) -> number;
    fn perimeter(self) -> number;
}

trait Drawable: Shape {
    fn draw(self, surface: Surface);
}

struct Circle { radius: number }
impl Shape for Circle;

struct Pair<T> { first: T, second: T }
```

### Type vocabulary → IR

`number`/`integer`/`string`/`boolean`/`unknown` = primitives; `T?` = nil-union; `A | B` = union; `Vec<T>` = `T[]`; `HashMap<K,V>` = `table<K,V>`; `fn(a: A) -> R` = `fun(a: A): R`; `Option<T>` = `T?` (sugar); `Result<T,E>` = multi-return `(T?, E?)`; `Self` = implementing struct; struct = sealed class; trait = interface class.

Banned: lifetimes/`&`, `mut`, `Box`/`Rc`, `where`, `dyn`. Rust flavour, Lua reality.

### Coexistence

Two type front-ends, one IR: LuaCATS annotations (`.lua`, fully supported, unchanged) and the shape DSL (`.lb` + tags). Interop total: `.lb` struct usable in `---@param`/`---@field`; `---@class` table can satisfy a `.lb` trait via `---@impl`; mixed projects are the norm. Luau: out of scope toolchain-wide.

## 4. Binding annotations (in `.lua`)

| Tag | Placement | Meaning |
|---|---|---|
| `---@use <module>` | file top | import shapes (resolution §6) |
| `---@struct <Struct>` | before local/assignment | bind value to struct — class carriers and plain data tables |
| `---@impl <Trait> for <Struct>` | before first method or carrier | conformance; completeness + signatures enforced |

```lua
---@use geometry

---@struct Circle
local Circle = {}
Circle.__index = Circle

---@impl Shape for Circle
function Circle:area()      return math.pi * self.radius ^ 2 end
function Circle:perimeter() return 2 * math.pi * self.radius end

function Circle.new(radius)
  return setmetatable({ radius = radius }, Circle)  -- literal checked vs struct fields
end

---@struct Point
local origin = { x = 0, y = 0 }
```

## 5. Checking semantics (hard errors unless noted)

- **Sealed:** missing non-optional field = error; unknown key read/write = error; `..` opens the struct (extras = `unknown`).
- **Coherence:** `---@impl T for S` — all trait fns present; params contravariant, returns covariant; `:` vs `.` receiver matches `self`; missing fn = error listing the gap; extra inherent methods fine.
- **Supertraits:** `impl Drawable for X` requires `Shape` conformance on the same carrier.
- **Instantiation:** `setmetatable(literal, Carrier)` → literal checked against struct, result typed as instance.
- **Narrowing:** shapes flow-narrow like classes.
- **Generics:** monomorphised per use site; bound violations at the call, rustc-style.
- **Strictness:** `---@struct` IS the opt-in — shape rules are hard errors at every strictness level.

### Diagnostics — `LB2xxx`

| Code | Condition |
|---|---|
| LB2001 | missing non-optional field on shape-bound literal |
| LB2002 | unknown key on sealed shape |
| LB2003 | `---@impl` incomplete — lists missing fns (fix-it: generate stubs) |
| LB2004 | impl signature mismatch — both spans shown |
| LB2005 | `---@use` unresolved module |
| LB2006 | `---@struct` names undeclared struct |
| LB2007 | generic bound unsatisfied |
| LB2008 | supertrait conformance missing |
| LB2010 | body in `.lb` |

All get `luabox explain` pages.

## 6. Resolution

`---@use <name>`, first hit wins: (1) sibling `<name>.lb`; (2) `[types] shape-paths` dirs in `luabox.toml`, in order; (3) dependency-exported shapes (`[types] shapes = [...]` in the dep's manifest). Same-tier ambiguity = error. `use` inside `.lb` resolves identically.

## 7. Analyser-only surface

- Shapes live exclusively in `luabox-db`; consumed by check, lint, LSP, doc. Nothing else sees them.
- No generated artifacts of any kind. The `.lb` file IS the declaration.
- Build/bundle shape-blind (invariant 1).
- Distribution ships `.lb` as opaque source inside package artifacts; consumers analyse directly.

## 8. Toolchain integration

check: same diagnostic stream. fmt: `.lb` formatted (4-space, trailing commas, one item/line, no options). lint: same DB. build/bundle: `.lb` always excluded. LSP: full in `.lb` (completion, hover with `///`, goto struct↔binding↔impl, cross-file rename, find-refs); in `.lua`: hover shows struct, stub-generation and bind-table code actions. doc: shapes as first-class type pages.

## 9. Architecture impact

Syntax: additive shape grammar module, own SyntaxKind space, Lua grammar untouched. Semantics: HIR type decls; sealed/interface/coherence in the IR; DB queries `shape_modules`, `resolve_use`, `impl_conformance`. Emit: untouched. Distribution: manifest fields + opaque `.lb` in artifacts, never parsed. Frontend: LSP only, no new CLI. Execution: untouched.

## 10. Testing (cucumber — write first)

`tests/features/shapes/`:

```gherkin
Feature: Sealed shape checking
  Scenario: missing field rejected
    Given a shape module "geometry" declaring struct Point { x: number, y: number }
    And a Lua file binding a table { x = 0 } with ---@struct Point
    When I run "luabox check"
    Then diagnostic LB2001 is reported naming field "y"

Feature: Trait coherence
  Scenario: incomplete impl rejected
    Given trait Shape with fns area and perimeter
    And a carrier table with ---@impl Shape for Circle defining only area
    When I run "luabox check"
    Then diagnostic LB2003 is reported listing "perimeter"

Feature: Ecosystem interop
  Scenario: LuaCATS class satisfies a shape trait
    Given trait Shape in "geometry.lb"
    And a ---@class annotated table with ---@impl Shape for Square
    When I run "luabox check"
    Then zero diagnostics are reported
```

Plus proptest fmt idempotence on `.lb`; `cargo-fuzz` on the shape parser.

## 11. Phasing

P0+: `.lb` grammar + parser + fmt (additive, no Lua-checking dependency). P1: IR lowering, sealed/coherence, binding tags, interop, LB2xxx, LSP basics. P2: manifest fields, dependency shapes, `.lb` in artifacts. P4+: rename polish, stub generation, doc rendering.

## 12. Open questions (escalate, don't guess)

1. **Resolved (P1, ticket #66):** `Result<T,E>` as `(T?, E?)` loses the exactly-one invariant — convention **accepted at P1** as proposed; revisit sum types post-P3. Implemented: in return position `Result<T,E>` lowers to the multi-return pair `(T?, E?)` (a conforming impl annotates `---@return T?, E?`); outside return position it degrades to `T | E | nil`.
2. Trait default method bodies — banned in v1; `default fn` + base-table convention later?
3. `impl Shape + Drawable for Circle;` sugar — decide at parser review.

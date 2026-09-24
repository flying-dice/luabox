# Typed Lua — Language Specification

Draft 1.

## 1. Overview

Typed Lua is Lua with type annotations. A Typed Lua program is a Lua program
plus types; the types are checked before compilation and then erased.
Compilation removes the types and emits the Lua that remains. Nothing is
added at runtime: no class system, no runtime type information, no helper
library.

### 1.1 Goals

1. **Lua stays Lua.** Every construct a program executes is plain Lua with
   plain Lua semantics. Removing the types from a correct program yields the
   program that runs.
2. **Types are written, not guessed.** A binding's type is either written
   down or taken, in one step, from something whose type is already known.
   Types never flow backwards from how a value is used.
3. **Small.** One way to name a type (`type`), structural typing, no
   nominal hierarchy.
4. **Readable output.** Compiled output keeps every line and column of the
   source, so errors at runtime point at the code that was written.

### 1.2 Non-goals

Typed Lua has no classes, interfaces, enums, access modifiers, decorators,
namespaces, abstract members, overloading declarations, or any construct
with runtime meaning of its own. Idioms Lua already has — tables,
metatables, closures, modules returned from a chunk — are what programs use.

## 2. Source files

| Extension   | Contents                                                        |
|-------------|-----------------------------------------------------------------|
| `.tlua`     | Typed Lua source. Compiles to a `.lua` file.                    |
| `.d.tlua`   | Declarations only (§8): types for code written in plain Lua.    |
| `.lua`      | Plain Lua. May be required from Typed Lua through declarations. |

A `.tlua` file is a chunk, exactly as a `.lua` file is: `require` finds it by
module name, and the chunk's `return` value is the module.

## 3. Lexical additions

No new reserved words. Two identifiers are *contextual keywords*, special
only at the start of a statement and followed by a name:

- `type` — a type alias statement (§4.3). `type(x)` remains the standard
  function call.
- `export` — only in `export type` (§7.4).

A third, `declare`, is contextual in `.d.tlua` files only (§8).

New tokens: `->` (function type arrow), `::` (type assertion), `?` (optional
suffix). `<` and `>` delimit generic parameter lists in the
positions defined in §4.

## 4. Syntax

The grammar below extends Lua's. Productions not shown are unchanged.
`[x]` is optional, `{x}` is zero or more.

### 4.1 Annotated bindings

```
local        ::= 'local' attnamelist ['=' explist]
attnamelist  ::= Name [':' Type] [attrib] {',' Name [':' Type] [attrib]}

funcbody     ::= [Generics] '(' [parlist] ')' [':' ReturnType] block 'end'
parlist      ::= param {',' param} [',' vararg] | vararg
param        ::= Name [':' Type]
vararg       ::= '...' [':' Type]

for-numeric  ::= 'for' Name [':' Type] '=' exp ',' exp [',' exp] 'do' block 'end'
for-generic  ::= 'for' Name [':' Type] {',' Name [':' Type]} 'in' explist 'do' block 'end'
```

Examples:

```lua
local count: integer = 0
local name, age: string, integer = "ada", 36

local function area(w: number, h: number): number
  return w * h
end

local function each<T>(list: {T}, f: (item: T) -> ()): ()
  for _, item in ipairs(list) do f(item) end
end
```

### 4.2 Type assertion

```
exp ::= ... | exp '::' Type
```

`value :: T` asserts that `value` has type `T` (§6.9). It binds tighter than
every binary operator.

### 4.3 Type alias

```
stat     ::= ... | ['export'] 'type' Name [Generics] '=' Type
Generics ::= '<' Name {',' Name} '>'
```

```lua
type Point = { x: number, y: number }
type Pair<A, B> = { first: A, second: B }
export type Id = string | integer
```

A `type` statement introduces a name into the type namespace of its scope.
Type names and value names live in separate namespaces.

### 4.4 Types

```
Type        ::= UnionType
UnionType   ::= PostfixType {'|' PostfixType}
PostfixType ::= PrimaryType {'?'}
PrimaryType ::= 'nil' | 'true' | 'false' | String | Number
              | Name [TypeArgs]
              | TableType
              | FunctionType
              | '(' Type ')'
TypeArgs    ::= '<' Type {',' Type} '>'

TableType   ::= '{' Type '}'                           -- array
              | '{' '[' Type ']' ':' Type '}'          -- map
              | '{' [Field {',' Field} [',']] '}'      -- record
Field       ::= Name ['?'] ':' Type
              | '[' Type ']' ':' Type

FunctionType ::= [Generics] '(' [FnParams] ')' '->' ReturnType
FnParams     ::= FnParam {',' FnParam} [',' '...' ':' Type] | '...' ':' Type
FnParam      ::= [Name ':'] Type
ReturnType   ::= Type | '(' [Type {',' Type} [',' '...' ':' Type]] ')'
```

`T?` is shorthand for `T | nil`. `()` as a return type means "returns
nothing"; `(A, B)` means two values.

## 5. Types

| Type                  | Values                                                  |
|-----------------------|---------------------------------------------------------|
| `nil`                 | `nil`                                                   |
| `boolean`             | `true`, `false`                                         |
| `number`              | every number                                            |
| `integer`             | numbers with an integer representation                  |
| `string`              | every string                                            |
| `thread`, `userdata`  | coroutines, userdata                                    |
| `"lit"`, `3`, `true`  | exactly that literal                                    |
| `{T}`                 | tables whose keys `1..n` hold `T`                       |
| `{[K]: V}`            | tables mapping `K` to `V`                               |
| `{ a: T, b?: U }`     | tables with field `a: T` and optional `b: U`            |
| `(A) -> R`            | functions                                               |
| `A \| B`              | values of either type                                   |
| `any`                 | any value; every use is permitted (the explicit escape) |
| `unknown`             | any value; no use is permitted until narrowed (§6.8)    |
| `never`               | no value                                                |

`integer` is a subtype of `number`. Literal types are subtypes of their
primitive. `nil` is a subtype of `T?` for every `T`.

### 5.1 Structural typing

A table type is compatible with another when it has every field the target
requires, each at a compatible type. Names given by `type` are aliases: two
aliases with the same structure are the same type. There is no declared
inheritance; a record with more fields is usable where a record with fewer
is expected, except where a table constructor is checked directly against a
record type (§6.4).

### 5.2 Function types

`(A1, A2) -> R` is compatible with `(B1, B2) -> S` when each `Bi` is
compatible with `Ai` (parameters are contravariant) and `R` with `S`
(returns are covariant). A function taking fewer parameters is compatible
with one taking more; the extra arguments are ignored, as in Lua.

### 5.3 Generics

Functions and type aliases may declare type parameters. At a call, type
arguments are bound from the argument types, left to right; a parameter no
argument binds is an error at the call. Type aliases take explicit
arguments: `Pair<string, number>`.

## 6. Typing rules

### 6.1 Every binding has a type

A binding's type comes from exactly one of:

1. its annotation;
2. its initializer, when the initializer's type is known (§6.2);
3. the expected type of its position, for parameters of a function literal
   passed where a function type is expected (§6.3).

A binding whose type none of these determine is an error: *cannot determine
the type of `x`; declare it*. There is no implicit `any`.

### 6.2 One-step propagation

An initializer's type is known when it is:

- a literal — widened to its primitive (`local n = 1` is `integer`,
  `local s = "a"` is `string`);
- a name whose type is known;
- a call to a function with a declared return type (the first return, or
  one per name for `local a, b = f()`);
- a field or index of a value whose type declares it;
- an operator applied to operands of known type (§6.7);
- a table constructor whose fields are all known (§6.4);
- a function literal whose signature is fully written or expected (§6.3).

Loop variables take the types the iterator's declared returns give them:
`ipairs(t)` over `{T}` yields `integer, T`; `pairs(t)` over `{[K]: V}`
yields `K, V`; any function-typed iterator yields its declared returns.

A binding's type is fixed at its declaration. A later assignment must be
compatible with it; it never widens it.

```lua
local count = 0
count = count + 1      -- ok
count = "three"        -- error: `string` is not `integer`
```

### 6.3 Functions

Every function declares its signature: each parameter has a type, and a
function that returns values declares its return types. A function that
returns nothing may omit `: ()`.

A function literal in a position with an expected function type — an
argument to a parameter of function type, the initializer of an annotated
binding, a field of an annotated table, a returned value of a function with
a declared function return — takes its parameter and return types from that
expected type and need not repeat them.

```lua
table.sort(list, function(a, b) return a < b end)   -- a, b from `list`'s element type
```

Recursion needs no special rule: the signature is declared before the body.

### 6.4 Tables

A table constructor's type is the record (or array, or map) of its entries,
with literal values widened.

**Open tables.** A table constructor bound to a local is *open* for the
rest of the block that declares it, until it *escapes* — is returned,
passed as an argument, assigned to another variable or field, or captured
by a function that runs before the block ends. While open, assigning a new
field adds it to the table's type, with the value's (known) type:

```lua
local M = {}
M.version = "1.0"
function M.area(w: number, h: number): number return w * h end
return M       -- escapes here, as { version: string, area: (number, number) -> number }
```

**Sealed tables.** Once a table escapes, or when its type is annotated, its
type is fixed. Assigning a field it does not have is an error; assigning a
field it has must be compatible with the field's type.

**Checked constructors.** A constructor checked directly against a record
type — an annotated binding, an argument, a returned value — must provide
every required field and no field the record does not declare.

### 6.5 Methods and `self`

`function t:m(...)` is `function t.m(self, ...)`. The type of `self` is the
type of `t`. To give instances a different type from the table that holds
their methods, write the parameter explicitly: `function Point.len(self:
Point): number`.

A call `x:m(args)` is `x.m(x, args)` and is checked as such.

### 6.6 Metatables

`setmetatable(t, mt)` returns `t`. When `mt` has a field `__index` whose type
is a table type `P`, the result's type is `t`'s type extended with every
field of `P` that `t` does not already have. No other metamethod changes a
type. This is the whole of the rule; prototype-based objects follow from it:

```lua
export type Point = { x: number, y: number, len: (self: Point) -> number }

local Proto = {}
Proto.__index = Proto
function Proto.len(self: Point): number
  return math.sqrt(self.x ^ 2 + self.y ^ 2)
end

local function new(x: number, y: number): Point
  return setmetatable({ x = x, y = y }, Proto)
end
```

### 6.7 Operators

Arithmetic on `integer` operands is `integer`, except `/` and `^`, which are
`number`; on any `number` operand, `number`. `..` on `string`/`number`
operands is `string`. Comparisons and `not` are `boolean`. `#` on a string
or table is `integer`. `a and b` is `b`'s type unioned with the falsy part of
`a`'s; `a or b` is the truthy part of `a`'s unioned with `b`'s. An operator
applied to operands it is not defined for is an error.

### 6.8 Narrowing

Inside a branch, a local's type is refined by the condition that guards it:

| Condition               | Refinement in the `then` branch              |
|-------------------------|----------------------------------------------|
| `x`                     | `x` without `nil` and `false`                 |
| `not x`                 | `x` restricted to `nil` / `false`             |
| `x ~= nil`, `x == nil`  | without / only `nil`                          |
| `type(x) == "string"`   | the members of `x` of that primitive          |
| `x == "lit"`            | the literal                                   |

`else` branches get the complement. A branch that ends in `return`, `error`
or `break` refines the code after it. Narrowing never changes a binding's
declared type, only its type at a use.

### 6.9 Assertions

`e :: T` is valid when `e`'s type and `T` are compatible in either direction,
or when `e` is `any`/`unknown`. It is checked, not trusted: an assertion
between unrelated types is an error. `any` is the only unchecked escape.

### 6.10 Modules

`require("name")` has the type of the named module's returned value. For a
`.tlua` module, that is the type its chunk returns. For a `.lua` module, it
is the type its declaration file gives (§8); without one, the result is
`unknown`, and binding it requires an annotation:

```lua
local json: { encode: (value: any) -> string } = require("json")
```

A `require` whose argument is not a string literal is `unknown` in the same
way.

## 7. Scope of names

### 7.1 Values

Unchanged from Lua.

### 7.2 Types

A `type` statement's name is visible from the statement to the end of the
enclosing block, and inside its own definition (so types may be
recursive). Type parameters are visible inside their declaration.

### 7.3 Globals

A global's type is declared by a declaration file (§8) or by its first
assignment in a chunk, which must have a known type. Reading a global with
no declared type is an error.

### 7.4 Exports

`export type` makes a type visible to modules that `require` this one, as a
member of the module's type namespace:

```lua
local geometry = require("geometry")
local p: geometry.Point = { x = 0, y = 0 }
```

## 8. Declaration files

A `.d.tlua` file declares the types of code written in plain Lua. It
contains only `type`, `export type` and `declare` statements:

```
stat ::= 'declare' Name ':' Type                    -- a global value
       | 'declare' 'module' String ':' Type         -- a module's returned value
```

```lua
declare module "json": {
  encode: (value: any) -> string,
  decode: (text: string) -> any,
}
declare love: { graphics: { rectangle: (mode: string, x: number, y: number, w: number, h: number) -> () } }
```

Declaration files are never compiled and produce no output.

## 9. Diagnostics

A program with any type error does not compile. There is no mode that
relaxes checking and no comment that suppresses a type error. The errors
are:

| Kind                  | Example                                             |
|-----------------------|-----------------------------------------------------|
| undeclared type       | a binding, parameter or return with no type (§6.1)  |
| mismatch              | a value not compatible with its target              |
| arity                 | too few or too many arguments                       |
| missing field         | a checked constructor lacks a required field        |
| unknown field         | reading, or writing to a sealed table, a field the type lacks |
| undefined operator    | an operator on operands it is not defined for       |
| unknown type name     | a type that names nothing in scope                  |
| invalid assertion     | `::` between unrelated types                        |

Advisory findings that do not concern types (style, unused locals) are
warnings and never stop compilation.

## 10. Compilation

### 10.1 Erasure

Compiling a `.tlua` file replaces every type-only construct with spaces:

- `: Type` after a binding, parameter or `)`;
- `<...>` generic parameter lists;
- `:: Type` assertions;
- entire `type` / `export type` statements.

Newlines inside an erased region are kept. The output therefore has exactly
the lines of the source, and every remaining token sits at the same line and
column. The output is valid Lua with the source's meaning.

### 10.2 Target versions

A project names the Lua version it targets. When source written for a newer
Lua version uses a construct the target lacks, the compiler may rewrite that
construct into an equivalent the target has. Rewrites preserve lines where
they can; where they cannot, the source map (§10.4) records the nearest
source line.

### 10.3 Bundling

A bundle packs a module graph into one Lua file. Each module's compiled
text is placed, unchanged, inside a function registered under the module's
name; `require` inside the bundle resolves registered names first. Only
`require` calls with a string-literal argument are bundled; any other
`require` is a compile error when bundling, unless the name is declared
external.

### 10.4 Source maps

Because erasure preserves positions, an unbundled output file needs no map:
line *n* of the output is line *n* of the source.

A bundle is accompanied by a map file, `<bundle>.map`, recording for each
line of the bundle the source file and line it came from:

```json
{
  "version": 1,
  "bundle": "app.lua",
  "files": ["src/main.tlua", "src/util.tlua"],
  "lines": [null, [0, 1], [0, 2], [1, 1]]
}
```

- `files` — source files, relative to the project root, forward slashes.
- `lines[i]` — bundle line `i + 1`: `[file index, source line]` (1-based),
  or `null` for lines the bundler generated.

Columns within a mapped line are the source's columns, except where a
rewrite (§10.2) or minification changed the line. A tool can rewrite a
runtime traceback against the map to point at the `.tlua` sources.

## 11. Open questions

1. **Open tables (§6.4).** The alternative is to seal every table at its
   constructor and require a type annotation for tables built up by
   assignment. Open-until-escape keeps the module idiom
   (`local M = {} ... return M`) annotation-free at the cost of one rule.
2. **Declaration output.** Whether compiling a `.tlua` module also emits a
   `.d.tlua` for its exports, so typed consumers of the compiled Lua keep
   its types.
3. **Extension name** — `.tlua` is a placeholder.

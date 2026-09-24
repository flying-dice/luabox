# Typed Lua — Language Specification

Draft 8. Status: proposed.

This document is normative. The guides in this section teach the language;
where they and this document differ, this document governs.

## 1. Overview

Typed Lua is Lua with type declarations. A Typed Lua program is a Lua
program plus types: the types are checked, then erased, and the Lua that
remains is the program that runs. Nothing is added at runtime — no class
system, no runtime type information, no helper library.

```lua
#include "geometry.luah"

local function area(r: Rect): number
  return r.w * r.h
end

local rects: Rect[] = { { w = 2, h = 3 }, { w = 4, h = 5 } }
local total: number = 0
for i: integer, r: Rect in ipairs(rects) do
  total = total + area(r)
end
```

### 1.1 Goals

1. **Lua stays Lua.** Every construct a program executes is plain Lua with
   plain Lua semantics. Removing the types from a correct program yields the
   program that runs, and every Lua program parses the same way here.
2. **Types are written, not guessed.** A binding's type is either written
   down or taken, in one step, from something whose type is already known.
   Types never flow backwards from how a value is used.
3. **Interfaces are declared.** What a module offers is written in its
   header. Code that uses a module sees the header, not the implementation.
4. **Small and explicit.** One way to name a type (`typedef`), structural
   typing, one preprocessor directive (`#include`), tables whose shape is
   fixed when they are built.
5. **Readable output.** Compiled output keeps every line and column of the
   source, so errors at runtime point at the code that was written.

### 1.2 Non-goals

Typed Lua has no classes, interfaces, enums, access modifiers, decorators,
namespaces, abstract members, generics, overloading, operator overloading,
or any construct with runtime meaning of its own. A feature is included
only when Lua or C already has it. It has no macros and no
conditional compilation. Idioms Lua already has — tables, metatables,
closures, modules returned from a chunk — are what programs use.

## 2. Files and projects

### 2.1 File kinds

| Extension | Contents                                                          |
|-----------|-------------------------------------------------------------------|
| `.luac`   | Typed Lua source. Always strictly checked; compiles to `.lua`.    |
| `.luah`   | A header (§8): the interface of a module, and global declarations. |
| `.lua`    | Plain Lua. Usable from Typed Lua through a header.                |

A `.luac` file is a chunk, exactly as a `.lua` file is: `require` finds it by
module name, and the chunk's `return` value is the module.

### 2.2 Projects

A project names:

- a **source root**, the directory module names are relative to;
- zero or more **header directories**, searched for headers (§8.2);
- a **target**, the Lua version the output must run on (§11.2).

Compiling `<source root>/net/http.luac` produces `net/http.lua` in the output
directory, which plain Lua loads with `require("net.http")`.

### 2.3 Module names

A module's name is its path relative to the directory it was found under —
the source root for sources, the header directory for headers — without the
extension, with `/` read as `.`. `net/http.luac` and `net/http.luah` both
name the module `net.http`.

## 3. Lexical structure

Typed Lua uses Lua's lexical rules, with these additions.

- `typedef` is a reserved word in `.luac` and `.luah` files.
- `extern` is a reserved word in `.luah` files.
- `->` is a token (the function type arrow). In Lua, `-` followed by `>`
  never occurs, so the token takes nothing from Lua.
- A line whose first non-blank characters are `#include` is an include
  directive (§8.2). `#` cannot begin a Lua statement, so the directive never
  conflicts with Lua code. A first line beginning `#!` is skipped, as in Lua.

`void`, `any`, `unknown`, `never` and the primitive type names (§5) are not
reserved: they are names in the type namespace, and remain usable as value
names.

## 4. Syntax

The grammar extends Lua's. Productions not shown are unchanged. `[x]` is
optional, `{x}` is zero or more, `|` separates alternatives.

### 4.1 Types

```
Type         ::= PostfixType {'|' PostfixType}
PostfixType  ::= PrimaryType {Suffix}
Suffix       ::= '?'                            -- optional
               | '[' ']'                        -- array
               | '[' Type ']'                   -- map
PrimaryType  ::= 'nil' | 'true' | 'false' | String | Numeral
               | Name
               | RecordType
               | FunctionType
               | '(' Type ')'

RecordType   ::= '{' [Field {',' Field} [',']] '}'
Field        ::= Name ':' Type                  -- named field
               | '[' String ']' ':' Type        -- field whose key is not a name
               | '[' Type ']' ':' Type          -- index part (§6.5)

FunctionType ::= '(' [FnParams] ')' '->' ReturnType
FnParams     ::= FnParam {',' FnParam} [',' '...' ':' Type]
               | '...' ':' Type
FnParam      ::= [Name ':'] Type
ReturnType   ::= Type
               | '(' Type ',' Type {',' Type} [',' '...' ':' Type] ')'
```

| Written                         | Meaning                                          |
|---------------------------------|--------------------------------------------------|
| `number?`                       | `number` or `nil`                                |
| `string[]`                      | array of strings                                 |
| `number[string]`                | table mapping strings to numbers                 |
| `string?[]`                     | array whose elements may be `nil`                |
| `string[]?`                     | an array of strings, or `nil`                    |
| `(s: string) -> number`         | function taking a string, returning a number     |
| `(...: any) -> void`            | function taking any arguments, returning nothing |
| `() -> (integer, string)`       | function returning two values                    |
| `((s: string) -> number)?`      | a function, or `nil`                             |
| `{ name: string, [string]: any }` | a record with an index part                    |

A parenthesized list of two or more types is a return list, valid only as a
`ReturnType`. Parameter names in a function type document it; they do not
affect the type. In `(A) -> R?`, the `?` belongs to `R`; write
`((A) -> R)?` for an optional function.

### 4.2 Declarations

A type follows the name it describes, after `:`.

```
local        ::= 'local' decl {',' decl} ['=' explist]
decl         ::= Name [attrib] [':' Type]

for-numeric  ::= 'for' Name [':' Type] '=' exp ',' exp [',' exp]
                 'do' block 'end'
for-generic  ::= 'for' Name [':' Type] {',' Name [':' Type]} 'in' explist
                 'do' block 'end'
```

```lua
local count: integer = 0
local name: string, age: integer = "ada", 36
local limit <const>: number = 1.5
local label: string?
```

### 4.3 Functions

Parameter types follow the parameters; the return type follows the
parameter list.

```
function-stat  ::= 'function' funcname funcbody
local-function ::= 'local' 'function' Name funcbody
function-lit   ::= 'function' funcbody
funcbody       ::= '(' [parlist] ')' [':' ReturnType] block 'end'
parlist        ::= param {',' param} [',' vararg] | vararg
param          ::= Name [':' Type]
vararg         ::= '...' [':' Type]
```

```lua
local function area(w: number, h: number): number
  return w * h
end

local function parse(s: string): (number?, string?)
  local n: number? = tonumber(s)
  if n then return n, nil end
  return nil, "not a number: " .. s
end

local function each(list: string[], f: (item: string) -> void)
  for _, item in ipairs(list) do f(item) end
end

local function sum(...: number): number
  local total: number = 0
  for _, n in ipairs({ ... }) do total = total + n end
  return total
end
```

### 4.4 Casts

```
exp ::= ... | '<' Type '>' exp
```

`<T> e` asserts that `e` has type `T` (§6.10). A cast is a unary operator
with the precedence of `not`, `#` and unary `-`.

### 4.5 Type definitions

```
stat ::= ... | 'typedef' Name '=' Type
```

```lua
typedef Point = { x: number, y: number }
typedef DrawMode = "fill" | "line"
typedef Logger = (message: string) -> void
typedef Node = { value: number, next: Node? }
```

### 4.6 Reading Typed Lua beside Lua

Every Lua program parses as Typed Lua exactly as it parses as Lua, except
that `typedef` is reserved. The additions sit where Lua allows nothing:

1. `:` after a declared name, a parameter or a loop variable, and after a
   function's parameter list, always begins a type.
2. A type ends at the first token that cannot continue it. No Lua statement
   begins with `?`, `[`, `|`, `<` or `->`, so a type never absorbs the
   statement after it.
3. In a type, `(` begins a function type when its matching `)` is followed
   by `->`; otherwise it groups a type or, in a return type, lists several.
4. `<` begins a cast only where an expression begins; after an expression
   it is the less-than operator. Lua reads `<<` as one token, so a cast
   directly after `<` needs a space: `a < <integer> b`.

## 5. Types

| Type                  | Values                                                   |
|-----------------------|----------------------------------------------------------|
| `nil`                 | `nil`                                                    |
| `boolean`             | `true`, `false`                                          |
| `number`              | every number                                             |
| `integer`             | numbers with an integral value (§5.5)                    |
| `string`              | every string                                             |
| `thread`              | coroutines                                               |
| `userdata`            | userdata                                                 |
| `"lit"`, `3`, `true`  | exactly that literal                                     |
| `T[]`                 | tables whose keys `1..n` hold `T`                        |
| `V[K]`                | tables mapping keys of type `K` to values of type `V`    |
| `{ a: T, b: U? }`     | tables with field `a` of type `T` and optional `b`       |
| `(A) -> R`            | functions                                                |
| `A \| B`              | values of either type                                    |
| `any`                 | any value; every use is permitted (the explicit escape)  |
| `unknown`             | any value; no use is permitted until narrowed or cast    |
| `void`                | nothing: a function that returns no values               |
| `never`               | no value: an expression that does not complete           |

`void` is valid only as a return type.

### 5.1 Compatibility

A value of type `S` may be used where `T` is expected when `S` is
*compatible* with `T`:

- every type is compatible with itself, with `any` and with `unknown`;
- `any` is compatible with every type;
- `never` is compatible with every type;
- `integer` is compatible with `number`;
- a literal type is compatible with its primitive;
- `S` is compatible with `A | B` when it is compatible with `A` or `B`;
  `A | B` is compatible with `T` when both `A` and `B` are;
- `T?` is `T | nil`;
- table and function types by §5.2 and §5.3.

`typedef` names are aliases: two names for the same structure are the same
type.

### 5.2 Table types

A record is compatible with another record when it has every field the
target declares, each at a compatible type; a field whose target type admits
`nil` may be absent. A record with more fields is compatible with one with
fewer, except where a table constructor is checked directly (§6.5).

`S[]` is compatible with `T[]`, and `V[K]` with `W[L]`, only when the element
types are the same type — a table that can be written through is not
covariant — or when the target is `any[]` or `any[any]` (§5.4).

### 5.3 Function types

`(A1, A2) -> R` is compatible with `(B1, B2) -> S` when each `Bi` is
compatible with `Ai` (parameters are contravariant) and `R` is compatible
with `S` (returns are covariant). A function taking fewer parameters is
compatible with one taking more; the extra arguments are ignored, as in Lua.
A function returning values is compatible with one returning `void`.

### 5.4 Code for values of any type

There are no type parameters. Code that works on values of any type takes
`any` or `unknown`, and its caller casts the result (§6.10):

```lua
local function largest(items: any[], measure: (item: any) -> number): any
  -- ...
end

local big = <Shape?> largest(scene, area)
```

Every array is compatible with `any[]`, and every map with `any[any]`, so
such functions accept any array or map.

### 5.5 Integers

On targets with an integer subtype (5.3 and later), `integer` is exactly the
integer values. On targets without one (5.1, 5.2, LuaJIT), `integer` is the
numbers with an integral value. The typing rules are the same on every
target.

## 6. Typing rules

### 6.1 Every binding has a type

A binding's type comes from exactly one of:

1. its declared type;
2. its initializer, when the initializer's type is known (§6.2);
3. the expected type of its position, for the parameters and returns of a
   function literal written where a function type is expected (§6.4).

A binding whose type none of these determine is an error: *cannot determine
the type of `x`; declare it*. There is no implicit `any`.

### 6.2 One-step propagation

An initializer's type is known when it is:

- a literal — `nil` is `nil`, `true`/`false` are `boolean`, a numeral
  without a fraction or exponent is `integer`, other numerals are `number`,
  a string is `string`;
- a name whose type is known;
- a call to a function with a declared return type — its first return, or
  one per name for `local a, b = f()`;
- a field or index of a value whose type declares it;
- an operator applied to operands of known type (§6.8);
- a cast (§6.10);
- a table constructor whose entries are all known (§6.5);
- a function literal whose signature is fully written.

A literal keeps its literal type where a type is expected — an argument, a
declared binding, a field of a checked constructor, a returned value — so
`rect("fill", …)` passes `"fill"` to a `DrawMode` parameter. It is widened to
its primitive only when it sets the type of an undeclared binding.

Loop variables take the types the iterator's returns give them: `ipairs(t)`
over `T[]` yields `integer, T`; `pairs(t)` over `V[K]` yields `K, V`, and
over a record yields `string, unknown`; any other function-typed iterator
yields its declared returns. A numeric `for` variable is `integer` when its
start and step are `integer` (a missing step is `1`), otherwise `number`.

A binding's type is fixed at its declaration. A later assignment must be
compatible with it; it never widens it.

```lua
local count = 0
count = count + 1      -- ok
count = "three"        -- error: `string` is not compatible with `integer`
```

### 6.3 Declarations without a value

A declared local with no initializer holds `nil`, so its type must admit
`nil`: `local label: string?` is valid; `local label: string` is an error.

The one exception is a **forward declaration**: a local of function type
declared without a value, for mutually recursive functions. The same block
must then define it exactly once, with a `function` statement of the same
name and a compatible signature, before the block ends. It may be called
only inside function bodies before that definition.

```lua
local is_odd: (n: integer) -> boolean

local function is_even(n: integer): boolean
  if n == 0 then return true end
  return is_odd(n - 1)
end

function is_odd(n: integer): boolean
  if n == 0 then return false end
  return is_even(n - 1)
end
```

### 6.4 Functions and calls

**Signatures.** Every function declares its signature: each parameter has a
type, and a function that returns values declares its return types. A
function with no return type written returns nothing (`void`). A function
literal written where a function type is expected — an argument, the
initializer of a declared binding, a field of a checked constructor, a
returned value — takes its parameter and return types from that type and
need not write them. An expected type `F?` gives a literal the types of `F`.

```lua
local on_key: (key: string) -> void = function(key)   -- key is string
  print("pressed " .. key)
end
```

Recursion needs no special rule: a function's signature is known before its
body is checked.

**Returns.** A function that declares return values must return on every
path; reaching its end is an error, unless the last statement is a call
returning `never` (such as `error(...)`). `return` may give fewer values than
declared when the missing ones admit `nil`; more values than declared is an
error. A `void` function may only `return` with no values.

**Arguments.** A call supplies one argument per parameter. Trailing
arguments may be left out when their parameter types admit `nil`. Extra
arguments are an error, except into a vararg parameter.

**Multiple results.** As in Lua, a call in the last position of an argument
list, a table constructor, a `return` or an assignment supplies all its
results; anywhere else it supplies its first. A `void` call used as a value
is an error. When an assignment has more names than values, the extra names
receive `nil` and their types must admit it.

**Varargs.** Inside a function whose vararg is `...: T`, `...` supplies values
of type `T`: `{ ... }` is `T[]` and `select(i, ...)` is `T`. A vararg written
without a type is an error unless the function takes its type from context.

### 6.5 Tables

**Constructor types.** A table constructor's type is built from its entries:
named entries become record fields, positional entries an array part, and
`[k] = v` entries an index part. Literal values are widened, unless the
constructor is checked against an expected type.

**Fixed shape.** A table's type is set by its constructor or its declared
type, and never grows. Assigning a field the type does not declare is an
error. Assigning a field it declares must be compatible with the field's
type; assigning `nil` requires the field's type to admit `nil`. A table is
built whole, in one constructor:

```lua
local function area(w: number, h: number): number return w * h end

return { version = "1.0", area = area }
```

**Checked constructors.** A constructor written where a record type is
expected — a declared binding, an argument, a field, a returned value — must
provide every field the record requires and no field it does not declare.
Nested constructors are checked against their fields' types. A constructor
checked against an array `T[]` may hold only positional entries of type `T`;
one checked against a map `V[K]` may hold any entries whose keys are
compatible with `K` and values with `V` — `left = -1` has the key `"left"`.
An empty constructor is a valid array or map: `local names: string[] = {}`.
When the expected type is a union of records, the constructor is checked
against the members its literal fields select (§6.9); it must match exactly
one.

**Indexing.** `t[i]` on `T[]` is `T` and requires an `integer` key. `t[k]` on
`V[K]` is `V?`: a key may be absent. A record's index part `[K]: V` types
every key of type `K` the record does not name, as `V?`:
`{ name: string, [string]: any }`. Indexing a record with a key it does not
declare, and that no index part covers, is an error.

**Length.** `#t` is `integer` for arrays, maps and records with an index
part.

### 6.6 Methods and `self`

`function t:m(...)` is `function t.m(self, ...)`: an assignment to the field
`m`, which `t`'s type must declare. The type of `self` is the type of `t`.
To give instances a different type from the table that holds their methods,
write the parameter explicitly: `function Point.len(self: Point): number`.

A call `x:m(args)` is `x.m(x, args)` and is checked as such.

### 6.7 Metatables

`setmetatable(t, mt)` returns `t`. When `mt`'s type has a field `__index`
whose type is a record `P`, the result's type is `t`'s type extended with
every field of `P` that `t` does not already have. No other metamethod
changes a type. This is the whole of the rule; prototype-based objects
follow from it:

```lua
-- vec.luah
typedef Vec = { x: number, y: number, len: (self: Vec) -> number }
new(x: number, y: number): Vec
```

```lua
-- vec.luac
#include <math.luah>
#include "vec.luah"

local function len(self: Vec): number
  return math.sqrt(self.x ^ 2 + self.y ^ 2)
end

local Meta = { __index = { len = len } }

local function new(x: number, y: number): Vec
  return setmetatable({ x = x, y = y }, Meta)
end

return { new = new }
```

### 6.8 Operators

| Operator                    | Operands                   | Result                   |
|-----------------------------|----------------------------|--------------------------|
| `+ - * %` and unary `-`     | `integer`                  | `integer`                |
| `+ - * %` and unary `-`     | `number`                   | `number`                 |
| `/ ^`                       | `number`                   | `number`                 |
| `//`                        | `integer` / `number`       | `integer` / `number`     |
| `& \| ~ << >>` and unary `~`| `integer`                  | `integer`                |
| `..`                        | `string` or `number`       | `string`                 |
| `< <= > >=`                 | both `number` or both `string` | `boolean`            |
| `== ~=`                     | any                        | `boolean`                |
| `not`                       | any                        | `boolean`                |
| `#`                         | `string` or table with an array part | `integer`      |
| `a and b`                   | any                        | `b`'s type, or the falsy part of `a`'s |
| `a or b`                    | any                        | the truthy part of `a`'s type, or `b`'s |

An operator applied to operands it is not defined for is an error.
Operators are defined on these types only: a table whose metatable provides
`__add` or another operator metamethod is still not an operand of `+`. Call
the operation as a function instead (`vec.add(a, b)`).

### 6.9 Narrowing

Inside a branch, the type of a local or parameter is refined by the
condition that guards it:

| Condition                   | Refinement where the condition is true        |
|-----------------------------|-----------------------------------------------|
| `x`                         | `x` without `nil` and `false`                 |
| `not x`                     | `x` restricted to `nil` and `false`           |
| `x == nil`, `x ~= nil`      | only / without `nil`                          |
| `type(x) == "string"` (etc.)| the members of `x` of that primitive          |
| `x == "lit"`                | the literal                                   |
| `x.f == "lit"`              | the members of `x` whose field `f` admits it  |
| `a and b`, `a or b`, `not a`| combined as in Lua                            |

The `else` branch gets the complement. After `if … then return end` — or
`error`, `break`, `goto` — the code that follows gets the complement too.
`assert(x)` refines `x` in the statements after it, as `x` does.

Narrowing applies to locals and parameters, not to fields: copy a field to a
local to narrow it. An assignment to a narrowed local resets it to the
assigned value's type. Narrowing never changes a binding's declared type,
only its type at a use, and does not carry into function literals.

```lua
#include <math.luah>

typedef Circle = { kind: "circle", r: number }
typedef Rect = { kind: "rect", w: number, h: number }
typedef Shape = Circle | Rect

local function area(s: Shape): number
  if s.kind == "circle" then
    return math.pi * s.r ^ 2      -- s is Circle
  end
  return s.w * s.h                -- s is Rect
end
```

### 6.10 Casts

`<T> e` is valid when `e`'s type is compatible with `T` or `T` with `e`'s
type, or when `e` is `any` or `unknown`. It is checked, not trusted: a cast
between unrelated types is an error. `any` is the only unchecked escape.

### 6.11 Modules

`require("name")` has the type the module's header gives it (§8.3). The
header must be included in the requiring file; a `require` of a module whose
header is not included is an error.

A `require` whose argument is not a string literal is `unknown`, and a cast
gives it a type:

```lua
local json = <{ encode: (value: any) -> string }> require(backend)
```

### 6.12 Loading code at runtime

`dofile`, `loadfile`, `load` and `loadstring` run code chosen by a path or a
string at runtime, which no header can describe. Their results are
`unknown` (§9.1), and a cast gives them a type, usually one declared in a
header:

```lua
#include "config.luah"          -- typedef Config = { host: string, port: integer }

local config = <Config> dofile("config.lua")
print(config.host)
```

A header is never looked up from a path passed to these functions.

## 7. Scope of names

### 7.1 Values

As in Lua, except for globals (§7.3).

### 7.2 Types

A `typedef` is visible throughout the block that contains it, including
before it and inside its own definition, so types may be recursive and
mutually recursive. Two `typedef`s of one name in one block are an error; an inner
block may shadow an outer name.

Types from an included header are visible from the `#include` line to the
end of the file.

### 7.3 Globals

A global is declared with `extern` in a header (§8.5). Reading or assigning
a global that no included header declares is an error. Assigning a declared
global — in a `.luac` file that defines it — must be compatible with its
declared type.

## 8. Headers

A `.luah` header declares an interface: the type of a module, the types that
go with it, and globals. Headers contain declarations only; they are never
compiled and produce no output.

### 8.1 Header syntax

```
header ::= {hstat}
hstat  ::= include
         | 'typedef' Name '=' Type
         | Name '(' [FnParams] ')' [':' ReturnType]             -- function
         | Name ':' Type                                        -- field
         | 'extern' Name '(' [FnParams] ')' [':' ReturnType]
         | 'extern' Name ':' Type
         | 'return' Type
```

A function prototype declares a function by signature alone — no
`function`, no body, no `end`. Every parameter must have a type; a missing
return type means `void`. `--` comments are allowed anywhere.

Each name has one prototype, because a Lua function is one function. A
function that Lua code calls in several forms — `table.insert(t, v)` and
`table.insert(t, pos, v)` — inspects its arguments at runtime; its prototype
covers every form with optional parameters and unions.

```lua
-- socket/core.luah
typedef Socket = {
  send: (self: Socket, data: string) -> (integer?, string?),
  receive: (self: Socket, pattern: string | integer) -> (string?, string?),
  close: (self: Socket) -> void,
}

connect(host: string, port: integer): (Socket?, string?)
gettime(): number
version: string
```

### 8.2 Includes

```
include ::= '#include' ( String | '<' Path '>' )
```

`#include "path.luah"` looks for the header relative to the including file,
then in the header directories in order. `#include <path.luah>` looks only in
the header directories; the standard headers (§9) are found there. An
include that finds nothing is an error.

Include directives appear at the top level of a file, outside any block.
Including a header makes its `typedef`s and `extern`s visible to the rest of
the file, along with those of the headers it includes. Its functions and
fields are not values in the including file: they describe the module's
value, reached through `require` (§8.3).

Each header is read once per file, however many times it is included, so
headers need no guards and may include each other. Two headers that declare
one name at different types are an error; declaring it again at the same
type is not.

### 8.3 Module headers

A header declares the module that shares its module name (§2.3):
`socket/core.luah` declares `require("socket.core")`, whether the module is
`socket/core.luac`, `socket/core.lua`, or a native library. The module's
type is the record of the header's functions and fields:

```lua
-- main.luac
#include <socket/core.luah>

local socket = require("socket.core")
local conn, err = socket.connect("example.com", 80)
if conn then
  conn:send("GET / HTTP/1.0\r\n\r\n")
  conn:close()
end
```

A module whose value is not a table declares it with `return`:

```lua
-- inspect.luah
return (value: any) -> string
```

A header has functions and fields or a `return`, not both. A header with
only `typedef`, `extern` and `#include` declares no module.

### 8.4 Implementing a header

A `.luac` module and the header with the same module name are a pair. The
module's returned value is checked against the header's module type: it
must provide every function and field the header declares, at a compatible
type. Every module that another module requires must have a header.

The header is the whole interface. A user of the module sees only what the
header declares; anything else the module's value carries is private to it.
A module brings in its own header's types the same way as any other file:

```lua
-- geometry.luah
typedef Rect = { w: number, h: number }
area(r: Rect): number
```

```lua
-- geometry.luac
#include "geometry.luah"

local function area(r: Rect): number
  return r.w * r.h
end

return { area = area }
```

### 8.5 Globals

`extern` declares a global, for values the host application, the runtime or
another chunk provides:

```lua
-- love.luah
typedef DrawMode = "fill" | "line"

typedef LoveGraphics = {
  rectangle: (mode: DrawMode, x: number, y: number, w: number, h: number) -> void,
}

typedef Love = {
  graphics: LoveGraphics,
  load: (() -> void)?,
  update: ((dt: number) -> void)?,
  draw: (() -> void)?,
}

extern love: Love
```

A program defines `love.update` by assigning the declared field:
`function love.update(dt) … end` takes `dt`'s type from the field.

### 8.6 Trust

A header for a plain Lua or native module is trusted: nothing checks the
module against it. A header may describe a native module's userdata as a
record of the fields its metatable provides; such a value's `type()` is
still `"userdata"`, so narrowing it with `type()` is an error. A header for
a `.luac` module is checked (§8.4).

## 9. Standard library

### 9.1 Base functions

The target's base functions are declared in every file without an include.
Their types:

| Function          | Type                                                                |
|-------------------|---------------------------------------------------------------------|
| `print`           | `(...: any) -> void`                                                |
| `type`            | `(v: any) -> TypeName`, the union of the eight type names           |
| `tostring`        | `(v: any) -> string`                                                |
| `tonumber`        | `(v: any, base: integer?) -> number?`                               |
| `error`           | `(message: any, level: integer?) -> never`                          |
| `assert`          | returns `v` without `nil` and `false`, and narrows `v` (§6.9)       |
| `pcall`           | `(f: (...: any) -> any, ...: any) -> (boolean, ...: any)`           |
| `xpcall`          | `(f: (...: any) -> any, handler: (err: any) -> any, ...: any) -> (boolean, ...: any)` |
| `select`          | `select("#", ...)` is `integer`; `select(i, ...)` per §6.4          |
| `ipairs`, `pairs` | loop iterators, typed per §6.2                                      |
| `next`            | `(t: any[any], key: any) -> (any, any)`                             |
| `setmetatable`    | per §6.7                                                            |
| `getmetatable`    | `(v: any) -> unknown`                                               |
| `rawget`, `rawset`, `rawequal`, `rawlen` | as their Lua counterparts, over `any`        |
| `require`         | per §6.11                                                           |
| `unpack` (5.1)    | `(list: any[], i: integer?, j: integer?) -> (...: any)`             |
| `dofile`          | `(path: string?) -> (...: unknown)`, per §6.12                      |
| `loadfile`        | `(path: string?, ...: any) -> (Chunk?, string?)`, per §6.12         |
| `load`, `loadstring` | `(chunk: string \| (() -> string?), ...: any) -> (Chunk?, string?)` |

`Chunk` is `(...: any) -> (...: unknown)`: a loaded chunk, whose results are
`unknown` until cast. `load` accepts only the forms the target's Lua
accepts, and `loadstring` exists only on 5.1 and LuaJIT. `collectgarbage`
is declared with `any` parameters and result.

### 9.2 Library headers

Every other library is used only after including its header:

| Header              | Declares                                     |
|---------------------|----------------------------------------------|
| `<string.luah>`     | `string`, and the methods of string values    |
| `<table.luah>`      | `table`                                       |
| `<math.luah>`       | `math`                                        |
| `<io.luah>`         | `io`, and the `File` type                     |
| `<os.luah>`         | `os`                                          |
| `<coroutine.luah>`  | `coroutine`                                   |
| `<utf8.luah>`       | `utf8` (5.3 and later)                        |
| `<debug.luah>`      | `debug`                                       |

String values have the functions of the `string` library as methods —
`s:upper()` — once `<string.luah>` is included. A target's own libraries
(LuaJIT's `bit`, `ffi` and `jit`, 5.2's `bit32`) come as headers of the same
names. Including a header the target does not have is an error.

## 10. Diagnostics

A program with any error does not compile. There is no mode that relaxes
checking and no comment that suppresses an error.

| Code   | Error                  | Example                                             |
|--------|------------------------|-----------------------------------------------------|
| TL0100 | undeclared type        | a binding, parameter or return with no type (§6.1)  |
| TL0101 | mismatch               | a value not compatible with its target              |
| TL0102 | arity                  | too few or too many arguments or return values      |
| TL0103 | missing field          | a checked constructor lacks a required field        |
| TL0104 | unknown field          | reading or writing a field the type does not declare |
| TL0105 | undefined operator     | an operator on operands it is not defined for       |
| TL0106 | unknown type name      | a type that names nothing in scope                  |
| TL0107 | invalid cast           | a cast between unrelated types                      |
| TL0108 | missing return         | a function's end is reachable but it declares returns |
| TL0109 | uninitialized          | a declared local with no value at a non-`nil` type  |
| TL0110 | undefined forward      | a forward declaration its block never defines       |
| TL0200 | include not found      | an `#include` that names no header                  |
| TL0201 | missing include        | a `require` whose module header is not included     |
| TL0202 | conflicting declaration | two headers declare one name at different types   |
| TL0203 | header mismatch        | a `.luac` module's value does not match its header  |
| TL0204 | undeclared global      | a global no included header declares                |
| TL0205 | missing header         | a required `.luac` module has no header             |

Findings that do not concern types — unused locals, unreachable code, style
— are warnings and never stop compilation.

## 11. Compilation

### 11.1 Erasure

Compiling a `.luac` file replaces every type-only construct with spaces:

- `: Type` after a declared name, parameter, vararg or loop variable;
- `: ReturnType` after a function's parameter list;
- `<T>` casts;
- entire `typedef` statements;
- `#include` lines.

```lua
local function area(w: number, h: number): number   -- source
local function area(w        , h        )           -- output
```

Newlines inside an erased region are kept. The output therefore has exactly
the lines of the source, and every remaining token sits at the same line and
column. The output is valid Lua with the source's meaning. Headers produce
no output, and plain `.lua` files in the source root are copied to the
output unchanged.

### 11.2 Target versions

A project names the Lua version it targets: 5.1, 5.2, 5.3, 5.4 or LuaJIT.
Source may use the syntax of the target or of a newer version. When it uses a
construct the target lacks, the compiler rewrites it into an equivalent the
target has. Rewrites preserve lines where they can; where they cannot, the
source map (§11.4) records the nearest source line.

### 11.3 Bundling

A bundle packs a module graph into one Lua file. Each module's compiled text
is placed, unchanged, inside a function registered under the module's name;
`require` inside the bundle resolves registered names first. Only `require`
calls with a string-literal argument are bundled. Modules with a header but
no source in the project — plain Lua or native modules outside the source
root — are left to the runtime's own `require`. Headers are not bundled.

### 11.4 Source maps

Because erasure preserves positions, an unbundled output file needs no map:
line *n* of the output is line *n* of the source.

A bundle is accompanied by a map file, `<bundle>.map`, recording for each
line of the bundle the source file and line it came from:

```json
{
  "version": 1,
  "bundle": "app.lua",
  "files": ["src/main.luac", "src/util.luac"],
  "lines": [null, [0, 1], [0, 2], [1, 1]]
}
```

- `files` — source files, relative to the project root, forward slashes.
- `lines[i]` — bundle line `i + 1`: `[file index, source line]` (1-based),
  or `null` for lines the bundler generated.

Columns within a mapped line are the source's columns, except where a
rewrite (§11.2) or minification changed the line. A tool can rewrite a
runtime traceback against the map to point at the `.luac` sources.

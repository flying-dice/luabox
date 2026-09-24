# Typed Lua — Language Specification

Draft 4.

## 1. Overview

Typed Lua is Lua with type declarations. A Typed Lua program is a Lua
program plus types; the types are checked before compilation and then
erased. Compilation removes the types and emits the Lua that remains.
Nothing is added at runtime: no class system, no runtime type information,
no helper library.

Types are written before the names they describe, and a module's interface
lives in a header that its users include.

```lua
#include "geometry.luah"

local function number area(Rect r)
  return r.w * r.h
end

local Rect[] rects = { { w = 2, h = 3 }, { w = 4, h = 5 } }
local number total = 0
for integer i, Rect r in ipairs(rects) do
  total = total + area(r)
end
```

### 1.1 Goals

1. **Lua stays Lua.** Every construct a program executes is plain Lua with
   plain Lua semantics. Removing the types from a correct program yields the
   program that runs.
2. **Types are written, not guessed.** A binding's type is either written
   down or taken, in one step, from something whose type is already known.
   Types never flow backwards from how a value is used.
3. **Interfaces are declared.** What a module offers is written in its
   header. Code that uses a module sees the header, not the implementation.
4. **Small.** One way to name a type (`typedef`), structural typing, no
   nominal hierarchy, one preprocessor directive (`#include`).
5. **Readable output.** Compiled output keeps every line and column of the
   source, so errors at runtime point at the code that was written.

### 1.2 Non-goals

Typed Lua has no classes, interfaces, enums, access modifiers, decorators,
namespaces, abstract members, overloading declarations, or any construct
with runtime meaning of its own. It has no macros and no conditional
compilation. Idioms Lua already has — tables, metatables, closures, modules
returned from a chunk — are what programs use.

## 2. Source files

| Extension | Contents                                                          |
|-----------|-------------------------------------------------------------------|
| `.luac`   | Typed Lua source. Always strictly checked; compiles to `.lua`.    |
| `.luah`   | A header (§8): the interface of a module, and global declarations. |
| `.lua`    | Plain Lua. Usable from Typed Lua through a header.                |

A `.luac` file is a chunk, exactly as a `.lua` file is: `require` finds it by
module name, and the chunk's `return` value is the module. Compiling
`src/geometry.luac` produces `geometry.lua`, which plain Lua loads with
`require("geometry")`.

A header describes a module whatever it is written in: a `.luac` module, a
plain Lua module, or a native module loaded from a shared library (`.dll`,
`.so`, `.dylib`).

## 3. Lexical additions

`typedef` is a reserved word in `.luac` and `.luah` files. `extern` is a
reserved word in `.luah` files. No other words are reserved; `void` and the
primitive type names (§5) are ordinary names in the type namespace.

A line whose first non-blank character is `#` followed by `include` is an
include directive (§8.2). `#` cannot begin a Lua statement, so the directive
never conflicts with Lua code.

New tokens in type positions: `?` (optional suffix), `[` `]` (array and map
suffixes), `<` `>` (generic parameter and argument lists, and casts).

## 4. Syntax

The grammar below extends Lua's. Productions not shown are unchanged.
`[x]` is optional, `{x}` is zero or more.

### 4.1 Declarations

A type is written before the name it describes.

```
local        ::= 'local' attnamelist ['=' explist]
attnamelist  ::= decl [attrib] {',' decl [attrib]}
decl         ::= [Type] Name

parlist      ::= param {',' param} [',' vararg] | vararg
param        ::= [Type] Name
vararg       ::= [Type] '...'

for-numeric  ::= 'for' decl '=' exp ',' exp [',' exp] 'do' block 'end'
for-generic  ::= 'for' decl {',' decl} 'in' explist 'do' block 'end'
```

```lua
local integer count = 0
local string name, integer age = "ada", 36
local number x <const> = 1.5
```

`local a b` declares `b` with type `a`, also when a line break separates
them. A Lua program that relies on reading this as two statements separates
them with `;`.

### 4.2 Functions

A function's return type is written after `function` and before its name;
type parameters follow the name.

```
function-stat  ::= 'function' [ReturnType] funcname [Generics] funcbody
local-function ::= 'local' 'function' [ReturnType] Name [Generics] funcbody
function-lit   ::= 'function' [ReturnType] [Generics] funcbody
funcbody       ::= '(' [parlist] ')' block 'end'
Generics       ::= '<' Name {',' Name} '>'
ReturnType     ::= Type | '(' Type {',' Type} [',' Type '...'] ')'
```

```lua
local function number area(number w, number h)
  return w * h
end

local function (number?, string?) parse(string s)
  local number? n = tonumber(s)
  if n then return n, nil end
  return nil, "not a number: " .. s
end

local function void each<T>(T[] list, void(T item) f)
  for _, item in ipairs(list) do f(item) end
end
```

A function with no return type written returns nothing (`void`), unless it
is a literal whose type comes from context (§6.3). A type parameter is
visible throughout its function's signature, including the return type
written before it.

In a function literal, a parenthesized list followed by a second
parenthesized list is a return type: `function (integer, string) (string s)`.

### 4.3 Casts

```
exp ::= ... | '<' Type '>' exp
```

`<T> e` asserts that `e` has type `T` (§6.9). A cast is a unary operator,
with the precedence of `not`, `#` and unary `-`. `<` begins a cast only where
an expression begins; elsewhere it is the comparison operator.

### 4.4 Type definitions

```
stat ::= ... | 'typedef' Type Name [Generics]
```

```lua
typedef { number x, number y } Point
typedef { A first, B second } Pair<A, B>
typedef string | integer Id
typedef void(string message) Logger
```

A `typedef` introduces a name into the type namespace of its scope. Type
names and value names live in separate namespaces.

### 4.5 Types

```
Type        ::= UnionType
UnionType   ::= PostfixType {'|' PostfixType}
PostfixType ::= PrimaryType {Suffix}
Suffix      ::= '?'                             -- optional
              | '[' ']'                         -- array
              | '[' Type ']'                    -- map
              | '(' [FnParams] ')'              -- function
PrimaryType ::= 'nil' | 'true' | 'false' | String | Number
              | Name [TypeArgs]
              | RecordType
              | '(' Type ')'
              | '(' Type ',' Type {',' Type} [',' Type '...'] ')'
TypeArgs    ::= '<' Type {',' Type} '>'

RecordType  ::= '{' [Field {',' Field} [',']] '}'
Field       ::= Type Name                       -- named field
              | Type                            -- map type: the other keys

FnParams    ::= FnParam {',' FnParam} [',' Type '...'] | Type '...'
FnParam     ::= Type [Name]
```

Suffixes apply left to right:

| Written              | Meaning                                         |
|----------------------|-------------------------------------------------|
| `number?`            | `number` or `nil`                               |
| `string[]`           | array of strings                                |
| `number[string]`     | table mapping strings to numbers                |
| `string?[]`          | array whose elements may be `nil`               |
| `string[]?`          | an array of strings, or `nil`                   |
| `number(string s)`   | function taking a string, returning a number    |
| `void(any ...)`      | function taking any arguments, returning nothing |
| `(integer, string)()`| function returning two values                   |

A parenthesized list of two or more types is a return list, and is valid
only where a function's return type is expected.

A record field whose type is `T?` may be absent. A field written as a map
type alone, with no name, types every key the record does not name:
`{ string name, any[string] }`.

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
| `T[]`                 | tables whose keys `1..n` hold `T`                       |
| `V[K]`                | tables mapping `K` to `V`                               |
| `{ T a, U? b }`       | tables with field `a` of type `T` and optional `b`      |
| `R(A)`                | functions                                               |
| `A \| B`              | values of either type                                   |
| `any`                 | any value; every use is permitted (the explicit escape) |
| `unknown`             | any value; no use is permitted until narrowed (§6.8)    |
| `void`                | nothing: a function that returns no values              |
| `never`               | no value                                                |

`integer` is a subtype of `number`. Literal types are subtypes of their
primitive. `nil` is a subtype of `T?` for every `T`. `void` is valid only as
a return type.

### 5.1 Structural typing

A table type is compatible with another when it has every field the target
requires, each at a compatible type. Names given by `typedef` are aliases:
two names for the same structure are the same type. There is no declared
inheritance; a record with more fields is usable where a record with fewer
is expected, except where a table constructor is checked directly against a
record type (§6.4).

### 5.2 Function types

`R(A1, A2)` is compatible with `S(B1, B2)` when each `Bi` is compatible with
`Ai` (parameters are contravariant) and `R` with `S` (returns are
covariant). A function taking fewer parameters is compatible with one taking
more; the extra arguments are ignored, as in Lua. Parameter names in a
function type document it and do not affect compatibility.

### 5.3 Generics

Functions and `typedef`s may declare type parameters. At a call, type
arguments are bound from the argument types, left to right; a parameter no
argument binds is an error at the call. A generic `typedef` takes explicit
arguments: `Pair<string, number>`.

## 6. Typing rules

### 6.1 Every binding has a type

A binding's type comes from exactly one of:

1. its declared type;
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
`ipairs(t)` over `T[]` yields `integer, T`; `pairs(t)` over `V[K]` yields
`K, V`; any function-typed iterator yields its declared returns.

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
returns nothing may write `void` or leave the return type out.

A function literal in a position with an expected function type — an
argument to a parameter of function type, the initializer of a declared
binding, a field of a declared table, a returned value of a function with a
declared function return — takes its parameter and return types from that
expected type and need not repeat them.

```lua
#include <table.luah>

table.sort(list, function(a, b) return a < b end)   -- a, b from `list`
```

Recursion needs no special rule: the signature is declared before the body.

### 6.4 Tables

A table constructor's type is the record (or array, or map) of its entries,
with literal values widened.

**Tables are fixed at construction.** A table's type is settled by its
constructor or its declared type, and never grows. Assigning a field the
type does not have is an error; assigning a field it has must be compatible
with the field's type. A table is built whole, in one constructor:

```lua
local function number area(number w, number h) return w * h end

return { version = "1.0", area = area }
```

**Checked constructors.** A constructor checked directly against a record
type — a declared binding, an argument, a returned value — must provide
every required field and no field the record does not declare. An empty
constructor is a valid array or map: `local string[] names = {}`.

### 6.5 Methods and `self`

`function t:m(...)` is `function t.m(self, ...)`, an assignment to the field
`m`, which `t`'s type must declare (§6.4). The type of `self` is the type of
`t`. To give instances a different type from the table that holds
their methods, write the parameter explicitly:
`function number Point.len(Point self)`.

A call `x:m(args)` is `x.m(x, args)` and is checked as such.

### 6.6 Metatables

`setmetatable(t, mt)` returns `t`. When `mt` has a field `__index` whose type
is a table type `P`, the result's type is `t`'s type extended with every
field of `P` that `t` does not already have. No other metamethod changes a
type. This is the whole of the rule; prototype-based objects follow from it:

```lua
-- point.luah
typedef { number x, number y, number(Point self) len } Point
Point new(number x, number y)
```

```lua
-- point.luac
#include <math.luah>
#include "point.luah"

local function number len(Point self)
  return math.sqrt(self.x ^ 2 + self.y ^ 2)
end

local Meta = { __index = { len = len } }

local function Point new(number x, number y)
  return setmetatable({ x = x, y = y }, Meta)
end

return { new = new }
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

### 6.9 Casts

`<T> e` is valid when `e`'s type and `T` are compatible in either direction,
or when `e` is `any` or `unknown`. It is checked, not trusted: a cast between
unrelated types is an error. `any` is the only unchecked escape.

### 6.10 Modules

`require("name")` has the type the module's header gives it (§8.3). The
header must be included in the requiring file; a `require` of a module
whose header is not included is an error.

A `require` whose argument is not a string literal is `unknown`, and a cast
gives it a type:

```lua
local json = <{ string(any value) encode }> require(backend)
```

## 7. Scope of names

### 7.1 Values

Unchanged from Lua, except for globals (§7.3).

### 7.2 Types

A `typedef` in a `.luac` file is visible from the statement to the end of
the enclosing block, and inside its own definition (so types may be
recursive). Type parameters are visible throughout their declaration.

Types from an included header are visible from the `#include` line to the
end of the file.

### 7.3 Globals

A global is declared with `extern` in a header (§8.5). Reading or assigning
a global that no included header declares is an error.

The standard library of the target Lua version comes as standard headers,
one per library, found in the header directories: `<string.luah>`,
`<table.luah>`, `<math.luah>`, `<io.luah>`, `<os.luah>`,
`<coroutine.luah>`, `<utf8.luah>`, `<debug.luah>`. A file uses a library
only after including its header. The base functions (`print`, `type`,
`pairs`, `ipairs`, `tostring`, `tonumber`, `error`, `assert`, `pcall`,
`select`, `setmetatable`, `getmetatable`, `rawget`, `rawset`, `require`,
and the rest of the target version's base library) need no include.

## 8. Headers

A `.luah` header declares an interface: the type of a module, the types
that go with it, and globals. Headers contain declarations only; they are
never compiled and produce no output.

### 8.1 Header syntax

```
header ::= {hstat}
hstat  ::= include
         | 'typedef' Type Name [Generics]
         | ReturnType Name [Generics] '(' [parlist] ')'  -- function prototype
         | Type Name                                     -- field
         | 'extern' ReturnType Name [Generics] '(' [parlist] ')'
         | 'extern' Type Name
         | 'return' Type
```

A prototype declares a function by signature alone — no `function`, no
body, no `end`. Every parameter must have a type.

```lua
-- socket/core.luah
typedef {
  (integer?, string?)(Socket self, string data) send,
  (string?, string?)(Socket self, string | integer pattern) receive,
  void(Socket self) close,
} Socket

(Socket?, string?) connect(string host, integer port)
number gettime()
string version
```

### 8.2 Includes

```
include ::= '#' 'include' ( String | '<' Path '>' )
```

`#include "path.luah"` looks for the header relative to the including file,
then in the project's header directories. `#include <path.luah>` looks only
in the header directories. The directive makes the header's `typedef`s,
module declarations and globals visible to the rest of the file.

A header may include other headers. Each header is read once per file,
however many times it is included. Two included headers that declare the
same name at different types are an error; declaring it again at the same
type is not.

### 8.3 Module headers

A header declares the module whose name is its path, relative to the
directory it was found under, with `/` read as `.`: `socket/core.luah`
declares `require("socket.core")`. The module's type is the record of the
header's prototypes and fields:

```lua
-- main.luac
#include <socket/core.luah>

local socket = require("socket.core")
local conn, err = socket.connect("example.com", 80)
if conn then
  conn:send("GET / HTTP/1.0\r\n\r\n")
end
```

A module whose value is not a table declares it with `return`:

```lua
-- inspect.luah
return string(any value)
```

A header has prototypes and fields or a `return`, not both.

### 8.4 Implementing a header

A `.luac` module and the header with the same module name are a pair. The
module's returned value is checked against the header's module type: it
must provide every prototype and field at a compatible type. A module that
is required by other modules must have a header.

The header is the whole interface. A user of the module sees only what the
header declares; anything else the module's value carries is private to it.
Types the module needs from its own header are brought in the same way as
anywhere else:

```lua
-- geometry.luah
typedef { number w, number h } Rect
number area(Rect r)
```

```lua
-- geometry.luac
#include "geometry.luah"

local function number area(Rect r)
  return r.w * r.h
end

return { area = area }
```

### 8.5 Globals

`extern` declares a global, for values the host application, the runtime or
another chunk provides:

```lua
-- love.luah
typedef {
  void("fill" | "line" mode, number x, number y, number w, number h) rectangle,
} LoveGraphics

extern { LoveGraphics graphics } love
extern void log(any ...)
```

A header with only `typedef` and `extern` declarations declares no module.

### 8.6 Trust

A header for a plain Lua or native module is trusted: nothing checks the
module against it. A header for a `.luac` module is checked (§8.4).

## 9. Diagnostics

A program with any type error does not compile. There is no mode that
relaxes checking and no comment that suppresses a type error. The errors
are:

| Kind                 | Example                                              |
|----------------------|------------------------------------------------------|
| undeclared type      | a binding, parameter or return with no type (§6.1)   |
| mismatch             | a value not compatible with its target               |
| arity                | too few or too many arguments                        |
| missing field        | a checked constructor lacks a required field         |
| unknown field        | reading or writing a field the type lacks            |
| undefined operator   | an operator on operands it is not defined for        |
| unknown type name    | a type that names nothing in scope                   |
| invalid cast         | a cast between unrelated types                       |
| missing include      | a `require` whose module header is not included      |
| include not found    | an `#include` that names no header                   |
| conflicting declaration | two headers declare one name at different types   |
| header mismatch      | a `.luac` module's value does not match its header   |
| undeclared global    | a global no included header declares                 |

Advisory findings that do not concern types (style, unused locals) are
warnings and never stop compilation.

## 10. Compilation

### 10.1 Erasure

Compiling a `.luac` file replaces every type-only construct with spaces:

- the type before a declared name or parameter;
- the return type after `function`;
- `<...>` type parameter lists;
- `<T>` casts;
- entire `typedef` statements;
- `#include` lines.

```lua
local function number area(number w, number h)   -- source
local function        area(       w,        h)   -- output
```

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
external. Headers are not bundled.

### 10.4 Source maps

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
rewrite (§10.2) or minification changed the line. A tool can rewrite a
runtime traceback against the map to point at the `.luac` sources.

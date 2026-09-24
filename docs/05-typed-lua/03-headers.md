# Headers

A header (`.luah`) is an interface: what a module offers, the types that go
with it, and the globals a program can use. Headers hold declarations only
and never produce output. Section numbers (§) refer to the
[specification](05-specification.md).

## What goes in a header

```lua
-- geometry.luah
#include "vec.luah"                     -- other headers this one needs

typedef Rect = { w: number, h: number } -- types users of the module need

area(r: Rect): number                   -- functions the module provides
unit(): Rect
version: string                         -- other fields of the module

extern SCALE: number                    -- a global (rare in a module header)
```

A function is written as a prototype: name, typed parameters, return type,
no body. A prototype with no return type returns nothing. Everything a
header declares that is not a `typedef` or `extern` is a field of the
module's value — `require("geometry").area`, `.unit` and `.version` above.

There is one prototype per name, because a Lua function is one function.
A function called in several forms checks its arguments at runtime, so its
prototype covers every form; a position that takes different types is `any`:

```lua
-- table.luah: insert(t, v) and insert(t, pos, v)
insert(list: any, pos_or_value: any, value: any)
```

A module whose value is not a table says what it is with `return`:

```lua
-- inspect.luah
return (value: any) -> string
```

## Headers for your own modules

Each module that another module requires has a header next to it with the
same name:

```
src/
  geometry.luah     the interface
  geometry.luac     the implementation
  main.luac         a user
```

The user includes the header and requires the module:

```lua
-- main.luac
#include "geometry.luah"

local geometry = require("geometry")
print(geometry.area({ w = 2, h = 3 }))
```

The compiler checks the implementation against the header: the module must
return a value with every function and field the header declares, at a
compatible type (§8.4). Anything else it returns is private — users can't
see it, because they only see the header.

The implementation includes its own header to use the types in it:

```lua
-- geometry.luac
#include "geometry.luah"

local function area(r: Rect): number return r.w * r.h end
local function unit(): Rect return { w = 1, h = 1 } end

return { area = area, unit = unit, version = "1.0" }
```

Including a header gives you its types and globals. It does not put the
module's functions in scope as names; you reach them through `require`, as
at runtime.

## Headers for plain Lua

An existing `.lua` module needs no changes. Write a header beside it that
describes what it does, and Typed Lua code can use it:

```lua
-- csv.luah, next to csv.lua
typedef Row = string[]

split(line: string, sep: string): Row
parse(text: string): Row[]
```

A header for plain Lua is trusted: the compiler can't check `csv.lua`
against it, so it has to be right. The compiler copies `csv.lua` into the
output unchanged.

## Headers for native modules

A native module is a shared library (`.dll`, `.so`) that `require` loads.
It has no Lua source, so a header is the only way to type it. Name the
header after the module: `require("socket.core")` is declared by
`socket/core.luah`.

```lua
-- headers/socket/core.luah
typedef Socket = {
  send: (self: Socket, data: string) -> (integer, string),
  close: (self: Socket) -> void,
}

connect(host: string, port: integer): (Socket, string)
```

A native module's objects are usually userdata with a metatable. Describe
them as a record of the methods the metatable provides, as `Socket` does.

## Globals

`extern` declares a global. Use it for what a host application or runtime
provides:

```lua
-- love.luah
typedef LoveGraphics = {
  rectangle: (mode: string, x: number, y: number, w: number, h: number) -> void,
}

typedef Love = {
  graphics: LoveGraphics,
  update: (dt: number) -> void,
  draw: () -> void,
}

extern love: Love
```

Reading or writing a global that no included header declares is an error,
so a typo in a global name is caught. A program defines a callback by
assigning a declared field, with the field's types:

```lua
#include <love.luah>

function love.update(dt: number)
end
```

## Loading files at runtime

`dofile`, `loadfile` and `load` take a path or a string at runtime, so no
header can be found for them. Their results are `any`, like C's `void *`.
Declare the shape in a header and convert to it where the file is loaded:

```lua
-- config.luah
typedef Config = { host: string, port: integer }
```

```lua
#include "config.luah"

local config: Config = dofile("config.lua")   -- typed from here on
print(config.host)
```

That declaration is the one place you vouch for the file's contents;
everything after it is checked.

## Finding headers

- `#include "file.luah"` looks next to the including file first, then in
  the project's header directories.
- `#include <file.luah>` looks only in the header directories. The standard
  library headers are found this way: `<string.luah>`, `<table.luah>`,
  `<math.luah>`, `<io.luah>`, `<os.luah>`, `<coroutine.luah>`,
  `<utf8.luah>`, `<debug.luah>`.

A header is read once per file however often it is included, so headers
need no include guards and may include each other. Two headers declaring the
same name at different types is an error.

## The standard library

The base functions — `print`, `type`, `tostring`, `tonumber`, `pairs`,
`ipairs`, `select`, `error`, `assert`, `pcall`, `setmetatable`, `require`
and the rest — are always available. Every library (`string`, `table`,
`math`, …) needs its header first. Including `<string.luah>` also gives
string values their methods, so `s:upper()` works.

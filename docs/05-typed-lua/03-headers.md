# Headers

A header (`.luah`) is an interface: what a module offers, the types that go
with it, and the globals a program can use. Headers hold declarations only
and never produce output. Section numbers (§) refer to the
[specification](05-specification.md).

## What goes in a header

```lua
-- geometry.luah
#include "vec.luah"                     -- other headers this one needs

typedef { number w, number h } Rect     -- types users of the module need

number area(Rect r)                     -- functions the module provides
Rect unit()
string version                          -- other fields of the module

extern number SCALE                     -- a global (rare in a module header)
```

A function is written as a prototype: return type, name, typed parameters,
no body. Everything a header declares that is not a `typedef` or `extern` is
a field of the module's value — `require("geometry").area`, `.unit` and
`.version` above.

A module whose value is not a table says what it is with `return`:

```lua
-- inspect.luah
return string(any value)
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

local function number area(Rect r) return r.w * r.h end
local function Rect unit() return { w = 1, h = 1 } end

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
typedef string[] Row

Row split(string line, string sep)
Row[] parse(string text)
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
typedef {
  (integer?, string?)(Socket self, string data) send,
  void(Socket self) close,
} Socket

(Socket?, string?) connect(string host, integer port)
```

A native module's objects are usually userdata with a metatable. Describe
them as a record of the methods the metatable provides, as `Socket` does.

## Globals

`extern` declares a global. Use it for what a host application or runtime
provides:

```lua
-- love.luah
typedef {
  void("fill" | "line" mode, number x, number y, number w, number h) rectangle,
} LoveGraphics

typedef {
  LoveGraphics graphics,
  (void(number dt))? update,
  (void())? draw,
} Love

extern Love love
```

Reading or writing a global that no included header declares is an error,
so a typo in a global name is caught. A program defines a callback by
assigning a declared field; the function takes its types from the field:

```lua
#include <love.luah>

function love.update(dt)     -- dt is number
end
```

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

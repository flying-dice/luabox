# Typed Lua examples

Worked programs for the [Typed Lua proposal](../../docs/05-typed-lua/01-overview.md).
Each example has its source in `src/` and, in `dist/`, the exact Lua the
compiler must produce from it. The compiler does not exist yet, so `dist/`
is written by hand. `check.py` holds it to the spec:

```sh
python3 check.py            # uses $LUA, else `lua` on PATH
```

- every `.luac` has an output with the same lines, in which each token keeps
  its line and column (§11.1 of the spec);
- every output loads in a stock Lua interpreter;
- the runnable examples print exactly `expected-output.txt`.

All five target Lua 5.1, the oldest runtime, so each one also runs on
anything newer. When the compiler lands, these become its golden tests.

| Example                                           | Shows                                                         |
|---------------------------------------------------|---------------------------------------------------------------|
| [01-hello](01-hello/)                             | typed locals, a typed function, a typed loop, a library include |
| [02-geometry](02-geometry/)                       | modules with headers, metatable objects, a tagged record, a forward declaration |
| [03-plain-lua-module](03-plain-lua-module/)       | using an existing `.lua` module through a header you write     |
| [04-native-module](04-native-module/)             | a header for a native library, multiple returns, an `any` parameter |
| [05-love-game](05-love-game/)                     | `extern` globals from a host (LÖVE), callbacks assigned to declared fields, maps |

## 01-hello

The whole language in eleven lines. `string.rep` needs
`#include <string.luah>`; `print` and `ipairs` are base functions and need
nothing. `ipairs` returns `any`, so the loop declares `i` and `name`.
Compare `src/main.luac` with `dist/main.lua`: the types become spaces and
nothing else changes.

## 02-geometry

Three modules, two with headers:

- `vec.luah` / `vec.luac` — a vector object built the plain Lua way, with a
  metatable. The header's `Vec` type lists the fields and the methods; the
  table passed to `setmetatable` needs only the fields `__index` doesn't
  supply (§6.7). `scale` calls `new` before `new` is written, so `new` is
  declared first and defined later, like a C prototype (§6.3).
- `shapes.luah` / `shapes.luac` — plain data. `Shape` is one record for
  every kind of shape, with a `kind` tag, the way C code does it; each kind
  sets the fields it uses.
- `main.luac` — requires both through their headers.

Break it to see the checker's job. Each change below is an error:

| Change                                                  | Error                                   |
|---------------------------------------------------------|-----------------------------------------|
| in `main.luac`, `r = 1` → `r = "1"`                     | TL0101 mismatch: `string` is not `number` |
| in `main.luac`, `r = 1` → `radius = 1`                  | TL0103 unknown field `radius` on `Shape` |
| in `main.luac`, `v:len()` → `v:len(1)`                  | TL0102 arity: `len` takes no arguments  |
| in `shapes.luac`, `s.w * s.h` → `s.w * s.kind`          | TL0104 undefined operator: `number * string` |
| in `vec.luac`, remove `add` from the returned table     | TL0203 header mismatch: `vec` lacks `add` |
| in `main.luac`, delete `#include "vec.luah"`            | TL0201 missing include for `require("vec")` |
| in `vec.luac`, add `Meta.extra = 1` after `Meta`        | TL0103 unknown field `extra`: tables do not grow |

## 03-plain-lua-module

`csv.lua` is ordinary Lua, left as it is. `csv.luah` beside it declares its
interface, and `main.luac` uses it like any other module. The header is
trusted: if it lies about `csv.lua`, the checker cannot tell. The compiler
copies `csv.lua` to the output unchanged.

`to_item` returns `nil` for a bad row. Any value may be `nil`, as in Lua, so
the caller checks with `if item then`.

## 04-native-module

`headers/socket/core.luah` declares a native library that
`require("socket.core")` loads — a `.dll` or `.so` with no Lua source. The
project lists `headers/` as a header directory, so `main.luac` includes it
with angle brackets. `receive` takes a pattern string or a byte count, so
its header gives that parameter `any`. `fetch_status` returns two values in
the usual `value, error` style.

This one needs LuaSocket installed to run, so `check.py` only loads it.

## 05-love-game

LÖVE provides `love` as a global, so `headers/love.luah` declares it
`extern`. The game defines `love.update` and the other callbacks by
assigning fields the header declares, with the same types.
`number[string]` maps key names to directions.

This one needs LÖVE to run, so `check.py` only loads it.

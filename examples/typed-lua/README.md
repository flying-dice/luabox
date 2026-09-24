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
| [02-geometry](02-geometry/)                       | modules with headers, metatable objects, a tagged union, `any` plus a cast, a forward declaration |
| [03-plain-lua-module](03-plain-lua-module/)       | using an existing `.lua` module through a header you write     |
| [04-native-module](04-native-module/)             | a header for a native library, multiple returns, narrowing     |
| [05-love-game](05-love-game/)                     | `extern` globals from a host (LÖVE), callbacks typed by their field, maps keyed by a literal union |

## 01-hello

The whole language in eleven lines. `string.rep` needs
`#include <string.luah>`; `print` and `ipairs` are base functions and need
nothing. Compare `src/main.luac` with `dist/main.lua`: the types become
spaces and nothing else changes.

## 02-geometry

Three modules, two with headers:

- `vec.luah` / `vec.luac` — a vector object built the plain Lua way, with a
  metatable. The header's `Vec` type lists the fields and the methods;
  `setmetatable` gives the table the methods from `__index` (§6.7). `scale`
  calls `new` before `new` is written, so `new` is forward-declared (§6.3).
- `shapes.luah` / `shapes.luac` — plain data. `Shape` is a union told apart
  by the `kind` field, and `area` narrows on it (§6.9). `largest` works on
  any array, so it takes `any[]` and `main.luac` casts its result.
- `main.luac` — requires both through their headers. Each table in `scene`
  is checked against the `Shape` member its `kind` selects.

Break it to see the checker's job. Each change below is an error:

| Change                                                  | Error                                   |
|---------------------------------------------------------|-----------------------------------------|
| in `main.luac`, `r = 1` → `r = "1"`                     | TL0101 mismatch: `string` is not `number` |
| in `main.luac`, drop `r = 1`                            | TL0103 missing field `r` in `Circle`    |
| in `main.luac`, `kind = "circle"` → `kind = "square"`   | TL0101 mismatch: no member of `Shape` has `kind` `"square"` |
| in `shapes.luac`, move `return s.w * s.h` above the `if` | TL0104 unknown field `w` on `Circle`   |
| in `vec.luac`, remove `add` from the returned table     | TL0203 header mismatch: `vec` lacks `add` |
| in `main.luac`, delete `#include "vec.luah"`            | TL0201 missing include for `require("vec")` |
| in `vec.luac`, add `Meta.extra = 1` after `Meta`        | TL0104 unknown field `extra`: tables do not grow |

## 03-plain-lua-module

`csv.lua` is ordinary Lua, left as it is. `csv.luah` beside it declares its
interface, and `main.luac` uses it like any other module. The header is
trusted: if it lies about `csv.lua`, the checker cannot tell. The compiler
copies `csv.lua` to the output unchanged.

`to_item` shows narrowing doing the work of a null check: after
`if price == nil then return nil end`, `price` is a `number`.

## 04-native-module

`headers/socket/core.luah` declares a native library that
`require("socket.core")` loads — a `.dll` or `.so` with no Lua source. The
project lists `headers/` as a header directory, so `main.luac` includes it
with angle brackets. `fetch_status` returns two values, and
`if not conn then return … end` narrows `conn` for the rest of the function.

This one needs LuaSocket installed to run, so `check.py` only loads it.

## 05-love-game

LÖVE provides `love` as a global, so `headers/love.luah` declares it
`extern`. The game defines `love.update` and the other callbacks by
assigning fields the header declares; `dt` gets its type from the
`update` field and needs no annotation. `number[Key]` is a map whose keys
must be one of the `Key` literals.

This one needs LÖVE to run, so `check.py` only loads it.

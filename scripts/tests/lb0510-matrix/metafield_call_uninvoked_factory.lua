-- The `__call` fixture with its last line changed to `type(f)`: nothing calls
-- `f`, so the `self:build()` inside `Factory.__call` is never reached and the
-- program runs fine. Byte-identical lint output against opposite runtime
-- verdicts was the round-7 false positive; the gate now asks whether a plain
-- call on an instance happens, not whether a colon call is written somewhere.
---@class Factory
local Factory = {}
function Factory:build() return 42 end
function Factory.__call(self) return self:build() end
local f = setmetatable({}, Factory)
print(type(f))

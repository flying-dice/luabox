-- `false and ctor` never evaluates its right operand, so the name holds
-- `false` and no instance of Cache exists. The `if c then` guard is what
-- keeps the program running -- it is the idiom this shape appears in, and
-- it is a *name* condition, so nothing prunes the colon call inside it.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local c = false and setmetatable({}, Cache)
if c then print(c:reset()) end
print("done")

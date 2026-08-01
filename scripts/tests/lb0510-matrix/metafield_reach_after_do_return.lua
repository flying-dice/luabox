-- `do return end` ends the chunk, so every statement after it is dead.
-- This is the only spelling Lua accepts -- a bare `return` must be the last
-- statement of its block.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local c = setmetatable({}, Cache)
print("done")
do return end
c:reset()

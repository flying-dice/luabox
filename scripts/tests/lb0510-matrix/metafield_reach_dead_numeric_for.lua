-- A numeric `for` whose literal bounds run zero times: `for i = 1, 0` never
-- enters its body, exactly as `if false then` never enters its block.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local c = setmetatable({}, Cache)
for _ = 1, 0 do c:reset() end
print("done")

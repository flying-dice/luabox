-- The uninvoked twin of the multi-return slot shape.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local function make() return 1, setmetatable({}, Cache) end
local n, c = make()
print(n, type(c))

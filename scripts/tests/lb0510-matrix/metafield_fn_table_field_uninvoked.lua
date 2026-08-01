-- The uninvoked twin of the table-field miss: same silence, and here it is
-- the right answer. The pair is what makes the miss visible.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local box = { c = setmetatable({}, Cache) }
print(type(box.c))

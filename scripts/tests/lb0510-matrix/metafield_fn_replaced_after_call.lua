-- DISCLOSED false negative: the name-to-body map is insert-last-wins, so a
-- function CALLED and then REPLACED resolves to the replacement. The body
-- the call really enters is never marked reached, and its colon call is
-- not counted.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local c = setmetatable({}, Cache)
local function run() return c:reset() end
run()
run = function() return "replaced" end
print(type(run))

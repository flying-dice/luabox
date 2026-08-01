-- The twin: a literal-true branch is live and its colon call counts.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local c = setmetatable({}, Cache)
if true then c:reset() end
print("done")

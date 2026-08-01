-- The twin, where the undecided name really is truthy: the construction is
-- the value and the colon call fails. Seeding through a non-literal cond is
-- what makes this row right and its partner wrong.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local other = { reset = function() return 1 end }
local ready = true
local c = ready and setmetatable({}, Cache) or other
print(c:reset())

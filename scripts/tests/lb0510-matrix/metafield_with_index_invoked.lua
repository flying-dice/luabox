-- Metafield AND `__index`: wired, so there is nothing to report either way.
---@class Counter
local Counter = {}
Counter.__index = Counter
Counter.__tostring = function(c) return "counter" end
function Counter:value() return self.n end
local c = setmetatable({ n = 1 }, Counter)
print(c:value())

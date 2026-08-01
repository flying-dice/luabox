-- The canonical carrier program in its constructor spelling, with a metafield
-- alongside: `Counter.new` returns the construction, and the local it binds
-- takes the colon call.
---@class Counter
local Counter = {}
Counter.__tostring = function(c) return "counter" end
function Counter:value() return self.n end
function Counter.new(n) return setmetatable({ n = n }, Counter) end
local c = Counter.new(1)
print(c:value())

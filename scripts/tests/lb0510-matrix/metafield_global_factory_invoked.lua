-- The constructor pattern with a *global* factory: `function make() ... end`
-- is an assignment to a name, which the shape collector used to ignore
-- entirely.
---@class Counter
local Counter = {}
Counter.__tostring = function(c) return "counter" end
function Counter:value() return self.n end
function make(n) return setmetatable({ n = n }, Counter) end
local c = make(1)
print(c:value())

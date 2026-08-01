-- The uninvoked twin of the global factory: the instance is constructed and
-- never has a method reached on it.
---@class Counter
local Counter = {}
Counter.__tostring = function(c) return "counter" end
function Counter:value() return self.n end
function make(n) return setmetatable({ n = n }, Counter) end
local c = make(1)
print(type(c))

-- A class that happens to overload an operator. `c:value()` is reached.
---@class Counter
local Counter = {}
Counter.__tostring = function(c) return "counter" end
function Counter:value() return self.n end
local c = setmetatable({ n = 1 }, Counter)
print(c:value())

-- The uninvoked twin of the depth-two constructor miss.
---@class Counter
local Counter = {}
Counter.__mode = "k"
function Counter:value() return self.n end
local function inner() return setmetatable({ n = 1 }, Counter) end
local function outer() return inner() end
local c = outer()
print(type(c))

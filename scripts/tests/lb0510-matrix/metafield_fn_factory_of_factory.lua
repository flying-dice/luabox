-- DISCLOSED false negative: constructor depth is one on purpose. `outer`
-- returns `inner()`, and a factory that returns another factory's result is
-- not chased -- an unbounded fixpoint over (value x carrier) is a cost a big
-- file would pay for a rare shape.
---@class Counter
local Counter = {}
Counter.__mode = "k"
function Counter:value() return self.n end
local function inner() return setmetatable({ n = 1 }, Counter) end
local function outer() return inner() end
local c = outer()
print(c:value())

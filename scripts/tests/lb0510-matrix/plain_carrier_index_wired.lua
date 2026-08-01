-- No metafield, `__index` written: silent and correct.
---@class Counter
local Counter = {}
Counter.__index = Counter
function Counter:value() return self.n end
local c = setmetatable({ n = 1 }, Counter)
print(c:value())

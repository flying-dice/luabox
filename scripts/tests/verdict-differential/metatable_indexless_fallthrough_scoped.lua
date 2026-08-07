---@class FallCounter
---@field n integer
local FallCounter = {}
function FallCounter:value() return self.n end

local c = setmetatable({ n = 1 }, FallCounter)
print(c:value())
c:nonexistent()

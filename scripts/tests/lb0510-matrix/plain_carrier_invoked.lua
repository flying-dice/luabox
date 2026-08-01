-- No metafield: the rule's original region. The reviewer's round-2 repro.
---@class Counter
local Counter = {}
function Counter:value() return self.n end
local c = setmetatable({ n = 1 }, Counter)
print(c:value())

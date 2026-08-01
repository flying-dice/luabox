-- No metafield, and nothing invoked. Still reported, on purpose: the
-- construction cannot serve the lookup the annotation promises. The two
-- columns disagree here, and that disagreement is the documented bound.
---@class Counter
local Counter = {}
function Counter:value() return self.n end
local c = setmetatable({ n = 1 }, Counter)
print(type(c))

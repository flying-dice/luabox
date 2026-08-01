-- The same carrier, with the method actually reached on both instances.
-- Two constructions => two findings, one per site.
---@class Sorter
local Sorter = {}
Sorter.__lt = function(a, b) return rawget(a, "n") < rawget(b, "n") end
function Sorter:cmp(other) return self.n < other.n end
local a = setmetatable({ n = 1 }, Sorter)
local b = setmetatable({ n = 2 }, Sorter)
print(a:cmp(b))

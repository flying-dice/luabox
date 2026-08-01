-- An `__lt` comparator. Two constructions, one carrier; `Sorter:cmp` exists
-- but the file compares with `<`, which goes through the metamethod.
---@class Sorter
local Sorter = {}
Sorter.__lt = function(a, b) return rawget(a, "n") < rawget(b, "n") end
function Sorter:cmp(other) return self.n < other.n end
local a = setmetatable({ n = 1 }, Sorter)
local b = setmetatable({ n = 2 }, Sorter)
print(a < b)

---@class DynamicIndexed
---@field [string] fun(): string
local H = {}
for _, n in ipairs({ "one" }) do
  H[n] = function() return n end
end
return H

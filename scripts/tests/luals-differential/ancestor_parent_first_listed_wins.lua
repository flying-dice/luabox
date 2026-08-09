---@class AncBase
---@field x string
---@class AncSub : AncBase
---@field x number
---@class AncChild : AncBase, AncSub

---@param n number
local function want(n) end

---@type AncChild
local c
want(c.x)

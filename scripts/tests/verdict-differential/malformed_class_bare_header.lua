-- The header names nothing at all; the `---@type A` below is a consumer of
-- the name it meant to declare, two lines away.
---@class
---@field x number
local M = {}

---@type A
local a = M

return { M, a }

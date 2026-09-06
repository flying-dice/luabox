-- Two independent mistakes on one header: the trailing comma leaves an
-- extends slot with no name in it, and `P` is declared nowhere.
---@class A : P,
---@field x number
local M = {}

return M

-- `123abc` lexes as a name token but cannot be one: an identifier does not
-- start with a digit.
---@class 123abc
---@field x number
local M = {}

return M

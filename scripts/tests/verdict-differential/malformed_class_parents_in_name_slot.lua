---@class Base
---@field b number
local Base = {}

-- The parent list sits where the class name should be: this header names
-- nothing, so it declares nothing.
---@class : Base
---@field x number
local M = {}

return { Base, M }


---@class Base<T>
---@field item T
---@class SubBare : Base
---@class SubBound : Base<number>
---@type SubBare
local sb
---@type SubBound
local sd

---@param s string
local function want_string(s) end
---@param n number
local function want_number(n) end

want_string(sd.item)
want_string(sb.item)


---@class Base<V>
---@field item V
---@class A : Base<number>
---@class B : Base<number>
---@class C : A, B
---@type C
local c

---@param s string
local function want_string(s) end
---@param n number
local function want_number(n) end

want_string(c.item)

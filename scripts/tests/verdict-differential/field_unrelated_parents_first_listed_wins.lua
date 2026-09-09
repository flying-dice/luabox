
---@class P1
---@field x number
---@class P2
---@field x string
---@class C : P1, P2
---@type C
local c

---@param s string
local function want_string(s) end
---@param n number
local function want_number(n) end

want_string(c.x)

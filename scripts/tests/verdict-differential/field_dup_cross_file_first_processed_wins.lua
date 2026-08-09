
---@type Foo
local f

---@param s string
local function want_string(s) end
---@param n number
local function want_number(n) end

want_string(f.x)

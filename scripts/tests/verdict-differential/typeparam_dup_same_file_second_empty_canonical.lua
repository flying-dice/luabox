
---@class Boxed<T>
---@field value T
---@class Boxed
---@field other number
---@type Boxed<string>
local b

---@param s string
local function want_string(s) end
---@param n number
local function want_number(n) end

want_string(b.value)

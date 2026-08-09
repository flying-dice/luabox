---@class SurplusT<T>
---@field value T

---@class SurplusT<T, V>
---@field extra V

---@type SurplusT<string>
local b

---@param s string
local function want_string(s) end
---@param n number
local function want_number(n) end

want_string(b.value)
want_number(b.extra)

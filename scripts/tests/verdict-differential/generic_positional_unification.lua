---@class BoxedT<T>
---@field value T

---@class BoxedT<U>
---@field other U

---@type BoxedT<string>
local b

---@param s string
local function want_string(s) end
---@param n number
local function want_number(n) end

want_string(b.value)
want_string(b.other)
want_number(b.value)

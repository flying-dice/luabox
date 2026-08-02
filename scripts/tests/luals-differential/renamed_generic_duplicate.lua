---@class Boxed<T>
---@field value T

---@class Boxed<U>
---@field extra U

---@type Boxed<string>
local b

---@param s string
local function want(s) end

want(b.value)
want(b.extra)

---@meta
-- Lua 5.3 math library.

---@class mathlib
---@field pi number
---@field huge number
---@field maxinteger integer
---@field mininteger integer
math = {}

---@param x number
---@return number
function math.abs(x) end

---@param x number
---@return number
function math.acos(x) end

---@param x number
---@return number
function math.asin(x) end

---@param y number
---@param x? number
---@return number
function math.atan(y, x) end

---@param x number
---@return integer
function math.ceil(x) end

---@param x number
---@return number
function math.cos(x) end

---@param x number
---@return number
function math.exp(x) end

---@param x number
---@return integer
function math.floor(x) end

---@param x number
---@param y number
---@return number
function math.fmod(x, y) end

---@param x number
---@param base? number
---@return number
function math.log(x, base) end

---@param x number
---@param ... number
---@return number
function math.max(x, ...) end

---@param x number
---@param ... number
---@return number
function math.min(x, ...) end

---@param x number
---@return number integral
---@return number fractional
function math.modf(x) end

-- `random()` gives a float in [0,1); `random(m)` and `random(m,n)` give integers.
---@return number
---@overload fun(m: integer): integer
---@overload fun(m: integer, n: integer): integer
function math.random() end

---@param x? integer
---@param y? integer
function math.randomseed(x, y) end

---@param x number
---@return number
function math.sin(x) end

---@param x number
---@return number
function math.sqrt(x) end

---@param x number
---@return number
function math.tan(x) end

-- 5.3+.
---@param x number
---@return integer|nil
function math.tointeger(x) end

---@param x any
---@return string|nil
function math.type(x) end

---@param m integer
---@param n integer
---@return boolean
function math.ult(m, n) end

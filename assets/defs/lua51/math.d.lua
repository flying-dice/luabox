---@meta
-- Lua 5.1 math library. (No integer subtype: `ceil`/`floor` return number.
-- No math.type/tointeger/maxinteger/ult — those are 5.3+. `math.mod` was
-- removed after 5.0 and is intentionally absent.)

---@class mathlib
---@field pi number
---@field huge number
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

---@param x number
---@return number
function math.atan(x) end

---@param y number
---@param x number
---@return number
function math.atan2(y, x) end

---@param x number
---@return number
function math.ceil(x) end

---@param x number
---@return number
function math.cos(x) end

---@param x number
---@return number
function math.cosh(x) end

---@param x number
---@return number
function math.deg(x) end

---@param x number
---@return number
function math.exp(x) end

---@param x number
---@return number
function math.floor(x) end

---@param x number
---@param y number
---@return number
function math.fmod(x, y) end

---@param x number
---@return number mantissa
---@return integer exponent
function math.frexp(x) end

---@param m number
---@param e integer
---@return number
function math.ldexp(m, e) end

---@param x number
---@return number
function math.log(x) end

---@param x number
---@return number
function math.log10(x) end

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

---@param x number
---@param y number
---@return number
function math.pow(x, y) end

---@param x number
---@return number
function math.rad(x) end

---@return number
---@overload fun(m: integer): integer
---@overload fun(m: integer, n: integer): integer
function math.random() end

---@param x? integer
function math.randomseed(x) end

---@param x number
---@return number
function math.sin(x) end

---@param x number
---@return number
function math.sinh(x) end

---@param x number
---@return number
function math.sqrt(x) end

---@param x number
---@return number
function math.tan(x) end

---@param x number
---@return number
function math.tanh(x) end

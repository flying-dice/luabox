---@meta
-- Lua 5.2 basic library — global functions and values.
-- SPEC.md §3 definition package. Signatures follow canonical LuaLS defs.

---@param ... any
function print(...) end

---@param v any
---@return string
function type(v) end

---@param t table
---@return function iterator
---@return table state
---@return any control
function pairs(t) end

---@param t table
---@return function iterator
---@return table state
---@return integer control
function ipairs(t) end

---@param t table
---@param index? any
---@return any key
---@return any value
function next(t, index) end

---@param index integer|string
---@param ... any
---@return any ...
function select(index, ...) end

---@param v any
---@return string
function tostring(v) end

-- `tonumber(e)` yields a float/number; the `(e, base)` form yields an integer.
---@param e any
---@param base? integer
---@return number|nil
---@overload fun(e: string, base: integer): integer|nil
function tonumber(e, base) end

---@param f function
---@param ... any
function pcall(f, ...) end

-- 5.2+ `xpcall` forwards extra arguments to `f` (5.1 does not).
---@param f function
---@param msgh function
---@param ... any
function xpcall(f, msgh, ...) end

---@param message any
---@param level? integer
function error(message, level) end

---@param v any
---@param message? any
---@param ... any
function assert(v, message, ...) end

---@param t table
---@param metatable? table
---@return table
function setmetatable(t, metatable) end

---@param object any
---@return table|nil
function getmetatable(object) end

---@param t table
---@param k any
---@return any
function rawget(t, k) end

---@param t table
---@param k any
---@param v any
---@return table
function rawset(t, k, v) end

---@param a any
---@param b any
---@return boolean
function rawequal(a, b) end

---@param v table|string
---@return integer
function rawlen(v) end

---@param modname string
---@return any
function require(modname) end

---@param opt? string
---@param arg? integer
---@return any
function collectgarbage(opt, arg) end

---@param chunk string|function
---@param chunkname? string
---@param mode? string
---@param env? table
---@return function|nil
---@return string? errmsg
function load(chunk, chunkname, mode, env) end

---@param filename? string
---@param mode? string
---@param env? table
---@return function|nil
---@return string? errmsg
function loadfile(filename, mode, env) end

---@param filename? string
---@return any ...
function dofile(filename) end

---@type table
_G = {}

---@type string
_VERSION = "Lua 5.2"

---@meta
-- Lua 5.1 basic library — global functions and values.

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

---@param e any
---@param base? integer
---@return number|nil
---@overload fun(e: string, base: integer): number|nil
function tonumber(e, base) end

---@param f function
---@param ... any
function pcall(f, ...) end

-- 5.1 `xpcall(f, handler)` — no extra arguments are forwarded to `f`
-- (that is a 5.2+ feature).
---@param f function
---@param msgh function
function xpcall(f, msgh) end

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

---@param modname string
---@return any
function require(modname) end

---@param opt? string
---@param arg? integer
---@return any
function collectgarbage(opt, arg) end

-- 5.1 `unpack` is a global (moved to `table.unpack` in 5.2).
---@param list table
---@param i? integer
---@param j? integer
---@return any ...
function unpack(list, i, j) end

---@param func function
---@param chunkname? string
---@return function|nil
---@return string? errmsg
function load(func, chunkname) end

---@param text string
---@param chunkname? string
---@return function|nil
---@return string? errmsg
function loadstring(text, chunkname) end

---@param filename? string
---@return function|nil
---@return string? errmsg
function loadfile(filename) end

---@param filename? string
---@return any ...
function dofile(filename) end

-- 5.1 environment functions (removed in 5.2).
---@param f function|integer
---@param table table
---@return function
function setfenv(f, table) end

---@param f? function|integer
---@return table
function getfenv(f) end

---@param name string
---@param ... any
function module(name, ...) end

---@type table
_G = {}

---@type string
_VERSION = "Lua 5.1"

---@meta
-- Lua 5.3 utf8 library (5.3+).

---@class utf8lib
---@field charpattern string
utf8 = {}

---@param ... integer
---@return string
function utf8.char(...) end

---@param s string
---@return function
function utf8.codes(s) end

---@param s string
---@param i? integer
---@param j? integer
---@return integer ...
function utf8.codepoint(s, i, j) end

---@param s string
---@param i? integer
---@param j? integer
---@return integer|nil
---@return integer? errpos
function utf8.len(s, i, j) end

---@param s string
---@param n integer
---@param i? integer
---@return integer|nil
function utf8.offset(s, n, i) end

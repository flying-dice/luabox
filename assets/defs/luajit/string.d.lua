---@meta
-- Lua 5.1 string library. (No string.pack/unpack/packsize — those are 5.3+.)

---@class stringlib
string = {}

---@param s string
---@param i? integer
---@param j? integer
---@return integer ...
function string.byte(s, i, j) end

---@param ... integer
---@return string
function string.char(...) end

---@param f function
---@return string
function string.dump(f) end

---@param s string
---@param pattern string
---@param init? integer
---@param plain? boolean
---@return integer|nil start
---@return integer? finish
---@return any ...
function string.find(s, pattern, init, plain) end

---@param s string
---@param ... any
---@return string
function string.format(s, ...) end

---@param s string
---@param pattern string
---@return function
function string.gmatch(s, pattern) end

---@param s string
---@param pattern string
---@param repl string|table|function
---@param n? integer
---@return string
---@return integer count
function string.gsub(s, pattern, repl, n) end

---@param s string
---@return integer
function string.len(s) end

---@param s string
---@return string
function string.lower(s) end

---@param s string
---@param pattern string
---@param init? integer
---@return string|nil ...
function string.match(s, pattern, init) end

---@param s string
---@param n integer
---@return string
function string.rep(s, n) end

---@param s string
---@return string
function string.reverse(s) end

---@param s string
---@param i integer
---@param j? integer
---@return string
function string.sub(s, i, j) end

---@param s string
---@return string
function string.upper(s) end

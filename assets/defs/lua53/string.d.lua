---@meta
-- Lua 5.3 string library.
-- NOTE: string values carry the `string` library as their metatable
-- `__index`, so `("x"):rep(3)` works at runtime. Modeling method-on-value
-- dispatch needs the checker to know a string literal's metatable — that is
-- TODO(P1) (value-method dispatch); these defs declare the module surface.

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
---@param strip? boolean
---@return string
function string.dump(f, strip) end

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
---@param init? integer
---@return function
function string.gmatch(s, pattern, init) end

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
---@param sep? string
---@return string
function string.rep(s, n, sep) end

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

-- 5.3+ binary packing.
---@param fmt string
---@param ... any
---@return string
function string.pack(fmt, ...) end

---@param fmt string
---@param s string
---@param pos? integer
---@return any ...
function string.unpack(fmt, s, pos) end

---@param fmt string
---@return integer
function string.packsize(fmt) end

---@meta
-- LuaJIT `bit` library (Mike Pall's BitOp).

---@class bitlib
bit = {}

---@param x integer
---@return integer
function bit.tobit(x) end

---@param x integer
---@param n? integer
---@return string
function bit.tohex(x, n) end

---@param x integer
---@return integer
function bit.bnot(x) end

---@param x integer
---@param ... integer
---@return integer
function bit.band(x, ...) end

---@param x integer
---@param ... integer
---@return integer
function bit.bor(x, ...) end

---@param x integer
---@param ... integer
---@return integer
function bit.bxor(x, ...) end

---@param x integer
---@param n integer
---@return integer
function bit.lshift(x, n) end

---@param x integer
---@param n integer
---@return integer
function bit.rshift(x, n) end

---@param x integer
---@param n integer
---@return integer
function bit.arshift(x, n) end

---@param x integer
---@param n integer
---@return integer
function bit.rol(x, n) end

---@param x integer
---@param n integer
---@return integer
function bit.ror(x, n) end

---@param x integer
---@return integer
function bit.bswap(x) end

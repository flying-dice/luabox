---@meta
-- Lua 5.1 table library. (`unpack` is a global in 5.1, not `table.unpack`;
-- `table.pack`/`table.move` are 5.2+/5.3+.)

---@class tablelib
table = {}

---@param list table
---@param value any
---@overload fun(list: table, pos: integer, value: any)
function table.insert(list, value) end

---@param list table
---@param pos? integer
---@return any removed
function table.remove(list, pos) end

---@param list table
---@param sep? string
---@param i? integer
---@param j? integer
---@return string
function table.concat(list, sep, i, j) end

---@param list table
---@param comp? function
function table.sort(list, comp) end

-- 5.1-only: the largest positive integer key of an array-like table.
---@param list table
---@return integer
function table.maxn(list) end

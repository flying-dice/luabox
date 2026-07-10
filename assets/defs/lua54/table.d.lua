---@meta
-- Lua 5.4 table library.

---@class tablelib
table = {}

-- Two forms: `insert(list, value)` and `insert(list, pos, value)`.
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

-- 5.2+ moved `unpack` here from the global namespace.
---@param list table
---@param i? integer
---@param j? integer
---@return any ...
function table.unpack(list, i, j) end

-- 5.2+.
---@param ... any
---@return table
function table.pack(...) end

-- 5.3+.
---@param a1 table
---@param f integer
---@param e integer
---@param t integer
---@param a2? table
---@return table
function table.move(a1, f, e, t, a2) end

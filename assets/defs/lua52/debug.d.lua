---@meta
-- Lua 5.2 debug library (minimal surface).

---@class debuglib
debug = {}

---@param message? any
---@param level? integer
---@return string
function debug.traceback(message, level) end

---@param thread_or_func any
---@param func_or_what? any
---@param what? string
---@return table|nil
function debug.getinfo(thread_or_func, func_or_what, what) end

---@param object any
---@return table|nil
function debug.getmetatable(object) end

---@param value any
---@param metatable? table
---@return any
function debug.setmetatable(value, metatable) end

---@param hook? function
---@param mask? string
---@param count? integer
function debug.sethook(hook, mask, count) end

---@return function|nil
function debug.gethook() end

---@return table
function debug.getregistry() end

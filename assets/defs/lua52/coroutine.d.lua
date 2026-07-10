---@meta
-- Lua 5.2 coroutine library.

---@class coroutinelib
coroutine = {}

---@param f function
---@return thread
function coroutine.create(f) end

---@param co thread
---@param ... any
---@return boolean success
---@return any ...
function coroutine.resume(co, ...) end

---@param ... any
---@return any ...
function coroutine.yield(...) end

---@param co thread
---@return string
function coroutine.status(co) end

---@param f function
---@return function
function coroutine.wrap(f) end

---@return boolean
function coroutine.isyieldable() end

---@return thread
---@return boolean ismain
function coroutine.running() end

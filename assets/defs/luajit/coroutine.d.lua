---@meta
-- Lua 5.1 coroutine library. (`isyieldable` is 5.2+, `close` is 5.4.)

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

---@return thread|nil
function coroutine.running() end

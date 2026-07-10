---@meta
-- LuaJIT `jit` library (minimal surface — JIT control and version info).

---@class jitlib
---@field version string
---@field version_num integer
---@field os string
---@field arch string
jit = {}

---@param ... any
function jit.on(...) end

---@param ... any
function jit.off(...) end

function jit.flush() end

---@return boolean status
---@return any ...
function jit.status() end

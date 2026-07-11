---@meta
-- Lua 5.1 package library — module loading (`require` and friends).

---@class packagelib
---@field path string
---@field cpath string
---@field loaded table
---@field preload table
---@field loaders table
---@field config string
package = {}

---@param modname string
---@param funcname string
---@return function|nil
---@return string? errmsg
function package.loadlib(modname, funcname) end

--- Sets a module's environment so it can see (and create) globals — the
--- Lua 5.1 idiom `module(..., package.seeall)`. Removed in 5.2+.
---@param module table
function package.seeall(module) end

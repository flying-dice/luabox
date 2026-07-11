---@meta
-- Lua 5.2 package library — module loading (`require` and friends).
-- `package.loaders` was renamed `package.searchers`; `package.seeall` is
-- gone (5.2 dropped `module()`).

---@class packagelib
---@field path string
---@field cpath string
---@field loaded table
---@field preload table
---@field searchers table
---@field config string
package = {}

---@param modname string
---@param funcname string
---@return function|nil
---@return string? errmsg
function package.loadlib(modname, funcname) end

---@param name string
---@param path string
---@param sep? string
---@param rep? string
---@return string|nil
---@return string? errmsg
function package.searchpath(name, path, sep, rep) end

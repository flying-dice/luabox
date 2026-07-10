---@meta
-- Lua 5.4 io library. File-handle methods (`f:read()` etc.) are declared on
-- the `file` class but colon-method dispatch on values is TODO(P1).

---@class file
---@field read fun(self: file, ...: any): any
---@field write fun(self: file, ...: any): file
---@field lines fun(self: file, ...: any): function
---@field close fun(self: file): boolean
---@field flush fun(self: file): file
---@field seek fun(self: file, whence?: string, offset?: integer): integer|nil
---@field setvbuf fun(self: file, mode: string, size?: integer): file

---@class iolib
---@field stdin file
---@field stdout file
---@field stderr file
io = {}

---@param filename string
---@param mode? string
---@return file|nil
---@return string? errmsg
function io.open(filename, mode) end

---@param file? file
function io.close(file) end

---@param ... string|integer
---@return string|nil ...
function io.read(...) end

---@param ... string|number
---@return file
function io.write(...) end

---@param filename? string
---@param ... any
---@return function
function io.lines(filename, ...) end

---@param file? string|file
---@return file
function io.input(file) end

---@param file? string|file
---@return file
function io.output(file) end

---@param prog string
---@param mode? string
---@return file|nil
function io.popen(prog, mode) end

---@return file
function io.tmpfile() end

---@param obj any
---@return string|nil
function io.type(obj) end

function io.flush() end

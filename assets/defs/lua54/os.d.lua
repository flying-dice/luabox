---@meta
-- Lua 5.4 os library.

---@class oslib
os = {}

---@return number
function os.clock() end

---@param format? string
---@param time? integer
---@return string|table
function os.date(format, time) end

---@param t2 integer
---@param t1 integer
---@return number
function os.difftime(t2, t1) end

---@param command? string
---@return boolean|nil
---@return string? kind
---@return integer? code
function os.execute(command) end

-- 5.2+: `code` may be a boolean or integer, plus a `close` flag.
---@param code? boolean|integer
---@param close? boolean
function os.exit(code, close) end

---@param varname string
---@return string|nil
function os.getenv(varname) end

---@param filename string
---@return boolean|nil
---@return string? errmsg
function os.remove(filename) end

---@param oldname string
---@param newname string
---@return boolean|nil
---@return string? errmsg
function os.rename(oldname, newname) end

---@param locale? string
---@param category? string
---@return string|nil
function os.setlocale(locale, category) end

---@param t? table
---@return integer
function os.time(t) end

---@return string
function os.tmpname() end

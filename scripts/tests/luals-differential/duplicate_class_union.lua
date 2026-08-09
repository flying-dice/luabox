---@class Config
---@field host string

---@class Config
---@field port number

---@param c Config
local function use(c) print(c.host, c.port) end

return use

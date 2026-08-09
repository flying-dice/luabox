---@param s string
local function use(s)
    local up = s:upper()
    local part = up:sub(1, 2)
    local n = s:len()
    return part, n
end

return use

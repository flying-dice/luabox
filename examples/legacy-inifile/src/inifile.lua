-- legacy-inifile: a small INI parser annotated entirely with LuaCATS
-- (`---@class` / `---@param` / `---@return`) — what an existing, idiomatic
-- Lua 5.1 library looks like when luabox checks it.

---@class IniFile
---@field sections table<string, table<string, string>>
local IniFile = {}
IniFile.__index = IniFile

-- Reserved for a planned duplicate-key strict mode. It is intentionally unused
-- today; the ignore's reason is mandatory (a bare ignore is itself a lint).
---@luabox-ignore unused-local reserved for the planned duplicate-key mode
local ALLOW_DUPLICATE_KEYS = false

---Trim leading and trailing whitespace.
---@param s string
---@return string
local function trim(s)
    return (s:gsub("^%s*(.-)%s*$", "%1"))
end

---Parse INI text into an IniFile. The return type is inferred through the
---`setmetatable` call, so `ini:get(...)` resolves via the `__index` chain.
---@param text string
local function parse(text)
    local sections = { default = {} }
    local current = "default"
    for line in (text .. "\n"):gmatch("(.-)\n") do
        local stripped = trim(line)
        local is_comment = stripped:sub(1, 1) == ";" or stripped:sub(1, 1) == "#"
        if stripped == "" or is_comment then
            -- blank line or comment: skip it
        else
            local header = stripped:match("^%[(.+)%]$")
            if header then
                current = header
                sections[current] = sections[current] or {}
            else
                local key, value = stripped:match("^([^=]+)=(.*)$")
                if key then
                    sections[current][trim(key)] = trim(value)
                end
            end
        end
    end
    return setmetatable({ sections = sections }, IniFile)
end

---Look up a value, or return nil if the section or key is absent.
---@param section string
---@param key string
---@return string|nil
function IniFile:get(section, key)
    local s = self.sections[section]
    if s == nil then
        return nil
    end
    return s[key]
end

---List the section names present in the file.
---@return string[]
function IniFile:section_names()
    local names = {}
    for name in pairs(self.sections) do
        names[#names + 1] = name
    end
    table.sort(names)
    return names
end

local M = { parse = parse }

-- Legacy compatibility: also expose the module as a global, the way many old
-- Lua 5.1 libraries did. Allowed via `[lint] globals = ["inifile"]`.
inifile = M

return M

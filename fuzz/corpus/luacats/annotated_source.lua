---@class Animal
---@field name string
local Animal = {}

---@param sound string the noise
---@return string
function Animal:speak(sound)
  return sound
end

local x = 1 ---@type integer

---@alias Color
---| '"red"'
---| '"green"'

-- a plain comment

---@type number

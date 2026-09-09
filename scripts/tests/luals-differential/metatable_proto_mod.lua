local Proto = {}

function Proto.hello()
  return 1
end

---@class MetaCarrier
local T = setmetatable({}, { __index = Proto })
return T

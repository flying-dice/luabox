---@class UpcBase
---@field x string
---@class UpcTrait : UpcBase
---@class UpcImpl : UpcBase
---@field x number
---@class UpcBad : UpcTrait, UpcImpl

---@type UpcBad
local bad
---@type UpcTrait
local up = bad
return up

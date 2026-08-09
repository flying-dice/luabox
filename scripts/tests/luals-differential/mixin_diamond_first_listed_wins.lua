---@class MixBase
---@field x string
---@class MixTrait : MixBase
---@class MixImpl : MixBase
---@field x number
---@class MixBad : MixTrait, MixImpl

---@param n number
local function want(n) end

---@type MixBad
local bad
want(bad.x)

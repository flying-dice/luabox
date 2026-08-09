---@class CarP1
---@field m fun():number

---@class CarP2
local CarrierP2 = {}
function CarrierP2.m() return 1 end

---@class CarChild : CarP2, CarP1

---@type CarChild
local t = {}
return t

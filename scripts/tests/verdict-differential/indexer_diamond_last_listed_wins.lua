---@class DiaBase<T>
---@field [string] T

---@class DiaMid1 : DiaBase<number>

---@class DiaMid2 : DiaBase<string>

---@class DiaLeaf : DiaMid1, DiaMid2

---@type DiaLeaf
local c

---@param s string
local function want_string(s) end

want_string(c.anything)

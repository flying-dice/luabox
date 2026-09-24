



local vec = require("vec")
local shapes = require("shapes")

-- One record type for every kind of shape; each kind sets the fields it uses.
local scene          = {
  { kind = "circle", at = vec.new(0, 0), r = 1 },
  { kind = "rect", at = vec.new(2, 2), w = 3, h = 4 },
}

local v      = vec.add(vec.new(3, 0), vec.new(0, 4))
print("length", v:len())
print("scaled", v:scale(2):len())
print("total area", string.format("%.2f", shapes.total_area(scene)))

local big = shapes.largest(scene)
if big then
  print("largest", big.kind)
end

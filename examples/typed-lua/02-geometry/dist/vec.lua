


-- `new` is used by `scale` before it is defined: a forward declaration.
local new

local function len(self     )
  return math.sqrt(self.x * self.x + self.y * self.y)
end

local function scale(self     , k        )
  return new(self.x * k, self.y * k)
end

local Meta = { __index = { len = len, scale = scale } }

function new(x        , y        )
  return setmetatable({ x = x, y = y }, Meta)
end

local function add(a     , b     )
  return new(a.x + b.x, a.y + b.y)
end

return { new = new, add = add }

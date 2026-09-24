


local function area(s       )
  if s.kind == "circle" then
    return math.pi * s.r ^ 2 -- s is a Circle here
  end
  return s.w * s.h -- and a Rect here
end

local function total_area(shapes         )
  local total         = 0
  for _, s in ipairs(shapes) do
    total = total + area(s)
  end
  return total
end

-- Works on any array, so it takes `any` and callers cast the result.
local function largest(items       , measure                       )
  local best      = nil
  local best_size         = -math.huge
  for _, item in ipairs(items) do
    local size = measure(item)
    if size > best_size then
      best, best_size = item, size
    end
  end
  return best
end

return { area = area, total_area = total_area, largest = largest }

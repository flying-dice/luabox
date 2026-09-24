


local function area(s       )
  if s.kind == "circle" then
    return math.pi * s.r ^ 2
  end
  return s.w * s.h
end

local function total_area(shapes         )
  local total         = 0
  for _, s        in ipairs(shapes) do
    total = total + area(s)
  end
  return total
end

local function largest(shapes         )
  local best        = nil
  local best_area         = -math.huge
  for _, s        in ipairs(shapes) do
    local a = area(s)
    if a > best_area then
      best, best_area = s, a
    end
  end
  return best
end

return { area = area, total_area = total_area, largest = largest }

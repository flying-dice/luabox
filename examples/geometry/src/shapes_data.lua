---@use geometry

-- A Point is plain data — no methods. Binding a table literal with `---@struct`
-- seals it against the struct declaration: every non-optional field must be
-- present, and no unknown keys are allowed.
---@struct Point
local origin = { x = 0, y = 0 }

-- The optional `label` field (declared `string?`) may be provided or omitted.
---@struct Point
local corner = { x = 1, y = 1, label = "top-right" }

-- Pair<T> is generic. Here T is inferred as `number` from the field values;
-- a mismatched pair like `{ first = 1, second = "x" }` would fail to unify T.
---@struct Pair
local dimensions = { first = 640, second = 480 }

-- ---------------------------------------------------------------------------
-- Sealed checking, demonstrated. Each line below WOULD be a hard error under
-- `luabox check` (shape rules are errors at every strictness level). They are
-- commented out so the project stays green — uncomment one to see it fire:
--
--   ---@struct Point
--   local missing = { x = 0 }
--       -- error[LB2001]: missing non-optional field `y`
--       --               on a value bound to struct `Point`
--
--   ---@struct Point
--   local extra = { x = 0, y = 0, z = 0 }
--       -- error[LB2002]: unknown key `z` on sealed struct `Point`
-- ---------------------------------------------------------------------------

return { origin = origin, corner = corner, dimensions = dimensions }

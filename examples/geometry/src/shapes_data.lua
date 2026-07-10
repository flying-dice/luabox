-- Plain data consuming `.luab` types through the standard annotation
-- positions. The scope is ambient: no imports, types addressed by their
-- fully-qualified names (SHAPES-V2.md).

-- A Point is plain data — no methods. `---@type geometry.Point` checks the
-- literal against the sealed object type: every non-optional field must be
-- present, and no unknown keys are allowed.
---@type geometry.Point
local origin = { x = 0, y = 0 }

-- The optional `label` field (declared `label?: string`) may be provided or
-- omitted.
---@type geometry.Point
local corner = { x = 1, y = 1, label = "top-right" }

-- Pair<T> is generic, monomorphised at the use site.
---@type geometry.Pair<number>
local dimensions = { first = 640, second = 480 }

-- ---------------------------------------------------------------------------
-- Sealed checking, demonstrated. Each line below WOULD be an error under
-- `luabox check` (this project sets `[types] strict = true`). They are
-- commented out so the project stays green — uncomment one to see it fire:
--
--   ---@type geometry.Point
--   local missing = { x = 0 }
--       -- error[LB0302]: missing required field `y` in table literal
--
--   ---@type geometry.Point
--   local extra = { x = 0, y = 0, z = 0 }
--       -- error[LB0303]: unknown field `z` (geometry.Point does not
--       --                declare it)
-- ---------------------------------------------------------------------------

return { origin = origin, corner = corner, dimensions = dimensions }

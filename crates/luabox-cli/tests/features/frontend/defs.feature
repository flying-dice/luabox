Feature: stdlib definition packages — `---@meta` `.d.lua` ambient types
  SPEC.md §3 — luabox ships per-edition definition packages describing the
  real stdlib (basic globals, string/table/math/io/os/coroutine/debug plus
  version-specific bit32/bit/jit/utf8). They are selected by `edition` and
  merged beneath a file's own annotations, so calls to `print`, `string.*`,
  `math.*`, ... are arity- and type-checked. Project-local `[types] defs`
  layer additional packages resolved from the `defs/` directory.

  Scenario: a stdlib misuse is caught in a strict project
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      string.rep("x", "y")
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"

  Scenario: passing anything to print is fine
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      print(1, "two", true, nil, {})
      """
    When I run "luabox check"
    Then the command succeeds
    And stderr contains "check: 0 errors"

  Scenario: a version-gated stdlib signature is enforced in its edition
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      string.pack(123, 1)
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"

  Scenario: a global absent from the edition is not itself an error
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      local x = bit32.band(1, 2)
      print(x)
      """
    When I run "luabox check"
    Then the command succeeds

  Scenario: the same module resolves where the edition provides it
    Given a strict project with edition "5.2"
    And a file "src/main.lua" containing:
      """
      local x = bit32.band(1, 2)
      print(x)
      """
    When I run "luabox check"
    Then the command succeeds

  Scenario: a project-local defs package is loaded and enforced
    Given a file "defs/game.d.lua" containing:
      """
      ---@meta
      ---@param name string
      ---@return boolean
      function register(name) end
      """
    And a file "luabox.toml" containing:
      """
      [package]
      name = "fixture"
      version = "0.1.0"
      edition = "5.4"

      [types]
      strict = true
      defs = ["game"]
      """
    And a file "src/main.lua" containing:
      """
      register(123)
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"

  # --- cross-package type sharing (#108, the luals workspace.library model) ---
  # A dependency's own `[types] defs` join the consumer's ambient scope
  # automatically. These scenarios place the dependency under `lua_modules/`
  # (where a non-path dependency resolves), so the consumer is the single
  # project root the harness runs from.

  Scenario: a dependency's defs classes are ambient and checked in the consumer
    Given a file "lua_modules/geometry/luabox.toml" containing:
      """
      [package]
      name = "geometry"
      version = "0.1.0"
      edition = "5.4"

      [types]
      strict = true
      defs = ["geometry"]
      """
    And a file "lua_modules/geometry/defs/geometry.d.lua" containing:
      """
      ---@meta
      ---@class geometry.Point
      ---@field x number
      ---@field y number
      """
    And a file "luabox.toml" containing:
      """
      [package]
      name = "app"
      version = "0.1.0"
      edition = "5.4"

      [types]
      strict = true

      [dependencies]
      geometry = "1.0"
      """
    And a file "src/main.lua" containing:
      """
      ---@param p geometry.Point
      local function use(p) end
      use({ x = 1, y = "no" })
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"

  Scenario: a consumer carrier claiming a dependency interface must implement it
    Given a file "lua_modules/geometry/luabox.toml" containing:
      """
      [package]
      name = "geometry"
      version = "0.1.0"
      edition = "5.4"

      [types]
      strict = true
      defs = ["geometry"]
      """
    And a file "lua_modules/geometry/defs/geometry.d.lua" containing:
      """
      ---@meta
      ---@class geometry.Shape
      ---@field area fun(self): number
      ---@class geometry.Drawable : geometry.Shape
      ---@field draw fun(self): string
      """
    And a file "luabox.toml" containing:
      """
      [package]
      name = "app"
      version = "0.1.0"
      edition = "5.4"

      [types]
      strict = true

      [dependencies]
      geometry = "1.0"
      """
    And a file "src/square.lua" containing:
      """
      ---@class app.Square : geometry.Drawable
      ---@field side integer
      local Square = {}
      Square.__index = Square

      function Square:area()
        return self.side
      end

      return Square
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"
    And stdout contains "missing member `draw`"

  Scenario: a dependency's def-declared global API is param-checked
    Given a file "lua_modules/geometry/luabox.toml" containing:
      """
      [package]
      name = "geometry"
      version = "0.1.0"
      edition = "5.4"

      [types]
      strict = true
      defs = ["geometry"]
      """
    And a file "lua_modules/geometry/defs/geometry.d.lua" containing:
      """
      ---@meta
      ---@class geometrylib
      geometry = {}
      ---@param x number
      ---@param y number
      ---@return table
      function geometry.point(x, y) end
      """
    And a file "luabox.toml" containing:
      """
      [package]
      name = "app"
      version = "0.1.0"
      edition = "5.4"

      [types]
      strict = true

      [dependencies]
      geometry = "1.0"
      """
    And a file "src/main.lua" containing:
      """
      return geometry.point("no", 2)
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"

  Scenario: the same class from two packages is a collision warning, consumer wins
    Given a file "lua_modules/geometry/luabox.toml" containing:
      """
      [package]
      name = "geometry"
      version = "0.1.0"
      edition = "5.4"

      [types]
      strict = true
      defs = ["geometry"]
      """
    And a file "lua_modules/geometry/defs/geometry.d.lua" containing:
      """
      ---@meta
      ---@class geometry.Point
      ---@field x number
      ---@field y number
      """
    And a file "defs/dup.d.lua" containing:
      """
      ---@meta
      ---@class geometry.Point
      ---@field x number
      """
    And a file "luabox.toml" containing:
      """
      [package]
      name = "app"
      version = "0.1.0"
      edition = "5.4"

      [types]
      strict = true
      defs = ["dup"]

      [dependencies]
      geometry = "1.0"
      """
    And a file "src/main.lua" containing:
      """
      ---@type geometry.Point
      local p = { x = 1 }
      return p
      """
    When I run "luabox check"
    Then the command succeeds
    And stdout contains "LB0307"
    And stdout contains "defs/dup.d.lua"
    And stdout contains "geometry/defs/geometry.d.lua"

  Scenario: an unresolvable defs entry is a clear diagnostic
    Given a file "luabox.toml" containing:
      """
      [package]
      name = "fixture"
      version = "0.1.0"
      edition = "5.4"

      [types]
      strict = true
      defs = ["nonexistent"]
      """
    And a file "src/main.lua" containing:
      """
      print("hi")
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "cannot resolve definition package `nonexistent`"

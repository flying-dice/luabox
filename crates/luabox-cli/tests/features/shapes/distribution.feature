Feature: .luab type distribution across package boundaries
  SHAPES-V2.md — a dependency exports its type surface through `[types]
  entry` in its own manifest: the entrypoint's `export type` declarations
  mount under the dependency's package name (`geo.Point`). Internal module
  paths are not addressable from outside, and positional checking fires
  cross-package. `.luab` ships as opaque source and never leaks into build
  output. All scenarios are hermetic: path dependencies used in place, or a
  pre-populated `lua_modules/`.

  Scenario: a path dependency's exported type resolves and checks cross-package
    Given a file "luabox.toml" containing:
      """
      [package]
      name = "consumer"
      version = "0.1.0"
      edition = "5.4"

      [types]
      strict = true

      [dependencies]
      geo = { path = "geo" }
      """
    And a file "geo/luabox.toml" containing:
      """
      [package]
      name = "geo"
      version = "1.0.0"
      edition = "5.4"

      [types]
      shape-paths = ["shapes"]
      entry = "shapes/init.luab"
      """
    And a file "geo/shapes/init.luab" containing:
      """
      export type Point = { x: number, y: number }
      """
    And a file "src/main.lua" containing:
      """
      ---@type geo.Point
      local p = { x = 0 }
      return p
      """
    When I run "luabox check"
    Then diagnostic LB0302 is reported naming field "y"
    And the command fails

  Scenario: an installed dependency's type surface in lua_modules resolves
    Given a file "luabox.toml" containing:
      """
      [package]
      name = "consumer"
      version = "0.1.0"
      edition = "5.4"

      [types]
      strict = true

      [dependencies]
      geo = "1.0"
      """
    And a file "lua_modules/geo/luabox.toml" containing:
      """
      [package]
      name = "geo"
      version = "1.0.0"
      edition = "5.4"

      [types]
      shape-paths = ["shapes"]
      entry = "shapes/init.luab"
      """
    And a file "lua_modules/geo/shapes/init.luab" containing:
      """
      export type Point = { x: number, y: number }
      """
    And a file "src/main.lua" containing:
      """
      ---@type geo.Point
      local p = { x = 0 }
      return p
      """
    When I run "luabox check"
    Then diagnostic LB0302 is reported naming field "y"
    And the command fails

  Scenario: a type the entrypoint does not export stays private
    Given a file "luabox.toml" containing:
      """
      [package]
      name = "consumer"
      version = "0.1.0"
      edition = "5.4"

      [types]
      strict = true

      [dependencies]
      geo = { path = "geo" }
      """
    And a file "geo/luabox.toml" containing:
      """
      [package]
      name = "geo"
      version = "1.0.0"
      edition = "5.4"

      [types]
      shape-paths = ["shapes"]
      entry = "shapes/init.luab"
      """
    And a file "geo/shapes/init.luab" containing:
      """
      export type Point = internal.Vec2
      type Private = { secret: number }
      """
    And a file "geo/shapes/internal.luab" containing:
      """
      export type Vec2 = { x: number, y: number }
      """
    And a file "src/main.lua" containing:
      """
      ---@type geo.Private
      local p = { secret = 1 }
      return p
      """
    When I run "luabox check"
    Then diagnostic LB0305 is reported
    And the command fails

  Scenario: a dependency without a type entrypoint exports nothing
    Given a file "luabox.toml" containing:
      """
      [package]
      name = "consumer"
      version = "0.1.0"
      edition = "5.4"

      [types]
      strict = true

      [dependencies]
      geo = { path = "geo" }
      """
    And a file "geo/luabox.toml" containing:
      """
      [package]
      name = "geo"
      version = "1.0.0"
      edition = "5.4"
      """
    And a file "geo/geometry.luab" containing:
      """
      export type Point = { x: number, y: number }
      """
    And a file "src/main.lua" containing:
      """
      ---@type geo.Point
      local p = { x = 0, y = 0 }
      return p
      """
    When I run "luabox check"
    Then diagnostic LB0305 is reported
    And the command fails

  Scenario: a dependency's .luab never leaks into build output
    Given a project with edition "5.4" targeting "5.1"
    And a file "lua_modules/geo/geometry.luab" containing:
      """
      export type Point = { x: number, y: number }
      """
    And a file "src/main.lua" containing:
      """
      print("hello")
      """
    When I run "luabox build"
    Then the command succeeds
    And the emitted output contains no "type Point"

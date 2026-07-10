Feature: .luab shape distribution across package boundaries
  SHAPES.md §6 (resolution tier 3) and §7 — a dependency exports shape modules
  by listing them in `[types] shapes` in its own manifest; the consumer then
  resolves `---@use <module>` to the dependency's `.luab`, and sealed checking
  fires cross-package. A module a dependency does not export stays private, and
  two dependencies exporting the same name are an ambiguity. `.luab` ships as
  opaque source and never leaks into build output. All scenarios are hermetic:
  path dependencies used in place, or a pre-populated `lua_modules/`.

  Scenario: a path dependency's exported shape resolves and seals cross-package
    Given a file "luabox.toml" containing:
      """
      [package]
      name = "consumer"
      version = "0.1.0"
      edition = "5.4"

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
      shapes = ["geometry"]
      """
    And a file "geo/geometry.luab" containing:
      """
      struct Point { x: number, y: number }
      """
    And a file "src/main.lua" containing:
      """
      ---@use geometry

      ---@struct Point
      local p = { x = 0 }
      """
    When I run "luabox check"
    Then diagnostic LB2001 is reported naming field "y"
    And the command fails

  Scenario: an installed dependency's shape module in lua_modules resolves
    Given a file "luabox.toml" containing:
      """
      [package]
      name = "consumer"
      version = "0.1.0"
      edition = "5.4"

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
      shapes = ["geometry"]
      """
    And a file "lua_modules/geo/geometry.luab" containing:
      """
      struct Point { x: number, y: number }
      """
    And a file "src/main.lua" containing:
      """
      ---@use geometry

      ---@struct Point
      local p = { x = 0 }
      """
    When I run "luabox check"
    Then diagnostic LB2001 is reported naming field "y"
    And the command fails

  Scenario: a dependency that does not export the module is unresolved
    Given a file "luabox.toml" containing:
      """
      [package]
      name = "consumer"
      version = "0.1.0"
      edition = "5.4"

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
      struct Point { x: number, y: number }
      """
    And a file "src/main.lua" containing:
      """
      ---@use geometry
      """
    When I run "luabox check"
    Then diagnostic LB2005 is reported
    And the command fails

  Scenario: two dependencies exporting the same module are ambiguous
    Given a file "luabox.toml" containing:
      """
      [package]
      name = "consumer"
      version = "0.1.0"
      edition = "5.4"

      [dependencies]
      alpha = { path = "alpha" }
      beta = { path = "beta" }
      """
    And a file "alpha/luabox.toml" containing:
      """
      [package]
      name = "alpha"
      version = "1.0.0"
      edition = "5.4"

      [types]
      shapes = ["geometry"]
      """
    And a file "alpha/geometry.luab" containing:
      """
      struct Point { x: number }
      """
    And a file "beta/luabox.toml" containing:
      """
      [package]
      name = "beta"
      version = "1.0.0"
      edition = "5.4"

      [types]
      shapes = ["geometry"]
      """
    And a file "beta/geometry.luab" containing:
      """
      struct Point { x: number }
      """
    And a file "src/main.lua" containing:
      """
      ---@use geometry
      """
    When I run "luabox check"
    Then diagnostic LB2005 is reported
    And stdout contains "ambiguous"
    And stdout contains "alpha/geometry.luab"
    And stdout contains "beta/geometry.luab"
    And the command fails

  Scenario: a dependency's .luab never leaks into build output
    Given a project with edition "5.4" targeting "5.1"
    And a file "lua_modules/geo/geometry.luab" containing:
      """
      struct Point { x: number, y: number }
      """
    And a file "src/main.lua" containing:
      """
      print("hello")
      """
    When I run "luabox build"
    Then the command succeeds
    And the emitted output contains no "struct Point"

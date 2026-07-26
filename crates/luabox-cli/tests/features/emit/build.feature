Feature: luabox build — target lowering emit (tree mode)
  SPEC.md §2.1/§4 (§18 P3, flying-dice/luabox#4): in its default tree mode
  (`bundle = false`, `mode = plain`) `luabox build` lowers the edition
  (dialect you write) to the target (dialect you ship) and emits every file
  under the out dir, mirroring the source layout. Check runs first — build
  refuses on check errors.

  Scenario: goto lowered away for a 5.1 target
    Given a project with edition "5.4" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      local i = 0
      ::top::
      i = i + 1
      if i < 3 then goto top end
      print(i)
      """
    When I run "luabox build"
    Then the command succeeds
    And the file "dist/src/main.lua" exists
    And the emitted output contains no "goto"
    And "dist/src/main.lua" contains "repeat"

  Scenario: bitops lowered through the tree-shaken rt polyfill
    Given a project with edition "5.4" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      local mask = 5
      print(mask & 3)
      """
    When I run "luabox build"
    Then the command succeeds
    And "dist/src/main.lua" contains "__luabox_rt.band(mask, 3)"
    And the emitted output contains no "&"

  Scenario: edition equals target copies byte-identical
    Given a project with edition "5.1" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      local x   =   1 -- odd spacing survives a copy
      print(x)
      """
    When I run "luabox build"
    Then the command succeeds
    And "dist/src/main.lua" equals:
      """
      local x   =   1 -- odd spacing survives a copy
      print(x)
      """

  Scenario: build refuses on check errors
    Given a strict project with edition "5.4" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      ---@param n number
      local function double(n)
        return n * 2
      end
      double("nope")
      """
    When I run "luabox build"
    Then the command fails
    And diagnostic LB0300 is reported
    And the file "dist/src/main.lua" does not exist

  Scenario: irreducible goto is a hard build error
    Given a project with edition "5.4" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      while true do
        goto out
      end
      ::out::
      print("after")
      """
    When I run "luabox build"
    Then the command fails
    And diagnostic LB0601 is reported
    And the file "dist/src/main.lua" does not exist

  Scenario: the summary names the source and ship dialects
    Given a project with edition "5.4" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      print("hi")
      """
    When I run "luabox build"
    Then the command succeeds
    And stdout contains "build: 1 files emitted to dist (5.4 -> 5.1)"

  Scenario: --target overrides the manifest's [build] target
    Given a project with edition "5.4" targeting "5.4"
    And a file "src/main.lua" containing:
      """
      local q = 7 // 2
      print(q)
      """
    When I run "luabox build --target 5.1"
    Then the command succeeds
    And stdout contains "(5.4 -> 5.1)"
    And "dist/src/main.lua" contains "math.floor(7 / 2)"

  Scenario: an unknown --target lists the supported dialects
    Given a project with edition "5.4" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      print("hi")
      """
    When I run "luabox build --target 6.0"
    Then the command fails
    And stderr contains "unknown target `6.0`; expected one of: 5.1, 5.2, 5.3, 5.4, luajit"

  Scenario: --out redirects the emitted tree
    Given a project with edition "5.4" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      print("hi")
      """
    When I run "luabox build --out build"
    Then the command succeeds
    And the file "build/src/main.lua" exists
    And the file "dist/src/main.lua" does not exist

  Scenario: the emitted tree mirrors the source layout
    Given a project with edition "5.4" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      print("hi")
      """
    And a file "src/util/text.lua" containing:
      """
      return {}
      """
    When I run "luabox build"
    Then the command succeeds
    And the file "dist/src/main.lua" exists
    And the file "dist/src/util/text.lua" exists

  Scenario: definition files are not shipped
    Given a project with edition "5.4" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      print("hi")
      """
    And a file "src/api.d.lua" containing:
      """
      ---@meta
      ---@class Api
      """
    When I run "luabox build"
    Then the command succeeds
    And the file "dist/src/api.d.lua" does not exist

  Scenario: bundling without an entry point is a hard error
    Given a project with edition "5.1" targeting "5.1"
    And a file "src/lib.lua" containing:
      """
      return 1
      """
    When I run "luabox build --bundle"
    Then the command fails
    And stderr contains "bundle entry `src/main.lua` was not found"

  Scenario: a missing --entry names the path it looked for
    Given a project with edition "5.1" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      print("hi")
      """
    When I run "luabox build --bundle --entry src/nope.lua"
    Then the command fails
    And stderr contains "bundle entry `src/nope.lua` was not found at `src/nope.lua`"

  Scenario: --outfile needs exactly one entry point
    Given a project with edition "5.1" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      print("a")
      """
    And a file "src/other.lua" containing:
      """
      print("b")
      """
    When I run "luabox build --bundle --entry src/main.lua --entry src/other.lua --outfile dist/one.lua"
    Then the command fails
    And stderr contains "`outfile` is valid only with exactly one entry point, but 2 are configured"

  Scenario: --bundle and --no-bundle contradict each other
    Given a project with edition "5.1" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      print("hi")
      """
    When I run "luabox build --bundle --no-bundle"
    Then the command exits with code 2

  Scenario: tree mode ignores the bundle-only flags rather than rejecting them
    Given a project with edition "5.1" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      print("hi")
      """
    When I run "luabox build --no-bundle --sourcemap --minify"
    Then the command succeeds
    And the file "dist/src/main.lua" exists
    And the file "dist/src/main.lua.map" does not exist

  Scenario: a project without a manifest builds on the defaults
    Given an empty directory
    And a file "src/main.lua" containing:
      """
      local x = 1
      print(x)
      """
    When I run "luabox build"
    Then the command succeeds
    And stdout contains "(5.4 -> 5.4)"
    And the file "dist/src/main.lua" exists

  Scenario: --target still overrides the defaults when there is no manifest
    Given an empty directory
    And a file "src/main.lua" containing:
      """
      local x = 1 // 2
      print(x)
      """
    When I run "luabox build --target 5.1"
    Then the command succeeds
    And stdout contains "(5.4 -> 5.1)"
    And "dist/src/main.lua" contains "math.floor"

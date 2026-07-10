Feature: Canonical formatting — luabox fmt
  SPEC.md §10: StyLua-compatible canonical style, idempotent, `--check` for
  CI. Formatting never changes what a program means: sources that do not
  parse cleanly are left untouched, byte for byte.

  Scenario: formats a messy Lua file to canonical style
    Given an empty directory
    And I run "luabox init --edition 5.4"
    And a file "src/main.lua" containing:
      """
      local x   =1
      if x   then
      print( 'hi' )
      end
      """
    When I run "luabox fmt"
    Then the command succeeds
    And stdout contains "(1 changed)"
    And "src/main.lua" equals:
      """
      local x = 1
      if x then
          print("hi")
      end
      """

  Scenario: formatting is idempotent
    Given an empty directory
    And a file "main.lua" containing:
      """
      local t={1,2,   3}
      """
    And I run "luabox fmt"
    When I run "luabox fmt"
    Then the command succeeds
    And stdout contains "(0 changed)"
    And "main.lua" equals:
      """
      local t = { 1, 2, 3 }
      """

  Scenario: check mode fails on unformatted code and writes nothing
    Given an empty directory
    And a file "main.lua" containing:
      """
      local x   = 1
      """
    When I run "luabox fmt --check"
    Then the command fails
    And stdout contains "would reformat main.lua"
    And stderr contains "would be reformatted"
    And "main.lua" equals:
      """
      local x   = 1
      """

  Scenario: check mode passes on formatted code
    Given an empty directory
    And a file "main.lua" containing:
      """
      local x = 1
      """
    When I run "luabox fmt --check"
    Then the command succeeds
    And stdout contains "all formatted"

  Scenario: .luab shape modules are formatted too
    Given an empty directory
    And a file "shapes/point.luab" containing:
      """
      struct   Point{x:number,y:number}
      """
    When I run "luabox fmt"
    Then the command succeeds
    And "shapes/point.luab" equals:
      """
      struct Point {
          x: number,
          y: number,
      }
      """

  Scenario: broken Lua is left untouched
    Given an empty directory
    And a file "broken.lua" containing:
      """
      local = 5
      """
    When I run "luabox fmt"
    Then the command succeeds
    And stdout contains "(0 changed)"
    And "broken.lua" equals:
      """
      local = 5
      """

  Scenario: the build output directory is skipped
    Given an empty directory
    And I run "luabox init --edition 5.4"
    And a file "dist/generated.lua" containing:
      """
      local x   =1
      """
    When I run "luabox fmt"
    Then the command succeeds
    And "dist/generated.lua" equals:
      """
      local x   =1
      """

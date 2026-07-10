Feature: .luab files are analyser-only
  SHAPES-V2.md invariants — shapes never affect runtime output; .luab never
  on the require path, never in build/bundle output; method bodies are
  rejected.

  Scenario: body in .luab rejected
    Given a shape module containing a method with a body
    When I run "luabox check"
    Then diagnostic LB2010 is reported
    And stdout contains "implementations live in .lua"

  Scenario: build output identical with and without shapes
    Given a project with edition "5.4" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      local mask = 5
      print(mask & 3)
      """
    When I run "luabox build"
    Then the command succeeds
    And I stash the build output
    Given a file "src/geometry.luab" containing:
      """
      type Point = { x: number, y: number }
      """
    When I run "luabox build"
    Then the command succeeds
    And the build output is byte-identical to the stashed output

  Scenario: .luab files are formatted by luabox fmt
    Given an empty directory
    And a file "shapes/bag.luab" containing:
      """
      export type Bag={count:number,label?:string}
      """
    When I run "luabox fmt"
    Then the command succeeds
    And "shapes/bag.luab" equals:
      """
      export type Bag = {
          count: number,
          label?: string,
      }
      """
    When I run "luabox fmt"
    Then the command succeeds
    And stdout contains "(0 changed)"

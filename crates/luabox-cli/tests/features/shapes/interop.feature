@wip
Feature: Ecosystem interop
  SHAPES.md §3 — two type front-ends, one IR. Interop between LuaCATS
  annotations and .lb shapes is total.

  Scenario: LuaCATS class satisfies a shape trait
    Given trait Shape in "geometry.lb"
    And a ---@class annotated table with ---@impl Shape for Square
    When I run "luabox check"
    Then zero diagnostics are reported

  Scenario: shape struct usable in a LuaCATS annotation
    Given struct Point in "geometry.lb"
    And a Lua function annotated ---@param p Point reading p.x
    When I run "luabox check"
    Then zero diagnostics are reported

  Scenario: unresolved shape module diagnosed
    Given a Lua file with ---@use missing_module
    When I run "luabox check"
    Then diagnostic LB2005 is reported

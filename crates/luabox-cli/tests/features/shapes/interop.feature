Feature: Ecosystem interop
  SHAPES-V2.md — two type front-ends, one IR. `.luab` types are consumed
  through the standard LuaCATS positions only; interop with `---@class` is
  total and conformance is positional.

  Scenario: LuaCATS class satisfies a shape type positionally
    Given type Shape in "geometry.luab"
    And a ---@class annotated table asserted as geometry.Shape
    When I run "luabox check"
    Then zero diagnostics are reported

  Scenario: shape type usable in a LuaCATS annotation
    Given type Point in "geometry.luab"
    And a Lua function annotated ---@param p geometry.Point reading p.x
    When I run "luabox check"
    Then zero diagnostics are reported

  Scenario: unknown fully-qualified type name diagnosed
    Given type Point in "geometry.luab"
    And a Lua file binding a table { x = 0 } with ---@type missing.Module
    When I run "luabox check"
    Then diagnostic LB0305 is reported
    And the command fails

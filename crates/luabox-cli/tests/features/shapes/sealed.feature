Feature: Sealed shape checking
  SHAPES.md §5 — structs are sealed: missing non-optional fields and unknown
  keys are hard errors at every strictness level; `..` opens the shape.

  Scenario: missing field rejected
    Given a shape module "geometry" declaring struct Point { x: number, y: number }
    And a Lua file binding a table { x = 0 } with ---@struct Point
    When I run "luabox check"
    Then diagnostic LB2001 is reported naming field "y"

  Scenario: unknown key on sealed shape rejected
    Given a shape module "geometry" declaring struct Point { x: number, y: number }
    And a Lua file binding a table { x = 0, y = 0, z = 0 } with ---@struct Point
    When I run "luabox check"
    Then diagnostic LB2002 is reported naming key "z"

  Scenario: open shape accepts extra keys
    Given a shape module "geometry" declaring struct Bag { n: number, .. }
    And a Lua file binding a table { n = 1, extra = true } with ---@struct Bag
    When I run "luabox check"
    Then zero shape diagnostics are reported

  Scenario: optional field may be omitted
    Given a shape module "geometry" declaring struct Point { x: number, label: string? }
    And a Lua file binding a table { x = 0 } with ---@struct Point
    When I run "luabox check"
    Then zero shape diagnostics are reported

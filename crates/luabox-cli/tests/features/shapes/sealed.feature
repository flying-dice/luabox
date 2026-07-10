Feature: Sealed object checking (positional)
  SHAPES-V2.md — object types are sealed structural tables, and conformance
  is positional: a table literal flowing into an annotated position
  (`---@type` / `---@param` / `---@return`) must carry every required member
  and no undeclared keys. There are no binding tags and no imports; types are
  addressed by fully-qualified name.

  Scenario: missing field rejected
    Given a shape module "geometry" declaring type Point = { x: number, y: number }
    And a Lua file binding a table { x = 0 } with ---@type geometry.Point
    When I run "luabox check"
    # A `---@type` object annotation on a table constructor defers whole-carrier
    # conformance to the final value; still missing `y`, it errors at the
    # annotation (LB0300) naming the member (SHAPES-V2.md).
    Then diagnostic LB0300 is reported naming field "y"
    And the command fails

  Scenario: unknown key on sealed object rejected
    Given a shape module "geometry" declaring type Point = { x: number, y: number }
    And a Lua file binding a table { x = 0, y = 0, z = 0 } with ---@type geometry.Point
    When I run "luabox check"
    Then diagnostic LB0303 is reported naming key "z"
    And the command fails

  Scenario: optional field may be omitted
    Given a shape module "geometry" declaring type Point = { x: number, label?: string }
    And a Lua file binding a table { x = 0 } with ---@type geometry.Point
    When I run "luabox check"
    Then zero diagnostics are reported

  Scenario: short names do not resolve — references are fully qualified
    Given a shape module "geometry" declaring type Point = { x: number }
    And a Lua file binding a table { x = 0 } with ---@type Point
    When I run "luabox check"
    Then diagnostic LB0305 is reported
    And the command fails

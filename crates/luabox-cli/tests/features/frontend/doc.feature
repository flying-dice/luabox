Feature: luabox doc — static documentation site
  SPEC.md §13: `luabox doc` generates a static site into `doc/` from
  LuaCATS annotations and `.luab` shape declarations — search, cross-linked
  types, one page per module and per class/struct/trait, no external
  assets. Doc examples running under `luabox test --doc` are not
  implemented yet. `--open` (launching a browser) is deliberately not
  scenario-tested.

  Scenario: generates an index listing a documented function
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      --- Adds two numbers.
      ---@param a number
      ---@param b number
      ---@return number
      local function add(a, b)
        return a + b
      end
      """
    When I run "luabox doc"
    Then the command succeeds
    And the file "doc/index.html" exists
    And "doc/index.html" contains "add"
    And "doc/module.main.html" contains "function add(a: number, b: number): number"
    And "doc/module.main.html" contains "Adds two numbers."

  Scenario: class page lists its own and inherited fields
    Given a project with edition "5.4"
    And a file "src/shapes.lua" containing:
      """
      ---@class Shape
      ---@field id integer the identity
      local Shape = {}

      --- A circle.
      ---@class Circle: Shape
      ---@field radius number the radius
      local Circle = {}
      """
    When I run "luabox doc"
    Then the command succeeds
    And the file "doc/class.Circle.html" exists
    And "doc/class.Circle.html" contains "radius"
    And "doc/class.Circle.html" contains "Fields inherited from"
    And "doc/class.Circle.html" contains "id"

  Scenario: .luab type page renders fields and docs
    Given a project with edition "5.4"
    And a file "src/geometry.luab" containing:
      """
      --- A 2D point.
      type Point = {
          x: number,
          y: number,
      }
      """
    When I run "luabox doc"
    Then the command succeeds
    And the file "doc/type.geometry.Point.html" exists
    And "doc/type.geometry.Point.html" contains "A 2D point."
    And "doc/type.geometry.Point.html" contains "number"

  Scenario: type names cross-link to their documented pages
    Given a project with edition "5.4"
    And a file "src/geometry.luab" containing:
      """
      type Point = { x: number, y: number }
      """
    And a file "src/main.lua" containing:
      """
      --- Distance from the origin.
      ---@param p geometry.Point the point
      ---@return number
      local function dist(p)
        return (p.x ^ 2 + p.y ^ 2) ^ 0.5
      end
      """
    When I run "luabox doc"
    Then the command succeeds
    And "doc/module.main.html" contains 'href="type.geometry.Point.html"'

  Scenario: type page lists methods and the export badge
    Given a project with edition "5.4"
    And a file "src/geometry.luab" containing:
      """
      --- Things with an area.
      export type Shape = {
          --- The enclosed area.
          area(self): number,
      }
      """
    When I run "luabox doc"
    Then the command succeeds
    And the file "doc/type.geometry.Shape.html" exists
    And "doc/type.geometry.Shape.html" contains "area(self)"
    And "doc/type.geometry.Shape.html" contains "export"

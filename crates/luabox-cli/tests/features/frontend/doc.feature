Feature: luabox doc — static documentation site
  SPEC.md §13: `luabox doc` generates a static site into `doc/` from
  LuaCATS annotations and `.lb` shape declarations — search, cross-linked
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

  Scenario: .lb struct page renders with a sealed marker
    Given a project with edition "5.4"
    And a file "src/geometry.lb" containing:
      """
      /// A 2D point.
      struct Point {
          x: number,
          y: number,
      }
      """
    When I run "luabox doc"
    Then the command succeeds
    And the file "doc/struct.Point.html" exists
    And "doc/struct.Point.html" contains "sealed"
    And "doc/struct.Point.html" contains "A 2D point."
    And "doc/struct.Point.html" contains "number"

  Scenario: type names cross-link to their documented pages
    Given a project with edition "5.4"
    And a file "src/geometry.lb" containing:
      """
      struct Point { x: number, y: number }
      """
    And a file "src/main.lua" containing:
      """
      ---@use geometry

      --- Distance from the origin.
      ---@param p Point the point
      ---@return number
      local function dist(p)
        return (p.x ^ 2 + p.y ^ 2) ^ 0.5
      end
      """
    When I run "luabox doc"
    Then the command succeeds
    And "doc/module.main.html" contains 'href="struct.Point.html"'

  Scenario: trait page lists required functions and implementors
    Given a project with edition "5.4"
    And a file "src/geometry.lb" containing:
      """
      struct Circle { radius: number }

      /// Things with an area.
      trait Shape {
          /// The enclosed area.
          fn area(self) -> number;
      }

      impl Shape for Circle;
      """
    When I run "luabox doc"
    Then the command succeeds
    And the file "doc/trait.Shape.html" exists
    And "doc/trait.Shape.html" contains "fn area(self)"
    And "doc/trait.Shape.html" contains 'href="struct.Circle.html"'
    And "doc/struct.Circle.html" contains 'href="trait.Shape.html"'

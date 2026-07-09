@wip
Feature: Trait coherence
  SHAPES.md §5 — ---@impl enforces completeness and signature compatibility;
  supertraits must be conformed on the same carrier.

  Scenario: incomplete impl rejected
    Given trait Shape with fns area and perimeter
    And a carrier table with ---@impl Shape for Circle defining only area
    When I run "luabox check"
    Then diagnostic LB2003 is reported listing "perimeter"

  Scenario: impl signature mismatch rejected
    Given trait Shape with fn area(self) -> number
    And a carrier table with ---@impl Shape for Circle whose area returns a string
    When I run "luabox check"
    Then diagnostic LB2004 is reported with both spans

  Scenario: supertrait conformance required
    Given trait Drawable: Shape in "geometry.lb"
    And a carrier table with ---@impl Drawable for Circle but no Shape impl
    When I run "luabox check"
    Then diagnostic LB2008 is reported

  Scenario: extra inherent methods are fine
    Given trait Shape with fn area(self) -> number
    And a carrier table with ---@impl Shape for Circle defining area and an inherent helper
    When I run "luabox check"
    Then zero shape diagnostics are reported

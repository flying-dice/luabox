Feature: Positional structural conformance
  SHAPES-V2.md — there is no conformance tag. A value is a `geometry.Shape`
  exactly where one is demanded; the assignability error names the missing
  or mismatched members. Intersection types require every merged member.

  Scenario: carrier with all methods conforms
    Given type Shape with methods area and perimeter
    And a carrier table defining area and perimeter asserted as geometry.Shape
    When I run "luabox check"
    Then zero diagnostics are reported

  Scenario: missing method diagnosed positionally, naming the member
    Given type Shape with methods area and perimeter
    And a carrier table defining only area asserted as geometry.Shape
    When I run "luabox check"
    Then diagnostic LB0300 is reported listing "perimeter"
    And the command fails

  Scenario: method signature mismatch diagnosed
    Given type Shape with methods area and perimeter
    And a carrier table whose area method returns a string asserted as geometry.Shape
    When I run "luabox check"
    Then diagnostic LB0300 is reported listing "area"
    And the command fails

  Scenario: intersection requires members from every part
    Given type Drawable = Shape & draw in "geometry.luab"
    And a carrier table defining area and perimeter asserted as geometry.Drawable
    When I run "luabox check"
    Then diagnostic LB0300 is reported listing "draw"
    And the command fails

  Scenario: extra inherent methods are fine
    Given type Shape with methods area and perimeter
    And a carrier table defining area, perimeter and an inherent helper asserted as geometry.Shape
    When I run "luabox check"
    Then zero diagnostics are reported

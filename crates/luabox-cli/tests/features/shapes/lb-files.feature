@wip
Feature: .lb files are analyser-only
  SHAPES.md §1 invariants — shapes never affect runtime output; .lb never on
  the require path, never in build/bundle output; bodies are rejected.

  Scenario: body in .lb rejected
    Given a shape module containing a fn with a body
    When I run "luabox check"
    Then diagnostic LB2010 is reported
    And stderr contains "implementations live in .lua"

  Scenario: build output identical with and without shapes
    Given a project that builds successfully
    And the same project with .lb shape modules added
    When I run "luabox build" on both
    Then the emitted output is byte-identical

  Scenario: .lb files are formatted by luabox fmt
    Given a shape module with inconsistent whitespace
    When I run "luabox fmt"
    Then the .lb file is rewritten in canonical style
    And running "luabox fmt" again changes nothing

Feature: Diagnostic explain pages — luabox explain
  Every diagnostic is coded LBnnnn and carries a rustc-style explain page
  (SPEC.md §14). `luabox explain <code>` prints the title and page, and fails
  helpfully for codes that are unknown or malformed.

  Scenario: explaining a known code
    Given an empty directory
    When I run "luabox explain LB1001"
    Then the command succeeds
    And stdout contains "edition"

  Scenario: unknown code fails
    Given an empty directory
    When I run "luabox explain LB9999"
    Then the command fails
    And stderr contains "no such diagnostic"

  Scenario: malformed code fails
    Given an empty directory
    When I run "luabox explain banana"
    Then the command fails

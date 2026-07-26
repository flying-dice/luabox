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
    And stderr contains "`banana` is not a valid diagnostic code; codes look like LB0421"

  Scenario Outline: each diagnostic family has an explain page titled with its code
    Given an empty directory
    When I run "luabox explain <code>"
    Then the command succeeds
    And stdout contains "<code>: <title>"
    And stdout contains "# <code>: <title>"

    Examples:
      | code   | title                                                            |
      | LB0001 | syntax error                                                     |
      | LB0300 | type mismatch                                                    |
      | LB0301 | wrong argument count                                             |
      | LB0304 | return count/type mismatch                                       |
      | LB0305 | unknown type name in annotation                                  |
      | LB0313 | wrong number of generic type arguments                           |
      | LB0314 | recursive alias (cyclic-alias)                                   |
      | LB0503 | shadowed local (shadowed-local)                                  |
      | LB0505 | explicit nil comparison as truthiness                            |
      | LB0506 | string concatenation in a loop (concat-in-loop)                  |
      | LB0507 | `pairs` on an array (pairs-on-array)                             |
      | LB0508 | empty `if ... then` body (empty-then)                            |
      | LB0601 | irreducible `goto`                                               |
      | LB0603 | `<close>` lowering fidelity                                      |
      | LB0606 | integer/float divergence on lowering                             |

  Scenario: a lint page names the rule id that produces it
    Given an empty directory
    When I run "luabox explain LB0502"
    Then the command succeeds
    And stdout contains "unused-param"

  Scenario: a page carries the worked example that explains the rule
    Given an empty directory
    When I run "luabox explain LB0503"
    Then the command succeeds
    And stdout contains "```lua"
    And stdout contains "Rename one of the two, or reuse the outer binding."

  Scenario: a well-formed but unregistered code is rejected
    Given an empty directory
    When I run "luabox explain LB0421"
    Then the command fails
    And stderr contains "no such diagnostic code `LB0421`"

  Scenario: codes are case-sensitive
    Given an empty directory
    When I run "luabox explain lb0501"
    Then the command fails
    And stderr contains "is not a valid diagnostic code"

  Scenario: a code needs exactly four digits
    Given an empty directory
    When I run "luabox explain LB501"
    Then the command fails
    And stderr contains "is not a valid diagnostic code"

  Scenario: omitting the code is a usage error, not a diagnostic lookup
    Given an empty directory
    When I run "luabox explain"
    Then the command exits with code 2
    And stderr contains "required arguments were not provided"

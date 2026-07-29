Feature: The published manifest schema — luabox schema
  The manifest contract is written twice: once as the hand-rolled parser in
  `luabox-manifest`, once as a JSON Schema (draft 2020-12) so editors,
  validators and LLM coding assistants can read it (SPEC.md §5). `luabox
  schema` prints that document — no project, no flags, no filesystem, so
  `luabox schema > luabox.schema.json` works anywhere. The two copies are
  pinned to each other by the parity suite in `luabox-manifest`; what these
  scenarios own is that the CLI actually hands the document over intact.

  Scenario: the schema is machine-readable on stdout
    Given an empty directory
    When I run "luabox schema"
    Then the command succeeds
    And stdout is valid JSON

  Scenario: it is a draft 2020-12 schema that identifies itself
    Given an empty directory
    When I run "luabox schema"
    Then the command succeeds
    And stdout contains "$schema"
    And stdout contains "https://json-schema.org/draft/2020-12/schema"
    And stdout contains "$id"

  Scenario Outline: every manifest table is described
    Given an empty directory
    When I run "luabox schema"
    Then the command succeeds
    And stdout contains "<table>"

    Examples:
      | table            |
      | package          |
      | build            |
      | types            |
      | dependencies     |
      | dev-dependencies |
      | lint             |

  Scenario: the document is self-describing enough to code against
    # The point of publishing it: a reader who has only this file must learn
    # the closed vocabularies and the TOML→JSON caveat from it alone.
    Given an empty directory
    When I run "luabox schema"
    Then the command succeeds
    And stdout contains "luajit"
    And stdout contains "nvim-plugin"
    And stdout contains "TOML"

  Scenario: --help says what the document is for
    Given an empty directory
    When I run "luabox schema --help"
    Then the command succeeds
    And stdout contains "JSON Schema"
    And stdout contains "luabox.toml"

  Scenario: it needs no project to run in
    Given a file "not-a-manifest.txt" containing:
      """
      there is no luabox.toml here
      """
    When I run "luabox schema"
    Then the command exits with code 0

  Scenario: it takes no flags
    Given an empty directory
    When I run "luabox schema --format json"
    Then the command exits with code 2
    And stderr contains "unexpected argument '--format' found"

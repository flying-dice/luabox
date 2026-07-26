Feature: luabox lsp — file URIs
  SPEC.md §8 — every position the editor sends and every location the server
  hands back is keyed by a `file://` URI, so path ↔ URI conversion has to
  round-trip for names the URI path grammar cannot hold verbatim: spaces,
  non-ASCII characters, and reserved punctuation all percent-encode on the way
  out and decode on the way back in. A name that survives that round trip is
  indistinguishable from a plain ASCII one everywhere else in the protocol.

  Background:
    Given a project with edition "5.4"

  Scenario: a file name containing spaces round-trips through its URI
    Given a file "my utils.lua" containing:
      """
      local value = 1
      print(value)
      """
    And the language server is running
    And the document "my utils.lua" is open
    When I request the definition at 1:8 in "my utils.lua"
    Then the location is in "my utils.lua"
    And the location starts at 0:6

  Scenario: a directory name containing spaces round-trips too
    Given a file "my modules/helper.lua" containing:
      """
      local function computeHelper() return 1 end
      return computeHelper
      """
    And the language server is running
    And the document "my modules/helper.lua" is open
    When I request document symbols for "my modules/helper.lua"
    Then the symbols include "computeHelper"

  Scenario: a non-ASCII file name round-trips through its URI
    Given a file "héllo.lua" containing:
      """
      local valeur = 1
      print(valeur)
      """
    And the language server is running
    And the document "héllo.lua" is open
    When I request the definition at 1:8 in "héllo.lua"
    Then the location is in "héllo.lua"
    And the location starts at 0:6

  Scenario: diagnostics are published against the encoded URI
    Given a file "café/module.lua" containing:
      """
      local unused = 1
      """
    And the language server is running
    And the document "café/module.lua" is open
    Then the diagnostics for "café/module.lua" include LB0501

  Scenario: a workspace symbol reports the encoded URI of its declaring file
    Given a file "café/module.lua" containing:
      """
      local function computeThing() return 1 end
      return computeThing
      """
    And the language server is running
    When I search the workspace for symbols matching "computeThing"
    Then the symbols include "computeThing"
    And symbol "computeThing" is declared in "café/module.lua"

  Scenario: reserved punctuation in a file name is percent-encoded
    Given a file "notes#1.lua" containing:
      """
      local value = 1
      print(value)
      """
    And the language server is running
    And the document "notes#1.lua" is open
    When I request the definition at 1:8 in "notes#1.lua"
    Then the location is in "notes#1.lua"
    And the location starts at 0:6

  Scenario: a non-BMP character ahead of an identifier keeps UTF-16 columns honest
    Given a file "main.lua" containing:
      """
      local value = 1
      print("🙂", value)
      """
    And the language server is running
    And the document "main.lua" is open
    When I hover at 1:14 in "main.lua"
    Then the hover text contains "local value"
    And the hover range spans 1:12 to 1:17

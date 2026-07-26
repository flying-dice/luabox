Feature: luabox lsp — find references and document highlight
  SPEC.md §8 — references answer "where else is this used?" across the whole
  workspace, honouring `includeDeclaration`; document highlight answers the
  same question for the current file only, tagging each occurrence as a read
  or a write so the editor can shade them differently.

  Background:
    Given a project with edition "5.4"

  Scenario: references to a local include its declaration on request
    Given a file "main.lua" containing:
      """
      local value = 1
      print(value)
      return value + value
      """
    And the language server is running
    And the document "main.lua" is open
    When I request references at 1:8 in "main.lua" including the declaration
    Then the reply lists 4 locations
    And 4 of the locations are in "main.lua"
    And a location starts at 0:6

  Scenario: excluding the declaration leaves only the uses
    Given a file "main.lua" containing:
      """
      local value = 1
      print(value)
      return value + value
      """
    And the language server is running
    And the document "main.lua" is open
    When I request references at 1:8 in "main.lua" excluding the declaration
    Then the reply lists 3 locations
    And no location starts at 0:6

  Scenario: references to a global function span files
    Given a file "a.lua" containing:
      """
      function greet() return 1 end
      """
    And a file "b.lua" containing:
      """
      greet()
      greet()
      """
    And the language server is running
    And the document "b.lua" is open
    When I request references at 0:0 in "b.lua" including the declaration
    Then the reply lists 3 locations
    And 1 of the locations are in "a.lua"
    And 2 of the locations are in "b.lua"

  Scenario: document highlight distinguishes writes from reads
    Given a file "main.lua" containing:
      """
      local x = 1
      x = 2
      print(x)
      """
    And the language server is running
    And the document "main.lua" is open
    When I request document highlights at 0:6 in "main.lua"
    Then the reply lists 3 highlights
    And the highlight on line 0 is a write
    And the highlight on line 1 is a write
    And the highlight on line 2 is a read

  Scenario: document highlight stays inside the current file
    Given a file "a.lua" containing:
      """
      function greet() return 1 end
      """
    And a file "b.lua" containing:
      """
      greet()
      greet()
      """
    And the language server is running
    And the document "b.lua" is open
    When I request document highlights at 0:0 in "b.lua"
    Then the reply lists 2 highlights
    And the highlight on line 0 is a read

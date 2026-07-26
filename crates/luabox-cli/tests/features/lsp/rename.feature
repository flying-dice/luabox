Feature: luabox lsp — rename
  SPEC.md §8 — rename returns a `WorkspaceEdit` covering the declaration and
  every reference, across files when the symbol is workspace-global. Each
  edit is narrowed to the bare identifier, never a wider span. `prepareRename`
  pre-selects that identifier, and declines on anything that is not one.

  Background:
    Given a project with edition "5.4"

  Scenario: renaming a local rewrites its declaration and every use
    Given a file "main.lua" containing:
      """
      local value = 1
      print(value)
      return value + value
      """
    And the language server is running
    And the document "main.lua" is open
    When I rename the symbol at 1:8 in "main.lua" to "amount"
    Then the workspace edit spans 1 files
    And the workspace edit makes 4 edits
    And every edit writes "amount"

  Scenario: renaming a global function reaches every file that calls it
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
    When I rename the symbol at 0:0 in "b.lua" to "hello"
    Then the workspace edit spans 2 files
    And the workspace edit makes 3 edits
    And the workspace edit touches "a.lua"
    And the workspace edit touches "b.lua"
    And every edit writes "hello"

  Scenario: renaming a class field narrows the field annotation to the name
    Given a file "point.lua" containing:
      """
      ---@class Point
      ---@field x number
      """
    And a file "use.lua" containing:
      """
      ---@type Point
      local p = nil
      print(p.x)
      print(p.x)
      """
    And the language server is running
    And the document "use.lua" is open
    When I rename the symbol at 2:8 in "use.lua" to "col"
    Then the workspace edit spans 2 files
    And the workspace edit makes 3 edits
    And every edit writes "col"

  Scenario: prepare rename pre-selects the identifier under the cursor
    Given a file "main.lua" containing:
      """
      local value = 1
      print(value)
      """
    And the language server is running
    And the document "main.lua" is open
    When I prepare a rename at 1:8 in "main.lua"
    Then the prepared range spans 1:6 to 1:11

  Scenario: prepare rename declines on something that is not a symbol
    Given a file "main.lua" containing:
      """
      local value = 1
      """
    And the language server is running
    And the document "main.lua" is open
    When I prepare a rename at 0:14 in "main.lua"
    Then the reply is null

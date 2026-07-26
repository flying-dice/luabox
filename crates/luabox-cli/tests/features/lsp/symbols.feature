Feature: luabox lsp — document and workspace symbols
  SPEC.md §8 — document symbols give the editor's outline: classes with
  their fields, top-level functions with their nested ones, and top-level
  locals. Workspace symbols answer the same question across every file the
  server has indexed, matched case-insensitively.

  Background:
    Given a project with edition "5.4"

  Scenario: the outline covers classes, functions, and top-level locals
    Given a file "main.lua" containing:
      """
      ---@class Shape
      ---@field kind string

      local top = 1

      function M.helper() end

      local function outer()
          local function inner() end
      end
      """
    And the language server is running
    And the document "main.lua" is open
    When I request document symbols for "main.lua"
    Then the symbols include "Shape"
    And the symbols include "top"
    And the symbols include "M.helper"
    And the symbols include "outer"
    And symbol "Shape" is a class
    And symbol "outer" is a function
    And symbol "top" is a variable

  Scenario: class fields and nested functions are nested under their parent
    Given a file "main.lua" containing:
      """
      ---@class Shape
      ---@field kind string

      local function outer()
          local function inner() end
      end
      """
    And the language server is running
    And the document "main.lua" is open
    When I request document symbols for "main.lua"
    Then symbol "Shape" has child "kind"
    And symbol "outer" has child "inner"
    And the symbols do not include "inner"

  Scenario: a workspace search finds symbols in every indexed file
    Given a file "shapes.lua" containing:
      """
      ---@class Shape
      ---@field kind string
      """
    And a file "main.lua" containing:
      """
      function computeArea() return 1 end
      """
    And the language server is running
    When I search the workspace for symbols matching "a"
    Then the symbols include "Shape"
    And the symbols include "computeArea"
    And symbol "Shape" is declared in "shapes.lua"
    And symbol "computeArea" is declared in "main.lua"

  Scenario: the workspace query ignores case
    Given a file "main.lua" containing:
      """
      function computeArea() return 1 end
      """
    And the language server is running
    When I search the workspace for symbols matching "COMPUTEAREA"
    Then the symbols include "computeArea"

  Scenario: a query nothing matches comes back empty
    Given a file "main.lua" containing:
      """
      function computeArea() return 1 end
      """
    And the language server is running
    When I search the workspace for symbols matching "zzz_definitely_not_a_symbol"
    Then the reply is an empty list

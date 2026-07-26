Feature: luabox lsp — inlay hints
  SPEC.md §8 — inlay hints render the display-mode inference into the
  source, so unannotated Lua reads like a statically typed language:
  inferred binding types after each name, and the inferred (or annotated)
  return type after a parameter list. Function names and implicit `self`
  are deliberately skipped — those are hover's territory.

  Background:
    Given a project with edition "5.4"

  Scenario: unannotated locals and for-variables carry their inferred type
    Given a file "main.lua" containing:
      """
      local count = 42
      local greeting = "hi"
      local flag = true
      for i = 1, 10 do
        print(i)
      end
      """
    And the language server is running
    And the document "main.lua" is open
    When I request inlay hints for the first 6 lines of "main.lua"
    Then an inlay hint at 0:11 reads ": integer"
    And an inlay hint at 1:14 reads ": string"
    And an inlay hint at 2:10 reads ": boolean"
    And an inlay hint at 3:5 reads ": integer"

  Scenario: a table literal hints its inferred shape
    Given a file "main.lua" containing:
      """
      local point = { x = 1, y = 2 }
      return point
      """
    And the language server is running
    And the document "main.lua" is open
    When I request inlay hints for the first 2 lines of "main.lua"
    Then an inlay hint at 0:11 reads ": { x: integer, y: integer }"

  Scenario: an annotated return renders verbatim, even for a cross-file class
    Given a file "main.lua" containing:
      """
      local Rect = {}
      Rect.__index = Rect

      ---@param width number
      ---@param height number
      ---@return Rect
      function Rect.new(width, height)
        return setmetatable({ width = width, height = height }, Rect)
      end
      """
    And the language server is running
    And the document "main.lua" is open
    When I request inlay hints for the first 9 lines of "main.lua"
    Then an inlay hint at 6:32 reads ": Rect"
    And an inlay hint at 6:23 reads ": number"

  Scenario: a local function name gets no binding hint, only a return hint
    Given a file "main.lua" containing:
      """
      local function helper()
        return 1
      end
      local result = helper()
      return result
      """
    And the language server is running
    And the document "main.lua" is open
    When I request inlay hints for the first 5 lines of "main.lua"
    Then no inlay hint sits at 0:21
    And an inlay hint at 0:23 reads ": integer"
    And an inlay hint at 3:12 reads ": integer"

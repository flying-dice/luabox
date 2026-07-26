Feature: luabox lsp — hover
  SPEC.md §8 — hovering an identifier renders it as a `lua` code block: the
  binding's type, a function's resolved signature, or a class field's
  declared type, followed by the LuaCATS doc text attached to it.

  Background:
    Given a project with edition "5.4"

  Scenario: hovering an annotated local shows its type and doc text
    Given a file "main.lua" containing:
      """
      ---the answer
      ---@type number
      local answer = 42
      print(answer)
      """
    And the language server is running
    And the document "main.lua" is open
    When I hover at 3:8 in "main.lua"
    Then the hover text contains "local answer: number"
    And the hover text contains "the answer"

  Scenario: hovering a call site shows the callee's signature
    Given a file "main.lua" containing:
      """
      ---Stringify a number.
      ---@param n number
      ---@return string
      local function stringify(n) return tostring(n) end
      stringify(1)
      """
    And the language server is running
    And the document "main.lua" is open
    When I hover at 4:3 in "main.lua"
    Then the hover text contains "function stringify(n: number): string"
    And the hover text contains "Stringify a number."

  Scenario: hovering a class field shows the field type and its description
    Given a file "main.lua" containing:
      """
      ---@class Point
      ---@field x number horizontal
      ---@field y number

      ---@type Point
      local p = nil
      print(p.x)
      """
    And the language server is running
    And the document "main.lua" is open
    When I hover at 6:8 in "main.lua"
    Then the hover text contains "Point.x: number"
    And the hover text contains "horizontal"

  Scenario: hover renders a single see-reference inline
    Given a file "main.lua" containing:
      """
      ---Frobnicates.
      ---@see other.frob
      local function frob() end
      frob()
      """
    And the language server is running
    And the document "main.lua" is open
    When I hover at 3:1 in "main.lua"
    Then the hover text contains "See: other.frob"

  Scenario: hovering an empty position answers null rather than guessing
    Given a file "main.lua" containing:
      """
      local answer = 42
      """
    And the language server is running
    And the document "main.lua" is open
    When I hover at 1:0 in "main.lua"
    Then the reply is null

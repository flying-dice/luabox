Feature: luabox lsp — signature help
  SPEC.md §8 — while the cursor sits inside a call's argument list the
  server renders the callee's resolved signature, tracks which parameter is
  active as commas are typed, and surfaces the `---@param` doc text. Every
  `---@overload` is offered as its own signature. Outside a call there is
  nothing to show.

  Background:
    Given a project with edition "5.4"

  Scenario: the signature, its parameters, and their docs are rendered
    Given a file "main.lua" containing:
      """
      ---@param a number the first arg
      ---@param b string
      local function f(a, b) end
      f(1, "two")
      """
    And the language server is running
    And the document "main.lua" is open
    When I request signature help at 3:2 in "main.lua"
    Then the signature labels are "f(a: number, b: string)"
    And the active parameter is 0
    And parameter 0 documents "the first arg"

  Scenario: the active parameter advances past a comma
    Given a file "main.lua" containing:
      """
      ---@param a number
      ---@param b string
      local function f(a, b) end
      f(1, "two")
      """
    And the language server is running
    And the document "main.lua" is open
    When I request signature help at 3:4 in "main.lua"
    Then the active parameter is 1

  Scenario: extra arguments clamp to the last declared parameter
    Given a file "main.lua" containing:
      """
      ---@param a number
      local function f(a) end
      f(1, 2, 3)
      """
    And the language server is running
    And the document "main.lua" is open
    When I request signature help at 2:8 in "main.lua"
    Then the active parameter is 0

  Scenario: a method call resolves through the receiver's class
    Given a file "main.lua" containing:
      """
      ---@class Point
      ---@field translate fun(dx: number, dy: number): Point

      ---@type Point
      local p = nil
      p:translate(1, 2)
      """
    And the language server is running
    And the document "main.lua" is open
    When I request signature help at 5:12 in "main.lua"
    Then the signature labels are "Point:translate(dx: number, dy: number): Point"
    And the active parameter is 0

  Scenario: an overloaded function offers every signature
    Given a file "main.lua" containing:
      """
      ---@param a number
      ---@overload fun(a: string): boolean
      local function f(a) end
      f(1)
      """
    And the language server is running
    And the document "main.lua" is open
    When I request signature help at 3:2 in "main.lua"
    Then the signature labels are "f(a: number) | f(a: string): boolean"

  Scenario: outside any call there is no signature to show
    Given a file "main.lua" containing:
      """
      local x = 1
      """
    And the language server is running
    And the document "main.lua" is open
    When I request signature help at 0:8 in "main.lua"
    Then the reply is null

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

  Scenario: hovering a parameter marks it as a param
    Given a file "main.lua" containing:
      """
      ---@param n number
      local function f(n) return n end
      return f
      """
    And the language server is running
    And the document "main.lua" is open
    When I hover at 1:17 in "main.lua"
    Then the hover text contains "(param) n: number"

  Scenario: hovering a loop variable marks it as a for binding
    Given a file "main.lua" containing:
      """
      for i = 1, 10 do print(i) end
      """
    And the language server is running
    And the document "main.lua" is open
    When I hover at 0:23 in "main.lua"
    Then the hover text contains "(for) i: unknown"

  Scenario: hovering the implicit self of a method marks it as a param
    Given a file "main.lua" containing:
      """
      ---@class Greeter
      local G = {}
      function G:greet() return self end
      """
    And the language server is running
    And the document "main.lua" is open
    When I hover at 2:26 in "main.lua"
    Then the hover text contains "(param) self:"

  Scenario: hovering a local function name shows its signature, not its binding
    Given a file "main.lua" containing:
      """
      ---Doubles.
      ---@param n number
      ---@return number
      local function double(n) return n * 2 end
      print(double(2))
      """
    And the language server is running
    And the document "main.lua" is open
    When I hover at 4:8 in "main.lua"
    Then the hover text contains "function double(n: number): number"
    And the hover text contains "Doubles."
    And the hover text does not contain "local double"

  Scenario: hovering a dotted function falls back to its signature
    Given a file "main.lua" containing:
      """
      local M = {}
      ---Helps.
      ---@param n number
      ---@return string
      function M.helper(n) return tostring(n) end
      M.helper(1)
      return M
      """
    And the language server is running
    And the document "main.lua" is open
    When I hover at 5:3 in "main.lua"
    Then the hover text contains "function M.helper(n: number): string"
    And the hover text contains "Helps."

  Scenario: hovering a class name shows it as a class with its doc text
    Given a file "main.lua" containing:
      """
      ---A 2-D point.
      ---@class Point
      ---@field x number

      local alias = Point
      return alias
      """
    And the language server is running
    And the document "main.lua" is open
    When I hover at 4:14 in "main.lua"
    Then the hover text contains "class Point"
    And the hover text contains "A 2-D point."

  Scenario: hovering an optional field marks it with a question mark
    Given a file "main.lua" containing:
      """
      ---@class Config
      ---@field debug? boolean

      ---@type Config
      local cfg = nil
      print(cfg.debug)
      """
    And the language server is running
    And the document "main.lua" is open
    When I hover at 5:11 in "main.lua"
    Then the hover text contains "(field) Config.debug?: boolean"

  Scenario: hovering the receiver shows its own binding, not the field
    Given a file "main.lua" containing:
      """
      ---@class Point
      ---@field x number

      ---@type Point
      local p = nil
      print(p.x)
      """
    And the language server is running
    And the document "main.lua" is open
    When I hover at 5:6 in "main.lua"
    Then the hover text contains "local p: Point"

  Scenario: hovering an alias-typed local keeps the alias name
    Given a file "main.lua" containing:
      """
      ---@alias Id string

      ---@type Id
      local key = nil
      print(key)
      """
    And the language server is running
    And the document "main.lua" is open
    When I hover at 4:6 in "main.lua"
    Then the hover text contains "local key: Id"

  Scenario: hovering a literal-typed local renders the literal
    Given a file "main.lua" containing:
      """
      ---@type 42
      local answer = nil
      print(answer)
      """
    And the language server is running
    And the document "main.lua" is open
    When I hover at 2:6 in "main.lua"
    Then the hover text contains "local answer: 42"

  Scenario: an unannotated local renders as unknown
    Given a file "main.lua" containing:
      """
      local thing = 42
      print(thing)
      """
    And the language server is running
    And the document "main.lua" is open
    When I hover at 1:8 in "main.lua"
    Then the hover text contains "local thing: unknown"

  Scenario: several see-references render as a bullet list
    Given a file "main.lua" containing:
      """
      ---Frobnicates.
      ---@see other.frob
      ---@see also.this
      local function frob() end
      frob()
      return frob
      """
    And the language server is running
    And the document "main.lua" is open
    When I hover at 4:1 in "main.lua"
    Then the hover text contains "* other.frob"
    And the hover text contains "* also.this"

  Scenario: the hover range covers exactly the identifier
    Given a file "main.lua" containing:
      """
      local answer = 42
      print(answer)
      """
    And the language server is running
    And the document "main.lua" is open
    When I hover at 1:8 in "main.lua"
    Then the hover range spans 1:6 to 1:12

  Scenario: a table-constructor key is neither a use nor a binding
    Given a file "main.lua" containing:
      """
      local t = { key = 1 }
      return t
      """
    And the language server is running
    And the document "main.lua" is open
    When I hover at 0:12 in "main.lua"
    Then the reply is null

  Scenario: an undeclared global has nothing to describe
    Given a file "main.lua" containing:
      """
      print(nothing_here)
      """
    And the language server is running
    And the document "main.lua" is open
    When I hover at 0:6 in "main.lua"
    Then the reply is null

  Scenario: a field the class does not declare has nothing to describe
    Given a file "main.lua" containing:
      """
      ---@class Point
      ---@field x number

      ---@type Point
      local p = nil
      print(p.z)
      """
    And the language server is running
    And the document "main.lua" is open
    When I hover at 5:8 in "main.lua"
    Then the reply is null

Feature: luabox lsp — completion
  SPEC.md §8 — after `.` or `:` on a value whose class is known, completion
  offers that class's fields and methods; in a plain position it offers the
  visible scope plus **auto-require imports**, each carrying the
  `local x = require("m").x` insert that brings the name into scope.

  Background:
    Given a project with edition "5.4"

  Scenario: completing after a dot offers the class fields
    Given a file "main.lua" containing:
      """
      ---@class Point
      ---@field x number
      ---@field y number
      ---@field translate fun(dx: number): Point

      ---@type Point
      local p = nil
      print(p.
      """
    And the language server is running
    And the document "main.lua" is open
    When I request completion at 7:8 in "main.lua"
    Then the completion list contains "x"
    And the completion list contains "y"
    And the completion list contains "translate"
    And completion item "x" has detail "Point.x: number"

  Scenario: member completion never mixes in keywords
    Given a file "main.lua" containing:
      """
      ---@class Point
      ---@field x number

      ---@type Point
      local p = nil
      print(p.
      """
    And the language server is running
    And the document "main.lua" is open
    When I request completion at 5:8 in "main.lua"
    Then the completion list does not contain "local"

  Scenario: completing after a colon offers only methods
    Given a file "main.lua" containing:
      """
      ---@class Point
      ---@field x number
      ---@field translate fun(dx: number): Point

      ---@type Point
      local p = nil
      p:
      """
    And the language server is running
    And the document "main.lua" is open
    When I request completion at 6:2 in "main.lua"
    Then the completion list contains "translate"
    And the completion list does not contain "x"
    And every completion item is a method

  Scenario: a plain position offers locals, functions, and keywords
    Given a file "main.lua" containing:
      """
      local alpha = 1
      local function beta() end
      al
      """
    And the language server is running
    And the document "main.lua" is open
    When I request completion at 2:2 in "main.lua"
    Then the completion list contains "alpha"
    And the completion list contains "beta"
    And the completion list contains "local"

  Scenario: a name exported by another module is offered with a require insert
    Given a file "a/b/c.lua" containing:
      """
      local M = {}
      function M.greet() end
      return M
      """
    And a file "main.lua" containing:
      """
      gr
      """
    And the language server is running
    And the document "main.lua" is open
    When I request completion at 0:2 in "main.lua"
    Then the completion list contains "greet"
    And completion item "greet" imports it from module "a.b.c"
    And the import for "greet" is inserted at 0:0

  Scenario: the import lands after the requires the file already has
    Given a file "dep.lua" containing:
      """
      local M = {}
      function M.helper() end
      return M
      """
    And a file "a/b/c.lua" containing:
      """
      local M = {}
      function M.greet() end
      return M
      """
    And a file "main.lua" containing:
      """
      local d = require("dep")
      gr
      """
    And the language server is running
    And the document "main.lua" is open
    When I request completion at 1:2 in "main.lua"
    Then completion item "greet" imports it from module "a.b.c"
    And the import for "greet" is inserted at 1:0

  Scenario: a module that is already required is not auto-imported again
    Given a file "a/b/c.lua" containing:
      """
      local M = {}
      function M.greet() end
      return M
      """
    And a file "main.lua" containing:
      """
      local m = require("a.b.c")
      gr
      """
    And the language server is running
    And the document "main.lua" is open
    When I request completion at 1:2 in "main.lua"
    Then the completion list does not contain "greet"

  Scenario: a name already in scope wins over the auto-import
    Given a file "a/b/c.lua" containing:
      """
      local M = {}
      function M.greet() end
      return M
      """
    And a file "main.lua" containing:
      """
      local greet = 1
      gr
      """
    And the language server is running
    And the document "main.lua" is open
    When I request completion at 1:2 in "main.lua"
    Then the completion list contains "greet"
    And completion item "greet" inserts nothing extra

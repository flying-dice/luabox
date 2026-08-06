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

  # The receiver's own class is declared in *this* file, but inherits from
  # a parent declared in another — completion used to answer only from the
  # file-local class, silently dropping any inherited member (#47).
  Scenario: a member inherited from a parent in another file is offered
    Given a file "base.lua" containing:
      """
      ---@class Base
      ---@field id number
      """
    And a file "main.lua" containing:
      """
      ---@class Sub : Base
      ---@field name string

      ---@type Sub
      local s = nil
      s.
      """
    And the language server is running
    And the document "main.lua" is open
    When I request completion at 5:2 in "main.lua"
    Then the completion list contains "id"
    And the completion list contains "name"

  # A bound generic reference resolves its type argument (#48): the checker
  # monomorphises `Box<number>` at the reference site, so completion must
  # detail `item` as `number`, not the free `T` a bare-name lookup leaves it.
  Scenario: a bound generic class's member is offered with the bound type
    Given a file "box.lua" containing:
      """
      ---@class Box<T>
      ---@field item T
      """
    And a file "main.lua" containing:
      """
      ---@type Box<number>
      local b = nil
      b.
      """
    And the language server is running
    And the document "main.lua" is open
    When I request completion at 2:2 in "main.lua"
    Then the completion list contains "item"
    And completion item "item" has detail "Box.item: number"

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

  # A `---@class` carrier export crosses `require` as the class it carries
  # (#56), not a structural table — `module_export` reifies it to
  # `Ty::Named`, and auto-require used to match on `Ty::Table` alone, so a
  # carrier module's names silently stopped being offered (#49).
  Scenario: a class carrier module's members are offered with a require insert
    Given a file "widget.lua" containing:
      """
      ---@class Widget
      local W = {}
      function W.make() end
      return W
      """
    And a file "main.lua" containing:
      """
      mak
      """
    And the language server is running
    And the document "main.lua" is open
    When I request completion at 0:3 in "main.lua"
    Then the completion list contains "make"
    And completion item "make" imports it from module "widget"

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

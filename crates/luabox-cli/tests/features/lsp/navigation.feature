Feature: luabox lsp — goto definition, type definition, and implementation
  SPEC.md §8 — the three navigation requests. Definition follows a value to
  where it was bound (including a `require` to the module file it resolves
  to); type definition follows a value to the `---@class`/`---@alias` that
  names its type; implementation follows a class to its subclasses. Named
  types and inheritance are workspace-global, so all three cross files.

  Background:
    Given a project with edition "5.4"

  Scenario: definition jumps from a use to the declaration
    Given a file "main.lua" containing:
      """
      local value = 1
      print(value)
      """
    And the language server is running
    And the document "main.lua" is open
    When I request the definition at 1:8 in "main.lua"
    Then the location is in "main.lua"
    And the location starts at 0:6

  Scenario: definition resolves a require to the module file
    Given a file "util/helpers.lua" containing:
      """
      return {}
      """
    And a file "main.lua" containing:
      """
      local h = require("util.helpers")
      return h
      """
    And the language server is running
    And the document "main.lua" is open
    When I request the definition at 0:22 in "main.lua"
    Then the location is in "util/helpers.lua"

  Scenario: definition follows an imported plain table function to its exporting module
    Given a file "src/greeter.lua" containing:
      """
      -- No class annotation: an ordinary exported Lua table.

      local M = {}
      function M.greet() return "hello" end
      return M
      """
    And a file "main.lua" containing:
      """
      local g = require("greeter")
      print(g.greet())
      """
    And the language server is running
    And the document "main.lua" is open
    When I request the definition at 1:9 in "main.lua"
    Then the location is in "src/greeter.lua"
    And the location starts at 3:11

  Scenario: definition follows assignment members in an unsaved exporting buffer
    Given a file "src/greeter.lua" containing:
      """
      return {}
      """
    And a file "main.lua" containing:
      """
      local g = require("greeter")
      print(g.greet())
      """
    And the language server is running
    And the document "main.lua" is open
    And the document "src/greeter.lua" is open
    When I change "src/greeter.lua" to:
      """
      -- Unsaved editor contents, not the disk's empty table.
      local M = {}
      M.greet = function() return "hello" end
      return M
      """
    And I request the definition at 1:9 in "main.lua"
    Then the location is in "src/greeter.lua"
    And the location starts at 2:2

  Scenario: definition follows a directly returned table literal member
    Given a file "greeter.lua" containing:
      """
      return { greet = function() return "hello" end }
      """
    And a file "main.lua" containing:
      """
      local g = require("greeter")
      print(g.greet())
      """
    And the language server is running
    And the document "main.lua" is open
    When I request the definition at 1:9 in "main.lua"
    Then the location is in "greeter.lua"
    And the location starts at 0:9

  Scenario: definition follows a ---@source redirect on a file's only statement
    Given a file "main.lua" containing:
      """
      ---@source native/impl.c:12
      local function f() end
      """
    And the language server is running
    And the document "main.lua" is open
    When I request the definition at 1:15 in "main.lua"
    Then the location is in "native/impl.c"
    And the location starts at 11:0

  Scenario: type definition jumps from a typed local to its class
    Given a file "main.lua" containing:
      """
      ---@class Point
      ---@field x number

      ---@type Point
      local p = nil
      print(p)
      """
    And the language server is running
    And the document "main.lua" is open
    When I request the type definition at 5:6 in "main.lua"
    Then the location is in "main.lua"
    And the location starts on line 0

  Scenario: type definition jumps from an alias-typed local to the alias
    Given a file "main.lua" containing:
      """
      ---@alias Id string

      ---@type Id
      local key = nil
      print(key)
      """
    And the language server is running
    And the document "main.lua" is open
    When I request the type definition at 4:6 in "main.lua"
    Then the location starts on line 0

  Scenario: type definition crosses files to the declaring module
    Given a file "point.lua" containing:
      """
      ---@class Point
      ---@field x number
      """
    And a file "main.lua" containing:
      """
      ---@type Point
      local p = nil
      print(p)
      """
    And the language server is running
    And the document "main.lua" is open
    When I request the type definition at 2:6 in "main.lua"
    Then the location is in "point.lua"

  Scenario: a primitive-typed value has no type definition to jump to
    Given a file "main.lua" containing:
      """
      ---@type number
      local n = 1
      print(n)
      """
    And the language server is running
    And the document "main.lua" is open
    When I request the type definition at 2:6 in "main.lua"
    Then the reply is null

  Scenario: implementation lists every subclass of an interface
    Given a file "base.lua" containing:
      """
      ---@class Base
      ---@field id number
      """
    And a file "derived.lua" containing:
      """
      ---@class Derived : Base
      """
    And a file "other.lua" containing:
      """
      ---@class Other : Base
      """
    And the language server is running
    And the document "base.lua" is open
    When I request implementations at 0:10 in "base.lua"
    Then the reply lists 2 locations
    And 1 of the locations are in "derived.lua"
    And 1 of the locations are in "other.lua"

  Scenario: a class nobody extends has no implementations
    Given a file "base.lua" containing:
      """
      ---@class Base
      """
    And the language server is running
    And the document "base.lua" is open
    When I request implementations at 0:10 in "base.lua"
    Then the reply is an empty list

  Scenario: definition on a declaration answers with itself
    Given a file "main.lua" containing:
      """
      local value = 1
      print(value)
      """
    And the language server is running
    And the document "main.lua" is open
    When I request the definition at 0:6 in "main.lua"
    Then the location is in "main.lua"
    And the location starts at 0:6

  Scenario: definition on a position that names nothing answers null
    Given a file "main.lua" containing:
      """
      local value = 1
      print(value)
      """
    And the language server is running
    And the document "main.lua" is open
    When I request the definition at 0:12 in "main.lua"
    Then the reply is null

  Scenario: definition on a class field jumps to its field annotation
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
    When I request the definition at 5:8 in "main.lua"
    Then the location is in "main.lua"
    And the location starts on line 1

  Scenario: definition on a method jumps to the field that declares it
    Given a file "main.lua" containing:
      """
      ---@class Greeter
      ---@field greet fun(self: Greeter): string

      ---@type Greeter
      local g = nil
      g:greet()
      """
    And the language server is running
    And the document "main.lua" is open
    When I request the definition at 5:3 in "main.lua"
    Then the location starts on line 1

  # A receiver's own class declared in *this* file, inheriting from a
  # parent declared in another — the shape the file-local-only member
  # lookup used to answer null for, though `luabox check` resolved the
  # member fine through the merged workspace ambient (#46, #54).
  Scenario: definition on a member inherited from a parent in another file jumps to the parent
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
      print(s.id)
      """
    And the language server is running
    And the document "main.lua" is open
    When I request the definition at 5:8 in "main.lua"
    Then the location is in "base.lua"

  Scenario: definition on a dotted function jumps to its declaration
    Given a file "main.lua" containing:
      """
      local M = {}
      function M.helper() return 1 end
      M.helper()
      """
    And the language server is running
    And the document "main.lua" is open
    When I request the definition at 2:3 in "main.lua"
    Then the location starts on line 1

  Scenario: definition on a global function use jumps to its declaration
    Given a file "main.lua" containing:
      """
      function greet() return 1 end
      greet()
      """
    And the language server is running
    And the document "main.lua" is open
    When I request the definition at 1:0 in "main.lua"
    Then the location starts on line 0

  Scenario: a require resolves through the src directory
    Given a file "src/util/text.lua" containing:
      """
      return {}
      """
    And a file "main.lua" containing:
      """
      local t = require("util.text")
      return t
      """
    And the language server is running
    And the document "main.lua" is open
    When I request the definition at 0:22 in "main.lua"
    Then the location is in "src/util/text.lua"

  Scenario: a require nothing resolves has no definition
    Given a file "main.lua" containing:
      """
      local missing = require("nope.not_here")
      return missing
      """
    And the language server is running
    And the document "main.lua" is open
    When I request the definition at 0:28 in "main.lua"
    Then the reply is null

  Scenario: a ---@source with no line defaults to the top of the file
    Given a file "main.lua" containing:
      """
      ---@source native/impl.c
      local function f() end
      return f
      """
    And the language server is running
    And the document "main.lua" is open
    When I request the definition at 1:15 in "main.lua"
    Then the location is in "native/impl.c"
    And the location spans 0:0 to 0:0

  Scenario: a ---@source line and column both land on the redirect target
    Given a file "main.lua" containing:
      """
      ---@source native/impl.c:12:4
      local function f() end
      return f
      """
    And the language server is running
    And the document "main.lua" is open
    When I request the definition at 1:15 in "main.lua"
    Then the location is in "native/impl.c"
    And the location spans 11:4 to 11:4

  Scenario: a ---@source naming a URI is used verbatim
    Given a file "main.lua" containing:
      """
      ---@source https://example.com/lib.lua:3
      local function f() end
      return f
      """
    And the language server is running
    And the document "main.lua" is open
    When I request the definition at 1:15 in "main.lua"
    Then the location URI is "https://example.com/lib.lua"
    And the location spans 2:0 to 2:0

  Scenario: type definition follows self to the class its method is declared on
    Given a file "main.lua" containing:
      """
      ---@class G
      ---@field name string

      local G = {}
      function G:greet() return self end
      """
    And the language server is running
    And the document "main.lua" is open
    When I request the type definition at 4:26 in "main.lua"
    Then the location is in "main.lua"
    And the location starts on line 0

  Scenario: type definition follows an inferred return type to its class
    Given a file "main.lua" containing:
      """
      ---@class Point
      ---@field x number

      ---@return Point
      local function make() return nil end
      local p = make()
      print(p)
      """
    And the language server is running
    And the document "main.lua" is open
    When I request the type definition at 6:6 in "main.lua"
    Then the location starts on line 0

  Scenario: type definition jumps from an enum-typed local to the enum
    Given a file "main.lua" containing:
      """
      ---@enum Color
      local Color = { red = 1 }

      ---@type Color
      local c = nil
      print(c)
      """
    And the language server is running
    And the document "main.lua" is open
    When I request the type definition at 5:6 in "main.lua"
    Then the location starts on line 0

  Scenario: an optional annotation still names the class it wraps
    Given a file "main.lua" containing:
      """
      ---@class Point
      ---@field x number

      ---@type Point?
      local p = nil
      print(p)
      """
    And the language server is running
    And the document "main.lua" is open
    When I request the type definition at 5:6 in "main.lua"
    Then the location starts on line 0

  Scenario: a union names no single type, so there is nowhere to jump
    Given a file "main.lua" containing:
      """
      ---@class Point
      ---@field x number

      ---@type Point|string
      local p = nil
      print(p)
      """
    And the language server is running
    And the document "main.lua" is open
    When I request the type definition at 5:6 in "main.lua"
    Then the reply is null

  Scenario: an unannotated primitive local has no type to jump to
    Given a file "main.lua" containing:
      """
      local n = 1
      print(n)
      """
    And the language server is running
    And the document "main.lua" is open
    When I request the type definition at 1:6 in "main.lua"
    Then the reply is null

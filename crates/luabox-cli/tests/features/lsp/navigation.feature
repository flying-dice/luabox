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

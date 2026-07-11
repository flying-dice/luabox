Feature: luabox check — member visibility (#115)
  SPEC.md §3 + §19 — luabox enforces LuaCATS field/method visibility the
  way lua-language-server's `invisible` diagnostic does: `---@private`
  members are reachable only from the owning class's own methods,
  `---@protected` from the class and its subclasses, and `---@package`
  only from the file that declares the class. A restricted member still
  exists, so an out-of-scope access is `invisible` (LB0312), never
  `undefined-field`. luals always warns; luabox follows its strictness
  ladder (a warning in the default project, an error under strict). The
  same `---@diagnostic disable` directive luals recognises suppresses it.

  Scenario: reading a private field from inside the class is clean
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@class Account
      ---@field private balance number
      local Account = {}
      Account.__index = Account

      function Account:total()
        return self.balance
      end

      return Account
      """
    When I run "luabox check"
    Then the command succeeds
    And stdout does not contain "LB0312"

  Scenario: reading a private field from outside the class is flagged
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@class Account
      ---@field private balance number
      local Account = {}

      ---@param a Account
      local function show(a)
        return a.balance
      end

      return show
      """
    When I run "luabox check"
    Then the command succeeds
    And stdout contains "warning[LB0312]"
    And stdout contains "private"

  Scenario: a private access is suppressed by a diagnostic directive
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@class Account
      ---@field private balance number
      local Account = {}

      ---@param a Account
      local function show(a)
        ---@diagnostic disable-next-line: invisible
        return a.balance
      end

      return show
      """
    When I run "luabox check"
    Then the command succeeds
    And stdout does not contain "LB0312"

  Scenario: a protected member is reachable from a subclass method
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@class Base
      ---@field protected token? string
      local Base = {}
      Base.__index = Base

      ---@class Child : Base
      local Child = {}
      Child.__index = Child

      function Child:reveal()
        return self.token
      end

      return Child
      """
    When I run "luabox check"
    Then the command succeeds
    And stdout does not contain "LB0312"

  Scenario: a package member declared in another file is invisible
    Given a project with edition "5.4"
    And a file "defs/widgets.d.lua" containing:
      """
      ---@meta
      ---@class Widget
      ---@field package handle number
      """
    And a file "luabox.toml" containing:
      """
      [package]
      name = "fixture"
      version = "0.1.0"
      edition = "5.4"

      [types]
      strict = false
      defs = ["widgets"]
      """
    And a file "src/main.lua" containing:
      """
      ---@param w Widget
      local function grab(w)
        return w.handle
      end

      return grab
      """
    When I run "luabox check"
    Then the command succeeds
    And stdout contains "warning[LB0312]"
    And stdout contains "package"

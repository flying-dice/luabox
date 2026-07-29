Feature: luabox check — luals-parity doc diagnostics (#111, #112, #113)
  SPEC.md §3 + §19 — luabox matches lua-language-server's advisory
  annotation diagnostics: `deprecated` (LB0308) at every use of a
  `---@deprecated` symbol, `discard-returns` (LB0309) when a
  `---@nodiscard` call's result is thrown away, `duplicate-doc-field`
  (LB0311) for a repeated `---@field`, and `duplicate-doc-alias`
  (LB0310) for a `---@alias` name declared in more than one file. All
  four are warnings (they never fail the command), suppressible via the
  same `---@diagnostic disable` directive luals recognises.

  Scenario: using a deprecated function is flagged at the call site
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@deprecated
      local function oldApi() end

      oldApi()
      """
    When I run "luabox check"
    Then the command succeeds
    And stdout contains "warning[LB0308]"
    And stdout contains "deprecated"

  Scenario: the deprecated declaration alone is not flagged
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@deprecated
      local function oldApi() end
      """
    When I run "luabox check"
    Then the command succeeds
    And stdout does not contain "LB0308"

  Scenario: a deprecated use is suppressed by a diagnostic directive
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@deprecated
      local function oldApi() end

      ---@diagnostic disable-next-line: deprecated
      oldApi()
      """
    When I run "luabox check"
    Then the command succeeds
    And stdout does not contain "LB0308"

  Scenario: a deprecated method is flagged at a `:` call site
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@class Widget
      local Widget = {}
      Widget.__index = Widget

      ---@deprecated
      function Widget:legacy() end

      ---@return Widget
      function Widget.new()
        return setmetatable({}, Widget)
      end

      Widget.new():legacy()
      """
    When I run "luabox check"
    Then the command succeeds
    And stdout contains "warning[LB0308]"
    And stdout contains "use of deprecated `legacy`"

  Scenario: a deprecated method on a plain prototype is flagged at a `:` call site
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      local Proto = {}
      Proto.__index = Proto

      ---@deprecated
      function Proto:legacy() end

      local p = setmetatable({}, Proto)
      p:legacy()
      """
    When I run "luabox check"
    Then the command succeeds
    And stdout contains "warning[LB0308]"
    And stdout contains "use of deprecated `legacy`"

  Scenario: a `---@field`-declared method still carries its carrier's `---@deprecated`
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@class Decl
      ---@field legacy fun(self: Decl)
      local Decl = {}
      Decl.__index = Decl

      ---@deprecated
      function Decl:legacy() end

      ---@type Decl
      local d
      d:legacy()
      """
    When I run "luabox check"
    Then the command succeeds
    And stdout contains "warning[LB0308]"
    And stdout contains "use of deprecated `legacy`"

  Scenario: a `---@class` carrier with no `__index` link still carries `---@deprecated` (#33)
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@class Plain
      local Plain = {}

      ---@deprecated
      function Plain:legacy() end

      ---@type Plain
      local p
      p:legacy()
      """
    When I run "luabox check"
    Then the command succeeds
    And stdout contains "warning[LB0308]"
    And stdout contains "use of deprecated `legacy`"
    And stdout does not contain "LB0306"

  Scenario: a `---@type fun(...)` assignment carries `---@deprecated` to a `:` call site (#38)
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@class Cls
      local Cls = {}

      ---@deprecated
      ---@type fun(self: Cls, n: integer)
      Cls.m = function(self, n) end

      ---@type Cls
      local o
      o:m(1)
      """
    When I run "luabox check"
    Then the command succeeds
    And stdout contains "warning[LB0308]"
    And stdout contains "use of deprecated `m`"

  Scenario: a carrier-style method in a defs file carries `---@deprecated` to its use site (#39)
    Given a file "defs/game.d.lua" containing:
      """
      ---@meta

      ---@class Widget
      local Widget = {}

      ---@deprecated
      ---@param n integer
      function Widget:render(n) end
      """
    And a file "luabox.toml" containing:
      """
      [package]
      name = "fixture"
      version = "0.1.0"
      edition = "5.4"

      [types]
      strict = true
      defs = ["game"]
      """
    And a file "src/main.lua" containing:
      """
      ---@param w Widget
      local function use(w)
        w:render(1)
      end
      return use
      """
    When I run "luabox check"
    Then the command succeeds
    And stdout contains "warning[LB0308]"
    And stdout does not contain "LB0306"

  Scenario: a method with no deprecation tag keeps the `:` call site clean
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      local Proto = {}
      Proto.__index = Proto

      function Proto:current() end

      local p = setmetatable({}, Proto)
      p:current()
      """
    When I run "luabox check"
    Then the command succeeds
    And stdout does not contain "LB0308"

  Scenario: discarding a nodiscard return is flagged
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@nodiscard
      ---@return boolean
      local function saveState() return true end

      saveState()
      """
    When I run "luabox check"
    Then the command succeeds
    And stdout contains "warning[LB0309]"

  Scenario: using the nodiscard return keeps the check clean
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@nodiscard
      ---@return boolean
      local function saveState() return true end

      local ok = saveState()
      print(ok)
      """
    When I run "luabox check"
    Then the command succeeds
    And stdout does not contain "LB0309"

  Scenario: a repeated field on a class is flagged
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@class Point
      ---@field x number
      ---@field y number
      ---@field x integer
      local Point = {}
      return Point
      """
    When I run "luabox check"
    Then the command succeeds
    And stdout contains "warning[LB0311]"
    And stdout contains "duplicate field `x`"

  Scenario: the same alias in two files is flagged at the losing site
    Given a project with edition "5.4"
    And a file "src/a.lua" containing:
      """
      ---@alias Id integer
      return {}
      """
    And a file "src/b.lua" containing:
      """
      ---@alias Id string
      return {}
      """
    When I run "luabox check"
    Then the command succeeds
    And stdout contains "warning[LB0310]"
    And stdout contains "declared more than once"

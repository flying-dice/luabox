Feature: luabox lsp — published diagnostics
  SPEC.md §8 — the editor never asks for diagnostics; the server pushes
  `textDocument/publishDiagnostics` after every open, change, and close.
  The findings are the ones `luabox check` and `luabox lint` report, with
  the same codes, severities, and — because the project manifest is read at
  startup — the same strictness.

  Scenario: opening a file with a type error publishes it at the argument
    Given a project with edition "5.4"
    And a file "main.lua" containing:
      """
      ---@param n number
      local function f(n) end
      f("no")
      """
    And the language server is running
    When I open "main.lua"
    Then the diagnostics for "main.lua" include LB0300
    And diagnostic LB0300 in "main.lua" spans 2:2 to 2:6
    And diagnostic LB0300 in "main.lua" comes from "luabox"

  Scenario: a strict project publishes the same mismatch as an error
    Given a strict project with edition "5.4"
    And a file "main.lua" containing:
      """
      ---@param n number
      local function f(n) end
      f("no")
      """
    And the language server is running
    When I open "main.lua"
    Then diagnostic LB0300 in "main.lua" is an error

  Scenario: without strict types the mismatch is only a warning
    Given a project with edition "5.4"
    And a file "main.lua" containing:
      """
      ---@param n number
      local function f(n) end
      f("no")
      """
    And the language server is running
    When I open "main.lua"
    Then diagnostic LB0300 in "main.lua" is a warning

  Scenario: an edit that fixes the error clears the diagnostics
    Given a project with edition "5.4"
    And a file "main.lua" containing:
      """
      ---@param n number
      local function f(n) end
      f("no")
      """
    And the language server is running
    And the document "main.lua" is open
    When I change "main.lua" to:
      """
      ---@param n number
      local function f(n) end
      f(1)
      """
    Then the diagnostics for "main.lua" are empty

  Scenario: an incremental edit introduces the error at the spliced range
    Given a project with edition "5.4"
    And a file "main.lua" containing:
      """
      ---@param n number
      local function f(n) end
      f(1)
      """
    And the language server is running
    And the document "main.lua" is open
    When I replace 2:2 through 2:3 of "main.lua" with "'no'"
    Then the diagnostics for "main.lua" include LB0300
    And diagnostic LB0300 in "main.lua" spans 2:2 to 2:6

  Scenario: closing a buffer reverts the diagnostics to the file on disk
    Given a project with edition "5.4"
    And a file "main.lua" containing:
      """
      ---@param n number
      local function f(n) end
      f(1)
      """
    And the language server is running
    And the document "main.lua" is open
    And I change "main.lua" to:
      """
      ---@param n number
      local function f(n) end
      f("no")
      """
    When I close "main.lua"
    Then the diagnostics for "main.lua" are empty

  Scenario: a parse error is published as LB0001
    Given a project with edition "5.4"
    And a file "broken.lua" containing:
      """
      local = 1
      """
    And the language server is running
    When I open "broken.lua"
    Then the diagnostics for "broken.lua" include LB0001
    And diagnostic LB0001 in "broken.lua" is an error

  Scenario: a lint finding is published under its own source
    Given a project with edition "5.4"
    And a file "main.lua" containing:
      """
      local unused = 1
      """
    And the language server is running
    When I open "main.lua"
    Then the diagnostics for "main.lua" include LB0501
    And diagnostic LB0501 in "main.lua" is a warning
    And diagnostic LB0501 in "main.lua" comes from "luabox-lint"
    And diagnostic LB0501 in "main.lua" spans 0:6 to 0:12

  Scenario: control-flow legality is published as an error, not as a lint (#44)
    Given a project with edition "5.4"
    And a file "main.lua" containing:
      """
      local function f()
        goto nowhere
      end
      return f
      """
    And the language server is running
    When I open "main.lua"
    Then the diagnostics for "main.lua" include LB0020
    And diagnostic LB0020 in "main.lua" is an error
    And diagnostic LB0020 in "main.lua" comes from "luabox"
    And diagnostic LB0020 in "main.lua" spans 1:7 to 1:14

  Scenario: `break` outside a loop is published in the editor too
    Given a project with edition "5.4"
    And a file "main.lua" containing:
      """
      for i = 1, 3 do end
      break
      """
    And the language server is running
    When I open "main.lua"
    Then the diagnostics for "main.lua" include LB0022
    And diagnostic LB0022 in "main.lua" is an error
    And diagnostic LB0022 in "main.lua" comes from "luabox"

  Scenario: a luabox-ignore comment suppresses the lint finding in the editor
    Given a project with edition "5.4"
    And a file "main.lua" containing:
      """
      local unused = 1 ---@luabox-ignore unused-local intentional
      """
    And the language server is running
    When I open "main.lua"
    Then the diagnostics for "main.lua" do not include LB0501

  Scenario: using a deprecated function is flagged at the call site
    Given a project with edition "5.4"
    And a file "main.lua" containing:
      """
      ---@deprecated
      local function oldApi() end

      oldApi()
      """
    And the language server is running
    When I open "main.lua"
    Then the diagnostics for "main.lua" include LB0308
    And diagnostic LB0308 in "main.lua" is a warning

  Scenario: the deprecated declaration alone is not flagged
    Given a project with edition "5.4"
    And a file "main.lua" containing:
      """
      ---@deprecated
      local function oldApi() end
      """
    And the language server is running
    When I open "main.lua"
    Then the diagnostics for "main.lua" do not include LB0308

  Scenario: a [lint] key naming no known rule is logged, not silently ignored
    # A config problem belongs to no document, so it has no URI to publish
    # against — it goes to the client's log pane instead (CC-M8).
    Given a project with edition "5.4" and lint rule "unused-locl" set to "allow"
    And a file "main.lua" containing:
      """
      local x = 1
      """
    And the language server is running
    When I open "main.lua"
    Then the server logged a warning containing "unknown lint rule id `unused-locl` in `[lint]`"
    And the server logged a warning containing "did you mean `unused-local`?"
    # ...and the rule the entry failed to silence still publishes.
    And the diagnostics for "main.lua" include LB0501

  Scenario: a correctly spelled [lint] key logs nothing
    Given a project with edition "5.4" and lint rule "unused-local" set to "allow"
    And a file "main.lua" containing:
      """
      local x = 1
      """
    And the language server is running
    When I open "main.lua"
    Then the server logged nothing containing "unknown lint rule id"
    And the diagnostics for "main.lua" do not include LB0501

  Scenario: a carrier-style method in a defs file resolves in the editor too (#39)
    # Diagnostics flow through the same seam as `luabox check`, so the defs
    # surface an editor sees is the surface the CLI checks: no phantom
    # `undefined field`, and the method's own tag still publishes.
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
    And a file "main.lua" containing:
      """
      ---@param w Widget
      local function use(w)
        w:render(1)
      end
      return use
      """
    And the language server is running
    When I open "main.lua"
    Then the diagnostics for "main.lua" do not include LB0306
    And the diagnostics for "main.lua" include LB0308

  # --- types from a bare luarocks tree (#30, decisions/09) ---------------
  # The server harvests the installed rock sources' LuaCATS surfaces at
  # startup, so a rock's classes and module return types resolve in the
  # editor exactly as they do under `luabox check` — with no manifest
  # declaration at all. Vendored bodies are still never checked.

  Scenario: a rock class from a bare lua_modules tree resolves in the editor
    Given a strict project with edition "5.4"
    And a file "lua_modules/share/lua/5.4/mylib/init.lua" containing:
      """
      ---@class mylib.Point
      ---@field x number
      ---@field y number

      local M = {}

      ---@param x number
      ---@param y number
      ---@return mylib.Point
      function M.point(x, y)
        return { x = x, y = y }
      end

      return M
      """
    And a file "main.lua" containing:
      """
      ---@type mylib.Point
      local p = { x = 1, y = 2 }
      return p
      """
    And the language server is running
    When I open "main.lua"
    Then the diagnostics for "main.lua" are empty

  Scenario: a rock module's return type reaches the editor's use site
    Given a strict project with edition "5.4"
    And a file "lua_modules/share/lua/5.4/mylib/init.lua" containing:
      """
      ---@class mylib.Point
      ---@field x number
      ---@field y number

      local M = {}

      ---@param x number
      ---@param y number
      ---@return mylib.Point
      function M.point(x, y)
        return { x = x, y = y }
      end

      return M
      """
    And a file "main.lua" containing:
      """
      local mylib = require("mylib")
      local p = mylib.point(1, 2)
      return p.nope
      """
    And the language server is running
    When I open "main.lua"
    Then the diagnostics for "main.lua" include LB0306

  Scenario: a rock source that does not parse publishes nothing
    Given a strict project with edition "5.4"
    And a file "lua_modules/share/lua/5.4/broken/init.lua" containing:
      """
      ---@class broken.Thing
      local = = =
      """
    And a file "main.lua" containing:
      """
      return 1
      """
    And the language server is running
    When I open "main.lua"
    Then the diagnostics for "main.lua" are empty

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

  # Shockwave round 3: `pl.lua` and `pl/init.lua` both answer to the module
  # `pl`, and the two frontends key the harvest differently — `luabox check`
  # by path, the server by module name. They agree only if the rock walk hands
  # files back in `require`-candidate order (flat `<rel>.lua` first). It did
  # not: `Vec<PathBuf>::sort` is component-wise and put the directory first,
  # so the SAME source got opposite verdicts in CI and in the editor. The
  # `luabox check` half of this pair lives in
  # `features/frontend/luarocks-tree.feature`.
  Scenario: a flat rock module beats its init form, the way `require` resolves it
    Given a strict project with edition "5.4"
    And a file "lua_modules/share/lua/5.4/pl.lua" containing:
      """
      ---@type number
      local flat = 1
      return flat
      """
    And a file "lua_modules/share/lua/5.4/pl/init.lua" containing:
      """
      ---@type string
      local init = "s"
      return init
      """
    And a file "main.lua" containing:
      """
      local pl = require("pl")
      ---@param s string
      local function want(s) return s end
      return want(pl)
      """
    And the language server is running
    When I open "main.lua"
    Then the diagnostics for "main.lua" include LB0300

  # #48: a `---@type` over a table-field assignment used to be dropped on the
  # floor — no declared type, no mismatch. The editor path and `luabox check`
  # read the same crate, so the fix has to surface here too, at the
  # initializer.
  Scenario: a --@type over a table-field assignment is enforced in the editor
    Given a strict project with edition "5.4"
    And a file "main.lua" containing:
      """
      local M = {}
      ---@type string
      M.a = 1
      return M
      """
    And the language server is running
    When I open "main.lua"
    Then the diagnostics for "main.lua" include LB0300
    And diagnostic LB0300 in "main.lua" spans 2:6 to 2:7

  # #50: a `---@class` carried by a *global* never collected its members, so
  # every method call through the class read as an undefined field. The
  # editor must see the members exactly as `luabox check` does.
  Scenario: a class carried by a global resolves its methods in the editor
    Given a strict project with edition "5.4"
    And a file "main.lua" containing:
      """
      ---@class Glob
      Glob = {}
      function Glob:size() return 1 end
      ---@param g Glob
      local function use(g) return g:size() end
      return use
      """
    And the language server is running
    When I open "main.lua"
    Then the diagnostics for "main.lua" do not include LB0306

  # --- the declaration-driven `---@class` cycle check (round 12 R12-1) ----
  # `LB0318` used to reach the editor only through the resolver's back-edge,
  # which is reference-driven: a types file nothing in the project touches
  # resolved nothing, so the editor was GREEN on a cycle `luabox check`
  # reported red — the editor/CLI split this repo treats as blocking, with
  # the sides swapped. The server now runs the same declaration-driven
  # strongly-connected-component pass the CLI does, over the workspace's own
  # `---@class` graph. Measured against the pinned lua-language-server
  # 3.13.5: `circle-doc-class` fires on both fixtures below.

  Scenario: an unreferenced self-cyclic class is reported in the editor
    Given a strict project with edition "5.4"
    And a file "types.lua" containing:
      """
      ---@class Widget : Widget
      """
    And the language server is running
    When I open "types.lua"
    Then the diagnostics for "types.lua" include LB0318
    And diagnostic LB0318 in "types.lua" is an error

  # `a.lua` is never opened: the graph is the workspace's, so the partner's
  # half of the cycle reaches its own document anyway.
  Scenario: a mutual cycle is reported on the file that declares each member
    Given a strict project with edition "5.4"
    And a file "a.lua" containing:
      """
      ---@class RingA : RingB
      """
    And a file "b.lua" containing:
      """
      ---@class RingB : RingA
      """
    And the language server is running
    When I open "b.lua"
    Then the diagnostics for "b.lua" include LB0318
    And the diagnostics for "a.lua" include LB0318

  Scenario: the declaring file's own disable silences the cycle in the editor
    Given a strict project with edition "5.4"
    And a file "types.lua" containing:
      """
      ---@diagnostic disable-next-line: circle-doc-class
      ---@class Widget : Widget
      """
    And the language server is running
    When I open "types.lua"
    Then the diagnostics for "types.lua" do not include LB0318

  Scenario: an acyclic hierarchy draws no cycle diagnostic
    Given a strict project with edition "5.4"
    And a file "main.lua" containing:
      """
      ---@class Top
      ---@class Left : Top
      ---@class Right : Top
      ---@class Bottom : Left, Right
      """
    And the language server is running
    When I open "main.lua"
    Then the diagnostics for "main.lua" are empty

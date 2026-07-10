Feature: luabox bundle — single-file require-graph emit
  SPEC.md §7 (§18 P3): `luabox bundle` inlines the static require graph of
  `src/main.lua` into one target-lowered file at `<out>/<package>.lua`,
  with lazy module init over Lua-faithful `require` semantics. Modules are
  tree-shaken at module level; dynamic requires fail loudly; `--minify`
  mangles locals (never property names); `--sourcemap` writes a
  `.lua.map` consumed by `luabox unmap`.

  Scenario: requires are inlined into a single bundle
    Given a project with edition "5.1" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      local util = require("util")
      print(util.greet())
      """
    And a file "src/util.lua" containing:
      """
      local M = {}
      function M.greet()
        return "hello-from-util"
      end
      return M
      """
    When I run "luabox bundle"
    Then the command succeeds
    And the file "dist/fixture.lua" exists
    And "dist/fixture.lua" contains '__luabox_modules["util"] = function(...)'
    And "dist/fixture.lua" contains '__luabox_require("util")'
    And "dist/fixture.lua" contains "hello-from-util"
    And stdout contains "1 module(s) inlined"

  Scenario: unreachable modules are tree-shaken
    Given a project with edition "5.1" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      print(require("used"))
      """
    And a file "src/used.lua" containing:
      """
      return "used-body"
      """
    And a file "src/unused.lua" containing:
      """
      return "unused-body"
      """
    When I run "luabox bundle"
    Then the command succeeds
    And "dist/fixture.lua" contains "used-body"
    And "dist/fixture.lua" does not contain "unused-body"

  Scenario: dynamic require is a hard bundle error
    Given a project with edition "5.1" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      local name = "a"
      local m = require(name)
      print(m)
      """
    When I run "luabox bundle"
    Then the command fails
    And stderr contains "src/main.lua:2"
    And stderr contains "string literal"

  Scenario: lowering hoists one shared rt prelude across modules
    Given a project with edition "5.4" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      local util = require("util")
      print(5 & 3, util.flags(6))
      """
    And a file "src/util.lua" containing:
      """
      local M = {}
      function M.flags(x)
        return x & 4
      end
      return M
      """
    When I run "luabox bundle"
    Then the command succeeds
    And "dist/fixture.lua" contains exactly 1 occurrence of "local __luabox_rt = (function()"
    And "dist/fixture.lua" contains "__luabox_rt.band"
    And "dist/fixture.lua" does not contain "&"

  Scenario: minify mangles locals but never property names
    Given a project with edition "5.1" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      local util = require("util")
      local accumulator = util.compute(21)
      print(accumulator, util.label)
      """
    And a file "src/util.lua" containing:
      """
      local M = {}
      M.label = "twice"
      function M.compute(amount)
        return amount * 2
      end
      return M
      """
    When I run "luabox bundle --minify"
    Then the command succeeds
    And "dist/fixture.lua" does not contain "accumulator"
    And "dist/fixture.lua" does not contain "amount"
    And "dist/fixture.lua" contains ".compute"
    And "dist/fixture.lua" contains ".label"

  Scenario: sourcemap is written and unmap rewrites a traceback
    Given a project with edition "5.1" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      local util = require("util")
      util.explode()
      """
    And a file "src/util.lua" containing:
      """
      local M = {}
      function M.explode()
        error("kaboom")
      end
      return M
      """
    When I run "luabox bundle --sourcemap"
    Then the command succeeds
    And the file "dist/fixture.lua.map" exists
    And "dist/fixture.lua.map" contains "src/util.lua"
    When I unmap the last bundle line of "dist/fixture.lua"
    Then the command succeeds
    And stdout contains "src/main.lua:"

  Scenario: bundle refuses without the conventional entry point
    Given a project with edition "5.1" targeting "5.1"
    And a file "src/lib.lua" containing:
      """
      return {}
      """
    When I run "luabox bundle"
    Then the command fails
    And stderr contains "src/main.lua"

Feature: luabox build — single-file require-graph bundling
  SPEC.md §7 (§18 P3, flying-dice/luabox#4): with `bundle = true` (or
  `--bundle`) `luabox build` inlines the static require graph of each entry
  point into one target-lowered file. `plain` mode names each bundle from
  its entry basename under `out` (esbuild semantics), or `--outfile` for a
  single entry. Modules are tree-shaken; dynamic requires fail loudly;
  `--minify` mangles locals (never property names); `--sourcemap` writes a
  `.map` consumed by `luabox unmap`. `luabox bundle` no longer exists.

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
    When I run "luabox build --bundle"
    Then the command succeeds
    And the file "dist/main.lua" exists
    And "dist/main.lua" contains '__luabox_modules["util"] = function(...)'
    And "dist/main.lua" contains '__luabox_require("util")'
    And "dist/main.lua" contains "hello-from-util"
    And stdout contains "1 module(s) inlined"

  Scenario: bundle = true bundles without a flag
    Given a project with edition "5.1" targeting "5.1" bundling
    And a file "src/main.lua" containing:
      """
      print(require("used"))
      """
    And a file "src/used.lua" containing:
      """
      return "used-body"
      """
    When I run "luabox build"
    Then the command succeeds
    And the file "dist/main.lua" exists
    And "dist/main.lua" contains "used-body"

  Scenario: --no-bundle overrides bundle = true back to tree mode
    Given a project with edition "5.1" targeting "5.1" bundling
    And a file "src/main.lua" containing:
      """
      print("tree-mode")
      """
    When I run "luabox build --no-bundle"
    Then the command succeeds
    And the file "dist/src/main.lua" exists
    And the file "dist/main.lua" does not exist

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
    When I run "luabox build --bundle"
    Then the command succeeds
    And "dist/main.lua" contains "used-body"
    And "dist/main.lua" does not contain "unused-body"

  Scenario: dynamic require is a hard build error
    Given a project with edition "5.1" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      local name = "a"
      local m = require(name)
      print(m)
      """
    When I run "luabox build --bundle"
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
    When I run "luabox build --bundle"
    Then the command succeeds
    And "dist/main.lua" contains exactly 1 occurrence of "local __luabox_rt = (function()"
    And "dist/main.lua" contains "__luabox_rt.band"
    And "dist/main.lua" does not contain "&"

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
    When I run "luabox build --bundle --minify"
    Then the command succeeds
    And "dist/main.lua" does not contain "accumulator"
    And "dist/main.lua" does not contain "amount"
    And "dist/main.lua" contains ".compute"
    And "dist/main.lua" contains ".label"

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
    When I run "luabox build --bundle --sourcemap"
    Then the command succeeds
    And the file "dist/main.lua.map" exists
    And "dist/main.lua.map" contains "src/util.lua"
    When I unmap the last bundle line of "dist/main.lua"
    Then the command succeeds
    And stdout contains "src/main.lua:"

  Scenario: outfile names a single bundle
    Given a project with edition "5.1" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      print("hi")
      """
    When I run "luabox build --bundle --outfile dist/game.lua"
    Then the command succeeds
    And the file "dist/game.lua" exists
    And the file "dist/main.lua" does not exist

  Scenario: multiple entries produce one bundle each, named from basenames
    Given a project with edition "5.1" targeting "5.1"
    And a file "src/a.lua" containing:
      """
      print("from-a")
      """
    And a file "src/b.lua" containing:
      """
      print("from-b")
      """
    When I run "luabox build --bundle --entry src/a.lua --entry src/b.lua"
    Then the command succeeds
    And the file "dist/a.lua" exists
    And the file "dist/b.lua" exists
    And "dist/a.lua" contains "from-a"
    And "dist/b.lua" contains "from-b"

  Scenario: outfile with multiple entries is rejected
    Given a project with edition "5.1" targeting "5.1"
    And a file "src/a.lua" containing:
      """
      print("from-a")
      """
    And a file "src/b.lua" containing:
      """
      print("from-b")
      """
    When I run "luabox build --bundle --entry src/a.lua --entry src/b.lua --outfile dist/x.lua"
    Then the command fails
    And stderr contains "outfile"
    And stderr contains "exactly one entry"

  Scenario: bundling refuses without an existing entry point
    Given a project with edition "5.1" targeting "5.1"
    And a file "src/lib.lua" containing:
      """
      return {}
      """
    When I run "luabox build --bundle"
    Then the command fails
    And stderr contains "src/main.lua"

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

  # --- minification (minify) ----------------------------------------------

  Scenario: minification renames locals in declaration order and strips comments
    Given a project with edition "5.1" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      -- a comment nobody needs at runtime
      local greeting_text = "hi"
      local answer_value = 42 -- trailing
      print(greeting_text, answer_value)
      """
    When I run "luabox build --bundle --minify --entry src/main.lua"
    Then the command succeeds
    And "dist/main.lua" equals:
      """
      -- bundled by luabox (5.1 -> 5.1)
      local a="hi"local b=42 print(a,b)
      """

  Scenario: each nested function scope restarts the rename alphabet it can reuse
    Given a project with edition "5.1" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      local function outer_function(parameter_one)
        local function inner_function(parameter_two)
          return parameter_one + parameter_two
        end
        return inner_function(1)
      end
      print(outer_function(2))
      """
    When I run "luabox build --bundle --minify --entry src/main.lua"
    Then the command succeeds
    And "dist/main.lua" contains "local function a(b)local function c(d)return b+d end return c(1)end"

  Scenario: the rename alphabet carries into two-character names when it runs out
    Given a project with edition "5.1" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      local a1,a2,a3,a4,a5,a6,a7,a8,a9,a10,a11,a12,a13,a14 = 1,2,3,4,5,6,7,8,9,10,11,12,13,14
      local a15,a16,a17,a18,a19,a20,a21,a22,a23,a24,a25,a26,a27,a28 = 15,16,17,18,19,20,21,22,23,24,25,26,27,28
      print(a28)
      """
    When I run "luabox build --bundle --minify --entry src/main.lua"
    Then the command succeeds
    And "dist/main.lua" contains "a0,b0"
    And "dist/main.lua" contains "print(b0)"

  Scenario: globals and library members are never renamed
    Given a project with edition "5.1" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      local local_name = 1
      global_name = 2
      print(local_name, global_name, string.format("%d", 1))
      """
    When I run "luabox build --bundle --minify --entry src/main.lua"
    Then the command succeeds
    And "dist/main.lua" contains "global_name=2"
    And "dist/main.lua" contains "string.format"
    And "dist/main.lua" does not contain "local_name"

  Scenario: minification is on by default when the manifest asks for it
    Given a file "luabox.toml" containing:
      """
      [package]
      name = "fixture"
      version = "0.1.0"
      edition = "5.1"

      [build]
      target = "5.1"
      bundle = true
      minify = true
      """
    And a file "src/main.lua" containing:
      """
      local descriptive_name = 1
      print(descriptive_name)
      """
    When I run "luabox build"
    Then the command succeeds
    And stdout contains "minified"
    And "dist/main.lua" does not contain "descriptive_name"

  Scenario: strict bundling reports missing literal modules with a remedy
    Given a project with edition "5.1" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      print(require("missing"))
      """
    When I run "luabox build --bundle --strict-bundle"
    Then the command fails
    And stderr contains "cannot resolve module"
    And stderr contains "--external missing"
    And the file "dist/main.lua" does not exist

  Scenario: strict bundling allows explicitly declared runtime modules
    Given a project with edition "5.1" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      print(require("socket"))
      """
    When I run "luabox build --bundle --strict-bundle --external socket"
    Then the command succeeds
    And "dist/main.lua" contains 'require("socket")'

  Scenario: strict bundling refuses to silently run in tree mode
    Given a project with edition "5.1" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      print("hello")
      """
    When I run "luabox build --strict-bundle"
    Then the command fails
    And stderr contains "--strict-bundle requires bundle output"
    And the file "dist/src/main.lua" does not exist

  Scenario: strict bundling applies to Neovim packaging too
    Given a project with edition "5.1" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      print(require("missing"))
      """
    When I run "luabox build --mode nvim-plugin --strict-bundle"
    Then the command fails
    And stderr contains "cannot resolve module"

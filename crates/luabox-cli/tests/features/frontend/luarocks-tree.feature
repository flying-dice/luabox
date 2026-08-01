Feature: A genuine luarocks tree under lua_modules/
  README "Using dependencies": luabox consumes a rock tree, it does not
  produce one. `luarocks install --tree lua_modules <rock>` writes Lua
  sources to `lua_modules/share/lua/<X.Y>/`, C modules to
  `lua_modules/lib/lua/<X.Y>/`, and rock metadata to
  `lua_modules/lib/luarocks/rocks-<X.Y>/`.

  That tree is *vendored*, never first-party source: `check`, `lint` and
  `fmt` walk past it whatever it contains. It is still on the module path,
  so `luabox build --bundle` inlines from it (SPEC.md §7) — for the build's
  target dialect only, and never a C module.

  Its *annotations* are read, though (#30, decisions/09): a rock's installed
  sources are harvested for their LuaCATS surfaces — classes, enums, aliases
  and each module's `require`-export type — with no manifest declaration of
  any kind. Surfaces only: a vendored body is never typechecked, and a rock
  file that does not parse is skipped in silence.

  Scenario: check ignores the vendored tree and counts only project files
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      return 1
      """
    And a file "lua_modules/share/lua/5.4/pl/tablex.lua" containing:
      """
      ---@param n number
      local function double(n)
        return n * 2
      end

      return double("nope")
      """
    When I run "luabox check"
    Then the command succeeds
    And stdout does not contain "LB0300"
    And stderr contains "check: 0 errors, 0 warnings in 1 files"

  Scenario: a rock that does not even parse cannot fail the project
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      return 1
      """
    And a file "lua_modules/share/lua/5.1/legacy/broken.lua" containing:
      """
      local = = =
      """
    When I run "luabox check"
    Then the command succeeds
    And stderr contains "in 1 files"

  Scenario: lint walks past the vendored tree
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      return 1
      """
    And a file "lua_modules/share/lua/5.4/pl/tablex.lua" containing:
      """
      undeclared_global = 1
      return undeclared_global
      """
    When I run "luabox lint"
    Then the command succeeds
    And stderr contains "in 1 files"

  Scenario: fmt walks past the vendored tree
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      return 1
      """
    And a file "lua_modules/share/lua/5.4/pl/tablex.lua" containing:
      """
      local    x   =    1
      return      x
      """
    When I run "luabox fmt --check"
    Then the command succeeds
    And stdout contains "checked 1 files; all formatted"

  Scenario: build inlines a module from share/lua/<target version>
    Given a project with edition "5.4" targeting "5.4"
    And a file "src/main.lua" containing:
      """
      local tablex = require("pl.tablex")
      print(tablex.size())
      """
    And a file "lua_modules/share/lua/5.4/pl/tablex.lua" containing:
      """
      local M = {}
      function M.size()
        return "from-the-rock-tree"
      end
      return M
      """
    When I run "luabox build --bundle"
    Then the command succeeds
    And stdout contains "1 module(s) inlined"
    And "dist/main.lua" contains "from-the-rock-tree"
    And "dist/main.lua" contains '__luabox_modules["pl.tablex"] = function(...)'

  Scenario: a luajit target reads the 5.1 tree luarocks installs it under
    Given a project with edition "luajit" targeting "luajit"
    And a file "src/main.lua" containing:
      """
      local tablex = require("pl.tablex")
      print(tablex.size())
      """
    And a file "lua_modules/share/lua/5.1/pl/tablex.lua" containing:
      """
      local M = {}
      function M.size()
        return "from-the-jit-tree"
      end
      return M
      """
    When I run "luabox build --bundle"
    Then the command succeeds
    And stdout contains "1 module(s) inlined"
    And "dist/main.lua" contains "from-the-jit-tree"

  Scenario: a tree installed for another version is not this build's tree
    Given a project with edition "5.1" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      print(require("pl.tablex"))
      """
    And a file "lua_modules/share/lua/5.4/pl/tablex.lua" containing:
      """
      return "from-the-wrong-tree"
      """
    When I run "luabox build --bundle"
    Then the command succeeds
    And stdout contains "0 module(s) inlined"
    And "dist/main.lua" does not contain "from-the-wrong-tree"
    And "dist/main.lua" contains 'require("pl.tablex")'

  Scenario: a C module stays an external require rather than failing the build
    Given a project with edition "5.1" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      local lfs = require("lfs")
      print(lfs)
      """
    And a file "lua_modules/lib/lua/5.1/lfs.so" containing:
      """
      not-lua
      """
    When I run "luabox build --bundle"
    Then the command succeeds
    And stdout contains "0 module(s) inlined"
    And "dist/main.lua" contains 'require("lfs")'

  Scenario: the older flat lua_modules layout still resolves
    Given a project with edition "5.4" targeting "5.4"
    And a file "src/main.lua" containing:
      """
      print(require("pkg.helper"))
      """
    And a file "lua_modules/pkg/src/helper.lua" containing:
      """
      return "from-the-flat-layout"
      """
    When I run "luabox build --bundle"
    Then the command succeeds
    And stdout contains "1 module(s) inlined"
    And "dist/main.lua" contains "from-the-flat-layout"

  Scenario: a rock's class is published to the consumer with no manifest declaration
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
    And a file "src/main.lua" containing:
      """
      ---@type mylib.Point
      local p = { x = 1, y = 2 }
      return p
      """
    When I run "luabox check"
    Then the command succeeds
    And stdout does not contain "LB0305"
    And stderr contains "check: 0 errors, 0 warnings in 1 files"

  Scenario: misusing a rock type is reported in the consumer, not the rock
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
    And a file "src/main.lua" containing:
      """
      local mylib = require("mylib")
      local p = mylib.point(1, 2)
      return p.nope
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0306"
    And stdout contains "undefined field `nope` on `mylib.Point`"
    And stdout contains "src/main.lua"
    And stdout does not contain "lua_modules"

  Scenario: an annotated rock body that is wrong is still never checked
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      return 1
      """
    And a file "lua_modules/share/lua/5.4/mylib/init.lua" containing:
      """
      ---@class mylib.Point
      ---@field x number

      ---@type mylib.Point
      local wrong = { x = "not a number" }

      ---@param n number
      local function double(n) return n * 2 end

      return { wrong = wrong, doubled = double("nope") }
      """
    When I run "luabox check"
    Then the command succeeds
    And stdout does not contain "LB0300"
    And stderr contains "check: 0 errors, 0 warnings in 1 files"

  Scenario: an annotated rock that does not parse is skipped in silence
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@type mylib.Point
      local p = { x = 1 }
      return p
      """
    And a file "lua_modules/share/lua/5.4/mylib/init.lua" containing:
      """
      ---@class mylib.Point
      ---@field x number
      return {}
      """
    And a file "lua_modules/share/lua/5.4/mylib/broken.lua" containing:
      """
      ---@class mylib.Ghost
      local = = =
      """
    When I run "luabox check"
    Then the command succeeds
    And stdout does not contain "LB0001"
    And stdout does not contain "LB0305"

  Scenario: an unannotated rock stays unknown to the checker
    Given a strict project with edition "5.4"
    And a file "lua_modules/share/lua/5.4/plain/init.lua" containing:
      """
      local M = {}
      function M.anything() return 1 end
      return M
      """
    And a file "src/main.lua" containing:
      """
      local plain = require("plain")
      return plain.whatever_it_pleases
      """
    When I run "luabox check"
    Then the command succeeds
    And stderr contains "check: 0 errors, 0 warnings in 1 files"

  Scenario: an explicit defs package beats the rock's own declaration
    Given a file "luabox.toml" containing:
      """
      [package]
      name = "fixture"
      version = "0.1.0"
      edition = "5.4"

      [types]
      strict = true
      defs = ["mylib"]
      """
    And a file "defs/mylib.d.lua" containing:
      """
      ---@meta

      ---@class mylib.Point
      ---@field x number
      """
    And a file "lua_modules/share/lua/5.4/mylib/init.lua" containing:
      """
      ---@class mylib.Point
      ---@field x number
      ---@field y number
      return {}
      """
    And a file "src/main.lua" containing:
      """
      ---@type mylib.Point
      local p = { x = 1 }
      return p
      """
    When I run "luabox check"
    Then the command succeeds
    And stdout does not contain "LB0302"
    And stdout does not contain "LB0307"

  # Shockwave round 3, the `luabox check` half of the colliding-name pair (the
  # editor half is `features/lsp/diagnostics.feature`). `pl.lua` and
  # `pl/init.lua` both answer to `pl`; `require` tries the flat form first, so
  # `pl` is the number, and passing it where a string is wanted must flag —
  # here, and identically in the editor.
  Scenario: a flat rock module beats its init form, the way `require` resolves it
    Given a file "luabox.toml" containing:
      """
      [package]
      name = "fixture"
      version = "0.1.0"
      edition = "5.4"

      [types]
      strict = true
      """
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
    And a file "src/main.lua" containing:
      """
      local pl = require("pl")
      ---@param s string
      local function want(s) return s end
      return want(pl)
      """
    When I run "luabox check"
    Then the command fails
    And diagnostic LB0300 is reported

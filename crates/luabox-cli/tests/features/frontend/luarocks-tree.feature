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

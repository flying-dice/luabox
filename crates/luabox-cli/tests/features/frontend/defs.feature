Feature: stdlib definition packages — `---@meta` `.d.lua` ambient types
  SPEC.md §3 — luabox ships per-edition definition packages describing the
  real stdlib (basic globals, string/table/math/io/os/coroutine/debug plus
  version-specific bit32/bit/jit/utf8). They are selected by `edition` and
  merged beneath a file's own annotations, so calls to `print`, `string.*`,
  `math.*`, ... are arity- and type-checked. Project-local `[types] defs`
  layer additional packages resolved from the `defs/` directory.

  Scenario: a stdlib misuse is caught in a strict project
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      string.rep("x", "y")
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"

  Scenario: passing anything to print is fine
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      print(1, "two", true, nil, {})
      """
    When I run "luabox check"
    Then the command succeeds
    And stderr contains "check: 0 errors"

  Scenario: a version-gated stdlib signature is enforced in its edition
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      string.pack(123, 1)
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"

  Scenario: a global absent from the edition is not itself an error
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      local x = bit32.band(1, 2)
      print(x)
      """
    When I run "luabox check"
    Then the command succeeds

  Scenario: the same module resolves where the edition provides it
    Given a strict project with edition "5.2"
    And a file "src/main.lua" containing:
      """
      local x = bit32.band(1, 2)
      print(x)
      """
    When I run "luabox check"
    Then the command succeeds

  Scenario: a project-local defs package is loaded and enforced
    Given a file "defs/game.d.lua" containing:
      """
      ---@meta
      ---@param name string
      ---@return boolean
      function register(name) end
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
      register(123)
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"

  # --- cross-package type sharing (#108, the luals workspace.library model) ---
  # A dependency's own `[types] defs` join the consumer's ambient scope
  # automatically. These scenarios place the dependency under `lua_modules/`
  # (where a non-path dependency resolves), so the consumer is the single
  # project root the harness runs from.

  Scenario: a dependency's defs classes are ambient and checked in the consumer
    Given a file "lua_modules/geometry/luabox.toml" containing:
      """
      [package]
      name = "geometry"
      version = "0.1.0"
      edition = "5.4"

      [types]
      strict = true
      defs = ["geometry"]
      """
    And a file "lua_modules/geometry/defs/geometry.d.lua" containing:
      """
      ---@meta
      ---@class geometry.Point
      ---@field x number
      ---@field y number
      """
    And a file "luabox.toml" containing:
      """
      [package]
      name = "app"
      version = "0.1.0"
      edition = "5.4"

      [types]
      strict = true

      [dependencies]
      geometry = "1.0"
      """
    And a file "src/main.lua" containing:
      """
      ---@param p geometry.Point
      local function use(p) end
      use({ x = 1, y = "no" })
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"

  Scenario: a consumer carrier claiming a dependency interface must implement it
    Given a file "lua_modules/geometry/luabox.toml" containing:
      """
      [package]
      name = "geometry"
      version = "0.1.0"
      edition = "5.4"

      [types]
      strict = true
      defs = ["geometry"]
      """
    And a file "lua_modules/geometry/defs/geometry.d.lua" containing:
      """
      ---@meta
      ---@class geometry.Shape
      ---@field area fun(self): number
      ---@class geometry.Drawable : geometry.Shape
      ---@field draw fun(self): string
      """
    And a file "luabox.toml" containing:
      """
      [package]
      name = "app"
      version = "0.1.0"
      edition = "5.4"

      [types]
      strict = true

      [dependencies]
      geometry = "1.0"
      """
    And a file "src/square.lua" containing:
      """
      ---@class app.Square : geometry.Drawable
      ---@field side integer
      local Square = {}
      Square.__index = Square

      function Square:area()
        return self.side
      end

      return Square
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"
    And stdout contains "missing member `draw`"

  Scenario: a dependency's def-declared global API is param-checked
    Given a file "lua_modules/geometry/luabox.toml" containing:
      """
      [package]
      name = "geometry"
      version = "0.1.0"
      edition = "5.4"

      [types]
      strict = true
      defs = ["geometry"]
      """
    And a file "lua_modules/geometry/defs/geometry.d.lua" containing:
      """
      ---@meta
      ---@class geometrylib
      geometry = {}
      ---@param x number
      ---@param y number
      ---@return table
      function geometry.point(x, y) end
      """
    And a file "luabox.toml" containing:
      """
      [package]
      name = "app"
      version = "0.1.0"
      edition = "5.4"

      [types]
      strict = true

      [dependencies]
      geometry = "1.0"
      """
    And a file "src/main.lua" containing:
      """
      return geometry.point("no", 2)
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"

  Scenario: the same class from two packages is a collision warning, consumer wins
    Given a file "lua_modules/geometry/luabox.toml" containing:
      """
      [package]
      name = "geometry"
      version = "0.1.0"
      edition = "5.4"

      [types]
      strict = true
      defs = ["geometry"]
      """
    And a file "lua_modules/geometry/defs/geometry.d.lua" containing:
      """
      ---@meta
      ---@class geometry.Point
      ---@field x number
      ---@field y number
      """
    And a file "defs/dup.d.lua" containing:
      """
      ---@meta
      ---@class geometry.Point
      ---@field x number
      """
    And a file "luabox.toml" containing:
      """
      [package]
      name = "app"
      version = "0.1.0"
      edition = "5.4"

      [types]
      strict = true
      defs = ["dup"]

      [dependencies]
      geometry = "1.0"
      """
    And a file "src/main.lua" containing:
      """
      ---@type geometry.Point
      local p = { x = 1 }
      return p
      """
    When I run "luabox check"
    Then the command succeeds
    And stdout contains "LB0307"
    And stdout contains "defs/dup.d.lua"
    And stdout contains "geometry/defs/geometry.d.lua"

  Scenario: an unresolvable defs entry is a clear diagnostic
    Given a file "luabox.toml" containing:
      """
      [package]
      name = "fixture"
      version = "0.1.0"
      edition = "5.4"

      [types]
      strict = true
      defs = ["nonexistent"]
      """
    And a file "src/main.lua" containing:
      """
      print("hi")
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "cannot resolve definition package `nonexistent`"

  Scenario: a defs package may be a directory of `*.d.lua` files, nested included
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
    And a file "defs/mylib/one.d.lua" containing:
      """
      ---@meta

      ---@class Ambient
      ---@field n number
      """
    And a file "defs/mylib/nested/two.d.lua" containing:
      """
      ---@meta

      ---@class Other
      ---@field s string
      """
    And a file "src/main.lua" containing:
      """
      ---@type Ambient
      local a = { n = 1 }
      ---@type Other
      local b = { s = "ok" }
      return a, b
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

  Scenario: a class from a nested defs file is enforced like any other
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
    And a file "defs/mylib/nested/two.d.lua" containing:
      """
      ---@meta

      ---@class Other
      ---@field s string
      """
    And a file "src/main.lua" containing:
      """
      ---@type Other
      local b = { s = 2 }
      return b
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"
    And stdout contains "expected `string`, found `2`"

  # --- carrier-style members in a defs file (#39) ---------------------------
  # `---@field` is the defs convention, but luals makes no distinction: a
  # `function Class:method()` written in a library file is a member of that
  # class. A defs file is never inferred, so its carrier attachments are
  # harvested syntactically — signatures and tags included.

  Scenario: a carrier-style method in a defs file is a member of its class
    Given a file "defs/game.d.lua" containing:
      """
      ---@meta

      ---@class Widget
      local Widget = {}

      ---@param n integer
      ---@return string
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
        return w:render(1)
      end
      return use
      """
    When I run "luabox check"
    Then the command succeeds
    And stdout does not contain "LB0306"
    And stderr contains "check: 0 errors, 0 warnings"

  # Shockwave round 3: `function Animal:speak()` names the LOCAL `Animal`, so
  # it belongs to `Wrapper` — the class that local carries — not to the
  # unrelated class that happens to be *named* `Animal`. Both orderings of the
  # two class blocks must agree, and both must agree with what the same body
  # does as ordinary project source (#39 is a defs/project parity goal).

  Scenario: a carrier variable wins over a same-named class declared after it
    Given a file "defs/zoo.d.lua" containing:
      """
      ---@meta

      ---@class Wrapper
      local Animal = {}

      ---@class Animal
      local Zoo = {}

      ---@return string
      function Animal:speak() end
      """
    And a file "luabox.toml" containing:
      """
      [package]
      name = "fixture"
      version = "0.1.0"
      edition = "5.4"

      [types]
      strict = true
      defs = ["zoo"]
      """
    And a file "src/main.lua" containing:
      """
      ---@param w Wrapper
      local function use(w)
        return w:speak()
      end
      return use
      """
    When I run "luabox check"
    Then the command succeeds
    And stdout does not contain "LB0306"

  Scenario: the same file with the two class blocks swapped gives the same answer
    Given a file "defs/zoo.d.lua" containing:
      """
      ---@meta

      ---@class Animal
      local Zoo = {}

      ---@class Wrapper
      local Animal = {}

      ---@return string
      function Animal:speak() end
      """
    And a file "luabox.toml" containing:
      """
      [package]
      name = "fixture"
      version = "0.1.0"
      edition = "5.4"

      [types]
      strict = true
      defs = ["zoo"]
      """
    And a file "src/main.lua" containing:
      """
      ---@param w Wrapper
      ---@param a Animal
      local function use(w, a)
        return w:speak(), a:speak()
      end
      return use
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "undefined field `speak` on `Animal`"
    And stdout does not contain "undefined field `speak` on `Wrapper`"

  Scenario: the same body as project source agrees with the defs file
    Given a file "luabox.toml" containing:
      """
      [package]
      name = "fixture"
      version = "0.1.0"
      edition = "5.4"

      [types]
      strict = true
      """
    And a file "src/zoo.lua" containing:
      """
      ---@class Wrapper
      local Animal = {}

      ---@class Animal
      local Zoo = {}

      ---@return string
      function Animal:speak() end

      return { Animal = Animal, Zoo = Zoo }
      """
    And a file "src/main.lua" containing:
      """
      ---@param w Wrapper
      ---@param a Animal
      local function use(w, a)
        return w:speak(), a:speak()
      end
      return use
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "undefined field `speak` on `Animal`"
    And stdout does not contain "undefined field `speak` on `Wrapper`"

  Scenario: a carrier-style method's signature is enforced at the use site
    Given a file "defs/game.d.lua" containing:
      """
      ---@meta

      ---@class Widget
      local Widget = {}

      ---@param n integer
      ---@return string
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
        return w:render("nope")
      end
      return use
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "error[LB0300]"
    And stdout contains "expected `integer`"

  Scenario: a member neither declared nor attached in a defs file is still undefined
    Given a file "defs/game.d.lua" containing:
      """
      ---@meta

      ---@class Widget
      local Widget = {}

      function Widget:render() end
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
        w:nosuchthing()
      end
      return use
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "error[LB0306]"
    And stdout contains "undefined field `nosuchthing`"

  # Shockwave round 4, finding B/J: within the lexical rank the carrier map
  # took the FIRST carrier for a repeated variable name, but `function M:m()`
  # names the binding in scope — which Lua resolves to the LAST `local M`.
  # The project-source path resolves the binding and always said "last"; the
  # defs path said "first", so the two disagreed on every repeated-carrier
  # shape. Both now follow the binding.

  Scenario: a repeated carrier variable in a defs file follows the last binding
    Given a file "defs/zoo.d.lua" containing:
      """
      ---@meta

      ---@class Alpha
      local M = {}

      ---@class Beta
      local M = {}

      ---@return string
      function M:only() end
      """
    And a file "luabox.toml" containing:
      """
      [package]
      name = "fixture"
      version = "0.1.0"
      edition = "5.4"

      [types]
      strict = true
      defs = ["zoo"]
      """
    And a file "src/main.lua" containing:
      """
      ---@param a Alpha
      ---@param b Beta
      local function use(a, b)
        return b:only(), a:only()
      end
      return use
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "undefined field `only` on `Alpha`"
    And stdout does not contain "undefined field `only` on `Beta`"

  Scenario: the same repeated carrier as project source gives the same answer
    Given a file "luabox.toml" containing:
      """
      [package]
      name = "fixture"
      version = "0.1.0"
      edition = "5.4"

      [types]
      strict = true
      """
    And a file "src/zoo.lua" containing:
      """
      ---@class Alpha
      local M = {}

      ---@class Beta
      local M = {}

      ---@return string
      function M:only() end

      return M
      """
    And a file "src/main.lua" containing:
      """
      ---@param a Alpha
      ---@param b Beta
      local function use(a, b)
        return b:only(), a:only()
      end
      return use
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "undefined field `only` on `Alpha`"
    And stdout does not contain "undefined field `only` on `Beta`"

  Scenario: the repeated carrier declared the other way round flips with it
    Given a file "defs/zoo.d.lua" containing:
      """
      ---@meta

      ---@class Beta
      local M = {}

      ---@class Alpha
      local M = {}

      ---@return string
      function M:only() end
      """
    And a file "luabox.toml" containing:
      """
      [package]
      name = "fixture"
      version = "0.1.0"
      edition = "5.4"

      [types]
      strict = true
      defs = ["zoo"]
      """
    And a file "src/main.lua" containing:
      """
      ---@param a Alpha
      ---@param b Beta
      local function use(a, b)
        return a:only(), b:only()
      end
      return use
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "undefined field `only` on `Beta`"
    And stdout does not contain "undefined field `only` on `Alpha`"

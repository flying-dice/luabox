Feature: luabox check — cross-module argument checking (#46)
  A function reached through `require` is argument-checked at its call
  sites exactly like a same-file one: the resolved signature's `---@param`
  types produce LB0300 and its arity produces LB0301, on both sides of the
  module boundary.

  The export *type* already crossed that boundary (#85 / #30) — a required
  function's `---@return` flowed into the consumer, and field misuse was
  reported there. Parameter enforcement did not: the checker resolved a
  callee's signature only through its local-binding and dotted-name
  registries, and neither knows a required module's members, so every
  `require`d function in every project — project modules and vendored rocks
  alike — was unchecked at its call sites.

  The measured table on #46 is the acceptance matrix; its five rows are the
  first five scenarios below, verbatim.

  Conservatism is inherited from the same-file path rather than re-decided:
  an **unannotated** exported function is not argument-checked, because an
  unannotated same-file function is not either.

  Scenario: row 1 — same file, direct, is argument-checked
    Given a strict project with edition "5.4"
    And a file "src/app.lua" containing:
      """
      ---@param w number
      local function area(w) end
      area("nope")
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"

  Scenario: row 2 — same file, through a table field, is argument-checked
    Given a strict project with edition "5.4"
    And a file "src/app.lua" containing:
      """
      local M = {}
      ---@param w number
      function M.area(w) end
      M.area("nope")
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"

  Scenario: row 3 — cross-module via require, direct, is argument-checked
    Given a strict project with edition "5.4"
    And a file "src/direct.lua" containing:
      """
      ---@param n number
      ---@return number
      return function(n)
        return n
      end
      """
    And a file "src/app.lua" containing:
      """
      local f = require("direct")
      local x = f("nope")
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"

  Scenario: row 4 — cross-module via require, through a table field, is argument-checked
    Given a strict project with edition "5.4"
    And a file "src/geom.lua" containing:
      """
      local M = {}
      ---@param w number
      ---@param h number
      ---@return number
      function M.area(w, h)
        return w * h
      end
      return M
      """
    And a file "src/app.lua" containing:
      """
      local geom = require("geom")
      local x = geom.area("nope", 4)
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"

  Scenario: row 5 — cross-module with an explicit ---@type at the call site still checks
    Given a strict project with edition "5.4"
    And a file "src/geom.lua" containing:
      """
      local M = {}
      ---@param w number
      ---@param h number
      ---@return number
      function M.area(w, h)
        return w * h
      end
      return M
      """
    And a file "src/app.lua" containing:
      """
      ---@type fun(w: number, h: number): number
      local area = require("geom").area
      local x = area("nope", 4)
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"

  Scenario: the same required module used correctly reports nothing
    Given a strict project with edition "5.4"
    And a file "src/geom.lua" containing:
      """
      local M = {}
      ---@param w number
      ---@param h number
      ---@return number
      function M.area(w, h)
        return w * h
      end
      return M
      """
    And a file "src/app.lua" containing:
      """
      local geom = require("geom")
      local x = geom.area(3, 4)
      """
    When I run "luabox check"
    Then the command succeeds
    And stderr contains "check: 0 errors, 0 warnings"

  Scenario: too few arguments across the boundary report arity
    Given a strict project with edition "5.4"
    And a file "src/geom.lua" containing:
      """
      local M = {}
      ---@param w number
      ---@param h number
      ---@return number
      function M.area(w, h)
        return w * h
      end
      return M
      """
    And a file "src/app.lua" containing:
      """
      local geom = require("geom")
      local x = geom.area(3)
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0301"

  Scenario: too many arguments across the boundary report arity
    Given a strict project with edition "5.4"
    And a file "src/geom.lua" containing:
      """
      local M = {}
      ---@param w number
      ---@param h number
      ---@return number
      function M.area(w, h)
        return w * h
      end
      return M
      """
    And a file "src/app.lua" containing:
      """
      local geom = require("geom")
      local x = geom.area(3, 4, 5)
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0301"

  Scenario: an unannotated exported function is not argument-checked
    Given a strict project with edition "5.4"
    And a file "src/plain.lua" containing:
      """
      local M = {}
      function M.f(a, b)
        return a
      end
      return M
      """
    And a file "src/app.lua" containing:
      """
      local p = require("plain")
      p.f("anything")
      p.f(1, 2, 3)
      """
    When I run "luabox check"
    Then the command succeeds
    And stderr contains "check: 0 errors, 0 warnings"

  Scenario: an omitted optional parameter is not an arity error across the boundary
    Given a strict project with edition "5.4"
    And a file "src/opt.lua" containing:
      """
      local M = {}
      ---@param a number
      ---@param b? string
      function M.f(a, b) end
      return M
      """
    And a file "src/app.lua" containing:
      """
      local o = require("opt")
      o.f(1)
      """
    When I run "luabox check"
    Then the command succeeds
    And stderr contains "check: 0 errors, 0 warnings"

  Scenario: varargs lift the arity ceiling across the boundary
    Given a strict project with edition "5.4"
    And a file "src/va.lua" containing:
      """
      local M = {}
      ---@param a number
      ---@vararg string
      function M.f(a, ...) end
      return M
      """
    And a file "src/app.lua" containing:
      """
      local v = require("va")
      v.f(1, "x", "y")
      """
    When I run "luabox check"
    Then the command succeeds
    And stderr contains "check: 0 errors, 0 warnings"

  Scenario: a function stored in a local then called is argument-checked
    Given a strict project with edition "5.4"
    And a file "src/geom.lua" containing:
      """
      local M = {}
      ---@param w number
      ---@param h number
      ---@return number
      function M.area(w, h)
        return w * h
      end
      return M
      """
    And a file "src/app.lua" containing:
      """
      local geom = require("geom")
      local area = geom.area
      local x = area("nope", 4)
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"

  Scenario: a nested table export is argument-checked
    Given a strict project with edition "5.4"
    And a file "src/nest.lua" containing:
      """
      local M = { util = {} }
      ---@param s string
      ---@return string
      function M.util.fmt(s)
        return s
      end
      return M
      """
    And a file "src/app.lua" containing:
      """
      local n = require("nest")
      local x = n.util.fmt(42)
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"

  Scenario: a colon-method on an exported class is argument-checked
    Given a strict project with edition "5.4"
    And a file "src/box.lua" containing:
      """
      ---@class Box
      local Box = {}
      Box.__index = Box

      ---@return Box
      function Box.new()
        return setmetatable({}, Box)
      end

      ---@param w number
      ---@return number
      function Box:grow(w)
        return w
      end
      return Box
      """
    And a file "src/app.lua" containing:
      """
      local Box = require("box")
      local b = Box.new()
      local x = b:grow("nope")
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"

  Scenario: a module reached through init.lua is argument-checked
    Given a strict project with edition "5.4"
    And a file "src/pkg/init.lua" containing:
      """
      local M = {}
      ---@param s string
      ---@return string
      function M.id(s)
        return s
      end
      return M
      """
    And a file "src/app.lua" containing:
      """
      local p = require("pkg")
      local x = p.id(42)
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"

  Scenario: a vendored rock's function is argument-checked at the consumer
    Given a strict project with edition "5.4"
    And a file "lua_modules/share/lua/5.4/rk.lua" containing:
      """
      local M = {}
      ---@param n number
      ---@return number
      function M.twice(n)
        return n * 2
      end
      return M
      """
    And a file "src/app.lua" containing:
      """
      local rk = require("rk")
      local x = rk.twice("nope")
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"

  Scenario: a vendored rock's function called correctly reports nothing
    Given a strict project with edition "5.4"
    And a file "lua_modules/share/lua/5.4/rk.lua" containing:
      """
      local M = {}
      ---@param n number
      ---@return number
      function M.twice(n)
        return n * 2
      end
      return M
      """
    And a file "src/app.lua" containing:
      """
      local rk = require("rk")
      local x = rk.twice(21)
      """
    When I run "luabox check"
    Then the command succeeds
    And stderr contains "check: 0 errors, 0 warnings"

  # Before #56 this spelling was the disclosed gap: a `---@field`-declared
  # member lived only in the class declaration, and the carrier crossed the
  # boundary as its structural table — so the consumer never saw `send`. The
  # export is the class now, so the declared and attached spellings are
  # argument-checked identically (the next scenario is the attached twin).
  Scenario: a member declared as a ---@field reaches the consumer argument-checked
    Given a strict project with edition "5.4"
    And a file "src/api.lua" containing:
      """
      ---@class Api
      ---@field send fun(payload: string): boolean
      local Api = {}
      return Api
      """
    And a file "src/app.lua" containing:
      """
      local api = require("api")
      local ok = api.send(42)
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"

  Scenario: the same class member, attached rather than declared, is argument-checked
    Given a strict project with edition "5.4"
    And a file "src/api.lua" containing:
      """
      ---@class Api
      local Api = {}

      ---@param payload string
      ---@return boolean
      function Api.send(payload)
        return true
      end
      return Api
      """
    And a file "src/app.lua" containing:
      """
      local api = require("api")
      local ok = api.send(42)
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"

  Scenario: a function re-exported from a second require is not checked transitively
    Given a strict project with edition "5.4"
    And a file "src/geom.lua" containing:
      """
      local M = {}
      ---@param w number
      ---@param h number
      ---@return number
      function M.area(w, h)
        return w * h
      end
      return M
      """
    And a file "src/reexport.lua" containing:
      """
      local geom = require("geom")
      return { area = geom.area }
      """
    And a file "src/app.lua" containing:
      """
      local r = require("reexport")
      local x = r.area("nope", 4)
      """
    When I run "luabox check"
    Then the command succeeds
    And stderr contains "check: 0 errors, 0 warnings"

  Scenario: a function re-exported from the same file is argument-checked
    Given a strict project with edition "5.4"
    And a file "src/reexport.lua" containing:
      """
      ---@param w number
      ---@param h number
      ---@return number
      local function area(w, h)
        return w * h
      end
      return { area = area }
      """
    And a file "src/app.lua" containing:
      """
      local r = require("reexport")
      local x = r.area("nope", 4)
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"

  Scenario: a dynamic require path stays unchecked
    Given a strict project with edition "5.4"
    And a file "src/geom.lua" containing:
      """
      local M = {}
      ---@param w number
      ---@param h number
      ---@return number
      function M.area(w, h)
        return w * h
      end
      return M
      """
    And a file "src/app.lua" containing:
      """
      local name = "geom"
      local geom = require(name)
      local x = geom.area("nope", 4)
      """
    When I run "luabox check"
    Then the command succeeds
    And stderr contains "check: 0 errors, 0 warnings"

  Scenario: a trailing nil-admitting parameter is optional across the boundary too
    Given a strict project with edition "5.4"
    And a file "src/geom.lua" containing:
      """
      local M = {}
      ---@param w number
      ---@param scale number|nil
      ---@return number
      function M.area(w, scale)
        if scale == nil then return w end
        return w * scale
      end
      return M
      """
    And a file "src/app.lua" containing:
      """
      local geom = require("geom")
      local x = geom.area(3)
      return x
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

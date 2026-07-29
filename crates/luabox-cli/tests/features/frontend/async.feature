Feature: luabox check — `---@async` and await-in-sync (LB0316)
  SPEC.md §3: `---@async` marks a function that yields. Calling one from a
  function that is not itself `---@async` is lua-language-server's
  `await-in-sync` diagnostic — a warning, so it never fails a build on its own.
  The top-level chunk counts as async (a main chunk may yield), so only calls
  inside an unmarked function are flagged.

  Scenario: calling an `---@async` function from a sync function is flagged
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@async
      ---@return string
      local function fetch()
        return "x"
      end

      local function sync()
        return fetch()
      end
      return sync
      """
    When I run "luabox check"
    Then the command succeeds
    And stdout contains "LB0316"
    And stdout contains "call to async `fetch` in a non-async function"

  Scenario: await-in-sync is a warning, so it never fails the command
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@async
      local function fetch() end

      local function sync()
        fetch()
      end
      return sync
      """
    When I run "luabox check"
    Then the command succeeds
    And stderr contains "check: 0 errors, 1 warnings in 1 files"

  Scenario: an `---@async` caller may await freely
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@async
      local function fetch()
        return "x"
      end

      ---@async
      local function outer()
        return fetch()
      end
      return outer
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

  Scenario: the top-level chunk may yield, so a call there is not flagged
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@async
      local function fetch()
        return "x"
      end
      return fetch()
      """
    When I run "luabox check"
    Then the command succeeds
    And stdout does not contain "LB0316"

  Scenario: an `---@async` method is flagged at a `:` call from a sync function
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@class Client
      local Client = {}
      Client.__index = Client

      ---@async
      function Client:fetch() end

      ---@return Client
      function Client.new()
        return setmetatable({}, Client)
      end

      local function sync()
        Client.new():fetch()
      end
      return sync
      """
    When I run "luabox check"
    Then the command succeeds
    And stdout contains "LB0316"
    And stdout contains "call to async `fetch` in a non-async function"

  Scenario: an `---@async` method on a plain prototype is flagged at a `:` call
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      local Proto = {}
      Proto.__index = Proto

      ---@async
      function Proto:fetch() end

      local p = setmetatable({}, Proto)
      local function sync()
        p:fetch()
      end
      return sync
      """
    When I run "luabox check"
    Then the command succeeds
    And stdout contains "LB0316"
    And stdout contains "call to async `fetch` in a non-async function"

  Scenario: a `---@field`-declared method still carries its carrier's `---@async`
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@class Decl
      ---@field fetch fun(self: Decl)
      local Decl = {}
      Decl.__index = Decl

      ---@async
      function Decl:fetch() end

      ---@type Decl
      local d
      local function sync()
        d:fetch()
      end
      return sync
      """
    When I run "luabox check"
    Then the command succeeds
    And stdout contains "LB0316"
    And stdout contains "call to async `fetch` in a non-async function"

  Scenario: an `---@async` method awaited from an `---@async` caller is clean
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@class Client
      local Client = {}
      Client.__index = Client

      ---@async
      function Client:fetch() end

      ---@type Client
      local c

      ---@async
      local function poll()
        c:fetch()
      end
      return poll
      """
    When I run "luabox check"
    Then the command succeeds
    And stdout does not contain "LB0316"

  Scenario: a `---@class` carrier with no `__index` link still carries `---@async` (#33)
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@class Plain
      local Plain = {}

      ---@async
      function Plain:fetch() end

      ---@type Plain
      local p
      local function sync()
        p:fetch()
      end
      return sync
      """
    When I run "luabox check"
    Then the command succeeds
    And stdout contains "LB0316"
    And stdout contains "call to async `fetch` in a non-async function"
    And stdout does not contain "LB0306"

  Scenario: a `---@type fun(...)` assignment carries `---@async` to a `:` call site (#38)
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@class Cls
      local Cls = {}

      ---@async
      ---@type fun(self: Cls)
      Cls.fetch = function(self) end

      ---@type Cls
      local o
      local function sync()
        o:fetch()
      end
      return sync
      """
    When I run "luabox check"
    Then the command succeeds
    And stdout contains "LB0316"
    And stdout contains "call to async `fetch` in a non-async function"

  Scenario: a carrier-style method in a defs file carries `---@async` to its use site (#39)
    Given a file "defs/game.d.lua" containing:
      """
      ---@meta

      ---@class Widget
      local Widget = {}

      ---@async
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
        w:render()
      end
      return use
      """
    When I run "luabox check"
    Then the command succeeds
    And stdout contains "LB0316"
    And stdout does not contain "LB0306"

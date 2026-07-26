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

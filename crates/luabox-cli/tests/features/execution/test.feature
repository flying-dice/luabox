Feature: luabox test — the built-in runner (deprecated)
  Zero-config discovery (`*_test.lua`, `*.test.lua`, `tests/`), a runtime
  resolved from the manifest edition (or `LUABOX_LUA`), and a human report
  whose exit code is nonzero iff anything failed.

  The command is deprecated (luabox is a toolchain, not a runtime: code
  coupled to its deployment environment cannot be faithfully executed on a
  bare interpreter) — it keeps working but warns on every invocation and is
  slated for removal.

  These scenarios are hermetic: they never require a real Lua. A "fake Lua
  runtime" (a tiny `.bat` shim pointed at via `LUABOX_LUA`) echoes each test
  file — authored here as raw runner protocol — so discovery, aggregation
  and exit codes are exercised without an interpreter installed. The real
  busted/flat harness is proven end-to-end by the `luabox-test` integration
  tests, behind a runtime probe.

  Scenario: every invocation warns that the command is deprecated
    Given a fake Lua runtime
    And a passing test file "unit_test.lua" with test "adds"
    When I run "luabox test" with the fake runtime
    Then the command succeeds
    And stderr contains "`luabox test` is deprecated"

  Scenario: --coverage errors out and will not be implemented
    Given a fake Lua runtime
    And a passing test file "unit_test.lua" with test "adds"
    When I run "luabox test --coverage" with the fake runtime
    Then the command fails
    And stderr contains "--coverage is not implemented"
    And stderr contains "deprecated"

  Scenario: a clear error when no Lua runtime can be resolved
    Given a passing test file "unit_test.lua" with test "adds"
    When I run "luabox test" with env "LUABOX_LUA=luabox-no-such-runtime-xyz"
    Then the command fails
    And stderr contains "LUABOX_LUA"

  Scenario: discovers test files and fails when a test fails
    Given a fake Lua runtime
    And a passing test file "alpha_test.lua" with test "alpha works"
    And a failing test file "beta_test.lua" with test "beta broke" failing with "expected 1 but got 2"
    When I run "luabox test" with the fake runtime
    Then the command fails
    And stdout contains "PASS alpha works"
    And stdout contains "FAIL beta broke"
    And stdout contains "expected 1 but got 2"
    And stdout contains "1 passed; 1 failed"

  Scenario: a fully passing suite succeeds
    Given a fake Lua runtime
    And a passing test file "ok_test.lua" with test "works"
    When I run "luabox test" with the fake runtime
    Then the command succeeds
    And stdout contains "test result: ok"

  Scenario: a path pattern runs only the matching file
    Given a fake Lua runtime
    And a passing test file "alpha_test.lua" with test "alpha works"
    And a failing test file "beta_test.lua" with test "beta broke" failing with "boom"
    When I run "luabox test alpha" with the fake runtime
    Then the command succeeds
    And stdout contains "alpha works"

  Scenario: no test files is a friendly no-op, not a failure
    Given a fake Lua runtime
    When I run "luabox test" with the fake runtime
    Then the command succeeds
    And stdout contains "no test files found"

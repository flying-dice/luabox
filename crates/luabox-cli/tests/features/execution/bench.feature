Feature: luabox bench — criterion-style benchmarking across runtimes (SPEC.md §11)
  Zero-config discovery (`*_bench.lua`, `*.bench.lua`, `bench/`), driving the
  embedded bench harness against every Lua runtime found on PATH. Benches
  never fail the build: the command always exits 0.

  These scenarios are hermetic: they never require a real Lua. A "fake bench
  runtime" (a tiny `.bat` shim pointed at via `LUABOX_LUA`) echoes each bench
  file — authored here as raw `LUABOX_BENCH_*` protocol — so discovery and
  the stats table are exercised without an interpreter installed. The real
  harness (adaptive batching, `os.clock()` timing) is proven end-to-end by
  the `luabox-test` integration tests, behind a runtime probe.

  Scenario: discovers a bench file and reports its stats via the fake runtime
    Given a fake bench runtime
    And a bench file "fib_bench.lua" with bench "fib(20)" producing samples "120.0,118.0,121.0"
    When I run "luabox bench" with the fake bench runtime
    Then the command succeeds
    And stdout contains "fib(20)"
    And stdout contains "LUABOX_LUA"

  Scenario: no bench files is a friendly no-op, not a failure
    Given a fake bench runtime
    When I run "luabox bench" with the fake bench runtime
    Then the command succeeds
    And stdout contains "no bench files found"

  Scenario: the comparison table has the expected column shape
    Given a fake bench runtime
    And a bench file "sort_bench.lua" with bench "sort(1000)" producing samples "10.0,11.0,9.0,10.5"
    When I run "luabox bench" with the fake bench runtime
    Then the command succeeds
    And stdout contains "BENCH"
    And stdout contains "RUNTIME"
    And stdout contains "MEDIAN"
    And stdout contains "MEAN"
    And stdout contains "MIN"
    And stdout contains "OUTLIERS"

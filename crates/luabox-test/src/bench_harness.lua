-- luabox bench harness — embedded asset, loaded by `luabox bench`.
--
-- Invoked as: <runtime> <this-file> <bench_file.lua> [more_bench_files...]
--
-- Lua-side API (registered while bench files are dofile()'d):
--   bench(name, fn)
--   bench(name, { setup = fn, iters = n }, fn)
--
-- `setup`, if given, is called once (before warmup) to build a fixture;
-- its return value is passed as the sole argument to `fn` on every call,
-- warmup and timed alike. `iters`, if given, fixes the batch size and
-- skips adaptive calibration — pick it large enough that a batch clears
-- one tick of `os.clock()`'s resolution, or every batch reads as 0ns.
--
-- Protocol (TAB-separated; same field escaping as the test harness: `\\`,
-- `\n`, `\r`, `\t`):
--   LUABOX_BENCH_RUNTIME <version>
--   LUABOX_BENCH_BEGIN   <name>
--   LUABOX_BENCH_RESULT  <name> <ns_per_iter>   -- one line per timed batch
--   LUABOX_BENCH_ERROR   <name> <message>
--   LUABOX_BENCH_DONE    <count>
--
-- Criterion-lite protocol per bench:
--   1. warmup: run `fn(state)` until ~50ms elapsed or 10 iterations,
--      whichever comes first (JIT/cache warmup; not measured).
--   2. calibrate (skipped when `iters` is given): double the batch size
--      from 1 until one batch takes >= 10ms via os.clock(), so per-call
--      timer overhead is negligible relative to the batch.
--   3. measure: run timed batches of that size, each reported as one
--      RESULT line (ns/iter for that batch), until >= 10 batches have run
--      or ~1s total has elapsed, whichever comes first. The Rust side
--      computes median/mean/stddev/outliers across the reported batches.
--
-- A bench whose setup or timed function raises is reported as
-- LUABOX_BENCH_ERROR and skipped; the harness itself always exits 0 —
-- benches never fail the build (SPEC.md §11).
--
-- Maximally portable: Lua 5.1-compatible (also runs on 5.2/5.3/5.4 and
-- LuaJIT). No goto, no bitops, no integer division, no os.exit(boolean).

local bench_count = 0

local function esc(s)
  s = string.gsub(s, "\\", "\\\\")
  s = string.gsub(s, "\n", "\\n")
  s = string.gsub(s, "\r", "\\r")
  s = string.gsub(s, "\t", "\\t")
  return s
end

local function emit(...)
  local parts = { ... }
  local n = select("#", ...)
  for i = 1, n do
    parts[i] = esc(tostring(parts[i]))
  end
  io.write(table.concat(parts, "\t", 1, n))
  io.write("\n")
end

-- ------------------------------------------------------------------ --
-- Collector.
-- ------------------------------------------------------------------ --

local benches = {}

function bench(name, opts_or_fn, maybe_fn)
  local opts, fn
  if maybe_fn ~= nil then
    opts = opts_or_fn
    fn = maybe_fn
  else
    opts = {}
    fn = opts_or_fn
  end
  benches[#benches + 1] = { name = name, setup = opts.setup, iters = opts.iters, fn = fn }
end

-- ------------------------------------------------------------------ --
-- Timing.
-- ------------------------------------------------------------------ --

local WARMUP_SECONDS = 0.05
local WARMUP_MAX_ITERS = 10
local MIN_BATCH_SECONDS = 0.01
local MIN_BATCHES = 10
local MAX_TOTAL_SECONDS = 1.0
-- Defensive caps, not part of the timing budget: guarantee termination
-- even for a pathologically cheap function on a coarse clock.
local MAX_CALIBRATE_ITERS = 100000000
local MAX_BATCHES = 1000

-- Run `fn(state)` `n` times back-to-back; return elapsed seconds.
local function timed_run(fn, state, n)
  local start = os.clock()
  for _ = 1, n do
    fn(state)
  end
  return os.clock() - start
end

local function warmup(fn, state)
  local iters = 0
  local start = os.clock()
  while true do
    fn(state)
    iters = iters + 1
    if os.clock() - start >= WARMUP_SECONDS or iters >= WARMUP_MAX_ITERS then
      break
    end
  end
end

-- Double the batch size from 1 until one batch takes >= MIN_BATCH_SECONDS.
-- Returns the calibrated batch size; the calibration runs themselves are
-- not reported as results.
local function calibrate(fn, state)
  local n = 1
  while true do
    local elapsed = timed_run(fn, state, n)
    if elapsed >= MIN_BATCH_SECONDS or n >= MAX_CALIBRATE_ITERS then
      return n
    end
    n = n * 2
  end
end

local function run_bench(case)
  emit("LUABOX_BENCH_BEGIN", case.name)

  local ok, err = pcall(function()
    local state = nil
    if case.setup then
      state = case.setup()
    end

    warmup(case.fn, state)

    local batch_size = case.iters
    if batch_size == nil or batch_size < 1 then
      batch_size = calibrate(case.fn, state)
    end

    local batches_run = 0
    local total_elapsed = 0
    while true do
      local elapsed = timed_run(case.fn, state, batch_size)
      total_elapsed = total_elapsed + elapsed
      batches_run = batches_run + 1
      local ns_per_iter = (elapsed / batch_size) * 1e9
      emit("LUABOX_BENCH_RESULT", case.name, ns_per_iter)
      if batches_run >= MIN_BATCHES or total_elapsed >= MAX_TOTAL_SECONDS
          or batches_run >= MAX_BATCHES then
        break
      end
    end
  end)

  if ok then
    bench_count = bench_count + 1
  else
    emit("LUABOX_BENCH_ERROR", case.name, tostring(err))
  end
end

-- ------------------------------------------------------------------ --
-- Run.
-- ------------------------------------------------------------------ --

emit("LUABOX_BENCH_RUNTIME", _VERSION or "unknown")

-- Load (register) every bench file passed as an argument.
for i = 1, #arg do
  local path = arg[i]
  local ok, err = pcall(dofile, path)
  if not ok then
    emit("LUABOX_BENCH_ERROR", path, "load error: " .. tostring(err))
  end
end

for _, case in ipairs(benches) do
  run_bench(case)
end

emit("LUABOX_BENCH_DONE", bench_count)
io.stdout:flush()
os.exit(0)

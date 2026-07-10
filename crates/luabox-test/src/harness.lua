-- luabox test harness — embedded asset, loaded by `luabox test`.
--
-- Invoked as: <runtime> <this-file> <test_file.lua> [more_test_files...]
--
-- Provides two authoring styles over one collector:
--   * flat native API:   test(name, fn)
--   * busted-compatible:  describe/it/before_each/after_each + assert.*
--
-- It dofile()s each test file (which registers cases), runs every case
-- (optionally filtered by the LUABOX_TEST_FILTER env var, plain substring
-- on the full test name), and prints a line-oriented machine protocol to
-- stdout for the Rust runner to parse. Fields are TAB-separated; newlines,
-- tabs, carriage returns and backslashes inside a field are escaped.
--
-- Protocol lines:
--   LUABOX_TEST_RUNTIME <version>
--   LUABOX_TEST_BEGIN   <name>
--   LUABOX_TEST_PASS    <name>
--   LUABOX_TEST_FAIL    <name> <message>
--   LUABOX_TEST_DONE    <passed> <failed>
--
-- Maximally portable: Lua 5.1-compatible (also runs on 5.2/5.3/5.4 and
-- LuaJIT). No goto, no bitops, no integer division, no os.exit(boolean).

local passed = 0
local failed = 0

-- Field escaping: keep every field on a single physical line and free of
-- the TAB delimiter. The Rust side reverses this exactly.
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
-- Collector: describe/it context stack + flat test().
-- ------------------------------------------------------------------ --

local cases = {}
local root_ctx = { name = nil, befores = {}, afters = {} }
local stack = { root_ctx }

local function current()
  return stack[#stack]
end

local function full_name(leaf)
  local parts = {}
  for i = 1, #stack do
    if stack[i].name then
      parts[#parts + 1] = stack[i].name
    end
  end
  parts[#parts + 1] = leaf
  return table.concat(parts, " ")
end

-- Snapshot the before_each/after_each hooks currently in scope: outer
-- describes run before inner ones; after_each unwinds inner-to-outer.
local function collect_hooks()
  local befores, afters = {}, {}
  for i = 1, #stack do
    for _, f in ipairs(stack[i].befores) do
      befores[#befores + 1] = f
    end
  end
  for i = #stack, 1, -1 do
    for _, f in ipairs(stack[i].afters) do
      afters[#afters + 1] = f
    end
  end
  return befores, afters
end

function describe(name, fn)
  stack[#stack + 1] = { name = name, befores = {}, afters = {} }
  local ok, err = pcall(fn)
  stack[#stack] = nil
  if not ok then
    -- A describe body that blows up mid-registration is itself a failure.
    error(err, 0)
  end
end

function before_each(fn)
  local ctx = current()
  ctx.befores[#ctx.befores + 1] = fn
end

function after_each(fn)
  local ctx = current()
  ctx.afters[#ctx.afters + 1] = fn
end

local function register(name, fn)
  local befores, afters = collect_hooks()
  cases[#cases + 1] = {
    name = full_name(name),
    fn = fn,
    befores = befores,
    afters = afters,
  }
end

function it(name, fn)
  register(name, fn)
end

-- Native flat API (SPEC.md §11).
function test(name, fn)
  register(name, fn)
end

-- ------------------------------------------------------------------ --
-- Assertions: busted-compatible `assert` table that is still callable
-- as the plain `assert(v, msg)` builtin.
-- ------------------------------------------------------------------ --

local real_assert = assert

local function deep_equal(a, b)
  if a == b then
    return true
  end
  if type(a) ~= "table" or type(b) ~= "table" then
    return false
  end
  for k, v in pairs(a) do
    if not deep_equal(v, b[k]) then
      return false
    end
  end
  for k in pairs(b) do
    if a[k] == nil then
      return false
    end
  end
  return true
end

local A = setmetatable({}, {
  __call = function(_, ...)
    return real_assert(...)
  end,
})

function A.equal(expected, actual, msg)
  if expected ~= actual then
    error(msg or ("expected " .. tostring(expected) .. " but got " .. tostring(actual)), 2)
  end
end
A.equals = A.equal

function A.same(expected, actual, msg)
  if not deep_equal(expected, actual) then
    error(msg or ("tables are not deeply equal: expected " .. tostring(expected)
      .. " but got " .. tostring(actual)), 2)
  end
end

function A.is_true(v, msg)
  if v ~= true then
    error(msg or ("expected true but got " .. tostring(v)), 2)
  end
end

function A.is_false(v, msg)
  if v ~= false then
    error(msg or ("expected false but got " .. tostring(v)), 2)
  end
end

function A.is_nil(v, msg)
  if v ~= nil then
    error(msg or ("expected nil but got " .. tostring(v)), 2)
  end
end

function A.truthy(v, msg)
  if not v then
    error(msg or ("expected a truthy value but got " .. tostring(v)), 2)
  end
end

function A.falsy(v, msg)
  if v then
    error(msg or ("expected a falsy value but got " .. tostring(v)), 2)
  end
end

-- assert.has_error(fn[, expected]) — fn must raise; if `expected` given,
-- the raised message must contain it (plain substring).
function A.has_error(fn, expected)
  local ok, err = pcall(fn)
  if ok then
    error("expected the function to raise an error, but it did not", 2)
  end
  if expected ~= nil then
    local text = tostring(err)
    if not string.find(text, tostring(expected), 1, true) then
      error("expected an error containing '" .. tostring(expected)
        .. "' but got '" .. text .. "'", 2)
    end
  end
end

-- busted's `assert.are.equal` / `assert.is.truthy` grouping aliases.
A.are = { equal = A.equal, equals = A.equal, same = A.same }
A.is = { truthy = A.truthy, falsy = A.falsy, is_true = A.is_true, is_nil = A.is_nil }

assert = A

-- ------------------------------------------------------------------ --
-- Run.
-- ------------------------------------------------------------------ --

local function run_case(case)
  emit("LUABOX_TEST_BEGIN", case.name)
  local err = nil
  for _, f in ipairs(case.befores) do
    if err == nil then
      local ok, e = pcall(f)
      if not ok then
        err = e
      end
    end
  end
  if err == nil then
    local ok, e = pcall(case.fn)
    if not ok then
      err = e
    end
  end
  -- after_each hooks always run, even after a failure.
  for _, f in ipairs(case.afters) do
    local ok, e = pcall(f)
    if not ok and err == nil then
      err = e
    end
  end
  if err == nil then
    emit("LUABOX_TEST_PASS", case.name)
    passed = passed + 1
  else
    emit("LUABOX_TEST_FAIL", case.name, tostring(err))
    failed = failed + 1
  end
end

emit("LUABOX_TEST_RUNTIME", _VERSION or "unknown")

-- Load (register) every test file passed as an argument.
for i = 1, #arg do
  local path = arg[i]
  local ok, err = pcall(dofile, path)
  if not ok then
    emit("LUABOX_TEST_BEGIN", path)
    emit("LUABOX_TEST_FAIL", path, "load error: " .. tostring(err))
    failed = failed + 1
  end
end

local filter = os.getenv("LUABOX_TEST_FILTER")
if filter == "" then
  filter = nil
end

for _, case in ipairs(cases) do
  if filter == nil or string.find(case.name, filter, 1, true) then
    run_case(case)
  end
end

emit("LUABOX_TEST_DONE", passed, failed)
io.stdout:flush()
os.exit(failed == 0 and 0 or 1)

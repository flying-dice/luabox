Feature: LuaCATS type syntax — every shape an annotation can take
  SPEC.md §3: annotations carry a small type grammar of their own — named and
  dotted types with generic arguments, `T?`, `T[]`, unions, tuples, table
  literals with fields and indexers, `fun(...)` signatures, literal types, and
  backtick captures. `luabox check` is the only place that grammar is
  observable, so each shape is pinned by the diagnostic it does (or does not)
  produce. Malformed type syntax recovers rather than derailing the file.

  # --- `fun(...)` signatures ----------------------------------------------

  Scenario: a `fun` type with named parameters and a return accepts a matching lambda
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@param cb fun(a: number, b: string): boolean
      local function go(cb) end

      go(function(a, b) return true end)
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

  Scenario: a `fun` type may declare optional and variadic parameters
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@param cb fun(a?: number, ...: string): number, string
      local function go(cb) end
      return go
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

  Scenario: a `fun` type may return nothing at all
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@class Widget
      ---@field on fun(self: Widget, name: string)

      ---@param w Widget
      local function use(w)
        w:on("click")
      end
      return use
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

  Scenario: a `fun` type may return another `fun` type
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@param mk fun(): fun(n: number): string
      local function use(mk) end
      return use
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

  Scenario: a bare `...` stands for both the parameter list and the returns
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@param f fun(...): ...
      local function use(f) end
      return use
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

  # --- table literal types ------------------------------------------------

  Scenario: a table literal type checks its fields structurally
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@type { x: number, y: string }
      local bad = { x = 1, y = 2 }
      return bad
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"
    And stdout contains "expected `string`, found `2`"

  Scenario: a table literal type with a string indexer accepts arbitrary keys
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@type { [string]: number }
      local counts = { hits = 1, misses = 2 }
      return counts
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

  Scenario: an indexer key may itself be a literal type
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@type { [1]: string }
      local first = { "a" }
      return first
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

  Scenario: a union of table literal types is a discriminated shape
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@type { kind: "a" }|{ kind: "b" }
      local tagged = { kind = "a" }
      return tagged
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

  # --- tuples, arrays, optionals, parentheses -----------------------------

  Scenario: a tuple type accepts a positional table literal
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@type [number, string]
      local pair = { 1, "a" }
      return pair
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

  Scenario: `[]` nests, so `number[][]` is an array of arrays
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@type number[][]
      local flat = { 1 }
      return flat
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"
    And stdout contains "expected `number[]`, found `1`"

  Scenario: parentheses group a union so `[]` applies to the whole of it
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@type (number|string)[]
      local mixed = { 1, "a" }
      ---@type number?
      local maybe = nil
      return mixed, maybe
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

  Scenario: a tuple may be one arm of a union
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@type [number, string]|nil
      local pair
      return pair
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

  # --- literal and built-in types -----------------------------------------

  Scenario Outline: a literal-type union rejects a value outside it
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@type <union>
      local bad = <value>
      return bad
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"
    And stdout contains 'expected `<rendered>`, found `<found>`'

    Examples: double-quoted, single-quoted, numeric, and boolean literals
      | union | value      | rendered | found      |
      | "on"  | "sideways" | "on"     | "sideways" |
      | 'yes' | 'maybe'    | "yes"    | "maybe"    |
      | 1     | 7          | 1        | 7          |
      | true  | false      | true     | false      |

  Scenario: every built-in scalar name is a known type
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@type any
      local a
      ---@type unknown
      local u
      ---@type userdata
      local ud
      ---@type thread
      local th
      ---@type nil
      local n
      ---@type integer
      local i = 1
      return a, u, ud, th, n, i
      """
    When I run "luabox check"
    Then the command succeeds
    And stdout does not contain "LB0305"

  # --- named types with generic arguments ---------------------------------

  Scenario: `table<K, V>` checks the value type of an entry
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@type table<string, number>
      local bad = { a = "x" }
      return bad
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"
    And stdout contains 'expected `number`, found `"x"`'

  Scenario: a dotted type name that resolves to nothing is an unknown type
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@type foo.Bar
      local x
      return x
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0305"
    And stdout contains "unknown type name `foo.Bar` in annotation"

  Scenario: a backtick capture names a type parameter, not an unknown type
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@generic T
      ---@param name `T`
      ---@return T
      local function new(name) end
      return new
      """
    When I run "luabox check"
    Then the command succeeds
    And stdout does not contain "LB0305"

  # --- malformed type syntax recovers -------------------------------------

  Scenario Outline: a malformed type annotation does not derail the rest of the file
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@type <annotation>
      local ignored

      ---@param n number
      local function double(n) return n * 2 end

      double("nope")
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"
    And stdout contains 'expected `number`, found `"nope"`'

    Examples: unclosed brackets, stray punctuation, and truncated constructs
      | annotation           |
      | { x: number          |
      | (number              |
      | number,,string       |
      | { [string: number }  |
      | { x number }         |
      | "on                  |
      | ?                    |
      | fun(a: number        |
      | [number, string      |
      | table<string         |

  Scenario: a union with nothing after the bar still checks the rest of the file
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@type number|
      local ignored

      ---@param n number
      local function double(n) return n * 2 end

      double("nope")
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains 'expected `number`, found `"nope"`'

  Scenario: an unterminated generic argument list still resolves the base name
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@type Box<number
      local b
      return b
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0305"
    And stdout contains "unknown type name `Box` in annotation"

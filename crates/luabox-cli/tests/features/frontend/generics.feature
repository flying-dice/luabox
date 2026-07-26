Feature: luabox check — generic annotations
  SPEC.md §3: `---@generic T[: Constraint]` introduces type parameters on a
  function; `---@class Name<T>` and `---@alias Name<T>` introduce them on a
  type. Type arguments are inferred luals-style from the first argument that
  fixes each parameter, then substituted through the rest of the signature —
  so a second argument that disagrees with the first is a plain LB0300. The
  arity of an explicit `<...>` instantiation is checked (LB0313), and a
  `: Constraint` bound is enforced against the inferred argument.

  Scenario: a generic function threads its argument type through to the return
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@generic T
      ---@param x T
      ---@return T
      local function id(x)
        return x
      end

      ---@param n number
      local function wants(n) end

      wants(id(42))
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

  Scenario: a second argument that disagrees with the inferred binding is a mismatch
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@generic T
      ---@param a T
      ---@param b T
      ---@return T
      local function pick(a, b)
        return a
      end

      pick(5, "x")
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"
    And stdout contains "type mismatch: expected `integer`, found"

  Scenario: a type argument that violates its constraint is reported
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@class Shape
      ---@field area fun(self): number

      ---@generic T : Shape
      ---@param x T
      ---@return T
      local function identity(x)
        return x
      end

      identity(5)
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"
    And stdout contains "type argument `T` = `integer` does not satisfy constraint `Shape`"

  Scenario: a generic class instantiated with a concrete argument checks its fields
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@class Pair<T>
      ---@field first T
      ---@field second T

      ---@type Pair<number>
      local p = { first = 1, second = 2 }
      return p
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

  Scenario: a field violating the instantiated type parameter is a mismatch
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@class Pair<T>
      ---@field first T
      ---@field second T

      ---@type Pair<number>
      local p = { first = 1, second = "x" }
      return p
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"
    And stdout contains "type mismatch: expected `number`, found"

  Scenario: an uninstantiated generic class is deliberately lenient
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@class Pair<T>
      ---@field first T
      ---@field second T

      ---@type Pair
      local p = { first = 1, second = "anything" }
      return p
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

  Scenario: too many type arguments for a generic class names the expected arity
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@class Box<T>
      ---@field value T

      ---@type Box<number, string>
      local b = { value = 1 }
      return b
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0313"
    And stdout contains "`Box` takes 1 type argument, but 2 were supplied"
    And stdout contains "expected 1 type argument"

  Scenario: a generic alias is arity-checked the same way
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@alias Duo<T> { first: T, second: T }

      ---@type Duo<number, string>
      local x = { first = 1, second = 2 }
      return x
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0313"
    And stdout contains "`Duo` takes 1 type argument, but 2 were supplied"

  Scenario: a type parameter is not an unknown type name
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@generic T
      ---@param items T[]
      ---@return T
      local function head(items)
        return items[1]
      end
      return head
      """
    When I run "luabox check"
    Then the command succeeds
    And stdout does not contain "LB0305"

  Scenario: an arity error only warns without strict types
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@class Box<T>
      ---@field value T

      ---@type Box<number, string>
      local b = { value = 1 }
      return b
      """
    When I run "luabox check"
    Then the command succeeds
    And stdout contains "warning[LB0313]"

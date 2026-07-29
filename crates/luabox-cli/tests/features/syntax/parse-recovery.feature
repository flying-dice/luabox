Feature: Parser error recovery — LB0001 says what was expected, and where
  SPEC.md §2 + §14: the parser is error-recovering, so a broken file still
  produces a tree and the command reports what it wanted rather than bailing
  at the first surprise. Every syntax problem is LB0001, and it is always a
  hard error — parse errors do not ride the `[types] strict` ladder the way
  the type diagnostics do.

  Scenario Outline: an unclosed construct names the delimiter it wanted
    Given a project with edition "5.4"
    And a Lua file containing '<source>'
    When I run "luabox check"
    Then the command fails
    And diagnostic LB0001 is reported
    And stdout contains "<message>"

    Examples: brackets, block keywords, and string delimiters
      | source                       | message                 |
      | local t = { 1, 2             | expected '}'            |
      | print((1 + 2                 | expected ')'            |
      | local function f() return 1  | expected 'end'          |
      | if x then print(1)           | expected 'end'          |
      | repeat print(1)              | expected 'until'        |
      | local s = "abc               | unterminated string     |
      | local s = [[abc              | unterminated long string |
      | --[[ never ends              | unterminated long comment |

  Scenario Outline: a missing block keyword is named rather than guessed at
    Given a project with edition "5.4"
    And a Lua file containing '<source>'
    When I run "luabox check"
    Then the command fails
    And stdout contains "<message>"

    Examples: the keywords that open a body
      | source                 | message         |
      | if x print(1) end      | expected 'then' |
      | while x print(1) end   | expected 'do'   |
      | for i = 1 do end       | expected ','    |

  Scenario Outline: a token that cannot start anything is reported where it sits
    Given a project with edition "5.4"
    And a Lua file containing '<source>'
    When I run "luabox check"
    Then the command fails
    And stdout contains "<message>"

    Examples: stray punctuation, keywords, and missing operands
      | source                | message                     |
      | local x = 1 @ 2       | unexpected '@'              |
      | end                   | unexpected 'end'            |
      | local x = 1 + + 2     | expected expression         |
      | function () end       | expected a name             |
      | return 1 local x = 2  | statement after 'return'    |

  Scenario: recovery keeps going, so one line can report several problems
    Given a project with edition "5.4"
    And a Lua file containing 'local end = 1'
    When I run "luabox check"
    Then the command fails
    And stdout contains "expected a name"
    And stdout contains "unexpected 'end'"
    And stderr contains "check: 4 errors, 0 warnings in 1 files"

  Scenario: a parse error is an error even when types are only warnings
    Given a project with edition "5.4"
    And a Lua file containing 'local = 5'
    When I run "luabox check"
    Then the command exits with code 1
    And stdout contains "error[LB0001]"

  Scenario: deeply nested but balanced parentheses parse cleanly
    Given a project with edition "5.4"
    And a Lua file containing 'local x = ((((((((((1)))))))))) return x'
    When I run "luabox check"
    Then the command succeeds
    And stdout does not contain "LB0001"

  Scenario: recovery still lets the rest of the file be typechecked
    Given a strict project with edition "5.4"
    And a file "src/broken.lua" containing:
      """
      local t = { 1, 2
      """
    And a file "src/typed.lua" containing:
      """
      ---@param n number
      local function double(n)
        return n * 2
      end

      double("nope")
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0001"
    And stdout contains "LB0300"
    And stderr contains "in 2 files"

  Scenario Outline: the diagnostic names the exact token the grammar wanted
    Given a project with edition "5.4"
    And a Lua file containing '<source>'
    When I run "luabox check"
    Then the command fails
    And stdout contains "<message>"

    Examples: keywords, brackets, and punctuation
      | source                   | message         |
      | for k v in pairs(t) do end | expected 'in'   |
      | ::lbl                    | expected '::'   |
      | local t = {} t[1 = 2     | expected ']'    |
      | local x = a[             | expected ']'    |
      | goto                     | expected a name |
      | local a, = 1             | expected a name |
      | local x = 1 local        | expected a name |
      | x = function(a b) end    | expected ')'    |
      | f(a,)                    | expected expression |

  Scenario: a table field list wants a separator between entries
    Given a project with edition "5.4"
    And a Lua file containing 'local t = { 1 2 }'
    When I run "luabox check"
    Then the command fails
    And stdout contains "expected ',' or ';' between table fields"

  # 300 levels: comfortably past luabox's MAX_DEPTH (220), which is itself set
  # above what reference Lua accepts (~197, LUAI_MAXCCALLS).
  Scenario: nesting past the parser's depth limit bails out once, not per level
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      local x = ((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((1))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))
      return x
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains exactly 1 occurrence of "nesting limit exceeded"

  Scenario: a deeply nested table constructor hits the same single bailout
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      local t = {{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}
      return t
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains exactly 1 occurrence of "nesting limit exceeded"

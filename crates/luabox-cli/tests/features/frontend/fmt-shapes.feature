Feature: Canonical formatting — the shape of every construct
  SPEC.md §10: `luabox fmt` re-emits the whole tree, so every statement,
  expression, comment position and literal form has a canonical shape. These
  scenarios pin the emitted layout construct by construct, and each one is
  written so a second `luabox fmt` is a no-op — idempotence is the property
  that makes the formatter safe to run in a pre-commit hook.

  # --- statements ---------------------------------------------------------

  Scenario: every block-opening statement puts its body on its own indented line
    Given an empty directory
    And a file "main.lua" containing:
      """
      do local c = 3 end
      while true do break end
      repeat local x = 1 until x
      for i=1,10,2 do end
      for k,v in pairs(t) do end
      """
    When I run "luabox fmt"
    Then the command succeeds
    And "main.lua" equals:
      """
      do
          local c = 3
      end
      while true do
          break
      end
      repeat
          local x = 1
      until x
      for i = 1, 10, 2 do
      end
      for k, v in pairs(t) do
      end
      """

  Scenario: an if/elseif/else chain keeps each keyword at the outer indent
    Given an empty directory
    And a file "main.lua" containing:
      """
      if a then elseif b then else end
      """
    When I run "luabox fmt"
    Then the command succeeds
    And "main.lua" equals:
      """
      if a then
      elseif b then
      else
      end
      """

  Scenario: function declarations, varargs, labels and gotos keep their spelling
    Given an empty directory
    And a file "main.lua" containing:
      """
      goto done
      ::done::
      function M.f(a,b) return a+b end
      local function g(...) return ... end
      """
    When I run "luabox fmt"
    Then the command succeeds
    And "main.lua" equals:
      """
      goto done
      ::done::
      function M.f(a, b)
          return a + b
      end
      local function g(...)
          return ...
      end
      """

  Scenario: a semicolon the author wrote is preserved, spacing and all
    Given an empty directory
    And a file "main.lua" containing:
      """
      local a = 1;
      local b = 2 ;;
      """
    When I run "luabox fmt"
    Then the command succeeds
    And "main.lua" equals:
      """
      local a = 1;
      local b = 2;;
      """

  Scenario: nested blocks indent by one level each
    Given an empty directory
    And a file "main.lua" containing:
      """
      if a then if b then if c then print(1) end end end
      """
    When I run "luabox fmt"
    Then the command succeeds
    And "main.lua" equals:
      """
      if a then
          if b then
              if c then
                  print(1)
              end
          end
      end
      """

  # --- tables -------------------------------------------------------------

  Scenario: a table constructor is spaced inside its braces and collapses onto one line
    Given an empty directory
    And a file "main.lua" containing:
      """
      local t = {1,2,3}
      local u = {a=1,b=2}
      local v = {
        a = 1,
        b = 2,
      }
      local w = {}
      local nested = {a={b={c=1}}}
      local mixed = {1,2,x=3,[4]="y"}
      """
    When I run "luabox fmt"
    Then the command succeeds
    And "main.lua" equals:
      """
      local t = { 1, 2, 3 }
      local u = { a = 1, b = 2 }
      local v = { a = 1, b = 2 }
      local w = {}
      local nested = { a = { b = { c = 1 } } }
      local mixed = { 1, 2, x = 3, [4] = "y" }
      """

  # --- calls --------------------------------------------------------------

  Scenario: the sugar call forms keep their shape, separated by one space
    Given an empty directory
    And a file "main.lua" containing:
      """
      f{1,2}
      g"str"
      require "mod"
      obj:method(1):chain(2):more(3)
      local s = ("x"):rep(3)
      """
    When I run "luabox fmt"
    Then the command succeeds
    And "main.lua" equals:
      """
      f { 1, 2 }
      g "str"
      require "mod"
      obj:method(1):chain(2):more(3)
      local s = ("x"):rep(3)
      """

  Scenario: a function argument breaks across lines with the call parentheses hugging it
    Given an empty directory
    And a file "main.lua" containing:
      """
      h(function() return 1 end)
      """
    When I run "luabox fmt"
    Then the command succeeds
    And "main.lua" equals:
      """
      h(function()
          return 1
      end)
      """

  # --- operators and attributes -------------------------------------------

  Scenario: every binary operator is surrounded by exactly one space
    Given an empty directory
    And a file "main.lua" containing:
      """
      local z = a and b or c
      local w = -x ^ 2
      local u = a .. b .. c
      local s = a<b and b>c
      local r = #t
      local q = a//b
      local p = a~b
      """
    When I run "luabox fmt"
    Then the command succeeds
    And "main.lua" equals:
      """
      local z = a and b or c
      local w = -x ^ 2
      local u = a .. b .. c
      local s = a < b and b > c
      local r = #t
      local q = a // b
      local p = a ~ b
      """

  Scenario: a local attribute stays attached to the name it qualifies
    Given an empty directory
    And a file "main.lua" containing:
      """
      local x <const> = 1
      local y <close> = setmetatable({},{__close=function() end})
      """
    When I run "luabox fmt"
    Then the command succeeds
    And "main.lua" equals:
      """
      local x <const> = 1
      local y <close> = setmetatable({}, { __close = function() end })
      """

  # --- comments in awkward positions --------------------------------------

  Scenario: a comment keeps the position it was written in
    Given an empty directory
    And a file "main.lua" containing:
      """
      -- leading
      local a = 1 -- trailing
      local c = --[[inline]] 3
      function f() -- after the header
        -- inside
      end
      if x then -- after then
        -- body
      end
      """
    When I run "luabox fmt"
    Then the command succeeds
    And "main.lua" equals:
      """
      -- leading
      local a = 1 -- trailing
      local c = --[[inline]] 3
      function f() -- after the header
          -- inside
      end
      if x then -- after then
          -- body
      end
      """

  Scenario: a block comment that opens a line becomes a line of its own
    Given an empty directory
    And a file "main.lua" containing:
      """
      --[[ block ]] local b = 2
      """
    When I run "luabox fmt"
    Then the command succeeds
    And "main.lua" equals:
      """
      --[[ block ]]
      local b = 2
      """

  Scenario: a comment inside a table forces the table to stay expanded
    Given an empty directory
    And a file "main.lua" containing:
      """
      local t = { -- in table
        a = 1, -- after field
      }
      """
    When I run "luabox fmt"
    Then the command succeeds
    And "main.lua" equals:
      """
      local t = { -- in table
          a = 1, -- after field
      }
      """

  # --- literals -----------------------------------------------------------

  Scenario: numeric literals are re-emitted exactly as written
    Given an empty directory
    And a file "main.lua" containing:
      """
      local a = 0xFF
      local b = 1e10
      local c = 1E10
      local d = .5
      local e = 5.
      local f = 0x1p4
      local g = 0X1.8P3
      local h = 1e-5
      local i = 3.14159
      """
    When I run "luabox fmt"
    Then the command succeeds
    And "main.lua" equals:
      """
      local a = 0xFF
      local b = 1e10
      local c = 1E10
      local d = .5
      local e = 5.
      local f = 0x1p4
      local g = 0X1.8P3
      local h = 1e-5
      local i = 3.14159
      """

  Scenario: string escapes and long-bracket levels survive a round trip
    Given an empty directory
    And a file "main.lua" containing:
      """
      local a = [[plain]]
      local b = [==[level two]==]
      local c = "tab\there"
      local e = "\097\010"
      """
    When I run "luabox fmt"
    Then the command succeeds
    And "main.lua" equals:
      """
      local a = [[plain]]
      local b = [==[level two]==]
      local c = "tab\there"
      local e = "\097\010"
      """

  # --- idempotence --------------------------------------------------------

  Scenario: a second pass over a mixed file changes nothing
    Given an empty directory
    And a file "main.lua" containing:
      """
      local t = {1,2,3} -- keep
      function M.f(a,b) return a+b end
      for i=1,10 do print(i) end
      """
    And I run "luabox fmt"
    When I run "luabox fmt"
    Then the command succeeds
    And stdout contains "(0 changed)"
    And "main.lua" equals:
      """
      local t = { 1, 2, 3 } -- keep
      function M.f(a, b)
          return a + b
      end
      for i = 1, 10 do
          print(i)
      end
      """

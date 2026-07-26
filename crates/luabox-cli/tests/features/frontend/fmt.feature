Feature: Canonical formatting — luabox fmt
  SPEC.md §10: StyLua-compatible canonical style, idempotent, `--check` for
  CI. Formatting never changes what a program means: sources that do not
  parse cleanly are left untouched, byte for byte.

  Scenario: formats a messy Lua file to canonical style
    Given an empty directory
    And I run "luabox init --edition 5.4"
    And a file "src/main.lua" containing:
      """
      local x   =1
      if x   then
      print( 'hi' )
      end
      """
    When I run "luabox fmt"
    Then the command succeeds
    And stdout contains "(1 changed)"
    And "src/main.lua" equals:
      """
      local x = 1
      if x then
          print("hi")
      end
      """

  Scenario: formatting is idempotent
    Given an empty directory
    And a file "main.lua" containing:
      """
      local t={1,2,   3}
      """
    And I run "luabox fmt"
    When I run "luabox fmt"
    Then the command succeeds
    And stdout contains "(0 changed)"
    And "main.lua" equals:
      """
      local t = { 1, 2, 3 }
      """

  Scenario: check mode fails on unformatted code and writes nothing
    Given an empty directory
    And a file "main.lua" containing:
      """
      local x   = 1
      """
    When I run "luabox fmt --check"
    Then the command fails
    And stdout contains "would reformat main.lua"
    And stderr contains "would be reformatted"
    And "main.lua" equals:
      """
      local x   = 1
      """

  Scenario: check mode passes on formatted code
    Given an empty directory
    And a file "main.lua" containing:
      """
      local x = 1
      """
    When I run "luabox fmt --check"
    Then the command succeeds
    And stdout contains "all formatted"

  Scenario: broken Lua is left untouched
    Given an empty directory
    And a file "broken.lua" containing:
      """
      local = 5
      """
    When I run "luabox fmt"
    Then the command succeeds
    And stdout contains "(0 changed)"
    And "broken.lua" equals:
      """
      local = 5
      """

  # --- string literal normalization ---------------------------------------

  Scenario: single-quoted strings are normalized to the preferred double quote
    Given an empty directory
    And a file "main.lua" containing:
      """
      local a = 'single'
      """
    When I run "luabox fmt"
    Then the command succeeds
    And "main.lua" equals:
      """
      local a = "single"
      """

  Scenario: the now-redundant quote escape is dropped when converting
    Given an empty directory
    And a file "main.lua" containing:
      """
      local a = 'it\'s'
      """
    When I run "luabox fmt"
    Then the command succeeds
    And "main.lua" equals:
      """
      local a = "it's"
      """

  Scenario: a string containing a double quote keeps its single quotes
    Given an empty directory
    And a file "main.lua" containing:
      """
      local a = 'say "hi"'
      """
    When I run "luabox fmt"
    Then the command succeeds
    And "main.lua" equals:
      """
      local a = 'say "hi"'
      """

  Scenario: an escaped double quote also blocks the conversion
    Given an empty directory
    And a file "main.lua" containing:
      """
      local a = 'say \"hi\"'
      """
    When I run "luabox fmt"
    Then the command succeeds
    And "main.lua" equals:
      """
      local a = 'say \"hi\"'
      """

  Scenario: every other escape survives the conversion byte for byte
    Given an empty directory
    And a file "main.lua" containing:
      """
      local a = 'a\nb\116'
      local b = '\65\x42'
      local c = 'tab\there'
      local d = '\u{48}i'
      """
    When I run "luabox fmt"
    Then the command succeeds
    And "main.lua" equals:
      """
      local a = "a\nb\116"
      local b = "\65\x42"
      local c = "tab\there"
      local d = "\u{48}i"
      """

  Scenario: long-bracket strings are never requoted, at any level
    Given an empty directory
    And a file "main.lua" containing:
      """
      local a = [[long bracket]]
      local b = [==[nested ]] bracket]==]
      """
    When I run "luabox fmt"
    Then the command succeeds
    And "main.lua" equals:
      """
      local a = [[long bracket]]
      local b = [==[nested ]] bracket]==]
      """

  Scenario: requoting is idempotent
    Given an empty directory
    And a file "main.lua" containing:
      """
      local a = 'it\'s'
      local b = 'say "hi"'
      local c = [[raw]]
      """
    And I run "luabox fmt"
    When I run "luabox fmt"
    Then the command succeeds
    And stdout contains "(0 changed)"

  Scenario: check mode notices a requoting-only difference
    Given an empty directory
    And a file "main.lua" containing:
      """
      local a = 'single'
      """
    When I run "luabox fmt --check"
    Then the command fails
    And stdout contains "would reformat main.lua"

  Scenario: the build output directory is skipped
    Given an empty directory
    And I run "luabox init --edition 5.4"
    And a file "dist/generated.lua" containing:
      """
      local x   =1
      """
    When I run "luabox fmt"
    Then the command succeeds
    And "dist/generated.lua" equals:
      """
      local x   =1
      """

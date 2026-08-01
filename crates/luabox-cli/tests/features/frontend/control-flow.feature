Feature: Control-flow legality — goto / label / break (#44)
  Three programs every reference Lua refuses to *load* used to pass `luabox
  check` and `luabox lint` clean: a `goto` naming no visible label, a label
  declared twice in one scope, and a `break` with no enclosing loop. They are
  errors (`LB0020`, `LB0021`, `LB0022`), not lints — the file would never
  run — and they are edition-independent with one exception: duplicate-label
  scope tightened in 5.4, which is why `--target` has to be judged too (see
  the `--target` block below). The `break` rule holds in every edition; the
  goto/label rules hold in every edition that has `goto` at all.

  Under `edition = "5.1"` the two halves differ, measured: `::a::` parses and
  is `LB0010` (dialect legality), while `goto` is not in the 5.1 grammar at
  all and is `LB0001` (a parse error) — so neither collects a second
  control-flow complaint, but for different reasons.

  Every verdict below is the verdict of `luac5.4 -p` (and `luac5.1 -p` for
  `break`) on the same program.

  Scenario Outline: illegal control flow is rejected the way reference Lua rejects it
    Given a project with edition "<edition>"
    And a Lua file containing '<source>'
    When I run "luabox check"
    Then the command fails
    And diagnostic <code> is reported

    Examples: no visible label for `goto` (LB0020)
      | edition | source                                                            | code   |
      | 5.4     | local function f() goto nowhere end                               | LB0020 |
      | 5.4     | goto zzz                                                          | LB0020 |
      | 5.4     | ::a:: local f = function() goto a end                             | LB0020 |
      | 5.4     | do ::a:: end do goto a end                                        | LB0020 |
      | 5.4     | for i=1,3 do local f=function() goto continue end ::continue:: end | LB0020 |
      | 5.2     | local function f() goto nowhere end                               | LB0020 |
      | luajit  | local function f() goto nowhere end                               | LB0020 |

    Examples: label already defined (LB0021)
      | edition | source                             | code   |
      | 5.4     | local function f() ::a:: ::a:: end | LB0021 |
      | 5.4     | ::a:: ::b:: ::a::                  | LB0021 |
      | 5.4     | ::a:: do ::a:: end                 | LB0021 |
      | 5.2     | ::a:: ::a::                        | LB0021 |
      | luajit  | ::a:: ::a::                        | LB0021 |

    Examples: `break` outside a loop (LB0022) — every edition, `goto` or not
      | edition | source                                          | code   |
      | 5.4     | local x = 1 break                               | LB0022 |
      | 5.4     | for i=1,3 do end break                          | LB0022 |
      | 5.4     | do break end                                    | LB0022 |
      | 5.4     | while true do local f = function() break end end | LB0022 |
      | 5.1     | local x = 1 break                               | LB0022 |
      | 5.1     | while true do local f = function() break end end | LB0022 |
      | luajit  | for i=1,3 do end break                          | LB0022 |

  Scenario Outline: legal control flow is left alone
    Given a project with edition "<edition>"
    And a Lua file containing '<source>'
    When I run "luabox check"
    Then the command succeeds
    And no control-flow diagnostic is reported

    Examples: `goto` reaching a visible label — forwards, backwards, outwards
      | edition | source                                                              |
      | 5.4     | do goto a end ::a::                                                 |
      | 5.4     | do do goto a end end ::a::                                          |
      | 5.4     | ::a:: do goto a end                                                 |
      | 5.4     | ::top:: goto top                                                    |
      | 5.4     | goto done print(1) ::done::                                         |
      | 5.4     | for i=1,3 do if i==2 then goto continue end print(i) ::continue:: end |
      | 5.4     | repeat goto cont ::cont:: until true                                |
      | 5.2     | for i=1,3 do goto continue ::continue:: end                         |
      | luajit  | for i=1,3 do goto continue ::continue:: end                         |

    Examples: the same label name in different scopes
      | edition | source                                                    |
      | 5.4     | do ::a:: end do ::a:: end                                 |
      | 5.4     | do ::a:: end ::a::                                        |
      | 5.4     | if x then ::a:: else ::a:: end                            |
      | 5.4     | local function f() ::a:: end local function g() ::a:: end |
      | 5.2     | ::a:: do ::a:: end                                        |
      | luajit  | ::a:: do ::a:: end                                        |

    Examples: `break` inside a loop, through `do`/`if` blocks
      | edition | source                                                        |
      | 5.4     | while true do break end                                       |
      | 5.4     | repeat break until true                                       |
      | 5.4     | for i=1,3 do break end                                        |
      | 5.4     | for k in pairs({}) do break end                               |
      | 5.4     | while true do do break end end                                |
      | 5.4     | while true do if x then break end end                         |
      | 5.4     | while true do local f = function() while c do break end end end |
      | 5.1     | while true do do break end end                                |
      | 5.1     | for i=1,3 do break end                                        |

  # `--target` asks "would this source be legal there?" (see `check`'s module
  # doc), and the duplicate-label rule is the one control-flow rule that
  # differs by edition — `checkrepeated` tightened in 5.4. So the legality
  # pass has to run for the ship target too, not just the edition
  # (Shockwave round 2). Verdicts below are `luac5.4 -p` / `luac5.2 -p`.
  Scenario Outline: control-flow legality is judged for the --target too
    Given a project with edition "<edition>"
    And a Lua file containing '<source>'
    When I run "luabox check --target <target>"
    Then the command fails
    And diagnostic <code> is reported

    Examples: legal in the edition, unloadable on the ship target
      | edition | target | source                     | code   |
      | 5.2     | 5.4    | ::a:: do ::a:: end         | LB0021 |
      | 5.3     | 5.4    | ::a:: do ::a:: end         | LB0021 |
      | luajit  | 5.4    | ::a:: do ::a:: end         | LB0021 |
      | 5.2     | 5.4    | ::a:: do do ::a:: end end  | LB0021 |

  Scenario Outline: a target that accepts the source stays quiet
    Given a project with edition "<edition>"
    And a Lua file containing '<source>'
    When I run "luabox check --target <target>"
    Then the command succeeds
    And no control-flow diagnostic is reported

    Examples: legal under both the edition and the target
      | edition | target | source             |
      | 5.4     | 5.2    | do ::a:: end ::a:: |
      | 5.2     | 5.4    | do ::a:: end ::a:: |
      | 5.2     | 5.2    | ::a:: do ::a:: end |
      | 5.4     | 5.4    | do ::a:: end ::a:: |

  Scenario: the duplicate label is reported once, not once per legality pass
    Given a project with edition "5.4"
    And a Lua file containing '::a:: do ::a:: end'
    When I run "luabox check --target 5.4"
    Then the command fails
    And diagnostic LB0021 is reported
    And stdout contains exactly 1 occurrence of "error[LB0021]"

  # The control that proves `--target` is live at pass 2 while pass 3 stays
  # quiet: `goto` and `::a::` are both illegal in 5.1, so the reader gets two
  # LB0010s and nothing from the control-flow pass (which skips goto/label for
  # editions without `goto` at all). Shockwave round 3's exact shape.
  Scenario: `goto` on a 5.1 target stays the dialect finding, with no second complaint
    Given a project with edition "5.2"
    And a Lua file containing 'do goto a end ::a::'
    When I run "luabox check --target 5.1"
    Then the command fails
    And diagnostic LB0010 is reported
    And stdout contains exactly 2 occurrence of "error[LB0010]"
    And no control-flow diagnostic is reported

  Scenario: the same program is clean on the edition alone
    Given a project with edition "5.2"
    And a Lua file containing 'do goto a end ::a::'
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

  # The reverse direction cannot produce a false negative — the shadow is
  # already illegal in the edition, so `--target` has nothing left to add.
  Scenario: edition 5.4 rejects the nested label shadow with no target at all
    Given a project with edition "5.4"
    And a Lua file containing '::a:: do ::a:: end'
    When I run "luabox check"
    Then the command fails
    And diagnostic LB0021 is reported

  Scenario: `break` outside a loop is reported once whatever the target
    Given a project with edition "5.1"
    And a Lua file containing 'local x = 1 break'
    When I run "luabox check --target 5.4"
    Then the command fails
    And diagnostic LB0022 is reported
    And stdout contains exactly 1 occurrence of "error[LB0022]"

  # `luabox build` runs its check gate on the *edition* only, by design:
  # lowering exists to handle constructs the target rejects. Nothing lowers a
  # duplicate label away, so the residual validation of the lowered output is
  # what catches it — before anything is written.
  Scenario: `luabox build` refuses to emit a tree the target cannot load
    Given a file "luabox.toml" containing:
      """
      [package]
      name = "shadow"
      edition = "5.2"
      """
    And a file "src/main.lua" containing:
      """
      ::a:: do ::a:: end
      return 1
      """
    When I run "luabox build --target 5.4"
    Then the command fails
    And stdout contains "LB0021"
    And the file "dist/src/main.lua" does not exist

  Scenario: a near-miss label name gets a did-you-mean nudge
    Given a project with edition "5.4"
    And a Lua file containing 'for i=1,3 do goto continu ::continue:: end'
    When I run "luabox check"
    Then the command fails
    And diagnostic LB0020 is reported
    And stdout contains "did you mean `continue`?"

  Scenario: an unrelated label name gets no nudge
    Given a project with edition "5.4"
    And a Lua file containing 'goto nowhere ::completely_different::'
    When I run "luabox check"
    Then the command fails
    And diagnostic LB0020 is reported
    And stdout does not contain "did you mean"

  Scenario: the unresolved goto underlines the label name, not the statement
    Given a project with edition "5.4"
    And a Lua file containing 'goto nowhere'
    When I run "luabox check"
    Then the command fails
    And stdout contains "     ^^^^^^^ no matching label is visible here"

  Scenario: a duplicate label points at the second one and names the first
    Given a project with edition "5.4"
    And a Lua file containing '::a:: ::a::'
    When I run "luabox check"
    Then the command fails
    And stdout contains "duplicate label"
    And stdout contains "first defined here"

  Scenario: `luabox lint` reports the same legality errors as `luabox check`
    Given a file "luabox.toml" containing:
      """
      [package]
      edition = "5.4"
      """
    And a file "src/main.lua" containing:
      """
      local function f()
        goto nowhere
      end
      ::a::
      ::a::
      break
      return f
      """
    When I run "luabox lint"
    Then the command fails
    And stdout contains "LB0020"
    And stdout contains "LB0021"
    And stdout contains "LB0022"

  Scenario: legality errors are not lint rules and cannot be silenced by an ignore
    Given a file "luabox.toml" containing:
      """
      [package]
      edition = "5.4"
      """
    And a file "src/main.lua" containing:
      """
      ---@luabox-ignore unused-local not a lint anyway
      break
      """
    When I run "luabox lint"
    Then the command fails
    And stdout contains "LB0022"

  Scenario: a file with a syntax error is not second-guessed
    Given a project with edition "5.4"
    And a Lua file containing 'while true do break'
    When I run "luabox check"
    Then the command fails
    And diagnostic LB0001 is reported
    And no control-flow diagnostic is reported

  Scenario Outline: every control-flow code has an explain page
    Given an empty directory
    When I run "luabox explain <code>"
    Then the command succeeds
    And stdout contains "# <code>: <title>"

    Examples:
      | code   | title                       |
      | LB0020 | no visible label for `goto` |
      | LB0021 | label already defined       |
      | LB0022 | `break` outside a loop      |

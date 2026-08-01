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

  # `luabox build`'s check gate runs the *edition* dialect-legality pass plus
  # the *target*'s control-flow pass. Lowering exists to handle constructs the
  # target's parser rejects, so those are not gated; nothing lowers a
  # duplicate label away, so what the target's loader rejects is — against the
  # source, which is what gives the finding its span. The residual pass over
  # the lowered output stays as the belt-and-braces catch for anything
  # lowering itself introduces.
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

  # Shockwave round 4, finding M: the build-time report used to be
  # reconstructed from the *lowered* text and so carried no labels at all,
  # while `check --target` gave full spans for the identical defect.
  Scenario: `luabox build`'s duplicate-label report carries source spans
    Given a project with edition "5.2"
    And a Lua file containing '::a:: do ::a:: end'
    When I run "luabox build --target 5.4"
    Then the command fails
    And stdout contains "duplicate label"
    And stdout contains "first defined here"
    And stdout contains "src/main.lua:1:12"

  # Shockwave round 4, finding D: every bundle mode routed around the residual
  # control-flow pass, so `bundle = true` and `mode = "love"` shipped a chunk
  # `luac5.4 -p` refuses — with exit code 0. Zero bundle-mode scenarios
  # existed, which is why it survived three rounds.
  Scenario: bundling refuses to write a chunk the target cannot load
    Given a project with edition "5.2" targeting "5.4" bundling
    And a Lua file containing '::a:: do ::a:: end'
    When I run "luabox build"
    Then the command fails
    And stdout contains "LB0021"
    And the file "dist/main.lua" does not exist

  Scenario: love mode refuses to write an archive the target cannot load
    Given a project with edition "5.2" targeting "5.4" using mode "love"
    And a Lua file containing '::a:: do ::a:: end'
    When I run "luabox build"
    Then the command fails
    And stdout contains "LB0021"
    And the file "dist/fixture.love" does not exist

  # The third bundle-shaped emit. `plain` bundling and `love` were pinned in
  # round 4; `nvim-plugin` routes through the same `luabox-bundle` twin and
  # was the one shape with no scenario behind it.
  Scenario: nvim-plugin mode refuses to write a tree the target cannot load
    Given a project with edition "5.2" targeting "5.4" using mode "nvim-plugin"
    And a Lua file containing '::a:: do ::a:: end'
    When I run "luabox build"
    Then the command fails
    And stdout contains "LB0021"
    And the file "dist/fixture/lua/fixture/init.lua" does not exist

  # Tree mode against a *manifest* ship target, with no flag in sight: the
  # gate reads `[build] target` and asks the loader's question of the source.
  # Every other build-side control-flow scenario passes `--target` or bundles,
  # so this row of the matrix rested on the check-side scenario alone.
  Scenario: tree mode refuses a manifest ship target's loader verdict
    Given a project with edition "5.2" targeting "5.4"
    And a Lua file containing '::a:: do ::a:: end'
    When I run "luabox build"
    Then the command fails
    And stdout contains "LB0021"
    And the file "dist/src/main.lua" does not exist

  # …and the flag beats the manifest on the build path too: 5.2 accepts the
  # nested label its own edition accepts, so there is nothing to refuse.
  Scenario: an explicit target overrides the manifest's on the build path
    Given a project with edition "5.2" targeting "5.4"
    And a Lua file containing '::a:: do ::a:: end'
    When I run "luabox build --target 5.2"
    Then the command succeeds
    And the file "dist/src/main.lua" exists

  # Shockwave round 4, finding E: two `LB0021`s at the same primary span are
  # not the same finding — `repeated_label_scope` makes the first-definition
  # site dialect-dependent — so keeping whichever pass ran first printed one
  # line as a duplicate *and* cited it as the first definition, out of source
  # order. The ship target's verdict wins, and the merged set is sorted.
  Scenario: a target-dialect duplicate names one first-definition site, in source order
    Given a project with edition "5.2"
    And a file "src/main.lua" containing:
      """
      ::a::
      do
        ::a::
        ::a::
      end
      """
    When I run "luabox check --target 5.4"
    Then the command fails
    And stdout contains exactly 2 occurrence of "error[LB0021]"
    And stdout contains "src/main.lua:3:5" before "src/main.lua:4:5"
    And stdout contains exactly 2 occurrence of "src/main.lua:1:3"
    And stdout contains exactly 1 occurrence of "src/main.lua:3:5"
    And stdout contains exactly 1 occurrence of "src/main.lua:4:5"

  # Shockwave round 4, finding H: `[build] target` reached the require
  # resolver and the rock harvest but never the legality passes, so a project
  # that *declares* it shipped 5.4 passed a check that `--target 5.4` failed.
  Scenario: a manifest-declared ship target reaches the control-flow pass
    Given a project with edition "5.2" targeting "5.4"
    And a Lua file containing '::a:: do ::a:: end'
    When I run "luabox check"
    Then the command fails
    And diagnostic LB0021 is reported

  Scenario: the `--target` flag overrides a manifest-declared ship target
    Given a project with edition "5.2" targeting "5.4"
    And a Lua file containing '::a:: do ::a:: end'
    When I run "luabox check --target 5.2"
    Then the command succeeds
    And zero diagnostics are reported

  # A manifest ship target is a declaration that the project is *lowered*
  # there, so it asks the loader's question and not the parser's: reporting
  # `LB0011` on every `//` in a 5.3 project that ships 5.1 would fail
  # `luabox check` for using the feature `[build] target` exists to provide.
  # An explicit `--target` is the literal "would this source be legal there?"
  # question and still asks both.
  Scenario: a manifest ship target does not report constructs lowering rewrites
    Given a project with edition "5.3" targeting "5.1"
    And a Lua file containing 'local x = 7 // 2'
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

  Scenario: an explicit target still reports constructs lowering rewrites
    Given a project with edition "5.3" targeting "5.1"
    And a Lua file containing 'local x = 7 // 2'
    When I run "luabox check --target 5.1"
    Then the command fails
    And diagnostic LB0011 is reported
    And stdout contains "not legal on target 5.1"

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

  # Shockwave round 5, finding E residual. Passes 2 and 3 are two legality
  # *axes* over one file, each internally sorted but concatenated in pass
  # order, so a control-flow finding early in the file rendered after a
  # dialect finding late in it. A reader scanning a file's diagnostics reads
  # down the file, not down luabox's pass list.
  Scenario: the two legality axes render in one source order, not in pass order
    Given a project with edition "5.2"
    And a file "src/main.lua" containing:
      """
      ::a:: ::a::
      local n = 1 // 2
      """
    When I run "luabox check"
    Then the command fails
    And diagnostic LB0021 is reported
    And diagnostic LB0011 is reported
    And stdout contains "src/main.lua:1:9" before "src/main.lua:2:13"

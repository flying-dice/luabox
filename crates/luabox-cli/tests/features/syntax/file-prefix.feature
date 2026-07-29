Feature: File prefix — `#!` line and UTF-8 byte-order mark
  Reference Lua skips two things before the first token of a chunk, and
  luabox reads exactly what reference Lua reads (SPEC.md §2):

    * a `#`-led FIRST line (`skipcomment`, lauxlib.c) — every version since
      5.0, so every edition here;
    * a UTF-8 byte-order mark (`skipBOM`, lauxlib.c) — 5.2 onward, and
      LuaJIT (lj_lex.c). 5.1 has no such skip and rejects the file, so
      luabox does too, by name rather than by codepoint.

  Both are position-sensitive: they only count at byte 0. A `#` on any
  later line is the length operator, and a BOM anywhere else is an illegal
  character — in reference Lua and here alike.

  Scenario Outline: a shebang'd file is ordinary source in every edition
    Given a project with edition "<edition>"
    And a file "src/main.lua" containing:
      """
      #!/usr/bin/env lua
      local function greet(name)
          return "hi " .. name
      end
      return greet
      """
    When I run "<command>"
    Then the command succeeds

    Examples: check
      | edition | command       |
      | 5.1     | luabox check  |
      | 5.2     | luabox check  |
      | 5.3     | luabox check  |
      | 5.4     | luabox check  |
      | luajit  | luabox check  |

    Examples: lint / build / fmt
      | edition | command           |
      | 5.1     | luabox lint       |
      | 5.4     | luabox lint       |
      | luajit  | luabox lint       |
      | 5.1     | luabox build      |
      | 5.4     | luabox build      |
      | 5.4     | luabox fmt --check |

  Scenario: any `#` first line is skipped, not just `#!`
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      # this is what skipcomment actually does
      return 1
      """
    When I run "luabox check"
    Then the command succeeds

  Scenario: `fmt` formats a shebang'd file and leaves the shebang alone
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      #!/usr/bin/env lua
      local   t = {1,2,3}
      return t
      """
    When I run "luabox fmt"
    Then the command succeeds
    And "src/main.lua" equals:
      """
      #!/usr/bin/env lua
      local t = { 1, 2, 3 }
      return t
      """

  Scenario: a `#` line below byte 0 is still the length operator, and an error
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """

      #!/usr/bin/env lua
      return 1
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0001"

  Scenario Outline: a leading BOM is skipped from 5.2 on, and by LuaJIT
    Given a project with edition "<edition>"
    And a Lua file with a UTF-8 BOM containing:
      """
      local x = 1
      return x
      """
    When I run "luabox check"
    Then the command succeeds

    Examples:
      | edition |
      | 5.2     |
      | 5.3     |
      | 5.4     |
      | luajit  |

  Scenario: 5.1 rejects a BOM and says so in words, not codepoints
    Given a project with edition "5.1"
    And a Lua file with a UTF-8 BOM containing:
      """
      local x = 1
      return x
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "file starts with a UTF-8 byte-order mark, which Lua 5.1 rejects"
    And stdout contains "save the file without a BOM"

  Scenario: a BOM'd file formats, and the mark survives byte-exact
    Given a project with edition "5.4"
    And a Lua file with a UTF-8 BOM containing:
      """
      local   t = {1,2,3}
      return t
      """
    When I run "luabox fmt"
    Then the command succeeds
    And "src/main.lua" equals, with a leading UTF-8 BOM:
      """
      local t = { 1, 2, 3 }
      return t
      """

  Scenario: a BOM followed by a shebang is the order lauxlib.c accepts
    Given a project with edition "5.4"
    And a Lua file with a UTF-8 BOM containing:
      """
      #!/usr/bin/env lua
      return 1
      """
    When I run "luabox check"
    Then the command succeeds

  # --- bundling (SPEC.md §7) ------------------------------------------------
  #
  # A bundle splices every module's text into ONE file, so the file prefix
  # stops being a prefix: a module's `#!` line lands mid-file where `#` is
  # the length operator, and a module's BOM lands where a mark is an illegal
  # character. Both are therefore cut from every module, and the *entry*'s
  # `#!` line is re-emitted at byte 0 so the bundle is still an executable
  # script. No BOM is ever emitted: the bundle is a new file, and a 5.1
  # target has no `skipBOM` to survive one.

  Scenario Outline: a shebang'd entry and a shebang'd module bundle into one executable script
    Given a project with edition "5.4" targeting "<target>" bundling
    And a file "src/main.lua" containing:
      """
      #!/usr/bin/env lua
      local util = require("util")
      print(util.greet())
      """
    And a file "src/util.lua" containing:
      """
      #!/usr/bin/env lua
      local M = {}
      function M.greet()
        return "hello-from-util"
      end
      return M
      """
    When I run "<command>"
    Then the command succeeds
    And "dist/main.lua" starts with "#!/usr/bin/env lua"
    And "dist/main.lua" contains exactly 1 occurrence of "#!/usr/bin/env lua"
    And "dist/main.lua" contains "hello-from-util"

    Examples:
      | target | command               |
      | 5.4    | luabox build          |
      | 5.4    | luabox build --minify |
      | 5.1    | luabox build          |
      | 5.1    | luabox build --minify |

  Scenario Outline: a shebang on a required module alone never reaches the bundle
    Given a project with edition "5.4" targeting "<target>" bundling
    And a file "src/main.lua" containing:
      """
      local util = require("util")
      print(util.greet())
      """
    And a file "src/util.lua" containing:
      """
      #!/usr/bin/env lua
      local M = {}
      function M.greet()
        return "hello-from-util"
      end
      return M
      """
    When I run "<command>"
    Then the command succeeds
    And "dist/main.lua" does not contain "#!/usr/bin/env lua"
    And "dist/main.lua" contains "hello-from-util"

    Examples:
      | target | command               |
      | 5.4    | luabox build          |
      | 5.4    | luabox build --minify |
      | 5.1    | luabox build          |
      | 5.1    | luabox build --minify |

  Scenario Outline: only the entry's shebang survives, and minifying does not drop it
    Given a project with edition "5.4" targeting "<target>" bundling
    And a file "src/main.lua" containing:
      """
      #!/usr/bin/env lua
      local util = require("util")
      print(util.greet())
      """
    And a file "src/util.lua" containing:
      """
      local M = {}
      function M.greet()
        return "hello-from-util"
      end
      return M
      """
    When I run "<command>"
    Then the command succeeds
    And "dist/main.lua" starts with "#!/usr/bin/env lua"
    And "dist/main.lua" contains exactly 1 occurrence of "#!/usr/bin/env lua"

    Examples:
      | target | command               |
      | 5.4    | luabox build          |
      | 5.4    | luabox build --minify |
      | 5.1    | luabox build          |
      | 5.1    | luabox build --minify |

  Scenario Outline: a BOM'd module bundles, and the bundle carries no mark
    Given a project with edition "5.4" targeting "<target>" bundling
    And a file "src/main.lua" containing:
      """
      local util = require("util")
      print(util.greet())
      """
    And a file "src/util.lua" with a UTF-8 BOM containing:
      """
      local M = {}
      function M.greet()
        return "hello-from-util"
      end
      return M
      """
    When I run "<command>"
    Then the command succeeds
    And "dist/main.lua" contains "hello-from-util"
    And "dist/main.lua" carries no UTF-8 byte-order mark

    Examples:
      | target | command               |
      | 5.4    | luabox build          |
      | 5.4    | luabox build --minify |

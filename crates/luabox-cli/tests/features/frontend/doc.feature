Feature: luabox doc — static documentation site
  SPEC.md §13: `luabox doc` generates a static site into `doc/` from
  LuaCATS annotations — search, cross-linked types, one page per module and
  per class/struct/trait, no external assets. `--open` (launching a browser)
  is deliberately not scenario-tested.

  Scenario: generates an index listing a documented function
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      --- Adds two numbers.
      ---@param a number
      ---@param b number
      ---@return number
      local function add(a, b)
        return a + b
      end
      """
    When I run "luabox doc"
    Then the command succeeds
    And the file "doc/index.html" exists
    And "doc/index.html" contains "add"
    And "doc/module.main.html" contains "function add(a: number, b: number): number"
    And "doc/module.main.html" contains "Adds two numbers."

  Scenario: class page lists its own and inherited fields
    Given a project with edition "5.4"
    And a file "src/shapes.lua" containing:
      """
      ---@class Shape
      ---@field id integer the identity
      local Shape = {}

      --- A circle.
      ---@class Circle: Shape
      ---@field radius number the radius
      local Circle = {}
      """
    When I run "luabox doc"
    Then the command succeeds
    And the file "doc/class.Circle.html" exists
    And "doc/class.Circle.html" contains "radius"
    And "doc/class.Circle.html" contains "Fields inherited from"
    And "doc/class.Circle.html" contains "id"
    And "doc/class.Circle.html" does not contain "Subclasses"
    And "doc/class.Circle.html" does not contain "Implementors"

  Scenario: parent class page lists its subclasses (#87)
    Given a project with edition "5.4"
    And a file "src/shapes.lua" containing:
      """
      ---@class Shape
      ---@field id integer
      local Shape = {}

      --- A circle.
      ---@class Circle: Shape
      local Circle = {}

      ---@class Rect: Shape
      local Rect = {}

      ---@class Lonely
      local Lonely = {}
      """
    When I run "luabox doc"
    Then the command succeeds
    And "doc/class.Shape.html" contains "<h2>Subclasses</h2>"
    And "doc/class.Shape.html" contains 'href="class.Circle.html"'
    And "doc/class.Shape.html" contains 'href="class.Rect.html"'
    And "doc/class.Lonely.html" does not contain "Subclasses"
    And "doc/class.Lonely.html" does not contain "Implementors"

  Scenario: an all-function-typed parent is headed "Implementors" instead
    Given a project with edition "5.4"
    And a file "src/shapes.lua" containing:
      """
      ---@class Shape
      ---@field area fun(self): number
      ---@field perimeter fun(self): number
      local Shape = {}

      ---@class Circle: Shape
      local Circle = {}
      """
    When I run "luabox doc"
    Then the command succeeds
    And "doc/class.Shape.html" contains "<h2>Implementors</h2>"
    And "doc/class.Shape.html" does not contain "<h2>Subclasses</h2>"

  # --- annotation rendering -----------------------------------------------

  Scenario: parameters and returns each get their own definition list
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@param value string the raw value
      ---@return string the safe value
      local function escape(value)
        return value
      end
      return escape
      """
    When I run "luabox doc"
    Then the command succeeds
    And "doc/module.main.html" contains "<h3>Parameters</h3>"
    And "doc/module.main.html" contains "<dt>value: string</dt>"
    And "doc/module.main.html" contains "<dd>the raw value</dd>"
    And "doc/module.main.html" contains "<h3>Returns</h3>"
    And "doc/module.main.html" contains "<dd>the safe value</dd>"

  Scenario: an optional parameter is marked in the definition list
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@param opts? table the options
      local function configure(opts) end
      return configure
      """
    When I run "luabox doc"
    Then the command succeeds
    And "doc/module.main.html" contains "<dt>opts?: table</dt>"

  Scenario: a deprecated function is badged
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@deprecated
      ---@return number
      local function old()
        return 1
      end
      return old
      """
    When I run "luabox doc"
    Then the command succeeds
    And "doc/module.main.html" contains ">deprecated</span>"

  Scenario: `---@see` renders a cross-reference section
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@class Point
      ---@field x number
      local Point = {}

      ---@param p Point
      ---@see Point the receiver type
      local function dist(p) end

      return Point, dist
      """
    When I run "luabox doc"
    Then the command succeeds
    And "doc/module.main.html" contains "<h3>See also</h3>"
    And "doc/module.main.html" contains "the receiver type"
    And "doc/module.main.html" contains "class.Point.html"

  Scenario: field scope and optionality are rendered in the field table
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@class Widget
      ---@field id integer the identity
      ---@field label? string
      ---@field private secret string
      local Widget = {}
      return Widget
      """
    When I run "luabox doc"
    Then the command succeeds
    And "doc/class.Widget.html" contains "<h2>Fields</h2>"
    And "doc/class.Widget.html" contains "<td>id</td>"
    And "doc/class.Widget.html" contains "<td>label?</td>"
    And "doc/class.Widget.html" contains "<td>private secret</td>"

  Scenario: a generic class page is titled by its bare name
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@class Box<T>
      ---@field value T
      local Box = {}
      return Box
      """
    When I run "luabox doc"
    Then the command succeeds
    And the file "doc/class.Box.html" exists
    And "doc/class.Box.html" contains "<h1>Class <code>Box</code></h1>"

  # --- doc prose is Markdown ----------------------------------------------

  Scenario: blank-line-separated prose becomes separate paragraphs
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      --- First line
      --- still the first paragraph.
      ---
      --- Second paragraph.
      local function documented() end
      return documented
      """
    When I run "luabox doc"
    Then the command succeeds
    And "doc/module.main.html" contains "<p>First line still the first paragraph.</p>"
    And "doc/module.main.html" contains "<p>Second paragraph.</p>"

  Scenario: backticks become inline code
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      --- Call `f(x)` twice.
      local function documented() end
      return documented
      """
    When I run "luabox doc"
    Then the command succeeds
    And "doc/module.main.html" contains "Call <code>f(x)</code> twice."

  Scenario: dashes become an unordered list
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      --- Steps:
      --- - normalize
      --- - escape
      local function documented() end
      return documented
      """
    When I run "luabox doc"
    Then the command succeeds
    And "doc/module.main.html" contains "<ul>"
    And "doc/module.main.html" contains "<li>normalize</li>"
    And "doc/module.main.html" contains "<li>escape</li>"

  Scenario: numbers become an ordered list
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      --- 1. first
      --- 2. second
      local function documented() end
      return documented
      """
    When I run "luabox doc"
    Then the command succeeds
    And "doc/module.main.html" contains "<ol>"
    And "doc/module.main.html" contains "<li>first</li>"

  Scenario: a fenced block becomes a preformatted code block with escaped content
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      --- Example:
      ---
      --- ```lua
      --- local ok = a < b
      --- ```
      local function documented() end
      return documented
      """
    When I run "luabox doc"
    Then the command succeeds
    And "doc/module.main.html" contains "<pre><code>local ok = a &lt; b"

  Scenario: prose is HTML-escaped rather than injected
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      --- Handles <script> & "quotes" safely.
      local function documented() end
      return documented
      """
    When I run "luabox doc"
    Then the command succeeds
    And "doc/module.main.html" contains "&lt;script&gt; &amp; &quot;quotes&quot;"
    And "doc/module.main.html" does not contain "<script> &"

  Scenario: an interface declared only in a `.d.lua` def still gets a page and lists implementors
    Given a file "defs/geometry.d.lua" containing:
      """
      ---@meta
      ---@class geometry.Shape
      ---@field area fun(self): number
      """
    And a file "luabox.toml" containing:
      """
      [package]
      name = "fixture"
      version = "0.1.0"
      edition = "5.4"

      [types]
      strict = true
      defs = ["geometry"]
      """
    And a file "src/circle.lua" containing:
      """
      ---@class geometry.Circle: geometry.Shape
      local Circle = {}

      ---@return number
      function Circle:area()
        return 0
      end
      """
    When I run "luabox doc"
    Then the command succeeds
    And the file "doc/class.geometry.Shape.html" exists
    And "doc/class.geometry.Shape.html" contains "<h2>Implementors</h2>"
    And "doc/class.geometry.Shape.html" contains 'href="class.geometry.Circle.html"'

  Scenario: doc refuses on a parse error like build does
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      --[[ never closed
      local x = 1
      """
    When I run "luabox doc"
    Then the command fails
    And stdout contains "error[LB0001]"
    And stderr contains "doc refuses to generate"

  Scenario: doc still generates when the only problems are type errors
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@param n number
      ---@return number
      local function double(n)
        return n * 2
      end

      print(double("oops"))
      """
    When I run "luabox doc"
    Then the command succeeds
    And the file "doc/index.html" exists

//! `---@private` / `---@protected` / `---@package` members — luals'
//! `invisible` rule, `LB0312` (#115).
//!
//! An access is "inside the class" when it happens in one of that class's own
//! methods (for `protected`, a subclass's methods count; for `package`, the
//! declaring file). Everything else is out of reach. Conservative throughout:
//! visibility is only judged against an unambiguous single-class receiver.

use luabox_diag::{Diagnostic, Severity};
use luabox_syntax::lua;
use luabox_syntax::lua::{Dialect, parse};
use luabox_types::{Strictness, check_file, check_file_with_ambient, stdlib_defs};

fn check(source: &str, strictness: Strictness) -> Vec<Diagnostic> {
    let parse = parse(source, Dialect::Lua54);
    assert_eq!(parse.errors(), &[], "fixture must parse cleanly");
    check_file(&parse, "test.lua", strictness, Dialect::Lua54)
}

fn codes(source: &str, strictness: Strictness) -> Vec<String> {
    check(source, strictness)
        .iter()
        .map(|d| d.code.to_string())
        .collect()
}

fn strict_codes(source: &str) -> Vec<String> {
    codes(source, Strictness::Strict)
}

/// Strict check with the Lua 5.4 stdlib ambient layer (its
/// `setmetatable` signature must not mask inference).
fn strict_codes_ambient(src: &str) -> Vec<String> {
    let parse = lua::parse(src, lua::Dialect::Lua54);
    assert_eq!(parse.errors(), &[], "fixture must parse cleanly");
    check_file_with_ambient(
        &parse,
        "test.lua",
        Strictness::Strict,
        lua::Dialect::Lua54,
        Some(stdlib_defs(lua::Dialect::Lua54)),
    )
    .iter()
    .map(|d| d.code.to_string())
    .collect()
}

#[test]
fn private_field_clean_inside_own_method() {
    // A `---@field private` read from the owning class's own method is
    // allowed — the receiver is `self`, whose class is the enclosing one.
    let src = "\
---@class Account
---@field private balance number
local Account = {}
Account.__index = Account

function Account:total()
  return self.balance
end
";
    assert_eq!(strict_codes(src), Vec::<String>::new());
}

#[test]
fn private_field_blocked_outside() {
    // The same read from a plain function is `invisible` — and it is
    // LB0312, not LB0306: the member exists, it is just not visible.
    let src = "\
---@class Account
---@field private balance number
local Account = {}
Account.__index = Account

---@param a Account
local function show(a)
  return a.balance
end
";
    assert_eq!(strict_codes(src), vec!["LB0312"]);
}

#[test]
fn private_is_warning_in_warn_mode() {
    let src = "\
---@class Account
---@field private balance number
local Account = {}

---@param a Account
local function show(a)
  return a.balance
end
";
    let diags = check(src, Strictness::Warn);
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].code.to_string(), "LB0312");
    assert_eq!(diags[0].severity, Severity::Warning);
    // ...and an error under strict (stricter than luals, like LB0306).
    assert_eq!(check(src, Strictness::Strict)[0].severity, Severity::Error);
}

#[test]
fn protected_clean_in_subclass_blocked_elsewhere() {
    let src = "\
---@class Base
---@field protected token? string
local Base = {}
Base.__index = Base

---@class Child : Base
local Child = {}
Child.__index = Child

function Child:reveal()
  return self.token
end

---@param b Base
local function leak(b)
  return b.token
end
";
    assert_eq!(strict_codes(src), vec!["LB0312"]);
}

#[test]
fn private_not_visible_in_subclass() {
    // luals: private is same-class-only — a subclass method cannot read a
    // private parent member (protected would).
    let src = "\
---@class Base
---@field private secret? string
local Base = {}
Base.__index = Base

---@class Child : Base
local Child = {}
Child.__index = Child

function Child:peek()
  return self.secret
end
";
    assert_eq!(strict_codes(src), vec!["LB0312"]);
}

#[test]
fn package_clean_same_file() {
    let src = "\
---@class Config
---@field package secret string
local Config = {}
Config.__index = Config

---@param c Config
local function read(c)
  return c.secret
end
";
    assert_eq!(strict_codes(src), Vec::<String>::new());
}

#[test]
fn private_suppressed_by_directive() {
    let src = "\
---@class Account
---@field private balance number
local Account = {}

---@param a Account
local function show(a)
  ---@diagnostic disable-next-line: invisible
  return a.balance
end
";
    assert_eq!(strict_codes(src), Vec::<String>::new());
}

#[test]
fn private_suppressed_file_wide() {
    let src = "\
---@diagnostic disable: invisible
---@class Account
---@field private balance number
local Account = {}

---@param a Account
local function show(a)
  return a.balance
end
";
    assert_eq!(strict_codes(src), Vec::<String>::new());
}

#[test]
fn standalone_private_on_method() {
    // The tag-above-a-method form: `---@private function Class:m()`. The
    // method is reachable from the class's own methods, invisible outside.
    let src = "\
---@class Widget
local Widget = {}
Widget.__index = Widget

---@private
function Widget:_init() end

function Widget:render()
  self:_init()
end

---@param w Widget
local function use(w)
  w:_init()
end
";
    assert_eq!(strict_codes(src), vec!["LB0312"]);
}

#[test]
fn visible_public_sibling_is_clean() {
    // A public member alongside a private one is never flagged.
    let src = "\
---@class Account
---@field private balance number
---@field owner string
local Account = {}

---@param a Account
local function show(a)
  return a.owner
end
";
    assert_eq!(strict_codes(src), Vec::<String>::new());
}

#[test]
fn public_redeclaration_in_subclass_reopens_member() {
    // A subclass re-declaring an inherited private member as a plain
    // `---@field` opens it back to public (nearest declaration wins).
    let src = "\
---@class Base
---@field private tag? string
local Base = {}

---@class Child : Base
---@field tag? string
local Child = {}

---@param c Child
local function read(c)
  return c.tag
end
";
    assert_eq!(strict_codes(src), Vec::<String>::new());
}

#[test]
fn private_blocked_on_inferred_instance_receiver() {
    // The idiomatic case: a constructor result (an inference *instance*
    // shape, not an annotated `---@type`) still resolves its class, so a
    // private read on it from module scope is `invisible`.
    let src = "\
---@class Circle
---@field private radius number
local Circle = {}
Circle.__index = Circle

---@return Circle
function Circle.new()
  return setmetatable({ radius = 1 }, Circle)
end

local c = Circle.new()
print(c.radius)
";
    assert_eq!(strict_codes_ambient(src), vec!["LB0312"]);
}

#[test]
fn standalone_private_on_bare_assign_carrier_blocked_outside() {
    // The bare-assignment carrier form: `---@private Carrier.method =
    // function() end`. luals resolves the class from the assignment base
    // (`vm.getDefinedClass` on the `setfield` node), so the member is
    // private to the class and invisible outside its methods.
    let src = "\
---@class Widget
local Widget = {}
Widget.__index = Widget

---@private
Widget.reset = function() end

---@param w Widget
local function use(w)
  return w.reset
end
";
    assert_eq!(strict_codes(src), vec!["LB0312"]);
}

#[test]
fn standalone_private_on_bare_assign_carrier_clean_inside() {
    // Read from inside a `:` method of the same class — the enclosing class
    // context makes the private member visible (clean).
    let src = "\
---@class Widget
local Widget = {}
Widget.__index = Widget

---@private
Widget.reset = function() end

function Widget:render()
  return self.reset
end
";
    assert_eq!(strict_codes(src), Vec::<String>::new());
}

#[test]
fn standalone_private_from_inside_bare_assigned_method() {
    // Item 2 (access side): a method itself *defined* via the bare-assign
    // carrier form (`Widget.render = function(self) ... end`) that reads a
    // private member of its own class is judged in-class (clean). Both the
    // `function Widget:m()` and `Widget.m = function()` forms desugar to the
    // same HIR assignment, so the enclosing-class judgment is symmetric.
    let src = "\
---@class Widget
local Widget = {}
Widget.__index = Widget

---@private
Widget.reset = function() end

---@param self Widget
Widget.render = function(self)
  return self.reset
end
";
    assert_eq!(strict_codes(src), Vec::<String>::new());
}

#[test]
fn standalone_protected_on_bare_assign_carrier() {
    // `---@protected` variant: readable in the owning class's own method,
    // invisible from an unrelated function.
    let src = "\
---@class Widget
local Widget = {}
Widget.__index = Widget

---@protected
Widget.token = function() end

function Widget:reveal()
  return self.token
end

---@param w Widget
local function leak(w)
  return w.token
end
";
    assert_eq!(strict_codes(src), vec!["LB0312"]);
}

#[test]
fn standalone_package_on_bare_assign_carrier() {
    // `---@package` variant: accessible anywhere in the file that declares
    // the class (clean same-file).
    let src = "\
---@class Config
local Config = {}
Config.__index = Config

---@package
Config.secret = function() end

---@param c Config
local function read(c)
  return c.secret
end
";
    assert_eq!(strict_codes(src), Vec::<String>::new());
}

#[test]
fn standalone_visibility_multi_target_assign_not_wired() {
    // Conservative: a visibility tag above a multi-target assignment
    // (`a.x, b.y = f, g`) is *not* wired — the base of a member does not
    // resolve to a single class, so nothing is marked private and the
    // outside read stays clean (no false `invisible`).
    let src = "\
---@class Widget
local Widget = {}
Widget.__index = Widget

local other = {}

---@private
Widget.reset, other.y = function() end, 1

---@param w Widget
local function use(w)
  return w.reset
end
";
    assert_eq!(strict_codes(src), Vec::<String>::new());
}

#[test]
fn standalone_visibility_on_non_carrier_base_unaffected() {
    // A bare assignment whose base is not a declared class carrier: the tag
    // finds no class to attach to, so nothing is marked private and the read
    // stays clean (no false `invisible`).
    let src = "\
---@class Widget
local Widget = {}
Widget.__index = Widget

local plain = {}

---@private
plain.reset = function() end

local x = plain.reset
";
    assert_eq!(strict_codes(src), Vec::<String>::new());
}

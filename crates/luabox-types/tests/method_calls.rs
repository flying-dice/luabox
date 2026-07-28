//! `:` method calls: argument checking through resolved class shapes (#118)
//! and the callee's use-site tags reaching the call site (#33).
//!
//! Method resolution is the engine's, not the annotation checker's: the
//! receiver's shape (declared or inferred, through `__index` chains) names the
//! member, and its signature governs the call. Arguments are only checked when
//! the receiver resolves to a *declared* `---@class` — a plain inferred
//! prototype's contract is not authoritative enough to manufacture argument
//! errors from. The callee's own tags (`---@deprecated`, `---@async`) carry no
//! such risk and are reported for every resolved receiver.

use luabox_diag::Severity;
use luabox_syntax::lua;
use luabox_types::{Strictness, check_file_with_ambient, stdlib_defs};

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

/// A class carrying a typed method and a `---@deprecated` method, plus a
/// `---@return Class` constructor — the shared fixture for the `:`-call
/// resolution tests.
const WIDGET_CLASS: &str = "\
---@class Widget
---@field size number
local Widget = {}
Widget.__index = Widget

---@param w number
---@return number
function Widget:resize(w)
  return w
end

---@deprecated
function Widget:legacy() end

---@param n number
---@return Widget
function Widget.new(n)
  return setmetatable({ size = n }, Widget)
end
";

/// A plain prototype — no `---@class` anywhere — carrying a tagged method
/// and an untagged one.
const PROTOTYPE_CARRIER: &str = "\
local Proto = {}
Proto.__index = Proto

---@deprecated
function Proto:legacy() end

---@async
function Proto:fetch() end

---@param n number
function Proto:resize(n) end
";

/// A `---@class` whose methods are *both* `---@field`-declared and defined,
/// the declaration shadowing the carrier the tags are written on.
const FIELD_DECLARED_CARRIER: &str = "\
---@class Decl
---@field legacy fun(self: Decl)
---@field fetch fun(self: Decl)
---@field resize fun(self: Decl, n: number)
local Decl = {}
Decl.__index = Decl

---@deprecated
function Decl:legacy() end

---@async
function Decl:fetch() end

function Decl:resize(n) end
";

#[test]
fn method_call_wrong_arg_type_flagged() {
    let src = format!(
        "{WIDGET_CLASS}
local w = Widget.new(1)
w:resize(\"nope\")
"
    );
    assert_eq!(strict_codes_ambient(&src), vec!["LB0300"]);
}

#[test]
fn method_call_correct_arg_is_clean() {
    let src = format!(
        "{WIDGET_CLASS}
local w = Widget.new(1)
w:resize(2)
"
    );
    assert_eq!(strict_codes_ambient(&src), Vec::<String>::new());
}

#[test]
fn method_call_wrong_arity_flagged() {
    // The implicit `self` is not counted: `resize` takes one explicit
    // argument, so zero and two both mis-arity (LB0301).
    let src = format!(
        "{WIDGET_CLASS}
local w = Widget.new(1)
w:resize()
w:resize(2, 3)
"
    );
    assert_eq!(strict_codes_ambient(&src), vec!["LB0301", "LB0301"]);
}

#[test]
fn deprecated_method_call_flagged() {
    let src = format!(
        "{WIDGET_CLASS}
local w = Widget.new(1)
w:legacy()
"
    );
    assert_eq!(strict_codes_ambient(&src), vec!["LB0308"]);
}

#[test]
fn deprecated_method_call_is_warning_even_under_strict() {
    let src = format!(
        "{WIDGET_CLASS}
local w = Widget.new(1)
w:legacy()
"
    );
    let parse = lua::parse(&src, lua::Dialect::Lua54);
    assert_eq!(parse.errors(), &[], "fixture must parse cleanly");
    let diags = check_file_with_ambient(
        &parse,
        "test.lua",
        Strictness::Strict,
        lua::Dialect::Lua54,
        Some(stdlib_defs(lua::Dialect::Lua54)),
    );
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].code.to_string(), "LB0308");
    assert_eq!(diags[0].severity, Severity::Warning);
}

#[test]
fn method_call_via_self_resolves_and_checks() {
    // Inside a method, `self` resolves to the declared class: a wrong-typed
    // `self:resize(...)` is checked, and `self:legacy()` flags deprecation.
    let src = format!(
        "{WIDGET_CLASS}
function Widget:grow()
  self:resize(\"bad\")
  self:legacy()
end
"
    );
    assert_eq!(strict_codes_ambient(&src), vec!["LB0300", "LB0308"]);
}

#[test]
fn method_call_via_typed_local_resolves() {
    let src = format!(
        "{WIDGET_CLASS}
---@type Widget
local w
w:resize(\"bad\")
"
    );
    assert_eq!(strict_codes_ambient(&src), vec!["LB0300"]);
}

#[test]
fn method_call_inherited_through_parent_resolves() {
    // A subclass instance reaches the parent's typed method through the
    // `__index` chain: `Derived` indexes into `Base`, so `d:resize(...)`
    // resolves to `Base.resize` and its argument is checked.
    let src = "\
---@class Base
local Base = {}
Base.__index = Base

---@param w number
function Base:resize(w) end

---@class Derived : Base
local Derived = setmetatable({}, { __index = Base })
Derived.__index = Derived

---@return Derived
function Derived.new()
  return setmetatable({}, Derived)
end

local d = Derived.new()
d:resize(\"bad\")
";
    assert_eq!(strict_codes_ambient(src), vec!["LB0300"]);
}

#[test]
fn method_call_unknown_receiver_is_silent() {
    // An `any`/unknown receiver resolves to no class: nothing is reported,
    // even with an obviously wrong argument (mandatory conservatism).
    let src = format!(
        "{WIDGET_CLASS}
---@param x any
local function use(x)
  x:resize(\"whatever\")
end
"
    );
    assert_eq!(strict_codes_ambient(&src), Vec::<String>::new());
}

#[test]
fn method_call_plain_table_receiver_is_silent() {
    // A plain inferred table with no declared class carries no method
    // signature: the `:` call is not checked.
    let src = "\
local t = {}
function t.thing(a, b) return a end
t:thing(1)
";
    assert_eq!(strict_codes_ambient(src), Vec::<String>::new());
}

#[test]
fn method_call_unknown_method_adds_no_arg_finding() {
    // A method the class does not declare resolves to no signature, so the
    // argument checker stays silent — it manufactures no arity/type
    // finding. Inference's own `undefined-field` rule (#90, LB0306) owns
    // the "no such method" finding; the `:`-call arg check adds nothing.
    let src = format!(
        "{WIDGET_CLASS}
local w = Widget.new(1)
w:nonexistent(1, 2, 3)
"
    );
    let codes = strict_codes_ambient(&src);
    assert_eq!(codes, vec!["LB0306"]);
    assert!(!codes.iter().any(|c| c == "LB0300" || c == "LB0301"));
}

#[test]
fn deprecated_method_on_plain_prototype_flagged() {
    let src = format!(
        "{PROTOTYPE_CARRIER}
local p = setmetatable({{}}, Proto)
p:legacy()
"
    );
    assert_eq!(strict_codes_ambient(&src), vec!["LB0308"]);
}

#[test]
fn untagged_method_on_plain_prototype_is_clean() {
    let src = format!(
        "{PROTOTYPE_CARRIER}
local p = setmetatable({{}}, Proto)
p:resize(1)
"
    );
    assert_eq!(strict_codes_ambient(&src), Vec::<String>::new());
}

#[test]
fn plain_prototype_method_args_stay_unchecked() {
    // Publishing the signature for its tags must not start argument-checking
    // a structurally-resolved receiver: a plain prototype has no declared
    // class, so a wrong-typed or mis-counted argument stays silent exactly
    // as before (SPEC §19 conservatism). Only the tag is reported.
    let src = format!(
        "{PROTOTYPE_CARRIER}
local p = setmetatable({{}}, Proto)
p:resize(\"nope\")
p:resize(1, 2, 3)
p:legacy(\"unexpected\")
"
    );
    assert_eq!(strict_codes_ambient(&src), vec!["LB0308"]);
}

#[test]
fn async_method_on_plain_prototype_flagged() {
    let src = format!(
        "{PROTOTYPE_CARRIER}
local p = setmetatable({{}}, Proto)
local function sync()
  p:fetch()
end
"
    );
    assert_eq!(strict_codes_ambient(&src), vec!["LB0316"]);
}

#[test]
fn async_method_on_plain_prototype_in_async_caller_is_clean() {
    let src = format!(
        "{PROTOTYPE_CARRIER}
local p = setmetatable({{}}, Proto)
---@async
local function poll()
  p:fetch()
end
"
    );
    assert_eq!(strict_codes_ambient(&src), Vec::<String>::new());
}

#[test]
fn deprecated_method_declared_as_field_flagged() {
    let src = format!(
        "{FIELD_DECLARED_CARRIER}
---@type Decl
local d
d:legacy()
"
    );
    assert_eq!(strict_codes_ambient(&src), vec!["LB0308"]);
}

#[test]
fn untagged_method_declared_as_field_is_clean() {
    let src = format!(
        "{FIELD_DECLARED_CARRIER}
---@type Decl
local d
d:resize(1)
"
    );
    assert_eq!(strict_codes_ambient(&src), Vec::<String>::new());
}

#[test]
fn field_declared_method_keeps_its_declared_signature() {
    // The tag carry-over must not disturb the `---@field` declaration: its
    // parameter list still governs, so a wrong-typed argument is LB0300 and
    // the deprecation is still reported alongside it.
    let src = format!(
        "{FIELD_DECLARED_CARRIER}
---@type Decl
local d
d:resize(\"nope\")
d:legacy()
"
    );
    assert_eq!(strict_codes_ambient(&src), vec!["LB0300", "LB0308"]);
}

#[test]
fn async_method_declared_as_field_flagged() {
    let src = format!(
        "{FIELD_DECLARED_CARRIER}
---@type Decl
local d
local function sync()
  d:fetch()
end
"
    );
    assert_eq!(strict_codes_ambient(&src), vec!["LB0316"]);
}

#[test]
fn async_method_declared_as_field_in_async_caller_is_clean() {
    let src = format!(
        "{FIELD_DECLARED_CARRIER}
---@type Decl
local d
---@async
local function poll()
  d:fetch()
end
"
    );
    assert_eq!(strict_codes_ambient(&src), Vec::<String>::new());
}

#[test]
fn method_tags_reach_a_self_call_through_a_field_declaration() {
    // The receiver shape route (`self` inside a sibling method) resolves
    // through the same declared class shape as `---@type Decl`.
    let src = format!(
        "{FIELD_DECLARED_CARRIER}
function Decl:go()
  self:legacy()
  self:fetch()
end
"
    );
    assert_eq!(strict_codes_ambient(&src), vec!["LB0308", "LB0316"]);
}

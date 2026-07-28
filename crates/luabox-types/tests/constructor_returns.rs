//! Constructor results flowing through `setmetatable` (#73).
//!
//! `return setmetatable({...}, Class)` from a `---@return Class` constructor
//! is the idiomatic Lua OOP shape. The instance the call yields must unify
//! with the declared class — so it satisfies a `Class` parameter, resolves the
//! class's methods, and mis-typed constructor fields still surface.

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

const CIRCLE_CLASS: &str = "\
---@class Circle
---@field radius number
local Circle = {}
Circle.__index = Circle

function Circle:area()
  return self.radius * self.radius
end
";

#[test]
fn constructor_setmetatable_satisfies_declared_class_return() {
    let src = format!(
        "{CIRCLE_CLASS}
---@param radius number
---@return Circle
function Circle.new(radius)
  return setmetatable({{ radius = radius }}, Circle)
end
"
    );
    assert_eq!(strict_codes_ambient(&src), Vec::<String>::new());
}

#[test]
fn wrong_constructor_return_still_errors() {
    // Returning something that is NOT the declared class must stay an
    // error: a plain number...
    let src = format!(
        "{CIRCLE_CLASS}
---@return Circle
function Circle.new()
  return 42
end
"
    );
    assert_eq!(strict_codes_ambient(&src), vec!["LB0304"]);
    // ...and an instance of an unrelated class missing the fields.
    let src = format!(
        "{CIRCLE_CLASS}
---@class Empty
local Empty = {{}}
Empty.__index = Empty

---@return Circle
function Circle.new()
  return setmetatable({{}}, Empty)
end
"
    );
    assert_eq!(strict_codes_ambient(&src), vec!["LB0304"]);
}

#[test]
fn annotated_instance_resolves_declared_fields_and_methods() {
    let src = format!(
        "{CIRCLE_CLASS}
---@param radius number
---@return Circle
function Circle.new(radius)
  return setmetatable({{ radius = radius }}, Circle)
end

---@param n number
local function wantn(n) end

local c = Circle.new(2)
wantn(c.radius)
wantn(c:area())
"
    );
    assert_eq!(strict_codes_ambient(&src), Vec::<String>::new());
}

#[test]
fn self_in_class_methods_uses_declared_field_types() {
    // The declaration wins over the inferred constructor value: `count`
    // is declared `integer`, so `string.rep` (integer count) accepts it
    // even though the constructor stored a plain `number` param.
    let src = "\
---@class Banner
---@field count integer
local Banner = {}
Banner.__index = Banner

function Banner:draw()
  return string.rep(\"#\", self.count)
end

---@param count integer
---@return Banner
function Banner.new(count)
  return setmetatable({ count = count }, Banner)
end
";
    assert_eq!(strict_codes_ambient(src), Vec::<String>::new());
    // Negative: a declared `string` field stays a string — arithmetic
    // consumers of it error.
    let bad = "\
---@param n number
local function wantn(n) end

---@class Tag
---@field label string
local Tag = {}
Tag.__index = Tag

function Tag:size()
  wantn(self.label)
end
";
    assert_eq!(strict_codes_ambient(bad), vec!["LB0300"]);
}

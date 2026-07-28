//! `LB0315`: non-exhaustive `if`/`elseif` dispatch on a finite discriminant
//! (#121).
//!
//! An `if` chain that compares one name against literals, covers only some
//! members of that name's finite type (a literal union or an `---@enum`), and
//! has no `else`, falls through silently. The analysis is deliberately narrow
//! — anything less certain than "every branch is `x == <literal>` on the same
//! `x`, and every member of `x`'s type is a literal" reports nothing.

use luabox_diag::{Diagnostic, Severity};
use luabox_syntax::lua::{Dialect, parse};
use luabox_types::{Strictness, check_file};

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

#[test]
fn nonexhaustive_union_if_is_flagged() {
    // `s : "a"|"b"|"c"`, only "a"/"b" handled, no else -> LB0315 naming "c".
    let src = "\
---@alias Color \"a\"|\"b\"|\"c\"
---@param s Color
local function f(s)
  if s == \"a\" then
    return 1
  elseif s == \"b\" then
    return 2
  end
end
";
    let diags = check(src, Strictness::Strict);
    let codes: Vec<String> = diags.iter().map(|d| d.code.to_string()).collect();
    assert_eq!(codes, vec!["LB0315"]);
    assert!(
        diags[0].message.contains("`\"c\"`"),
        "message should name the uncovered member: {}",
        diags[0].message
    );
}

#[test]
fn exhaustive_union_if_is_clean() {
    let src = "\
---@alias Color \"a\"|\"b\"|\"c\"
---@param s Color
local function f(s)
  if s == \"a\" then
    return 1
  elseif s == \"b\" then
    return 2
  elseif s == \"c\" then
    return 3
  end
end
";
    assert_eq!(strict_codes(src), Vec::<String>::new());
}

#[test]
fn union_if_with_else_is_clean() {
    let src = "\
---@alias Color \"a\"|\"b\"|\"c\"
---@param s Color
local function f(s)
  if s == \"a\" then
    return 1
  elseif s == \"b\" then
    return 2
  else
    return 0
  end
end
";
    assert_eq!(strict_codes(src), Vec::<String>::new());
}

#[test]
fn open_type_discriminant_is_never_flagged() {
    // A `string` discriminant is open-ended — never an exhaustiveness site.
    let src = "\
---@param s string
local function f(s)
  if s == \"a\" then
    return 1
  elseif s == \"b\" then
    return 2
  end
end
";
    assert_eq!(strict_codes(src), Vec::<String>::new());
}

#[test]
fn union_containing_a_nonliteral_is_never_flagged() {
    // `"a"|"b"|string` is not finite: the `string` member opens it up.
    let src = "\
---@alias Loose \"a\"|\"b\"|string
---@param s Loose
local function f(s)
  if s == \"a\" then
    return 1
  elseif s == \"b\" then
    return 2
  end
end
";
    assert_eq!(strict_codes(src), Vec::<String>::new());
}

#[test]
fn reversed_operand_order_is_analysed() {
    // `"a" == s` reads the same as `s == "a"`.
    let src = "\
---@alias Color \"a\"|\"b\"|\"c\"
---@param s Color
local function f(s)
  if \"a\" == s then
    return 1
  elseif \"b\" == s then
    return 2
  end
end
";
    assert_eq!(strict_codes(src), vec!["LB0315"]);
}

#[test]
fn compound_branch_condition_bails() {
    // A non-`==`-literal branch aborts the analysis — no false positive.
    let src = "\
---@alias Color \"a\"|\"b\"|\"c\"
---@param s Color
---@param ready boolean
local function f(s, ready)
  if s == \"a\" then
    return 1
  elseif s == \"b\" and ready then
    return 2
  end
end
";
    assert_eq!(strict_codes(src), Vec::<String>::new());
}

#[test]
fn nonexhaustive_enum_if_is_flagged() {
    let src = "\
---@enum Dir
local Dir = { up = 1, down = 2, left = 3 }
---@param d Dir
local function step(d)
  if d == Dir.up then
    return 1
  elseif d == Dir.down then
    return 2
  end
end
";
    let diags = check(src, Strictness::Strict);
    let codes: Vec<String> = diags.iter().map(|d| d.code.to_string()).collect();
    assert_eq!(codes, vec!["LB0315"]);
    assert!(
        diags[0].message.contains("`Dir.left`"),
        "message should name the uncovered enum member: {}",
        diags[0].message
    );
}

#[test]
fn exhaustive_enum_if_is_clean() {
    let src = "\
---@enum Dir
local Dir = { up = 1, down = 2 }
---@param d Dir
local function step(d)
  if d == Dir.up then
    return 1
  elseif d == Dir.down then
    return 2
  end
end
";
    assert_eq!(strict_codes(src), Vec::<String>::new());
}

#[test]
fn nonexhaustive_if_warns_in_warn_mode() {
    // Rides the strictness ladder: a warning in warn mode.
    let src = "\
---@alias Color \"a\"|\"b\"|\"c\"
---@param s Color
local function f(s)
  if s == \"a\" then
    return 1
  elseif s == \"b\" then
    return 2
  end
end
";
    let diags = check(src, Strictness::Warn);
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].code.to_string(), "LB0315");
    assert_eq!(diags[0].severity, Severity::Warning);
}

#[test]
fn generic_alias_substitutes() {
    // #117: `Pair<number>` monomorphises `{ first: T, second: T }`, so the
    // string `second` is caught against `number`.
    let src = "\
---@alias Pair<T> { first: T, second: T }
---@type Pair<number>
local bad = { first = 1, second = \"two\" }
";
    assert_eq!(strict_codes(src), vec!["LB0300"]);
}

#[test]
fn generic_alias_valid_instantiation() {
    let src = "\
---@alias Pair<T> { first: T, second: T }
---@type Pair<number>
local ok = { first = 1, second = 2 }
";
    assert_eq!(strict_codes(src), Vec::<String>::new());
}

#[test]
fn generic_alias_bare_reference_is_lenient() {
    // A bare `Pair` (no `<...>`) resolves its params to `unknown` — no
    // arity error, no field checking — matching luals and generic classes.
    let src = "\
---@alias Pair<T> { first: T, second: T }
---@type Pair
local anything = { first = 1, second = \"two\" }
";
    assert_eq!(strict_codes(src), Vec::<String>::new());
}

#[test]
fn generic_alias_arity_mismatch_reported() {
    // #117: an explicit `<...>` list of the wrong length is LB0313.
    let src = "\
---@alias Pair<T> { first: T, second: T }
---@type Pair<number, string>
local x = { first = 1, second = 2 }
";
    assert_eq!(strict_codes(src), vec!["LB0313"]);
}

#[test]
fn generic_class_arity_mismatch_reported() {
    // #124: LB0313 extends to generic `---@class Name<T>` references — an
    // explicit `<...>` list of the wrong length is flagged. The initializer
    // matches the substituted shape so only the arity finding remains.
    let src = "\
---@class Box<T>
---@field value T

---@type Box<number, string>
local b = { value = 1 }
";
    assert_eq!(strict_codes(src), vec!["LB0313"]);
}

#[test]
fn generic_class_bare_and_correct_arity_are_clean() {
    // A correct-arity reference and a bare reference (no `<...>`) both pass.
    let src = "\
---@class Box<T>
---@field value T

---@type Box<number>
local ok = { value = 1 }

---@type Box
local bare = { value = 2 }
";
    assert_eq!(strict_codes(src), Vec::<String>::new());
}

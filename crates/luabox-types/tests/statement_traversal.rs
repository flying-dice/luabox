// test code — panics document assumptions
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::string_slice
)]
//! Checker coverage of every *statement* and *argument* shape.
//!
//! The checker walks the concrete syntax tree itself (it is not driven off the
//! HIR), so each statement kind is a separate traversal arm: a mistake in one
//! of them silently swallows every diagnostic in that construct's body. These
//! tests plant a known-bad expression inside each statement shape and assert
//! the diagnostic still surfaces — the "the checker actually looks here" pin
//! that a happy-path fixture cannot give.

use luabox_diag::Diagnostic;
use luabox_syntax::lua::{Dialect, parse};
use luabox_types::{Strictness, check_file};

fn check(source: &str) -> Vec<Diagnostic> {
    let parsed = parse(source, Dialect::Lua54);
    assert_eq!(parsed.errors(), &[], "fixture must parse cleanly");
    check_file(&parsed, "test.lua", Strictness::Strict, Dialect::Lua54)
}

fn codes(source: &str) -> Vec<String> {
    check(source).iter().map(|d| d.code.to_string()).collect()
}

/// A callee that only accepts a `number`, so `want("x")` is always `LB0300`.
const WANT: &str = "\
---@param n number
local function want(n) end
";

// === statement bodies are traversed ======================================

#[test]
fn while_condition_and_body_are_checked() {
    let src = format!(
        "{WANT}\
while want(\"cond\") do
  want(\"body\")
end
"
    );
    assert_eq!(codes(&src), vec!["LB0300", "LB0300"]);
}

#[test]
fn repeat_body_and_until_condition_are_checked() {
    let src = format!(
        "{WANT}\
repeat
  want(\"body\")
until want(\"cond\")
"
    );
    assert_eq!(codes(&src), vec!["LB0300", "LB0300"]);
}

#[test]
fn do_block_body_is_checked() {
    let src = format!(
        "{WANT}\
do
  want(\"inside\")
end
"
    );
    assert_eq!(codes(&src), vec!["LB0300"]);
}

#[test]
fn numeric_for_bounds_step_and_body_are_checked() {
    let src = format!(
        "{WANT}\
for i = want(\"start\"), want(\"end\"), want(\"step\") do
  want(\"body\")
end
"
    );
    assert_eq!(codes(&src), vec!["LB0300"; 4]);
}

#[test]
fn generic_for_exprs_and_body_are_checked() {
    let src = format!(
        "{WANT}\
for k, v in pairs({{}}), want(\"extra\") do
  want(\"body\")
end
"
    );
    assert_eq!(codes(&src), vec!["LB0300", "LB0300"]);
}

#[test]
fn if_elseif_else_arms_are_all_checked() {
    let src = format!(
        "{WANT}\
if want(\"if\") then
  want(\"then\")
elseif want(\"elseif\") then
  want(\"elseif-body\")
else
  want(\"else\")
end
"
    );
    assert_eq!(codes(&src), vec!["LB0300"; 5]);
}

#[test]
fn break_goto_and_label_statements_check_clean() {
    // Jump statements carry no expressions; they must be inert (and must not
    // abort the surrounding traversal — the `want` after the label still
    // reports).
    let src = format!(
        "{WANT}\
while true do
  goto continue
  ::continue::
  break
end
want(\"after\")
"
    );
    assert_eq!(codes(&src), vec!["LB0300"]);
}

#[test]
fn nested_loops_do_not_lose_diagnostics() {
    let src = format!(
        "{WANT}\
for _ = 1, 3 do
  while true do
    repeat
      do
        want(\"deep\")
      end
    until true
    break
  end
end
"
    );
    assert_eq!(codes(&src), vec!["LB0300"]);
}

// === expression shapes ===================================================

#[test]
fn unary_operand_is_checked() {
    let src = format!(
        "{WANT}\
local bad = -want(\"operand\")
local also_bad = #want(\"len\")
local negated = not want(\"not\")
"
    );
    assert_eq!(codes(&src), vec!["LB0300"; 3]);
}

#[test]
fn parenthesised_expression_carries_its_inner_type() {
    // `(s)` is not opaque: the parenthesised string still mismatches a
    // `number` parameter, and the parenthesised sub-expression is traversed.
    let src = format!(
        "{WANT}\
---@type string
local s
want((s))
want((want(\"inner\")))
"
    );
    assert_eq!(codes(&src), vec!["LB0300", "LB0300", "LB0300"]);
}

#[test]
fn parenthesised_callee_resolves_its_signature() {
    let src = format!(
        "{WANT}\
(want)(\"through-parens\")
"
    );
    assert_eq!(codes(&src), vec!["LB0300"]);
}

#[test]
fn keyed_table_fields_are_checked_by_key_shape() {
    // `["name"] = v` is the same obligation as `name = v`; `[1] = v` is a
    // positional item; a dynamic key is not checkable and stays silent.
    let src = "\
---@class Rec
---@field a number
---@field b number

---@param r Rec
local function take(r) end

local dyn = \"a\"
take({ [\"a\"] = \"wrong\", [\"b\"] = 2 })
take({ [dyn] = 1, a = 1, b = 2 })
";
    // Only the `["a"] = "wrong"` slot mismatches; the dynamic-key literal is
    // complete via its named fields and reports nothing.
    assert_eq!(codes(src), vec!["LB0300"]);
}

#[test]
fn integer_keyed_table_fields_fill_tuple_positions() {
    let src = "\
---@param t [string, number]
local function take(t) end
take({ [1] = \"a\", [2] = 2 })
take({ [1] = 1, [2] = \"b\" })
";
    assert_eq!(codes(src), vec!["LB0300", "LB0300"]);
}

#[test]
fn keyed_table_fields_are_visited_as_expressions() {
    // Both halves of `[k] = v` are ordinary expressions and are traversed even
    // when the literal itself carries no expected shape.
    let src = format!(
        "{WANT}\
local t = {{ [want(\"key\")] = want(\"value\") }}
"
    );
    assert_eq!(codes(&src), vec!["LB0300", "LB0300"]);
}

// === sugar argument forms ================================================

#[test]
fn string_literal_call_sugar_is_checked_as_an_argument() {
    // `f"s"` passes the string literal as the sole argument.
    let src = format!(
        "{WANT}\
want\"sugar\"
"
    );
    assert_eq!(codes(&src), vec!["LB0300"]);
}

#[test]
fn long_bracket_string_call_sugar_is_unquoted_for_checking() {
    // The literal type is the *content*, so a `[[...]]`-delimited argument is
    // checked as its inner text — here against a literal-typed parameter.
    let src = "\
---@param s \"yes\"
local function only_yes(s) end
only_yes[[yes]]
only_yes[[no]]
";
    assert_eq!(codes(src), vec!["LB0300"]);
}

#[test]
fn table_constructor_call_sugar_is_checked_as_an_argument() {
    // `f{ ... }` passes the table constructor as the sole argument, with full
    // field-level checking.
    let src = "\
---@class Opt
---@field mode string

---@param o Opt
local function configure(o) end
configure{ mode = \"fast\" }
configure{ mode = 2 }
configure{ }
";
    assert_eq!(codes(src), vec!["LB0300", "LB0302"]);
}

#[test]
fn method_call_string_sugar_is_checked() {
    let src = "\
---@class Writer
---@field write fun(self, s: number)

---@param w Writer
local function use(w)
  w:write\"text\"
end
";
    assert_eq!(codes(src), vec!["LB0300"]);
}

// === version gating on `:` methods =======================================

#[test]
fn version_gated_method_call_reports_against_the_edition() {
    // `---@version 5.1` on a `:` method excludes a 5.4 project: the call is
    // flagged at the method *name* (not the whole call), the same `LB0308` a
    // gated free function gets.
    let src = "\
---@class Legacy
local L = {}
---@version 5.1
function L:old() end
L:old()
";
    let diags = check(src);
    assert_eq!(
        diags.iter().map(|d| d.code.to_string()).collect::<Vec<_>>(),
        vec!["LB0308"]
    );
    assert!(
        diags[0].message.contains("defined in `5.1/luajit`"),
        "{:?}",
        diags[0].message
    );
    let label = diags[0].primary_label().expect("primary label");
    assert_eq!(&src[label.span.range.clone()], "old");
}

#[test]
fn version_gated_method_is_silent_on_a_matching_edition() {
    let src = "\
---@class Legacy
local L = {}
---@version 5.4
function L:current() end
L:current()
";
    assert_eq!(codes(src), Vec::<String>::new());
}

// === generic constraints reached through composite parameter types =======

const SHAPE: &str = "\
---@class Shape
---@field area number
";

#[test]
fn constraint_violation_is_located_through_a_union_parameter() {
    // `T` is fixed by the *second* argument; the offending slot is then found
    // by scanning parameter types for a mention of `T` — here buried in the
    // first parameter's union, which is what the scan must see through.
    let src = format!(
        "{SHAPE}
---@generic T : Shape
---@param x T|nil
---@param seed T
---@return T
local function f(x, seed) end
f(nil, 5)
"
    );
    assert_eq!(codes(&src), vec!["LB0300"]);
}

#[test]
fn constraint_violation_is_located_through_an_array_parameter() {
    let src = format!(
        "{SHAPE}
---@generic T : Shape
---@param xs T[]
---@return T
local function f(xs) end
f({{ 5 }})
"
    );
    assert_eq!(codes(&src), vec!["LB0300"]);
}

#[test]
fn constraint_violation_is_located_through_a_table_parameter() {
    let src = format!(
        "{SHAPE}
---@generic T : Shape
---@param m table<string, T>
---@return T
local function f(m) end
---@type table<string, number>
local nums
f(nums)
"
    );
    assert_eq!(codes(&src), vec!["LB0300"]);
}

#[test]
fn constraint_violation_is_located_through_a_callback_parameter() {
    let src = format!(
        "{SHAPE}
---@generic T : Shape
---@param cb fun(): T
---@return T
local function f(cb) end
---@type fun(): number
local mk
f(mk)
"
    );
    assert_eq!(codes(&src), vec!["LB0300"]);
}

#[test]
fn satisfied_constraints_through_composite_parameters_are_clean() {
    let src = format!(
        "{SHAPE}
---@generic T : Shape
---@param xs T[]
---@return T
local function f(xs) end
---@type Shape[]
local shapes
f(shapes)
"
    );
    assert_eq!(codes(&src), Vec::<String>::new());
}

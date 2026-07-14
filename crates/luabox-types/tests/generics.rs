//! Real LuaCATS generics end-to-end (#84): generic `---@class<T>` references,
//! `---@generic` functions with call-site inference, backtick capture, and
//! bounded (`: Constraint`) type parameters — checked through the public
//! [`luabox_types::check_file`] API, matching lua-language-server semantics.

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

// --- 1. generic classes -------------------------------------------------

const PAIR: &str = "\
---@class Pair<T>
---@field first T
---@field second T
";

#[test]
fn generic_class_reference_substitutes_fields() {
    // A string in a `number`-instantiated field is a real error...
    let bad = format!(
        "{PAIR}
---@type Pair<number>
local p = {{ first = 1, second = \"x\" }}
"
    );
    assert_eq!(codes(&bad), vec!["LB0300"]);

    // ...and the correct literal is clean.
    let good = format!(
        "{PAIR}
---@type Pair<number>
local p = {{ first = 1, second = 2 }}
"
    );
    assert_eq!(codes(&good), Vec::<String>::new());
}

#[test]
fn bare_generic_class_reference_is_lenient() {
    // No type arguments: parameters become `unknown`, so anything conforms
    // (luals is lenient here — match it). No LB0305 for the `T` fields either.
    let src = format!(
        "{PAIR}
---@type Pair
local p = {{ first = 1, second = \"anything\" }}
"
    );
    assert_eq!(codes(&src), Vec::<String>::new());
}

#[test]
fn generic_class_params_do_not_trip_lb0305() {
    // The declaring block references `T` in its fields; that must not be an
    // unknown-type-name error.
    let src = format!("{PAIR}return {{}}\n");
    assert_eq!(codes(&src), Vec::<String>::new());
}

#[test]
fn nested_generic_class_substitutes() {
    let base = "\
---@class Box<T>
---@field value T
";
    let bad = format!(
        "{base}
---@type Box<Box<number>>
local b = {{ value = {{ value = \"x\" }} }}
"
    );
    assert_eq!(codes(&bad), vec!["LB0300"]);

    let good = format!(
        "{base}
---@type Box<Box<number>>
local b = {{ value = {{ value = 5 }} }}
"
    );
    assert_eq!(codes(&good), Vec::<String>::new());
}

// --- 2. generic functions with call-site inference ----------------------

const ID: &str = "\
---@generic T
---@param x T
---@return T
local function id(x)
  return x
end
";

#[test]
fn generic_function_return_flows_inferred_type() {
    // `id(5)` fixes T = integer; using the result where a string is required
    // proves the integer flowed through the return.
    let src = format!(
        "{ID}
---@param s string
local function wants_string(s) end
local n = id(5)
wants_string(n)
"
    );
    assert_eq!(codes(&src), vec!["LB0300"]);

    // `id(\"s\")` fixes T = string; using it where a number is required errors.
    let src = format!(
        "{ID}
---@param n number
local function wants_number(n) end
local s = id(\"s\")
wants_number(s)
"
    );
    assert_eq!(codes(&src), vec!["LB0300"]);
}

#[test]
fn generic_function_consistent_result_is_clean() {
    let src = format!(
        "{ID}
---@param n number
local function wants_number(n) end
wants_number(id(42))
"
    );
    assert_eq!(codes(&src), Vec::<String>::new());
}

#[test]
fn two_param_generic_reports_at_second_argument() {
    // `pick(a: T, b: T)` — the first argument fixes T = integer, so the
    // second (a string) mismatches. First-binding-wins, luals-style.
    let src = "\
---@generic T
---@param a T
---@param b T
---@return T
local function pick(a, b)
  return a
end
pick(5, \"x\")
";
    let diags = check(src);
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code.to_string(), "LB0300");
    // The diagnostic points at the offending second argument.
    let label = diags[0].primary_label().expect("primary label");
    assert_eq!(&src[label.span.range.clone()], "\"x\"");
}

// --- 3. bounded (constraint) type parameters ----------------------------

const SHAPE: &str = "\
---@class Shape
---@field area fun(self): number
";

#[test]
fn generic_constraint_violation_reports() {
    let src = format!(
        "{SHAPE}
---@generic T : Shape
---@param x T
---@return T
local function identity(x)
  return x
end
identity(5)
"
    );
    assert_eq!(codes(&src), vec!["LB0300"]);
}

#[test]
fn generic_constraint_satisfied_is_clean() {
    let src = format!(
        "{SHAPE}
---@generic T : Shape
---@param x T
---@return T
local function identity(x)
  return x
end
---@type Shape
local s = {{ area = function(self) return 1 end }}
identity(s)
"
    );
    assert_eq!(codes(&src), Vec::<String>::new());
}

// --- 4. backtick capture ------------------------------------------------

#[test]
fn backtick_captures_class_from_string_literal() {
    let src = "\
---@class Circle
---@field radius number

---@generic T
---@param name `T`
---@return T
local function new(name) end

---@param n number
local function wantn(n) end

local c = new(\"Circle\")
wantn(c.radius)
";
    assert_eq!(codes(src), Vec::<String>::new());

    // The captured type is really `Circle`: reading `radius` as a string errors.
    let bad = "\
---@class Circle
---@field radius number

---@generic T
---@param name `T`
---@return T
local function new(name) end

---@param s string
local function wants(s) end

local c = new(\"Circle\")
wants(c.radius)
";
    assert_eq!(codes(bad), vec!["LB0300"]);
}

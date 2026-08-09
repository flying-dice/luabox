// test code — panics document assumptions
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::string_slice
)]
//! Real LuaCATS generics end-to-end (#84): generic `---@class<T>` references,
//! `---@generic` functions with call-site inference, backtick capture, and
//! bounded (`: Constraint`) type parameters — checked through the public
//! [`luabox_types::check_file`] API, matching lua-language-server semantics.

use luabox_diag::Diagnostic;
use luabox_syntax::lua::{Dialect, parse};
use luabox_types::{Strictness, build_ambient, check_file, check_file_with_ambient};

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

// --- 5. generic classes through a `[types] defs` package ----------------

/// Strict-mode codes for a file checked against an ambient built from `defs`.
fn ambient_codes(source: &str, defs: &[&str]) -> Vec<String> {
    let parsed = parse(source, Dialect::Lua54);
    assert_eq!(parsed.errors(), &[], "fixture must parse cleanly");
    let owned: Vec<String> = defs.iter().map(|s| (*s).to_string()).collect();
    let ambient = build_ambient(Dialect::Lua54, &owned);
    check_file_with_ambient(
        &parsed,
        "test.lua",
        Strictness::Strict,
        Dialect::Lua54,
        Some(&ambient),
    )
    .iter()
    .map(|d| d.code.to_string())
    .collect()
}

const BOX_DEF: &str = "\
---@meta
---@class Box<T>
---@field value T
";

#[test]
fn generic_class_declared_in_a_defs_package_instantiates_in_a_consumer() {
    // The generic template is registered from the *ambient* layer, not from
    // the consuming file's own annotations.
    let good = "\
---@type Box<number>
local b = { value = 1 }
";
    assert_eq!(ambient_codes(good, &[BOX_DEF]), Vec::<String>::new());

    let bad = "\
---@type Box<number>
local b = { value = \"x\" }
";
    assert_eq!(ambient_codes(bad, &[BOX_DEF]), vec!["LB0300"]);
}

const MAP_DEF: &str = "\
---@meta
---@class Map<K, V>
---@field [K] V
";

#[test]
fn generic_class_indexer_fields_substitute_both_parameters() {
    // `---@field [K] V` becomes an indexer on the template; both type
    // parameters must be substituted at the reference site.
    let good = "\
---@type Map<string, number>
local m = { alpha = 1 }
";
    assert_eq!(ambient_codes(good, &[MAP_DEF]), Vec::<String>::new());

    // The *value* parameter is enforced...
    let bad_value = "\
---@type Map<string, number>
local m = { alpha = \"one\" }
";
    assert_eq!(ambient_codes(bad_value, &[MAP_DEF]), vec!["LB0300"]);

    // ...and so is the *key* parameter: a string key does not match `[integer]`.
    let bad_key = "\
---@type Map<integer, number>
local m = { alpha = 1 }
";
    assert_eq!(ambient_codes(bad_key, &[MAP_DEF]), vec!["LB0303"]);
}

#[test]
fn locally_declared_generic_class_indexer_fields_substitute() {
    let src = "\
---@class Lookup<K, V>
---@field [K] V

---@type Lookup<string, boolean>
local ok = { flag = true }
---@type Lookup<string, boolean>
local bad = { flag = 1 }
";
    assert_eq!(codes(src), vec!["LB0300"]);
}

// --- F36: an inherited indexer's KEY, not just its value, is substituted ---

const MAP_CLASS: &str = "\
---@class Map<K, V>
---@field [K] V
";

#[test]
fn inherited_indexer_key_is_substituted_not_left_free() {
    // `Users : Map<integer, number>` inherits an indexer keyed on `K`. If the
    // key is left as the free `Ty::Named(\"K\")` (only the value substituted),
    // `assign.rs` resolves the unbound key as "matches anything", so a
    // `string`-keyed literal is wrongly accepted with no LB0303 — the false
    // accept `env.rs:1251` produces. The direct (non-inherited) spelling
    // already substitutes both (`generic_class_indexer_fields_substitute_both_parameters`
    // above); this pins the inheritance path to the same rule.
    let bad_key = format!(
        "{MAP_CLASS}
---@class Users : Map<integer, number>

---@type Users
local u = {{ alpha = 1 }}
"
    );
    assert_eq!(
        codes(&bad_key),
        vec!["LB0303"],
        "an inherited indexer's key must reject a mismatched literal key exactly as the direct spelling does"
    );

    // The rejecting probe's pair: a correctly-keyed literal must still be
    // accepted, so the fix is a substitution — not a blanket rejection.
    let good_key = format!(
        "{MAP_CLASS}
---@class Users : Map<integer, number>

---@type Users
local u = {{ [1] = 2 }}
"
    );
    assert_eq!(codes(&good_key), Vec::<String>::new());

    // The value parameter must still be enforced through inheritance too.
    let bad_value = format!(
        "{MAP_CLASS}
---@class Users : Map<integer, number>

---@type Users
local u = {{ [1] = \"not a number\" }}
"
    );
    assert_eq!(codes(&bad_value), vec!["LB0300"]);
}

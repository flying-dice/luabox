//! Call-return propagation to unannotated locals (#106), def-declared scalar
//! fields on global tables (#105), and overload-aware call results + tuple
//! types (#86). Matching lua-language-server behavior (ecosystem parity).
//!
//! Each headline behavior is proven by *misuse*: the propagated type is fed to
//! a `number` parameter, so a `string` (etc.) result surfaces as `LB0300`.

use luabox_diag::Diagnostic;
use luabox_syntax::lua::{Dialect, parse};
use luabox_types::{Strictness, check_file, check_file_with_ambient, combined_defs};

/// Strict-mode diagnostic codes for a file checked against the stdlib (plus
/// any extra `.d.lua` sources) ambient layer.
fn ambient_codes(src: &str, defs: &[String]) -> Vec<String> {
    let parsed = parse(src, Dialect::Lua54);
    assert_eq!(parsed.errors(), &[], "fixture must parse cleanly");
    let ambient = combined_defs(Dialect::Lua54, defs);
    codes(&check_file_with_ambient(
        &parsed,
        "test.lua",
        Strictness::Strict,
        Some(&ambient),
    ))
}

/// Strict-mode diagnostic codes for a self-contained file (no ambient).
fn strict_codes(src: &str) -> Vec<String> {
    let parsed = parse(src, Dialect::Lua54);
    assert_eq!(parsed.errors(), &[], "fixture must parse cleanly");
    codes(&check_file(&parsed, "test.lua", Strictness::Strict))
}

/// Warn-mode codes against the stdlib (plus extra defs) — `unknown` is
/// lenient here, so an unannotated-callee result that stays `unknown`
/// produces nothing, while a *mistyped* result would still mismatch.
fn warn_ambient_codes(src: &str, defs: &[String]) -> Vec<String> {
    let parsed = parse(src, Dialect::Lua54);
    assert_eq!(parsed.errors(), &[], "fixture must parse cleanly");
    let ambient = combined_defs(Dialect::Lua54, defs);
    codes(&check_file_with_ambient(
        &parsed,
        "test.lua",
        Strictness::Warn,
        Some(&ambient),
    ))
}

fn codes(diags: &[Diagnostic]) -> Vec<String> {
    diags.iter().map(|d| d.code.to_string()).collect()
}

const WANT_NUMBER: &str = "\
---@param n number
local function want(n) end
";

// === #106: call return propagates to unannotated locals ===================

#[test]
fn stdlib_call_return_types_local() {
    // `string.rep` (an ambient dotted function) returns `string`; the
    // unannotated `p` must carry it so the misuse is caught.
    let src = format!(
        "{WANT_NUMBER}\
local p = string.rep(\"x\", 3)
want(p)
"
    );
    assert_eq!(ambient_codes(&src, &[]), vec!["LB0300"]);
}

#[test]
fn own_annotated_function_return_types_local() {
    let src = format!(
        "\
---@return string
local function name() return \"x\" end
{WANT_NUMBER}\
local p = name()
want(p)
"
    );
    assert_eq!(strict_codes(&src), vec!["LB0300"]);
}

#[test]
fn multi_return_distributes_positionally() {
    // `local a, b = pair()` — `a` = string, `b` = number.
    let src = format!(
        "\
---@return string
---@return number
local function pair() return \"x\", 1 end
{WANT_NUMBER}\
local a, b = pair()
want(a)
want(b)
"
    );
    // `a` (string) misuses; `b` (number) is fine.
    assert_eq!(strict_codes(&src), vec!["LB0300"]);
}

#[test]
fn non_last_call_truncates_to_one_value() {
    // `local a, b = two(), num()` — the non-last `two()` truncates to its
    // first value (string), `num()` supplies `b` (number).
    let src = format!(
        "\
---@return string
---@return string
local function two() return \"a\", \"b\" end
---@return number
local function num() return 1 end
{WANT_NUMBER}\
local a, b = two(), num()
want(a)
want(b)
"
    );
    // Only `a` (string) misuses; `b` is number.
    assert_eq!(strict_codes(&src), vec!["LB0300"]);
}

#[test]
fn annotated_local_overrides_call_return() {
    // `---@type` is authoritative: a subtype call return (integer -> number)
    // binds `p` at `number`; no assignment or use error.
    let src = format!(
        "\
---@return integer
local function mk() return 1 end
---@type number
local p = mk()
{WANT_NUMBER}\
want(p)
"
    );
    assert_eq!(strict_codes(&src), Vec::<String>::new());
}

#[test]
fn unannotated_ambient_callee_stays_unknown() {
    // An ambient function without `---@return` yields `unknown`, not a
    // fabricated type: in warn mode (where `unknown` is lenient) the later use
    // produces nothing. A *mistyped* result would mismatch even here.
    let def = "\
---@meta
---@class futzlib
futz = {}
---@param x number
function futz.go(x) end
"
    .to_string();
    let src = format!(
        "{WANT_NUMBER}\
local p = futz.go(1)
want(p)
"
    );
    assert_eq!(warn_ambient_codes(&src, &[def]), Vec::<String>::new());
}

#[test]
fn defs_global_dotted_api_return_types_local() {
    let def = "\
---@meta
---@class ziplib
zlib = {}
---@param s string
---@return string
function zlib.compress(s) end
"
    .to_string();
    let src = format!(
        "{WANT_NUMBER}\
local r = zlib.compress(\"x\")
want(r)
"
    );
    assert_eq!(ambient_codes(&src, &[def]), vec!["LB0300"]);
}

#[test]
fn method_call_known_sig_return_types_local() {
    // `obj:m()` resolves through the class field's function signature.
    let src = format!(
        "\
---@class Greeter
---@field greet fun(self): string
{WANT_NUMBER}\
---@param g Greeter
local function use_it(g)
  local s = g:greet()
  want(s)
end
"
    );
    assert_eq!(strict_codes(&src), vec!["LB0300"]);
}

// === #105: def-declared scalar fields on global tables ====================

#[test]
fn annotated_scalar_field_typed_at_read_site() {
    let def = "\
---@meta
---@class ziplib
zlib = {}
---@type string
zlib.version = \"1.3.1\"
"
    .to_string();
    let src = format!(
        "{WANT_NUMBER}\
want(zlib.version)
"
    );
    assert_eq!(ambient_codes(&src, &[def]), vec!["LB0300"]);
}

#[test]
fn nested_scalar_field_typed_at_read_site() {
    let def = "\
---@meta
---@class mylib
mylib = {}
---@class mylib.sub
mylib.sub = {}
---@type number
mylib.sub.const = 42
---@type string
mylib.sub.name = \"x\"
"
    .to_string();
    let src = format!(
        "{WANT_NUMBER}\
want(mylib.sub.const)
want(mylib.sub.name)
"
    );
    // `const` (number) fine; `name` (string) misuses.
    assert_eq!(ambient_codes(&src, &[def]), vec!["LB0300"]);
}

#[test]
fn bare_typed_literal_field_widens() {
    // No annotation — the literal's widened type surfaces (luals behavior).
    let def = "\
---@meta
---@class ziplib
zlib = {}
zlib.version = \"1.3.1\"
"
    .to_string();
    let src = format!(
        "{WANT_NUMBER}\
want(zlib.version)
"
    );
    assert_eq!(ambient_codes(&src, &[def]), vec!["LB0300"]);
}

#[test]
fn function_fields_unregressed() {
    let def = "\
---@meta
---@class ziplib
zlib = {}
---@type string
zlib.version = \"1.3.1\"
---@param s string
---@return string
function zlib.compress(s) end
"
    .to_string();
    let src = format!(
        "{WANT_NUMBER}\
local r = zlib.compress(\"x\")
want(r)
"
    );
    assert_eq!(ambient_codes(&src, &[def]), vec!["LB0300"]);
}

#[test]
fn stdlib_version_constant_is_string() {
    // `_VERSION` is a real Lua string constant declared in basic.d.lua.
    let src = format!(
        "{WANT_NUMBER}\
want(_VERSION)
"
    );
    assert_eq!(ambient_codes(&src, &[]), vec!["LB0300"]);
}

#[test]
fn plain_global_table_scalar_field_typed_at_read_site() {
    // The issue's own example: NO `---@class` on the base table (the dominant
    // def style — love2d's `love = {}`). The annotated field must still
    // surface (luals types it).
    let def = "\
---@meta
zlib = {}
---@type string
zlib.version = \"1.3.1\"
"
    .to_string();
    let src = format!(
        "{WANT_NUMBER}\
want(zlib.version)
"
    );
    assert_eq!(ambient_codes(&src, &[def]), vec!["LB0300"]);
}

#[test]
fn plain_global_table_nested_scalar_field() {
    // Plain nested sub-table: `zlib.sub = {}` (no annotations anywhere on the
    // path) still folds, and both annotated and literal-widened leaf fields
    // read back typed.
    let def = "\
---@meta
zlib = {}
zlib.sub = {}
---@type number
zlib.sub.const = 42
zlib.sub.bare = \"hello\"
"
    .to_string();
    let src = format!(
        "{WANT_NUMBER}\
want(zlib.sub.const)
want(zlib.sub.bare)
"
    );
    // `const` (number) fine; `bare` (literal-widened string) misuses.
    assert_eq!(ambient_codes(&src, &[def]), vec!["LB0300"]);
}

#[test]
fn plain_global_table_function_fields_unregressed() {
    // Function fields of a plain (`---@class`-less) global table keep
    // registering by dotted name alongside the folded scalar.
    let def = "\
---@meta
zlib = {}
---@type string
zlib.version = \"1.3.1\"
---@param s string
---@return string
function zlib.compress(s) end
"
    .to_string();
    let src = format!(
        "{WANT_NUMBER}\
local r = zlib.compress(\"x\")
want(r)
want(zlib.version)
"
    );
    // Both the call result and the scalar field are `string` misuses.
    assert_eq!(ambient_codes(&src, &[def]), vec!["LB0300", "LB0300"]);
}

// === #86: overload-aware call results + tuple types =======================

#[test]
fn matching_overload_drives_return_type() {
    // The primary rejects a string arg; the `---@overload` accepts it, so the
    // call's result is the overload's `string` return.
    let src = format!(
        "\
---@overload fun(x: string): string
---@param x number
---@return number
local function f(x) end
{WANT_NUMBER}\
local r = f(\"hi\")
want(r)
"
    );
    assert_eq!(strict_codes(&src), vec!["LB0300"]);
}

#[test]
fn primary_signature_still_used_when_it_matches() {
    let src = "\
---@overload fun(x: string): string
---@param x number
---@return number
local function f(x) end
---@param s string
local function wantstr(s) end
local r = f(1)
wantstr(r)
";
    // `f(1)` matches the primary => `number` => misuse against `string`.
    assert_eq!(strict_codes(src), vec!["LB0300"]);
}

#[test]
fn no_matching_overload_reports_against_primary() {
    let src = "\
---@overload fun(x: string): string
---@param x number
---@return number
local function f(x) end
f(true)
";
    // Neither the primary (number) nor the overload (string) accepts a
    // boolean — the argument error is reported against the primary.
    assert_eq!(strict_codes(src), vec!["LB0300"]);
}

#[test]
fn tuple_positional_reads_flow_per_position() {
    let src = format!(
        "\
---@type [string, number]
local t = {{ \"a\", 1 }}
{WANT_NUMBER}\
---@param s string
local function wantstr(s) end
wantstr(t[1])
want(t[2])
"
    );
    // `t[1]` = string (fine against string), `t[2]` = number (fine against
    // number): no misuse.
    assert_eq!(strict_codes(&src), Vec::<String>::new());
}

#[test]
fn tuple_positional_read_misuse_per_position() {
    let src = format!(
        "\
---@type [string, number]
local t = {{ \"a\", 1 }}
{WANT_NUMBER}\
want(t[1])
"
    );
    // `t[1]` = string, fed to `number` => misuse.
    assert_eq!(strict_codes(&src), vec!["LB0300"]);
}

#[test]
fn tuple_in_param_position_checks_literal_per_position() {
    let src = "\
---@param t [string, number]
local function f(t) end
f({ \"a\", 1 })
f({ 1, \"a\" })
";
    // First call conforms; second is wrong at both positions.
    assert_eq!(strict_codes(src), vec!["LB0300", "LB0300"]);
}

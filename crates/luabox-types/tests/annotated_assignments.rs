// test code — panics document assumptions
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! `---@type fun(…)` and doc-block tags on an *assignment* (#38).
//!
//! `Carrier.m = function(…) end` is the assignment spelling of a function
//! definition, and luals binds a doc block to the function value there exactly
//! as it does above `function Carrier.m()`. An explicit `---@type fun(…)` is
//! authoritative for the value (SPEC §3): it supplies the signature, and the
//! block's use-site tags (`---@deprecated`/`---@async`/`---@nodiscard`/
//! `---@version`) — which `fun(…)` syntax cannot express — ride along.
//!
//! `---@type A, B` is positional, as on a `local`, so a lone annotation over a
//! multi-assignment declares the first target only.

use luabox_diag::Diagnostic;
use luabox_syntax::lua;
use luabox_types::{Strictness, check_file_with_ambient, stdlib_defs};

fn check(src: &str) -> Vec<Diagnostic> {
    let parse = lua::parse(src, lua::Dialect::Lua54);
    assert_eq!(parse.errors(), &[], "fixture must parse cleanly");
    check_file_with_ambient(
        &parse,
        "test.lua",
        Strictness::Strict,
        lua::Dialect::Lua54,
        Some(stdlib_defs(lua::Dialect::Lua54)),
    )
}

fn strict_codes(src: &str) -> Vec<String> {
    check(src).iter().map(|d| d.code.to_string()).collect()
}

// --- the declared signature reaches the value -----------------------------

#[test]
fn typed_assignment_signature_checks_a_dotted_call() {
    let src = "\
local M = {}
---@type fun(n: integer)
M.f = function(n) end
M.f(\"nope\")
return M
";
    assert_eq!(strict_codes(src), vec!["LB0300"]);
}

#[test]
fn typed_assignment_signature_accepts_a_correct_call() {
    let src = "\
local M = {}
---@type fun(n: integer)
M.f = function(n) end
M.f(1)
return M
";
    assert_eq!(strict_codes(src), Vec::<String>::new());
}

#[test]
fn typed_assignment_signature_governs_arity() {
    let src = "\
local M = {}
---@type fun(n: integer)
M.f = function(n) end
M.f()
return M
";
    assert_eq!(strict_codes(src), vec!["LB0301"]);
}

#[test]
fn typed_assignment_return_type_flows_into_the_caller() {
    let src = "\
local M = {}
---@type fun(): string
M.f = function() return \"x\" end
---@type integer
local n = M.f()
return n
";
    assert_eq!(strict_codes(src), vec!["LB0300"]);
}

#[test]
fn typed_assignment_to_a_bare_global_is_applied() {
    let src = "\
---@type fun(n: integer)
handler = function(n) end
handler(\"nope\")
";
    assert_eq!(strict_codes(src), vec!["LB0300"]);
}

#[test]
fn typed_assignment_types_the_literal_parameters_bidirectionally() {
    // The bidirectional rule `---@type` on a `local` follows (#120) applies
    // to the assignment spelling too: `w` is a `Widget` inside the lambda, so
    // a bad field read is flagged.
    let src = "\
---@class Widget
---@field size number
local Widget = {}

local M = {}
---@type fun(w: Widget)
M.use = function(w)
  return w.nofield
end
return M, Widget
";
    assert_eq!(strict_codes(src), vec!["LB0306"]);
}

// --- the block's tags ride along ------------------------------------------

#[test]
fn deprecated_rides_a_typed_assignment_to_the_call_site() {
    let src = "\
local M = {}
---@deprecated
---@type fun(n: integer)
M.f = function(n) end
M.f(1)
return M
";
    assert_eq!(strict_codes(src), vec!["LB0308"]);
}

#[test]
fn async_rides_a_typed_assignment_to_the_call_site() {
    let src = "\
local M = {}
---@async
---@type fun()
M.f = function() end
local function sync()
  M.f()
end
return M, sync
";
    assert_eq!(strict_codes(src), vec!["LB0316"]);
}

#[test]
fn nodiscard_rides_a_typed_assignment_to_the_call_site() {
    let src = "\
local M = {}
---@nodiscard
---@type fun(): integer
M.f = function() return 1 end
M.f()
return M
";
    assert_eq!(strict_codes(src), vec!["LB0309"]);
}

#[test]
fn a_bare_block_signature_on_an_assignment_is_applied() {
    // The `---@param`/`---@return` spelling attaches to an assignment the same
    // way it attaches to `function M.f()` — the other half of #38.
    let src = "\
local M = {}
---@deprecated
---@param n integer
M.f = function(n) end
M.f(\"nope\")
return M
";
    assert_eq!(strict_codes(src), vec!["LB0308", "LB0300"]);
}

// --- method-style assignments ---------------------------------------------

#[test]
fn typed_method_assignment_surfaces_tags_at_a_colon_call() {
    // The shape #38 was found in: `---@deprecated` + `---@type fun(self: …)`
    // above `C.m = function(self) end`, called as `o:m()`.
    let src = "\
---@class Cls
local Cls = {}

---@deprecated
---@async
---@type fun(self: Cls, n: integer)
Cls.m = function(self, n) end

---@type Cls
local o
local function use()
  o:m(1)
end
return Cls, use
";
    assert_eq!(strict_codes(src), vec!["LB0308", "LB0316"]);
}

#[test]
fn typed_method_assignment_checks_arguments_at_a_colon_call() {
    // The implicit `self` is not an explicit argument: `n` is checked against
    // the declared `integer`.
    let src = "\
---@class Cls
local Cls = {}

---@type fun(self: Cls, n: integer)
Cls.m = function(self, n) end

---@type Cls
local o
o:m(\"nope\")
return Cls
";
    assert_eq!(strict_codes(src), vec!["LB0300"]);
}

// --- multi-assignment: the annotation is positional ------------------------

#[test]
fn one_annotation_over_a_multi_assignment_declares_the_first_target_only() {
    let src = "\
local M = {}
---@type fun(a: integer)
M.a, M.b = function(a) end, function(b) end
M.a(\"nope\")
M.b(\"anything\")
return M
";
    assert_eq!(strict_codes(src), vec!["LB0300"]);
}

#[test]
fn a_two_slot_annotation_over_a_multi_assignment_declares_both() {
    let src = "\
local M = {}
---@type fun(a: integer), fun(b: string)
M.a, M.b = function(a) end, function(b) end
M.a(\"nope\")
M.b(1)
return M
";
    assert_eq!(strict_codes(src), vec!["LB0300", "LB0300"]);
}

// --- parity boundaries -----------------------------------------------------

#[test]
fn a_signature_mismatch_against_the_literal_is_silent() {
    // luals has no "declared signature disagrees with the literal" diagnostic:
    // `---@type` simply covers the value's type, and the literal's own extra
    // or missing parameters are not compared against it. Silence is parity.
    let src = "\
local M = {}
---@type fun(a: integer)
M.extra = function(a, b, c) end
---@type fun(a: integer, b: integer)
M.missing = function(a) end
return M
";
    assert_eq!(strict_codes(src), Vec::<String>::new());
}

#[test]
fn a_declared_signature_still_governs_a_mismatched_literal() {
    // ...and the *declared* signature is what call sites see, whichever way
    // the literal's own parameter list disagrees.
    let src = "\
local M = {}
---@type fun(a: integer)
M.extra = function(a, b, c) end
M.extra(1, 2)
return M
";
    assert_eq!(strict_codes(src), vec!["LB0301"]);
}

#[test]
fn a_non_function_typed_assignment_is_unchanged() {
    // `---@type` over a non-function value on an assignment is out of scope:
    // it declares nothing callable and adds no diagnostic (the conservative
    // boundary — a `---@type` local is where that rule is enforced).
    let src = "\
local M = {}
---@type integer
M.n = \"not an integer\"
return M
";
    assert_eq!(strict_codes(src), Vec::<String>::new());
}

#[test]
fn a_typed_assignment_over_a_non_literal_right_hand_side_is_unchanged() {
    // `---@type fun(…)` over a *name* (not a function literal) declares the
    // variable but defines no callable here; the existing inference path owns
    // it, and nothing new is manufactured.
    let src = "\
local M = {}
local other = function(a, b) end
---@type fun(a: integer)
M.f = other
M.f(1, 2)
return M
";
    assert_eq!(strict_codes(src), Vec::<String>::new());
}

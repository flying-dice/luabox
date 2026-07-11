//! Undefined-field reads on declared `---@class` values (#90) — luals'
//! `undefined-field`, mapped onto luabox's strictness ladder (SPEC.md §19).
//!
//! Reading a field a value's declared class shape does not provide — `self.f`
//! in a class method, `x.f` on a `---@type Class` local — is `LB0306`, a
//! warning in warn mode and an error in strict (stricter than luals, which is
//! always a warning). Un-annotated code stays lenient: a declaration is the
//! precondition for the obligation.

use luabox_diag::Severity;
use luabox_syntax::lua::{Dialect, parse};
use luabox_types::{Strictness, check_file};

fn diags(source: &str, strictness: Strictness) -> Vec<luabox_diag::Diagnostic> {
    let parsed = parse(source, Dialect::Lua54);
    assert_eq!(parsed.errors(), &[], "fixture must parse cleanly");
    check_file(&parsed, "test.lua", strictness)
}

fn strict_codes(source: &str) -> Vec<String> {
    diags(source, Strictness::Strict)
        .iter()
        .map(|d| d.code.to_string())
        .collect()
}

// (a) `self.nope` inside a class method → flagged, naming field and class.
#[test]
fn self_read_of_undeclared_field_is_flagged() {
    let src = "\
---@class Point
---@field x number
---@field y number
local Point = {}
Point.__index = Point

function Point:shift()
  return self.nope
end
";
    let ds = diags(src, Strictness::Strict);
    assert_eq!(
        ds.iter().map(|d| d.code.to_string()).collect::<Vec<_>>(),
        vec!["LB0306"]
    );
    assert!(
        ds[0].message.contains("`nope`") && ds[0].message.contains("`Point`"),
        "names field and class: {}",
        ds[0].message
    );
    // In-file class carries a "declared here" secondary label (#107/#80 plumbing).
    assert!(
        ds[0].labels.iter().any(|l| !l.primary),
        "expected a decl-site secondary label: {:?}",
        ds[0].labels
    );
}

// (b) `p.nope` on a `---@type Class` local → flagged.
#[test]
fn typed_local_read_of_undeclared_field_is_flagged() {
    let src = "\
---@class Point
---@field x number
---@field y number

---@type Point
local p = { x = 1, y = 2 }
local ok = p.x
local bad = p.nope
return ok, bad
";
    assert_eq!(strict_codes(src), vec!["LB0306"]);
}

// (c) declared field, parent-inherited field, and carrier inherent method
//     reads are all clean.
#[test]
fn declared_inherited_and_carrier_method_reads_are_clean() {
    // `id?` is optional so the `: Base` carrier conformance (#107) imposes no
    // obligation — this test isolates the *read* rule.
    let src = "\
---@class Base
---@field id? number

---@class Widget : Base
---@field size number
local Widget = {}
Widget.__index = Widget

function Widget:helper()
  return 1
end

function Widget:use()
  local a = self.size    -- own declared field
  local b = self.id      -- inherited from Base
  local c = self:helper() -- carrier inherent method
  return a, b, c
end
";
    assert_eq!(strict_codes(src), Vec::<String>::new());
}

// (d) a class with an indexer stays open (dynamic access is declared).
#[test]
fn class_with_indexer_is_clean() {
    let src = "\
---@class Bag
---@field size number
---@field [string] boolean

---@type Bag
local b = { size = 1 }
local x = b.anything
return x
";
    assert_eq!(strict_codes(src), Vec::<String>::new());
}

// (e) an unknown / unannotated base invents no obligation.
#[test]
fn unannotated_base_is_clean() {
    let src = "\
local function f(t)
  return t.whatever
end
return f
";
    assert_eq!(strict_codes(src), Vec::<String>::new());
}

// (f) strict → error, warn → warning (stricter than luals, which is always a
//     warning).
#[test]
fn severity_rides_the_ladder() {
    let src = "\
---@class Point
---@field x number
local Point = {}
Point.__index = Point

function Point:shift()
  return self.nope
end
";
    let strict = diags(src, Strictness::Strict);
    assert_eq!(strict.len(), 1);
    assert_eq!(strict[0].code.to_string(), "LB0306");
    assert_eq!(strict[0].severity, Severity::Error);

    let warn = diags(src, Strictness::Warn);
    assert_eq!(warn.len(), 1);
    assert_eq!(warn[0].code.to_string(), "LB0306");
    assert_eq!(warn[0].severity, Severity::Warning);

    // `none` suppresses the whole type layer.
    assert!(diags(src, Strictness::None).is_empty());
}

// (h) `---@diagnostic disable: undefined-field` suppresses it — luals'
//     suppression vocabulary, honored checker-side for this rule.
#[test]
fn diagnostic_disable_suppresses_it() {
    let file_wide = "\
---@diagnostic disable: undefined-field
---@class Point
---@field x number
local Point = {}
Point.__index = Point

function Point:shift()
  return self.nope
end
";
    assert_eq!(strict_codes(file_wide), Vec::<String>::new());

    // A line-scoped directive suppresses only its read; a second, un-guarded
    // read still fires.
    let line_scoped = "\
---@class Point
---@field x number
local Point = {}
Point.__index = Point

function Point:shift()
  ---@diagnostic disable-next-line: undefined-field
  local a = self.nope
  local b = self.other
  return a, b
end
";
    assert_eq!(strict_codes(line_scoped), vec!["LB0306"]);

    // A directive naming a *different* rule does not suppress it.
    let unrelated = "\
---@diagnostic disable: undefined-global
---@class Point
---@field x number
local Point = {}
Point.__index = Point

function Point:shift()
  return self.nope
end
";
    assert_eq!(strict_codes(unrelated), vec!["LB0306"]);
}

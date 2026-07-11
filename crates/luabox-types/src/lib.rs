//! Unified type IR and checker — **Semantics** bounded context
//! (SPEC.md §3, §16).
//!
//! One internal type IR fed (eventually) by two front-ends: LuaCATS
//! annotations (`---@class` etc., full compatibility non-negotiable) and
//! the `.luab` shape DSL (SHAPES.md). One checker, no parallel type system.
//!
//! **P0 scope (this crate today):** the annotation-driven subset behind
//! `luabox check` — types come from LuaCATS annotations and literals only.
//! The load-bearing design decision is the *structural* table
//! representation ([`ty::TableTy`]): field map + indexers + array part,
//! never an opaque `table` primitive, so checking a table literal against
//! a `---@class` parameter produces field-level diagnostics and P1's rich
//! table inference (SPEC.md §3 hard requirement) extends this IR instead
//! of replacing it.
//!
//! **P1 (TODO):** bidirectional inference, flow-sensitive narrowing,
//! metatable/`__index` resolution, method calls, generics as real type
//! variables, cross-file `require` resolution over the salsa DB, `.luab`
//! shape checking, function subtyping.
//!
//! Diagnostics carry `LB03xx` codes registered in `luabox-diag` (this
//! crate depends on it the way rustc crates depend on `rustc_errors`).

mod assign;
mod check;
mod defs;
mod directive;
mod env;
mod generics;
mod infer;
mod lower;
pub mod shape;
pub mod ty;

pub use assign::assignable;
pub use defs::{
    Ambient, DefFile, combined as combined_defs, combined_checked as combined_defs_checked,
    stdlib as stdlib_defs,
};
pub use env::TypeEnv;
pub use infer::{ExternalTypes, InferredBinding, InferredReturn};
pub use shape::{DepShapeExport, ShapeOptions, ShapeStore};

use luabox_diag::Diagnostic;
use luabox_syntax::{lua, luacats};

/// The strictness ladder (SPEC.md §3): `none` → `warn` → `strict`.
///
/// - `None`: type diagnostics are suppressed entirely.
/// - `Warn`: mismatches are warnings; `unknown` is assignable both ways.
/// - `Strict`: mismatches are errors; `unknown -> T` is itself a mismatch
///   (untyped = `unknown`, not `any`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strictness {
    /// No type diagnostics.
    None,
    /// Warning severity, permissive `unknown`.
    Warn,
    /// Error severity, strict `unknown`.
    Strict,
}

impl Strictness {
    /// Map the manifest's `[types] strict` boolean: `true` → strict,
    /// `false` → warn. TODO: surface the full three-level ladder (plus
    /// per-file overrides) in the manifest; `None` is currently only
    /// reachable programmatically.
    #[must_use]
    pub fn from_manifest_flag(strict: bool) -> Self {
        if strict {
            Strictness::Strict
        } else {
            Strictness::Warn
        }
    }
}

/// The display-inference surface behind editor inlay hints: every named
/// binding's final inferred type, every unannotated function's inferred
/// return types, and the cross-file exchange surface (the module's export
/// type + observed outgoing call arguments).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DisplayTypes {
    /// Every binding's reified type at its declaration range.
    pub bindings: Vec<InferredBinding>,
    /// Inferred returns per unannotated function, keyed by source range.
    pub returns: Vec<InferredReturn>,
    /// The inferred type of the chunk's `return` value — what a dependent
    /// file's `require` of this module evaluates to.
    pub module_export: Option<ty::Ty>,
    /// Argument types observed at calls of functions this file does not
    /// define, keyed by terminal callee name — parameter seeds for the
    /// files this one requires.
    pub outgoing_calls: std::collections::HashMap<String, Vec<ty::Ty>>,
}

/// Run the rich table inference in *display mode* over one parsed file —
/// the editor inlay-hint surface.
///
/// Same inference as [`check_file`] (annotations stay authoritative;
/// `ambient` merges definition-package globals beneath the file's own
/// declarations, exactly as in [`check_file_shaped`]) with two additions:
///
/// - **Call-site parameter seeding** — an unannotated parameter takes the
///   union of the argument types observed at the function's call sites, so
///   bodies of unannotated functions type through.
/// - **Cross-file inputs** (`externals`) — `require("mod")` evaluates to
///   the target module's export type, and exported functions' parameters
///   seed from dependent files' observed call arguments.
///
/// Display-only — the checker never sees seeded types, so no diagnostic
/// can arise from them.
#[must_use]
pub fn infer_display_types(
    parse: &lua::Parse,
    file: &str,
    ambient: Option<&Ambient>,
    externals: Option<&ExternalTypes>,
) -> DisplayTypes {
    let items = luacats::harvest(parse);
    let env = TypeEnv::build_from_items(parse, &items, None, ambient);
    let lowered = luabox_hir::lower(parse);
    let outcome = infer::run(&lowered, &env, file, false, true, externals);
    DisplayTypes {
        bindings: outcome.binding_types,
        returns: outcome.fn_returns,
        module_export: outcome.module_export,
        outgoing_calls: outcome.outgoing_calls,
    }
}

/// Typecheck one parsed file against its own annotations (no `.luab` shape
/// resolution — see [`check_file_shaped`]).
///
/// `file` names the file in diagnostic spans. Cross-file `require`
/// resolution is P1 — every file is checked against a per-file
/// environment.
#[must_use]
pub fn check_file(parse: &lua::Parse, file: &str, strictness: Strictness) -> Vec<Diagnostic> {
    check_file_shaped(parse, file, strictness, None, None)
}

/// Typecheck one parsed file with the ambient `.luab` package scope in
/// reach when `shapes` is provided (SHAPES-V2.md).
///
/// There are no shape tags and no imports: `.luab` types resolve in the
/// standard annotation positions by fully-qualified name, and conformance
/// is positional — a value is checked against a shape type exactly where
/// one is demanded. The scope is built once per store and shared, so a
/// file that never names a shape type pays a lookup miss, not scope
/// construction (the v2 zero-cost invariant).
///
/// `ambient` is the definition-package layer selected by the project
/// `edition` ([`stdlib_defs`] / [`combined_defs`]): its stdlib globals and
/// module tables become visible to both the checker and inference, merged
/// beneath the file's own declarations (SPEC.md §3).
#[must_use]
pub fn check_file_shaped(
    parse: &lua::Parse,
    file: &str,
    strictness: Strictness,
    shapes: Option<&ShapeOptions<'_>>,
    ambient: Option<&Ambient>,
) -> Vec<Diagnostic> {
    let items = luacats::harvest(parse);
    // A `---@meta` definition package: its `---@class` declarations are
    // contracts, not carriers, so no `: Interface` conformance runs inside it
    // (#107).
    let is_meta = items.iter().any(|it| {
        it.block
            .tags
            .iter()
            .any(|t| matches!(t, luacats::Tag::Meta(_)))
    });

    let mut diags: Vec<Diagnostic> = Vec::new();
    let scope = shapes.map(ShapeOptions::scope);

    let env = TypeEnv::build_from_items(parse, &items, scope.as_deref(), ambient);
    let mut inferred_types = std::collections::HashMap::new();
    if strictness != Strictness::None {
        // Rich table inference (SPEC.md §3) runs first: the checker uses
        // its published types wherever annotations are absent (annotations
        // always win), and inference contributes its own diagnostics
        // (LB0306) at the same strictness-mapped severity.
        let lowered = luabox_hir::lower(parse);
        let inference = infer::run(
            &lowered,
            &env,
            file,
            strictness == Strictness::Strict,
            false,
            None,
        );
        diags.extend(check::run(
            parse,
            &env,
            file,
            strictness == Strictness::Strict,
            is_meta,
            &inference.expr_types,
            &inference.carrier_final,
            &inference.carrier_class_final,
        ));
        diags.extend(inference.diags);
        inferred_types = inference.expr_types;
    }
    let _ = inferred_types;

    // Honor luals' `---@diagnostic disable: undefined-field` for the
    // undefined-field read rule (LB0306, #90). Scoped to this one rule; see
    // `directive.rs` for why it does not reuse the linter's engine.
    if diags.iter().any(|d| d.code.to_string() == "LB0306") {
        let source = parse.syntax().text().to_string();
        let sup = directive::UndefinedFieldSuppression::scan(&source);
        if sup.any() {
            diags.retain(|d| {
                d.code.to_string() != "LB0306"
                    || !sup.suppresses(
                        d.primary_label()
                            .map_or(0, |l| directive::line_of(&source, l.span.range.start)),
                    )
            });
        }
    }

    diags.sort_by_key(|d| d.primary_label().map_or(0, |l| l.span.range.start));
    diags
}

#[cfg(test)]
mod tests {
    use luabox_diag::Severity;
    use luabox_syntax::lua::{Dialect, parse};

    use super::*;

    fn check(source: &str, strictness: Strictness) -> Vec<Diagnostic> {
        let parse = parse(source, Dialect::Lua54);
        assert_eq!(parse.errors(), &[], "fixture must parse cleanly");
        check_file(&parse, "test.lua", strictness)
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

    // --- rule a: call sites -------------------------------------------

    #[test]
    fn call_argument_type_mismatch() {
        let src = "\
---@param n number
local function double(n)
  return n * 2
end
double(\"nope\")
";
        assert_eq!(strict_codes(src), vec!["LB0300"]);
    }

    #[test]
    fn call_with_matching_literal_is_clean() {
        let src = "\
---@param n number
---@param s string
local function f(n, s) end
f(2, \"ok\")
";
        assert_eq!(strict_codes(src), Vec::<String>::new());
    }

    #[test]
    fn arity_too_few_and_too_many() {
        let src = "\
---@param a number
---@param b number
local function f(a, b) end
f(1)
f(1, 2, 3)
";
        assert_eq!(strict_codes(src), vec!["LB0301", "LB0301"]);
    }

    #[test]
    fn optional_params_and_varargs_relax_arity() {
        let src = "\
---@param a number
---@param b? number
local function f(a, b) end
---@param a number
---@param ... string
local function g(a, ...) end
f(1)
f(1, 2)
g(1)
g(1, \"x\", \"y\")
";
        assert_eq!(strict_codes(src), Vec::<String>::new());
    }

    #[test]
    fn vararg_arguments_are_typechecked() {
        let src = "\
---@param a number
---@param ... string
local function g(a, ...) end
g(1, \"x\", 2)
";
        assert_eq!(strict_codes(src), vec!["LB0300"]);
    }

    #[test]
    fn nil_satisfies_optional_param() {
        let src = "\
---@param a number
---@param b? string
local function f(a, b) end
f(1, nil)
";
        assert_eq!(strict_codes(src), Vec::<String>::new());
    }

    #[test]
    fn annotated_local_flows_into_call() {
        let src = "\
---@param n number
local function f(n) end
---@type string
local s = \"x\"
f(s)
";
        assert_eq!(strict_codes(src), vec!["LB0300"]);
    }

    #[test]
    fn annotated_call_result_flows_into_call() {
        let src = "\
---@return string
local function name() return \"x\" end
---@param n number
local function f(n) end
f(name())
";
        assert_eq!(strict_codes(src), vec!["LB0300"]);
    }

    #[test]
    fn function_reference_argument() {
        let src = "\
---@param cb fun(x: number)
local function on(cb) end
---@param x number
local function handler(x) end
on(handler)
on(3)
";
        assert_eq!(strict_codes(src), vec!["LB0300"]);
    }

    #[test]
    fn multi_return_expansion_fills_arity() {
        let src = "\
---@return number, string
local function pair() return 1, \"x\" end
---@param a number
---@param b string
local function f(a, b) end
f(pair())
";
        assert_eq!(strict_codes(src), Vec::<String>::new());
    }

    #[test]
    fn unknown_callee_is_never_checked() {
        assert_eq!(strict_codes("print(1, 2, 3)\n"), Vec::<String>::new());
    }

    #[test]
    fn dotted_function_names_resolve() {
        let src = "\
local M = {}
---@param n number
function M.double(n)
  return n * 2
end
M.double(\"no\")
";
        assert_eq!(strict_codes(src), vec!["LB0300"]);
    }

    // --- table literals against class shapes ----------------------------

    const POINT: &str = "\
---@class Point
---@field x number
---@field y number
---@field label? string

---@param p Point
local function use(p) end
";

    #[test]
    fn table_literal_missing_required_field() {
        let src = format!("{POINT}use({{ x = 1 }})\n");
        let diags = check(&src, Strictness::Strict);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(diags[0].code.to_string(), "LB0302");
        assert!(diags[0].message.contains("`y`"), "{}", diags[0].message);
    }

    #[test]
    fn table_literal_unknown_field() {
        let src = format!("{POINT}use({{ x = 1, y = 2, z = 3 }})\n");
        let diags = check(&src, Strictness::Strict);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(diags[0].code.to_string(), "LB0303");
        assert!(diags[0].message.contains("`z`"), "{}", diags[0].message);
    }

    #[test]
    fn table_literal_field_type_mismatch() {
        let src = format!("{POINT}use({{ x = 1, y = \"two\" }})\n");
        let diags = check(&src, Strictness::Strict);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(diags[0].code.to_string(), "LB0300");
    }

    #[test]
    fn table_literal_optional_field_may_be_absent() {
        let src = format!("{POINT}use({{ x = 1, y = 2 }})\n");
        assert_eq!(strict_codes(&src), Vec::<String>::new());
    }

    #[test]
    fn each_field_problem_is_its_own_diagnostic() {
        let src = format!("{POINT}use({{ y = \"two\", z = 3 }})\n");
        let mut found: Vec<String> = strict_codes(&src);
        found.sort();
        assert_eq!(found, vec!["LB0300", "LB0302", "LB0303"]);
    }

    #[test]
    fn inherited_fields_count() {
        let src = "\
---@class Base
---@field id number

---@class Derived: Base
---@field name string

---@param d Derived
local function f(d) end
f({ name = \"x\" })
f({ id = 1, name = \"x\" })
";
        assert_eq!(strict_codes(src), vec!["LB0302"]);
    }

    #[test]
    fn indexer_keeps_class_open() {
        let src = "\
---@class Bag
---@field size number
---@field [string] boolean

---@param b Bag
local function f(b) end
f({ size = 1, extra = true })
f({ size = 1, extra = 3 })
";
        assert_eq!(strict_codes(src), vec!["LB0300"]);
    }

    #[test]
    fn array_items_checked_against_array_part() {
        let src = "\
---@param xs string[]
local function f(xs) end
f({ \"a\", \"b\" })
f({ \"a\", 2 })
";
        assert_eq!(strict_codes(src), vec!["LB0300"]);
    }

    #[test]
    fn nested_table_literal_checked_field_by_field() {
        let src = "\
---@class Inner
---@field n number

---@class Outer
---@field inner Inner

---@param o Outer
local function f(o) end
f({ inner = { n = \"no\" } })
";
        assert_eq!(strict_codes(src), vec!["LB0300"]);
    }

    // --- rule b: annotated locals ----------------------------------------

    #[test]
    fn typed_local_initializer_checked() {
        let src = "\
---@type number
local n = \"nope\"
";
        assert_eq!(strict_codes(src), vec!["LB0300"]);
    }

    #[test]
    fn typed_local_assignment_checked() {
        let src = "\
---@type number
local n = 1
n = \"nope\"
";
        assert_eq!(strict_codes(src), vec!["LB0300"]);
    }

    #[test]
    fn untyped_local_assignment_unchecked() {
        let src = "\
local n = 1
n = \"fine\"
";
        assert_eq!(strict_codes(src), Vec::<String>::new());
    }

    #[test]
    fn typed_local_table_literal_field_level() {
        let src = "\
---@class Cfg
---@field port number

---@type Cfg
local cfg = { port = \"8080\" }
";
        assert_eq!(strict_codes(src), vec!["LB0300"]);
    }

    // --- rule c: returns ---------------------------------------------------

    #[test]
    fn return_type_mismatch() {
        let src = "\
---@return number
local function f()
  return \"nope\"
end
";
        assert_eq!(strict_codes(src), vec!["LB0304"]);
    }

    #[test]
    fn return_count_mismatch() {
        let src = "\
---@return number, string
local function f()
  return 1
end
---@return number
local function g()
  return 1, 2
end
";
        assert_eq!(strict_codes(src), vec!["LB0304", "LB0304"]);
    }

    #[test]
    fn nilable_missing_returns_are_fine() {
        let src = "\
---@return number, string?
local function f()
  return 1
end
";
        assert_eq!(strict_codes(src), Vec::<String>::new());
    }

    #[test]
    fn nested_unannotated_function_returns_unchecked() {
        let src = "\
---@return number
local function f()
  local g = function()
    return \"inner is fine\"
  end
  g()
  return 1
end
";
        assert_eq!(strict_codes(src), Vec::<String>::new());
    }

    #[test]
    fn annotated_param_flows_into_return_check() {
        let src = "\
---@param s string
---@return number
local function f(s)
  return s
end
";
        assert_eq!(strict_codes(src), vec!["LB0304"]);
    }

    // --- name-based `@param` binding (#74) --------------------------------

    #[test]
    fn partial_param_annotations_bind_by_name() {
        // Annotating only params 2..5 of a 6-param function must not shift
        // the tags onto the wrong positions (#74's exact repro).
        let src = "\
---@param b string
---@param c boolean
---@param d integer
---@param e string
local function f(a, b, c, d, e, g)
  return a, b, c, d, e, g
end
f(1, \"s\", true, 2, \"t\", 3)
";
        assert_eq!(strict_codes(src), Vec::<String>::new());
        // And a call violating the *named* params errors per slot.
        let bad = src.replace(
            "f(1, \"s\", true, 2, \"t\", 3)",
            "f(1, true, \"s\", 2.5, 2, 3)",
        );
        assert_eq!(
            strict_codes(&bad),
            vec!["LB0300", "LB0300", "LB0300", "LB0300"]
        );
    }

    #[test]
    fn out_of_order_param_tags_bind_by_name() {
        let src = "\
---@param b string
---@param a number
local function f(a, b) end
f(1, \"s\")
f(\"s\", 1)
";
        assert_eq!(strict_codes(src), vec!["LB0300", "LB0300"]);
    }

    #[test]
    fn vararg_param_tag_binds_regardless_of_position() {
        let src = "\
---@param ... string
---@param a number
local function f(a, ...) end
f(1, \"x\", \"y\")
f(1, \"x\", 2)
";
        assert_eq!(strict_codes(src), vec!["LB0300"]);
    }

    #[test]
    fn duplicate_param_tag_names_first_wins() {
        let src = "\
---@param a number
---@param a string
local function f(a) end
f(1)
f(\"x\")
";
        assert_eq!(strict_codes(src), vec!["LB0300"]);
    }

    #[test]
    fn param_tag_naming_no_parameter_is_unbound() {
        // TODO(P2): LuaLS warns here; today the tag is silently unbound and
        // the parameter stays permissive `unknown`.
        let src = "\
---@param sied number
local function f(side)
  return side
end
f(1)
f(\"anything\")
";
        assert_eq!(strict_codes(src), Vec::<String>::new());
    }

    // --- constructor returns through `setmetatable` (#73) ------------------

    /// Strict check with the Lua 5.4 stdlib ambient layer (its
    /// `setmetatable` signature must not mask inference).
    fn strict_codes_ambient(src: &str) -> Vec<String> {
        let parse = lua::parse(src, lua::Dialect::Lua54);
        assert_eq!(parse.errors(), &[], "fixture must parse cleanly");
        check_file_shaped(
            &parse,
            "test.lua",
            Strictness::Strict,
            None,
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

    // --- enums, aliases, LB0305 -----------------------------------------

    #[test]
    fn enum_member_satisfies_enum_param() {
        let src = "\
---@enum Color
local Color = {
  red = 1,
  green = 2,
}
---@param c Color
local function paint(c) end
paint(Color.red)
paint(2)
paint(9)
";
        assert_eq!(strict_codes(src), vec!["LB0300"]);
    }

    #[test]
    fn alias_literal_union() {
        let src = "\
---@alias Mode \"fast\"|\"slow\"
---@param m Mode
local function run(m) end
run(\"fast\")
run(\"medium\")
";
        assert_eq!(strict_codes(src), vec!["LB0300"]);
    }

    #[test]
    fn unknown_type_name_reported() {
        let src = "\
---@param x Wibble
local function f(x) end
";
        assert_eq!(strict_codes(src), vec!["LB0305"]);
    }

    #[test]
    fn generic_params_do_not_report_lb0305() {
        let src = "\
---@generic T
---@param x T
---@return T
local function id(x)
  return x
end
id(1)
";
        assert_eq!(strict_codes(src), Vec::<String>::new());
    }

    // --- strictness ladder -----------------------------------------------

    #[test]
    fn warn_mode_downgrades_severity() {
        let src = "\
---@param n number
local function f(n) end
f(\"no\")
";
        let warn = check(src, Strictness::Warn);
        assert_eq!(warn.len(), 1);
        assert_eq!(warn[0].severity, Severity::Warning);
        let strict = check(src, Strictness::Strict);
        assert_eq!(strict[0].severity, Severity::Error);
    }

    #[test]
    fn unknown_argument_strict_vs_warn() {
        let src = "\
---@param n number
local function f(n) end
local x = tonumber(\"3\")
f(x)
";
        // Warn: `unknown` flows freely. Strict: unknown -> number errors.
        assert_eq!(codes(src, Strictness::Warn), Vec::<String>::new());
        assert_eq!(codes(src, Strictness::Strict), vec!["LB0300"]);
    }

    #[test]
    fn none_suppresses_everything() {
        let src = "\
---@param n number
local function f(n) end
f(\"no\")
";
        assert_eq!(codes(src, Strictness::None), Vec::<String>::new());
    }

    #[test]
    fn strictness_from_manifest_flag() {
        assert_eq!(Strictness::from_manifest_flag(true), Strictness::Strict);
        assert_eq!(Strictness::from_manifest_flag(false), Strictness::Warn);
    }

    // --- end to end ---------------------------------------------------------

    #[test]
    fn clean_annotated_file_has_no_diagnostics() {
        let src = "\
---@class Greeter
---@field name string
---@field excited? boolean

---@param g Greeter
---@return string
local function greet(g)
  return g.name
end

---@type Greeter
local g = { name = \"lua\" }
greet(g)
greet({ name = \"world\", excited = true })
";
        assert_eq!(strict_codes(src), Vec::<String>::new());
    }

    #[test]
    fn diagnostics_carry_file_and_span() {
        let src = "\
---@param n number
local function f(n) end
f(\"no\")
";
        let diags = check(src, Strictness::Strict);
        let label = diags[0].primary_label().expect("primary label");
        assert_eq!(label.span.file, "test.lua");
        assert_eq!(&src[label.span.range.clone()], "\"no\"");
    }
}

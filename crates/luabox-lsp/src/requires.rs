//! The **one** place a `require("mod")` is turned into the module's export
//! type, shared by every surface that needs it.
//!
//! Two resolvers used to answer "what is `local m = require(\"mod\")`?" and
//! they did not agree (#54): the diagnostics pipeline threaded the module's
//! real export type into `check_file_with_requires`, while hover and
//! completion read the file's LuaCATS annotations only — so the *same*
//! binding typed correctly in the problems pane and hovered `unknown`. The
//! fix is not a second lookup in the hover provider; it is this module, which
//! both callers go through.
//!
//! The map is built exactly the way the type pass builds it, because it *is*
//! the map the type pass uses:
//!
//! - **project modules** come from the database ([`Analysis::require_exports`],
//!   the check-mode `module_surface_checked` surface, #85);
//! - **rock modules** come from the vendored-tree harvest's
//!   [`RockSurfaces::by_module`] map (#30), merged with `or_insert` so a
//!   project file that shadows a rock module keeps the database's answer —
//!   the same precedence path-keyed resolution gives `luabox check`.
//!
//! [`require_module_of`] is the other half: which *binding* a module string
//! belongs to. It is deliberately narrow — the initialiser must be exactly a
//! static `require("...")` call in the matching position of a `local`
//! statement. Everything looser stays `unknown`, which is the honest answer
//! rather than a guess:
//!
//! - `local m = require(name)` — a dynamic require has no statically known
//!   module, and the type pass does not resolve one either;
//! - `local m = require("a") or require("b")` — the initialiser is a binary
//!   expression, not a require; which branch runs is a runtime fact, and
//!   naming either one would be a coin flip presented as a type;
//! - `local m = require("mod").sub` — the binding is the *field*, not the
//!   module.
//!
//! Shadowing needs no special case: the lookup is keyed on the binding's own
//! declaration range, so `local m = require("a")` followed by
//! `local m = require("b")` gives each `m` its own module.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use luabox_db::Analysis;
use luabox_hir::Binding;
use luabox_syntax::lua::SyntaxKind;
use luabox_syntax::lua::ast::{self, AstNode};
use luabox_syntax::luacats::{TypeExpr, TypeExprKind};
use luabox_types::RockSurfaces;
use luabox_types::ty::{FieldTy, Ty};

use crate::merged_ambient::MergedAmbient;
use crate::sema::FileSema;

/// Module string → export type for one file's static `require`s: the project
/// database's answers, with the rock harvest's module-keyed answers merged
/// beneath them.
#[derive(Debug, Default)]
pub struct RequireExports {
    by_module: HashMap<String, Ty>,
}

impl RequireExports {
    /// Resolve every static `require` in `path` against the project database
    /// and the harvested rock tree.
    ///
    /// A file the analysis does not know resolves to an empty set rather than
    /// failing: a caller with no exports and a caller with a missing file want
    /// the same behaviour (nothing resolves), and every consumer here is a
    /// best-effort editor surface.
    #[must_use]
    pub fn resolve(analysis: &Analysis, path: &Path, rocks: &RockSurfaces) -> Self {
        let mut by_module = analysis.require_exports(path).unwrap_or_default();
        // A `require` the database cannot resolve may still name a module of
        // the vendored rock tree (#30): the db only holds project files, so
        // rock exports are matched by module *name* here. `or_insert` keeps
        // the db's answer where it has one, so a project file shadowing a rock
        // module still wins — the same precedence `luabox check` gets from
        // path-keyed resolution.
        if !rocks.by_module().is_empty()
            && let Some(lowered) = analysis.lower(path)
        {
            for edge in lowered.file().requires() {
                if let Some(ty) = rocks.by_module().get(&edge.module) {
                    by_module
                        .entry(edge.module.clone())
                        .or_insert_with(|| ty.clone());
                }
            }
        }
        Self { by_module }
    }

    /// The map to thread into [`luabox_types::check_file_with_requires`].
    #[must_use]
    pub fn by_module(&self) -> &HashMap<String, Ty> {
        &self.by_module
    }

    /// The export type of `module`, if it resolved. `None` for a module the
    /// project does not have and no rock provides — `unknown` to the type
    /// pass, and `unknown` to every surface that reads this.
    #[must_use]
    pub fn get(&self, module: &str) -> Option<&Ty> {
        self.by_module.get(module)
    }

    /// The export type of the module `binding` is initialised from — the type
    /// hover and completion must show for a `require` binding, because it is
    /// the type the problems pane already checks that binding against.
    ///
    /// `None` when the binding is not a plain `local x = require("...")`, or
    /// when the module resolved to nothing. Both are `unknown` to the type
    /// pass too, so both stay `unknown` here.
    #[must_use]
    pub fn binding_export(&self, sema: &FileSema, binding: &Binding) -> Option<&Ty> {
        self.get(require_module_of(sema, binding)?)
    }
}

/// The module string of the static `require` that initialises `binding`, when
/// the binding is a `local` name whose matching initialiser is exactly that
/// call. See the module docs for what is deliberately excluded.
#[must_use]
pub fn require_module_of<'a>(sema: &'a FileSema, binding: &Binding) -> Option<&'a str> {
    let declares = |stmt: &ast::LocalStmt| {
        stmt.names()
            .position(|n| n.name().is_some_and(|t| t.text_range() == binding.range))
    };
    let (local, index) = sema
        .root
        .descendants()
        .filter(|node| node.kind() == SyntaxKind::LOCAL_STMT)
        .filter_map(ast::LocalStmt::cast)
        .find_map(|stmt| declares(&stmt).map(|index| (stmt, index)))?;
    // Positional: `local a, b = require("x"), require("y")` gives each name
    // its own module, and a name past the end of the value list has none.
    let value = local.values()?.exprs().nth(index)?;
    let range = value.syntax().text_range();
    // Identity, not containment: the initialiser must *be* the require call.
    // `require("a") or require("b")` and `require("mod").sub` both contain one
    // and are both something else.
    sema.requires()
        .iter()
        .find(|edge| edge.range == range)
        .map(|edge| edge.module.as_str())
}

/// The named fields of a module export, when the export is a structural table
/// — the shape of the overwhelmingly common `local M = {} … return M` module.
///
/// A `---@class` module export is the other shape, and it is [`Ty::Named`]
/// in **both** of its spellings (#56): an *instance* export (`---@type
/// Point` on the returned local) always was, and a *carrier* export
/// (`---@class Point` over `local P = {}`) now crosses the `require`
/// boundary as the class it carries — the workspace-global identity, which
/// is what luals resolves the require to. This function declines those;
/// [`export_class`] is their half, and members resolve through the merged
/// ambient environment ([`luabox_types::Ambient::class_members`]) — the
/// same surface `luabox check` enforces, in the editor and in CI alike.
#[must_use]
pub fn export_fields(ty: &Ty) -> Option<&BTreeMap<String, FieldTy>> {
    match ty {
        Ty::Table(table) => Some(&table.fields),
        _ => None,
    }
}

/// The `require` binding's module's structural table export
/// ([`export_fields`]) — but **only** when [`receiver_type`] does not
/// *resolve* to a class shape for the same binding (R7, N16).
///
/// `receiver_type` already gives an explicit `---@type`/`---@param`
/// annotation priority over anything inferred from a `require`, and gives a
/// `require`d module that itself carries a `---@class` priority over the
/// table shape too — but that precedence lived only inside `receiver_type`
/// itself. The structural-export lookup used to run as an
/// unconditional first check ahead of it, on every member surface, so an
/// explicit annotation naming a real class lost to the plain table the
/// `require`d module happened to return: `---@type Point` over `local m =
/// require("m")`, with `m.lua` a bare `local M = {} … return M`, hovered
/// `(field) m.x: 42` off the table instead of `(field) Point.x: string` off
/// the annotation. This is the single gate both hover and completion now
/// check before reading the module's table shape, so a binding with a class
/// reference of its own — whether written or carried — never falls back to
/// it, on either surface, again.
///
/// The gate checks `ambient.class_members_of(...)`, not merely
/// `receiver_type(...).is_some()` (N16): a *presence* check suppresses the
/// structural route for **any** `---@type`, including one whose name
/// resolves to nothing — `---@type table`, `---@type Bogus`, or a mid-edit
/// partially-typed class name, none of which `ambient` can ever answer a
/// member for. Gating on presence alone left those cases dead on both
/// surfaces: the class arm below (`ambient_member_items`/`member_hover`'s
/// second branch) also fails to resolve, and nothing catches it — hover
/// `None`, completion `[]`, even though `luabox check` reports no
/// diagnostics for the same file. A *resolves* check falls back to the
/// table shape exactly when the annotation cannot answer for member access,
/// matching what the checker itself would do.
#[must_use]
pub fn require_struct_fields<'s, 'a>(
    sema: &'s FileSema,
    exports: &'a RequireExports,
    ambient: &MergedAmbient,
    binding: &Binding,
) -> Option<(&'s str, &'a BTreeMap<String, FieldTy>)> {
    if let Some(ty) = receiver_type(sema, exports, binding)
        && ambient.class_members_of(&ty).is_some()
    {
        return None;
    }
    let module = require_module_of(sema, binding)?;
    let fields = exports.get(module).and_then(export_fields)?;
    Some((module, fields))
}

/// The class a module export names, when the export crossed the boundary as
/// a class (#56) — both `---@class` spellings do: the carrier exports as
/// the class it carries, the instance as its `---@type`. The counterpart of
/// [`export_fields`]; members come from
/// [`luabox_types::Ambient::class_members`], never from the export type
/// itself.
#[must_use]
pub fn export_class(ty: &Ty) -> Option<&str> {
    match ty {
        Ty::Named(name) => Some(name),
        _ => None,
    }
}

/// The receiver `binding`'s class **reference** (#48, #56): its own
/// `---@type` annotation, used exactly as written — arguments included, so
/// `Box<number>` stays `Box<number>`, not just `Box` — or, when the binding
/// has no such annotation, a bare reference to the class the `require`
/// module its initialiser resolves to carries (both `---@class` spellings,
/// #56: [`export_class`]). A carrier export never carries explicit type
/// arguments (`reify_export` erases what it cannot bind, #42), so that half
/// is always argument-less.
///
/// This is the **one** receiver→class-reference rule, called by hover,
/// completion, signature help and goto-definition, so a surface cannot
/// silently miss it the way signature help did (#50), hover/completion each
/// carried their own copy of it (#56), and — the shape this closes — so a
/// generic reference cannot resolve correctly on one surface and leniently
/// on another (#48). The reference returned is a *candidate*: the caller
/// still confirms it against
/// [`luabox_types::Ambient::class_members_of`]/[`crate::merged_ambient::MergedAmbient::class_members_of`]
/// (an annotation can name anything, including a type that resolves to
/// nothing).
#[must_use]
pub fn receiver_type(
    sema: &FileSema,
    exports: &RequireExports,
    binding: &Binding,
) -> Option<TypeExpr> {
    if let Some(ty) = sema.annotated_type(binding) {
        return Some(ty);
    }
    let name = require_module_of(sema, binding)
        .and_then(|module| exports.get(module))
        .and_then(export_class)?;
    Some(bare_named(name))
}

/// A synthetic, argument-less `Named` reference to `name` — the shape a
/// `require`-carried class always is (#42), and the shape
/// [`luabox_types::Ambient::class_members_of`] treats identically to a bare
/// `---@type Name` annotation (#48's "unbound stays lenient" rule). The span
/// is a placeholder: nothing downstream of [`receiver_type`] reads it (no
/// diagnostic is raised against this expression; it never appears in a
/// source file).
fn bare_named(name: &str) -> TypeExpr {
    TypeExpr {
        kind: TypeExprKind::Named {
            name: name.to_string(),
            args: Vec::new(),
        },
        span: luabox_syntax::luacats::Span { start: 0, end: 0 },
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code — panics document assumptions"
)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use luabox_db::{AnalysisHost, Change, Dialect, Strictness};

    fn root() -> PathBuf {
        PathBuf::from(if cfg!(windows) { r"C:\ws" } else { "/ws" })
    }

    /// Analyse `files` (the first is the file under test) with the workspace
    /// root set, so `require` resolution finds the siblings.
    fn analyze(files: &[(&str, &str)]) -> (Analysis, PathBuf) {
        let mut host = AnalysisHost::new(Dialect::Lua54, Strictness::Warn);
        host.set_root(root());
        let mut first = None;
        for (rel, text) in files {
            let path = root().join(rel);
            first.get_or_insert_with(|| path.clone());
            host.apply_change(Change::SetFileText {
                path,
                dialect: Dialect::Lua54,
                text: (*text).to_string(),
            });
        }
        (host.snapshot(), first.expect("at least one file"))
    }

    /// The module the binding named `name` in the first file is required from.
    fn module_of(files: &[(&str, &str)], name: &str) -> Option<String> {
        let (analysis, path) = analyze(files);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        let lowered = analysis.lower(&path).expect("lowered");
        let (_, binding) = lowered.file().bindings().find(|(_, b)| b.name == name)?;
        require_module_of(&sema, binding).map(ToString::to_string)
    }

    #[test]
    fn a_plain_local_require_names_its_module() {
        let files = [("main.lua", "local m = require(\"other\")\nreturn m\n")];
        assert_eq!(module_of(&files, "m").as_deref(), Some("other"));
    }

    #[test]
    fn a_multi_name_local_matches_requires_positionally() {
        let src = "local a, b = require(\"x\"), require(\"y\")\nreturn a, b\n";
        let files = [("main.lua", src)];
        assert_eq!(module_of(&files, "a").as_deref(), Some("x"));
        assert_eq!(module_of(&files, "b").as_deref(), Some("y"));
    }

    #[test]
    fn a_name_past_the_end_of_the_value_list_has_no_module() {
        let files = [("main.lua", "local a, b = require(\"x\")\nreturn a, b\n")];
        assert_eq!(module_of(&files, "b"), None);
    }

    /// `require("a") or require("b")` is a binary expression, not a require:
    /// which branch runs is a runtime fact (see the module docs).
    #[test]
    fn an_or_chained_require_names_no_module() {
        let src = "local m = require(\"a\") or require(\"b\")\nreturn m\n";
        assert_eq!(module_of(&[("main.lua", src)], "m"), None);
    }

    #[test]
    fn a_field_of_a_require_is_not_the_module_binding() {
        let src = "local helper = require(\"other\").helper\nreturn helper\n";
        assert_eq!(module_of(&[("main.lua", src)], "helper"), None);
    }

    #[test]
    fn a_dynamic_require_names_no_module() {
        let src = "local name = \"other\"\nlocal m = require(name)\nreturn m\n";
        assert_eq!(module_of(&[("main.lua", src)], "m"), None);
    }

    #[test]
    fn a_local_with_no_initialiser_names_no_module() {
        assert_eq!(module_of(&[("main.lua", "local m\nreturn m\n")], "m"), None);
    }

    #[test]
    fn a_shadowing_require_binding_keeps_its_own_module() {
        // Two bindings both named `m`; the lookup is keyed on the declaration
        // range, so each answers with the module it was declared from.
        let src = "\
local m = require(\"first\")
print(m)
local m = require(\"second\")
return m
";
        let (analysis, path) = analyze(&[("main.lua", src)]);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        let lowered = analysis.lower(&path).expect("lowered");
        let modules: Vec<String> = lowered
            .file()
            .bindings()
            .filter(|(_, b)| b.name == "m")
            .filter_map(|(_, b)| require_module_of(&sema, b))
            .map(ToString::to_string)
            .collect();
        assert_eq!(modules, vec!["first", "second"]);
    }

    /// `binding_export` is the two halves joined: the binding's module, then
    /// that module's export type out of the shared map.
    #[test]
    fn a_require_binding_exports_the_modules_type() {
        let files = [
            ("main.lua", "local m = require(\"other\")\nreturn m\n"),
            (
                "other.lua",
                "local M = {}\n---@return string\nfunction M.helper() return \"s\" end\nreturn M\n",
            ),
        ];
        let (analysis, path) = analyze(&files);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        let exports = RequireExports::resolve(&analysis, &path, &RockSurfaces::default());
        let lowered = analysis.lower(&path).expect("lowered");
        let (_, binding) = lowered
            .file()
            .bindings()
            .find(|(_, b)| b.name == "m")
            .expect("the binding");
        let ty = exports.binding_export(&sema, binding).expect("its export");
        let fields = export_fields(ty).expect("a structural table export");
        assert!(fields.contains_key("helper"), "{ty}");
    }

    #[test]
    fn export_fields_declines_a_non_table_export() {
        assert!(export_fields(&Ty::Number).is_none());
        assert!(export_fields(&Ty::Named("Point".to_string())).is_none());
    }

    #[test]
    fn a_project_module_resolves_through_the_database() {
        let files = [
            ("main.lua", "local m = require(\"other\")\nreturn m\n"),
            (
                "other.lua",
                "local M = {}\n---@return string\nfunction M.helper() return \"s\" end\nreturn M\n",
            ),
        ];
        let (analysis, path) = analyze(&files);
        let exports = RequireExports::resolve(&analysis, &path, &RockSurfaces::default());
        let ty = exports
            .by_module()
            .get("other")
            .expect("the module's export type");
        assert!(ty.to_string().contains("helper"), "{ty}");
    }

    #[test]
    fn a_module_the_project_does_not_have_resolves_to_nothing() {
        let files = [("main.lua", "local m = require(\"absent\")\nreturn m\n")];
        let (analysis, path) = analyze(&files);
        let exports = RequireExports::resolve(&analysis, &path, &RockSurfaces::default());
        assert!(exports.by_module().is_empty());
    }

    #[test]
    fn a_file_the_analysis_does_not_know_resolves_to_nothing() {
        let (analysis, _) = analyze(&[("main.lua", "return 1\n")]);
        let exports = RequireExports::resolve(
            &analysis,
            &root().join("absent.lua"),
            &RockSurfaces::default(),
        );
        assert!(exports.by_module().is_empty());
    }

    // === `receiver_type` (#48, #56) ========================================

    /// The bare class name `receiver_type` resolves for `name` — the
    /// argument-discarding view every display/locate call site reads via
    /// `sema::named_of(&ty)`.
    fn class_of(files: &[(&str, &str)], name: &str) -> Option<String> {
        crate::sema::named_of(&type_of(files, name)?)
    }

    /// The full class reference `receiver_type` resolves for `name`,
    /// arguments included — what a member-lookup call site hands to
    /// `Ambient::class_members_of` (#48).
    fn type_of(files: &[(&str, &str)], name: &str) -> Option<TypeExpr> {
        let (analysis, path) = analyze(files);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        let exports = RequireExports::resolve(&analysis, &path, &RockSurfaces::default());
        let lowered = analysis.lower(&path).expect("lowered");
        let (_, binding) = lowered.file().bindings().find(|(_, b)| b.name == name)?;
        receiver_type(&sema, &exports, binding)
    }

    #[test]
    fn an_explicit_annotation_names_the_class_directly() {
        let src = "\
---@class Point
---@field x number

---@type Point
local p = nil
";
        assert_eq!(
            class_of(&[("main.lua", src)], "p").as_deref(),
            Some("Point")
        );
    }

    /// The candidate is returned even when the class is declared in another
    /// file — resolving it is the caller's job, against the merged ambient
    /// (#46/#47's defect was skipping that lookup, not this one).
    #[test]
    fn an_annotation_naming_a_cross_file_class_still_names_it() {
        let files = [
            ("main.lua", "---@type Point\nlocal p = nil\nprint(p)\n"),
            ("point.lua", "---@class Point\n---@field x number\n"),
        ];
        assert_eq!(class_of(&files, "p").as_deref(), Some("Point"));
    }

    #[test]
    fn a_require_binding_falls_back_to_the_required_modules_carrier_class() {
        let files = [
            ("main.lua", "local p = require(\"point\")\nprint(p)\n"),
            (
                "point.lua",
                "---@class Point\n---@field x number\nlocal P = {}\nreturn P\n",
            ),
        ];
        assert_eq!(class_of(&files, "p").as_deref(), Some("Point"));
    }

    #[test]
    fn a_plain_table_binding_names_no_class() {
        let files = [("main.lua", "local t = {}\nprint(t)\n")];
        assert_eq!(class_of(&files, "t"), None);
    }

    /// #48: an explicit `Box<number>` annotation is returned whole —
    /// arguments included — not narrowed to the bare name `class_of` (and
    /// the pre-#48 `class_of_receiver`) reads off it.
    #[test]
    fn an_explicit_annotation_keeps_its_type_arguments() {
        let files = [(
            "main.lua",
            "---@class Box<T>\n---@field item T\n\n---@type Box<number>\nlocal b = nil\n",
        )];
        let ty = type_of(&files, "b").expect("a reference");
        assert_eq!(crate::sema::render_type(&ty), "Box<number>");
    }

    /// A `require`-carried class is always argument-less (#42's erasure) —
    /// `receiver_type`'s synthesised reference must not invent arguments a
    /// real annotation never wrote. (A *generic* carrier's export erases to
    /// a structural table rather than `Ty::Named` at all, #69 — a separate,
    /// pre-existing gap, not this test's concern.)
    #[test]
    fn a_required_carrier_class_reference_has_no_arguments() {
        let files = [
            ("main.lua", "local b = require(\"box\")\nprint(b)\n"),
            (
                "box.lua",
                "---@class Box\n---@field item number\nlocal B = {}\nreturn B\n",
            ),
        ];
        let ty = type_of(&files, "b").expect("a reference");
        assert_eq!(crate::sema::render_type(&ty), "Box");
    }
}

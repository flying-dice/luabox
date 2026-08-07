//! The merged workspace ambient layer every editor surface must read (#56):
//! [`luabox_types::Ambient`] (the dialect stdlib plus `[types] defs`) with
//! every project file's workspace-global classes/enums/aliases folded in,
//! plus the harvested rock tree's surfaces — the same environment
//! `luabox check` enforces.
//!
//! A bare `Ambient` is the *unmerged* defs-only layer: it type-checks
//! identically to a merged one (`with_project_types`/`with_rock_types` both
//! return plain `Ambient`) and silently drops every cross-file class when
//! passed to a surface that expects the merged view — precisely the mistake
//! `server.rs` made once, passing `&self.ambient` straight through before the
//! merge existed. [`MergedAmbient`] makes the two types distinct so that
//! mistake is a compile error the second time, not a doc comment (the
//! workspace already uses this "consuming builder returns a distinguished
//! type" shape for `RockSurfaces`/`FileTypes`/`Strictness`).
//!
//! It also memoises the one thing the server's per-revision cache does not:
//! [`Ambient::class_members`]'s derivation. The revision-keyed `Rc` in
//! `server.rs` reuses the *layer* across requests at the same revision, but
//! `class_members` re-walks the receiver's ancestor chain and re-clones its
//! fields on every call regardless — so a `.`-triggered completion and the
//! hover right after it each pay the walk again for the same class at the
//! same revision. [`MergedAmbient::class_members`] memoises per class name
//! for this instance's lifetime, which is exactly the revision's lifetime,
//! since one `MergedAmbient` is built per revision and shared behind the
//! server's `Rc`.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;

use luabox_syntax::luacats::{TypeExpr, TypeExprKind};
use luabox_types::ty::TableTy;
use luabox_types::{Ambient, FileTypes};

use crate::sema::{self, FileSemaCache};

/// The merged ambient layer, plus a per-instance memo of resolved class
/// shapes, keyed on the *rendered* reference (`sema::render_type`) rather
/// than a bare class name: `Ambient::class_members_of` (#48) resolves a
/// bound generic reference (`Box<number>`) to a different shape than the
/// same class referenced bare or bound to something else (`Box<string>`),
/// so the cache key has to carry the arguments — `render_type`'s output for
/// a `Named` reference is exactly `name` with no arguments and
/// `name<arg1, arg2, ...>` with them, so an unbound lookup and a bound one
/// never collide, and two bound lookups collide only when they are the same
/// reference.
pub struct MergedAmbient {
    ambient: Ambient,
    shapes: RefCell<HashMap<String, Option<Rc<TableTy>>>>,
    /// The absolute paths of every project-local `[types] defs` file this
    /// workspace's `base` layer was built from (N18) — the *genuine* ambient
    /// scope, as opposed to any file merely named `*.d.lua`. Empty unless a
    /// caller opts in via [`Self::with_ambient_paths`]; `server.rs` is the
    /// one production caller, computed alongside `base` itself so the two
    /// can never disagree about what counts as ambient.
    ambient_paths: HashSet<PathBuf>,
    /// [`crate::sema::locate_field`]'s `FileSema` cache (N20, round 4 review
    /// R10 half-fixed), living here rather than per-call: one `MergedAmbient`
    /// is built per [`luabox_db::Analysis::revision`] and shared behind the
    /// server's `Rc` (this type's own doc, above) — exactly the lifetime a
    /// `locate_field` cache needs to survive from one hover to the
    /// goto-definition right after it, instead of rebuilding every workspace
    /// file's `FileSema` from scratch on each call.
    sema_cache: FileSemaCache,
}

impl MergedAmbient {
    /// Build the merged layer the way every LSP surface needs it: `base`
    /// (the dialect stdlib + `[types] defs`) plus every project file's
    /// workspace-global contribution, plus the harvested rock tree — the
    /// exact sequence `luabox check` merges in `check_cmd`/`query.rs`.
    #[must_use]
    pub fn build<'a>(
        base: &Ambient,
        project_types: &[FileTypes],
        rocks: impl IntoIterator<Item = &'a FileTypes>,
    ) -> Self {
        let ambient = base
            .with_project_types(project_types)
            .with_rock_types(rocks);
        MergedAmbient {
            ambient,
            shapes: RefCell::new(HashMap::new()),
            ambient_paths: HashSet::new(),
            sema_cache: FileSemaCache::default(),
        }
    }

    /// Attach the set of paths that are genuinely part of `base`'s ambient
    /// scope (a project-local `[types] defs` entry) — [`Self::ambient_paths`]
    /// answers from this set. A plain [`Self::build`] leaves it empty, which
    /// is the correct answer for every test/caller that never configured
    /// `[types] defs` in the first place.
    #[must_use]
    pub fn with_ambient_paths(mut self, paths: HashSet<PathBuf>) -> Self {
        self.ambient_paths = paths;
        self
    }

    /// The workspace's genuinely-configured ambient `[types] defs` file
    /// paths (N18) — the only files [`crate::sema::locate_field`]'s search
    /// order may give elevated precedence over ordinary load order, because
    /// it is the only case `luabox_types::env::TypeEnv::merge_file_types`
    /// itself gives one (via the separately-seeded `Ambient` this whole
    /// layer is built from). A `*.d.lua`-suffixed file that is not in this
    /// set is, to the merge, an ordinary project file like any other.
    #[must_use]
    pub fn ambient_paths(&self) -> &HashSet<PathBuf> {
        &self.ambient_paths
    }

    /// [`crate::sema::locate_field`]'s shared `FileSema` cache for this
    /// revision (N20) — see the field doc for why living here closes the
    /// cross-call gap the per-call-local cache left open.
    #[must_use]
    pub fn sema_cache(&self) -> &FileSemaCache {
        &self.sema_cache
    }

    /// The raw merged layer, for callers that must hand it to a
    /// `luabox_types` function taking `&Ambient` directly
    /// ([`luabox_types::check_file_with_requires`],
    /// [`crate::requires::RequireExports::resolve`], ...).
    #[must_use]
    pub fn get(&self) -> &Ambient {
        &self.ambient
    }

    /// [`Ambient::class_members`], memoised per class name for this
    /// instance's lifetime: the first lookup of a class walks its ancestor
    /// chain and clones its shape; every later lookup at the same revision —
    /// from any surface — reuses the `Rc`.
    #[must_use]
    pub fn class_members(&self, name: &str) -> Option<Rc<TableTy>> {
        if let Some(cached) = self.shapes.borrow().get(name) {
            return cached.clone();
        }
        let shape = self.ambient.class_members(name).map(Rc::new);
        self.shapes
            .borrow_mut()
            .insert(name.to_string(), shape.clone());
        shape
    }

    /// [`Ambient::class_members_of`], memoised (#48): a receiver's
    /// annotated/inferred type reference (`Box`, `Box<number>`,
    /// `Box<Pair<number>>`, ...), monomorphised exactly as the checker
    /// monomorphises the same reference at its use site. `None` for
    /// anything the underlying call declines — a compound type expression
    /// (`T?`, a union, an array) or a `Named` reference to something that is
    /// not a class.
    ///
    /// An unbound reference (`Box`, no `<...>`) shares the exact cache
    /// entry [`Self::class_members`] would build for the bare name — same
    /// key, same result, one derivation regardless of which of the two a
    /// caller reaches it through.
    #[must_use]
    pub fn class_members_of(&self, ty: &TypeExpr) -> Option<Rc<TableTy>> {
        let TypeExprKind::Named { name, args } = &ty.kind else {
            return None;
        };
        if args.is_empty() {
            return self.class_members(name);
        }
        let key = sema::render_type(ty);
        if let Some(cached) = self.shapes.borrow().get(&key) {
            return cached.clone();
        }
        let shape = self.ambient.class_members_of(ty).map(Rc::new);
        self.shapes.borrow_mut().insert(key, shape.clone());
        shape
    }

    /// Whether `name`'s ancestry was too deep for the most recent
    /// [`Self::class_members`]/[`Self::class_members_of`] call to resolve
    /// fully (`LB0317`) — [`Ambient::class_ancestry_truncated`], through the
    /// merged layer, so a caller checks the exact same env whichever of the
    /// two entry points it read the shape through.
    #[must_use]
    pub fn class_ancestry_truncated(&self, name: &str) -> bool {
        self.ambient.class_ancestry_truncated(name)
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
    use luabox_types::build_ambient;

    /// Parse `source` as the type of a `---@type` tag.
    fn type_expr(source: &str) -> TypeExpr {
        use luabox_syntax::luacats::{Tag, parse_block};
        let block = parse_block(&format!("---@type {source}"), 0);
        block
            .tags
            .iter()
            .find_map(|tag| match tag {
                Tag::Type(t) => t.types.first().cloned(),
                _ => None,
            })
            .expect("a @type tag with one type")
    }

    fn merged(src: &str) -> MergedAmbient {
        let root = PathBuf::from(if cfg!(windows) { r"C:\ws" } else { "/ws" });
        let mut host = AnalysisHost::new(Dialect::Lua54, Strictness::Warn);
        host.set_root(root.clone());
        host.apply_change(Change::SetFileText {
            path: root.join("main.lua"),
            dialect: Dialect::Lua54,
            text: src.to_string(),
        });
        let analysis = host.snapshot();
        let base = build_ambient(Dialect::Lua54, &[]);
        MergedAmbient::build(&base, &analysis.project_types(), &[])
    }

    #[test]
    fn class_members_resolves_a_declared_class() {
        let ambient = merged("---@class Point\n---@field x number\n");
        let shape = ambient.class_members("Point").expect("Point resolves");
        assert!(shape.fields.contains_key("x"));
    }

    #[test]
    fn class_members_declines_an_undeclared_name() {
        let ambient = merged("---@class Point\n---@field x number\n");
        assert!(ambient.class_members("Nope").is_none());
    }

    /// The whole point of the memo: the second lookup of the same class
    /// reuses the first's `Rc` rather than rederiving the shape.
    #[test]
    fn class_members_memoises_the_same_class_across_calls() {
        let ambient = merged("---@class Point\n---@field x number\n");
        let first = ambient.class_members("Point").expect("Point resolves");
        let second = ambient.class_members("Point").expect("Point resolves");
        assert!(Rc::ptr_eq(&first, &second), "expected the memoised Rc");
    }

    /// A `None` answer is memoised too — a class that does not exist should
    /// not be re-walked on every completion request either.
    #[test]
    fn class_members_memoises_a_negative_answer() {
        let ambient = merged("---@class Point\n---@field x number\n");
        assert!(ambient.class_members("Nope").is_none());
        assert!(ambient.class_members("Nope").is_none());
        assert_eq!(ambient.shapes.borrow().len(), 1);
    }

    // === class_members_of (#48) ============================================

    fn box_ambient() -> MergedAmbient {
        merged("---@class Box<T>\n---@field item T\n")
    }

    #[test]
    fn class_members_of_resolves_a_bound_reference() {
        let ambient = box_ambient();
        let shape = ambient
            .class_members_of(&type_expr("Box<number>"))
            .expect("Box<number> resolves");
        assert_eq!(shape.fields["item"].ty.to_string(), "number");
    }

    #[test]
    fn class_members_of_an_unbound_reference_stays_lenient() {
        let ambient = box_ambient();
        let shape = ambient
            .class_members_of(&type_expr("Box"))
            .expect("Box resolves");
        assert_eq!(shape.fields["item"].ty.to_string(), "T");
    }

    #[test]
    fn class_members_of_declines_a_compound_type_expression() {
        let ambient = box_ambient();
        for src in ["Box?", "Box|nil", "Box[]"] {
            assert!(ambient.class_members_of(&type_expr(src)).is_none(), "{src}");
        }
    }

    /// The unbound case shares its cache entry with the plain by-name
    /// lookup — one derivation, reached through either call.
    #[test]
    fn class_members_of_an_unbound_reference_reuses_class_members_entry() {
        let ambient = box_ambient();
        let by_name = ambient.class_members("Box").expect("Box resolves");
        let via_ty = ambient
            .class_members_of(&type_expr("Box"))
            .expect("Box resolves");
        assert!(Rc::ptr_eq(&by_name, &via_ty), "expected the shared entry");
    }

    /// The failure mode named in review round 4: two different bindings of
    /// the same generic class must never share a cache slot. `Box<number>`
    /// and `Box<string>` are looked up interleaved (the shape a real
    /// hover-then-completion-then-hover sequence would produce) and each
    /// must answer with its own binding every time, never the other's.
    #[test]
    fn class_members_of_does_not_cross_bindings_of_the_same_class() {
        let ambient = box_ambient();
        let number_ty = type_expr("Box<number>");
        let string_ty = type_expr("Box<string>");

        let first_number = ambient.class_members_of(&number_ty).expect("resolves");
        assert_eq!(first_number.fields["item"].ty.to_string(), "number");

        let first_string = ambient.class_members_of(&string_ty).expect("resolves");
        assert_eq!(first_string.fields["item"].ty.to_string(), "string");

        // Re-read both a second time, interleaved: neither may have been
        // clobbered by the other's cache entry.
        let second_string = ambient.class_members_of(&string_ty).expect("resolves");
        assert_eq!(second_string.fields["item"].ty.to_string(), "string");
        let second_number = ambient.class_members_of(&number_ty).expect("resolves");
        assert_eq!(second_number.fields["item"].ty.to_string(), "number");

        // And the repeats are the memoised entries, not fresh derivations.
        assert!(Rc::ptr_eq(&first_number, &second_number));
        assert!(Rc::ptr_eq(&first_string, &second_string));
    }

    // === class_ancestry_truncated (production readiness review, finding 2) =

    /// A single-file `---@class C0`, `---@class C1 : C0`, ..., `Cn : C(n-1)`
    /// chain of length `n`, matching `luabox-types`' own ancestry-limit
    /// fixtures.
    fn chain_source(n: usize) -> String {
        use std::fmt::Write as _;
        let mut src = String::from("---@class C0\n---@field item number\n");
        for i in 1..=n {
            let _ = writeln!(src, "---@class C{i} : C{}", i - 1);
        }
        src
    }

    #[test]
    fn class_ancestry_truncated_is_false_for_a_class_that_resolves_fully() {
        let ambient = merged(&chain_source(150));
        let shape = ambient.class_members("C150").expect("C150 resolves");
        assert!(shape.fields.contains_key("item"), "C0's field must survive");
        assert!(!ambient.class_ancestry_truncated("C150"));
    }

    /// The gap finding 2 names directly: `Ambient::class_members` walks a
    /// *different*, long-lived `TypeEnv` than the one `crate::check::run`
    /// drains for the Problems panel, and nothing used to drain — or even
    /// expose — this one's ledger. A class deep enough to trip
    /// `DiamondGuard`'s cap must both lose `C0`'s field from the shape
    /// `class_members` returns AND be flagged by
    /// `class_ancestry_truncated`, exactly as `luabox check` would report
    /// `LB0317` for the same class.
    #[test]
    fn class_ancestry_truncated_is_true_for_a_class_diamond_guard_cut_off() {
        let ambient = merged(&chain_source(400));
        let shape = ambient.class_members("C400").expect("C400 resolves");
        assert!(
            !shape.fields.contains_key("item"),
            "C0's field is above the cutoff and must not appear"
        );
        assert!(ambient.class_ancestry_truncated("C400"));
    }

    /// The query must reflect the class just resolved, not every class this
    /// layer has ever touched — a shallow class looked up after a truncated
    /// one must not inherit the previous call's `true`.
    #[test]
    fn class_ancestry_truncated_does_not_leak_across_classes() {
        let src = format!(
            "{}\n---@class Shallow\n---@field x number\n",
            chain_source(400)
        );
        let ambient = merged(&src);
        ambient.class_members("C400").expect("C400 resolves");
        assert!(ambient.class_ancestry_truncated("C400"));
        ambient.class_members("Shallow").expect("Shallow resolves");
        assert!(!ambient.class_ancestry_truncated("Shallow"));
    }
}

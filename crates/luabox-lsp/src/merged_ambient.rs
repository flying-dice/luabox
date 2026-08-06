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
use std::collections::HashMap;
use std::rc::Rc;

use luabox_syntax::luacats::{TypeExpr, TypeExprKind};
use luabox_types::ty::TableTy;
use luabox_types::{Ambient, FileTypes};

use crate::sema;

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
        }
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
}

//! Rendering a class member's resolved type the way `luabox check` renders
//! it **at the reference site**.
//!
//! [`crate::merged_ambient::MergedAmbient::class_members_of`] answers with
//! the shape the checker resolves, monomorphised against whatever type
//! arguments the reference itself carries (#48) — but a *bare* reference
//! carries none, so a generic class's own trailing parameters come back as
//! free [`Ty::Named`] parameter names. `luabox check` never shows those: it
//! erases a bare reference's own free parameters to `unknown` through
//! `TypeEnv::class_shape_bound_export`.
//!
//! M21 (round 6 review) taught `hover.rs` that erasure and nothing else, so
//! the three other surfaces reading the same shape — member completion's
//! detail line, and signature help's parameter and return labels — kept
//! rendering the free parameter name raw. Same symbol, same buffer: the
//! popup said `T`, the hover said `unknown` (production readiness review,
//! finding 3). This module is the one renderer all four sites go through, so
//! a fifth reader of `class_members_of` inherits the agreement instead of
//! having to remember it.

use std::path::Path;

use luabox_db::Analysis;
use luabox_syntax::luacats::{TypeExpr, TypeExprKind};
use luabox_types::ty::Ty;

use crate::merged_ambient::MergedAmbient;
use crate::sema;

/// The erasure in force for **one** class reference, resolved once and then
/// applied to as many of that reference's member types as a surface renders
/// (signature help alone renders one per parameter plus one per return).
///
/// `params` is empty whenever nothing is to be erased — the reference is
/// bound, or it names no class of its own — which makes [`Self::render`] a
/// plain `Display` in exactly the cases the checker leaves alone.
pub struct OwnParamErasure {
    /// The referenced class's own declared `<T, U, ...>` names, and only
    /// when the reference left them unbound.
    params: Vec<String>,
}

impl OwnParamErasure {
    /// The erasure a reference to `class` written as `ty` puts in force.
    ///
    /// Empty (erasing nothing) unless `ty` is a `Named` reference with **no**
    /// arguments: a bound reference (`Box<number>`) has already had a real
    /// type substituted by the checker's own monomorphisation (#48), and a
    /// compound type expression is not a class reference at all.
    ///
    /// The parameter list comes from [`sema::class_own_params`], whose doc
    /// records the one way this is narrower than the checker's own erasure:
    /// only the *directly*-referenced class's own literal parameters are
    /// recognised, not one inherited under a different name from a deeper
    /// ancestor.
    #[must_use]
    pub fn at_reference(
        ty: &TypeExpr,
        class: &str,
        analysis: &Analysis,
        current: &Path,
        ambient: &MergedAmbient,
    ) -> Self {
        let TypeExprKind::Named { args, .. } = &ty.kind else {
            return Self { params: Vec::new() };
        };
        if !args.is_empty() {
            return Self { params: Vec::new() };
        }
        Self {
            params: sema::class_own_params(
                analysis,
                current,
                class,
                ambient.ambient_paths(),
                ambient.sema_cache(),
                ambient.search_order_cache(),
            ),
        }
    }

    /// `ty` rendered for display: `unknown` when it is exactly one of the
    /// referenced class's own unbound parameters, otherwise its own
    /// `Display`.
    ///
    /// Deliberately shallow — only a `Ty::Named` standing alone is erased,
    /// never a parameter buried inside a compound type. That matches what
    /// the four call sites actually hand it (a field's type, or one
    /// parameter/return of a `fun(...)`-typed field), and going deeper would
    /// be this crate's approximation drifting further from
    /// `class_shape_bound_export`'s real substitution rather than closer to
    /// it.
    #[must_use]
    pub fn render(&self, ty: &Ty) -> String {
        match ty {
            Ty::Named(name) if self.params.iter().any(|p| p == name) => "unknown".to_string(),
            other => other.to_string(),
        }
    }
}

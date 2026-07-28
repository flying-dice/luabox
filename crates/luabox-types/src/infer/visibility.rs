//! Member visibility — the luals `invisible` diagnostic (`LB0312`, #115).
//!
//! Decides whether a `recv.member` access is *inside* the class that declared
//! `member` `---@private` / `---@protected` / `---@package`, and reports the
//! access when it is not. Conservative throughout: visibility is only ever
//! judged against an unambiguous single-class receiver.

use std::collections::HashSet;

use luabox_diag::{Diagnostic, Label, Span};
use luabox_hir::{BodyId, ExprId};
use luabox_syntax::luacats;

use crate::codes::INVISIBLE;
use crate::ty::Ty;

use super::{ITy, Infer};

impl Infer<'_> {
    /// The single `---@class` a receiver value resolves to, if any: a
    /// `Ty::Named` class, or an inference shape whose (metatable-chained)
    /// declaration names a class. `None` for anything else (a union, a plain
    /// table, `unknown`) — the conservative direction, so visibility is only
    /// ever judged against an unambiguous class receiver.
    pub(super) fn receiver_class(&self, recv: &ITy) -> Option<String> {
        match recv {
            ITy::Ty(Ty::Named(class)) if self.env.is_class(class) => Some(class.clone()),
            ITy::Shape(id) => self.shape_declared_class(*id),
            _ => None,
        }
    }

    /// The declared `---@class` name of a shape, following the `__index` chain
    /// (the receiver-side of [`Self::receiver_class`], reused to name the
    /// enclosing class of a carrier method).
    pub(super) fn shape_declared_class(&self, id: usize) -> Option<String> {
        let mut cur = Some(id);
        let mut seen = HashSet::new();
        while let Some(s) = cur {
            if !seen.insert(s) {
                break;
            }
            if let Some(class) = &self.shapes[s].declared
                && self.env.is_class(class)
            {
                return Some(class.clone());
            }
            cur = self.index_delegate(s);
        }
        None
    }

    /// Check an access `recv.member` (or `recv:member()`) against the member's
    /// declared visibility and report `invisible` (`LB0312`) when it is not
    /// reachable from here (#115). `recv_class` is the receiver's resolved
    /// class; a public member (or one on a non-restricting class) is silent.
    pub(super) fn check_visibility(
        &mut self,
        body: BodyId,
        expr: ExprId,
        recv_class: &str,
        member: &str,
    ) {
        if self.pass != 1 {
            return;
        }
        let Some((scope, owner)) = self.env.member_visibility(recv_class, member) else {
            return;
        };
        let allowed = match scope {
            luacats::FieldScope::Public => return,
            // Private: only the owning class's own methods.
            luacats::FieldScope::Private => self.class_ctx.iter().any(|c| c == &owner),
            // Protected: the owning class or any subclass method.
            luacats::FieldScope::Protected => self
                .class_ctx
                .iter()
                .any(|c| c == &owner || self.env.is_subclass(c, &owner)),
            // Package: anywhere in the file that declares the owning class.
            luacats::FieldScope::Package => self.env.declares_class_locally(&owner),
        };
        if !allowed {
            self.report_invisible(body, expr, member, &owner, scope);
        }
    }

    /// Report an `invisible` access (`LB0312`). Follows the strictness ladder
    /// like its sibling `undefined-field` (`LB0306`) — a warning in warn mode,
    /// an error in strict — which is stricter than luals (always a warning).
    fn report_invisible(
        &mut self,
        body: BodyId,
        expr: ExprId,
        member: &str,
        owner: &str,
        scope: luacats::FieldScope,
    ) {
        let Some((start, end)) = self.expr_range(body, expr) else {
            return;
        };
        let (kind, reach) = match scope {
            luacats::FieldScope::Private => ("private", "its own class"),
            luacats::FieldScope::Protected => ("protected", "its class and subclasses"),
            luacats::FieldScope::Package => ("package", "the file that declares its class"),
            luacats::FieldScope::Public => return,
        };
        let mut diag = Diagnostic::new(
            INVISIBLE,
            self.severity,
            format!("cannot access {kind} member `{member}` of `{owner}` here"),
        )
        .with_label(Label::primary(
            Span::new(self.file, start..end),
            format!("`{member}` is {kind} to `{owner}` — accessible only from {reach}"),
        ));
        if let Some(range) = self.env.class_decl_span(owner) {
            diag = diag.with_label(Label::secondary(
                Span::new(self.file.to_string(), range),
                format!("`{owner}` declared here"),
            ));
        }
        self.diags.push(diag);
    }
}

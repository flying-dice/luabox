//! Carrier conformance: does an accumulated table shape satisfy the contract
//! its `---@type` / `---@class` annotation promised?
//!
//! Both obligations are *deferred* — a carrier local (`local M = {}` extended
//! by later `M.f = ...` / `function M:m()` lines) only satisfies its
//! annotation once the whole file has been walked, so the check runs against
//! the final accumulated shape inference publishes, after traversal:
//!
//! - `---@type` carriers, attributed to the annotation (`LB0300`); and
//! - `---@class Name : Interface` carriers, one diagnostic per missing or
//!   mismatched inherited member (#107), with same-file parent carriers
//!   reached through the `Child.__index = Base` chain counted as provided.

use std::collections::HashSet;
use std::ops::Range;

use luabox_diag::{Diagnostic, Label, Span};

use crate::assign::Exactness;
use crate::codes::TYPE_MISMATCH;
use crate::ty::{FieldTy, TableTy, Ty};

use super::{Checker, ClassObligation};

impl Checker<'_> {
    /// Run every deferred `---@type` carrier conformance obligation against
    /// the binding's final accumulated shape. The diagnostic is attributed to
    /// the `---@type` annotation and carries the same member-naming detail as
    /// an immediate mismatch. A carrier whose accumulated shape satisfies the
    /// type produces nothing.
    pub(super) fn check_deferred_carriers(&mut self) {
        for carrier in std::mem::take(&mut self.deferred_carriers) {
            let Some(found) = self.carrier_final.get(&carrier.decl_key).cloned() else {
                continue; // inference published no final shape (e.g. off)
            };
            if self.assignable(&found, &carrier.target) {
                continue;
            }
            let detail = crate::assign::explain_mismatch(
                self.env,
                Exactness::from_strict(self.strict),
                &found,
                &carrier.target,
            )
            .map_or(String::new(), |d| format!(": {d}"));
            self.report_full(
                TYPE_MISMATCH,
                carrier.span.clone(),
                format!(
                    "type mismatch: expected `{}`, found `{found}`{detail}",
                    carrier.target
                ),
                format!("expected `{}`", carrier.target),
                None,
            );
        }
    }

    /// Verify each `---@class Name : Parent` carrier against the interface(s)
    /// it declares it extends (#107) — the strictness luals declares but does
    /// not check.
    ///
    /// The obligation is every member the parent chain declares (the merged
    /// [`TypeEnv::class_shape`] of each parent), *excluding* members `Name`
    /// re-declares as its own `---@field` (those are governed by `Name`'s own
    /// declaration). Each obliged member must be satisfied by the carrier's
    /// FINAL accumulated shape. The rule, precisely — a member is satisfied
    /// when it is:
    ///
    /// - **(a) provided by the carrier** — `function X:m()` / `X.f = ...`,
    ///   *plus* anything inherited through a `setmetatable(X, { __index =
    ///   Base })` chain, which [`infer::reify_shape`] already folds into the
    ///   reified `carrier_final` shape. A provided member's type is checked
    ///   against the parent's declaration via [`Checker::assignable`]
    ///   (function subtyping absorbs `self`/receiver looseness); a mismatch is
    ///   reported.
    /// - **(b) inherited from a parent carrier** defined in this file and
    ///   reachable through the class's parent chain — the fallback that
    ///   covers the `X.__index = Base` idiom, whose delegation the carrier's
    ///   own reified shape does not fold in. The inherited implementation is
    ///   the base's concern, so its signature is not re-checked here.
    /// - **(c) optional / nil-admitting in the parent** — no obligation.
    ///
    /// Only a member satisfied by none of these is reported missing. This is
    /// what keeps classic inheritance from being wrongly flagged: a subclass
    /// that inherits a concrete base method (idiom (a) or (b)) is silent.
    pub(super) fn check_class_conformance(&mut self) {
        for ob in std::mem::take(&mut self.class_obligations) {
            let Some(Ty::Table(provided)) = self.carrier_final.get(&ob.decl_key).cloned() else {
                continue; // inference published no final shape (e.g. off)
            };
            let Some(parents) = self.env.class_parents(&ob.name) else {
                continue;
            };
            let parents: Vec<String> = parents.to_vec();
            // Members already handled — dedup across a diamond of parents.
            let mut seen: HashSet<String> = HashSet::new();
            for parent in &parents {
                let Some(pshape) = self.env.class_shape(parent) else {
                    continue;
                };
                for (member, field) in &pshape.fields {
                    if !seen.insert(member.clone()) {
                        continue;
                    }
                    // A member `Name` re-declares is its own declaration's
                    // responsibility, not an inherited obligation.
                    if self.env.class_declares_own(&ob.name, member) {
                        continue;
                    }
                    // Optional / nil-admitting members impose no obligation.
                    if field.optional || field.ty.admits_nil() {
                        continue;
                    }
                    self.check_class_member(&ob, parent, member, field, &provided);
                }
            }
        }
    }

    /// Check one obliged member against the carrier's final shape. See
    /// [`Checker::check_class_conformance`] for the rule.
    fn check_class_member(
        &mut self,
        ob: &ClassObligation,
        parent: &str,
        member: &str,
        field: &FieldTy,
        provided: &TableTy,
    ) {
        // (a) provided on the carrier (own members + `setmetatable` chain).
        if let Some(actual) = provided.fields.get(member) {
            let expected = if field.optional {
                field.ty.clone().optional()
            } else {
                field.ty.clone()
            };
            if !self.assignable(&actual.ty, &expected) {
                let detail = crate::assign::explain_mismatch(
                    self.env,
                    Exactness::from_strict(self.strict),
                    &actual.ty,
                    &expected,
                )
                .map_or(String::new(), |d| format!(": {d}"));
                self.report_class_conformance(
                    ob.span.clone(),
                    format!(
                        "`{}` does not satisfy `{parent}`: member `{member}` has the wrong type",
                        ob.name
                    ),
                    format!("expected `{expected}`, found `{}`{detail}", actual.ty),
                    parent,
                );
            }
            return;
        }
        // (b) inherited from a parent carrier in this file (the `X.__index =
        //     Base` chain the reified carrier shape does not fold in).
        if self.member_on_parent_carrier(&ob.name, member) {
            return;
        }
        // (c) missing entirely.
        self.report_class_conformance(
            ob.span.clone(),
            format!(
                "`{}` does not satisfy `{parent}`: missing member `{member}`",
                ob.name
            ),
            format!("expected member `{member}` of type `{}`", field.ty),
            parent,
        );
    }

    /// Whether `member` is defined on any parent carrier of `class` in this
    /// file, walking the parent chain transitively. The parent-carrier
    /// fallback of [`Checker::check_class_conformance`].
    fn member_on_parent_carrier(&self, class: &str, member: &str) -> bool {
        let mut stack: Vec<String> = self
            .env
            .class_parents(class)
            .map(<[String]>::to_vec)
            .unwrap_or_default();
        let mut seen: HashSet<String> = HashSet::new();
        while let Some(parent) = stack.pop() {
            if !seen.insert(parent.clone()) {
                continue;
            }
            if let Some(Ty::Table(shape)) = self.carrier_class_final.get(&parent)
                && shape.fields.contains_key(member)
            {
                return true;
            }
            if let Some(grandparents) = self.env.class_parents(&parent) {
                stack.extend(grandparents.iter().cloned());
            }
        }
        false
    }

    /// Report an LB0300 `: Interface` conformance failure at the `---@class`
    /// tag, with a "declared here" secondary label at the parent's in-file
    /// declaration when it has one (ambient/defs parents have none) (#107).
    fn report_class_conformance(
        &mut self,
        span: Range<usize>,
        message: String,
        label: String,
        parent: &str,
    ) {
        let mut diag = Diagnostic::new(TYPE_MISMATCH, self.severity, message)
            .with_label(Label::primary(Span::new(self.file, span), label));
        if let Some(range) = self.env.class_decl_span(parent) {
            diag = diag.with_label(Label::secondary(
                Span::new(self.file.to_string(), range),
                format!("`{parent}` declared here"),
            ));
        }
        self.diags.push(diag);
    }
}

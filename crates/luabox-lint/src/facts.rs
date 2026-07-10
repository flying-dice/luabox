//! Declared-type facts for type-informed rules.
//!
//! Rules must be type-informed (SPEC.md §9) but the shared `TypeEnv` exposes
//! no per-binding query API across the crate boundary, so this module harvests
//! the LuaCATS front-end directly (`---@param`, `---@type`) and answers the
//! narrow questions the rules ask — "can this value be `false`?",
//! "is this shape an array?" — over the raw [`TypeExpr`] grammar. When a
//! binding has no annotation its type is unknown and the rules stay silent
//! (the "unknown → skip" contract), so this is precise and never guesses.

use std::collections::HashMap;

use luabox_hir::{Binding, BindingId, BindingKind, LoweredFile};
use luabox_syntax::lua;
use luabox_syntax::luacats::{self, Span, Tag, TypeExpr, TypeExprKind};

/// Declared types keyed by the binding they annotate.
#[derive(Debug, Default)]
pub struct TypeFacts {
    binding_types: HashMap<BindingId, TypeExpr>,
}

impl TypeFacts {
    /// Harvest `---@param` and `---@type` annotations and bind them to HIR
    /// bindings by name within the annotated statement.
    #[must_use]
    pub fn build(parse: &lua::Parse, lowered: &LoweredFile) -> Self {
        let items = luacats::harvest(parse);
        let mut binding_types = HashMap::new();
        for item in &items {
            let Some(target) = item.target else {
                continue;
            };
            for tag in &item.block.tags {
                match tag {
                    Tag::Param(p) => {
                        if let Some(bid) = param_binding(lowered, target, &p.name) {
                            binding_types.insert(bid, p.ty.clone());
                        }
                    }
                    Tag::Type(t) => {
                        let locals = local_bindings(lowered, target);
                        for (bid, ty) in locals.into_iter().zip(&t.types) {
                            binding_types.insert(bid, ty.clone());
                        }
                    }
                    _ => {}
                }
            }
        }
        Self { binding_types }
    }

    /// Whether the binding's declared type provably excludes `false` (so a
    /// `~= nil` / `== nil` guard is equivalent to plain truthiness). Unknown
    /// or unannotated bindings return `false` — the rule stays silent.
    #[must_use]
    pub fn excludes_false(&self, binding: BindingId) -> bool {
        self.binding_types
            .get(&binding)
            .is_some_and(|ty| excludes_false(&ty.kind))
    }

    /// Whether the binding's declared type is an array shape (`T[]` /
    /// `table<integer, V>`).
    #[must_use]
    pub fn is_array(&self, binding: BindingId) -> bool {
        self.binding_types
            .get(&binding)
            .is_some_and(|ty| is_array(&ty.kind))
    }
}

/// Whether `offset` falls inside the target statement span.
fn within(target: Span, binding: &Binding) -> bool {
    let start = usize::from(binding.range.start());
    target.start <= start && start < target.end
}

/// The earliest parameter binding named `name` within `target` (the outer
/// function's parameter, ahead of any nested-function shadow).
fn param_binding(lowered: &LoweredFile, target: Span, name: &str) -> Option<BindingId> {
    lowered
        .bindings()
        .filter(|(_, b)| {
            matches!(b.kind, BindingKind::Param) && b.name == name && within(target, b)
        })
        .min_by_key(|(_, b)| b.range.start())
        .map(|(id, _)| id)
}

/// Local bindings declared directly by the target statement, in source order.
fn local_bindings(lowered: &LoweredFile, target: Span) -> Vec<BindingId> {
    let mut locals: Vec<(BindingId, u32)> = lowered
        .bindings()
        .filter(|(_, b)| matches!(b.kind, BindingKind::Local) && within(target, b))
        .map(|(id, b)| (id, b.range.start().into()))
        .collect();
    locals.sort_by_key(|(_, start)| *start);
    locals.into_iter().map(|(id, _)| id).collect()
}

/// Whether a type can never be `false`.
fn excludes_false(kind: &TypeExprKind) -> bool {
    match kind {
        // `boolean` and the wildcards can be `false`; a generic capture is
        // unknown. Everything else (number, string, classes, tables, ...)
        // is truthy or, at worst, `nil` — which the guard handles correctly.
        TypeExprKind::Named { name, .. } => !matches!(name.as_str(), "boolean" | "any" | "unknown"),
        TypeExprKind::BoolLit(value) => *value,
        TypeExprKind::Optional(inner) | TypeExprKind::Paren(inner) => excludes_false(&inner.kind),
        TypeExprKind::Union(members) => members.iter().all(|m| excludes_false(&m.kind)),
        TypeExprKind::Array(_)
        | TypeExprKind::Table(_)
        | TypeExprKind::Fun { .. }
        | TypeExprKind::Tuple(_)
        | TypeExprKind::StringLit(_)
        | TypeExprKind::NumberLit(_) => true,
        TypeExprKind::Backtick(_) | TypeExprKind::Error => false,
    }
}

/// Whether a type is an array shape.
fn is_array(kind: &TypeExprKind) -> bool {
    match kind {
        TypeExprKind::Array(_) => true,
        TypeExprKind::Paren(inner) => is_array(&inner.kind),
        TypeExprKind::Named { name, args } => {
            name == "table"
                && args.len() == 2
                && matches!(
                    &args[0].kind,
                    TypeExprKind::Named { name, .. } if name == "integer" || name == "number"
                )
        }
        _ => false,
    }
}

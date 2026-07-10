//! [`LintContext`]: everything a rule needs for one file — the parse tree, the
//! HIR [`LoweredFile`], the declared-type facts, the config, and a precomputed
//! binding use-index shared across rules.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use luabox_hir::{BindingId, Expr, HirId, LoweredFile, Resolution, Stmt};
use luabox_syntax::lua;
use rowan::TextRange;

use crate::config::LintConfig;
use crate::facts::TypeFacts;

/// Convert a rowan range to a byte range.
#[must_use]
pub fn to_range(range: TextRange) -> Range<usize> {
    usize::from(range.start())..usize::from(range.end())
}

/// Which bindings are read/written across the whole file.
///
/// Computed once per file and shared by the `unused-*` rules. A "read" is any
/// name reference that is not the direct target of an assignment; a reference
/// inside a nested closure (resolved as an [`Resolution::Upvalue`]) counts, so
/// genuine captures are never mistaken for unused.
#[derive(Debug, Default)]
pub struct UseIndex {
    /// Bindings referenced as a read at least once.
    pub read: HashSet<BindingId>,
    /// Total name references (reads + writes) per binding.
    pub occurrences: HashMap<BindingId, usize>,
}

impl UseIndex {
    fn build(lowered: &LoweredFile) -> Self {
        // Pass 1: collect the ids of name expressions that are assignment
        // targets (writes).
        let mut write_targets: HashSet<HirId> = HashSet::new();
        for (body_id, body) in lowered.bodies() {
            for (_, stmt) in body.stmts() {
                if let Stmt::Assign { targets, .. } = stmt {
                    for &target in targets {
                        if matches!(body.expr(target), Expr::Name(_)) {
                            write_targets.insert(HirId::expr(body_id, target));
                        }
                    }
                }
            }
        }

        // Pass 2: tally references.
        let mut index = UseIndex::default();
        for (body_id, body) in lowered.bodies() {
            for (expr_id, expr) in body.exprs() {
                if !matches!(expr, Expr::Name(_)) {
                    continue;
                }
                let hir = HirId::expr(body_id, expr_id);
                let Some(binding) = binding_of(lowered.resolution(hir)) else {
                    continue;
                };
                *index.occurrences.entry(binding).or_default() += 1;
                if !write_targets.contains(&hir) {
                    index.read.insert(binding);
                }
            }
        }
        index
    }

    /// Whether the binding is read anywhere (including nested closures).
    #[must_use]
    pub fn is_read(&self, binding: BindingId) -> bool {
        self.read.contains(&binding)
    }

    /// Total number of name references to the binding.
    #[must_use]
    pub fn occurrences(&self, binding: BindingId) -> usize {
        self.occurrences.get(&binding).copied().unwrap_or(0)
    }
}

/// The binding a name resolution refers to (local or upvalue), if any.
#[must_use]
pub fn binding_of(res: Option<&Resolution>) -> Option<BindingId> {
    match res? {
        Resolution::Local(b) => Some(*b),
        Resolution::Upvalue { binding, .. } => Some(*binding),
        Resolution::Global(_) => None,
    }
}

/// Per-file context passed to every [`crate::rule::Rule`].
pub struct LintContext<'a> {
    /// The file name used in diagnostic spans.
    pub file: &'a str,
    /// The source text.
    pub source: &'a str,
    /// The parse tree (lossless).
    pub parse: &'a lua::Parse,
    /// The lowered HIR with name resolution.
    pub lowered: &'a LoweredFile,
    /// Declared-type facts harvested from LuaCATS annotations.
    pub facts: &'a TypeFacts,
    /// Effective lint configuration (for rules that consult it).
    pub config: &'a LintConfig,
    /// Shared binding use-index.
    pub uses: UseIndex,
}

impl<'a> LintContext<'a> {
    /// Build a context, computing the use-index.
    #[must_use]
    pub fn new(
        file: &'a str,
        source: &'a str,
        parse: &'a lua::Parse,
        lowered: &'a LoweredFile,
        facts: &'a TypeFacts,
        config: &'a LintConfig,
    ) -> Self {
        Self {
            file,
            source,
            parse,
            lowered,
            facts,
            config,
            uses: UseIndex::build(lowered),
        }
    }

    /// The byte range a HIR node was lowered from, if recorded.
    #[must_use]
    pub fn node_range(&self, id: HirId) -> Option<Range<usize>> {
        self.lowered.source_map().range(id).map(to_range)
    }

    /// The source slice for a byte range.
    #[must_use]
    pub fn text(&self, range: &Range<usize>) -> &str {
        self.source.get(range.clone()).unwrap_or("")
    }
}

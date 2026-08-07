//! Goto implementation: from a `---@class Interface` declaration to every
//! `---@class X : Interface` that extends it across the workspace.
//!
//! Class inheritance is workspace-global (#85/#110), so an interface's
//! implementors can live in any file. We identify the class the cursor is on
//! (the `---@class` tag whose span covers the offset), then walk every file's
//! [`FileSema::classes`], collecting the declaration site of each class whose
//! parent list (`: Interface`) names it. `None` when the cursor is not on a
//! class declaration; an empty vec when the class has no subclasses.

use lsp_types::Location;
use luabox_db::Analysis;
use luabox_syntax::luacats::{Span, Tag};

use crate::sema::{self, FileSema};
use crate::uri::path_to_uri;

/// Every implementor of the `---@class` under `offset`: the declaration site of
/// each workspace class that lists it as a parent.
#[must_use]
pub fn implementation(
    analysis: &Analysis,
    target: &FileSema,
    offset: usize,
) -> Option<Vec<Location>> {
    let name = class_at(target, offset)?;
    let mut out = Vec::new();
    collect_implementors(target, &name, &mut out);
    for path in analysis.files() {
        if path == target.path {
            continue;
        }
        if let Some(file) = FileSema::new(analysis, path) {
            collect_implementors(&file, &name, &mut out);
        }
    }
    out.sort_by(|a, b| key(a).cmp(&key(b)));
    out.dedup();
    Some(out)
}

/// The name of the `---@class` whose tag span covers `offset`, if any.
fn class_at(sema: &FileSema, offset: usize) -> Option<String> {
    for item in sema.items() {
        for tag in &item.block.tags {
            if let Tag::Class(c) = tag
                && !c.name.is_empty()
                && c.span.start <= offset
                && offset <= c.span.end
            {
                return Some(c.name.clone());
            }
        }
    }
    None
}

/// Append the declaration site of every class in `sema` that names `parent`
/// among its parents.
///
/// Round 5 review N12 flagged this alongside `luabox-types`'s five
/// hand-rolled ancestor walks as a candidate for the same cleanup. It is
/// not one: those walk *up* a chain of `---@class` parents, so they need a
/// cycle guard (a malformed or adversarial `: Parent` graph can cycle) and,
/// for `DiamondGuard` specifically, type-argument sensitivity (`env.rs`'s
/// doc comment on `DiamondGuard` and `TypeEnv::walk_ancestor_names`). This
/// function does neither — it is a single pass over one file's classes,
/// each checked against `parent` by one direct, non-recursive membership
/// test (`info.tag.parents`), with no edge ever followed and therefore
/// nothing to guard against looping. There is no walk here to unify with
/// `walk_ancestor_names` or replace with `DiamondGuard`; the two families
/// share a data shape (`---@class ... : Parent`) but not an algorithm.
///
/// What *is* true, separately from N12: `luabox-types`'s own
/// `TypeEnv::is_subclass` (`env.rs`) treats "subclass" as transitive
/// (walking the full ancestor chain via `walk_ancestor_names`), while this
/// function only matches *direct* parents — a three-level
/// `Leaf : Mid : Base` reports `Mid` but not `Leaf` as an implementor of
/// `Base`. That is an intentional scope choice this module's own doc
/// comment states ("collecting the declaration site of each class whose
/// parent list (`: Interface`) names it"), not a residual left by this
/// finding; making it transitive would be new behaviour with its own
/// design/perf/test cost, out of scope for closing N12.
fn collect_implementors(sema: &FileSema, parent: &str, out: &mut Vec<Location>) {
    for info in sema.classes().values() {
        let extends = info
            .tag
            .parents
            .iter()
            .any(|p| sema::named_of(p).as_deref() == Some(parent));
        if extends {
            out.push(location(sema, info.tag.span));
        }
    }
}

fn location(sema: &FileSema, span: Span) -> Location {
    Location {
        uri: path_to_uri(&sema.path),
        range: sema.index.range(span.start..span.end),
    }
}

/// A total order over locations: file, then start, then end (mirrors
/// [`crate::references`]).
fn key(loc: &Location) -> (&str, u32, u32, u32, u32) {
    (
        loc.uri.as_str(),
        loc.range.start.line,
        loc.range.start.character,
        loc.range.end.line,
        loc.range.end.character,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    use luabox_db::{AnalysisHost, Change, Dialect, Strictness};

    fn analyze(files: &[(&str, &str)]) -> (Analysis, PathBuf) {
        let mut host = AnalysisHost::new(Dialect::Lua54, Strictness::Warn);
        let root = Path::new(if cfg!(windows) { r"C:\ws" } else { "/ws" });
        let mut first = None;
        for (rel, text) in files {
            let path = root.join(rel);
            first.get_or_insert_with(|| path.clone());
            host.apply_change(Change::SetFileText {
                path,
                dialect: Dialect::Lua54,
                text: (*text).to_string(),
            });
        }
        (host.snapshot(), first.expect("at least one file"))
    }

    fn offset_of(text: &str, needle: &str) -> usize {
        text.find(needle).expect("needle present")
    }

    fn run(files: &[(&str, &str)], offset: usize) -> Option<Vec<Location>> {
        let (analysis, path) = analyze(files);
        let target = FileSema::new(&analysis, &path).expect("target sema");
        implementation(&analysis, &target, offset)
    }

    #[test]
    fn interface_returns_its_subclasses_across_files() {
        let files = &[
            ("base.lua", "---@class Base\n---@field id number\n"),
            ("derived.lua", "---@class Derived : Base\n"),
            ("other.lua", "---@class Other : Base\n"),
        ];
        // Cursor on the `---@class Base` line in base.lua.
        let offset = offset_of("---@class Base\n", "Base");
        let impls = run(files, offset).expect("implementation");
        assert_eq!(impls.len(), 2, "{impls:?}");
        assert!(
            impls
                .iter()
                .any(|l| l.uri.as_str().ends_with("derived.lua")),
            "{impls:?}"
        );
        assert!(
            impls.iter().any(|l| l.uri.as_str().ends_with("other.lua")),
            "{impls:?}"
        );
    }

    #[test]
    fn interface_without_subclasses_is_empty() {
        let files = &[("base.lua", "---@class Base\n")];
        let offset = offset_of("---@class Base\n", "Base");
        let impls = run(files, offset).expect("implementation");
        assert!(impls.is_empty(), "{impls:?}");
    }

    #[test]
    fn cursor_off_a_class_returns_none() {
        let files = &[("main.lua", "local x = 1\n")];
        assert!(run(files, 0).is_none());
    }
}

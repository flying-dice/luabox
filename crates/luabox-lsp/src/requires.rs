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

use std::collections::HashMap;
use std::path::Path;

use luabox_db::Analysis;
use luabox_types::RockSurfaces;
use luabox_types::ty::Ty;

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
}

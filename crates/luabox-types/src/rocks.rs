//! Type surfaces harvested from a vendored luarocks tree (#30).
//!
//! `luarocks install --tree lua_modules <rock>` installs the rock's *ordinary
//! Lua sources* under `lua_modules/share/lua/<X.Y>/`. Those sources are
//! frequently annotated — LuaCATS is the ecosystem's annotation dialect, and a
//! rock that documents itself for lua-language-server has already written
//! everything the checker needs. Before #30 none of it reached your use sites:
//! cross-package types required a `[dependencies]` entry plus a per-package
//! `luabox.toml` with `[types] defs`, which a luarocks tree does not have. That
//! was the documented sharp edge.
//!
//! [`harvest`] closes it by reading those installed sources for their
//! **surfaces** and nothing else:
//!
//! * a rock file's `---@class`/`---@enum`/`---@alias` declarations, so a rock
//!   type is nameable and enforceable in your code
//!   ([`crate::Ambient::with_project_types`]); and
//! * its `require`-export type, so `local m = require("rock")` is typed
//!   ([`crate::check_file_with_requires`]).
//!
//! # Ambient-relaxed, never checked
//!
//! Vendored code is not your code and is never typechecked (that rule is #16's
//! and stands): [`harvest`] runs [`crate::module_surface`], whose diagnostics
//! are discarded by construction — it returns a surface, not findings. A rock
//! file with a type error in its body therefore produces nothing, and a rock
//! file that does not parse is skipped with a debug-level note
//! ([`RockSurfaces::skipped`]), never a project diagnostic. Signatures are
//! visible; bodies are invisible; failures never gate.
//!
//! # Only annotated sources contribute
//!
//! A rock source with no `---@` annotation anywhere is skipped before it is
//! parsed. That is conservative by construction, not just cheap: the export
//! type of an unannotated module is a table of `unknown`-typed members whose
//! shape is whatever the file's top-level assignments happen to reveal, so
//! *adding* it could only turn dynamic module construction into
//! `undefined-field` noise about code the user did not write. Un-annotated
//! rocks therefore stay exactly as they were before #30 — requirable,
//! bundlable, `unknown` to the checker — and the explicit `[types] defs`
//! escape hatch is how you type one.

use std::collections::BTreeMap;
use std::path::PathBuf;

use luabox_syntax::lua::{self, Dialect};

use crate::env::FileTypes;
use crate::ty::Ty;
use crate::{Ambient, FileArtifacts, module_surface_with_artifacts};

/// The marker that makes a rock source worth parsing: any LuaCATS annotation
/// comment. `---@meta`, `---@class`, `---@param`, `---@return` — all of them
/// start this way, and nothing else in Lua does.
const ANNOTATION_MARKER: &str = "---@";

/// The dialect rock sources are parsed with: the richest one, so nothing is
/// rejected for belonging to a newer edition than the tree's version directory
/// suggests. This mirrors how `.d.lua` definition files are parsed
/// ([`crate::Ambient`]) — a *surface* is read as permissively as possible,
/// because the alternative to reading it is having no types at all.
const HARVEST_DIALECT: Dialect = Dialect::Lua54;

/// One installed rock source offered to [`harvest`] — the Semantics-side shape
/// of `luabox_manifest::layout::RockSource` (Distribution owns the tree walk
/// and must not depend on this crate, SPEC.md §16).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RockModule {
    /// The dotted `require` name this file answers to (`pl.tablex`).
    pub module: String,
    /// How debug notes name this file (its root-relative path).
    pub label: String,
    /// The file on disk — the identity a resolved `require` is matched against
    /// by a caller that resolves module strings to paths (`luabox check`).
    pub path: PathBuf,
    /// The source text.
    pub text: String,
}

/// What [`harvest`] found in a luarocks tree.
///
/// Every map is populated **first-wins in the caller's order** (path-sorted, so
/// deterministic). Two files can answer to one module name — `pl.lua` and
/// `pl/init.lua` — and path order puts `pl.lua` first, which is also the order
/// `luabox_bundle::resolve_candidates` tries them in, so the harvest and
/// `require` resolution cannot disagree about which file is `pl`.
#[derive(Debug, Default)]
pub struct RockSurfaces {
    types: Vec<FileTypes>,
    by_module: BTreeMap<String, Ty>,
    by_path: Vec<(PathBuf, Ty)>,
    skipped: Vec<String>,
}

impl RockSurfaces {
    /// The workspace-global `---@class`/`---@enum`/`---@alias` declarations the
    /// harvested rocks contribute, in harvest order.
    ///
    /// Merge these **after** the project's own files
    /// ([`crate::Ambient::with_project_types`] is first-wins): explicit beats
    /// implicit, so a name the project declares wins over a rock's.
    #[must_use]
    pub fn types(&self) -> &[FileTypes] {
        &self.types
    }

    /// Module name → export type, for a caller whose `require` resolution is
    /// name-keyed (the LSP, whose database only knows project files).
    #[must_use]
    pub fn by_module(&self) -> &BTreeMap<String, Ty> {
        &self.by_module
    }

    /// Rock file path → export type, for a caller that resolves a `require`
    /// string to a path first (`luabox check`, through
    /// `luabox_bundle::resolve_module`). Path-keyed resolution is the precise
    /// one: a project file that shadows a rock module wins because resolution
    /// returns *its* path.
    #[must_use]
    pub fn by_path(&self) -> &[(PathBuf, Ty)] {
        &self.by_path
    }

    /// The labels of rock sources that were annotated but did not parse, in
    /// harvest order — a debug-level note for the curious, never a diagnostic
    /// (see the module docs).
    #[must_use]
    pub fn skipped(&self) -> &[String] {
        &self.skipped
    }

    /// Whether nothing at all was harvested — no types, no exports. True for a
    /// project with no rock tree, which is the common case.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.types.is_empty() && self.by_path.is_empty()
    }
}

/// Harvest the annotation surfaces of `sources` against `ambient` (the dialect
/// stdlib plus any explicit `[types] defs`, which is what a rock's own
/// annotations resolve against).
///
/// Order is precedence: `sources` is consumed in the caller's order (the
/// path-sorted order `luabox_manifest::layout::collect_rock_sources` produces)
/// and the first file to claim a module name keeps it.
///
/// Nothing here can fail and nothing here can diagnose — see the module docs.
#[must_use]
pub fn harvest(ambient: &Ambient, sources: &[RockModule]) -> RockSurfaces {
    let mut out = RockSurfaces::default();
    for source in sources {
        if !source.text.contains(ANNOTATION_MARKER) {
            continue;
        }
        let parse = lua::parse(&source.text, HARVEST_DIALECT);
        if !parse.errors().is_empty() {
            // A recovered tree is a guess at the file's structure, and a
            // guessed surface is worse than no surface. Note and move on.
            out.skipped.push(source.label.clone());
            continue;
        }
        let artifacts = FileArtifacts::new(&parse);
        let surface =
            module_surface_with_artifacts(&parse, &source.label, Some(ambient), &artifacts);
        if let Some(export) = surface.export {
            out.by_module
                .entry(source.module.clone())
                .or_insert_with(|| export.clone());
            out.by_path.push((source.path.clone(), export));
        }
        if !surface.types.is_empty() {
            out.types.push(surface.types);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    // test code — panics document assumptions
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use crate::stdlib_defs;

    /// A rock source at the given module name.
    fn rock(module: &str, text: &str) -> RockModule {
        RockModule {
            module: module.to_owned(),
            label: format!("lua_modules/share/lua/5.4/{}.lua", module.replace('.', "/")),
            path: PathBuf::from(format!(
                "/proj/lua_modules/share/lua/5.4/{}.lua",
                module.replace('.', "/")
            )),
            text: text.to_owned(),
        }
    }

    /// An annotated rock: a class plus a constructor returning it.
    const ANNOTATED: &str = "\
---@class mylib.Point
---@field x number
---@field y number

local M = {}

---@param x number
---@param y number
---@return mylib.Point
function M.point(x, y)
  return { x = x, y = y }
end

return M
";

    fn harvest_of(sources: &[RockModule]) -> RockSurfaces {
        harvest(stdlib_defs(Dialect::Lua54), sources)
    }

    #[test]
    fn an_annotated_rock_contributes_its_class_and_its_export_type() {
        let surfaces = harvest_of(&[rock("mylib", ANNOTATED)]);
        assert!(!surfaces.is_empty());
        assert_eq!(surfaces.types().len(), 1);
        assert!(surfaces.types()[0].classes.contains_key("mylib.Point"));
        assert!(surfaces.by_module().contains_key("mylib"));
        assert_eq!(surfaces.by_path().len(), 1);
        assert_eq!(
            surfaces.by_path()[0].0,
            PathBuf::from("/proj/lua_modules/share/lua/5.4/mylib.lua")
        );
        assert!(surfaces.skipped().is_empty());
    }

    #[test]
    fn an_unannotated_rock_contributes_nothing_and_is_not_reported_as_skipped() {
        let surfaces = harvest_of(&[rock(
            "plain",
            "local M = {}\nfunction M.f() end\nreturn M\n",
        )]);
        assert!(surfaces.is_empty());
        // Not "skipped": skipping an unannotated file is the normal path, not
        // a failure worth a debug note.
        assert!(surfaces.skipped().is_empty());
    }

    #[test]
    fn an_annotated_rock_that_does_not_parse_is_skipped_by_label() {
        let surfaces = harvest_of(&[rock("broken", "---@class Wat\nlocal = = =\n")]);
        assert!(surfaces.is_empty());
        assert_eq!(
            surfaces.skipped(),
            ["lua_modules/share/lua/5.4/broken.lua".to_owned()]
        );
    }

    #[test]
    fn a_type_error_in_a_rock_body_yields_no_finding_because_none_are_returned() {
        // The rock misuses its own annotated function. Harvesting reads its
        // surface; it never checks it, so there is nowhere for a diagnostic to
        // come out — and the surface is still complete.
        let surfaces = harvest_of(&[rock(
            "bad",
            "---@param n number\n\
             ---@return number\n\
             local function double(n) return n * 2 end\n\
             local M = { doubled = double(\"nope\") }\n\
             ---@class bad.Thing\n\
             return M\n",
        )]);
        assert!(surfaces.by_module().contains_key("bad"));
        assert!(surfaces.types()[0].classes.contains_key("bad.Thing"));
    }

    #[test]
    fn the_first_file_claiming_a_module_name_wins() {
        // `pl.lua` sorts before `pl/init.lua`, which is also the order
        // `resolve_candidates` tries them in.
        let mut flat = rock("pl", "---@class pl.Flat\nreturn { flat = true }\n");
        flat.label = "lua_modules/share/lua/5.4/pl.lua".to_owned();
        let mut init = rock("pl", "---@class pl.Init\nreturn { init = true }\n");
        init.label = "lua_modules/share/lua/5.4/pl/init.lua".to_owned();
        init.path = PathBuf::from("/proj/lua_modules/share/lua/5.4/pl/init.lua");

        let surfaces = harvest_of(&[flat, init]);
        assert_eq!(surfaces.by_module().len(), 1);
        // Both files' classes still contribute — only the *module name* is
        // exclusive, and both paths keep their own export.
        assert_eq!(surfaces.types().len(), 2);
        assert_eq!(surfaces.by_path().len(), 2);
    }

    #[test]
    fn a_rock_with_no_return_contributes_its_types_but_no_export() {
        let surfaces = harvest_of(&[rock("globalish", "---@class g.Thing\n---@field n number\n")]);
        assert!(surfaces.by_module().is_empty());
        assert!(surfaces.by_path().is_empty());
        assert_eq!(surfaces.types().len(), 1);
        assert!(!surfaces.is_empty());
    }

    #[test]
    fn harvesting_nothing_yields_an_empty_surface_set() {
        let surfaces = harvest_of(&[]);
        assert!(surfaces.is_empty());
        assert!(surfaces.types().is_empty());
        assert!(surfaces.by_module().is_empty());
        assert!(surfaces.skipped().is_empty());
    }

    /// Strict-check `src` against the stdlib plus the harvested surfaces of
    /// `sources`, layered the way `check_cmd` layers them.
    fn consumer_codes(sources: &[RockModule], src: &str) -> Vec<String> {
        let surfaces = harvest_of(sources);
        let ambient = stdlib_defs(Dialect::Lua54)
            .with_project_types(std::iter::empty())
            .with_rock_types(surfaces.types());
        let parse = lua::parse(src, Dialect::Lua54);
        crate::check_file_with_ambient(
            &parse,
            "src/main.lua",
            crate::Strictness::Strict,
            Dialect::Lua54,
            Some(&ambient),
        )
        .iter()
        .map(|d| d.code.to_string())
        .collect()
    }

    #[test]
    fn a_harvested_class_is_enforced_in_a_consuming_file() {
        // The end-to-end point of #30, at the crate boundary: the rock's class
        // resolves in a consumer's ambient scope and its fields are enforced.
        assert_eq!(
            consumer_codes(
                &[rock("mylib", ANNOTATED)],
                "---@type mylib.Point\nlocal p = { x = 1, y = \"no\" }\nreturn p\n",
            ),
            ["LB0300"]
        );
        // …and the same annotation without the rock is an unknown type name.
        assert_eq!(
            consumer_codes(
                &[],
                "---@type mylib.Point\nlocal p = { x = 1, y = \"no\" }\nreturn p\n",
            ),
            ["LB0305"]
        );
    }

    #[test]
    fn with_rock_types_leaves_a_name_a_project_file_already_declared_untouched() {
        // The project declares `mylib.Point` with only `x`. The rock's
        // two-field version must not union its `y` back in, or `[types] defs`
        // would stop being an escape hatch.
        let surfaces = harvest_of(&[rock("mylib", ANNOTATED)]);
        let project = crate::module_surface(
            &lua::parse(
                "---@class mylib.Point\n---@field x number\n",
                Dialect::Lua54,
            ),
            "defs.lua",
            None,
        );
        let ambient = stdlib_defs(Dialect::Lua54)
            .with_project_types([&project.types])
            .with_rock_types(surfaces.types());
        let parse = lua::parse(
            "---@type mylib.Point\nlocal p = { x = 1 }\nreturn p\n",
            Dialect::Lua54,
        );
        let diags = crate::check_file_with_ambient(
            &parse,
            "src/main.lua",
            crate::Strictness::Strict,
            Dialect::Lua54,
            Some(&ambient),
        );
        assert!(diags.is_empty(), "`x` alone must be complete: {diags:?}");
    }

    #[test]
    fn with_rock_types_fills_only_unclaimed_names() {
        let surfaces = harvest_of(&[
            rock("a", "---@class shared.Thing\n---@field from_a number\n"),
            rock("b", "---@class shared.Thing\n---@field from_b number\n"),
            rock("c", "---@class other.Thing\n---@field n number\n"),
        ]);
        // Among rocks it is first-wins too (path-sorted, silently — the user
        // declared neither side and cannot act on a warning about it).
        let ambient = stdlib_defs(Dialect::Lua54)
            .with_project_types(std::iter::empty())
            .with_rock_types(surfaces.types());
        let clean = lua::parse(
            "---@type shared.Thing\nlocal t = { from_a = 1 }\nreturn t\n",
            Dialect::Lua54,
        );
        assert!(
            crate::check_file_with_ambient(
                &clean,
                "m.lua",
                crate::Strictness::Strict,
                Dialect::Lua54,
                Some(&ambient),
            )
            .is_empty(),
            "the first rock's declaration wins"
        );
        // The unrelated name from the third rock is present too.
        let other = lua::parse(
            "---@type other.Thing\nlocal t = { n = 1 }\nreturn t\n",
            Dialect::Lua54,
        );
        assert!(
            crate::check_file_with_ambient(
                &other,
                "m.lua",
                crate::Strictness::Strict,
                Dialect::Lua54,
                Some(&ambient),
            )
            .is_empty()
        );
    }

    #[test]
    fn a_harvested_alias_is_expandable_in_a_consuming_file() {
        assert_eq!(
            consumer_codes(
                &[rock("ids", "---@alias rock.Id integer\n")],
                "---@param id rock.Id\nlocal function f(id) end\nf(\"no\")\nreturn f\n",
            ),
            ["LB0300"]
        );
    }

    #[test]
    fn a_harvested_enum_is_nameable_in_a_consuming_file() {
        let codes = consumer_codes(
            &[rock(
                "modes",
                "---@enum rock.Mode\nlocal Mode = { read = 1, write = 2 }\nreturn Mode\n",
            )],
            "---@type rock.Mode\nlocal m = 1\nreturn m\n",
        );
        assert!(!codes.contains(&"LB0305".to_owned()), "{codes:?}");
    }
}

//! Static `require` name → file resolution (SPEC.md §7).
//!
//! # Algorithm
//!
//! A dotted module name `a.b.c` maps to the relative path `a/b/c`; for each
//! search location, both `<path>.lua` and `<path>/init.lua` are tried, in
//! that order. Search locations, in order:
//!
//! 1. the project root: `<root>/a/b/c.lua`, `<root>/a/b/c/init.lua`;
//! 2. the project source tree: `<root>/src/a/b/c.lua`,
//!    `<root>/src/a/b/c/init.lua`;
//! 3. dependencies, in two layouts (both live — the first existing file
//!    wins, flat first so nothing that resolved before resolves elsewhere
//!    now).
//!
//! ## Dependency layout A — flat package directories
//!
//! The first name segment selects the package directory
//! `<root>/lua_modules/<a>/`, and the remaining segments resolve inside it,
//! its `src/` tree first:
//!
//! - `require "pkg"` → `lua_modules/pkg/src/init.lua`,
//!   `lua_modules/pkg/init.lua`;
//! - `require "pkg.x.y"` → `lua_modules/pkg/src/x/y.lua`,
//!   `lua_modules/pkg/src/x/y/init.lua`, `lua_modules/pkg/x/y.lua`,
//!   `lua_modules/pkg/x/y/init.lua`.
//!
//! This is the layout a sibling checkout or a hand-vendored package has, and
//! the one a project-relative path dependency points at.
//!
//! ## Dependency layout B — a luarocks tree
//!
//! `luarocks install --tree lua_modules <rock>` (README "Using
//! dependencies") writes the rock's Lua sources into a versioned
//! `share/lua/<X.Y>/` directory, mirroring the interpreter's own
//! `package.path`. The *whole* dotted name maps under it:
//!
//! - `require "pl.tablex"` → `lua_modules/share/lua/<X.Y>/pl/tablex.lua`,
//!   `lua_modules/share/lua/<X.Y>/pl/tablex/init.lua`.
//!
//! `<X.Y>` is the version directory of the dialect resolution is performed
//! for — `5.1`, `5.2`, `5.3`, `5.4`, and `5.1` for LuaJIT, which shares
//! 5.1's install prefix. Only that one version directory is searched: a tree
//! populated for a different interpreter is not this build's tree.
//!
//! ## C modules are not resolvable
//!
//! A luarocks tree also holds compiled C modules at
//! `lua_modules/lib/lua/<X.Y>/<name>.so` (`.dll` on Windows) and rock
//! metadata under `lua_modules/lib/luarocks/rocks-<X.Y>/`. Neither is
//! searched. A `require` naming a C module therefore resolves nowhere and is
//! treated exactly like any other unresolved name — left as a runtime
//! `require` in the bundle, with no diagnostic. That is the correct outcome
//! and not a gap to close: a shared object cannot be inlined into a text
//! bundle, so the module has to stay external and be shipped alongside it.
//!
//! The first existing file wins. Names that resolve nowhere are treated as
//! *external* (runtime `require`, e.g. C modules or stdlib-adjacent
//! libraries) and the call site is left untouched in the bundle.
//!
//! Resolved paths are canonicalized so the same file reached through two
//! spellings is bundled once (module identity is the file, its map key is
//! the first require string that reached it).

use std::path::{Path, PathBuf};

use luabox_syntax::Dialect;

/// The `share/lua/<X.Y>` / `lib/lua/<X.Y>` version directory a luarocks tree
/// installs `dialect`'s modules under. LuaJIT is ABI- and path-compatible
/// with 5.1 and shares its prefix, which is what luarocks itself does.
///
/// Re-exported as [`crate::rocks_version_dir`]: the rock-tree *type harvest*
/// (#30) walks that same directory to read the installed sources' LuaCATS
/// annotations, and must look in exactly the directory `require` resolution
/// searches or the editor would see surfaces the checker cannot resolve.
/// Distribution (`luabox-manifest`) owns the walk but never sees a `Dialect`
/// (SPEC.md §16), so the caller passes this string in.
#[must_use]
pub fn rocks_version_dir(dialect: Dialect) -> &'static str {
    match dialect {
        Dialect::Lua51 | Dialect::LuaJit => "5.1",
        Dialect::Lua52 => "5.2",
        Dialect::Lua53 => "5.3",
        Dialect::Lua54 => "5.4",
    }
}

/// Resolve `module` against the project rooted at `root`. `None` means
/// "external": no file in the project or its `lua_modules/` provides it.
///
/// `dialect` selects the luarocks version directory (see the module docs):
/// the build target for the bundler, the project edition everywhere else.
///
/// Re-exported as [`crate::resolve_module`] so front-ends that need the
/// bundler's exact `require` path-mapping (e.g. `luabox check`'s cross-file
/// type resolution, #85) share this one algorithm rather than re-deriving
/// it.
pub fn resolve(root: &Path, module: &str, dialect: Dialect) -> Option<PathBuf> {
    resolve_candidates(root, module, dialect)
        .into_iter()
        .find(|c| c.is_file())
        .map(|c| c.canonicalize().unwrap_or(c))
}

/// The ordered candidate file paths `module` may resolve to under `root`, in
/// SPEC.md §7 priority order (project root, then `src/`, then the flat
/// `lua_modules/<pkg>/` tree — `src/` first — then the luarocks
/// `lua_modules/share/lua/<X.Y>/` tree). Empty when `module` is not a legal
/// module name (empty, or a segment that is empty or `..`).
///
/// This is the single source of the resolution *ordering*. Two front-ends
/// consume it with different existence tests, and therefore can never disagree
/// on which file a `require` names:
///
/// - [`resolve`] (this crate's bundler and `luabox check`) picks the first
///   candidate that exists on disk;
/// - the incremental database (`luabox-db`, behind the LSP) picks the first
///   candidate present in its in-memory project file set.
///
/// Before this was shared, `luabox-db` approximated resolution by trailing-path
/// suffix match, so a module buried at `lib/util/helper.lua` was reachable as
/// `require("helper")` in the editor but not under `luabox check` — the exact
/// silent divergence this factoring makes structurally impossible.
pub fn resolve_candidates(root: &Path, module: &str, dialect: Dialect) -> Vec<PathBuf> {
    // Reject shapes that cannot be a module name (empty segments) or that
    // would escape the project tree (`..`, absolute-ish names).
    if module.is_empty() || module.split('.').any(|seg| seg.is_empty() || seg == "..") {
        return Vec::new();
    }

    let rel = module.replace('.', "/");
    let mut candidates: Vec<PathBuf> = Vec::new();
    for base in [root.to_path_buf(), root.join("src")] {
        candidates.push(base.join(format!("{rel}.lua")));
        candidates.push(base.join(rel.clone()).join("init.lua"));
    }

    let modules = root.join("lua_modules");

    let (first, rest) = match module.split_once('.') {
        Some((first, rest)) => (first, Some(rest)),
        None => (module, None),
    };
    let pkg = modules.join(first);
    match rest {
        None => {
            candidates.push(pkg.join("src").join("init.lua"));
            candidates.push(pkg.join("init.lua"));
        }
        Some(rest) => {
            let rel = rest.replace('.', "/");
            for base in [pkg.join("src"), pkg] {
                candidates.push(base.join(format!("{rel}.lua")));
                candidates.push(base.join(rel.clone()).join("init.lua"));
            }
        }
    }

    // A luarocks tree: the whole dotted name under the dialect's version
    // directory, exactly where the interpreter's own `package.path` would
    // look. C modules (`lib/lua/<X.Y>/`) are deliberately not searched — see
    // the module docs.
    let share = modules
        .join("share")
        .join("lua")
        .join(rocks_version_dir(dialect));
    candidates.push(share.join(format!("{rel}.lua")));
    candidates.push(share.join(rel).join("init.lua"));

    candidates
}

#[cfg(test)]
mod tests {
    // test code — panics document assumptions
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    /// Root-relative candidate strings, forward-slashed — stable to assert on.
    fn rel_candidates(module: &str, dialect: Dialect) -> Vec<String> {
        let root = Path::new("/proj");
        resolve_candidates(root, module, dialect)
            .into_iter()
            .map(|c| {
                c.strip_prefix(root)
                    .unwrap_or(&c)
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect()
    }

    #[test]
    fn the_luarocks_share_tree_is_searched_for_the_whole_dotted_name() {
        let candidates = rel_candidates("pl.tablex", Dialect::Lua54);
        assert!(
            candidates.contains(&"lua_modules/share/lua/5.4/pl/tablex.lua".to_string()),
            "{candidates:?}"
        );
        assert!(
            candidates.contains(&"lua_modules/share/lua/5.4/pl/tablex/init.lua".to_string()),
            "{candidates:?}"
        );
    }

    #[test]
    fn a_single_segment_name_also_reaches_the_luarocks_share_tree() {
        let candidates = rel_candidates("inifile", Dialect::Lua53);
        assert!(
            candidates.contains(&"lua_modules/share/lua/5.3/inifile.lua".to_string()),
            "{candidates:?}"
        );
        assert!(
            candidates.contains(&"lua_modules/share/lua/5.3/inifile/init.lua".to_string()),
            "{candidates:?}"
        );
    }

    #[test]
    fn luajit_shares_the_five_one_version_directory() {
        // luarocks installs LuaJIT rocks under the 5.1 prefix; resolution has
        // to look where the rock actually landed, not under a `luajit/` dir.
        let jit = rel_candidates("pl.tablex", Dialect::LuaJit);
        let five_one = rel_candidates("pl.tablex", Dialect::Lua51);
        assert_eq!(jit, five_one);
        assert!(
            jit.contains(&"lua_modules/share/lua/5.1/pl/tablex.lua".to_string()),
            "{jit:?}"
        );
    }

    #[test]
    fn every_dialect_maps_to_its_own_version_directory() {
        for (dialect, dir) in [
            (Dialect::Lua51, "5.1"),
            (Dialect::Lua52, "5.2"),
            (Dialect::Lua53, "5.3"),
            (Dialect::Lua54, "5.4"),
            (Dialect::LuaJit, "5.1"),
        ] {
            assert_eq!(rocks_version_dir(dialect), dir);
            let candidates = rel_candidates("x", dialect);
            assert!(
                candidates.contains(&format!("lua_modules/share/lua/{dir}/x.lua")),
                "{dialect:?}: {candidates:?}"
            );
        }
    }

    #[test]
    fn only_the_requested_dialects_version_directory_is_searched() {
        let candidates = rel_candidates("pl.tablex", Dialect::Lua54);
        for other in ["5.1", "5.2", "5.3"] {
            assert!(
                !candidates
                    .iter()
                    .any(|c| c.contains(&format!("share/lua/{other}/"))),
                "{other} must not be searched under a 5.4 build: {candidates:?}"
            );
        }
    }

    #[test]
    fn the_flat_layout_still_wins_over_the_luarocks_tree() {
        let candidates = rel_candidates("pkg.x", Dialect::Lua54);
        let flat = candidates
            .iter()
            .position(|c| c == "lua_modules/pkg/src/x.lua")
            .expect("flat candidate present");
        let rocks = candidates
            .iter()
            .position(|c| c == "lua_modules/share/lua/5.4/pkg/x.lua")
            .expect("luarocks candidate present");
        assert!(
            flat < rocks,
            "back-compat: the flat layout is tried first ({candidates:?})"
        );
    }

    #[test]
    fn the_project_tree_still_wins_over_every_dependency_layout() {
        let candidates = rel_candidates("pl.tablex", Dialect::Lua54);
        assert_eq!(
            &candidates[..4],
            &[
                "pl/tablex.lua",
                "pl/tablex/init.lua",
                "src/pl/tablex.lua",
                "src/pl/tablex/init.lua",
            ]
        );
    }

    #[test]
    fn c_module_and_rock_metadata_directories_are_never_candidates() {
        // `lib/lua/<X.Y>/*.so` cannot be inlined into a text bundle, so it is
        // not searched: such requires stay external, like any unresolved name.
        let candidates = rel_candidates("lfs", Dialect::Lua54);
        assert!(
            !candidates.iter().any(|c| c.contains("lua_modules/lib/")),
            "{candidates:?}"
        );
    }

    #[test]
    fn an_illegal_module_name_has_no_candidates_in_any_layout() {
        for bad in ["", ".", "a..b", "..", "a.."] {
            assert!(
                resolve_candidates(Path::new("/proj"), bad, Dialect::Lua54).is_empty(),
                "`{bad}` must not resolve"
            );
        }
    }

    #[test]
    fn resolve_finds_a_file_in_the_luarocks_tree_on_disk() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let file = tmp
            .path()
            .join("lua_modules/share/lua/5.4/pl")
            .join("tablex.lua");
        std::fs::create_dir_all(file.parent().expect("has a parent")).expect("mkdir");
        std::fs::write(&file, "return {}\n").expect("write");

        let found = resolve(tmp.path(), "pl.tablex", Dialect::Lua54).expect("resolves");
        assert_eq!(found, file.canonicalize().expect("canonicalize"));

        // The same tree under a 5.1 build is not this build's tree.
        assert!(resolve(tmp.path(), "pl.tablex", Dialect::Lua51).is_none());
    }
}

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
//! 3. dependencies: the first name segment selects the package directory
//!    `<root>/lua_modules/<a>/` (the layout `luabox install` materializes),
//!    and the remaining segments resolve inside it, its `src/` tree first:
//!    - `require "pkg"` → `lua_modules/pkg/src/init.lua`,
//!      `lua_modules/pkg/init.lua`;
//!    - `require "pkg.x.y"` → `lua_modules/pkg/src/x/y.lua`,
//!      `lua_modules/pkg/src/x/y/init.lua`, `lua_modules/pkg/x/y.lua`,
//!      `lua_modules/pkg/x/y/init.lua`.
//!
//! The first existing file wins. Names that resolve nowhere are treated as
//! *external* (runtime `require`, e.g. C modules or stdlib-adjacent
//! libraries) and the call site is left untouched in the bundle.
//!
//! Resolved paths are canonicalized so the same file reached through two
//! spellings is bundled once (module identity is the file, its map key is
//! the first require string that reached it).

use std::path::{Path, PathBuf};

/// Resolve `module` against the project rooted at `root`. `None` means
/// "external": no file in the project or its `lua_modules/` provides it.
///
/// Re-exported as [`crate::resolve_module`] so front-ends that need the
/// bundler's exact `require` path-mapping (e.g. `luabox check`'s cross-file
/// type resolution, #85) share this one algorithm rather than re-deriving
/// it.
pub fn resolve(root: &Path, module: &str) -> Option<PathBuf> {
    // Reject shapes that cannot be a module name (empty segments) or that
    // would escape the project tree (`..`, absolute-ish names).
    if module.is_empty() || module.split('.').any(|seg| seg.is_empty() || seg == "..") {
        return None;
    }

    let rel = module.replace('.', "/");
    let mut candidates: Vec<PathBuf> = Vec::new();
    for base in [root.to_path_buf(), root.join("src")] {
        candidates.push(base.join(format!("{rel}.lua")));
        candidates.push(base.join(rel.clone()).join("init.lua"));
    }

    let (first, rest) = match module.split_once('.') {
        Some((first, rest)) => (first, Some(rest)),
        None => (module, None),
    };
    let pkg = root.join("lua_modules").join(first);
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

    candidates
        .into_iter()
        .find(|c| c.is_file())
        .map(|c| c.canonicalize().unwrap_or(c))
}

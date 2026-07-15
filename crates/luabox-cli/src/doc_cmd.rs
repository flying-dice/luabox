//! `luabox doc [--open]` — a static documentation site from LuaCATS
//! annotations (SPEC.md §13).
//!
//! Pipeline:
//!
//! 1. **Discover** the project (nearest `luabox.toml`, like `luabox check`)
//!    and walk its `.lua` files.
//! 2. **Harvest** the model (`doc_cmd::model`): per-file modules with
//!    functions/classes/aliases/enums from the LuaCATS harvest.
//! 3. **Render** (`doc_cmd::render`) a zero-install static site into
//!    `<root>/doc/` (a sibling of the `[build] out` directory): one page
//!    per module and per class/type, an index with a client-side search
//!    box over an embedded JSON index, cross-links through one global
//!    name table, inline CSS/JS only — no external assets.
//! 4. `--open` launches the generated `index.html` in the default browser.
//!
//! Doc text renders through the minimal markdown renderer
//! (`doc_cmd::markdown`). Running doc examples as tested blocks is *not*
//! implemented; fenced code blocks in doc text render as plain code.

mod markdown;
mod model;
mod render;

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use anyhow::Context;
use luabox_resolve::manifest::Manifest;

use crate::check_cmd;
use model::DocModel;

/// Execute `luabox doc` from `cwd`.
pub fn run(cwd: &Path, open: bool) -> anyhow::Result<()> {
    let project = check_cmd::discover(cwd)?;
    let lua_files =
        crate::project::collect_lua_files(&project.root, project.out_dir.as_deref(), true)?;
    let package = manifest_facts(&project.root);

    let mut modules = Vec::new();
    for path in &lua_files {
        let rel = crate::project::display_rel(path, &project.root);
        let source = fs::read_to_string(path).with_context(|| format!("cannot read `{rel}`"))?;
        let name = model::module_name(&rel);
        modules.push(model::lua_module(&name, &source, project.dialect));
    }
    harvest_def_modules(&project, &mut modules);

    let model = DocModel { package, modules };

    let out_dir = project.root.join("doc");
    fs::create_dir_all(&out_dir)
        .with_context(|| format!("cannot create `{}`", out_dir.display()))?;
    let pages = render::pages(&model);
    for (name, html) in &pages {
        let path = out_dir.join(name);
        fs::write(&path, html).with_context(|| format!("cannot write `{}`", path.display()))?;
    }
    eprintln!(
        "doc: generated {} pages into `{}`",
        pages.len(),
        crate::project::display_rel(&out_dir, &project.root)
    );

    if open {
        open_in_browser(&out_dir.join("index.html"));
    }
    Ok(())
}

/// Fold `---@meta` def-file classes into `modules` so an interface that
/// lives only in a def (never reopened by a real carrier — SPEC.md §3's
/// `[types] defs`, e.g. `examples/geometry`'s `geometry.Shape`) still gets a
/// `class.<name>.html` page and can show implementors (#87).
///
/// `project::collect_lua_files` (with `exclude_d_lua`) deliberately excludes `*.d.lua` from the
/// project's own `.lua` files (they are ambient, not project source), so
/// without this step those classes are invisible to `luabox doc` — the gap
/// this task set out to close. The def files are resolved with the exact
/// same `check_cmd::resolve_project_defs`/`dep_defs` the typechecker uses,
/// so "does this class have a page" tracks "is this class ambient" exactly.
///
/// A def file's `---@class` can also be the same class a real module
/// reopens (`geometry.Circle` in both `defs/geometry.d.lua` and
/// `src/circle.lua`, merged by the typechecker but *not* by this doc
/// model — `model::classes_by_name`'s documented MVP gap). Giving both
/// declarations a `class.Circle.html` page would silently let one clobber
/// the other when pages are written by file name, so a class already known
/// by name (from a real module, or an earlier def file) is dropped here
/// instead: the real carrier's page — richer, with methods — wins, and the
/// def only contributes classes with no carrier of their own.
fn harvest_def_modules(project: &check_cmd::Project, modules: &mut Vec<model::Module>) {
    let mut known: BTreeSet<String> = modules
        .iter()
        .flat_map(|m| m.classes.iter().map(|c| c.name.clone()))
        .collect();

    let (mut defs, _diags) = check_cmd::resolve_project_defs(&project.root, &project.defs);
    defs.extend(project.dep_defs.iter().cloned());

    for def in &defs {
        let name = def_module_name(&def.file);
        let mut module = model::lua_module(&name, &def.text, project.dialect);
        module.classes.retain(|c| known.insert(c.name.clone()));
        if !module.classes.is_empty()
            || !module.aliases.is_empty()
            || !module.enums.is_empty()
            || !module.functions.is_empty()
        {
            modules.push(module);
        }
    }
}

/// A def file's display label (`defs/geometry.d.lua`, or a
/// dependency-prefixed `geometry/defs/geometry.d.lua`) to a doc module name:
/// the last path segment, `.d.lua` stripped. A directory-style def package
/// (`defs/<name>/*.lua`, multiple files) collapses every file in it to its
/// own stem, dropping the shared directory name — an accepted MVP gap, same
/// spirit as `classes_by_name`'s "later declaration wins".
fn def_module_name(file: &str) -> String {
    let stem = file.rsplit('/').next().unwrap_or(file);
    stem.strip_suffix(".d.lua").unwrap_or(stem).to_string()
}

/// The package name from the manifest (a default when the project is
/// manifest-less).
fn manifest_facts(root: &Path) -> String {
    let fallback = || {
        root.file_name().map_or_else(
            || "package".to_string(),
            |n| n.to_string_lossy().into_owned(),
        )
    };
    let Ok(text) = fs::read_to_string(root.join("luabox.toml")) else {
        return fallback();
    };
    let Ok(manifest) = Manifest::parse(&text) else {
        return fallback();
    };
    manifest.package.name
}

/// Open `index` in the platform's default browser. Best-effort: a failure
/// to spawn the opener is reported but never fails the command — the site
/// was already generated.
fn open_in_browser(index: &Path) {
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("cmd")
        .args(["/C", "start", ""])
        .arg(index)
        .spawn();
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(index).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let result = std::process::Command::new("xdg-open").arg(index).spawn();

    if let Err(error) = result {
        eprintln!(
            "doc: generated site, but could not open `{}`: {error}",
            index.display()
        );
    }
}

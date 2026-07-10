//! `luabox doc [--open]` — a static documentation site from LuaCATS
//! annotations and `.luab` shape declarations (SPEC.md §13).
//!
//! Pipeline:
//!
//! 1. **Discover** the project (nearest `luabox.toml`, like `luabox check`)
//!    and walk its `.lua` and `.luab` files, plus any `.luab` modules under the
//!    manifest's `[types] shape-paths`.
//! 2. **Harvest** the model (`doc_cmd::model`): per-file modules with
//!    functions/classes/aliases/enums from the LuaCATS harvest, per-file
//!    shape modules with structs/traits/impls/aliases and their `///` docs.
//! 3. **Render** (`doc_cmd::render`) a zero-install static site into
//!    `<root>/doc/` (a sibling of the `[build] out` directory): one page
//!    per module and per class/struct/trait, an index with a client-side
//!    search box over an embedded JSON index, cross-links through one
//!    global name table, inline CSS/JS only — no external assets.
//! 4. `--open` launches the generated `index.html` in the default browser.
//!
//! Doc text renders through the minimal markdown renderer
//! (`doc_cmd::markdown`). Running doc examples as tested blocks under
//! `luabox test --doc` (SPEC.md §13) is *not* implemented yet; fenced code
//! blocks in doc text render as plain code.

mod markdown;
mod model;
mod render;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;
use luabox_resolve::manifest::Manifest;

use crate::check_cmd;
use model::DocModel;

/// Execute `luabox doc` from `cwd`.
pub fn run(cwd: &Path, open: bool) -> anyhow::Result<()> {
    let project = check_cmd::discover(cwd)?;
    let (lua_files, lb_files) = check_cmd::collect_files(&project)?;
    let (package, shape_paths) = manifest_facts(&project.root);

    // `.luab` modules: project files plus the manifest's shape-paths tiers,
    // deduplicated (a shape path inside the project root is walked twice).
    let mut lb_set: BTreeSet<PathBuf> = lb_files.into_iter().collect();
    for dir in &shape_paths {
        collect_lb(dir, &mut lb_set);
    }

    let mut modules = Vec::new();
    let mut impls = Vec::new();
    for path in &lua_files {
        let rel = check_cmd::display_rel(path, &project.root);
        let source = fs::read_to_string(path).with_context(|| format!("cannot read `{rel}`"))?;
        let name = model::module_name(&rel);
        let (module, module_impls) = model::lua_module(&name, &source, project.dialect);
        modules.push(module);
        impls.extend(module_impls);
    }

    let mut shape_modules = Vec::new();
    for path in &lb_set {
        let rel = check_cmd::display_rel(path, &project.root);
        let source = fs::read_to_string(path).with_context(|| format!("cannot read `{rel}`"))?;
        // Shape modules resolve by file stem (SHAPES.md §6).
        let name = path
            .file_stem()
            .map_or_else(|| rel.clone(), |s| s.to_string_lossy().into_owned());
        let module = model::shape_module(&name, &source);
        impls.extend(module.impls.clone());
        shape_modules.push(module);
    }

    let model = DocModel {
        package,
        modules,
        shape_modules,
        impls,
    };

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
        check_cmd::display_rel(&out_dir, &project.root)
    );

    if open {
        open_in_browser(&out_dir.join("index.html"));
    }
    Ok(())
}

/// The package name and absolute shape-path directories from the manifest
/// (defaults when the project is manifest-less).
fn manifest_facts(root: &Path) -> (String, Vec<PathBuf>) {
    let fallback = || {
        root.file_name().map_or_else(
            || "package".to_string(),
            |n| n.to_string_lossy().into_owned(),
        )
    };
    let Ok(text) = fs::read_to_string(root.join("luabox.toml")) else {
        return (fallback(), Vec::new());
    };
    let Ok(manifest) = Manifest::parse(&text) else {
        return (fallback(), Vec::new());
    };
    let shape_paths = manifest
        .types
        .shape_paths
        .iter()
        .map(|p| root.join(p))
        .collect();
    (manifest.package.name, shape_paths)
}

/// Collect every `.luab` file under `dir`, recursively, into `out`.
fn collect_lb(dir: &Path, out: &mut BTreeSet<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_lb(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("luab") {
            out.insert(path);
        }
    }
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

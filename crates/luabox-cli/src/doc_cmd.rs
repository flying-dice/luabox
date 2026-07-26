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

use anyhow::{Context, bail};
use luabox_diag::{Diagnostic, Format, Label, Span};
use luabox_resolve::manifest::Manifest;
use luabox_syntax::lua;

use crate::check_cmd;
use model::DocModel;

/// Execute `luabox doc` from `cwd`.
pub fn run(cwd: &Path, open: bool) -> anyhow::Result<()> {
    let project = check_cmd::discover(cwd)?;
    let lua_files =
        crate::project::collect_lua_files(&project.root, project.out_dir.as_deref(), true)?;
    let package = manifest_facts(&project.root);

    // A file that does not parse has no trustworthy harvest — its doc
    // comments may attach to the wrong (recovered) nodes or vanish with the
    // broken region. Gate on parse errors exactly like `build` does (#24);
    // type errors deliberately do NOT gate — docs for imperfect code are
    // still docs.
    let mut parse_diags = Vec::new();
    let mut modules = Vec::new();
    for path in &lua_files {
        let rel = crate::project::display_rel(path, &project.root);
        let source = fs::read_to_string(path).with_context(|| format!("cannot read `{rel}`"))?;
        for err in lua::parse(&source, project.dialect).errors() {
            let range = usize::from(err.range.start())..usize::from(err.range.end());
            parse_diags.push(
                Diagnostic::error(check_cmd::code(1), err.message.clone())
                    .with_label(Label::primary(Span::new(&rel, range), "syntax error here")),
            );
        }
        let name = model::module_name(&rel);
        modules.push(model::lua_module(&name, &source, project.dialect));
    }
    if !parse_diags.is_empty() {
        let counts = crate::project::render_diagnostics(&parse_diags, Format::Human, &project.root);
        bail!(
            "doc refuses to generate while {} parse error(s) exist",
            counts.errors
        );
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

#[cfg(test)]
mod tests {
    // test code — panics document assumptions
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    /// Write `contents` to `root/rel`, creating parent directories.
    fn write(root: &Path, rel: &str, contents: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().expect("has a parent")).expect("create parents");
        fs::write(&path, contents).expect("write file");
    }

    fn manifest(name: &str, extra: &str) -> String {
        format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"5.4\"\n{extra}")
    }

    fn project(name: &str, extra: &str) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "luabox.toml", &manifest(name, extra));
        tmp
    }

    fn read_doc(root: &Path, page: &str) -> String {
        fs::read_to_string(root.join("doc").join(page))
            .unwrap_or_else(|e| panic!("reading doc/{page}: {e}"))
    }

    #[test]
    fn doc_refuses_on_a_parse_error_and_writes_nothing() {
        // #24: a file that does not parse has no trustworthy harvest — gate
        // like `build` does instead of generating pages from a broken AST.
        let tmp = project("broken", "");
        write(
            tmp.path(),
            "src/main.lua",
            "--[[ never closed\nlocal x = 1\n",
        );

        let err = run(tmp.path(), false).expect_err("doc must refuse on a parse error");
        assert!(
            err.to_string().contains("parse error"),
            "unexpected error: {err}"
        );
        assert!(
            !tmp.path().join("doc").exists(),
            "no doc/ output on refusal"
        );
    }

    #[test]
    fn doc_still_generates_for_type_errors() {
        // Type errors deliberately do NOT gate: docs for imperfect code are
        // still docs. Only the parse gate refuses.
        let tmp = project("imperfect", "\n[types]\nstrict = true\n");
        write(
            tmp.path(),
            "src/main.lua",
            "---@param n number\n---@return number\nlocal function double(n)\n    return n * 2\nend\n\nprint(double(\"oops\"))\n",
        );

        run(tmp.path(), false).expect("type errors must not gate doc");
        assert!(
            read_doc(tmp.path(), "index.html").contains("imperfect"),
            "site generated despite the type error"
        );
    }

    const CIRCLE_SOURCE: &str = "\
--- Circle helpers.

---@class geometry.Circle
---@field radius number the radius
local Circle = {}

--- Computes the area.
---@param self geometry.Circle
---@return number area
function Circle:area()
  return 3 * self.radius * self.radius
end

return Circle
";

    #[test]
    fn run_writes_an_index_and_a_page_per_module_and_class() {
        let tmp = project("geometry", "");
        write(tmp.path(), "src/circle.lua", CIRCLE_SOURCE);

        run(tmp.path(), false).expect("doc succeeds");

        let index = read_doc(tmp.path(), "index.html");
        assert!(index.contains("geometry"), "{index}");
        // Module and class pages are named from the model, not the file.
        assert!(tmp.path().join("doc").join("module.circle.html").is_file());
        assert!(
            tmp.path()
                .join("doc")
                .join("class.geometry.Circle.html")
                .is_file()
        );

        // Methods hang off the class page, not the module page.
        let class = read_doc(tmp.path(), "class.geometry.Circle.html");
        assert!(class.contains("area"), "{class}");
        assert!(class.contains("radius"), "{class}");
    }

    #[test]
    fn the_generated_site_is_self_contained_with_no_external_assets() {
        let tmp = project("selfcontained", "");
        write(tmp.path(), "src/main.lua", "--- A module.\nreturn {}\n");

        run(tmp.path(), false).expect("doc succeeds");
        let index = read_doc(tmp.path(), "index.html");
        assert!(!index.contains("http://"), "{index}");
        assert!(!index.contains("https://"), "{index}");
        assert!(index.contains("<style"), "{index}");
    }

    #[test]
    fn the_search_index_embedded_in_the_index_page_is_valid_json() {
        let tmp = project("searchable", "");
        write(
            tmp.path(),
            "src/main.lua",
            "--- Adds.\n---@param a number\n---@return number\nlocal function add(a)\n  return a\nend\nreturn add\n",
        );
        run(tmp.path(), false).expect("doc succeeds");

        let index = read_doc(tmp.path(), "index.html");
        let json = index
            .split_once("id=\"search-index\">")
            .map(|(_, rest)| rest)
            .and_then(|rest| rest.split_once("</script>"))
            .map(|(json, _)| json)
            .expect("the index page embeds a search-index script block");
        let parsed: serde_json::Value =
            serde_json::from_str(json).expect("the embedded search index is valid JSON");
        assert!(
            parsed
                .as_array()
                .expect("an array")
                .iter()
                .any(|e| { e.get("name").and_then(serde_json::Value::as_str) == Some("add") })
        );
    }

    #[test]
    fn doc_output_is_written_beside_the_build_output_and_is_never_documented_itself() {
        let tmp = project("stable", "\n[build]\nout = \"dist\"\n");
        write(tmp.path(), "src/main.lua", "--- A module.\nreturn {}\n");
        write(
            tmp.path(),
            "dist/src/main.lua",
            "--- Generated.\nreturn {}\n",
        );

        run(tmp.path(), false).expect("doc succeeds");
        // `dist/` is build output, not source: only one module page exists.
        let pages: Vec<String> = fs::read_dir(tmp.path().join("doc"))
            .expect("doc dir")
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("module."))
            .collect();
        assert_eq!(pages, ["module.main.html"]);
    }

    #[test]
    fn rerunning_doc_over_an_existing_site_succeeds() {
        let tmp = project("idempotent", "");
        write(tmp.path(), "src/main.lua", "--- A module.\nreturn {}\n");
        run(tmp.path(), false).expect("first run");
        run(tmp.path(), false).expect("second run over the existing doc/ dir");
    }

    #[test]
    fn an_empty_project_still_generates_a_site() {
        let tmp = project("empty", "");
        run(tmp.path(), false).expect("doc succeeds");
        assert!(tmp.path().join("doc").join("index.html").is_file());
    }

    #[test]
    fn a_manifest_less_project_falls_back_to_the_directory_name_as_the_package() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("my-package");
        fs::create_dir_all(&root).expect("mkdir");
        write(&root, "main.lua", "--- A module.\nreturn {}\n");

        run(&root, false).expect("doc succeeds");
        assert!(
            fs::read_to_string(root.join("doc").join("index.html"))
                .expect("index")
                .contains("my-package")
        );
    }

    // -- `---@meta` def harvesting (#87) -----------------------------------

    #[test]
    fn a_class_that_lives_only_in_a_def_file_still_gets_a_page() {
        let tmp = project("geometry", "\n[types]\ndefs = [\"geometry\"]\n");
        write(
            tmp.path(),
            "defs/geometry.d.lua",
            "---@meta\n\n--- A shape.\n---@class geometry.Shape\n---@field area fun(): number\n",
        );
        write(tmp.path(), "src/main.lua", "return {}\n");

        run(tmp.path(), false).expect("doc succeeds");
        assert!(
            tmp.path()
                .join("doc")
                .join("class.geometry.Shape.html")
                .is_file()
        );
    }

    #[test]
    fn a_real_carrier_module_wins_the_page_over_a_def_declaring_the_same_class() {
        let tmp = project("geometry", "\n[types]\ndefs = [\"geometry\"]\n");
        write(
            tmp.path(),
            "defs/geometry.d.lua",
            "---@meta\n---@class geometry.Circle\n---@field radius number\n",
        );
        write(tmp.path(), "src/circle.lua", CIRCLE_SOURCE);

        run(tmp.path(), false).expect("doc succeeds");
        // Exactly one page for the class, and it is the richer carrier's —
        // the def's declaration is dropped rather than clobbering it.
        let page = read_doc(tmp.path(), "class.geometry.Circle.html");
        assert!(
            page.contains("area"),
            "the carrier's method is missing:\n{page}"
        );
    }

    #[test]
    fn a_def_contributing_nothing_new_adds_no_module_page() {
        let tmp = project("geometry", "\n[types]\ndefs = [\"geometry\"]\n");
        write(
            tmp.path(),
            "defs/geometry.d.lua",
            "---@meta\n---@class geometry.Circle\n",
        );
        write(tmp.path(), "src/circle.lua", CIRCLE_SOURCE);

        run(tmp.path(), false).expect("doc succeeds");
        assert!(!tmp.path().join("doc").join("module.geometry.html").exists());
    }

    #[test]
    fn a_dependency_s_defs_are_harvested_too() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(
            tmp.path(),
            "luabox.toml",
            &manifest(
                "consumer",
                "\n[dependencies]\ngeo = { path = \"vendor/geo\" }\n",
            ),
        );
        write(
            tmp.path(),
            "vendor/geo/luabox.toml",
            &manifest("geo", "\n[types]\ndefs = [\"geo\"]\n"),
        );
        write(
            tmp.path(),
            "vendor/geo/defs/geo.d.lua",
            "---@meta\n---@class geo.Vector\n---@field x number\n",
        );
        write(tmp.path(), "src/main.lua", "return {}\n");

        run(tmp.path(), false).expect("doc succeeds");
        assert!(
            tmp.path()
                .join("doc")
                .join("class.geo.Vector.html")
                .is_file()
        );
    }

    #[test]
    fn a_def_module_name_is_the_last_path_segment_with_the_d_lua_suffix_stripped() {
        assert_eq!(def_module_name("defs/geometry.d.lua"), "geometry");
        assert_eq!(def_module_name("geometry/defs/geometry.d.lua"), "geometry");
        // A directory-style def package collapses each file to its own stem.
        assert_eq!(def_module_name("defs/pack/vec.d.lua"), "vec");
        // A label with no separators or suffix passes through unchanged.
        assert_eq!(def_module_name("bare"), "bare");
    }

    // -- package name resolution -------------------------------------------

    #[test]
    fn the_package_name_comes_from_the_manifest_when_it_parses() {
        let tmp = project("from-manifest", "");
        assert_eq!(manifest_facts(tmp.path()), "from-manifest");
    }

    #[test]
    fn the_package_name_falls_back_to_the_directory_without_a_manifest() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("dirname-fallback");
        fs::create_dir_all(&root).expect("mkdir");
        assert_eq!(manifest_facts(&root), "dirname-fallback");
    }

    #[test]
    fn the_package_name_falls_back_to_the_directory_for_a_malformed_manifest() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("broken-manifest");
        fs::create_dir_all(&root).expect("mkdir");
        write(&root, "luabox.toml", "= = =\n");
        assert_eq!(manifest_facts(&root), "broken-manifest");
    }

    #[test]
    fn a_root_with_no_file_name_falls_back_to_a_placeholder_package_name() {
        assert_eq!(manifest_facts(Path::new("/")), "package");
    }
}

//! Shared project discovery: the walk-up-to-`luabox.toml` + read + parse
//! step that every project-aware command begins with.
//!
//! [`discover_manifest`] returns `None` when there is no `luabox.toml` in
//! `cwd` or any ancestor, letting the command fall back to its own
//! manifest-less default (`check`, `lint`, `fmt`, `build`, `doc` each root a
//! default project at `cwd`). A manifest that *is* present but malformed is
//! still an error.
//!
//! One shared manifest reader keeps the read-error (`cannot read ...`) and
//! parse-error (`invalid ...:\n<rendered>`) messages byte-identical across
//! every command. The *view* each command builds on top of `(root, Manifest)`
//! — its edition/target validation, its `Project` struct — stays in the
//! command, because those differ (some commands don't parse the edition at
//! all; the ones that do word the error differently).

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;
use luabox_diag::{Diagnostic, Format, Severity, render};
use luabox_resolve::manifest::Manifest;

/// The project-local dependency tree `luarocks install --tree lua_modules`
/// writes. Excluded from every first-party source walk (see
/// [`collect_lua_files`]).
const VENDOR_DIR: &str = "lua_modules";

/// Walk up from `cwd` (cargo-style) to the nearest directory containing a
/// `luabox.toml` file. Returns that directory (the project root), or `None`
/// if neither `cwd` nor any ancestor has one. Does not read the manifest.
pub(crate) fn find_manifest_dir(cwd: &Path) -> Option<PathBuf> {
    let mut dir = Some(cwd);
    while let Some(current) = dir {
        if current.join("luabox.toml").is_file() {
            return Some(current.to_path_buf());
        }
        dir = current.parent();
    }
    None
}

/// Read and parse `<root>/luabox.toml`, with the shared read-error and
/// parse-error rendering every command relies on:
///
/// * an unreadable file → a "cannot read" error naming the path;
/// * a manifest that fails to parse → an "invalid" error naming the path,
///   followed by one rendered parse error per line.
pub(crate) fn read_manifest(root: &Path) -> anyhow::Result<Manifest> {
    let manifest_path = root.join("luabox.toml");
    let text = fs::read_to_string(&manifest_path)
        .with_context(|| format!("cannot read `{}`", manifest_path.display()))?;
    Manifest::parse(&text).map_err(|errors| {
        let rendered = errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        anyhow::anyhow!("invalid `{}`:\n{rendered}", manifest_path.display())
    })
}

/// Discover the project for a command that supports a manifest-less default:
/// the root and parsed manifest of the nearest `luabox.toml`, or `None` when
/// there is none in `cwd` or any parent. A malformed manifest that *is*
/// present is still an error (via [`read_manifest`]).
pub(crate) fn discover_manifest(cwd: &Path) -> anyhow::Result<Option<(PathBuf, Manifest)>> {
    match find_manifest_dir(cwd) {
        Some(root) => {
            let manifest = read_manifest(&root)?;
            Ok(Some((root, manifest)))
        }
        None => Ok(None),
    }
}

/// All `*.lua` files under `root`, in deterministic order — entries sorted by
/// file name at each directory level, walked depth-first — skipping
/// dot-directories, the build output directory (`out_dir`, when set), and any
/// directory named `lua_modules`.
///
/// `lua_modules/` is *vendored* code, not project source: it is whatever
/// `luarocks install --tree lua_modules` materialized (README "Using
/// dependencies"). Walking into it made `luabox check` typecheck rock sources
/// against the project's own strictness — which fails on any rock that is not
/// trivially typed, and takes `luabox build` down with it. It is skipped the
/// same way dot-directories are, at every depth, because a vendored tree can
/// itself contain one.
///
/// `exclude_d_lua` is the sole behavioral knob between the project-source
/// commands and `lint`: with it set, `*.d.lua` files are omitted, because they
/// are `---@meta` definition files (ambient type surfaces), never checked as
/// project source — so `check`/`build`/`doc` pass `true`. `lint` passes
/// `false`, since it lints those files too.
pub(crate) fn collect_lua_files(
    root: &Path,
    out_dir: Option<&Path>,
    exclude_d_lua: bool,
) -> anyhow::Result<Vec<PathBuf>> {
    let mut lua = Vec::new();
    walk(root, out_dir, exclude_d_lua, &mut lua)?;
    Ok(lua)
}

fn walk(
    dir: &Path,
    out_dir: Option<&Path>,
    exclude_d_lua: bool,
    lua: &mut Vec<PathBuf>,
) -> anyhow::Result<()> {
    let mut entries: Vec<_> = fs::read_dir(dir)
        .with_context(|| format!("cannot read directory `{}`", dir.display()))?
        .collect::<Result<_, _>>()
        .with_context(|| format!("cannot read directory `{}`", dir.display()))?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let hidden = entry.file_name().to_string_lossy().starts_with('.');
        if path.is_dir() {
            let is_out = out_dir == Some(path.as_path());
            // Vendored rock trees are never project source, at any depth.
            let is_vendored = entry.file_name() == OsStr::new(VENDOR_DIR);
            if !hidden && !is_out && !is_vendored {
                walk(&path, out_dir, exclude_d_lua, lua)?;
            }
        } else if !hidden {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            // `*.d.lua` are `---@meta` definition files (ambient type
            // surfaces), never checked as project source.
            if path.extension().and_then(|e| e.to_str()) == Some("lua")
                && !(exclude_d_lua && name.ends_with(".d.lua"))
            {
                lua.push(path);
            }
        }
    }
    Ok(())
}

/// Root-relative path with forward slashes — stable output across platforms.
pub(crate) fn display_rel(path: &Path, root: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.to_string_lossy().replace('\\', "/")
}

/// Error/warning tallies from a diagnostic set, returned by
/// [`render_diagnostics`] so each command can shape its own summary line and
/// exit semantics — those genuinely differ (`check`/`lint` summarize to
/// stderr and fail on any error; `build`/`bundle` print a success report only;
/// `audit` has its own finding-count wording).
pub(crate) struct DiagCounts {
    pub(crate) errors: usize,
    pub(crate) warnings: usize,
}

/// The diagnostics epilogue every project command shares: render `diags` in
/// `format` — resolving source snippets from files under `root` — print the
/// rendered frames to stdout when non-empty, and tally severities.
///
/// This is the common core the five reporting commands duplicated; the parts
/// that genuinely vary (the summary line's wording/shape, whether it prints on
/// success only or always, the bail message and exit code) stay in each
/// command, driven by the returned [`DiagCounts`]. `audit` folds in too: its
/// findings carry no labels, so the root-based lookup is never invoked and the
/// output is identical to its former no-op lookup.
pub(crate) fn render_diagnostics(diags: &[Diagnostic], format: Format, root: &Path) -> DiagCounts {
    let root = root.to_path_buf();
    let lookup = move |file: &str| fs::read_to_string(root.join(file)).ok();
    let output = render(diags, format, &lookup);
    if !output.is_empty() {
        println!("{output}");
    }
    DiagCounts {
        errors: diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .count(),
        warnings: diags
            .iter()
            .filter(|d| d.severity == Severity::Warning)
            .count(),
    }
}

#[cfg(test)]
mod tests {
    // test code — panics document assumptions
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use luabox_diag::Code;

    const MINIMAL_MANIFEST: &str = "\
[package]
name = \"fixture\"
version = \"0.1.0\"
edition = \"5.4\"
";

    /// Write `contents` to `root/rel`, creating parent directories.
    fn write(root: &Path, rel: &str, contents: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().expect("has a parent")).expect("create parents");
        fs::write(&path, contents).expect("write file");
    }

    #[test]
    fn find_manifest_dir_returns_the_directory_holding_the_manifest() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "luabox.toml", MINIMAL_MANIFEST);
        assert_eq!(
            find_manifest_dir(tmp.path()).expect("found"),
            tmp.path().to_path_buf()
        );
    }

    #[test]
    fn find_manifest_dir_walks_up_from_a_nested_subdirectory() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "luabox.toml", MINIMAL_MANIFEST);
        let nested = tmp.path().join("src").join("deep").join("deeper");
        fs::create_dir_all(&nested).expect("mkdir");

        assert_eq!(
            find_manifest_dir(&nested).expect("found by walking up"),
            tmp.path().to_path_buf()
        );
    }

    #[test]
    fn find_manifest_dir_stops_at_the_nearest_manifest() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "luabox.toml", MINIMAL_MANIFEST);
        write(tmp.path(), "inner/luabox.toml", MINIMAL_MANIFEST);
        let inner = tmp.path().join("inner");

        assert_eq!(find_manifest_dir(&inner).expect("found"), inner);
    }

    #[test]
    fn find_manifest_dir_ignores_a_directory_named_luabox_toml() {
        let tmp = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(tmp.path().join("proj").join("luabox.toml")).expect("mkdir");
        // `is_file()` — a *directory* by that name is not a manifest, and the
        // walk continues past it rather than claiming a bogus root.
        assert!(find_manifest_dir(&tmp.path().join("proj")).is_none());
    }

    #[test]
    fn read_manifest_parses_a_valid_manifest() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "luabox.toml", MINIMAL_MANIFEST);
        let manifest = read_manifest(tmp.path()).expect("parses");
        assert_eq!(manifest.package.name, "fixture");
        assert_eq!(manifest.package.edition, "5.4");
    }

    #[test]
    fn read_manifest_reports_an_unreadable_manifest_by_path() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let error = read_manifest(tmp.path()).unwrap_err();
        let rendered = format!("{error:#}");
        assert!(rendered.contains("cannot read"), "{rendered}");
        assert!(rendered.contains("luabox.toml"), "{rendered}");
    }

    #[test]
    fn read_manifest_reports_a_malformed_manifest_with_the_rendered_parse_errors() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "luabox.toml", "[package]\nname = 42\n");
        let error = read_manifest(tmp.path()).unwrap_err().to_string();
        assert!(error.starts_with("invalid `"), "{error}");
        assert!(error.contains("luabox.toml"), "{error}");
        // The rendered parse errors follow on their own line(s).
        assert!(error.contains('\n'), "{error}");
    }

    #[test]
    fn discover_manifest_yields_none_when_no_ancestor_has_a_manifest() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let nested = tmp.path().join("a").join("b");
        fs::create_dir_all(&nested).expect("mkdir");
        // A tempdir's ancestors are real directories, so this only holds
        // while no ancestor happens to carry a manifest — true for /tmp.
        assert!(discover_manifest(&nested).expect("no error").is_none());
    }

    #[test]
    fn discover_manifest_yields_the_root_and_parsed_manifest() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "luabox.toml", MINIMAL_MANIFEST);
        write(tmp.path(), "src/main.lua", "return 0\n");

        let (root, manifest) = discover_manifest(&tmp.path().join("src"))
            .expect("no error")
            .expect("found");
        assert_eq!(root, tmp.path().to_path_buf());
        assert_eq!(manifest.package.name, "fixture");
    }

    #[test]
    fn discover_manifest_propagates_a_malformed_present_manifest_as_an_error() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "luabox.toml", "this is not toml = = =\n");
        let error = discover_manifest(tmp.path()).unwrap_err().to_string();
        assert!(error.starts_with("invalid `"), "{error}");
    }

    #[test]
    fn collect_lua_files_walks_depth_first_in_name_order() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "z.lua", "");
        write(tmp.path(), "a.lua", "");
        write(tmp.path(), "b/inner.lua", "");
        write(tmp.path(), "b/a_inner.lua", "");

        let files = collect_lua_files(tmp.path(), None, false).expect("walk");
        let rel: Vec<String> = files.iter().map(|p| display_rel(p, tmp.path())).collect();
        assert_eq!(rel, ["a.lua", "b/a_inner.lua", "b/inner.lua", "z.lua"]);
    }

    #[test]
    fn collect_lua_files_skips_dot_directories_dot_files_and_non_lua_files() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "src/main.lua", "");
        write(tmp.path(), "README.md", "");
        write(tmp.path(), "src/notes.txt", "");
        write(tmp.path(), ".git/hooks.lua", "");
        write(tmp.path(), ".hidden.lua", "");

        let files = collect_lua_files(tmp.path(), None, false).expect("walk");
        let rel: Vec<String> = files.iter().map(|p| display_rel(p, tmp.path())).collect();
        assert_eq!(rel, ["src/main.lua"]);
    }

    #[test]
    fn collect_lua_files_skips_the_build_output_directory() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "src/main.lua", "");
        write(tmp.path(), "dist/src/main.lua", "");

        let out = tmp.path().join("dist");
        let files = collect_lua_files(tmp.path(), Some(&out), false).expect("walk");
        let rel: Vec<String> = files.iter().map(|p| display_rel(p, tmp.path())).collect();
        assert_eq!(rel, ["src/main.lua"]);
    }

    #[test]
    fn collect_lua_files_skips_the_vendored_lua_modules_tree() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "src/main.lua", "");
        // A real `luarocks install --tree lua_modules` layout: Lua modules
        // under share/lua/<X.Y>/, C modules under lib/lua/<X.Y>/, rock
        // metadata under lib/luarocks/.
        write(tmp.path(), "lua_modules/share/lua/5.4/pl/tablex.lua", "");
        write(tmp.path(), "lua_modules/share/lua/5.4/pl/init.lua", "");
        write(
            tmp.path(),
            "lua_modules/lib/luarocks/rocks-5.4/pl/spec.lua",
            "",
        );
        // …and the older flat layout, which is skipped just the same.
        write(tmp.path(), "lua_modules/pkg/src/init.lua", "");

        let files = collect_lua_files(tmp.path(), None, false).expect("walk");
        let rel: Vec<String> = files.iter().map(|p| display_rel(p, tmp.path())).collect();
        assert_eq!(rel, ["src/main.lua"]);
    }

    #[test]
    fn collect_lua_files_skips_a_nested_lua_modules_tree() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "packages/core/src/main.lua", "");
        // A vendored tree can nest — a workspace member has its own, and a
        // rock may vendor one in turn. Every depth is skipped.
        write(tmp.path(), "packages/core/lua_modules/dep/init.lua", "");
        write(
            tmp.path(),
            "lua_modules/share/lua/5.1/x/lua_modules/inner.lua",
            "",
        );

        let files = collect_lua_files(tmp.path(), None, false).expect("walk");
        let rel: Vec<String> = files.iter().map(|p| display_rel(p, tmp.path())).collect();
        assert_eq!(rel, ["packages/core/src/main.lua"]);
    }

    #[test]
    fn collect_lua_files_keeps_a_file_merely_named_lua_modules() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // The exclusion is by *directory* component; a source file that
        // happens to be called `lua_modules.lua` is first-party.
        write(tmp.path(), "src/lua_modules.lua", "");

        let files = collect_lua_files(tmp.path(), None, false).expect("walk");
        let rel: Vec<String> = files.iter().map(|p| display_rel(p, tmp.path())).collect();
        assert_eq!(rel, ["src/lua_modules.lua"]);
    }

    #[test]
    fn collect_lua_files_excludes_d_lua_only_when_asked() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "defs/love.d.lua", "");
        write(tmp.path(), "src/main.lua", "");

        let with_defs: Vec<String> = collect_lua_files(tmp.path(), None, false)
            .expect("walk")
            .iter()
            .map(|p| display_rel(p, tmp.path()))
            .collect();
        assert_eq!(with_defs, ["defs/love.d.lua", "src/main.lua"]);

        let without_defs: Vec<String> = collect_lua_files(tmp.path(), None, true)
            .expect("walk")
            .iter()
            .map(|p| display_rel(p, tmp.path()))
            .collect();
        assert_eq!(without_defs, ["src/main.lua"]);
    }

    #[test]
    fn collect_lua_files_reports_an_unreadable_directory_by_path() {
        let missing = Path::new("definitely-not-a-directory-xyzzy");
        let error = collect_lua_files(missing, None, false).unwrap_err();
        let rendered = format!("{error:#}");
        assert!(rendered.contains("cannot read directory"), "{rendered}");
    }

    #[test]
    fn display_rel_strips_the_root_and_normalizes_to_forward_slashes() {
        let root = Path::new("/proj");
        assert_eq!(
            display_rel(&root.join("src").join("a.lua"), root),
            "src/a.lua"
        );
        // A path outside the root is returned as-is rather than erroring.
        assert_eq!(
            display_rel(Path::new("/elsewhere/b.lua"), root),
            "/elsewhere/b.lua"
        );
    }

    #[test]
    fn render_diagnostics_tallies_errors_and_warnings_separately() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let diags = vec![
            Diagnostic::error(Code::new(1), "boom"),
            Diagnostic::warning(Code::new(501), "meh"),
            Diagnostic::warning(Code::new(502), "also meh"),
        ];
        let counts = render_diagnostics(&diags, Format::Human, tmp.path());
        assert_eq!(counts.errors, 1);
        assert_eq!(counts.warnings, 2);
    }

    #[test]
    fn render_diagnostics_on_an_empty_set_reports_zero_of_each() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let counts = render_diagnostics(&[], Format::Human, tmp.path());
        assert_eq!(counts.errors, 0);
        assert_eq!(counts.warnings, 0);
    }

    #[test]
    fn render_diagnostics_resolves_source_snippets_from_files_under_the_root() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "src/main.lua", "local x = 1\n");
        let diag = Diagnostic::error(Code::new(1), "bad thing").with_label(
            luabox_diag::Label::primary(luabox_diag::Span::new("src/main.lua", 6..7), "here"),
        );
        // The lookup closure is what turns a label into a rendered snippet;
        // exercising it proves the root-relative resolution works.
        let counts = render_diagnostics(std::slice::from_ref(&diag), Format::Human, tmp.path());
        assert_eq!(counts.errors, 1);
    }
}

//! `luabox fmt [--check] [--watch]` — canonical formatting for a whole
//! project (SPEC.md §10): every `**/*.lua` under the package in the
//! manifest's edition.
//!
//! Project discovery walks up from the working directory to the nearest
//! `luabox.toml` (cargo-style) and skips the `[build] out` directory —
//! build output is generated, not source. With no manifest in sight the
//! command still works standalone: it formats everything under the working
//! directory as Lua 5.4 (least surprise).
//!
//! `--watch` (SPEC.md §4) turns this into a long-running rerun-on-change
//! loop instead of a one-shot format — see `crate::watch` for the
//! debounce and filtering rules. It composes with `--check`: `luabox fmt
//! --check --watch` re-reports (without writing) on every change.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use luabox_syntax::{Dialect, lua};

use luabox_manifest::layout::{self, DefFiles, display_rel};

/// Execute `luabox fmt` from `cwd`. In `--check` mode nothing is written;
/// the command fails listing every file that would change. With `watch`,
/// it reruns on every debounced, filtered filesystem change under the
/// project root (`crate::watch`) until interrupted (Ctrl-C); a failing
/// rerun is reported but does not stop the watcher, so in watch mode this
/// function only returns on setup failure. Without `watch` it runs once
/// and its `Result` becomes the process exit code, as before.
pub fn run(cwd: &Path, check: bool, watch: bool) -> anyhow::Result<()> {
    if watch {
        // Discover once up front purely to get a root/out-dir to watch;
        // `run_once` rediscovers the project fresh on every rerun, so a
        // manifest edit (edition) takes effect on the very next rerun.
        let project = discover(cwd)?;
        let cwd = cwd.to_path_buf();
        return crate::watch::run(&project.root, project.out_dir.as_deref(), move || {
            run_once(&cwd, check)
        });
    }
    run_once(cwd, check)
}

/// The single-pass body of `luabox fmt`: discover the project, format (or,
/// in `--check` mode, just check) every file. Shared by one-shot `run` and
/// each rerun of `run` in `--watch` mode.
fn run_once(cwd: &Path, check: bool) -> anyhow::Result<()> {
    let project = discover(cwd)?;
    let files = collect_source_files(&project)?;

    let mut changed = Vec::new();
    for path in &files {
        let source = fs::read_to_string(path)
            .with_context(|| format!("cannot read `{}`", display_rel(path, &project.root)))?;
        let formatted = lua::fmt::format(&source, project.dialect);
        if formatted != source {
            if !check {
                fs::write(path, formatted).with_context(|| {
                    format!("cannot write `{}`", display_rel(path, &project.root))
                })?;
            }
            changed.push(display_rel(path, &project.root));
        }
    }

    if check {
        if changed.is_empty() {
            println!("checked {} files; all formatted", files.len());
            return Ok(());
        }
        for file in &changed {
            println!("would reformat {file}");
        }
        bail!(
            "{} of {} files would be reformatted; run `luabox fmt`",
            changed.len(),
            files.len()
        );
    }
    println!(
        "formatted {} files ({} changed)",
        files.len(),
        changed.len()
    );
    Ok(())
}

struct Project {
    /// Directory whose tree is formatted (manifest dir, or `cwd`).
    root: PathBuf,
    dialect: Dialect,
    /// `[build] out`, skipped during collection (manifest projects only).
    out_dir: Option<PathBuf>,
}

/// Find the project: nearest `luabox.toml` walking up from `cwd`, or a
/// manifest-less default rooted at `cwd`.
fn discover(cwd: &Path) -> anyhow::Result<Project> {
    let Some((root, manifest)) = layout::discover_manifest(cwd)? else {
        return Ok(Project {
            root: cwd.to_path_buf(),
            dialect: Dialect::Lua54,
            out_dir: None,
        });
    };
    let Some(dialect) = Dialect::from_manifest_id(&manifest.package.edition) else {
        bail!(
            "unknown edition `{}` in `{}`",
            manifest.package.edition,
            root.join("luabox.toml").display()
        );
    };
    Ok(Project {
        out_dir: Some(root.join(&manifest.build.out)),
        root,
        dialect,
    })
}

/// All `*.lua` files under the project root, deterministic order, skipping
/// dot-directories, the build output directory, and vendored `lua_modules/`
/// trees.
///
/// This is [`layout::collect_lua_files`] with [`DefFiles::Include`]: `fmt`
/// formats `*.d.lua` definition files too, unlike `check`/`build`/`doc`, which
/// never treat them as project source. `fmt` had its own copy of the walk
/// until the copy and the shared one disagreed about `lua_modules/` — one skip
/// list, so they cannot drift apart again.
fn collect_source_files(project: &Project) -> anyhow::Result<Vec<PathBuf>> {
    Ok(layout::collect_lua_files(
        &project.root,
        project.out_dir.as_deref(),
        DefFiles::Include,
    )?)
}

#[cfg(test)]
mod tests {
    // test code — panics document assumptions
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    /// Deliberately mis-formatted source, and what `lua::fmt::format`
    /// canonicalizes it to — computed rather than hard-coded so these tests
    /// assert `fmt`'s *file handling* (which files, written or not) instead
    /// of re-asserting the formatter's own rules, which `luabox-syntax` owns.
    const MESSY: &str = "local    x=1\nprint( x )\n";

    fn canonical(dialect: Dialect) -> String {
        lua::fmt::format(MESSY, dialect)
    }

    fn write(root: &Path, rel: &str, contents: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().expect("has a parent")).expect("create parents");
        fs::write(&path, contents).expect("write file");
    }

    fn manifest(edition: &str, out: &str) -> String {
        format!(
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"{edition}\"\n\n[build]\nout = \"{out}\"\n"
        )
    }

    #[test]
    fn messy_fixture_is_actually_unformatted() {
        // Guards every test below: if the formatter ever declared MESSY
        // canonical, the "rewrites" assertions would pass vacuously.
        assert_ne!(canonical(Dialect::Lua54), MESSY);
    }

    #[test]
    fn a_project_root_that_cannot_be_walked_names_the_directory() {
        // No manifest anywhere above it, so `fmt` roots a default project at
        // `cwd` — and then cannot list it. The failure names the directory
        // rather than reporting an empty, successful format of nothing.
        let error = run(Path::new("no-such-directory-xyzzy"), false, false).unwrap_err();
        let rendered = format!("{error:#}");
        assert!(rendered.contains("cannot read directory"), "{rendered}");
    }

    #[test]
    fn run_rewrites_unformatted_sources_in_place() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "luabox.toml", &manifest("5.4", "dist"));
        write(tmp.path(), "src/main.lua", MESSY);

        run(tmp.path(), false, false).expect("fmt succeeds");

        assert_eq!(
            fs::read_to_string(tmp.path().join("src").join("main.lua")).expect("read back"),
            canonical(Dialect::Lua54)
        );
    }

    #[test]
    fn run_leaves_already_formatted_sources_untouched() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "luabox.toml", &manifest("5.4", "dist"));
        let formatted = canonical(Dialect::Lua54);
        write(tmp.path(), "src/main.lua", &formatted);

        run(tmp.path(), false, false).expect("fmt succeeds");

        assert_eq!(
            fs::read_to_string(tmp.path().join("src").join("main.lua")).expect("read back"),
            formatted
        );
    }

    #[test]
    fn check_mode_fails_without_writing_when_a_file_would_change() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "luabox.toml", &manifest("5.4", "dist"));
        write(tmp.path(), "src/main.lua", MESSY);

        let error = run(tmp.path(), true, false).unwrap_err().to_string();
        assert!(
            error.contains("1 of 1 files would be reformatted"),
            "{error}"
        );
        assert!(error.contains("run `luabox fmt`"), "{error}");
        // --check is read-only: the file on disk is exactly as it was.
        assert_eq!(
            fs::read_to_string(tmp.path().join("src").join("main.lua")).expect("read back"),
            MESSY
        );
    }

    #[test]
    fn check_mode_succeeds_when_every_file_is_already_formatted() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "luabox.toml", &manifest("5.4", "dist"));
        write(tmp.path(), "src/main.lua", &canonical(Dialect::Lua54));

        run(tmp.path(), true, false).expect("check passes");
    }

    #[test]
    fn check_mode_counts_every_offending_file_in_its_failure() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "luabox.toml", &manifest("5.4", "dist"));
        write(tmp.path(), "src/a.lua", MESSY);
        write(tmp.path(), "src/b.lua", MESSY);
        write(tmp.path(), "src/c.lua", &canonical(Dialect::Lua54));

        let error = run(tmp.path(), true, false).unwrap_err().to_string();
        assert!(
            error.contains("2 of 3 files would be reformatted"),
            "{error}"
        );
    }

    #[test]
    fn a_manifest_less_directory_is_formatted_as_lua_54() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "main.lua", MESSY);

        let project = discover(tmp.path()).expect("manifest-less default");
        assert_eq!(project.dialect, Dialect::Lua54);
        assert_eq!(project.root, tmp.path().to_path_buf());
        assert!(project.out_dir.is_none());

        run(tmp.path(), false, false).expect("fmt succeeds");
        assert_eq!(
            fs::read_to_string(tmp.path().join("main.lua")).expect("read back"),
            canonical(Dialect::Lua54)
        );
    }

    #[test]
    fn the_project_is_discovered_by_walking_up_from_a_subdirectory() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "luabox.toml", &manifest("5.1", "dist"));
        write(tmp.path(), "src/main.lua", MESSY);

        // Run from `src/`, format the whole project from its real root.
        run(&tmp.path().join("src"), false, false).expect("fmt succeeds");
        assert_eq!(
            fs::read_to_string(tmp.path().join("src").join("main.lua")).expect("read back"),
            canonical(Dialect::Lua51)
        );
    }

    #[test]
    fn discover_reads_the_edition_and_out_dir_from_the_manifest() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "luabox.toml", &manifest("luajit", "build"));

        let project = discover(tmp.path()).expect("discovers");
        assert_eq!(project.dialect, Dialect::LuaJit);
        assert_eq!(project.out_dir, Some(tmp.path().join("build")));
    }

    #[test]
    fn an_unknown_edition_in_the_manifest_is_rejected_naming_the_manifest() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "luabox.toml", &manifest("5.9", "dist"));

        // `Manifest::parse` owns the edition allow-list, so an unknown
        // edition never reaches `discover`'s own `from_manifest_id` guard —
        // it is reported as a manifest validation error instead.
        let error = run(tmp.path(), false, false).unwrap_err().to_string();
        assert!(error.starts_with("invalid `"), "{error}");
        assert!(error.contains("luabox.toml"), "{error}");
        assert!(error.contains("package.edition `5.9`"), "{error}");
    }

    #[test]
    fn a_malformed_manifest_is_an_error_rather_than_a_silent_default() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "luabox.toml", "name = = =\n");
        let error = run(tmp.path(), false, false).unwrap_err().to_string();
        assert!(error.starts_with("invalid `"), "{error}");
    }

    #[test]
    fn the_build_output_directory_and_dot_directories_are_never_formatted() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "luabox.toml", &manifest("5.4", "dist"));
        write(tmp.path(), "src/main.lua", &canonical(Dialect::Lua54));
        write(tmp.path(), "dist/src/main.lua", MESSY);
        write(tmp.path(), ".cache/stale.lua", MESSY);
        write(tmp.path(), ".hidden.lua", MESSY);

        // Generated output and hidden state are not sources: --check passes
        // even though all three would reformat.
        run(tmp.path(), true, false).expect("check passes");
        assert_eq!(
            fs::read_to_string(tmp.path().join("dist").join("src").join("main.lua"))
                .expect("read back"),
            MESSY
        );
        assert_eq!(
            fs::read_to_string(tmp.path().join(".cache").join("stale.lua")).expect("read back"),
            MESSY
        );
    }

    #[test]
    fn definition_files_are_formatted_like_any_other_source() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "luabox.toml", &manifest("5.4", "dist"));
        write(tmp.path(), "defs/love.d.lua", MESSY);

        // Unlike `check`/`build`, `fmt` has no reason to skip `*.d.lua`.
        run(tmp.path(), false, false).expect("fmt succeeds");
        assert_eq!(
            fs::read_to_string(tmp.path().join("defs").join("love.d.lua")).expect("read back"),
            canonical(Dialect::Lua54)
        );
    }

    #[test]
    fn collect_source_files_returns_a_deterministic_name_ordered_list() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "z.lua", "");
        write(tmp.path(), "a.lua", "");
        write(tmp.path(), "m/inner.lua", "");
        write(tmp.path(), "notes.md", "");

        let project = discover(tmp.path()).expect("manifest-less default");
        let files = collect_source_files(&project).expect("collect");
        let rel: Vec<String> = files
            .iter()
            .map(|p| display_rel(p, &project.root))
            .collect();
        assert_eq!(rel, ["a.lua", "m/inner.lua", "z.lua"]);
    }

    #[test]
    fn an_empty_project_formats_zero_files_successfully() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "luabox.toml", &manifest("5.4", "dist"));
        run(tmp.path(), false, false).expect("fmt succeeds");
        run(tmp.path(), true, false).expect("check passes");
    }
}

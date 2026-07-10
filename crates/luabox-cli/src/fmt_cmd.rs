//! `luabox fmt [--check] [--watch]` — canonical formatting for a whole
//! project (SPEC.md §10): every `**/*.lua` under the package in the
//! manifest's edition, plus every `**/*.luab` shape module via the shape
//! formatter.
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
use luabox_resolve::manifest::Manifest;
use luabox_syntax::{Dialect, lua, shape};

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
        let formatted = match path.extension().and_then(|e| e.to_str()) {
            Some("lua") => lua::fmt::format(&source, project.dialect),
            _ => shape::format(&source),
        };
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
    let mut dir = Some(cwd);
    while let Some(current) = dir {
        let manifest_path = current.join("luabox.toml");
        if manifest_path.is_file() {
            let text = fs::read_to_string(&manifest_path)
                .with_context(|| format!("cannot read `{}`", manifest_path.display()))?;
            let manifest = Manifest::parse(&text).map_err(|errors| {
                let rendered = errors
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("\n");
                anyhow::anyhow!("invalid `{}`:\n{rendered}", manifest_path.display())
            })?;
            let Some(dialect) = Dialect::from_manifest_id(&manifest.package.edition) else {
                bail!(
                    "unknown edition `{}` in `{}`",
                    manifest.package.edition,
                    manifest_path.display()
                );
            };
            return Ok(Project {
                root: current.to_path_buf(),
                dialect,
                out_dir: Some(current.join(&manifest.build.out)),
            });
        }
        dir = current.parent();
    }
    Ok(Project {
        root: cwd.to_path_buf(),
        dialect: Dialect::Lua54,
        out_dir: None,
    })
}

/// All `*.lua` / `*.luab` files under the project root, deterministic order,
/// skipping dot-directories and the build output directory.
fn collect_source_files(project: &Project) -> anyhow::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    walk(&project.root, project, &mut files)?;
    Ok(files)
}

fn walk(dir: &Path, project: &Project, files: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    let mut entries: Vec<_> = fs::read_dir(dir)
        .with_context(|| format!("cannot read directory `{}`", dir.display()))?
        .collect::<Result<_, _>>()
        .with_context(|| format!("cannot read directory `{}`", dir.display()))?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let name = entry.file_name();
        let hidden = name.to_string_lossy().starts_with('.');
        if path.is_dir() {
            let is_out = project.out_dir.as_deref() == Some(path.as_path());
            if !hidden && !is_out {
                walk(&path, project, files)?;
            }
        } else if !hidden
            && matches!(
                path.extension().and_then(|e| e.to_str()),
                Some("lua" | "luab")
            )
        {
            files.push(path);
        }
    }
    Ok(())
}

/// Root-relative path with forward slashes — stable output across platforms.
fn display_rel(path: &Path, root: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.to_string_lossy().replace('\\', "/")
}

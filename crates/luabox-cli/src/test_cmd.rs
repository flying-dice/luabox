//! `luabox test [pattern] [--watch] [--matrix] [--coverage]` — the built-in
//! test runner (SPEC.md §11).
//!
//! Zero-config: discovers `*_test.lua` / `*.test.lua` / anything under
//! `tests/`, resolves a Lua runtime from the manifest `edition` (or the
//! `LUABOX_LUA` override, or every runtime on PATH with `--matrix`), and
//! drives the embedded harness in `luabox-test`. One process per test file,
//! parallel across files; the human report goes to stdout and the process
//! exits nonzero iff anything failed.
//!
//! `--watch` reuses the shared watch driver (`crate::watch`): a rerun on
//! every debounced source/manifest change, forever, a failing rerun
//! reported but not fatal. `--coverage` (source-map-aware instrumentation,
//! SPEC.md §11) isn't built yet; the flag is accepted only so it fails
//! loudly with a clear error rather than silently running without it —
//! see `run` below. It's hidden from `--help` accordingly.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use luabox_resolve::manifest::Manifest;
use luabox_test::runner::SuiteOptions;
use luabox_test::runtime::{RuntimeSpec, resolve_default, resolve_matrix};
use luabox_test::{RuntimeReport, run_suite};

/// Execute `luabox test`. In `--watch` mode this only returns on setup
/// failure (the watch root can't be observed); otherwise it runs once and
/// its `Result` becomes the process exit code.
pub fn run(
    cwd: &Path,
    pattern: Option<&str>,
    watch: bool,
    coverage: bool,
    matrix: bool,
) -> anyhow::Result<()> {
    if coverage {
        bail!(
            "--coverage is not implemented yet (SPEC.md §11); track progress \
             at the project backlog"
        );
    }

    if watch {
        let project = discover(cwd)?;
        let cwd = cwd.to_path_buf();
        let pattern = pattern.map(str::to_owned);
        return crate::watch::run(&project.root, project.out_dir.as_deref(), move || {
            run_once(&cwd, pattern.as_deref(), matrix)
        });
    }
    run_once(cwd, pattern, matrix)
}

/// One discovery + run + report cycle. Shared by the one-shot path and each
/// `--watch` rerun (which rediscovers the project from scratch, so a
/// manifest edit takes effect on the next rerun).
fn run_once(cwd: &Path, pattern: Option<&str>, matrix: bool) -> anyhow::Result<()> {
    let project = discover(cwd)?;
    let files = luabox_test::discover(&project.root, project.out_dir.as_deref());

    if files.is_empty() {
        println!(
            "no test files found (looked for *_test.lua, *.test.lua, and anything under tests/)"
        );
        return Ok(());
    }

    let opts = SuiteOptions {
        files: &files,
        pattern,
        root: &project.root,
    };

    let runtimes = resolve_runtimes(&project.edition, &project.root, matrix)?;
    let reports: Vec<RuntimeReport> = runtimes
        .iter()
        .map(|runtime| run_suite(runtime, &opts))
        .collect();

    let (text, summary) = luabox_test::render(&reports, matrix);
    print!("{text}");

    if summary.failed > 0 {
        bail!(
            "test failed: {} passed, {} failed",
            summary.passed,
            summary.failed
        );
    }
    Ok(())
}

/// Resolve the runtime(s) to run against. `--matrix` probes every Lua on
/// PATH (noting what's missing and warning if fewer than two are present);
/// otherwise a single runtime is resolved from the edition / `LUABOX_LUA`.
fn resolve_runtimes(edition: &str, root: &Path, matrix: bool) -> anyhow::Result<Vec<RuntimeSpec>> {
    if matrix {
        let resolution = resolve_matrix();
        if resolution.found.is_empty() {
            bail!(
                "--matrix found no Lua runtimes on PATH (probed 5.1/5.2/5.3/5.4/luajit/lua). \
                 Install at least one with `luabox toolchain install`, put a Lua on PATH, \
                 or set LUABOX_LUA"
            );
        }
        if !resolution.missing.is_empty() {
            eprintln!(
                "note: --matrix ran {} runtime(s); not found on PATH: {}",
                resolution.found.len(),
                resolution.missing.join(", ")
            );
        }
        if resolution.found.len() < 2 {
            eprintln!(
                "note: --matrix is most useful with multiple runtimes; only one was found \
                 ({}). Install more Lua versions to exercise the cross-version matrix.",
                resolution.found[0].label
            );
        }
        Ok(resolution.found)
    } else {
        let runtime = resolve_default(edition, root).map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(vec![runtime])
    }
}

/// The bit of project state the runner needs.
struct Project {
    root: PathBuf,
    edition: String,
    out_dir: Option<PathBuf>,
}

/// Find the project: nearest `luabox.toml` walking up from `cwd`, or a
/// manifest-less default rooted at `cwd` (edition 5.4 — least surprise,
/// matching `luabox check`).
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
            return Ok(Project {
                root: current.to_path_buf(),
                edition: manifest.package.edition.clone(),
                out_dir: Some(current.join(&manifest.build.out)),
            });
        }
        dir = current.parent();
    }
    Ok(Project {
        root: cwd.to_path_buf(),
        edition: "5.4".to_string(),
        out_dir: None,
    })
}

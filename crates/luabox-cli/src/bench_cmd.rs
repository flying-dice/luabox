//! `luabox bench` — criterion-style statistical benchmarking across
//! runtimes (SPEC.md §11, ticket #26).
//!
//! Zero-config: discovers `*_bench.lua` / `*.bench.lua` / anything under
//! `bench/`, resolves **every** Lua runtime found on `PATH` (plus
//! `LUABOX_LUA`, if set — see [`luabox_test::runtime::resolve_matrix`]) and
//! drives the embedded bench harness in `luabox-test` against each one.
//! Cross-runtime comparison is the point (SPEC.md §11: "statistical
//! benchmarking across runtimes"), so unlike `luabox test` there is no
//! single-runtime mode to opt out of it.
//!
//! Benches never fail the build: this command always exits 0, printing a
//! human comparison table (`BENCH | RUNTIME | MEDIAN | ±STDDEV | ...`)
//! grouped by bench name so every runtime's numbers for a given bench sit
//! together.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;
use luabox_resolve::manifest::Manifest;
use luabox_test::bench::{self, SuiteOptions};
use luabox_test::runtime::resolve_matrix;

/// Execute `luabox bench`. Always returns `Ok(())` on the happy discovery
/// path — a per-file or per-bench failure is reported in the table, not as
/// a nonzero exit (SPEC.md §11: benches don't fail builds).
pub fn run(cwd: &Path) -> anyhow::Result<()> {
    let project = discover(cwd)?;
    let files = bench::discover(&project.root, project.out_dir.as_deref());

    if files.is_empty() {
        println!(
            "no bench files found (looked for *_bench.lua, *.bench.lua, and anything under bench/)"
        );
        return Ok(());
    }

    let resolution = resolve_matrix();
    if resolution.found.is_empty() {
        println!(
            "no Lua runtime found on PATH (probed 5.1/5.2/5.3/5.4/luajit/lua); nothing to \
             benchmark against. Install one, or set LUABOX_LUA; managed toolchains arrive with \
             `luabox toolchain` (#27)"
        );
        return Ok(());
    }
    if !resolution.missing.is_empty() {
        eprintln!(
            "note: benchmarking against {} runtime(s); not found on PATH: {}",
            resolution.found.len(),
            resolution.missing.join(", ")
        );
    }
    if resolution.found.len() < 2 {
        eprintln!(
            "note: cross-runtime comparison is most useful with multiple runtimes; only one \
             was found ({}). Install more Lua versions to compare.",
            resolution.found[0].label
        );
    }

    let opts = SuiteOptions {
        files: &files,
        root: &project.root,
    };

    let reports: Vec<bench::RuntimeReport> = resolution
        .found
        .iter()
        .map(|runtime| bench::run_suite(runtime, &opts))
        .collect();

    print!("{}", bench::render(&reports));
    Ok(())
}

/// The bit of project state the runner needs.
struct Project {
    root: PathBuf,
    out_dir: Option<PathBuf>,
}

/// Find the project: nearest `luabox.toml` walking up from `cwd`, or a
/// manifest-less default rooted at `cwd` (mirrors `luabox test`'s
/// discovery in `test_cmd.rs`).
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
                out_dir: Some(current.join(&manifest.build.out)),
            });
        }
        dir = current.parent();
    }
    Ok(Project {
        root: cwd.to_path_buf(),
        out_dir: None,
    })
}

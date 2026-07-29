//! Fixture helpers shared by the two cucumber harnesses (`acceptance.rs` and
//! `lsp_acceptance.rs`).
//!
//! Both suites build the same kind of scenario project — a `luabox.toml` and
//! some Lua files under a temp dir — and both read Gherkin docstrings the same
//! way. Only the `World` differs, and cucumber's step attributes are bound to
//! one `World` type, so the `#[given]`/`#[when]`/`#[then]` shims necessarily
//! stay per-suite; everything under them lives here, parameterized on the
//! project root rather than on a `World`.
//!
//! This file is a module of each test binary (`mod support;`), not a target of
//! its own — it is not compiled or counted separately.

use std::path::{Path, PathBuf};

use cucumber::gherkin::Step;

/// The `luabox` executable every scenario drives.
///
/// Defaults to the binary cargo built for this test target
/// (`CARGO_BIN_EXE_luabox`) — the only path any local run, `cargo test
/// --workspace`, or the coverage-e2e job in `ci.yml` ever takes.
///
/// A non-empty `LUABOX_E2E_BIN` overrides it, which is what makes these
/// black-box suites reusable as a *release* gate: `release.yml` installs the
/// draft release's binary with the shipped install script and points this at
/// the installed executable, so the artefact users will download is the thing
/// under test (SPEC.md §16.2 — the scenarios never touch an internal API, so
/// nothing but the process path has to change).
pub fn luabox_bin() -> PathBuf {
    match std::env::var("LUABOX_E2E_BIN") {
        Ok(path) if !path.is_empty() => PathBuf::from(path),
        _ => PathBuf::from(env!("CARGO_BIN_EXE_luabox")),
    }
}

/// The step's docstring, normalized: the leading newline after `"""` is
/// stripped and exactly one trailing newline is guaranteed — matching the
/// formatter's final-newline convention so `equals:` comparisons are exact.
pub fn docstring(step: &Step) -> String {
    let raw = step
        .docstring
        .as_deref()
        .expect("this step requires a docstring (\"\"\" … \"\"\")");
    let body = raw.strip_prefix('\n').unwrap_or(raw);
    format!("{}\n", body.trim_end_matches(['\n', '\r']))
}

/// Write `content` to `root/relative`, creating parent directories.
pub fn write_file(root: &Path, relative: &str, content: &str) {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("failed to create parent directories");
    }
    std::fs::write(&path, content).unwrap_or_else(|e| panic!("cannot write `{relative}`: {e}"));
}

/// The `[package]` table every fixture manifest opens with.
pub fn package_table(edition: &str) -> String {
    format!(
        "[package]\n\
         name = \"fixture\"\n\
         version = \"0.1.0\"\n\
         edition = \"{edition}\"\n"
    )
}

/// The smallest manifest a scenario project needs: `[package]` plus the
/// `[types] strict` flag the "project"/"strict project" steps vary.
pub fn manifest(edition: &str, strict: bool) -> String {
    format!("{}\n[types]\nstrict = {strict}\n", package_table(edition))
}

/// Write [`manifest`] to `root/luabox.toml`.
pub fn write_manifest(root: &Path, edition: &str, strict: bool) {
    write_file(root, "luabox.toml", &manifest(edition, strict));
}

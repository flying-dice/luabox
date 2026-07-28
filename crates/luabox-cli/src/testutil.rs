//! Fixture helpers shared by the command modules' `#[cfg(test)] mod tests`.
//!
//! Every command test builds the same thing — a tempdir holding a
//! `luabox.toml` and some Lua files — and each module used to carry its own
//! copy of the three-line writer, the manifest template and the project
//! builder. They live here now; the modules keep only the wrappers whose
//! signature is genuinely their own (`fmt_cmd`'s `[build] out`, for instance).
//!
//! Test-only: the module is `#[cfg(test)]` and never reaches the binary.

use std::fs;
use std::path::Path;

/// Write `contents` to `root/rel`, creating parent directories.
pub fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().expect("has a parent")).expect("create parents");
    fs::write(&path, contents).expect("write file");
}

/// Read `root/rel` back, naming the file if it cannot be read.
pub fn read(root: &Path, rel: &str) -> String {
    fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("reading {rel}: {e}"))
}

/// A `luabox.toml` body for a package named `name`, with `extra` (further
/// tables, already newline-prefixed) appended verbatim.
pub fn manifest_named(name: &str, edition: &str, extra: &str) -> String {
    format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"{edition}\"\n{extra}")
}

/// [`manifest_named`] for the conventional `fixture` package.
pub fn manifest(edition: &str, extra: &str) -> String {
    manifest_named("fixture", edition, extra)
}

/// A project rooted in a fresh tempdir whose `luabox.toml` is `manifest_text`
/// verbatim — for the tests that pin how a hand-written (or malformed)
/// manifest is handled.
pub fn project_with_manifest(manifest_text: &str) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    write(tmp.path(), "luabox.toml", manifest_text);
    tmp
}

/// A project rooted in a fresh tempdir with [`manifest`] in place.
pub fn project(edition: &str, extra: &str) -> tempfile::TempDir {
    project_with_manifest(&manifest(edition, extra))
}

/// A project rooted in a fresh tempdir with [`manifest_named`] in place — for
/// the tests whose subject is the package name itself.
pub fn project_named(name: &str, edition: &str, extra: &str) -> tempfile::TempDir {
    project_with_manifest(&manifest_named(name, edition, extra))
}

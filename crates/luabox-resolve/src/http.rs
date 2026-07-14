//! The resolver's single outbound-HTTP seam.
//!
//! Registry and LuaRocks fetches all shell out to the `curl` CLI rather than
//! link an HTTP crate (SPEC.md §6 keeps the dependency tree small; this
//! mirrors the toolchain installer's approach). Three call sites used to
//! hand-build the same `curl` invocation independently — the registry index
//! fetch, the registry artifact download, and the LuaRocks rockspec/manifest
//! fetch. This module owns that invocation in one place: flag construction,
//! process spawning, and the text-body vs. download-to-file split.
//!
//! It deliberately does **not** own error *interpretation*: each caller keeps
//! its own exit-code policy (the registry maps curl's exit 22 — HTTP 404 — to
//! "package absent") and renders failures into its own error type. So the
//! functions here return the raw process results and let the caller decide.
//! When a real HTTP client eventually replaces `curl`, this is the only module
//! that changes.

use std::io;
use std::path::Path;
use std::process::{Command, ExitStatus, Output};

/// The shared leading flags: `-f` (fail on HTTP errors), `-s` (silent),
/// `-S` (still surface errors), optional `-L` (follow redirects), and the
/// `--max-time` cap in seconds.
fn base_args(max_time_secs: u32, follow_redirects: bool) -> Vec<String> {
    let flags = if follow_redirects { "-fsSL" } else { "-fsS" };
    vec![
        flags.to_owned(),
        "--max-time".to_owned(),
        max_time_secs.to_string(),
    ]
}

/// Fetch `url` into memory with `curl`, capturing stdout and stderr.
///
/// The raw [`Output`] is returned so the caller can apply its own exit-code
/// policy (success vs. curl's 404-shaped exit 22) and format its own error.
pub(crate) fn get(url: &str, max_time_secs: u32, follow_redirects: bool) -> io::Result<Output> {
    let mut args = base_args(max_time_secs, follow_redirects);
    args.push(url.to_owned());
    Command::new("curl").args(&args).output()
}

/// Download `url` straight to `dest` with `curl -o` (streamed; stdio is
/// inherited so progress/errors reach the terminal). Returns the exit status
/// for the caller to check.
pub(crate) fn download(
    url: &str,
    dest: &Path,
    max_time_secs: u32,
    follow_redirects: bool,
) -> io::Result<ExitStatus> {
    Command::new("curl")
        .args(base_args(max_time_secs, follow_redirects))
        .arg("-o")
        .arg(dest)
        .arg(url)
        .status()
}

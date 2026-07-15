//! The resolver's single outbound-HTTP seam.
//!
//! LuaRocks fetches shell out to the `curl` CLI rather than link an HTTP crate
//! (SPEC.md §6 keeps the dependency tree small; this mirrors the toolchain
//! installer's approach). This module owns that invocation in one place: flag
//! construction and process spawning.
//!
//! It deliberately does **not** own error *interpretation*: the caller keeps
//! its own exit-code policy and renders failures into its own error type, so
//! this returns the raw process results. When a real HTTP client eventually
//! replaces `curl`, this is the only module that changes.

use std::io;
use std::process::{Command, Output};

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

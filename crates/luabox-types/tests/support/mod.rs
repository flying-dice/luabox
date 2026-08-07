//! Fixture helpers shared by the two duplicate-`---@class`-merge suites
//! (`duplicate_class_merge.rs`, `duplicate_class_merge_property.rs`, #59).
//!
//! Round 3 review F79: `check`, `surface` and `check_cross` were ~40 lines
//! duplicated near-verbatim between the two files, differing only in
//! `check`'s return type (one file wanted the full [`Diagnostic`]s, to
//! inspect a message; the other only ever reduced to codes) and
//! `check_cross`'s element type (`&[&str]` vs `&[String]`). Both are
//! resolved here rather than copied: `check` returns the full diagnostics,
//! and `codes` is the reduction most call sites want; `check_cross` is
//! generic over `AsRef<str>` so either owned or borrowed fixture strings
//! work.
//!
//! This file is a module of each test binary (`mod support;`), not a target
//! of its own — it is not compiled or counted separately. Each binary that
//! includes it uses whatever subset of these helpers its own fixtures need
//! (directly, or transitively — `codes` calls `check`, `check_cross` calls
//! `check_cross_diags` calls `surface`), so a binary that never needs, say,
//! `check_cross`'s codes-only reduction leaves it genuinely unused in that
//! one compilation; `dead_code` is suppressed crate-wide for this module
//! rather than per binary.
#![allow(dead_code)]

use luabox_diag::Diagnostic;
use luabox_syntax::lua::{self, Dialect, parse};
use luabox_types::{
    Ambient, FileTypes, Strictness, check_file_with_ambient, module_surface, stdlib_defs,
};

/// Strict-check one standalone file against the stdlib ambient.
pub fn check(src: &str) -> Vec<Diagnostic> {
    let parsed = parse(src, Dialect::Lua54);
    assert_eq!(parsed.errors(), &[], "fixture must parse cleanly:\n{src}");
    check_file_with_ambient(
        &parsed,
        "test.lua",
        Strictness::Strict,
        lua::Dialect::Lua54,
        Some(stdlib_defs(Dialect::Lua54)),
    )
}

/// [`check`], reduced to diagnostic codes — what most call sites want.
pub fn codes(src: &str) -> Vec<String> {
    check(src).iter().map(|d| d.code.to_string()).collect()
}

/// The workspace surface one project file contributes.
pub fn surface(src: &str, base: &Ambient) -> FileTypes {
    let parsed = parse(src, Dialect::Lua54);
    assert_eq!(parsed.errors(), &[], "fixture must parse cleanly:\n{src}");
    module_surface(&parsed, "m.lua", Some(base)).types
}

/// [`check_cross`]'s full-diagnostic form: `files` merged in the caller's
/// order (file-order dependence — which of two conflicting cross-file
/// declarations wins — is itself part of what the class-merge-precedence
/// matrix measures, `docs/03-reference/03-class-merge-precedence.md`), then
/// `consumer` checked against the merged surface.
pub fn check_cross_diags<S: AsRef<str>>(files: &[S], consumer: &str) -> Vec<Diagnostic> {
    let base = stdlib_defs(Dialect::Lua54);
    let types: Vec<FileTypes> = files.iter().map(|f| surface(f.as_ref(), base)).collect();
    let ambient = base.with_project_types(types.iter());
    let parsed = parse(consumer, Dialect::Lua54);
    assert_eq!(parsed.errors(), &[], "consumer must parse cleanly");
    check_file_with_ambient(
        &parsed,
        "consumer.lua",
        Strictness::Strict,
        Dialect::Lua54,
        Some(&ambient),
    )
}

/// Check `consumer` against the merged surface of every `file`.
pub fn check_cross<S: AsRef<str>>(files: &[S], consumer: &str) -> Vec<String> {
    check_cross_diags(files, consumer)
        .iter()
        .map(|d| d.code.to_string())
        .collect()
}

/// [`check`], but self-inclusive: `src`'s own workspace surface is folded
/// into its own checking ambient before `src` is checked against it —
/// mirroring the CLI batch path's project-wide ambient (`check_cmd.rs`'s
/// `run_passes`: every project file's surface, the file *being checked*
/// included, is reified and merged before any file is checked; see
/// `docs/03-reference/03-class-merge-precedence.md`'s finding 1). `check`
/// alone (stdlib ambient only) cannot exercise a class seeing its own
/// same-file carrier-attached methods through inheritance from a same-file
/// parent: `TypeEnv::collect_class`'s ancestry walk only ever reads
/// `TypeEnv::classes[name].methods`, and nothing populates that for a
/// project file's own declarations except this self-fold — `check_cross`
/// exercises the cross-file half (a library file's already-folded surface
/// merging into a *different* consumer's ambient) but, like `check`, never
/// folds `consumer` beneath its own ambient, so it cannot stand in for this
/// either.
pub fn check_self(src: &str) -> Vec<Diagnostic> {
    let base = stdlib_defs(Dialect::Lua54);
    let own = surface(src, base);
    let ambient = base.with_project_types(std::iter::once(&own));
    let parsed = parse(src, Dialect::Lua54);
    assert_eq!(parsed.errors(), &[], "fixture must parse cleanly:\n{src}");
    check_file_with_ambient(
        &parsed,
        "test.lua",
        Strictness::Strict,
        Dialect::Lua54,
        Some(&ambient),
    )
}

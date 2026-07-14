//! `luabox audit` — RUSTSEC-analog advisory check (SPEC.md §6, §14).
//!
//! Discovers the project, reads `luabox.lock` (missing lockfile is a hard
//! error — there is nothing to audit without one), resolves the advisory
//! database directory, matches it against the lockfile
//! (`luabox_resolve::advisory::AdvisoryDb::check`), and renders findings as
//! `LB1100` diagnostics via `luabox-diag`.
//!
//! # Database location
//!
//! `LUABOX_ADVISORY_DB` (a directory path), else `<home>/.luabox/advisory-db`.
//! **No hosted default feed exists yet** — this command never touches the
//! network. When the resolved directory does not exist (whether because
//! nothing was configured, or because a configured path is stale/wrong),
//! `luabox audit` prints an informational note and exits `0` rather than
//! failing: a security check must never fail a build/CI pipeline purely
//! because a database was never wired up, or teams learn to disable the
//! check instead of fixing the setup (see `luabox-resolve::advisory` module
//! docs for the full rationale).
//!
//! # Severity → exit code
//!
//! `critical`/`high` findings render as `Error`; `medium`/`low` render as
//! `Warning`. The exit code is nonzero iff at least one `Error`-severity
//! finding was produced — matching `luabox check`'s convention (SPEC.md §14).
//! A malformed advisory file is reported as a warning on stderr and does not
//! stop the rest of the audit.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use luabox_diag::{Code, Diagnostic, Format, Severity};
use luabox_resolve::advisory::{AdvisoryDb, AdvisorySeverity, Finding};
use luabox_resolve::{LOCKFILE_NAME, Lockfile};

pub fn run(cwd: &Path) -> anyhow::Result<()> {
    let root = discover(cwd)?;

    let lock_path = root.join(LOCKFILE_NAME);
    let Ok(lock_text) = fs::read_to_string(&lock_path) else {
        bail!(
            "no `{LOCKFILE_NAME}` found in `{}` — run `luabox install` first",
            root.display()
        );
    };
    let lockfile = Lockfile::parse(&lock_text)
        .with_context(|| format!("cannot parse `{}`", lock_path.display()))?;

    let Some(db_dir) = advisory_db_dir() else {
        println!(
            "no advisory database configured (set LUABOX_ADVISORY_DB, or populate \
             ~/.luabox/advisory-db); skipping audit — see `luabox explain LB1100`"
        );
        return Ok(());
    };

    let (db, warnings) = AdvisoryDb::load(&db_dir);
    for warning in &warnings {
        eprintln!("warning: {warning}");
    }

    let findings = db.check(&lockfile);
    let diags: Vec<Diagnostic> = findings.iter().map(finding_to_diagnostic).collect();

    // Findings carry no source labels, so the root-based lookup is never
    // invoked — rendering is identical to a no-op lookup.
    let counts = crate::project::render_diagnostics(&diags, Format::Human, &root);
    let (errors, warns) = (counts.errors, counts.warnings);
    println!(
        "audit: {} advisory(ies) loaded, {} finding(s) ({errors} error, {warns} warning) \
         against {} locked package(s)",
        db.len(),
        findings.len(),
        lockfile.packages.len()
    );

    if errors > 0 {
        bail!("audit failed with {errors} error-severity finding(s)");
    }
    Ok(())
}

/// Renders one match as an `LB1100` diagnostic. Findings carry no source
/// span (an advisory is about a dependency version, not a line of code), so
/// the diagnostic has no labels — just the headline plus notes.
fn finding_to_diagnostic(finding: &Finding) -> Diagnostic {
    let severity = match finding.advisory.severity {
        AdvisorySeverity::Critical | AdvisorySeverity::High => Severity::Error,
        AdvisorySeverity::Medium | AdvisorySeverity::Low => Severity::Warning,
    };
    let mut diag = Diagnostic::new(
        Code::new(1100),
        severity,
        format!(
            "{} {} {}: {} ({})",
            finding.advisory.id,
            finding.package,
            finding.version,
            finding.advisory.title,
            finding.advisory.severity
        ),
    )
    .with_note(finding.advisory.description.clone());
    if let Some(url) = &finding.advisory.url {
        diag = diag.with_note(format!("more info: {url}"));
    }
    if !finding.advisory.patched.is_empty() {
        let versions: Vec<String> = finding
            .advisory
            .patched
            .iter()
            .map(ToString::to_string)
            .collect();
        diag = diag.with_note(format!("patched versions: {}", versions.join(" or ")));
    }
    diag
}

/// Nearest `luabox.toml` walking up from `cwd`, cargo-style — mirrors
/// `deps_cmd::discover`. Auditing needs a project root to find
/// `luabox.lock`; a manifest-less directory has nothing to audit.
fn discover(cwd: &Path) -> anyhow::Result<PathBuf> {
    crate::project::require_root(cwd)
}

/// Resolves the advisory database directory: `LUABOX_ADVISORY_DB` if it
/// names an existing directory, else `<home>/.luabox/advisory-db` if that
/// exists. `None` means "nothing configured" either way — including an
/// `LUABOX_ADVISORY_DB` that points nowhere, which is treated the same as
/// unset rather than a hard error (see module docs: absence must never fail
/// the command).
fn advisory_db_dir() -> Option<PathBuf> {
    if let Ok(dir) = env::var("LUABOX_ADVISORY_DB")
        && !dir.trim().is_empty()
    {
        let path = PathBuf::from(dir);
        return path.is_dir().then_some(path);
    }
    let home = home_dir()?;
    let default = home.join(".luabox").join("advisory-db");
    default.is_dir().then_some(default)
}

/// `$HOME` (unix) / `%USERPROFILE%` (windows). Unlike `deps_cmd::home_dir`,
/// a missing home directory is not an error here — it just means the
/// default database location cannot be checked, which folds into "no
/// database configured".
fn home_dir() -> Option<PathBuf> {
    for var in ["HOME", "USERPROFILE"] {
        if let Ok(dir) = env::var(var)
            && !dir.trim().is_empty()
        {
            return Some(PathBuf::from(dir));
        }
    }
    None
}

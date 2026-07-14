//! `luabox outdated` — report dependencies behind their latest release.
//!
//! Reads the project manifest's `[dependencies]` and `[dev-dependencies]` and,
//! for each **git** dependency on a GitHub repo, compares the pinned tag to the
//! repo's latest release tag (SPEC.md §6: git deps are luabox's registry-less
//! install mechanism). Non-git deps are listed with their kind and never marked
//! outdated. Always exits 0 — it is a report, not a gate.
//!
//! `--format json` emits the frozen contract the editor GUIs are built against;
//! the default `text` renders a table and a summary.

use std::path::Path;

use anyhow::{Result, bail};
use luabox_resolve::manifest::Dependency;
use serde::Serialize;

use crate::github;

/// One dependency row in the frozen `--format json` contract.
#[derive(Debug, Serialize)]
struct DepReport {
    name: String,
    /// `git` | `path` | `workspace` | `registry`.
    kind: &'static str,
    /// `owner/name` for a GitHub git dep, else `null`.
    repo: Option<String>,
    /// The git URL for a git dep, else `null`.
    url: Option<String>,
    /// The current pin: a git tag/rev/branch, or a registry version req, else
    /// `null`.
    current: Option<String>,
    /// The repo's latest release tag (git+GitHub only), else `null`.
    latest: Option<String>,
    outdated: bool,
}

/// The frozen top-level JSON envelope: `{"dependencies":[…]}`.
#[derive(Debug, Serialize)]
struct OutdatedOutput {
    dependencies: Vec<DepReport>,
}

/// Whether a **tag**-pinned git dep is outdated: a newer release tag exists and
/// differs from the current pin. A missing latest (no releases) is not
/// outdated — there is nothing to move to.
fn tag_outdated(current: &str, latest: Option<&str>) -> bool {
    matches!(latest, Some(latest) if latest != current)
}

/// `luabox outdated`: build one report row per manifest dependency, render.
pub fn run(cwd: &Path, format: &str) -> Result<()> {
    match format {
        "json" | "text" => {}
        other => bail!("unknown --format `{other}`; expected `text` or `json`"),
    }

    let (_root, manifest) = crate::project::discover_required(cwd)?;
    let token = github::token();

    // `[dependencies]` then `[dev-dependencies]`, each already name-sorted
    // (BTreeMap), so the report order is stable.
    let deps = manifest
        .dependencies
        .iter()
        .chain(manifest.dev_dependencies.iter());

    let mut reports = Vec::new();
    for (name, dep) in deps {
        reports.push(report_for(name, dep, token.as_deref())?);
    }

    if format == "json" {
        let output = OutdatedOutput {
            dependencies: reports,
        };
        println!(
            "{}",
            serde_json::to_string_pretty(&output)
                .unwrap_or_else(|_| "{\"dependencies\":[]}".to_owned())
        );
        return Ok(());
    }

    render_text(&reports);
    Ok(())
}

/// Builds the report row for one dependency, hitting GitHub only for git deps
/// on a GitHub repo.
fn report_for(name: &str, dep: &Dependency, token: Option<&str>) -> Result<DepReport> {
    match dep {
        Dependency::Version(req) => Ok(DepReport {
            name: name.to_owned(),
            kind: "registry",
            repo: None,
            url: None,
            current: Some(req.clone()),
            latest: None,
            outdated: false,
        }),
        Dependency::Path(_) => Ok(plain(name, "path")),
        Dependency::Workspace(_) => Ok(plain(name, "workspace")),
        Dependency::Git(git) => {
            // The current pin: a tag is comparable to release tags; a rev or
            // branch is not, so `latest` stays informational (see below).
            let current = git
                .tag
                .clone()
                .or_else(|| git.rev.clone())
                .or_else(|| git.branch.clone());
            let repo = github::parse_github_repo(&git.git);

            // Only a GitHub repo yields a latest release tag.
            let latest = match &repo {
                Some(full_name) => github::latest_release_tag(full_name, token)?,
                None => None,
            };

            // Outdated only for a tag pin — a rev/branch pin is left alone by
            // `luabox update`, so reporting it "outdated" would be misleading;
            // `latest` is still surfaced for information.
            let outdated = match &git.tag {
                Some(tag) => tag_outdated(tag, latest.as_deref()),
                None => false,
            };

            Ok(DepReport {
                name: name.to_owned(),
                kind: "git",
                repo,
                url: Some(git.git.clone()),
                current,
                latest,
                outdated,
            })
        }
    }
}

/// A report row for a dependency with no version/pin surface (path/workspace).
fn plain(name: &str, kind: &'static str) -> DepReport {
    DepReport {
        name: name.to_owned(),
        kind,
        repo: None,
        url: None,
        current: None,
        latest: None,
        outdated: false,
    }
}

/// The human `text` rendering: a table plus an "N of M are outdated" summary.
fn render_text(reports: &[DepReport]) {
    if reports.is_empty() {
        println!("no dependencies declared in luabox.toml");
        return;
    }

    let name_width = reports
        .iter()
        .map(|r| r.name.chars().count())
        .max()
        .unwrap_or(4)
        .max(4);
    let current_width = reports
        .iter()
        .map(|r| r.current.as_deref().unwrap_or("-").chars().count())
        .max()
        .unwrap_or(7)
        .max(7);
    let latest_width = reports
        .iter()
        .map(|r| r.latest.as_deref().unwrap_or("-").chars().count())
        .max()
        .unwrap_or(6)
        .max(6);

    println!(
        "{:<name_width$}  {:<8}  {:<current_width$}  {:<latest_width$}  STATUS",
        "NAME", "KIND", "CURRENT", "LATEST"
    );
    for report in reports {
        let status = if report.outdated { "OUTDATED" } else { "ok" };
        println!(
            "{:<name_width$}  {:<8}  {:<current_width$}  {:<latest_width$}  {status}",
            report.name,
            report.kind,
            report.current.as_deref().unwrap_or("-"),
            report.latest.as_deref().unwrap_or("-"),
        );
    }

    let outdated = reports.iter().filter(|r| r.outdated).count();
    println!();
    println!(
        "{outdated} of {} dependency(ies) are outdated",
        reports.len()
    );
}

#[cfg(test)]
mod tests {
    use super::tag_outdated;

    #[test]
    fn tag_behind_latest_is_outdated() {
        assert!(tag_outdated("v0.1.0", Some("v0.1.2")));
    }

    #[test]
    fn tag_at_latest_is_current() {
        assert!(!tag_outdated("v0.1.2", Some("v0.1.2")));
    }

    #[test]
    fn no_latest_release_is_not_outdated() {
        // A repo with no releases has nothing to move to.
        assert!(!tag_outdated("v0.1.0", None));
    }
}

//! `luabox outdated` — report dependencies behind their latest available version.
//!
//! Reports every dependency of the resolved project (the rockspec's registry
//! deps fused with `luabox.toml`'s `path`/`git`/`workspace` sources, SPEC.md §6):
//!
//! * **registry** deps (rockspec-declared, resolved from luarocks.org) compare
//!   the **locked** version (from `luabox.lock`) against the highest version in
//!   the luarocks manifest — outdated when a newer one exists.
//! * **git** deps on a GitHub repo compare the pinned tag against the repo's
//!   latest GitHub release (release probing, anonymous or `LUABOX_GITHUB_TOKEN`).
//! * **path**/**workspace** deps (and non-GitHub / rev/branch git pins) are
//!   listed with their kind and never marked outdated.
//!
//! Always exits 0 — it is a report, not a gate. `--format json` emits the frozen
//! `{"dependencies":[…]}` contract the editor GUIs are built against; the
//! default `text` renders a table and a summary.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Result, bail};
use luabox_resolve::manifest::Dependency;
use luabox_resolve::{
    LOCKFILE_NAME, LockedSource, Lockfile, LuaRocksProvider, PackageId, PackageProvider,
};
use semver::Version;
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
    /// The current pin: a git tag/rev/branch, or a registry dep's locked
    /// version (its version requirement when unlocked), else `null`.
    current: Option<String>,
    /// The latest available version: a git+GitHub repo's latest release tag, or
    /// a registry rock's highest luarocks.org version, else `null`.
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

/// `luabox outdated`: build one report row per resolved dependency, render.
pub fn run(cwd: &Path, format: &str) -> Result<()> {
    match format {
        "json" | "text" => {}
        other => bail!("unknown --format `{other}`; expected `text` or `json`"),
    }

    let project = crate::deps_cmd::discover(cwd)?;
    // The resolved graph: rockspec registry deps fused with luabox.toml sources.
    let manifest = project.resolved_manifest()?;
    let token = github::token();
    let locked = load_locked_registry(&project.root);
    let luarocks = LuaRocksProvider::from_env(crate::deps_cmd::store_root()?.join("luarocks"));

    // `[dependencies]` then `[dev-dependencies]`, each already name-sorted
    // (BTreeMap), so the report order is stable.
    let deps = manifest
        .dependencies
        .iter()
        .chain(manifest.dev_dependencies.iter());

    let mut reports = Vec::new();
    for (name, dep) in deps {
        reports.push(report_for(name, dep, token.as_deref(), &luarocks, &locked)?);
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

/// The locked concrete version of every registry package in `luabox.lock`,
/// keyed by name. A missing or unparseable lockfile yields an empty map (the
/// report simply has no locked version to compare) — outdated never fails on it.
fn load_locked_registry(root: &Path) -> BTreeMap<String, Version> {
    let Ok(text) = std::fs::read_to_string(root.join(LOCKFILE_NAME)) else {
        return BTreeMap::new();
    };
    let Ok(lock) = Lockfile::parse(&text) else {
        return BTreeMap::new();
    };
    lock.packages
        .into_iter()
        .filter(|package| matches!(package.source, Some(LockedSource::Registry)))
        .map(|package| (package.name, package.version))
        .collect()
}

/// The highest version of registry rock `name` on luarocks.org (or the mirror),
/// or `None` when the rock is unknown/unreachable — a report never fails on it.
fn registry_latest(luarocks: &LuaRocksProvider, name: &str) -> Option<Version> {
    luarocks
        .list_versions(&PackageId::registry(name))
        .ok()
        .and_then(|versions| versions.into_iter().max())
}

/// Builds the report row for one dependency, reaching luarocks.org for registry
/// deps and GitHub only for git deps on a GitHub repo.
fn report_for(
    name: &str,
    dep: &Dependency,
    token: Option<&str>,
    luarocks: &LuaRocksProvider,
    locked: &BTreeMap<String, Version>,
) -> Result<DepReport> {
    match dep {
        Dependency::Version(req) => {
            // Registry (luarocks.org) dep: compare the locked version against
            // the highest version the registry offers.
            let current = locked.get(name);
            let latest = registry_latest(luarocks, name);
            let outdated = matches!((current, &latest), (Some(cur), Some(lat)) if lat > cur);
            Ok(DepReport {
                name: name.to_owned(),
                kind: "registry",
                repo: None,
                url: None,
                // Prefer the concrete locked version; fall back to the version
                // requirement when the project has not been installed yet.
                current: current
                    .map(Version::to_string)
                    .or_else(|| Some(req.clone())),
                latest: latest.map(|version| version.to_string()),
                outdated,
            })
        }
        Dependency::Path(_) => Ok(plain(name, "path")),
        // A url tarball is pinned by sha256 — immutable content, never outdated.
        Dependency::Url(_) => Ok(plain(name, "url")),
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
        println!("no dependencies declared in luabox.toml or the rockspec");
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

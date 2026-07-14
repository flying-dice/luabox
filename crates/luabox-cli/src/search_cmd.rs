//! `luabox search <query>` — discover luabox packages on GitHub.
//!
//! luabox has no hosted registry (SPEC.md §6). A **package** is a public
//! GitHub repository that carries the topic `luabox` **and** a root
//! `luabox.toml`; the topic finds candidates and the manifest presence filter
//! excludes the toolchain/editor repos (which carry the topic but ship no root
//! `luabox.toml`). Installing a hit is `luabox add <name> --git <url> --tag
//! <latest>` — a git dependency pinned to the repo's latest release.
//!
//! `--format json` emits the frozen contract the editor GUIs are built against;
//! the default `text` renders a human table.

use anyhow::{Result, bail};
use serde::Serialize;

use crate::github;

/// One discovered package in the frozen `--format json` contract.
#[derive(Debug, Serialize)]
struct SearchResult {
    /// The `[package] name` from the repo's `luabox.toml` (repo name if
    /// unreadable).
    name: String,
    /// `owner/name`.
    repo: String,
    /// The repo's GitHub URL — what `luabox add --git` takes.
    url: String,
    description: Option<String>,
    stars: u64,
    /// The latest release tag — what `luabox add --tag` pins to (`null` when
    /// the repo has no releases or tags).
    latest: Option<String>,
    topics: Vec<String>,
}

/// The frozen top-level JSON envelope: `{"results":[…]}`.
#[derive(Debug, Serialize)]
struct SearchOutput {
    results: Vec<SearchResult>,
}

/// `luabox search`: query GitHub, filter to real packages, render.
pub fn run(query: Option<&str>, format: &str) -> Result<()> {
    match format {
        "json" | "text" => {}
        other => bail!("unknown --format `{other}`; expected `text` or `json`"),
    }

    let token = github::token();
    let (repos, total) = github::search(query.unwrap_or(""), token.as_deref())?;

    let mut results = Vec::new();
    for repo in repos {
        // The topic search returns the candidate; the root `luabox.toml` is the
        // filter that separates packages from toolchain/editor repos.
        let Some(manifest) = github::root_manifest(&repo.full_name, &repo.default_branch)? else {
            continue;
        };
        let name = github::parse_package_name(&manifest).unwrap_or_else(|| {
            // Fall back to the bare repo name when the manifest has no
            // `[package] name` (or is unreadable as TOML).
            repo.full_name
                .split('/')
                .next_back()
                .unwrap_or(&repo.full_name)
                .to_owned()
        });
        let latest = github::latest_release_tag(&repo.full_name, token.as_deref())?;
        results.push(SearchResult {
            name,
            repo: repo.full_name,
            url: repo.html_url,
            description: repo.description,
            stars: repo.stargazers_count,
            latest,
            topics: repo.topics,
        });
    }

    if format == "json" {
        let output = SearchOutput { results };
        println!(
            "{}",
            serde_json::to_string_pretty(&output).unwrap_or_else(|_| "{\"results\":[]}".to_owned())
        );
        return Ok(());
    }

    render_text(query.unwrap_or("").trim(), &results, total, token.is_some());
    Ok(())
}

/// The human `text` rendering: a table of packages, or a clear empty note, plus
/// a truncation line when GitHub had more topic matches than the page size.
fn render_text(query: &str, results: &[SearchResult], total: u64, authed: bool) {
    if results.is_empty() {
        if query.is_empty() {
            println!("no luabox packages found on GitHub (topic:luabox + a root luabox.toml)");
        } else {
            println!("no luabox packages found for `{query}` (topic:luabox + a root luabox.toml)");
        }
        note_scanned(total);
        if !authed {
            println!(
                "note: unauthenticated (60 req/hr); set LUABOX_GITHUB_TOKEN or GITHUB_TOKEN to raise the limit"
            );
        }
        return;
    }

    let name_width = results
        .iter()
        .map(|r| r.name.chars().count())
        .max()
        .unwrap_or(4)
        .max(4);
    let latest_width = results
        .iter()
        .map(|r| r.latest.as_deref().unwrap_or("-").chars().count())
        .max()
        .unwrap_or(6)
        .max(6);

    println!(
        "{:<name_width$}  {:<latest_width$}  {:>6}  REPO",
        "NAME", "LATEST", "STARS"
    );
    for result in results {
        println!(
            "{:<name_width$}  {:<latest_width$}  {:>6}  {}",
            result.name,
            result.latest.as_deref().unwrap_or("-"),
            result.stars,
            result.repo
        );
        if let Some(description) = &result.description {
            println!("{:name_width$}    {description}", "");
        }
    }
    println!();
    println!(
        "{} package(s) found. Install one with: luabox add <name> --git <url> --tag <latest>",
        results.len()
    );
    note_scanned(total);
}

/// Notes how many topic matches were scanned, and whether the page truncated.
fn note_scanned(total: u64) {
    if total > github::SEARCH_LIMIT as u64 {
        println!(
            "note: showing the top {} of {total} topic:luabox repositories (by stars); refine your query to narrow",
            github::SEARCH_LIMIT
        );
    }
}

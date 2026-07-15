//! `luabox search [query]` — discover rocks on luarocks.org (the registry).
//!
//! luabox follows the pnpm/bun model: [luarocks.org](https://luarocks.org)
//! **is** the registry (SPEC.md §6). Search reads the same root `manifest.json`
//! the resolver's [`LuaRocksProvider`] bridge does (cached under
//! `<store>/luarocks/`, or a `LUABOX_LUAROCKS_MIRROR` for hermetic/offline
//! use), and matches the query as a case-insensitive substring of rock names.
//! It is an anonymous registry read — no GitHub, no token.
//!
//! A listing carries no per-rock description: the manifest does not include one
//! and fetching a rockspec per rock just to list is wasteful (and unfriendly to
//! offline mirrors), so `description` is honestly `null`. For the same reason a
//! rock's **supported-dialect family set** (#5) is *not* surfaced here: it lives
//! in the per-version rockspec's `lua` constraint, which the name-index listing
//! never fetches — adding a rockspec fetch per result would slow every search
//! for data most callers do not need. It surfaces at resolve time instead
//! (`LB1003`).
//!
//! `--format json` emits the frozen `{"results":[…]}` contract the editor GUIs
//! are built against; the default `text` renders an aligned table.

use anyhow::{Result, bail};
use luabox_resolve::LuaRocksProvider;
use serde::Serialize;

/// The most rocks an empty-query listing renders (the whole registry is tens of
/// thousands of rocks; a bare `luabox search` caps to the first page by name).
const LISTING_CAP: usize = 50;

/// One discovered rock in the frozen `--format json` contract.
#[derive(Debug, Serialize)]
struct SearchResult {
    /// The bare rock name (a luarocks.org `repository` key).
    name: String,
    /// The highest translated semver, or `null` when the rock has no
    /// numeric-versioned release.
    latest: Option<String>,
    /// How many distinct translated semver versions the rock has.
    versions: usize,
    /// Always `null` for a listing — the manifest carries no description and a
    /// listing never fetches per-rock rockspecs (see the module docs).
    description: Option<String>,
}

/// The frozen top-level JSON envelope: `{"results":[…]}`.
#[derive(Debug, Serialize)]
struct SearchOutput {
    results: Vec<SearchResult>,
}

/// `luabox search`: query luarocks.org, render.
pub fn run(query: Option<&str>, format: &str) -> Result<()> {
    match format {
        "json" | "text" => {}
        other => bail!("unknown --format `{other}`; expected `text` or `json`"),
    }

    let query = query.unwrap_or("").trim();
    let provider = LuaRocksProvider::from_env(crate::deps_cmd::store_root()?.join("luarocks"));
    let mut rocks = provider.search(query)?;

    // An empty query lists the whole registry; cap it (and note the cap in the
    // text rendering). A real query returns all its matches.
    let total = rocks.len();
    let capped = query.is_empty() && total > LISTING_CAP;
    if capped {
        rocks.truncate(LISTING_CAP);
    }

    let results: Vec<SearchResult> = rocks
        .into_iter()
        .map(|rock| SearchResult {
            name: rock.name,
            latest: rock.latest.map(|v| v.to_string()),
            versions: rock.version_count,
            description: None,
        })
        .collect();

    if format == "json" {
        let output = SearchOutput { results };
        println!(
            "{}",
            serde_json::to_string_pretty(&output).unwrap_or_else(|_| "{\"results\":[]}".to_owned())
        );
        return Ok(());
    }

    render_text(query, &results, total, capped);
    Ok(())
}

/// The human `text` rendering: aligned `NAME  LATEST  VERSIONS` rows, or a clear
/// empty note, plus a truncation line when an empty-query listing was capped.
fn render_text(query: &str, results: &[SearchResult], total: usize, capped: bool) {
    if results.is_empty() {
        if query.is_empty() {
            println!("no rocks found on luarocks.org");
        } else {
            println!("no rocks found for `{query}` on luarocks.org");
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
        "{:<name_width$}  {:<latest_width$}  VERSIONS",
        "NAME", "LATEST"
    );
    for result in results {
        println!(
            "{:<name_width$}  {:<latest_width$}  {}",
            result.name,
            result.latest.as_deref().unwrap_or("-"),
            result.versions
        );
    }
    println!();
    if capped {
        println!(
            "showing the first {} of {total} rocks (by name); refine your query to narrow",
            results.len()
        );
    } else {
        println!("{} rock(s) found on luarocks.org", results.len());
    }
}

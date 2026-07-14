//! GitHub-as-registry client — the seam `luabox search` / `luabox outdated`
//! (and `luabox update`'s re-pin) reach GitHub through.
//!
//! luabox has no hosted registry (SPEC.md §6): a "package" is a **public
//! GitHub repo carrying the topic `luabox` and a root `luabox.toml`**, and
//! installing one is a git dependency pinned to its latest release tag. This
//! module discovers those repos and reads their release tags.
//!
//! ## Transport
//!
//! No HTTP crate is linked (SPEC.md §6): every request shells out to `curl`,
//! exactly as [`luabox_resolve`]'s transport and `luabox upgrade` do. Unlike
//! those call sites this one must branch on the HTTP status (a 404 on
//! `releases/latest` means "fall back to tags", a 404 on a raw `luabox.toml`
//! means "not a package"), so it reads `%{http_code}` rather than letting
//! `curl -f` collapse every non-2xx into one exit code.
//!
//! ## Auth
//!
//! An optional token from `LUABOX_GITHUB_TOKEN` (else `GITHUB_TOKEN`) is sent
//! as `Authorization: Bearer <t>`, raising GitHub's anonymous 60 req/hr search
//! limit to 5000/hr. Everything degrades gracefully without one.
//!
//! ## Parsing
//!
//! The response bodies are parsed with `serde_json` into the minimal structs
//! below — never by byte-index slicing (`string_slice` is denied on production
//! paths, and it panics on UTF-8 boundaries).

use std::env;
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

/// How long any single GitHub request may take, in seconds.
const HTTP_TIMEOUT_SECS: u32 = 30;

/// The most repositories a search reports on (the frozen contract bounds the
/// result set; GitHub is asked for exactly this page size).
pub(crate) const SEARCH_LIMIT: usize = 30;

/// One repository from `GET /search/repositories`, trimmed to the fields the
/// discovery contract needs.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Repo {
    /// `owner/name`.
    pub(crate) full_name: String,
    pub(crate) html_url: String,
    pub(crate) description: Option<String>,
    #[serde(default)]
    pub(crate) stargazers_count: u64,
    #[serde(default)]
    pub(crate) topics: Vec<String>,
    /// The branch a raw `luabox.toml` fetch must target.
    #[serde(default = "default_branch")]
    pub(crate) default_branch: String,
}

fn default_branch() -> String {
    "main".to_owned()
}

/// `GET /search/repositories` envelope.
#[derive(Debug, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    total_count: u64,
    #[serde(default)]
    items: Vec<Repo>,
}

/// The one field of `GET /repos/{o}/{r}/releases/latest` we read.
#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
}

/// The one field of `GET /user` we read — the authenticated login.
#[derive(Debug, Deserialize)]
struct User {
    login: String,
}

/// One entry of `GET /repos/{o}/{r}/tags`.
#[derive(Debug, Deserialize)]
struct Tag {
    name: String,
}

/// A completed `curl` response: the HTTP status and the body text.
struct HttpResponse {
    status: u16,
    body: String,
}

/// Where an authentication token came from, for `luabox whoami` to report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TokenSource {
    /// A `LUABOX_GITHUB_TOKEN` / `GITHUB_TOKEN` environment variable.
    Env,
    /// The OS keychain, populated by `luabox login`.
    Keychain,
}

impl TokenSource {
    /// The stable `--format json` label (`"env"` / `"keychain"`).
    pub(crate) fn label(self) -> &'static str {
        match self {
            TokenSource::Env => "env",
            TokenSource::Keychain => "keychain",
        }
    }
}

/// Pure token-precedence resolution: `LUABOX_GITHUB_TOKEN` env wins, then
/// `GITHUB_TOKEN` env, then a keychain-stored token; blank values are ignored
/// at every level (an env var set to whitespace does not mask the keychain).
/// Env beats the keychain so CI and one-off overrides are always honored.
fn resolve_token(
    luabox_env: Option<&str>,
    github_env: Option<&str>,
    keychain: Option<&str>,
) -> Option<(String, TokenSource)> {
    for candidate in [luabox_env, github_env] {
        if let Some(value) = candidate
            && !value.trim().is_empty()
        {
            return Some((value.to_owned(), TokenSource::Env));
        }
    }
    if let Some(value) = keychain
        && !value.trim().is_empty()
    {
        return Some((value.to_owned(), TokenSource::Keychain));
    }
    None
}

/// The token to authenticate GitHub requests with and where it came from, or
/// `None` (anonymous). Reads the two env vars, then the OS keychain; a keychain
/// that cannot be reached is treated as "no stored token" (never fatal).
pub(crate) fn token_with_source() -> Option<(String, TokenSource)> {
    let luabox_env = env::var("LUABOX_GITHUB_TOKEN").ok();
    let github_env = env::var("GITHUB_TOKEN").ok();
    // A keychain read that errors (unavailable store) degrades to `None`.
    let keychain = crate::keychain::retrieve().ok().flatten();
    resolve_token(
        luabox_env.as_deref(),
        github_env.as_deref(),
        keychain.as_deref(),
    )
}

/// The token to authenticate GitHub requests with, or `None` (anonymous).
/// `LUABOX_GITHUB_TOKEN` wins over `GITHUB_TOKEN`, then the keychain.
pub(crate) fn token() -> Option<String> {
    token_with_source().map(|(token, _)| token)
}

/// `curl` `url`, returning the HTTP status and body. GitHub API headers are
/// always sent; a bearer token is attached when `token` is `Some`. Does **not**
/// fail on non-2xx status — the caller inspects [`HttpResponse::status`].
fn http_get(url: &str, token: Option<&str>) -> Result<HttpResponse> {
    let mut command = Command::new("curl");
    command.args([
        "-sSL",
        "--max-time",
        &HTTP_TIMEOUT_SECS.to_string(),
        "-H",
        "Accept: application/vnd.github+json",
        "-H",
        "X-GitHub-Api-Version: 2022-11-28",
        // Append the HTTP status on its own trailing line so we can split it
        // back off the body without a second request.
        "-w",
        "\n%{http_code}",
    ]);
    if let Some(token) = token {
        command
            .arg("-H")
            .arg(format!("Authorization: Bearer {token}"));
    }
    command.arg(url);

    let output = command
        .output()
        .with_context(|| format!("running `curl` for {url}"))?;
    if !output.status.success() {
        bail!(
            "`curl` failed for {url} (exit {}): {}",
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let (body, code) = text
        .rsplit_once('\n')
        .with_context(|| format!("{url} returned no HTTP status line"))?;
    let status: u16 = code
        .trim()
        .parse()
        .with_context(|| format!("{url} returned an unparseable HTTP status `{code}`"))?;
    Ok(HttpResponse {
        status,
        body: body.to_owned(),
    })
}

/// Turns a non-2xx GitHub API status into a helpful error (rate-limit guidance
/// on 403/429, otherwise the status and body).
fn api_error(url: &str, response: &HttpResponse, authed: bool) -> anyhow::Error {
    if matches!(response.status, 403 | 429) {
        let hint = if authed {
            "authenticated GitHub rate limit reached; wait and retry"
        } else {
            "GitHub anonymous rate limit reached (60 req/hr); set LUABOX_GITHUB_TOKEN \
             or GITHUB_TOKEN to raise it to 5000/hr"
        };
        return anyhow::anyhow!("{url}: HTTP {} — {hint}", response.status);
    }
    anyhow::anyhow!("{url}: HTTP {} — {}", response.status, response.body.trim())
}

/// Search public repositories carrying the `luabox` topic, narrowed by
/// `query` terms, sorted by stars. Returns the page (bounded to
/// [`SEARCH_LIMIT`]) and the total match count (for a truncation note).
pub(crate) fn search(query: &str, token: Option<&str>) -> Result<(Vec<Repo>, u64)> {
    // `topic:luabox` is always present; free-text terms narrow name/description
    // /README the way GitHub search does. Spaces become `+` in the query
    // component; the terms are appended after the qualifier.
    let mut q = String::from("topic:luabox");
    let terms = query.trim();
    if !terms.is_empty() {
        q.push(' ');
        q.push_str(terms);
    }
    let encoded = q.replace(' ', "+");
    let url = format!(
        "https://api.github.com/search/repositories?q={encoded}&sort=stars&order=desc&per_page={SEARCH_LIMIT}"
    );
    let response = http_get(&url, token)?;
    if response.status != 200 {
        return Err(api_error(&url, &response, token.is_some()));
    }
    let parsed = parse_search(&response.body).context("parsing the GitHub search response")?;
    Ok((parsed.items, parsed.total_count))
}

/// Fetch the root `luabox.toml` of `repo` on `branch` from
/// `raw.githubusercontent.com` (public content, un-rate-limited and un-authed),
/// or `None` when the repo has no such file (HTTP 404 — not a package).
pub(crate) fn root_manifest(full_name: &str, branch: &str) -> Result<Option<String>> {
    let url = format!("https://raw.githubusercontent.com/{full_name}/{branch}/luabox.toml");
    let response = http_get(&url, None)?;
    match response.status {
        200 => Ok(Some(response.body)),
        404 => Ok(None),
        _ => Err(api_error(&url, &response, false)),
    }
}

/// The latest release tag of `owner/name`: `releases/latest`'s `tag_name`,
/// falling back to the newest entry of the `tags` list when a repo has tags but
/// no published release, and `None` when it has neither.
pub(crate) fn latest_release_tag(full_name: &str, token: Option<&str>) -> Result<Option<String>> {
    let release_url = format!("https://api.github.com/repos/{full_name}/releases/latest");
    let response = http_get(&release_url, token)?;
    match response.status {
        200 => {
            if let Some(tag) = parse_latest_release(&response.body) {
                return Ok(Some(tag));
            }
        }
        404 => {}
        _ => return Err(api_error(&release_url, &response, token.is_some())),
    }

    // No published release — fall back to the newest tag.
    let tags_url = format!("https://api.github.com/repos/{full_name}/tags?per_page=1");
    let response = http_get(&tags_url, token)?;
    match response.status {
        200 => Ok(parse_first_tag(&response.body)),
        404 => Ok(None),
        _ => Err(api_error(&tags_url, &response, token.is_some())),
    }
}

/// The authenticated user's login from `GET /user` (used by `luabox login`'s
/// final confirmation and by `luabox whoami`). Requires a valid `token`.
pub(crate) fn authenticated_login(token: &str) -> Result<String> {
    let url = "https://api.github.com/user";
    let response = http_get(url, Some(token))?;
    if response.status != 200 {
        return Err(api_error(url, &response, true));
    }
    parse_user_login(&response.body)
        .with_context(|| format!("{url} response was not the expected JSON shape"))
}

// --- pure parsing (unit-tested; no IO) --------------------------------------

/// Extract the `login` from a `GET /user` body (`None` if absent/malformed).
fn parse_user_login(json: &str) -> Option<String> {
    serde_json::from_str::<User>(json)
        .ok()
        .map(|user| user.login)
}

/// Parse a `GET /search/repositories` body into its items and total count.
fn parse_search(json: &str) -> Result<SearchResponse> {
    serde_json::from_str(json).context("GitHub search response was not the expected JSON shape")
}

/// Extract `tag_name` from a `releases/latest` body (`None` if absent/malformed).
fn parse_latest_release(json: &str) -> Option<String> {
    serde_json::from_str::<Release>(json)
        .ok()
        .map(|release| release.tag_name)
}

/// The newest tag name from a `tags` list body (the list is newest-first).
fn parse_first_tag(json: &str) -> Option<String> {
    serde_json::from_str::<Vec<Tag>>(json)
        .ok()
        .and_then(|tags| tags.into_iter().next())
        .map(|tag| tag.name)
}

/// The `[package] name` of a `luabox.toml`, or `None` when it is unreadable or
/// declares no package name. Parsed with `toml_edit` (already a dependency),
/// never by hand-scanning.
pub(crate) fn parse_package_name(manifest_toml: &str) -> Option<String> {
    let doc = manifest_toml.parse::<toml_edit::DocumentMut>().ok()?;
    doc.get("package")?.get("name")?.as_str().map(str::to_owned)
}

/// Extract `owner/name` from a GitHub repository URL (`https://github.com/o/n`,
/// `https://github.com/o/n.git`, `git@github.com:o/n.git`, or a bare
/// `github.com/o/n`), or `None` when the URL is not a GitHub repo. Uses `split`
/// throughout — never byte-index slicing.
pub(crate) fn parse_github_repo(url: &str) -> Option<String> {
    // Everything after the host, whether the separator is `/` (https) or `:`
    // (scp-style ssh).
    let after_host = url.split("github.com").nth(1)?;
    let path = after_host.trim_start_matches([':', '/']);
    let mut segments = path.split('/').filter(|segment| !segment.is_empty());
    let owner = segments.next()?;
    let name = segments.next()?.trim_end_matches(".git");
    if owner.is_empty() || name.is_empty() {
        return None;
    }
    Some(format!("{owner}/{name}"))
}

#[cfg(test)]
mod tests {
    use super::{
        TokenSource, parse_first_tag, parse_github_repo, parse_latest_release, parse_package_name,
        parse_search, parse_user_login, resolve_token,
    };

    #[test]
    fn parses_user_login() {
        let json = r#"{"login":"octocat","id":1,"type":"User"}"#;
        assert_eq!(parse_user_login(json).as_deref(), Some("octocat"));
    }

    #[test]
    fn malformed_user_body_is_none() {
        assert_eq!(parse_user_login(r#"{"message":"Bad credentials"}"#), None);
    }

    #[test]
    fn env_token_wins_over_keychain() {
        let resolved = resolve_token(Some("env-tok"), None, Some("kc-tok"));
        assert_eq!(resolved, Some(("env-tok".to_owned(), TokenSource::Env)));
    }

    #[test]
    fn luabox_env_wins_over_github_env() {
        let resolved = resolve_token(Some("luabox"), Some("github"), None);
        assert_eq!(resolved, Some(("luabox".to_owned(), TokenSource::Env)));
    }

    #[test]
    fn github_env_used_when_luabox_absent() {
        let resolved = resolve_token(None, Some("github"), Some("kc"));
        assert_eq!(resolved, Some(("github".to_owned(), TokenSource::Env)));
    }

    #[test]
    fn blank_env_does_not_mask_keychain() {
        // An env var set to whitespace is ignored, so the keychain token wins.
        let resolved = resolve_token(Some("   "), Some(""), Some("kc-tok"));
        assert_eq!(resolved, Some(("kc-tok".to_owned(), TokenSource::Keychain)));
    }

    #[test]
    fn keychain_used_when_no_env() {
        let resolved = resolve_token(None, None, Some("kc-tok"));
        assert_eq!(resolved, Some(("kc-tok".to_owned(), TokenSource::Keychain)));
    }

    #[test]
    fn no_token_anywhere_is_anonymous() {
        assert_eq!(resolve_token(None, None, None), None);
        assert_eq!(resolve_token(Some(""), Some("  "), Some("")), None);
    }

    #[test]
    fn token_source_labels_are_stable() {
        assert_eq!(TokenSource::Env.label(), "env");
        assert_eq!(TokenSource::Keychain.label(), "keychain");
    }

    #[test]
    fn parses_search_items_and_total() {
        let json = r#"{
            "total_count": 42,
            "incomplete_results": false,
            "items": [
                {
                    "full_name": "flying-dice/luabox-vscode",
                    "html_url": "https://github.com/flying-dice/luabox-vscode",
                    "description": "VS Code extension",
                    "stargazers_count": 7,
                    "topics": ["lsp", "luabox"],
                    "default_branch": "main"
                }
            ]
        }"#;
        let parsed = parse_search(json).expect("valid search JSON");
        assert_eq!(parsed.total_count, 42);
        assert_eq!(parsed.items.len(), 1);
        let repo = &parsed.items[0];
        assert_eq!(repo.full_name, "flying-dice/luabox-vscode");
        assert_eq!(repo.stargazers_count, 7);
        assert_eq!(repo.topics, ["lsp", "luabox"]);
        assert_eq!(repo.default_branch, "main");
    }

    #[test]
    fn search_tolerates_missing_optional_fields() {
        // A null description and absent topics/default_branch must not error.
        let json = r#"{"total_count":1,"items":[{
            "full_name":"o/n","html_url":"https://github.com/o/n","description":null
        }]}"#;
        let parsed = parse_search(json).expect("valid search JSON");
        let repo = &parsed.items[0];
        assert!(repo.description.is_none());
        assert!(repo.topics.is_empty());
        assert_eq!(repo.default_branch, "main");
    }

    #[test]
    fn parses_latest_release_tag() {
        let json = r#"{"tag_name":"v0.1.2","name":"luabox 0.1.2","draft":false}"#;
        assert_eq!(parse_latest_release(json).as_deref(), Some("v0.1.2"));
    }

    #[test]
    fn latest_release_absent_is_none() {
        assert_eq!(parse_latest_release(r#"{"message":"Not Found"}"#), None);
    }

    #[test]
    fn parses_first_tag_from_list() {
        let json = r#"[{"name":"v0.3.0"},{"name":"v0.2.0"},{"name":"v0.1.0"}]"#;
        assert_eq!(parse_first_tag(json).as_deref(), Some("v0.3.0"));
    }

    #[test]
    fn empty_tag_list_is_none() {
        assert_eq!(parse_first_tag("[]"), None);
    }

    #[test]
    fn extracts_package_name_from_manifest() {
        let toml = "[package]\nname = \"cool-lib\"\nversion = \"1.0.0\"\nedition = \"5.4\"\n";
        assert_eq!(parse_package_name(toml).as_deref(), Some("cool-lib"));
    }

    #[test]
    fn missing_package_name_is_none() {
        assert_eq!(parse_package_name("[build]\ntarget = \"5.1\"\n"), None);
        assert_eq!(parse_package_name("not : valid = toml ="), None);
    }

    #[test]
    fn parses_github_repo_from_every_url_shape() {
        for (url, expected) in [
            (
                "https://github.com/flying-dice/luabox",
                "flying-dice/luabox",
            ),
            (
                "https://github.com/flying-dice/luabox.git",
                "flying-dice/luabox",
            ),
            (
                "https://github.com/flying-dice/luabox/",
                "flying-dice/luabox",
            ),
            (
                "git@github.com:flying-dice/luabox.git",
                "flying-dice/luabox",
            ),
            ("github.com/owner/name", "owner/name"),
            ("http://github.com/owner/name.git", "owner/name"),
        ] {
            assert_eq!(parse_github_repo(url).as_deref(), Some(expected), "{url}");
        }
    }

    #[test]
    fn non_github_url_has_no_repo() {
        assert_eq!(parse_github_repo("https://gitlab.com/owner/name.git"), None);
        assert_eq!(parse_github_repo("https://github.com/owner"), None);
        assert_eq!(parse_github_repo("not a url"), None);
    }
}

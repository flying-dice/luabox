//! `luabox publish [--dry-run]` — upload the authored rockspec to luarocks.org
//! (SPEC.md §6, GitHub issue #2).
//!
//! luabox follows the pnpm/bun model: luarocks.org **is** the registry and the
//! project's `*.rockspec` **is** the authored package manifest. `luabox
//! publish` is a thin proxy that gets a package *into* luarocks.org — it
//! uploads the rockspec you wrote, verbatim. It generates nothing: there is no
//! rockspec-compilation step (that model was superseded when the rockspec
//! became the manifest).
//!
//! # Pipeline
//!
//! 1. **Gates** (all local — no network — and in order):
//!    - a root `*.rockspec` exists (it is the package manifest);
//!    - it parses statically and carries `package`, `version`, and a
//!      `source.url`, and its filename is the canonical
//!      `<package>-<version>.rockspec`;
//!    - `luabox check` is green (the same gate the old publish used);
//!    - it is a **pure-Lua** rock (`build.type = builtin`, no C sources) — the
//!      same classification the luarocks bridge applies on the way in
//!      (SPEC.md §6: luabox is not a C build system).
//! 2. **`--dry-run`** stops here, printing the rockspec and the upload target.
//! 3. **API key** resolution: the `LUABOX_LUAROCKS_API_KEY` env override wins,
//!    else the OS keychain (populated by `luabox login --luarocks`). No key is
//!    an onboarding error, never a stack trace.
//! 4. **Upload** — a multipart POST of the rockspec file to
//!    `<base>/api/1/<key>/upload` via `curl` (no HTTP crate, SPEC.md §6). The
//!    key lives in the URL path, so it is **redacted** from every echoed
//!    command and error. The base URL is overridable with
//!    `LUABOX_LUAROCKS_URL` (hermetic tests, private mirrors).

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};
use luabox_resolve::luarocks::rockspec::{Build, ModuleSpec, Rockspec};

use crate::deps_cmd;

/// The luarocks.org base URL, overridable via [`LUAROCKS_URL_ENV`].
const DEFAULT_BASE_URL: &str = "https://luarocks.org";

/// Environment variable overriding the luarocks.org base URL (hermetic tests,
/// private mirrors). Mirrors the bridge's server-override seam.
const LUAROCKS_URL_ENV: &str = "LUABOX_LUAROCKS_URL";

/// How long the upload may take, in seconds.
const UPLOAD_TIMEOUT_SECS: u32 = 120;

/// Execute `luabox publish`. With `dry_run`, validate and preview only — no
/// network, no key required.
pub fn run(cwd: &Path, dry_run: bool) -> Result<()> {
    let project = deps_cmd::discover(cwd)?;

    // Gate a: a root rockspec is the package manifest — its absence is an
    // onboarding error, not a failure to find a file.
    let (rockspec_path, spec) = match (&project.rockspec_path, &project.rockspec) {
        (Some(path), Some(spec)) => (path.clone(), spec.clone()),
        _ => bail!("{}", no_rockspec_message()),
    };
    let file_name = rockspec_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let text = std::fs::read_to_string(&rockspec_path)
        .with_context(|| format!("cannot read `{}`", rockspec_path.display()))?;

    // Gate b: required fields + canonical filename.
    let upload = validate(&spec, &file_name)?;

    // Gate c: `luabox check` must be green (reuse the same gate the old
    // publish used).
    println!("publish: running `luabox check`");
    crate::check_cmd::run_once(&project.root, None, "human", None).map_err(|_| {
        anyhow!("publish blocked: `luabox check` reported errors (fix them and retry)")
    })?;

    // Gate d: pure-Lua only (SPEC.md §6).
    if let Err(why) = classify_pure_lua(&upload.package, &spec.build) {
        bail!("{why}");
    }

    let base_url = base_url();
    if dry_run {
        return preview(&text, &upload, &base_url);
    }

    let key = resolve_api_key()?;
    upload_rockspec(&base_url, &key, &rockspec_path, &upload)
}

/// The validated identity read off a rockspec, ready to publish.
#[derive(Debug)]
struct Upload {
    package: String,
    version: String,
}

/// Gate b: `package`, `version`, and `source.url` must be present, and the
/// filename must be the canonical `<package>-<version>.rockspec` luarocks.org
/// expects.
fn validate(spec: &Rockspec, file_name: &str) -> Result<Upload> {
    let package = spec
        .package
        .clone()
        .filter(|p| !p.trim().is_empty())
        .ok_or_else(|| {
            anyhow!("the rockspec has no `package` field; add `package = \"<name>\"`")
        })?;
    let version = spec
        .version
        .clone()
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| {
            anyhow!("the rockspec has no `version` field; add `version = \"<x.y.z>-<rev>\"`")
        })?;
    if spec.source.url.as_deref().unwrap_or("").trim().is_empty() {
        bail!(
            "the rockspec has no statically resolvable `source.url`; luarocks.org needs a \
             `source = {{ url = \"git+https://…\" }}` pointing at your published source"
        );
    }

    let expected = format!("{package}-{version}.rockspec");
    if file_name != expected {
        bail!(
            "the rockspec filename `{file_name}` does not match its contents: luarocks.org \
             requires the canonical `<package>-<version>.rockspec`, i.e. `{expected}` for \
             `package = \"{package}\"` / `version = \"{version}\"`. Rename the file (or fix \
             the fields) so they agree"
        );
    }
    Ok(Upload { package, version })
}

/// Gate d: refuse a C/native rock, mirroring the luarocks bridge's `classify`
/// rules (`build.type` make/cmake/command, native module sources, or
/// `external_dependencies`). luabox publishes pure-Lua rocks only — the same
/// policy it applies on the way *in* (SPEC.md §6).
fn classify_pure_lua(package: &str, build: &Build) -> std::result::Result<(), String> {
    let reject = |why: String| {
        Err(format!(
            "`{package}` is a C/native rock ({why}); luabox publishes pure-Lua rocks only \
             — it is a toolchain, not a C build system (SPEC.md §6). Publish C rocks with \
             the `luarocks` CLI"
        ))
    };
    if build.has_external_dependencies {
        return reject("it declares `external_dependencies`".to_owned());
    }
    match build.build_type.as_deref() {
        Some("make") => return reject("build.type = make".to_owned()),
        Some("cmake") => return reject("build.type = cmake".to_owned()),
        Some("command") => return reject("build.type = command".to_owned()),
        _ => {}
    }
    for (name, module) in &build.modules {
        match module {
            ModuleSpec::NativeFile(path) => {
                return reject(format!("module `{name}` has native source `{path}`"));
            }
            ModuleSpec::Native => {
                return reject(format!("module `{name}` compiles C sources"));
            }
            ModuleSpec::LuaFile(_) | ModuleSpec::Unknown => {}
        }
    }
    Ok(())
}

/// `--dry-run`: print the rockspec content and the exact upload target, plus
/// whether a key is configured, then exit 0 — no network, no key required.
// Uniform `Result` dispatch signature.
#[allow(clippy::unnecessary_wraps)]
fn preview(text: &str, upload: &Upload, base_url: &str) -> Result<()> {
    println!("--- rockspec ({}-{}) ---", upload.package, upload.version);
    print!("{text}");
    if !text.ends_with('\n') {
        println!();
    }
    println!("--- upload target ---");
    println!("  rock:    {}", upload.package);
    println!("  version: {}", upload.version);
    println!("  server:  {base_url}");
    let key_status = if crate::auth_cmd::luarocks_key_configured() {
        "configured"
    } else {
        "NOT configured (run `luabox login --luarocks` before publishing)"
    };
    println!("  api key: {key_status}");
    println!("dry run: nothing was uploaded.");
    Ok(())
}

/// Resolve the luarocks.org API key: the [`crate::auth_cmd::LUAROCKS_API_KEY_ENV`]
/// env override (non-blank) wins, else the keychain (via `luabox login
/// --luarocks`). A missing key is an onboarding error.
fn resolve_api_key() -> Result<String> {
    if let Ok(value) = std::env::var(crate::auth_cmd::LUAROCKS_API_KEY_ENV)
        && !value.trim().is_empty()
    {
        return Ok(value.trim().to_owned());
    }
    if let Some(key) = crate::keychain::retrieve_luarocks_key().ok().flatten()
        && !key.trim().is_empty()
    {
        return Ok(key.trim().to_owned());
    }
    bail!("{}", missing_key_message())
}

/// The base URL to upload to: [`LUAROCKS_URL_ENV`] if set (non-blank), else
/// [`DEFAULT_BASE_URL`]. A trailing slash is trimmed so path joins are clean.
fn base_url() -> String {
    let raw = std::env::var(LUAROCKS_URL_ENV)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_owned());
    raw.trim().trim_end_matches('/').to_owned()
}

/// The upload endpoint for `key` (which is embedded in the URL path — hence the
/// redaction elsewhere).
fn upload_url(base_url: &str, key: &str) -> String {
    format!("{base_url}/api/1/{key}/upload")
}

/// The `curl` argument vector for the multipart upload: the rockspec file goes
/// in the `rockspec_file` form field, and the HTTP status is appended on its
/// own trailing line (`-w`) so we can read it back without a second request.
///
/// `-f` is deliberately **not** used: luarocks.org returns its "already
/// exists" / validation errors as a JSON body with a 4xx status, and `-f`
/// would discard that body — so we read the status ourselves and surface the
/// server's message.
fn upload_args(url: &str, rockspec_path: &Path) -> Vec<String> {
    vec![
        "-sSL".to_owned(),
        "--max-time".to_owned(),
        UPLOAD_TIMEOUT_SECS.to_string(),
        "-F".to_owned(),
        format!("rockspec_file=@{}", rockspec_path.display()),
        "-w".to_owned(),
        "\n%{http_code}".to_owned(),
        url.to_owned(),
    ]
}

/// POST the rockspec to luarocks.org and report the outcome. The API key is
/// never logged: it is redacted from any echoed command or error text.
fn upload_rockspec(base_url: &str, key: &str, rockspec_path: &Path, upload: &Upload) -> Result<()> {
    let url = upload_url(base_url, key);
    println!(
        "publishing `{}-{}` to {} …",
        upload.package, upload.version, base_url
    );

    let output = Command::new("curl")
        .args(upload_args(&url, rockspec_path))
        .output()
        .map_err(|e| {
            anyhow!(
                "running `curl` to upload the rockspec: {}",
                redact(&e.to_string(), key)
            )
        })?;
    if !output.status.success() {
        bail!(
            "`curl` failed to upload the rockspec (exit {}): {}",
            output.status.code().unwrap_or(-1),
            redact(&String::from_utf8_lossy(&output.stderr), key).trim()
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let (status, body) = split_status(&stdout)
        .ok_or_else(|| anyhow!("luarocks.org returned no HTTP status line"))?;

    match parse_upload_response(status, body) {
        UploadOutcome::Ok { module_url } => {
            print!("published `{}-{}`", upload.package, upload.version);
            match module_url {
                Some(link) => println!(" — {link}"),
                None => println!(" to {base_url}"),
            }
            println!("consumers can now `luarocks install {}`", upload.package);
            Ok(())
        }
        UploadOutcome::Err(message) => bail!(
            "luarocks.org rejected the upload (HTTP {status}): {}",
            redact(&message, key)
        ),
    }
}

/// The outcome of parsing an upload response body against its HTTP status.
#[derive(Debug, PartialEq, Eq)]
enum UploadOutcome {
    /// Success; the module page URL when the server returned one.
    Ok { module_url: Option<String> },
    /// Failure carrying the server's message (a duplicate version, a
    /// validation error, …).
    Err(String),
}

/// Split curl's `<body>\n<status>` output (the `-w "\n%{http_code}"` trailer)
/// into a parsed status and the body. `None` if there is no trailing status.
fn split_status(text: &str) -> Option<(u16, &str)> {
    let (body, code) = text.rsplit_once('\n')?;
    let status = code.trim().parse::<u16>().ok()?;
    Some((status, body))
}

/// Interpret an upload response: a 2xx is success (with the module URL when the
/// server names one); anything else surfaces the server's own message, falling
/// back to the raw body then the status.
fn parse_upload_response(status: u16, body: &str) -> UploadOutcome {
    if (200..300).contains(&status) {
        return UploadOutcome::Ok {
            module_url: extract_module_url(body),
        };
    }
    UploadOutcome::Err(extract_error_message(body).unwrap_or_else(|| {
        let trimmed = body.trim();
        if trimmed.is_empty() {
            format!("HTTP {status}")
        } else {
            trimmed.to_owned()
        }
    }))
}

/// Pull a module page URL out of a success body, tolerant of the field the API
/// uses (`module_url`, or a `module` object's `url`).
fn extract_module_url(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    if let Some(url) = value.get("module_url").and_then(|v| v.as_str()) {
        return Some(url.to_owned());
    }
    value
        .get("module")
        .and_then(|m| m.get("url"))
        .and_then(|v| v.as_str())
        .map(str::to_owned)
}

/// Pull a human message out of an error body: a top-level `error` string, or
/// the first entry of an `errors` array/object.
fn extract_error_message(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    if let Some(message) = value.get("error").and_then(|v| v.as_str()) {
        return Some(message.to_owned());
    }
    match value.get("errors") {
        Some(serde_json::Value::Array(items)) => items
            .iter()
            .find_map(|item| item.as_str())
            .map(str::to_owned),
        Some(serde_json::Value::String(message)) => Some(message.clone()),
        _ => None,
    }
}

/// Replace every occurrence of `key` with a redaction marker, so a leaked URL
/// or error never exposes the API key. A blank key is a no-op.
fn redact(text: &str, key: &str) -> String {
    if key.is_empty() {
        return text.to_owned();
    }
    text.replace(key, "<redacted>")
}

/// The onboarding error when there is no rockspec to publish.
fn no_rockspec_message() -> String {
    "this project has no `*.rockspec`, so there is nothing to publish. The rockspec is \
     luabox's authored package manifest (SPEC.md §6). Run `luabox init` to scaffold one, \
     then fill in its `source.url` and `build.modules` before publishing"
        .to_owned()
}

/// The onboarding error when no luarocks.org API key is configured.
fn missing_key_message() -> String {
    format!(
        "no luarocks.org API key is configured. Create one at \
         https://luarocks.org/settings/api-keys, then run `luabox login --luarocks` to store \
         it (or set {} in your environment) and publish again",
        crate::auth_cmd::LUAROCKS_API_KEY_ENV
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use luabox_resolve::luarocks::rockspec::{Rockspec, Source};

    fn spec(package: &str, version: &str, url: Option<&str>) -> Rockspec {
        Rockspec {
            package: Some(package.to_owned()),
            version: Some(version.to_owned()),
            source: Source {
                url: url.map(str::to_owned),
                ..Source::default()
            },
            ..Rockspec::default()
        }
    }

    #[test]
    fn validate_accepts_canonical_filename() {
        let s = spec(
            "app",
            "0.1.0-1",
            Some("git+https://example.invalid/app.git"),
        );
        let upload = validate(&s, "app-0.1.0-1.rockspec").expect("valid");
        assert_eq!(upload.package, "app");
        assert_eq!(upload.version, "0.1.0-1");
    }

    #[test]
    fn validate_rejects_filename_mismatch() {
        let s = spec(
            "app",
            "0.1.0-1",
            Some("git+https://example.invalid/app.git"),
        );
        let err = validate(&s, "app-0.2.0-1.rockspec").expect_err("mismatch");
        let text = err.to_string();
        assert!(text.contains("app-0.1.0-1.rockspec"), "{text}");
    }

    #[test]
    fn validate_rejects_missing_source_url() {
        let s = spec("app", "0.1.0-1", None);
        let err = validate(&s, "app-0.1.0-1.rockspec").expect_err("no url");
        assert!(err.to_string().contains("source.url"), "{err}");

        let blank = spec("app", "0.1.0-1", Some("   "));
        assert!(validate(&blank, "app-0.1.0-1.rockspec").is_err());
    }

    #[test]
    fn validate_rejects_missing_package_or_version() {
        let mut s = spec("app", "0.1.0-1", Some("git+https://x/app.git"));
        s.package = None;
        assert!(validate(&s, "app-0.1.0-1.rockspec").is_err());
    }

    /// A [`Build`] with the given type and one module — enough to exercise
    /// `classify_pure_lua` without triggering `field_reassign_with_default`.
    fn build_with(build_type: Option<&str>, module: Option<(&str, ModuleSpec)>) -> Build {
        let mut modules = std::collections::BTreeMap::new();
        if let Some((name, spec)) = module {
            modules.insert(name.to_owned(), spec);
        }
        Build {
            build_type: build_type.map(str::to_owned),
            modules,
            has_external_dependencies: false,
        }
    }

    #[test]
    fn pure_lua_rock_passes_and_c_rock_is_refused() {
        let ok = build_with(
            Some("builtin"),
            Some(("app", ModuleSpec::LuaFile("src/app.lua".to_owned()))),
        );
        assert!(classify_pure_lua("app", &ok).is_ok());

        let c_build = build_with(Some("make"), None);
        let err = classify_pure_lua("luasocket", &c_build).expect_err("C rock");
        assert!(err.contains("luasocket"), "{err}");
        assert!(err.contains("pure-Lua"), "{err}");

        let native = build_with(
            Some("builtin"),
            Some(("cjson", ModuleSpec::NativeFile("cjson.c".to_owned()))),
        );
        assert!(classify_pure_lua("lua-cjson", &native).is_err());

        let ext = Build {
            build_type: Some("builtin".to_owned()),
            modules: std::collections::BTreeMap::new(),
            has_external_dependencies: true,
        };
        assert!(classify_pure_lua("openssl", &ext).is_err());
    }

    #[test]
    fn upload_url_embeds_the_key() {
        assert_eq!(
            upload_url("https://luarocks.org", "SECRET"),
            "https://luarocks.org/api/1/SECRET/upload"
        );
    }

    #[test]
    fn upload_args_send_the_rockspec_multipart_field() {
        let args = upload_args(
            "https://luarocks.org/api/1/SECRET/upload",
            Path::new("app-0.1.0-1.rockspec"),
        );
        assert!(
            args.iter().any(|a| a.starts_with("rockspec_file=@")),
            "{args:?}"
        );
        assert!(args.iter().any(|a| a == "-F"), "{args:?}");
    }

    #[test]
    fn error_text_never_leaks_the_key() {
        // A URL/error that embeds the key must be redacted before it is shown.
        let key = "SECRETKEY";
        let url = upload_url("https://luarocks.org", key);
        let leaked = format!("`curl` failed for {url}");
        let safe = redact(&leaked, key);
        assert!(!safe.contains(key), "redacted: {safe}");
        assert!(safe.contains("<redacted>"), "redacted: {safe}");
    }

    #[test]
    fn split_status_separates_body_and_code() {
        assert_eq!(
            split_status("{\"ok\":true}\n200"),
            Some((200, "{\"ok\":true}"))
        );
        assert_eq!(split_status("no status line"), None);
    }

    #[test]
    fn success_response_extracts_module_url() {
        let outcome = parse_upload_response(
            200,
            r#"{"module":{"name":"app","url":"https://luarocks.org/modules/me/app"}}"#,
        );
        assert_eq!(
            outcome,
            UploadOutcome::Ok {
                module_url: Some("https://luarocks.org/modules/me/app".to_owned())
            }
        );

        // A bare 2xx with no recognizable URL is still success.
        assert_eq!(
            parse_upload_response(201, "{}"),
            UploadOutcome::Ok { module_url: None }
        );
    }

    #[test]
    fn error_response_surfaces_server_message() {
        let outcome =
            parse_upload_response(409, r#"{"errors":["Version 0.1.0-1 already exists"]}"#);
        assert_eq!(
            outcome,
            UploadOutcome::Err("Version 0.1.0-1 already exists".to_owned())
        );
        let outcome = parse_upload_response(422, r#"{"error":"invalid rockspec"}"#);
        assert_eq!(outcome, UploadOutcome::Err("invalid rockspec".to_owned()));
        // A non-JSON error body falls back to the raw text.
        assert_eq!(
            parse_upload_response(500, "Internal Server Error"),
            UploadOutcome::Err("Internal Server Error".to_owned())
        );
    }
}

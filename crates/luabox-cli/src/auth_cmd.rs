//! `luabox login` / `logout` / `whoami` — GitHub authentication.
//!
//! Users authenticate through the browser via the OAuth 2.0 **Device
//! Authorization Grant** (RFC 8628) instead of pasting a Personal Access
//! Token. The resulting token is stored encrypted at rest in the OS keychain
//! ([`crate::keychain`]) and authenticates luabox's **git-source** GitHub
//! operations — `luabox outdated`'s release probing and `luabox update`'s
//! re-pin — transparently afterwards (see [`crate::github::token_with_source`]).
//! Registry reads (`luabox search`) are anonymous and never use it.
//!
//! ## Device flow, in four steps
//!
//! 1. POST `login/device/code` with the `client_id` (no scope requested — an
//!    unscoped token already lifts the API rate limit; least privilege). We get
//!    back a `user_code`, a `verification_uri`, and a `device_code`.
//! 2. Show the user the `user_code` + `verification_uri` (and best-effort open
//!    their browser to `verification_uri_complete`).
//! 3. Poll `login/oauth/access_token` at the server's `interval`, honoring
//!    `authorization_pending` / `slow_down` / `expired_token` / `access_denied`
//!    until an `access_token` arrives or the `expires_in` deadline passes.
//! 4. Fetch the login (`GET /user`), store the token, report "Signed in as …".
//!
//! ## Testing seam
//!
//! All HTTP is behind the [`DeviceFlow`] trait so the state machine, response
//! parsing, and event emission are unit-tested against canned bodies with no
//! network (see the `tests` module). The three network calls live only in the
//! [`CurlDeviceFlow`] production impl, which shells out to `curl` like the rest
//! of the toolchain (SPEC.md §6 — no HTTP crate).

use std::env;
use std::io::{self, Write};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// GitHub OAuth App **`client_id`** for the device flow.
///
/// This is a PUBLIC value — a `client_id` is not a secret and is safe to
/// commit. It is the real luabox GitHub OAuth App's `client_id`, with the
/// device flow enabled. It can be overridden at runtime via the
/// `LUABOX_GITHUB_CLIENT_ID` environment variable, so the flow can be exercised
/// against a different app without recompiling.
const GITHUB_CLIENT_ID: &str = "Ov23ligfhUuTeMztzPY2";

/// The `grant_type` the device flow polls with.
const DEVICE_GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:device_code";

/// The effective `client_id`: `LUABOX_GITHUB_CLIENT_ID` if set (non-blank),
/// else the compiled-in [`GITHUB_CLIENT_ID`].
fn client_id() -> String {
    match env::var("LUABOX_GITHUB_CLIENT_ID") {
        Ok(value) if !value.trim().is_empty() => value,
        _ => GITHUB_CLIENT_ID.to_owned(),
    }
}

/// Output mode shared by `login` and `whoami`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    /// Human-readable prose.
    Text,
    /// Machine-readable: newline-delimited JSON events (`login`) or one JSON
    /// object (`whoami`).
    Json,
}

impl Format {
    /// Parse the `--format` value, rejecting anything but `text`/`json`.
    fn parse(value: &str) -> Result<Self> {
        match value {
            "text" => Ok(Format::Text),
            "json" => Ok(Format::Json),
            other => bail!("unknown --format `{other}`; expected `text` or `json`"),
        }
    }
}

// --- device-code + poll response parsing (pure; unit-tested) ----------------

/// The parsed `login/device/code` response.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::struct_field_names)] // `device_code` is GitHub's field name.
struct DeviceCode {
    device_code: String,
    user_code: String,
    verification_uri: String,
    verification_uri_complete: Option<String>,
    expires_in: u64,
    interval: u64,
}

/// The raw `login/device/code` JSON shape.
#[derive(Debug, Deserialize)]
struct DeviceCodeBody {
    device_code: String,
    user_code: String,
    verification_uri: String,
    #[serde(default)]
    verification_uri_complete: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    interval: Option<u64>,
}

/// Default overall deadline when GitHub omits `expires_in` (seconds).
const DEFAULT_EXPIRES_IN: u64 = 900;
/// Default/minimum poll interval when GitHub omits or under-specifies it.
const DEFAULT_INTERVAL: u64 = 5;
/// How much `slow_down` adds to the interval (RFC 8628 §3.5).
const SLOW_DOWN_STEP: u64 = 5;

/// Parse a `login/device/code` body, applying sensible defaults for optional
/// timing fields.
fn parse_device_code(json: &str) -> Result<DeviceCode> {
    let body: DeviceCodeBody =
        serde_json::from_str(json).context("device/code response was not the expected JSON")?;
    Ok(DeviceCode {
        device_code: body.device_code,
        user_code: body.user_code,
        verification_uri: body.verification_uri,
        verification_uri_complete: body.verification_uri_complete,
        expires_in: body.expires_in.unwrap_or(DEFAULT_EXPIRES_IN),
        interval: body.interval.unwrap_or(DEFAULT_INTERVAL).max(1),
    })
}

/// The outcome of parsing one poll response.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PollOutcome {
    /// The user has not authorized yet — keep polling at the same interval.
    Pending,
    /// We polled too fast — back off (interval += [`SLOW_DOWN_STEP`]).
    SlowDown,
    /// The device code expired before authorization — the user must restart.
    Expired,
    /// The user denied the request.
    Denied,
    /// Authorization succeeded; the access token is enclosed.
    Success(String),
    /// Any other error, carrying GitHub's message.
    Other(String),
}

/// The raw `login/oauth/access_token` JSON shape (success *or* error).
#[derive(Debug, Deserialize)]
struct PollBody {
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    error_description: Option<String>,
}

/// Parse one poll response into a [`PollOutcome`]. A body with an
/// `access_token` is success; otherwise the `error` code selects the variant.
fn parse_poll(json: &str) -> PollOutcome {
    let body: PollBody = match serde_json::from_str(json) {
        Ok(body) => body,
        Err(err) => return PollOutcome::Other(format!("unparseable poll response: {err}")),
    };
    if let Some(token) = body.access_token.filter(|t| !t.is_empty()) {
        return PollOutcome::Success(token);
    }
    match body.error.as_deref() {
        Some("authorization_pending") => PollOutcome::Pending,
        Some("slow_down") => PollOutcome::SlowDown,
        Some("expired_token") => PollOutcome::Expired,
        Some("access_denied") => PollOutcome::Denied,
        Some(other) => PollOutcome::Other(
            body.error_description
                .unwrap_or_else(|| format!("GitHub returned error `{other}`")),
        ),
        None => PollOutcome::Other("poll response had neither access_token nor error".to_owned()),
    }
}

/// What the polling loop should do next, given a [`PollOutcome`] and the
/// current interval. Pure — this is the device-flow state machine.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Step {
    /// Keep polling after `interval` seconds.
    Continue(u64),
    /// Authorization complete; the token is enclosed.
    Done(String),
    /// Terminal failure with a user-facing message.
    Fail(String),
}

/// Advance the state machine. `slow_down` bumps the interval by
/// [`SLOW_DOWN_STEP`]; `authorization_pending` keeps it; the rest are terminal.
fn next_step(outcome: PollOutcome, interval: u64) -> Step {
    match outcome {
        PollOutcome::Pending => Step::Continue(interval),
        PollOutcome::SlowDown => Step::Continue(interval + SLOW_DOWN_STEP),
        PollOutcome::Success(token) => Step::Done(token),
        PollOutcome::Expired => Step::Fail(
            "the device code expired before you authorized; run `luabox login` again".to_owned(),
        ),
        PollOutcome::Denied => Step::Fail("authorization was denied on GitHub".to_owned()),
        PollOutcome::Other(message) => Step::Fail(message),
    }
}

// --- JSON events for `--format json` (frozen GUI contract) ------------------

/// The first `login --format json` event: what to show the user.
#[derive(Debug, Serialize)]
struct PromptEvent<'a> {
    event: &'static str,
    user_code: &'a str,
    verification_uri: &'a str,
    verification_uri_complete: Option<&'a str>,
    expires_in: u64,
}

/// The terminal success event.
#[derive(Debug, Serialize)]
struct SuccessEvent<'a> {
    event: &'static str,
    login: &'a str,
}

/// The terminal error event.
#[derive(Debug, Serialize)]
struct ErrorEvent<'a> {
    event: &'static str,
    message: &'a str,
}

/// Serialize the `prompt` event as a single JSON line.
fn prompt_event_json(device: &DeviceCode) -> String {
    let event = PromptEvent {
        event: "prompt",
        user_code: &device.user_code,
        verification_uri: &device.verification_uri,
        verification_uri_complete: device.verification_uri_complete.as_deref(),
        expires_in: device.expires_in,
    };
    serde_json::to_string(&event).unwrap_or_else(|_| String::from("{\"event\":\"prompt\"}"))
}

/// Serialize the `success` event as a single JSON line.
fn success_event_json(login: &str) -> String {
    let event = SuccessEvent {
        event: "success",
        login,
    };
    serde_json::to_string(&event).unwrap_or_else(|_| String::from("{\"event\":\"success\"}"))
}

/// Serialize the `error` event as a single JSON line.
fn error_event_json(message: &str) -> String {
    let event = ErrorEvent {
        event: "error",
        message,
    };
    serde_json::to_string(&event).unwrap_or_else(|_| String::from("{\"event\":\"error\"}"))
}

// --- HTTP seam --------------------------------------------------------------

/// The three network calls the device flow makes, behind a trait so the flow
/// can be driven with canned responses in tests.
trait DeviceFlow {
    /// POST `login/device/code`; returns the raw JSON body.
    fn request_device_code(&self, client_id: &str) -> Result<String>;
    /// POST `login/oauth/access_token`; returns the raw JSON body.
    fn poll_access_token(&self, client_id: &str, device_code: &str) -> Result<String>;
    /// `GET /user` with the token; returns the authenticated login.
    fn fetch_login(&self, token: &str) -> Result<String>;
}

/// How long any single auth HTTP call may take, in seconds.
const HTTP_TIMEOUT_SECS: u32 = 30;

/// Production [`DeviceFlow`] impl: shells out to `curl`.
struct CurlDeviceFlow;

impl CurlDeviceFlow {
    /// POST `body` (form-encoded) to `url` with `Accept: application/json` and
    /// return stdout, failing on a non-2xx status (`curl -f`).
    fn post_form(url: &str, body: &str) -> Result<String> {
        let output = Command::new("curl")
            .args([
                "-fsS",
                "--max-time",
                &HTTP_TIMEOUT_SECS.to_string(),
                "-X",
                "POST",
                "-H",
                "Accept: application/json",
                "-d",
                body,
                url,
            ])
            .output()
            .with_context(|| format!("running `curl` for {url}"))?;
        if !output.status.success() {
            bail!(
                "`curl` failed for {url} (exit {}): {}",
                output.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

impl DeviceFlow for CurlDeviceFlow {
    fn request_device_code(&self, client_id: &str) -> Result<String> {
        // No `scope` field: an unscoped token still lifts the rate limit
        // (least privilege).
        let body = format!("client_id={client_id}");
        Self::post_form("https://github.com/login/device/code", &body)
    }

    fn poll_access_token(&self, client_id: &str, device_code: &str) -> Result<String> {
        let body = format!(
            "client_id={client_id}&device_code={device_code}&grant_type={DEVICE_GRANT_TYPE}"
        );
        Self::post_form("https://github.com/login/oauth/access_token", &body)
    }

    fn fetch_login(&self, token: &str) -> Result<String> {
        crate::github::authenticated_login(token)
    }
}

// --- browser opener ---------------------------------------------------------

/// Best-effort, non-fatal browser open. Any failure is silently ignored — the
/// user always has the printed `verification_uri` + `user_code` to fall back on.
fn open_browser(url: &str) {
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = Command::new("cmd");
        // The empty "" is `start`'s title argument; without it a quoted URL is
        // mistaken for the window title.
        command.args(["/C", "start", "", url]);
        command
    };
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = Command::new("open");
        command.arg(url);
        command
    };
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let mut command = {
        let mut command = Command::new("xdg-open");
        command.arg(url);
        command
    };
    let _ = command.spawn();
}

// --- orchestration ----------------------------------------------------------

/// A completed authentication.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Session {
    token: String,
    login: String,
}

/// Injectable side effects so [`drive_login`] can be tested without sleeping or
/// spawning a browser.
struct Hooks<'a> {
    /// Pause for N seconds between polls (a no-op in tests).
    sleep: &'a dyn Fn(u64),
    /// Open the given URL in a browser (a no-op in tests).
    open_browser: &'a dyn Fn(&str),
    /// The current instant, for the `expires_in` deadline (fixed in tests).
    now: &'a dyn Fn() -> Instant,
}

/// Run the device flow to completion, emitting UX to `out` (stdout).
///
/// Returns the [`Session`] on success. Keychain storage is intentionally *not*
/// done here — the caller does it — so this function stays free of OS side
/// effects and is fully testable with a mock [`DeviceFlow`].
fn drive_login<H: DeviceFlow>(
    client_id: &str,
    http: &H,
    format: Format,
    out: &mut dyn Write,
    hooks: &Hooks<'_>,
) -> Result<Session> {
    let device = parse_device_code(&http.request_device_code(client_id)?)?;

    // Step 2: show the user what to do.
    match format {
        Format::Json => writeln!(out, "{}", prompt_event_json(&device))?,
        Format::Text => {
            writeln!(
                out,
                "To authenticate, open {} and enter the code:\n\n    {}\n",
                device.verification_uri, device.user_code
            )?;
            if let Some(complete) = &device.verification_uri_complete {
                writeln!(out, "Opening your browser to {complete} …")?;
            }
        }
    }
    out.flush()?;
    if let Some(complete) = &device.verification_uri_complete {
        (hooks.open_browser)(complete);
    }

    // Step 3: poll until authorized, denied, or expired.
    let deadline = (hooks.now)() + Duration::from_secs(device.expires_in);
    let mut interval = device.interval;
    let token = loop {
        if (hooks.now)() >= deadline {
            bail!("timed out waiting for authorization; run `luabox login` again");
        }
        (hooks.sleep)(interval);
        let body = http.poll_access_token(client_id, &device.device_code)?;
        match next_step(parse_poll(&body), interval) {
            Step::Continue(next) => interval = next,
            Step::Done(token) => break token,
            Step::Fail(message) => bail!("{message}"),
        }
    };

    // Step 4: identify the user.
    let login = http.fetch_login(&token)?;
    match format {
        Format::Json => writeln!(out, "{}", success_event_json(&login))?,
        Format::Text => writeln!(out, "Signed in as {login}")?,
    }
    out.flush()?;

    Ok(Session { token, login })
}

/// `luabox login` — authenticate via the GitHub device flow and store the token.
pub fn login(format: &str) -> Result<()> {
    let format = Format::parse(format)?;
    let client_id = client_id();
    let http = CurlDeviceFlow;
    let hooks = Hooks {
        sleep: &|seconds| std::thread::sleep(Duration::from_secs(seconds)),
        open_browser: &open_browser,
        now: &Instant::now,
    };

    let mut stdout = io::stdout();
    let session = match drive_login(&client_id, &http, format, &mut stdout, &hooks) {
        Ok(session) => session,
        Err(err) => {
            // In JSON mode the GUIs expect a terminal `error` event on stdout;
            // still return the error so the exit code is non-zero.
            if format == Format::Json {
                println!("{}", error_event_json(&err.to_string()));
            }
            return Err(err);
        }
    };

    // Persist the token. A keychain that can't be reached is not fatal: tell
    // the user how to keep the session via the env var (to stderr so the JSON
    // event stream on stdout stays clean).
    if let Err(err) = crate::keychain::store(&session.token) {
        eprintln!(
            "warning: couldn't store the token in the OS keychain ({err}). \
             To keep using it, set LUABOX_GITHUB_TOKEN in your environment:\n\
             \n    LUABOX_GITHUB_TOKEN={}\n",
            session.token
        );
    }
    Ok(())
}

/// The environment variable overriding the stored luarocks.org API key. Read
/// by `luabox publish` and `luabox whoami`; wins over the keychain (CI/one-off
/// overrides), mirroring the GitHub token precedence.
pub(crate) const LUAROCKS_API_KEY_ENV: &str = "LUABOX_LUAROCKS_API_KEY";

/// `luabox login --luarocks` — store the luarocks.org upload API key in the OS
/// keychain so `luabox publish` can use it. The key is read from stdin (get it
/// from <https://luarocks.org/settings/api-keys>), validated non-empty, and
/// stored encrypted at rest.
pub fn login_luarocks() -> Result<()> {
    eprintln!(
        "Paste your luarocks.org API key (from https://luarocks.org/settings/api-keys), \
         then press Enter:"
    );
    let mut line = String::new();
    io::stdin()
        .read_line(&mut line)
        .context("reading the API key from stdin")?;
    let key = line.trim();
    if key.is_empty() {
        bail!("no API key entered; nothing was stored");
    }

    crate::keychain::store_luarocks_key(key).map_err(|err| {
        anyhow::anyhow!(
            "couldn't store the luarocks.org API key in the OS keychain ({err}). \
             Set {LUAROCKS_API_KEY_ENV} in your environment instead to keep using it."
        )
    })?;
    println!("Stored your luarocks.org API key. `luabox publish` will now use it.");
    Ok(())
}

/// `luabox logout` — delete any stored token (GitHub) *and* the luarocks.org
/// API key. Idempotent.
// Uniform `Result` dispatch signature (like every other command).
#[allow(clippy::unnecessary_wraps)]
pub fn logout() -> Result<()> {
    match crate::keychain::delete() {
        Ok(()) => println!("Signed out (any stored GitHub token was removed)."),
        // An unreachable keychain means nothing is stored there to remove.
        Err(_) => println!("No stored GitHub token to remove."),
    }
    match crate::keychain::delete_luarocks_key() {
        Ok(()) => println!("Removed any stored luarocks.org API key."),
        Err(_) => println!("No stored luarocks.org API key to remove."),
    }
    Ok(())
}

/// Whether a luarocks.org API key is configured: the
/// [`LUAROCKS_API_KEY_ENV`] env override (non-blank) or a keychain-stored key.
/// A keychain that cannot be reached is treated as "no key" (never fatal).
pub(crate) fn luarocks_key_configured() -> bool {
    if env::var(LUAROCKS_API_KEY_ENV).is_ok_and(|value| !value.trim().is_empty()) {
        return true;
    }
    crate::keychain::retrieve_luarocks_key()
        .ok()
        .flatten()
        .is_some_and(|key| !key.trim().is_empty())
}

/// Shape the `whoami --format json` object for a resolved (or absent) GitHub
/// identity. The GitHub `login`/`source` fields are the frozen contract the
/// editor extensions parse; `luarocks` is an additive boolean reporting
/// whether a luarocks.org API key is configured (for `luabox publish`).
fn whoami_json(identity: Option<(&str, &str)>, luarocks: bool) -> String {
    #[derive(Serialize)]
    struct WhoAmI<'a> {
        login: Option<&'a str>,
        source: Option<&'a str>,
        luarocks: bool,
    }
    let value = match identity {
        Some((login, source)) => WhoAmI {
            login: Some(login),
            source: Some(source),
            luarocks,
        },
        None => WhoAmI {
            login: None,
            source: None,
            luarocks,
        },
    };
    serde_json::to_string(&value)
        .unwrap_or_else(|_| String::from("{\"login\":null,\"source\":null,\"luarocks\":false}"))
}

/// `luabox whoami` — report the signed-in GitHub identity, or "not signed in".
/// Always exits 0 (it is a status query, not a gate).
pub fn whoami(format: &str) -> Result<()> {
    let format = Format::parse(format)?;
    let luarocks = luarocks_key_configured();

    let Some((token, source)) = crate::github::token_with_source() else {
        match format {
            Format::Json => println!("{}", whoami_json(None, luarocks)),
            Format::Text => println!("not signed in"),
        }
        return Ok(());
    };

    let login = crate::github::authenticated_login(&token)?;
    match format {
        Format::Json => println!("{}", whoami_json(Some((&login, source.label())), luarocks)),
        Format::Text => println!("{login}"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::time::Instant;

    use super::{
        DeviceCode, DeviceFlow, Format, Hooks, PollOutcome, Session, Step, drive_login,
        error_event_json, next_step, parse_device_code, parse_poll, prompt_event_json,
        success_event_json, whoami_json,
    };

    // --- parsing ---------------------------------------------------------

    #[test]
    fn parses_full_device_code_response() {
        let json = r#"{
            "device_code":"dc-123",
            "user_code":"WDJB-MJHT",
            "verification_uri":"https://github.com/login/device",
            "verification_uri_complete":"https://github.com/login/device?user_code=WDJB-MJHT",
            "expires_in":900,
            "interval":5
        }"#;
        let device = parse_device_code(json).expect("valid");
        assert_eq!(device.device_code, "dc-123");
        assert_eq!(device.user_code, "WDJB-MJHT");
        assert_eq!(
            device.verification_uri_complete.as_deref(),
            Some("https://github.com/login/device?user_code=WDJB-MJHT")
        );
        assert_eq!(device.expires_in, 900);
        assert_eq!(device.interval, 5);
    }

    #[test]
    fn device_code_defaults_optional_timing_and_uri() {
        let json = r#"{
            "device_code":"dc","user_code":"AAAA-BBBB",
            "verification_uri":"https://github.com/login/device"
        }"#;
        let device = parse_device_code(json).expect("valid");
        assert!(device.verification_uri_complete.is_none());
        assert_eq!(device.expires_in, 900);
        assert_eq!(device.interval, 5);
    }

    #[test]
    fn device_code_interval_floored_to_one() {
        let json = r#"{"device_code":"d","user_code":"u","verification_uri":"v","interval":0}"#;
        assert_eq!(parse_device_code(json).expect("valid").interval, 1);
    }

    #[test]
    fn poll_variants_parse_to_outcomes() {
        assert_eq!(
            parse_poll(r#"{"error":"authorization_pending"}"#),
            PollOutcome::Pending
        );
        assert_eq!(
            parse_poll(r#"{"error":"slow_down"}"#),
            PollOutcome::SlowDown
        );
        assert_eq!(
            parse_poll(r#"{"error":"expired_token"}"#),
            PollOutcome::Expired
        );
        assert_eq!(
            parse_poll(r#"{"error":"access_denied"}"#),
            PollOutcome::Denied
        );
        assert_eq!(
            parse_poll(r#"{"access_token":"gho_abc","token_type":"bearer"}"#),
            PollOutcome::Success("gho_abc".to_owned())
        );
    }

    #[test]
    fn poll_unknown_error_carries_description() {
        assert_eq!(
            parse_poll(r#"{"error":"unsupported_grant_type","error_description":"nope"}"#),
            PollOutcome::Other("nope".to_owned())
        );
    }

    // --- state machine ---------------------------------------------------

    #[test]
    fn pending_keeps_interval() {
        assert_eq!(next_step(PollOutcome::Pending, 5), Step::Continue(5));
    }

    #[test]
    fn slow_down_bumps_interval_by_five() {
        assert_eq!(next_step(PollOutcome::SlowDown, 5), Step::Continue(10));
        assert_eq!(next_step(PollOutcome::SlowDown, 10), Step::Continue(15));
    }

    #[test]
    fn success_is_done_with_token() {
        assert_eq!(
            next_step(PollOutcome::Success("t".to_owned()), 5),
            Step::Done("t".to_owned())
        );
    }

    #[test]
    fn expired_and_denied_are_terminal_failures() {
        assert!(matches!(next_step(PollOutcome::Expired, 5), Step::Fail(_)));
        assert!(matches!(next_step(PollOutcome::Denied, 5), Step::Fail(_)));
        assert!(matches!(
            next_step(PollOutcome::Other("x".to_owned()), 5),
            Step::Fail(_)
        ));
    }

    // --- event shapes ----------------------------------------------------

    #[test]
    fn prompt_event_has_frozen_shape() {
        let device = DeviceCode {
            device_code: "dc".to_owned(),
            user_code: "WDJB-MJHT".to_owned(),
            verification_uri: "https://github.com/login/device".to_owned(),
            verification_uri_complete: Some(
                "https://github.com/login/device?user_code=x".to_owned(),
            ),
            expires_in: 900,
            interval: 5,
        };
        let json = prompt_event_json(&device);
        let value: serde_json::Value = serde_json::from_str(&json).expect("valid json");
        assert_eq!(value["event"], "prompt");
        assert_eq!(value["user_code"], "WDJB-MJHT");
        assert_eq!(value["verification_uri"], "https://github.com/login/device");
        assert_eq!(
            value["verification_uri_complete"],
            "https://github.com/login/device?user_code=x"
        );
        assert_eq!(value["expires_in"], 900);
    }

    #[test]
    fn prompt_event_complete_uri_is_null_when_absent() {
        let device = DeviceCode {
            device_code: "dc".to_owned(),
            user_code: "u".to_owned(),
            verification_uri: "v".to_owned(),
            verification_uri_complete: None,
            expires_in: 600,
            interval: 5,
        };
        let value: serde_json::Value =
            serde_json::from_str(&prompt_event_json(&device)).expect("valid");
        assert!(value["verification_uri_complete"].is_null());
    }

    #[test]
    fn success_and_error_events_have_frozen_shape() {
        let value: serde_json::Value =
            serde_json::from_str(&success_event_json("octocat")).expect("valid");
        assert_eq!(value["event"], "success");
        assert_eq!(value["login"], "octocat");

        let value: serde_json::Value =
            serde_json::from_str(&error_event_json("boom")).expect("valid");
        assert_eq!(value["event"], "error");
        assert_eq!(value["message"], "boom");
    }

    // --- whoami shaping --------------------------------------------------

    #[test]
    fn whoami_json_signed_in_and_out() {
        let value: serde_json::Value =
            serde_json::from_str(&whoami_json(Some(("octocat", "keychain")), true)).expect("valid");
        assert_eq!(value["login"], "octocat");
        assert_eq!(value["source"], "keychain");
        assert_eq!(value["luarocks"], true);

        let value: serde_json::Value =
            serde_json::from_str(&whoami_json(None, false)).expect("valid");
        assert!(value["login"].is_null());
        assert!(value["source"].is_null());
        assert_eq!(value["luarocks"], false);
    }

    // --- end-to-end flow with a mock transport ---------------------------

    /// A [`DeviceFlow`] that hands back scripted bodies: one device-code body
    /// and a queue of poll bodies consumed in order.
    struct MockFlow {
        device_body: String,
        polls: RefCell<Vec<String>>,
        login: String,
    }

    impl DeviceFlow for MockFlow {
        fn request_device_code(&self, _client_id: &str) -> anyhow::Result<String> {
            Ok(self.device_body.clone())
        }
        fn poll_access_token(
            &self,
            _client_id: &str,
            _device_code: &str,
        ) -> anyhow::Result<String> {
            Ok(self.polls.borrow_mut().remove(0))
        }
        fn fetch_login(&self, _token: &str) -> anyhow::Result<String> {
            Ok(self.login.clone())
        }
    }

    fn far_future() -> Instant {
        Instant::now()
    }

    fn test_hooks<'a>() -> Hooks<'a> {
        Hooks {
            sleep: &|_| {},
            open_browser: &|_| {},
            now: &far_future,
        }
    }

    fn device_body() -> String {
        r#"{"device_code":"dc","user_code":"WDJB-MJHT",
            "verification_uri":"https://github.com/login/device",
            "verification_uri_complete":"https://github.com/login/device?user_code=WDJB-MJHT",
            "expires_in":900,"interval":5}"#
            .to_owned()
    }

    #[test]
    fn drive_login_polls_pending_then_succeeds() {
        let mock = MockFlow {
            device_body: device_body(),
            polls: RefCell::new(vec![
                r#"{"error":"authorization_pending"}"#.to_owned(),
                r#"{"error":"slow_down"}"#.to_owned(),
                r#"{"access_token":"gho_secret"}"#.to_owned(),
            ]),
            login: "octocat".to_owned(),
        };
        let mut out = Vec::new();
        let hooks = test_hooks();
        let session = drive_login("cid", &mock, Format::Json, &mut out, &hooks).expect("succeeds");
        assert_eq!(
            session,
            Session {
                token: "gho_secret".to_owned(),
                login: "octocat".to_owned(),
            }
        );

        // Emits a prompt event first, then a success event — NDJSON.
        let lines: Vec<&str> = std::str::from_utf8(&out)
            .expect("utf8")
            .lines()
            .filter(|l| !l.trim().is_empty())
            .collect();
        assert_eq!(lines.len(), 2, "prompt then success");
        let prompt: serde_json::Value = serde_json::from_str(lines[0]).expect("json");
        assert_eq!(prompt["event"], "prompt");
        assert_eq!(prompt["user_code"], "WDJB-MJHT");
        let success: serde_json::Value = serde_json::from_str(lines[1]).expect("json");
        assert_eq!(success["event"], "success");
        assert_eq!(success["login"], "octocat");
    }

    #[test]
    fn drive_login_reports_denied() {
        let mock = MockFlow {
            device_body: device_body(),
            polls: RefCell::new(vec![r#"{"error":"access_denied"}"#.to_owned()]),
            login: "octocat".to_owned(),
        };
        let mut out = Vec::new();
        let hooks = test_hooks();
        let err = drive_login("cid", &mock, Format::Text, &mut out, &hooks)
            .expect_err("denied should fail");
        assert!(err.to_string().contains("denied"), "{err}");
    }

    #[test]
    fn drive_login_reports_expired() {
        let mock = MockFlow {
            device_body: device_body(),
            polls: RefCell::new(vec![r#"{"error":"expired_token"}"#.to_owned()]),
            login: "octocat".to_owned(),
        };
        let mut out = Vec::new();
        let hooks = test_hooks();
        let err = drive_login("cid", &mock, Format::Text, &mut out, &hooks)
            .expect_err("expired should fail");
        assert!(err.to_string().contains("expired"), "{err}");
    }
}

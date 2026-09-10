//! Ending a session while a `window/workDoneProgress/create` is outstanding,
//! driven through the **production sequence**.
//!
//! # Why this file exists at all
//!
//! Round 8 fixed a `shutdown`/`exit` pair drained onto `Server::pending`
//! becoming invisible to `Connection::handle_shutdown`, which reads the raw
//! channel — the server answered the shutdown, waited out `handle_shutdown`'s
//! own 30 s bound for a notification it was already holding, and exited 1.
//! The unit tests written for it drove `Server::new` then `main_loop`.
//!
//! `run` does not do that. It opens **two** create windows before the loop:
//! `Server::new`'s startup rock harvest, and then `bootstrap`'s workspace
//! index. Skipping `bootstrap` skipped the second one, and with it the whole
//! defect: window 1 aborts on the `shutdown` correctly and leaves the `exit`
//! on the channel, window 2 then drains that `exit` onto `pending`, and
//! `handle_shutdown` finds an empty channel — byte-identical to the behaviour
//! before the round-8 fix (Shockwave round 9). A test that cannot see the
//! second window cannot see that.
//!
//! So every test here goes through [`luabox_lsp::run`] itself: the real
//! handshake, the real `Server::new`, the real `bootstrap`, the real loop.
//! What is asserted is what a client would observe — the session ends, it
//! ends *promptly*, and `run` returns `Ok`.
//!
//! # Why the completion channel
//!
//! The failure mode is a 30 s block, not a wrong answer. `join()` would sit
//! through it and then report on the exit code alone, so a regression would
//! look like a slow pass. [`Session::finish`] waits on a channel the server
//! thread fires as `run` returns, with a bound well under that 30 s, and
//! fails on the *hang*.
//!
//! # `LUABOX_PERF_FACTOR` (#84)
//!
//! [`read_timeout`] and [`finish_timeout`] widen with the same
//! `LUABOX_PERF_FACTOR` multiplier `scripts/perf-gate.sh` reads, so a
//! shared/loaded machine gets slack an idle one does not need. Unset
//! defaults to `1.0`. Set-but-invalid — non-numeric, non-finite, zero,
//! negative, or empty/whitespace-only (a parse failure, not treated as
//! unset) — panics rather than falling back silently. Test-only:
//! production's `SHUTDOWN_EXIT_TIMEOUT` (`server.rs`) never reads this
//! variable. See [`resolve_perf_factor`] and [`scaled_wait`] for the exact
//! parsing and scaling rules.
//!
//! `finish_timeout` additionally never grows past
//! [`FINISH_TIMEOUT_CEILING`] — see that constant's doc comment, and
//! `finish_still_rejects_a_timeout_at_the_scaled_bound` below for the proof
//! that the cap keeps rejecting a failure at the largest bound it allows
//! (that test proves the cap's own logic, not the full `Session::finish`
//! wiring to it — see its doc comment). `read_timeout` carries no such cap
//! and can exceed 30 s at a large enough factor (100 s at
//! `LUABOX_PERF_FACTOR=10`); see [`read_timeout`]'s doc comment for why
//! that does not weaken the regression proof.

// test code — panics document assumptions
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use lsp_server::{Connection, ErrorCode, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{
    DidChangeConfiguration, Exit, Initialized, Notification as _, Progress,
};
use lsp_types::request::{Request as _, Shutdown, WorkDoneProgressCreate};
use lsp_types::{
    ClientCapabilities, DidChangeConfigurationParams, InitializeParams, WindowClientCapabilities,
    WorkspaceFolder,
};
use serde_json::{Value, json};
use tempfile::TempDir;

/// Unscaled read budget — see [`read_timeout`].
const READ_TIMEOUT_BASE: Duration = Duration::from_secs(10);

/// Unscaled completion budget — see [`finish_timeout`].
const FINISH_TIMEOUT_BASE: Duration = Duration::from_secs(5);

/// The ceiling `finish_timeout` never scales past, regardless of
/// `LUABOX_PERF_FACTOR`.
///
/// The round-9 regression makes `run` block for the full 30 s of
/// production's `SHUTDOWN_EXIT_TIMEOUT` (`server.rs`) — a wall-clock
/// `recv_timeout`, not CPU-bound work a loaded box does more slowly, so it
/// takes just as long on a slow machine as a fast one. An uncapped
/// `finish_timeout` could eventually scale past that 30 s, at which point a
/// timeout here would no longer clearly attribute the failure to the
/// watchdog rather than to `LUABOX_PERF_FACTOR`'s own slack. 25 s keeps
/// that attribution unambiguous at any factor this suite runs with.
const FINISH_TIMEOUT_CEILING: Duration = Duration::from_secs(25);

/// `LUABOX_PERF_FACTOR`'s parsing and validation, taking the same shape
/// `std::env::var` hands back so the policy is testable without ever setting
/// or clearing the real variable — mutating it from a test would race every
/// other test in this binary, which shares the process environment and runs
/// in parallel by default. See the module doc comment for the policy this
/// implements.
///
/// Leading/trailing ASCII whitespace is trimmed before parsing. An empty or
/// whitespace-only value trims to `""`, which fails `f64::parse` and panics
/// as "not a float" — it is a parse error like any other, not a second
/// spelling of unset; only `VarError::NotPresent` defaults to `1.0`. This is
/// stricter than shells where `VAR=` sometimes reads as empty/falsy rather
/// than a type error — deliberately: this parser does not claim that parity.
fn resolve_perf_factor(var: Result<String, std::env::VarError>) -> f64 {
    match var {
        Err(std::env::VarError::NotPresent) => 1.0,
        Err(err) => panic!("LUABOX_PERF_FACTOR is set but not valid UTF-8: {err}"),
        Ok(raw) => {
            let factor: f64 = raw
                .trim()
                .parse()
                .unwrap_or_else(|err| panic!("LUABOX_PERF_FACTOR={raw:?} is not a float: {err}"));
            assert!(
                factor.is_finite() && factor > 0.0,
                "LUABOX_PERF_FACTOR must be a finite, positive number, got {factor} \
                 (parsed from {raw:?})"
            );
            factor
        }
    }
}

/// `LUABOX_PERF_FACTOR` — a positive, finite multiplier widening the
/// timeouts below for a machine under load (a concurrent `cargo build`,
/// e.g.).
fn perf_factor() -> f64 {
    resolve_perf_factor(std::env::var("LUABOX_PERF_FACTOR"))
}

/// Scales `base` by `factor`, the one place [`read_timeout`],
/// [`finish_timeout`], and this module's own tests turn a `LUABOX_PERF_FACTOR`
/// into an actual wait — so the arithmetic cannot drift between them.
///
/// [`resolve_perf_factor`] only checks `factor` is finite and positive; that
/// does not mean `base * factor` is a representable `Duration`.
/// `Duration::try_from_secs_f64` catches the far end: a factor large enough
/// (`1e300`, say) to overflow panics here, by name, before
/// `finish_timeout`'s `.min(FINISH_TIMEOUT_CEILING)` ever runs — otherwise
/// that overflow panics inside `Duration::mul_f64` with no mention of
/// `LUABOX_PERF_FACTOR` at all. It does not catch the near end: a factor
/// small enough (`1e-300`) that `base * factor` rounds to an exact-zero
/// `Duration` is still `Ok(0)`, which would silently turn "wait `base`,
/// scaled" into "wait nothing" — the explicit zero check below is what
/// rejects that.
fn scaled_wait(base: Duration, factor: f64, which: &str) -> Duration {
    let secs = base.as_secs_f64() * factor;
    let scaled = Duration::try_from_secs_f64(secs).unwrap_or_else(|err| {
        panic!(
            "LUABOX_PERF_FACTOR={factor} scales {which}'s {base:?} base to an \
             unrepresentable duration ({secs} s): {err}"
        )
    });
    assert!(
        scaled > Duration::ZERO,
        "LUABOX_PERF_FACTOR={factor} scales {which}'s {base:?} base down to \
         zero — too small a factor to widen anything with"
    );
    scaled
}

/// How long any single read from the server is allowed to take. Unlike
/// [`finish_timeout`] this has no ceiling: at `LUABOX_PERF_FACTOR=10` it is
/// 100 s, past the 30 s production `SHUTDOWN_EXIT_TIMEOUT` (`server.rs`).
/// That does not turn a genuine production timeout into a passing (merely
/// slow) read here — `read_timeout` only bounds patience for ordinary
/// handshake/setup traffic before a `shutdown`/`exit` pair is even sent, and
/// is not the bound this suite's regression proof relies on; `finish_timeout`
/// is, and that one stays capped under 30 s regardless of factor.
fn read_timeout() -> Duration {
    scaled_wait(READ_TIMEOUT_BASE, perf_factor(), "read_timeout")
}

/// How long `run` gets to return after the `exit` goes out. Scaled by
/// `LUABOX_PERF_FACTOR`, capped at [`FINISH_TIMEOUT_CEILING`] so a loaded
/// box's slack cannot also widen this past 30 s and let the round-9
/// regression pass as merely slow.
fn finish_timeout() -> Duration {
    scaled_wait(FINISH_TIMEOUT_BASE, perf_factor(), "finish_timeout").min(FINISH_TIMEOUT_CEILING)
}

/// The watchdog check [`Session::finish`] makes on the completion channel:
/// fail on the hang rather than sitting through it. Factored out to a free
/// function of the same `Result` `mpsc::Receiver::recv_timeout` hands back so
/// the failure case — `run` never signals, at any `bound` — is exercisable
/// deterministically, without a real thread or a real wait. See
/// `finish_still_rejects_a_timeout_at_the_scaled_bound` below.
fn assert_run_finished(
    ended: Result<(), mpsc::RecvTimeoutError>,
    bound: Duration,
    seen: &[Message],
) {
    assert!(
        ended.is_ok(),
        "`run` did not return within {bound:?} of the `exit` — \
         the shutdown handshake is waiting for a notification the server \
         is already holding. Seen: {seen:?}"
    );
}

/// A server running `luabox_lsp::run` on a worker thread, past the initialize
/// handshake, with the client end here.
struct Session {
    client: Connection,
    /// Fires when `run` returns — see the module note on why `join` alone is
    /// not enough.
    finished: mpsc::Receiver<()>,
    thread: Option<std::thread::JoinHandle<anyhow::Result<()>>>,
    /// Everything read off the wire so far, in arrival order. The assertions
    /// about progress traffic are about this.
    seen: Vec<Message>,
    next_id: i32,
    _dir: TempDir,
}

/// A client that advertises `window.workDoneProgress` — without it the server
/// sends no creates at all and there is no window to end a session inside.
fn progress_capabilities() -> ClientCapabilities {
    ClientCapabilities {
        window: Some(WindowClientCapabilities {
            work_done_progress: Some(true),
            ..WindowClientCapabilities::default()
        }),
        ..ClientCapabilities::default()
    }
}

/// Start `run` over an in-memory connection on a project holding `files`, and
/// complete the initialize handshake exactly as an editor does.
///
/// Files matter: `bootstrap` indexes them, and its progress token is the one
/// that reports per file. A project with nothing in it would still open the
/// second window, but a server that wrongly reported under a dead token would
/// have less to say and the assertions would be weaker.
fn start(files: &[(&str, &str)]) -> Session {
    let dir = TempDir::new().expect("tempdir");
    let root: PathBuf = dir.path().canonicalize().expect("canonicalize");
    for (rel, text) in files {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(path, text).expect("write");
    }

    let (server_conn, client) = Connection::memory();
    let (done, finished) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        let outcome = luabox_lsp::run(server_conn);
        let _ = done.send(());
        outcome
    });

    #[allow(deprecated, reason = "InitializeParams carries deprecated fields")]
    let params = InitializeParams {
        workspace_folders: Some(vec![WorkspaceFolder {
            uri: luabox_lsp::path_to_uri(&root),
            name: "test".to_string(),
        }]),
        capabilities: progress_capabilities(),
        ..InitializeParams::default()
    };
    let mut session = Session {
        client,
        finished,
        thread: Some(thread),
        seen: Vec::new(),
        next_id: 0,
        _dir: dir,
    };
    let id = session.request("initialize", serde_json::to_value(params).unwrap());
    session.await_response(&id);
    session.notify(Initialized::METHOD, json!({}));
    session
}

impl Session {
    /// Send a request and hand back the id it went out under.
    fn request(&mut self, method: &str, params: Value) -> RequestId {
        let id = RequestId::from(self.next_id);
        self.next_id += 1;
        self.client
            .sender
            .send(Message::Request(Request::new(
                id.clone(),
                method.to_string(),
                params,
            )))
            .expect("the server is still reading");
        id
    }

    fn notify(&self, method: &str, params: Value) {
        self.client
            .sender
            .send(Message::Notification(Notification::new(
                method.to_string(),
                params,
            )))
            .expect("the server is still reading");
    }

    /// Read one message, recording it.
    fn recv(&mut self) -> Message {
        let message = self
            .client
            .receiver
            .recv_timeout(read_timeout())
            .expect("the server answers within the read bound");
        self.seen.push(message.clone());
        message
    }

    /// Read up to and including the next `window/workDoneProgress/create`.
    fn await_create(&mut self) -> Request {
        loop {
            if let Message::Request(request) = self.recv()
                && request.method == WorkDoneProgressCreate::METHOD
            {
                return request;
            }
        }
    }

    /// Read up to and including the response to `id`.
    fn await_response(&mut self, id: &RequestId) -> Response {
        loop {
            if let Message::Response(response) = self.recv()
                && response.id == *id
            {
                return response;
            }
        }
    }

    /// Accept a create the way a real editor does.
    fn accept(&self, create: &Request) {
        self.client
            .sender
            .send(Message::Response(Response::new_ok(
                create.id.clone(),
                Value::Null,
            )))
            .expect("the server is still reading");
    }

    /// Refuse a create — the `-32601` answer a client without work-done
    /// progress support sends.
    fn refuse(&self, create: &Request) {
        self.client
            .sender
            .send(Message::Response(Response::new_err(
                create.id.clone(),
                ErrorCode::MethodNotFound as i32,
                "no work-done progress here".to_string(),
            )))
            .expect("the server is still reading");
    }

    /// The ordered `shutdown` → `exit` pair a clean session ends with.
    fn shut_down(&mut self) -> RequestId {
        let id = self.request(Shutdown::METHOD, Value::Null);
        self.notify(Exit::METHOD, Value::Null);
        id
    }

    /// Drain whatever is still on the wire without blocking.
    fn drain(&mut self) {
        while let Ok(message) = self.client.receiver.try_recv() {
            self.seen.push(message);
        }
    }

    /// Every `window/workDoneProgress/create` seen so far, as the token it
    /// carried.
    fn creates(&self) -> Vec<String> {
        self.seen
            .iter()
            .filter_map(|message| match message {
                Message::Request(request) if request.method == WorkDoneProgressCreate::METHOD => {
                    Some(request.params["token"].as_str().unwrap_or("").to_owned())
                }
                _ => None,
            })
            .collect()
    }

    /// Every `$/progress` seen so far, as the token it was reported under.
    fn progress_tokens(&self) -> Vec<String> {
        self.seen
            .iter()
            .filter_map(|message| match message {
                Message::Notification(not) if not.method == Progress::METHOD => {
                    Some(not.params["token"].as_str().unwrap_or("").to_owned())
                }
                _ => None,
            })
            .collect()
    }

    /// Wait for `run` to return, failing on a hang rather than sitting
    /// through one, and hand back what it returned.
    ///
    /// Takes `&mut self` so the traffic assertions can still read
    /// [`Self::seen`] afterwards — what the client observed is half of what
    /// these tests are about, and that the thread ended is the other half.
    fn finish(&mut self) -> anyhow::Result<()> {
        let bound = finish_timeout();
        let ended = self.finished.recv_timeout(bound);
        self.drain();
        assert_run_finished(ended, bound, &self.seen);
        self.thread
            .take()
            .expect("finish is called once")
            .join()
            .expect("the server thread does not panic")
    }
}

/// A project with enough files that `bootstrap`'s token has per-file reports
/// to make, so a server reporting under a dead token is loud about it.
const FILES: &[(&str, &str)] = &[
    ("a.lua", "return {}\n"),
    ("b.lua", "return {}\n"),
    ("c.lua", "return {}\n"),
];

/// **The filed round-9 repro.** `shutdown` + `exit` sent into the *first*
/// create window — the startup rock harvest, before the loop is entered.
///
/// Window 1 aborts on the `shutdown` and leaves the `exit` on the channel,
/// which is correct. What was not correct is that `bootstrap` then opened a
/// second window, read that `exit` off the channel onto `Server::pending`, and
/// left `Connection::handle_shutdown` reading an empty channel for 30 s before
/// returning the protocol error that becomes exit 1.
#[test]
fn a_shutdown_and_exit_inside_the_first_create_window_end_the_session_cleanly() {
    let mut session = start(FILES);
    let create = session.await_create();
    assert!(
        create.params["token"]
            .as_str()
            .unwrap()
            .starts_with("luabox/rock-harvest"),
        "the first window is the startup harvest's: {create:?}"
    );
    // Answered with a shutdown instead of a response — what an editor sends
    // when the user closes the window during the harvest.
    let shutdown = session.shut_down();
    session.await_response(&shutdown);

    session.finish().expect("a clean shutdown, not exit 1");
}

/// The same window, with the request sent twice. A client that repeats its
/// `shutdown` must not turn a clean exit into a protocol error: the handshake
/// answers the request it is holding, and keeps looking for the `exit`.
#[test]
fn two_shutdowns_then_an_exit_inside_the_first_window_end_the_session_cleanly() {
    let mut session = start(FILES);
    session.await_create();
    let first = session.request(Shutdown::METHOD, Value::Null);
    let second = session.request(Shutdown::METHOD, Value::Null);
    session.notify(Exit::METHOD, Value::Null);

    session.await_response(&first);
    session.await_response(&second);
    session.finish().expect("a clean shutdown, not exit 1");
}

/// The second window on its own: the client answers the startup create
/// normally, and only then ends the session — so the `shutdown` lands inside
/// `bootstrap`'s window rather than the harvest's.
///
/// Nothing may be reported under `bootstrap`'s token after that: the client
/// has asked to shut down, and a `begin`/`report`/`end` sequence plus another
/// create is traffic to a client that is leaving.
#[test]
fn an_exit_after_the_first_create_is_answered_ends_the_session_cleanly() {
    let mut session = start(FILES);
    let harvest = session.await_create();
    session.accept(&harvest);

    let bootstrap = session.await_create();
    let shutdown = session.shut_down();
    session.await_response(&shutdown);
    session.finish().expect("a clean shutdown, not exit 1");

    let bootstrap_token = bootstrap.params["token"].as_str().unwrap();
    assert!(
        !session
            .progress_tokens()
            .iter()
            .any(|token| token == bootstrap_token),
        "the session was ending, so nothing may be reported under \
         `{bootstrap_token}`: {:?}",
        session.progress_tokens()
    );
}

/// A config reload opens a create window from *inside* the loop. One reload,
/// answered, then an ordinary shutdown: the control that says the fix did not
/// break the working path.
#[test]
fn a_single_reload_then_a_shutdown_ends_the_session_cleanly() {
    let mut session = start(FILES);
    let harvest = session.await_create();
    session.accept(&harvest);
    let bootstrap = session.await_create();
    session.accept(&bootstrap);

    session.notify(
        DidChangeConfiguration::METHOD,
        serde_json::to_value(DidChangeConfigurationParams {
            settings: json!({}),
        })
        .unwrap(),
    );
    let reload = session.await_create();
    assert!(
        reload.params["token"]
            .as_str()
            .unwrap()
            .starts_with("luabox/reload"),
        "the third window is the reload's: {reload:?}"
    );
    session.accept(&reload);

    let shutdown = session.shut_down();
    session.await_response(&shutdown);
    session.finish().expect("a clean shutdown");
}

/// **The second-queued-reload variant.** Two `didChangeConfiguration`
/// notifications are queued back to back and the session is ended inside the
/// *first* reload's window.
///
/// The first window aborts on the `shutdown` — and then the loop still has the
/// second reload queued, which opens *another* window, which drains the
/// `exit`. Aborting one window is not enough; a session that is ending must
/// stop opening them.
#[test]
fn two_queued_reloads_with_a_shutdown_in_the_first_window_end_the_session_cleanly() {
    let mut session = start(FILES);
    let harvest = session.await_create();
    session.accept(&harvest);
    let bootstrap = session.await_create();
    session.accept(&bootstrap);

    let reload_params = serde_json::to_value(DidChangeConfigurationParams {
        settings: json!({}),
    })
    .unwrap();
    session.notify(DidChangeConfiguration::METHOD, reload_params.clone());
    session.notify(DidChangeConfiguration::METHOD, reload_params);

    // The first reload's create. The second reload is already queued behind
    // it, so the shutdown sent here has a whole extra window to fall into.
    session.await_create();
    let shutdown = session.shut_down();
    session.await_response(&shutdown);
    session.finish().expect("a clean shutdown, not exit 1");
}

/// B2. A client that answers *nothing* sets the never-answers flag, which
/// used to skip the **wait** on the next create while still sending it — so a
/// late error answer to that second create landed in `main_loop`'s discard
/// arm, and `$/progress` went out under a token the client had refused.
///
/// The refusal check is what the whole create mechanism exists for, and a
/// latency optimisation may not step around it. The flag now means *no
/// further creates at all*: the token that timed out keeps the degraded path
/// (it is the behaviour this mechanism replaced, so it is no worse), and
/// every later announcement is silent for the rest of the session.
///
/// Written so it fails either way it can be wrong: if a second create goes
/// out it is refused here, and any `$/progress` under its token is the defect.
#[test]
fn a_create_after_an_unanswered_one_never_carries_progress_the_client_refused() {
    let mut session = start(FILES);
    // Ignore the startup create entirely: no response, no error. The server
    // waits out `PROGRESS_CREATE_TIMEOUT` and sets the never-answers flag.
    let ignored = session.await_create();
    let ignored_token = ignored.params["token"].as_str().unwrap().to_owned();

    // Give the session room to try again, refusing anything it sends. A
    // second create is the pre-fix behaviour; after the fix this read times
    // out because there is nothing more to read.
    let mut refused: Vec<String> = Vec::new();
    while let Ok(message) = session
        .client
        .receiver
        .recv_timeout(Duration::from_millis(750))
    {
        session.seen.push(message.clone());
        if let Message::Request(request) = &message
            && request.method == WorkDoneProgressCreate::METHOD
        {
            refused.push(request.params["token"].as_str().unwrap_or("").to_owned());
            session.refuse(request);
        }
    }

    let shutdown = session.shut_down();
    session.await_response(&shutdown);
    session.finish().expect("a clean shutdown");

    for token in &refused {
        assert!(
            !session.progress_tokens().iter().any(|seen| seen == token),
            "progress was reported under `{token}`, which the client refused: \
             {:?}",
            session.progress_tokens()
        );
    }
    // …and the chosen semantics: one timeout mutes the session. The token
    // that timed out still reports — that is the degraded path this mechanism
    // replaced — but nothing else is even asked for.
    assert_eq!(
        session.creates(),
        vec![ignored_token.clone()],
        "one timeout must mute every later create: {:?}",
        session.creates()
    );
    assert!(
        session
            .progress_tokens()
            .iter()
            .all(|token| *token == ignored_token),
        "only the timed-out token may be reported under: {:?}",
        session.progress_tokens()
    );
}

/// Check the configured watchdog stays below the production timeout and
/// that its assertion rejects a timeout result (#84). Injecting the result
/// avoids a scheduler-dependent sleep. This unit test covers the assertion,
/// not the full `Session::finish` timeout path; the six protocol tests above
/// exercise successful shutdown through the real session.
#[test]
fn finish_still_rejects_a_timeout_at_the_scaled_bound() {
    let bound = finish_timeout();
    assert!(
        bound < Duration::from_secs(30),
        "finish_timeout() must stay under production's 30 s \
         SHUTDOWN_EXIT_TIMEOUT for this proof to mean anything, got {bound:?}"
    );

    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        assert_run_finished(Err(mpsc::RecvTimeoutError::Timeout), bound, &[]);
    }));
    assert!(
        outcome.is_err(),
        "assert_run_finished must panic on a timeout at finish_timeout() \
         ({bound:?}), even at the largest LUABOX_PERF_FACTOR this cap allows — \
         a quiet return here means the hang detector stopped detecting hangs"
    );
}

/// `resolve_perf_factor`'s parsing policy — every case runs against the
/// `Result` shape directly, never `std::env::set_var`/`remove_var`: this
/// binary runs its `#[test]` functions in parallel by default, and the real
/// process environment is shared across all of them.
// Every case parses a literal straight from a `&str` and compares it to the
// same literal typed as `f64` — an exact round trip with no arithmetic in
// between, so there is no accumulated float error for `float_cmp` to be
// warning about.
#[allow(clippy::float_cmp)]
mod perf_factor_parsing {
    use super::resolve_perf_factor;
    use std::env::VarError;

    #[test]
    fn unset_defaults_to_one() {
        assert_eq!(resolve_perf_factor(Err(VarError::NotPresent)), 1.0);
    }

    #[test]
    fn a_plain_integer_is_honoured() {
        assert_eq!(resolve_perf_factor(Ok("4".to_string())), 4.0);
    }

    #[test]
    fn a_fractional_value_is_honoured() {
        assert_eq!(resolve_perf_factor(Ok("2.5".to_string())), 2.5);
    }

    #[test]
    fn surrounding_whitespace_is_trimmed() {
        assert_eq!(resolve_perf_factor(Ok("  3  ".to_string())), 3.0);
    }

    #[test]
    #[should_panic(expected = "not a float")]
    fn non_numeric_panics() {
        resolve_perf_factor(Ok("fast".to_string()));
    }

    #[test]
    #[should_panic(expected = "finite, positive number")]
    fn zero_panics() {
        resolve_perf_factor(Ok("0".to_string()));
    }

    #[test]
    #[should_panic(expected = "finite, positive number")]
    fn negative_panics() {
        resolve_perf_factor(Ok("-2".to_string()));
    }

    #[test]
    #[should_panic(expected = "finite, positive number")]
    fn nan_panics() {
        resolve_perf_factor(Ok("NaN".to_string()));
    }

    #[test]
    #[should_panic(expected = "finite, positive number")]
    fn infinite_panics() {
        resolve_perf_factor(Ok("inf".to_string()));
    }

    #[test]
    #[should_panic(expected = "not valid UTF-8")]
    fn non_unicode_panics() {
        resolve_perf_factor(Err(VarError::NotUnicode("\u{FFFD}".into())));
    }
}

/// **Red-first (#84, B1).** `resolve_perf_factor` alone does not bound
/// `factor` to what `base * factor` can represent as a `Duration` — these
/// drive [`scaled_wait`] directly with factors large/small enough to break
/// that, without ever touching the real `LUABOX_PERF_FACTOR`.
mod scaled_wait_bounds {
    use super::{FINISH_TIMEOUT_BASE, READ_TIMEOUT_BASE, scaled_wait};

    #[test]
    #[should_panic(expected = "unrepresentable duration")]
    fn a_factor_large_enough_to_overflow_a_duration_panics() {
        scaled_wait(FINISH_TIMEOUT_BASE, 1e300, "finish_timeout");
    }

    #[test]
    #[should_panic(expected = "down to zero")]
    fn a_factor_small_enough_to_underflow_to_zero_panics() {
        scaled_wait(READ_TIMEOUT_BASE, 1e-300, "read_timeout");
    }
}

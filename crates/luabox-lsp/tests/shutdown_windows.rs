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

/// How long any single read from the server is allowed to take. Generous
/// relative to the work involved, and far below the 30 s block that is the
/// failure being tested for.
const READ_TIMEOUT: Duration = Duration::from_secs(10);

/// How long `run` gets to return after the `exit` goes out. The bug makes it
/// take 30 s; a healthy server takes microseconds.
const FINISH_TIMEOUT: Duration = Duration::from_secs(5);

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
            .recv_timeout(READ_TIMEOUT)
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
        let ended = self.finished.recv_timeout(FINISH_TIMEOUT);
        self.drain();
        assert!(
            ended.is_ok(),
            "`run` did not return within {FINISH_TIMEOUT:?} of the `exit` — \
             the shutdown handshake is waiting for a notification the server \
             is already holding. Seen: {:?}",
            self.seen
        );
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

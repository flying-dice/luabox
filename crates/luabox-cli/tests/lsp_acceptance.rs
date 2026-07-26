//! Cucumber acceptance tests for `luabox lsp` — the editor-facing surface
//! (SPEC.md §8 + §16.2).
//!
//! Black-box, like `acceptance.rs`: every scenario spawns the real `luabox`
//! binary with the `lsp` subcommand and speaks LSP JSON-RPC over the child's
//! stdin/stdout (`Content-Length` framing). Nothing links `luabox-lsp`
//! directly, so a scenario exercises the whole stack the editor sees —
//! transport, capability advertisement, salsa analysis host, and the
//! `luabox-db` queries behind every request.
//!
//! Reliability rules the harness enforces:
//!
//! - replies are matched **by request id**, so an interleaved
//!   `publishDiagnostics` (or any other server-initiated notification) never
//!   gets mistaken for a response;
//! - diagnostics are unsolicited, so every notification is collected as it is
//!   read and the `didOpen`/`didChange` steps block until *their* file's
//!   publish arrives — no sleeps anywhere;
//! - the child is always driven through `shutdown`/`exit` and reaped when the
//!   world drops, so a failing scenario cannot leak a server process.

// Cucumber step functions receive owned captures by signature contract.
#![allow(clippy::needless_pass_by_value)]
// test code — panics document assumptions
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::string_slice
)]

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cucumber::gherkin::Step;
use cucumber::{World, given, then, when};
use serde_json::{Value, json};

/// How long a step waits for a reply before declaring the server hung.
/// Generous: a cold `cargo test` run may start dozens of servers at once.
const REPLY_TIMEOUT: Duration = Duration::from_secs(60);

/// How long teardown waits for a cleanly shut-down child before killing it.
const EXIT_TIMEOUT: Duration = Duration::from_secs(10);

// === Transport ===========================================================

/// A running `luabox lsp` child: its stdin (framed writes), a reader thread
/// decoding `Content-Length` frames off stdout into `rx`, and the handles
/// needed to reap both.
#[derive(Debug)]
struct Server {
    child: Child,
    /// `None` once teardown has closed the pipe (the server's EOF signal).
    stdin: Option<ChildStdin>,
    reader: Option<JoinHandle<()>>,
    rx: Receiver<Value>,
}

impl Server {
    /// Spawn `luabox lsp` rooted at `root` and start decoding its stdout.
    fn spawn(root: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_luabox"))
            .arg("lsp")
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // The server logs manifest problems to stderr; discard it rather
            // than pipe it, so a chatty server can never fill a pipe buffer
            // and deadlock the scenario.
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to spawn `luabox lsp`");
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let (tx, rx) = channel();
        let reader = std::thread::spawn(move || read_frames(stdout, &tx));
        Self {
            child,
            stdin: Some(stdin),
            reader: Some(reader),
            rx,
        }
    }

    /// Write one `Content-Length`-framed JSON-RPC message.
    fn send(&mut self, message: &Value) {
        let body = serde_json::to_vec(message).expect("serialize message");
        let stdin = self.stdin.as_mut().expect("stdin is still open");
        write!(stdin, "Content-Length: {}\r\n\r\n", body.len()).expect("write header");
        stdin.write_all(&body).expect("write body");
        stdin.flush().expect("flush");
    }

    /// Complete the `shutdown`/`exit` handshake, close stdin, and reap the
    /// child — killing it if it overstays [`EXIT_TIMEOUT`]. Every step is
    /// best-effort: teardown must never panic over an already-dead server.
    fn stop(&mut self) {
        if self.stdin.is_some() {
            self.send(&json!({
                "jsonrpc": "2.0",
                "id": SHUTDOWN_ID,
                "method": "shutdown",
                "params": Value::Null,
            }));
            let deadline = Instant::now() + EXIT_TIMEOUT;
            while Instant::now() < deadline {
                match self.rx.recv_timeout(Duration::from_millis(200)) {
                    Ok(message) => {
                        if message.get("id").and_then(Value::as_i64) == Some(SHUTDOWN_ID) {
                            break;
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
            self.send(&json!({ "jsonrpc": "2.0", "method": "exit", "params": Value::Null }));
        }
        // EOF on stdin is the server's unambiguous stop signal.
        drop(self.stdin.take());
        self.reap();
        if let Some(reader) = self.reader.take() {
            // The child is gone, so stdout is at EOF and the thread has ended.
            let _ = reader.join();
        }
    }

    /// Wait for the child to exit, killing it once [`EXIT_TIMEOUT`] passes.
    fn reap(&mut self) {
        let deadline = Instant::now() + EXIT_TIMEOUT;
        loop {
            match self.child.try_wait() {
                // Exited, or unwaitable — either way there is nothing to reap.
                Ok(Some(_)) | Err(_) => return,
                Ok(None) => {}
            }
            if Instant::now() >= deadline {
                let _ = self.child.kill();
                let _ = self.child.wait();
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

/// The request id teardown uses for `shutdown` — far outside the range the
/// scenarios allocate, so it can never collide with a pending reply.
const SHUTDOWN_ID: i64 = 999_999;

/// Decode `Content-Length`-framed JSON messages off `stdout` until EOF (or
/// until the receiving world is dropped), forwarding each to `tx`.
fn read_frames(stdout: ChildStdout, tx: &Sender<Value>) {
    let mut reader = BufReader::new(stdout);
    while let Some(message) = read_frame(&mut reader) {
        if tx.send(message).is_err() {
            return;
        }
    }
}

/// Read one framed message: headers until the blank line, then exactly
/// `Content-Length` bytes of JSON. `None` at EOF or on a malformed frame.
fn read_frame(reader: &mut BufReader<ChildStdout>) -> Option<Value> {
    let mut length: Option<usize> = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let header = line.trim_end_matches(['\r', '\n']);
        if header.is_empty() {
            break;
        }
        // Header names are case-insensitive per the protocol.
        let (name, value) = header.split_once(':')?;
        if name.trim().eq_ignore_ascii_case("content-length") {
            length = value.trim().parse().ok();
        }
    }
    let mut body = vec![0u8; length?];
    reader.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}

/// Render a filesystem path as a `file://` URI, percent-encoding everything
/// outside the URI path charset (mirrors `luabox_lsp::path_to_uri`, which the
/// server uses for the URIs it hands back).
fn path_to_uri(path: &Path) -> String {
    use std::fmt::Write as _;

    let path = path.to_string_lossy().replace('\\', "/");
    let mut uri = String::from("file://");
    if !path.starts_with('/') {
        uri.push('/');
    }
    for &byte in path.as_bytes() {
        if byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'-' | b'.'
                    | b'_'
                    | b'~'
                    | b'!'
                    | b'$'
                    | b'&'
                    | b'\''
                    | b'('
                    | b')'
                    | b'*'
                    | b'+'
                    | b','
                    | b';'
                    | b'='
                    | b':'
                    | b'@'
                    | b'/'
            )
        {
            uri.push(byte as char);
        } else {
            let _ = write!(uri, "%{byte:02X}");
        }
    }
    uri
}

// === World ===============================================================

#[derive(Debug, World)]
#[world(init = Self::new)]
struct LspWorld {
    /// Owns the fixture project for the scenario's lifetime.
    _dir: tempfile::TempDir,
    /// The canonical fixture root: URIs are built from it, and the server is
    /// initialized against it, so both sides agree on file identity.
    root: PathBuf,
    server: Option<Server>,
    /// The `initialize` result — capabilities and server info.
    init: Value,
    /// The latest `publishDiagnostics` payload per URI.
    diagnostics: HashMap<String, Vec<Value>>,
    /// The last request's `result` and `error`.
    reply: Value,
    error: Option<Value>,
    /// The call-hierarchy item stashed by a prepare step.
    item: Option<Value>,
    next_id: i64,
}

impl LspWorld {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let root = dir.path().canonicalize().expect("canonicalize temp dir");
        Self {
            _dir: dir,
            root,
            server: None,
            init: Value::Null,
            diagnostics: HashMap::new(),
            reply: Value::Null,
            error: None,
            item: None,
            next_id: 1,
        }
    }

    fn uri(&self, relative: &str) -> String {
        path_to_uri(&self.root.join(relative))
    }

    fn write_file(&self, relative: &str, content: &str) {
        let path = self.root.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("failed to create parent directories");
        }
        std::fs::write(&path, content).unwrap_or_else(|e| panic!("cannot write `{relative}`: {e}"));
    }

    /// Spawn the server and complete the `initialize`/`initialized` handshake.
    fn start(&mut self) {
        assert!(
            self.server.is_none(),
            "the language server is already running"
        );
        self.server = Some(Server::spawn(&self.root));
        let root_uri = path_to_uri(&self.root);
        let params = json!({
            "processId": Value::Null,
            "rootUri": root_uri,
            "workspaceFolders": [{ "uri": root_uri, "name": "fixture" }],
            // Deliberately bare: no dynamic registration and no work-done
            // progress, so the server never sends a request of its own and
            // every message the scenario sees is a reply or a notification.
            "capabilities": {},
        });
        self.request("initialize", params);
        self.init = self.ok_reply().clone();
        self.notify("initialized", json!({}));
    }

    fn send(&mut self, message: &Value) {
        self.server
            .as_mut()
            .expect("the language server is not running")
            .send(message);
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.send(&json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    /// Read the next message, recording server-initiated notifications.
    fn pump(&mut self) -> Value {
        let message = {
            let server = self
                .server
                .as_ref()
                .expect("the language server is not running");
            server.rx.recv_timeout(REPLY_TIMEOUT).unwrap_or_else(|e| {
                panic!("the language server sent nothing within the timeout: {e}")
            })
        };
        if message.get("method").and_then(Value::as_str) == Some("textDocument/publishDiagnostics")
            && let Some(uri) = message["params"]["uri"].as_str()
        {
            let diagnostics = message["params"]["diagnostics"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            self.diagnostics.insert(uri.to_string(), diagnostics);
        }
        message
    }

    /// Send a request and wait for *its* reply, id-matched — notifications and
    /// any other traffic that arrives first are consumed on the way.
    fn request(&mut self, method: &str, params: Value) {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }));
        let response = loop {
            let message = self.pump();
            if message.get("method").is_none()
                && message.get("id").and_then(Value::as_i64) == Some(id)
            {
                break message;
            }
        };
        self.error = response.get("error").cloned();
        self.reply = response.get("result").cloned().unwrap_or(Value::Null);
    }

    /// The last reply, asserting the server did not answer with an error.
    fn ok_reply(&self) -> &Value {
        assert!(
            self.error.is_none(),
            "the server replied with an error: {:?}",
            self.error
        );
        &self.reply
    }

    fn reply_array(&self) -> &Vec<Value> {
        self.ok_reply()
            .as_array()
            .unwrap_or_else(|| panic!("expected a list reply, got {}", self.reply))
    }

    /// Block until the next `publishDiagnostics` for `uri` arrives.
    fn wait_diagnostics(&mut self, uri: &str) {
        loop {
            let message = self.pump();
            if message.get("method").and_then(Value::as_str)
                == Some("textDocument/publishDiagnostics")
                && message["params"]["uri"].as_str() == Some(uri)
            {
                return;
            }
        }
    }

    fn diagnostics_for(&self, relative: &str) -> &[Value] {
        let uri = self.uri(relative);
        self.diagnostics
            .get(&uri)
            .unwrap_or_else(|| panic!("no diagnostics have been published for `{relative}`"))
    }

    /// The value at a dotted path inside the advertised capabilities.
    fn capability(&self, path: &str) -> &Value {
        let mut current = &self.init["capabilities"];
        for segment in path.split('.') {
            current = &current[segment];
        }
        current
    }

    fn text_document(&self, relative: &str) -> Value {
        json!({ "uri": self.uri(relative) })
    }

    fn doc_position(&self, relative: &str, line: u32, character: u32) -> Value {
        json!({
            "textDocument": { "uri": self.uri(relative) },
            "position": { "line": line, "character": character },
        })
    }
}

impl Drop for LspWorld {
    fn drop(&mut self) {
        if let Some(mut server) = self.server.take() {
            server.stop();
        }
    }
}

// === Shared helpers ======================================================

/// The step's docstring, normalized the same way `acceptance.rs` does: the
/// leading newline after `"""` stripped, exactly one trailing newline.
fn docstring(step: &Step) -> String {
    let raw = step
        .docstring
        .as_deref()
        .expect("this step requires a docstring (\"\"\" … \"\"\")");
    let body = raw.strip_prefix('\n').unwrap_or(raw);
    format!("{}\n", body.trim_end_matches(['\n', '\r']))
}

fn range(start_line: u32, start_col: u32, end_line: u32, end_col: u32) -> Value {
    json!({
        "start": { "line": start_line, "character": start_col },
        "end": { "line": end_line, "character": end_col },
    })
}

/// A diagnostic's `code`, which luabox always reports as an `LBxxxx` string.
fn code_of(diagnostic: &Value) -> &str {
    diagnostic["code"].as_str().unwrap_or_default()
}

/// The single location a goto-style reply carries, whether the server answered
/// with a scalar `Location` or a one-element array.
fn single_location(reply: &Value) -> &Value {
    match reply {
        Value::Array(locations) => {
            assert_eq!(locations.len(), 1, "expected exactly one location: {reply}");
            &locations[0]
        }
        Value::Object(_) => reply,
        other => panic!("expected a location, got {other}"),
    }
}

/// One absolute semantic token, resolved against the advertised legend.
struct DecodedToken {
    line: u32,
    start: u32,
    token_type: String,
    modifiers: Vec<String>,
}

/// Decode the delta-encoded `semanticTokens/full` stream into absolute tokens,
/// resolving type and modifier indices through the legend the server
/// advertised at initialize.
fn decode_tokens(world: &LspWorld) -> Vec<DecodedToken> {
    let legend = world.capability("semanticTokensProvider.legend");
    let types: Vec<&str> = legend["tokenTypes"]
        .as_array()
        .expect("legend token types")
        .iter()
        .map(|t| t.as_str().unwrap_or_default())
        .collect();
    let modifiers: Vec<&str> = legend["tokenModifiers"]
        .as_array()
        .expect("legend token modifiers")
        .iter()
        .map(|t| t.as_str().unwrap_or_default())
        .collect();
    let data = world.ok_reply()["data"]
        .as_array()
        .expect("a semantic token data array")
        .clone();
    assert_eq!(data.len() % 5, 0, "the token stream must be 5-tuples");

    let mut out = Vec::new();
    let (mut line, mut start) = (0u32, 0u32);
    for token in data.chunks_exact(5) {
        let field = |i: usize| u32::try_from(token[i].as_u64().expect("token field")).unwrap();
        let (delta_line, delta_start) = (field(0), field(1));
        if delta_line > 0 {
            line += delta_line;
            start = delta_start;
        } else {
            start += delta_start;
        }
        let type_index = field(3) as usize;
        let bitset = field(4);
        out.push(DecodedToken {
            line,
            start,
            token_type: types
                .get(type_index)
                .unwrap_or_else(|| panic!("token type {type_index} is outside the legend"))
                .to_string(),
            modifiers: modifiers
                .iter()
                .enumerate()
                .filter(|(i, _)| bitset & (1 << i) != 0)
                .map(|(_, name)| (*name).to_string())
                .collect(),
        });
    }
    out
}

// === Project fixtures ====================================================

fn write_manifest(world: &LspWorld, edition: &str, strict: bool) {
    let manifest = format!(
        "[package]\n\
         name = \"fixture\"\n\
         version = \"0.1.0\"\n\
         edition = \"{edition}\"\n\
         \n\
         [types]\n\
         strict = {strict}\n"
    );
    world.write_file("luabox.toml", &manifest);
}

#[given(expr = "a project with edition {string}")]
fn project_with_edition(world: &mut LspWorld, edition: String) {
    write_manifest(world, &edition, false);
}

#[given(expr = "a strict project with edition {string}")]
fn strict_project_with_edition(world: &mut LspWorld, edition: String) {
    write_manifest(world, &edition, true);
}

#[given(expr = "a file {string} containing:")]
fn file_containing(world: &mut LspWorld, path: String, step: &Step) {
    world.write_file(&path, &docstring(step));
}

#[given("the language server is running")]
#[when("the language server starts")]
fn language_server_starts(world: &mut LspWorld) {
    world.start();
}

#[given(expr = "the document {string} is open")]
#[when(expr = "I open {string}")]
fn open_document(world: &mut LspWorld, path: String) {
    let uri = world.uri(&path);
    let text = std::fs::read_to_string(world.root.join(&path))
        .unwrap_or_else(|e| panic!("cannot read `{path}`: {e}"));
    world.notify(
        "textDocument/didOpen",
        json!({
            "textDocument": {
                "uri": uri,
                "languageId": "lua",
                "version": 1,
                "text": text,
            }
        }),
    );
    world.wait_diagnostics(&uri);
}

#[given(expr = "I change {string} to:")]
#[when(expr = "I change {string} to:")]
fn change_document(world: &mut LspWorld, path: String, step: &Step) {
    let uri = world.uri(&path);
    world.notify(
        "textDocument/didChange",
        json!({
            "textDocument": { "uri": uri, "version": 2 },
            "contentChanges": [{ "text": docstring(step) }],
        }),
    );
    world.wait_diagnostics(&uri);
}

#[when(expr = "I replace {int}:{int} through {int}:{int} of {string} with {string}")]
fn change_document_incrementally(
    world: &mut LspWorld,
    start_line: u32,
    start_col: u32,
    end_line: u32,
    end_col: u32,
    path: String,
    replacement: String,
) {
    let uri = world.uri(&path);
    world.notify(
        "textDocument/didChange",
        json!({
            "textDocument": { "uri": uri, "version": 2 },
            "contentChanges": [{
                "range": range(start_line, start_col, end_line, end_col),
                "text": replacement,
            }],
        }),
    );
    world.wait_diagnostics(&uri);
}

#[when(expr = "I close {string}")]
fn close_document(world: &mut LspWorld, path: String) {
    let uri = world.uri(&path);
    world.notify(
        "textDocument/didClose",
        json!({ "textDocument": { "uri": uri } }),
    );
    world.wait_diagnostics(&uri);
}

// === Handshake ===========================================================

#[then(expr = "the server identifies itself as {string}")]
fn server_identifies_itself(world: &mut LspWorld, name: String) {
    assert_eq!(
        world.init["serverInfo"]["name"].as_str(),
        Some(name.as_str()),
        "serverInfo: {}",
        world.init["serverInfo"]
    );
}

#[then(expr = "capability {string} is advertised")]
fn capability_is_advertised(world: &mut LspWorld, path: String) {
    let value = world.capability(&path);
    assert!(
        value != &Value::Null && value != &Value::Bool(false),
        "capability `{path}` is not advertised: {value}"
    );
}

#[then(expr = "capability {string} equals {int}")]
fn capability_equals_int(world: &mut LspWorld, path: String, expected: i64) {
    let value = world.capability(&path);
    assert_eq!(
        value.as_i64(),
        Some(expected),
        "capability `{path}` is {value}"
    );
}

#[then(expr = "capability {string} includes {string}")]
fn capability_includes(world: &mut LspWorld, path: String, expected: String) {
    let value = world.capability(&path);
    let items = value
        .as_array()
        .unwrap_or_else(|| panic!("capability `{path}` is not a list: {value}"));
    assert!(
        items.iter().any(|item| item.as_str() == Some(&expected)),
        "capability `{path}` does not include `{expected}`: {value}"
    );
}

#[when("I send an unsupported request")]
fn send_unsupported_request(world: &mut LspWorld) {
    // A method outside the advertised set: the server must answer, not hang.
    world.request("textDocument/moniker", json!({}));
}

#[then(expr = "the server replies with an error mentioning {string}")]
fn reply_is_error_mentioning(world: &mut LspWorld, needle: String) {
    let error = world
        .error
        .as_ref()
        .unwrap_or_else(|| panic!("expected an error reply, got {}", world.reply));
    let message = error["message"].as_str().unwrap_or_default();
    assert!(
        message.contains(&needle),
        "error message `{message}` does not mention `{needle}`"
    );
}

// === Diagnostics =========================================================

#[then(expr = "the diagnostics for {string} include {word}")]
fn diagnostics_include(world: &mut LspWorld, path: String, code: String) {
    let diagnostics = world.diagnostics_for(&path);
    assert!(
        diagnostics.iter().any(|d| code_of(d) == code),
        "expected `{code}` among the diagnostics for `{path}`: {diagnostics:?}"
    );
}

#[then(expr = "the diagnostics for {string} do not include {word}")]
fn diagnostics_do_not_include(world: &mut LspWorld, path: String, code: String) {
    let diagnostics = world.diagnostics_for(&path);
    assert!(
        !diagnostics.iter().any(|d| code_of(d) == code),
        "unexpected `{code}` among the diagnostics for `{path}`: {diagnostics:?}"
    );
}

#[then(expr = "the diagnostics for {string} are empty")]
fn diagnostics_are_empty(world: &mut LspWorld, path: String) {
    let diagnostics = world.diagnostics_for(&path);
    assert!(
        diagnostics.is_empty(),
        "expected no diagnostics for `{path}`: {diagnostics:?}"
    );
}

/// The one diagnostic carrying `code`, for the per-field assertions below.
fn diagnostic_with_code<'a>(world: &'a LspWorld, path: &str, code: &str) -> &'a Value {
    let diagnostics = world.diagnostics_for(path);
    diagnostics
        .iter()
        .find(|d| code_of(d) == code)
        .unwrap_or_else(|| panic!("no `{code}` diagnostic for `{path}`: {diagnostics:?}"))
}

#[then(expr = "diagnostic {word} in {string} is an error")]
fn diagnostic_is_an_error(world: &mut LspWorld, code: String, path: String) {
    let diagnostic = diagnostic_with_code(world, &path, &code);
    assert_eq!(diagnostic["severity"].as_u64(), Some(1), "{diagnostic}");
}

#[then(expr = "diagnostic {word} in {string} is a warning")]
fn diagnostic_is_a_warning(world: &mut LspWorld, code: String, path: String) {
    let diagnostic = diagnostic_with_code(world, &path, &code);
    assert_eq!(diagnostic["severity"].as_u64(), Some(2), "{diagnostic}");
}

#[then(expr = "diagnostic {word} in {string} comes from {string}")]
fn diagnostic_source(world: &mut LspWorld, code: String, path: String, source: String) {
    let diagnostic = diagnostic_with_code(world, &path, &code);
    assert_eq!(
        diagnostic["source"].as_str(),
        Some(source.as_str()),
        "{diagnostic}"
    );
}

#[then(expr = "diagnostic {word} in {string} spans {int}:{int} to {int}:{int}")]
fn diagnostic_spans(
    world: &mut LspWorld,
    code: String,
    path: String,
    start_line: u32,
    start_col: u32,
    end_line: u32,
    end_col: u32,
) {
    let diagnostic = diagnostic_with_code(world, &path, &code);
    assert_eq!(
        diagnostic["range"],
        range(start_line, start_col, end_line, end_col),
        "{diagnostic}"
    );
}

// === Generic reply shape =================================================

#[then("the reply is null")]
fn reply_is_null(world: &mut LspWorld) {
    assert_eq!(*world.ok_reply(), Value::Null, "expected a null reply");
}

#[then("the reply is an empty list")]
fn reply_is_an_empty_list(world: &mut LspWorld) {
    assert!(
        world.reply_array().is_empty(),
        "expected an empty list, got {}",
        world.reply
    );
}

// === Hover ===============================================================

#[when(expr = "I hover at {int}:{int} in {string}")]
fn hover_at(world: &mut LspWorld, line: u32, character: u32, path: String) {
    let params = world.doc_position(&path, line, character);
    world.request("textDocument/hover", params);
}

#[then(expr = "the hover text contains {string}")]
fn hover_text_contains(world: &mut LspWorld, needle: String) {
    let text = world.ok_reply()["contents"]["value"]
        .as_str()
        .unwrap_or_else(|| panic!("expected markup hover contents, got {}", world.reply));
    assert!(text.contains(&needle), "hover text:\n{text}");
}

// === Completion ==========================================================

#[when(expr = "I request completion at {int}:{int} in {string}")]
fn request_completion(world: &mut LspWorld, line: u32, character: u32, path: String) {
    let params = world.doc_position(&path, line, character);
    world.request("textDocument/completion", params);
}

/// The completion item labelled `label`, or a panic naming every offered item.
fn completion_item<'a>(world: &'a LspWorld, label: &str) -> &'a Value {
    let items = world.reply_array();
    items
        .iter()
        .find(|item| item["label"].as_str() == Some(label))
        .unwrap_or_else(|| {
            let labels: Vec<&str> = items
                .iter()
                .filter_map(|item| item["label"].as_str())
                .collect();
            panic!("no completion item `{label}`; offered: {labels:?}")
        })
}

#[then(expr = "the completion list contains {string}")]
fn completion_list_contains(world: &mut LspWorld, label: String) {
    let _ = completion_item(world, &label);
}

#[then(expr = "the completion list does not contain {string}")]
fn completion_list_does_not_contain(world: &mut LspWorld, label: String) {
    let items = world.reply_array();
    assert!(
        !items
            .iter()
            .any(|item| item["label"].as_str() == Some(&label)),
        "completion list unexpectedly contains `{label}`"
    );
}

#[then(expr = "every completion item is a method")]
fn every_completion_item_is_a_method(world: &mut LspWorld) {
    let items = world.reply_array();
    assert!(!items.is_empty(), "expected at least one completion item");
    // `CompletionItemKind::METHOD` is 2 in the protocol's numbering.
    assert!(
        items.iter().all(|item| item["kind"].as_u64() == Some(2)),
        "expected only methods: {items:?}"
    );
}

#[then(expr = "completion item {string} has detail {string}")]
fn completion_item_has_detail(world: &mut LspWorld, label: String, detail: String) {
    let item = completion_item(world, &label);
    assert_eq!(item["detail"].as_str(), Some(detail.as_str()), "{item}");
}

/// The single `additionalTextEdits` insert an auto-require item carries.
fn import_edit<'a>(world: &'a LspWorld, label: &str) -> &'a Value {
    let item = completion_item(world, label);
    let edits = item["additionalTextEdits"]
        .as_array()
        .unwrap_or_else(|| panic!("`{label}` carries no additionalTextEdits: {item}"));
    assert_eq!(edits.len(), 1, "expected one import insert: {edits:?}");
    &edits[0]
}

#[then(expr = "completion item {string} imports it from module {string}")]
fn completion_item_imports_from(world: &mut LspWorld, label: String, module: String) {
    let expected = format!("local {label} = require(\"{module}\").{label}\n");
    let edit = import_edit(world, &label);
    assert_eq!(edit["newText"].as_str(), Some(expected.as_str()), "{edit}");
}

#[then(expr = "the import for {string} is inserted at {int}:{int}")]
fn import_inserted_at(world: &mut LspWorld, label: String, line: u32, character: u32) {
    let edit = import_edit(world, &label);
    let position = json!({ "line": line, "character": character });
    assert_eq!(edit["range"]["start"], position, "{edit}");
    assert_eq!(edit["range"]["end"], position, "{edit}");
}

#[then(expr = "completion item {string} inserts nothing extra")]
fn completion_item_inserts_nothing(world: &mut LspWorld, label: String) {
    let item = completion_item(world, &label);
    assert_eq!(
        item["additionalTextEdits"],
        Value::Null,
        "`{label}` must not carry an import insert: {item}"
    );
}

// === Navigation ==========================================================

#[when(expr = "I request the definition at {int}:{int} in {string}")]
fn request_definition(world: &mut LspWorld, line: u32, character: u32, path: String) {
    let params = world.doc_position(&path, line, character);
    world.request("textDocument/definition", params);
}

#[when(expr = "I request the type definition at {int}:{int} in {string}")]
fn request_type_definition(world: &mut LspWorld, line: u32, character: u32, path: String) {
    let params = world.doc_position(&path, line, character);
    world.request("textDocument/typeDefinition", params);
}

#[when(expr = "I request implementations at {int}:{int} in {string}")]
fn request_implementations(world: &mut LspWorld, line: u32, character: u32, path: String) {
    let params = world.doc_position(&path, line, character);
    world.request("textDocument/implementation", params);
}

#[then(expr = "the location is in {string}")]
fn location_is_in(world: &mut LspWorld, path: String) {
    let location = single_location(world.ok_reply());
    let uri = location["uri"].as_str().unwrap_or_default();
    let expected = world.uri(&path);
    assert_eq!(uri, expected, "location: {location}");
}

#[then(expr = "the location starts at {int}:{int}")]
fn location_starts_at(world: &mut LspWorld, line: u32, character: u32) {
    let location = single_location(world.ok_reply());
    assert_eq!(
        location["range"]["start"],
        json!({ "line": line, "character": character }),
        "location: {location}"
    );
}

#[then(expr = "the location starts on line {int}")]
fn location_starts_on_line(world: &mut LspWorld, line: u32) {
    let location = single_location(world.ok_reply());
    assert_eq!(
        location["range"]["start"]["line"].as_u64(),
        Some(u64::from(line)),
        "location: {location}"
    );
}

#[then(expr = "the reply lists {int} locations")]
fn reply_lists_locations(world: &mut LspWorld, count: usize) {
    let locations = world.reply_array();
    assert_eq!(locations.len(), count, "locations: {locations:?}");
}

#[then(expr = "{int} of the locations are in {string}")]
fn locations_in_file(world: &mut LspWorld, count: usize, path: String) {
    let uri = world.uri(&path);
    let found = world
        .reply_array()
        .iter()
        .filter(|location| location["uri"].as_str() == Some(uri.as_str()))
        .count();
    assert_eq!(
        found, count,
        "expected {count} location(s) in `{path}`: {}",
        world.reply
    );
}

// === References and highlights ==========================================

#[when(expr = "I request references at {int}:{int} in {string} including the declaration")]
fn request_references_with_declaration(
    world: &mut LspWorld,
    line: u32,
    character: u32,
    path: String,
) {
    let mut params = world.doc_position(&path, line, character);
    params["context"] = json!({ "includeDeclaration": true });
    world.request("textDocument/references", params);
}

#[when(expr = "I request references at {int}:{int} in {string} excluding the declaration")]
fn request_references_without_declaration(
    world: &mut LspWorld,
    line: u32,
    character: u32,
    path: String,
) {
    let mut params = world.doc_position(&path, line, character);
    params["context"] = json!({ "includeDeclaration": false });
    world.request("textDocument/references", params);
}

#[then(expr = "a location starts at {int}:{int}")]
fn a_location_starts_at(world: &mut LspWorld, line: u32, character: u32) {
    let start = json!({ "line": line, "character": character });
    assert!(
        world
            .reply_array()
            .iter()
            .any(|location| location["range"]["start"] == start),
        "no location starts at {line}:{character}: {}",
        world.reply
    );
}

#[then(expr = "no location starts at {int}:{int}")]
fn no_location_starts_at(world: &mut LspWorld, line: u32, character: u32) {
    let start = json!({ "line": line, "character": character });
    assert!(
        !world
            .reply_array()
            .iter()
            .any(|location| location["range"]["start"] == start),
        "a location unexpectedly starts at {line}:{character}: {}",
        world.reply
    );
}

#[when(expr = "I request document highlights at {int}:{int} in {string}")]
fn request_document_highlights(world: &mut LspWorld, line: u32, character: u32, path: String) {
    let params = world.doc_position(&path, line, character);
    world.request("textDocument/documentHighlight", params);
}

#[then(expr = "the reply lists {int} highlights")]
fn reply_lists_highlights(world: &mut LspWorld, count: usize) {
    let highlights = world.reply_array();
    assert_eq!(highlights.len(), count, "highlights: {highlights:?}");
}

/// `DocumentHighlightKind`: 2 is Read, 3 is Write.
fn assert_highlight_kind(world: &LspWorld, line: u32, kind: u64, name: &str) {
    let highlights = world.reply_array();
    let highlight = highlights
        .iter()
        .find(|h| h["range"]["start"]["line"].as_u64() == Some(u64::from(line)))
        .unwrap_or_else(|| panic!("no highlight on line {line}: {highlights:?}"));
    assert_eq!(
        highlight["kind"].as_u64(),
        Some(kind),
        "the highlight on line {line} is not a {name}: {highlight}"
    );
}

#[then(expr = "the highlight on line {int} is a read")]
fn highlight_is_a_read(world: &mut LspWorld, line: u32) {
    assert_highlight_kind(world, line, 2, "read");
}

#[then(expr = "the highlight on line {int} is a write")]
fn highlight_is_a_write(world: &mut LspWorld, line: u32) {
    assert_highlight_kind(world, line, 3, "write");
}

// === Rename ==============================================================

#[when(expr = "I rename the symbol at {int}:{int} in {string} to {string}")]
fn rename_symbol(world: &mut LspWorld, line: u32, character: u32, path: String, new_name: String) {
    let mut params = world.doc_position(&path, line, character);
    params["newName"] = json!(new_name);
    world.request("textDocument/rename", params);
}

#[when(expr = "I prepare a rename at {int}:{int} in {string}")]
fn prepare_rename(world: &mut LspWorld, line: u32, character: u32, path: String) {
    let params = world.doc_position(&path, line, character);
    world.request("textDocument/prepareRename", params);
}

/// The `changes` map of the last reply's `WorkspaceEdit`.
fn workspace_changes(world: &LspWorld) -> &serde_json::Map<String, Value> {
    world.ok_reply()["changes"]
        .as_object()
        .unwrap_or_else(|| panic!("expected a WorkspaceEdit with changes, got {}", world.reply))
}

#[then(expr = "the workspace edit spans {int} files")]
fn workspace_edit_spans_files(world: &mut LspWorld, count: usize) {
    let changes = workspace_changes(world);
    assert_eq!(changes.len(), count, "changes: {changes:?}");
}

#[then(expr = "the workspace edit makes {int} edits")]
fn workspace_edit_makes_edits(world: &mut LspWorld, count: usize) {
    let changes = workspace_changes(world);
    let total: usize = changes
        .values()
        .map(|edits| edits.as_array().map_or(0, Vec::len))
        .sum();
    assert_eq!(total, count, "changes: {changes:?}");
}

#[then(expr = "the workspace edit touches {string}")]
fn workspace_edit_touches(world: &mut LspWorld, path: String) {
    let uri = world.uri(&path);
    let changes = workspace_changes(world);
    assert!(
        changes.contains_key(&uri),
        "the workspace edit does not touch `{path}`: {changes:?}"
    );
}

#[then(expr = "every edit writes {string}")]
fn every_edit_writes(world: &mut LspWorld, text: String) {
    let changes = workspace_changes(world);
    for (uri, edits) in changes {
        for edit in edits.as_array().expect("a list of text edits") {
            assert_eq!(
                edit["newText"].as_str(),
                Some(text.as_str()),
                "unexpected replacement in {uri}: {edit}"
            );
        }
    }
}

#[then(expr = "the prepared range spans {int}:{int} to {int}:{int}")]
fn prepared_range_spans(
    world: &mut LspWorld,
    start_line: u32,
    start_col: u32,
    end_line: u32,
    end_col: u32,
) {
    assert_eq!(
        *world.ok_reply(),
        range(start_line, start_col, end_line, end_col),
        "prepare rename reply"
    );
}

// === Formatting ==========================================================

#[when(expr = "I format {string}")]
fn format_document(world: &mut LspWorld, path: String) {
    let params = json!({
        "textDocument": world.text_document(&path),
        "options": { "tabSize": 4, "insertSpaces": true },
    });
    world.request("textDocument/formatting", params);
}

#[when(expr = "I format lines {int} to {int} of {string}")]
fn format_range(world: &mut LspWorld, start_line: u32, end_line: u32, path: String) {
    let params = json!({
        "textDocument": world.text_document(&path),
        "range": range(start_line, 0, end_line, 0),
        "options": { "tabSize": 4, "insertSpaces": true },
    });
    world.request("textDocument/rangeFormatting", params);
}

#[then("the reply is a single text edit")]
fn reply_is_a_single_text_edit(world: &mut LspWorld) {
    let edits = world.reply_array();
    assert_eq!(edits.len(), 1, "expected one edit: {edits:?}");
}

#[then("the text edit produces:")]
fn text_edit_produces(world: &mut LspWorld, step: &Step) {
    let edits = world.reply_array();
    assert_eq!(edits.len(), 1, "expected one edit: {edits:?}");
    assert_eq!(
        edits[0]["newText"].as_str().unwrap_or_default(),
        docstring(step),
        "formatted text"
    );
}

// === Symbols =============================================================

#[when(expr = "I request document symbols for {string}")]
fn request_document_symbols(world: &mut LspWorld, path: String) {
    let params = json!({ "textDocument": world.text_document(&path) });
    world.request("textDocument/documentSymbol", params);
}

#[when(expr = "I search the workspace for symbols matching {string}")]
fn search_workspace_symbols(world: &mut LspWorld, query: String) {
    world.request("workspace/symbol", json!({ "query": query }));
}

/// The top-level symbol named `name` in the last reply.
fn symbol_named<'a>(world: &'a LspWorld, name: &str) -> &'a Value {
    let symbols = world.reply_array();
    symbols
        .iter()
        .find(|symbol| symbol["name"].as_str() == Some(name))
        .unwrap_or_else(|| {
            let names: Vec<&str> = symbols
                .iter()
                .filter_map(|symbol| symbol["name"].as_str())
                .collect();
            panic!("no symbol `{name}`; found: {names:?}")
        })
}

#[then(expr = "the symbols include {string}")]
fn symbols_include(world: &mut LspWorld, name: String) {
    let _ = symbol_named(world, &name);
}

#[then(expr = "the symbols do not include {string}")]
fn symbols_do_not_include(world: &mut LspWorld, name: String) {
    assert!(
        !world
            .reply_array()
            .iter()
            .any(|symbol| symbol["name"].as_str() == Some(&name)),
        "symbols unexpectedly include `{name}`: {}",
        world.reply
    );
}

/// `SymbolKind` numbering: class 5, function 12, variable 13, field 8.
fn symbol_kind_number(kind: &str) -> u64 {
    match kind {
        "class" => 5,
        "field" => 8,
        "function" => 12,
        "variable" => 13,
        other => panic!("unknown symbol kind `{other}`"),
    }
}

#[then(expr = "symbol {string} is a {word}")]
fn symbol_is_a(world: &mut LspWorld, name: String, kind: String) {
    let symbol = symbol_named(world, &name);
    assert_eq!(
        symbol["kind"].as_u64(),
        Some(symbol_kind_number(&kind)),
        "{symbol}"
    );
}

#[then(expr = "symbol {string} has child {string}")]
fn symbol_has_child(world: &mut LspWorld, name: String, child: String) {
    let symbol = symbol_named(world, &name);
    let children = symbol["children"]
        .as_array()
        .unwrap_or_else(|| panic!("`{name}` has no children: {symbol}"));
    assert!(
        children.iter().any(|c| c["name"].as_str() == Some(&child)),
        "`{name}` has no child `{child}`: {children:?}"
    );
}

#[then(expr = "symbol {string} is declared in {string}")]
fn symbol_is_declared_in(world: &mut LspWorld, name: String, path: String) {
    let expected = world.uri(&path);
    let symbol = symbol_named(world, &name);
    assert_eq!(
        symbol["location"]["uri"].as_str(),
        Some(expected.as_str()),
        "{symbol}"
    );
}

// === Signature help ======================================================

#[when(expr = "I request signature help at {int}:{int} in {string}")]
fn request_signature_help(world: &mut LspWorld, line: u32, character: u32, path: String) {
    let params = world.doc_position(&path, line, character);
    world.request("textDocument/signatureHelp", params);
}

#[then(expr = "the signature labels are {string}")]
fn signature_labels_are(world: &mut LspWorld, expected: String) {
    let labels: Vec<&str> = world.ok_reply()["signatures"]
        .as_array()
        .unwrap_or_else(|| panic!("expected signatures, got {}", world.reply))
        .iter()
        .filter_map(|signature| signature["label"].as_str())
        .collect();
    let expected: Vec<&str> = expected.split(" | ").collect();
    assert_eq!(labels, expected);
}

#[then(expr = "the active parameter is {int}")]
fn active_parameter_is(world: &mut LspWorld, index: u64) {
    assert_eq!(
        world.ok_reply()["activeParameter"].as_u64(),
        Some(index),
        "signature help: {}",
        world.reply
    );
}

#[then(expr = "parameter {int} documents {string}")]
fn parameter_documents(world: &mut LspWorld, index: usize, needle: String) {
    let parameters = world.ok_reply()["signatures"][0]["parameters"]
        .as_array()
        .unwrap_or_else(|| panic!("expected parameters, got {}", world.reply));
    let documentation = parameters
        .get(index)
        .unwrap_or_else(|| panic!("no parameter {index}: {parameters:?}"))["documentation"]["value"]
        .as_str()
        .unwrap_or_default();
    assert!(
        documentation.contains(&needle),
        "parameter {index} documentation: {documentation}"
    );
}

// === Code actions ========================================================

#[when(expr = "I request code actions on line {int} of {string}")]
fn request_code_actions(world: &mut LspWorld, line: u32, path: String) {
    let params = json!({
        "textDocument": world.text_document(&path),
        // The whole line: a generous window that still proves the overlap
        // filter runs (a caret anywhere on the finding offers its fix).
        "range": range(line, 0, line, 200),
        "context": { "diagnostics": [] },
    });
    world.request("textDocument/codeAction", params);
}

/// The single quick-fix action in the last reply.
fn quickfix(world: &LspWorld) -> &Value {
    let actions = world.reply_array();
    actions
        .iter()
        .find(|action| action["kind"].as_str() == Some("quickfix"))
        .unwrap_or_else(|| panic!("no quickfix among the offered actions: {actions:?}"))
}

#[then("a quickfix is offered")]
fn a_quickfix_is_offered(world: &mut LspWorld) {
    let _ = quickfix(world);
}

#[then("no quickfix is offered")]
fn no_quickfix_is_offered(world: &mut LspWorld) {
    let actions = world.reply_array();
    assert!(
        !actions
            .iter()
            .any(|action| action["kind"].as_str() == Some("quickfix")),
        "a quickfix was unexpectedly offered: {actions:?}"
    );
}

#[then(expr = "the quickfix resolves {word}")]
fn quickfix_resolves(world: &mut LspWorld, code: String) {
    let action = quickfix(world);
    let diagnostics = action["diagnostics"]
        .as_array()
        .unwrap_or_else(|| panic!("the quickfix references no diagnostic: {action}"));
    assert!(
        diagnostics.iter().any(|d| code_of(d) == code),
        "the quickfix does not resolve `{code}`: {diagnostics:?}"
    );
}

#[then(expr = "the quickfix writes {string} at {int}:{int} to {int}:{int}")]
fn quickfix_writes(
    world: &mut LspWorld,
    text: String,
    start_line: u32,
    start_col: u32,
    end_line: u32,
    end_col: u32,
) {
    let action = quickfix(world);
    let changes = action["edit"]["changes"]
        .as_object()
        .unwrap_or_else(|| panic!("the quickfix carries no edit: {action}"));
    let edits: Vec<&Value> = changes
        .values()
        .filter_map(Value::as_array)
        .flatten()
        .collect();
    assert_eq!(edits.len(), 1, "expected one edit: {edits:?}");
    assert_eq!(edits[0]["newText"].as_str(), Some(text.as_str()));
    assert_eq!(
        edits[0]["range"],
        range(start_line, start_col, end_line, end_col)
    );
}

// === Call hierarchy ======================================================

#[given(expr = "I prepare the call hierarchy at {int}:{int} in {string}")]
#[when(expr = "I prepare the call hierarchy at {int}:{int} in {string}")]
fn prepare_call_hierarchy(world: &mut LspWorld, line: u32, character: u32, path: String) {
    let params = world.doc_position(&path, line, character);
    world.request("textDocument/prepareCallHierarchy", params);
    let items = world.reply_array();
    world.item = items.first().cloned();
}

#[then(expr = "the prepared item is named {string}")]
fn prepared_item_is_named(world: &mut LspWorld, name: String) {
    let item = world
        .item
        .as_ref()
        .unwrap_or_else(|| panic!("no call-hierarchy item was prepared: {}", world.reply));
    assert_eq!(item["name"].as_str(), Some(name.as_str()), "{item}");
}

#[then(expr = "the prepared item selects {int}:{int} to {int}:{int}")]
fn prepared_item_selects(
    world: &mut LspWorld,
    start_line: u32,
    start_col: u32,
    end_line: u32,
    end_col: u32,
) {
    let item = world
        .item
        .as_ref()
        .unwrap_or_else(|| panic!("no call-hierarchy item was prepared: {}", world.reply));
    assert_eq!(
        item["selectionRange"],
        range(start_line, start_col, end_line, end_col),
        "{item}"
    );
}

#[when("I request incoming calls for the prepared item")]
fn request_incoming_calls(world: &mut LspWorld) {
    let item = world
        .item
        .clone()
        .expect("no call-hierarchy item was prepared");
    world.request("callHierarchy/incomingCalls", json!({ "item": item }));
}

#[when("I request outgoing calls for the prepared item")]
fn request_outgoing_calls(world: &mut LspWorld) {
    let item = world
        .item
        .clone()
        .expect("no call-hierarchy item was prepared");
    world.request("callHierarchy/outgoingCalls", json!({ "item": item }));
}

#[then(expr = "an incoming call comes from {string} in {string}")]
fn incoming_call_from(world: &mut LspWorld, name: String, path: String) {
    let uri = world.uri(&path);
    let calls = world.reply_array();
    let call = calls
        .iter()
        .find(|call| call["from"]["name"].as_str() == Some(&name))
        .unwrap_or_else(|| panic!("no incoming call from `{name}`: {calls:?}"));
    assert_eq!(call["from"]["uri"].as_str(), Some(uri.as_str()), "{call}");
}

#[then(expr = "the incoming call from {string} lists {int} call sites")]
fn incoming_call_sites(world: &mut LspWorld, name: String, count: usize) {
    let calls = world.reply_array();
    let call = calls
        .iter()
        .find(|call| call["from"]["name"].as_str() == Some(&name))
        .unwrap_or_else(|| panic!("no incoming call from `{name}`: {calls:?}"));
    let ranges = call["fromRanges"].as_array().expect("fromRanges");
    assert_eq!(ranges.len(), count, "{call}");
}

#[then(expr = "the outgoing calls are {string}")]
fn outgoing_calls_are(world: &mut LspWorld, expected: String) {
    let names: Vec<&str> = world
        .reply_array()
        .iter()
        .filter_map(|call| call["to"]["name"].as_str())
        .collect();
    let expected: Vec<&str> = expected.split(", ").collect();
    assert_eq!(names, expected);
}

#[then(expr = "the outgoing call to {string} lists {int} call sites")]
fn outgoing_call_sites(world: &mut LspWorld, name: String, count: usize) {
    let calls = world.reply_array();
    let call = calls
        .iter()
        .find(|call| call["to"]["name"].as_str() == Some(&name))
        .unwrap_or_else(|| panic!("no outgoing call to `{name}`: {calls:?}"));
    let ranges = call["fromRanges"].as_array().expect("fromRanges");
    assert_eq!(ranges.len(), count, "{call}");
}

// === Folding and selection ranges =======================================

#[when(expr = "I request folding ranges for {string}")]
fn request_folding_ranges(world: &mut LspWorld, path: String) {
    let params = json!({ "textDocument": world.text_document(&path) });
    world.request("textDocument/foldingRange", params);
}

#[then(expr = "a folding range covers lines {int} to {int}")]
fn folding_range_covers(world: &mut LspWorld, start: u64, end: u64) {
    let ranges = world.reply_array();
    assert!(
        ranges
            .iter()
            .any(|r| r["startLine"].as_u64() == Some(start) && r["endLine"].as_u64() == Some(end)),
        "no fold covering lines {start}..{end}: {ranges:?}"
    );
}

#[then(expr = "a {word} folding range covers lines {int} to {int}")]
fn kinded_folding_range_covers(world: &mut LspWorld, kind: String, start: u64, end: u64) {
    let ranges = world.reply_array();
    assert!(
        ranges.iter().any(|r| r["kind"].as_str() == Some(&kind)
            && r["startLine"].as_u64() == Some(start)
            && r["endLine"].as_u64() == Some(end)),
        "no `{kind}` fold covering lines {start}..{end}: {ranges:?}"
    );
}

#[when(expr = "I request selection ranges at {int}:{int} in {string}")]
fn request_selection_ranges(world: &mut LspWorld, line: u32, character: u32, path: String) {
    let params = json!({
        "textDocument": world.text_document(&path),
        "positions": [{ "line": line, "character": character }],
    });
    world.request("textDocument/selectionRange", params);
}

/// The expand chain of the first requested position, outermost last.
fn selection_chain(world: &LspWorld) -> Vec<Value> {
    let mut chain = Vec::new();
    let mut current = &world.reply_array()[0];
    loop {
        chain.push(current["range"].clone());
        if current["parent"].is_object() {
            current = &current["parent"];
        } else {
            return chain;
        }
    }
}

#[then(expr = "the selection chain has at least {int} ranges")]
fn selection_chain_length(world: &mut LspWorld, count: usize) {
    let chain = selection_chain(world);
    assert!(
        chain.len() >= count,
        "expected at least {count} ranges, got {}: {chain:?}",
        chain.len()
    );
}

#[then(expr = "the innermost selection range spans {int}:{int} to {int}:{int}")]
fn innermost_selection_range(
    world: &mut LspWorld,
    start_line: u32,
    start_col: u32,
    end_line: u32,
    end_col: u32,
) {
    let chain = selection_chain(world);
    assert_eq!(chain[0], range(start_line, start_col, end_line, end_col));
}

#[then(expr = "the outermost selection range spans {int}:{int} to {int}:{int}")]
fn outermost_selection_range(
    world: &mut LspWorld,
    start_line: u32,
    start_col: u32,
    end_line: u32,
    end_col: u32,
) {
    let chain = selection_chain(world);
    assert_eq!(
        chain[chain.len() - 1],
        range(start_line, start_col, end_line, end_col)
    );
}

// === Semantic tokens =====================================================

#[when(expr = "I request semantic tokens for {string}")]
fn request_semantic_tokens(world: &mut LspWorld, path: String) {
    let params = json!({ "textDocument": world.text_document(&path) });
    world.request("textDocument/semanticTokens/full", params);
}

#[then("the token stream is well formed")]
fn token_stream_is_well_formed(world: &mut LspWorld) {
    let tokens = decode_tokens(world);
    assert!(!tokens.is_empty(), "expected at least one semantic token");
}

/// The decoded token starting exactly at `line:character`.
fn token_at(tokens: &[DecodedToken], line: u32, character: u32) -> &DecodedToken {
    tokens
        .iter()
        .find(|token| token.line == line && token.start == character)
        .unwrap_or_else(|| panic!("no semantic token at {line}:{character}"))
}

#[then(expr = "the token at {int}:{int} is a {word}")]
fn token_is_a(world: &mut LspWorld, line: u32, character: u32, kind: String) {
    let tokens = decode_tokens(world);
    let token = token_at(&tokens, line, character);
    assert_eq!(token.token_type, kind);
}

#[then(expr = "the token at {int}:{int} is marked {string}")]
fn token_is_marked(world: &mut LspWorld, line: u32, character: u32, modifier: String) {
    let tokens = decode_tokens(world);
    let token = token_at(&tokens, line, character);
    assert!(
        token.modifiers.contains(&modifier),
        "the token at {line}:{character} is not marked `{modifier}`: {:?}",
        token.modifiers
    );
}

#[then(expr = "the token at {int}:{int} is not marked {string}")]
fn token_is_not_marked(world: &mut LspWorld, line: u32, character: u32, modifier: String) {
    let tokens = decode_tokens(world);
    let token = token_at(&tokens, line, character);
    assert!(
        !token.modifiers.contains(&modifier),
        "the token at {line}:{character} is unexpectedly marked `{modifier}`"
    );
}

// === Inlay hints =========================================================

#[when(expr = "I request inlay hints for the first {int} lines of {string}")]
fn request_inlay_hints(world: &mut LspWorld, lines: u32, path: String) {
    let params = json!({
        "textDocument": world.text_document(&path),
        "range": range(0, 0, lines, 0),
    });
    world.request("textDocument/inlayHint", params);
}

#[then(expr = "an inlay hint at {int}:{int} reads {string}")]
fn inlay_hint_reads(world: &mut LspWorld, line: u32, character: u32, label: String) {
    let hints = world.reply_array();
    let hint = hints
        .iter()
        .find(|hint| hint["position"] == json!({ "line": line, "character": character }))
        .unwrap_or_else(|| panic!("no inlay hint at {line}:{character}: {hints:?}"));
    assert_eq!(hint["label"].as_str(), Some(label.as_str()), "{hint}");
}

#[then(expr = "no inlay hint sits at {int}:{int}")]
fn no_inlay_hint_at(world: &mut LspWorld, line: u32, character: u32) {
    let hints = world.reply_array();
    assert!(
        !hints
            .iter()
            .any(|hint| hint["position"] == json!({ "line": line, "character": character })),
        "an inlay hint unexpectedly sits at {line}:{character}: {hints:?}"
    );
}

#[tokio::main]
async fn main() {
    // Mirrors `acceptance.rs`: `@wip` gates feature files written ahead of
    // the behaviour they describe (SPEC.md §16.2).
    // `fail_on_skipped`: an undefined or ambiguous step is a *failure*,
    // not a quiet skip. Without it a feature file could describe behaviour
    // no step definition implements and the suite would still go green.
    LspWorld::cucumber()
        .fail_on_skipped()
        .filter_run_and_exit("tests/features/lsp", |feature, _rule, scenario| {
            let tagged = |tag: &str| {
                feature.tags.iter().any(|t| t == tag) || scenario.tags.iter().any(|t| t == tag)
            };
            !tagged("wip")
        })
        .await;
}

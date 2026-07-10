//! End-to-end tests: a real client driving the server in-process over
//! [`lsp_server::Connection::memory`] — full initialize handshake, document
//! lifecycle, published diagnostics, and every tranche-1 request.

use std::path::PathBuf;
use std::time::Duration;

use lsp_server::{Connection, Message, Notification, Request, RequestId};
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, Exit, Initialized,
    Notification as _, PublishDiagnostics,
};
use lsp_types::request::{
    Completion, DocumentSymbolRequest, GotoDefinition, HoverRequest, Request as _, Shutdown,
};
use lsp_types::{
    CompletionItemKind, CompletionParams, CompletionResponse, DiagnosticSeverity,
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DocumentSymbolParams, DocumentSymbolResponse, GotoDefinitionParams, GotoDefinitionResponse,
    HoverContents, HoverParams, InitializeParams, NumberOrString, PartialResultParams, Position,
    PublishDiagnosticsParams, Range, SymbolKind, TextDocumentContentChangeEvent,
    TextDocumentIdentifier, TextDocumentItem, TextDocumentPositionParams, Uri,
    VersionedTextDocumentIdentifier, WorkDoneProgressParams, WorkspaceFolder,
};
use tempfile::TempDir;

// === Harness =============================================================

struct TestClient {
    conn: Connection,
    server_thread: std::thread::JoinHandle<anyhow::Result<()>>,
    _dir: TempDir,
    root: PathBuf,
    next_id: i32,
    init_result: serde_json::Value,
}

/// Create a project on disk, start the server on an in-memory connection,
/// and complete the initialize handshake.
fn start(files: &[(&str, &str)]) -> TestClient {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path().canonicalize().expect("canonicalize");
    for (rel, text) in files {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(path, text).expect("write");
    }

    let (server_conn, client_conn) = Connection::memory();
    let server = std::thread::spawn(move || luabox_lsp::run(server_conn));

    let root_uri = luabox_lsp::path_to_uri(&root);
    #[allow(deprecated, reason = "InitializeParams carries deprecated fields")]
    let params = InitializeParams {
        workspace_folders: Some(vec![WorkspaceFolder {
            uri: root_uri,
            name: "test".to_string(),
        }]),
        ..InitializeParams::default()
    };
    let mut client = TestClient {
        conn: client_conn,
        server_thread: server,
        _dir: dir,
        root,
        next_id: 0,
        init_result: serde_json::Value::Null,
    };
    let id = client.send_request_raw("initialize", serde_json::to_value(params).unwrap());
    client.init_result = client.wait_response(&id);
    client.notify::<Initialized>(lsp_types::InitializedParams {});
    client
}

impl TestClient {
    fn uri(&self, rel: &str) -> Uri {
        luabox_lsp::path_to_uri(&self.root.join(rel))
    }

    fn send_request_raw(&mut self, method: &str, params: serde_json::Value) -> RequestId {
        let id = RequestId::from(self.next_id);
        self.next_id += 1;
        self.conn
            .sender
            .send(Message::Request(Request::new(
                id.clone(),
                method.to_string(),
                params,
            )))
            .expect("send");
        id
    }

    fn recv(&self) -> Message {
        self.conn
            .receiver
            .recv_timeout(Duration::from_secs(30))
            .expect("server timed out")
    }

    /// Skip interleaved notifications until the response for `id` arrives.
    fn wait_response(&self, id: &RequestId) -> serde_json::Value {
        loop {
            if let Message::Response(resp) = self.recv()
                && resp.id == *id
            {
                assert!(resp.error.is_none(), "server error: {:?}", resp.error);
                return resp.result.unwrap_or(serde_json::Value::Null);
            }
        }
    }

    fn request<R: lsp_types::request::Request>(&mut self, params: R::Params) -> R::Result {
        let id = self.send_request_raw(R::METHOD, serde_json::to_value(params).unwrap());
        serde_json::from_value(self.wait_response(&id)).expect("decode response")
    }

    fn notify<N: lsp_types::notification::Notification>(&self, params: N::Params) {
        self.conn
            .sender
            .send(Message::Notification(Notification::new(
                N::METHOD.to_string(),
                params,
            )))
            .expect("send");
    }

    /// The next publishDiagnostics for `uri`.
    fn wait_diagnostics(&self, uri: &Uri) -> Vec<lsp_types::Diagnostic> {
        loop {
            if let Message::Notification(not) = self.recv()
                && not.method == PublishDiagnostics::METHOD
            {
                let params: PublishDiagnosticsParams =
                    serde_json::from_value(not.params).expect("decode diagnostics");
                if params.uri.as_str() == uri.as_str() {
                    return params.diagnostics;
                }
            }
        }
    }

    fn open(&self, uri: &Uri, text: &str) -> Vec<lsp_types::Diagnostic> {
        self.notify::<DidOpenTextDocument>(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: uri.clone(),
                language_id: "lua".to_string(),
                version: 1,
                text: text.to_string(),
            },
        });
        self.wait_diagnostics(uri)
    }

    fn change(&self, uri: &Uri, text: &str) -> Vec<lsp_types::Diagnostic> {
        self.notify::<DidChangeTextDocument>(DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier {
                uri: uri.clone(),
                version: 2,
            },
            content_changes: vec![TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: text.to_string(),
            }],
        });
        self.wait_diagnostics(uri)
    }

    fn close(&self, uri: &Uri) -> Vec<lsp_types::Diagnostic> {
        self.notify::<DidCloseTextDocument>(DidCloseTextDocumentParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
        });
        self.wait_diagnostics(uri)
    }

    fn hover(&mut self, uri: &Uri, line: u32, character: u32) -> Option<lsp_types::Hover> {
        self.request::<HoverRequest>(HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                position: Position { line, character },
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
    }

    fn definition(&mut self, uri: &Uri, line: u32, character: u32) -> Option<lsp_types::Location> {
        let response = self.request::<GotoDefinition>(GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                position: Position { line, character },
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })?;
        match response {
            GotoDefinitionResponse::Scalar(location) => Some(location),
            other => panic!("expected a scalar location, got {other:?}"),
        }
    }

    fn complete(&mut self, uri: &Uri, line: u32, character: u32) -> Vec<lsp_types::CompletionItem> {
        let response = self.request::<Completion>(CompletionParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                position: Position { line, character },
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            context: None,
        });
        match response {
            Some(CompletionResponse::Array(items)) => items,
            Some(other) => panic!("expected an item array, got {other:?}"),
            None => Vec::new(),
        }
    }

    fn symbols(&mut self, uri: &Uri) -> Vec<lsp_types::DocumentSymbol> {
        let response = self.request::<DocumentSymbolRequest>(DocumentSymbolParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        });
        match response {
            Some(DocumentSymbolResponse::Nested(symbols)) => symbols,
            other => panic!("expected nested symbols, got {other:?}"),
        }
    }

    fn shutdown(mut self) {
        let id = self.send_request_raw(Shutdown::METHOD, serde_json::Value::Null);
        let _ = self.wait_response(&id);
        self.notify::<Exit>(());
        self.server_thread
            .join()
            .expect("server thread panicked")
            .expect("server errored");
    }
}

fn range(start: (u32, u32), end: (u32, u32)) -> Range {
    Range {
        start: Position::new(start.0, start.1),
        end: Position::new(end.0, end.1),
    }
}

fn hover_text(hover: &lsp_types::Hover) -> &str {
    match &hover.contents {
        HoverContents::Markup(markup) => &markup.value,
        other => panic!("expected markup hover contents, got {other:?}"),
    }
}

fn code_of(diag: &lsp_types::Diagnostic) -> &str {
    match &diag.code {
        Some(NumberOrString::String(code)) => code,
        other => panic!("expected a string code, got {other:?}"),
    }
}

/// A call whose argument violates the `---@param` annotation (LB0300).
const TYPE_ERROR: &str = "\
---@param n number
local function f(n) end
f(\"no\")
";

const TYPE_OK: &str = "\
---@param n number
local function f(n) end
f(1)
";

// === Handshake ===========================================================

#[test]
fn initialize_advertises_tranche_one_capabilities() {
    let client = start(&[]);
    let caps = &client.init_result["capabilities"];
    // Full sync (documented tranche-1 choice), hover, definition,
    // documentSymbol, and completion triggered by `.` / `:`.
    assert_eq!(caps["textDocumentSync"], 1);
    assert_eq!(caps["hoverProvider"], true);
    assert_eq!(caps["definitionProvider"], true);
    assert_eq!(caps["documentSymbolProvider"], true);
    assert_eq!(
        caps["completionProvider"]["triggerCharacters"],
        serde_json::json!([".", ":"])
    );
    assert_eq!(client.init_result["serverInfo"]["name"], "luabox-lsp");
    client.shutdown();
}

// === Diagnostics =========================================================

#[test]
fn open_with_type_error_publishes_diagnostic_with_range() {
    let client = start(&[]);
    let uri = client.uri("main.lua");
    let diags = client.open(&uri, TYPE_ERROR);
    assert_eq!(diags.len(), 1, "{diags:?}");
    let diag = &diags[0];
    assert_eq!(code_of(diag), "LB0300");
    // The argument `"no"` on line 2, columns 2..6.
    assert_eq!(diag.range, range((2, 2), (2, 6)));
    assert_eq!(diag.source.as_deref(), Some("luabox"));
    client.shutdown();
}

#[test]
fn edit_that_fixes_the_error_clears_diagnostics() {
    let client = start(&[]);
    let uri = client.uri("main.lua");
    assert_eq!(client.open(&uri, TYPE_ERROR).len(), 1);
    assert!(client.change(&uri, TYPE_OK).is_empty());
    client.shutdown();
}

#[test]
fn syntax_errors_are_published() {
    let client = start(&[]);
    let uri = client.uri("broken.lua");
    let diags = client.open(&uri, "local = 1\n");
    assert!(!diags.is_empty());
    assert!(diags.iter().any(|d| code_of(d) == "LB0001"), "{diags:?}");
    assert_eq!(diags[0].severity, Some(DiagnosticSeverity::ERROR));
    client.shutdown();
}

#[test]
fn diagnostic_columns_are_utf16() {
    let client = start(&[]);
    let uri = client.uri("main.lua");
    // The emoji is 4 bytes but 2 UTF-16 units, shifting the error columns.
    let source = "---@param n number\nlocal function f(n) end\n--[[\u{1F600}]]f(\"no\")\n";
    let diags = client.open(&uri, source);
    assert_eq!(diags.len(), 1, "{diags:?}");
    // Byte offset of `"no"` on its line is 12; UTF-16 column is 10.
    assert_eq!(diags[0].range, range((2, 10), (2, 14)));
    client.shutdown();
}

#[test]
fn manifest_strictness_controls_severity() {
    // Without a manifest the default is warn; `strict = true` makes errors.
    let manifest = "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"5.4\"\n\n[types]\nstrict = true\n";
    let client = start(&[("luabox.toml", manifest)]);
    let uri = client.uri("main.lua");
    let diags = client.open(&uri, TYPE_ERROR);
    assert_eq!(diags[0].severity, Some(DiagnosticSeverity::ERROR));
    client.shutdown();

    let client = start(&[]);
    let uri = client.uri("main.lua");
    let diags = client.open(&uri, TYPE_ERROR);
    assert_eq!(diags[0].severity, Some(DiagnosticSeverity::WARNING));
    client.shutdown();
}

#[test]
fn close_reverts_to_disk_content() {
    // Disk is clean; the editor buffer introduces an error; closing reverts.
    let client = start(&[("main.lua", TYPE_OK)]);
    let uri = client.uri("main.lua");
    assert_eq!(client.open(&uri, TYPE_ERROR).len(), 1);
    assert!(client.close(&uri).is_empty());
    client.shutdown();
}

// === Hover ===============================================================

#[test]
fn hover_on_annotated_local_shows_its_type() {
    let client = start(&[]);
    let uri = client.uri("main.lua");
    let source = "---the answer\n---@type number\nlocal answer = 42\nprint(answer)\n";
    client.open(&uri, source);
    let mut client = client;
    // Hover over `answer` inside `print(answer)`.
    let hover = client.hover(&uri, 3, 8).expect("hover");
    let text = hover_text(&hover);
    assert!(text.contains("```lua"), "{text}");
    assert!(text.contains("local answer: number"), "{text}");
    assert!(text.contains("the answer"), "{text}");
    client.shutdown();
}

#[test]
fn hover_on_function_shows_signature_and_docs() {
    let client = start(&[]);
    let uri = client.uri("main.lua");
    let source = "\
---Stringify a number.
---@param n number
---@return string
local function stringify(n) return tostring(n) end
stringify(1)
";
    client.open(&uri, source);
    let mut client = client;
    // Hover over the call site.
    let hover = client.hover(&uri, 4, 3).expect("hover");
    let text = hover_text(&hover);
    assert!(
        text.contains("function stringify(n: number): string"),
        "{text}"
    );
    assert!(text.contains("Stringify a number."), "{text}");
    client.shutdown();
}

#[test]
fn hover_on_class_field_shows_field_type() {
    let client = start(&[]);
    let uri = client.uri("main.lua");
    let source = "\
---@class Point
---@field x number horizontal
---@field y number

---@type Point
local p = nil
print(p.x)
";
    client.open(&uri, source);
    let mut client = client;
    // Hover over `x` in `p.x`.
    let hover = client.hover(&uri, 6, 8).expect("hover");
    let text = hover_text(&hover);
    assert!(text.contains("Point.x: number"), "{text}");
    assert!(text.contains("horizontal"), "{text}");
    client.shutdown();
}

// === Goto definition =====================================================

#[test]
fn goto_definition_on_local_use_returns_decl_range() {
    let client = start(&[]);
    let uri = client.uri("main.lua");
    client.open(&uri, "local value = 1\nprint(value)\n");
    let mut client = client;
    // `value` inside `print(...)` → the declaration on line 0, cols 6..11.
    let location = client.definition(&uri, 1, 8).expect("definition");
    assert_eq!(location.uri.as_str(), uri.as_str());
    assert_eq!(location.range, range((0, 6), (0, 11)));
    client.shutdown();
}

#[test]
fn goto_definition_resolves_require_to_module_file() {
    let client = start(&[
        ("util/helpers.lua", "return {}\n"),
        ("main.lua", "local h = require(\"util.helpers\")\n"),
    ]);
    let uri = client.uri("main.lua");
    client.open(&uri, "local h = require(\"util.helpers\")\n");
    let mut client = client;
    // Anywhere inside the require call works; aim at the string.
    let location = client.definition(&uri, 0, 22).expect("definition");
    assert!(
        location.uri.as_str().ends_with("util/helpers.lua"),
        "{}",
        location.uri.as_str()
    );
    client.shutdown();
}

#[test]
fn goto_definition_on_class_field_jumps_to_field_annotation() {
    let client = start(&[]);
    let uri = client.uri("main.lua");
    let source = "\
---@class Point
---@field x number

---@type Point
local p = nil
print(p.x)
";
    client.open(&uri, source);
    let mut client = client;
    let location = client.definition(&uri, 5, 8).expect("definition");
    // The `@field x number` tag is on line 1.
    assert_eq!(location.range.start.line, 1);
    client.shutdown();
}

// === Completion ==========================================================

#[test]
fn completion_after_dot_on_class_value_offers_fields() {
    let client = start(&[]);
    let uri = client.uri("main.lua");
    let source = "\
---@class Point
---@field x number
---@field y number
---@field translate fun(dx: number): Point

---@type Point
local p = nil
print(p.
";
    client.open(&uri, source);
    let mut client = client;
    // Cursor right after `p.` on line 7.
    let items = client.complete(&uri, 7, 8);
    let x = items.iter().find(|i| i.label == "x").expect("field x");
    assert_eq!(x.kind, Some(CompletionItemKind::FIELD));
    assert_eq!(x.detail.as_deref(), Some("Point.x: number"));
    assert!(items.iter().any(|i| i.label == "y"));
    let translate = items
        .iter()
        .find(|i| i.label == "translate")
        .expect("method");
    assert_eq!(translate.kind, Some(CompletionItemKind::FUNCTION));
    // Member completion never mixes in keywords.
    assert!(!items.iter().any(|i| i.label == "local"), "{items:?}");
    client.shutdown();
}

#[test]
fn completion_after_colon_offers_only_methods() {
    let client = start(&[]);
    let uri = client.uri("main.lua");
    let source = "\
---@class Point
---@field x number
---@field translate fun(dx: number): Point

---@type Point
local p = nil
p:
";
    client.open(&uri, source);
    let mut client = client;
    let items = client.complete(&uri, 6, 2);
    assert!(items.iter().any(|i| i.label == "translate"));
    assert!(
        items
            .iter()
            .all(|i| i.kind == Some(CompletionItemKind::METHOD)),
        "{items:?}"
    );
    assert!(!items.iter().any(|i| i.label == "x"), "{items:?}");
    client.shutdown();
}

#[test]
fn plain_completion_offers_locals_functions_and_keywords() {
    let client = start(&[]);
    let uri = client.uri("main.lua");
    let source = "local alpha = 1\nlocal function beta() end\nal\n";
    client.open(&uri, source);
    let mut client = client;
    let items = client.complete(&uri, 2, 2);
    let alpha = items.iter().find(|i| i.label == "alpha").expect("alpha");
    assert_eq!(alpha.kind, Some(CompletionItemKind::VARIABLE));
    let beta = items.iter().find(|i| i.label == "beta").expect("beta");
    assert_eq!(beta.kind, Some(CompletionItemKind::FUNCTION));
    assert!(items.iter().any(|i| i.label == "local"));
    // Sorted and deduplicated.
    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    let mut sorted = labels.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(labels, sorted);
    client.shutdown();
}

// === Document symbols ====================================================

#[test]
fn document_symbols_cover_functions_locals_and_classes() {
    let client = start(&[]);
    let uri = client.uri("main.lua");
    let source = "\
---@class Shape
---@field kind string

local top = 1

function M.helper() end

local function outer()
    local function inner() end
end
";
    client.open(&uri, source);
    let mut client = client;
    let symbols = client.symbols(&uri);
    let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"Shape"), "{names:?}");
    assert!(names.contains(&"top"), "{names:?}");
    assert!(names.contains(&"M.helper"), "{names:?}");
    assert!(names.contains(&"outer"), "{names:?}");

    let class = symbols.iter().find(|s| s.name == "Shape").unwrap();
    assert_eq!(class.kind, SymbolKind::CLASS);
    let fields = class.children.as_ref().expect("class fields");
    assert_eq!(fields[0].name, "kind");
    assert_eq!(fields[0].kind, SymbolKind::FIELD);

    let outer = symbols.iter().find(|s| s.name == "outer").unwrap();
    assert_eq!(outer.kind, SymbolKind::FUNCTION);
    let children = outer.children.as_ref().expect("nested function");
    assert_eq!(children[0].name, "inner");

    let top = symbols.iter().find(|s| s.name == "top").unwrap();
    assert_eq!(top.kind, SymbolKind::VARIABLE);
    // `inner` is nested, not top-level.
    assert!(!names.contains(&"inner"), "{names:?}");
    client.shutdown();
}

// === .lb shape files =====================================================

#[test]
fn lb_files_publish_shape_parse_errors() {
    let client = start(&[]);
    let uri = client.uri("shapes.lb");
    // A trait fn with a body: rejected at parse time with LB2010.
    let source = "trait Shape {\n    fn area(self) -> number { return 1 }\n}\n";
    let diags = client.open(&uri, source);
    assert!(
        diags.iter().any(|d| code_of(d) == "LB2010"),
        "expected LB2010, got {diags:?}"
    );
    // A clean edit clears them.
    let clean = "trait Shape {\n    fn area(self) -> number;\n}\n";
    assert!(client.change(&uri, clean).is_empty());
    client.shutdown();
}

#[test]
fn lb_goto_and_hover_resolve_struct_names() {
    let client = start(&[]);
    let uri = client.uri("shapes.lb");
    let source = "struct Point { x: number, y: number }\ntype Pair = Point;\n";
    client.open(&uri, source);
    let mut client = client;
    // `Point` on line 1 (the alias) → the struct declaration on line 0.
    let location = client.definition(&uri, 1, 14).expect("definition");
    assert_eq!(location.range, range((0, 7), (0, 12)));
    let hover = client.hover(&uri, 1, 14).expect("hover");
    assert!(
        hover_text(&hover).contains("struct Point"),
        "{}",
        hover_text(&hover)
    );
    client.shutdown();
}

// === Project bootstrap ===================================================

#[test]
fn bootstrapped_files_answer_requests_without_open() {
    // main.lua is on disk but never opened: hover still works because the
    // bootstrap walked the tree into the analysis host.
    let source = "---@type number\nlocal answer = 42\nprint(answer)\n";
    let client = start(&[("main.lua", source)]);
    let uri = client.uri("main.lua");
    let mut client = client;
    let hover = client.hover(&uri, 2, 8).expect("hover");
    assert!(
        hover_text(&hover).contains("local answer: number"),
        "{}",
        hover_text(&hover)
    );
    client.shutdown();
}

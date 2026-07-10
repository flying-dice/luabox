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
    Completion, DocumentSymbolRequest, Formatting, GotoDefinition, HoverRequest, InlayHintRequest,
    RangeFormatting, Request as _, SemanticTokensFullRequest, Shutdown,
};
use lsp_types::{
    CompletionItemKind, CompletionParams, CompletionResponse, DiagnosticSeverity,
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DocumentFormattingParams, DocumentRangeFormattingParams, DocumentSymbolParams,
    DocumentSymbolResponse, FormattingOptions, GotoDefinitionParams, GotoDefinitionResponse,
    HoverContents, HoverParams, InitializeParams, InlayHint, InlayHintLabel, InlayHintParams,
    NumberOrString, PartialResultParams, Position, PublishDiagnosticsParams, Range, SemanticToken,
    SemanticTokensParams, SemanticTokensResult, SymbolKind, TextDocumentContentChangeEvent,
    TextDocumentIdentifier, TextDocumentItem, TextDocumentPositionParams, TextEdit, Uri,
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

    fn formatting(&mut self, uri: &Uri) -> Option<Vec<TextEdit>> {
        self.request::<Formatting>(DocumentFormattingParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            options: FormattingOptions {
                tab_size: 4,
                insert_spaces: true,
                ..FormattingOptions::default()
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
    }

    fn range_formatting(&mut self, uri: &Uri, range: Range) -> Option<Vec<TextEdit>> {
        self.request::<RangeFormatting>(DocumentRangeFormattingParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            range,
            options: FormattingOptions {
                tab_size: 4,
                insert_spaces: true,
                ..FormattingOptions::default()
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
    }

    fn inlay_hints(&mut self, uri: &Uri, range: Range) -> Vec<InlayHint> {
        self.request::<InlayHintRequest>(InlayHintParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            range,
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .unwrap_or_default()
    }

    fn semantic_tokens(&mut self, uri: &Uri) -> Vec<SemanticToken> {
        let response = self.request::<SemanticTokensFullRequest>(SemanticTokensParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        });
        match response {
            Some(SemanticTokensResult::Tokens(tokens)) => tokens.data,
            other => panic!("expected a token stream, got {other:?}"),
        }
    }

    /// Decode the delta stream into absolute tokens with legend names,
    /// asserting delta-consistency (indices in the advertised legend,
    /// strictly increasing positions) along the way.
    fn decode_tokens(&self, data: &[SemanticToken]) -> Vec<DecodedToken> {
        let legend = &self.init_result["capabilities"]["semanticTokensProvider"]["legend"];
        let types: Vec<String> =
            serde_json::from_value(legend["tokenTypes"].clone()).expect("legend token types");
        let modifiers: Vec<String> = serde_json::from_value(legend["tokenModifiers"].clone())
            .expect("legend token modifiers");
        let mut out: Vec<DecodedToken> = Vec::new();
        let (mut line, mut start) = (0u32, 0u32);
        for token in data {
            if token.delta_line > 0 {
                line += token.delta_line;
                start = token.delta_start;
            } else {
                start += token.delta_start;
            }
            if let Some(prev) = out.last() {
                assert!(
                    (line, start) > (prev.line, prev.start),
                    "token stream not strictly increasing at ({line}, {start})"
                );
            }
            assert!(token.length > 0, "zero-length token at ({line}, {start})");
            let token_type = types
                .get(token.token_type as usize)
                .unwrap_or_else(|| panic!("token type {} outside the legend", token.token_type))
                .clone();
            let mods: Vec<String> = modifiers
                .iter()
                .enumerate()
                .filter(|(i, _)| token.token_modifiers_bitset & (1 << i) != 0)
                .map(|(_, m)| m.clone())
                .collect();
            assert!(
                token.token_modifiers_bitset < (1 << modifiers.len()),
                "modifier bit outside the legend: {:#b}",
                token.token_modifiers_bitset
            );
            out.push(DecodedToken {
                line,
                start,
                length: token.length,
                token_type,
                modifiers: mods,
            });
        }
        out
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

/// One absolute, legend-resolved semantic token.
#[derive(Debug)]
struct DecodedToken {
    line: u32,
    start: u32,
    length: u32,
    token_type: String,
    modifiers: Vec<String>,
}

/// The decoded token starting exactly at `(line, start)`.
fn token_at(tokens: &[DecodedToken], line: u32, start: u32) -> &DecodedToken {
    tokens
        .iter()
        .find(|t| t.line == line && t.start == start)
        .unwrap_or_else(|| panic!("no token at ({line}, {start}) in {tokens:?}"))
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

// === .luab shape files =====================================================

#[test]
fn lb_files_publish_shape_parse_errors() {
    let client = start(&[]);
    let uri = client.uri("shapes.luab");
    // A method with a body: rejected at parse time with LB2010.
    let source = "type Shape = {\n    area(self): number { return 1 },\n}\n";
    let diags = client.open(&uri, source);
    assert!(
        diags.iter().any(|d| code_of(d) == "LB2010"),
        "expected LB2010, got {diags:?}"
    );
    // A clean edit clears them.
    let clean = "type Shape = {\n    area(self): number,\n}\n";
    assert!(client.change(&uri, clean).is_empty());
    client.shutdown();
}

#[test]
fn lb_goto_and_hover_resolve_type_names() {
    let client = start(&[]);
    let uri = client.uri("shapes.luab");
    let source = "type Point = { x: number, y: number }\ntype Pair = Point\n";
    client.open(&uri, source);
    let mut client = client;
    // `Point` on line 1 (the alias RHS) → the declaration on line 0.
    let location = client.definition(&uri, 1, 14).expect("definition");
    assert_eq!(location.range, range((0, 5), (0, 10)));
    let hover = client.hover(&uri, 1, 14).expect("hover");
    assert!(
        hover_text(&hover).contains("type Point"),
        "{}",
        hover_text(&hover)
    );
    client.shutdown();
}

// === SHAPES-V2 ambient package scope (goto/hover/completion) ============

const SHAPES_MANIFEST: &str = "\
[package]
name = \"demo\"
version = \"0.1.0\"
edition = \"5.4\"

[types]
shape-paths = [\"shapes\"]
";

const GEOMETRY_LUAB: &str = "\
--- A 2D point.
type Point = { x: number, y: number }
";

#[test]
fn lua_annotation_goto_and_hover_resolve_shape_types() {
    let client = start(&[
        ("luabox.toml", SHAPES_MANIFEST),
        ("shapes/geometry.luab", GEOMETRY_LUAB),
    ]);
    let uri = client.uri("main.lua");
    let source = "---@type geometry.Point\nlocal p = nil\nprint(p)\n";
    client.open(&uri, source);
    let mut client = client;
    // Cursor inside `geometry` (the first segment of the dotted reference,
    // not the declared name itself) still resolves the whole path.
    let location = client.definition(&uri, 0, 12).expect("definition");
    assert!(
        location.uri.as_str().ends_with("shapes/geometry.luab"),
        "{}",
        location.uri.as_str()
    );
    // The `Point` name token in `type Point = { ... }` on line 1.
    assert_eq!(location.range, range((1, 5), (1, 10)));

    let hover = client.hover(&uri, 0, 12).expect("hover");
    let text = hover_text(&hover);
    assert!(text.contains("type Point"), "{text}");
    assert!(text.contains("A 2D point."), "{text}");
    client.shutdown();
}

#[test]
fn lb_cross_file_goto_resolves_qualified_reference() {
    let render = "type Canvas = { origin: geometry.Point }\n";
    let client = start(&[
        ("luabox.toml", SHAPES_MANIFEST),
        ("shapes/geometry.luab", GEOMETRY_LUAB),
        ("shapes/render.luab", render),
    ]);
    let uri = client.uri("shapes/render.luab");
    client.open(&uri, render);
    let mut client = client;
    // Cursor inside `geometry`, the qualifying segment of `geometry.Point`.
    let location = client.definition(&uri, 0, 27).expect("definition");
    assert!(
        location.uri.as_str().ends_with("shapes/geometry.luab"),
        "{}",
        location.uri.as_str()
    );
    assert_eq!(location.range, range((1, 5), (1, 10)));
    client.shutdown();
}

#[test]
fn lua_annotation_completion_offers_scope_type_names() {
    let client = start(&[
        ("luabox.toml", SHAPES_MANIFEST),
        ("shapes/geometry.luab", GEOMETRY_LUAB),
    ]);
    let uri = client.uri("main.lua");
    let source = "---@type \nlocal p = nil\n";
    client.open(&uri, source);
    let mut client = client;
    // Cursor right after `---@type `, nothing typed yet.
    let items = client.complete(&uri, 0, 9);
    let point = items
        .iter()
        .find(|i| i.label == "geometry.Point")
        .unwrap_or_else(|| panic!("expected `geometry.Point`, got {items:?}"));
    assert_eq!(point.kind, Some(CompletionItemKind::CLASS));
    client.shutdown();
}

#[test]
fn lb_completion_offers_builtins_and_siblings() {
    let client = start(&[]);
    let uri = client.uri("shapes.luab");
    let source = "type Point = { x: number, y: number }\ntype Pair = \n";
    client.open(&uri, source);
    let mut client = client;
    let items = client.complete(&uri, 1, 12);
    let number = items
        .iter()
        .find(|i| i.label == "number")
        .unwrap_or_else(|| panic!("expected builtin `number`, got {items:?}"));
    assert_eq!(number.kind, Some(CompletionItemKind::KEYWORD));
    let point = items
        .iter()
        .find(|i| i.label == "Point")
        .unwrap_or_else(|| panic!("expected sibling `Point`, got {items:?}"));
    assert_eq!(point.kind, Some(CompletionItemKind::CLASS));
    client.shutdown();
}

// === Formatting ==========================================================

/// Un-canonical spacing/indentation that still parses cleanly.
const MESSY_LUA: &str = "local x=1\nif x>0 then\nprint( x )\nend\n";

#[test]
fn initialize_advertises_formatting_and_semantic_tokens() {
    let client = start(&[]);
    let caps = &client.init_result["capabilities"];
    assert_eq!(caps["documentFormattingProvider"], true);
    assert_eq!(caps["documentRangeFormattingProvider"], true);
    assert_eq!(caps["semanticTokensProvider"]["full"], true);
    let types = &caps["semanticTokensProvider"]["legend"]["tokenTypes"];
    for expected in [
        "variable",
        "parameter",
        "function",
        "keyword",
        "comment",
        "interface",
    ] {
        assert!(
            types.as_array().unwrap().iter().any(|t| t == expected),
            "legend missing `{expected}`: {types}"
        );
    }
    client.shutdown();
}

#[test]
fn formatting_returns_whole_document_edit_matching_canonical_fmt() {
    let client = start(&[]);
    let uri = client.uri("main.lua");
    client.open(&uri, MESSY_LUA);
    let mut client = client;
    let edits = client.formatting(&uri).expect("edits");
    assert_eq!(edits.len(), 1, "{edits:?}");
    let edit = &edits[0];
    // A single whole-document replacement...
    assert_eq!(edit.range.start, Position::new(0, 0));
    assert_eq!(edit.range.end, Position::new(4, 0));
    // ...whose application equals the canonical formatter's output.
    let canonical = luabox_syntax::lua::fmt::format(MESSY_LUA, luabox_syntax::lua::Dialect::Lua54);
    assert_ne!(
        canonical, MESSY_LUA,
        "fixture must not already be canonical"
    );
    assert_eq!(edit.new_text, canonical);
    client.shutdown();
}

#[test]
fn formatting_canonical_document_returns_no_edits() {
    let canonical = luabox_syntax::lua::fmt::format(MESSY_LUA, luabox_syntax::lua::Dialect::Lua54);
    let client = start(&[]);
    let uri = client.uri("main.lua");
    client.open(&uri, &canonical);
    let mut client = client;
    let edits = client.formatting(&uri).expect("edits");
    assert!(edits.is_empty(), "{edits:?}");
    client.shutdown();
}

#[test]
fn formatting_parse_error_document_returns_no_edits() {
    // The formatter never destroys broken code: no edits, not an error
    // response (`wait_response` panics on protocol errors).
    let client = start(&[]);
    let uri = client.uri("broken.lua");
    client.open(&uri, "local = 1\n");
    let mut client = client;
    let edits = client.formatting(&uri).expect("edits");
    assert!(edits.is_empty(), "{edits:?}");
    client.shutdown();
}

#[test]
fn lb_formatting_matches_shape_formatter() {
    let source = "type Point = {x: number, y: number}\n";
    let client = start(&[]);
    let uri = client.uri("shapes.luab");
    client.open(&uri, source);
    let mut client = client;
    let edits = client.formatting(&uri).expect("edits");
    assert_eq!(edits.len(), 1, "{edits:?}");
    let canonical = luabox_syntax::shape::format(source);
    assert_ne!(canonical, source, "fixture must not already be canonical");
    assert_eq!(edits[0].new_text, canonical);
    // A parse-error .luab document yields no edits.
    client.change(&uri, "type = {\n");
    let edits = client.formatting(&uri).expect("edits");
    assert!(edits.is_empty(), "{edits:?}");
    client.shutdown();
}

#[test]
fn range_formatting_returns_the_whole_document_edit() {
    // MVP range semantics: the canonical formatter is whole-file, so a
    // range request returns the same single whole-document edit.
    let client = start(&[]);
    let uri = client.uri("main.lua");
    client.open(&uri, MESSY_LUA);
    let mut client = client;
    let full = client.formatting(&uri).expect("full edits");
    let ranged = client
        .range_formatting(&uri, range((2, 0), (2, 10)))
        .expect("range edits");
    assert_eq!(ranged, full);
    client.shutdown();
}

// === Semantic tokens =====================================================

#[test]
fn semantic_tokens_distinguish_locals_globals_params_and_doc_comments() {
    let source = "\
---@param n number
local function double(n)
    return n * 2
end
-- plain comment
local answer = double(2)
print(answer)
";
    let client = start(&[]);
    let uri = client.uri("main.lua");
    client.open(&uri, source);
    let mut client = client;
    let data = client.semantic_tokens(&uri);
    assert!(!data.is_empty());
    let tokens = client.decode_tokens(&data);

    // `---@param n number`: a LuaCATS annotation, not a prose comment.
    let doc = token_at(&tokens, 0, 0);
    assert_eq!(doc.token_type, "comment");
    assert!(
        doc.modifiers.contains(&"documentation".to_string()),
        "{doc:?}"
    );
    // `-- plain comment` stays an undecorated comment.
    let prose = token_at(&tokens, 4, 0);
    assert_eq!(prose.token_type, "comment");
    assert!(prose.modifiers.is_empty(), "{prose:?}");

    // Keywords and the declaration site.
    assert_eq!(token_at(&tokens, 1, 0).token_type, "keyword"); // local
    assert_eq!(token_at(&tokens, 1, 6).token_type, "keyword"); // function
    let decl = token_at(&tokens, 1, 15); // double
    assert_eq!(decl.token_type, "function");
    assert!(
        decl.modifiers.contains(&"declaration".to_string()),
        "{decl:?}"
    );
    let param_decl = token_at(&tokens, 1, 22); // n
    assert_eq!(param_decl.token_type, "parameter");

    // `n` inside the body resolves through HIR to the parameter.
    assert_eq!(token_at(&tokens, 2, 11).token_type, "parameter");
    assert_eq!(token_at(&tokens, 2, 13).token_type, "operator"); // *
    assert_eq!(token_at(&tokens, 2, 15).token_type, "number"); // 2

    // Local vs global: `answer` is a plain variable, `print` is a global
    // (static) from the standard library (defaultLibrary).
    let local_use = token_at(&tokens, 6, 6); // answer in print(answer)
    assert_eq!(local_use.token_type, "variable");
    assert!(local_use.modifiers.is_empty(), "{local_use:?}");
    let global = token_at(&tokens, 6, 0); // print
    assert_eq!(global.token_type, "variable");
    assert!(
        global.modifiers.contains(&"static".to_string()),
        "{global:?}"
    );
    assert!(
        global.modifiers.contains(&"defaultLibrary".to_string()),
        "{global:?}"
    );
    // `double(2)` resolves to the local function.
    assert_eq!(token_at(&tokens, 5, 15).token_type, "function");
    client.shutdown();
}

#[test]
fn semantic_token_columns_and_lengths_are_utf16() {
    // The emoji is 4 bytes but 2 UTF-16 units.
    let source = "--[[\u{1F600}]] local x = 1\n";
    let client = start(&[]);
    let uri = client.uri("main.lua");
    client.open(&uri, source);
    let mut client = client;
    let data = client.semantic_tokens(&uri);
    let tokens = client.decode_tokens(&data);
    let comment = token_at(&tokens, 0, 0);
    assert_eq!(comment.token_type, "comment");
    assert_eq!(comment.length, 8); // --[[ + emoji(2) + ]]
    assert_eq!(token_at(&tokens, 0, 9).token_type, "keyword"); // local
    let x = token_at(&tokens, 0, 15);
    assert_eq!(x.token_type, "variable");
    assert!(x.modifiers.contains(&"declaration".to_string()), "{x:?}");
    client.shutdown();
}

#[test]
fn lb_semantic_tokens_classify_shape_declarations() {
    let source = "\
--- A closed shape.
export type Shape = {
    area(self): number,
}
type Circle = { radius: number }
type Pair<T> = { first: T }
";
    let client = start(&[]);
    let uri = client.uri("shapes.luab");
    client.open(&uri, source);
    let mut client = client;
    let data = client.semantic_tokens(&uri);
    let tokens = client.decode_tokens(&data);

    // `///` doc comment vs keywords.
    let doc = token_at(&tokens, 0, 0);
    assert_eq!(doc.token_type, "comment");
    assert!(
        doc.modifiers.contains(&"documentation".to_string()),
        "{doc:?}"
    );
    assert_eq!(token_at(&tokens, 1, 0).token_type, "keyword"); // export
    assert_eq!(token_at(&tokens, 1, 7).token_type, "keyword"); // type

    // The declared name is a type declaration.
    let shape = token_at(&tokens, 1, 12);
    assert_eq!(shape.token_type, "type");
    assert!(
        shape.modifiers.contains(&"declaration".to_string()),
        "{shape:?}"
    );

    // Method member: method, `self` keyword, return type.
    assert_eq!(token_at(&tokens, 2, 4).token_type, "method"); // area
    assert_eq!(token_at(&tokens, 2, 9).token_type, "keyword"); // self
    assert_eq!(token_at(&tokens, 2, 16).token_type, "type"); // number

    // Field name and field type.
    assert_eq!(token_at(&tokens, 4, 16).token_type, "property"); // radius
    assert_eq!(token_at(&tokens, 4, 24).token_type, "type"); // number

    // Generics: `T` declares a type parameter and its use resolves to it.
    let generic = token_at(&tokens, 5, 10);
    assert_eq!(generic.token_type, "typeParameter");
    assert!(
        generic.modifiers.contains(&"declaration".to_string()),
        "{generic:?}"
    );
    assert_eq!(token_at(&tokens, 5, 24).token_type, "typeParameter"); // first: T
    client.shutdown();
}

#[test]
fn lb_semantic_tokens_survive_parse_errors() {
    // The shape tree is lossless: broken input still highlights.
    let source = "type Shape = {\n    area(self): number { return 1 },\n}\n";
    let client = start(&[]);
    let uri = client.uri("shapes.luab");
    client.open(&uri, source);
    let mut client = client;
    let data = client.semantic_tokens(&uri);
    let tokens = client.decode_tokens(&data);
    assert!(!tokens.is_empty());
    assert_eq!(token_at(&tokens, 1, 4).token_type, "method"); // area
    client.shutdown();
}

// === Project bootstrap ===================================================

// === Inlay hints =========================================================

/// The plain string label of a hint.
fn hint_label(hint: &InlayHint) -> &str {
    match &hint.label {
        InlayHintLabel::String(label) => label,
        other @ InlayHintLabel::LabelParts(_) => panic!("expected a string label, got {other:?}"),
    }
}

/// The hint whose position is exactly `(line, character)`.
fn hint_at(hints: &[InlayHint], line: u32, character: u32) -> &InlayHint {
    hints
        .iter()
        .find(|h| h.position == Position::new(line, character))
        .unwrap_or_else(|| panic!("no hint at ({line}, {character}) in {hints:?}"))
}

#[test]
fn initialize_advertises_inlay_hints() {
    let client = start(&[]);
    assert_eq!(
        client.init_result["capabilities"]["inlayHintProvider"],
        true
    );
    client.shutdown();
}

#[test]
fn inlay_hints_show_inferred_types_for_unannotated_locals() {
    let source = "\
local count = 42
local greeting = \"hi\"
local flag = true
for i = 1, 10 do
  print(i)
end
";
    let client = start(&[]);
    let uri = client.uri("main.lua");
    client.open(&uri, source);
    let mut client = client;
    let hints = client.inlay_hints(&uri, range((0, 0), (6, 0)));
    assert_eq!(hint_label(hint_at(&hints, 0, 11)), ": integer"); // count
    assert_eq!(hint_label(hint_at(&hints, 1, 14)), ": string"); // greeting
    assert_eq!(hint_label(hint_at(&hints, 2, 10)), ": boolean"); // flag
    assert_eq!(hint_label(hint_at(&hints, 3, 5)), ": integer"); // for i
    client.shutdown();
}

#[test]
fn inlay_hints_show_inferred_table_shapes() {
    let source = "\
local point = { x = 1, y = 2 }
";
    let client = start(&[]);
    let uri = client.uri("main.lua");
    client.open(&uri, source);
    let mut client = client;
    let hints = client.inlay_hints(&uri, range((0, 0), (1, 0)));
    assert_eq!(
        hint_label(hint_at(&hints, 0, 11)),
        ": { x: integer, y: integer }"
    );
    client.shutdown();
}

#[test]
fn inlay_hints_render_annotated_types_inline() {
    let source = "\
---@param width number
---@param height number
---@return number
local function area(width, height)
  return width * height
end
---@type number
local n = 1
";
    let client = start(&[]);
    let uri = client.uri("main.lua");
    client.open(&uri, source);
    let mut client = client;
    let hints = client.inlay_hints(&uri, range((0, 0), (8, 0)));
    // The annotations live in the doc block; the signature still hints.
    assert_eq!(hint_label(hint_at(&hints, 3, 25)), ": number"); // width
    assert_eq!(hint_label(hint_at(&hints, 3, 33)), ": number"); // height
    assert_eq!(hint_label(hint_at(&hints, 3, 34)), ": number"); // returns
    assert_eq!(hint_label(hint_at(&hints, 7, 7)), ": number"); // n
    client.shutdown();
}

#[test]
fn inlay_hints_render_annotated_returns_verbatim() {
    // `Rect` is not declared in this file (a `.luab` shape or cross-file
    // class): the return hint must still render the annotation text.
    let source = "\
local Rect = {}
Rect.__index = Rect

---@param width number
---@param height number
---@return Rect
function Rect.new(width, height)
  return setmetatable({ width = width, height = height }, Rect)
end
";
    let client = start(&[]);
    let uri = client.uri("main.lua");
    client.open(&uri, source);
    let mut client = client;
    let hints = client.inlay_hints(&uri, range((0, 0), (9, 0)));
    assert_eq!(hint_label(hint_at(&hints, 6, 32)), ": Rect"); // returns
    assert_eq!(hint_label(hint_at(&hints, 6, 23)), ": number"); // width
    client.shutdown();
}

#[test]
fn inlay_hints_skip_function_names_and_unknowns() {
    let source = "\
local function helper()
  return 1
end
local result = helper()
";
    let client = start(&[]);
    let uri = client.uri("main.lua");
    client.open(&uri, source);
    let mut client = client;
    let hints = client.inlay_hints(&uri, range((0, 0), (4, 0)));
    // The function *name* gets no binding hint, but the parameter list
    // gets a return-type hint...
    assert!(
        !hints.iter().any(|h| h.position == Position::new(0, 21)),
        "local function name must not get a binding hint: {hints:?}"
    );
    assert_eq!(hint_label(hint_at(&hints, 0, 23)), ": integer");
    // ...and the inferred call result flows into the local.
    assert_eq!(hint_label(hint_at(&hints, 3, 12)), ": integer");
    client.shutdown();
}

#[test]
fn inlay_hints_seed_params_from_call_sites() {
    let source = "\
local function area(w, h)
  local result = w * h
  return result
end
local a = area(3, 4)
";
    let client = start(&[]);
    let uri = client.uri("main.lua");
    client.open(&uri, source);
    let mut client = client;
    let hints = client.inlay_hints(&uri, range((0, 0), (5, 0)));
    assert_eq!(hint_label(hint_at(&hints, 0, 21)), ": integer"); // w
    assert_eq!(hint_label(hint_at(&hints, 0, 24)), ": integer"); // h
    assert_eq!(hint_label(hint_at(&hints, 0, 25)), ": integer"); // returns
    assert_eq!(hint_label(hint_at(&hints, 1, 14)), ": integer"); // result
    assert_eq!(hint_label(hint_at(&hints, 4, 7)), ": integer"); // a
    client.shutdown();
}

#[test]
fn inlay_hints_type_self_through_the_index_idiom() {
    let source = "\
local Circle = {}
Circle.__index = Circle

function Circle.new(radius)
  return setmetatable({ radius = radius }, Circle)
end

function Circle:area()
  local r = self.radius
  return r * r
end

local c = Circle.new(2)
";
    let client = start(&[]);
    let uri = client.uri("main.lua");
    client.open(&uri, source);
    let mut client = client;
    let hints = client.inlay_hints(&uri, range((0, 0), (13, 0)));
    assert_eq!(hint_label(hint_at(&hints, 3, 26)), ": integer"); // radius param
    assert_eq!(hint_label(hint_at(&hints, 8, 9)), ": integer"); // r = self.radius
    assert_eq!(hint_label(hint_at(&hints, 7, 22)), ": integer"); // area returns
    client.shutdown();
}

#[test]
fn inlay_hints_show_multi_value_returns() {
    let source = "\
local function pair()
  return 1, \"x\"
end
";
    let client = start(&[]);
    let uri = client.uri("main.lua");
    client.open(&uri, source);
    let mut client = client;
    let hints = client.inlay_hints(&uri, range((0, 0), (3, 0)));
    assert_eq!(hint_label(hint_at(&hints, 0, 21)), ": integer, string");
    client.shutdown();
}

#[test]
fn inlay_hints_respect_the_requested_range() {
    let source = "\
local first = 1
local second = 2
";
    let client = start(&[]);
    let uri = client.uri("main.lua");
    client.open(&uri, source);
    let mut client = client;
    let hints = client.inlay_hints(&uri, range((1, 0), (2, 0)));
    assert!(
        hints.iter().all(|h| h.position.line == 1),
        "only line 1 hints expected: {hints:?}"
    );
    client.shutdown();
}

#[test]
fn inlay_hints_cross_file_params_and_require_results() {
    let geometry = "\
local M = {}

function M.area(w, h)
  local result = w * h
  return result
end

return M
";
    let main = "\
local geo = require(\"geometry\")
local a = geo.area(3, 4)
";
    let client = start(&[("geometry.lua", geometry), ("main.lua", main)]);
    let geo_uri = client.uri("geometry.lua");
    let main_uri = client.uri("main.lua");
    let mut client = client;

    // geometry.lua: params seeded from main.lua's call site.
    let hints = client.inlay_hints(&geo_uri, range((0, 0), (8, 0)));
    assert_eq!(hint_label(hint_at(&hints, 2, 17)), ": integer"); // w
    assert_eq!(hint_label(hint_at(&hints, 2, 20)), ": integer"); // h
    assert_eq!(hint_label(hint_at(&hints, 2, 21)), ": integer"); // returns
    assert_eq!(hint_label(hint_at(&hints, 3, 14)), ": integer"); // result

    // main.lua: the require result is the module table, and the call
    // result types through the exported function's inferred returns.
    let hints = client.inlay_hints(&main_uri, range((0, 0), (2, 0)));
    assert!(
        hint_label(hint_at(&hints, 0, 9)).contains("area: fun("),
        "{hints:?}"
    );
    assert_eq!(hint_label(hint_at(&hints, 1, 7)), ": integer"); // a

    // Editing the dependent invalidates the dependency's hints.
    client.change(
        &main_uri,
        "local geo = require(\"geometry\")\nlocal a = geo.area(\"s\", 4)\n",
    );
    let hints = client.inlay_hints(&geo_uri, range((0, 0), (8, 0)));
    assert_eq!(hint_label(hint_at(&hints, 2, 17)), ": string"); // w reseeded
    client.shutdown();
}

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

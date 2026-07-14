//! End-to-end tests: a real client driving the server in-process over
//! [`lsp_server::Connection::memory`] — full initialize handshake, document
//! lifecycle, published diagnostics, and every tranche-1 request.

use std::path::PathBuf;
use std::time::Duration;

use lsp_server::{Connection, Message, Notification, Request, RequestId};
use lsp_types::notification::{
    DidChangeConfiguration, DidChangeTextDocument, DidChangeWatchedFiles, DidCloseTextDocument,
    DidOpenTextDocument, Exit, Initialized, Notification as _, Progress, PublishDiagnostics,
};
use lsp_types::request::{
    CallHierarchyIncomingCalls, CallHierarchyOutgoingCalls, CallHierarchyPrepare,
    CodeActionRequest, Completion, DocumentHighlightRequest, DocumentSymbolRequest,
    FoldingRangeRequest, Formatting, GotoDefinition, GotoImplementation, GotoTypeDefinition,
    HoverRequest, InlayHintRequest, PrepareRenameRequest, RangeFormatting, References, Rename,
    Request as _, SelectionRangeRequest, SemanticTokensFullRequest, Shutdown, SignatureHelpRequest,
    WorkspaceSymbolRequest,
};
use lsp_types::{
    CallHierarchyIncomingCall, CallHierarchyIncomingCallsParams, CallHierarchyItem,
    CallHierarchyOutgoingCall, CallHierarchyOutgoingCallsParams, CallHierarchyPrepareParams,
    ClientCapabilities, CodeActionContext, CodeActionKind, CodeActionOrCommand, CodeActionParams,
    CompletionItemKind, CompletionParams, CompletionResponse, DiagnosticSeverity,
    DidChangeConfigurationParams, DidChangeTextDocumentParams, DidChangeWatchedFilesParams,
    DidCloseTextDocumentParams, DidOpenTextDocumentParams, DocumentFormattingParams,
    DocumentHighlight, DocumentHighlightKind, DocumentHighlightParams,
    DocumentRangeFormattingParams, DocumentSymbolParams, DocumentSymbolResponse, FileChangeType,
    FileEvent, FoldingRange, FoldingRangeKind, FoldingRangeParams, FormattingOptions,
    GotoDefinitionParams, GotoDefinitionResponse, HoverContents, HoverParams, InitializeParams,
    InlayHint, InlayHintLabel, InlayHintParams, NumberOrString, ParameterLabel,
    PartialResultParams, Position, PrepareRenameResponse, ProgressParams, ProgressParamsValue,
    PublishDiagnosticsParams, Range, ReferenceContext, ReferenceParams, RenameParams,
    SelectionRange, SelectionRangeParams, SemanticToken, SemanticTokensParams,
    SemanticTokensResult, SignatureHelp, SignatureHelpParams, SymbolInformation, SymbolKind,
    TextDocumentContentChangeEvent, TextDocumentIdentifier, TextDocumentItem,
    TextDocumentPositionParams, TextEdit, Uri, VersionedTextDocumentIdentifier,
    WindowClientCapabilities, WorkDoneProgress, WorkDoneProgressParams, WorkspaceEdit,
    WorkspaceFolder, WorkspaceSymbolParams, WorkspaceSymbolResponse,
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
    start_with(files, ClientCapabilities::default())
}

/// Like [`start`], but with explicit client capabilities — used to opt into
/// optional protocol features (e.g. work-done progress) that are off by
/// default in the plain harness.
fn start_with(files: &[(&str, &str)], capabilities: ClientCapabilities) -> TestClient {
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
        capabilities,
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

    /// Send an incremental (ranged) change batch and return the resulting
    /// diagnostics. Each change is relative to the state after the previous.
    fn change_incremental(
        &self,
        uri: &Uri,
        changes: Vec<TextDocumentContentChangeEvent>,
    ) -> Vec<lsp_types::Diagnostic> {
        self.notify::<DidChangeTextDocument>(DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier {
                uri: uri.clone(),
                version: 2,
            },
            content_changes: changes,
        });
        self.wait_diagnostics(uri)
    }

    /// Notify `workspace/didChangeConfiguration` (the settings payload is
    /// unused — the server re-reads `luabox.toml` from disk).
    fn notify_config_changed(&self) {
        self.notify::<DidChangeConfiguration>(DidChangeConfigurationParams {
            settings: serde_json::Value::Null,
        });
    }

    /// Notify `workspace/didChangeWatchedFiles` that `rel` changed on disk.
    fn notify_watched_change(&self, rel: &str) {
        self.notify::<DidChangeWatchedFiles>(DidChangeWatchedFilesParams {
            changes: vec![FileEvent {
                uri: self.uri(rel),
                typ: FileChangeType::CHANGED,
            }],
        });
    }

    /// Collect the `$/progress` notification kinds ("begin"/"report"/"end")
    /// until (and including) the terminating "end".
    fn drain_progress(&self) -> Vec<&'static str> {
        let mut kinds = Vec::new();
        loop {
            if let Message::Notification(not) = self.recv()
                && not.method == Progress::METHOD
            {
                let params: ProgressParams =
                    serde_json::from_value(not.params).expect("decode progress");
                let ProgressParamsValue::WorkDone(value) = params.value;
                let kind = match value {
                    WorkDoneProgress::Begin(_) => "begin",
                    WorkDoneProgress::Report(_) => "report",
                    WorkDoneProgress::End(_) => "end",
                };
                kinds.push(kind);
                if kind == "end" {
                    return kinds;
                }
            }
        }
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

    fn signature_help(&mut self, uri: &Uri, line: u32, character: u32) -> Option<SignatureHelp> {
        self.request::<SignatureHelpRequest>(SignatureHelpParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                position: Position { line, character },
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            context: None,
        })
    }

    fn prepare_call_hierarchy(
        &mut self,
        uri: &Uri,
        line: u32,
        character: u32,
    ) -> Vec<CallHierarchyItem> {
        self.request::<CallHierarchyPrepare>(CallHierarchyPrepareParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                position: Position { line, character },
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .unwrap_or_default()
    }

    fn incoming_calls(&mut self, item: CallHierarchyItem) -> Vec<CallHierarchyIncomingCall> {
        self.request::<CallHierarchyIncomingCalls>(CallHierarchyIncomingCallsParams {
            item,
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .unwrap_or_default()
    }

    fn outgoing_calls(&mut self, item: CallHierarchyItem) -> Vec<CallHierarchyOutgoingCall> {
        self.request::<CallHierarchyOutgoingCalls>(CallHierarchyOutgoingCallsParams {
            item,
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .unwrap_or_default()
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

    fn type_definition(
        &mut self,
        uri: &Uri,
        line: u32,
        character: u32,
    ) -> Option<lsp_types::Location> {
        let response = self.request::<GotoTypeDefinition>(GotoDefinitionParams {
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

    fn implementation(&mut self, uri: &Uri, line: u32, character: u32) -> Vec<lsp_types::Location> {
        let response = self.request::<GotoImplementation>(GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                position: Position { line, character },
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        });
        match response {
            Some(GotoDefinitionResponse::Array(locations)) => locations,
            None => Vec::new(),
            other => panic!("expected a location array, got {other:?}"),
        }
    }

    fn references(
        &mut self,
        uri: &Uri,
        line: u32,
        character: u32,
        include_declaration: bool,
    ) -> Vec<lsp_types::Location> {
        self.request::<References>(ReferenceParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                position: Position { line, character },
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            context: ReferenceContext {
                include_declaration,
            },
        })
        .unwrap_or_default()
    }

    fn rename(
        &mut self,
        uri: &Uri,
        line: u32,
        character: u32,
        new_name: &str,
    ) -> Option<WorkspaceEdit> {
        self.request::<Rename>(RenameParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                position: Position { line, character },
            },
            new_name: new_name.to_string(),
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
    }

    fn prepare_rename(
        &mut self,
        uri: &Uri,
        line: u32,
        character: u32,
    ) -> Option<PrepareRenameResponse> {
        self.request::<PrepareRenameRequest>(TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            position: Position { line, character },
        })
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

    fn workspace_symbols(&mut self, query: &str) -> Vec<SymbolInformation> {
        let response = self.request::<WorkspaceSymbolRequest>(WorkspaceSymbolParams {
            query: query.to_string(),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        });
        match response {
            Some(WorkspaceSymbolResponse::Flat(symbols)) => symbols,
            Some(other) => panic!("expected a flat symbol list, got {other:?}"),
            None => Vec::new(),
        }
    }

    fn document_highlight(
        &mut self,
        uri: &Uri,
        line: u32,
        character: u32,
    ) -> Vec<DocumentHighlight> {
        self.request::<DocumentHighlightRequest>(DocumentHighlightParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                position: Position { line, character },
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .unwrap_or_default()
    }

    fn folding_ranges(&mut self, uri: &Uri) -> Vec<FoldingRange> {
        self.request::<FoldingRangeRequest>(FoldingRangeParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .unwrap_or_default()
    }

    fn selection_ranges(&mut self, uri: &Uri, positions: Vec<Position>) -> Vec<SelectionRange> {
        self.request::<SelectionRangeRequest>(SelectionRangeParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            positions,
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .unwrap_or_default()
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

    fn code_actions(&mut self, uri: &Uri, range: Range) -> Vec<CodeActionOrCommand> {
        self.request::<CodeActionRequest>(CodeActionParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            range,
            context: CodeActionContext::default(),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
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

/// A ranged `didChange` content change replacing `[start, end)` with `text`.
fn ranged_change(start: (u32, u32), end: (u32, u32), text: &str) -> TextDocumentContentChangeEvent {
    TextDocumentContentChangeEvent {
        range: Some(range(start, end)),
        range_length: None,
        text: text.to_string(),
    }
}

/// The source text covered by an LSP range (UTF-16 positions → bytes via the
/// same line index the server uses), for asserting an edit is name-precise.
fn edit_text(source: &str, range: Range) -> &str {
    let index = luabox_lsp::LineIndex::new(source);
    let start = index.offset(range.start);
    let end = index.offset(range.end);
    &source[start..end]
}

/// Assert every edit in `changes` sets text `new` over a range that spans
/// exactly `old` in its file, returning the total edit count. Files are matched
/// to edits by URI suffix. This is the correctness invariant: a rename edit
/// must replace the bare identifier, never a wider span.
#[allow(clippy::mutable_key_type, reason = "Uri key matches WorkspaceEdit")]
fn assert_edits_are(
    changes: &WorkspaceEdit,
    files: &[(&str, &str)],
    old: &str,
    new: &str,
) -> usize {
    let map = changes.changes.as_ref().expect("changes");
    let mut total = 0;
    for (uri, edits) in map {
        let source = files
            .iter()
            .find(|(rel, _)| uri.as_str().ends_with(rel))
            .map_or_else(|| panic!("no source for {uri:?}"), |(_, s)| *s);
        for edit in edits {
            assert_eq!(edit.new_text, new, "{edit:?}");
            assert_eq!(
                edit_text(source, edit.range),
                old,
                "edit range must be exactly `{old}` in {uri:?}: {edit:?}"
            );
            total += 1;
        }
    }
    total
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
    // Incremental sync (kind 2), hover, definition, documentSymbol, and
    // completion triggered by `.` / `:`.
    assert_eq!(caps["textDocumentSync"], 2);
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

#[test]
fn hover_shows_single_see_reference_inline() {
    let client = start(&[]);
    let uri = client.uri("main.lua");
    let source = "---Frobnicates.\n---@see other.frob\nlocal function frob() end\nfrob()\n";
    client.open(&uri, source);
    let mut client = client;
    // Hover over the call site.
    let hover = client.hover(&uri, 3, 1).expect("hover");
    let text = hover_text(&hover);
    assert!(text.contains("Frobnicates."), "{text}");
    // One reference renders inline, LuaLS-style.
    assert!(text.contains("See: other.frob"), "{text}");
    client.shutdown();
}

#[test]
fn hover_lists_multiple_see_references() {
    let client = start(&[]);
    let uri = client.uri("main.lua");
    let source = "\
---@see first.helper
---@see second.helper explains the trick
local function frob() end
frob()
";
    client.open(&uri, source);
    let mut client = client;
    let hover = client.hover(&uri, 3, 1).expect("hover");
    let text = hover_text(&hover);
    assert!(text.contains("See:"), "{text}");
    assert!(text.contains("* first.helper"), "{text}");
    assert!(
        text.contains("* second.helper explains the trick"),
        "{text}"
    );
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

#[test]
fn goto_definition_redirects_via_source_annotation_with_line_and_col() {
    let client = start(&[]);
    let uri = client.uri("main.lua");
    let source = "\
---@source native/impl.c:12:4
function ffi_call() end
ffi_call()
";
    client.open(&uri, source);
    let mut client = client;
    // Goto-def on the use: redirected to the `@source` location instead of
    // the declaration. The relative path resolves against the annotated
    // file's directory; the target need not exist (LuaLS does not check).
    let location = client.definition(&uri, 2, 1).expect("definition");
    assert!(
        location.uri.as_str().ends_with("native/impl.c"),
        "{}",
        location.uri.as_str()
    );
    // `:12:4` is a 1-based line and 0-based column (LuaLS `line - 1, char`).
    assert_eq!(location.range.start, Position::new(11, 4));
    assert_eq!(location.range.end, location.range.start);
    client.shutdown();
}

#[test]
fn goto_definition_source_redirect_without_line_targets_file_start() {
    let client = start(&[]);
    let uri = client.uri("main.lua");
    let source = "---@source native/impl.c\nlocal function f() end\nf()\n";
    client.open(&uri, source);
    let mut client = client;
    let location = client.definition(&uri, 2, 0).expect("definition");
    assert!(
        location.uri.as_str().ends_with("native/impl.c"),
        "{}",
        location.uri.as_str()
    );
    // No `:line` defaults to line 1 / column 0 → position (0, 0).
    assert_eq!(location.range.start, Position::new(0, 0));
    client.shutdown();
}

#[test]
fn goto_definition_on_field_redirects_via_class_block_source() {
    let client = start(&[]);
    let uri = client.uri("main.lua");
    let source = "\
---@source native/point.c:3
---@class Point
---@field x number

---@type Point
local p = nil
print(p.x)
";
    client.open(&uri, source);
    let mut client = client;
    // A `@source` in the class block redirects field jumps too (LuaLS
    // `jump-source.lua` handles `doc.field.name` via the block's source).
    let location = client.definition(&uri, 6, 8).expect("definition");
    assert!(
        location.uri.as_str().ends_with("native/point.c"),
        "{}",
        location.uri.as_str()
    );
    assert_eq!(location.range.start, Position::new(2, 0));
    client.shutdown();
}

// === Goto type-definition ================================================

#[test]
fn initialize_advertises_type_definition_and_implementation() {
    let client = start(&[]);
    let caps = &client.init_result["capabilities"];
    assert_eq!(caps["typeDefinitionProvider"], true);
    assert_eq!(caps["implementationProvider"], true);
    client.shutdown();
}

#[test]
fn type_definition_on_typed_local_jumps_to_class() {
    let client = start(&[]);
    let uri = client.uri("main.lua");
    let source = "\
---@class Point
---@field x number

---@type Point
local p = nil
print(p)
";
    client.open(&uri, source);
    let mut client = client;
    // Cursor on `p` inside `print(p)` (line 5, `print(` is 6 chars).
    let location = client.type_definition(&uri, 5, 6).expect("type definition");
    assert_eq!(location.uri.as_str(), uri.as_str());
    // The `---@class Point` tag is on line 0.
    assert_eq!(location.range.start.line, 0, "{location:?}");
    client.shutdown();
}

#[test]
fn type_definition_on_alias_typed_local_jumps_to_alias() {
    let client = start(&[]);
    let uri = client.uri("main.lua");
    let source = "\
---@alias Id string

---@type Id
local key = nil
print(key)
";
    client.open(&uri, source);
    let mut client = client;
    // Cursor on `key` inside `print(key)` (line 4).
    let location = client.type_definition(&uri, 4, 6).expect("type definition");
    // The `---@alias Id` tag is on line 0.
    assert_eq!(location.range.start.line, 0, "{location:?}");
    client.shutdown();
}

#[test]
fn type_definition_on_primitive_typed_local_is_none() {
    let client = start(&[]);
    let uri = client.uri("main.lua");
    client.open(&uri, "---@type number\nlocal n = 1\nprint(n)\n");
    let mut client = client;
    // Cursor on `n` inside `print(n)` (line 2) — a primitive has no declaration.
    assert!(client.type_definition(&uri, 2, 6).is_none());
    client.shutdown();
}

#[test]
fn type_definition_crosses_files_to_the_class() {
    let client = start(&[
        ("point.lua", "---@class Point\n---@field x number\n"),
        ("main.lua", "---@type Point\nlocal p = nil\nprint(p)\n"),
    ]);
    let uri = client.uri("main.lua");
    client.open(&uri, "---@type Point\nlocal p = nil\nprint(p)\n");
    let mut client = client;
    // Cursor on `p` inside `print(p)` (line 2).
    let location = client.type_definition(&uri, 2, 6).expect("type definition");
    assert!(
        location.uri.as_str().ends_with("point.lua"),
        "{}",
        location.uri.as_str()
    );
    client.shutdown();
}

// === Goto implementation =================================================

#[test]
fn implementation_on_interface_returns_subclasses_across_files() {
    let client = start(&[
        ("base.lua", "---@class Base\n---@field id number\n"),
        ("derived.lua", "---@class Derived : Base\n"),
        ("other.lua", "---@class Other : Base\n"),
    ]);
    let base_uri = client.uri("base.lua");
    client.open(&base_uri, "---@class Base\n---@field id number\n");
    let mut client = client;
    // Cursor on the `---@class Base` line (line 0, on `Base`).
    let impls = client.implementation(&base_uri, 0, 10);
    assert_eq!(impls.len(), 2, "{impls:?}");
    assert!(
        impls
            .iter()
            .any(|l| l.uri.as_str().ends_with("derived.lua")),
        "{impls:?}"
    );
    assert!(
        impls.iter().any(|l| l.uri.as_str().ends_with("other.lua")),
        "{impls:?}"
    );
    client.shutdown();
}

#[test]
fn implementation_on_interface_without_subclasses_is_empty() {
    let client = start(&[("base.lua", "---@class Base\n")]);
    let base_uri = client.uri("base.lua");
    client.open(&base_uri, "---@class Base\n");
    let mut client = client;
    let impls = client.implementation(&base_uri, 0, 10);
    assert!(impls.is_empty(), "{impls:?}");
    client.shutdown();
}

// === Find references =====================================================

#[test]
fn initialize_advertises_references() {
    let client = start(&[]);
    assert_eq!(
        client.init_result["capabilities"]["referencesProvider"],
        true
    );
    client.shutdown();
}

#[test]
fn references_of_local_honor_include_declaration() {
    let client = start(&[]);
    let uri = client.uri("main.lua");
    client.open(
        &uri,
        "local value = 1\nprint(value)\nreturn value + value\n",
    );
    let mut client = client;
    // Cursor on `value` inside `print(value)` (line 1).
    let with = client.references(&uri, 1, 8, true);
    // The declaration plus three uses, all in this file.
    assert_eq!(with.len(), 4, "{with:?}");
    assert!(with.iter().all(|l| l.uri.as_str() == uri.as_str()));
    assert!(
        with.iter().any(|l| l.range == range((0, 6), (0, 11))),
        "declaration expected: {with:?}"
    );

    let without = client.references(&uri, 1, 8, false);
    assert_eq!(without.len(), 3, "{without:?}");
    assert!(
        !without.iter().any(|l| l.range == range((0, 6), (0, 11))),
        "declaration must be excluded: {without:?}"
    );
    client.shutdown();
}

#[test]
fn references_of_global_function_span_files() {
    let client = start(&[
        ("a.lua", "function greet() return 1 end\n"),
        ("b.lua", "greet()\ngreet()\n"),
    ]);
    let a_uri = client.uri("a.lua");
    let b_uri = client.uri("b.lua");
    let mut client = client;
    // Cursor on the first `greet` call in b.lua.
    let with = client.references(&b_uri, 0, 0, true);
    // The declaration in a.lua plus two call sites in b.lua.
    assert_eq!(with.len(), 3, "{with:?}");
    assert_eq!(
        with.iter()
            .filter(|l| l.uri.as_str() == a_uri.as_str())
            .count(),
        1,
        "declaration in a.lua: {with:?}"
    );
    assert_eq!(
        with.iter()
            .filter(|l| l.uri.as_str() == b_uri.as_str())
            .count(),
        2,
        "two uses in b.lua: {with:?}"
    );

    let without = client.references(&b_uri, 0, 0, false);
    assert_eq!(without.len(), 2, "{without:?}");
    assert!(
        without.iter().all(|l| l.uri.as_str() == b_uri.as_str()),
        "only uses remain: {without:?}"
    );
    client.shutdown();
}

// === Rename ==============================================================

#[test]
fn initialize_advertises_rename_with_prepare() {
    let client = start(&[]);
    // `renameProvider` is the options object, so prepareRename is advertised.
    assert_eq!(
        client.init_result["capabilities"]["renameProvider"]["prepareProvider"],
        true
    );
    client.shutdown();
}

#[test]
fn rename_local_updates_every_use_and_declaration_name_precise() {
    let src = "local value = 1\nprint(value)\nreturn value + value\n";
    let files = &[("main.lua", src)];
    let client = start(files);
    let uri = client.uri("main.lua");
    client.open(&uri, src);
    let mut client = client;
    // Cursor on `value` inside `print(value)` (line 1).
    let edit = client.rename(&uri, 1, 8, "amount").expect("rename");
    // One file, declaration plus three uses, each replacing exactly `value`.
    assert_eq!(edit.changes.as_ref().unwrap().len(), 1, "{edit:?}");
    assert_eq!(assert_edits_are(&edit, files, "value", "amount"), 4);
    client.shutdown();
}

#[test]
fn rename_global_function_across_two_files() {
    let files = &[
        ("a.lua", "function greet() return 1 end\n"),
        ("b.lua", "greet()\ngreet()\n"),
    ];
    let client = start(files);
    let b_uri = client.uri("b.lua");
    let mut client = client;
    // Cursor on the first `greet` call in b.lua.
    let edit = client.rename(&b_uri, 0, 0, "hello").expect("rename");
    // Two files: the declaration in a.lua plus two calls in b.lua.
    assert_eq!(edit.changes.as_ref().unwrap().len(), 2, "{edit:?}");
    assert_eq!(assert_edits_are(&edit, files, "greet", "hello"), 3);
    client.shutdown();
}

#[test]
fn rename_class_field_across_files_narrows_field_annotation() {
    // The `---@field x number` declaration must be narrowed to `x`, never the
    // whole annotation line.
    let files = &[
        ("point.lua", "---@class Point\n---@field x number\n"),
        (
            "use.lua",
            "---@type Point\nlocal p = nil\nprint(p.x)\nprint(p.x)\n",
        ),
    ];
    let client = start(files);
    let use_uri = client.uri("use.lua");
    let mut client = client;
    // Cursor on the `x` of the first `p.x` (line 2, `print(p.x)` → col 8).
    let edit = client.rename(&use_uri, 2, 8, "col").expect("rename");
    // Two files: the `@field` decl in point.lua plus two accesses in use.lua.
    assert_eq!(edit.changes.as_ref().unwrap().len(), 2, "{edit:?}");
    assert_eq!(assert_edits_are(&edit, files, "x", "col"), 3);
    client.shutdown();
}

#[test]
fn prepare_rename_returns_the_identifier_range() {
    let src = "local value = 1\nprint(value)\n";
    let client = start(&[]);
    let uri = client.uri("main.lua");
    client.open(&uri, src);
    let mut client = client;
    // Cursor inside `value` in `print(value)`.
    let response = client.prepare_rename(&uri, 1, 8).expect("prepare");
    match response {
        PrepareRenameResponse::Range(selected) => {
            // `value` on line 1, columns 6..11 — the identifier, nothing wider.
            assert_eq!(selected, range((1, 6), (1, 11)));
        }
        other => panic!("expected a bare range, got {other:?}"),
    }
    client.shutdown();
}

#[test]
fn prepare_rename_on_a_non_symbol_returns_none() {
    let src = "local value = 1\n";
    let client = start(&[]);
    let uri = client.uri("main.lua");
    client.open(&uri, src);
    let mut client = client;
    // Cursor on the numeric literal `1` — not a renameable symbol.
    assert!(client.prepare_rename(&uri, 0, 14).is_none());
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

/// A module returning a table with one exported function `<name>`.
fn module_exporting(name: &str) -> String {
    format!("local M = {{}}\nfunction M.{name}() end\nreturn M\n")
}

/// The single `additionalTextEdits` insert of an auto-require completion item.
fn require_edit(item: &lsp_types::CompletionItem) -> &TextEdit {
    let edits = item
        .additional_text_edits
        .as_ref()
        .expect("auto-require item carries an additionalTextEdits");
    assert_eq!(edits.len(), 1, "one require insert: {edits:?}");
    &edits[0]
}

#[test]
fn auto_require_completion_inserts_require_for_nested_module() {
    // `a/b/c.lua` exports `greet`; completing `gr` in an unrelated file offers
    // it with a require insert for the reversed module path `a.b.c`.
    let client = start(&[("a/b/c.lua", &module_exporting("greet"))]);
    let uri = client.uri("main.lua");
    client.open(&uri, "gr\n");
    let mut client = client;

    let items = client.complete(&uri, 0, 2);
    let greet = items
        .iter()
        .find(|i| i.label == "greet")
        .expect("auto-require greet item");
    assert_eq!(greet.kind, Some(CompletionItemKind::FUNCTION));
    assert_eq!(greet.detail.as_deref(), Some("Auto import from \"a.b.c\""));
    let edit = require_edit(greet);
    assert_eq!(edit.new_text, "local greet = require(\"a.b.c\").greet\n");
    // A zero-width insert at the very top of the file.
    assert_eq!(edit.range, range((0, 0), (0, 0)));

    // Existing plain completions are still present alongside auto-require.
    assert!(items.iter().any(|i| i.label == "local"), "{items:?}");
    client.shutdown();
}

#[test]
fn auto_require_completion_uses_init_module_path() {
    // `foo/init.lua` reverses to the module `foo`, not `foo.init`.
    let client = start(&[("foo/init.lua", &module_exporting("bar"))]);
    let uri = client.uri("main.lua");
    client.open(&uri, "ba\n");
    let mut client = client;

    let items = client.complete(&uri, 0, 2);
    let bar = items
        .iter()
        .find(|i| i.label == "bar")
        .expect("auto-require bar item");
    assert_eq!(
        require_edit(bar).new_text,
        "local bar = require(\"foo\").bar\n"
    );
    client.shutdown();
}

#[test]
fn auto_require_places_insert_after_existing_requires() {
    // A new require lands on its own line after the last existing require.
    let client = start(&[
        ("dep.lua", &module_exporting("helper")),
        ("a/b/c.lua", &module_exporting("greet")),
    ]);
    let uri = client.uri("main.lua");
    client.open(&uri, "local d = require(\"dep\")\ngr\n");
    let mut client = client;

    let items = client.complete(&uri, 1, 2);
    let greet = items
        .iter()
        .find(|i| i.label == "greet")
        .expect("auto-require greet item");
    let edit = require_edit(greet);
    assert_eq!(edit.new_text, "local greet = require(\"a.b.c\").greet\n");
    // Inserted at the start of line 1 (right after the existing require line).
    assert_eq!(edit.range, range((1, 0), (1, 0)));
    client.shutdown();
}

#[test]
fn auto_require_not_offered_when_module_already_required() {
    // The module is already required under a different local name, so no
    // auto-require is offered for its exports.
    let client = start(&[("a/b/c.lua", &module_exporting("greet"))]);
    let uri = client.uri("main.lua");
    client.open(&uri, "local m = require(\"a.b.c\")\ngr\n");
    let mut client = client;

    let items = client.complete(&uri, 1, 2);
    assert!(
        !items.iter().any(|i| i.label == "greet"),
        "already-required module must not be auto-imported: {items:?}"
    );
    client.shutdown();
}

#[test]
fn auto_require_not_offered_when_name_already_in_scope() {
    // A local named `greet` already binds the name; the scope item wins and
    // carries no require insert.
    let client = start(&[("a/b/c.lua", &module_exporting("greet"))]);
    let uri = client.uri("main.lua");
    client.open(&uri, "local greet = 1\ngr\n");
    let mut client = client;

    let items = client.complete(&uri, 1, 2);
    let greet = items
        .iter()
        .find(|i| i.label == "greet")
        .expect("the in-scope local greet");
    assert_eq!(greet.kind, Some(CompletionItemKind::VARIABLE));
    assert!(
        greet.additional_text_edits.is_none(),
        "in-scope local must not gain a require insert: {greet:?}"
    );
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

// === Workspace symbols ====================================================

#[test]
fn initialize_advertises_workspace_symbols() {
    let client = start(&[]);
    assert_eq!(
        client.init_result["capabilities"]["workspaceSymbolProvider"],
        true
    );
    client.shutdown();
}

#[test]
fn workspace_symbols_match_a_class_and_a_function_across_two_files() {
    let files = &[
        ("shapes.lua", "---@class Shape\n---@field kind string\n"),
        ("main.lua", "function computeArea() return 1 end\n"),
    ];
    let client = start(files);
    let shapes_uri = client.uri("shapes.lua");
    let main_uri = client.uri("main.lua");
    client.open(&shapes_uri, files[0].1);
    client.open(&main_uri, files[1].1);
    let mut client = client;

    let results = client.workspace_symbols("");
    let names: Vec<&str> = results.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"Shape"), "{names:?}");
    assert!(names.contains(&"kind"), "{names:?}");
    assert!(names.contains(&"computeArea"), "{names:?}");

    let shape = results.iter().find(|s| s.name == "Shape").unwrap();
    assert_eq!(shape.kind, SymbolKind::CLASS);
    assert_eq!(shape.location.uri.as_str(), shapes_uri.as_str());
    let func = results.iter().find(|s| s.name == "computeArea").unwrap();
    assert_eq!(func.kind, SymbolKind::FUNCTION);
    assert_eq!(func.location.uri.as_str(), main_uri.as_str());
    client.shutdown();
}

#[test]
fn workspace_symbols_query_is_case_insensitive() {
    let client = start(&[("main.lua", "function computeArea() return 1 end\n")]);
    let uri = client.uri("main.lua");
    client.open(&uri, "function computeArea() return 1 end\n");
    let mut client = client;
    let results = client.workspace_symbols("COMPUTEAREA");
    assert_eq!(results.len(), 1, "{results:?}");
    assert_eq!(results[0].name, "computeArea");
    client.shutdown();
}

#[test]
fn workspace_symbols_non_matching_query_returns_empty() {
    let client = start(&[("main.lua", "function computeArea() return 1 end\n")]);
    let uri = client.uri("main.lua");
    client.open(&uri, "function computeArea() return 1 end\n");
    let mut client = client;
    assert!(
        client
            .workspace_symbols("zzz_definitely_not_a_symbol")
            .is_empty()
    );
    client.shutdown();
}

// === Document highlight ==================================================

#[test]
fn initialize_advertises_document_highlight() {
    let client = start(&[]);
    assert_eq!(
        client.init_result["capabilities"]["documentHighlightProvider"],
        true
    );
    client.shutdown();
}

#[test]
fn document_highlight_distinguishes_read_and_write() {
    let client = start(&[]);
    let uri = client.uri("main.lua");
    let source = "local x = 1\nx = 2\nprint(x)\n";
    client.open(&uri, source);
    let mut client = client;
    // Cursor on the declaration.
    let hits = client.document_highlight(&uri, 0, 6);
    assert_eq!(hits.len(), 3, "{hits:?}");
    let kind_on_line = |line: u32| {
        hits.iter()
            .find(|h| h.range.start.line == line)
            .unwrap_or_else(|| panic!("no highlight on line {line}: {hits:?}"))
            .kind
    };
    assert_eq!(kind_on_line(0), Some(DocumentHighlightKind::WRITE)); // declaration
    assert_eq!(kind_on_line(1), Some(DocumentHighlightKind::WRITE)); // reassignment
    assert_eq!(kind_on_line(2), Some(DocumentHighlightKind::READ)); // print(x)
    client.shutdown();
}

#[test]
fn document_highlight_is_scoped_to_the_current_file() {
    let client = start(&[
        ("a.lua", "function greet() return 1 end\n"),
        ("b.lua", "greet()\ngreet()\n"),
    ]);
    let b_uri = client.uri("b.lua");
    client.open(&b_uri, "greet()\ngreet()\n");
    let mut client = client;
    let hits = client.document_highlight(&b_uri, 0, 0);
    // Two calls in b.lua; the declaration in a.lua never appears.
    assert_eq!(hits.len(), 2, "{hits:?}");
    assert!(
        hits.iter()
            .all(|h| h.kind == Some(DocumentHighlightKind::READ)),
        "{hits:?}"
    );
    client.shutdown();
}

// === Folding ranges =======================================================

#[test]
fn initialize_advertises_folding_ranges() {
    let client = start(&[]);
    assert_eq!(
        client.init_result["capabilities"]["foldingRangeProvider"],
        true
    );
    client.shutdown();
}

#[test]
fn folding_covers_a_function_body_and_a_multiline_table() {
    let client = start(&[]);
    let uri = client.uri("main.lua");
    let source = "\
local function f()
  return 1
end
local t = {
  1,
  2,
}
";
    client.open(&uri, source);
    let mut client = client;
    let ranges = client.folding_ranges(&uri);
    assert!(
        ranges
            .iter()
            .any(|r| r.start_line == 0 && r.end_line == 2 && r.kind.is_none()),
        "function body fold missing: {ranges:?}"
    );
    assert!(
        ranges
            .iter()
            .any(|r| r.start_line == 3 && r.end_line == 6 && r.kind.is_none()),
        "table fold missing: {ranges:?}"
    );
    client.shutdown();
}

#[test]
fn folding_marks_multiline_comments_with_the_comment_kind() {
    let client = start(&[]);
    let uri = client.uri("main.lua");
    let source = "--[[\nlong comment\n]]\nlocal x = 1\n";
    client.open(&uri, source);
    let mut client = client;
    let ranges = client.folding_ranges(&uri);
    let comment = ranges
        .iter()
        .find(|r| r.kind == Some(FoldingRangeKind::Comment))
        .unwrap_or_else(|| panic!("no comment fold: {ranges:?}"));
    assert_eq!((comment.start_line, comment.end_line), (0, 2));
    client.shutdown();
}

// === Selection ranges =====================================================

#[test]
fn initialize_advertises_selection_ranges() {
    let client = start(&[]);
    assert_eq!(
        client.init_result["capabilities"]["selectionRangeProvider"],
        true
    );
    client.shutdown();
}

#[test]
fn selection_range_expands_from_token_out_through_ancestors() {
    let client = start(&[]);
    let uri = client.uri("main.lua");
    let source = "local x = 1 + 2\n";
    client.open(&uri, source);
    let mut client = client;
    // Cursor inside the `1` literal (columns 10..11).
    let result = client.selection_ranges(&uri, vec![Position::new(0, 10)]);
    assert_eq!(result.len(), 1, "{result:?}");
    let mut ranges = vec![result[0].range];
    let mut parent = result[0].parent.as_deref();
    while let Some(p) = parent {
        ranges.push(p.range);
        parent = p.parent.as_deref();
    }
    // Innermost is the `1` token; each step widens, up to the whole file.
    assert_eq!(ranges[0], range((0, 10), (0, 11)));
    assert!(ranges.contains(&range((0, 10), (0, 15))), "{ranges:?}"); // `1 + 2`
    assert_eq!(*ranges.last().unwrap(), range((0, 0), (1, 0)));
    for pair in ranges.windows(2) {
        assert!(
            pair[1].start <= pair[0].start && pair[0].end <= pair[1].end && pair[1] != pair[0],
            "{ranges:?}"
        );
    }
    client.shutdown();
}

#[test]
fn selection_range_returns_one_result_per_position_in_order() {
    let client = start(&[]);
    let uri = client.uri("main.lua");
    let source = "local a = 1\nlocal b = 2\n";
    client.open(&uri, source);
    let mut client = client;
    let result = client.selection_ranges(&uri, vec![Position::new(0, 6), Position::new(1, 6)]);
    assert_eq!(result.len(), 2, "{result:?}");
    assert_eq!(result[0].range.start, Position::new(0, 6));
    assert_eq!(result[1].range.start, Position::new(1, 6));
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
fn inlay_hints_show_operator_overload_results() {
    // A `---@operator` result types the operator expression: an unannotated
    // local bound to `a + b` (both `Vec`) hints as `: Vec`, not `: unknown`
    // (#114). Proves the overload rides the same shared inference the LSP uses.
    let source = "\
---@class Vec
---@operator add(Vec): Vec

---@type Vec
local a
---@type Vec
local b
local c = a + b
";
    let client = start(&[]);
    let uri = client.uri("main.lua");
    client.open(&uri, source);
    let mut client = client;
    let hints = client.inlay_hints(&uri, range((0, 0), (8, 0)));
    assert_eq!(hint_label(hint_at(&hints, 7, 7)), ": Vec"); // c
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
    // `Rect` is not declared in this file (a cross-file class): the return
    // hint must still render the annotation text.
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
fn cross_file_alias_name_resolves_and_enforces_in_editor() {
    // `Id` is declared only in ids.lua; main.lua names it in a `---@param`.
    // Aliases are workspace-global (#110), so the misuse is diagnosed in the
    // editor exactly as under `luabox check` — editor/CI parity, riding the
    // same `Ambient::with_project_types` path as workspace-global classes.
    let ids = "---@alias Id string\nlocal M = {}\nreturn M\n";
    let main = "\
---@param x Id
local function use(x) end
use(42)
";
    let client = start(&[("ids.lua", ids), ("main.lua", main)]);
    let main_uri = client.uri("main.lua");
    let diags = client.open(&main_uri, main);
    assert!(
        diags.iter().any(|d| code_of(d) == "LB0300"),
        "cross-file alias misuse must be diagnosed in the editor: {diags:?}"
    );
    // A genuinely undeclared type name still trips LB0305 — workspace aliases
    // don't mask real unknown-type errors.
    let unknown = "\
---@param x Nope
local function use(x) end
";
    let diags = client.change(&main_uri, unknown);
    assert!(
        diags.iter().any(|d| code_of(d) == "LB0305"),
        "an undeclared type name is still LB0305: {diags:?}"
    );
    client.shutdown();
}

// === Lint diagnostics and quick-fixes ====================================

#[test]
fn open_with_unused_local_publishes_lint_diagnostic() {
    // `luabox lint`'s `unused-local` (LB0501, style/warn) surfaces as an
    // editor diagnostic alongside the type passes, tagged with the distinct
    // `luabox-lint` source so it's distinguishable from type diagnostics.
    let client = start(&[]);
    let uri = client.uri("main.lua");
    let diags = client.open(&uri, "local unused = 1\n");
    let lint = diags
        .iter()
        .find(|d| code_of(d) == "LB0501")
        .unwrap_or_else(|| panic!("expected an LB0501 lint diagnostic: {diags:?}"));
    assert_eq!(lint.severity, Some(DiagnosticSeverity::WARNING));
    assert_eq!(lint.source.as_deref(), Some("luabox-lint"));
    // The binding `unused` on line 0, columns 6..12.
    assert_eq!(lint.range, range((0, 6), (0, 12)));
    client.shutdown();
}

#[test]
fn luabox_ignore_suppresses_a_lint_diagnostic() {
    // A well-formed `---@luabox-ignore <rule> <reason>` on the finding's line
    // suppresses it — `lint_source` applies suppression internally, so the
    // editor honours it exactly as the CLI does.
    let client = start(&[]);
    let uri = client.uri("main.lua");
    let diags = client.open(
        &uri,
        "local unused = 1 ---@luabox-ignore unused-local intentional\n",
    );
    assert!(
        !diags.iter().any(|d| code_of(d) == "LB0501"),
        "the ignore comment must suppress LB0501: {diags:?}"
    );
    client.shutdown();
}

#[test]
#[allow(
    clippy::mutable_key_type,
    reason = "WorkspaceEdit keys its edits by Uri throughout these tests"
)]
fn code_action_offers_a_quickfix_for_a_fixable_lint() {
    // The `unused-local` fix renames the binding to `_name`; it must be offered
    // as a `quickfix` code action carrying the edit and referencing the lint
    // diagnostic it resolves.
    let src = "local unused = 1\n";
    let client = start(&[]);
    let uri = client.uri("main.lua");
    client.open(&uri, src);
    let mut client = client;
    // Request actions over the whole first line.
    let actions = client.code_actions(&uri, range((0, 0), (0, 16)));
    let action = actions
        .iter()
        .find_map(|a| match a {
            CodeActionOrCommand::CodeAction(action) => Some(action),
            CodeActionOrCommand::Command(_) => None,
        })
        .unwrap_or_else(|| panic!("expected a code action: {actions:?}"));
    assert_eq!(action.kind, Some(CodeActionKind::QUICKFIX));

    // The quick-fix references the diagnostic it resolves.
    let referenced = action.diagnostics.as_ref().expect("referenced diagnostics");
    assert_eq!(referenced.len(), 1, "{referenced:?}");
    assert_eq!(code_of(&referenced[0]), "LB0501");

    // ...and its edit renames exactly `unused` → `_unused`.
    let changes = action
        .edit
        .as_ref()
        .expect("edit")
        .changes
        .as_ref()
        .unwrap();
    let edits = changes
        .iter()
        .find(|(u, _)| u.as_str() == uri.as_str())
        .map(|(_, e)| e)
        .expect("edits for the file");
    assert_eq!(edits.len(), 1, "{edits:?}");
    assert_eq!(edits[0].range, range((0, 6), (0, 12)));
    assert_eq!(edits[0].new_text, "_unused");
    client.shutdown();
}

#[test]
fn initialize_advertises_code_actions() {
    let client = start(&[]);
    assert_eq!(
        client.init_result["capabilities"]["codeActionProvider"],
        true
    );
    client.shutdown();
}

// === Type-driven code actions (#129) =====================================

/// The `CodeAction`s (not `Command`s) among a code-action response.
fn actions_only(actions: &[CodeActionOrCommand]) -> Vec<&lsp_types::CodeAction> {
    actions
        .iter()
        .filter_map(|a| match a {
            CodeActionOrCommand::CodeAction(a) => Some(a),
            CodeActionOrCommand::Command(_) => None,
        })
        .collect()
}

/// The sole single-file `TextEdit` list of an action.
#[allow(clippy::mutable_key_type, reason = "Uri key matches WorkspaceEdit")]
fn action_edits(action: &lsp_types::CodeAction) -> &[TextEdit] {
    action
        .edit
        .as_ref()
        .expect("edit")
        .changes
        .as_ref()
        .expect("changes")
        .values()
        .next()
        .expect("one file")
}

/// A source that types a table literal missing a required field `y`, yielding
/// an `LB0302` on the `{ x = 1 }` constructor.
const MISSING_FIELD: &str = "\
---@class Point
---@field x number
---@field y number

---@param p Point
local function use(p) end
use({ x = 1 })
";

#[test]
fn code_action_add_missing_field_offers_typed_quickfix() {
    let client = start(&[]);
    let uri = client.uri("main.lua");
    let diags = client.open(&uri, MISSING_FIELD);
    assert!(
        diags.iter().any(|d| code_of(d) == "LB0302"),
        "expected an LB0302: {diags:?}"
    );
    let mut client = client;
    // Request over the `{ x = 1 }` constructor on line 6.
    let actions = client.code_actions(&uri, range((6, 4), (6, 13)));
    let action = actions_only(&actions)
        .into_iter()
        .find(|a| a.title == "Add missing field `y`")
        .unwrap_or_else(|| panic!("expected add-missing-field: {actions:?}"));
    assert_eq!(action.kind, Some(CodeActionKind::QUICKFIX));
    // References the LB0302 it resolves.
    let referenced = action.diagnostics.as_ref().expect("referenced diagnostics");
    assert_eq!(code_of(&referenced[0]), "LB0302");
    // Inserts a `y = nil, -- TODO` stub.
    let edits = action_edits(action);
    assert_eq!(edits.len(), 1, "{edits:?}");
    assert!(edits[0].new_text.contains("y = nil, -- TODO"), "{edits:?}");
    client.shutdown();
}

#[test]
fn code_action_annotate_local_from_inference() {
    let client = start(&[]);
    let uri = client.uri("main.lua");
    client.open(&uri, "local n = 42\nprint(n)\n");
    let mut client = client;
    let actions = client.code_actions(&uri, range((0, 0), (0, 11)));
    let action = actions_only(&actions)
        .into_iter()
        .find(|a| a.title == "Annotate `n` with inferred type")
        .unwrap_or_else(|| panic!("expected annotate action: {actions:?}"));
    assert_eq!(action.kind, Some(CodeActionKind::REFACTOR_REWRITE));
    let edits = action_edits(action);
    assert_eq!(edits[0].new_text, "---@type integer\n");
    // Inserted at the top of the local's line.
    assert_eq!(edits[0].range, range((0, 0), (0, 0)));
    client.shutdown();
}

#[test]
fn code_action_annotate_local_absent_when_annotated() {
    let client = start(&[]);
    let uri = client.uri("main.lua");
    client.open(&uri, "---@type number\nlocal n = 42\nprint(n)\n");
    let mut client = client;
    let actions = client.code_actions(&uri, range((1, 0), (1, 11)));
    assert!(
        !actions_only(&actions)
            .iter()
            .any(|a| a.title.starts_with("Annotate")),
        "already-annotated local must not offer annotate: {actions:?}"
    );
    client.shutdown();
}

#[test]
fn code_action_generate_class_from_literal() {
    let client = start(&[]);
    let uri = client.uri("main.lua");
    client.open(
        &uri,
        "local cfg = { count = 1, label = \"x\" }\nprint(cfg)\n",
    );
    let mut client = client;
    let actions = client.code_actions(&uri, range((0, 0), (0, 5)));
    let action = actions_only(&actions)
        .into_iter()
        .find(|a| a.title == "Generate `---@class Cfg` from table literal")
        .unwrap_or_else(|| panic!("expected generate-class action: {actions:?}"));
    assert_eq!(action.kind, Some(CodeActionKind::REFACTOR_REWRITE));
    let text = &action_edits(action)[0].new_text;
    assert!(text.contains("---@class Cfg"), "{text}");
    assert!(text.contains("---@field count integer"), "{text}");
    assert!(text.contains("---@field label string"), "{text}");
    client.shutdown();
}

#[test]
fn code_action_colon_to_dot_convert() {
    let client = start(&[]);
    let uri = client.uri("main.lua");
    client.open(&uri, "function T:m(x) end\n");
    let mut client = client;
    // Cursor on the method name.
    let actions = client.code_actions(&uri, range((0, 9), (0, 12)));
    let action = actions_only(&actions)
        .into_iter()
        .find(|a| a.title.starts_with("Convert `:` method"))
        .unwrap_or_else(|| panic!("expected colon→dot convert: {actions:?}"));
    assert_eq!(action.kind, Some(CodeActionKind::REFACTOR_REWRITE));
    // Two edits: `:` → `.` and a prepended `self, `.
    let edits = action_edits(action);
    assert_eq!(edits.len(), 2, "{edits:?}");
    assert!(edits.iter().any(|e| e.new_text == "."), "{edits:?}");
    assert!(edits.iter().any(|e| e.new_text == "self, "), "{edits:?}");
    client.shutdown();
}

#[test]
fn code_action_dot_convert_absent_without_self() {
    let client = start(&[]);
    let uri = client.uri("main.lua");
    client.open(&uri, "function T.m(x) end\n");
    let mut client = client;
    let actions = client.code_actions(&uri, range((0, 9), (0, 12)));
    assert!(
        !actions_only(&actions)
            .iter()
            .any(|a| a.title.starts_with("Convert")),
        "a dotted function without a self param is not convertible: {actions:?}"
    );
    client.shutdown();
}

// === Signature help ======================================================

#[test]
fn initialize_advertises_signature_help() {
    let client = start(&[]);
    let caps = &client.init_result["capabilities"];
    assert_eq!(
        caps["signatureHelpProvider"]["triggerCharacters"],
        serde_json::json!(["(", ","])
    );
    assert_eq!(
        caps["signatureHelpProvider"]["retriggerCharacters"],
        serde_json::json!([","])
    );
    client.shutdown();
}

#[test]
fn signature_help_inside_a_call_shows_params_and_docs() {
    let client = start(&[]);
    let uri = client.uri("main.lua");
    let source = "\
---@param a number the first arg
---@param b string
local function f(a, b) end
f(1, 2)
";
    client.open(&uri, source);
    let mut client = client;
    // Cursor right after `f(` on the last line.
    let help = client.signature_help(&uri, 3, 2).expect("signature help");
    assert_eq!(help.signatures.len(), 1);
    let sig = &help.signatures[0];
    assert_eq!(sig.label, "f(a: number, b: string)");
    assert_eq!(help.active_parameter, Some(0));
    assert_eq!(sig.active_parameter, Some(0));
    let params = sig.parameters.as_ref().expect("parameters");
    assert_eq!(params.len(), 2);
    match &params[0].label {
        ParameterLabel::LabelOffsets([start, end]) => {
            assert_eq!(&sig.label[*start as usize..*end as usize], "a: number");
        }
        other @ ParameterLabel::Simple(_) => panic!("expected label offsets, got {other:?}"),
    }
    let doc = match params[0].documentation.as_ref().expect("param doc") {
        lsp_types::Documentation::MarkupContent(m) => m.value.clone(),
        other @ lsp_types::Documentation::String(_) => {
            panic!("expected markup documentation, got {other:?}")
        }
    };
    assert!(doc.contains("the first arg"), "{doc}");
    client.shutdown();
}

#[test]
fn signature_help_active_parameter_advances_across_a_comma() {
    let client = start(&[]);
    let uri = client.uri("main.lua");
    let source = "\
---@param a number
---@param b string
local function f(a, b) end
f(1, 2)
";
    client.open(&uri, source);
    let mut client = client;
    // Cursor right after the comma on `f(1, 2)`.
    let help = client.signature_help(&uri, 3, 4).expect("signature help");
    assert_eq!(help.active_parameter, Some(1));
    client.shutdown();
}

#[test]
fn signature_help_clamps_to_the_last_declared_parameter() {
    let client = start(&[]);
    let uri = client.uri("main.lua");
    let source = "\
---@param a number
local function f(a) end
f(1, 2, 3)
";
    client.open(&uri, source);
    let mut client = client;
    // Cursor after the third argument; only one parameter is declared.
    let help = client.signature_help(&uri, 2, 8).expect("signature help");
    assert_eq!(help.active_parameter, Some(0));
    client.shutdown();
}

#[test]
fn signature_help_on_a_method_call_resolves_via_class_fields() {
    let client = start(&[]);
    let uri = client.uri("main.lua");
    let source = "\
---@class Point
---@field translate fun(dx: number, dy: number): Point

---@type Point
local p = nil
p:translate(1, 2)
";
    client.open(&uri, source);
    let mut client = client;
    // Cursor right after `translate(` on the last line.
    let help = client.signature_help(&uri, 5, 12).expect("signature help");
    assert_eq!(help.signatures.len(), 1);
    assert_eq!(
        help.signatures[0].label,
        "Point:translate(dx: number, dy: number): Point"
    );
    assert_eq!(help.active_parameter, Some(0));
    client.shutdown();
}

#[test]
fn signature_help_on_an_overloaded_function_returns_every_signature() {
    let client = start(&[]);
    let uri = client.uri("main.lua");
    let source = "\
---@param a number
---@overload fun(a: string): boolean
local function f(a) end
f(1)
";
    client.open(&uri, source);
    let mut client = client;
    let help = client.signature_help(&uri, 3, 2).expect("signature help");
    let labels: Vec<&str> = help.signatures.iter().map(|s| s.label.as_str()).collect();
    assert_eq!(labels, vec!["f(a: number)", "f(a: string): boolean"]);
    client.shutdown();
}

#[test]
fn signature_help_outside_any_call_is_none() {
    let client = start(&[]);
    let uri = client.uri("main.lua");
    client.open(&uri, "local x = 1\n");
    let mut client = client;
    assert!(client.signature_help(&uri, 0, 8).is_none());
    client.shutdown();
}

// === Call hierarchy ======================================================

#[test]
fn initialize_advertises_call_hierarchy() {
    let client = start(&[]);
    assert_eq!(
        client.init_result["capabilities"]["callHierarchyProvider"],
        true
    );
    client.shutdown();
}

#[test]
fn prepare_call_hierarchy_returns_the_function_item() {
    let client = start(&[]);
    let uri = client.uri("main.lua");
    let source = "local function greet() end\ngreet()\n";
    client.open(&uri, source);
    let mut client = client;
    // Cursor on the `greet` declaration name (line 0).
    let items = client.prepare_call_hierarchy(&uri, 0, 16);
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0].name, "greet");
    assert_eq!(items[0].kind, SymbolKind::FUNCTION);
    assert_eq!(items[0].uri.as_str(), uri.as_str());
    // The selection range is the name; the full range covers the statement.
    assert_eq!(items[0].selection_range, range((0, 15), (0, 20)));
    assert_eq!(items[0].range.start, Position::new(0, 0));

    // Preparing on the call site resolves to the same declaration.
    let from_call = client.prepare_call_hierarchy(&uri, 1, 0);
    assert_eq!(from_call.len(), 1, "{from_call:?}");
    assert_eq!(from_call[0].selection_range, items[0].selection_range);
    client.shutdown();
}

#[test]
fn outgoing_calls_lists_callees_within_a_function() {
    let client = start(&[]);
    let uri = client.uri("main.lua");
    let source = "\
local function a() end
local function b() end
local function caller()
  a()
  b()
  a()
end
";
    client.open(&uri, source);
    let mut client = client;
    // Prepare on `caller` (line 2).
    let item = client.prepare_call_hierarchy(&uri, 2, 16)[0].clone();
    let calls = client.outgoing_calls(item);
    let names: Vec<&str> = calls.iter().map(|c| c.to.name.as_str()).collect();
    assert_eq!(names, vec!["a", "b"], "{calls:?}");
    // `a` is called twice (lines 3 and 5), `b` once (line 4).
    let a = calls.iter().find(|c| c.to.name == "a").unwrap();
    assert_eq!(a.from_ranges.len(), 2, "{a:?}");
    assert_eq!(a.from_ranges[0], range((3, 2), (3, 3)));
    assert_eq!(a.from_ranges[1], range((5, 2), (5, 3)));
    let b = calls.iter().find(|c| c.to.name == "b").unwrap();
    assert_eq!(b.from_ranges, vec![range((4, 2), (4, 3))]);
    client.shutdown();
}

#[test]
fn incoming_calls_finds_callers_across_two_files() {
    let files = &[
        ("a.lua", "function greet() return 1 end\n"),
        (
            "b.lua",
            "local function useGreet()\n  greet()\n  greet()\nend\n",
        ),
    ];
    let client = start(files);
    let a_uri = client.uri("a.lua");
    let b_uri = client.uri("b.lua");
    let mut client = client;
    // Prepare on the `greet` declaration in a.lua, then request incoming.
    let item = client.prepare_call_hierarchy(&a_uri, 0, 10)[0].clone();
    let calls = client.incoming_calls(item);
    assert_eq!(calls.len(), 1, "one caller: {calls:?}");
    assert_eq!(calls[0].from.name, "useGreet");
    assert_eq!(calls[0].from.uri.as_str(), b_uri.as_str());
    // Both call sites in b.lua are collected, at their name-token ranges.
    assert_eq!(
        calls[0].from_ranges,
        vec![range((1, 2), (1, 7)), range((2, 2), (2, 7))],
        "{calls:?}"
    );
    client.shutdown();
}

#[test]
fn incoming_calls_group_top_level_calls_under_a_module_item() {
    let files = &[
        ("a.lua", "function greet() return 1 end\n"),
        ("b.lua", "greet()\n"),
    ];
    let client = start(files);
    let a_uri = client.uri("a.lua");
    let mut client = client;
    let item = client.prepare_call_hierarchy(&a_uri, 0, 10)[0].clone();
    let calls = client.incoming_calls(item);
    assert_eq!(calls.len(), 1, "{calls:?}");
    // A top-level call is attributed to a synthetic module item.
    assert_eq!(calls[0].from.kind, SymbolKind::MODULE);
    assert!(calls[0].from.name.ends_with("b.lua"), "{calls:?}");
    assert_eq!(calls[0].from_ranges, vec![range((0, 0), (0, 5))]);
    client.shutdown();
}

// === Protocol maturity: incremental sync =================================

#[test]
fn initialize_advertises_incremental_sync() {
    let client = start(&[]);
    // Kind 2 is `TextDocumentSyncKind::INCREMENTAL`.
    assert_eq!(client.init_result["capabilities"]["textDocumentSync"], 2);
    client.shutdown();
}

#[test]
fn incremental_ranged_insert_introduces_error_at_precise_range() {
    // Open a clean document, then splice `"no"` over the `1` in `f(1)`. The
    // resulting `f("no")` must diagnose at exactly the spliced span, proving
    // the ranged insert landed at the right byte offset.
    let client = start(&[]);
    let uri = client.uri("main.lua");
    assert!(client.open(&uri, TYPE_OK).is_empty());
    let diags = client.change_incremental(&uri, vec![ranged_change((2, 2), (2, 3), "\"no\"")]);
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(code_of(&diags[0]), "LB0300");
    assert_eq!(diags[0].range, range((2, 2), (2, 6)));
    client.shutdown();
}

#[test]
fn incremental_ranged_deletion_removes_a_whole_line() {
    // A document with an offending call on line 2 and a valid one on line 3;
    // deleting line 2 (a cross-line-boundary range) must clear the diagnostic.
    let src = "\
---@param n number
local function f(n) end
f(\"no\")
f(1)
";
    let client = start(&[]);
    let uri = client.uri("main.lua");
    assert_eq!(client.open(&uri, src).len(), 1);
    // Delete line 2 entirely: (2,0)..(3,0) → "".
    let diags = client.change_incremental(&uri, vec![ranged_change((2, 0), (3, 0), "")]);
    assert!(diags.is_empty(), "{diags:?}");
    client.shutdown();
}

#[test]
fn incremental_batch_applies_changes_in_dependent_order() {
    // Two changes in one batch: the first inserts a blank line at the top
    // (shifting everything down one line), the second edits at line 3 — a
    // coordinate that only exists *after* the first change. Applying against
    // the original text would map line 3 wrongly.
    let client = start(&[]);
    let uri = client.uri("main.lua");
    assert!(client.open(&uri, TYPE_OK).is_empty());
    let diags = client.change_incremental(
        &uri,
        vec![
            ranged_change((0, 0), (0, 0), "\n"),
            ranged_change((3, 2), (3, 3), "\"no\""),
        ],
    );
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(code_of(&diags[0]), "LB0300");
    // `f("no")` is now on line 3 (pushed down by the inserted blank line).
    assert_eq!(diags[0].range, range((3, 2), (3, 6)));
    client.shutdown();
}

#[test]
fn incremental_full_replace_swaps_whole_document() {
    // A change with no range replaces the entire buffer.
    let client = start(&[]);
    let uri = client.uri("main.lua");
    assert!(client.open(&uri, TYPE_OK).is_empty());
    let diags = client.change_incremental(
        &uri,
        vec![TextDocumentContentChangeEvent {
            range: None,
            range_length: None,
            text: TYPE_ERROR.to_string(),
        }],
    );
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(code_of(&diags[0]), "LB0300");
    client.shutdown();
}

// === Protocol maturity: work-done progress ===============================

#[test]
fn bootstrap_reports_work_done_progress_when_supported() {
    // A client that advertises window.workDoneProgress gets a begin/report/end
    // progress sequence for the startup workspace index.
    let caps = ClientCapabilities {
        window: Some(WindowClientCapabilities {
            work_done_progress: Some(true),
            ..WindowClientCapabilities::default()
        }),
        ..ClientCapabilities::default()
    };
    let client = start_with(&[("a.lua", "return {}\n")], caps);
    let kinds = client.drain_progress();
    assert_eq!(kinds.first(), Some(&"begin"), "{kinds:?}");
    assert_eq!(kinds.last(), Some(&"end"), "{kinds:?}");
    assert!(kinds.contains(&"report"), "{kinds:?}");
    client.shutdown();
}

// === Protocol maturity: didChangeConfiguration ===========================

#[test]
fn did_change_configuration_reloads_strictness() {
    // No manifest → warn. Open a type error and see a warning. Then write a
    // strict manifest and notify configuration change: the same open buffer's
    // diagnostic is republished as an error, without a restart.
    let client = start(&[]);
    let uri = client.uri("main.lua");
    let diags = client.open(&uri, TYPE_ERROR);
    assert_eq!(diags[0].severity, Some(DiagnosticSeverity::WARNING));

    let manifest = "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"5.4\"\n\n[types]\nstrict = true\n";
    std::fs::write(client.root.join("luabox.toml"), manifest).expect("write manifest");
    client.notify_config_changed();

    let diags = client.wait_diagnostics(&uri);
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].severity, Some(DiagnosticSeverity::ERROR));
    client.shutdown();
}

// === Protocol maturity: didChangeWatchedFiles ============================

#[test]
fn did_change_watched_files_reloads_external_edit() {
    // A file that is on disk but never opened; an external edit introduces an
    // error, and the watched-files notification makes the server re-read it and
    // republish diagnostics.
    let client = start(&[("mod.lua", TYPE_OK)]);
    let uri = client.uri("mod.lua");
    std::fs::write(client.root.join("mod.lua"), TYPE_ERROR).expect("rewrite mod.lua");
    client.notify_watched_change("mod.lua");
    let diags = client.wait_diagnostics(&uri);
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(code_of(&diags[0]), "LB0300");
    client.shutdown();
}

#[test]
fn did_change_watched_files_reloads_manifest() {
    // Editing `luabox.toml` externally reloads config and republishes open
    // documents — flipping strict turns an open warning into an error.
    let client = start(&[]);
    let uri = client.uri("main.lua");
    let diags = client.open(&uri, TYPE_ERROR);
    assert_eq!(diags[0].severity, Some(DiagnosticSeverity::WARNING));

    let manifest = "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"5.4\"\n\n[types]\nstrict = true\n";
    std::fs::write(client.root.join("luabox.toml"), manifest).expect("write manifest");
    client.notify::<DidChangeWatchedFiles>(DidChangeWatchedFilesParams {
        changes: vec![FileEvent {
            uri: client.uri("luabox.toml"),
            typ: FileChangeType::CHANGED,
        }],
    });

    let diags = client.wait_diagnostics(&uri);
    assert_eq!(diags[0].severity, Some(DiagnosticSeverity::ERROR));
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

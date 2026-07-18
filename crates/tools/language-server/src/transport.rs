use std::fmt;
use std::io::{BufRead, Write};

use pop_foundation::DiagnosticSeverity;
use pop_query::CancellationToken;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    DocumentAnalysis, DocumentSymbol, DocumentUri, DocumentVersion, Hover, InlayHint,
    LanguageServer, LanguageServerError, ProtocolDiagnostic, ProtocolPosition, ProtocolQuickFix,
    ProtocolRange,
};

const MAXIMUM_HEADER_BYTES: usize = 8 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TransportLimits {
    maximum_frame_bytes: usize,
    maximum_document_bytes: usize,
}

impl TransportLimits {
    #[must_use]
    pub const fn new(maximum_frame_bytes: usize, maximum_document_bytes: usize) -> Self {
        Self {
            maximum_frame_bytes,
            maximum_document_bytes,
        }
    }
}

impl Default for TransportLimits {
    fn default() -> Self {
        Self::new(8 * 1024 * 1024, 4 * 1024 * 1024)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TransportError {
    Io(String),
    HeaderTooLarge,
    MissingContentLength,
    DuplicateContentLength,
    InvalidContentLength,
    FrameTooLarge { length: usize, limit: usize },
    UnexpectedEndOfInput,
    InvalidJson(String),
}

impl fmt::Display for TransportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(detail) => write!(formatter, "language-server transport I/O failed: {detail}"),
            Self::HeaderTooLarge => formatter.write_str("language-server header is too large"),
            Self::MissingContentLength => {
                formatter.write_str("language-server frame has no Content-Length")
            }
            Self::DuplicateContentLength => {
                formatter.write_str("language-server frame repeats Content-Length")
            }
            Self::InvalidContentLength => {
                formatter.write_str("language-server Content-Length is invalid")
            }
            Self::FrameTooLarge { length, limit } => {
                write!(
                    formatter,
                    "language-server frame has {length} bytes; limit is {limit}"
                )
            }
            Self::UnexpectedEndOfInput => {
                formatter.write_str("language-server frame ended before its declared length")
            }
            Self::InvalidJson(detail) => {
                write!(formatter, "invalid language-server JSON: {detail}")
            }
        }
    }
}

impl std::error::Error for TransportError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExitStatus {
    Success,
    Failure,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Lifecycle {
    WaitingForInitialize,
    Running,
    Shutdown,
}

struct Connection {
    lifecycle: Lifecycle,
    server: Option<LanguageServer>,
    limits: TransportLimits,
}

impl Connection {
    const fn new(limits: TransportLimits) -> Self {
        Self {
            lifecycle: Lifecycle::WaitingForInitialize,
            server: None,
            limits,
        }
    }

    fn handle(&mut self, message: &Value) -> Result<ConnectionAction, TransportError> {
        if message.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return Ok(ConnectionAction::Reply(error_response(
                &message.get("id").cloned().unwrap_or(Value::Null),
                -32600,
                "Invalid Request",
            )));
        }
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            return Ok(ConnectionAction::Reply(error_response(
                &message.get("id").cloned().unwrap_or(Value::Null),
                -32600,
                "Invalid Request",
            )));
        };
        let id = message.get("id").cloned();
        let params = message.get("params").cloned().unwrap_or(Value::Null);

        if method == "exit" && id.is_none() {
            return Ok(ConnectionAction::Exit(
                if self.lifecycle == Lifecycle::Shutdown {
                    ExitStatus::Success
                } else {
                    ExitStatus::Failure
                },
            ));
        }

        if self.lifecycle == Lifecycle::Shutdown {
            return Ok(id.map_or(ConnectionAction::None, |id| {
                ConnectionAction::Reply(error_response(&id, -32600, "Invalid Request"))
            }));
        }

        match method {
            "initialize" => Ok(self.initialize(id, params)),
            "initialized" if self.lifecycle == Lifecycle::Running && id.is_none() => {
                Ok(ConnectionAction::None)
            }
            "shutdown" if self.lifecycle == Lifecycle::Running => {
                let Some(id) = id else {
                    return Ok(ConnectionAction::None);
                };
                self.lifecycle = Lifecycle::Shutdown;
                Ok(ConnectionAction::Reply(success_response(&id, &Value::Null)))
            }
            "textDocument/didOpen" if self.lifecycle == Lifecycle::Running && id.is_none() => {
                self.open(params)
            }
            "textDocument/didChange" if self.lifecycle == Lifecycle::Running && id.is_none() => {
                self.change(params)
            }
            "textDocument/didClose" if self.lifecycle == Lifecycle::Running && id.is_none() => {
                self.close(params)
            }
            "textDocument/hover" if self.lifecycle == Lifecycle::Running => self.hover(id, params),
            "textDocument/documentSymbol" if self.lifecycle == Lifecycle::Running => {
                self.document_symbols(id, params)
            }
            "textDocument/codeAction" if self.lifecycle == Lifecycle::Running => {
                self.code_actions(id, params)
            }
            "textDocument/inlayHint" if self.lifecycle == Lifecycle::Running => {
                self.inlay_hints(id, params)
            }
            "$/cancelRequest" if id.is_none() => Ok(ConnectionAction::None),
            _ => Ok(id.map_or(ConnectionAction::None, |id| {
                let code = if self.lifecycle == Lifecycle::WaitingForInitialize {
                    -32002
                } else {
                    -32601
                };
                let message = if code == -32002 {
                    "Server not initialized"
                } else {
                    "Method not found"
                };
                ConnectionAction::Reply(error_response(&id, code, message))
            })),
        }
    }

    fn initialize(&mut self, id: Option<Value>, params: Value) -> ConnectionAction {
        let Some(id) = id else {
            return ConnectionAction::None;
        };
        if self.lifecycle != Lifecycle::WaitingForInitialize {
            return ConnectionAction::Reply(error_response(&id, -32600, "Invalid Request"));
        }
        let params: InitializeParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return ConnectionAction::Reply(error_response(
                    &id,
                    -32602,
                    &format!("Invalid params: {error}"),
                ));
            }
        };
        let server = match LanguageServer::initialize(params.locale.as_deref()) {
            Ok(server) => server,
            Err(error) => {
                return ConnectionAction::Reply(error_response(
                    &id,
                    -32602,
                    &format!("Invalid params: {error}"),
                ));
            }
        };
        self.server = Some(server);
        self.lifecycle = Lifecycle::Running;
        ConnectionAction::Reply(success_response(
            &id,
            &json!({
                "capabilities": {
                    "positionEncoding": "utf-16",
                    "textDocumentSync": 1,
                    "hoverProvider": true,
                    "documentSymbolProvider": true,
                    "codeActionProvider": true,
                    "inlayHintProvider": true
                },
                "serverInfo": {
                    "name": "Pop Lang",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
        ))
    }

    fn open(&mut self, params: Value) -> Result<ConnectionAction, TransportError> {
        let params: DidOpenParams = decode_notification(params)?;
        let uri = DocumentUri::new(params.text_document.uri)
            .map_err(|error| TransportError::InvalidJson(error.to_string()))?;
        if let Some(error) = self.document_size_error(&uri, &params.text_document.text) {
            return self.log_error(&error);
        }
        let version = document_version(params.text_document.version)?;
        let server = self.server.as_mut().expect("running server");
        match server.open_with_updates(
            uri.clone(),
            version,
            params.text_document.text,
            &CancellationToken::new(),
        ) {
            Ok(updates) => Ok(ConnectionAction::Replies(
                updates
                    .iter()
                    .map(|(uri, analysis)| publish_diagnostics(uri, analysis))
                    .collect(),
            )),
            Err(error) => self.log_error(&error),
        }
    }

    fn change(&mut self, params: Value) -> Result<ConnectionAction, TransportError> {
        let params: DidChangeParams = decode_notification(params)?;
        if params.content_changes.len() != 1 || params.content_changes[0].range.is_some() {
            return Err(TransportError::InvalidJson(
                "the bootstrap server accepts exactly one full-text change".to_owned(),
            ));
        }
        let uri = DocumentUri::new(params.text_document.uri)
            .map_err(|error| TransportError::InvalidJson(error.to_string()))?;
        let text = params.content_changes.into_iter().next().unwrap().text;
        if let Some(error) = self.document_size_error(&uri, &text) {
            return self.log_error(&error);
        }
        let version = document_version(params.text_document.version)?;
        let server = self.server.as_mut().expect("running server");
        match server.change_with_updates(&uri, version, text, &CancellationToken::new()) {
            Ok(updates) => Ok(ConnectionAction::Replies(
                updates
                    .iter()
                    .map(|(uri, analysis)| publish_diagnostics(uri, analysis))
                    .collect(),
            )),
            Err(error) => self.log_error(&error),
        }
    }

    fn close(&mut self, params: Value) -> Result<ConnectionAction, TransportError> {
        let params: DidCloseParams = decode_notification(params)?;
        let uri = DocumentUri::new(params.text_document.uri)
            .map_err(|error| TransportError::InvalidJson(error.to_string()))?;
        let server = self.server.as_mut().expect("running server");
        let updates = match server.close_with_updates(&uri, &CancellationToken::new()) {
            Ok(Some(updates)) => updates,
            Ok(None) => {
                return self.log_error(&LanguageServerError::DocumentNotOpen { uri });
            }
            Err(error) => return self.log_error(&error),
        };
        let mut publications = vec![json!({
            "jsonrpc": "2.0",
            "method": "textDocument/publishDiagnostics",
            "params": {"uri": uri.as_str(), "diagnostics": []}
        })];
        publications.extend(
            updates
                .iter()
                .map(|(uri, analysis)| publish_diagnostics(uri, analysis)),
        );
        Ok(ConnectionAction::Replies(publications))
    }

    fn hover(&self, id: Option<Value>, params: Value) -> Result<ConnectionAction, TransportError> {
        let Some(id) = id else {
            return Ok(ConnectionAction::None);
        };
        let params: TextDocumentPositionParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return Ok(ConnectionAction::Reply(error_response(
                    &id,
                    -32602,
                    &format!("Invalid params: {error}"),
                )));
            }
        };
        let uri = match DocumentUri::new(params.text_document.uri) {
            Ok(uri) => uri,
            Err(error) => {
                return Ok(ConnectionAction::Reply(error_response(
                    &id,
                    -32602,
                    &error.to_string(),
                )));
            }
        };
        let position = ProtocolPosition::new(params.position.line, params.position.character);
        let server = self.server.as_ref().expect("running server");
        match server.hover(&uri, position, &CancellationToken::new()) {
            Ok(hover) => Ok(ConnectionAction::Reply(success_response(
                &id,
                &hover.map_or(Value::Null, |hover| protocol_hover(&hover)),
            ))),
            Err(error) => Ok(ConnectionAction::Reply(language_server_error(
                server, &id, &error,
            )?)),
        }
    }

    fn document_symbols(
        &self,
        id: Option<Value>,
        params: Value,
    ) -> Result<ConnectionAction, TransportError> {
        let Some(id) = id else {
            return Ok(ConnectionAction::None);
        };
        let params: DocumentSymbolParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return Ok(ConnectionAction::Reply(error_response(
                    &id,
                    -32602,
                    &format!("Invalid params: {error}"),
                )));
            }
        };
        let uri = match DocumentUri::new(params.text_document.uri) {
            Ok(uri) => uri,
            Err(error) => {
                return Ok(ConnectionAction::Reply(error_response(
                    &id,
                    -32602,
                    &error.to_string(),
                )));
            }
        };
        let server = self.server.as_ref().expect("running server");
        match server.document_symbols(&uri, &CancellationToken::new()) {
            Ok(symbols) => Ok(ConnectionAction::Reply(success_response(
                &id,
                &Value::Array(symbols.iter().map(protocol_document_symbol).collect()),
            ))),
            Err(error) => Ok(ConnectionAction::Reply(language_server_error(
                server, &id, &error,
            )?)),
        }
    }

    fn code_actions(
        &self,
        id: Option<Value>,
        params: Value,
    ) -> Result<ConnectionAction, TransportError> {
        let Some(id) = id else {
            return Ok(ConnectionAction::None);
        };
        let params: CodeActionParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return Ok(ConnectionAction::Reply(error_response(
                    &id,
                    -32602,
                    &format!("Invalid params: {error}"),
                )));
            }
        };
        let uri = match DocumentUri::new(params.text_document.uri) {
            Ok(uri) => uri,
            Err(error) => {
                return Ok(ConnectionAction::Reply(error_response(
                    &id,
                    -32602,
                    &error.to_string(),
                )));
            }
        };
        let server = self.server.as_ref().expect("running server");
        let version = match server.document_version(&uri) {
            Ok(version) => version,
            Err(error) => {
                return Ok(ConnectionAction::Reply(language_server_error(
                    server, &id, &error,
                )?));
            }
        };
        let requested_fixes = params
            .context
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic["source"] == "pop")
            .filter(|diagnostic| {
                diagnostic["data"]["documentVersion"].as_u64() == Some(version.value())
            })
            .flat_map(|diagnostic| {
                let code = diagnostic.get("code").map(protocol_code);
                diagnostic["data"]["fixIds"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .filter_map(move |fix_id| {
                        code.as_ref().map(|code| (code.clone(), fix_id.to_owned()))
                    })
            })
            .collect::<Vec<_>>();
        let range = params.range.into();
        match server.code_actions(&uri, range, &requested_fixes, &CancellationToken::new()) {
            Ok(actions) => Ok(ConnectionAction::Reply(success_response(
                &id,
                &Value::Array(
                    actions
                        .iter()
                        .map(|(code, fix)| {
                            let diagnostic = params.context.diagnostics.iter().find(|value| {
                                value["source"] == "pop"
                                    && value["data"]["documentVersion"].as_u64()
                                        == Some(version.value())
                                    && value
                                        .get("code")
                                        .is_some_and(|value| protocol_code(value) == *code)
                                    && value["data"]["fixIds"].as_array().is_some_and(|fix_ids| {
                                        fix_ids
                                            .iter()
                                            .any(|fix_id| fix_id.as_str() == Some(fix.id()))
                                    })
                            });
                            protocol_code_action(&uri, version, diagnostic, fix)
                        })
                        .collect(),
                ),
            ))),
            Err(error) => Ok(ConnectionAction::Reply(language_server_error(
                server, &id, &error,
            )?)),
        }
    }

    fn inlay_hints(
        &self,
        id: Option<Value>,
        params: Value,
    ) -> Result<ConnectionAction, TransportError> {
        let Some(id) = id else {
            return Ok(ConnectionAction::None);
        };
        let params: InlayHintParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return Ok(ConnectionAction::Reply(error_response(
                    &id,
                    -32602,
                    &format!("Invalid params: {error}"),
                )));
            }
        };
        let uri = match DocumentUri::new(params.text_document.uri) {
            Ok(uri) => uri,
            Err(error) => {
                return Ok(ConnectionAction::Reply(error_response(
                    &id,
                    -32602,
                    &error.to_string(),
                )));
            }
        };
        let server = self.server.as_ref().expect("running server");
        match server.inlay_hints(&uri, params.range.into(), &CancellationToken::new()) {
            Ok(hints) => Ok(ConnectionAction::Reply(success_response(
                &id,
                &Value::Array(hints.iter().map(protocol_inlay_hint).collect()),
            ))),
            Err(error) => Ok(ConnectionAction::Reply(language_server_error(
                server, &id, &error,
            )?)),
        }
    }

    fn document_size_error(&self, uri: &DocumentUri, text: &str) -> Option<LanguageServerError> {
        if text.len() > self.limits.maximum_document_bytes {
            return Some(LanguageServerError::DocumentTooLarge {
                uri: uri.clone(),
                length: text.len() as u64,
                limit: self.limits.maximum_document_bytes as u64,
            });
        }
        None
    }

    fn log_error(&self, error: &LanguageServerError) -> Result<ConnectionAction, TransportError> {
        let server = self.server.as_ref().expect("running server");
        let message = server
            .render_error(error)
            .map_err(|error| TransportError::InvalidJson(error.to_string()))?;
        Ok(ConnectionAction::Reply(json!({
            "jsonrpc": "2.0",
            "method": "window/logMessage",
            "params": {"type": 1, "message": message}
        })))
    }
}

enum ConnectionAction {
    None,
    Reply(Value),
    Replies(Vec<Value>),
    Exit(ExitStatus),
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InitializeParams {
    #[serde(default)]
    locale: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DidOpenParams {
    text_document: TextDocumentItem,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TextDocumentItem {
    uri: String,
    version: i64,
    text: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DidChangeParams {
    text_document: VersionedTextDocument,
    content_changes: Vec<ContentChange>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct VersionedTextDocument {
    uri: String,
    version: i64,
}

#[derive(Deserialize)]
struct ContentChange {
    #[serde(default)]
    range: Option<Value>,
    text: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DidCloseParams {
    text_document: TextDocumentIdentifier,
}

#[derive(Deserialize)]
struct TextDocumentIdentifier {
    uri: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TextDocumentPositionParams {
    text_document: TextDocumentIdentifier,
    position: Position,
}

#[derive(Clone, Copy, Deserialize)]
struct Position {
    line: u32,
    character: u32,
}

#[derive(Clone, Copy, Deserialize)]
struct Range {
    start: Position,
    end: Position,
}

impl From<Range> for ProtocolRange {
    fn from(range: Range) -> Self {
        Self {
            start: ProtocolPosition::new(range.start.line, range.start.character),
            end: ProtocolPosition::new(range.end.line, range.end.character),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodeActionParams {
    text_document: TextDocumentIdentifier,
    range: Range,
    context: CodeActionContext,
}

#[derive(Deserialize)]
struct CodeActionContext {
    #[serde(default)]
    diagnostics: Vec<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InlayHintParams {
    text_document: TextDocumentIdentifier,
    range: Range,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DocumentSymbolParams {
    text_document: TextDocumentIdentifier,
}

fn decode_notification<T: for<'de> Deserialize<'de>>(params: Value) -> Result<T, TransportError> {
    serde_json::from_value(params).map_err(|error| TransportError::InvalidJson(error.to_string()))
}

fn protocol_code(value: &Value) -> String {
    value
        .as_str()
        .map_or_else(|| value.to_string(), str::to_owned)
}

fn document_version(version: i64) -> Result<DocumentVersion, TransportError> {
    u64::try_from(version)
        .map(DocumentVersion::new)
        .map_err(|_| TransportError::InvalidJson("document version must be nonnegative".to_owned()))
}

fn publish_diagnostics(uri: &DocumentUri, analysis: &DocumentAnalysis) -> Value {
    let diagnostics: Vec<_> = analysis
        .diagnostics()
        .iter()
        .map(|diagnostic| protocol_diagnostic(uri, analysis.version(), diagnostic))
        .collect();
    json!({
        "jsonrpc": "2.0",
        "method": "textDocument/publishDiagnostics",
        "params": {
            "uri": uri.as_str(),
            "version": analysis.version().value(),
            "diagnostics": diagnostics
        }
    })
}

fn protocol_diagnostic(
    uri: &DocumentUri,
    version: DocumentVersion,
    diagnostic: &ProtocolDiagnostic,
) -> Value {
    let range = diagnostic.range();
    let related_information = diagnostic
        .related()
        .iter()
        .map(|related| {
            json!({
                "location": {
                    "uri": uri.as_str(),
                    "range": protocol_range(related.range())
                },
                "message": related.message()
            })
        })
        .collect::<Vec<_>>();
    let fix_ids = diagnostic
        .fixes()
        .iter()
        .map(ProtocolQuickFix::id)
        .collect::<Vec<_>>();
    json!({
        "range": {
            "start": {
                "line": range.start().line(),
                "character": range.start().character()
            },
            "end": {
                "line": range.end().line(),
                "character": range.end().character()
            }
        },
        "severity": match diagnostic.severity() {
            DiagnosticSeverity::Error => 1,
            DiagnosticSeverity::Warning => 2,
            DiagnosticSeverity::Information => 3,
            DiagnosticSeverity::Hint => 4,
        },
        "code": diagnostic.code(),
        "source": "pop",
        "message": diagnostic.message(),
        "relatedInformation": related_information,
        "data": {
            "category": diagnostic_category(diagnostic.category()),
            "documentVersion": version.value(),
            "warningWave": diagnostic.warning_wave(),
            "suppressionKey": diagnostic.suppression_key(),
            "notes": diagnostic.notes(),
            "fixIds": fix_ids
        }
    })
}

const fn diagnostic_category(category: pop_foundation::DiagnosticCategory) -> &'static str {
    use pop_foundation::DiagnosticCategory;
    match category {
        DiagnosticCategory::Syntax => "Syntax",
        DiagnosticCategory::Resolution => "Resolution",
        DiagnosticCategory::Type => "Type",
        DiagnosticCategory::Flow => "Flow",
        DiagnosticCategory::CompileTime => "CompileTime",
        DiagnosticCategory::RuntimeSafety => "RuntimeSafety",
        DiagnosticCategory::Style => "Style",
        DiagnosticCategory::Backend => "Backend",
        DiagnosticCategory::Project => "Project",
        DiagnosticCategory::Tooling => "Tooling",
    }
}

fn protocol_code_action(
    uri: &DocumentUri,
    version: DocumentVersion,
    diagnostic: Option<&Value>,
    fix: &ProtocolQuickFix,
) -> Value {
    json!({
        "title": fix.title(),
        "kind": "quickfix",
        "isPreferred": fix.is_safe(),
        "diagnostics": diagnostic.into_iter().collect::<Vec<_>>(),
        "edit": {
            "documentChanges": [{
                "textDocument": {"uri": uri.as_str(), "version": version.value()},
                "edits": fix.edits().iter().map(|edit| json!({
                    "range": protocol_range(edit.range()),
                    "newText": edit.replacement()
                })).collect::<Vec<_>>()
            }]
        },
        "data": {"fixId": fix.id()}
    })
}

fn protocol_inlay_hint(hint: &InlayHint) -> Value {
    json!({
        "position": {
            "line": hint.position().line(),
            "character": hint.position().character()
        },
        "label": hint.label(),
        "kind": 2,
        "paddingRight": true
    })
}

fn protocol_hover(hover: &Hover) -> Value {
    let mut value = hover.signature().to_owned();
    if let Some(summary) = hover.summary() {
        value.push_str("\n\n");
        value.push_str(summary);
    }
    json!({
        "contents": {"kind": "plaintext", "value": value},
        "range": protocol_range(hover.range())
    })
}

fn protocol_document_symbol(symbol: &DocumentSymbol) -> Value {
    json!({
        "name": symbol.name(),
        "detail": symbol.kind(),
        "kind": symbol_kind(symbol.kind()),
        "range": protocol_range(symbol.range()),
        "selectionRange": protocol_range(symbol.selection_range())
    })
}

fn symbol_kind(kind: &str) -> u8 {
    match kind {
        "function" => 12,
        "constant" => 14,
        "record" => 23,
        "union" | "error" | "enum" => 10,
        "interface" => 11,
        "class" | "attribute" => 5,
        "type alias" => 26,
        _ => 13,
    }
}

fn protocol_range(range: ProtocolRange) -> Value {
    json!({
        "start": {"line": range.start().line(), "character": range.start().character()},
        "end": {"line": range.end().line(), "character": range.end().character()}
    })
}

fn language_server_error(
    server: &LanguageServer,
    id: &Value,
    error: &LanguageServerError,
) -> Result<Value, TransportError> {
    let message = server
        .render_error(error)
        .map_err(|error| TransportError::InvalidJson(error.to_string()))?;
    let code = if matches!(error, LanguageServerError::Cancelled) {
        -32800
    } else {
        -32602
    };
    Ok(error_response(id, code, &message))
}

fn success_response(id: &Value, result: &Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn error_response(id: &Value, code: i32, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn read_message<R: BufRead>(
    reader: &mut R,
    limits: TransportLimits,
) -> Result<Option<Value>, TransportError> {
    let mut content_length = None;
    let mut header_bytes: usize = 0;
    loop {
        let mut line = Vec::new();
        let read = reader
            .read_until(b'\n', &mut line)
            .map_err(|error| TransportError::Io(error.to_string()))?;
        if read == 0 {
            return if header_bytes == 0 {
                Ok(None)
            } else {
                Err(TransportError::UnexpectedEndOfInput)
            };
        }
        header_bytes = header_bytes
            .checked_add(read)
            .ok_or(TransportError::HeaderTooLarge)?;
        if header_bytes > MAXIMUM_HEADER_BYTES {
            return Err(TransportError::HeaderTooLarge);
        }
        if line == b"\r\n" || line == b"\n" {
            break;
        }
        let line = std::str::from_utf8(&line)
            .map_err(|error| TransportError::InvalidJson(error.to_string()))?;
        let line = line.trim_end_matches(['\r', '\n']);
        let Some((name, value)) = line.split_once(':') else {
            return Err(TransportError::InvalidContentLength);
        };
        if name.eq_ignore_ascii_case("Content-Length") {
            if content_length.is_some() {
                return Err(TransportError::DuplicateContentLength);
            }
            content_length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .map_err(|_| TransportError::InvalidContentLength)?,
            );
        }
    }
    let length = content_length.ok_or(TransportError::MissingContentLength)?;
    if length > limits.maximum_frame_bytes {
        return Err(TransportError::FrameTooLarge {
            length,
            limit: limits.maximum_frame_bytes,
        });
    }
    let mut body = vec![0; length];
    reader
        .read_exact(&mut body)
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::UnexpectedEof => TransportError::UnexpectedEndOfInput,
            _ => TransportError::Io(error.to_string()),
        })?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|error| TransportError::InvalidJson(error.to_string()))
}

fn write_message<W: Write>(writer: &mut W, message: &Value) -> Result<(), TransportError> {
    let body = serde_json::to_vec(message)
        .map_err(|error| TransportError::InvalidJson(error.to_string()))?;
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())
        .map_err(|error| TransportError::Io(error.to_string()))?;
    writer
        .write_all(&body)
        .and_then(|()| writer.flush())
        .map_err(|error| TransportError::Io(error.to_string()))
}

/// Serves one bounded LSP stdio-style connection until `exit` or input closes.
///
/// # Errors
///
/// Returns a transport error for malformed framing, malformed JSON, or I/O
/// failure. JSON-RPC request errors are returned to the client without ending
/// the connection.
pub fn serve<R: BufRead, W: Write>(
    mut reader: R,
    mut writer: W,
    limits: TransportLimits,
) -> Result<ExitStatus, TransportError> {
    let mut connection = Connection::new(limits);
    while let Some(message) = read_message(&mut reader, limits)? {
        match connection.handle(&message)? {
            ConnectionAction::None => {}
            ConnectionAction::Reply(reply) => write_message(&mut writer, &reply)?,
            ConnectionAction::Replies(replies) => {
                for reply in replies {
                    write_message(&mut writer, &reply)?;
                }
            }
            ConnectionAction::Exit(status) => return Ok(status),
        }
    }
    Ok(ExitStatus::Failure)
}

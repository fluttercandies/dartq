//! The typed minimal LSP surface: only the structures this milestone's
//! methods actually consume or emit (LSP 3.17). Field names are pinned by
//! the spec via `camelCase` renames; everything else about the protocol
//! stays untyped [`serde_json::Value`].

use serde::{Deserialize, Serialize};

/// Zero-based line and UTF-16 code-unit column (LSP §Position — the same
/// UTF-16 convention dartq spans already use, PRD §6/§7/§10.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

/// Half-open [`Position`] range (LSP §Range).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

/// `textDocument/didOpen` payload (LSP [`§DidOpenTextDocumentParams`]).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DidOpenParams {
    pub text_document: TextDocumentItem,
}

/// One opened document: identity, client version, full text.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextDocumentItem {
    pub uri: String,
    pub language_id: String,
    pub version: i64,
    pub text: String,
}

/// `textDocument/didChange` payload (LSP [`§DidChangeTextDocumentParams`]).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DidChangeParams {
    pub text_document: VersionedTextDocumentIdentifier,
    pub content_changes: Vec<TextDocumentContentChangeEvent>,
}

/// Identity plus the client's monotonically increasing version counter.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionedTextDocumentIdentifier {
    pub uri: String,
    pub version: i64,
}

/// One incremental edit description. Under full sync (`textDocumentSync: 1`,
/// the only mode this server negotiates) `range` must be absent and `text`
/// is the whole document; a present `range` marks a full-sync violation and
/// the server skips the event.
#[derive(Debug, Clone, Deserialize)]
pub struct TextDocumentContentChangeEvent {
    pub range: Option<Range>,
    pub text: String,
}

/// `textDocument/didClose` payload (LSP [`§DidCloseTextDocumentParams`]).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DidCloseParams {
    pub text_document: TextDocumentIdentifier,
}

/// Document identity without a version.
#[derive(Debug, Clone, Deserialize)]
pub struct TextDocumentIdentifier {
    pub uri: String,
}

/// One `textDocument/publishDiagnostics` entry (LSP [`§Diagnostic`]). Only the
/// fields dartq produces are modeled; the field set is closed on purpose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    pub range: Range,
    /// 1 Error · 2 Warning · 3 Information (LSP [`§DiagnosticSeverity`]).
    pub severity: u8,
    /// The stable dartq code name (`snake_case`, PRD §13.2).
    pub code: String,
    pub source: String,
    pub message: String,
}

/// `textDocument/publishDiagnostics` payload (LSP [`§PublishDiagnosticsParams`]).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishDiagnosticsParams {
    pub uri: String,
    pub version: Option<i64>,
    pub diagnostics: Vec<Diagnostic>,
}

/// One `dartq/analyze` entry: the LSP diagnostic flattened next to its
/// document URI (the pull face returns the whole workspace, so every entry
/// must carry its document).
#[derive(Debug, Clone, Serialize)]
pub struct AnalyzeEntry {
    pub uri: String,
    #[serde(flatten)]
    pub diagnostic: Diagnostic,
}

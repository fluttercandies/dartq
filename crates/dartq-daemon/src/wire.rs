//! The typed wire-shape vocabulary of the daemon's protocol surface: the
//! value shapes (position, range, diagnostic) the diagnostic rows and
//! plugin findings carry, the `dartq/diagnostics` push params, and the
//! `dartq/analyze` pull face — its entry type, the engine-diagnostic
//! conversion, and the frozen v1 row/envelope builders. Positions are
//! zero-based lines with UTF-16 code-unit columns — the same UTF-16
//! convention dartq spans already use; everything else about the protocol
//! stays untyped [`serde_json::Value`].

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::commands::SCHEMA_VERSION;
use dartq_diagnostics::Severity;
use dartq_engine::Engine;

/// `dartq/diagnostics` push payload: the file's whole current diagnostic
/// set (a push replaces the client's set for that URI — an empty array
/// clears it).
#[derive(Debug, Clone, Serialize)]
pub struct DiagnosticsParams {
    pub uri: String,
    pub diagnostics: Vec<Diagnostic>,
}

/// Zero-based line and UTF-16 code-unit column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

/// Half-open [`Position`] range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

/// One `dartq/diagnostics` entry. Only the fields dartq produces are
/// modeled; the field set is closed on purpose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    pub range: Range,
    /// 1 Error · 2 Warning · 3 Information.
    pub severity: u8,
    /// The stable dartq code name (`snake_case`).
    pub code: String,
    pub source: String,
    pub message: String,
}

/// One `dartq/analyze` entry: the diagnostic flattened next to its
/// document URI (the pull face returns the whole workspace, so every entry
/// must carry its document).
#[derive(Debug, Clone, Serialize)]
pub struct AnalyzeEntry {
    pub uri: String,
    #[serde(flatten)]
    pub diagnostic: Diagnostic,
}

/// Converts one engine diagnostic to the wire shape over the engine's read
/// faces (the read task runs exactly one pass per view — the conversion is
/// the expensive half). Returns `None` only for a non-resident file, which
/// the engine's "live libraries only" contract excludes.
///
/// # Panics
///
/// Panics when the diagnostic range overflows `u32` or the engine content
/// is not UTF-8 — both internal invariant breaches (content is validated
/// at `set_content` time).
pub fn convert(engine: &Engine, diagnostic: &dartq_diagnostics::Diagnostic) -> Option<Diagnostic> {
    let index = engine.line_index(diagnostic.file)?;
    let bytes = engine.content(diagnostic.file)?;
    let text = std::str::from_utf8(bytes).expect("engine content is valid UTF-8");
    // `line_col_utf16` answers the oracle's 1-based (line, column); wire
    // positions are zero-based.
    let end_offset = diagnostic
        .range
        .offset
        .checked_add(diagnostic.range.length)
        .expect("diagnostic range overflows u32");
    let (start_line, start_column) = index.line_col_utf16(text, diagnostic.range.offset);
    let (end_line, end_column) = index.line_col_utf16(text, end_offset);
    let severity = match diagnostic.severity {
        Severity::Error => 1,
        Severity::Warning => 2,
        Severity::Info => 3,
    };
    Some(Diagnostic {
        range: Range {
            start: Position {
                line: start_line - 1,
                character: start_column - 1,
            },
            end: Position {
                line: end_line - 1,
                character: end_column - 1,
            },
        },
        severity,
        code: diagnostic.code.name.to_owned(),
        source: "dartq".into(),
        message: diagnostic.message.clone().into_owned(),
    })
}

/// One compact/full `dartq/analyze` row: the entry's own serialization —
/// the single source of the row bytes the pull face serves (the minimal
/// tier builds its reduced row at the call site, `analyze_answer` in the
/// commands module).
///
/// # Panics
///
/// Panics when the entry serialization fails — an internal invariant
/// breach (`AnalyzeEntry` is schema-pinned and always serializes).
#[must_use]
pub fn row(entry: &AnalyzeEntry) -> Value {
    serde_json::to_value(entry).expect("analyze entry serializes")
}

/// The frozen v1 envelope: the version stamp over the rows. `symbols` is
/// the full-tier session economics and stays absent here (the published
/// schema declares it absent-tolerant); the commands module's
/// `session_answer` adds it at the full tier.
#[must_use]
pub fn envelope(rows: &[Value]) -> Value {
    json!({ "schema_version": SCHEMA_VERSION, "diagnostics": rows })
}

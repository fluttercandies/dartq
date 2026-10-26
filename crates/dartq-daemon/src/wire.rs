//! The typed wire-shape vocabulary of the daemon's protocol surface: the
//! value shapes (position, range, diagnostic) the diagnostic rows and the
//! plugin findings carry, the `dartq/diagnostics` push params, and the
//! `dartq/analyze` pull face — its entry type, the engine-diagnostic
//! conversion, and the frozen v1 row/envelope builders. Push positions are
//! zero-based lines with UTF-16 code-unit columns — the same UTF-16
//! convention dartq spans already use; the pull rows flatten to the
//! compact vocabulary (`code`/`sev`/`file`/`line`/`col`/`len`/`msg`,
//! 1-based position, lowercase severity word). Everything else about the
//! protocol stays untyped [`serde_json::Value`].

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
pub struct Diagnostic {
    pub range: Range,
    /// 1 Error · 2 Warning · 3 Information.
    pub severity: u8,
    /// The stable dartq code name (`snake_case`).
    pub code: String,
    pub source: String,
    pub message: String,
}

/// The compact vocabulary's severity word: 1 `error` · 2 `warning` ·
/// 3 `info` (the pull rows' `sev` value; the push face keeps its 1/2/3
/// scale).
#[must_use]
pub fn severity_word(severity: u8) -> &'static str {
    match severity {
        1 => "error",
        2 => "warning",
        _ => "info",
    }
}

/// One `dartq/analyze` entry: the diagnostic flattened next to its
/// document URI (the pull face returns the whole workspace, so every entry
/// must carry its document) plus the span's UTF-16 length — the quantity
/// the flattened row's `len` carries, which the zero-based position pair
/// alone cannot recover for a multi-line span.
#[derive(Debug, Clone)]
pub struct AnalyzeEntry {
    pub uri: String,
    pub diagnostic: Diagnostic,
    /// `range.end − range.start` in UTF-16 code units; carried for the
    /// pull row builder, never serialized.
    pub len: u32,
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

/// One compact/full `dartq/analyze` row: the entry flattened to the
/// compact vocabulary — `{code, sev, file, line, col, len, msg}` — the
/// single source of the row bytes the pull face serves (the minimal tier
/// builds its reduced row at the call site, `analyze_answer` in the
/// commands module). `line`/`col` are 1-based (the push face's zero-based
/// convention plus one); `doc` (the diagnostic's documentation) has no
/// daemon-side source and the key is omitted entirely when absent.
#[must_use]
pub fn row(entry: &AnalyzeEntry) -> Value {
    let diagnostic = &entry.diagnostic;
    json!({
        "code": diagnostic.code,
        "sev": severity_word(diagnostic.severity),
        "file": entry.uri,
        "line": diagnostic.range.start.line + 1,
        "col": diagnostic.range.start.character + 1,
        "len": entry.len,
        "msg": diagnostic.message,
    })
}

/// The frozen v1 envelope: the version stamp over the rows. `symbols` is
/// the full-tier session economics and stays absent here (the published
/// schema declares it absent-tolerant); the commands module's
/// `session_answer` adds it at the full tier.
#[must_use]
pub fn envelope(rows: &[Value]) -> Value {
    json!({ "v": SCHEMA_VERSION, "diags": rows })
}

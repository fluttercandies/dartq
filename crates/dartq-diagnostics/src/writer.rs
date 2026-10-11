//! Output writers reproducing `dart analyze --format=machine` and
//! `--format=json` field-for-field and byte-for-byte (oracle facts §1/§2).
//!
//! With no diagnostics the machine writer emits nothing at all, while the
//! JSON writer still prints the bare `{"version":1,"diagnostics":[]}` line —
//! both matching the oracle (only the human-readable default differs).

use std::io::{self, Write};

use serde::Serialize;

use crate::code::CodeInfo;
use crate::model::{AnalysisOutput, Diagnostic};

/// Writes diagnostics in the oracle machine format, one line per diagnostic:
///
/// ```text
/// SEVERITY|TYPE|CODE|FILE_PATH|LINE|COLUMN|LENGTH|MESSAGE
/// ```
///
/// Field order, 1-based line/column, UTF-16 length, uppercase severity/type/
/// code, and the `\`-escaping of the `FILE_PATH` and `MESSAGE` fields all
/// match `emitMachineFormat` in the oracle's `analyze.dart` exactly.
///
/// # Errors
/// Propagates any I/O error surfaced by `out`.
pub fn write_machine(out: &mut impl Write, output: &AnalysisOutput<'_>) -> io::Result<()> {
    use std::fmt::Write as _;

    let mut field = String::new();
    for d in output.diagnostics {
        let (line, column) = output.files.line_col(d.file, d.range.offset);
        // The whole row is assembled in the reused buffer and flushed with a
        // single `write_all` — no per-field format machinery on the I/O path.
        field.clear();
        field.push_str(d.severity.as_machine_str());
        field.push('|');
        field.push_str(d.code.dtype.as_str());
        field.push('|');
        field.push_str(d.code.machine);
        field.push('|');
        escape_into(
            output.files.file_path(d.file).to_string_lossy().as_ref(),
            &mut field,
        );
        // Infallible: `String`'s `fmt::Write` never fails.
        let _ = write!(field, "|{line}|{column}|{}", d.range.length);
        field.push('|');
        escape_into(&d.message, &mut field);
        field.push('\n');
        out.write_all(field.as_bytes())?;
    }
    Ok(())
}

/// Writes diagnostics in the oracle JSON format:
///
/// ```json
/// {"version":1,"diagnostics":[...]}
/// ```
///
/// Single line, compact, key order and casing identical to the oracle
/// (`code` lowercase, `severity`/`type` uppercase). `correctionMessage` is
/// emitted only when present; `documentation` is emitted only for official
/// codes with a published docs page (`hasPublishedDocs: true` in the
/// analyzer registry — `errors.dart` `url` getter), as
/// `https://dart.dev/diagnostics/<code>`; unpublished codes (e.g.
/// `expected_token`) and `dartq/`-namespace codes omit the key. The
/// envelope is always emitted, even with no diagnostics, and the output
/// always ends with a newline — the oracle routes every format through
/// `writeln` (measured: empty set → exactly `{"version":1,"diagnostics":[]}\n`).
///
/// Serialization goes through [`JsonDiagnostic`] so `serde_json` owns the
/// string escaping; its escape table is byte-identical to Dart's
/// `jsonEncode` for every character the writers can encounter (short escapes
/// for the common controls, lowercase `\u00xx` for the remaining C0 bytes,
/// DEL and non-ASCII verbatim as UTF-8), which the goldens pin.
///
/// # Errors
/// Propagates any I/O error surfaced by `out`.
pub fn write_json(out: &mut impl Write, output: &AnalysisOutput<'_>) -> io::Result<()> {
    out.write_all(b"{\"version\":1,\"diagnostics\":[")?;
    // Reused across diagnostics: the one buffer `documentation_url` builds its
    // URL in, so published codes cost no per-diagnostic allocation.
    let mut url = String::new();
    for (i, d) in output.diagnostics.iter().enumerate() {
        if i > 0 {
            out.write_all(b",")?;
        }
        write_json_diagnostic(out, output, d, &mut url)?;
    }
    out.write_all(b"]}\n")
}

/// The one-diagnostic JSON object. Field declaration order is the oracle's
/// key order — `serde` serializes struct fields in declaration order.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct JsonDiagnostic<'a> {
    code: &'a str,
    severity: &'a str,
    #[serde(rename = "type")]
    diagnostic_type: &'a str,
    location: JsonLocation<'a>,
    problem_message: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    correction_message: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    documentation: Option<&'a str>,
}

#[derive(Serialize)]
struct JsonLocation<'a> {
    file: &'a str,
    range: JsonRange,
}

#[derive(Serialize)]
struct JsonRange {
    start: JsonPosition,
    end: JsonPosition,
}

#[derive(Serialize)]
struct JsonPosition {
    offset: u32,
    line: u32,
    column: u32,
}

/// Streams one diagnostic's JSON object straight to `out` (`to_writer` — no
/// intermediate `Vec`, byte-identical to `serde_json::to_vec`). `url` is the
/// caller's reused buffer [`documentation_url`] builds into.
fn write_json_diagnostic(
    out: &mut impl Write,
    output: &AnalysisOutput<'_>,
    d: &Diagnostic,
    url: &mut String,
) -> serde_json::Result<()> {
    let (start_line, start_column) = output.files.line_col(d.file, d.range.offset);
    let (end_line, end_column) = output.files.line_col(d.file, d.range.end());
    let file = output.files.file_path(d.file).to_string_lossy();
    let diagnostic = JsonDiagnostic {
        code: d.code.name,
        severity: d.severity.as_machine_str(),
        diagnostic_type: d.code.dtype.as_str(),
        location: JsonLocation {
            file: &file,
            range: JsonRange {
                start: JsonPosition {
                    offset: d.range.offset,
                    line: start_line,
                    column: start_column,
                },
                end: JsonPosition {
                    offset: d.range.end(),
                    line: end_line,
                    column: end_column,
                },
            },
        },
        problem_message: &d.message,
        correction_message: d.correction.as_deref(),
        documentation: documentation_url(d.code, url),
    };
    serde_json::to_writer(out, &diagnostic)
}

/// The oracle emits `documentation` only for codes its registry marks
/// `hasPublishedDocs: true` (`errors.dart` `url` getter: published →
/// `https://dart.dev/diagnostics/<name>`, else `null`); `dartq/`-namespace
/// codes are never published (pinned by the registry tests in `code.rs`), so
/// the flag alone decides. The URL is built in the caller's reused buffer and
/// returned borrowed.
fn documentation_url(code: CodeInfo, buf: &mut String) -> Option<&str> {
    if code.has_published_docs {
        buf.clear();
        buf.push_str("https://dart.dev/diagnostics/");
        buf.push_str(code.name);
        Some(buf.as_str())
    } else {
        None
    }
}

/// Machine-format field escaping, equivalent to the oracle's
/// `_escapeForMachineMode`: `\` → `\\`, `|` → `\|`, newline → literal `\n`,
/// CR → literal `\r`; everything else verbatim.
///
/// Byte-oriented: the four special bytes are ASCII, so every slice between
/// them starts and ends on a char boundary and is copied verbatim — non-ASCII
/// text passes through as raw UTF-8 without per-char decode/re-encode.
fn escape_into(text: &str, out: &mut String) {
    let bytes = text.as_bytes();
    let mut copied = 0;
    for (i, &b) in bytes.iter().enumerate() {
        let escaped = match b {
            b'\\' => "\\\\",
            b'|' => "\\|",
            b'\n' => "\\n",
            b'\r' => "\\r",
            _ => continue,
        };
        out.push_str(&text[copied..i]);
        out.push_str(escaped);
        copied = i + 1;
    }
    out.push_str(&text[copied..]);
}

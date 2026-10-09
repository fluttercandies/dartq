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
    let mut field = String::new();
    for d in output.diagnostics {
        let (line, column) = output.files.line_col(d.file, d.range.offset);
        field.clear();
        escape_into(
            output.files.file_path(d.file).to_string_lossy().as_ref(),
            &mut field,
        );
        write!(
            out,
            "{}|{}|{}|{}|{line}|{column}|{}|",
            d.severity.as_machine_str(),
            d.code.dtype.as_str(),
            d.code.machine,
            field,
            d.range.length,
        )?;
        field.clear();
        escape_into(&d.message, &mut field);
        writeln!(out, "{field}")?;
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
/// `expected_token`) and `dartiq/`-namespace codes omit the key. The
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
    for (i, d) in output.diagnostics.iter().enumerate() {
        if i > 0 {
            out.write_all(b",")?;
        }
        out.write_all(&json_diagnostic_bytes(output, d)?)?;
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
    documentation: Option<String>,
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

fn json_diagnostic_bytes(
    output: &AnalysisOutput<'_>,
    d: &Diagnostic,
) -> serde_json::Result<Vec<u8>> {
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
        documentation: documentation_url(d.code),
    };
    serde_json::to_vec(&diagnostic)
}

/// The oracle emits `documentation` only for codes its registry marks
/// `hasPublishedDocs: true` (`errors.dart` `url` getter: published →
/// `https://dart.dev/diagnostics/<name>`, else `null`); `dartiq/`-namespace
/// codes are never published, so they carry no URL either way.
fn documentation_url(code: CodeInfo) -> Option<String> {
    if code.has_published_docs && !code.name.starts_with("dartiq/") {
        Some(format!("https://dart.dev/diagnostics/{}", code.name))
    } else {
        None
    }
}

/// Machine-format field escaping, equivalent to the oracle's
/// `_escapeForMachineMode`: `\` → `\\`, `|` → `\|`, newline → literal `\n`,
/// CR → literal `\r`; everything else verbatim.
fn escape_into(text: &str, out: &mut String) {
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '|' => out.push_str("\\|"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
}

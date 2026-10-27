//! Output writers reproducing `dart analyze --format=machine` and
//! `--format=json` field-for-field and byte-for-byte.
//!
//! With no diagnostics the machine writer emits nothing at all, while the
//! JSON writer still prints the bare `{"version":1,"diagnostics":[]}` line —
//! both matching the oracle (only the human-readable default differs).

use std::borrow::Cow;
use std::io::{self, Write};

use serde::Serialize;

use crate::code::CodeInfo;
use crate::model::{
    AnalysisOutput, ContextMessage, Diagnostic, DiagnosticType, FileId, SourceRange,
};

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
/// emitted only when present; `documentation` is emitted for every
/// first-party lint code (the published rules' diagnostics page, the rest
/// the `https://dart.dev/lints/<rule>` page) and for official analyzer
/// codes with a published docs page (`hasPublishedDocs: true` in the
/// analyzer registry — `errors.dart` `url` getter) as
/// `https://dart.dev/diagnostics/<code>`; unpublished analyzer codes
/// (e.g. `expected_token`), plugin lint codes, and `dartq/`-namespace
/// codes omit the key. The envelope is always emitted, even with no
/// diagnostics, and the output always ends with a newline — the oracle
/// routes every format through `writeln` (measured: empty set → exactly
/// `{"version":1,"diagnostics":[]}\n`).
///
/// Serialization goes through `JsonDiagnostic` so `serde_json` owns the
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
/// key order — `serde` serializes struct fields in declaration order:
/// `code`, `severity`, `type`, `location`, `problemMessage`,
/// `correctionMessage`, `contextMessages`, `documentation` (upstream Dart
/// SDK source, `analyze.dart` `emitJsonFormat` map literal :549-572).
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
    context_messages: Option<Vec<JsonContextMessage<'a>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    documentation: Option<&'a str>,
}

/// One `contextMessages` element: `{location: {file, range}, message}` —
/// the oracle's element shape, location first (upstream `analyze.dart`
/// `emitJsonFormat` :519-547).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct JsonContextMessage<'a> {
    location: JsonLocation<'a>,
    message: &'a str,
}

#[derive(Serialize)]
struct JsonLocation<'a> {
    file: Cow<'a, str>,
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
    let diagnostic = JsonDiagnostic {
        code: d.code.name,
        severity: d.severity.as_machine_str(),
        diagnostic_type: d.code.dtype.as_str(),
        location: json_location(output, d.file, d.range),
        problem_message: &d.message,
        correction_message: d.correction.as_deref(),
        context_messages: d.context.as_deref().map(|context| {
            context
                .iter()
                .map(|m| json_context_message(output, m))
                .collect()
        }),
        documentation: documentation_url(d.code, url),
    };
    serde_json::to_writer(out, &diagnostic)
}

/// One `{file, range}` pair resolved through the layout provider — the
/// shape the diagnostic's own location and every context-message location
/// share (upstream `locationJson`/`rangeJson`/`positionJson`,
/// `analyze.dart` `emitJsonFormat` :501-513). `to_string_lossy` gives the
/// `Cow` directly: borrowed for UTF-8-clean paths, owned otherwise.
fn json_location<'a>(
    output: &'a AnalysisOutput<'_>,
    file_id: FileId,
    range: SourceRange,
) -> JsonLocation<'a> {
    let (start_line, start_column) = output.files.line_col(file_id, range.offset);
    let (end_line, end_column) = output.files.line_col(file_id, range.end());
    JsonLocation {
        file: output.files.file_path(file_id).to_string_lossy(),
        range: JsonRange {
            start: JsonPosition {
                offset: range.offset,
                line: start_line,
                column: start_column,
            },
            end: JsonPosition {
                offset: range.end(),
                line: end_line,
                column: end_column,
            },
        },
    }
}

/// One context-message element in the oracle's shape: location object
/// first, message second (`analyze.dart` `emitJsonFormat` :530-543).
fn json_context_message<'a>(
    output: &'a AnalysisOutput<'_>,
    m: &'a ContextMessage,
) -> JsonContextMessage<'a> {
    JsonContextMessage {
        location: json_location(output, m.file, m.range),
        message: &m.message,
    }
}

/// The oracle emits `documentation` straight from the code's URL
/// (`analyze.dart` `emitJsonFormat`: `'documentation': ?url`):
///
/// * analyzer codes — `errors.dart`'s `url` getter: `null` unless the
///   registry marks the code `hasPublishedDocs: true`, then
///   `https://dart.dev/diagnostics/<name>`; `dartq/`-namespace codes are
///   never published (pinned by the registry tests in `code.rs`), so the
///   flag alone decides;
/// * lint codes — the linter's `LinterLintCode.url` (upstream Dart SDK
///   source, `pkg/linter/lib/src/lint_codes.dart`): every first-party
///   lint code carries a URL — the diagnostics page for published codes,
///   the `https://dart.dev/lints/<name>` page otherwise. The published
///   bit is read by code name from the captured first-party table
///   ([`crate::lint_docs`]), never from the row's `has_published_docs`
///   flag: that flag is persisted per row (the content store's
///   `code_published`) and would serve stale URLs from pre-capture
///   stores; the name-keyed table is stable across warm and cold rows.
///   A lint-typed code the table does not carry is a plugin code — the
///   analyzer-side `LintCode.url` is null, so no URL.
///
/// The URL is built in the caller's reused buffer and returned borrowed.
fn documentation_url(code: CodeInfo, buf: &mut String) -> Option<&str> {
    let prefix = match code.dtype {
        DiagnosticType::Lint => crate::lint_docs::lint_doc_prefix(code.name)?,
        _ if code.has_published_docs => "https://dart.dev/diagnostics/",
        _ => return None,
    };
    buf.clear();
    buf.push_str(prefix);
    buf.push_str(code.name);
    Some(buf.as_str())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::FileLayoutProvider;
    use std::path::Path;

    /// The JSON face's `documentation` decisions (the oracle's
    /// `documentation: ?url` face): a first-party published lint takes the
    /// diagnostics page, an unpublished first-party lint takes the lint
    /// page, a plugin lint code and an unpublished analyzer code omit the
    /// key, and a published analyzer code keeps the diagnostics page.
    #[test]
    fn json_documentation_decides_by_code_family() {
        struct OneFile;
        impl FileLayoutProvider for OneFile {
            fn file_path(&self, _file: FileId) -> &Path {
                Path::new("/w/lib/a.dart")
            }
            fn line_col(&self, _file: FileId, _offset: u32) -> (u32, u32) {
                (1, 1)
            }
        }
        let files = OneFile;
        let mut table = dartq_source::FileTable::new();
        let id = table.intern(Path::new("/w/lib/a.dart"));
        let row = |code: CodeInfo| {
            Diagnostic::new(
                code.dtype.severity(),
                code,
                "m",
                id,
                SourceRange {
                    offset: 0,
                    length: 1,
                },
                None,
            )
        };
        let diagnostics = [
            row(CodeInfo::new(
                "avoid_empty_else",
                "AVOID_EMPTY_ELSE",
                DiagnosticType::Lint,
                false,
            )),
            row(CodeInfo::new(
                "always_require_non_null_named_parameters",
                "ALWAYS_REQUIRE_NON_NULL_NAMED_PARAMETERS",
                DiagnosticType::Lint,
                false,
            )),
            row(CodeInfo::new(
                "plug_lint",
                "PLUG_LINT",
                DiagnosticType::Lint,
                false,
            )),
            row(CodeInfo::new(
                "expected_token",
                "EXPECTED_TOKEN",
                DiagnosticType::SyntacticError,
                false,
            )),
            row(CodeInfo::new(
                "uri_does_not_exist",
                "URI_DOES_NOT_EXIST",
                DiagnosticType::CompileTimeError,
                true,
            )),
        ];
        let output = AnalysisOutput {
            diagnostics: &diagnostics,
            files: &files,
        };
        let mut out = Vec::new();
        write_json(&mut out, &output).expect("write");
        let envelope: serde_json::Value =
            serde_json::from_slice(&out).expect("one JSON object");
        let rows = envelope["diagnostics"].as_array().expect("rows");
        assert_eq!(rows[0]["documentation"], "https://dart.dev/diagnostics/avoid_empty_else");
        assert_eq!(
            rows[1]["documentation"],
            "https://dart.dev/lints/always_require_non_null_named_parameters"
        );
        assert!(rows[2].get("documentation").is_none(), "plugin lint: no URL");
        assert!(rows[3].get("documentation").is_none(), "unpublished analyzer code");
        assert_eq!(
            rows[4]["documentation"], "https://dart.dev/diagnostics/uri_does_not_exist",
            "published analyzer code: the flag alone decides"
        );
    }
}

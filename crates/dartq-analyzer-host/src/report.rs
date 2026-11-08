//! The official analysis result in `dart analyze`'s own terms: the
//! analysis server's `AnalysisError` rows, the filter and order dartdev
//! applies to them, its exit tiers, and its `--format json` envelope
//! (upstream Dart SDK source, `pkg/dartdev/lib/src/commands/analyze.dart`
//! and `pkg/dartdev/lib/src/analysis_server.dart`).
//!
//! One row model serves both official paths: the resident server's
//! `analysis.errors` notifications deserialize into [`Row`] directly, and a
//! per-run `dart analyze --format json` envelope maps back into the same
//! rows ([`parse_envelope`]). [`write_json`] re-emits dartdev's envelope
//! from rows, so the json face carries `correctionMessage`,
//! `contextMessages` and `documentation` exactly as the analyzer reports
//! them on either path.

use std::cmp::Ordering;
use std::io::{self, Write};

use serde::{Deserialize, Serialize};

/// One `AnalysisError` of the analysis server protocol. Unknown keys
/// (`hasFix`, later additions) are ignored.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    pub severity: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub location: Location,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correction: Option<String>,
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context_messages: Vec<ContextMessage>,
}

/// An `AnalysisError` or context-message location. The line and column
/// fields are optional in the protocol; dartdev prints them as `null`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Location {
    pub file: String,
    pub offset: u32,
    pub length: u32,
    #[serde(default)]
    pub start_line: Option<u32>,
    #[serde(default)]
    pub start_column: Option<u32>,
    #[serde(default)]
    pub end_line: Option<u32>,
    #[serde(default)]
    pub end_column: Option<u32>,
}

/// One `DiagnosticMessage` of a row's `contextMessages`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextMessage {
    pub message: String,
    pub location: Location,
}

/// A finished analysis as dartdev reports it: the rows its machine and
/// json faces print, and the count of the rows they leave out — ERROR
/// rows of `pubspec.yaml` and `analysis_options.yaml`, which still make
/// the exit code 3.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub rows: Vec<Row>,
    pub priority_errors: usize,
}

impl Report {
    /// dartdev's report over the latest error list of every file: TODO
    /// rows of INFO severity dropped, priority rows split off, the rest in
    /// dartdev's order.
    pub fn from_files<'a>(files: impl IntoIterator<Item = (&'a str, &'a [Row])>) -> Self {
        let mut report = Self::default();
        for (file, rows) in files {
            let priority = matches!(basename(file), "analysis_options.yaml" | "pubspec.yaml");
            for row in rows {
                if row.kind == "TODO" && row.severity == "INFO" {
                    continue;
                }
                if priority && row.severity == "ERROR" {
                    report.priority_errors += 1;
                } else {
                    report.rows.push(row.clone());
                }
            }
        }
        report.rows.sort_by(compare);
        report
    }

    /// dartdev's exit code over this report: 3 on any error (priority rows
    /// included), 2 on warnings under `fatal_warnings`, 1 on infos under
    /// `fatal_infos`, else 0.
    #[must_use]
    pub fn exit_code(&self, fatal_warnings: bool, fatal_infos: bool) -> u8 {
        let has = |severity: &str| self.rows.iter().any(|row| row.severity == severity);
        if self.priority_errors > 0 || has("ERROR") {
            3
        } else if fatal_warnings && has("WARNING") {
            2
        } else if fatal_infos && has("INFO") {
            1
        } else {
            0
        }
    }
}

fn basename(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// dartdev's `AnalysisError.compareTo`: severity (error, warning, info,
/// other), file, offset, message — strings in UTF-16 code-unit order, as
/// Dart's `String.compareTo` compares them.
fn compare(a: &Row, b: &Row) -> Ordering {
    let rank = |row: &Row| match row.severity.as_str() {
        "ERROR" => 0,
        "WARNING" => 1,
        "INFO" => 2,
        _ => 3,
    };
    rank(a)
        .cmp(&rank(b))
        .then_with(|| utf16_cmp(&a.location.file, &b.location.file))
        .then_with(|| a.location.offset.cmp(&b.location.offset))
        .then_with(|| utf16_cmp(&a.message, &b.message))
}

fn utf16_cmp(a: &str, b: &str) -> Ordering {
    if a == b {
        Ordering::Equal
    } else {
        a.encode_utf16().cmp(b.encode_utf16())
    }
}

// ---------------------------------------------------------------------------
// The `--format json` envelope
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
struct Envelope<T> {
    version: u32,
    diagnostics: Vec<T>,
}

/// One envelope diagnostic, in `emitJsonFormat`'s key order.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JsonDiagnostic {
    code: String,
    severity: String,
    #[serde(rename = "type")]
    kind: String,
    location: JsonLocation,
    problem_message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    correction_message: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    context_messages: Vec<JsonContext>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    documentation: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct JsonContext {
    location: JsonLocation,
    message: String,
}

#[derive(Serialize, Deserialize)]
struct JsonLocation {
    file: String,
    range: JsonRange,
}

#[derive(Serialize, Deserialize)]
struct JsonRange {
    start: JsonPosition,
    end: JsonPosition,
}

#[derive(Serialize, Deserialize)]
struct JsonPosition {
    offset: u32,
    line: Option<u32>,
    column: Option<u32>,
}

impl From<&Location> for JsonLocation {
    fn from(location: &Location) -> Self {
        Self {
            file: location.file.clone(),
            range: JsonRange {
                start: JsonPosition {
                    offset: location.offset,
                    line: location.start_line,
                    column: location.start_column,
                },
                end: JsonPosition {
                    offset: location.offset.saturating_add(location.length),
                    line: location.end_line,
                    column: location.end_column,
                },
            },
        }
    }
}

impl From<JsonLocation> for Location {
    fn from(location: JsonLocation) -> Self {
        let JsonRange { start, end } = location.range;
        Self {
            file: location.file,
            offset: start.offset,
            length: end.offset.saturating_sub(start.offset),
            start_line: start.line,
            start_column: start.column,
            end_line: end.line,
            end_column: end.column,
        }
    }
}

/// Writes dartdev's `--format json` envelope over `rows`, one line,
/// without the `memory` key (the caller splices it).
///
/// # Errors
///
/// The writer's error.
pub fn write_json(out: &mut impl Write, rows: &[Row]) -> io::Result<()> {
    let diagnostics = rows
        .iter()
        .map(|row| JsonDiagnostic {
            code: row.code.clone(),
            severity: row.severity.clone(),
            kind: row.kind.clone(),
            location: (&row.location).into(),
            problem_message: row.message.clone(),
            correction_message: row.correction.clone(),
            context_messages: row
                .context_messages
                .iter()
                .map(|context| JsonContext {
                    location: (&context.location).into(),
                    message: context.message.clone(),
                })
                .collect(),
            documentation: row.url.clone(),
        })
        .collect();
    serde_json::to_writer(
        &mut *out,
        &Envelope {
            version: 1,
            diagnostics,
        },
    )?;
    out.write_all(b"\n")
}

/// The rows of one `dart analyze --format json` envelope line (a `memory`
/// key is ignored); `None` when the line is no envelope.
#[must_use]
pub fn parse_envelope(line: &str) -> Option<Vec<Row>> {
    let envelope: Envelope<JsonDiagnostic> = serde_json::from_str(line).ok()?;
    Some(
        envelope
            .diagnostics
            .into_iter()
            .map(|d| Row {
                severity: d.severity,
                kind: d.kind,
                location: d.location.into(),
                message: d.problem_message,
                correction: d.correction_message,
                code: d.code,
                url: d.documentation,
                context_messages: d
                    .context_messages
                    .into_iter()
                    .map(|context| ContextMessage {
                        message: context.message,
                        location: context.location.into(),
                    })
                    .collect(),
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An `analysis.errors` row as the server sends it: correction,
    /// documentation url and one context message.
    const SERVER_ROW: &str = r#"{"severity":"ERROR","type":"COMPILE_TIME_ERROR","location":{"file":"/p/lib/a.dart","offset":20,"length":3,"startLine":2,"startColumn":5,"endLine":2,"endColumn":8},"message":"The name 'foo' is already defined.","correction":"Try renaming one of the declarations.","code":"duplicate_definition","url":"https://dart.dev/diagnostics/duplicate_definition","contextMessages":[{"message":"The first definition of this name.","location":{"file":"/p/lib/a.dart","offset":4,"length":3,"startLine":1,"startColumn":5,"endLine":1,"endColumn":8}}],"hasFix":true}"#;

    fn row(severity: &str, kind: &str, file: &str, offset: u32, message: &str) -> Row {
        Row {
            severity: severity.into(),
            kind: kind.into(),
            location: Location {
                file: file.into(),
                offset,
                length: 1,
                start_line: Some(1),
                start_column: Some(offset + 1),
                end_line: Some(1),
                end_column: Some(offset + 2),
            },
            message: message.into(),
            correction: None,
            code: "c".into(),
            url: None,
            context_messages: Vec::new(),
        }
    }

    /// The json face keeps correctionMessage, contextMessages and
    /// documentation in dartdev's key order, and the per-run envelope
    /// parses back into the identical row (the #4122 fidelity pin).
    #[test]
    fn the_json_envelope_keeps_correction_context_and_documentation() {
        let server: Row = serde_json::from_str(SERVER_ROW).expect("server row");
        let mut out = Vec::new();
        write_json(&mut out, std::slice::from_ref(&server)).expect("write");
        let line = String::from_utf8(out).expect("UTF-8");
        assert_eq!(
            line,
            concat!(
                r#"{"version":1,"diagnostics":[{"code":"duplicate_definition","severity":"ERROR","#,
                r#""type":"COMPILE_TIME_ERROR","location":{"file":"/p/lib/a.dart","range":{"#,
                r#""start":{"offset":20,"line":2,"column":5},"end":{"offset":23,"line":2,"column":8}}},"#,
                r#""problemMessage":"The name 'foo' is already defined.","#,
                r#""correctionMessage":"Try renaming one of the declarations.","#,
                r#""contextMessages":[{"location":{"file":"/p/lib/a.dart","range":{"#,
                r#""start":{"offset":4,"line":1,"column":5},"end":{"offset":7,"line":1,"column":8}}},"#,
                r#""message":"The first definition of this name."}],"#,
                r#""documentation":"https://dart.dev/diagnostics/duplicate_definition"}]}"#,
                "\n"
            )
        );
        assert_eq!(parse_envelope(line.trim_end()), Some(vec![server]));
        assert_eq!(parse_envelope("Analyzing lib..."), None);
    }

    /// dartdev's filter, priority split, order and exit tiers.
    #[test]
    fn the_report_filters_splits_orders_and_tiers_like_dartdev() {
        let a = [
            row("INFO", "TODO", "/p/a.dart", 0, "todo"),
            row("INFO", "LINT", "/p/a.dart", 9, "b"),
            row("INFO", "LINT", "/p/a.dart", 9, "a"),
            row("WARNING", "STATIC_WARNING", "/p/a.dart", 30, "w"),
        ];
        let b = [row("INFO", "HINT", "/p/B.dart", 0, "h")];
        let yaml = [
            row("ERROR", "COMPILE_TIME_ERROR", "/p/pubspec.yaml", 0, "bad"),
            row("WARNING", "STATIC_WARNING", "/p/pubspec.yaml", 0, "kept"),
        ];
        let report = Report::from_files([
            ("/p/a.dart", &a[..]),
            ("/p/B.dart", &b[..]),
            ("/p/pubspec.yaml", &yaml[..]),
        ]);
        assert_eq!(report.priority_errors, 1);
        let order: Vec<(&str, &str)> = report
            .rows
            .iter()
            .map(|row| (row.location.file.as_str(), row.message.as_str()))
            .collect();
        assert_eq!(
            order,
            [
                ("/p/a.dart", "w"),
                ("/p/pubspec.yaml", "kept"),
                ("/p/B.dart", "h"),
                ("/p/a.dart", "a"),
                ("/p/a.dart", "b"),
            ]
        );
        assert_eq!(
            report.exit_code(true, false),
            3,
            "the priority error counts"
        );

        let clean = Report::from_files([("/p/a.dart", &a[1..])]);
        assert_eq!(clean.exit_code(true, false), 2);
        assert_eq!(clean.exit_code(false, false), 0);
        assert_eq!(clean.exit_code(false, true), 1);
        assert_eq!(Report::default().exit_code(true, true), 0);
    }
}

//! The CLI's output-format layer: the one `--format <lon|json|text>` axis
//! every verb with output carries, and the three emitters.
//!
//! * `lon` (the default) — canonical LON ([Language Object Notation v1],
//!   the `lon` crate) over the value: record tables factor repeated keys,
//!   safe strings drop their quotes — the compact AI-first data face. The
//!   default is a constant, never the terminal's shape: the same
//!   invocation prints the same bytes piped or on a TTY.
//! * `json` — the frozen single-line JSON shapes (the oracle-parity
//!   envelope on the diagnostic faces, the frozen v1 contracts elsewhere),
//!   unchanged for jq consumers.
//! * `text` — the human face: bespoke renderings on the read verbs that
//!   warrant one (analyze groups, the fix listing, the packages table,
//!   session identity lines), the indented value everywhere else; write
//!   faces fall back to `lon` (their output is a record, not prose).
//!
//! The three faces encode the same value — there is no key-renaming
//! layer. The producers own the vocabulary: the command faces emit the
//! short snake_case table the daemon's `dartq/*` answers share natively
//! (`v`, `diags`, `sev`, `decls`, `refs`, `off`, and so on), and the two
//! frozen long-spelling contracts keep theirs on every face — the
//! diagnostic faces' json envelope (the oracle-parity dialect) and the
//! compat report's `schema_version`.
//!
//! [Language Object Notation v1]: https://github.com/fluttercandies/lon/blob/main/spec/lon-v1.md

use std::io::{self, Write};

use clap::ValueEnum;
use serde_json::Value;

use dartq_diagnostics::{AnalysisOutput, Diagnostic, FileLayoutProvider, Severity};

/// The unified `--format` axis. `lon` is the constant default; an invalid
/// spelling is clap's usage error (exit 64), the same tier every other
/// flag typo takes.
#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum OutputFormat {
    /// Canonical LON (the default).
    Lon,
    /// Single-line JSON, the frozen shapes (jq consumers).
    Json,
    /// Human rendering on the read faces; write faces fall back to lon.
    Text,
}

/// One LON line of `value`.
pub(crate) fn lon_line(value: &Value) -> String {
    // The values are serde_json graphs: no non-finite floats exist in the
    // model, and every producer's depth sits far under the codec's limits
    // — an error here is a program bug, not a runtime case.
    lon::to_string(value).expect("the value graph encodes as LON")
}

/// The `lon` face: one canonical line.
pub(crate) fn write_lon(out: &mut impl Write, value: &Value) -> io::Result<()> {
    writeln!(out, "{}", lon_line(value))
}

/// The `json` face: the frozen single-line shape, untouched.
pub(crate) fn write_json_value(out: &mut impl Write, value: &Value) -> io::Result<()> {
    writeln!(out, "{value}")
}

/// The generic `text` face: the value, indented — the read verbs without
/// a bespoke renderer (query, explain, impact, compat, index, rename,
/// resolve, the debug status listing).
pub(crate) fn write_text_value(out: &mut impl Write, value: &Value) -> io::Result<()> {
    writeln!(
        out,
        "{}",
        serde_json::to_string_pretty(value).expect("the value pretty-prints")
    )
}

/// The analyze/check faces' `lon` document: one flat `diags` row per
/// diagnostic — `{v diags:[{code sev file line col len msg doc?}]}` —
/// the short-vocabulary spelling of the diagnostics set (the json face's
/// oracle envelope is a different, frozen dialect and stays untouched).
pub(crate) fn diags_document(output: &AnalysisOutput<'_>) -> Value {
    let mut url = String::new();
    let rows = output
        .diagnostics
        .iter()
        .map(|diag| diag_row(diag, output, &mut url))
        .collect::<Vec<_>>();
    serde_json::json!({"v": 1, "diags": rows})
}

/// One `diags` row. `url` is the caller's reused buffer for the
/// published-docs URL (the registry flag alone decides, the writer's
/// `documentation_url` rule).
fn diag_row(diag: &Diagnostic, output: &AnalysisOutput<'_>, url: &mut String) -> Value {
    let (line, col) = output.files.line_col(diag.file, diag.range.offset);
    let mut row = serde_json::json!({
        "code": diag.code.name,
        "sev": severity_word(diag.severity),
        "file": output.files.file_path(diag.file).display().to_string(),
        "line": line,
        "col": col,
        "len": diag.range.length,
        "msg": diag.message.to_string(),
    });
    if diag.code.has_published_docs {
        url.clear();
        url.push_str("https://dart.dev/diagnostics/");
        url.push_str(diag.code.name);
        row["doc"] = Value::String(url.clone());
    }
    row
}

/// The `sev` value spelling: lowercase, the daemon's compact rows' own —
/// the one severity spelling this crate's lon/text faces print (the json
/// face's oracle envelope keeps the analyzer's uppercase, in
/// dartq-diagnostics).
fn severity_word(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Info => "info",
    }
}

/// The analyze `text` face's row spelling: `sev code line:col msg` under
/// the file heading, `sev` lowercase like every non-oracle face (the
/// grouped human renderer in `main` owns headings and the count tail;
/// this is the row).
pub(crate) fn text_diag_row(diag: &Diagnostic, layout: &dyn FileLayoutProvider) -> String {
    let (line, col) = layout.line_col(diag.file, diag.range.offset);
    format!(
        "{} {} {}:{} {}",
        severity_word(diag.severity),
        diag.code.name,
        line,
        col,
        diag.message
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The emitters are one source: `lon` one canonical line (bare safe
    /// strings, record tables where they pay), `json` the frozen
    /// single-line shape, `text` the indented value — the same keys on
    /// every face, no renaming between them.
    #[test]
    fn emitters_shape_their_faces() {
        let value = serde_json::json!({"v": 1, "diags": []});
        let mut lon = Vec::new();
        write_lon(&mut lon, &value).expect("lon");
        assert_eq!(String::from_utf8(lon).expect("utf-8"), "{v:1 diags:[]}\n");

        let mut json = Vec::new();
        write_json_value(&mut json, &value).expect("json");
        assert_eq!(
            String::from_utf8(json).expect("utf-8"),
            "{\"v\":1,\"diags\":[]}\n",
            "the json face writes the value verbatim"
        );

        let mut text = Vec::new();
        write_text_value(&mut text, &value).expect("text");
        let text = String::from_utf8(text).expect("utf-8");
        assert!(text.contains("\"v\": 1"), "indented, same keys: {text}");
    }

    /// LON round-trips through the crate's own decoder: the emitted line
    /// decodes to the value (the canonical-encoding contract the e2e
    /// assertions lean on).
    #[test]
    fn lon_line_decodes_to_the_value() {
        let value = serde_json::json!({
            "v": 1,
            "sym": "users",
            "decls": [
                {"id": 1, "note": "01"},
                {"id": 2, "note": "02"},
            ],
        });
        let line = lon_line(&value);
        let decoded = lon::from_str::<Value>(&line).expect("decodes");
        assert_eq!(decoded, value);
        assert!(
            line.contains("[id note;1 \"01\";2 \"02\"]"),
            "the record table factors repeated keys; numeric-looking strings stay quoted: {line}"
        );
    }
}

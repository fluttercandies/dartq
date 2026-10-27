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
//! compat report's `schema_version`. The analyze lon document itself is
//! the shared builder in `dartq_commands::analyze::diags_document`
//! (#2657: one encoder, both the CLI and the daemon/MCP faces render
//! through) — re-exported here as the CLI face's spelling.
//!
//! [Language Object Notation v1]: https://github.com/fluttercandies/lon/blob/main/spec/lon-v1.md

use std::io::{self, Write};

use clap::ValueEnum;
use serde_json::Value;

use dartq_diagnostics::{Diagnostic, FileLayoutProvider};

/// The analyze/check faces' lon document — the shared builder both the CLI
/// and the daemon's `dartq/analyze` render through (`dartq-commands`' own,
/// #2657: no second encoder).
pub(crate) use dartq_commands::analyze::diags_document;
use dartq_commands::analyze::severity_word;

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
    use std::path::{Path, PathBuf};

    use dartq_diagnostics::{AnalysisOutput, FileId, Severity, SourceRange, UNDEFINED_IDENTIFIER};

    use super::*;

    /// A layout over the interned paths, optionally carrying an analysis
    /// root — the provider face the lon byte pins project through.
    struct Layout {
        table: dartq_source::FileTable,
        root: Option<PathBuf>,
    }

    impl dartq_diagnostics::FileLayoutProvider for Layout {
        fn file_path(&self, file: FileId) -> &Path {
            self.table.path(file).expect("interned FileId")
        }

        fn analysis_root(&self) -> Option<&Path> {
            self.root.as_deref()
        }

        fn line_col(&self, _file: FileId, _offset: u32) -> (u32, u32) {
            (1, 27)
        }
    }

    fn published_diag(table: &mut dartq_source::FileTable, path: &Path) -> Diagnostic {
        let file = table.intern(path);
        Diagnostic::new(
            Severity::Error,
            UNDEFINED_IDENTIFIER,
            "Expected an identifier.",
            file,
            SourceRange {
                offset: 26,
                length: 1,
            },
            None,
        )
    }

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

    /// The shared builder's lon bytes — the face this crate prints and the
    /// daemon's `dartq/analyze` answers byte-for-byte (#2657): the empty
    /// set is the bare `{v:1 diags:[]}`, one row renders the object form,
    /// two uniform rows factor into the record table, and the root rides
    /// exactly once when rows exist.
    #[test]
    fn diags_document_lon_bytes_over_the_shared_builder() {
        let mut files = Layout {
            table: dartq_source::FileTable::new(),
            root: Some(PathBuf::from("/w/pkg")),
        };
        let empty: [Diagnostic; 0] = [];
        let output = AnalysisOutput {
            diagnostics: &empty,
            files: &files,
        };
        assert_eq!(
            lon_line(&diags_document(&output)),
            "{v:1 diags:[]}",
            "the empty set carries no root"
        );

        let one = [published_diag(
            &mut files.table,
            Path::new("/w/pkg/lib/a.dart"),
        )];
        let output = AnalysisOutput {
            diagnostics: &one,
            files: &files,
        };
        assert_eq!(
            lon_line(&diags_document(&output)),
            "{v:1 root:/w/pkg diags:[{code:undefined_identifier sev:error file:lib/a.dart \
             line:1 col:27 len:1 msg:\"Expected an identifier.\"}]}",
            "one row: the object form, no table header spent"
        );

        let two = [
            published_diag(&mut files.table, Path::new("/w/pkg/lib/a.dart")),
            published_diag(&mut files.table, Path::new("/w/pkg/lib/a.dart")),
        ];
        let output = AnalysisOutput {
            diagnostics: &two,
            files: &files,
        };
        assert_eq!(
            lon_line(&diags_document(&output)),
            "{v:1 root:/w/pkg diags:[code sev file line col len msg;undefined_identifier \
             error lib/a.dart 1 27 1 \"Expected an identifier.\";undefined_identifier error \
             lib/a.dart 1 27 1 \"Expected an identifier.\"]}",
            "two uniform rows factor into the record table"
        );
    }
}

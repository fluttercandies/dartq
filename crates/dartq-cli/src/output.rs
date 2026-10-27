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
use std::path::Path;

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
/// diagnostic — `{v root? diags:[{code sev file line col len msg}]}` —
/// the short-vocabulary spelling of the diagnostics set (the json face's
/// oracle envelope is a different, frozen dialect and stays untouched:
/// absolute paths, no root key, `documentation` per the oracle's url
/// rules).
///
/// The `file` column spells through the one file locator
/// (`dartq_commands::locate`): relative to the run's analysis root when
/// the file lies inside it, absolute otherwise. The envelope carries that
/// root exactly once (`root:<absolute path>`) so the relative spellings
/// are unambiguous — a mixed set (a workspace row beside a pub-cache row)
/// reads `root:/w/pkg` + `file:lib/a.dart` + `file:/Users/…/x.dart`. The
/// key rides only when rows exist and the run has one root (the empty
/// set is still `{v:1 diags:[]}`; a multi-root merge or a degenerate
/// root keeps absolute spellings and no key). The `doc` URL is off on
/// this face: `code` is the lookup key and `explain` returns the URL —
/// the oracle machine format carries none either.
///
/// # Row shape at small N
///
/// The lon crate's encoder already amortizes for row count: arrays below
/// two elements never form a record table (`lon` 0.1
/// `write_record_table`: `values.len() < 2` → the ordinary form), and the
/// two-row/two-field edge compares structural bytes before factoring.
/// One diagnostic therefore renders the object form
/// `diags:[{code:… sev:… …}]` — no table header. Measured on the m0
/// single-error row (located `file:main.dart`): the object form is 108
/// bytes, a forced single-row table (`[code sev file line col len
/// msg;…]`) 106 — a ±2-byte class delta that flips sign with value
/// lengths, not a lever. This function does no shape selection of its
/// own: the encoder's gate decides, deterministically for every N.
pub(crate) fn diags_document(output: &AnalysisOutput<'_>) -> Value {
    // One provider read for the whole document: the root decides every
    // row's spelling.
    let root = output.files.analysis_root();
    let rows = output
        .diagnostics
        .iter()
        .map(|diag| diag_row(diag, output, root))
        .collect::<Vec<_>>();
    match (root, rows.is_empty()) {
        (Some(root), false) => serde_json::json!({
            "v": 1,
            "root": root.display().to_string(),
            "diags": rows,
        }),
        _ => serde_json::json!({"v": 1, "diags": rows}),
    }
}

/// One `diags` row: the locator-spelled `file`, no `doc` (see
/// [`diags_document`]).
fn diag_row(diag: &Diagnostic, output: &AnalysisOutput<'_>, root: Option<&Path>) -> Value {
    let (line, col) = output.files.line_col(diag.file, diag.range.offset);
    let path = output.files.file_path(diag.file);
    let file = match root {
        Some(root) => dartq_commands::locate(root, path),
        None => path.display().to_string(),
    };
    serde_json::json!({
        "code": diag.code.name,
        "sev": severity_word(diag.severity),
        "file": file,
        "line": line,
        "col": col,
        "len": diag.range.length,
        "msg": diag.message.to_string(),
    })
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
    use std::path::{Path, PathBuf};

    use dartq_diagnostics::{FileId, Severity, SourceRange, UNDEFINED_IDENTIFIER};

    use super::*;

    /// A layout over the interned paths, optionally carrying an analysis
    /// root — the provider face the locator tests project through.
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

    /// The locator policy on the document face: the envelope carries the
    /// root once, rows inside the root spell relative (no `package:`
    /// form, no leading `./`), rows outside keep the absolute spelling,
    /// and `doc` is off — even for a published code (the json face's
    /// `documentation` is that face's own).
    #[test]
    fn diags_document_locates_files_and_carries_the_root() {
        let mut table = dartq_source::FileTable::new();
        let inside = table.intern(Path::new("/w/pkg/lib/a.dart"));
        let outside = table.intern(Path::new("/Users/x/.pub-cache/x/lib/b.dart"));
        let row = |file: FileId| {
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
        };
        let diagnostics = [row(inside), row(outside)];
        let files = Layout {
            table,
            root: Some(PathBuf::from("/w/pkg")),
        };
        let output = AnalysisOutput {
            diagnostics: &diagnostics,
            files: &files,
        };
        let document = diags_document(&output);
        assert_eq!(document["root"], "/w/pkg", "the root rides exactly once");
        let diags = document["diags"].as_array().expect("rows");
        assert_eq!(diags[0]["file"], "lib/a.dart", "inside the root: relative");
        assert_eq!(
            diags[1]["file"],
            "/Users/x/.pub-cache/x/lib/b.dart",
            "outside the root: absolute"
        );
        assert!(
            diags.iter().all(|row| row.get("doc").is_none()),
            "the doc URL is off on this face: {}",
            document
        );
    }

    /// The no-root and empty-set faces: a provider without one root keeps
    /// absolute spellings and no `root` key, and the clean run is still
    /// the bare `{v:1 diags:[]}` document.
    #[test]
    fn diags_document_without_a_root_or_rows() {
        let mut table = dartq_source::FileTable::new();
        let diagnostics =
            [published_diag(&mut table, Path::new("/w/pkg/lib/a.dart"))];
        let files = Layout { table, root: None };
        let output = AnalysisOutput {
            diagnostics: &diagnostics,
            files: &files,
        };
        let document = diags_document(&output);
        assert!(document.get("root").is_none(), "no root to carry");
        assert_eq!(
            document["diags"][0]["file"],
            "/w/pkg/lib/a.dart",
            "absolute spelling stands"
        );

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
    }

    /// The small-N row shapes (the crate's own gate, pinned so a codec
    /// change is a visible contract change): one row renders the object
    /// form — no table header — and two uniform rows factor into the
    /// record table.
    #[test]
    fn diags_document_row_shape_by_count() {
        let mut table = dartq_source::FileTable::new();
        let mut files = Layout {
            table,
            root: Some(PathBuf::from("/w/pkg")),
        };
        let one = [published_diag(&mut files.table, Path::new("/w/pkg/lib/a.dart"))];
        let output = AnalysisOutput {
            diagnostics: &one,
            files: &files,
        };
        let line = lon_line(&diags_document(&output));
        assert!(
            line.contains(
                "diags:[{code:undefined_identifier sev:error file:lib/a.dart \
                 line:1 col:27 len:1 msg:\"Expected an identifier.\"}]"
            ),
            "one row: the object form, no header spent: {line}"
        );

        let two = [
            published_diag(&mut files.table, Path::new("/w/pkg/lib/a.dart")),
            published_diag(&mut files.table, Path::new("/w/pkg/lib/a.dart")),
        ];
        let output = AnalysisOutput {
            diagnostics: &two,
            files: &files,
        };
        let line = lon_line(&diags_document(&output));
        assert!(
            line.contains("[code sev file line col len msg;"),
            "two uniform rows factor into the record table: {line}"
        );
    }
}

//! The check face's core: the M0 pipeline (engine-api-spec §2.1) over a
//! discovered file set — bytes → scan → `@dart` override rescan → parse —
//! producing the canonical-order diagnostic set the oracle-exact writers
//! render. Stops there by design: no pub/package/SDK/engine involvement
//! (the spec's M1 forbid-list).

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use dartq_diagnostics::{
    AnalysisOutput, CodeRegistry, Diagnostic, FileId, FileLayoutProvider, Severity, SourceRange,
};
use dartq_language::{
    DEFAULT_LANGUAGE_VERSION, Feature, FeatureSet, LanguageVersion, triple_shift_enabled,
};
use dartq_parser::{Message, Parser, ParserFeatures, ParserListener, TokenSpan};
use dartq_scanner::{ScanConfig, ScanResult, scan};
use dartq_source::{FileTable, LineIndex};

/// One check run's result: the per-file layout state behind the writers and
/// the canonical-order diagnostic set (file discovery order, then source
/// position, then code/message — the engine-spec §1.3 canonical-sort
/// granularity M0 has).
pub struct CheckRun {
    files: FileSet,
    diagnostics: Vec<Diagnostic>,
}

impl CheckRun {
    /// The writers' input: diagnostics plus the file layout they resolve
    /// positions against.
    #[must_use]
    pub fn output(&self) -> AnalysisOutput<'_> {
        AnalysisOutput {
            diagnostics: &self.diagnostics,
            files: &self.files,
        }
    }

    /// The canonical-order diagnostic set.
    #[must_use]
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// The number of analyzed files.
    #[must_use]
    pub fn file_count(&self) -> usize {
        self.files.sources.len()
    }

    /// Total lines of the analyzed sources: newline bytes counted in one
    /// pass (the engine discovery convention, no line splitting).
    #[must_use]
    pub fn loc(&self) -> u64 {
        self.files
            .sources
            .iter()
            .map(|source| source.bytes().filter(|&b| b == b'\n').count() as u64)
            .sum()
    }
}

/// Runs the M0 pipeline over a discovered file set (see
/// [`crate::discover::discover`]). A read/decode failure aborts the run —
/// a partial diagnostic set must never masquerade as a valid result.
///
/// # Errors
///
/// Source read or UTF-8 decode failure.
pub fn analyze_files(files: &BTreeSet<PathBuf>) -> Result<CheckRun, io::Error> {
    let mut files_state = FileSet::default();
    let mut diags = Vec::new();
    for path in files {
        let (canonical, source) = read_source(path)?;
        let file = files_state.push(&source, &canonical);
        diags.extend(analyze_file(&source, file));
    }
    diags.sort_by(|a, b| {
        a.file
            .index()
            .cmp(&b.file.index())
            .then_with(|| a.range.offset.cmp(&b.range.offset))
            .then_with(|| a.code.machine.cmp(b.code.machine))
            .then_with(|| a.message.cmp(&b.message))
    });
    Ok(CheckRun {
        files: files_state,
        diagnostics: diags,
    })
}

/// Canonical absolute path (the oracle prints symlink-resolved paths) plus
/// strict UTF-8 source. A read/decode failure is an error, never a silently
/// skipped file — lossy output would be format drift downstream.
///
/// Exactly one leading UTF-8 BOM is dropped (oracle read-layer parity: the
/// analyzer read path strips it before scanning; spans are relative to the
/// stripped text). A second BOM is content.
///
/// # Errors
///
/// Canonicalization, read, or UTF-8 decode failure.
pub fn read_source(path: &Path) -> io::Result<(PathBuf, String)> {
    let canonical = path.canonicalize()?;
    let mut source = fs::read_to_string(&canonical)?;
    if source.starts_with('\u{FEFF}') {
        source.drain(..'\u{FEFF}'.len_utf8());
    }
    Ok((canonical, source))
}

/// Per-file state behind [`FileLayoutProvider`]: canonical paths live in the
/// [`FileTable`], positions resolve through the production [`LineIndex`] path
/// (the same conversion the diagnostics goldens pin).
#[derive(Default)]
pub struct FileSet {
    table: FileTable,
    sources: Vec<String>,
    indexes: Vec<LineIndex>,
}

impl FileSet {
    /// Interns `path` (canonicalized by the caller) and registers its layout.
    pub fn push(&mut self, source: &str, path: &Path) -> FileId {
        let file = self.table.intern(path);
        debug_assert_eq!(file.index() as usize, self.sources.len());
        self.indexes.push(LineIndex::new(source));
        self.sources.push(source.to_owned());
        file
    }
}

impl FileLayoutProvider for FileSet {
    fn file_path(&self, file: FileId) -> &Path {
        self.table.path(file).expect("interned FileId")
    }

    fn line_col(&self, file: FileId, utf16_offset: u32) -> (u32, u32) {
        let index = file.index() as usize;
        // Diagnostic offsets are UTF-16; the fused conversion reports the
        // official `LineInfo.getLocation` (line, UTF-16 column) directly.
        self.indexes[index].line_col_utf16(&self.sources[index], utf16_offset)
    }
}

/// Runs the M0 pipeline over one file: scan (+ override rescan), then parse
/// through a diagnostics-collecting listener.
fn analyze_file(source: &str, file: FileId) -> Vec<Diagnostic> {
    let (version, scanned, mut diags) = scan_file(source, file);
    let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(version));
    // The parser owns its listener by value and exposes no accessor; a shared
    // sink is the minimal way to collect events out. When #18 starts emitting
    // `handle_recoverable_error`, these diagnostics flow without CLI changes.
    let sink = Rc::new(RefCell::new(Vec::new()));
    Parser::new(
        source,
        &scanned.tokens,
        features,
        DiagnosticListener {
            file,
            sink: Rc::clone(&sink),
        },
    )
    .parse_unit();
    diags.extend(std::mem::take(&mut *sink.borrow_mut()));
    diags.extend(scanned.diagnostics);
    diags
}

/// Scans `source`, honoring the `@dart` override consumer contract (scanner
/// module docs; official `source_loader.dart:1194–1209` — the config is
/// re-derived and the file rescanned before parsing). An override above the
/// latest known version is dropped with the official `STATIC_WARNING`
/// (official scanner: `_overrideVersion = null`); the scanner reports only
/// the last header-window match, so the loop settles after at most one
/// rescan.
fn scan_file(source: &str, file: FileId) -> (LanguageVersion, ScanResult, Vec<Diagnostic>) {
    let mut version = DEFAULT_LANGUAGE_VERSION;
    loop {
        let features = FeatureSet::for_language_version(version);
        let result = scan(
            source,
            ScanConfig {
                enable_triple_shift: triple_shift_enabled(version),
                enable_augmentations: features.is_enabled(Feature::Augmentations),
            },
            file,
        );
        match result.language_version_override {
            Some(override_version) if override_version.is_above_latest() => {
                // The warning pins at the version comment — the whole
                // `// @dart = x.y` lexeme, isomorphic to the official
                // `LanguageVersionToken` (`analyzer scanner.dart:124`
                // `.atOffset(versionToken.offset, versionToken.length)`; oracle
                // `dart analyze --format=machine` on a line-1 comment:
                // `1|1|15`).
                let code = CodeRegistry::builtin()
                    .get("invalid_language_version_override")
                    .expect("official registry pins invalid_language_version_override");
                let span = result
                    .language_version_override_span
                    .expect("scanner records the comment span with every override");
                let warning = Diagnostic::new(
                    Severity::Warning,
                    code,
                    format!(
                        "The language version override can't specify a version greater than \
                         the latest known language version: {DEFAULT_LANGUAGE_VERSION}."
                    ),
                    file,
                    span,
                    None,
                );
                return (version, result, vec![warning]);
            }
            Some(override_version) if override_version != version => version = override_version,
            _ => return (version, result, Vec::new()),
        }
    }
}

/// Collects parser recoverable errors into [`Diagnostic`]s. Per the parser's
/// `message.rs` contract, the listener owns the translation into a
/// `dartq_diagnostics::Diagnostic`, including file attribution; event spans
/// arrive precomputed in the external UTF-16 convention ([`TokenSpan`]).
struct DiagnosticListener {
    file: FileId,
    sink: Rc<RefCell<Vec<Diagnostic>>>,
}

impl ParserListener for DiagnosticListener {
    fn handle_recoverable_error(&mut self, message: Message, start: TokenSpan, end: TokenSpan) {
        let offset = start.range.offset;
        let length = end.range.end() - offset;
        self.sink.borrow_mut().push(Diagnostic::new(
            Severity::Error,
            message.code,
            message.text,
            self.file,
            SourceRange { offset, length },
            None,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dartq_diagnostics::{EXPECTED_TOKEN, write_machine};

    /// Byte-shape parity with the harness machine parser
    /// (`tests/harness/src/machine.rs` `parse_machine`): 8 `|`-separated
    /// fields, uppercase severity/type/code, 1-based line/col, UTF-16 length.
    #[test]
    fn machine_line_parity_with_harness() {
        let mut set = FileSet::default();
        let file = set.push("void f() { x }\n", Path::new("/c/lib/a.dart"));
        // offset 11 (`x`), length 1 → line 1, column 12
        let diag = Diagnostic::new(
            Severity::Error,
            EXPECTED_TOKEN,
            "Expected to find ';'.",
            file,
            SourceRange {
                offset: 11,
                length: 1,
            },
            None,
        );
        let mut out = Vec::new();
        write_machine(
            &mut out,
            &AnalysisOutput {
                diagnostics: &[diag],
                files: &set,
            },
        )
        .expect("write");
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "ERROR|SYNTACTIC_ERROR|EXPECTED_TOKEN|/c/lib/a.dart|1|12|1|Expected to find ';'.\n"
        );
    }

    /// The F11 override contract: differing override rescans; above-latest
    /// drops with the official warning; last header match wins.
    #[test]
    fn override_rescan_drop_and_last_wins() {
        let mut table = FileTable::new();
        let file = table.intern(Path::new("t.dart"));

        let (version, _, warnings) = scan_file("// @dart = 2.12\nvar x = 1;\n", file);
        assert_eq!(version, LanguageVersion::new(2, 12), "override adopted");
        assert!(warnings.is_empty());

        let (version, _, warnings) = scan_file("// @dart = 9.9\n", file);
        assert_eq!(version, DEFAULT_LANGUAGE_VERSION, "above-latest dropped");
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code.name, "invalid_language_version_override");
        assert_eq!(warnings[0].severity, Severity::Warning);
        assert_eq!(warnings[0].range.offset, 0, "span at the comment");
        assert_eq!(warnings[0].range.length, 14, "whole comment lexeme");

        // Era anchors (oracle dart 3.13.1, facts version-skew §1.3 T2): 3.14
        // is above the oracle's latest known version — warned and dropped;
        // 3.13 is the latest and is accepted.
        let (version, _, warnings) = scan_file("// @dart = 3.14\nvar x = 1;\n", file);
        assert_eq!(
            version, DEFAULT_LANGUAGE_VERSION,
            "above-latest 3.14 dropped"
        );
        assert_eq!(warnings.len(), 1);

        let (version, _, warnings) = scan_file("// @dart = 3.13\nvar x = 1;\n", file);
        assert_eq!(
            version,
            LanguageVersion::new(3, 13),
            "oracle latest accepted"
        );
        assert!(warnings.is_empty());

        let (version, ..) = scan_file("// @dart = 2.12\n// @dart = 3.1\n", file);
        assert_eq!(version, LanguageVersion::new(3, 1), "last match wins");

        let (version, ..) = scan_file("var x = 1;\n", file);
        assert_eq!(version, DEFAULT_LANGUAGE_VERSION);
    }

    /// The above-latest warning emits the oracle-matching machine line: the
    /// span is the whole `@dart` comment lexeme (oracle Dart 3.13.1,
    /// `dart analyze --format=machine` on `// @dart = 99.9`:
    /// `WARNING|STATIC_WARNING|INVALID_LANGUAGE_VERSION_OVERRIDE|<file>|1|1|15|…`;
    /// indented: `1|3|15`).
    #[test]
    fn override_warning_machine_span() {
        fn machine_line(source: &str, path: &str) -> String {
            let mut set = FileSet::default();
            let file = set.push(source, Path::new(path));
            let (_, _, diags) = scan_file(source, file);
            assert_eq!(diags.len(), 1);
            let mut out = Vec::new();
            write_machine(
                &mut out,
                &AnalysisOutput {
                    diagnostics: &diags,
                    files: &set,
                },
            )
            .expect("write");
            String::from_utf8(out).unwrap()
        }

        let expected = format!(
            "The language version override can't specify a version greater than \
             the latest known language version: {DEFAULT_LANGUAGE_VERSION}."
        );
        assert_eq!(
            machine_line("// @dart = 99.9\nvar x = 1;\n", "/c/lib/a.dart"),
            format!(
                "WARNING|STATIC_WARNING|INVALID_LANGUAGE_VERSION_OVERRIDE|/c/lib/a.dart|\
                 1|1|15|{expected}\n"
            )
        );
        assert_eq!(
            machine_line("  // @dart = 9.9\n", "/c/lib/b.dart"),
            format!(
                "WARNING|STATIC_WARNING|INVALID_LANGUAGE_VERSION_OVERRIDE|/c/lib/b.dart|\
                 1|3|14|{expected}\n"
            )
        );
    }

    /// The CLI read face strips exactly one leading BOM, matching the oracle
    /// read layer: single BOM gone, second BOM kept as content, BOM-only
    /// file reads back empty.
    #[test]
    fn read_source_strips_one_leading_bom() {
        let root = std::env::temp_dir().join(format!("dq_cmd_bom_{}", std::process::id()));
        fs::create_dir_all(&root).expect("mkdir");
        fs::write(root.join("single.dart"), "\u{FEFF}var x;").expect("write");
        fs::write(root.join("double.dart"), "\u{FEFF}\u{FEFF}x").expect("write");
        fs::write(root.join("only.dart"), "\u{FEFF}").expect("write");
        fs::write(root.join("plain.dart"), "var x;").expect("write");

        let (_, single) = read_source(&root.join("single.dart")).expect("read");
        assert_eq!(single, "var x;");
        let (_, double) = read_source(&root.join("double.dart")).expect("read");
        assert_eq!(double, "\u{FEFF}x");
        let (_, only) = read_source(&root.join("only.dart")).expect("read");
        assert!(only.is_empty(), "BOM-only file strips to the empty string");
        let (_, plain) = read_source(&root.join("plain.dart")).expect("read");
        assert_eq!(plain, "var x;");

        fs::remove_dir_all(&root).ok();
    }

    /// End to end over a real temp tree: discovery, canonical paths, machine
    /// output, and the canonical diagnostic order.
    #[test]
    fn analyze_files_runs_pipeline_over_temp_tree() {
        let root = std::env::temp_dir().join(format!("dq_cmd_check_{}", std::process::id()));
        fs::create_dir_all(root.join("lib")).expect("mkdir");
        fs::write(root.join("lib/clean.dart"), "var ok = 1;\n").expect("write");
        fs::write(root.join("lib/broken.dart"), "void f() { x }\n").expect("write");

        let files = crate::discover::discover(std::slice::from_ref(&root)).expect("discover");
        let run = analyze_files(&files).expect("check core");
        assert_eq!(run.file_count(), 2);
        assert_eq!(run.loc(), 2);
        assert_eq!(run.diagnostics().len(), 1, "broken.dart's syntax error");
        assert_eq!(
            run.diagnostics()[0].code.name, "expected_token",
            "canonical order puts the one diagnostic forward"
        );

        fs::remove_dir_all(&root).ok();
    }
}

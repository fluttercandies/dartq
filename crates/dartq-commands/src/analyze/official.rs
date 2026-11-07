//! The official analyzer backend: one `dart analyze --format machine` child
//! whose rows parse into the [`Diagnostic`] model the engine also produces,
//! so the two backends print through one set of writers and one exit table.
//! The CLI selects the backend, resolves the `dart` binary, and owns the
//! printing and the exit tier; this module owns the spawn and the parse.
//!
//! The row grammar is upstream's `emitMachineFormat` (upstream Dart SDK
//! source, `pkg/dartdev/lib/src/commands/analyze.dart`):
//!
//! ```text
//! SEVERITY|TYPE|CODE|FILE_PATH|LINE|COLUMN|LENGTH|MESSAGE
//! ```
//!
//! `FILE_PATH` and `MESSAGE` backslash-escape `\`, `|`, newline, and carriage
//! return. Rows arrive sorted by upstream's own comparator (severity, path,
//! offset, message — the order [`super::oracle_order`] reproduces for the
//! engine), so their arrival order stands and nothing re-sorts them.
//!
//! The format carries less than the json face prints. A row has no offset:
//! [`OfficialRun::parse`] recovers it from the row's source file, so offsets
//! and end positions stay exact. A row has no correction or context messages,
//! so those json keys are absent on this backend. And upstream leaves ERROR
//! rows of `pubspec.yaml` and `analysis_options.yaml` out of the format while
//! still counting them in its exit code, which is why the exit code comes
//! from the child and is never recomputed from the rows.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use dartq_diagnostics::{
    AnalysisOutput, CodeInfo, Diagnostic, DiagnosticType, FileLayoutProvider, Severity,
};
use dartq_source::{FileId, SourceRange, utf16_len_of};
use rustc_hash::FxHashMap;
use thiserror::Error;

use crate::check::{FileSet, read_source};

/// Why no official analysis ran: the environment failed, not the analysis
/// (the CLI's usage tier). A child that started and failed is an
/// [`OfficialRun`] with its own exit code.
#[derive(Debug, Error)]
pub enum OfficialError {
    /// The toolchain resolver found no `dart` binary.
    #[error("no dart binary found (DARTQ_DART, then PATH); install the Dart SDK or pass --engine")]
    NoDart,
    /// The resolved binary did not start.
    #[error("cannot run {}: {source}", .dart.display())]
    Spawn { dart: PathBuf, source: io::Error },
}

/// One finished `dart analyze` child, shaped for the writers: the same
/// diagnostic rows the engine path yields, plus the streams and the exit code
/// the CLI relays.
pub struct OfficialRun {
    /// The rows that parsed, in the official display order.
    pub diagnostics: Vec<Diagnostic>,
    /// Stdout lines that are not rows, verbatim and in order:
    /// `diagnostics.len() + unparsed.len()` is the child's non-empty stdout
    /// line count.
    pub unparsed: Vec<String>,
    /// The child's stderr, verbatim.
    pub stderr: Vec<u8>,
    /// The child's exit code; `None` when a signal ended it.
    pub code: Option<i32>,
    files: FileSet,
    root: Option<PathBuf>,
}

impl OfficialRun {
    /// Shapes one child's captured streams. `root` is the run's analysis root
    /// (the lon/text file locator's base); `None` keeps absolute spellings.
    #[must_use]
    pub fn parse(stdout: &[u8], stderr: Vec<u8>, code: Option<i32>, root: Option<PathBuf>) -> Self {
        let stdout = String::from_utf8_lossy(stdout);
        let mut rows = Rows::default();
        let mut diagnostics = Vec::new();
        let mut unparsed = Vec::new();
        for line in stdout.lines().filter(|line| !line.is_empty()) {
            match rows.parse(line) {
                Some(row) => diagnostics.push(row),
                None => unparsed.push(line.to_owned()),
            }
        }
        Self {
            diagnostics,
            unparsed,
            stderr,
            code,
            files: rows.files,
            root,
        }
    }

    /// The writers' input: the rows plus this run as their file layout.
    #[must_use]
    pub fn output(&self) -> AnalysisOutput<'_> {
        AnalysisOutput {
            diagnostics: &self.diagnostics,
            files: self,
        }
    }
}

impl FileLayoutProvider for OfficialRun {
    fn file_path(&self, file: FileId) -> &Path {
        self.files.file_path(file)
    }

    fn analysis_root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    fn line_col(&self, file: FileId, utf16_offset: u32) -> (u32, u32) {
        self.files.line_col(file, utf16_offset)
    }
}

/// The child's command line: `<dart> analyze --format machine <args>`, run in
/// the invocation directory. The format is fixed: dartq parses machine rows
/// and re-emits the face the user chose. `args` ends with the positional
/// targets.
#[must_use]
pub fn command(dart: &Path, args: &[OsString]) -> Command {
    let mut command = Command::new(dart);
    command.args(["analyze", "--format", "machine"]).args(args);
    command
}

/// Runs one official analysis to completion and shapes the result. `dart` is
/// the CLI's resolver answer; `root` as in [`OfficialRun::parse`].
///
/// # Errors
///
/// [`OfficialError`] when `dart` is `None` or does not start.
pub fn run(
    dart: Option<&Path>,
    args: &[OsString],
    root: Option<PathBuf>,
) -> Result<OfficialRun, OfficialError> {
    let dart = dart.ok_or(OfficialError::NoDart)?;
    let output = command(dart, args)
        .output()
        .map_err(|source| OfficialError::Spawn {
            dart: dart.to_path_buf(),
            source,
        })?;
    Ok(OfficialRun::parse(
        &output.stdout,
        output.stderr,
        output.status.code(),
        root,
    ))
}

/// One parse pass's interning state, keyed by raw column text: each distinct
/// file reads once, each distinct code interns once.
#[derive(Default)]
struct Rows<'a> {
    files: FileSet,
    /// `FILE_PATH` column → file id and UTF-16 length; `None` when it does
    /// not read.
    opened: FxHashMap<&'a str, Option<(FileId, u32)>>,
    codes: FxHashMap<(&'a str, DiagnosticType), CodeInfo>,
}

impl<'a> Rows<'a> {
    /// The diagnostic `line` spells, or `None` when it is no row: wrong
    /// shape, an unknown severity or type, or a position its file does not
    /// hold (the file changed or vanished after the analyzer read it).
    fn parse(&mut self, line: &'a str) -> Option<Diagnostic> {
        let [severity, dtype, code, path, row, column, length, message] = columns(line)?;
        let severity = match severity {
            "ERROR" => Severity::Error,
            "WARNING" => Severity::Warning,
            "INFO" => Severity::Info,
            _ => return None,
        };
        let dtype = diagnostic_type(dtype)?;
        let position = (row.parse().ok()?, column.parse().ok()?);
        let length = length.parse().ok()?;
        let (file, offset) = self.locate(path, position)?;
        Some(Diagnostic::new(
            severity,
            self.code(code, dtype),
            unescape(message),
            file,
            SourceRange { offset, length },
            None,
        ))
    }

    /// The row's file and the UTF-16 offset of its 1-based position.
    fn locate(&mut self, path: &'a str, position: (u32, u32)) -> Option<(FileId, u32)> {
        let files = &mut self.files;
        let (file, len) = (*self.opened.entry(path).or_insert_with(|| {
            let (canonical, source) = read_source(Path::new(&unescape(path))).ok()?;
            let file = files
                .file_id(&canonical)
                .unwrap_or_else(|| files.push(&source, &canonical));
            Some((file, utf16_len_of(&source)))
        }))?;
        Some((file, offset_of(&self.files, file, len, position)?))
    }

    /// The row's code as a `'static` [`CodeInfo`]: official, lint, and plugin
    /// codes alike, leaked once per distinct code and reclaimed at the
    /// one-shot process's exit (the plugin face's interning posture). The
    /// row's own type stands; `name` is the lowercase form the machine
    /// format uppercased.
    fn code(&mut self, machine: &'a str, dtype: DiagnosticType) -> CodeInfo {
        *self.codes.entry((machine, dtype)).or_insert_with(|| {
            let name = Box::leak(machine.to_ascii_lowercase().into_boxed_str());
            CodeInfo::new(
                name,
                Box::leak(machine.to_owned().into_boxed_str()),
                dtype,
                false,
            )
        })
    }
}

/// Splits one row at its first seven unescaped `|`: the eighth column, the
/// message, keeps every later `|`. `None` when the line has fewer columns.
fn columns(line: &str) -> Option<[&str; 8]> {
    let mut columns = [""; 8];
    let mut bytes = line.bytes().enumerate();
    let mut start = 0;
    let mut filled = 0;
    while filled < 7 {
        match bytes.next()? {
            (_, b'\\') => {
                bytes.next();
            }
            (at, b'|') => {
                columns[filled] = &line[start..at];
                start = at + 1;
                filled += 1;
            }
            _ => {}
        }
    }
    columns[7] = &line[start..];
    Some(columns)
}

/// Inverts upstream's `_escapeForMachineMode`.
fn unescape(field: &str) -> String {
    let mut text = String::with_capacity(field.len());
    let mut chars = field.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            text.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => text.push('\n'),
            Some('r') => text.push('\r'),
            Some(escaped) => text.push(escaped),
            None => text.push('\\'),
        }
    }
    text
}

/// The `TYPE` column's family. `CHECKED_MODE_COMPILE_TIME_ERROR` and
/// `STATIC_TYPE_WARNING` exist only in the plugin protocol's type vocabulary;
/// they fold onto the family dartq models, which keeps the row on every face
/// and changes only the json `type` spelling.
fn diagnostic_type(column: &str) -> Option<DiagnosticType> {
    Some(match column {
        "SYNTACTIC_ERROR" => DiagnosticType::SyntacticError,
        "COMPILE_TIME_ERROR" | "CHECKED_MODE_COMPILE_TIME_ERROR" => {
            DiagnosticType::CompileTimeError
        }
        "STATIC_WARNING" | "STATIC_TYPE_WARNING" => DiagnosticType::StaticWarning,
        "LINT" => DiagnosticType::Lint,
        "HINT" => DiagnosticType::Hint,
        "TODO" => DiagnosticType::Todo,
        _ => return None,
    })
}

/// The UTF-16 offset in `0..=len` at 1-based `position`: a binary search over
/// the layout's own offset → position conversion, which is strictly monotone,
/// so the writers' `line_col` answers the row's position by construction.
fn offset_of(files: &FileSet, file: FileId, len: u32, position: (u32, u32)) -> Option<u32> {
    let (mut low, mut high) = (0, len);
    while low < high {
        let middle = low + (high - low) / 2;
        if files.line_col(file, middle) < position {
            low = middle + 1;
        } else {
            high = middle;
        }
    }
    (files.line_col(file, low) == position).then_some(low)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use dartq_diagnostics::{write_json, write_machine};

    use super::*;

    /// A canonical temp root (the path spelling official rows carry) holding
    /// one file with `source`, and that file's row spelling.
    fn fixture(source: &str) -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir
            .path()
            .canonicalize()
            .expect("canonical root")
            .join("a.dart");
        fs::write(&path, source).expect("write");
        (dir, path.display().to_string())
    }

    fn parse(stdout: &str) -> OfficialRun {
        OfficialRun::parse(stdout.as_bytes(), Vec::new(), Some(0), None)
    }

    /// All three severities, and every escape in the message column, parse
    /// into the row model and re-emit byte-for-byte through the machine
    /// writer, in arrival order. Positions cross a UTF-16 surrogate pair, a
    /// BMP wide character, and a CRLF break.
    #[test]
    fn rows_reemit_byte_exact() {
        let (_dir, path) = fixture("var a = '😀é'; x\r\nvar b = 1;\n");
        let stdout = format!(
            "ERROR|SYNTACTIC_ERROR|EXPECTED_TOKEN|{path}|1|16|1|Expected to find ';'.\n\
             WARNING|STATIC_WARNING|DEAD_CODE|{path}|2|5|1|Dead \\| code \\\\ here\\nnext\\rend.\n\
             INFO|LINT|AVOID_PRINT|{path}|2|1|3|Don't invoke 'print'.\n"
        );
        let run = parse(&stdout);
        assert_eq!(run.unparsed, Vec::<String>::new());

        let mut emitted = Vec::new();
        write_machine(&mut emitted, &run.output()).expect("write");
        assert_eq!(String::from_utf8(emitted).expect("UTF-8"), stdout);

        let [error, warning, info] = run.diagnostics.as_slice() else {
            panic!("three rows: {:?}", run.diagnostics);
        };
        assert_eq!(error.severity, Severity::Error);
        assert_eq!(error.code.name, "expected_token");
        assert_eq!(error.code.machine, "EXPECTED_TOKEN");
        assert_eq!(error.code.dtype, DiagnosticType::SyntacticError);
        assert_eq!(error.range.offset, 15, "the surrogate pair counts as two");
        assert_eq!(error.range.length, 1);

        assert_eq!(warning.severity, Severity::Warning);
        assert_eq!(warning.message, "Dead | code \\ here\nnext\rend.");
        assert_eq!(warning.range.offset, 22, "CRLF is one line break");

        assert_eq!(info.severity, Severity::Info);
        assert_eq!(info.code.dtype, DiagnosticType::Lint);
        assert_eq!(info.range.offset, 18);
    }

    /// The first seven separators delimit; every later `|` is message text.
    #[test]
    fn a_raw_pipe_stays_in_the_message_column() {
        let (_dir, path) = fixture("var a;\n");
        let run = parse(&format!(
            "INFO|HINT|SOME_HINT|{path}|1|1|3|left | right || end\n"
        ));
        assert_eq!(run.unparsed, Vec::<String>::new());
        assert_eq!(run.diagnostics[0].message, "left | right || end");
    }

    /// An escaped separator in the path column stays in the path.
    #[cfg(unix)]
    #[test]
    fn an_escaped_pipe_stays_in_the_path_column() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().canonicalize().expect("root").join("a|b.dart");
        fs::write(&path, "var a;\n").expect("write");
        let spelled = path.display().to_string().replace('|', "\\|");
        let run = parse(&format!("INFO|LINT|X|{spelled}|1|1|3|m\n"));
        assert_eq!(run.unparsed, Vec::<String>::new());
        assert_eq!(run.file_path(run.diagnostics[0].file), path);
    }

    /// A stdout line that is no row rides `unparsed` verbatim, in order, and
    /// no line is lost: the two lists add up to the child's output.
    #[test]
    fn unparseable_lines_keep_their_count() {
        let (dir, path) = fixture("var a;\n");
        let missing = dir.path().join("gone.dart").display().to_string();
        let junk = [
            "Please report this at dartbug.com.".to_owned(),
            format!("FATAL|LINT|X|{path}|1|1|3|unknown severity"),
            format!("INFO|NEW_TYPE|X|{path}|1|1|3|unknown type"),
            format!("INFO|LINT|X|{path}|one|1|3|non-numeric line"),
            format!("INFO|LINT|X|{path}|1|1"),
            format!("INFO|LINT|X|{missing}|1|1|3|file gone"),
            format!("INFO|LINT|X|{path}|9|1|3|position past the file"),
        ];
        let good = format!("INFO|LINT|X|{path}|1|1|3|fine");
        let stdout = format!("{}\n\n{good}\n", junk.join("\n"));

        let run = parse(&stdout);
        assert_eq!(run.unparsed, junk);
        assert_eq!(run.diagnostics.len(), 1, "the good row still parses");
    }

    /// The json writer over a parsed row: the offset and end position come
    /// from the source, and the key set is the machine format's (no
    /// correction, no context). A leading BOM is outside the offsets, as in
    /// the analyzer's read layer.
    #[test]
    fn the_json_face_recovers_offsets_from_the_source() {
        let (_dir, path) = fixture("\u{FEFF}void f() {}\nvoid g() { x }\n");
        let run = parse(&format!(
            "ERROR|SYNTACTIC_ERROR|EXPECTED_TOKEN|{path}|2|12|1|Expected to find ';'.\n"
        ));
        let mut emitted = Vec::new();
        write_json(&mut emitted, &run.output()).expect("write");
        assert_eq!(
            String::from_utf8(emitted).expect("UTF-8"),
            format!(
                "{{\"version\":1,\"diagnostics\":[{{\"code\":\"expected_token\",\
                 \"severity\":\"ERROR\",\"type\":\"SYNTACTIC_ERROR\",\"location\":{{\
                 \"file\":\"{path}\",\"range\":{{\
                 \"start\":{{\"offset\":23,\"line\":2,\"column\":12}},\
                 \"end\":{{\"offset\":24,\"line\":2,\"column\":13}}}}}},\
                 \"problemMessage\":\"Expected to find ';'.\"}}]}}\n"
            )
        );
    }

    /// The plugin protocol's two extra type spellings fold onto a modeled
    /// family instead of leaving the data face.
    #[test]
    fn plugin_only_types_fold_onto_a_modeled_family() {
        assert_eq!(
            diagnostic_type("CHECKED_MODE_COMPILE_TIME_ERROR"),
            Some(DiagnosticType::CompileTimeError)
        );
        assert_eq!(
            diagnostic_type("STATIC_TYPE_WARNING"),
            Some(DiagnosticType::StaticWarning)
        );
    }

    /// The child always runs the machine format, ahead of the forwarded
    /// arguments; nothing here starts a process.
    #[test]
    fn the_command_fixes_the_machine_format() {
        let args = [OsString::from("--fatal-infos"), OsString::from("lib")];
        let command = command(Path::new("/sdk/bin/dart"), &args);
        assert_eq!(command.get_program(), "/sdk/bin/dart");
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            ["analyze", "--format", "machine", "--fatal-infos", "lib"]
        );
    }

    /// A resolver miss and a binary that does not start are errors, not runs.
    #[test]
    fn environment_failures_are_errors() {
        assert!(matches!(run(None, &[], None), Err(OfficialError::NoDart)));
        let dir = tempfile::tempdir().expect("tempdir");
        let absent = dir.path().join("dart");
        let error = run(Some(&absent), &[], None).err().expect("spawn fails");
        assert!(matches!(error, OfficialError::Spawn { .. }));
        assert!(
            error.to_string().contains(&absent.display().to_string()),
            "{error}"
        );
    }
}

//! The official analyzer engine: the Dart analysis server's own rows,
//! reached either through the resident server (`dartq-analyzer-host`) or
//! through one `dart analyze --format json` child per run, and shaped into
//! the [`Diagnostic`] model the dartq engine also produces, so both engines
//! print through one set of writers and one exit table.
//!
//! Both paths carry the server's `AnalysisError` rows whole
//! ([`dartq_analyzer_host::Row`]): positions come from the server (no
//! source re-read), and the json face re-emits `dart analyze`'s envelope
//! from the rows ([`OfficialRun::write_json`]), `correctionMessage`,
//! `contextMessages` and `documentation` included. Upstream leaves ERROR
//! rows of `pubspec.yaml` and `analysis_options.yaml` out of its machine
//! and json faces while counting them in its exit code, so the exit code is
//! the child's (per run) or dartdev's tiers over the full report
//! (resident), never recomputed from the printed rows.

use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use dartq_analyzer_host::{Report, Row, report};
use dartq_diagnostics::{
    AnalysisOutput, CodeInfo, ContextMessage, Diagnostic, DiagnosticType, FileLayoutProvider,
    Severity,
};
use dartq_source::{FileId, FileTable, SourceRange};
use rustc_hash::FxHashMap;
use thiserror::Error;

/// Why no official analysis ran: the environment failed, not the analysis
/// (the CLI's usage tier). A child that started and failed is an
/// [`OfficialRun`] with its own exit code.
#[derive(Debug, Error)]
pub enum OfficialError {
    /// The toolchain resolver found no `dart` binary.
    #[error(
        "no dart binary found (DARTQ_DART, then PATH); install the Dart SDK or pass --engine dartq"
    )]
    NoDart,
    /// The resolved binary did not start.
    #[error("cannot run {}: {source}", .dart.display())]
    Spawn { dart: PathBuf, source: io::Error },
    /// The analyzer's process tree crossed the memory budget and was
    /// killed — retryable, never a freeze.
    #[error(
        "analyzer exceeded the memory budget ({budget_gb} GB configured; measured {measured_gb:.1} GB) — killed; raise analyzer.official.memory_budget_gb in dartq.yaml or retry"
    )]
    BudgetKilled { budget_gb: u64, measured_gb: f64 },
}

/// The RSS poll cadence of the per-run budget watchdog.
#[cfg(unix)]
const RSS_POLL: std::time::Duration = std::time::Duration::from_secs(1);

/// One finished official analysis, shaped for the writers.
pub struct OfficialRun {
    /// The rows in the official display order, as diagnostics.
    pub diagnostics: Vec<Diagnostic>,
    /// The same rows as the server reported them (the json face's source).
    rows: Vec<Row>,
    /// Stdout lines that are not the envelope, verbatim and in order.
    pub unparsed: Vec<String>,
    /// The child's stderr, verbatim (empty on the resident path).
    pub stderr: Vec<u8>,
    /// The exit code; `None` when a signal ended the child.
    pub code: Option<i32>,
    layout: Layout,
    root: Option<PathBuf>,
}

impl OfficialRun {
    /// Shapes one `dart analyze --format json` child's captured streams.
    /// `root` is the run's analysis root (the lon/text file locator's
    /// base); `None` keeps absolute spellings.
    #[must_use]
    pub fn parse(stdout: &[u8], stderr: Vec<u8>, code: Option<i32>, root: Option<PathBuf>) -> Self {
        let stdout = String::from_utf8_lossy(stdout);
        let mut rows = None;
        let mut unparsed = Vec::new();
        for line in stdout.lines().filter(|line| !line.is_empty()) {
            match rows
                .is_none()
                .then(|| report::parse_envelope(line))
                .flatten()
            {
                Some(parsed) => rows = Some(parsed),
                None => unparsed.push(line.to_owned()),
            }
        }
        Self::of_rows(rows.unwrap_or_default(), unparsed, stderr, code, root)
    }

    /// Shapes a resident server's report: the exit code is dartdev's tiers
    /// under the forwarded fatal flags.
    #[must_use]
    pub fn of_report(
        report: Report,
        fatal_warnings: bool,
        fatal_infos: bool,
        root: Option<PathBuf>,
    ) -> Self {
        let code = i32::from(report.exit_code(fatal_warnings, fatal_infos));
        Self::of_rows(report.rows, Vec::new(), Vec::new(), Some(code), root)
    }

    fn of_rows(
        rows: Vec<Row>,
        mut unparsed: Vec<String>,
        stderr: Vec<u8>,
        code: Option<i32>,
        root: Option<PathBuf>,
    ) -> Self {
        let mut layout = Layout::default();
        let mut codes = FxHashMap::default();
        let mut diagnostics = Vec::with_capacity(rows.len());
        let mut kept = Vec::with_capacity(rows.len());
        for row in rows {
            match layout.diagnostic(&row, &mut codes) {
                Some(diagnostic) => {
                    diagnostics.push(diagnostic);
                    kept.push(row);
                }
                None => unparsed.push(serde_json::to_string(&row).expect("a row serializes")),
            }
        }
        Self {
            diagnostics,
            rows: kept,
            unparsed,
            stderr,
            code,
            layout,
            root,
        }
    }

    /// The kept (modelable) rows in dartdev order — the face writer's
    /// input (the one render `render_official_face` performs over the
    /// same set the resident daemon hands it).
    #[must_use]
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// The writers' input: the rows plus this run as their file layout.
    #[must_use]
    pub fn output(&self) -> AnalysisOutput<'_> {
        AnalysisOutput {
            diagnostics: &self.diagnostics,
            files: self,
        }
    }

    /// `dart analyze --format json`'s envelope over this run's rows,
    /// without the `memory` key.
    ///
    /// # Errors
    ///
    /// The writer's error.
    pub fn write_json(&self, out: &mut impl Write) -> io::Result<()> {
        report::write_json(out, &self.rows)
    }
}

impl FileLayoutProvider for OfficialRun {
    fn file_path(&self, file: FileId) -> &Path {
        self.layout.files.path(file).expect("a row's file")
    }

    fn analysis_root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    fn line_col(&self, file: FileId, utf16_offset: u32) -> (u32, u32) {
        self.layout
            .positions
            .get(&(file, utf16_offset))
            .copied()
            .unwrap_or((0, 0))
    }
}

/// The files the rows name and the positions the server reported for
/// them: every offset a writer asks about is a row's start or end.
#[derive(Default)]
struct Layout {
    files: FileTable,
    positions: FxHashMap<(FileId, u32), (u32, u32)>,
}

impl Layout {
    fn locate(&mut self, location: &report::Location) -> (FileId, SourceRange) {
        // The server spells canonical paths already: no realpath per file.
        let path = Path::new(&*location.file);
        let file = self.files.intern_normalized(path, path.to_path_buf());
        let range = SourceRange {
            offset: location.offset,
            length: location.length,
        };
        let position =
            |line: Option<u32>, column: Option<u32>| (line.unwrap_or(0), column.unwrap_or(0));
        self.positions.insert(
            (file, range.offset),
            position(location.start_line, location.start_column),
        );
        self.positions
            .entry((file, range.end()))
            .or_insert_with(|| position(location.end_line, location.end_column));
        (file, range)
    }

    /// The row as a diagnostic; `None` for a severity or type dartq does
    /// not model.
    fn diagnostic(
        &mut self,
        row: &Row,
        codes: &mut FxHashMap<(String, DiagnosticType), CodeInfo>,
    ) -> Option<Diagnostic> {
        let severity = match row.severity.as_str() {
            "ERROR" => Severity::Error,
            "WARNING" => Severity::Warning,
            "INFO" => Severity::Info,
            _ => return None,
        };
        let dtype = diagnostic_type(&row.kind)?;
        let code = *codes.entry((row.code.clone(), dtype)).or_insert_with(|| {
            // Official, lint and plugin codes alike are leaked once per
            // distinct code and reclaimed at the one-shot process's exit.
            CodeInfo::new(
                Box::leak(row.code.clone().into_boxed_str()),
                Box::leak(row.code.to_ascii_uppercase().into_boxed_str()),
                dtype,
                false,
            )
        });
        let (file, range) = self.locate(&row.location);
        let context = row
            .context_messages
            .iter()
            .map(|context| {
                let (file, range) = self.locate(&context.location);
                ContextMessage {
                    message: context.message.clone().into(),
                    file,
                    range,
                }
            })
            .collect();
        Some(
            Diagnostic::new(
                severity,
                code,
                row.message.clone(),
                file,
                range,
                row.correction.clone(),
            )
            .with_context(context),
        )
    }
}

/// The `type` family. `CHECKED_MODE_COMPILE_TIME_ERROR` and
/// `STATIC_TYPE_WARNING` exist only in the plugin protocol's type
/// vocabulary; they fold onto the family dartq models, which changes only
/// the lon/text faces (the json face prints the row's own type).
fn diagnostic_type(kind: &str) -> Option<DiagnosticType> {
    Some(match kind {
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

/// The child's command line: `<dart> analyze --format json <args>`, run in
/// the invocation directory. The format is fixed: the envelope carries
/// every row field, and dartq re-emits the face the user chose. `args` ends
/// with the positional targets.
#[must_use]
pub fn command(dart: &Path, args: &[OsString]) -> Command {
    let mut command = Command::new(dart);
    command.args(["analyze", "--format", "json"]).args(args);
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
    run_with_budget(dart, args, root, None)
}

/// [`run`] with an RSS budget on the child's whole process tree: `None` or
/// `0` leaves it unbounded, a positive budget polls the tree once per
/// [`RSS_POLL`] and kills it the first time it is over.
///
/// # Errors
///
/// [`OfficialError`] as [`run`], plus [`OfficialError::BudgetKilled`].
pub fn run_with_budget(
    dart: Option<&Path>,
    args: &[OsString],
    root: Option<PathBuf>,
    budget_bytes: Option<u64>,
) -> Result<OfficialRun, OfficialError> {
    let dart = dart.ok_or(OfficialError::NoDart)?;
    #[cfg(unix)]
    {
        run_watchdog(dart, args, root, budget_bytes.filter(|&bytes| bytes > 0))
    }
    #[cfg(not(unix))]
    {
        // No process-tree surface off unix: the budget is unenforceable.
        let _ = budget_bytes;
        let output = command(dart, args)
            .env_remove("DARTQ_SHIM")
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
}

#[cfg(unix)]
fn run_watchdog(
    dart: &Path,
    args: &[OsString],
    root: Option<PathBuf>,
    budget: Option<u64>,
) -> Result<OfficialRun, OfficialError> {
    use std::io::Read as _;
    use std::process::Stdio;

    let spawn_error = |source| OfficialError::Spawn {
        dart: dart.to_path_buf(),
        source,
    };
    let mut child = command(dart, args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // A nested `dartq` the child spawns must not take itself for the
        // PATH shim.
        .env_remove("DARTQ_SHIM")
        .spawn()
        .map_err(spawn_error)?;
    let drain = |mut pipe: Box<dyn io::Read + Send>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = pipe.read_to_end(&mut buf);
            buf
        })
    };
    let stdout = drain(Box::new(child.stdout.take().expect("piped stdout")));
    let stderr = drain(Box::new(child.stderr.take().expect("piped stderr")));
    let mut peak = 0;
    let killed = loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let stdout = stdout.join().unwrap_or_default();
                let stderr = stderr.join().unwrap_or_default();
                return Ok(OfficialRun::parse(&stdout, stderr, status.code(), root));
            }
            Ok(None) => {}
            Err(source) => break Err(spawn_error(source)),
        }
        if let Some(budget) = budget {
            let measured = dartq_toolchain::proc::tree_rss_bytes(child.id());
            peak = peak.max(measured);
            if measured > budget {
                break Ok(budget);
            }
        }
        std::thread::sleep(RSS_POLL);
    };
    dartq_toolchain::proc::kill_tree(child.id());
    let _ = child.wait();
    let _ = stdout.join();
    let _ = stderr.join();
    let budget = killed?;
    Err(OfficialError::BudgetKilled {
        budget_gb: budget >> 30,
        measured_gb: peak as f64 / f64::from(1u32 << 30),
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use dartq_diagnostics::{write_json, write_machine};

    use super::*;

    /// One envelope row with a correction, a documentation url and a
    /// context message — the fields the old machine-format parse lost.
    const ENVELOPE: &str = concat!(
        r#"{"version":1,"diagnostics":[{"code":"duplicate_definition","severity":"ERROR","#,
        r#""type":"COMPILE_TIME_ERROR","location":{"file":"/p/lib/a.dart","range":{"#,
        r#""start":{"offset":20,"line":2,"column":5},"end":{"offset":23,"line":2,"column":8}}},"#,
        r#""problemMessage":"The name 'foo' is already defined.","#,
        r#""correctionMessage":"Try renaming one of the declarations.","#,
        r#""contextMessages":[{"location":{"file":"/p/lib/a.dart","range":{"#,
        r#""start":{"offset":4,"line":1,"column":5},"end":{"offset":7,"line":1,"column":8}}},"#,
        r#""message":"The first definition of this name."}],"#,
        r#""documentation":"https://dart.dev/diagnostics/duplicate_definition"},"#,
        r#"{"code":"avoid_print","severity":"INFO","type":"LINT","location":{"file":"/p/lib/a.dart","range":{"#,
        r#""start":{"offset":30,"line":3,"column":1},"end":{"offset":35,"line":3,"column":6}}},"#,
        r#""problemMessage":"Don't invoke 'print' in production code.","#,
        r#""correctionMessage":"Try using a logging framework.","#,
        r#""documentation":"https://dart.dev/diagnostics/avoid_print"}]}"#
    );

    fn parse(stdout: &str) -> OfficialRun {
        OfficialRun::parse(stdout.as_bytes(), Vec::new(), Some(3), None)
    }

    /// The #4122 pin: the per-run json face re-emits the child's envelope
    /// byte for byte — correctionMessage, contextMessages and documentation
    /// included — and the rows reach the diagnostic model whole.
    #[test]
    fn the_json_face_keeps_correction_context_and_documentation() {
        let run = parse(&format!("{ENVELOPE}\n"));
        assert_eq!(run.unparsed, Vec::<String>::new());
        let mut emitted = Vec::new();
        run.write_json(&mut emitted).expect("write");
        assert_eq!(
            String::from_utf8(emitted).expect("UTF-8"),
            format!("{ENVELOPE}\n")
        );

        let [error, lint] = run.diagnostics.as_slice() else {
            panic!("two rows: {:?}", run.diagnostics);
        };
        assert_eq!(error.code.name, "duplicate_definition");
        assert_eq!(error.code.machine, "DUPLICATE_DEFINITION");
        assert_eq!(
            error.correction.as_deref(),
            Some("Try renaming one of the declarations.")
        );
        let context = error.context.as_deref().expect("one context message");
        assert_eq!(
            context[0].range,
            SourceRange {
                offset: 4,
                length: 3
            }
        );
        assert_eq!(run.line_col(error.file, 20), (2, 5));
        assert_eq!(run.line_col(error.file, 23), (2, 8));
        assert_eq!(lint.code.dtype, DiagnosticType::Lint);

        // The shared writers over the same rows: machine positions come
        // from the server, the engine-side json writer agrees on locations.
        let mut machine = Vec::new();
        write_machine(&mut machine, &run.output()).expect("write");
        assert_eq!(
            String::from_utf8(machine).expect("UTF-8"),
            "ERROR|COMPILE_TIME_ERROR|DUPLICATE_DEFINITION|/p/lib/a.dart|2|5|3|The name 'foo' is already defined.\n\
             INFO|LINT|AVOID_PRINT|/p/lib/a.dart|3|1|5|Don't invoke 'print' in production code.\n"
        );
        let mut engine_json = Vec::new();
        write_json(&mut engine_json, &run.output()).expect("write");
        assert!(String::from_utf8(engine_json).expect("UTF-8").contains(
            r#""range":{"start":{"offset":20,"line":2,"column":5},"end":{"offset":23,"line":2,"column":8}}"#
        ));
    }

    /// Lines around the envelope ride `unparsed` verbatim; a row of an
    /// unknown severity or type is kept as one unparsed line, not lost.
    #[test]
    fn non_envelope_lines_and_unknown_rows_are_kept_unparsed() {
        let odd = ENVELOPE.replace(r#""severity":"INFO""#, r#""severity":"FATAL""#);
        let run = parse(&format!("Please report this at dartbug.com.\n{odd}\n"));
        assert_eq!(run.diagnostics.len(), 1);
        assert_eq!(run.unparsed.len(), 2);
        assert_eq!(run.unparsed[0], "Please report this at dartbug.com.");
        assert!(run.unparsed[1].contains("FATAL"), "{:?}", run.unparsed);
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

    /// The child always runs the json format, ahead of the forwarded
    /// arguments; nothing here starts a process.
    #[test]
    fn the_command_fixes_the_json_format() {
        let args = [OsString::from("--fatal-infos"), OsString::from("lib")];
        let command = command(Path::new("/sdk/bin/dart"), &args);
        assert_eq!(command.get_program(), "/sdk/bin/dart");
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            ["analyze", "--format", "json", "--fatal-infos", "lib"]
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

    /// The RSS watchdog kills an over-budget child on its first poll.
    #[cfg(unix)]
    #[test]
    fn an_over_budget_analyzer_is_killed_and_reaped() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().expect("tempdir");
        let fake_dart = dir.path().join("dart");
        fs::write(&fake_dart, "#!/bin/sh\nexec sleep 30\n").expect("write fake dart");
        fs::set_permissions(&fake_dart, fs::Permissions::from_mode(0o755)).expect("chmod");
        let started = std::time::Instant::now();
        let error = run_with_budget(Some(&fake_dart), &[], None, Some(1))
            .err()
            .expect("the budget trips");
        let OfficialError::BudgetKilled {
            budget_gb,
            measured_gb,
        } = error
        else {
            panic!("expected BudgetKilled, got {error:?}");
        };
        assert_eq!(budget_gb, 0, "1 byte is 0 GiB — the configured figure");
        assert!(measured_gb > 0.0, "the watchdog measured a live tree");
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
        assert!(
            error
                .to_string()
                .contains("analyzer.official.memory_budget_gb")
        );
    }

    /// An unlimited budget (`0` or `None`) never kills.
    #[cfg(unix)]
    #[test]
    fn an_unlimited_budget_lets_the_child_finish() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().expect("tempdir");
        let fake_dart = dir.path().join("dart");
        fs::write(&fake_dart, "#!/bin/sh\nexit 0\n").expect("write fake dart");
        fs::set_permissions(&fake_dart, fs::Permissions::from_mode(0o755)).expect("chmod");
        for budget in [None, Some(0)] {
            let run = run_with_budget(Some(&fake_dart), &[], None, budget).expect("finishes");
            assert_eq!(run.code, Some(0));
            assert!(run.diagnostics.is_empty());
        }
    }
}

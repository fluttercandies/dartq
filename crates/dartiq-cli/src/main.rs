//! dartiq CLI — the M0 differential face (`dartiq check`) plus `dartiq version`.
//!
//! `check` drives the M0 pipeline (engine-api-spec §2.1): bytes → scan →
//! `@dart` override rescan → parse, with all diagnostics written through
//! `dartiq-diagnostics`' oracle-exact writers. It stops there by design: no
//! pub/package/SDK/engine involvement (the spec's M1 forbid-list).
//!
//! Published contract (engine-api-spec §4.2, oracle-machine-format §3):
//! - diagnostic lines go to stdout only; usage/internal failures report on
//!   stderr and keep stdout empty;
//! - exit codes: 0 clean or info-only · 1 fatal infos · 2 fatal warnings ·
//!   3 errors · 4 internal failure · 64 usage error. The differential
//!   harness treats {0,1,2,3} as a valid diagnostic set (Jaccard input);
//!   4/64 are harness-own failures.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::rc::Rc;

use dartiq_diagnostics::{
    AnalysisOutput, CodeRegistry, Diagnostic, FileId, FileLayoutProvider, Severity, SourceRange,
    write_json, write_machine,
};
use dartiq_language::{
    DEFAULT_LANGUAGE_VERSION, Feature, FeatureSet, LanguageVersion, triple_shift_enabled,
};
use dartiq_parser::{Message, Parser, ParserFeatures, ParserListener, TokenSpan};
use dartiq_scanner::{ScanConfig, ScanResult, scan};
use dartiq_source::{FileTable, LineIndex};

const USAGE_LINE: &str = "usage: dartiq check [--format machine|json] [--no-fatal-warnings] \
 [--fatal-infos] [PATH...]
       dartiq version";

/// Process exit codes (engine-api-spec §4.2, pinned to analyze.dart:719–731).
mod exit {
    pub const CLEAN: u8 = 0;
    pub const FATAL_INFOS: u8 = 1;
    pub const FATAL_WARNINGS: u8 = 2;
    pub const ERRORS: u8 = 3;
    pub const INTERNAL_ERROR: u8 = 4;
    pub const USAGE: u8 = 64;
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    ExitCode::from(run(&args))
}

fn run(args: &[String]) -> u8 {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match parse_args(&args) {
        Ok(Command::Version) => {
            println!("dartiq {}", env!("CARGO_PKG_VERSION"));
            exit::CLEAN
        }
        Ok(Command::Check(check_args)) => check(&check_args),
        Err(message) => {
            eprintln!("error: {message}\n\n{USAGE_LINE}");
            exit::USAGE
        }
    }
}

enum Command {
    Check(CheckArgs),
    Version,
}

struct CheckArgs {
    format: Format,
    /// On by default (`--no-fatal-warnings` clears it) — mirrors the oracle.
    fatal_warnings: bool,
    /// Off by default (`--fatal-infos` sets it) — mirrors the oracle.
    fatal_infos: bool,
    /// Targets; empty resolves to the current directory.
    paths: Vec<PathBuf>,
}

enum Format {
    Machine,
    Json,
}

fn parse_args(args: &[&str]) -> Result<Command, String> {
    match args {
        [] | ["version" | "--version"] => Ok(Command::Version),
        ["check", rest @ ..] => {
            let mut check = CheckArgs {
                format: Format::Machine,
                fatal_warnings: true,
                fatal_infos: false,
                paths: Vec::new(),
            };
            let mut rest = rest.iter().copied();
            while let Some(flag) = rest.next() {
                match flag {
                    "--no-fatal-warnings" => check.fatal_warnings = false,
                    "--fatal-infos" => check.fatal_infos = true,
                    "--format" => {
                        let value = rest
                            .next()
                            .ok_or("`--format` needs a value (machine|json)")?;
                        check.format = parse_format(value)?;
                    }
                    value if value.starts_with("--format=") => {
                        check.format = parse_format(&value["--format=".len()..])?;
                    }
                    value if value.starts_with('-') => {
                        return Err(format!("unrecognized option `{value}`"));
                    }
                    path => check.paths.push(PathBuf::from(path)),
                }
            }
            if check.paths.is_empty() {
                check.paths.push(PathBuf::from("."));
            }
            Ok(Command::Check(check))
        }
        [first, ..] if first.starts_with('-') => Err(format!("unrecognized option `{first}`")),
        [first, ..] => Err(format!("unknown command `{first}`")),
    }
}

fn parse_format(value: &str) -> Result<Format, String> {
    match value {
        "machine" => Ok(Format::Machine),
        "json" => Ok(Format::Json),
        other => Err(format!("invalid `--format` value `{other}` (machine|json)")),
    }
}

/// Severity arithmetic (engine-api-spec §4.2): the highest fatal tier present.
/// Errors are always fatal (3); warnings fatal unless `--no-fatal-warnings`
/// (2); infos fatal only under `--fatal-infos` (1).
fn severity_exit(diags: &[Diagnostic], fatal_warnings: bool, fatal_infos: bool) -> u8 {
    diags
        .iter()
        .map(|d| match d.severity {
            Severity::Error => exit::ERRORS,
            Severity::Warning if fatal_warnings => exit::FATAL_WARNINGS,
            Severity::Info if fatal_infos => exit::FATAL_INFOS,
            _ => exit::CLEAN,
        })
        .max()
        .unwrap_or(exit::CLEAN)
}

enum DiscoverError {
    /// Path argument does not exist → usage error (64), oracle wording.
    Missing(String),
    /// Walk I/O failure → internal error (4).
    Io(io::Error),
}

/// Canonical, sorted, deduplicated `.dart` file set under `paths`. A file
/// argument is taken as-is; a directory argument is walked recursively,
/// skipping dot-directories and build/system directories, and never
/// descending through symlinked directories (cycle safety).
fn discover(paths: &[PathBuf]) -> Result<BTreeSet<PathBuf>, DiscoverError> {
    fn walk(dir: &Path, files: &mut BTreeSet<PathBuf>) -> io::Result<()> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            let path = entry.path();
            if file_type.is_dir() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.starts_with('.') || name == "build" || name == "node_modules" {
                    continue;
                }
                walk(&path, files)?;
            } else if file_type.is_file() && path.extension().is_some_and(|ext| ext == "dart") {
                files.insert(path);
            }
        }
        Ok(())
    }

    let mut files = BTreeSet::new();
    for path in paths {
        let meta =
            fs::metadata(path).map_err(|_| DiscoverError::Missing(path.display().to_string()))?;
        if meta.is_file() {
            files.insert(path.clone());
        } else {
            walk(path, &mut files).map_err(DiscoverError::Io)?;
        }
    }
    Ok(files)
}

fn check(args: &CheckArgs) -> u8 {
    let files = match discover(&args.paths) {
        Ok(files) => files,
        Err(DiscoverError::Missing(path)) => {
            eprintln!("error: Directory or file doesn't exist: {path}\n\n{USAGE_LINE}");
            return exit::USAGE;
        }
        Err(DiscoverError::Io(error)) => {
            eprintln!("dartiq: internal error: {error}");
            return exit::INTERNAL_ERROR;
        }
    };

    // Analysis runs under the crash tier (engine-api-spec §4.2: internal
    // failure → 4, the oracle `_Result.crash(4)` analog): a panic anywhere in
    // the pipeline aborts the run with stderr kept from the panic hook and
    // stdout kept empty — a partial diagnostic set must never masquerade as a
    // valid {0,1,2,3} result.
    let analyzed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut files_state = FileSet::default();
        let mut diags = Vec::new();
        for path in &files {
            let (canonical, source) = read_source(path)?;
            let file = files_state.push(&source, &canonical);
            diags.extend(analyze_file(&source, file));
        }
        // Canonical order: file discovery order (sorted paths), then source
        // position, then code/message — deterministic output at the engine-spec
        // §1.3 canonical-sort granularity M0 has.
        diags.sort_by(|a, b| {
            a.file
                .index()
                .cmp(&b.file.index())
                .then_with(|| a.range.offset.cmp(&b.range.offset))
                .then_with(|| a.code.machine.cmp(b.code.machine))
                .then_with(|| a.message.cmp(&b.message))
        });
        Ok::<_, io::Error>((files_state, diags))
    }));
    let (files_state, diags) = match analyzed {
        Ok(Ok(analyzed)) => analyzed,
        Ok(Err(error)) => {
            eprintln!("dartiq: internal error: {error}");
            return exit::INTERNAL_ERROR;
        }
        Err(_) => {
            eprintln!("dartiq: internal error: analysis panicked");
            return exit::INTERNAL_ERROR;
        }
    };

    let stdout = io::stdout();
    let mut out = stdout.lock();
    let output = AnalysisOutput {
        diagnostics: &diags,
        files: &files_state,
    };
    let written = match args.format {
        Format::Machine => write_machine(&mut out, &output),
        Format::Json => write_json(&mut out, &output),
    };
    if let Err(error) = written.and_then(|()| out.flush()) {
        eprintln!("dartiq: internal error: writing diagnostics failed: {error}");
        return exit::INTERNAL_ERROR;
    }
    severity_exit(&diags, args.fatal_warnings, args.fatal_infos)
}

/// Canonical absolute path (the oracle prints symlink-resolved paths) plus
/// strict UTF-8 source. A read/decode failure is an internal error (4), never
/// a silently skipped file — lossy output would be format drift downstream.
fn read_source(path: &Path) -> io::Result<(PathBuf, String)> {
    let canonical = path.canonicalize()?;
    let source = fs::read_to_string(&canonical)?;
    Ok((canonical, source))
}

/// Per-file state behind [`FileLayoutProvider`]: canonical paths live in the
/// [`FileTable`], positions resolve through the production [`LineIndex`] path
/// (the same conversion the diagnostics goldens pin).
#[derive(Default)]
struct FileSet {
    table: FileTable,
    sources: Vec<String>,
    indexes: Vec<LineIndex>,
}

impl FileSet {
    /// Interns `path` (canonicalized by the caller) and registers its layout.
    fn push(&mut self, source: &str, path: &Path) -> FileId {
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
        let source = &self.sources[index];
        let index = &self.indexes[index];
        // Diagnostic offsets are UTF-16; LineIndex works in bytes.
        let byte = index.utf16_to_byte(source, utf16_offset);
        let (line, col_bytes) = index.byte_to_line_col(byte);
        let line_start = byte - (col_bytes - 1);
        // Column counts UTF-16 code units within the line (official
        // `LineInfo.getLocation` semantics).
        let column =
            index.byte_to_utf16(source, byte) - index.byte_to_utf16(source, line_start) + 1;
        (line, column)
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
/// `dartiq_diagnostics::Diagnostic`, including file attribution; event spans
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
    use dartiq_diagnostics::EXPECTED_TOKEN;

    fn parse_args_of(args: &[&str]) -> Result<Command, String> {
        parse_args(args)
    }

    fn check_args(args: &[&str]) -> CheckArgs {
        match parse_args_of(args) {
            Ok(Command::Check(check)) => check,
            Ok(Command::Version) => panic!("expected check command, got version"),
            Err(message) => panic!("expected check command, got error: {message}"),
        }
    }

    #[test]
    fn check_defaults() {
        let check = check_args(&["check"]);
        assert!(matches!(check.format, Format::Machine));
        assert!(check.fatal_warnings, "fatal-warnings default on");
        assert!(!check.fatal_infos, "fatal-infos default off");
        assert_eq!(check.paths, vec![PathBuf::from(".")], "default target cwd");
    }

    #[test]
    fn check_flags_both_spellings() {
        let check = check_args(&[
            "check",
            "--format=json",
            "--no-fatal-warnings",
            "--fatal-infos",
            "/tmp/a",
            "/tmp/b",
        ]);
        assert!(matches!(check.format, Format::Json));
        assert!(!check.fatal_warnings);
        assert!(check.fatal_infos);
        assert_eq!(
            check.paths,
            vec![PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b")]
        );

        let check = check_args(&["check", "--format", "machine", "dir/"]);
        assert!(matches!(check.format, Format::Machine));
        assert_eq!(check.paths, vec![PathBuf::from("dir/")]);
    }

    #[test]
    fn usage_errors() {
        for args in [
            &["check", "--format"][..],
            &["check", "--format", "yaml"],
            &["check", "--format=yaml"],
            &["check", "--bogus"],
            &["check", "-x"],
            &["frobnicate"],
            &["--bogus"],
        ] {
            assert!(parse_args_of(args).is_err(), "must reject {args:?}");
        }
    }

    #[test]
    fn version_forms() {
        for args in [&[][..], &["version"], &["--version"]] {
            assert!(
                matches!(parse_args_of(args), Ok(Command::Version)),
                "must accept {args:?}"
            );
        }
    }

    fn one_diag(severity: Severity) -> Diagnostic {
        let mut table = FileTable::new();
        let file = table.intern(Path::new("/c/lib/a.dart"));
        Diagnostic::new(
            severity,
            EXPECTED_TOKEN,
            "m",
            file,
            SourceRange {
                offset: 0,
                length: 0,
            },
            None,
        )
    }

    /// The engine-api-spec §4.2 table, row by row.
    #[test]
    fn exit_code_severity_arithmetic() {
        let clean: Vec<Diagnostic> = Vec::new();
        let info = one_diag(Severity::Info);
        let warning = one_diag(Severity::Warning);
        let error = one_diag(Severity::Error);

        assert_eq!(severity_exit(&clean, true, false), exit::CLEAN);
        // INFO only, default flags → 0; with --fatal-infos → 1
        assert_eq!(
            severity_exit(std::slice::from_ref(&info), true, false),
            exit::CLEAN
        );
        assert_eq!(
            severity_exit(std::slice::from_ref(&info), true, true),
            exit::FATAL_INFOS
        );
        // WARNING, default → 2; --no-fatal-warnings → 0
        assert_eq!(
            severity_exit(std::slice::from_ref(&warning), true, false),
            exit::FATAL_WARNINGS
        );
        assert_eq!(
            severity_exit(std::slice::from_ref(&warning), false, false),
            exit::CLEAN
        );
        // warnings suppressed but fatal infos → the info tier still counts
        assert_eq!(
            severity_exit(&[warning, info.clone()], false, true),
            exit::FATAL_INFOS
        );
        // any ERROR wins regardless of flags
        assert_eq!(
            severity_exit(std::slice::from_ref(&error), true, false),
            exit::ERRORS
        );
        assert_eq!(severity_exit(&[error], false, true), exit::ERRORS);
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

    /// Above-latest override alone → the warning tier (2), cleared by
    /// `--no-fatal-warnings`; no other check behavior changes.
    #[test]
    fn check_exit_tiers_for_above_latest_override() {
        let root = std::env::temp_dir().join(format!("dq_cli_override_{}", std::process::id()));
        fs::create_dir_all(&root).expect("mkdir");
        fs::write(root.join("override.dart"), "// @dart = 99.9\nvar ok = 1;\n").expect("write");

        let args = CheckArgs {
            format: Format::Machine,
            fatal_warnings: true,
            fatal_infos: false,
            paths: vec![root.clone()],
        };
        assert_eq!(check(&args), exit::FATAL_WARNINGS);
        assert_eq!(
            check(&CheckArgs {
                fatal_warnings: false,
                ..args
            }),
            exit::CLEAN
        );

        fs::remove_dir_all(&root).ok();
    }

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

    /// End to end over a real temp tree: discovery, canonical paths, machine
    /// output, and the error-tier exit code.
    #[test]
    fn check_runs_pipeline_over_temp_tree() {
        let root = std::env::temp_dir().join(format!("dq_cli_check_{}", std::process::id()));
        fs::create_dir_all(root.join("lib")).expect("mkdir");
        fs::write(root.join("lib/clean.dart"), "var ok = 1;\n").expect("write");
        fs::write(root.join("lib/broken.dart"), "void f() { x }\n").expect("write");
        fs::create_dir_all(root.join(".dart_tool")).expect("mkdir");
        fs::write(root.join(".dart_tool/skipme.dart"), "var no = 0;\n").expect("write");

        let args = CheckArgs {
            format: Format::Machine,
            fatal_warnings: true,
            fatal_infos: false,
            paths: vec![root.clone()],
        };
        // broken.dart has one scanner/parsed syntax error → exit 3
        assert_eq!(check(&args), exit::ERRORS);

        let clean_args = CheckArgs {
            paths: vec![root.join("lib/clean.dart")],
            format: Format::Machine,
            ..args
        };
        assert_eq!(check(&clean_args), exit::CLEAN);

        // Nonexistent path → usage tier
        let missing = CheckArgs {
            paths: vec![root.join("nope.dart")],
            ..clean_args
        };
        assert_eq!(check(&missing), exit::USAGE);

        fs::remove_dir_all(&root).ok();
    }
}

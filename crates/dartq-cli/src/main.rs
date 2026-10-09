//! dartq CLI — `dartq check` (the M0 differential face), `dartq analyze`
//! (the M1 engine face), and `dartq version`.
//!
//! `check` drives the M0 pipeline (engine-api-spec §2.1): bytes → scan →
//! `@dart` override rescan → parse, with all diagnostics written through
//! `dartq-diagnostics`' oracle-exact writers. It stops there by design: no
//! pub/package/SDK/engine involvement (the spec's M1 forbid-list).
//!
//! `analyze` drives the full engine over a real package directory:
//! pub faces → [`Engine::new`] → [`Engine::discover_workspace`] →
//! [`Engine::analyze`], printed through the same oracle-exact writers.
//!
//! Published contract (engine-api-spec §4.2, oracle-machine-format §3):
//! - diagnostic lines go to stdout only; usage/internal failures report on
//!   stderr and keep stdout empty;
//! - exit codes follow dartdev's analyze semantics (analyze.dart:719–731
//!   `_Result`, severity selection analyze.dart:398–409, usage exit
//!   dartdev.dart:209–210 `EX_USAGE`): 0 clean or info-only · 1 fatal infos ·
//!   2 fatal warnings · 3 errors · 4 internal failure · 64 usage error. The
//!   differential harness treats {0,1,2,3} as a valid diagnostic set
//!   (Jaccard input); 4/64 are harness-own failures.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use clap::{Args, Parser as _, Subcommand, ValueEnum};

use dartq_diagnostics::{
    AnalysisOutput, CodeRegistry, Diagnostic, FileId, FileLayoutProvider, Severity, SourceRange,
    write_json, write_machine,
};
use dartq_engine::{
    CancellationToken, Cancelled, Engine, EngineConfig, EngineEvent, memory_store,
};
use dartq_language::{
    DEFAULT_LANGUAGE_VERSION, Feature, FeatureSet, LanguageVersion, triple_shift_enabled,
};
use dartq_package::PackageTable;
use dartq_parser::{Message, Parser, ParserFeatures, ParserListener, TokenSpan};
use dartq_pub::{LockFile, PackageConfig, Pubspec, build_graph};
use dartq_scanner::{ScanConfig, ScanResult, scan};
use dartq_sdk::SdkSnapshot;
use dartq_source::{FileTable, LineIndex};

/// Process exit codes (engine-api-spec §4.2, pinned to analyze.dart:719–731;
/// usage exit dartdev.dart:209–210).
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
    match Cli::try_parse_from(std::iter::once("dartq").chain(args.iter().map(String::as_str))) {
        Ok(Cli {
            command: Some(Command::Check(check_args)),
        }) => check(&check_args),
        Ok(Cli {
            command: Some(Command::Analyze(analyze_args)),
        }) => analyze(&analyze_args),
        // Bare `dartq`, `dartq version`, and `--version` all land here.
        Ok(Cli { command: _ }) => {
            println!("dartq {}", env!("CARGO_PKG_VERSION"));
            exit::CLEAN
        }
        Err(error) => {
            let _ = error.print();
            if error.use_stderr() {
                exit::USAGE
            } else {
                exit::CLEAN
            }
        }
    }
}

#[derive(clap::Parser)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Analyze a package directory through the full engine (scan, parse,
    /// resolution; the semantic seam lands with #120/#121).
    Analyze(AnalyzeArgs),
    /// Scan + parse only (the M0 differential face, engine-api-spec §4.1).
    Check(CheckArgs),
    /// Print the version.
    Version,
}

#[derive(Args)]
struct CheckArgs {
    #[arg(long, value_enum, default_value_t = CheckFormat::Machine)]
    format: CheckFormat,
    /// On by default (`--no-fatal-warnings` clears it) — mirrors the oracle.
    #[arg(long)]
    no_fatal_warnings: bool,
    /// Off by default (`--fatal-infos` sets it) — mirrors the oracle.
    #[arg(long)]
    fatal_infos: bool,
    /// Targets; empty resolves to the current directory.
    #[arg(default_value = ".")]
    paths: Vec<PathBuf>,
}

#[derive(Clone, Copy, ValueEnum)]
enum CheckFormat {
    /// SEVERITY|TYPE|CODE|FILE|LINE|COL|LENGTH|MESSAGE per diagnostic.
    Machine,
    /// Single-line JSON envelope.
    Json,
}

#[derive(Args)]
struct AnalyzeArgs {
    /// Package directory holding pubspec.yaml and .dart_tool/package_config.json.
    dir: PathBuf,
    #[arg(long, value_enum, default_value_t = AnalyzeFormat::Machine)]
    format: AnalyzeFormat,
    /// Write one metrics JSON object to PATH at exit (engine-api-spec §3).
    #[arg(long)]
    metrics_json: Option<PathBuf>,
    /// Dart SDK root; defaults to $DART_SDK.
    #[arg(long)]
    sdk: Option<PathBuf>,
    /// On by default (`--no-fatal-warnings` clears it) — mirrors the oracle.
    #[arg(long)]
    no_fatal_warnings: bool,
    /// Off by default (`--fatal-infos` sets it) — mirrors the oracle.
    #[arg(long)]
    fatal_infos: bool,
}

#[derive(Clone, Copy, ValueEnum)]
enum AnalyzeFormat {
    /// SEVERITY|TYPE|CODE|FILE|LINE|COL|LENGTH|MESSAGE per diagnostic
    /// (byte-parity differential face).
    Machine,
    /// Human-readable, grouped by file, severity-sorted.
    Human,
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
            eprintln!("error: Directory or file doesn't exist: {path}");
            return exit::USAGE;
        }
        Err(DiscoverError::Io(error)) => {
            eprintln!("dartq: internal error: {error}");
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
            eprintln!("dartq: internal error: {error}");
            return exit::INTERNAL_ERROR;
        }
        Err(_) => {
            eprintln!("dartq: internal error: analysis panicked");
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
        CheckFormat::Machine => write_machine(&mut out, &output),
        CheckFormat::Json => write_json(&mut out, &output),
    };
    if let Err(error) = written.and_then(|()| out.flush()) {
        eprintln!("dartq: internal error: writing diagnostics failed: {error}");
        return exit::INTERNAL_ERROR;
    }
    severity_exit(&diags, !args.no_fatal_warnings, args.fatal_infos)
}

// ---------------------------------------------------------------------------
// dartq analyze — the M1 engine face
// ---------------------------------------------------------------------------

/// Numbers one analyze run collected for `--metrics-json` (engine-api-spec
/// §3; the struct belongs to the CLI, the engine contributes only numbers
/// via `DiscoveryStats` and `LibraryAnalyzed` events).
struct RunMetrics {
    files: usize,
    loc: u64,
    diagnostics: usize,
    discovery_ms: u64,
    analyze_ms: u64,
    total_ms: u64,
    libraries_analyzed: usize,
    libraries_from_cache: usize,
    max_rss_bytes: Option<u64>,
}

impl RunMetrics {
    fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "files": self.files,
            "loc": self.loc,
            "diagnostics": self.diagnostics,
            "discovery_ms": self.discovery_ms,
            "analyze_ms": self.analyze_ms,
            "total_ms": self.total_ms,
            "libraries_analyzed": self.libraries_analyzed,
            "libraries_from_cache": self.libraries_from_cache,
            "max_rss_bytes": self.max_rss_bytes,
        })
    }
}

/// `dartq analyze` — crash tier like [`check`] (engine-api-spec §4.2): a
/// panic anywhere aborts with exit 4 and stdout kept empty, because
/// [`analyze_run`] writes only after analysis fully succeeded.
fn analyze(args: &AnalyzeArgs) -> u8 {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        analyze_run(args, &mut BufWriter::new(io::stdout().lock()))
    }));
    match outcome {
        Ok(code) => code,
        Err(_) => {
            eprintln!("dartq: internal error: analysis panicked");
            exit::INTERNAL_ERROR
        }
    }
}

/// One analyze run over a real package directory: pub faces → engine →
/// oracle-format output on `out`, metrics JSON at exit.
fn analyze_run(args: &AnalyzeArgs, out: &mut impl Write) -> u8 {
    let started = Instant::now();

    // Usage tier: nothing below may run without these faces.
    let sdk_root = args
        .sdk
        .clone()
        .or_else(|| env::var_os("DART_SDK").map(PathBuf::from));
    let Some(sdk_root) = sdk_root else {
        eprintln!("error: no Dart SDK root (pass --sdk PATH or set DART_SDK)");
        return exit::USAGE;
    };
    // Canonicalize up front so the printed paths are symlink-resolved (the
    // oracle convention) and package roots derived from `config_dir` inherit
    // the canonical form.
    let dir = match args.dir.canonicalize() {
        Ok(dir) => dir,
        Err(_) => {
            eprintln!(
                "error: Directory or file doesn't exist: {}",
                args.dir.display()
            );
            return exit::USAGE;
        }
    };
    let config_dir = dir.join(".dart_tool");
    let (config, lock, pubspec) = match load_workspace_faces(&dir, &config_dir) {
        Ok(faces) => faces,
        Err(message) => {
            eprintln!("error: {message}");
            return exit::USAGE;
        }
    };
    let graph = match build_graph(
        &config,
        &lock,
        &pubspec,
        &BTreeMap::new(),
        None,
        &config_dir,
    ) {
        Ok(graph) => graph,
        Err(error) => {
            eprintln!("error: {error}");
            return exit::USAGE;
        }
    };
    let packages = match PackageTable::build(&config, &graph, &config_dir) {
        Ok(packages) => packages,
        Err(error) => {
            eprintln!("error: {error}");
            return exit::USAGE;
        }
    };
    let sdk = match SdkSnapshot::load(&sdk_root, None) {
        Ok(sdk) => sdk,
        Err(error) => {
            eprintln!(
                "error: Dart SDK at {} unusable: {error}",
                sdk_root.display()
            );
            return exit::USAGE;
        }
    };

    let mut engine = match Engine::new(EngineConfig {
        store: memory_store(),
        sdk,
        packages,
        pub_config: config,
        config_dir,
        semdiag: None,
    }) {
        Ok(engine) => engine,
        Err(error) => {
            eprintln!("error: engine context unusable: {error}");
            return exit::USAGE;
        }
    };

    let discovery_at = Instant::now();
    let stats = match engine.discover_workspace() {
        Ok(stats) => stats,
        Err(error) => {
            eprintln!("dartq: internal error: discovery failed: {error}");
            return exit::INTERNAL_ERROR;
        }
    };
    let discovery_ms = elapsed_ms(discovery_at.elapsed());

    let mut analyzed = (0_usize, 0_usize);
    let mut on_event = |event: EngineEvent| {
        if let EngineEvent::LibraryAnalyzed { from_cache, .. } = event {
            if from_cache {
                analyzed.1 += 1;
            } else {
                analyzed.0 += 1;
            }
        }
    };
    let analyze_at = Instant::now();
    let mut diags = match engine.analyze(&[], &mut on_event, &CancellationToken::default()) {
        Ok(diags) => diags,
        Err(Cancelled) => {
            eprintln!("dartq: internal error: analysis cancelled");
            return exit::INTERNAL_ERROR;
        }
    };
    let analyze_ms = elapsed_ms(analyze_at.elapsed());

    // Oracle display order (DiagnosticWithPath.compareTo, dartdev
    // analyze.dart:686–707): severity, file path, position, message. Position
    // order is the UTF-16 offset (line/column are monotone in it), so no
    // line-column computation is spent on sorting.
    let layout = EngineLayout { engine: &engine };
    diags.sort_by(|a, b| {
        severity_rank(a.severity)
            .cmp(&severity_rank(b.severity))
            .then_with(|| {
                path_key(layout.file_path(a.file)).cmp(path_key(layout.file_path(b.file)))
            })
            .then_with(|| a.range.offset.cmp(&b.range.offset))
            .then_with(|| a.message.cmp(&b.message))
    });

    let written = match args.format {
        AnalyzeFormat::Machine => write_machine(
            out,
            &AnalysisOutput {
                diagnostics: &diags,
                files: &layout,
            },
        ),
        AnalyzeFormat::Human => write_human(out, &diags, &layout),
    };
    if let Err(error) = written.and_then(|()| out.flush()) {
        eprintln!("dartq: internal error: writing diagnostics failed: {error}");
        return exit::INTERNAL_ERROR;
    }

    if let Some(metrics_path) = &args.metrics_json {
        let metrics = RunMetrics {
            files: stats.file_count,
            loc: stats.loc_total,
            diagnostics: diags.len(),
            discovery_ms,
            analyze_ms,
            total_ms: elapsed_ms(started.elapsed()),
            libraries_analyzed: analyzed.0,
            libraries_from_cache: analyzed.1,
            max_rss_bytes: max_rss_bytes(),
        };
        if let Err(error) = write_metrics(metrics_path, &metrics) {
            eprintln!("dartq: internal error: writing metrics failed: {error}");
            return exit::INTERNAL_ERROR;
        }
    }
    severity_exit(&diags, !args.no_fatal_warnings, args.fatal_infos)
}

/// The three pub faces the engine's context derives from (the same assembly
/// the engine integration fixture pins): `package_config.json`,
/// `pubspec.lock`, `pubspec.yaml` — `dir` already canonicalized.
fn load_workspace_faces(
    dir: &Path,
    config_dir: &Path,
) -> Result<(Arc<PackageConfig>, LockFile, Pubspec), String> {
    let config_text = fs::read_to_string(config_dir.join("package_config.json")).map_err(|_| {
        format!(
            "no .dart_tool/package_config.json under {} (run `dart pub get`)",
            dir.display()
        )
    })?;
    let config = PackageConfig::parse(&config_text).map_err(|error| error.to_string())?;
    let lock_text = fs::read_to_string(dir.join("pubspec.lock")).map_err(|_| {
        format!(
            "no pubspec.lock under {} (run `dart pub get`)",
            dir.display()
        )
    })?;
    let lock = LockFile::parse(&lock_text).map_err(|error| error.to_string())?;
    let pubspec_text = fs::read_to_string(dir.join("pubspec.yaml"))
        .map_err(|_| format!("no pubspec.yaml under {}", dir.display()))?;
    let pubspec = Pubspec::parse(&pubspec_text).map_err(|error| error.to_string())?;
    Ok((Arc::new(config), lock, pubspec))
}

/// [`FileLayoutProvider`] over the engine's read faces: paths from the file
/// table, positions from the resident line index, text borrowed straight
/// from the content store — no per-diagnostic file reads or copies.
struct EngineLayout<'a> {
    engine: &'a Engine,
}

impl FileLayoutProvider for EngineLayout<'_> {
    fn file_path(&self, file: FileId) -> &Path {
        self.engine
            .file_table()
            .path(file)
            .expect("analyzed FileId")
    }

    fn line_col(&self, file: FileId, utf16_offset: u32) -> (u32, u32) {
        let index = self.engine.line_index(file).expect("analyzed FileId");
        // Engine content decoded as UTF-8 at set_content time (engine doc).
        let text = std::str::from_utf8(self.engine.content(file).expect("analyzed FileId"))
            .expect("engine content is UTF-8");
        index.line_col_utf16(text, utf16_offset)
    }
}

/// Human format: diagnostics grouped by file (paths ascending), severity-
/// sorted within each group. Not a differential face — layout may change
/// freely.
fn write_human(
    out: &mut impl Write,
    diags: &[Diagnostic],
    layout: &dyn FileLayoutProvider,
) -> io::Result<()> {
    let mut groups: BTreeMap<&Path, Vec<&Diagnostic>> = BTreeMap::new();
    for diag in diags {
        groups
            .entry(layout.file_path(diag.file))
            .or_default()
            .push(diag);
    }
    for (path, group) in &groups {
        writeln!(out, "{}", path.display())?;
        for diag in group {
            let (line, column) = layout.line_col(diag.file, diag.range.offset);
            writeln!(
                out,
                "  {line}:{column} • {} • {} • {}",
                diag.severity, diag.code.name, diag.message
            )?;
        }
    }
    match diags.len() {
        0 => writeln!(out, "No issues found."),
        1 => writeln!(out, "1 issue found."),
        n => writeln!(out, "{n} issues found."),
    }
}

/// One JSON object at exit (engine-api-spec §3, benchmark-harness-spec §2.4).
fn write_metrics(path: &Path, metrics: &RunMetrics) -> io::Result<()> {
    let mut file = fs::File::create(path)?;
    writeln!(file, "{}", metrics.json())
}

/// Oracle severity priority (`DiagnosticWithPath._severityPriority`): Error
/// first, then Warning, then Info.
fn severity_rank(severity: Severity) -> u8 {
    match severity {
        Severity::Error => 0,
        Severity::Warning => 1,
        Severity::Info => 2,
    }
}

/// Plain byte key of a path — the oracle's `filePath.compareTo` string order.
fn path_key(path: &Path) -> &[u8] {
    path.as_os_str().as_encoded_bytes()
}

fn elapsed_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// Peak RSS through getrusage; macOS reports bytes, Linux KiB (normalized to
/// bytes per benchmark-harness-spec §2.2).
#[cfg(unix)]
fn max_rss_bytes() -> Option<u64> {
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) } != 0 {
        return None;
    }
    #[cfg(target_os = "macos")]
    let rss = u64::try_from(usage.ru_maxrss).ok()?; // bytes on macOS
    #[cfg(not(target_os = "macos"))]
    let rss = u64::try_from(usage.ru_maxrss).ok()? * 1024; // KiB on Linux
    Some(rss)
}

#[cfg(not(unix))]
fn max_rss_bytes() -> Option<u64> {
    None
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
    use dartq_diagnostics::EXPECTED_TOKEN;

    /// Parses `args` (without the program name) into a subcommand; `None`
    /// means the bare-`dartq`/version fallback.
    fn parse_args_of(args: &[&str]) -> Result<Option<Command>, clap::Error> {
        Cli::try_parse_from(std::iter::once("dartq").chain(args.iter().copied()))
            .map(|cli| cli.command)
    }

    fn check_args(args: &[&str]) -> CheckArgs {
        match parse_args_of(args) {
            Ok(Some(Command::Check(check))) => check,
            _ => panic!("expected check command"),
        }
    }

    #[test]
    fn check_defaults() {
        let check = check_args(&["check"]);
        assert!(matches!(check.format, CheckFormat::Machine));
        assert!(!check.no_fatal_warnings, "fatal-warnings default on");
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
        assert!(matches!(check.format, CheckFormat::Json));
        assert!(check.no_fatal_warnings);
        assert!(check.fatal_infos);
        assert_eq!(
            check.paths,
            vec![PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b")]
        );

        let check = check_args(&["check", "--format", "machine", "dir/"]);
        assert!(matches!(check.format, CheckFormat::Machine));
        assert_eq!(check.paths, vec![PathBuf::from("dir/")]);
    }

    #[test]
    fn usage_errors() {
        for args in [
            &["check", "--format"][..],
            &["check", "--format", "yaml"][..],
            &["check", "--format=yaml"][..],
            &["check", "--bogus"][..],
            &["check", "-x"][..],
            &["frobnicate"][..],
            &["--bogus"][..],
            &["analyze"][..],
            &["analyze", "--format", "json", "/p"][..],
            &["analyze", "--metrics-json"][..],
        ] {
            assert!(parse_args_of(args).is_err(), "must reject {args:?}");
        }
    }

    #[test]
    fn version_forms() {
        assert!(
            matches!(parse_args_of(&[]), Ok(None) | Ok(Some(Command::Version))),
            "bare dartq falls back to version"
        );
        assert!(matches!(
            parse_args_of(&["version"]),
            Ok(Some(Command::Version))
        ));
        // `--version` surfaces as clap's display error; run() routes it to
        // stdout + exit 0 (use_stderr false).
        let error = match parse_args_of(&["--version"]) {
            Err(error) => error,
            Ok(_) => panic!("--version must surface as a display error"),
        };
        assert_eq!(error.kind(), clap::error::ErrorKind::DisplayVersion);
        assert!(!error.use_stderr());
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
            format: CheckFormat::Machine,
            no_fatal_warnings: false,
            fatal_infos: false,
            paths: vec![root.clone()],
        };
        assert_eq!(check(&args), exit::FATAL_WARNINGS);
        assert_eq!(
            check(&CheckArgs {
                no_fatal_warnings: true,
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
            format: CheckFormat::Machine,
            no_fatal_warnings: false,
            fatal_infos: false,
            paths: vec![root.clone()],
        };
        // broken.dart has one scanner/parsed syntax error → exit 3
        assert_eq!(check(&args), exit::ERRORS);

        let clean_args = CheckArgs {
            paths: vec![root.join("lib/clean.dart")],
            format: CheckFormat::Machine,
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

    // -----------------------------------------------------------------------
    // dartq analyze
    // -----------------------------------------------------------------------

    /// The analyze command face: machine default, flag spellings. Malformed
    /// spellings are covered by `usage_errors`.
    #[test]
    fn analyze_args_parse() {
        let parsed = parse_args_of(&["analyze", "/pkg"])
            .expect("parse")
            .expect("subcommand");
        let Command::Analyze(args) = parsed else {
            panic!("expected analyze command")
        };
        assert!(
            matches!(args.format, AnalyzeFormat::Machine),
            "machine default"
        );
        assert!(args.metrics_json.is_none() && args.sdk.is_none());
        assert!(!args.no_fatal_warnings && !args.fatal_infos);
        assert_eq!(args.dir, PathBuf::from("/pkg"));

        let parsed = parse_args_of(&[
            "analyze",
            "--format=human",
            "--metrics-json",
            "/tmp/m.json",
            "--sdk",
            "/sdk",
            "--no-fatal-warnings",
            "--fatal-infos",
            "/pkg",
        ])
        .expect("parse")
        .expect("subcommand");
        let Command::Analyze(args) = parsed else {
            panic!("expected analyze command")
        };
        assert!(matches!(args.format, AnalyzeFormat::Human));
        assert_eq!(args.metrics_json, Some(PathBuf::from("/tmp/m.json")));
        assert_eq!(args.sdk, Some(PathBuf::from("/sdk")));
        assert!(args.no_fatal_warnings && args.fatal_infos);
    }

    /// Throwaway pub workspace + synthetic dart:core-only SDK — the same
    /// assembly the dartq-engine integration fixture (tests/common/mod.rs)
    /// pins, so the CLI's workspace faces are exercised exactly as the
    /// engine's own tests do.
    fn analyze_fixture(tag: &str, libraries: &[(&str, &str)]) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("dq_cli_analyze_{tag}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let write = |relative: &str, content: &str| {
            let path = root.join(relative);
            fs::create_dir_all(path.parent().expect("parent")).expect("dir");
            fs::write(path, content).expect("file");
        };
        write(
            "pubspec.yaml",
            "name: fixture\nversion: 0.1.0\nenvironment:\n  sdk: ^3.5.0\n",
        );
        write(
            "pubspec.lock",
            "packages: {}\nsdks:\n  dart: \">=3.5.0 <4.0.0\"\n",
        );
        write(
            ".dart_tool/package_config.json",
            r#"{"configVersion": 2, "packages": [{"name": "fixture", "rootUri": "../", "languageVersion": "3.5"}], "generator": "pub"}"#,
        );
        write("sdk/version", "3.13.1\n");
        write(
            "sdk/lib/libraries.json",
            r#"{"vm": {"libraries": {"core": {"uri": "core/core.dart"}}}}"#,
        );
        write(
            "sdk/lib/_internal/allowed_experiments.json",
            r#"{"version": 1, "experimentSets": {"sdkExperiments": []}, "sdk": {"default": {"experimentSet": "sdkExperiments"}}, "packages": {}}"#,
        );
        for (relative, content) in libraries {
            write(relative, content);
        }
        root
    }

    fn analyze_args(
        root: &Path,
        format: AnalyzeFormat,
        metrics_json: Option<PathBuf>,
    ) -> AnalyzeArgs {
        AnalyzeArgs {
            dir: root.to_path_buf(),
            format,
            metrics_json,
            sdk: Some(root.join("sdk")),
            no_fatal_warnings: false,
            fatal_infos: false,
        }
    }

    /// End to end over a real temp workspace: engine discovery + analysis,
    /// oracle machine bytes on stdout, error-tier exit code, metrics JSON at
    /// exit.
    ///
    /// Hand trace against `tests/harness/src/machine.rs` (`parse_machine` +
    /// GOLDEN): 8 `|`-separated fields; `void f() { x }` puts `x` at UTF-16
    /// offset 11 → line 1, column 12, length 1 — the same geometry the
    /// `machine_line_parity_with_harness` check test pins, and none of the
    /// four field escapes (`\n`, `\r`, `\\`, `\|`) fire on this text.
    #[test]
    fn analyze_runs_engine_over_temp_workspace() {
        let root = analyze_fixture(
            "smoke",
            &[
                ("lib/broken.dart", "void f() { x }\n"),
                ("lib/clean.dart", "var ok = 1;\n"),
            ],
        );
        let metrics_path = root.join("metrics.json");
        let mut out: Vec<u8> = Vec::new();
        let code = analyze_run(
            &analyze_args(&root, AnalyzeFormat::Machine, Some(metrics_path.clone())),
            &mut out,
        );

        let broken = root.canonicalize().expect("root").join("lib/broken.dart");
        assert_eq!(
            String::from_utf8(out).expect("stdout is UTF-8"),
            format!(
                "ERROR|SYNTACTIC_ERROR|EXPECTED_TOKEN|{}|1|12|1|Expected to find ';'.\n",
                broken.display()
            ),
            "oracle machine bytes (emitMachineFormat field order)"
        );
        assert_eq!(code, exit::ERRORS, "any ERROR → 3 (analyze.dart:398)");

        let metrics: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(metrics_path).expect("metrics at exit"))
                .expect("one JSON object");
        assert_eq!(metrics["files"], 2, "both lib files discovered");
        assert_eq!(metrics["diagnostics"], 1);
        assert_eq!(metrics["libraries_analyzed"], 2, "cold run recompute");
        assert_eq!(metrics["libraries_from_cache"], 0);
        assert!(metrics["max_rss_bytes"].is_u64(), "getrusage hook wired");
        assert!(metrics["total_ms"].is_u64());

        fs::remove_dir_all(&root).ok();
    }

    /// Clean set → exit 0 with empty machine output (the oracle empty edge);
    /// human format groups by file with the issue-count tail.
    #[test]
    fn analyze_clean_and_human_faces() {
        let root = analyze_fixture("faces", &[("lib/clean.dart", "var ok = 1;\n")]);

        let mut out: Vec<u8> = Vec::new();
        let code = analyze_run(&analyze_args(&root, AnalyzeFormat::Machine, None), &mut out);
        assert_eq!(code, exit::CLEAN);
        assert!(
            out.is_empty(),
            "machine mode emits nothing for an empty set"
        );

        let mut out: Vec<u8> = Vec::new();
        let code = analyze_run(&analyze_args(&root, AnalyzeFormat::Human, None), &mut out);
        assert_eq!(code, exit::CLEAN);
        let text = String::from_utf8(out).expect("UTF-8");
        assert!(text.contains("lib/clean.dart"), "grouped by file: {text}");
        assert!(text.contains("No issues found."), "empty-set tail: {text}");

        fs::remove_dir_all(&root).ok();
    }

    /// Usage tier: missing dir, missing package config, and unusable SDK all
    /// exit 64 with stderr-only output (engine-api-spec §4.2; dartdev
    /// EX_USAGE, dartdev.dart:209–210).
    #[test]
    fn analyze_usage_tiers() {
        let root = analyze_fixture("usage", &[("lib/clean.dart", "var ok = 1;\n")]);

        let missing_dir = analyze_args(&root.join("nope"), AnalyzeFormat::Machine, None);
        assert_eq!(analyze_run(&missing_dir, &mut Vec::new()), exit::USAGE);

        let no_config = analyze_fixture("noconfig", &[]);
        fs::remove_file(no_config.join(".dart_tool/package_config.json")).ok();
        let no_config_args = analyze_args(&no_config, AnalyzeFormat::Machine, None);
        assert_eq!(analyze_run(&no_config_args, &mut Vec::new()), exit::USAGE);

        let no_sdk = AnalyzeArgs {
            sdk: Some(root.join("no-such-sdk")),
            ..analyze_args(&root, AnalyzeFormat::Machine, None)
        };
        assert_eq!(analyze_run(&no_sdk, &mut Vec::new()), exit::USAGE);

        fs::remove_dir_all(&root).ok();
        fs::remove_dir_all(&no_config).ok();
    }
}

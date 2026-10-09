//! dartq CLI — `dartq check` (the M0 differential face), `dartq analyze`
//! (the M1 engine face), `dartq query symbol` (the index face), and
//! `dartq version`.
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
//! `query symbol` drives the same workspace assembly and answers from the
//! symbol index over the analyzed state (PRD §20.3): the queried name's
//! declaration rows plus resolved reference rows. A new output face, not a
//! diagnostics face — rows sort by file, then position.
//!
//! Published contract (engine-api-spec §4.2, oracle-machine-format §3):
//! - diagnostic lines go to stdout only; usage/internal failures report on
//!   stderr and keep stdout empty;
//! - exit codes follow dartdev's analyze semantics (analyze.dart:719–731
//!   `_Result`, severity selection analyze.dart:398–409, usage exit
//!   dartdev.dart:209–210 `EX_USAGE`): 0 clean or info-only · 1 fatal infos ·
//!   2 fatal warnings · 3 errors · 4 internal failure · 64 usage error. The
//!   differential harness treats {0,1,2,3} as a valid diagnostic set
//!   (Jaccard input); 4/64 are harness-own failures. The query face uses
//!   the same tiers: 0 on success (zero results included), 64 usage, 4
//!   internal.

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
use dartq_engine::semdiag;
use dartq_engine::{
    CancellationToken, Cancelled, DeclHit, DeclKind, Engine, EngineConfig, EngineEvent, RefHit,
    RefKind, SymbolAnswer, memory_store,
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

mod compat;
mod fix;
mod impact;

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
        Ok(Cli {
            command: Some(Command::Query(query_args)),
        }) => match query_args.face {
            QueryFace::Symbol(symbol_args) => query_symbol(&symbol_args),
        },
        Ok(Cli {
            command: Some(Command::Impact(impact_args)),
        }) => impact::run_entry(&impact_args),
        Ok(Cli {
            command: Some(Command::Compat(compat_args)),
        }) => compat::run_entry(&compat_args),
        Ok(Cli {
            command: Some(Command::Daemon(daemon_args)),
        }) => daemon(&daemon_args),
        Ok(Cli {
            command: Some(Command::Fix(fix_args)),
        }) => fix::run_entry(&fix_args),
        // Bare `dartq` and `dartq version` land here; `--version` short-
        // circuits in `try_parse_from` as a DisplayVersion error (routed to
        // stdout + exit 0 by the arm below).
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
#[command(name = "dartq", version)]
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
    /// Assess the workspace under several SDK roots — a per-axis ✓/✗
    /// compatibility matrix (PRD §15.5).
    Compat(compat::CompatArgs),
    /// Serve the LSP daemon over stdin/stdout (one client; overlay + full
    /// diagnostics, PRD §16.3).
    Daemon(DaemonArgs),
    /// Emit or apply anchored fix patches (PRD §15.6); `--dry-run` only
    /// outputs them.
    Fix(fix::FixArgs),
    /// Assess what a change to one file would affect (PRD §15.4).
    Impact(impact::ImpactArgs),
    /// Query the analyzed workspace's symbol index (PRD §20.3).
    Query(QueryArgs),
    /// Print the version.
    Version,
}

#[derive(Args)]
struct CheckArgs {
    #[arg(long, value_enum, default_value_t = CheckFormat::Machine)]
    format: CheckFormat,
    /// Write one metrics JSON object to PATH at exit (engine-api-spec §3).
    #[arg(long)]
    metrics_json: Option<PathBuf>,
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

#[derive(Args)]
struct DaemonArgs {
    /// Package directory holding `pubspec.yaml` and
    /// `.dart_tool/package_config.json`.
    dir: PathBuf,
    /// Dart SDK root; defaults to `$DART_SDK`.
    #[arg(long)]
    sdk: Option<PathBuf>,
}

#[derive(Args)]
struct QueryArgs {
    #[command(subcommand)]
    face: QueryFace,
}

#[derive(Subcommand)]
enum QueryFace {
    /// Declarations of one symbol name and resolved references to them.
    Symbol(SymbolArgs),
}

#[derive(Args)]
struct SymbolArgs {
    /// The symbol name to look up.
    name: String,
    /// Package directory holding pubspec.yaml and
    /// `.dart_tool/package_config.json`; defaults to the current directory.
    #[arg(default_value = ".")]
    dir: PathBuf,
    #[arg(long, value_enum, default_value_t = QueryFormat::Machine)]
    format: QueryFormat,
    /// Dart SDK root; defaults to `$DART_SDK`.
    #[arg(long)]
    sdk: Option<PathBuf>,
}

#[derive(Clone, Copy, ValueEnum)]
enum QueryFormat {
    /// One row per line with a stable field order — `DECL` declaration rows
    /// and `REF` reference rows (a symbol listing, not the diagnostics
    /// machine face).
    Machine,
    /// Single-line JSON object.
    Json,
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

#[derive(Debug)]
enum DiscoverError {
    /// Path argument does not exist → usage error (64). The message body is
    /// the oracle's `usageException` text (analyze.dart:191); the `error:`
    /// prefix is dartq's and the oracle's trailing usage block is omitted
    /// (`UsageException.toString` appends it, args usage_exception.dart:12)
    /// — a deliberate divergence: stderr is outside the stdout-only
    /// differential gate.
    Missing(String),
    /// Walk I/O failure → internal error (4).
    Io(io::Error),
}

/// Canonical, sorted, deduplicated `.dart` file set under `paths`. A file
/// argument is taken as-is (top-level `fs::metadata` follows links) and is
/// unfiltered except for the `.dart` extension: the oracle yields explicit
/// files without applying the walk rules (context_root.dart:71-72) but only
/// `.dart` ones reach the driver (context_manager.dart:748-754 via
/// file_paths.dart:75-78 `isDart`) — without the filter a non-Dart arg would
/// parse as Dart and emit phantom diagnostics. A directory argument is
/// walked recursively with the oracle's only built-in exclusion — the
/// dot-basename rule, checked on every child path, directories and files
/// alike (context_root.dart:116 + 146-149). `build`/`node_modules` are
/// analysis_options concerns (excluded globs), not walk-level ones.
///
/// Symlinked directories are never descended into (cycle safety). This is a
/// deliberate subset of the oracle's walk, which descends symlinked
/// directories with canonical-path dedup (context_root.dart:119-129); the
/// differential harness canonicalizes both sides, so the gate never sees the
/// difference. Symlinked `.dart` files are included — the walk stats the
/// link target and accepts regular files, matching the oracle's walk, which
/// analyzes linked files too.
fn discover(paths: &[PathBuf]) -> Result<BTreeSet<PathBuf>, DiscoverError> {
    fn walk(dir: &Path, files: &mut BTreeSet<PathBuf>) -> io::Result<()> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            let path = entry.path();
            // Byte-prefix test: the dot rule is ASCII, and reading the raw
            // name bytes avoids a per-entry lossy-name allocation.
            if entry.file_name().as_encoded_bytes().starts_with(b".") {
                continue;
            }
            if file_type.is_dir() {
                walk(&path, files)?;
            } else if path.extension().is_some_and(|ext| ext == "dart")
                // `entry.file_type()` reports link-host facts (it does not
                // follow symlinks): a symlinked `.dart` file is neither
                // `is_dir` nor `is_file` there, yet the oracle analyzes it —
                // stat the target (`fs::metadata` follows the link); a
                // dangling link has no target and is skipped. Regular files
                // take the cached-type path without a syscall.
                && (file_type.is_file()
                    || (file_type.is_symlink()
                        && fs::metadata(&path).is_ok_and(|meta| meta.is_file())))
            {
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
            // `.dart` only (oracle isDart parity — see the fn doc).
            if path.extension().is_some_and(|ext| ext == "dart") {
                files.insert(path.clone());
            }
        } else {
            walk(path, &mut files).map_err(DiscoverError::Io)?;
        }
    }
    Ok(files)
}

fn check(args: &CheckArgs) -> u8 {
    let started = Instant::now();
    let discovery_at = Instant::now();
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
    let discovery_ms = elapsed_ms(discovery_at.elapsed());

    // Analysis runs under the crash tier (engine-api-spec §4.2: internal
    // failure → 4, the oracle `_Result.crash(4)` analog): a panic anywhere in
    // the pipeline aborts the run with stderr kept from the panic hook and
    // stdout kept empty — a partial diagnostic set must never masquerade as a
    // valid {0,1,2,3} result.
    let analyze_at = Instant::now();
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
    let analyze_ms = elapsed_ms(analyze_at.elapsed());
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

    // Buffered like the analyze/query faces: the writers emit one writeln
    // per line, and an unbuffered lock would flush per diagnostic line.
    let stdout = io::stdout();
    let mut out = BufWriter::new(stdout.lock());
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

    if let Some(metrics_path) = &args.metrics_json {
        // LOC counts newline bytes — the engine discovery convention, one
        // pass, no line splitting.
        let loc: u64 = files_state
            .sources
            .iter()
            .map(|source| source.bytes().filter(|&b| b == b'\n').count() as u64)
            .sum();
        let metrics = RunMetrics {
            files: files.len(),
            loc,
            diagnostics: diags.len(),
            discovery_ms,
            analyze_ms,
            total_ms: elapsed_ms(started.elapsed()),
            // No engine libraries on the check face; the counters exist to
            // keep the JSON shape identical to `analyze`.
            libraries_analyzed: 0,
            libraries_from_cache: 0,
            max_rss_bytes: max_rss_bytes(),
        };
        if let Err(error) = write_metrics(metrics_path, &metrics) {
            eprintln!("dartq: internal error: writing metrics failed: {error}");
            return exit::INTERNAL_ERROR;
        }
    }
    severity_exit(&diags, !args.no_fatal_warnings, args.fatal_infos)
}

// ---------------------------------------------------------------------------
// dartq analyze — the M1 engine face
// ---------------------------------------------------------------------------

/// Numbers one CLI run collected for `--metrics-json` (engine-api-spec §3;
/// the struct belongs to the CLI). `analyze` fills discovery/library numbers
/// from `DiscoveryStats` and `LibraryAnalyzed` events; `check` reports its
/// own walk/parse phases and zero library counters — the JSON shape is
/// identical because the §4.1 grammar pins `--metrics-json` on both faces.
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

/// The usage-tier workspace assembly shared by `analyze` and `query
/// symbol`: SDK root resolution, canonicalized package directory (printed
/// paths are symlink-resolved, the oracle convention), the three pub faces,
/// package tables, SDK snapshot, and the engine context. Usage failures
/// print on stderr and return the usage tier (64); stdout stays empty. The
/// canonical directory comes back for the caller's discovery phase.
fn open_engine(dir: &Path, sdk: Option<&Path>) -> Result<(Engine, PathBuf), u8> {
    let (config, dir) = engine_config(dir, sdk)?;
    let engine = match Engine::new(config) {
        Ok(engine) => engine,
        Err(error) => {
            eprintln!("error: engine context unusable: {error}");
            return Err(exit::USAGE);
        }
    };
    Ok((engine, dir))
}

/// The assembled [`EngineConfig`] behind [`open_engine`], plus the
/// canonicalized package directory. The daemon face consumes the config
/// directly (`Session::new` owns its engine lifetime, #328).
fn engine_config(dir: &Path, sdk: Option<&Path>) -> Result<(EngineConfig, PathBuf), u8> {
    let sdk_root = sdk
        .map(Path::to_path_buf)
        .or_else(|| env::var_os("DART_SDK").map(PathBuf::from));
    let Some(sdk_root) = sdk_root else {
        eprintln!("error: no Dart SDK root (pass --sdk PATH or set DART_SDK)");
        return Err(exit::USAGE);
    };
    // Canonicalize up front so the printed paths are symlink-resolved (the
    // oracle convention) and package roots derived from `config_dir` inherit
    // the canonical form.
    let dir = match dir.canonicalize() {
        Ok(dir) => dir,
        Err(_) => {
            eprintln!("error: Directory or file doesn't exist: {}", dir.display());
            return Err(exit::USAGE);
        }
    };
    let config_dir = dir.join(".dart_tool");
    let (config, lock, pubspec) = match load_workspace_faces(&dir, &config_dir) {
        Ok(faces) => faces,
        Err(message) => {
            eprintln!("error: {message}");
            return Err(exit::USAGE);
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
            return Err(exit::USAGE);
        }
    };
    let packages = match PackageTable::build(&config, &graph, &config_dir) {
        Ok(packages) => packages,
        Err(error) => {
            eprintln!("error: {error}");
            return Err(exit::USAGE);
        }
    };
    let sdk_snapshot = match SdkSnapshot::load(&sdk_root, None) {
        Ok(sdk) => sdk,
        Err(error) => {
            eprintln!(
                "error: Dart SDK at {} unusable: {error}",
                sdk_root.display()
            );
            return Err(exit::USAGE);
        }
    };
    Ok((
        EngineConfig {
            store: memory_store(),
            sdk: sdk_snapshot,
            packages,
            pub_config: config,
            config_dir,
            // The semantic families are part of the analyze contract (#236):
            // `undefined_*` / `wrong_number_of_*` ride the default diagnoser.
            semdiag: Some(semdiag::diagnoser()),
        },
        dir,
    ))
}

/// One analyze run over a real package directory: pub faces → engine →
/// oracle-format output on `out`, metrics JSON at exit.
fn analyze_run(args: &AnalyzeArgs, out: &mut impl Write) -> u8 {
    let started = Instant::now();
    let (mut engine, _dir) = match open_engine(&args.dir, args.sdk.as_deref()) {
        Ok(faces) => faces,
        Err(code) => return code,
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
    let layout = EngineLayout::over(&engine, diags.iter().map(|d| d.file));
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

/// The LSP daemon face: assemble the engine context, hand it to the daemon
/// session, and serve stdin/stdout until `shutdown`+`exit` or EOF. The
/// workspace sweep (discovery + first analysis) runs inside the session at
/// `initialized`, so the client sees full diagnostics immediately after the
/// handshake.
fn daemon(args: &DaemonArgs) -> u8 {
    let (config, _dir) = match engine_config(&args.dir, args.sdk.as_deref()) {
        Ok(faces) => faces,
        Err(code) => return code,
    };
    let session = match dartq_daemon::Session::new(config) {
        Ok(session) => session,
        Err(error) => {
            eprintln!("dartq: internal error: daemon session unusable: {error}");
            return exit::INTERNAL_ERROR;
        }
    };
    // The reader thread owns stdin; only the main loop writes stdout, so the
    // buffer needs no lock. The `Stdin` handle (not `StdinLock`) is what is
    // `Send`. On `exit` the reader may still be blocked on the next read —
    // process exit reaps it.
    let stdin = io::BufReader::new(io::stdin());
    let stdout = io::BufWriter::new(io::stdout().lock());
    match dartq_daemon::run(stdin, stdout, session) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("dartq: internal error: daemon transport failed: {error}");
            exit::INTERNAL_ERROR
        }
    }
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
///
/// The (text, line index) pair is snapshotted once per distinct file in the
/// output set: the writers call [`FileLayoutProvider::line_col`] up to twice
/// per diagnostic (dartq-diagnostics writer.rs:128-129) and the content face
/// re-validates UTF-8 on every call, so uncached resolution is O(D·N) on a
/// diagnostic-heavy file. The check face's [`FileSet`] caches the same pair
/// per file.
struct EngineLayout<'a> {
    engine: &'a Engine,
    /// Exactly the FileIds the caller will query, each with its resident
    /// text and line index.
    cache: BTreeMap<FileId, (&'a str, &'a LineIndex)>,
}

impl<'a> EngineLayout<'a> {
    /// Snapshots (text, index) once per distinct file in `files`.
    fn over(engine: &'a Engine, files: impl IntoIterator<Item = FileId>) -> Self {
        let mut cache = BTreeMap::new();
        for file in files {
            cache.entry(file).or_insert_with(|| {
                // Engine content decoded as UTF-8 at set_content time
                // (engine doc); validated exactly once per file here.
                let text = std::str::from_utf8(engine.content(file).expect("analyzed FileId"))
                    .expect("engine content is UTF-8");
                (text, engine.line_index(file).expect("analyzed FileId"))
            });
        }
        Self { engine, cache }
    }
}

impl FileLayoutProvider for EngineLayout<'_> {
    fn file_path(&self, file: FileId) -> &Path {
        self.engine
            .file_table()
            .path(file)
            .expect("analyzed FileId")
    }

    fn line_col(&self, file: FileId, utf16_offset: u32) -> (u32, u32) {
        let (text, index) = self.cache.get(&file).expect("output-set FileId");
        // Diagnostic offsets are UTF-16; the fused conversion reports the
        // official `LineInfo.getLocation` (line, UTF-16 column) directly.
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

// ---------------------------------------------------------------------------
// dartq query symbol — the index face (PRD §20.3)
// ---------------------------------------------------------------------------

/// `dartq query symbol` — crash tier like [`analyze`]: a panic aborts with
/// exit 4 and stdout kept empty, because [`query_symbol_run`] writes only
/// after the query fully succeeded.
fn query_symbol(args: &SymbolArgs) -> u8 {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        query_symbol_run(args, &mut BufWriter::new(io::stdout().lock()))
    }));
    match outcome {
        Ok(code) => code,
        Err(_) => {
            eprintln!("dartq: internal error: analysis panicked");
            exit::INTERNAL_ERROR
        }
    }
}

/// One query run: the analyze workspace assembly, one analyze pass (the
/// read-after-write — the query reflects and never re-drives it), then the
/// engine's symbol answer on `out`. Exit 0 on success, zero results
/// included.
fn query_symbol_run(args: &SymbolArgs, out: &mut impl Write) -> u8 {
    let (mut engine, _dir) = match open_engine(&args.dir, args.sdk.as_deref()) {
        Ok(faces) => faces,
        Err(code) => return code,
    };
    if let Err(error) = engine.discover_workspace() {
        eprintln!("dartq: internal error: discovery failed: {error}");
        return exit::INTERNAL_ERROR;
    }
    // The read-after-write: diagnostics are not part of this face, but the
    // analyze pass is what builds the resolution stage the reference
    // surface resolves through.
    let mut on_event = |_: EngineEvent| {};
    if let Err(Cancelled) = engine.analyze(&[], &mut on_event, &CancellationToken::default()) {
        eprintln!("dartq: internal error: analysis cancelled");
        return exit::INTERNAL_ERROR;
    }
    let SymbolAnswer {
        mut decls,
        mut refs,
    } = engine.query_symbols(&args.name);
    let layout = EngineLayout::over(
        &engine,
        decls
            .iter()
            .map(|d| d.file)
            .chain(refs.iter().map(|r| r.target_file)),
    );

    // Output determinism: file path byte order, then position. The engine
    // returns id-ordered rows; path order is the printed contract.
    decls.sort_by(|a, b| {
        path_key(layout.file_path(a.file))
            .cmp(path_key(layout.file_path(b.file)))
            .then_with(|| a.range.offset.cmp(&b.range.offset))
    });
    refs.sort_by(|a, b| {
        path_key(layout.file_path(a.target_file))
            .cmp(path_key(layout.file_path(b.target_file)))
            .then_with(|| a.range.offset.cmp(&b.range.offset))
    });

    let written = match args.format {
        QueryFormat::Machine => write_query_machine(out, &layout, &decls, &refs),
        QueryFormat::Json => write_query_json(out, &layout, &args.name, &decls, &refs),
    };
    if let Err(error) = written.and_then(|()| out.flush()) {
        eprintln!("dartq: internal error: writing query results failed: {error}");
        return exit::INTERNAL_ERROR;
    }
    exit::CLEAN
}

/// Machine format: one row per line with a stable field order — the query
/// face's own grammar (a symbol listing, deliberately not the diagnostics
/// 8-field face):
///
/// ```text
/// DECL|file|container|kind|name|line|column|length   (container `-` when
///                                                     top-level; 1-based)
/// REF|target-file|kind|qualified|offset|length       (the use site's
///                                                     UTF-16 offset)
/// ```
fn write_query_machine(
    out: &mut impl Write,
    layout: &dyn FileLayoutProvider,
    decls: &[DeclHit<'_>],
    refs: &[RefHit],
) -> io::Result<()> {
    for decl in decls {
        let (line, column) = layout.line_col(decl.file, decl.range.offset);
        writeln!(
            out,
            "DECL|{}|{}|{}|{}|{line}|{column}|{}",
            layout.file_path(decl.file).display(),
            decl.container.unwrap_or("-"),
            decl_kind_name(decl.kind),
            decl.name,
            decl.range.length,
        )?;
    }
    for reference in refs {
        writeln!(
            out,
            "REF|{}|{}|{}|{}|{}",
            layout.file_path(reference.target_file).display(),
            ref_kind_name(reference.kind),
            reference.qualified,
            reference.range.offset,
            reference.range.length,
        )?;
    }
    Ok(())
}

/// JSON format: one single-line object — `name` plus `declarations` and
/// `references` arrays carrying the machine rows' fields (snake_case, the
/// metrics-JSON convention). Zero results print empty arrays.
fn write_query_json(
    out: &mut impl Write,
    layout: &dyn FileLayoutProvider,
    name: &str,
    decls: &[DeclHit<'_>],
    refs: &[RefHit],
) -> io::Result<()> {
    let declarations: Vec<serde_json::Value> = decls
        .iter()
        .map(|decl| {
            let (line, column) = layout.line_col(decl.file, decl.range.offset);
            serde_json::json!({
                "file": layout.file_path(decl.file).display().to_string(),
                "container": decl.container,
                "kind": decl_kind_name(decl.kind),
                "name": decl.name,
                "line": line,
                "column": column,
                "length": decl.range.length,
            })
        })
        .collect();
    let references: Vec<serde_json::Value> = refs
        .iter()
        .map(|reference| {
            serde_json::json!({
                "target_file": layout.file_path(reference.target_file).display().to_string(),
                "kind": ref_kind_name(reference.kind),
                "qualified": reference.qualified,
                "offset": reference.range.offset,
                "length": reference.range.length,
            })
        })
        .collect();
    writeln!(
        out,
        "{}",
        serde_json::json!({
            "name": name,
            "declarations": declarations,
            "references": references,
        })
    )
}

/// Stable machine spelling of a declaration family (the query face's own
/// grammar — not a diagnostics code).
fn decl_kind_name(kind: DeclKind) -> &'static str {
    match kind {
        DeclKind::Class => "class",
        DeclKind::Enum => "enum",
        DeclKind::Mixin => "mixin",
        DeclKind::Extension => "extension",
        DeclKind::ExtensionType => "extension-type",
        DeclKind::TypeAlias => "type-alias",
        DeclKind::TopLevelFn => "function",
        DeclKind::TopLevelVar => "variable",
        DeclKind::TypeParam => "type-param",
        DeclKind::Method => "method",
        DeclKind::Getter => "getter",
        DeclKind::Setter => "setter",
        DeclKind::Field => "field",
    }
}

/// Stable machine spelling of a reference relation.
fn ref_kind_name(kind: RefKind) -> &'static str {
    match kind {
        RefKind::Referenced => "reference",
        RefKind::Read => "read",
        RefKind::Invoked => "invoked",
        RefKind::Written => "written",
        RefKind::ReadWritten => "read-written",
    }
}

/// One JSON object at exit (engine-api-spec §3, benchmark-harness-spec §2.4).
/// A failed write is an internal error (4) at the call site, never silent —
/// a run without its metrics file would corrupt benchmark data.
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
///
/// Exactly one leading UTF-8 BOM is dropped (oracle read-layer parity: the
/// analyzer read path strips it before scanning; spans are relative to the
/// stripped text). A second BOM is content.
fn read_source(path: &Path) -> io::Result<(PathBuf, String)> {
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
            "--metrics-json",
            "/tmp/m.json",
            "--no-fatal-warnings",
            "--fatal-infos",
            "/tmp/a",
            "/tmp/b",
        ]);
        assert!(matches!(check.format, CheckFormat::Json));
        assert_eq!(check.metrics_json, Some(PathBuf::from("/tmp/m.json")));
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
            &["check", "--metrics-json"][..],
            &["query"][..],
            &["query", "symbol"][..],
            &["query", "bogus"][..],
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

    /// Above-latest override alone → the warning tier (2), cleared by
    /// `--no-fatal-warnings`; no other check behavior changes.
    #[test]
    fn check_exit_tiers_for_above_latest_override() {
        let root = std::env::temp_dir().join(format!("dq_cli_override_{}", std::process::id()));
        fs::create_dir_all(&root).expect("mkdir");
        fs::write(root.join("override.dart"), "// @dart = 99.9\nvar ok = 1;\n").expect("write");

        let args = CheckArgs {
            format: CheckFormat::Machine,
            metrics_json: None,
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

    /// The CLI read face strips exactly one leading BOM, matching the oracle
    /// read layer: single BOM gone, second BOM kept as content, BOM-only
    /// file reads back empty.
    #[test]
    fn read_source_strips_one_leading_bom() {
        let root = std::env::temp_dir().join(format!("dq_cli_bom_{}", std::process::id()));
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
            metrics_json: None,
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

    /// `--metrics-json` writes the analyze-shaped JSON at exit (spec §4.1
    /// pins the flag in the check grammar); the library counters stay at zero
    /// because the check face has no engine libraries.
    #[test]
    fn check_writes_metrics_json() {
        let root = std::env::temp_dir().join(format!("dq_cli_metrics_{}", std::process::id()));
        fs::create_dir_all(&root).expect("mkdir");
        fs::write(root.join("a.dart"), "void f() { x }\n").expect("write");

        let metrics_path = root.join("metrics.json");
        let args = CheckArgs {
            format: CheckFormat::Machine,
            metrics_json: Some(metrics_path.clone()),
            no_fatal_warnings: false,
            fatal_infos: false,
            paths: vec![root.clone()],
        };
        assert_eq!(check(&args), exit::ERRORS);

        let metrics: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&metrics_path).expect("metrics at exit"))
                .expect("one JSON object");
        assert_eq!(metrics["files"], 1);
        assert_eq!(metrics["loc"], 1);
        assert_eq!(metrics["diagnostics"], 1);
        assert_eq!(metrics["libraries_analyzed"], 0);
        assert_eq!(metrics["libraries_from_cache"], 0);
        assert!(metrics["max_rss_bytes"].is_u64());
        assert!(metrics["discovery_ms"].is_u64());
        assert!(metrics["analyze_ms"].is_u64());
        assert!(metrics["total_ms"].is_u64());

        fs::remove_dir_all(&root).ok();
    }

    /// The walk's symlink rule, both halves: a symlinked `.dart` file is
    /// analyzed (its target stats as a regular file — the oracle's walk
    /// includes linked files, and skipping them would flood the differential
    /// gate with phantom oracle-only entries), while a symlinked directory is
    /// never descended into (cycle safety) — `real.dart` must be found once
    /// through `real/`, not a second time through `linked_dir/`. A dangling
    /// link has no target and is skipped.
    #[cfg(unix)]
    #[test]
    fn discover_includes_symlinked_files_not_symlinked_dirs() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!("dq_cli_symlink_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("real")).expect("mkdir");
        fs::write(root.join("real/real.dart"), "var a = 1;\n").expect("write");
        symlink(root.join("real/real.dart"), root.join("link.dart")).expect("file link");
        symlink(root.join("real"), root.join("linked_dir")).expect("dir link");
        symlink(root.join("gone.dart"), root.join("dangling.dart")).expect("dangling link");

        let files = discover(std::slice::from_ref(&root)).expect("discover");
        assert_eq!(
            files,
            BTreeSet::from([root.join("link.dart"), root.join("real/real.dart")]),
            "linked file in, linked dir not descended into, dangling link skipped"
        );

        fs::remove_dir_all(&root).ok();
    }

    /// Walk parity with the oracle's only built-in exclusion — the
    /// dot-basename rule, checked on every child path, directories and files
    /// alike (context_root.dart:116 + 146-149): `build/` IS descended into
    /// (the old dartq-only `build`/`node_modules` skip diverged from the
    /// oracle, which reserves those names to analysis_options globs), and a
    /// dot-prefixed FILE (`.gen.dart`) is excluded exactly like a
    /// dot-directory. Explicit file args bypass the walk rules: a dot-file
    /// arg is taken unfiltered (context_root.dart:71-72), while a
    /// non-`.dart` arg is ignored (context_manager.dart:748-754 via
    /// file_paths.dart:75-78 `isDart`) — through `check` that is an empty
    /// set and a clean exit instead of phantom syntax diagnostics.
    #[test]
    fn discover_walk_and_explicit_arg_parity() {
        let root = std::env::temp_dir().join(format!("dq_cli_walk_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("build")).expect("mkdir");
        fs::write(root.join("build/x.dart"), "var ok = 1;\n").expect("write");
        fs::write(root.join(".gen.dart"), "var ok = 1;\n").expect("write");
        fs::write(root.join("notes.txt"), "not dart\n").expect("write");
        fs::write(root.join("real.dart"), "var ok = 1;\n").expect("write");

        let files = discover(std::slice::from_ref(&root)).expect("discover");
        assert_eq!(
            files,
            BTreeSet::from([root.join("build/x.dart"), root.join("real.dart")]),
            "build/ descended into, dot-file excluded by the basename rule"
        );

        // Explicit args are unfiltered by the walk rules; only `.dart` counts.
        assert_eq!(
            discover(&[root.join(".gen.dart")]).expect("dot-file arg"),
            BTreeSet::from([root.join(".gen.dart")]),
            "explicit dot-file arg is not filtered"
        );
        assert!(
            discover(&[root.join("notes.txt")])
                .expect("non-dart arg")
                .is_empty(),
            "non-.dart explicit arg is ignored (oracle isDart)"
        );

        // End to end: the ignored arg parses nothing → empty set, exit 0.
        let args = CheckArgs {
            format: CheckFormat::Machine,
            metrics_json: None,
            no_fatal_warnings: false,
            fatal_infos: false,
            paths: vec![root.join("notes.txt")],
        };
        assert_eq!(check(&args), exit::CLEAN);

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

    // -----------------------------------------------------------------------
    // dartq query symbol
    // -----------------------------------------------------------------------

    /// The query command face: name positional, default dir, flag
    /// spellings. Malformed spellings are covered by `usage_errors`.
    #[test]
    fn query_args_parse() {
        let parsed = parse_args_of(&["query", "symbol", "Foo"])
            .expect("parse")
            .expect("subcommand");
        let Command::Query(args) = parsed else {
            panic!("expected query command")
        };
        // Single-variant face: an irrefutable destructure.
        let QueryFace::Symbol(symbol) = args.face;
        assert_eq!(symbol.name, "Foo");
        assert_eq!(symbol.dir, PathBuf::from("."), "default target cwd");
        assert!(matches!(symbol.format, QueryFormat::Machine));
        assert!(symbol.sdk.is_none());

        let parsed = parse_args_of(&[
            "query",
            "symbol",
            "--format=json",
            "--sdk",
            "/sdk",
            "Foo",
            "/pkg",
        ])
        .expect("parse")
        .expect("subcommand");
        let Command::Query(args) = parsed else {
            panic!("expected query command")
        };
        let QueryFace::Symbol(symbol) = args.face;
        assert!(matches!(symbol.format, QueryFormat::Json));
        assert_eq!(symbol.sdk, Some(PathBuf::from("/sdk")));
        assert_eq!(symbol.dir, PathBuf::from("/pkg"));
        assert_eq!(symbol.name, "Foo");
    }

    fn symbol_args(root: &Path, format: QueryFormat, name: &str) -> SymbolArgs {
        SymbolArgs {
            name: name.to_string(),
            dir: root.to_path_buf(),
            format,
            sdk: Some(root.join("sdk")),
        }
    }

    /// End to end over a real temp workspace: the class plus its two bare
    /// cross-file uses in machine bytes (target file, then offset order), a
    /// member hit with its container name, the JSON face, the empty-result
    /// edge (exit 0, no rows), and the usage tier.
    #[test]
    fn query_symbol_end_to_end() {
        let root = analyze_fixture(
            "query",
            &[
                (
                    "lib/a.dart",
                    "class Foo {\n  void meth() {}\n}\nvoid foo() {}\n",
                ),
                (
                    "lib/b.dart",
                    "import 'a.dart';\nvoid main() {\n  Foo? f = Foo();\n  foo();\n}\n",
                ),
            ],
        );
        let a = root.canonicalize().expect("root").join("lib/a.dart");

        // `Foo? f` types a reference; `Foo()` is an implicit-creation
        // invocation; both target a.dart unqualified.
        let mut out: Vec<u8> = Vec::new();
        let code = query_symbol_run(&symbol_args(&root, QueryFormat::Machine, "Foo"), &mut out);
        assert_eq!(code, exit::CLEAN);
        assert_eq!(
            String::from_utf8(out).expect("UTF-8"),
            format!(
                "DECL|{}|-|class|Foo|1|7|3\n\
                 REF|{}|reference|false|33|3\n\
                 REF|{}|invoked|false|42|3\n",
                a.display(),
                a.display(),
                a.display()
            )
        );

        // A member hit: the container name surfaces; no uses → no REF rows,
        // still exit 0.
        let mut out: Vec<u8> = Vec::new();
        let code = query_symbol_run(&symbol_args(&root, QueryFormat::Machine, "meth"), &mut out);
        assert_eq!(code, exit::CLEAN);
        assert_eq!(
            String::from_utf8(out).expect("UTF-8"),
            format!("DECL|{}|Foo|method|meth|2|8|4\n", a.display())
        );

        // JSON face: one object, snake_case fields mirroring the rows.
        let mut out: Vec<u8> = Vec::new();
        let code = query_symbol_run(&symbol_args(&root, QueryFormat::Json, "Foo"), &mut out);
        assert_eq!(code, exit::CLEAN);
        let json: serde_json::Value =
            serde_json::from_str(&String::from_utf8(out).expect("UTF-8")).expect("one object");
        assert_eq!(json["name"], "Foo");
        assert_eq!(json["declarations"][0]["kind"], "class");
        assert_eq!(json["declarations"][0]["name"], "Foo");
        assert!(json["declarations"][0]["container"].is_null());
        assert_eq!(json["declarations"][0]["line"], 1);
        assert_eq!(json["declarations"][0]["column"], 7);
        assert_eq!(json["references"].as_array().expect("array").len(), 2);
        assert_eq!(json["references"][0]["kind"], "reference");
        assert_eq!(
            json["references"][0]["target_file"],
            a.display().to_string()
        );
        assert_eq!(json["references"][0]["qualified"], false);
        assert_eq!(json["references"][0]["offset"], 33);
        assert_eq!(json["references"][1]["kind"], "invoked");
        assert_eq!(json["references"][1]["offset"], 42);

        // Zero results: exit 0, machine prints the empty set.
        let mut out: Vec<u8> = Vec::new();
        let code = query_symbol_run(&symbol_args(&root, QueryFormat::Machine, "Nope"), &mut out);
        assert_eq!(code, exit::CLEAN);
        assert!(out.is_empty(), "no rows for a miss");

        // Usage tier: missing directory.
        let missing = symbol_args(&root.join("nope"), QueryFormat::Machine, "Foo");
        assert_eq!(query_symbol_run(&missing, &mut Vec::new()), exit::USAGE);

        fs::remove_dir_all(&root).ok();
    }
}

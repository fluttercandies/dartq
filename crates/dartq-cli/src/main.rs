//! dartq CLI — `dartq check` (the M0 differential face), `dartq analyze`
//! (the M1 engine face), the `dartq query` index faces (`symbol` /
//! `references` / `callers`, PRD §20.3 + §15.2), `dartq explain` (the
//! §15.3 one-stop AI report), `dartq index` (the §15.1 index build), and
//! `dartq version`.
//!
//! The per-command cores live in `dartq-commands` (the one implementation
//! the daemon RPC faces share, PRD §15.1); this binary adds only argument
//! parsing, output formatting, and the exit tiers.
//!
//! `check` drives the M0 pipeline (engine-api-spec §2.1): bytes → scan →
//! `@dart` override rescan → parse, with all diagnostics written through
//! `dartq-diagnostics`' oracle-exact writers. It stops there by design: no
//! pub/package/SDK/engine involvement (the spec's M1 forbid-list).
//!
//! `analyze` drives the full engine over a real package directory:
//! pub faces → [`dartq_commands::workspace::open_engine`] → engine
//! discovery → analysis, printed through the same oracle-exact writers.
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
//!
//! The daemon face is the one deviation from the crash tier: it installs no
//! catch-unwind, so a panic on the main thread unwinds out of `main` and
//! the process exits with the Rust runtime's default 101 instead of 4. The
//! crash tier exists to keep a partial diagnostic set from masquerading as
//! a harness-valid {0,1,2,3} result; the daemon serves an LSP stream, not
//! a diagnostic set, and its reader/watch threads' panics are thread-local
//! regardless — the main loop observes the channel disconnect and ends the
//! session through the transport's own exit path.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use clap::{Args, Parser as _, Subcommand, ValueEnum};

use dartq_commands::analyze::AnalyzeRun;
use dartq_commands::discover::{DiscoverError, discover};
use dartq_commands::explain::ExplainError;
use dartq_commands::query::{decl_kind_name, ref_kind_name};
use dartq_commands::workspace::{engine_config, open_engine};
use dartq_diagnostics::{
    AnalysisOutput, Diagnostic, FileLayoutProvider, Severity, write_json, write_machine,
};
use dartq_engine::{DeclHit, RefHit};

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
mod index;

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
            QueryFace::References(symbol_args) => query_references(&symbol_args),
            QueryFace::Callers(symbol_args) => query_callers(&symbol_args),
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
        Ok(Cli {
            command: Some(Command::Assists(assists_args)),
        }) => fix::run_assists_entry(&assists_args),
        Ok(Cli {
            command: Some(Command::Index(index_args)),
        }) => index::run_entry(&index_args),
        Ok(Cli {
            command: Some(Command::Explain(explain_args)),
        }) => explain_entry(&explain_args),
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
    /// Assemble the §15.3 one-stop AI report for one symbol over the
    /// analyzed workspace.
    Explain(ExplainArgs),
    /// Emit or apply anchored fix patches (PRD §15.6); `--dry-run` only
    /// outputs them.
    Fix(fix::FixArgs),
    /// Query the plugin hosts' assists for one file location (PRD §14);
    /// report-only.
    Assists(fix::AssistsArgs),
    /// Build the symbol index over the analyzed workspace and report what
    /// it built (PRD §15.1).
    Index(index::IndexArgs),
    /// Assess what a change to one file would affect (PRD §15.4).
    Impact(impact::ImpactArgs),
    /// Query the analyzed workspace's symbol index (PRD §20.3).
    Query(QueryArgs),
    /// Print the version.
    Version,
}

/// Scan + parse only (the M0 differential face, engine-api-spec §4.1).
///
/// Analysis-options boundary: this face consumes no analysis_options.yaml —
/// no `exclude:` on the walk, no `errors:` remaps, no lint rules. Options
/// are the engine/analyze face's concern (`dartq-engine`'s
/// `AnalysisOptions`, whose loader needs the package universe this face
/// deliberately lacks); differential corpora driving this face must be
/// options-free.
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
    /// Analyze PATH with the content of FILE instead of the on-disk bytes
    /// (PRD §16.3): repeatable, relative PATH resolves against DIR, and
    /// FILE `-` reads stdin. The engine-view faces only — `check` has no
    /// engine to overlay.
    #[arg(long = "overlay", value_name = "PATH=FILE")]
    overlays: Vec<String>,
    /// Dart SDK root; defaults to $DART_SDK.
    #[arg(long)]
    sdk: Option<PathBuf>,
    /// On by default (`--no-fatal-warnings` clears it) — mirrors the oracle.
    #[arg(long)]
    no_fatal_warnings: bool,
    /// Off by default (`--fatal-infos` sets it) — mirrors the oracle.
    #[arg(long)]
    fatal_infos: bool,
    /// Memory budget in MiB (PRD §17.1): RSS is sampled at phase and
    /// library boundaries; over budget the engine degrades tier-by-tier
    /// and reports each step as an INFO `dartq/memory_degraded` row
    /// (diagnostics stay complete at every tier). Unset = unlimited —
    /// the output is byte-identical to an unbudgeted run.
    #[arg(long, value_name = "MiB")]
    memory_limit: Option<u64>,
    /// JSONL trace sink (PRD §17.4): the engine appends one JSON object per
    /// engine event — §17.4 taxonomy names, stable field order, flushed per
    /// event so a killed CI run keeps its tail. Unset = no sink and zero
    /// event work: the output face is byte-identical to an untraced run.
    #[arg(long, value_name = "PATH")]
    trace_events: Option<PathBuf>,
    /// Hard cap for the L3 content store (PRD §17.2), the persistent
    /// cross-session cache under `~/.cache/dartq/`: after each write batch
    /// the store evicts least-recently-touched, unreferenced contents until
    /// the stored total is at or under this size. Grammar: NUMBER
    /// [KiB|MiB|GiB] — suffix case-insensitive, a bare number is bytes
    /// (512MiB, 10GiB, 4096). Unset = the 10 GiB default.
    #[arg(
        long,
        value_name = "SIZE",
        value_parser = dartq_engine::store::parse_size_limit
    )]
    cache_size_limit: Option<u64>,
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
    /// Watch the package directory and re-analyze after changes settle
    /// (PRD §16.7 mode 3). Poll-based — a bounded-interval walk with burst
    /// debouncing — a convenience layer: explicit push and content
    /// verification stay the truth.
    #[arg(long)]
    watch: bool,
}

#[derive(Args)]
struct ExplainArgs {
    /// The symbol name to explain.
    symbol: String,
    /// Package directory holding pubspec.yaml and
    /// `.dart_tool/package_config.json`; defaults to the current directory.
    #[arg(default_value = ".")]
    dir: PathBuf,
    #[arg(long, value_enum, default_value_t = ExplainFormat::Json)]
    format: ExplainFormat,
    /// Dart SDK root; defaults to $DART_SDK.
    #[arg(long)]
    sdk: Option<PathBuf>,
    /// Report detail tier (the schema's verbosity table; `compact` by
    /// default, `minimal` swaps the section arrays for `counts`).
    #[arg(short = 'v', long, value_enum, default_value_t = Verbosity::Compact)]
    verbosity: Verbosity,
}

#[derive(Clone, Copy, ValueEnum)]
enum ExplainFormat {
    /// Single-line JSON report — the §15.3 contract's only format (the
    /// report is schema-valid at birth; this flag spells it).
    Json,
}

/// The report detail tier (dartq-commands [`Verbosity`]'s spelling for the
/// CLI; the canonical semantics live in dartq-schema's crate docs).
#[derive(Clone, Copy, ValueEnum)]
enum Verbosity {
    /// Red-line fields only: the symbol plus the declaration locators, the
    /// section arrays swapped for `counts`.
    Minimal,
    /// Every section as an array, empty sections included (the default).
    Compact,
    /// Compact (the canonical tables list no explain-only extras).
    Full,
}

impl Verbosity {
    fn into_commands(self) -> dartq_commands::Verbosity {
        match self {
            Self::Minimal => dartq_commands::Verbosity::Minimal,
            Self::Compact => dartq_commands::Verbosity::Compact,
            Self::Full => dartq_commands::Verbosity::Full,
        }
    }
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
    /// All reference rows of one symbol name, kinded (the same rows the
    /// symbol face serves today; the faces diverge as later semantics land).
    References(SymbolArgs),
    /// Invocation-shaped use sites of one symbol name (the references rows
    /// with kind `invoked`).
    Callers(SymbolArgs),
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
        dartq_commands::check::analyze_files(&files)
    }));
    let analyze_ms = elapsed_ms(analyze_at.elapsed());
    let run = match analyzed {
        Ok(Ok(run)) => run,
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
    let written = match args.format {
        CheckFormat::Machine => write_machine(&mut out, &run.output()),
        CheckFormat::Json => write_json(&mut out, &run.output()),
    };
    if let Err(error) = written.and_then(|()| out.flush()) {
        eprintln!("dartq: internal error: writing diagnostics failed: {error}");
        return exit::INTERNAL_ERROR;
    }

    if let Some(metrics_path) = &args.metrics_json {
        let metrics = RunMetrics {
            files: run.file_count(),
            loc: run.loc(),
            diagnostics: run.diagnostics().len(),
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
    severity_exit(run.diagnostics(), !args.no_fatal_warnings, args.fatal_infos)
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
            "schema_version": dartq_commands::SCHEMA_VERSION,
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
/// oracle-format output on `out`, metrics JSON at exit. The workspace
/// assembly and the engine pass — plain or `--overlay` — live in
/// `dartq-commands`; this shell adds the crash tier, the writers, and the
/// exit tier.
fn analyze_run(args: &AnalyzeArgs, out: &mut impl Write) -> u8 {
    if args.overlays.is_empty() {
        analyze_plain(args, out)
    } else {
        analyze_overlaid(args, out)
    }
}

/// One engine analyze pass under the crash tier (engine-api-spec §4.2, the
/// `check` face's shape): a panic anywhere in the pipeline exits 4 with
/// stdout untouched — the harness's auto-bisect keys the Panic signature on
/// exit 4 + empty stdout, and a partial diagnostic set must never masquerade
/// as a valid {0,1,2,3} result. A run `Err` stays the internal tier with the
/// error on stderr; both reports are stderr-only.
fn under_crash_tier<T>(
    pass: impl FnOnce() -> Result<T, dartq_commands::analyze::AnalyzeError>,
) -> Result<T, u8> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(pass)) {
        Ok(Ok(run)) => Ok(run),
        Ok(Err(error)) => {
            eprintln!("dartq: internal error: {error}");
            Err(exit::INTERNAL_ERROR)
        }
        Err(_) => {
            eprintln!("dartq: internal error: analysis panicked");
            Err(exit::INTERNAL_ERROR)
        }
    }
}

/// The plain path: the shared run, byte-pinned.
fn analyze_plain(args: &AnalyzeArgs, out: &mut impl Write) -> u8 {
    let started = Instant::now();
    let (mut engine, dir) = match open_engine(&args.dir, args.sdk.as_deref()) {
        Ok(faces) => faces,
        Err(error) => {
            eprintln!("error: {error}");
            return exit::USAGE;
        }
    };
    engine.set_memory_limit(args.memory_limit);
    engine.set_trace_sink(match &args.trace_events {
        Some(path) => match dartq_engine::event::TraceSink::create(path) {
            Ok(sink) => Some(sink),
            Err(error) => {
                eprintln!("error: trace events: {error}");
                return exit::USAGE;
            }
        },
        None => None,
    });
    arm_content_store(&mut engine, args.cache_size_limit);
    let run = match under_crash_tier(|| {
        dartq_commands::analyze::run(&mut engine, std::slice::from_ref(&dir))
    }) {
        Ok(run) => run,
        Err(code) => return code,
    };
    analyze_finish(args, &run, started, out)
}

/// Arms the L3 content store (PRD §17.2) at the default cache root with
/// `size_limit` (unset = the 10 GiB default): analyze persists each apply
/// batch's changed contents cross-session and the store evicts down to the
/// cap. An unwritable store warns on stderr and leaves the engine unarmed
/// — the cache is an accelerator, never a dependency; no home directory
/// (nowhere to put it) stays silently unarmed.
fn arm_content_store(engine: &mut dartq_engine::Engine, size_limit: Option<u64>) {
    let Some(root) = dartq_engine::store::default_root() else {
        return;
    };
    let config = dartq_engine::store::StoreConfig {
        root,
        size_limit: size_limit.unwrap_or(dartq_engine::store::DEFAULT_SIZE_LIMIT),
    };
    if let Err(error) = engine.open_content_store(config) {
        eprintln!("warning: content store disabled: {error}");
    }
}

/// The `--overlay` path (PRD §16.3): the specs decode here (relative paths
/// resolve against the package directory, `-` reads stdin), then
/// [`dartq_commands::analyze::run_overlaid`] runs the contract order —
/// discovery first, the overlay content enqueued after it (last-wins per
/// path), one analyze pass, the oracle display order — and this face
/// renders and tiers like every sibling.
fn analyze_overlaid(args: &AnalyzeArgs, out: &mut impl Write) -> u8 {
    let started = Instant::now();
    let (mut engine, dir) = match open_engine(&args.dir, args.sdk.as_deref()) {
        Ok(faces) => faces,
        Err(error) => {
            eprintln!("error: {error}");
            return exit::USAGE;
        }
    };
    engine.set_memory_limit(args.memory_limit);
    engine.set_trace_sink(match &args.trace_events {
        Some(path) => match dartq_engine::event::TraceSink::create(path) {
            Ok(sink) => Some(sink),
            Err(error) => {
                eprintln!("error: trace events: {error}");
                return exit::USAGE;
            }
        },
        None => None,
    });
    arm_content_store(&mut engine, args.cache_size_limit);
    let overlays = match load_overlays(&args.overlays, &args.dir) {
        Ok(overlays) => overlays,
        Err(error) => {
            eprintln!("error: {error}");
            return exit::USAGE;
        }
    };
    let run = match under_crash_tier(|| {
        dartq_commands::analyze::run_overlaid(&mut engine, &overlays, std::slice::from_ref(&dir))
    }) {
        Ok(run) => run,
        Err(code) => return code,
    };
    analyze_finish(args, &run, started, out)
}

/// The shared analyze postlude: oracle-format diagnostics on `out`, the
/// `--metrics-json` facts at exit, then the severity exit tier.
fn analyze_finish(
    args: &AnalyzeArgs,
    run: &AnalyzeRun<'_>,
    started: Instant,
    out: &mut impl Write,
) -> u8 {
    let discovery_ms = elapsed_ms(run.discovery);
    let analyze_ms = elapsed_ms(run.analysis);

    let written = match args.format {
        AnalyzeFormat::Machine => write_machine(
            out,
            &AnalysisOutput {
                diagnostics: &run.diagnostics,
                files: &run.layout,
            },
        ),
        AnalyzeFormat::Human => write_human(out, &run.diagnostics, &run.layout),
    };
    if let Err(error) = written.and_then(|()| out.flush()) {
        eprintln!("dartq: internal error: writing diagnostics failed: {error}");
        return exit::INTERNAL_ERROR;
    }

    if let Some(metrics_path) = &args.metrics_json {
        let metrics = RunMetrics {
            files: run.file_count,
            loc: run.loc,
            diagnostics: run.diagnostics.len(),
            discovery_ms,
            analyze_ms,
            total_ms: elapsed_ms(started.elapsed()),
            libraries_analyzed: run.libraries_analyzed,
            libraries_from_cache: run.libraries_from_cache,
            max_rss_bytes: max_rss_bytes(),
        };
        if let Err(error) = write_metrics(metrics_path, &metrics) {
            eprintln!("dartq: internal error: writing metrics failed: {error}");
            return exit::INTERNAL_ERROR;
        }
    }
    severity_exit(&run.diagnostics, !args.no_fatal_warnings, args.fatal_infos)
}

/// Decodes `--overlay PATH=FILE` specs: the analyzed path (relative specs
/// resolve against the package directory — the CLI twin of the daemon's
/// workspace-root resolution) plus the local file whose content replaces
/// the on-disk bytes (`-` reads stdin, the AI-agent buffer case).
fn load_overlays(specs: &[String], dir: &Path) -> Result<Vec<(PathBuf, Vec<u8>)>, String> {
    specs
        .iter()
        .map(|spec| {
            let (path, file) = spec
                .split_once('=')
                .ok_or_else(|| format!("--overlay expects PATH=FILE, got `{spec}`"))?;
            if path.is_empty() || file.is_empty() {
                return Err(format!("--overlay expects PATH=FILE, got `{spec}`"));
            }
            let path = if Path::new(path).is_absolute() {
                PathBuf::from(path)
            } else {
                dir.join(path)
            }
            // The engine's discovery interns canonical package-root paths;
            // the overlay must address the same identity (a non-canonical
            // spelling would land on a phantom extra file). A path that
            // does not exist yet (agent-introduced) keeps its spelling.
            .canonicalize()
            .unwrap_or_else(|_| PathBuf::from(path));
            let bytes = if file == "-" {
                let mut bytes = Vec::new();
                io::stdin()
                    .read_to_end(&mut bytes)
                    .map_err(|error| format!("reading overlay stdin failed: {error}"))?;
                bytes
            } else {
                fs::read(file)
                    .map_err(|error| format!("reading overlay `{file}` failed: {error}"))?
            };
            Ok((path, bytes))
        })
        .collect()
}

/// The LSP daemon face: assemble the engine context, hand it to the daemon
/// session, and serve stdin/stdout until `shutdown`+`exit` or EOF. The
/// workspace sweep (discovery + first analysis) runs inside the session at
/// `initialized`, so the client sees full diagnostics immediately after the
/// handshake.
fn daemon(args: &DaemonArgs) -> u8 {
    let (config, _dir) = match engine_config(&args.dir, args.sdk.as_deref()) {
        Ok(faces) => faces,
        Err(error) => {
            eprintln!("error: {error}");
            return exit::USAGE;
        }
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
    // process exit reaps it. With `--watch`, the server also spawns its poll
    // thread over the package directory (PRD §16.7 mode 3).
    let stdin = io::BufReader::new(io::stdin());
    let stdout = io::BufWriter::new(io::stdout().lock());
    let watch = args
        .watch
        .then(|| dartq_daemon::WatchOptions::new(args.dir.clone()));
    match dartq_daemon::run(stdin, stdout, session, watch) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("dartq: internal error: daemon transport failed: {error}");
            exit::INTERNAL_ERROR
        }
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
        // The oracle's clean-set tail carries the exclamation mark
        // (`dart analyze`, 3.13.1: `No issues found!`); the counted tails
        // keep the period (`N issues found.`).
        0 => writeln!(out, "No issues found!"),
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

/// `dartq query references` — the same rows the symbol face serves today
/// (the index's reference surface is name-keyed and already kinded); a
/// separate spelling per the PRD §15.2 family, diverging as later semantics
/// land. Same tiers, same runner shape.
fn query_references(args: &SymbolArgs) -> u8 {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        query_references_run(args, &mut BufWriter::new(io::stdout().lock()))
    }));
    match outcome {
        Ok(code) => code,
        Err(_) => {
            eprintln!("dartq: internal error: analysis panicked");
            exit::INTERNAL_ERROR
        }
    }
}

/// The references face's runner: the symbol runner verbatim (the seam where
/// the faces' rows diverge).
fn query_references_run(args: &SymbolArgs, out: &mut impl Write) -> u8 {
    query_symbol_run(args, out)
}

/// `dartq query callers` — crash tier like [`query_symbol`].
fn query_callers(args: &SymbolArgs) -> u8 {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        query_callers_run(args, &mut BufWriter::new(io::stdout().lock()))
    }));
    match outcome {
        Ok(code) => code,
        Err(_) => {
            eprintln!("dartq: internal error: analysis panicked");
            exit::INTERNAL_ERROR
        }
    }
}

/// One callers run: the symbol query's runner with the invocation-shaped
/// filter applied (`dartq_commands::query::callers`); exit 0 on success,
/// zero results included.
fn query_callers_run(args: &SymbolArgs, out: &mut impl Write) -> u8 {
    let (mut engine, _dir) = match open_engine(&args.dir, args.sdk.as_deref()) {
        Ok(faces) => faces,
        Err(error) => {
            eprintln!("error: {error}");
            return exit::USAGE;
        }
    };
    let run = match dartq_commands::query::callers(&mut engine, &args.name) {
        Ok(run) => run,
        Err(error) => {
            eprintln!("dartq: internal error: {error}");
            return exit::INTERNAL_ERROR;
        }
    };

    let written = write_query_out(out, &run, &args.name, args.format);
    if let Err(error) = written.and_then(|()| out.flush()) {
        eprintln!("dartq: internal error: writing query results failed: {error}");
        return exit::INTERNAL_ERROR;
    }
    exit::CLEAN
}

/// The query faces' shared output: machine rows or the one-line JSON
/// envelope (`dartq_commands::query::to_json`, the frozen v1 contract).
fn write_query_out(
    out: &mut impl Write,
    run: &dartq_commands::query::SymbolRun<'_>,
    name: &str,
    format: QueryFormat,
) -> io::Result<()> {
    match format {
        QueryFormat::Machine => write_query_machine(out, &run.layout, &run.decls, &run.refs),
        QueryFormat::Json => writeln!(out, "{}", dartq_commands::query::to_json(run, name)),
    }
}

/// One query run: the analyze workspace assembly, one analyze pass (the
/// read-after-write — the query reflects and never re-drives it), then the
/// engine's symbol answer on `out`. Exit 0 on success, zero results
/// included.
fn query_symbol_run(args: &SymbolArgs, out: &mut impl Write) -> u8 {
    let (mut engine, _dir) = match open_engine(&args.dir, args.sdk.as_deref()) {
        Ok(faces) => faces,
        Err(error) => {
            eprintln!("error: {error}");
            return exit::USAGE;
        }
    };
    let run = match dartq_commands::query::symbol(&mut engine, &args.name) {
        Ok(run) => run,
        Err(error) => {
            eprintln!("dartq: internal error: {error}");
            return exit::INTERNAL_ERROR;
        }
    };

    let written = write_query_out(out, &run, &args.name, args.format);
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
/// REF|target-file|kind|qualified|offset|length|use-file
///                                                 (offset is the use
///                                                 site's UTF-16 offset)
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
            "REF|{}|{}|{}|{}|{}|{}",
            layout.file_path(reference.target_file).display(),
            ref_kind_name(reference.kind),
            reference.qualified,
            reference.range.offset,
            reference.range.length,
            layout.file_path(reference.file).display(),
        )?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// dartq explain — the §15.3 AI report face
// ---------------------------------------------------------------------------

/// `dartq explain` — crash tier like the other engine faces: a panic aborts
/// with exit 4 and stdout kept empty, because [`explain_run`] writes only
/// after the report fully succeeded.
fn explain_entry(args: &ExplainArgs) -> u8 {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        explain_run(args, &mut BufWriter::new(io::stdout().lock()))
    }));
    match outcome {
        Ok(code) => code,
        Err(_) => {
            eprintln!("dartq: internal error: analysis panicked");
            exit::INTERNAL_ERROR
        }
    }
}

/// One explain run: the workspace assembly, the collector-installed engine,
/// and the report assembly all live in `dartq_commands::explain` (the one
/// implementation the daemon's `dartq/explain` shares); this shell adds the
/// crash tier, the tier mapping, and the JSON output. The report is
/// schema-valid at birth (validated against `Domain::Explain` where it is
/// built); an unknown symbol is the report's own empty seam report, not an
/// error — printed as-is, exit 0. Exit tiers: workspace/engine assembly is
/// the usage tier, discovery/cancellation the internal tier.
fn explain_run(args: &ExplainArgs, out: &mut impl Write) -> u8 {
    let report = match dartq_commands::explain::explain(
        &args.dir,
        args.sdk.as_deref(),
        &args.symbol,
        args.verbosity.into_commands(),
    ) {
        Ok(report) => report,
        Err(error @ (ExplainError::Workspace(_) | ExplainError::Engine(_))) => {
            eprintln!("error: {error}");
            return exit::USAGE;
        }
        Err(error) => {
            eprintln!("dartq: internal error: {error}");
            return exit::INTERNAL_ERROR;
        }
    };

    let written = out
        .write_all(
            serde_json::to_string(&report)
                .expect("the validated report serializes")
                .as_bytes(),
        )
        .and_then(|()| out.write_all(b"\n"))
        .and_then(|()| out.flush());
    if let Err(error) = written {
        eprintln!("dartq: internal error: writing the explain report failed: {error}");
        return exit::INTERNAL_ERROR;
    }
    exit::CLEAN
}

/// One JSON object at exit (engine-api-spec §3, benchmark-harness-spec §2.4).
/// A failed write is an internal error (4) at the call site, never silent —
/// a run without its metrics file would corrupt benchmark data.
fn write_metrics(path: &Path, metrics: &RunMetrics) -> io::Result<()> {
    let mut file = fs::File::create(path)?;
    writeln!(file, "{}", metrics.json())
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use dartq_diagnostics::{EXPECTED_TOKEN, SourceRange};
    use dartq_source::FileTable;

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
            &["query", "references"][..],
            &["query", "callers"][..],
            &["query", "bogus"][..],
            &["explain"][..],
            &["explain", "--format=machine", "Dog"][..],
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

    /// The compat face's `--context` flag: repeatable labels, `--sdk-root`
    /// unchanged alongside.
    #[test]
    fn compat_context_flag_parse() {
        let parsed = parse_args_of(&[
            "compat",
            "--context",
            "a",
            "--context",
            "b",
            "--sdk-root",
            "/s",
            "f.dart",
        ])
        .expect("parse")
        .expect("subcommand");
        let Command::Compat(args) = parsed else {
            panic!("expected compat command")
        };
        assert_eq!(args.contexts, vec!["a".to_owned(), "b".to_owned()]);
        assert_eq!(args.sdk_roots, vec![PathBuf::from("/s")]);
        assert_eq!(args.path, PathBuf::from("f.dart"));
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
        assert!(args.overlays.is_empty(), "no overlay by default");
        assert!(!args.no_fatal_warnings && !args.fatal_infos);
        assert_eq!(args.memory_limit, None, "unlimited by default");
        assert_eq!(args.trace_events, None, "untraced by default");
        assert_eq!(
            args.cache_size_limit, None,
            "unset = the 10 GiB default cap"
        );
        assert_eq!(args.dir, PathBuf::from("/pkg"));

        let parsed = parse_args_of(&[
            "analyze",
            "--format=human",
            "--metrics-json",
            "/tmp/m.json",
            "--overlay",
            "lib/a.dart=/tmp/a.dart",
            "--overlay=lib/b.dart=/tmp/b.dart",
            "--sdk",
            "/sdk",
            "--no-fatal-warnings",
            "--fatal-infos",
            "--memory-limit",
            "2048",
            "--trace-events",
            "/tmp/dartq-trace.jsonl",
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
        assert_eq!(
            args.overlays,
            vec![
                "lib/a.dart=/tmp/a.dart".to_owned(),
                "lib/b.dart=/tmp/b.dart".to_owned(),
            ],
            "repeatable --overlay specs"
        );
        assert!(args.no_fatal_warnings && args.fatal_infos);
        assert_eq!(
            args.memory_limit,
            Some(2048),
            "--memory-limit MiB parses and plumbs"
        );
        assert_eq!(
            args.trace_events,
            Some(PathBuf::from("/tmp/dartq-trace.jsonl")),
            "--trace-events parses and plumbs"
        );
    }

    /// The `--cache-size-limit` grammar rides the engine's
    /// `parse_size_limit` through the clap value parser: suffixed and bare
    /// sizes parse to bytes; a typo is a clap usage error carrying the
    /// accepted spelling.
    #[test]
    fn cache_size_limit_flag_parses_through_the_grammar() {
        let parsed = |text: &str| {
            let args = parse_args_of(&["analyze", "--cache-size-limit", text, "/pkg"])
                .expect("parse")
                .expect("subcommand");
            let Command::Analyze(args) = args else {
                panic!("expected analyze command")
            };
            args.cache_size_limit
        };
        assert_eq!(parsed("512MiB"), Some(512 * 1024 * 1024));
        assert_eq!(parsed("10GiB"), Some(10 * 1024 * 1024 * 1024));
        assert_eq!(parsed("4096"), Some(4096), "a bare number is bytes");
        for text in ["10TiB", "1.5GiB", "huge"] {
            assert!(
                parse_args_of(&["analyze", "--cache-size-limit", text, "/pkg"]).is_err(),
                "`{text}` must be a usage error"
            );
        }
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
            overlays: Vec::new(),
            sdk: Some(root.join("sdk")),
            no_fatal_warnings: false,
            fatal_infos: false,
            memory_limit: None,
            trace_events: None,
            cache_size_limit: None,
        }
    }

    /// End to end over a real temp workspace: engine discovery + analysis,
    /// oracle machine bytes on stdout, error-tier exit code, metrics JSON at
    /// exit.
    ///
    /// Hand trace against the serial oracle probe (dart 3.13.1, machine) and
    /// `tests/harness/src/machine.rs` (`parse_machine`): 8 `|`-separated
    /// fields; `void f() { x }` puts `x` at UTF-16 offset 11 → line 1,
    /// column 12, length 1 on BOTH rows — the syntax row ("Expected to find
    /// ';'.") and the semantic row ("Undefined name 'x'.") share the token's
    /// span and order by the message tie-break.
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
                "ERROR|SYNTACTIC_ERROR|EXPECTED_TOKEN|{}|1|12|1|Expected to find ';'.\n\
                 ERROR|COMPILE_TIME_ERROR|UNDEFINED_IDENTIFIER|{}|1|12|1|Undefined name 'x'.\n",
                broken.display(),
                broken.display()
            ),
            "oracle machine bytes (emitMachineFormat field order; serial oracle probe)"
        );
        assert_eq!(code, exit::ERRORS, "any ERROR → 3 (analyze.dart:398)");

        let metrics: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(metrics_path).expect("metrics at exit"))
                .expect("one JSON object");
        assert_eq!(metrics["files"], 2, "both lib files discovered");
        assert_eq!(metrics["diagnostics"], 2);
        assert_eq!(metrics["libraries_analyzed"], 2, "cold run recompute");
        assert_eq!(metrics["libraries_from_cache"], 0);
        assert!(metrics["max_rss_bytes"].is_u64(), "getrusage hook wired");
        assert!(metrics["total_ms"].is_u64());

        fs::remove_dir_all(&root).ok();
    }

    /// Clean set → exit 0 with empty machine output (the oracle empty edge);
    /// the human clean edge carries no per-file heading (nothing to group)
    /// and ends in the oracle tail — over the broken fixture both oracle
    /// rows appear under the one file heading (oracle display order,
    /// message tie-break within the shared token span) and the tail counts
    /// both.
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
        // The whole oracle clean shape in one pin: no `lib/clean.dart`
        // heading (the oracle prints none on a clean set), no
        // `Analyzing ...` preamble from this face, and the exact tail
        // spelling (`dart analyze`, 3.13.1).
        let text = String::from_utf8(out).expect("UTF-8");
        assert_eq!(text, "No issues found!\n", "oracle clean human output");

        fs::remove_dir_all(&root).ok();

        // The broken twin: both oracle rows under the file heading.
        let broken_root =
            analyze_fixture("faces-broken", &[("lib/broken.dart", "void f() { x }\n")]);
        let mut out: Vec<u8> = Vec::new();
        let code = analyze_run(
            &analyze_args(&broken_root, AnalyzeFormat::Human, None),
            &mut out,
        );
        assert_eq!(code, exit::ERRORS);
        let text = String::from_utf8(out).expect("UTF-8");
        let heading = broken_root
            .canonicalize()
            .expect("root")
            .join("lib/broken.dart");
        assert!(
            text.contains(heading.to_string_lossy().as_ref()),
            "file heading: {text}"
        );
        assert!(
            text.contains("• expected_token • Expected to find ';'.")
                && text.contains("• undefined_identifier • Undefined name 'x'."),
            "both oracle rows under the heading, in display order: {text}"
        );
        assert!(
            text.contains("2 issues found."),
            "the tail counts both rows: {text}"
        );

        fs::remove_dir_all(&broken_root).ok();
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

    /// The analyze face's crash tier: a panic anywhere in the engine pass
    /// maps to exit 4 — the exit half of the harness's Panic signature
    /// (exit 4 + empty stdout; stdout stays empty structurally because the
    /// writers run only after the pass succeeds, the `check` face's
    /// `catch_unwind` shape) — a run `Err` stays the internal tier, and a
    /// clean pass passes through with no exit override.
    #[test]
    fn analyze_crash_tier_maps_panic_to_internal_exit() {
        use dartq_commands::analyze::AnalyzeError;

        let panicked = under_crash_tier(|| -> Result<(), AnalyzeError> { panic!("injected") });
        assert!(
            matches!(panicked, Err(exit::INTERNAL_ERROR)),
            "panic → exit 4, not the runtime's 101"
        );

        let failed = under_crash_tier(|| Err::<(), _>(AnalyzeError::Cancelled));
        assert!(matches!(failed, Err(exit::INTERNAL_ERROR)), "run Err → 4");

        assert_eq!(
            under_crash_tier(|| Ok::<_, AnalyzeError>(7_u8)),
            Ok(7_u8),
            "a clean pass passes through"
        );
    }

    /// The `--overlay` face (PRD §16.3): the analyzed file's content is
    /// replaced by the local file's bytes — a clean overlay fixes the
    /// broken fixture (exit 0, empty machine output), an overlay carrying
    /// an error reports it against the analyzed path in exact oracle bytes
    /// (the mirrored postlude's pin), and a missing local file is the usage
    /// tier. The overlay path never rewrites the workspace on disk.
    #[test]
    fn analyze_overlay_replaces_content_for_one_run() {
        let root = analyze_fixture("overlay", &[("lib/broken.dart", "void f() { x }\n")]);
        // The overlay SOURCE files live outside the analyzed tree — a
        // root-level `.dart` file would be discovered and analyzed itself.
        let sources =
            std::env::temp_dir().join(format!("dq_cli_overlay_src_{}", std::process::id()));
        let _ = fs::remove_dir_all(&sources);
        fs::create_dir_all(&sources).expect("mkdir");
        let clean = sources.join("clean.dart");
        fs::write(&clean, "var ok = 1;\n").expect("write");
        let broken = sources.join("broken.dart");
        fs::write(&broken, "void g() { y }\n").expect("write");

        let target = root.join("lib/broken.dart");
        // Clean overlay: the error is gone.
        let fixed = AnalyzeArgs {
            overlays: vec![format!("{}={}", target.display(), clean.display())],
            ..analyze_args(&root, AnalyzeFormat::Machine, None)
        };
        assert_eq!(analyze_run(&fixed, &mut Vec::new()), exit::CLEAN);

        // The overlay never touches disk: the on-disk file stays broken.
        assert_eq!(
            fs::read_to_string(&target).expect("target intact"),
            "void f() { x }\n"
        );

        // Broken overlay: the errors surface against the analyzed path in
        // the oracle machine geometry (engine-canonical → oracle display
        // order; serial oracle probe on `void g() { y }`: `y` at UTF-16
        // offset 11 → line 1, column 12, length 1 on BOTH rows — the syntax
        // row ("Expected to find ';'.") and the semantic row ("Undefined
        // name 'y'.") share the token's span and order by the message
        // tie-break).
        let mut out: Vec<u8> = Vec::new();
        let broken_overlay = AnalyzeArgs {
            overlays: vec![format!("{}={}", target.display(), broken.display())],
            ..analyze_args(&root, AnalyzeFormat::Machine, None)
        };
        assert_eq!(analyze_run(&broken_overlay, &mut out), exit::ERRORS);
        assert_eq!(
            String::from_utf8(out).expect("stdout is UTF-8"),
            format!(
                "ERROR|SYNTACTIC_ERROR|EXPECTED_TOKEN|{}|1|12|1|Expected to find ';'.\n\
                 ERROR|COMPILE_TIME_ERROR|UNDEFINED_IDENTIFIER|{}|1|12|1|Undefined name 'y'.\n",
                target.canonicalize().expect("root").display(),
                target.canonicalize().expect("root").display()
            ),
            "the overlay path pins the mirrored oracle sort + writer bytes"
        );

        // Relative overlay spec resolves against the package dir.
        let relative = AnalyzeArgs {
            overlays: vec![format!("lib/broken.dart={}", clean.display())],
            ..analyze_args(&root, AnalyzeFormat::Machine, None)
        };
        assert_eq!(analyze_run(&relative, &mut Vec::new()), exit::CLEAN);

        // Missing local file → usage tier.
        let missing = AnalyzeArgs {
            overlays: vec![format!(
                "{}={}",
                target.display(),
                root.join("nope").display()
            )],
            ..analyze_args(&root, AnalyzeFormat::Machine, None)
        };
        assert_eq!(analyze_run(&missing, &mut Vec::new()), exit::USAGE);

        // Malformed spec → usage tier.
        let malformed = AnalyzeArgs {
            overlays: vec!["no-equals-sign".to_owned()],
            ..analyze_args(&root, AnalyzeFormat::Machine, None)
        };
        assert_eq!(analyze_run(&malformed, &mut Vec::new()), exit::USAGE);

        fs::remove_dir_all(&root).ok();
        fs::remove_dir_all(&sources).ok();
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
        let QueryFace::Symbol(symbol) = args.face else {
            panic!("expected symbol face")
        };
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
        let QueryFace::Symbol(symbol) = args.face else {
            panic!("expected symbol face")
        };
        assert!(matches!(symbol.format, QueryFormat::Json));
        assert_eq!(symbol.sdk, Some(PathBuf::from("/sdk")));
        assert_eq!(symbol.dir, PathBuf::from("/pkg"));
        assert_eq!(symbol.name, "Foo");
    }

    /// The references/callers faces share the symbol grammar (name
    /// positional, dir, format, sdk).
    #[test]
    fn query_new_faces_parse() {
        let parsed = parse_args_of(&["query", "references", "--format=json", "Foo", "/pkg"])
            .expect("parse")
            .expect("subcommand");
        let Command::Query(args) = parsed else {
            panic!("expected query command")
        };
        let QueryFace::References(symbol) = args.face else {
            panic!("expected references face")
        };
        assert_eq!(symbol.name, "Foo");
        assert_eq!(symbol.dir, PathBuf::from("/pkg"));

        let parsed = parse_args_of(&["query", "callers", "Foo"])
            .expect("parse")
            .expect("subcommand");
        let Command::Query(args) = parsed else {
            panic!("expected query command")
        };
        let QueryFace::Callers(symbol) = args.face else {
            panic!("expected callers face")
        };
        assert_eq!(symbol.name, "Foo");
        assert_eq!(symbol.dir, PathBuf::from("."), "default target cwd");
    }

    fn symbol_args(root: &Path, format: QueryFormat, name: &str) -> SymbolArgs {
        SymbolArgs {
            name: name.to_string(),
            dir: root.to_path_buf(),
            format,
            sdk: Some(root.join("sdk")),
        }
    }

    /// End to end over a real temp workspace: the class plus its cross-file
    /// uses (two bare, one import-prefixed) in machine bytes (target file,
    /// then offset order), a member hit with its container name, the JSON
    /// face (the `qualified` column: omitted on bare rows, `p.Foo` on the
    /// prefixed row), the empty-result edge (exit 0, no rows), and the
    /// usage tier.
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
                (
                    "lib/c.dart",
                    "import 'a.dart' as p;\n\nvoid from_c() {\n  p.Foo();\n}\n",
                ),
            ],
        );
        let a = root.canonicalize().expect("root").join("lib/a.dart");
        let b = root.canonicalize().expect("root").join("lib/b.dart");
        let c = root.canonicalize().expect("root").join("lib/c.dart");

        // `Foo? f` types a reference; `Foo()` is an implicit-creation
        // invocation; both target a.dart unqualified, use sites in b.dart;
        // c.dart's `p.Foo()` arrives through the armed `p` prefix.
        let mut out: Vec<u8> = Vec::new();
        let code = query_symbol_run(&symbol_args(&root, QueryFormat::Machine, "Foo"), &mut out);
        assert_eq!(code, exit::CLEAN);
        assert_eq!(
            String::from_utf8(out).expect("UTF-8"),
            format!(
                "DECL|{}|-|class|Foo|1|7|3\n\
                 REF|{}|reference|false|33|3|{}\n\
                 REF|{}|invoked|false|42|3|{}\n\
                 REF|{}|invoked|true|43|3|{}\n",
                a.display(),
                a.display(),
                b.display(),
                a.display(),
                b.display(),
                a.display(),
                c.display()
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
        assert_eq!(json["references"].as_array().expect("array").len(), 3);
        assert_eq!(json["references"][0]["kind"], "reference");
        assert_eq!(
            json["references"][0]["target_file"],
            a.display().to_string()
        );
        // The qualified-name column rides armed-prefix rows only: bare rows
        // keep the key omitted (the schema's optional string), the prefixed
        // `p.Foo()` row carries the exact spelling.
        assert!(json["references"][0].get("qualified").is_none());
        assert_eq!(json["references"][0]["offset"], 33);
        assert_eq!(json["references"][1]["kind"], "invoked");
        assert_eq!(json["references"][1]["offset"], 42);
        assert!(json["references"][1].get("qualified").is_none());
        assert_eq!(json["references"][2]["kind"], "invoked");
        assert_eq!(json["references"][2]["offset"], 43);
        assert_eq!(json["references"][2]["qualified"], "p.Foo");

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

    /// The references/callers faces end to end over the symbol test's
    /// fixture: `references` serves the symbol answer's rows byte-for-byte;
    /// `callers` keeps only the `invoked` rows (the `Foo()` constructor
    /// site at UTF-16 offset 41, the `foo()` call at 50) while the
    /// declaration rows stay for target identity; a declaration with no
    /// invocations prints its DECL row and no REF rows; the JSON envelope is
    /// the frozen v1 shape.
    #[test]
    fn query_references_and_callers_end_to_end() {
        let root = analyze_fixture(
            "refcall",
            &[
                (
                    "lib/a.dart",
                    "class Foo {\n  void meth() {}\n}\nvoid foo() {}\n",
                ),
                (
                    "lib/b.dart",
                    "import 'a.dart';\nvoid main() {\n  var f = Foo();\n  foo();\n}\n",
                ),
            ],
        );
        let a = root.canonicalize().expect("root").join("lib/a.dart");
        let b = root.canonicalize().expect("root").join("lib/b.dart");

        // references == symbol rows (the index's reference surface is
        // name-keyed and already kinded).
        let mut symbol_out: Vec<u8> = Vec::new();
        assert_eq!(
            query_symbol_run(
                &symbol_args(&root, QueryFormat::Machine, "Foo"),
                &mut symbol_out
            ),
            exit::CLEAN
        );
        let mut references_out: Vec<u8> = Vec::new();
        assert_eq!(
            query_references_run(
                &symbol_args(&root, QueryFormat::Machine, "Foo"),
                &mut references_out
            ),
            exit::CLEAN
        );
        assert_eq!(
            String::from_utf8(symbol_out).expect("UTF-8"),
            String::from_utf8(references_out).expect("UTF-8"),
            "the references spelling serves the symbol rows"
        );

        // callers Foo: the constructor site (the `var f` local carries no
        // reference site).
        let mut out: Vec<u8> = Vec::new();
        assert_eq!(
            query_callers_run(&symbol_args(&root, QueryFormat::Machine, "Foo"), &mut out),
            exit::CLEAN
        );
        assert_eq!(
            String::from_utf8(out).expect("UTF-8"),
            format!(
                "DECL|{}|-|class|Foo|1|7|3\nREF|{}|invoked|false|41|3|{}\n",
                a.display(),
                a.display(),
                b.display()
            )
        );

        // callers foo: the one call site.
        let mut out: Vec<u8> = Vec::new();
        assert_eq!(
            query_callers_run(&symbol_args(&root, QueryFormat::Machine, "foo"), &mut out),
            exit::CLEAN
        );
        assert_eq!(
            String::from_utf8(out).expect("UTF-8"),
            format!(
                "DECL|{}|-|function|foo|4|6|3\nREF|{}|invoked|false|50|3|{}\n",
                a.display(),
                a.display(),
                b.display()
            )
        );

        // callers meth: declared, never invoked — DECL row, no REF rows.
        let mut out: Vec<u8> = Vec::new();
        assert_eq!(
            query_callers_run(&symbol_args(&root, QueryFormat::Machine, "meth"), &mut out),
            exit::CLEAN
        );
        assert_eq!(
            String::from_utf8(out).expect("UTF-8"),
            format!("DECL|{}|Foo|method|meth|2|8|4\n", a.display())
        );

        // callers JSON: the frozen v1 envelope with the filtered references.
        let mut out: Vec<u8> = Vec::new();
        assert_eq!(
            query_callers_run(&symbol_args(&root, QueryFormat::Json, "Foo"), &mut out),
            exit::CLEAN
        );
        let json: serde_json::Value =
            serde_json::from_str(&String::from_utf8(out).expect("UTF-8")).expect("one object");
        assert_eq!(json["schema_version"], 1);
        assert_eq!(json["name"], "Foo");
        assert_eq!(json["declarations"].as_array().expect("array").len(), 1);
        let refs = json["references"].as_array().expect("array");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0]["kind"], "invoked");
        assert_eq!(refs[0]["offset"], 41);
        assert_eq!(refs[0]["target_file"], a.display().to_string());

        // Usage tier: missing directory.
        let missing = symbol_args(&root.join("nope"), QueryFormat::Machine, "Foo");
        assert_eq!(query_callers_run(&missing, &mut Vec::new()), exit::USAGE);

        fs::remove_dir_all(&root).ok();
    }

    // -----------------------------------------------------------------------
    // dartq explain
    // -----------------------------------------------------------------------

    /// The explain command face: symbol positional, default dir, flag
    /// spellings (`-v`/`--verbosity`, `--format=json`). Malformed spellings
    /// are covered by `usage_errors`.
    #[test]
    fn explain_args_parse() {
        let parsed = parse_args_of(&["explain", "Dog"])
            .expect("parse")
            .expect("subcommand");
        let Command::Explain(args) = parsed else {
            panic!("expected explain command")
        };
        assert_eq!(args.symbol, "Dog");
        assert_eq!(args.dir, PathBuf::from("."), "default target cwd");
        assert!(args.sdk.is_none());
        assert!(matches!(args.format, ExplainFormat::Json), "json default");
        assert!(
            matches!(args.verbosity, Verbosity::Compact),
            "compact default"
        );

        let parsed = parse_args_of(&[
            "explain",
            "--format=json",
            "--sdk",
            "/sdk",
            "-v",
            "full",
            "Dog",
            "/pkg",
        ])
        .expect("parse")
        .expect("subcommand");
        let Command::Explain(args) = parsed else {
            panic!("expected explain command")
        };
        assert_eq!(args.symbol, "Dog");
        assert_eq!(args.dir, PathBuf::from("/pkg"));
        assert_eq!(args.sdk, Some(PathBuf::from("/sdk")));
        assert!(matches!(args.verbosity, Verbosity::Full), "-v full parses");
        assert!(matches!(args.format, ExplainFormat::Json));

        let parsed = parse_args_of(&["explain", "--verbosity", "minimal", "Dog"])
            .expect("parse")
            .expect("subcommand");
        let Command::Explain(args) = parsed else {
            panic!("expected explain command")
        };
        assert!(matches!(args.verbosity, Verbosity::Minimal));
    }

    /// The explain fixture: one import chain plus its test — the same shape
    /// the dartq-commands explain tests pin, over the analyze faces' pub
    /// workspace + stub SDK.
    const EXPLAIN_ANIMAL: &str = "class Animal {\n  void breathe() {}\n}\n";
    const EXPLAIN_DOG: &str =
        "import 'animal.dart';\nclass Dog extends Animal {\n  void bark() {}\n}\n";
    const EXPLAIN_MAIN: &str =
        "import 'dog.dart';\nvoid main() {\n  var d = Dog();\n  d.bark();\n}\n";
    const EXPLAIN_TEST: &str =
        "import 'package:fixture/dog.dart';\nvoid main() {\n  var d = Dog();\n}\n";

    /// The name token's start offset in `source` (ASCII sources, so byte
    /// offset = UTF-16 offset).
    fn explain_offset(source: &str, needle: &str) -> u32 {
        source
            .find(needle)
            .unwrap_or_else(|| panic!("{needle} in source")) as u32
    }

    fn explain_args(root: &Path, verbosity: Verbosity, symbol: &str) -> ExplainArgs {
        ExplainArgs {
            symbol: symbol.to_owned(),
            dir: root.to_path_buf(),
            format: ExplainFormat::Json,
            sdk: Some(root.join("sdk")),
            verbosity,
        }
    }

    /// End to end over a real temp workspace: the compact report carries
    /// every section the substrate reaches (the validated-at-birth JSON the
    /// daemon face mirrors field for field), `minimal` swaps the arrays for
    /// `counts`, an unknown symbol prints the seam report at exit 0, and a
    /// missing directory is the usage tier.
    #[test]
    fn explain_end_to_end_pins_the_validated_report() {
        let root = analyze_fixture(
            "explain",
            &[
                ("lib/animal.dart", EXPLAIN_ANIMAL),
                ("lib/dog.dart", EXPLAIN_DOG),
                ("lib/main.dart", EXPLAIN_MAIN),
                ("test/dog_test.dart", EXPLAIN_TEST),
            ],
        );
        let canonical = root.canonicalize().expect("root");
        let animal = canonical.join("lib/animal.dart").display().to_string();
        let dog = canonical.join("lib/dog.dart").display().to_string();
        let main = canonical.join("lib/main.dart").display().to_string();
        let dog_test = canonical.join("test/dog_test.dart").display().to_string();

        // Compact: the full section set.
        let mut out: Vec<u8> = Vec::new();
        let code = explain_run(&explain_args(&root, Verbosity::Compact, "Dog"), &mut out);
        assert_eq!(code, exit::CLEAN);
        let line = String::from_utf8(out).expect("UTF-8");
        assert!(
            line.ends_with('\n') && line.lines().count() == 1,
            "one JSON line"
        );
        let json: serde_json::Value =
            serde_json::from_str(line.trim_end()).expect("the report parses");
        assert_eq!(json["schema_version"], 1);
        assert_eq!(json["symbol"], "Dog");
        assert_eq!(
            json["declaration"],
            serde_json::json!({
                "file": dog,
                "name": "Dog",
                "kind": "class",
                "range": [
                    explain_offset(EXPLAIN_DOG, "Dog"),
                    explain_offset(EXPLAIN_DOG, "Dog") + 3,
                ],
            })
        );
        assert_eq!(
            json["inheritance"],
            serde_json::json!([{ "name": "Animal", "file": animal }])
        );
        assert_eq!(
            json["methods"],
            serde_json::json!([{
                "name": "bark",
                "kind": "method",
                "range": [
                    explain_offset(EXPLAIN_DOG, "bark"),
                    explain_offset(EXPLAIN_DOG, "bark") + 4,
                ],
            }])
        );
        let references = json["references"].as_array().expect("references");
        assert_eq!(references.len(), 2, "the main use plus the test use");
        assert_eq!(references[0]["file"], main);
        assert_eq!(references[0]["kind"], "invoked");
        assert_eq!(references[1]["file"], dog_test);
        assert_eq!(
            json["dependencies"],
            serde_json::json!([{ "path": main }, { "path": dog_test }])
        );
        assert_eq!(
            json["affected_tests"],
            serde_json::json!([{ "path": dog_test }])
        );

        // Minimal: the declaration locators stay, the arrays swap for counts.
        let mut out: Vec<u8> = Vec::new();
        assert_eq!(
            explain_run(&explain_args(&root, Verbosity::Minimal, "Dog"), &mut out),
            exit::CLEAN
        );
        let json: serde_json::Value =
            serde_json::from_str(&String::from_utf8(out).expect("UTF-8")).expect("one object");
        assert_eq!(json["declaration"]["name"], "Dog", "the red line stays");
        assert!(json.get("references").is_none(), "arrays drop");
        assert_eq!(json["counts"]["references"], 2);
        assert_eq!(json["counts"]["affected_tests"], 1);

        // Unknown symbol: the empty seam report, printed as-is at exit 0.
        let mut out: Vec<u8> = Vec::new();
        assert_eq!(
            explain_run(&explain_args(&root, Verbosity::Compact, "Nope"), &mut out),
            exit::CLEAN
        );
        let json: serde_json::Value =
            serde_json::from_str(&String::from_utf8(out).expect("UTF-8")).expect("one object");
        assert_eq!(json["symbol"], "Nope");
        assert!(json.get("declaration").is_none());
        assert_eq!(json["references"], serde_json::json!([]));

        // Usage tier: a missing directory.
        let missing = explain_args(&root.join("nope"), Verbosity::Compact, "Dog");
        assert_eq!(explain_run(&missing, &mut Vec::new()), exit::USAGE);

        fs::remove_dir_all(&root).ok();
    }

    // -----------------------------------------------------------------------
    // dartq index
    // -----------------------------------------------------------------------

    /// The index command face: dir defaults to the cwd, `--full` accepted.
    #[test]
    fn index_args_parse() {
        let parsed = parse_args_of(&["index"])
            .expect("parse")
            .expect("subcommand");
        let Command::Index(args) = parsed else {
            panic!("expected index command")
        };
        assert!(!args.full, "no rebuild flag by default");
        assert_eq!(args.dir, PathBuf::from("."), "default target cwd");
        assert!(args.sdk.is_none());

        let parsed = parse_args_of(&["index", "--full", "--sdk", "/sdk", "/pkg"])
            .expect("parse")
            .expect("subcommand");
        let Command::Index(args) = parsed else {
            panic!("expected index command")
        };
        assert!(args.full);
        assert_eq!(args.dir, PathBuf::from("/pkg"));
        assert_eq!(args.sdk, Some(PathBuf::from("/sdk")));
    }

    /// The daemon's `--watch` flag (PRD §16.7 mode 3): parseable, off by
    /// default.
    #[test]
    fn daemon_watch_flag_parse() {
        let parsed = parse_args_of(&["daemon", "/pkg"])
            .expect("parse")
            .expect("subcommand");
        let Command::Daemon(args) = parsed else {
            panic!("expected daemon command")
        };
        assert!(!args.watch, "watch defaults off");

        let parsed = parse_args_of(&["daemon", "--watch", "/pkg"])
            .expect("parse")
            .expect("subcommand");
        let Command::Daemon(args) = parsed else {
            panic!("expected daemon command")
        };
        assert!(args.watch, "--watch parses");
    }
}

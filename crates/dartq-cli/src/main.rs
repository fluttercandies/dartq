//! dartq CLI — `dartq check` (the M0 differential face), `dartq analyze`
//! (the M1 engine face), the `dartq query` index faces (`symbol` /
//! `references` / `callers`, plus the `batch`
//! multi-symbol/multi-kind face), `dartq explain` (the one-stop AI
//! report), `dartq index` (the index build), `dartq rename` (the symbol
//! rename face), and `dartq version`.
//!
//! The per-command cores live in `dartq-commands` (the one implementation
//! the daemon RPC faces share); this binary adds only argument
//! parsing, output formatting, and the exit tiers.
//!
//! `check` drives the M0 pipeline — bytes → scan →
//! `@dart` override rescan → parse, with all diagnostics written through
//! `dartq-diagnostics`' oracle-exact writers. It stops there by design: no
//! pub/package/SDK/engine involvement — the M0 differential face covers
//! scan + parse and nothing downstream.
//!
//! `analyze` drives the full engine over a real package directory:
//! pub faces → [`dartq_commands::workspace::open_engine`] → engine
//! discovery → analysis, printed through the same oracle-exact writers.
//!
//! `query symbol` drives the same workspace assembly and answers from the
//! symbol index over the analyzed state: the queried name's
//! declaration rows plus resolved reference rows. A new output face, not a
//! diagnostics face — rows sort by file, then position.
//!
//! Published output contract:
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
//! dartq emits three JSON dialects over one shared row vocabulary — the
//! frozen v1 shapes `dartq-schema` freezes:
//!
//! | Dialect | Emitted by | Envelope / version contract | The same concept elsewhere |
//! | --- | --- | --- | --- |
//! | CLI oracle JSON | the `--format=json` faces (one line on stdout) | the oracle envelope (`version` key), byte parity with `dart analyze --format=json` on both faces; advisory notes ride stderr | the daemon serves its own rows under `dartq/*` — the RPC dialect below, not this one |
//! | Daemon RPC answers | `dartq/*` method results (dartq-daemon) | JSON-RPC 2.0 result carrying `schema_version`, an optional `verbosity` tier, and a `since` delta cursor; compact rows plus session economics (`symbols` table) | the CLI machine JSON's rows; validated against the schema crate's frozen documents |
//! | Schema-frozen shapes | never emitted — the contract itself (dartq-schema's per-domain validators, the CI gate) | one frozen v1 document per domain (Query, Explain, …) plus the Verbosity tier table | what both emitters above must keep passing |
//!
//! The daemon face is the one deviation from the crash tier: it installs no
//! catch-unwind, so a panic on the main thread unwinds out of `main` and
//! the process exits with the Rust runtime's default 101 instead of 4. The
//! crash tier exists to keep a partial diagnostic set from masquerading as
//! a harness-valid {0,1,2,3} result; the daemon serves an RPC stream, not
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

use dartq_commands::analyze::Target;
use dartq_commands::check::{FileSet, write_machine_verbosity};
use dartq_commands::config::{
    ConfigError, ContextConfig, ContextTarget, load as load_dartq_config, resolve_locator,
};
use dartq_commands::discover::{DiscoverError, discover};
use dartq_commands::explain::ExplainError;
use dartq_commands::query::{
    BatchQuery, QueryKind, decl_kind_name, declaration_extras, ref_kind_name,
};
use dartq_commands::workspace::{
    AnalyzeOverrides, WorkspaceAdvisory, engine_config_with_advisory, engine_config_with_overrides,
    open_engine,
};
use dartq_diagnostics::{
    AnalysisOutput, Diagnostic, FileLayoutProvider, Severity, SourceRange, write_json,
};
use dartq_engine::{DeclHit, EngineConfig};
use dartq_sdk::SdkSnapshot;

/// Process exit codes, pinned to the dartdev oracle (analyze.dart:719–731;
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
mod rename;
mod resolve;
mod test;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> ExitCode {
    // Lossy argv decode: `env::args` panics (exit 101) on a non-UTF-8
    // argument, ahead of any parse; the lossy spelling keeps the process in
    // control — an invalid path fails downstream resolution as the ordinary
    // usage error (exit 64), never a panic.
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
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
            QueryFace::Implementations(symbol_args) => query_implementations(&symbol_args),
            QueryFace::Dependencies(symbol_args) => query_dependencies(&symbol_args),
            QueryFace::Batch(batch_args) => query_batch(&batch_args),
        },
        Ok(Cli {
            command: Some(Command::Impact(impact_args)),
        }) => impact::run_entry(&impact_args),
        Ok(Cli {
            command: Some(Command::Compatibility(compat_args)),
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
            command: Some(Command::Rename(rename_args)),
        }) => rename::run_entry(&rename_args),
        Ok(Cli {
            command: Some(Command::Test(test_args)),
        }) => test::run_entry(&test_args),
        Ok(Cli {
            command: Some(Command::ResolveAt(resolve_args)),
        }) => resolve::resolve_at_entry(&resolve_args),
        Ok(Cli {
            command: Some(Command::Completion(completion_args)),
        }) => resolve::completion_entry(&completion_args),
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
    /// Analyze a package directory through the full engine: scan, parse,
    /// resolution, and the semantic/flow/lint diagnostic tiers.
    Analyze(AnalyzeArgs),
    /// Scan + parse only (the M0 differential face).
    Check(CheckArgs),
    /// Assess the workspace under several SDK roots — a per-axis ✓/✗
    /// compatibility matrix. `compat` stays as a hidden alias.
    #[command(alias = "compat")]
    Compatibility(compat::CompatArgs),
    /// Serve the RPC daemon over stdin/stdout (one client; overlay edits +
    /// full diagnostic pushes).
    Daemon(DaemonArgs),
    /// Assemble the one-stop AI report for one symbol over the
    /// analyzed workspace.
    Explain(ExplainArgs),
    /// Emit or apply anchored fix patches; `--dry-run` only
    /// outputs them.
    Fix(fix::FixArgs),
    /// Query the plugin hosts' assists for one file location;
    /// report-only.
    Assists(fix::AssistsArgs),
    /// Build the symbol index over the analyzed workspace and report what
    /// it built.
    Index(index::IndexArgs),
    /// Assess what a change to one file would affect.
    Impact(impact::ImpactArgs),
    /// Query the analyzed workspace's symbol index.
    Query(QueryArgs),
    /// Rename one symbol across the analyzed workspace: the dry run
    /// prints the machine-readable edit set; `--write` applies it in
    /// place, refusing any file outside the workspace.
    Rename(rename::RenameArgs),
    /// `dart test` (or, with `--flutter`, `flutter test`) with the same
    /// arguments and output, plus a result cache shared across worktrees,
    /// parallel execution under a host-wide memory lease, and
    /// `--impacted[=REF]` selection.
    Test(test::TestArgs),
    /// Resolve the symbol under one `PATH:LINE:COL` cursor over the
    /// analyzed workspace — the daemon's `dartq/resolveAt` answer,
    /// one-shot.
    ResolveAt(resolve::PositionArgs),
    /// Answer the visible-name candidates at one `PATH:LINE:COL` cursor
    /// over the analyzed workspace — the daemon's `dartq/completion`
    /// answer, one-shot.
    Completion(resolve::PositionArgs),
    /// Print the version.
    Version,
}

/// Scan + parse only (the M0 differential face).
///
/// Analysis-options boundary: this face consumes no `analysis_options.yaml` —
/// no `exclude:` on the walk, no `errors:` remaps, no lint rules. Options
/// are the engine/analyze face's concern (`dartq-engine`'s
/// `AnalysisOptions`, whose loader needs the package universe this face
/// deliberately lacks); differential corpora driving this face must be
/// options-free.
#[derive(Args)]
struct CheckArgs {
    #[arg(long, value_enum, default_value_t = CheckFormat::Machine)]
    format: CheckFormat,
    /// Machine-row volume (`-v`/`--verbosity concise|default|verbose`; the
    /// token-cost sweep's tier axis): `concise` keeps the identifying
    /// severity/type/code + span prefix but drops the message field,
    /// `default` is the byte-parity oracle row, `verbose` appends the
    /// correctionMessage as a ninth field (`-` when absent). The diagnostic
    /// set is complete at every tier; the JSON face is unaffected.
    #[arg(
        short = 'v',
        long,
        value_enum,
        value_name = "TIER",
        default_value_t = DiagnosticVerbosity::Default
    )]
    verbosity: DiagnosticVerbosity,
    /// Write one metrics JSON object to PATH at exit.
    #[arg(long)]
    metrics_json: Option<PathBuf>,
    /// The `--fatal-warnings` positive spelling (the oracle declares
    /// `--[no-]fatal-warnings` as one flag, so both spellings parse and the
    /// last spelling wins) — see `no_fatal_warnings`.
    #[arg(long, hide = true, overrides_with = "no_fatal_warnings")]
    fatal_warnings: bool,
    /// Fatal warnings are on by default; `--no-fatal-warnings` clears it and
    /// `--fatal-warnings` restores it — with both spellings in one
    /// invocation the last one wins (the oracle's package:args flag
    /// semantics).
    #[arg(long, overrides_with = "fatal_warnings")]
    no_fatal_warnings: bool,
    /// Off by default (`--fatal-infos` sets it) — mirrors the oracle.
    #[arg(long)]
    fatal_infos: bool,
    /// The dartdev global flags, accepted and ignored — see
    /// [`DartdevGlobals`].
    #[command(flatten)]
    dartdev_globals: DartdevGlobals,
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
#[expect(clippy::struct_excessive_bools)] // the bools are independent clap flags; an enum decomposition would obscure the mapping
struct AnalyzeArgs {
    /// Analysis targets; each names a directory or a file, mixing freely
    /// (the oracle's `rest` params). Empty resolves to the current
    /// directory (the oracle's `Directory.current` fallback).
    #[arg(default_value = ".")]
    targets: Vec<PathBuf>,
    #[arg(long, value_enum, default_value_t = AnalyzeFormat::Default)]
    format: AnalyzeFormat,
    /// Machine-row volume (`-v`/`--verbosity concise|default|verbose`; the
    /// token-cost sweep's tier axis): `concise` keeps the identifying
    /// severity/type/code + span prefix but drops the message field,
    /// `default` is the byte-parity oracle row, `verbose` appends the
    /// correctionMessage as a ninth field (`-` when absent). The diagnostic
    /// set is complete at every tier; the default-format and JSON faces are
    /// unaffected.
    #[arg(
        short = 'v',
        long,
        value_enum,
        value_name = "TIER",
        default_value_t = DiagnosticVerbosity::Default
    )]
    verbosity: DiagnosticVerbosity,
    /// Write one metrics JSON object to PATH at exit.
    #[arg(long)]
    metrics_json: Option<PathBuf>,
    /// Analyze PATH with the content of FILE instead of the on-disk bytes:
    /// repeatable, relative PATH resolves against DIR, and
    /// FILE `-` reads stdin. The engine-view faces only — `check` has no
    /// engine to overlay.
    #[arg(long = "overlay", value_name = "PATH=FILE")]
    overlays: Vec<String>,
    /// Dart SDK root; defaults to `$DART_SDK`. The oracle's spelling
    /// `--sdk-path` is the documented one; `--sdk` stays as a hidden compat
    /// alias.
    #[arg(long = "sdk-path", alias = "sdk", value_name = "PATH")]
    sdk_path: Option<PathBuf>,
    /// Package resolution configuration file (the dartdev `--packages`):
    /// analyzed in place of the resolved workspace's
    /// `.dart_tool/package_config.json`, its
    /// directory the `rootUri` resolution base. A missing file is the
    /// usage error the oracle ships.
    #[arg(long, value_name = "PATH")]
    packages: Option<PathBuf>,
    /// Enable one or more language experiments (the dartdev
    /// `--enable-experiment`): repeatable, comma-separated names accepted;
    /// a `no-` prefix is stripped before the known-feature test. Unknown
    /// names are the usage error `Unknown experiment(s): 'x'`. The flags
    /// are the context-level feature base; a package's analysis options
    /// declaring its own `enable-experiment` section replaces them for
    /// that subtree (the official assignment posture).
    #[arg(
        long = "enable-experiment",
        value_name = "NAME[,NAME...]",
        value_delimiter = ','
    )]
    enable_experiments: Vec<String>,
    /// Load analysis plugins (the dartdev `--[no-]plugins`, default on).
    /// When the effective `analysis_options.yaml` declares a `plugins:`
    /// section and this is not disabled, the analyze cycle spawns the
    /// declared plugins, feeds the engine diagnostics, and merges their
    /// findings (plus `dartq/plugin_crashed` crash rows) into the output;
    /// `--no-plugins` is the opt-out. Both spellings parse.
    #[arg(long)]
    plugins: bool,
    /// The `--no-plugins` spelling — see `plugins`.
    #[arg(long = "no-plugins", hide = true)]
    no_plugins: bool,
    /// The `--fatal-warnings` positive spelling (the oracle declares
    /// `--[no-]fatal-warnings` as one flag, so both spellings parse and the
    /// last spelling wins) — see `no_fatal_warnings`.
    #[arg(long, hide = true, overrides_with = "no_fatal_warnings")]
    fatal_warnings: bool,
    /// Fatal warnings are on by default; `--no-fatal-warnings` clears it and
    /// `--fatal-warnings` restores it — with both spellings in one
    /// invocation the last one wins (the oracle's package:args flag
    /// semantics).
    #[arg(long, overrides_with = "fatal_warnings")]
    no_fatal_warnings: bool,
    /// Off by default (`--fatal-infos` sets it) — mirrors the oracle.
    #[arg(long)]
    fatal_infos: bool,
    /// Memory budget in MiB: RSS is sampled at phase and
    /// library boundaries; over budget the engine degrades tier-by-tier
    /// and reports each step as an INFO `dartq/memory_degraded` row
    /// (diagnostics stay complete at every tier). Unset = unlimited —
    /// the output is byte-identical to an unbudgeted run.
    #[arg(long, value_name = "MiB")]
    memory_limit: Option<u64>,
    /// JSONL trace sink: the engine appends one JSON object per
    /// engine event — stable event names, stable field order, flushed per
    /// event so a killed CI run keeps its tail. Unset = no sink and zero
    /// event work: the output face is byte-identical to an untraced run.
    #[arg(long, value_name = "PATH")]
    trace_events: Option<PathBuf>,
    /// Override the analysis cache location (the oracle's hidden
    /// `--cache PATH`). dartq manages its own content store under
    /// `~/.cache/dartq/` (see `--cache-size-limit`), so the flag is accepted
    /// for CLI parity and ignored.
    #[arg(long, value_name = "PATH", hide = true)]
    cache: Option<PathBuf>,
    /// The oracle's hidden `--[no-]use-aot-snapshot` flag (defaults on
    /// upstream; picks the analysis-server snapshot flavor). dartq runs
    /// in-process — no snapshot exists — so both spellings parse and do
    /// nothing.
    #[arg(long = "use-aot-snapshot", hide = true)]
    use_aot_snapshot: bool,
    /// The `--no-use-aot-snapshot` spelling — see `use_aot_snapshot`.
    #[arg(long = "no-use-aot-snapshot", hide = true)]
    no_use_aot_snapshot: bool,
    /// The oracle's hidden `--memory` flag: with `--format=json`, the
    /// emitted envelope carries a `memory` key — the process's peak RSS in
    /// KiB (`usageInfo.memoryKB`, analyze.dart:587). Ignored in every other
    /// format (the oracle's `printMemory` gate).
    #[arg(long, hide = true)]
    memory: bool,
    /// Hard cap for the L3 content store, the persistent
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
    /// The dartdev global flags, accepted and ignored — see
    /// [`DartdevGlobals`].
    #[command(flatten)]
    dartdev_globals: DartdevGlobals,
}

#[derive(Clone, Copy, ValueEnum)]
enum AnalyzeFormat {
    /// SEVERITY|TYPE|CODE|FILE|LINE|COL|LENGTH|MESSAGE per diagnostic
    /// (byte-parity differential face).
    Machine,
    /// The oracle's default format: human-readable, grouped by file,
    /// severity-sorted (the unset `--format` face).
    Default,
    /// Single-line JSON envelope, byte parity with
    /// `dart analyze --format=json` (the shared oracle-exact writer).
    /// Diagnostics only: workspace advisories print as one line on stderr
    /// ahead of it.
    Json,
}

/// The dartdev global flags, accepted and ignored on the diagnostic faces
/// (ticket #2385, the cli-compat Flag-face P2). Upstream declares them on
/// the runner's global parser (`upstream Dart SDK source,
/// pkg/dartdev/lib/src/utils.dart`, `globalDartdevOptionsParser`), and the
/// global parse is merged past the command name, so they are accepted
/// positioned *after* the subcommand — `dart analyze --suppress-analytics`
/// is the standard CI incantation. dartq runs no analytics/telemetry layer
/// and no verbose logger, so the flags change nothing. Hidden, matching
/// the oracle's `analyze --help` face where the globals don't appear.
/// Known upstream corner, not replicated: `--suppress-analytics` combined
/// with `--enable-analytics`/`--disable-analytics` is an analytics-layer
/// usage error (exit 254) upstream; dartq has no analytics layer to
/// conflict with. The `-v` abbreviation upstream is not taken here — it
/// stays dartq's `--verbosity` short (the `-v`/`--verbose` collision is a
/// deferred compat-policy decision).
#[derive(Args, Default)]
#[expect(clippy::struct_excessive_bools)] // the bools are independent clap flags; an enum decomposition would obscure the mapping
struct DartdevGlobals {
    /// Upstream's global verbose logger (declared with the `-v`
    /// abbreviation there; only the long spelling is accepted here).
    #[arg(long, hide = true)]
    verbose: bool,
    /// Declared by the global parser but never read past command dispatch
    /// upstream — no version print, no exit (dartq's version face is the
    /// root-level `--version` / `version`, which this does not touch).
    #[arg(long, hide = true)]
    version: bool,
    /// Upstream turns analytics on for this run.
    #[arg(long, hide = true)]
    enable_analytics: bool,
    /// Upstream turns analytics off for this run.
    #[arg(long, hide = true)]
    disable_analytics: bool,
    /// Upstream's tool diagnostic output switch.
    #[arg(long, hide = true)]
    diagnostics: bool,
    /// Disallow analytics for this run without changing the analytics
    /// configuration.
    #[arg(long, hide = true)]
    suppress_analytics: bool,
    /// Upstream opts this run out of telemetry (hidden, positive-only —
    /// the global parser declares it non-negatable). dartq sends no
    /// telemetry; the flag parses and is a no-op.
    #[arg(long, hide = true)]
    disable_telemetry: bool,
    /// Upstream's deprecated analytics switch (defaults on, negatable —
    /// `--[no-]analytics`; both spellings parse and the last wins —
    /// deprecated upstream in favor of `--suppress-analytics`). dartq
    /// sends no analytics; the flag parses and is a no-op.
    #[arg(long, hide = true, overrides_with = "no_analytics")]
    analytics: bool,
    /// The `--[no-]analytics` negative spelling — see `analytics`.
    #[arg(long, hide = true, overrides_with = "analytics")]
    no_analytics: bool,
}

/// The machine rows' output volume (the token-cost sweep's tier axis); the
/// canonical semantics live in dartq-commands' `check` module, the one
/// renderer both diagnostic faces share.
#[derive(Clone, Copy, ValueEnum)]
enum DiagnosticVerbosity {
    /// Code + span rows: the message field drops.
    Concise,
    /// The oracle machine row verbatim (the default).
    Default,
    /// The machine row plus the correctionMessage field (`-` when absent).
    Verbose,
}

impl DiagnosticVerbosity {
    fn into_commands(self) -> dartq_commands::check::DiagnosticVerbosity {
        match self {
            Self::Concise => dartq_commands::check::DiagnosticVerbosity::Concise,
            Self::Default => dartq_commands::check::DiagnosticVerbosity::Default,
            Self::Verbose => dartq_commands::check::DiagnosticVerbosity::Verbose,
        }
    }
}

#[derive(Args)]
struct DaemonArgs {
    /// Package directory holding `pubspec.yaml` and
    /// `.dart_tool/package_config.json`.
    dir: PathBuf,
    /// Dart SDK root; defaults to `$DART_SDK`.
    #[arg(long)]
    sdk: Option<PathBuf>,
    /// A dartq.yaml context label to serve alongside the default context;
    /// repeatable. Empty serves every declared context; unknown labels are
    /// usage errors (the compat face's `--context` convention).
    #[arg(long = "context")]
    contexts: Vec<String>,
    /// Watch the package directory and re-analyze after changes settle.
    /// Poll-based — a bounded-interval walk with burst
    /// debouncing — a convenience layer: explicit push and content
    /// verification stay the truth.
    #[arg(long)]
    watch: bool,
    /// Memory budget in MiB: every session context arms the
    /// same sampled-RSS ladder the analyze face arms — over budget each
    /// engine degrades tier-by-tier and reports each step as an INFO
    /// `dartq/memory_degraded` row. Default: 2048, the daemon's standing
    /// per-context budget.
    #[arg(long, value_name = "MiB")]
    memory_limit: Option<u64>,
    /// Hard cap for the L3 content store, armed on every
    /// session context when set. Grammar: NUMBER [KiB|MiB|GiB] — suffix
    /// case-insensitive, a bare number is bytes (512MiB, 10GiB, 4096).
    /// Unset = the store stays unarmed (the daemon's standing posture;
    /// the analyze face arms it unconditionally at the 10 GiB default).
    #[arg(
        long,
        value_name = "SIZE",
        value_parser = dartq_engine::store::parse_size_limit
    )]
    cache_size_limit: Option<u64>,
    /// JSONL trace sink: the default context's engine appends
    /// one JSON object per engine event — stable event names, stable
    /// field order, flushed per event so a killed CI run keeps its tail.
    /// One sink is one writer, so declared `--context`s run untraced.
    /// Unset = no sink and zero event work.
    #[arg(long, value_name = "PATH")]
    trace_events: Option<PathBuf>,
}

#[derive(Args)]
struct ExplainArgs {
    /// The symbol name to explain — the symbol face's selector. On the
    /// `--diagnostic` face the one positional slot is the workspace `dir`
    /// (the selector syntax `explain --diagnostic CODE [dir]`): a
    /// positional is a dir, never a symbol, when `--diagnostic` is set.
    symbol: Option<String>,
    /// The diagnostic code to explain — selects the diagnostic face. The
    /// run's first hit of the code answers when `--file` is absent.
    #[arg(long, value_name = "CODE")]
    diagnostic: Option<String>,
    /// The file[:line[:col]] the diagnostic hit sits on — narrows the
    /// diagnostic face's selection.
    #[arg(long, value_name = "PATH[:LINE[:COL]]", requires = "diagnostic")]
    file: Option<String>,
    /// Package directory holding pubspec.yaml and
    /// `.dart_tool/package_config.json`; defaults to the current directory.
    #[arg(default_value = ".")]
    dir: PathBuf,
    #[arg(long, value_enum, default_value_t = ExplainFormat::Json)]
    format: ExplainFormat,
    /// Dart SDK root; defaults to `$DART_SDK`.
    #[arg(long)]
    sdk: Option<PathBuf>,
    /// Report detail tier (the schema's verbosity table; `compact` by
    /// default, `minimal` swaps the section arrays for `counts`).
    #[arg(short = 'v', long, value_enum, default_value_t = Verbosity::Compact)]
    verbosity: Verbosity,
}

#[derive(Clone, Copy, ValueEnum)]
enum ExplainFormat {
    /// Single-line JSON report — the report's only format (the
    /// report is schema-valid at birth; this flag spells it).
    Json,
}

/// The output detail tier (dartq-commands [`Verbosity`]'s spelling for the
/// CLI; the canonical per-face tables live in dartq-schema's crate docs).
#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum Verbosity {
    /// Red-line fields only: locating fields stay; prose and convenience
    /// spellings drop (the per-face tables pin exactly what remains).
    Minimal,
    /// Today's output shape (the default).
    Compact,
    /// Compact (the canonical tables list no full-only extras today).
    Full,
}

impl Verbosity {
    pub(crate) fn into_commands(self) -> dartq_commands::Verbosity {
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
    /// symbol face serves today; the later faces carry their own row
    /// families).
    References(SymbolArgs),
    /// Invocation-shaped use sites of one symbol name (the references rows
    /// with kind `invoked`).
    Callers(SymbolArgs),
    /// The queried type's subtypes / the queried member's overriding rows,
    /// on the declaration-row shape — the resolved supertype
    /// edges the index maintains, walked in reverse.
    Implementations(SymbolArgs),
    /// The declaring library's own dependencies: its import edges and
    /// the dependency-library symbols its resolved uses consume —
    /// `path|name` rows, the forward direction (the reverse question —
    /// who consumes the declaring library — is the impact face's answer).
    Dependencies(SymbolArgs),
    /// Every repeated `--symbol` × repeated `--kind` answer in one pass
    /// over one workspace assembly: one graph build, results
    /// in request order, one JSON envelope per line.
    Batch(BatchArgs),
}

/// The batch face's flags: the item list is the daemon `dartq/queryBatch`
/// request's own shape — one item per `--symbol`, each carrying the full
/// `--kind` set (absent kinds are the symbol face).
#[derive(Args)]
struct BatchArgs {
    /// The symbol names to look up; repeatable, order preserved.
    #[arg(long = "symbol", value_name = "NAME", required = true)]
    symbols: Vec<String>,
    /// Which query answers every symbol gets; repeatable, default `symbol`.
    #[arg(long, value_name = "KIND")]
    kinds: Vec<QueryKind>,
    /// Report detail tier (the schema's verbosity table; `compact` by
    /// default) — the same tier axis the single faces carry, applied to
    /// every JSONL answer.
    #[arg(short = 'v', long, value_enum, default_value_t = Verbosity::Compact)]
    verbosity: Verbosity,
    /// Package directory holding pubspec.yaml and
    /// `.dart_tool/package_config.json`; defaults to the current directory.
    #[arg(default_value = ".")]
    dir: PathBuf,
    /// Dart SDK root; defaults to `$DART_SDK`.
    #[arg(long)]
    sdk: Option<PathBuf>,
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
    /// Report detail tier (the schema's verbosity table; `compact` by
    /// default). `minimal` keeps the red-line locators: the JSON rows drop
    /// the convenience spellings and the element-face columns, the machine
    /// rows drop the container/`explicit` columns, and the reserved kind
    /// arrays (IMPL/DEP) never print.
    #[arg(short = 'v', long, value_enum, default_value_t = Verbosity::Compact)]
    verbosity: Verbosity,
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

/// Severity arithmetic: the highest fatal tier present.
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

    // Analysis runs under the crash tier (internal failure exits 4, the
    // oracle `_Result.crash(4)` analog): a panic anywhere in
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
        CheckFormat::Machine => {
            write_machine_verbosity(&mut out, &run.output(), args.verbosity.into_commands())
        }
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

/// Numbers one CLI run collected for `--metrics-json` (the struct belongs
/// to the CLI). `analyze` fills discovery/library numbers
/// from `DiscoveryStats` and `LibraryAnalyzed` events; `check` reports its
/// own walk/parse phases and zero library counters — the JSON shape is
/// identical because both faces' flag grammar pins `--metrics-json`.
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

/// `dartq analyze` — crash tier like [`check`]: a
/// panic anywhere aborts with exit 4 and stdout kept empty, because
/// [`analyze_run`] writes only after analysis fully succeeded.
fn analyze(args: &AnalyzeArgs) -> u8 {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        analyze_run(args, &mut BufWriter::new(io::stdout().lock()))
    }));
    if let Ok(code) = outcome {
        code
    } else {
        eprintln!("dartq: internal error: analysis panicked");
        exit::INTERNAL_ERROR
    }
}

/// One analyze run over the positional targets: each resolves to a
/// directory or file (the oracle's `rest` classification, analyze.dart:
/// 180-194), targets group onto their containing workspaces, one engine
/// runs per distinct workspace root, and the merged diagnostic set renders
/// through the oracle-format writers. The workspace assembly and the
/// engine pass — plain or `--overlay` — live in `dartq-commands`; this
/// shell adds the crash tier, the writers, and the exit tier.
fn analyze_run(args: &AnalyzeArgs, out: &mut impl Write) -> u8 {
    // Resolve every target before any analysis (the oracle's order: the
    // whole rest list is classified upfront, analyze.dart:182-193). A
    // missing target is the usage tier with the check face's wording.
    let targets = match resolve_targets(&args.targets) {
        Ok(targets) => targets,
        Err(path) => {
            eprintln!("error: Directory or file doesn't exist: {}", path.display());
            return exit::USAGE;
        }
    };
    analyze_targets(args, &targets, out)
}

/// One rest argument's on-disk kind — a directory target walks its tree,
/// a file target analyzes alone within its containing workspace. The path
/// canonicalizes so the emitted-path filter lands in the engine's interned
/// spelling universe (the overlay face's canonicalization convention,
/// [`load_overlays`]).
#[derive(Debug)]
enum TargetKind {
    Dir(PathBuf),
    File(PathBuf),
}

/// The target classification pass ([`analyze_run`] step one): metadata per
/// argument, `fs::metadata` following links — the discover walk's own
/// classification ([`dartq_commands::discover`]: dir → walked, file → taken
/// as-is, missing → the usage error). `Err` carries the first missing
/// argument, in argv order.
fn resolve_targets(paths: &[PathBuf]) -> Result<Vec<TargetKind>, PathBuf> {
    paths
        .iter()
        .map(|path| {
            let meta = fs::metadata(path).map_err(|_| path.clone())?;
            // The engine spells every path in canonical form; a canonical
            // target makes the coverage filter a plain prefix/equality
            // test and groups targets on their resolved workspace root.
            let canonical = path.canonicalize().map_err(|_| path.clone())?;
            Ok(if meta.is_file() {
                TargetKind::File(canonical)
            } else {
                TargetKind::Dir(canonical)
            })
        })
        .collect()
}

/// The workspace root `assemble` will compute for a target's package
/// directory — the grouping key that dedupes targets sharing one workspace
/// before any engine is built (a `lib/` + `test/` pair opens one engine).
/// Mirrors `package_assembly_with`'s root arms: pub's root walk, the
/// no-pubspec bare floor at the directory itself, the unparseable-pubspec
/// floor at the nearest pubspec dir. Other findings keep the input dir —
/// `open_engine` reproduces the refusal as the usage-tier error.
fn workspace_root_of(dir: &Path) -> PathBuf {
    match dartq_pub::find_workspace_root(dir) {
        Ok(workspace) => workspace.root,
        Err(dartq_pub::PubError::NoRoot(_)) => dir.to_path_buf(),
        Err(dartq_pub::PubError::Pubspec(_) | dartq_pub::PubError::OrphanMember(_)) => {
            // The nearest-pubspec floor: the first ancestor carrying a
            // `pubspec.yaml` is the root `assemble` picks for this arm.
            let mut dir = dir;
            loop {
                if dir.join("pubspec.yaml").is_file() {
                    return dir.to_path_buf();
                }
                match dir.parent() {
                    Some(parent) => dir = parent,
                    None => return dir.to_path_buf(),
                }
            }
        }
        Err(_) => dir.to_path_buf(),
    }
}

/// The target-driven run: one engine per distinct workspace root, each
/// serving the targets it covers; every run filters its emitted set to the
/// covered files (the oracle pushes diagnostics only inside the targets'
/// context roots) and the merged union renders through the shared writers.
fn analyze_targets(args: &AnalyzeArgs, targets: &[TargetKind], out: &mut impl Write) -> u8 {
    let started = Instant::now();

    // Group targets by resolved workspace root. A file target's package
    // directory is its parent (the oracle's file context root still finds
    // the containing workspace for resolution); a directory target's is
    // the directory itself.
    let mut groups: Vec<(PathBuf, Vec<Target>)> = Vec::new();
    for target in targets {
        let (assembly_dir, mapped) = match target {
            TargetKind::Dir(dir) => (dir.clone(), Target::Dir(dir.clone())),
            TargetKind::File(file) => (
                file.parent().map(Path::to_path_buf).unwrap_or_default(),
                Target::File(file.clone()),
            ),
        };
        let root = workspace_root_of(&assembly_dir);
        match groups.iter_mut().find(|(key, _)| *key == root) {
            Some((_, members)) => members.push(mapped),
            None => groups.push((root, vec![mapped])),
        }
    }

    // `--overlay` is a dartq extension the oracle never ships: relative
    // specs resolve against the single analyzed workspace root. Multiple
    // distinct roots leave the resolution base ambiguous — a usage refusal
    // rather than a guess.
    if !args.overlays.is_empty() && groups.len() > 1 {
        eprintln!("error: --overlay requires a single workspace target");
        return exit::USAGE;
    }

    // `--[no-]plugins` gates the whole weave: upstream `dart analyze`
    // loads plugins by default, `--no-plugins` is the opt-out. The flag
    // decides before any discovery probe — a `no-plugins` run is the exact
    // pre-plugin shape (no options read, no convention scan).
    let plugins_enabled = !args.no_plugins;
    let mut engines = Vec::with_capacity(groups.len());
    let mut members: Vec<Vec<Target>> = Vec::new();
    let mut roots: Vec<PathBuf> = Vec::new();
    let mut advisories: Vec<WorkspaceAdvisory> = Vec::new();
    let mut plugin_sets: Vec<dartq_commands::plugins::PluginSet> = Vec::with_capacity(groups.len());
    for (root, group) in groups {
        // The assembled config — not the opened engine — because the plugin
        // set launches off `pub_config`/`config_dir` before `Engine::new`
        // consumes the config.
        let (config, dir, stale_advisory) = match engine_config_with_overrides(
            &root,
            args.sdk_path.as_deref(),
            &analyze_overrides(args),
        ) {
            Ok(faces) => faces,
            Err(error) => {
                eprintln!("error: {error}");
                return exit::USAGE;
            }
        };
        // The launch is lazy (no sidecar starts): a plugin-free workspace
        // pays the two absence probes and nothing more.
        let plugin_set = if plugins_enabled {
            dartq_commands::plugins::PluginSet::launch(
                &root,
                &config.pub_config,
                &config.config_dir,
            )
        } else {
            dartq_commands::plugins::PluginSet::default()
        };
        let engine = match dartq_engine::Engine::new(config) {
            Ok(engine) => engine,
            Err(error) => {
                eprintln!("error: engine context unusable: {error}");
                return exit::USAGE;
            }
        };
        if let Some(advisory) = stale_advisory {
            advisories.push(advisory);
        }
        engines.push(engine);
        members.push(group);
        plugin_sets.push(plugin_set);
        // The lint stage's per-subtree root: the assembly's canonical
        // workspace dir (== the grouping `root` by construction — the
        // grouping key is the same `find_workspace_root` result).
        roots.push(dir);
    }

    // One engine per distinct workspace root: identical runtime arming on
    // each (memory budget, trace sink — multi-root runs keep every sink on
    // the one `--trace-events` file's first engine, matching the daemon's
    // one-sink posture — and the L3 store cap).
    let mut traces_left = true;
    for engine in &mut engines {
        engine.set_memory_limit(args.memory_limit);
        engine.set_trace_sink(match &args.trace_events {
            Some(path) if traces_left => {
                traces_left = false;
                match dartq_engine::event::TraceSink::create(path) {
                    Ok(sink) => Some(sink),
                    Err(error) => {
                        eprintln!("error: trace events: {error}");
                        return exit::USAGE;
                    }
                }
            }
            _ => None,
        });
        arm_content_store(engine, args.cache_size_limit);
    }

    // Relative overlay specs resolve against the single workspace root —
    // the multi-root refusal above guarantees `roots` holds exactly one
    // entry whenever overlays are present.
    let overlays = match load_overlays(
        &args.overlays,
        roots.first().map_or(Path::new("."), PathBuf::as_path),
    ) {
        Ok(overlays) => overlays,
        Err(error) => {
            eprintln!("error: {error}");
            return exit::USAGE;
        }
    };

    // A run per engine, each filtered to its targets' covered files. The
    // runs borrow their engines, so `engines` must outlive them — declared
    // above, dropped after `analyze_finish`. The overlay set (empty on
    // every multi-root shape by the refusal above) applies to the single
    // workspace's run.
    let mut runs = Vec::with_capacity(engines.len());
    for (i, (engine, targets)) in engines.iter_mut().zip(members.iter()).enumerate() {
        let plugin_set = if plugin_sets[i].is_empty() {
            None
        } else {
            Some(&mut plugin_sets[i])
        };
        let run = match under_crash_tier(|| {
            dartq_commands::analyze::run_targets(
                engine,
                &overlays,
                std::slice::from_ref(&roots[i]),
                targets,
                plugin_set,
            )
        }) {
            Ok(run) => run,
            Err(code) => {
                for set in plugin_sets {
                    set.shutdown();
                }
                return code;
            }
        };
        runs.push(run);
    }
    let merged = dartq_commands::analyze::merge_runs(runs);
    let code = analyze_finish(args, &merged, started, out, &advisories);
    // The graceful teardown: each live plugin handshake-completes before the
    // process exits (a bare drop is kill-only).
    for set in plugin_sets {
        set.shutdown();
    }
    code
}

/// One engine analyze pass under the crash tier — the
/// `check` face's shape: a panic anywhere in the pipeline exits 4 with
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

/// The analyze face's CLI overrides (#938): `--packages` and the
/// `--enable-experiment` list. `--plugins`/`--no-plugins` gate the plugin
/// weave before any launch, so they carry no override state either.
fn analyze_overrides(args: &AnalyzeArgs) -> AnalyzeOverrides {
    AnalyzeOverrides {
        packages: args.packages.clone(),
        experiments: args.enable_experiments.clone(),
    }
}

/// Arms the L3 content store at the default cache root with
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
    if let Err(error) = engine.open_content_store(&config) {
        eprintln!("warning: content store disabled: {error}");
    }
}

/// The shared analyze postlude: the assemblies' workspace advisories (the
/// #732 stale-config row, or its #759 missing-config sibling — one per
/// distinct workspace root) in the face's own
/// stream, then the oracle-format diagnostics, the `--metrics-json` facts
/// at exit, and the severity exit tier. The advisories print inside the
/// postlude (not at assembly time) so the crash tier's contract holds: a
/// panicked run keeps stdout empty, advisories included. They never join
/// the per-file set, the metrics count, or the exit tier.
///
/// The engine's version-skew degrade notices (#1311) and span-capacity
/// rejection notices (#1543) print on stderr ahead of every face —
/// warning-tier telemetry on the human stream, one line per
/// `ExperimentDegrade` event (once per unknown experiment name per run)
/// and one line per rejected file. Stdout is the data channel in every
/// format, so a notice never joins the machine rows, the default-format
/// groups, or the JSON envelopes (both of them; their advisories ride
/// stderr too).
fn analyze_finish(
    args: &AnalyzeArgs,
    run: &dartq_commands::analyze::MergedRun<'_>,
    started: Instant,
    out: &mut impl Write,
    advisories: &[WorkspaceAdvisory],
) -> u8 {
    let discovery_ms = elapsed_ms(run.discovery);
    let analyze_ms = elapsed_ms(run.analysis);
    let verbosity = args.verbosity.into_commands();

    for line in &run.degrade_notices {
        eprintln!("{line}");
    }

    let written = match args.format {
        // The advisories ride stderr on the machine face: the oracle's
        // machine stream carries diagnostics only, so each row prints through
        // the same shared writer onto the human stream while stdout keeps
        // the oracle rows alone.
        AnalyzeFormat::Machine => {
            let mut stderr = io::stderr().lock();
            let mut written = Ok(());
            for advisory in advisories {
                written = written.and_then(|()| {
                    write_stale_advisory_machine(&mut stderr, Some(advisory), verbosity)
                });
            }
            written.and_then(|()| {
                write_machine_verbosity(
                    out,
                    &AnalysisOutput {
                        diagnostics: &run.diagnostics,
                        files: run,
                    },
                    verbosity,
                )
            })
        }
        AnalyzeFormat::Default => {
            let mut written = Ok(());
            for advisory in advisories {
                written = written.and_then(|()| write_stale_advisory_human(out, Some(advisory)));
            }
            written.and_then(|()| write_human(out, &run.diagnostics, run))
        }
        // The envelope is the JSON face's whole stdout contract (the oracle
        // envelope carries diagnostics only), so the workspace advisories have
        // no row in it; each one-line note rides stderr instead — the same
        // stream the degrade notices print on — keeping the
        // degraded-resolution signal on every face. The oracle's `--memory`
        // gate (analyze.dart:200 `printMemory = memory && jsonFormat`)
        // appends the envelope's `memory` key: the process's peak RSS in KiB
        // (`usageInfo.memoryKB`, analyze.dart:587) — spliced before the
        // closing brace so the shared writer's byte-parity envelope is
        // untouched when the flag is absent. Metrics and the exit
        // tier are face-independent below.
        AnalyzeFormat::Json => {
            for advisory in advisories {
                eprintln!("Info • {} • {}", advisory.code().name, advisory.message());
            }
            let output = AnalysisOutput {
                diagnostics: &run.diagnostics,
                files: run,
            };
            if args.memory {
                write_json_memory(out, &output)
            } else {
                write_json(out, &output)
            }
        }
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

/// The `--memory` JSON face (the oracle's `usageInfo.memoryKB` key,
/// analyze.dart:587 — `memory: <peak RSS KiB>` appended after
/// `diagnostics`). The shared writer owns the byte-parity envelope, so the
/// key splices into its exact output: render to the buffer, strip the
/// trailing `}`+newline, append the member, reclose. A run without an RSS
/// reading (a non-unix host, a failed getrusage) omits the key — the
/// oracle's `if (usageInfo != null)` gate.
fn write_json_memory(out: &mut impl Write, output: &AnalysisOutput<'_>) -> io::Result<()> {
    let mut line = Vec::new();
    write_json(&mut line, output)?;
    // The envelope ends `]}\n` by the shared writer's contract.
    debug_assert!(line.ends_with(b"}\n"));
    line.truncate(line.len() - 2);
    if let Some(kb) = max_rss_bytes().map(|bytes| bytes / 1024) {
        write!(line, ",\"memory\":{kb}")?;
    }
    line.extend_from_slice(b"}\n");
    out.write_all(&line)
}

/// The machine-format advisory row, ahead of the diagnostics: the exact
/// 8-field shape every other `dartq/*` row uses (severity INFO — the
/// `dartq/memory_degraded` posture for a note that must never fail the run
/// through the exit tiers; type TOOL per the registry row), anchored at the
/// config the advisory names, rendered through the one oracle-exact writer.
fn write_stale_advisory_machine(
    out: &mut impl Write,
    advisory: Option<&WorkspaceAdvisory>,
    verbosity: dartq_commands::check::DiagnosticVerbosity,
) -> io::Result<()> {
    let Some(advisory) = advisory else {
        return Ok(());
    };
    // An empty source positions the row at 1:1 — the assembly has no
    // source span, the memory-budget row's own posture.
    let mut files = FileSet::default();
    let file = files.push("", advisory.config_path());
    let diagnostic = Diagnostic::new(
        Severity::Info,
        advisory.code(),
        advisory.message(),
        file,
        SourceRange {
            offset: 0,
            length: 0,
        },
        None,
    );
    write_machine_verbosity(
        out,
        &AnalysisOutput {
            diagnostics: &[diagnostic],
            files: &files,
        },
        verbosity,
    )
}

/// The human-format advisory line, ahead of the diagnostics groups: the
/// face's own `severity • code • message` grammar, position-less (the
/// assembly has no source span).
fn write_stale_advisory_human(
    out: &mut impl Write,
    advisory: Option<&WorkspaceAdvisory>,
) -> io::Result<()> {
    let Some(advisory) = advisory else {
        return Ok(());
    };
    writeln!(
        out,
        "Info • {} • {}",
        advisory.code().name,
        advisory.message()
    )
}

/// Decodes `--overlay PATH=FILE` specs: the analyzed path (relative specs
/// resolve against the package directory — the canonical root the assembly
/// returned, the CLI twin of the daemon's workspace-root resolution) plus
/// the local file whose content replaces the on-disk bytes (`-` reads
/// stdin, the AI-agent buffer case).
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
            let joined = if Path::new(path).is_absolute() {
                PathBuf::from(path)
            } else {
                dir.join(path)
            };
            // The engine's discovery interns canonical package-root paths;
            // the overlay must address the same identity (a non-canonical
            // spelling would land on a phantom extra file). A path that
            // does not exist yet (agent-introduced) keeps its spelling —
            // the dir-joined one, never the raw spec: a raw relative
            // spelling would intern a phantom file addressed from the
            // process cwd instead of the package directory.
            let path = joined.canonicalize().unwrap_or(joined);
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

/// The RPC daemon face: assemble the engine context, hand it to the daemon
/// session, and serve stdin/stdout until EOF — the stream's end is the
/// session's only end signal. The workspace sweep (discovery + first
/// analysis) runs inside the session before the first served turn, so the
/// `dartq/diagnostics` pushes leave ahead of anything the client sends.
///
/// When the workspace root's dartq.yaml declares contexts (#1086), the
/// declared contexts are assembled at this shell edge and the session is
/// constructed through [`dartq_daemon::Session::with_runtime`]; the
/// announced `context:` routing contract becomes reachable in production.
/// Without declared contexts the single-context construction is untouched.
/// The engine runtime flags (#1088) — `--memory-limit`, `--cache-size-limit`,
/// `--trace-events` — ride the same construction as the session's
/// [`dartq_daemon::SessionRuntime`] arming.
fn daemon(args: &DaemonArgs) -> u8 {
    let (config, _dir, stale_advisory) =
        match engine_config_with_advisory(&args.dir, args.sdk.as_deref()) {
            Ok(faces) => faces,
            Err(error) => {
                eprintln!("error: {error}");
                return exit::USAGE;
            }
        };
    let extras = match assemble_contexts(&config, &args.contexts) {
        Ok(assembled) => assembled,
        Err(ContextAssemblyError::Usage(message)) => {
            eprintln!("error: {message}");
            return exit::USAGE;
        }
        Err(ContextAssemblyError::Internal(error)) => {
            eprintln!("dartq: internal error: {error}");
            return exit::INTERNAL_ERROR;
        }
    };
    // The engine runtime flags (#1088): the memory budget and the L3 cap
    // replace the standing defaults only when passed; the sink's file is
    // created here — the analyze face's posture (create or usage error) —
    // so a bad `--trace-events` path fails before the session serves.
    let mut runtime = dartq_daemon::SessionRuntime::default();
    if let Some(limit) = args.memory_limit {
        runtime.memory_limit_mib = limit;
    }
    if let Some(limit) = args.cache_size_limit {
        runtime.content_store_limit = Some(limit);
    }
    if let Some(path) = &args.trace_events {
        runtime.trace = match dartq_engine::event::TraceSink::create(path) {
            Ok(sink) => Some(sink),
            Err(error) => {
                eprintln!("error: trace events: {error}");
                return exit::USAGE;
            }
        };
    }
    let assembled = if extras.is_empty() {
        dartq_daemon::Session::with_runtime(config, Vec::new(), None, stale_advisory, runtime)
    } else {
        // Declared contexts ride the session's default `pooled_mapped_store`
        // posture: assembly rebinds every context's store (base and declared
        // alike) to one session-internal mapped pool, so no caller-side store
        // is armed here — pool residency is read back through the session's
        // shared-store accessors.
        dartq_daemon::Session::with_runtime(config, extras, None, stale_advisory, runtime)
    };
    let session = match assembled {
        Ok(session) => session,
        Err(error) => {
            eprintln!("dartq: internal error: daemon session unusable: {error}");
            return exit::INTERNAL_ERROR;
        }
    };
    // The reader thread owns stdin; only the main loop writes stdout, so the
    // buffer needs no lock. The `Stdin` handle (not `StdinLock`) is what is
    // `Send`. When the input ends the reader may still be blocked on the
    // next read — process exit reaps it. With `--watch`, the server also
    // spawns its poll thread over the package directory.
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

/// Why the dartq.yaml context assembly refuses. A malformed config, an
/// unknown label, an unsupported locator, or an unusable context SDK is
/// the usage tier (`error: ` + exit 64 — the config module's and the
/// compat face's conventions); an unreadable config file is internal.
#[derive(Debug)]
enum ContextAssemblyError {
    /// The config or one of its values is unusable (the message body is
    /// the printed text).
    Usage(String),
    /// `dartq.yaml` exists but cannot be read.
    Internal(io::Error),
}

/// The daemon face's shell-edge context assembly:
/// loads the workspace root's dartq.yaml — the config module's parser, the
/// one `dartq.yaml` implementation — selects the `--context` labels (the
/// compat face's convention: empty selects every declared context; an
/// unknown label names the declared set), and forks the daemon's default
/// configuration per context along the legal fork dimensions: SDK root
/// (the locator resolved against the workspace root), target panel,
/// declared variables. Each context's SDK reloads from its resolved root
/// (the snapshot type is not `Clone`), and every returned config carries
/// the mandatory `EngineConfig::store` placeholder — the session's default
/// `pooled_mapped_store` posture rebinds every context's store at assembly,
/// so the placeholder is never consumed.
/// Equal configurations dedup inside the session.
///
/// An empty list — no config, or a config without contexts — keeps the
/// single-context session, byte-identical to the pre-#1086 face.
fn assemble_contexts(
    base: &EngineConfig,
    labels: &[String],
) -> Result<Vec<(String, EngineConfig)>, ContextAssemblyError> {
    // The contexts' workspace root: the config dir's parent — the same
    // root the session itself computes and where a relative `path:`
    // locator resolves.
    let Some(workspace_root) = base.config_dir.parent() else {
        return Ok(Vec::new());
    };
    let declared = match load_dartq_config(workspace_root) {
        Ok(Some(declared)) => declared.contexts,
        Ok(None) => {
            if labels.is_empty() {
                return Ok(Vec::new());
            }
            return Err(ContextAssemblyError::Usage(format!(
                "no dartq.yaml in {} (--context needs a config declaring contexts)",
                workspace_root.join("dartq.yaml").display()
            )));
        }
        Err(ConfigError::Invalid(message)) => return Err(ContextAssemblyError::Usage(message)),
        Err(ConfigError::Io(error)) => return Err(ContextAssemblyError::Internal(error)),
    };
    // A config without contexts keeps the single-context session — unless
    // labels were named, which the lookup below refuses (the compat face's
    // exact shape: `unknown context` over an empty declared set).
    if declared.is_empty() && labels.is_empty() {
        return Ok(Vec::new());
    }
    let selected: Vec<&ContextConfig> = if labels.is_empty() {
        declared.iter().collect()
    } else {
        labels
            .iter()
            .map(|label| {
                declared
                    .iter()
                    .find(|context| &context.label == label)
                    .ok_or_else(|| {
                        let declared = declared
                            .iter()
                            .map(|context| context.label.as_str())
                            .collect::<Vec<_>>()
                            .join(", ");
                        ContextAssemblyError::Usage(format!(
                            "unknown context `{label}` (declared: {declared})"
                        ))
                    })
            })
            .collect::<Result<Vec<_>, _>>()?
    };
    let contexts = selected
        .into_iter()
        .map(|context| {
            let root = resolve_locator(&context.label, &context.sdk, workspace_root)
                .map_err(|error| ContextAssemblyError::Usage(error.to_string()))?;
            let sdk = SdkSnapshot::load(&root, None).map_err(|error| {
                ContextAssemblyError::Usage(format!(
                    "context `{}`: Dart SDK at {} unusable: {error}",
                    context.label,
                    root.display()
                ))
            })?;
            let engine = EngineConfig {
                // The file-backed CAS store, same posture as the default
                // context (see the ADR caveat at the `daemon` arming below).
                store: dartq_engine::mmap_store(),
                sdk,
                packages: base.packages.clone(),
                pub_config: std::sync::Arc::clone(&base.pub_config),
                config_dir: base.config_dir.clone(),
                semdiag: base.semdiag.clone(),
                target: context.target.map_or_else(
                    || dartq_engine::DEFAULT_TARGET.to_owned(),
                    |target| target_panel(target).to_owned(),
                ),
                declared_variables: context
                    .define
                    .iter()
                    .map(|(name, value)| (name.clone(), value.clone()))
                    .collect(),
                experiments: base.experiments.clone(),
            };
            Ok((context.label.clone(), engine))
        })
        .collect::<Result<Vec<_>, ContextAssemblyError>>()?;
    Ok(contexts)
}

/// The declared target's `libraries.json` panel — the segment the engine's
/// `dart:` URI resolution dispatches against (the declared `target` picks
/// the panel: the default `io` is the native VM panel, `web` the
/// dart2js/dartdevc panel).
fn target_panel(target: ContextTarget) -> &'static str {
    match target {
        ContextTarget::Io => dartq_engine::DEFAULT_TARGET,
        ContextTarget::Web => "js",
        ContextTarget::Wasm => "wasm",
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
// dartq query symbol — the index face
// ---------------------------------------------------------------------------

/// `dartq query symbol` — crash tier like [`analyze`]: a panic aborts with
/// exit 4 and stdout kept empty, because [`query_symbol_run`] writes only
/// after the query fully succeeded.
fn query_symbol(args: &SymbolArgs) -> u8 {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        query_symbol_run(args, &mut BufWriter::new(io::stdout().lock()))
    }));
    if let Ok(code) = outcome {
        code
    } else {
        eprintln!("dartq: internal error: analysis panicked");
        exit::INTERNAL_ERROR
    }
}

/// `dartq query references` — the same rows the symbol face serves today
/// (the index's reference surface is name-keyed and already kinded); a
/// separate spelling in the same query family, diverging as later semantics
/// land. Same tiers, same runner shape.
fn query_references(args: &SymbolArgs) -> u8 {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        query_references_run(args, &mut BufWriter::new(io::stdout().lock()))
    }));
    if let Ok(code) = outcome {
        code
    } else {
        eprintln!("dartq: internal error: analysis panicked");
        exit::INTERNAL_ERROR
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
    if let Ok(code) = outcome {
        code
    } else {
        eprintln!("dartq: internal error: analysis panicked");
        exit::INTERNAL_ERROR
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

    let written = write_query_out(
        out,
        &run,
        &args.name,
        args.format,
        args.verbosity.into_commands(),
    );
    if let Err(error) = written.and_then(|()| out.flush()) {
        eprintln!("dartq: internal error: writing query results failed: {error}");
        return exit::INTERNAL_ERROR;
    }
    exit::CLEAN
}

/// `dartq query implementations` — crash tier like [`query_symbol`].
fn query_implementations(args: &SymbolArgs) -> u8 {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        query_implementations_run(args, &mut BufWriter::new(io::stdout().lock()))
    }));
    if let Ok(code) = outcome {
        code
    } else {
        eprintln!("dartq: internal error: analysis panicked");
        exit::INTERNAL_ERROR
    }
}

/// `dartq query implementations` — callers of the name's reverse supertype walk
/// (`dartq_commands::query::implementations`); exit 0 on success, zero
/// results included.
fn query_implementations_run(args: &SymbolArgs, out: &mut impl Write) -> u8 {
    let (mut engine, _dir) = match open_engine(&args.dir, args.sdk.as_deref()) {
        Ok(faces) => faces,
        Err(error) => {
            eprintln!("error: {error}");
            return exit::USAGE;
        }
    };
    let run = match dartq_commands::query::implementations(&mut engine, &args.name) {
        Ok(run) => run,
        Err(error) => {
            eprintln!("dartq: internal error: {error}");
            return exit::INTERNAL_ERROR;
        }
    };

    let written = write_query_out(
        out,
        &run,
        &args.name,
        args.format,
        args.verbosity.into_commands(),
    );
    if let Err(error) = written.and_then(|()| out.flush()) {
        eprintln!("dartq: internal error: writing query results failed: {error}");
        return exit::INTERNAL_ERROR;
    }
    exit::CLEAN
}

/// `dartq query dependencies` — crash tier like [`query_symbol`].
fn query_dependencies(args: &SymbolArgs) -> u8 {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        query_dependencies_run(args, &mut BufWriter::new(io::stdout().lock()))
    }));
    if let Ok(code) = outcome {
        code
    } else {
        eprintln!("dartq: internal error: analysis panicked");
        exit::INTERNAL_ERROR
    }
}

/// One dependencies run: the declaring library's own import edges and
/// consumed dependency symbols (`dartq_commands::query::dependencies`);
/// exit 0 on success, zero results included.
fn query_dependencies_run(args: &SymbolArgs, out: &mut impl Write) -> u8 {
    let (mut engine, _dir) = match open_engine(&args.dir, args.sdk.as_deref()) {
        Ok(faces) => faces,
        Err(error) => {
            eprintln!("error: {error}");
            return exit::USAGE;
        }
    };
    let run = match dartq_commands::query::dependencies(&mut engine, &args.name) {
        Ok(run) => run,
        Err(error) => {
            eprintln!("dartq: internal error: {error}");
            return exit::INTERNAL_ERROR;
        }
    };

    let written = write_query_out(
        out,
        &run,
        &args.name,
        args.format,
        args.verbosity.into_commands(),
    );
    if let Err(error) = written.and_then(|()| out.flush()) {
        eprintln!("dartq: internal error: writing query results failed: {error}");
        return exit::INTERNAL_ERROR;
    }
    exit::CLEAN
}

/// `dartq query batch` — the multi-symbol/multi-kind batch face: one
/// workspace assembly, one analyze pass, then every (`--symbol` × `--kind`)
/// pair answered in
/// request order (item order, then kind order). Crash tier like
/// [`query_symbol`]. Output is the JSONL batch convention — one frozen v1
/// query envelope per line, no aggregate envelope (dartq-schema's
/// token-economics docs) — so each line is the single faces' own JSON.
fn query_batch(args: &BatchArgs) -> u8 {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        query_batch_run(args, &mut BufWriter::new(io::stdout().lock()))
    }));
    if let Ok(code) = outcome {
        code
    } else {
        eprintln!("dartq: internal error: analysis panicked");
        exit::INTERNAL_ERROR
    }
}

/// One batch run: the request items (`--symbol` order, each carrying the
/// full `--kind` set, the symbol face when no kind came), the shared
/// `dartq_commands::query::batch` execution, then one JSON line per answer.
fn query_batch_run(args: &BatchArgs, out: &mut impl Write) -> u8 {
    let kinds = if args.kinds.is_empty() {
        vec![QueryKind::Symbol]
    } else {
        args.kinds.clone()
    };
    let queries: Vec<BatchQuery> = args
        .symbols
        .iter()
        .map(|name| BatchQuery {
            name: name.clone(),
            kinds: kinds.clone(),
        })
        .collect();
    let (mut engine, _dir) = match open_engine(&args.dir, args.sdk.as_deref()) {
        Ok(faces) => faces,
        Err(error) => {
            eprintln!("error: {error}");
            return exit::USAGE;
        }
    };
    let answers = match dartq_commands::query::batch(&mut engine, &queries) {
        Ok(answers) => answers,
        Err(error) => {
            eprintln!("dartq: internal error: {error}");
            return exit::INTERNAL_ERROR;
        }
    };

    let written = answers
        .iter()
        .try_for_each(|answer| {
            writeln!(
                out,
                "{}",
                dartq_commands::query::to_json_at_tier(
                    &answer.run,
                    answer.name,
                    args.verbosity.into_commands()
                )
            )
        })
        .and_then(|()| out.flush());
    if let Err(error) = written {
        eprintln!("dartq: internal error: writing query results failed: {error}");
        return exit::INTERNAL_ERROR;
    }
    exit::CLEAN
}

/// The query faces' shared output: machine rows or the tier-shaped
/// one-line JSON envelope (`dartq_commands::query::to_json_at_tier`, the
/// frozen v1 contract at compact/full, the red line at minimal).
fn write_query_out(
    out: &mut impl Write,
    run: &dartq_commands::query::SymbolRun<'_>,
    name: &str,
    format: QueryFormat,
    verbosity: dartq_commands::Verbosity,
) -> io::Result<()> {
    match format {
        QueryFormat::Machine => write_query_machine(out, run, verbosity),
        QueryFormat::Json => {
            writeln!(
                out,
                "{}",
                dartq_commands::query::to_json_at_tier(run, name, verbosity)
            )
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

    let written = write_query_out(
        out,
        &run,
        &args.name,
        args.format,
        args.verbosity.into_commands(),
    );
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
/// DECL|file|container|kind|name|line|column|length|signature|annotations
///                                                     (container `-` when
///                                                     top-level; 1-based;
///                                                     the trailing columns
///                                                     spell the JSON
///                                                     face's element-face
///                                                     extras — `signature`
///                                                     `-` when the shape
///                                                     has none,
///                                                     `annotations` `-`
///                                                     when none, else the
///                                                     verbatim texts
///                                                     joined `,`)
/// REF|target-file|kind|explicit|offset|length|use-file
///                                                 (offset is the use
///                                                 site's UTF-16 offset;
///                                                 `explicit` is
///                                                 `true`/`false` — the use
///                                                 is spelled `p.x` with an
///                                                 import prefix; the JSON
///                                                 face's `qualified`
///                                                 column is the
///                                                 `<prefix>.<name>` string,
///                                                 not this field)
/// IMPL|file|container|kind|name|line|column|length|signature|annotations
///                                                     (the implementations
///                                                     face's rows, the
///                                                     DECL grammar)
/// DEP|path|name                                     (the dependencies
///                                                     face's forward rows:
///                                                     the joined library's
///                                                     label plus the consumed
///                                                     symbol's name — `-` on
///                                                     a bare import edge)
/// ```
///
/// The file columns are the canonical paths (the machine grammar's
/// identity spelling; the JSON face projects the package-form labels).
///
/// At the `minimal` tier the grammar carries the red line only: the
/// container, `signature`, and `annotations` columns drop from DECL/IMPL
/// rows, the `explicit` column drops from REF rows (the machine spelling
/// of the JSON face's `qualified`), and the reserved kind arrays never
/// print (no IMPL/DEP rows):
///
/// ```text
/// DECL|file|kind|name|line|column|length
/// REF|target-file|kind|offset|length|use-file
/// ```
fn write_query_machine(
    out: &mut impl Write,
    run: &dartq_commands::query::SymbolRun<'_>,
    verbosity: dartq_commands::Verbosity,
) -> io::Result<()> {
    let minimal = verbosity == dartq_commands::Verbosity::Minimal;
    let layout = &run.layout;
    let engine = layout.engine();
    // The shared declaration-row grammar: DECL for the queried symbol's
    // own rows, IMPL for the implementations face's — same columns, the
    // prefix the harness parser dispatches on. The row renders to a String
    // so the emit loops over `out` never overlap the builder's borrows.
    let declaration_row = |prefix: &str, decl: &DeclHit<'_>| {
        let (line, column) = layout.line_col(decl.file, decl.range.offset);
        let file = layout.file_path(decl.file).display();
        let kind = decl_kind_name(decl.kind);
        if minimal {
            format!(
                "{prefix}|{file}|{kind}|{}|{line}|{column}|{}",
                decl.name, decl.range.length
            )
        } else {
            let (signature, annotations) = declaration_extras(engine, decl);
            format!(
                "{prefix}|{file}|{}|{kind}|{}|{line}|{column}|{}|{}|{}",
                decl.container.unwrap_or("-"),
                decl.name,
                decl.range.length,
                signature.as_deref().unwrap_or("-"),
                machine_annotations(&annotations),
            )
        }
    };
    for decl in &run.decls {
        writeln!(out, "{}", declaration_row("DECL", decl))?;
    }
    for reference in &run.refs {
        if minimal {
            writeln!(
                out,
                "REF|{}|{}|{}|{}|{}",
                layout.file_path(reference.target_file).display(),
                ref_kind_name(reference.kind),
                reference.range.offset,
                reference.range.length,
                layout.file_path(reference.file).display(),
            )?;
        } else {
            writeln!(
                out,
                "REF|{}|{}|{}|{}|{}|{}",
                layout.file_path(reference.target_file).display(),
                ref_kind_name(reference.kind),
                reference.explicit,
                reference.range.offset,
                reference.range.length,
                layout.file_path(reference.file).display(),
            )?;
        }
    }
    if !minimal {
        for decl in &run.implementations {
            writeln!(out, "{}", declaration_row("IMPL", decl))?;
        }
        for row in &run.dependencies {
            writeln!(
                out,
                "DEP|{}|{}",
                row.path,
                row.name.as_deref().unwrap_or("-")
            )?;
        }
    }
    Ok(())
}

/// The machine spelling of the annotations column: `-` when none, else the
/// verbatim texts joined with `,` — the column closes the row, so a
/// consumer splitting on `|` takes the tail verbatim.
fn machine_annotations(annotations: &[String]) -> String {
    if annotations.is_empty() {
        "-".to_owned()
    } else {
        annotations.join(",")
    }
}

// ---------------------------------------------------------------------------
// dartq explain — the one-stop AI report face
// ---------------------------------------------------------------------------

/// `dartq explain` — crash tier like the other engine faces: a panic aborts
/// with exit 4 and stdout kept empty, because [`explain_run`] writes only
/// after the report fully succeeded.
fn explain_entry(args: &ExplainArgs) -> u8 {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        explain_run(args, &mut BufWriter::new(io::stdout().lock()))
    }));
    if let Ok(code) = outcome {
        code
    } else {
        eprintln!("dartq: internal error: analysis panicked");
        exit::INTERNAL_ERROR
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
    // The diagnostic face's positional slot carries the workspace `dir`
    // (`explain --diagnostic CODE [dir]`): a positional is a dir, never a
    // symbol, when `--diagnostic` is set. The explicit second positional
    // still binds `dir`; both slots filled at once is ambiguous — usage.
    let report = match (&args.symbol, &args.diagnostic) {
        (dir_pos @ (None | Some(_)), Some(code)) => {
            let dir = match dir_pos {
                None => args.dir.clone(),
                Some(positional) if args.dir == PathBuf::from(".") => {
                    PathBuf::from(positional)
                }
                Some(_) => {
                    eprintln!("error: two positionals on --diagnostic; give CODE [dir]");
                    return exit::USAGE;
                }
            };
            let selector = match args.file.as_deref().map(parse_file_selector) {
                Some(Ok(selector)) => Some(selector),
                Some(Err(message)) => {
                    eprintln!("error: --file: {message}");
                    return exit::USAGE;
                }
                None => None,
            };
            match dartq_commands::explain::explain_diagnostic(
                &dir,
                args.sdk.as_deref(),
                code,
                selector,
                args.verbosity.into_commands(),
            ) {
                Ok(report) => report,
                Err(
                    error @ (ExplainError::Workspace(_)
                    | ExplainError::Engine(_)
                    | ExplainError::NoSuchDiagnostic(_)),
                ) => {
                    eprintln!("error: {error}");
                    return exit::USAGE;
                }
                Err(error) => {
                    eprintln!("dartq: internal error: {error}");
                    return exit::INTERNAL_ERROR;
                }
            }
        }
        (Some(symbol), None) => match dartq_commands::explain::explain(
            &args.dir,
            args.sdk.as_deref(),
            symbol,
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
        },
        _ => {
            eprintln!("error: give a symbol name or --diagnostic CODE");
            return exit::USAGE;
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

/// Parses the `--file` argument's `PATH[:LINE[:COL]]` spelling. The path
/// is canonicalized at the call site (the engine's interned spelling).
fn parse_file_selector(text: &str) -> Result<dartq_commands::explain::FileSelector, String> {
    // Split at most twice from the right: `path:line:col`, `path:line`,
    // `path` — a path with a drive letter or containing `:` still parses
    // because the numeric suffixes only count when they follow a `:`.
    let (path, line, column) = match text.rfind(':') {
        None => (text, None, None),
        Some(right) => {
            let tail = &text[right + 1..];
            if tail.chars().all(|c| c.is_ascii_digit()) && !tail.is_empty() {
                // One trailing number could be line or column; a second
                // `:` before it decides.
                let before = &text[..right];
                match before.rfind(':') {
                    Some(left)
                        if before[left + 1..]
                            .chars()
                            .all(|c| c.is_ascii_digit())
                            && !before[left + 1..].is_empty() =>
                    {
                        (
                            &before[..left],
                            Some(&before[left + 1..]),
                            Some(tail),
                        )
                    }
                    _ => (before, Some(tail), None),
                }
            } else {
                (text, None, None)
            }
        }
    };
    if path.is_empty() {
        return Err("the file path is empty".to_owned());
    }
    let parse_u32 = |text: &str, what: &str| {
        text.parse::<u32>()
            .map_err(|_| format!("the {what} must be a 1-based integer (got '{text}')"))
    };
    Ok(dartq_commands::explain::FileSelector {
        path: PathBuf::from(path),
        line: line.map(|s| parse_u32(s, "line")).transpose()?,
        column: column.map(|s| parse_u32(s, "column")).transpose()?,
    })
}

/// One JSON object at exit.
/// A failed write is an internal error (4) at the call site, never silent —
/// a run without its metrics file would corrupt benchmark data.
fn write_metrics(path: &Path, metrics: &RunMetrics) -> io::Result<()> {
    let mut file = fs::File::create(path)?;
    writeln!(file, "{}", metrics.json())
}

fn elapsed_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// Peak RSS through getrusage; macOS reports bytes, Linux KiB (normalized
/// to bytes).
#[cfg(unix)]
fn max_rss_bytes() -> Option<u64> {
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, &raw mut usage) } != 0 {
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
    use dartq_commands::workspace::open_engine_with_overrides;
    use dartq_diagnostics::{EXPECTED_TOKEN, SourceRange};
    use dartq_source::FileTable;
    use filetime::{FileTime, set_file_mtime};

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
        assert!(
            matches!(check.verbosity, DiagnosticVerbosity::Default),
            "default verbosity tier"
        );
        assert!(!check.no_fatal_warnings, "fatal-warnings default on");
        assert!(
            !check.fatal_warnings,
            "the positive spelling defaults unset"
        );
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

    /// G3 (cli-compat census): the `--[no-]fatal-warnings` pair resolves
    /// last-spelling-wins, the oracle's package:args flag semantics — with
    /// both spellings in one invocation the later one decides, on both
    /// diagnostic faces. `no_fatal_warnings` is the field the exit
    /// arithmetic reads (`!no_fatal_warnings` = fatal); an overridden
    /// spelling falls back to unset.
    #[test]
    fn fatal_warnings_last_spelling_wins() {
        let check_fatal = |args: &[&str]| !check_args(args).no_fatal_warnings;
        assert!(
            check_fatal(&["check", "--fatal-warnings"]),
            "the positive spelling alone keeps the fatal default"
        );
        assert!(
            !check_fatal(&["check", "--no-fatal-warnings"]),
            "the negative spelling alone clears fatality"
        );
        assert!(
            !check_fatal(&["check", "--fatal-warnings", "--no-fatal-warnings"]),
            "the later negative spelling wins"
        );
        assert!(
            check_fatal(&["check", "--no-fatal-warnings", "--fatal-warnings"]),
            "the later positive spelling restores fatality"
        );

        let analyze_no_fatal = |args: &[&str]| match parse_args_of(args) {
            Ok(Some(Command::Analyze(args))) => args.no_fatal_warnings,
            _ => panic!("expected analyze command"),
        };
        assert!(
            analyze_no_fatal(&["analyze", "--fatal-warnings", "--no-fatal-warnings", "/pkg"]),
            "the analyze face resolves the same pair"
        );
        assert!(
            !analyze_no_fatal(&["analyze", "--no-fatal-warnings", "--fatal-warnings", "/pkg"]),
            "the analyze face resolves the same pair"
        );
    }

    /// The `-v`/`--verbosity` tier flag on both diagnostic faces: the three
    /// tiers parse through long, `=` and short spellings (the analyze face
    /// pinned with the flag alongside the positional), the unset default is
    /// `default`; the invalid spellings are covered by `usage_errors`.
    #[test]
    fn verbosity_tier_parse() {
        assert!(
            matches!(
                check_args(&["check"]).verbosity,
                DiagnosticVerbosity::Default
            ),
            "unset default"
        );
        assert!(
            matches!(
                check_args(&["check", "-v", "concise"]).verbosity,
                DiagnosticVerbosity::Concise
            ),
            "short spelling"
        );
        assert!(
            matches!(
                check_args(&["check", "--verbosity=verbose"]).verbosity,
                DiagnosticVerbosity::Verbose
            ),
            "= spelling"
        );

        let parsed = parse_args_of(&["analyze", "--verbosity", "concise", "/pkg"])
            .expect("parse")
            .expect("subcommand");
        let Command::Analyze(args) = parsed else {
            panic!("expected analyze command")
        };
        assert!(matches!(args.verbosity, DiagnosticVerbosity::Concise));
    }

    #[test]
    fn usage_errors() {
        for args in [
            &["check", "--format"][..],
            &["check", "--format", "yaml"][..],
            &["check", "--format=yaml"][..],
            &["check", "--bogus"][..],
            &["check", "-x"][..],
            &["check", "-v"][..],
            &["check", "--verbosity", "chatty"][..],
            &["analyze", "-v", "chatty", "/pkg"][..],
            &["frobnicate"][..],
            &["--bogus"][..],
            // `json` is a legal analyze format (the dartdev oracle's allowed
            // set is exactly default|json|machine, commands/analyze.dart) —
            // the invalid-value role these lines pin is `yaml`'s and the
            // retired `human` spelling's, in both spellings.
            &["analyze", "--format", "yaml", "/p"][..],
            &["analyze", "--format=yaml", "/p"][..],
            &["analyze", "--format", "human", "/p"][..],
            &["analyze", "--format=human", "/p"][..],
            &["analyze", "--metrics-json"][..],
            &["check", "--metrics-json"][..],
            &["query"][..],
            &["query", "symbol"][..],
            &["query", "references"][..],
            &["query", "callers"][..],
            &["query", "bogus"][..],
            // `explain` parses bare since the face landed (74e3258a): the
            // symbol slot is optional and the run tier rejects the
            // no-selector form — the invalid-value role these lines pin is
            // the report format's (`--format=machine`; json is the only
            // spelling) and the required changed-file positional of
            // `impact` (483a6642).
            &["explain", "--format=machine", "Dog"][..],
            &["impact"][..],
        ] {
            assert!(parse_args_of(args).is_err(), "must reject {args:?}");
        }
    }

    #[test]
    fn version_forms() {
        assert!(
            matches!(parse_args_of(&[]), Ok(None | Some(Command::Version))),
            "bare dartq falls back to version"
        );
        assert!(matches!(
            parse_args_of(&["version"]),
            Ok(Some(Command::Version))
        ));
        // `--version` surfaces as clap's display error; run() routes it to
        // stdout + exit 0 (use_stderr false).
        let Err(error) = parse_args_of(&["--version"]) else {
            panic!("--version must surface as a display error")
        };
        assert_eq!(error.kind(), clap::error::ErrorKind::DisplayVersion);
        assert!(!error.use_stderr());
    }

    /// The dartdev global flags parse on both diagnostic faces and stay
    /// no-ops (ticket #2385, the cli-compat Flag-face P2; #2390 adds the
    /// hidden `--disable-telemetry` and the deprecated `--[no-]analytics`
    /// pair). Upstream declares them on the global dartdev parser
    /// (`upstream Dart SDK source, pkg/dartdev/lib/src/utils.dart`,
    /// `globalDartdevOptionsParser`) and merges the global parse past the
    /// command name, so the CI incantation `dart analyze
    /// --suppress-analytics` runs to the diagnostic-tier exit; `--version`
    /// is declared but never read past dispatch (no print, no exit). Each
    /// flag lands on exactly its own ignored field and never disturbs the
    /// positional targets; a genuinely unknown flag still takes the
    /// exit-64 usage path.
    #[test]
    fn dartdev_globals_accepted_and_ignored() {
        // Declaration order per `globalDartdevOptionsParser`.
        const GLOBALS: [&str; 8] = [
            "--verbose",
            "--version",
            "--enable-analytics",
            "--disable-analytics",
            "--disable-telemetry",
            "--diagnostics",
            "--analytics",
            "--suppress-analytics",
        ];
        for (index, flag) in GLOBALS.iter().enumerate() {
            let Command::Analyze(analyze) = parse_args_of(&["analyze", flag, "/pkg"])
                .unwrap_or_else(|error| panic!("{flag} must parse on analyze: {error}"))
                .expect("subcommand")
            else {
                panic!("expected analyze command")
            };
            let fields = [
                analyze.dartdev_globals.verbose,
                analyze.dartdev_globals.version,
                analyze.dartdev_globals.enable_analytics,
                analyze.dartdev_globals.disable_analytics,
                analyze.dartdev_globals.disable_telemetry,
                analyze.dartdev_globals.diagnostics,
                analyze.dartdev_globals.analytics,
                analyze.dartdev_globals.suppress_analytics,
            ];
            assert!(fields[index], "{flag} must land on its own no-op field");
            assert_eq!(
                fields.iter().filter(|set| **set).count(),
                1,
                "exactly the invoked flag is set"
            );
            assert_eq!(
                analyze.targets,
                vec![PathBuf::from("/pkg")],
                "{flag} must not consume the target positional"
            );

            let check = check_args(&["check", flag, "/tmp/a"]);
            assert_eq!(
                check.paths,
                vec![PathBuf::from("/tmp/a")],
                "{flag} must not consume the target positional (check)"
            );
            let fields = [
                check.dartdev_globals.verbose,
                check.dartdev_globals.version,
                check.dartdev_globals.enable_analytics,
                check.dartdev_globals.disable_analytics,
                check.dartdev_globals.disable_telemetry,
                check.dartdev_globals.diagnostics,
                check.dartdev_globals.analytics,
                check.dartdev_globals.suppress_analytics,
            ];
            assert!(
                fields[index],
                "{flag} must land on its own no-op field (check)"
            );
            assert_eq!(
                fields.iter().filter(|set| **set).count(),
                1,
                "exactly the invoked flag is set (check)"
            );
        }

        // The deprecated `--[no-]analytics` negation: the negative
        // spelling lands on the same pair, the last spelling wins.
        for args in [
            &["analyze", "--no-analytics", "/pkg"][..],
            &["check", "--no-analytics", "/tmp/a"][..],
        ] {
            let parsed = parse_args_of(args)
                .unwrap_or_else(|error| panic!("{args:?} must parse: {error}"))
                .expect("subcommand");
            let globals = match parsed {
                Command::Analyze(analyze) => analyze.dartdev_globals,
                Command::Check(check) => check.dartdev_globals,
                _ => panic!("expected analyze or check"),
            };
            assert!(!globals.analytics, "the negative spelling clears it");
            assert!(globals.no_analytics, "the negative spelling is set");
        }

        // The acceptance is not a wildcard: a flag outside the oracle's
        // global set still fails on the same faces (the exit-64 usage
        // path).
        for args in [
            &["analyze", "--suppress-analytics", "--bogus", "/pkg"][..],
            &["analyze", "--verbose", "-x", "/pkg"][..],
            &["check", "--suppress-analytics", "--bogus", "/tmp/a"][..],
        ] {
            assert!(parse_args_of(args).is_err(), "must reject {args:?}");
        }
    }

    /// The compatibility face's `--context` flag: repeatable labels,
    /// `--sdk-root` unchanged alongside.
    #[test]
    fn compatibility_context_flag_parse() {
        let parsed = parse_args_of(&[
            "compatibility",
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
        let Command::Compatibility(args) = parsed else {
            panic!("expected compatibility command")
        };
        assert_eq!(args.contexts, vec!["a".to_owned(), "b".to_owned()]);
        assert_eq!(args.sdk_roots, vec![PathBuf::from("/s")]);
        assert_eq!(args.path, PathBuf::from("f.dart"));
    }

    /// `compat` is the hidden alias for `compatibility`: it parses to the
    /// same variant and never surfaces in help.
    #[test]
    fn compat_is_a_hidden_alias() {
        let parsed = parse_args_of(&["compat", "f.dart"])
            .expect("parse")
            .expect("subcommand");
        assert!(
            matches!(parsed, Command::Compatibility(_)),
            "the `compat` spelling routes to Compatibility"
        );
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

    /// The exit-code severity table, row by row.
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
            verbosity: DiagnosticVerbosity::Default,
            metrics_json: None,
            fatal_warnings: false,
            no_fatal_warnings: false,
            fatal_infos: false,
            dartdev_globals: DartdevGlobals::default(),
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
            verbosity: DiagnosticVerbosity::Default,
            metrics_json: None,
            fatal_warnings: false,
            no_fatal_warnings: false,
            fatal_infos: false,
            dartdev_globals: DartdevGlobals::default(),
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

    /// `--metrics-json` writes the analyze-shaped JSON at exit (the flag
    /// sits in the check grammar too); the library counters stay at zero
    /// because the check face has no engine libraries.
    #[test]
    fn check_writes_metrics_json() {
        let root = std::env::temp_dir().join(format!("dq_cli_metrics_{}", std::process::id()));
        fs::create_dir_all(&root).expect("mkdir");
        fs::write(root.join("a.dart"), "void f() { x }\n").expect("write");

        let metrics_path = root.join("metrics.json");
        let args = CheckArgs {
            format: CheckFormat::Machine,
            verbosity: DiagnosticVerbosity::Default,
            metrics_json: Some(metrics_path.clone()),
            fatal_warnings: false,
            no_fatal_warnings: false,
            fatal_infos: false,
            dartdev_globals: DartdevGlobals::default(),
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
    /// alike (`context_root.dart:116` + 146-149): `build/` IS descended into
    /// (the old dartq-only `build`/`node_modules` skip diverged from the
    /// oracle, which reserves those names to `analysis_options` globs), and a
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
            verbosity: DiagnosticVerbosity::Default,
            metrics_json: None,
            fatal_warnings: false,
            no_fatal_warnings: false,
            fatal_infos: false,
            dartdev_globals: DartdevGlobals::default(),
            paths: vec![root.join("notes.txt")],
        };
        assert_eq!(check(&args), exit::CLEAN);

        fs::remove_dir_all(&root).ok();
    }

    // -----------------------------------------------------------------------
    // dartq analyze
    // -----------------------------------------------------------------------

    /// The analyze command face: the oracle default-format default over the
    /// cwd positional, flag spellings (`--format=default|machine|json`,
    /// `--sdk-path` with the hidden `--sdk` alias, both fatal-warnings
    /// spellings). Malformed spellings are covered by `usage_errors`.
    #[test]
    fn analyze_args_parse() {
        let parsed = parse_args_of(&["analyze", "/pkg"])
            .expect("parse")
            .expect("subcommand");
        let Command::Analyze(args) = parsed else {
            panic!("expected analyze command")
        };
        assert!(
            matches!(args.format, AnalyzeFormat::Default),
            "the oracle default-format default"
        );
        assert!(
            matches!(args.verbosity, DiagnosticVerbosity::Default),
            "default verbosity tier"
        );
        assert!(args.metrics_json.is_none() && args.sdk_path.is_none());
        assert!(args.overlays.is_empty(), "no overlay by default");
        assert!(!args.no_fatal_warnings && !args.fatal_infos);
        assert!(!args.fatal_warnings, "the positive spelling defaults unset");
        assert_eq!(args.memory_limit, None, "unlimited by default");
        assert_eq!(args.trace_events, None, "untraced by default");
        assert_eq!(
            args.cache_size_limit, None,
            "unset = the 10 GiB default cap"
        );
        assert_eq!(args.targets, vec![PathBuf::from("/pkg")]);

        // Bare `analyze` targets the cwd (the oracle's Directory.current
        // default).
        let parsed = parse_args_of(&["analyze"])
            .expect("parse")
            .expect("subcommand");
        let Command::Analyze(args) = parsed else {
            panic!("expected analyze command")
        };
        assert_eq!(args.targets, vec![PathBuf::from(".")], "default target cwd");

        let parsed = parse_args_of(&[
            "analyze",
            "--format=default",
            "--metrics-json",
            "/tmp/m.json",
            "--overlay",
            "lib/a.dart=/tmp/a.dart",
            "--overlay=lib/b.dart=/tmp/b.dart",
            "--sdk-path",
            "/sdk",
            "--fatal-warnings",
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
        assert!(matches!(args.format, AnalyzeFormat::Default));
        assert_eq!(args.metrics_json, Some(PathBuf::from("/tmp/m.json")));
        assert_eq!(args.sdk_path, Some(PathBuf::from("/sdk")));
        assert_eq!(
            args.overlays,
            vec![
                "lib/a.dart=/tmp/a.dart".to_owned(),
                "lib/b.dart=/tmp/b.dart".to_owned(),
            ],
            "repeatable --overlay specs"
        );
        assert!(
            args.no_fatal_warnings && args.fatal_infos,
            "both spellings parse; --fatal-infos is independent"
        );
        assert!(
            !args.fatal_warnings,
            "the later --no-fatal-warnings overrides the earlier --fatal-warnings"
        );
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

        // The hidden compat aliases: `--sdk` and the machine spelling.
        let parsed = parse_args_of(&["analyze", "--format=machine", "--sdk", "/sdk", "/pkg"])
            .expect("parse")
            .expect("subcommand");
        let Command::Analyze(args) = parsed else {
            panic!("expected analyze command")
        };
        assert!(matches!(args.format, AnalyzeFormat::Machine));
        assert_eq!(args.sdk_path, Some(PathBuf::from("/sdk")));

        let parsed = parse_args_of(&["analyze", "--format=json", "/pkg"])
            .expect("parse")
            .expect("subcommand");
        let Command::Analyze(args) = parsed else {
            panic!("expected analyze command")
        };
        assert!(
            matches!(args.format, AnalyzeFormat::Json),
            "the json spelling parses"
        );

        // Multiple positional targets parse in order (the oracle's `rest`).
        let parsed = parse_args_of(&["analyze", "/pkg", "lib/a.dart", "test/"])
            .expect("parse")
            .expect("subcommand");
        let Command::Analyze(args) = parsed else {
            panic!("expected analyze command")
        };
        assert_eq!(
            args.targets,
            vec![
                PathBuf::from("/pkg"),
                PathBuf::from("lib/a.dart"),
                PathBuf::from("test/"),
            ],
            "mixed file+dir positionals collect in argv order"
        );

        // The oracle's hidden flags: `--cache` takes a path, `--memory` and
        // `--[no-]use-aot-snapshot` are flags — all parse, all ignored.
        let parsed = parse_args_of(&[
            "analyze",
            "--cache",
            "/tmp/ana-cache",
            "--memory",
            "--use-aot-snapshot",
            "--no-use-aot-snapshot",
            "/pkg",
        ])
        .expect("parse")
        .expect("subcommand");
        let Command::Analyze(args) = parsed else {
            panic!("expected analyze command")
        };
        assert_eq!(args.cache, Some(PathBuf::from("/tmp/ana-cache")));
        assert!(args.memory, "--memory parses");
        assert!(
            args.use_aot_snapshot && args.no_use_aot_snapshot,
            "both --[no-]use-aot-snapshot spellings parse"
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
            // The pub-writer shape: pub always emits `packageUri: "lib/"`
            // (upstream pub source, lib/src/entrypoint.dart:511) — without
            // it the reader contract anchors the source root at the
            // package root itself (package_config_json.dart:152), and no
            // `package:` URI addresses the `lib/` members.
            r#"{"configVersion": 2, "packages": [{"name": "fixture", "rootUri": "../", "packageUri": "lib/", "languageVersion": "3.5"}], "generator": "pub", "generatorVersion": "3.13.1"}"#,
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
        // Deterministic fresh resolution chain: the freshness judgment is
        // strict mtime ordering (equal counts as stale), and coarse-mtime
        // filesystems would false-stale the byte-pinned faces. pubspec <
        // lock < config; the in-run content baseline would excuse a bare
        // re-stamp anyway, so the stale-advisory test swaps in a config
        // entry whose package directory is missing instead.
        let stamp = |relative: &str, secs: i64| {
            set_file_mtime(root.join(relative), FileTime::from_unix_time(secs, 0))
                .expect("set mtime");
        };
        stamp("pubspec.yaml", 1_000);
        stamp("pubspec.lock", 2_000);
        stamp(".dart_tool/package_config.json", 3_000);
        root
    }

    fn analyze_args(
        root: &Path,
        format: AnalyzeFormat,
        metrics_json: Option<PathBuf>,
    ) -> AnalyzeArgs {
        AnalyzeArgs {
            targets: vec![root.to_path_buf()],
            format,
            verbosity: DiagnosticVerbosity::Default,
            metrics_json,
            overlays: Vec::new(),
            sdk_path: Some(root.join("sdk")),
            packages: None,
            enable_experiments: Vec::new(),
            plugins: true,
            no_plugins: false,
            fatal_warnings: false,
            no_fatal_warnings: false,
            fatal_infos: false,
            memory_limit: None,
            trace_events: None,
            cache_size_limit: None,
            cache: None,
            use_aot_snapshot: false,
            no_use_aot_snapshot: false,
            memory: false,
            dartdev_globals: DartdevGlobals::default(),
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

    /// The verbosity tiers ride the analyze face end to end: `concise`
    /// strips each oracle row to its identifying prefix (the message field
    /// gone), `verbose` extends every default row by exactly one field, and
    /// the exit tier is untouched by the output volume. (The exact verbose
    /// ninth-field bytes — correctionMessage vs `-`, escaping included —
    /// are pinned at the shared renderer's tests in dartq-commands' check
    /// module.)
    #[test]
    fn analyze_verbosity_tiers_thread_the_row_faces() {
        let root = analyze_fixture("verbosity", &[("lib/broken.dart", "void f() { x }\n")]);

        let mut out: Vec<u8> = Vec::new();
        let concise = AnalyzeArgs {
            verbosity: DiagnosticVerbosity::Concise,
            ..analyze_args(&root, AnalyzeFormat::Machine, None)
        };
        assert_eq!(analyze_run(&concise, &mut out), exit::ERRORS);
        let concise_text = String::from_utf8(out).expect("UTF-8");
        assert_eq!(
            concise_text,
            format!(
                "ERROR|SYNTACTIC_ERROR|EXPECTED_TOKEN|{}|1|12|1\n\
                 ERROR|COMPILE_TIME_ERROR|UNDEFINED_IDENTIFIER|{}|1|12|1\n",
                root.canonicalize()
                    .expect("root")
                    .join("lib/broken.dart")
                    .display(),
                root.canonicalize()
                    .expect("root")
                    .join("lib/broken.dart")
                    .display()
            ),
            "concise rows keep the code + span prefix and drop the message"
        );

        let mut out: Vec<u8> = Vec::new();
        let verbose = AnalyzeArgs {
            verbosity: DiagnosticVerbosity::Verbose,
            ..analyze_args(&root, AnalyzeFormat::Machine, None)
        };
        assert_eq!(analyze_run(&verbose, &mut out), exit::ERRORS);
        let verbose_text = String::from_utf8(out).expect("UTF-8");
        for (concise_row, verbose_row) in concise_text.lines().zip(verbose_text.lines()) {
            assert!(
                verbose_row.starts_with(&format!("{concise_row}|")),
                "verbose extends the default row by one field: {verbose_row}"
            );
        }

        fs::remove_dir_all(&root).ok();
    }

    /// The Json face: one line, the oracle envelope — byte parity with
    /// `dart analyze --format=json` through dartq-diagnostics' shared
    /// oracle-exact writer (its goldens pin the byte-level contract; here
    /// the known fixture trace pins the CLI plumbing: both rows share `x`'s
    /// span (machine `1|12|1` → 1-based line 1, column 12 at UTF-16 offset
    /// 11, through the half-open end at offset 12), rows keep the
    /// severity-first order with the message tie-break, the advisory has no
    /// row, and the exit tier ignores the output face.
    #[test]
    fn analyze_json_face_serves_the_oracle_envelope() {
        let root = analyze_fixture("json", &[("lib/broken.dart", "void f() { x }\n")]);

        let mut out: Vec<u8> = Vec::new();
        let code = analyze_run(&analyze_args(&root, AnalyzeFormat::Json, None), &mut out);
        assert_eq!(code, exit::ERRORS, "the exit tier ignores the face");
        let text = String::from_utf8(out).expect("UTF-8");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 1, "one line: the envelope only");
        let envelope: serde_json::Value = serde_json::from_str(lines[0]).expect("one JSON object");
        assert_eq!(envelope["version"], 1, "the oracle envelope's version key");
        let rows = envelope["diagnostics"].as_array().expect("rows");
        assert_eq!(rows.len(), 2);
        let file = root
            .canonicalize()
            .expect("canonical root")
            .join("lib/broken.dart");
        let expected_range = serde_json::json!({
            "start": {"offset": 11, "line": 1, "column": 12},
            "end": {"offset": 12, "line": 1, "column": 13},
        });
        assert_eq!(
            rows[0],
            serde_json::json!({
                "code": "expected_token",
                "severity": "ERROR",
                "type": "SYNTACTIC_ERROR",
                "location": {"file": file.display().to_string(), "range": expected_range},
                "problemMessage": "Expected to find ';'.",
            }),
            "the syntax row: the oracle's closed field set, 1-based line/column, UTF-16 offsets"
        );
        assert_eq!(
            rows[1],
            serde_json::json!({
                "code": "undefined_identifier",
                "severity": "ERROR",
                "type": "COMPILE_TIME_ERROR",
                "location": {"file": file.display().to_string(), "range": expected_range},
                "problemMessage": "Undefined name 'x'.",
                "documentation": "https://dart.dev/diagnostics/undefined_identifier",
            }),
            "the semantic row in the shared span, severity-first order; the published \
             code carries the docs URL (the oracle's `documentation: ?url` face), the \
             unpublished `expected_token` row above omits the key"
        );

        fs::remove_dir_all(&root).ok();
    }

    /// The JSON face over a context-carrying single-file target (#2164, D1):
    /// the envelope's `contextMessages` resolve through the merged layout
    /// provider too, so a diagnostic whose context anchor carries a raw
    /// engine-table file id (not the merged union's id) used to panic
    /// mid-stream — rc 4 with a truncated, unparseable envelope (the
    /// full-corpus shape: a colliding id silently resolved a DIFFERENT
    /// file's line/column). The clean sibling forces the engine's id for
    /// the analyzed file apart from the merged table's, and the file target
    /// keeps the merged table minimal — the exact divergence shape.
    #[test]
    fn analyze_json_face_resolves_context_messages_through_the_merged_table() {
        let root = analyze_fixture(
            "json-context",
            &[
                ("lib/aaa_clean.dart", "int clean = 0;\n"),
                ("lib/bbb_broken.dart", "int x = 0;\nint x = 1;\n"),
            ],
        );

        let mut args = analyze_args(&root, AnalyzeFormat::Json, None);
        args.targets = vec![root.join("lib/bbb_broken.dart")];
        let mut out: Vec<u8> = Vec::new();
        let code = analyze_run(&args, &mut out);
        assert_eq!(code, exit::ERRORS, "the ERROR-tier duplicate reports");
        let text = String::from_utf8(out).expect("UTF-8");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 1, "one line: the envelope only");
        let envelope: serde_json::Value = serde_json::from_str(lines[0])
            .expect("a complete envelope: no panic truncated the stream");
        let rows = envelope["diagnostics"].as_array().expect("rows");
        let duplicate = rows
            .iter()
            .find(|row| row["code"] == "duplicate_definition")
            .expect("the duplicate_definition row");
        let file = root
            .canonicalize()
            .expect("canonical root")
            .join("lib/bbb_broken.dart");
        let file = file.display().to_string();
        assert_eq!(
            duplicate["location"]["file"], file,
            "the row's own location resolves through the merged table"
        );
        let context = duplicate["contextMessages"].as_array().expect("context");
        assert_eq!(context.len(), 1, "one anchor: the first definition");
        assert_eq!(
            context[0]["location"]["file"], file,
            "the context anchor resolves to the same canonical file — a raw \
             engine id here is the #2164 corruption"
        );
        assert_eq!(
            context[0]["location"]["range"]["start"]["line"], 1,
            "the anchor is the first definition's name"
        );
        assert_eq!(
            context[0]["message"], "The first definition of this name.",
            "the oracle's context wording"
        );

        fs::remove_dir_all(&root).ok();
    }

    /// The degrade notice's machine-format invariant (#1311): a skewed SDK
    /// whitelist surfaces the notice on stderr only — the machine rows and
    /// the JSON envelope (the parity-caliber stdout faces) carry no trace of
    /// it, and the exit tier ignores the telemetry. The once-per-skew /
    /// zero-when-clean notice contract itself is pinned at the shared core
    /// (`AnalyzeRun.degrade_notices`, dartq-commands' analyze tests).
    #[test]
    fn analyze_degrade_notice_stays_off_the_machine_and_json_faces() {
        let root = analyze_fixture("degrade", &[("lib/ok.dart", "var ok = 1;\n")]);
        fs::write(
            root.join("sdk/lib/_internal/allowed_experiments.json"),
            r#"{"version": 1, "experimentSets": {"sdkExperiments": ["future-flag"]}, "sdk": {"default": {"experimentSet": "sdkExperiments"}}, "packages": {"fixture": {"experimentSet": "sdkExperiments"}}}"#,
        )
        .expect("skewed whitelist");

        let mut out: Vec<u8> = Vec::new();
        assert_eq!(
            analyze_run(&analyze_args(&root, AnalyzeFormat::Machine, None), &mut out),
            exit::CLEAN,
            "the telemetry never joins the exit tier"
        );
        let machine = String::from_utf8(out).expect("UTF-8");
        assert!(
            !machine.contains("note: SDK") && !machine.contains("ExperimentDegrade"),
            "the machine face stays notice-free: {machine}"
        );

        let mut out: Vec<u8> = Vec::new();
        assert_eq!(
            analyze_run(&analyze_args(&root, AnalyzeFormat::Json, None), &mut out),
            exit::CLEAN
        );
        let envelope: serde_json::Value =
            serde_json::from_str(&String::from_utf8(out).expect("UTF-8"))
                .expect("one envelope: no notice line joined the stdout stream");
        let text = envelope.to_string();
        assert!(
            !text.contains("note: SDK") && !text.contains("ExperimentDegrade"),
            "the JSON envelope stays notice-free: {text}"
        );

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
        let code = analyze_run(&analyze_args(&root, AnalyzeFormat::Default, None), &mut out);
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
            &analyze_args(&broken_root, AnalyzeFormat::Default, None),
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

    /// The stale-config advisory face (#732): a stale fixture surfaces
    /// exactly ONE advisory ahead of the diagnostics — on the machine face
    /// the row rides stderr in the exact 8-field `dartq/*` shape (INFO|TOOL,
    /// anchored at the config the reasons name), on the default-format face
    /// the line leads stdout in the face's own grammar — and the advisory
    /// never joins the per-file set, the metrics count, or the exit tier.
    /// The fresh side of the contract is pinned by the byte-exact
    /// faces above (`analyze_clean_and_human_faces` — no advisory row, no
    /// extra human line).
    #[test]
    fn analyze_surfaces_stale_config_advisory_once() {
        let message = "package_config.json is out of date; run `dart pub get` \
                       (ghost's directory from package_config.json is missing)";
        // The stale trigger: a config entry whose package directory has no
        // pubspec — a freshness face the in-run content baseline cannot
        // excuse, unlike an mtime re-stamp.
        let stale_config = |root: &Path| {
            fs::write(
                root.join(".dart_tool/package_config.json"),
                r#"{"configVersion": 2, "packages": [{"name": "fixture", "rootUri": "../", "languageVersion": "3.5"}, {"name": "ghost", "rootUri": "../ghost", "languageVersion": "3.5"}], "generator": "pub", "generatorVersion": "3.13.1"}"#,
            )
            .expect("stale config");
        };

        // Machine: the advisory row rides stderr (the oracle machine stream
        // carries diagnostics only), so stdout carries the oracle rows
        // alone; the row's own bytes pin through the shared writer against
        // the assembly's advisory.
        let root = analyze_fixture("stale-machine", &[("lib/broken.dart", "void f() { x }\n")]);
        stale_config(&root);
        let canonical = root.canonicalize().expect("root");
        let config = canonical.join(".dart_tool/package_config.json");
        let broken = canonical.join("lib/broken.dart");
        let mut out: Vec<u8> = Vec::new();
        let code = analyze_run(&analyze_args(&root, AnalyzeFormat::Machine, None), &mut out);
        assert_eq!(
            String::from_utf8(out).expect("stdout is UTF-8"),
            format!(
                "ERROR|SYNTACTIC_ERROR|EXPECTED_TOKEN|{}|1|12|1|Expected to find ';'.\n\
                 ERROR|COMPILE_TIME_ERROR|UNDEFINED_IDENTIFIER|{}|1|12|1|Undefined name 'x'.\n",
                broken.display(),
                broken.display()
            ),
            "the oracle bytes only: the advisory never joins the machine stream"
        );
        assert_eq!(code, exit::ERRORS, "the advisory never moves the exit tier");

        let (_engine, _dir, advisory) = open_engine_with_overrides(
            &root,
            Some(root.join("sdk")).as_deref(),
            &AnalyzeOverrides::default(),
        )
        .expect("assembly surfaces the advisory");
        let Some(advisory) = advisory else {
            panic!("the stale fixture assembles an advisory")
        };
        let mut err: Vec<u8> = Vec::new();
        write_stale_advisory_machine(
            &mut err,
            Some(&advisory),
            DiagnosticVerbosity::Default.into_commands(),
        )
        .expect("the stderr row writes");
        assert_eq!(
            String::from_utf8(err).expect("stderr row is UTF-8"),
            format!(
                "INFO|TOOL|DARTQ/PACKAGE_CONFIG_OUT_OF_DATE|{}|1|1|0|{message}\n",
                config.display()
            ),
            "one advisory row in the exact 8-field shape, on the stderr stream"
        );

        // Default format: the advisory line leads, the issues tail counts
        // rows only.
        let root = analyze_fixture("stale-human", &[("lib/broken.dart", "void f() { x }\n")]);
        stale_config(&root);
        let mut out: Vec<u8> = Vec::new();
        let code = analyze_run(&analyze_args(&root, AnalyzeFormat::Default, None), &mut out);
        assert_eq!(code, exit::ERRORS);
        let text = String::from_utf8(out).expect("UTF-8");
        let mut lines = text.lines();
        assert_eq!(
            lines.next().expect("first line"),
            format!("Info • dartq/package_config_out_of_date • {message}"),
            "the advisory line leads"
        );
        assert!(
            !lines.any(|line| line.contains("package_config_out_of_date")),
            "exactly one advisory line: {text}"
        );
        assert!(
            text.contains("2 issues found."),
            "the advisory is uncounted by the tail: {text}"
        );

        fs::remove_dir_all(&root).ok();
    }

    /// Usage tier: missing dir and unusable SDK exit 64 with stderr-only
    /// output (dartdev `EX_USAGE`, dartdev.dart:209–210).
    #[test]
    fn analyze_usage_tiers() {
        let root = analyze_fixture("usage", &[("lib/clean.dart", "var ok = 1;\n")]);

        let missing_dir = analyze_args(&root.join("nope"), AnalyzeFormat::Machine, None);
        assert_eq!(analyze_run(&missing_dir, &mut Vec::new()), exit::USAGE);

        let no_sdk = AnalyzeArgs {
            sdk_path: Some(root.join("no-such-sdk")),
            ..analyze_args(&root, AnalyzeFormat::Machine, None)
        };
        assert_eq!(analyze_run(&no_sdk, &mut Vec::new()), exit::USAGE);

        fs::remove_dir_all(&root).ok();
    }

    /// The multi-target face end to end: mixed `dir + file` positionals
    /// resolve to the one workspace (both share the fixture root), the
    /// emitted set covers the directory target's subtree plus the file
    /// target exactly, and a missing argument exits 64 before any analysis
    /// — the oracle's `usageException` wording rides stderr
    /// (`resolve_targets` reports the first missing path in argv order).
    #[test]
    fn analyze_multi_target_union_and_missing_target() {
        let root = analyze_fixture(
            "multi",
            &[
                ("lib/broken_a.dart", "void f() { x }\n"),
                ("lib/broken_b.dart", "void g() { y }\n"),
                ("test/broken_t.dart", "void h() { z }\n"),
            ],
        );
        let canonical = root.canonicalize().expect("root");
        let lib = canonical.join("lib");
        let test_file = canonical.join("test/broken_t.dart");

        // resolve_targets: classification + canonicalization + the missing
        // path (argv order preserved on the error).
        let resolved = resolve_targets(&[lib.clone(), test_file.clone()])
            .expect("existing dir + file resolve");
        assert!(
            matches!(resolved[0], TargetKind::Dir(ref d) if *d == lib)
                && matches!(resolved[1], TargetKind::File(ref f) if *f == test_file),
            "dir → Dir target, file → File target"
        );
        let missing = resolve_targets(&[lib.clone(), root.join("gone"), test_file.clone()]);
        assert_eq!(
            missing.expect_err("missing arg"),
            root.join("gone"),
            "the first missing argument reports, in argv order"
        );

        // The union: `lib/` covers a+b, `test/broken_t.dart` adds t — all
        // three files' rows emit, oracle-ordered, one exit tier.
        let args = AnalyzeArgs {
            targets: vec![lib.clone(), test_file.clone()],
            ..analyze_args(&root, AnalyzeFormat::Machine, None)
        };
        let mut out: Vec<u8> = Vec::new();
        let code = analyze_run(&args, &mut out);
        assert_eq!(code, exit::ERRORS, "any ERROR → 3");
        let text = String::from_utf8(out).expect("UTF-8");
        for name in ["broken_a.dart", "broken_b.dart", "broken_t.dart"] {
            assert!(
                text.lines().any(|row| row.contains(name)),
                "the union covers {name}: {text}"
            );
        }

        // A lone file target emits that file's rows only — the siblings
        // stay analyzed (resolution context) but never emit (the oracle's
        // file context root).
        let args = AnalyzeArgs {
            targets: vec![test_file.clone()],
            ..analyze_args(&root, AnalyzeFormat::Machine, None)
        };
        let mut out: Vec<u8> = Vec::new();
        let code = analyze_run(&args, &mut out);
        assert_eq!(code, exit::ERRORS);
        let text = String::from_utf8(out).expect("UTF-8");
        assert!(
            text.lines().all(|row| row.contains("broken_t.dart")) && !text.is_empty(),
            "the file target emits its rows alone: {text}"
        );

        fs::remove_dir_all(&root).ok();
    }

    /// The `--memory` JSON face (analyze.dart:200, :587): the envelope
    /// gains a `memory` key — peak RSS in KiB — after `diagnostics`, only
    /// under `--format=json`; the spliced line keeps the shared writer's
    /// byte shape and stays valid single-line JSON.
    #[test]
    fn analyze_memory_flag_adds_the_oracle_json_key() {
        let root = analyze_fixture("memory", &[("lib/broken.dart", "void f() { x }\n")]);

        let args = AnalyzeArgs {
            memory: true,
            ..analyze_args(&root, AnalyzeFormat::Json, None)
        };
        let mut out: Vec<u8> = Vec::new();
        assert_eq!(analyze_run(&args, &mut out), exit::ERRORS);
        let text = String::from_utf8(out).expect("UTF-8");
        let line = text.trim_end_matches('\n');
        let envelope: serde_json::Value = serde_json::from_str(line).expect("one JSON object");
        assert_eq!(envelope["version"], 1);
        assert!(envelope["diagnostics"].is_array());
        assert!(
            envelope["memory"].is_u64(),
            "the oracle memory key, KiB integer: {line}"
        );
        assert!(
            line.ends_with("}"),
            "the memory key splices before the envelope's close: {line}"
        );

        // The flag is inert off the JSON face (the oracle's printMemory
        // gate): the machine stream stays byte-identical rows.
        let args = AnalyzeArgs {
            memory: true,
            ..analyze_args(&root, AnalyzeFormat::Machine, None)
        };
        let mut out: Vec<u8> = Vec::new();
        assert_eq!(analyze_run(&args, &mut out), exit::ERRORS);
        let machine = String::from_utf8(out).expect("UTF-8");
        assert!(
            !machine.contains("\"memory\":"),
            "the machine face never carries the memory key: {machine}"
        );

        fs::remove_dir_all(&root).ok();
    }

    /// `--overlay` refuses the ambiguous multi-root shape (the dartq
    /// extension has no oracle counterpart): targets spanning two distinct
    /// workspaces plus an overlay spec exit 64 rather than guessing the
    /// resolution base.
    #[test]
    fn analyze_overlay_refuses_multi_root_targets() {
        let root_a = analyze_fixture("overlay_a", &[("lib/a.dart", "var a = 1;\n")]);
        let root_b = analyze_fixture("overlay_b", &[("lib/b.dart", "var b = 1;\n")]);

        let args = AnalyzeArgs {
            targets: vec![root_a.clone(), root_b.clone()],
            overlays: vec!["lib/a.dart=/tmp/x.dart".to_owned()],
            ..analyze_args(&root_a, AnalyzeFormat::Machine, None)
        };
        assert_eq!(
            analyze_run(&args, &mut Vec::new()),
            exit::USAGE,
            "multi-root + overlay is a usage refusal"
        );

        fs::remove_dir_all(&root_a).ok();
        fs::remove_dir_all(&root_b).ok();
    }

    /// No `package_config.json` no longer refuses (#759): the run degrades
    /// to the oracle's `BasicWorkspace` floor — the missing-config advisory
    /// rides stderr (the machine stream keeps the oracle rows alone), and
    /// the `package:` import resolves to nothing, producing the same
    /// `uri_does_not_exist` ERROR row the oracle's degraded mode
    /// emits for unknown packages (its no-config runs measure exactly that
    /// face plus the cascades). The advisory still never moves the exit
    /// tier (the row's own ERROR does).
    #[test]
    fn analyze_without_config_degrades_with_missing_config_advisory() {
        let root = analyze_fixture(
            "no-config",
            &[(
                "lib/main.dart",
                "import 'package:missing/missing.dart';\nvar t = Missing;\n",
            )],
        );
        fs::remove_file(root.join(".dart_tool/package_config.json")).ok();
        let canonical = root.canonicalize().expect("root");
        let config = canonical.join(".dart_tool/package_config.json");
        let main = canonical.join("lib/main.dart");

        let mut out: Vec<u8> = Vec::new();
        let code = analyze_run(&analyze_args(&root, AnalyzeFormat::Machine, None), &mut out);
        let text = String::from_utf8(out).expect("stdout is UTF-8");
        assert!(
            !text.contains("INFO|TOOL|DARTQ/"),
            "the advisory never joins the machine stdout: {text}"
        );
        assert!(
            text.lines().any(|line| line.contains("URI_DOES_NOT_EXIST")
                && line.contains(&main.display().to_string())),
            "the package: import is unresolved, the oracle's degraded face: {text}"
        );
        assert_eq!(code, exit::ERRORS);

        // The stderr row's own bytes, pinned through the shared writer
        // against the assembly's advisory.
        let (_engine, _dir, advisory) = open_engine_with_overrides(
            &root,
            Some(root.join("sdk")).as_deref(),
            &AnalyzeOverrides::default(),
        )
        .expect("assembly surfaces the advisory");
        let Some(advisory) = advisory else {
            panic!("the missing-config fixture assembles an advisory")
        };
        let mut err: Vec<u8> = Vec::new();
        write_stale_advisory_machine(
            &mut err,
            Some(&advisory),
            DiagnosticVerbosity::Default.into_commands(),
        )
        .expect("the stderr row writes");
        assert_eq!(
            String::from_utf8(err).expect("stderr row is UTF-8"),
            format!(
                "INFO|TOOL|DARTQ/PACKAGE_CONFIG_MISSING|{}|1|1|0|package_config.json not \
                 found; run `dart pub get` (analyzing with `package:` imports unresolved)\n",
                config.display()
            ),
            "the missing-config advisory row, on the stderr stream"
        );

        fs::remove_dir_all(&root).ok();
    }

    /// A bare `.dart` file target — no `pubspec.yaml`, no
    /// `package_config.json`, the workspace walk's `NoRoot` floor at the
    /// file's own directory — still analyzes in the no-package context: the
    /// oracle's `dart analyze <file>` face emits the file's own semantic
    /// rows (SDK types resolve, `package:` URIs would degrade to
    /// `URI_DOES_NOT_EXIST`) rather than an empty stream. Pin the machine
    /// stdout's non-empty row set against regression to a zero-row run.
    #[test]
    fn analyze_bare_file_target_emits_its_own_rows() {
        let root = analyze_fixture("bare-file", &[("bad.dart", "int x = \"s\";\n")]);
        // Strip every package marker: the target's directory is the bare
        // floor, not a malformed package.
        fs::remove_file(root.join("pubspec.yaml")).ok();
        fs::remove_file(root.join("pubspec.lock")).ok();
        fs::remove_dir_all(root.join(".dart_tool")).ok();
        let file = root.canonicalize().expect("root").join("bad.dart");

        let args = AnalyzeArgs {
            targets: vec![file.clone()],
            ..analyze_args(&root, AnalyzeFormat::Machine, None)
        };
        let mut out: Vec<u8> = Vec::new();
        let code = analyze_run(&args, &mut out);
        assert_eq!(code, exit::ERRORS, "the file's own ERROR rows → 3");
        let text = String::from_utf8(out).expect("UTF-8");
        assert!(
            text.lines().any(|row| row.contains("INVALID_ASSIGNMENT")
                && row.contains(&file.display().to_string())),
            "the bare file's own diagnostic emits, never a zero-row run: {text}"
        );
        assert!(
            text.lines().all(|row| row.contains("bad.dart")),
            "no sibling or workspace noise joins the file target's set: {text}"
        );

        fs::remove_dir_all(&root).ok();
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

    /// The `--overlay` face: the analyzed file's content is
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

        // A not-yet-existing overlay target (the agent-introduced file)
        // resolves against the package dir too: the rows anchor at the
        // canonical dir-joined identity. The retired fallback kept the raw
        // `PATH` spec — a cwd-bound phantom the engine interned as a file
        // discovery never saw. The overlay substitutes only `lib/new.dart`'s
        // content; the on-disk `lib/broken.dart` stays broken (asserted
        // above) and stays in the analyzed set, so its rows lead the oracle
        // order (path bytes: `broken.dart` < `new.dart`; within one file the
        // shared-span rows order by the message tie-break).
        let mut out: Vec<u8> = Vec::new();
        let introduced = AnalyzeArgs {
            overlays: vec![format!("lib/new.dart={}", broken.display())],
            ..analyze_args(&root, AnalyzeFormat::Machine, None)
        };
        assert_eq!(analyze_run(&introduced, &mut out), exit::ERRORS);
        let canonical = root.canonicalize().expect("root");
        let new_file = canonical.join("lib/new.dart");
        let base_file = canonical.join("lib/broken.dart");
        assert_eq!(
            String::from_utf8(out).expect("stdout is UTF-8"),
            format!(
                "ERROR|SYNTACTIC_ERROR|EXPECTED_TOKEN|{}|1|12|1|Expected to find ';'.\n\
                 ERROR|COMPILE_TIME_ERROR|UNDEFINED_IDENTIFIER|{}|1|12|1|Undefined name 'x'.\n\
                 ERROR|SYNTACTIC_ERROR|EXPECTED_TOKEN|{}|1|12|1|Expected to find ';'.\n\
                 ERROR|COMPILE_TIME_ERROR|UNDEFINED_IDENTIFIER|{}|1|12|1|Undefined name 'y'.\n",
                base_file.display(),
                base_file.display(),
                new_file.display(),
                new_file.display()
            ),
            "the introduced file's rows anchor at the canonical dir-joined path"
        );

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
        assert!(
            matches!(symbol.verbosity, Verbosity::Compact),
            "compact default"
        );
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

    /// The query faces' `-v`/`--verbosity` tier flag (the explain face's
    /// spelling, G6/G9): every single face parses the three tiers through
    /// long, `=` and short spellings, the unset default is `compact`, and
    /// the batch face carries the same axis.
    #[test]
    fn query_verbosity_tier_parse() {
        let parse_symbol = |args: &[&str]| {
            let parsed = parse_args_of(args).expect("parse").expect("subcommand");
            let Command::Query(args) = parsed else {
                panic!("expected query command")
            };
            // The single faces share the `SymbolArgs` grammar, so the
            // references spelling's parsed flag set reads through the same
            // binding.
            match args.face {
                QueryFace::Symbol(symbol) | QueryFace::References(symbol) => symbol,
                _ => panic!("expected symbol face"),
            }
        };

        assert!(matches!(
            parse_symbol(&["query", "symbol", "Foo"]).verbosity,
            Verbosity::Compact
        ));
        assert!(matches!(
            parse_symbol(&["query", "symbol", "-v", "minimal", "Foo"]).verbosity,
            Verbosity::Minimal
        ));
        assert!(matches!(
            parse_symbol(&["query", "symbol", "--verbosity=full", "Foo"]).verbosity,
            Verbosity::Full
        ));
        // The flag rides alongside the other options, any argument position.
        assert!(matches!(
            parse_symbol(&[
                "query",
                "references",
                "--format=json",
                "--verbosity",
                "minimal",
                "Foo",
                "/pkg"
            ])
            .verbosity,
            Verbosity::Minimal
        ));

        let parsed = parse_args_of(&["query", "batch", "--symbol", "Foo", "-v", "minimal"])
            .expect("parse")
            .expect("subcommand");
        let Command::Query(args) = parsed else {
            panic!("expected query command")
        };
        let QueryFace::Batch(batch) = args.face else {
            panic!("expected batch face")
        };
        assert!(matches!(batch.verbosity, Verbosity::Minimal));
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

        // The implementations/dependencies faces share the same grammar,
        // and the batch `--kind` set parses both spellings through the one
        // `QueryKind: FromStr` surface.
        let parsed = parse_args_of(&["query", "implementations", "Foo"])
            .expect("parse")
            .expect("subcommand");
        let Command::Query(args) = parsed else {
            panic!("expected query command")
        };
        let QueryFace::Implementations(symbol) = args.face else {
            panic!("expected implementations face")
        };
        assert_eq!(symbol.name, "Foo");
        assert_eq!(symbol.dir, PathBuf::from("."), "default target cwd");

        let parsed = parse_args_of(&["query", "dependencies", "Foo"])
            .expect("parse")
            .expect("subcommand");
        let Command::Query(args) = parsed else {
            panic!("expected query command")
        };
        let QueryFace::Dependencies(symbol) = args.face else {
            panic!("expected dependencies face")
        };
        assert_eq!(symbol.name, "Foo");

        let parsed = parse_args_of(&[
            "query",
            "batch",
            "--symbol",
            "Animal",
            "--kinds",
            "implementations",
            "--kinds",
            "dependencies",
        ])
        .expect("parse")
        .expect("subcommand");
        let Command::Query(args) = parsed else {
            panic!("expected query command")
        };
        let QueryFace::Batch(batch) = args.face else {
            panic!("expected batch face")
        };
        assert_eq!(batch.symbols, vec!["Animal".to_owned()]);
        assert_eq!(
            batch.kinds,
            vec![QueryKind::Implementations, QueryKind::Dependencies]
        );
    }

    fn symbol_args(root: &Path, format: QueryFormat, name: &str) -> SymbolArgs {
        SymbolArgs {
            name: name.to_string(),
            dir: root.to_path_buf(),
            format,
            verbosity: Verbosity::Compact,
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
        // c.dart's `p.Foo()` arrives through the armed `p` prefix. The DECL
        // rows spell the machine grammar's trailing extras (`-` — the plain
        // fixture carries no signature face and no annotations).
        let mut out: Vec<u8> = Vec::new();
        let code = query_symbol_run(&symbol_args(&root, QueryFormat::Machine, "Foo"), &mut out);
        assert_eq!(code, exit::CLEAN);
        assert_eq!(
            String::from_utf8(out).expect("UTF-8"),
            format!(
                "DECL|{}|-|class|Foo|1|7|3|-|-\n\
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

        // A member hit: the container name surfaces, and the signature
        // column spells the element face (the rendered member signature);
        // no uses → no REF rows, still exit 0.
        let mut out: Vec<u8> = Vec::new();
        let code = query_symbol_run(&symbol_args(&root, QueryFormat::Machine, "meth"), &mut out);
        assert_eq!(code, exit::CLEAN);
        assert_eq!(
            String::from_utf8(out).expect("UTF-8"),
            format!("DECL|{}|Foo|method|meth|2|8|4|void meth()|-\n", a.display())
        );

        // JSON face: one object, snake_case fields mirroring the rows. The
        // file labels are the stable package-form references (#1556's
        // grammar: lib files project, the machine face keeps canonical).
        let mut out: Vec<u8> = Vec::new();
        let code = query_symbol_run(&symbol_args(&root, QueryFormat::Json, "Foo"), &mut out);
        assert_eq!(code, exit::CLEAN);
        let json: serde_json::Value =
            serde_json::from_str(&String::from_utf8(out).expect("UTF-8")).expect("one object");
        assert_eq!(json["name"], "Foo");
        assert_eq!(json["declarations"][0]["kind"], "class");
        assert_eq!(json["declarations"][0]["name"], "Foo");
        assert!(json["declarations"][0]["container"].is_null());
        assert_eq!(json["declarations"][0]["file"], "package:fixture/a.dart");
        assert_eq!(json["declarations"][0]["line"], 1);
        assert_eq!(json["declarations"][0]["column"], 7);
        assert_eq!(json["references"].as_array().expect("array").len(), 3);
        assert_eq!(json["references"][0]["kind"], "reference");
        assert_eq!(
            json["references"][0]["target_file"],
            "package:fixture/a.dart"
        );
        assert_eq!(json["references"][0]["file"], "package:fixture/b.dart");
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

        // The minimal tier: the machine rows keep the red line only — the
        // container and extras columns drop from DECL, `explicit` drops
        // from REF (the qualified spelling's machine twin) — and the JSON
        // envelope trims to the locators (the canonical tables' query
        // rows).
        let mut out: Vec<u8> = Vec::new();
        let args = SymbolArgs {
            verbosity: Verbosity::Minimal,
            ..symbol_args(&root, QueryFormat::Machine, "Foo")
        };
        assert_eq!(query_symbol_run(&args, &mut out), exit::CLEAN);
        assert_eq!(
            String::from_utf8(out).expect("UTF-8"),
            format!(
                "DECL|{}|class|Foo|1|7|3\n\
                 REF|{}|reference|33|3|{}\n\
                 REF|{}|invoked|42|3|{}\n\
                 REF|{}|invoked|43|3|{}\n",
                a.display(),
                a.display(),
                b.display(),
                a.display(),
                b.display(),
                a.display(),
                c.display()
            )
        );
        let mut out: Vec<u8> = Vec::new();
        let args = SymbolArgs {
            verbosity: Verbosity::Minimal,
            ..symbol_args(&root, QueryFormat::Json, "Foo")
        };
        assert_eq!(query_symbol_run(&args, &mut out), exit::CLEAN);
        let json: serde_json::Value =
            serde_json::from_str(&String::from_utf8(out).expect("UTF-8")).expect("one object");
        let decl = &json["declarations"][0];
        assert_eq!(decl["file"], "package:fixture/a.dart", "the locator stays");
        assert!(decl.get("container").is_none(), "the convenience drops");
        assert!(
            json["references"]
                .as_array()
                .expect("array")
                .iter()
                .all(|row| row.get("qualified").is_none() && row.get("file").is_none()),
            "minimal reference rows keep target/kind/offset/length only"
        );

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
                "DECL|{}|-|class|Foo|1|7|3|-|-\nREF|{}|invoked|false|41|3|{}\n",
                a.display(),
                a.display(),
                b.display()
            )
        );

        // callers foo: the one call site. The top-level function carries
        // its own signature face in the trailing column.
        let mut out: Vec<u8> = Vec::new();
        assert_eq!(
            query_callers_run(&symbol_args(&root, QueryFormat::Machine, "foo"), &mut out),
            exit::CLEAN
        );
        assert_eq!(
            String::from_utf8(out).expect("UTF-8"),
            // The DECL grammar closes at the annotations column — 10 fields,
            // 9 separators, no trailing one; the one shared machine writer
            // serves every query face alike.
            format!(
                "DECL|{}|-|function|foo|4|6|3|void foo()|-\nREF|{}|invoked|false|50|3|{}\n",
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
            format!("DECL|{}|Foo|method|meth|2|8|4|void meth()|-\n", a.display())
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
        assert_eq!(refs[0]["target_file"], "package:fixture/a.dart");

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
        assert_eq!(args.symbol.as_deref(), Some("Dog"));
        assert!(args.diagnostic.is_none(), "the symbol face selects");
        assert!(args.file.is_none());
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
        assert_eq!(args.symbol.as_deref(), Some("Dog"));
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

    /// The diagnostic face's flag contract: `--diagnostic CODE` selects it,
    /// `--file PATH[:LINE[:COL]]` narrows the row, and the one positional
    /// slot carries the workspace `dir` on this face (a positional is a
    /// dir, never a symbol, once `--diagnostic` is set).
    #[test]
    fn explain_diagnostic_args_parse() {
        let parsed = parse_args_of(&["explain", "--diagnostic", "unused_import"])
            .expect("parse")
            .expect("subcommand");
        let Command::Explain(args) = parsed else {
            panic!("expected explain command")
        };
        assert!(args.symbol.is_none(), "no positional given");
        assert_eq!(args.diagnostic.as_deref(), Some("unused_import"));
        assert!(args.file.is_none());
        assert_eq!(args.dir, PathBuf::from("."), "default target cwd");

        // The positional slot is the `[dir]` on the diagnostic face —
        // `explain --diagnostic CODE /pkg` parses with /pkg in the slot.
        let parsed = parse_args_of(&["explain", "--diagnostic", "unused_import", "/pkg"])
            .expect("parse")
            .expect("subcommand");
        let Command::Explain(args) = parsed else {
            panic!("expected explain command")
        };
        assert_eq!(args.symbol.as_deref(), Some("/pkg"), "the [dir] slot");

        let parsed = parse_args_of(&[
            "explain",
            "--diagnostic",
            "unused_import",
            "--file",
            "lib/a.dart:3:9",
        ])
        .expect("parse")
        .expect("subcommand");
        let Command::Explain(args) = parsed else {
            panic!("expected explain command")
        };
        assert_eq!(args.file.as_deref(), Some("lib/a.dart:3:9"));

        // `--file` without `--diagnostic` is a usage error (clap's
        // `requires` gate).
        assert!(parse_args_of(&["explain", "Dog", "--file", "a.dart"]).is_err());
        assert!(parse_args_of(&["explain", "--file", "a.dart"]).is_err());
    }

    /// `PATH[:LINE[:COL]]` splits right-to-left on numeric tails; a
    /// non-numeric tail stays in the path (drive letters, `:`-bearing
    /// paths), an empty path is the usage error.
    #[test]
    fn parse_file_selector_shapes() {
        let parse = |text: &str| parse_file_selector(text).expect("selector");

        let sel = parse("lib/a.dart");
        assert_eq!(sel.path, PathBuf::from("lib/a.dart"));
        assert_eq!((sel.line, sel.column), (None, None));

        let sel = parse("lib/a.dart:3");
        assert_eq!(sel.path, PathBuf::from("lib/a.dart"));
        assert_eq!((sel.line, sel.column), (Some(3), None));

        let sel = parse("lib/a.dart:3:9");
        assert_eq!(sel.path, PathBuf::from("lib/a.dart"));
        assert_eq!((sel.line, sel.column), (Some(3), Some(9)));

        // A non-numeric tail is not a position — it is part of the path.
        let sel = parse("weird:name.dart");
        assert_eq!(sel.path, PathBuf::from("weird:name.dart"));
        assert_eq!((sel.line, sel.column), (None, None));

        assert!(parse_file_selector(":3").is_err(), "empty path");
        // A trailing non-numeric tail folds into the path — only digits
        // count as the position suffix.
        let sel = parse("lib/a.dart:3:x");
        assert_eq!(sel.path, PathBuf::from("lib/a.dart:3:x"));
        assert_eq!((sel.line, sel.column), (None, None));
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
    #[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
    fn explain_offset(source: &str, needle: &str) -> u32 {
        source
            .find(needle)
            .unwrap_or_else(|| panic!("{needle} in source")) as u32
    }

    fn explain_args(root: &Path, verbosity: Verbosity, symbol: &str) -> ExplainArgs {
        ExplainArgs {
            symbol: Some(symbol.to_owned()),
            diagnostic: None,
            file: None,
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
        // The answer's file labels are the stable package-form references
        // (#1556's grammar): every `lib/` member projects to
        // `package:fixture/<rel>`; the non-lib test file keeps the
        // canonical path — the identity fallback no `package:` URI
        // addresses.
        let canonical = root.canonicalize().expect("root");
        let animal = "package:fixture/animal.dart";
        let dog = "package:fixture/dog.dart";
        let main = "package:fixture/main.dart";
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
            serde_json::json!([{ "name": "Animal", "file": animal, "clause": "extends" }])
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
        // Row order keys on the projected label (`package:fixture/main.dart`
        // vs the test tree's canonical path): `/` sorts before `p`, the same
        // posture the impact face pins over this fixture shape.
        assert_eq!(references[0]["file"], dog_test);
        assert_eq!(references[0]["kind"], "invoked");
        assert_eq!(references[1]["file"], main);
        assert_eq!(
            json["dependencies"],
            serde_json::json!([{ "path": dog_test }, { "path": main }])
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

    /// The diagnostic face end to end: `--diagnostic` selects the row, the
    /// report is schema-valid at its producing tier, `related` counts the
    /// run's same-code rows, and a code the run carries no row for is the
    /// usage tier (`NoSuchDiagnostic`).
    #[test]
    fn explain_diagnostic_end_to_end() {
        // One file with two unused imports — the `related` face can count
        // the same-code rows.
        const UNUSED: &str = "import 'dart:async';\nimport 'dart:io';\nvoid main() {}\n";
        let root = analyze_fixture("explain_diag", &[("lib/unused.dart", UNUSED)]);
        let canonical = root.canonicalize().expect("root");
        let file = canonical.join("lib/unused.dart").display().to_string();

        let args = ExplainArgs {
            symbol: None,
            diagnostic: Some("unused_import".to_owned()),
            file: None,
            dir: root.to_path_buf(),
            format: ExplainFormat::Json,
            sdk: Some(root.join("sdk")),
            verbosity: Verbosity::Compact,
        };
        let mut out: Vec<u8> = Vec::new();
        assert_eq!(explain_run(&args, &mut out), exit::CLEAN);
        let json: serde_json::Value =
            serde_json::from_str(&String::from_utf8(out).expect("UTF-8")).expect("one object");
        // The report is born schema-valid inside `explain_diagnostic`
        // (the producing tier rides the gate); the assertions pin the
        // sections the tier promises.
        assert_eq!(json["schema_version"], 1);
        assert_eq!(json["diagnostic"]["code"], "unused_import");
        assert_eq!(json["diagnostic"]["file"], file);
        assert_eq!(json["why"]["trigger_kind"], "unused-family");
        assert_eq!(json["related"]["total"], 2, "two unused imports in the run");
        assert!(
            json["related"].get("lines").is_none(),
            "lines join at full only"
        );

        // Full: `fixes` and `related.lines` join.
        let args = ExplainArgs {
            verbosity: Verbosity::Full,
            ..args
        };
        let mut out: Vec<u8> = Vec::new();
        assert_eq!(explain_run(&args, &mut out), exit::CLEAN);
        let json: serde_json::Value =
            serde_json::from_str(&String::from_utf8(out).expect("UTF-8")).expect("one object");
        assert!(json.get("fixes").is_some(), "fixes join at full");
        assert!(json["related"].get("lines").is_some(), "lines join at full");

        // No row for the code → usage tier.
        let args = ExplainArgs {
            diagnostic: Some("no_such_code".to_owned()),
            ..args
        };
        assert_eq!(explain_run(&args, &mut Vec::new()), exit::USAGE);

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

    /// The daemon's `--watch` flag: parseable, off by
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

    /// The daemon face's `--context` flag (the compat face's convention):
    /// repeatable labels alongside the untouched positional, `--sdk` and
    /// `--watch`, defaulting to empty (= every declared context).
    #[test]
    fn daemon_context_flag_parse() {
        let parsed = parse_args_of(&["daemon", "/pkg"])
            .expect("parse")
            .expect("subcommand");
        let Command::Daemon(args) = parsed else {
            panic!("expected daemon command")
        };
        assert!(args.contexts.is_empty(), "no labels by default");

        let parsed = parse_args_of(&[
            "daemon",
            "--context",
            "a",
            "--context",
            "b",
            "--sdk",
            "/s",
            "--watch",
            "/pkg",
        ])
        .expect("parse")
        .expect("subcommand");
        let Command::Daemon(args) = parsed else {
            panic!("expected daemon command")
        };
        assert_eq!(args.contexts, vec!["a".to_owned(), "b".to_owned()]);
        assert_eq!(args.sdk, Some(PathBuf::from("/s")));
        assert_eq!(args.dir, PathBuf::from("/pkg"));
        assert!(args.watch);
    }

    /// The daemon face's engine runtime flags (#1088): the parse shapes mirror
    /// the analyze face — a bare MiB budget, the size grammar through the
    /// engine's `parse_size_limit`, a JSONL sink path — and every flag
    /// defaults to the daemon's standing posture (the 2048 MiB budget, an
    /// unarmed store, no sink) that `SessionRuntime::default` carries.
    #[test]
    fn daemon_runtime_flags_parse() {
        let parsed = parse_args_of(&["daemon", "/pkg"])
            .expect("parse")
            .expect("subcommand");
        let Command::Daemon(args) = parsed else {
            panic!("expected daemon command")
        };
        assert_eq!(args.memory_limit, None, "the standing budget by default");
        assert_eq!(args.cache_size_limit, None, "the store stays unarmed");
        assert_eq!(args.trace_events, None, "untraced by default");

        let parsed = parse_args_of(&[
            "daemon",
            "--memory-limit",
            "4096",
            "--cache-size-limit",
            "512MiB",
            "--trace-events",
            "/tmp/dartq-trace.jsonl",
            "/pkg",
        ])
        .expect("parse")
        .expect("subcommand");
        let Command::Daemon(args) = parsed else {
            panic!("expected daemon command")
        };
        assert_eq!(
            args.memory_limit,
            Some(4096),
            "--memory-limit MiB parses and plumbs"
        );
        assert_eq!(
            args.cache_size_limit,
            Some(512 * 1024 * 1024),
            "--cache-size-limit rides the engine's size grammar"
        );
        assert_eq!(
            args.trace_events,
            Some(PathBuf::from("/tmp/dartq-trace.jsonl")),
            "--trace-events parses and plumbs"
        );
        for text in ["10TiB", "1.5GiB", "huge"] {
            assert!(
                parse_args_of(&["daemon", "--cache-size-limit", text, "/pkg"]).is_err(),
                "`{text}` must be a usage error"
            );
        }
    }

    /// Writes a two-context dartq.yaml at the workspace root: `plain`
    /// carries no fork dimensions (its shape equals the default context's),
    /// `web` forks the target panel and declares a variable.
    fn write_daemon_context_config(root: &Path) {
        fs::write(
            root.join("dartq.yaml"),
            "version: 1\ncontexts:\n  plain:\n    sdk: path:sdk\n  web:\n    sdk: path:sdk\n    target: web\n    define: {HEAVY: \"1\"}\n",
        )
        .expect("config");
    }

    /// The shell-edge assembly: an absent config (or a config without
    /// contexts) assembles nothing; the declared labels select in file
    /// order with their fork dimensions carried into the configs; an
    /// unknown label and a `--context` without a config are the usage
    /// tier; a malformed file stays structured.
    #[test]
    fn daemon_context_assembly_selects_and_forks() {
        let root = analyze_fixture("daemon_ctx", &[]);
        let (base, _, _) =
            engine_config_with_advisory(&root, Some(&root.join("sdk"))).expect("assembly");

        // Absent config: nothing to serve, with or without labels (the
        // labeled case is the usage tier, the compat face's shape).
        let extras = assemble_contexts(&base, &[]).expect("no config, no labels");
        assert!(extras.is_empty());
        let Err(error) = assemble_contexts(&base, &["x".to_owned()]) else {
            panic!("--context without a config refuses")
        };
        assert!(
            matches!(error, ContextAssemblyError::Usage(ref message) if message.contains("no dartq.yaml")),
            "{error:?}"
        );

        // The declared pair assembles in file order; `plain` carries the
        // default's fork shape, `web` the target panel and the declared
        // variable. Each config arms the file-backed CAS store (the daemon
        // arming above) — the store face is opaque through the configs, and
        // sharing is pinned over the wire in the dartq-daemon integration
        // tests.
        write_daemon_context_config(&root);
        let extras = assemble_contexts(&base, &[]).expect("all declared");
        assert_eq!(
            extras
                .iter()
                .map(|(label, _)| label.as_str())
                .collect::<Vec<_>>(),
            vec!["plain", "web"],
            "file order, not sorted"
        );
        assert_eq!(extras[0].1.target, dartq_engine::DEFAULT_TARGET);
        assert!(extras[0].1.declared_variables.is_empty());
        assert_eq!(extras[1].1.target, "js", "web selects the js panel");
        assert_eq!(extras[1].1.declared_variables["HEAVY"], "1");

        // A label subset keeps only its contexts; an unknown label names
        // the declared set.
        let extras = assemble_contexts(&base, &["web".to_owned()]).expect("subset");
        assert_eq!(extras.len(), 1);
        assert_eq!(extras[0].0, "web");
        let Err(error) = assemble_contexts(&base, &["nope".to_owned()]) else {
            panic!("an unknown label refuses")
        };
        assert!(
            matches!(error, ContextAssemblyError::Usage(ref message) if message.contains("unknown context `nope` (declared: plain, web)")),
            "{error:?}"
        );

        // A malformed file is the same structured usage tier, never a
        // silent single-context fallback.
        fs::write(root.join("dartq.yaml"), "contexts: {a: [unclosed\n").expect("config");
        let Err(error) = assemble_contexts(&base, &[]) else {
            panic!("a malformed config refuses")
        };
        assert!(
            matches!(error, ContextAssemblyError::Usage(ref message) if message.contains("invalid dartq.yaml")),
            "{error:?}"
        );

        fs::remove_dir_all(&root).ok();
    }
}

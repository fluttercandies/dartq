//! dartq CLI — the converged verb surface: `analyze` (the official
//! analyzer by default, the in-process engine behind `--engine`,
//! `--engine --fast` the parser-only pass), `query`, `explain`, `impact`,
//! `fix`, `rename`, `test`, `packages` (the universe inventory), and the
//! position faces `resolve` / `complete`; `check`, `index`, `assists`,
//! and `daemon` stay reachable as hidden faces (`check` takes `analyze`'s
//! default, `check --engine` is `analyze --engine --fast`'s alias;
//! `compatibility`, `resolve-at`, and `completion` alias `compat`,
//! `resolve`, and `complete`).
//!
//! Output is the one `--format <lon|json|text>` axis on every verb with
//! output, `lon` the constant default (never the terminal's shape — the
//! same invocation prints the same bytes piped or on a TTY); `json` keeps
//! the frozen single-line shapes for jq consumers, `text` is the explicit
//! human face, and write operations fall back to `lon` on `text` (their
//! output is a record, not prose). Every face encodes the same value —
//! the producers' short snake_case word list, the one the daemon's
//! `dartq/*` answers carry natively (see [`output`]).
//!
//! The per-command cores live in `dartq-commands` (the one implementation
//! the daemon RPC faces share); this binary adds only argument
//! parsing, output formatting, and the exit tiers.
//!
//! `analyze` and `check` delegate to the official analyzer by default: a
//! `dart analyze --format machine` child whose rows re-emit through the
//! same writers and exit tiers. `Backend` is the one dispatch point.
//!
//! `check --engine` drives the M0 pipeline — bytes → scan →
//! `@dart` override rescan → parse, with all diagnostics written through
//! `dartq-diagnostics`' oracle-exact writers. It stops there by design: no
//! pub/package/SDK/engine involvement — the M0 differential face covers
//! scan + parse and nothing downstream.
//!
//! `analyze --engine` drives the full engine over a real package directory:
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
//! | Daemon RPC answers | `dartq/*` method results (dartq-daemon) | JSON-RPC 2.0 result carrying the short-vocabulary fields (`v` and friends); compact rows plus session economics (`symbols` table) | the CLI lon face's rows; validated against the schema crate's frozen documents |
//! | Schema-frozen shapes | never emitted — the contract itself (dartq-schema's per-domain validators, the CI gate) | one frozen v1 document per domain (Query, Explain, …) plus the Verbosity tier table | what the json-face emitters above must keep passing |
//!
//! The daemon face is the one deviation from the crash tier: it installs no
//! catch-unwind, so a panic on the main thread unwinds out of `main` and
//! the process exits with the Rust runtime's default 101 instead of 4. The
//! crash tier exists to keep a partial diagnostic set from masquerading as
//! a harness-valid {0,1,2,3} result; the daemon serves an RPC stream, not
//! a diagnostic set, and its reader/watch threads' panics are thread-local
//! regardless — the main loop observes the channel disconnect and ends the
//! session through the transport's own exit path.

use std::ffi::OsString;
use std::fs;
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use clap::{Args, Parser as _, Subcommand, ValueEnum};

use dartq_commands::analyze::Target;
use dartq_commands::analyze::official::{self, OfficialRun};
use dartq_commands::config::{
    ConfigError, ContextConfig, ContextTarget, DartqConfig,
    load as load_dartq_config, resolve_locator,
};
use dartq_commands::discover::{DiscoverError, discover};
use dartq_commands::explain::ExplainError;
use dartq_commands::query::{BatchQuery, QueryKind, to_json_at_tier};
use dartq_commands::workspace::{
    AnalyzeOverrides, WorkspaceAdvisory, engine_config_with_advisory,
    engine_config_with_overrides_phased, open_engine,
};
use dartq_diagnostics::{AnalysisOutput, Diagnostic, Severity, write_json};
use dartq_engine::EngineConfig;
use dartq_engine::{AnswerAim, AnswerFace, AnswerProbe, AnswerShape, AnswerStamp, AnswerSummary};
use dartq_sdk::SdkSnapshot;
use rustc_hash::FxHashMap;
#[cfg(unix)]
mod compat;
mod debug;
mod events;
mod fix;
mod impact;
mod index;
mod mcp;
mod output;
mod packages;
mod rename;
// The resident-session routing layer: the transport is the Unix socket
// (`session_call`/`read_frame` over `UnixStream`), and `dartq_session`'s
// election only attaches on unix — on other hosts every face degrades to
// the in-process path, so the whole module is unix-only.
#[cfg(unix)]
mod resident;
mod resolve;
mod session;
mod test;

pub(crate) use output::OutputFormat;
use output::{diags_document, write_human, write_json_value, write_lon, write_text_value};

/// Process exit codes, pinned to the dartdev oracle (analyze.dart:719–731;
/// usage exit dartdev.dart:209–210) plus the sysexits-style tiers the
/// session-layer faces (#2578+) reserve.
mod exit {
    pub const CLEAN: u8 = 0;
    pub const FATAL_INFOS: u8 = 1;
    pub const FATAL_WARNINGS: u8 = 2;
    pub const ERRORS: u8 = 3;
    pub const INTERNAL_ERROR: u8 = 4;
    pub const USAGE: u8 = 64;
    /// EX_DATAERR — a request payload the consumer could not parse
    /// (reserved for the session/events protocol faces).
    #[expect(
        dead_code,
        reason = "reserved contract tier; the first consumer wires it"
    )]
    pub const DATA_ERROR: u8 = 65;
    /// EX_SOFTWARE — an internal defect reported through a protocol
    /// channel (the one-shot faces keep exit 4).
    pub const SOFTWARE: u8 = 70;
    /// A domain failure the resident engine answered over the protocol —
    /// the debug face's data tier: one `{"error":{"code","msg"}}`
    /// line rides stdout (the `DARTQ/DEBUG/*` machine code names the
    /// cause), rc 1.
    pub const DEBUG_DOMAIN: u8 = 1;
    /// The resident session is unavailable and could not be started
    /// (the standalone fallback refused — the session-layer faces).
    #[expect(
        dead_code,
        reason = "reserved contract tier; the first consumer wires it"
    )]
    pub const SESSION: u8 = 79;
}

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// `mi_option_purge_delay` — mimalloc v3's enum index 15. The safe
/// wrapper ships no option constants; the index is pinned against the
/// vendored `mimalloc.h` of libmimalloc-sys 0.1.49 (`c_src/mimalloc/v3`,
/// between the bindings' `eager_commit_delay` = 14 and
/// `use_numa_nodes` = 16).
const MI_OPTION_PURGE_DELAY: std::ffi::c_int = 15;

/// The allocator posture for every face of this binary (#4220): freed
/// pages return to the OS at the next abandonment check instead of
/// after the 10 ms batch window — the resident daemon's between-turn
/// RSS floor. Runs before any argument parsing so the serve daemon and
/// the one-shot CLI faces share it.
fn init_allocator() {
    // SAFETY: an mimalloc runtime-option write; the value is a plain
    // long, and setting it ahead of the first purge decision installs
    // it in place of the built-in default.
    unsafe { libmimalloc_sys::mi_option_set(MI_OPTION_PURGE_DELAY, 0) };
}

fn main() -> ExitCode {
    init_allocator();
    let mut raw_args = std::env::args_os();
    let argv0 = raw_args.next();
    // The `dart` PATH shim: when argv0 names this binary `dart` (the
    // symlink `dartq shim install dart` puts on PATH) or DARTQ_SHIM=dart,
    // the process IS the dart shim — `dart analyze` routes through the
    // job gate and the analyzer RSS budget, every other dart
    // subcommand execs the real SDK dart. Checked before the flutter
    // shim: `DARTQ_SHIM=dart` is the one env spelling that must not fall
    // into the flutter face.
    if dartq_toolchain::shim::is_dart_shim_invocation(argv0.as_deref()) {
        let raw: Vec<std::ffi::OsString> = raw_args.collect();
        let code = dart_shim(&raw);
        return ExitCode::from(u8::try_from(code).unwrap_or(exit::INTERNAL_ERROR));
    }
    // `flutter` PATH shim: when argv0 names this binary `flutter` (the
    // symlink `dartq shim install` puts on PATH) or DARTQ_SHIM is set,
    // the process IS the shim — config discovery, cache logic, and the
    // child spawn replace the usual CLI entirely, before any argument
    // parsing (the child gets the original OsString argv, never a lossy
    // copy).
    if dartq_commands::bcache::is_shim_invocation(argv0.as_deref()) {
        let raw: Vec<std::ffi::OsString> = raw_args.collect();
        let code = dartq_commands::bcache::run_flutter_shim(&raw);
        return ExitCode::from(u8::try_from(code).unwrap_or(exit::INTERNAL_ERROR));
    }
    // The explicit flutter forwarder: `dartq flutter <args…>` takes every
    // trailing argument verbatim (raw OsStrings, the same contract as the
    // shim face) and runs the same bcache orchestration. Dispatching
    // ahead of the clap parse keeps flutter's own flags (`--help`,
    // `--version`, `-d …`) out of dartq's grammar entirely — they are
    // flutter's to answer.
    let raw: Vec<std::ffi::OsString> = raw_args.collect();
    if let Some(argv) = flutter_face_argv(&raw) {
        let code = dartq_commands::bcache::run_flutter_explicit(argv);
        return ExitCode::from(u8::try_from(code).unwrap_or(exit::INTERNAL_ERROR));
    }
    // Lossy argv decode: `env::args` panics (exit 101) on a non-UTF-8
    // argument, ahead of any parse; the lossy spelling keeps the process in
    // control — an invalid path fails downstream resolution as the ordinary
    // usage error (exit 64), never a panic.
    let args: Vec<String> = raw
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    ExitCode::from(run(&args))
}

/// The explicit flutter face's dispatch: whether the raw argv (after the
/// program name) starts with the `flutter` token, and the forwarder's
/// argv — everything after it, verbatim, flags and all.
fn flutter_face_argv(argv: &[std::ffi::OsString]) -> Option<&[std::ffi::OsString]> {
    match argv.split_first() {
        Some((token, rest)) if token == std::ffi::OsStr::new("flutter") => Some(rest),
        _ => None,
    }
}

/// The `dart` shim face: `dart analyze …` is dartq's analyze command
/// verbatim (the argv after the `dart` token IS the subcommand line —
/// `analyze` plus its flags and targets), so it re-enters the normal CLI
/// parse and inherits the job gate and the analyzer RSS budget. Every
/// other dart invocation passes through to the real SDK dart, argv
/// untouched.
fn dart_shim(raw: &[std::ffi::OsString]) -> i32 {
    let is_analyze = raw
        .first()
        .is_some_and(|token| token == std::ffi::OsStr::new("analyze"));
    if is_analyze {
        let args: Vec<String> = raw
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        return i32::from(run(&args));
    }
    dartq_toolchain::shim::dart_passthrough(raw)
}

/// `--engine official` and `--engine dartq` spelled with a space, rewritten
/// to the `=` form clap requires: with an optional value, clap would
/// otherwise read the bare 0.1.12 spelling `--engine <target>` as an engine
/// value. Everything after `--` is left alone.
fn engine_spelling(args: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(args.len());
    let mut tokens = args.iter().peekable();
    while let Some(arg) = tokens.next() {
        if arg == "--" {
            out.push(arg.clone());
            out.extend(tokens.cloned());
            break;
        }
        match tokens.peek() {
            Some(value) if arg == "--engine" && matches!(value.as_str(), "official" | "dartq") => {
                out.push(format!("--engine={value}"));
                tokens.next();
            }
            _ => out.push(arg.clone()),
        }
    }
    out
}

fn run(args: &[String]) -> u8 {
    let args = engine_spelling(args);
    match Cli::try_parse_from(std::iter::once("dartq").chain(args.iter().map(String::as_str))) {
        Ok(Cli {
            command: Some(Command::Check(check_args)),
        }) => check_entry(&check_args),
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
        #[cfg(unix)]
        Ok(Cli {
            command: Some(Command::Compatibility(compat_args)),
        }) => compat::run_entry(&compat_args),
        Ok(Cli {
            command: Some(Command::Bcache(bcache_args)),
        }) => bcache(&bcache_args),
        Ok(Cli {
            command: Some(Command::Cache(cache_args)),
        }) => cache(&cache_args),
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
            command: Some(Command::Packages(packages_args)),
        }) => packages::run_entry(&packages_args),
        Ok(Cli {
            command: Some(Command::Session(session_args)),
        }) => session::run_entry(&session_args),
        Ok(Cli {
            command: Some(Command::Debug(debug_args)),
        }) => debug::run_entry(&debug_args),
        Ok(Cli {
            command: Some(Command::Events(events_args)),
        }) => events::run_entry(&events_args),
        Ok(Cli {
            command: Some(Command::Init),
        }) => init(),
        Ok(Cli {
            command: Some(Command::Mcp(mcp_args)),
        }) => mcp::run_entry(&mcp_args),
        Ok(Cli {
            command: Some(Command::Rename(rename_args)),
        }) => rename::run_entry(&rename_args),
        Ok(Cli {
            command: Some(Command::Shim(shim_args)),
        }) => shim(&shim_args),
        Ok(Cli {
            command: Some(Command::Test(test_args)),
        }) => test::run_entry(&queue_bypass(test_args)),
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
    /// Analyze a package directory: delegates to the official `dart
    /// analyze` (analyzer plugins included) and prints its findings through
    /// dartq's output faces. `--engine` runs the in-process dartq engine
    /// instead (`--engine --fast`: scan + parse only).
    Analyze(AnalyzeArgs),
    /// The hidden spelling of `analyze`, kept for compat: the same official
    /// default, and `--engine` runs scan + parse only (the M0 differential
    /// face, `analyze --engine --fast`).
    #[command(hide = true)]
    Check(CheckArgs),
    /// Assess the workspace under several SDK roots — a per-axis ✓/✗
    /// compatibility matrix. `compatibility` stays as a hidden alias.
    /// Unix-only: the face rides the resident-compat machinery, which the
    /// socket-less platforms do not host — the verb is absent there, not
    /// a usage error.
    #[cfg(unix)]
    #[command(name = "compat", alias = "compatibility")]
    Compatibility(compat::CompatArgs),
    /// Manage the Flutter build cache — the `flutter` PATH shim's
    /// content-addressed store: `stats` prints a one-line JSON summary,
    /// `gc` runs the budget/LRU pass and prints its report.
    Bcache(BcacheArgs),
    /// Manage every on-disk cache at once — the build cache, the facts
    /// snapshot store, and the content store: `stats` prints one JSON
    /// line per face plus a total (read-only), `gc` runs each store's own
    /// eviction pass and reports what it freed. A face that fails prints
    /// its own `error` line, the others still report, and the exit is 4.
    Cache(CacheArgs),
    /// Serve the RPC daemon over stdin/stdout (one client; overlay edits +
    /// full diagnostic pushes). Hidden — a protocol face; the resident
    /// session layer supersedes it for interactive use.
    #[command(hide = true)]
    Daemon(DaemonArgs),
    /// Assemble the one-stop AI report for one symbol over the
    /// analyzed workspace.
    Explain(ExplainArgs),
    /// Emit or apply anchored fix patches; `--dry-run` only
    /// outputs them.
    Fix(fix::FixArgs),
    /// Query the plugin hosts' assists for one file location;
    /// report-only. Hidden — the assists ride the `fix` family; the
    /// one-shot verb stays for scripted callers.
    #[command(hide = true)]
    Assists(fix::AssistsArgs),
    /// Build the symbol index over the analyzed workspace and report what
    /// it built. Hidden — an internal maintenance face.
    #[command(hide = true)]
    Index(index::IndexArgs),
    /// List the work points of the enclosing universe: every package
    /// root (workspace roots, members, independent packages, crossing
    /// path dependencies), resolved state, and the boundaries the scan
    /// pruned — the navigation face for multi-package trees.
    Packages(packages::PackagesArgs),
    /// Manage the resident analysis session: `start` elects or attaches
    /// the worktree's long-lived engine server, `status` lists the
    /// user's live sessions, `stop` ends one. Output is JSON lines on
    /// every face — the management channel is a machine channel.
    Session(session::SessionArgs),
    /// Drive debug sessions on the resident engine: `launch` or
    /// `attach`, breakpoints, stepping, paused-frame snapshots and
    /// evaluation, hot reload/restart, and cursor-paged event/output
    /// reads — the same `dartq/debug/*` faces the daemon serves MCP
    /// clients (one engine, one protocol). Output is one JSON line per
    /// call; a domain failure answers one `{"error":...}` line and exit
    /// 1.
    Debug(debug::DebugArgs),
    /// Read a session's events, cursor-paginated: the event log
    /// (`session` handle) or the cold-analysis progress snapshot
    /// (`progress` handle — files scanned, current package).
    Events(events::EventsArgs),
    /// Set up this Flutter project for dartq: write a documented
    /// `dartq.yaml` (only when none exists). The build cache is on with
    /// its defaults from then on; `dartq flutter <args…>` runs it
    /// explicitly, `dartq shim install` opts into PATH interception.
    Init,
    /// Serve the MCP stdio face: every tool mirrors a CLI verb and
    /// converges on the worktree's resident session — one engine for
    /// CLI, events, and MCP clients alike.
    Mcp(mcp::McpArgs),
    /// Assess what a change to one file would affect.
    Impact(impact::ImpactArgs),
    /// Query the analyzed workspace's symbol index.
    Query(QueryArgs),
    /// Rename one symbol across the analyzed workspace: the dry run
    /// prints the machine-readable edit set; `--write` applies it in
    /// place, refusing any file outside the workspace.
    Rename(rename::RenameArgs),
    /// Manage the SDK PATH shims: `install [flutter|dart]` links
    /// `~/.dartq/bin/<tool>` at this executable (the opt-in
    /// interception — `dartq flutter`/`dartq analyze` need no shim),
    /// `remove [flutter|dart]` unlinks it. `flutter` intercepts the
    /// build cache; `dart` routes only `dart analyze` through the run
    /// queue and analyzer memory budget — every other dart subcommand
    /// passes through verbatim. A real file at the path is never touched.
    Shim(ShimArgs),
    /// `dart test` (or, with `--flutter`, `flutter test`) with the same
    /// arguments and output, plus a result cache shared across worktrees,
    /// parallel execution under a host-wide memory lease, and
    /// `--impacted[=REF]` selection.
    Test(test::TestArgs),
    /// Resolve the symbol under one `PATH:LINE:COL` cursor over the
    /// analyzed workspace — the daemon's `dartq/resolveAt` answer,
    /// one-shot. `resolve-at` stays as a hidden alias.
    #[command(name = "resolve", alias = "resolve-at")]
    ResolveAt(resolve::PositionArgs),
    /// Answer the visible-name candidates at one `PATH:LINE:COL` cursor
    /// over the analyzed workspace — the daemon's `dartq/completion`
    /// answer, one-shot. `completion` stays as a hidden alias.
    #[command(name = "complete", alias = "completion")]
    Completion(resolve::PositionArgs),
    /// Print the version.
    Version,
}

/// The hidden `analyze` spelling: the official analyzer by default,
/// `--engine` the scan + parse pass (the M0 differential face).
///
/// Analysis-options boundary: the `--engine` pass consumes no
/// `analysis_options.yaml` —
/// no `exclude:` on the walk, no `errors:` remaps, no lint rules. Options
/// are the engine/analyze face's concern (`dartq-engine`'s
/// `AnalysisOptions`, whose loader needs the package universe this face
/// deliberately lacks); differential corpora driving this face must be
/// options-free.
#[derive(Args)]
struct CheckArgs {
    /// The analyzer: `official` (the default) or `dartq`, the in-process
    /// scan + parse pass (experimental, not diagnostic-parity yet; one
    /// stderr line says so). A bare `--engine` is `dartq`.
    #[arg(long, value_enum, num_args = 0..=1, require_equals = true, default_missing_value = "dartq")]
    engine: Option<EngineArg>,
    #[arg(long, value_enum, default_value_t = CheckFormat::Lon)]
    format: CheckFormat,
    /// Write one metrics JSON object to PATH at exit. Requires `--engine`.
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
    /// Run in-process, without the worktree's resident session. Requires
    /// `--engine`. The engine face consults the session layer by default:
    /// a live session is attached, and when none is live one is spawned
    /// and waited for — the answer is the daemon's over the same targets,
    /// rendered through the same writers. `--cold` skips all of that (the
    /// pre-#2627 behavior).
    /// Invocations carrying a per-run engine posture (`--overlay`,
    /// `--packages`, `--enable-experiment`, `--no-plugins`,
    /// `--memory-limit`, `--trace-events`, `--cache-size-limit`) and
    /// multi-workspace target sets always run in-process — the resident
    /// engine serves the plain face.
    #[arg(long)]
    cold: bool,
    /// Run outside the machine-wide job gate (see
    /// [`AnalyzeArgs::no_queue`]).
    #[arg(long, hide = true)]
    no_queue: bool,
    /// The dartdev global flags — see [`DartdevGlobals`].
    #[command(flatten)]
    dartdev_globals: DartdevGlobals,
    /// Targets; empty resolves to the current directory.
    #[arg(default_value = ".")]
    paths: Vec<PathBuf>,
}

#[derive(Clone, Copy, ValueEnum)]
enum CheckFormat {
    /// Canonical LON over the short vocabulary: `{v diags:[…]}`.
    Lon,
    /// Single-line JSON envelope.
    Json,
    /// Grouped human rendering (the analyze text face's shape).
    Text,
}

#[derive(Args)]
#[expect(clippy::struct_excessive_bools)] // the bools are independent clap flags; an enum decomposition would obscure the mapping
struct AnalyzeArgs {
    /// Analysis targets; each names a directory or a file, mixing freely
    /// (the oracle's `rest` params). Empty resolves to the current
    /// directory (the oracle's `Directory.current` fallback).
    #[arg(default_value = ".")]
    targets: Vec<PathBuf>,
    /// The analyzer: `official` runs the Dart analyzer (resident server
    /// or one `dart analyze` per run), `dartq` the in-process dartq engine
    /// (experimental, not diagnostic-parity yet; one stderr line says so).
    /// Unset, `analyzer.engine` in `dartq.yaml` decides, else `official`. A
    /// bare `--engine` is `dartq`. The flags marked `Requires --engine
    /// dartq` are usage errors on `official`.
    #[arg(long, value_enum, num_args = 0..=1, require_equals = true, default_missing_value = "dartq")]
    engine: Option<EngineArg>,
    /// Run `dart analyze` once for this run instead of asking the resident
    /// analysis server (`analyzer.official.resident: false` sets it for
    /// good). Official engine only.
    #[arg(long)]
    no_resident: bool,
    /// Analysis scope of a file or directory aim (official backend only).
    /// `package` — the default — roots the analysis at the aim's package
    /// context (the nearest ancestor `pubspec.yaml`), so a file in a pub
    /// workspace member analyzes that package instead of the whole
    /// workspace, the scope `dart analyze <file>` itself uses. `workspace`
    /// roots it at the aim's pub workspace root (the whole-workspace
    /// posture). `file` (alias `dir`) roots it at the aims themselves —
    /// the server's included set is the aim paths (files or directories),
    /// the universe `dart analyze <paths>`'s own root spelling opens. An
    /// aim that is a workspace root resolves to that root either way.
    ///
    /// Unset — the default — resolves per shape at run time (see
    /// [`resolve_scope`]): aims — files or directories — whose package
    /// context is their workspace root — the single-pubspec shape, where
    /// the package tier would root at the whole repo and gain nothing —
    /// take the file tier; every other default keeps the package tier. An
    /// explicit `--scope` never re-resolves.
    #[arg(long, value_enum)]
    scope: Option<AnalyzeScope>,
    /// Print one stderr line naming the engine that answered (and, for
    /// `official`, the SDK version and whether the resident server or a
    /// per-run `dart analyze` did; for `dartq`, whether the resident
    /// session or the in-process engine did).
    #[arg(long)]
    stats: bool,
    /// Serve the resident analysis server of the key directory DIR (the
    /// daemon entry the client starts; not for direct use).
    #[arg(long = "resident-serve", value_name = "DIR", hide = true)]
    resident_serve: Option<PathBuf>,
    /// Output format: the constant default is `lon` (canonical LON over
    /// the short vocabulary — never the terminal's shape, so a piped run
    /// and a TTY run print the same bytes); `json` is the frozen
    /// single-line oracle envelope, `text` the grouped human face.
    #[arg(long, value_enum, default_value_t = AnalyzeFormat::Lon)]
    format: AnalyzeFormat,
    /// Scan + parse only (the `check` pipeline): parser-tier diagnostics
    /// without the engine — the fast pass for pure-syntax sweeps. Requires
    /// `--engine`.
    #[arg(long)]
    fast: bool,
    /// Write one metrics JSON object to PATH at exit. Requires `--engine`.
    #[arg(long)]
    metrics_json: Option<PathBuf>,
    /// Analyze PATH with the content of FILE instead of the on-disk bytes:
    /// repeatable, relative PATH resolves against DIR, and
    /// FILE `-` reads stdin. Requires `--engine`; the engine-view faces
    /// only — `check` has no engine to overlay.
    #[arg(long = "overlay", value_name = "PATH=FILE")]
    overlays: Vec<String>,
    /// Dart SDK root; unset, resolves through the shared discovery chain
    /// (`$DART_SDK`, then `dartq.yaml`'s `sdk:`, then the `dart` on PATH,
    /// then the fvm default), and a Flutter SDK root resolves onto its
    /// embedded `bin/cache/dart-sdk`. The oracle's spelling `--sdk-path`
    /// is the documented one; `--sdk` stays as a hidden compat alias.
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
    /// The official analyzer runs them itself. On `--engine`, when the
    /// effective `analysis_options.yaml` declares a `plugins:`
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
    /// the output is byte-identical to an unbudgeted run. Requires
    /// `--engine`.
    #[arg(long, value_name = "MiB")]
    memory_limit: Option<u64>,
    /// Run outside the machine-wide job gate: by default every heavy
    /// analysis (an official `dart analyze` child, an in-process engine
    /// run) takes a job slot under `memory.max_concurrent`. dartq's own
    /// flag: the official analyzer has no such spelling.
    #[arg(long, hide = true)]
    no_queue: bool,
    /// Print each workspace engine's memory posture to stderr after the
    /// run (requires `--engine`) — one `dartq-memory` line per engine
    /// carrying the budget ladder's state (the armed limit, descended
    /// tiers, deepest tier key), its last phase-boundary RSS sample, and
    /// the warm lane's counters. The 5-instance product budget's
    /// observability face; the
    /// resident session's own posture is the daemon's `dartq/memory`
    /// face, so the flag always runs in-process.
    #[arg(long)]
    memory_report: bool,
    /// JSONL trace sink: the engine appends one JSON object per
    /// engine event — stable event names, stable field order, flushed per
    /// event so a killed CI run keeps its tail. Unset = no sink and zero
    /// event work: the output face is byte-identical to an untraced run.
    /// Requires `--engine`.
    #[arg(long, value_name = "PATH")]
    trace_events: Option<PathBuf>,
    /// Override the analysis cache location (the oracle's hidden
    /// `--cache PATH`). The official analyzer takes it; on `--engine` dartq
    /// manages its own content store under `~/.cache/dartq/` (see
    /// `--cache-size-limit`), so the flag is accepted for CLI parity and
    /// ignored.
    #[arg(long, value_name = "PATH", hide = true)]
    cache: Option<PathBuf>,
    /// The oracle's hidden `--[no-]use-aot-snapshot` flag (defaults on
    /// upstream; picks the analysis-server snapshot flavor). The official
    /// analyzer takes it; `--engine` runs in-process — no snapshot exists —
    /// so both spellings parse and do nothing there.
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
    /// (512MiB, 10GiB, 4096). Unset = the 10 GiB default. Requires
    /// `--engine`.
    #[arg(
        long,
        value_name = "SIZE",
        value_parser = dartq_engine::store::parse_size_limit
    )]
    cache_size_limit: Option<u64>,
    /// Run in-process, without the worktree's resident session. Requires
    /// `--engine`. The engine face consults a live session when one is
    /// attached (never spawning one — a syntax sweep must not pay an engine
    /// bring-up); `--cold` skips the consult (`--fast` under the analyze
    /// spelling takes the same route).
    #[arg(long)]
    cold: bool,
    /// The dartdev global flags — see [`DartdevGlobals`].
    #[command(flatten)]
    dartdev_globals: DartdevGlobals,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum AnalyzeFormat {
    /// Canonical LON over the short vocabulary: one `{v diags:[…]}`
    /// document, one flat `{code sev file line col len msg doc?}` row
    /// per diagnostic (the default).
    Lon,
    /// Single-line JSON envelope, byte parity with
    /// `dart analyze --format=json` (the shared oracle-exact writer).
    /// Diagnostics only: workspace advisories print as one line on stderr
    /// ahead of it.
    Json,
    /// The human face: diagnostics grouped by file, `sev code line:col
    /// msg` rows, the count tail.
    Text,
}

impl From<CheckFormat> for AnalyzeFormat {
    fn from(format: CheckFormat) -> Self {
        match format {
            CheckFormat::Lon => Self::Lon,
            CheckFormat::Json => Self::Json,
            CheckFormat::Text => Self::Text,
        }
    }
}

/// The analysis universe a file or directory aim opens (`--scope`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
enum AnalyzeScope {
    /// The aim's package context — the nearest ancestor `pubspec.yaml`. A pub
    /// workspace member resolves to its own package directory, not the
    /// workspace root: the analyzer keeps resolving `package:` URIs and
    /// reading `analysis_options.yaml` through the workspace but analyzes
    /// only the package's files, which is the universe `dart analyze <file>`
    /// builds.
    #[default]
    Package,
    /// The aim's pub workspace root — one analysis universe per worktree
    /// (the pre-`--scope` posture).
    Workspace,
    /// The aims themselves — the aim paths (files or directories) are the
    /// resident server's `analysis.setAnalysisRoots` included set verbatim,
    /// the universe `dart analyze <paths>` itself opens (dartdev passes the
    /// canonicalized rest targets — directories included — as `included`,
    /// upstream Dart SDK source,
    /// pkg/dartdev/lib/src/analysis_server.dart:174-198). A single-file aim
    /// in a single-pubspec repo analyzes that one file's context — an
    /// oracle-class cold start — and a directory aim analyzes the
    /// directory's tree instead of the whole package. The aim set keys its
    /// own resident (the roots key the daemon, so an aim never drags the
    /// whole package's server in or shares its context).
    #[value(alias = "dir")]
    File,
}

/// The analyze flags against the official backend, every flag in one class
/// (`check` carries a subset, same class):
///
/// | Class | Flags |
/// | --- | --- |
/// | forward — `dart analyze` takes it | the targets, `--sdk-path`, `--packages`, `--cache`, `--enable-experiment`, `--[no-]plugins`, `--[no-]use-aot-snapshot`, `--[no-]fatal-warnings`, `--fatal-infos`, `--verbose`, `--version`, `--suppress-analytics`, `--disable-telemetry`, `--[no-]analytics` |
/// | engine-only — legal only with `--engine` | `--fast`, `--metrics-json`, `--overlay`, `--memory-limit`, `--memory-report`, `--trace-events`, `--cache-size-limit`, `--cold` |
/// | consumed — dartq answers it, the child never sees it | `--engine`, `--format`, `--memory`, `--scope`, `--diagnostics`, `--enable-analytics`, `--disable-analytics` |
///
/// The child always runs the machine format, so `--format` selects dartq's
/// re-emission and `--memory` (upstream reads it on its json format only)
/// keeps the json face's own splice. The last three would break the run:
/// `--diagnostics` swaps upstream's logger for one that prefixes every
/// machine row and traces the server protocol to stdout, and
/// `--enable-analytics`/`--disable-analytics` make upstream rewrite the
/// machine's analytics setting and exit 0 without analyzing — a false clean.
impl AnalyzeArgs {
    /// The first engine-only flag set on this invocation.
    fn engine_only_flag(&self) -> Option<&'static str> {
        [
            (self.fast, "--fast"),
            (self.metrics_json.is_some(), "--metrics-json"),
            (!self.overlays.is_empty(), "--overlay"),
            (self.memory_limit.is_some(), "--memory-limit"),
            (self.memory_report, "--memory-report"),
            (self.trace_events.is_some(), "--trace-events"),
            (self.cache_size_limit.is_some(), "--cache-size-limit"),
            (self.cold, "--cold"),
        ]
        .into_iter()
        .find_map(|(set, flag)| set.then_some(flag))
    }

    /// The `dart analyze` arguments: the forwarded flags, then the targets.
    fn official_args(&self) -> Vec<OsString> {
        let mut argv = shared_official_args(
            self.fatal_warnings,
            self.no_fatal_warnings,
            self.fatal_infos,
            self.dartdev_globals,
        );
        for (flag, path) in [
            ("--sdk-path", &self.sdk_path),
            ("--packages", &self.packages),
            ("--cache", &self.cache),
        ] {
            if let Some(path) = path {
                argv.extend([OsString::from(flag), OsString::from(path)]);
            }
        }
        for name in &self.enable_experiments {
            argv.push(format!("--enable-experiment={name}").into());
        }
        argv.extend(set_flags([
            (self.plugins, "--plugins"),
            (self.no_plugins, "--no-plugins"),
            (self.use_aot_snapshot, "--use-aot-snapshot"),
            (self.no_use_aot_snapshot, "--no-use-aot-snapshot"),
        ]));
        argv.extend(self.targets.iter().map(OsString::from));
        argv
    }

    /// The resident request, or `None` when a flag needs the per-run child:
    /// `--no-resident`, `--sdk-path` (another SDK), `--cache` (the user's
    /// cache), `--enable-experiment`, `--[no-]plugins` (the server flag is
    /// newer than the oldest served SDK; dartdev answers it per SDK), the
    /// snapshot flavor, and the dartdev globals whose effect the resident
    /// path does not reproduce.
    fn resident_request(&self) -> Option<ResidentAsk> {
        let per_run = self.no_resident
            || self.sdk_path.is_some()
            || self.cache.is_some()
            || !self.enable_experiments.is_empty()
            || self.plugins
            || self.no_plugins
            || self.use_aot_snapshot
            || self.no_use_aot_snapshot
            || self.dartdev_globals.verbose
            || self.dartdev_globals.version;
        (!per_run).then(|| ResidentAsk {
            packages: self.packages.clone(),
            fatal_warnings: !self.no_fatal_warnings,
            fatal_infos: self.fatal_infos,
            scope: self.scope,
        })
    }
}

impl CheckArgs {
    /// The first engine-only flag set on this invocation.
    fn engine_only_flag(&self) -> Option<&'static str> {
        [
            (self.metrics_json.is_some(), "--metrics-json"),
            (self.cold, "--cold"),
        ]
        .into_iter()
        .find_map(|(set, flag)| set.then_some(flag))
    }

    /// The `dart analyze` arguments: the forwarded flags, then the targets.
    fn official_args(&self) -> Vec<OsString> {
        let mut argv = shared_official_args(
            self.fatal_warnings,
            self.no_fatal_warnings,
            self.fatal_infos,
            self.dartdev_globals,
        );
        argv.extend(self.paths.iter().map(OsString::from));
        argv
    }
}

/// The forwarded flags both faces carry: the fatal tiers and the dartdev
/// globals that leave the run intact.
fn shared_official_args(
    fatal_warnings: bool,
    no_fatal_warnings: bool,
    fatal_infos: bool,
    globals: DartdevGlobals,
) -> Vec<OsString> {
    set_flags([
        (fatal_warnings, "--fatal-warnings"),
        (no_fatal_warnings, "--no-fatal-warnings"),
        (fatal_infos, "--fatal-infos"),
        (globals.verbose, "--verbose"),
        (globals.version, "--version"),
        (globals.suppress_analytics, "--suppress-analytics"),
        (globals.disable_telemetry, "--disable-telemetry"),
        (globals.analytics, "--analytics"),
        (globals.no_analytics, "--no-analytics"),
    ])
    .collect()
}

/// The spellings of the set flags, in order.
fn set_flags<const N: usize>(flags: [(bool, &str); N]) -> impl Iterator<Item = OsString> {
    flags
        .into_iter()
        .filter_map(|(set, flag)| set.then(|| flag.into()))
}

/// The dartdev global flags, accepted on the diagnostic faces (ticket
/// #2385, the cli-compat Flag-face P2): the official backend forwards the
/// ones that leave `dart analyze`'s run intact (see
/// [`shared_official_args`]), the engine ignores them all. Upstream declares
/// them on the runner's global parser (`upstream Dart SDK source,
/// pkg/dartdev/lib/src/utils.dart`, `globalDartdevOptionsParser`), and the
/// global parse is merged past the command name, so they are accepted
/// positioned *after* the subcommand — `dart analyze --suppress-analytics`
/// is the standard CI incantation. The engine has no analytics/telemetry
/// layer and no verbose logger, so the flags change nothing there. Hidden,
/// matching the oracle's `analyze --help` face where the globals don't
/// appear.
/// Known upstream corner, not replicated: `--suppress-analytics` combined
/// with `--enable-analytics`/`--disable-analytics` is an analytics-layer
/// usage error (exit 254) upstream; dartq has no analytics layer to
/// conflict with. The `-v` abbreviation upstream is not taken here — it
/// stays dartq's `--verbosity` short (the `-v`/`--verbose` collision is a
/// deferred compat-policy decision).
#[derive(Args, Default, Clone, Copy)]
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
    /// OS file events by default, a bounded-interval walk under
    /// `DARTQ_WATCH_SUBSTRATE=poll` — a convenience layer: explicit push
    /// and content verification stay the truth.
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
    /// session context. Grammar: NUMBER [KiB|MiB|GiB] — suffix
    /// case-insensitive, a bare number is bytes (512MiB, 10GiB, 4096).
    /// Unset = the analyze face's unconditional 10 GiB default (the
    /// resident engine's cross-session posture; #4210).
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
    /// Output format; `lon` the default, `json` the report verbatim,
    /// `text` the indented report.
    #[arg(long, value_enum, default_value_t = OutputFormat::Lon)]
    format: OutputFormat,
    /// Dart SDK root; defaults to `$DART_SDK`.
    #[arg(long)]
    sdk: Option<PathBuf>,
    /// Report detail tier (the schema's verbosity table; `compact` by
    /// default, `minimal` swaps the section arrays for `counts`).
    #[arg(short = 'v', long, value_enum, default_value_t = Verbosity::Compact)]
    verbosity: Verbosity,
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

/// The `analyze --fast` face's [`CheckArgs`]: the check pipeline over the
/// analyze targets — the format maps one-to-one (the faces share the
/// same tri-axis).
fn check_from_analyze(args: &AnalyzeArgs) -> CheckArgs {
    CheckArgs {
        engine: args.engine,
        format: match args.format {
            AnalyzeFormat::Lon => CheckFormat::Lon,
            AnalyzeFormat::Json => CheckFormat::Json,
            AnalyzeFormat::Text => CheckFormat::Text,
        },
        metrics_json: args.metrics_json.clone(),
        fatal_warnings: args.fatal_warnings,
        no_fatal_warnings: args.no_fatal_warnings,
        fatal_infos: args.fatal_infos,
        cold: args.cold,
        no_queue: args.no_queue,
        dartdev_globals: args.dartdev_globals,
        paths: args.targets.clone(),
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
    /// every answer.
    #[arg(short = 'v', long, value_enum, default_value_t = Verbosity::Compact)]
    verbosity: Verbosity,
    /// Output format per answer: `lon` the default (one canonical line
    /// per answer), `json` the v1 envelope per line, `text` the
    /// indented envelope per answer.
    #[arg(long, value_enum, default_value_t = OutputFormat::Lon)]
    format: OutputFormat,
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
    /// The symbol name to look up; under `--fuzzy`, the fuzzy query.
    name: String,
    /// Package directory holding pubspec.yaml and
    /// `.dart_tool/package_config.json`; defaults to the current directory.
    #[arg(default_value = ".")]
    dir: PathBuf,
    /// Output format; `lon` the default (the short-vocabulary answer,
    /// canonical LON), `json` the v1 envelope, `text` the indented answer.
    #[arg(long, value_enum, default_value_t = OutputFormat::Lon)]
    format: OutputFormat,
    /// Report detail tier (the schema's verbosity table; `compact` by
    /// default). `minimal` keeps the red-line locators: the JSON rows drop
    /// the convenience spellings and the element-face columns, and the
    /// reserved kind arrays (IMPL/DEP) never print.
    #[arg(short = 'v', long, value_enum, default_value_t = Verbosity::Compact)]
    verbosity: Verbosity,
    /// Dart SDK root; defaults to `$DART_SDK`.
    #[arg(long)]
    sdk: Option<PathBuf>,
    /// Fuzzy discovery instead of the exact lookup: the declarations whose
    /// names fuzzy-match the argument, best-first in the `decls` rows —
    /// exact prefix, then word-boundary camelCase, then contiguous
    /// substring, then sparse subsequence, case-insensitively; ties by
    /// name length, then lexicographic; capped at the top 32. Answers on
    /// the symbol face only (no reference rows are joined).
    #[arg(long)]
    fuzzy: bool,
}

/// Severity arithmetic: the highest fatal tier present.
/// Errors are always fatal (3); warnings fatal unless `--no-fatal-warnings`
/// (2); infos fatal only under `--fatal-infos` (1). [`severity_tier`] is
/// the one ladder — the cold faces over their diagnostic rows, the
/// resident-routed faces over the answer's severity counts.
fn severity_exit(diags: &[Diagnostic], fatal_warnings: bool, fatal_infos: bool) -> u8 {
    let mut has_error = false;
    let mut has_warning = false;
    let mut has_info = false;
    for d in diags {
        match d.severity {
            Severity::Error => has_error = true,
            Severity::Warning => has_warning = true,
            Severity::Info => has_info = true,
        }
    }
    severity_tier(
        has_error,
        has_warning,
        has_info,
        fatal_warnings,
        fatal_infos,
    )
}

/// The severity ladder over presence flags (see [`severity_exit`]).
fn severity_tier(
    has_error: bool,
    has_warning: bool,
    has_info: bool,
    fatal_warnings: bool,
    fatal_infos: bool,
) -> u8 {
    if has_error {
        exit::ERRORS
    } else if has_warning && fatal_warnings {
        exit::FATAL_WARNINGS
    } else if has_info && fatal_infos {
        exit::FATAL_INFOS
    } else {
        exit::CLEAN
    }
}

fn check(args: &CheckArgs) -> u8 {
    // Buffered like the analyze/query faces: the writers emit one writeln
    // per line, and an unbuffered lock would flush per diagnostic line.
    let stdout = io::stdout();
    let mut out = BufWriter::new(stdout.lock());
    check_run(args, &mut out)
}

/// [`check`] over a caller-supplied writer — the sibling faces'
/// writer-taking shape (`query_symbol_run`, `explain_run`), so the output
/// documents are pinnable without capturing the process stdout.
fn check_run(args: &CheckArgs, out: &mut impl Write) -> u8 {
    // The resident consult (#2627): a live session answers the same M0
    // pipeline in the daemon, rendered through the same writers. Attach
    // only — never a spawn (see `resident::route_check`). No session
    // here is the check face's normal shape, not a fallback: it proceeds
    // in-process with no advisory.
    #[cfg(unix)]
    if !args.cold
        && let Some(code) = resident::route_check(args, out)
    {
        return code;
    }
    let started = Instant::now();
    let discovery_at = Instant::now();
    // The check face's own setup window: run start → discovery walk start.
    // Two adjacent captures — scan+parse has no SDK/assembly work, so the
    // phase is structurally ~0, measured rather than hardcoded.
    let assembly_ms = elapsed_ms(discovery_at.duration_since(started));
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
    let analyzed = crash_tier(|| dartq_commands::check::analyze_targets(&args.paths, &files));
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

    let output = run.output();
    let render_at = Instant::now();
    let written = emit_diagnostics(args.format.into(), false, &output, out);
    if let Err(error) = written.and_then(|()| out.flush()) {
        eprintln!("dartq: internal error: writing diagnostics failed: {error}");
        return exit::INTERNAL_ERROR;
    }
    let render_ms = elapsed_ms(render_at.elapsed());

    if let Some(metrics_path) = &args.metrics_json {
        let metrics = RunMetrics {
            files: run.file_count(),
            loc: run.loc(),
            diagnostics: run.diagnostics().len(),
            discovery_ms,
            // No SDK and no lint stage on the scan+parse face.
            sdk_ms: 0,
            assembly_ms,
            analyze_ms,
            lints_ms: 0,
            render_ms,
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
///
/// Phase split (#2628): every phase is a monotonic-clock (`Instant`)
/// wall-clock, and the phases are non-overlapping sub-intervals of the
/// `total_ms` window, so `discovery_ms + sdk_ms + assembly_ms + analyze_ms
/// + lints_ms + render_ms <= total_ms` always holds; the unattributed
/// remainder (target injection, the plugin weave, the merged sort, the
/// notice prints) is the slack. Interval boundaries, per face:
///
/// * `discovery_ms` — `analyze`: each engine run's
///   `Engine::discover_workspace()` call, summed across engines;
///   `check`: the `discover` walk.
/// * `sdk_ms` — `analyze`: the `SdkSnapshot::load` calls inside the config
///   assembly (the plain snapshot plus the embedder-table reload), summed
///   across workspace groups; `check`: no SDK face, `0`.
/// * `assembly_ms` — `analyze`: from the run start (target grouping)
///   through overlay loading — the config-assembly remainder, plugin
///   launch, `Engine::new`, runtime arming — minus the SDK loads;
///   `check`: the setup window between run start and the discovery walk
///   (structurally ~0).
/// * `analyze_ms` — the engine pass (`analyze_pass`), ending where the
///   lint stage begins — lints never double-count into it.
/// * `lints_ms` — the lint stage block after the engine pass
///   (`analyze_lints` plus the options registry/conflict/removed rows);
///   `check`: no lint stage, `0`.
/// * `render_ms` — the diagnostics render and flush (lon/json/text
///   writers); the metrics write itself is excluded — it happens after
///   `total_ms` is read.
/// * `total_ms` — the whole run window, read at metrics-construction time
///   (after the render, before the metrics file write).
///
/// Without `--metrics-json` the only cost of the split is the `Instant`
/// captures — no formatting, no allocation.
struct RunMetrics {
    files: usize,
    loc: u64,
    diagnostics: usize,
    discovery_ms: u64,
    sdk_ms: u64,
    assembly_ms: u64,
    analyze_ms: u64,
    lints_ms: u64,
    render_ms: u64,
    total_ms: u64,
    libraries_analyzed: usize,
    libraries_from_cache: usize,
    max_rss_bytes: Option<u64>,
}

impl RunMetrics {
    fn json(&self) -> serde_json::Value {
        // The one CLI-local face under a published long-stamp contract
        // (dartq-schema's `Domain::RunMetrics` freezes `schema_version`;
        // the compact `v` table is the stdout faces' own).
        serde_json::json!({
            "schema_version": dartq_commands::SCHEMA_VERSION,
            "files": self.files,
            "loc": self.loc,
            "diagnostics": self.diagnostics,
            "discovery_ms": self.discovery_ms,
            "sdk_ms": self.sdk_ms,
            "assembly_ms": self.assembly_ms,
            "analyze_ms": self.analyze_ms,
            "lints_ms": self.lints_ms,
            "render_ms": self.render_ms,
            "total_ms": self.total_ms,
            "libraries_analyzed": self.libraries_analyzed,
            "libraries_from_cache": self.libraries_from_cache,
            "max_rss_bytes": self.max_rss_bytes,
        })
    }
}

/// The analyze and check faces' analyzer. Both backends feed one row model,
/// one emitter set ([`emit_diagnostics`]), and one exit table, so this enum
/// is the whole seam: [`dispatch`] is its one match, another backend is one
/// more variant, and moving the product default is the one line in
/// [`Backend::of`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Backend {
    /// The official analyzer: a `dart analyze` child, analyzer plugins
    /// included (the default).
    Official,
    /// The in-process dartq engine (`--engine`).
    Engine,
}

/// The `--engine` values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum EngineArg {
    Official,
    Dartq,
}

impl Backend {
    /// The flag, then `analyzer.engine` in `dartq.yaml`, then `official`.
    fn of(flag: Option<EngineArg>, config: Option<dartq_commands::config::Engine>) -> Self {
        use dartq_commands::config::Engine;
        match (flag, config) {
            (Some(EngineArg::Dartq), _) | (None, Some(Engine::Dartq)) => Self::Engine,
            (Some(EngineArg::Official), _) | (None, Some(Engine::Official) | None) => {
                Self::Official
            }
        }
    }

    /// [`Backend::of`] over the invocation's discovered `dartq.yaml`
    /// (found once per invocation — see [`analyze`]). An unreadable
    /// config leaves the choice to the flag and the default: the analyze
    /// face never failed on `dartq.yaml` and must not start now.
    fn resolve(flag: Option<EngineArg>, config: Option<&DartqConfig>) -> Self {
        Self::of(flag, config.and_then(DartqConfig::engine))
    }
}

/// The dartq engine's one stderr line; the official engine prints none.
const ENGINE_NOTE: &str =
    "dartq analyze: using the dartq engine (experimental, not diagnostic-parity yet)";

/// The usage error of an engine-only flag on the official backend.
fn engine_only_message(flag: &str) -> String {
    format!("error: {flag} requires --engine dartq: only the in-process dartq engine reads it")
}

/// The usage error of an official-only flag on the dartq engine.
fn official_only_message(flag: &str) -> String {
    format!("error: {flag} requires --engine official: only the official analyzer reads it")
}

/// Routes one analyze or check invocation to its backend. `engine_only` is
/// the invocation's first engine-only flag: legal on [`Backend::Engine`], a
/// usage error on [`Backend::Official`] rather than a flag silently dropped.
fn dispatch(
    backend: Backend,
    engine_only: Option<&str>,
    official_only: Option<&str>,
    official: impl FnOnce() -> u8,
    engine: impl FnOnce() -> u8,
) -> u8 {
    match (backend, engine_only, official_only) {
        (Backend::Official, None, _) => official(),
        (Backend::Official, Some(flag), _) => {
            eprintln!("{}", engine_only_message(flag));
            exit::USAGE
        }
        (Backend::Engine, _, Some(flag)) => {
            eprintln!("{}", official_only_message(flag));
            exit::USAGE
        }
        (Backend::Engine, _, None) => {
            eprintln!("{ENGINE_NOTE}");
            engine()
        }
    }
}

/// `dartq analyze`.
fn analyze(args: &AnalyzeArgs) -> u8 {
    #[cfg(unix)]
    if let Some(dir) = &args.resident_serve {
        return dartq_analyzer_host::daemon::serve(
            dir,
            dartq_commands::render::render_official_face,
        );
    }
    // One config discovery per invocation: `dartq.yaml` is found once
    // here and threaded through every face below — the backend choice,
    // the official settings, the resident request's knobs — where each
    // knob used to re-walk the same ancestors. A file that appears or
    // changes mid-run is not re-read: single discovery per process is
    // the contract.
    let cwd = std::env::current_dir().unwrap_or_default();
    let config = dartq_commands::config::discover(&cwd);
    dispatch(
        Backend::resolve(args.engine, config.as_ref().ok().and_then(Option::as_ref)),
        args.engine_only_flag(),
        args.no_resident.then_some("--no-resident"),
        || {
            official_face(
                &OfficialFace {
                    format: args.format,
                    memory: args.memory,
                    argv: args.official_args(),
                    resident: args.resident_request(),
                    stats: args.stats,
                },
                &args.targets,
                args.no_queue,
                &config,
            )
        },
        || analyze_engine(args),
    )
}

/// `dartq check`: `analyze`'s entry under the hidden spelling, with
/// [`check`] the engine face.
fn check_entry(args: &CheckArgs) -> u8 {
    // One discovery per invocation, as in [`analyze`].
    let cwd = std::env::current_dir().unwrap_or_default();
    let config = dartq_commands::config::discover(&cwd);
    dispatch(
        Backend::resolve(args.engine, config.as_ref().ok().and_then(Option::as_ref)),
        args.engine_only_flag(),
        None,
        || {
            official_face(
                &OfficialFace {
                    format: args.format.into(),
                    memory: false,
                    argv: args.official_args(),
                    resident: None,
                    stats: false,
                },
                &args.paths,
                args.no_queue,
                &config,
            )
        },
        || check(args),
    )
}

/// One official analysis's invocation facts.
struct OfficialFace {
    format: AnalyzeFormat,
    memory: bool,
    argv: Vec<OsString>,
    /// The resident request, when the invocation allows the resident
    /// server; `None` runs `dart analyze` per run.
    resident: Option<ResidentAsk>,
    stats: bool,
}

/// What the resident server needs beyond the targets: the forwarded
/// flags it honors, and the analysis scope its roots resolve under — the
/// explicit `--scope` verbatim, `None` the shape-resolved default
/// ([`resolve_scope`]).
struct ResidentAsk {
    packages: Option<PathBuf>,
    fatal_warnings: bool,
    fatal_infos: bool,
    scope: Option<AnalyzeScope>,
}

/// A `dartq test` line that only asks for help or the version skips the
/// job gate: it runs no tests, so it must not wait behind a long run.
fn queue_bypass(args: test::TestArgs) -> test::TestArgs {
    if dartq_toolchain::zero_execution(&args.args) {
        let mut args = args;
        args.args.push("--no-queue".to_owned());
        return args;
    }
    args
}

/// One official analysis, crash tier included: the targets classify the way
/// the engine path classifies them (a missing one is the same usage tier
/// with the same wording), `dart` resolves through the shared toolchain
/// resolver, and a resolver miss or a binary that does not start is the
/// usage tier — the environment failed, like the engine path's missing SDK.
fn official_face(
    face: &OfficialFace,
    targets: &[PathBuf],
    no_queue: bool,
    config: &Result<Option<DartqConfig>, ConfigError>,
) -> u8 {
    let outcome = crash_tier(|| {
        let cwd = std::env::current_dir().unwrap_or_default();
        let settings = official_settings(config);
        let yaml = config.as_ref().ok().and_then(Option::as_ref);
        let max_percent = memory_max_percent(yaml);
        let resolved = match resolve_targets(targets) {
            Ok(targets) => targets,
            Err(path) => {
                eprintln!("error: Directory or file doesn't exist: {}", path.display());
                return exit::USAGE;
            }
        };
        // One root-resolution walk per distinct directory this invocation
        // (#4260): the analysis root, the scope pass, and the request
        // roots share it.
        let mut roots = RootMemo::default();
        let root = analysis_root(&resolved, &mut roots);
        let dart = dartq_toolchain::find_dart(None);
        let out = &mut BufWriter::new(io::stdout().lock());
        let err = &mut io::stderr().lock();
        // The resident server first: it shares one server between clients,
        // so a client never holds a queue ticket; the daemon takes one only
        // for a server start (a full analysis). Any failure falls through
        // to the per-run child below.
        let mut sdk = None;
        // `--no-queue` bypasses the job gate for this whole process.
        if no_queue {
            gate_bypass();
        }
        // A run nested inside another dartq job's slot (a test, a build)
        // must not start a detached resident server: it would outlive the
        // slot. It analyzes per run under its holder's slot instead.
        if let (Some(ask), Some(dart), true, false) =
            (&face.resident, &dart, settings.resident, gate_nested())
        {
            // The resident server is keyed on the aim's analysis roots (the
            // package contexts by default — see [`scope_roots`]): a
            // single-file aim analyzes its package, not the whole
            // workspace, while the aims scope its report. Every
            // `dartq analyze` over the same roots shares the daemon and
            // its cache.
            let warm = warm_posture(yaml);
            let request = dartq_analyzer_host::Request {
                dart: dart.clone(),
                targets: resolved
                    .iter()
                    .map(|target| match target {
                        TargetKind::Dir(dir) | TargetKind::File(dir) => dir.clone(),
                    })
                    .collect(),
                roots: scope_roots(
                    &resolved,
                    resolve_scope(ask.scope, &resolved, &mut roots),
                    &mut roots,
                ),
                packages: ask.packages.clone(),
                budget: settings.memory_budget_bytes,
                idle: Duration::from_secs(settings.idle_minutes.max(1) * 60),
                max_percent,
                max_concurrent: job_cap(yaml),
                pool: pool_posture(yaml),
                warm: warm.0,
                warm_ttl_secs: warm.1,
                face: Some(dartq_analyzer_host::Face {
                    format: match face.format {
                        AnalyzeFormat::Lon => dartq_analyzer_host::FaceFormat::Lon,
                        AnalyzeFormat::Json => dartq_analyzer_host::FaceFormat::Json,
                        AnalyzeFormat::Text => dartq_analyzer_host::FaceFormat::Text,
                    },
                    root: root.clone(),
                }),
            };
            match dartq_analyzer_host::analyze(&request) {
                dartq_analyzer_host::Outcome::Report { report, sdk } => {
                    if face.stats {
                        eprintln!(
                            "dartq analyze: engine official (Dart {}, resident)",
                            sdk.version
                        );
                    }
                    let run =
                        OfficialRun::of_report(report, ask.fatal_warnings, ask.fatal_infos, root);
                    return official_finish(&run, face.format, face.memory, out, err);
                }
                dartq_analyzer_host::Outcome::Face { stdout, meta, sdk } => {
                    if face.stats {
                        eprintln!(
                            "dartq analyze: engine official (Dart {}, resident)",
                            sdk.version
                        );
                    }
                    return face_finish(
                        &stdout,
                        &meta,
                        face,
                        ask.fatal_warnings,
                        ask.fatal_infos,
                        out,
                        err,
                    );
                }
                dartq_analyzer_host::Outcome::Budget(message) => {
                    eprintln!("error: {message}");
                    return exit::USAGE;
                }
                dartq_analyzer_host::Outcome::PerRun { reason, sdk: found } => {
                    if let Some(reason) = reason {
                        eprintln!("{reason}");
                    }
                    sdk = found;
                }
            }
        }
        // The machine-wide job gate: one slot and the run's last recorded
        // peak (`analyze:<cwd>`) for the run's life, the `dart analyze`
        // child included.
        let peak_key = format!("analyze:{}", cwd.display());
        let _slot = match job_slot(Job::Analyze, &peak_key, &cwd, max_percent, job_cap(yaml)) {
            Ok(slot) => slot,
            Err(message) => {
                eprintln!("error: {message}");
                return exit::USAGE;
            }
        };
        if face.stats {
            let version = sdk
                .or_else(|| dart.as_deref().and_then(dartq_analyzer_host::Sdk::of_dart))
                .map_or_else(|| "unknown".to_owned(), |sdk| sdk.version.to_string());
            eprintln!("dartq analyze: engine official (Dart {version}, per-run)");
        }
        // The child's RSS budget (`analyzer.official.memory_budget_gb`,
        // unbounded unless set): the watchdog kills an over-budget tree.
        let run = official::run_with_budget(
            dart.as_deref(),
            &face.argv,
            root,
            Some(settings.memory_budget_bytes),
        );
        match run {
            Ok(run) => official_finish(&run, face.format, face.memory, out, err),
            Err(error) => {
                eprintln!("error: {error}");
                exit::USAGE
            }
        }
    });
    outcome.unwrap_or_else(|_| {
        eprintln!("dartq: internal error: analysis panicked");
        exit::INTERNAL_ERROR
    })
}

/// Prints one official run and answers its exit code: the official stderr
/// verbatim, one advisory per stdout line that is no row, and the rows
/// through [`emit_diagnostics`].
///
/// The official exit code is the answer on every path, because it is the
/// only complete one. Codes 0–3 are the same severity ladder as
/// [`severity_exit`], computed by dartdev under the same forwarded flags,
/// and the machine format leaves out the rows behind one of them: errors in
/// `pubspec.yaml` and `analysis_options.yaml` reach the exit code only. Any
/// other code is the child's failure (4 its crash tier, 64 its usage tier):
/// the code passes through and stdout stays empty unless it delivered rows.
fn official_finish(
    run: &OfficialRun,
    format: AnalyzeFormat,
    memory: bool,
    out: &mut impl Write,
    err: &mut impl Write,
) -> u8 {
    // A failed stderr write has nowhere left to report.
    let _ = err.write_all(&run.stderr);
    for line in &run.unparsed {
        let _ = writeln!(err, "note: unparsed dart analyze output: {line}");
    }
    let code = run
        .code
        .and_then(|code| u8::try_from(code).ok())
        .unwrap_or_else(|| {
            let _ = writeln!(
                err,
                "error: dart analyze ended with exit status {:?}",
                run.code
            );
            exit::INTERNAL_ERROR
        });
    if code > exit::ERRORS && run.diagnostics.is_empty() {
        return code;
    }
    if code == exit::ERRORS && severity_exit(&run.diagnostics, false, false) != exit::ERRORS {
        let _ = writeln!(
            err,
            "note: dart analyze found errors its machine and json formats omit \
             (pubspec.yaml or analysis_options.yaml); run dart analyze to see them"
        );
    }
    // The one writer (#2627): the same render the resident daemon runs
    // over the report rows — the cold path asks it of the kept rows, so
    // routed hits and per-run renders are byte-identical.
    let rows: Vec<&dartq_analyzer_host::Row> = run.rows().iter().collect();
    let render = dartq_commands::render::render_official_face(
        &rows,
        &dartq_analyzer_host::Face {
            format: face_format(format),
            root: run.output().files.analysis_root().map(Path::to_path_buf),
        },
    );
    let written = if memory && matches!(format, AnalyzeFormat::Json) {
        let mut line = String::from_utf8(render.bytes).expect("the writer emits UTF-8");
        splice_memory_key(&mut line);
        out.write_all(line.as_bytes())
    } else {
        out.write_all(&render.bytes)
    };
    if let Err(error) = written.and_then(|()| out.flush()) {
        let _ = writeln!(
            err,
            "dartq: internal error: writing diagnostics failed: {error}"
        );
        return exit::INTERNAL_ERROR;
    }
    code
}

/// The resident face path's print: the daemon rendered the final stdout
/// bytes and shipped them with the counts — the client relays the bytes
/// (the oracle's `--memory` splice appends client-side, like every run
/// fact), prints the unmodelable rows as advisories, and answers the
/// severity ladder over the report's severities — the same tiers dartdev
/// computes, so `--[no-]fatal-warnings`/`--fatal-infos` behave unchanged.
fn face_finish(
    stdout: &[u8],
    meta: &dartq_analyzer_host::FaceMeta,
    face: &OfficialFace,
    fatal_warnings: bool,
    fatal_infos: bool,
    out: &mut impl Write,
    err: &mut impl Write,
) -> u8 {
    for line in &meta.unparsed {
        let _ = writeln!(err, "note: unparsed dart analyze output: {line}");
    }
    let code = severity_tier(
        meta.priority_errors > 0 || meta.has_error,
        meta.has_warning,
        meta.has_info,
        fatal_warnings,
        fatal_infos,
    );
    if code == exit::ERRORS && !meta.has_diag_error {
        let _ = writeln!(
            err,
            "note: dart analyze found errors its machine and json formats omit \
             (pubspec.yaml or analysis_options.yaml); run dart analyze to see them"
        );
    }
    // The `--memory` splice appends client-side (the RSS is this
    // process's own); every other byte ships verbatim.
    let written = if face.memory && matches!(face.format, AnalyzeFormat::Json) {
        let mut line = String::from_utf8(stdout.to_vec()).expect("the writer emits UTF-8");
        splice_memory_key(&mut line);
        out.write_all(line.as_bytes())
    } else {
        out.write_all(stdout)
    };
    if let Err(error) = written.and_then(|()| out.flush()) {
        let _ = writeln!(
            err,
            "dartq: internal error: writing diagnostics failed: {error}"
        );
        return exit::INTERNAL_ERROR;
    }
    code
}

/// The `AnalyzeFormat` → `FaceFormat` spelling of the one format enum.
fn face_format(format: AnalyzeFormat) -> dartq_analyzer_host::FaceFormat {
    match format {
        AnalyzeFormat::Lon => dartq_analyzer_host::FaceFormat::Lon,
        AnalyzeFormat::Json => dartq_analyzer_host::FaceFormat::Json,
        AnalyzeFormat::Text => dartq_analyzer_host::FaceFormat::Text,
    }
}

// ---------------------------------------------------------------------------
// Job gate + analyzer memory budget
// ---------------------------------------------------------------------------

/// The `analyzer.official` settings of `dartq.yaml` (memory budget,
/// resident server, idle timeout). Fail-open like the gate: an absent
/// config is the defaults, a malformed one warns once and keeps them — the
/// analyze face never failed on `dartq.yaml` before and must not start now.
/// The governing `memory.max_percent` (the default when dartq.yaml is
/// unreadable: [`official_settings`] already said so).
pub(crate) fn memory_max_percent(config: Option<&DartqConfig>) -> u8 {
    config.map_or(
        dartq_commands::config::DEFAULT_MEMORY_MAX_PERCENT,
        DartqConfig::memory_max_percent,
    )
}

/// The shared-analyzer pool posture: `DARTQ_POOL=1` opts the run in
/// (with `DARTQ_POOL_MAX_ROOTS`, 1–64, overriding the cap), else
/// `analyzer.pool` in `dartq.yaml` decides. `None` is the one-daemon-per-
/// worktree posture.
fn pool_posture(config: Option<&DartqConfig>) -> Option<u32> {
    let (enabled, max_roots) = config.map_or((false, 3), DartqConfig::pool);
    let env_on = std::env::var("DARTQ_POOL").is_ok_and(|value| value == "1");
    if !env_on && !enabled {
        return None;
    }
    let cap = std::env::var("DARTQ_POOL_MAX_ROOTS")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(max_roots)
        .clamp(1, 64);
    Some(cap)
}

/// The warm-standby posture beside [`pool_posture`]: the resolved
/// `analyzer.pool.warm_servers` / `warm_ttl_secs` of `dartq.yaml`, carried
/// into the daemon's spec. `warm == 0` is the spec's not-declared — the
/// daemon's own default resolves — so `warm_servers: false` has no spec
/// spelling: `DARTQ_WARM_SERVERS=0` stays the off arm (the env wins
/// daemon-side over whatever the spec carries).
fn warm_posture(config: Option<&DartqConfig>) -> (u32, u32) {
    let (warm, ttl) = config.map_or((false, 900), DartqConfig::warm);
    (u32::from(warm), u32::try_from(ttl).unwrap_or(u32::MAX))
}

/// The governing `memory.max_concurrent` (unlimited when dartq.yaml is
/// unreadable), under the `DARTQ_MAX_CONCURRENT` override.
pub(crate) fn job_cap(config: Option<&DartqConfig>) -> u32 {
    let configured = config.map_or(0, DartqConfig::memory_max_concurrent);
    #[cfg(unix)]
    {
        dartq_toolchain::ledger::cap(configured)
    }
    #[cfg(not(unix))]
    {
        configured
    }
}

/// How long a job waits at the machine-wide gate before it fails.
#[cfg(unix)]
pub(crate) const JOB_WAIT: Duration = Duration::from_secs(30 * 60);

/// The jobs this binary takes slots for.
#[derive(Clone, Copy)]
pub(crate) enum Job {
    /// A per-run official analysis (`dart analyze` under dartq).
    Analyze,
    /// An in-process dartq engine run (a standalone engine face).
    Engine,
    /// One `dartq test` invocation, whatever its shard count.
    Test,
}

#[cfg(unix)]
impl Job {
    fn face(self) -> dartq_toolchain::ledger::Face {
        use dartq_toolchain::ledger::Face;
        match self {
            Self::Analyze => Face::Analyze,
            Self::Engine => Face::Engine,
            Self::Test => Face::Test,
        }
    }
}

/// One job slot of the machine-wide gate for this process's tree: the
/// governing `memory.max_concurrent` (`cap`, resolved by the calling face
/// from its one discovered config), and the recorded peak of `peak_key`
/// (else a live peer's of `face`, else the default, unsized) as its
/// memory. Waits with notes on stderr; a run nested inside another dartq
/// job's slot joins it. `Ok(None)` runs ungated (`--no-queue`, no
/// ledger); the error is a wait that timed out, naming the holders.
#[cfg(unix)]
pub(crate) fn job_slot(
    job: Job,
    peak_key: &str,
    dir: &Path,
    max_percent: u8,
    cap: u32,
) -> Result<Option<JobSlot>, String> {
    use dartq_toolchain::ledger::{self, Request};
    #[cfg(test)]
    if !tests::gate_under_test() {
        return Ok(None);
    }
    let face = job.face();
    let (bytes, sized) = ledger::sizing(peak_key, face);
    let request = Request {
        face,
        label: dir.display().to_string(),
        bytes,
        pid: Some(std::process::id()),
        release: None,
        slot: true,
        cap,
        sized,
        peak_key: Some(peak_key.to_owned()),
        source_bytes: None,
    };
    let deadline = std::time::Instant::now() + JOB_WAIT;
    match ledger::acquire(request, bytes, max_percent, deadline, |note| {
        eprintln!("{note}");
    }) {
        Ok(lease) => Ok(lease.map(|lease| JobSlot {
            _lease: lease,
            peak_key: peak_key.to_owned(),
            face,
        })),
        Err(error @ ledger::Error::Timeout { .. }) => Err(error.to_string()),
        Err(ledger::Error::Io(_) | ledger::Error::Stopped) => Ok(None),
    }
}

#[cfg(not(unix))]
pub(crate) fn job_slot(
    _: Job,
    _: &str,
    _: &Path,
    _: u8,
    _: u32,
) -> Result<Option<JobSlot>, String> {
    Ok(None)
}

/// A held job slot. Dropping it records the process tree's peak RSS
/// (this process and its waited-for children) under its key, for the
/// next run's lease.
pub(crate) struct JobSlot {
    #[cfg(unix)]
    _lease: dartq_toolchain::ledger::Lease,
    /// Read by the unix Drop's peak record; the field carries nothing on
    /// hosts without the ledger.
    #[cfg(unix)]
    peak_key: String,
    #[cfg(unix)]
    face: dartq_toolchain::ledger::Face,
}

impl Drop for JobSlot {
    fn drop(&mut self) {
        #[cfg(unix)]
        dartq_toolchain::ledger::record_peak(
            &self.peak_key,
            self.face,
            dartq_toolchain::proc::self_peak_rss().max(dartq_toolchain::proc::children_peak_rss()),
        );
    }
}

/// The workspace root an in-process engine assembled from `target` (a
/// directory, or a file whose parent seeds the assembly) projects against:
/// the root `analyze --engine dartq` keys its `engine:` peak on, so every
/// engine face of one worktree shares that peak.
pub(crate) fn engine_root(target: &Path) -> PathBuf {
    let target = target
        .canonicalize()
        .unwrap_or_else(|_| target.to_path_buf());
    let dir = if target.is_file() {
        target.parent().map_or_else(PathBuf::new, Path::to_path_buf)
    } else {
        target
    };
    workspace_root_of(&dir)
}

/// The job slot of a one-shot face that assembles the engine in-process
/// (no live session answered): the `analyze --engine dartq` slot, keyed
/// on [`engine_root`]. The error is a wait that timed out.
pub(crate) fn engine_slot(target: &Path) -> Result<Option<JobSlot>, String> {
    let root = engine_root(target);
    let yaml = dartq_commands::config::discover(&root).ok().flatten();
    job_slot(
        Job::Engine,
        &format!("engine:{}", root.display()),
        &root,
        memory_max_percent(yaml.as_ref()),
        job_cap(yaml.as_ref()),
    )
}

/// [`engine_slot`] for a CLI face: a timed-out wait is reported on stderr
/// and answers the internal tier, as in [`analyze_targets`].
pub(crate) fn engine_gate(target: &Path) -> Result<Option<JobSlot>, u8> {
    engine_slot(target).map_err(|reason| {
        eprintln!("dartq: {reason}");
        exit::INTERNAL_ERROR
    })
}

/// `--no-queue`: every later gate of this process runs ungated.
pub(crate) fn gate_bypass() {
    #[cfg(unix)]
    dartq_toolchain::ledger::bypass();
}

/// Whether this process runs inside another dartq job's slot.
pub(crate) fn gate_nested() -> bool {
    #[cfg(unix)]
    {
        dartq_toolchain::ledger::nested()
    }
    #[cfg(not(unix))]
    {
        false
    }
}

fn official_settings(
    config: &Result<Option<DartqConfig>, ConfigError>,
) -> dartq_commands::config::OfficialSettings {
    match config {
        Ok(Some(config)) => config.official(),
        Ok(None) => DartqConfig::default().official(),
        Err(error) => {
            eprintln!(
                "dartq analyze: dartq.yaml unreadable ({error}) — using the analyzer defaults"
            );
            DartqConfig::default().official()
        }
    }
}

/// `dartq analyze --engine` — crash tier like [`check`]: a
/// panic anywhere aborts with exit 4 and stdout kept empty, because
/// [`analyze_run`] writes only after analysis fully succeeded.
fn analyze_engine(args: &AnalyzeArgs) -> u8 {
    // `--fast` is the check pipeline under the analyze spelling: the
    // same scan + parse tiers, the same crash discipline.
    if args.fast {
        return check(&check_from_analyze(args));
    }
    let outcome = crash_tier(|| analyze_run(args, &mut BufWriter::new(io::stdout().lock())));
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
///
/// The resident consult (#2627) leads: the plain face asks the
/// worktree's session for the same targets first (spawning it when none
/// is live) and prints the daemon-rendered answer through the same
/// writers — byte-identical to the cold path below, which serves every
/// invocation the resident engine cannot honor and every fallback.
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
    // One root-resolution walk per distinct directory this invocation
    // (#4260): the resident route and the engine grouping share it.
    let mut roots = RootMemo::default();
    #[cfg(unix)]
    if !args.cold
        && let Some(code) =
            resident::route_analyze(args, &targets, out, &session::spawn_server, &mut roots)
    {
        if args.stats {
            eprintln!("dartq analyze: engine dartq (resident)");
        }
        return code;
    }
    if args.stats {
        eprintln!("dartq analyze: engine dartq (in-process)");
    }
    analyze_targets(args, &targets, out, &mut roots)
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

impl TargetKind {
    /// The directory the target's workspace resolves from: a directory
    /// target itself, a file target's parent (the oracle's file context
    /// root still finds the containing workspace for resolution).
    fn package_dir(&self) -> &Path {
        match self {
            Self::Dir(dir) => dir,
            Self::File(file) => file.parent().unwrap_or(Path::new("")),
        }
    }
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
        // The nearest-pubspec floor: the first ancestor carrying a
        // `pubspec.yaml` is the root `assemble` picks for this arm.
        Err(dartq_pub::PubError::Pubspec(_) | dartq_pub::PubError::OrphanMember(_)) => {
            package_root_of(dir)
        }
        Err(_) => dir.to_path_buf(),
    }
}

/// The nearest directory at or above `dir` carrying a `pubspec.yaml` — the
/// package context, and the floor `assemble` roots at when the pubspec does
/// not parse.
///
/// This is the package context `dart analyze <file>` resolves through, and
/// the narrower scope: a pub workspace member resolves to its own package
/// directory, never the workspace root. The analyzer maps a member include
/// onto its workspace-rooted context and includes just that member
/// (`context_locator.dart` `_sortIncludedFoldersIntoWorkspaceResolutions`
/// groups a `resolution: workspace` folder under the workspace root, and
/// `ContextRoot.analyzedFiles` yields only the included folders' files), so
/// `package:` resolution, `analysis_options.yaml` and the context root stay
/// the workspace's while the analyzed file set narrows to the package — the
/// closest resident posture to the oracle's aim context, whose file aim is
/// narrower still (it analyzes only the aimed file). `dir` itself when no
/// pubspec exists on the walk.
fn package_root_of(dir: &Path) -> PathBuf {
    dartq_commands::workspace::nearest_pubspec_dir(dir).unwrap_or_else(|| dir.to_path_buf())
}

/// One invocation's root-resolution memo (#4260): the resident route, the
/// engine grouping, the scope pass, and the analysis root ask for the same
/// targets' workspace/package roots — each miss is a full ancestor walk
/// (a pubspec probe per level), each hit one hash lookup. Per-invocation
/// by construction: the tree can change between runs, so the memo never
/// outlives one.
#[derive(Default)]
pub(crate) struct RootMemo {
    workspace: FxHashMap<PathBuf, PathBuf>,
    package: FxHashMap<PathBuf, PathBuf>,
}

impl RootMemo {
    /// [`workspace_root_of`], walked once per distinct directory.
    pub(crate) fn workspace_root(&mut self, dir: &Path) -> PathBuf {
        if let Some(root) = self.workspace.get(dir) {
            return root.clone();
        }
        let root = workspace_root_of(dir);
        self.workspace.insert(dir.to_path_buf(), root.clone());
        root
    }

    /// [`package_root_of`], walked once per distinct directory.
    pub(crate) fn package_root(&mut self, dir: &Path) -> PathBuf {
        if let Some(root) = self.package.get(dir) {
            return root.clone();
        }
        let root = package_root_of(dir);
        self.package.insert(dir.to_path_buf(), root.clone());
        root
    }
}

/// The scope one run resolves: an explicit `--scope` verbatim — it never
/// re-resolves — and the default per shape: every aim — file or directory —
/// whose package context is its workspace root (the single-pubspec shape,
/// where the package tier roots at the whole repo and gains nothing:
/// package == workspace == repo) takes the file tier `dart analyze <paths>`
/// itself opens; anything else — a pub workspace member's aim (its package
/// tier is genuinely narrower) — keeps the package tier. A no-pubspec aim
/// fires too: its package tier floors at the aim's own directory while the
/// file tier is the oracle's own posture.
fn resolve_scope(
    explicit: Option<AnalyzeScope>,
    targets: &[TargetKind],
    roots: &mut RootMemo,
) -> AnalyzeScope {
    explicit.unwrap_or_else(|| {
        let aims_over_their_workspace_root = targets.iter().all(|target| {
            let dir = target.package_dir();
            roots.package_root(dir) == roots.workspace_root(dir)
        });
        if aims_over_their_workspace_root {
            AnalyzeScope::File
        } else {
            AnalyzeScope::Package
        }
    })
}

/// The distinct analysis roots one resident request opens for `targets`: the
/// package context of every aim (`--scope package`, the default), the aim's
/// pub workspace root (`--scope workspace`), or the aims themselves
/// (`--scope file` — the resident server's `included` set is the aim paths,
/// the universe `dart analyze <paths>` itself opens).
///
/// Aims sharing a root share the request — and the daemon, which keys on its
/// roots — so one invocation stays one resident however many aims cover the
/// same package, and aims in different packages land on their own roots
/// together in one multi-root context, never on a shared workspace root.
/// Under `--scope file` a distinct aim set is a distinct resident (an aim
/// keys its own answer context), the same-aim dedupe the client's
/// sorted-roots keying always ran. `Request.roots` is order-free (the client
/// sorts and dedups before keying), so the aim order never splits a daemon.
fn scope_roots(targets: &[TargetKind], scope: AnalyzeScope, memo: &mut RootMemo) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = targets
        .iter()
        .map(|target| match scope {
            AnalyzeScope::Package => memo.package_root(target.package_dir()),
            AnalyzeScope::Workspace => memo.workspace_root(target.package_dir()),
            AnalyzeScope::File => match target {
                TargetKind::Dir(path) | TargetKind::File(path) => path.clone(),
            },
        })
        .collect();
    roots.sort();
    roots.dedup();
    roots
}

/// The official run's analysis root: the one workspace root its targets
/// share, the root the engine path would project against, or `None` when
/// they span workspaces (the no-root face, absolute spellings).
///
/// This is the render root — the `file`/heading spellings' locator base — and
/// deliberately not the analysis universe: a `--scope package` run narrows
/// what the resident server analyzes (see [`scope_roots`]) while its rows
/// keep spelling against the workspace, so the output bytes do not move with
/// the scope.
fn analysis_root(targets: &[TargetKind], memo: &mut RootMemo) -> Option<PathBuf> {
    let mut roots = targets
        .iter()
        .map(|target| memo.workspace_root(target.package_dir()));
    let first = roots.next()?;
    roots.all(|root| root == first).then_some(first)
}

/// The target-driven run: one engine per distinct workspace root, each
/// serving the targets it covers; every run filters its emitted set to the
/// covered files (the oracle pushes diagnostics only inside the targets'
/// context roots) and the merged union renders through the shared writers.
fn analyze_targets(
    args: &AnalyzeArgs,
    targets: &[TargetKind],
    out: &mut impl Write,
    roots: &mut RootMemo,
) -> u8 {
    let started = Instant::now();

    // Group targets by resolved workspace root.
    let mut groups: Vec<(PathBuf, Vec<Target>)> = Vec::new();
    for target in targets {
        let mapped = match target {
            TargetKind::Dir(dir) => Target::Dir(dir.clone()),
            TargetKind::File(file) => Target::File(file.clone()),
        };
        let root = roots.workspace_root(target.package_dir());
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
    // The in-process engine is a heavy job: it holds a job slot of the
    // machine-wide gate for the run, sized by the first root's recorded
    // peak (one per worktree).
    if args.no_queue {
        gate_bypass();
    }
    let gate_root = groups
        .first()
        .map_or_else(PathBuf::new, |(root, _)| root.clone());
    let yaml = dartq_commands::config::discover(&gate_root).ok().flatten();
    let _slot = match job_slot(
        Job::Engine,
        &format!("engine:{}", gate_root.display()),
        &gate_root,
        memory_max_percent(yaml.as_ref()),
        job_cap(yaml.as_ref()),
    ) {
        Ok(slot) => slot,
        Err(reason) => {
            eprintln!("dartq: {reason}");
            return exit::INTERNAL_ERROR;
        }
    };
    // The cross-process facts cache (#2630): the one-shot face opens the
    // same XDG-rooted store the daemon flushes — after the answered-turn
    // probe seam below (#4260), so a probe hit never pays the open. An
    // unreadable store warns once and leaves every engine cold — the
    // accelerator posture, never a dependency.
    // The answered-turn probe's cache root (#4260): the same root the L3
    // arm below opens under — the probe reads the record by path, never
    // opening the store, so `None` (nowhere to put a cache) simply skips
    // the probe.
    let cache_root = dartq_engine::store::default_root();
    let single = groups.len() == 1;
    let mut engines = Vec::with_capacity(groups.len());
    let mut members: Vec<Vec<Target>> = Vec::new();
    let mut roots: Vec<PathBuf> = Vec::new();
    let mut advisories: Vec<WorkspaceAdvisory> = Vec::new();
    let mut plugin_sets: Vec<dartq_commands::plugins::PluginSet> = Vec::with_capacity(groups.len());
    // The SDK snapshot loads, summed across workspace groups — the one
    // sub-interval of the setup window attributed to `sdk_ms`.
    let mut sdk = Duration::ZERO;
    // The answered-turn envelope (#4243): the shape this run probes and
    // persists under, with its read-time stamps — or the first v1 gate
    // that declined (deferred to the arming side for the trace row).
    let mut envelope: Option<(AnswerShape, Vec<(PathBuf, AnswerStamp)>)> = None;
    let mut gated: Option<&'static str> = None;
    for (root, group) in groups {
        // The assembled config — not the opened engine — because the plugin
        // set launches off `pub_config`/`config_dir` before `Engine::new`
        // consumes the config. The phased spelling carries the SDK-load
        // clock for the metrics split.
        let (config, dir, stale_advisory, phases) = match engine_config_with_overrides_phased(
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
        sdk += phases.sdk;
        if let Some(advisory) = stale_advisory {
            advisories.push(advisory);
        }
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
        // The answered-turn probe seam (#4260): a single-workspace run
        // probes the record BEFORE `Engine::new` — every key input (the
        // generation header, the grants fingerprint, the stamps) derives
        // from the assembled config through the engine's own spellings —
        // so a re-ask over an unchanged tree serves the previous run's
        // rendered stdout without the engine, the store arms, or any
        // later phase ever running. Any gate, any miss, any decode or
        // stamp mismatch falls through to the unchanged full path below;
        // the stamps a miss hands back are the run's read-time record,
        // reused verbatim by the persist side, never a post-render
        // re-walk.
        if single {
            match answer_envelope_shape(args, targets, 1, &plugin_set) {
                Ok(shape) => match &cache_root {
                    Some(root) => {
                        match dartq_engine::probe_answer_unassembled(&config, root, &shape) {
                            (AnswerProbe::Hit { bytes, summary }, _) => {
                                plugin_set.shutdown();
                                return answer_envelope_serve(
                                    args,
                                    shape.face,
                                    &bytes,
                                    &summary,
                                    &advisories,
                                    out,
                                );
                            }
                            (AnswerProbe::Miss(_), stamps) => {
                                envelope = Some((shape, stamps));
                            }
                        }
                    }
                    // Nowhere to put a cache: the unarmed-store posture —
                    // the persist side stays selected (the buffer render)
                    // but the engine's own persist is the documented no-op.
                    None => envelope = Some((shape, Vec::new())),
                },
                Err(reason) => gated = Some(reason),
            }
        }
        let engine = match dartq_engine::Engine::new(config) {
            Ok(engine) => engine,
            Err(error) => {
                eprintln!("error: engine context unusable: {error}");
                return exit::USAGE;
            }
        };
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

    // The v1 gates' trace rows: the single-workspace gate verdict the
    // pre-assembly seam recorded, and the multi-engine
    // decline (one record per ask; the merged multi-engine face has no
    // single engine to pin it to) — written on the first engine once its
    // trace sink is armed, the same rows and position the post-arming
    // probe block wrote before the seam moved the probe earlier.
    if let Some(reason) = gated {
        engines[0].answer_gated(reason);
    } else if engines.len() > 1 {
        if let Err(reason) = answer_envelope_shape(args, targets, engines.len(), &plugin_sets[0]) {
            engines[0].answer_gated(reason);
        }
    }

    // Warm start (#2630): the engine's own header pins the SDK identity,
    // the package graph, and the context salt, so a stale or foreign
    // generation is a whole-generation miss — never a poisoned load.
    // Staged facts serve the first apply's content-hash lookups (SDK
    // units included: the keys are path-free); a miss is the plain cold
    // build. The store opens here — past the probe seam (#4260), so a
    // served hit never pays it.
    let facts_store = match dartq_snapshot::SnapshotStore::open(dartq_snapshot::SnapshotConfig::default())
    {
        Ok(store) => Some(store),
        Err(error) => {
            eprintln!("warning: facts snapshot store unavailable: {error}");
            None
        }
    };
    for engine in &mut engines {
        if let Some(store) = &facts_store {
            let header = engine.snapshot_header();
            if let Some(snapshot) = store.load::<dartq_engine::PersistedFacts>(&header) {
                engine.ingest_facts_snapshot(snapshot);
            }
        }
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

    // The setup window (run start → first analysis step) closes here; the
    // SDK loads measured inside it take `sdk_ms`, the assembly phase keeps
    // the remainder, so the two never overlap.
    let sdk_ms = elapsed_ms(sdk);
    let assembly_ms = elapsed_ms(started.elapsed().saturating_sub(sdk));

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
    // The answered-turn persist posture (#4243): a probe-missed gated run
    // renders the stdout face into a buffer first, so the record pins the
    // exact bytes the face wrote (byte-identical to the direct render by
    // construction — the same writers, the same input), then writes the
    // buffer through. Ungated runs keep the direct render unchanged.
    let persisting = envelope.is_some();
    let mut face_bytes = Vec::new();
    let code = if persisting {
        analyze_finish(
            args,
            &merged,
            started,
            &mut face_bytes,
            &advisories,
            sdk_ms,
            assembly_ms,
            &merged.memory,
        )
    } else {
        analyze_finish(
            args,
            &merged,
            started,
            out,
            &advisories,
            sdk_ms,
            assembly_ms,
            &merged.memory,
        )
    };
    if persisting {
        if let Err(error) = out.write_all(&face_bytes).and_then(|()| out.flush()) {
            eprintln!("dartq: internal error: writing diagnostics failed: {error}");
            for set in plugin_sets {
                set.shutdown();
            }
            return exit::INTERNAL_ERROR;
        }
    }
    // The record's payload, assembled while `merged` still lives: the
    // summary counts, and the blob bytes — the captured stdout face, except
    // the json+memory face whose `memory` key is per-process: it records
    // the pre-splice envelope (the deterministic writer over the same
    // output) and re-splices on serve.
    let record = envelope.map(|(shape, stamps)| {
        let mut has_error = false;
        let mut has_warning = false;
        let mut has_info = false;
        for diagnostic in &merged.diagnostics {
            match diagnostic.severity {
                Severity::Error => has_error = true,
                Severity::Warning => has_warning = true,
                Severity::Info => has_info = true,
            }
        }
        let summary = AnswerSummary {
            files: merged.file_count as u64,
            loc: merged.loc,
            diagnostics: merged.diagnostics.len() as u64,
            libraries_analyzed: merged.libraries_analyzed as u64,
            libraries_from_cache: merged.libraries_from_cache as u64,
            has_error,
            has_warning,
            has_info,
        };
        let blob = if shape.face == AnswerFace::AnalyzeJsonMemory {
            let mut envelope_bytes = Vec::new();
            let output = AnalysisOutput {
                diagnostics: &merged.diagnostics,
                files: &merged,
            };
            match write_json(&mut envelope_bytes, &output) {
                Ok(()) => envelope_bytes,
                // A render failure never blocks the run — the record is a
                // pure accelerator; skip the persist, warn once.
                Err(error) => {
                    eprintln!("warning: answered-turn record skipped: {error}");
                    Vec::new()
                }
            }
        } else {
            face_bytes
        };
        (shape, stamps, blob, summary)
    });
    // Release the runs' engine borrows before the flush below — `merged`
    // holds each `AnalyzeRun`'s `&mut Engine` until dropped, and the save
    // walks `&engines` immutably.
    drop(merged);
    // The flush (#2630): each engine's resident facts persist under its own
    // generation header, so the next cold run on an equal context ingests
    // them instead of re-deriving. A failed save is a future miss, never an
    // error — warn and continue (the daemon's save_generation posture).
    if let Some(store) = &facts_store {
        for engine in &engines {
            if let Err(error) = engine.save_facts_snapshot(store) {
                eprintln!("warning: facts snapshot save skipped: {error}");
            }
        }
    }
    // The answered-turn record beside the facts flush (#4243): the blob in
    // the content store (dedup — an unchanged answer writes zero bytes),
    // the sidecar under the read-time stamps captured before the run. A
    // failed persist is a future miss, never an error.
    if let Some((shape, stamps, blob, summary)) = record {
        if !blob.is_empty() {
            engines[0].persist_answer(&shape, stamps, &blob, summary);
        }
    }
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
    match crash_tier(pass) {
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

/// Serializes crash-tier hook arm/disarm: hook install/remove is
/// process-global, so two passes arming at once could drop a restore. The
/// guard is held only across the arm and disarm, never across `pass` — a
/// nested crash tier inside `pass` (the analyze face wraps
/// `analyze_targets`'s own tier) must not deadlock on it; the thread-local
/// `ARMED` flag collapses the inner tier into a plain `catch_unwind`
/// (the outer arming's hook already covers every panic in the subtree).
static CRASH_TIER_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

thread_local! {
    /// This thread already armed the crash-tier hook around an enclosing
    /// pass: a nested tier skips re-arming — the outer hook fires on any
    /// panic in the subtree, and hook disarm belongs to the outermost tier.
    static ARMED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// One crash-tier pass with deterministic panic death: a panic anywhere in
/// `pass` — on the calling thread or on a rayon worker the pass spawned —
/// exits the process at the internal tier, never relies on unwind
/// propagation. `catch_unwind` alone cannot save the face: the panic rides
/// back through rayon's job join, which stays open until every sibling job
/// of the same fan-out completes — a sibling's own unbounded work would
/// hold the join open forever and leave the process spinning at 100% CPU
/// (the post-panic hang class). The armed hook prints the panic through
/// the previous (default) hook, then `process::exit`s — sibling jobs die
/// with the process, the crash tier's stdout-stays-empty contract holds
/// (the writers buffer until success). The hook is chained over whatever
/// was installed before and restored when `pass` returns; a panic that
/// fires under it never unwinds, so `catch_unwind` below stays the
/// belt-and-suspenders tier for a hook bypass (`panic=abort` builds,
/// non-hook panic paths), not the mechanism.
pub(crate) fn crash_tier<T>(pass: impl FnOnce() -> T) -> std::thread::Result<T> {
    if ARMED.get() {
        // Nested tier inside an armed ancestor: the ancestor's hook covers
        // this subtree — run the plain unwind tier rather than re-arm (and
        // rather than serialize on the lock `pass` itself may still hold).
        return std::panic::catch_unwind(std::panic::AssertUnwindSafe(pass));
    }
    // Arm under the lock, run unlocked, disarm under the lock — the guard
    // scope is the swap only, so a nested tier inside `pass` never waits on
    // it (the `ARMED` flag above is the re-entrancy answer).
    let previous: std::sync::Arc<dyn Fn(&std::panic::PanicHookInfo<'_>) + Sync + Send + 'static> = {
        let _guard = CRASH_TIER_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // `take_hook` moves the installed hook out — shared ownership so the
        // armed hook can chain to it AND the disarm can reinstall the same
        // hook.
        let previous: std::sync::Arc<
            dyn Fn(&std::panic::PanicHookInfo<'_>) + Sync + Send + 'static,
        > = std::panic::take_hook().into();
        let chained = std::sync::Arc::clone(&previous);
        std::panic::set_hook(Box::new(move |info| {
            chained(info);
            std::process::exit(i32::from(exit::INTERNAL_ERROR));
        }));
        previous
    };
    ARMED.set(true);
    let armed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(pass));
    ARMED.set(false);
    {
        let _guard = CRASH_TIER_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| previous(info)));
    }
    armed
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

/// The answered-turn ask this run probes and persists under (#4243): the
/// face tag plus the canonicalized aim set — or the first v1 conservative
/// gate that declines. The gates exist because each
/// flag changes the answer without moving any key or stamp input: overlays
/// edit sources off-disk, `--packages` and `--enable-experiment` re-shape
/// resolution, the memory budget and trace sink are run postures the
/// rendered face doesn't fold, and the metrics/memory-report flags ask for
/// per-run side channels a served hit cannot honestly produce. The
/// workspace-plugin gate is the set itself, not the `--no-plugins` flag:
/// a plugin-free workspace renders identically under either flag, while a
/// plugin-ful one would serve a plugin-free answer to a plugins-enabled
/// re-ask under the same key. Multi-root runs decline on shape — one
/// record per ask, and the merged multi-engine face has no single engine
/// to pin it to.
fn answer_envelope_shape(
    args: &AnalyzeArgs,
    targets: &[TargetKind],
    engines: usize,
    plugin_set: &dartq_commands::plugins::PluginSet,
) -> Result<AnswerShape, &'static str> {
    if engines != 1 {
        return Err("gated-multi-engine");
    }
    if !plugin_set.is_empty() {
        return Err("gated-plugins");
    }
    if !args.overlays.is_empty() {
        return Err("gated-overlays");
    }
    if args.packages.is_some() {
        return Err("gated-packages");
    }
    if !args.enable_experiments.is_empty() {
        return Err("gated-experiments");
    }
    if args.memory_limit.is_some() {
        return Err("gated-memory-limit");
    }
    if args.trace_events.is_some() {
        return Err("gated-trace-events");
    }
    if args.metrics_json.is_some() {
        return Err("gated-metrics-json");
    }
    if args.memory_report {
        return Err("gated-memory-report");
    }
    let face = match (args.format, args.memory) {
        (AnalyzeFormat::Lon, _) => AnswerFace::AnalyzeLon,
        (AnalyzeFormat::Text, _) => AnswerFace::AnalyzeText,
        (AnalyzeFormat::Json, true) => AnswerFace::AnalyzeJsonMemory,
        (AnalyzeFormat::Json, false) => AnswerFace::AnalyzeJson,
    };
    let aims = targets
        .iter()
        .map(|target| match target {
            TargetKind::Dir(dir) => AnswerAim::Dir(dir.clone()),
            TargetKind::File(file) => AnswerAim::File(file.clone()),
        })
        .collect();
    Ok(AnswerShape::new(face, aims))
}

/// Serves one answered-turn hit (#4243): the recorded stdout face verbatim
/// — the json+memory face re-applies [`splice_memory_key`] over the
/// recorded pre-splice envelope, so the `memory` key reports this
/// process's own peak RSS (the resident-routed face's splice posture) —
/// then the data faces' stderr advisories and the severity exit tier from
/// the recorded flags. Stdout is byte-identical to a full-path render over
/// the same tree by construction: the bytes are the previous render's own,
/// and every input the render folds (sources, options, pubspecs, package
/// config, SDK, shape) moved a stamp or the key since.
fn answer_envelope_serve(
    args: &AnalyzeArgs,
    face: AnswerFace,
    bytes: &[u8],
    summary: &AnswerSummary,
    advisories: &[WorkspaceAdvisory],
    out: &mut impl Write,
) -> u8 {
    // The text face's advisory lines ride stdout (already inside the
    // served bytes); the data faces keep them on stderr — the same stream
    // split `analyze_finish` applies.
    if face != AnswerFace::AnalyzeText {
        for advisory in advisories {
            eprintln!("Info • {} • {}", advisory.code().name, advisory.message());
        }
    }
    let written = if face == AnswerFace::AnalyzeJsonMemory {
        let mut line = String::from_utf8(bytes.to_vec()).expect("the writer emits UTF-8");
        splice_memory_key(&mut line);
        out.write_all(line.as_bytes())
    } else {
        out.write_all(bytes)
    };
    if let Err(error) = written.and_then(|()| out.flush()) {
        eprintln!("dartq: internal error: writing diagnostics failed: {error}");
        return exit::INTERNAL_ERROR;
    }
    severity_tier(
        summary.has_error,
        summary.has_warning,
        summary.has_info,
        !args.no_fatal_warnings,
        args.fatal_infos,
    )
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
    sdk_ms: u64,
    assembly_ms: u64,
    memory: &[(dartq_engine::MemoryStatus, dartq_engine::WarmStats)],
) -> u8 {
    let discovery_ms = elapsed_ms(run.discovery);
    let analyze_ms = elapsed_ms(run.analysis);
    let lints_ms = elapsed_ms(run.lints);

    for line in &run.degrade_notices {
        eprintln!("{line}");
    }

    // The data faces (lon, json) keep the advisory off stdout — one
    // `Info • code • message` line on stderr per advisory, the same stream
    // the degrade notices print on (the json envelope carries diagnostics
    // only, so the advisories have no row in it); the text face leads stdout
    // with the same human line, ahead of the groups. Either way the
    // degraded-resolution signal stays on every face.
    // The render window: the format writer plus the flush (the metrics
    // write below stays outside every phase).
    let render_at = Instant::now();
    let written = if args.format == AnalyzeFormat::Text {
        advisories
            .iter()
            .try_for_each(|advisory| write_stale_advisory_human(out, Some(advisory)))
    } else {
        for advisory in advisories {
            eprintln!("Info • {} • {}", advisory.code().name, advisory.message());
        }
        Ok(())
    };
    let output = AnalysisOutput {
        diagnostics: &run.diagnostics,
        files: run,
    };
    let written = written.and_then(|()| emit_diagnostics(args.format, args.memory, &output, out));
    if let Err(error) = written.and_then(|()| out.flush()) {
        eprintln!("dartq: internal error: writing diagnostics failed: {error}");
        return exit::INTERNAL_ERROR;
    }
    let render_ms = elapsed_ms(render_at.elapsed());

    if let Some(metrics_path) = &args.metrics_json {
        let metrics = RunMetrics {
            files: run.file_count,
            loc: run.loc,
            diagnostics: run.diagnostics.len(),
            discovery_ms,
            sdk_ms,
            assembly_ms,
            analyze_ms,
            lints_ms,
            render_ms,
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
    if args.memory_report {
        for (status, warm) in memory {
            eprintln!("{}", memory_report_line(status, warm));
        }
    }
    severity_exit(&run.diagnostics, !args.no_fatal_warnings, args.fatal_infos)
}

/// One diagnostic set through the format's writer — the one emission point
/// of the analyze and check faces, whichever backend produced the rows. The
/// oracle's `--memory` gate (analyze.dart:200 `printMemory = memory &&
/// jsonFormat`) appends the json envelope's `memory` key: the process's peak
/// RSS in KiB (`usageInfo.memoryKB`, analyze.dart:587) — spliced before the
/// closing brace so the shared writer's byte-parity envelope is untouched
/// when the flag is absent.
fn emit_diagnostics(
    format: AnalyzeFormat,
    memory: bool,
    output: &AnalysisOutput<'_>,
    out: &mut impl Write,
) -> io::Result<()> {
    match format {
        AnalyzeFormat::Lon => write_lon(out, &diags_document(output)),
        AnalyzeFormat::Text => write_human(out, output.diagnostics, output.files),
        AnalyzeFormat::Json if memory => write_json_memory(out, output),
        AnalyzeFormat::Json => write_json(out, output),
    }
}

/// The `--memory-report` stderr line over one engine's posture — the
/// deterministic shape the flag pins: the `dartq-memory` prefix, the
/// budget ladder's state (the armed limit in MiB, the descended-tier
/// count, the deepest tier's machine key), the ladder's last
/// phase-boundary RSS sample in bytes, and the warm lane's
/// produced/skipped counters. An engine that holds no reading spells
/// `none` — an unbudgeted engine samples nothing, so its face reports
/// nothing rather than inventing a reading.
fn memory_report_line(
    status: &dartq_engine::MemoryStatus,
    warm: &dartq_engine::WarmStats,
) -> String {
    let read =
        |value: Option<u64>| value.map_or_else(|| "none".to_owned(), |bytes| bytes.to_string());
    format!(
        "dartq-memory rss={} limit_mib={} tiers={} tier={} warm={}/{}",
        read(status.rss_bytes),
        read(status.limit_mib),
        status.degraded_tiers,
        status.degraded_tier.unwrap_or("none"),
        warm.completed,
        warm.skipped,
    )
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
    let mut line = String::from_utf8(line).expect("the writer emits UTF-8");
    splice_memory_key(&mut line);
    out.write_all(line.as_bytes())
}

/// The `--memory` member splice over a rendered JSON-envelope line
/// (analyze face): `,"memory":<peak RSS KiB>` before the closing brace —
/// the resident-routed face applies the same splice over the
/// daemon-rendered bytes (the RSS is the calling process's own, the
/// quantity the flag names).
fn splice_memory_key(line: &mut String) {
    if !line.ends_with("}\n") {
        return;
    }
    line.truncate(line.len() - 2);
    if let Some(kb) = max_rss_bytes().map(|bytes| bytes / 1024) {
        line.push_str(&format!(",\"memory\":{kb}"));
    }
    line.push_str("}\n");
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
    // The engine runtime flags (#1088): the memory budget replaces the
    // standing default only when passed; the L3 cap defaults to the
    // analyze face's unconditional posture (a resident engine that never
    // arms the cross-session caches re-derives its whole workspace on
    // every spawn — #4210); the sink's file is created here — the analyze
    // face's posture (create or usage error) — so a bad `--trace-events`
    // path fails before the session serves.
    // The stdio daemon is a heavy job for its whole life: one slot of the
    // machine-wide gate, sized by its worktree's recorded peak.
    let yaml = dartq_commands::config::discover(&args.dir).ok().flatten();
    let _slot = match job_slot(
        Job::Engine,
        &format!("engine:{}", args.dir.display()),
        &args.dir,
        memory_max_percent(yaml.as_ref()),
        job_cap(yaml.as_ref()),
    ) {
        Ok(slot) => slot,
        Err(reason) => {
            eprintln!("dartq: {reason}");
            return exit::INTERNAL_ERROR;
        }
    };
    let mut runtime = dartq_daemon::SessionRuntime::default();
    if let Some(limit) = args.memory_limit {
        runtime.memory_limit_mib = limit;
    }
    // The stdio daemon is a resident engine like the session serve: the
    // cross-session caches stay armed at the analyze face's default cap
    // unless `--cache-size-limit` narrows it (#4210 — an unarmed store
    // makes the staged-facts and library-run replays silent no-ops).
    runtime.content_store_limit = args
        .cache_size_limit
        .or(Some(dartq_engine::store::DEFAULT_SIZE_LIMIT));
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
    let mut session = match assembled {
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
    match dartq_daemon::run(stdin, stdout, &mut session, watch) {
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
            // Each context reloads through the embedder locator over the
            // base resolution (the default context's posture): a
            // `sky_engine` package replaces the `dart:` table for the
            // context axes too, and an unusable embedder yaml degrades to
            // the plain snapshot — the analysis never refuses over the
            // embedder.
            let loaded = match dartq_commands::workspace::embedder_lib_dir(
                &base.pub_config,
                &base.config_dir,
            ) {
                Some(lib_dir) => SdkSnapshot::load(&root, Some(&lib_dir))
                    .or_else(|_| SdkSnapshot::load(&root, None)),
                None => SdkSnapshot::load(&root, None),
            };
            let sdk = loaded.map_err(|error| {
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
                // The fork shares the base resolution, so it shares the
                // base's discovered context roots too.
                context_roots: base.context_roots.clone(),
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

/// `--fuzzy` rides the symbol face only: the discovery answer joins no
/// reference rows, so the reference-family faces refuse the flag up front
/// (the usage tier) instead of silently answering their exact semantics.
fn refuse_fuzzy(args: &SymbolArgs) -> Option<u8> {
    args.fuzzy.then(|| {
        eprintln!("error: --fuzzy answers `dartq query symbol` only");
        exit::USAGE
    })
}

/// the references face's runner: the routed consult with the references
/// kind, then the symbol runner verbatim (the seam where the faces'
/// rows diverge).
fn query_references_run(args: &SymbolArgs, out: &mut impl Write) -> u8 {
    if let Some(code) = refuse_fuzzy(args) {
        return code;
    }
    #[cfg(unix)]
    if let Some(code) = resident::route_query(args, QueryKind::References, out) {
        return code;
    }
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

/// One callers run: the routed consult with the callers kind, then the
/// symbol query's runner with the invocation-shaped filter applied
/// (`dartq_commands::query::callers`); exit 0 on success, zero results
/// included.
fn query_callers_run(args: &SymbolArgs, out: &mut impl Write) -> u8 {
    if let Some(code) = refuse_fuzzy(args) {
        return code;
    }
    #[cfg(unix)]
    if let Some(code) = resident::route_query(args, QueryKind::Callers, out) {
        return code;
    }
    let _slot = match engine_gate(&args.dir) {
        Ok(slot) => slot,
        Err(code) => return code,
    };
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
/// results included. The routed consult leads (#2629).
fn query_implementations_run(args: &SymbolArgs, out: &mut impl Write) -> u8 {
    if let Some(code) = refuse_fuzzy(args) {
        return code;
    }
    #[cfg(unix)]
    if let Some(code) = resident::route_query(args, QueryKind::Implementations, out) {
        return code;
    }
    let _slot = match engine_gate(&args.dir) {
        Ok(slot) => slot,
        Err(code) => return code,
    };
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
/// exit 0 on success, zero results included. The routed consult leads
/// (#2629).
fn query_dependencies_run(args: &SymbolArgs, out: &mut impl Write) -> u8 {
    if let Some(code) = refuse_fuzzy(args) {
        return code;
    }
    #[cfg(unix)]
    if let Some(code) = resident::route_query(args, QueryKind::Dependencies, out) {
        return code;
    }
    let _slot = match engine_gate(&args.dir) {
        Ok(slot) => slot,
        Err(code) => return code,
    };
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

/// One batch run: the resident consult first (#2629 — the session's one
/// graph answers every item in one round trip), then the request items
/// (`--symbol` order, each carrying the full `--kind` set, the symbol face
/// when no kind came), the shared `dartq_commands::query::batch`
/// execution, then one JSON line per answer.
fn query_batch_run(args: &BatchArgs, out: &mut impl Write) -> u8 {
    #[cfg(unix)]
    if let Some(code) = resident::route_query_batch(args, out) {
        return code;
    }
    let _slot = match engine_gate(&args.dir) {
        Ok(slot) => slot,
        Err(code) => return code,
    };
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
            let value = to_json_at_tier(&answer.run, answer.name, args.verbosity.into_commands());
            match args.format {
                OutputFormat::Json => write_json_value(out, &value),
                OutputFormat::Lon => write_lon(out, &value),
                OutputFormat::Text => write_text_value(out, &value),
            }
        })
        .and_then(|()| out.flush());
    if let Err(error) = written {
        eprintln!("dartq: internal error: writing query results failed: {error}");
        return exit::INTERNAL_ERROR;
    }
    exit::CLEAN
}

/// The query faces' shared output: the tier-shaped envelope
/// (`dartq_commands::query::to_json_at_tier`, the frozen v1 contract at
/// compact/full, the red line at minimal) through the format axis — the
/// same value on every face (the envelope's `sym` and the rows'
/// short-vocabulary fields are the producer's own spelling).
fn write_query_out(
    out: &mut impl Write,
    run: &dartq_commands::query::SymbolRun<'_>,
    name: &str,
    format: OutputFormat,
    verbosity: dartq_commands::Verbosity,
) -> io::Result<()> {
    let answer = to_json_at_tier(run, name, verbosity);
    match format {
        OutputFormat::Json => write_json_value(out, &answer),
        OutputFormat::Lon => write_lon(out, &answer),
        OutputFormat::Text => write_text_value(out, &answer),
    }
}

/// One query run: the resident consult first (#2629 — the session's
/// maintained index answers the same envelope the cold pass below prints),
/// then the analyze workspace assembly, one analyze pass (the
/// read-after-write — the query reflects and never re-drives it), and the
/// engine's symbol answer on `out`. Exit 0 on success, zero results
/// included. `--fuzzy` routes the same run to the discovery face: the
/// ranked candidates replace the exact lookup's rows, the envelope is
/// unchanged.
fn query_symbol_run(args: &SymbolArgs, out: &mut impl Write) -> u8 {
    #[cfg(unix)]
    if let Some(code) = resident::route_query(args, QueryKind::Symbol, out) {
        return code;
    }
    let _slot = match engine_gate(&args.dir) {
        Ok(slot) => slot,
        Err(code) => return code,
    };
    let (mut engine, _dir) = match open_engine(&args.dir, args.sdk.as_deref()) {
        Ok(faces) => faces,
        Err(error) => {
            eprintln!("error: {error}");
            return exit::USAGE;
        }
    };
    let run = if args.fuzzy {
        dartq_commands::query::symbol_fuzzy(&mut engine, &args.name)
    } else {
        dartq_commands::query::symbol(&mut engine, &args.name)
    };
    let run = match run {
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
/// crash tier, the tier mapping, and the format axis. The report is
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
                Some(positional) if args.dir == PathBuf::from(".") => PathBuf::from(positional),
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
            let _slot = match engine_gate(&dir) {
                Ok(slot) => slot,
                Err(code) => return code,
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
        (Some(symbol), None) => {
            // The resident consult (#2629), attach-only: the daemon's
            // explain rebuilds the same per-query disk engine and renders
            // through the same writers; no live session falls straight
            // through to the cold report below.
            #[cfg(unix)]
            if let Some(code) = resident::route_explain(args, symbol, out) {
                return code;
            }
            let _slot = match engine_gate(&args.dir) {
                Ok(slot) => slot,
                Err(code) => return code,
            };
            match dartq_commands::explain::explain(
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
            }
        }
        _ => {
            eprintln!("error: give a symbol name or --diagnostic CODE");
            return exit::USAGE;
        }
    };

    let written = match args.format {
        OutputFormat::Json => out
            .write_all(report.to_string().as_bytes())
            .and_then(|()| out.write_all(b"\n")),
        OutputFormat::Lon => write_lon(out, &report),
        OutputFormat::Text => write_text_value(out, &report),
    }
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
                        if before[left + 1..].chars().all(|c| c.is_ascii_digit())
                            && !before[left + 1..].is_empty() =>
                    {
                        (&before[..left], Some(&before[left + 1..]), Some(tail))
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

/// The `bcache` maintenance face.
#[derive(Args)]
struct BcacheArgs {
    #[command(subcommand)]
    cmd: BcacheFace,
}

#[derive(Subcommand)]
enum BcacheFace {
    /// Print the store's one-line JSON stats.
    Stats,
    /// Run the budget/LRU garbage-collection pass.
    Gc,
}

/// The `cache` aggregate face.
#[derive(Args)]
struct CacheArgs {
    #[command(subcommand)]
    cmd: CacheFace,
}

#[derive(Subcommand)]
enum CacheFace {
    /// Print one JSON line per cache face, then the total.
    Stats,
    /// Run every store's own eviction pass and report what it freed.
    Gc,
}

/// The `shim` interception face.
#[derive(Args)]
struct ShimArgs {
    #[command(subcommand)]
    cmd: ShimFace,
}

/// Which SDK tool a shim verb installs or removes: `flutter` routes the
/// flutter build-cache face (the default, the only tool before `dart`
/// existed), `dart` routes `dart analyze` through the job gate and the
/// analyzer RSS budget.
#[derive(Clone, Copy, ValueEnum)]
enum ShimTool {
    /// `~/.dartq/bin/flutter` → `dart analyze` stays official.
    Flutter,
    /// `~/.dartq/bin/dart` → only `dart analyze` is intercepted; every
    /// other dart subcommand execs the real SDK dart.
    Dart,
}

#[derive(Subcommand)]
enum ShimFace {
    /// Link `~/.dartq/bin/<tool>` at this executable (idempotent; a
    /// real file at the path is never clobbered).
    Install {
        /// Which tool to intercept (default `flutter`).
        #[arg(value_enum, default_value_t = ShimTool::Flutter)]
        tool: ShimTool,
    },
    /// Remove the `~/.dartq/bin/<tool>` link — only the link; a real
    /// file is never deleted.
    Remove {
        /// Which tool's link to remove (default `flutter`).
        #[arg(value_enum, default_value_t = ShimTool::Flutter)]
        tool: ShimTool,
    },
}

/// `dartq init`: project scaffolding. The report is human-facing prose
/// on stdout — the command is interactive setup, not a machine face.
fn init() -> u8 {
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(err) => {
            eprintln!("dartq: {err}");
            return exit::INTERNAL_ERROR;
        }
    };
    match dartq_commands::bcache::init(&cwd) {
        Ok(report) => {
            if report.config_written {
                println!("wrote {}", report.config_path.display());
            } else {
                println!("{} already present", report.config_path.display());
            }
            println!(
                "flutter cache: `dartq flutter <args>` (or `dartq shim install` for PATH \
                 interception); `dart analyze` interception: `dartq shim install dart`"
            );
            exit::CLEAN
        }
        Err(err) => {
            eprintln!("dartq: {err}");
            exit::INTERNAL_ERROR
        }
    }
}

/// `dartq shim install|remove`: the opt-in PATH-interception pair, like
/// `init` a human-facing prose face. Every outcome — including a
/// refused removal over a real file — reports and exits clean: the
/// outcome is the answer, not a failure.
fn shim(args: &ShimArgs) -> u8 {
    // The dart shim lives in dartq-toolchain (the flutter shim's sibling,
    // not its dependency — bcache stays untouched). Its report prints
    // through the same outcome lines.
    match args.cmd {
        ShimFace::Install {
            tool: ShimTool::Dart,
        } => {
            return dart_shim_report(dartq_toolchain::shim::dart_shim_install());
        }
        ShimFace::Remove {
            tool: ShimTool::Dart,
        } => {
            return dart_shim_report(dartq_toolchain::shim::dart_shim_remove());
        }
        _ => {}
    }
    let result = match args.cmd {
        ShimFace::Install { .. } => dartq_commands::bcache::shim_install(),
        ShimFace::Remove { .. } => dartq_commands::bcache::shim_remove(),
    };
    match result {
        Ok(report) => {
            use dartq_commands::bcache::ShimOutcome;
            let path = report.shim_path.display();
            match report.outcome {
                ShimOutcome::Installed => println!("installed shim: {path}"),
                ShimOutcome::AlreadyPresent => println!("shim already installed: {path}"),
                ShimOutcome::Removed => println!("removed shim: {path}"),
                ShimOutcome::NotInstalled => println!("no shim at {path}"),
                ShimOutcome::Blocked => {
                    println!("{path} occupied by a non-symlink; left untouched")
                }
            }
            if let Some(export) = report.path_missing {
                println!("{export}");
            }
            exit::CLEAN
        }
        Err(err) => {
            eprintln!("dartq: {err}");
            exit::INTERNAL_ERROR
        }
    }
}

/// Print a dart-shim setup report — the same prose the flutter shim
/// face prints, over `dartq_toolchain::shim`'s report type.
fn dart_shim_report(
    result: Result<dartq_toolchain::shim::DartShimReport, dartq_toolchain::shim::DartShimError>,
) -> u8 {
    use dartq_toolchain::shim::DartShimOutcome;
    match result {
        Ok(report) => {
            let path = report.shim_path.display();
            match report.outcome {
                DartShimOutcome::Installed => println!("installed shim: {path}"),
                DartShimOutcome::AlreadyPresent => {
                    println!("shim already installed: {path}")
                }
                DartShimOutcome::Removed => println!("removed shim: {path}"),
                DartShimOutcome::NotInstalled => println!("no shim at {path}"),
                DartShimOutcome::Blocked => {
                    println!("{path} occupied by a non-symlink; left untouched")
                }
            }
            if let Some(export) = report.path_missing {
                println!("{export}");
            }
            exit::CLEAN
        }
        Err(err) => {
            eprintln!("dartq: {err}");
            exit::INTERNAL_ERROR
        }
    }
}

/// `dartq bcache stats|gc`: the build cache's maintenance faces, one
/// JSON line on stdout each (the machine-channel convention). A missing
/// `build_cache` section maintains the default store; `enabled: false`
/// gates the flutter faces, never the maintenance of the store it left
/// behind.
fn bcache(args: &BcacheArgs) -> u8 {
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(err) => {
            eprintln!("dartq: {err}");
            return exit::INTERNAL_ERROR;
        }
    };
    let config = match dartq_commands::bcache::discover_store_config(&cwd) {
        Ok(config) => config,
        Err(ConfigError::Invalid(message)) => {
            eprintln!("error: {message}");
            return exit::USAGE;
        }
        Err(ConfigError::Io(err)) => {
            eprintln!("dartq: {err}");
            return exit::INTERNAL_ERROR;
        }
    };
    let value =
        match args.cmd {
            BcacheFace::Stats => dartq_commands::bcache::bcache_stats(&config)
                .map(|stats| serde_json::to_value(stats)),
            BcacheFace::Gc => dartq_commands::bcache::bcache_gc(&config)
                .map(|report| serde_json::to_value(report)),
        };
    let value = match value {
        Ok(Ok(value)) => value,
        Ok(Err(err)) => {
            eprintln!("dartq: {err}");
            return exit::INTERNAL_ERROR;
        }
        Err(err) => {
            eprintln!("dartq: {err}");
            return exit::INTERNAL_ERROR;
        }
    };
    if write_json_value(&mut io::stdout().lock(), &value).is_err() {
        return exit::INTERNAL_ERROR;
    }
    exit::CLEAN
}

/// `dartq cache stats|gc`: the aggregate over every cache face, JSON lines
/// on stdout (the machine-channel convention). A failed face rides its own
/// `error` line and turns the exit internal once the others have answered;
/// a malformed `dartq.yaml` stops the run before any face, as it does for
/// `bcache`.
fn cache(args: &CacheArgs) -> u8 {
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(err) => {
            eprintln!("dartq: {err}");
            return exit::INTERNAL_ERROR;
        }
    };
    let config = match dartq_commands::cache::CacheConfig::discover(&cwd) {
        Ok(config) => config,
        Err(ConfigError::Invalid(message)) => {
            eprintln!("error: {message}");
            return exit::USAGE;
        }
        Err(ConfigError::Io(err)) => {
            eprintln!("dartq: {err}");
            return exit::INTERNAL_ERROR;
        }
    };
    let mut out = io::stdout().lock();
    let (written, failed) = match args.cmd {
        CacheFace::Stats => {
            let report = dartq_commands::cache::stats(&config);
            (report.write_lines(&mut out), report.failed)
        }
        CacheFace::Gc => {
            let report = dartq_commands::cache::gc(&config);
            (report.write_lines(&mut out), report.failed)
        }
    };
    if let Err(err) = written {
        eprintln!("dartq: {err}");
        return exit::INTERNAL_ERROR;
    }
    if failed == 0 {
        exit::CLEAN
    } else {
        exit::INTERNAL_ERROR
    }
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
        let args = engine_spelling(&args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>());
        Cli::try_parse_from(std::iter::once("dartq").chain(args.iter().map(String::as_str)))
            .map(|cli| cli.command)
    }

    pub(crate) fn check_args(args: &[&str]) -> CheckArgs {
        match parse_args_of(args) {
            Ok(Some(Command::Check(check))) => check,
            _ => panic!("expected check command"),
        }
    }

    /// The converged surface: every hidden alias still parses (compat,
    /// check, index, assists, daemon, compatibility, resolve-at,
    /// completion), and the new spellings (`compat`, `resolve`,
    /// `complete`) parse under their primary names.
    #[test]
    fn converged_verbs_and_aliases_all_parse() {
        for args in [
            vec!["check"],
            vec!["index"],
            vec!["assists", "/tmp/a.dart"],
            vec!["daemon", "/tmp"],
            vec!["compat", "/tmp/a.dart"],
            vec!["compatibility", "/tmp/a.dart"],
            vec!["resolve", "/tmp/a.dart:1:1"],
            vec!["resolve-at", "/tmp/a.dart:1:1"],
            vec!["complete", "/tmp/a.dart:1:1"],
            vec!["completion", "/tmp/a.dart:1:1"],
            vec!["packages"],
            vec!["analyze", "--fast"],
        ] {
            assert!(
                parse_args_of(&args).expect("parse").is_some(),
                "`{}` stays reachable",
                args.join(" ")
            );
        }
    }

    /// `analyze --fast` is the check pipeline: the delegation maps the
    /// analyze flags onto the check face one-to-one (the shared tri-axis).
    #[test]
    fn analyze_fast_maps_onto_the_check_pipeline() {
        let analyze_args = |args: &[&str]| match parse_args_of(args) {
            Ok(Some(Command::Analyze(args))) => args,
            _ => panic!("expected analyze command"),
        };
        let fast = analyze_args(&["analyze", "--engine", "--fast", "/pkg"]);
        assert!(fast.fast);
        let mapped = check_from_analyze(&fast);
        assert_eq!(
            mapped.engine,
            Some(EngineArg::Dartq),
            "the pipeline stays on the engine face"
        );
        assert!(matches!(mapped.format, CheckFormat::Lon), "lon default");
        assert_eq!(mapped.paths, vec![PathBuf::from("/pkg")]);
        assert!(!mapped.no_fatal_warnings, "fatal warnings stay on");

        let json = check_from_analyze(&analyze_args(&["analyze", "--fast", "--format=json"]));
        assert!(matches!(json.format, CheckFormat::Json));
        let text = check_from_analyze(&analyze_args(&["analyze", "--fast", "--format=text"]));
        assert!(matches!(text.format, CheckFormat::Text));
    }

    #[test]
    fn check_defaults() {
        let check = check_args(&["check"]);
        assert_eq!(check.engine, None, "the official backend is the default");
        assert!(matches!(check.format, CheckFormat::Lon), "lon default");
        assert!(!check.no_fatal_warnings, "fatal-warnings default on");
        assert!(
            !check.fatal_warnings,
            "the positive spelling defaults unset"
        );
        assert!(!check.fatal_infos, "fatal-infos default off");
        assert_eq!(check.paths, vec![PathBuf::from(".")], "default target cwd");
    }

    /// The `--memory-report` line is the pinned deterministic shape: the
    /// `dartq-memory` prefix, the ladder state (limit MiB, descended-tier
    /// count, deepest tier key), the last RSS sample in bytes, and the
    /// warm counters — a descended budget surfaces its exact tier.
    #[test]
    fn the_memory_report_line_pins_the_posture_shape() {
        let status = dartq_engine::MemoryStatus {
            rss_bytes: Some(52_428_800),
            limit_mib: Some(2048),
            degraded_tiers: 2,
            degraded_tier: Some("semantic_slices"),
        };
        let warm = dartq_engine::WarmStats {
            completed: 3,
            skipped: 17,
        };
        assert_eq!(
            memory_report_line(&status, &warm),
            "dartq-memory rss=52428800 limit_mib=2048 tiers=2 tier=semantic_slices warm=3/17"
        );

        // The no-pressure posture stays at tier 0 with no tier key.
        let status = dartq_engine::MemoryStatus {
            rss_bytes: Some(4_194_304),
            limit_mib: Some(64),
            degraded_tiers: 0,
            degraded_tier: None,
        };
        assert_eq!(
            memory_report_line(&status, &warm),
            "dartq-memory rss=4194304 limit_mib=64 tiers=0 tier=none warm=3/17"
        );
    }

    /// An engine that holds no reading spells `none` — an unbudgeted
    /// engine samples nothing, so the face reports nothing rather than
    /// inventing a reading.
    #[test]
    fn the_memory_report_line_spells_none_for_an_unbudgeted_engine() {
        let warm = dartq_engine::WarmStats::default();
        assert_eq!(
            memory_report_line(&dartq_engine::MemoryStatus::default(), &warm),
            "dartq-memory rss=none limit_mib=none tiers=0 tier=none warm=0/0"
        );
    }

    /// `--memory-report` parses on the analyze face, and the resident
    /// consult declines it — the flag reads the in-process engine's own
    /// budget ladder, so the run goes cold (a plain run stays routable).
    #[test]
    fn memory_report_parses_and_stays_in_process() {
        match parse_args_of(&["analyze", "--memory-report", "/pkg"]).expect("parse") {
            Some(Command::Analyze(args)) => {
                assert!(args.memory_report);
                assert!(
                    !resident::analyze_routable(&args),
                    "--memory-report reads the in-process engine: the consult declines"
                );
            }
            _ => panic!("expected analyze command"),
        }
        let args = match parse_args_of(&["analyze", "/pkg"]).expect("parse") {
            Some(Command::Analyze(args)) => args,
            _ => panic!("expected analyze command"),
        };
        assert!(
            resident::analyze_routable(&args),
            "the plain run keeps the resident consult"
        );
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

        let check = check_args(&["check", "--format", "text", "dir/"]);
        assert!(matches!(check.format, CheckFormat::Text));
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

    #[test]
    fn usage_errors() {
        for args in [
            &["check", "--format"][..],
            &["check", "--format", "yaml"][..],
            &["check", "--format=yaml"][..],
            &["check", "--bogus"][..],
            &["check", "-x"][..],
            &["check", "--format", "machine"][..],
            &["analyze", "--format", "machine", "/p"][..],
            &["analyze", "--format", "default", "/p"][..],
            &["frobnicate"][..],
            &["--bogus"][..],
            // The tri-axis is the whole set: lon|json|text. `yaml` is the
            // invalid-value role; `machine` and `default` are retired
            // spellings, both rejected.
            &["analyze", "--format", "yaml", "/p"][..],
            &["analyze", "--format=yaml", "/p"][..],
            &["analyze", "--format", "human", "/p"][..],
            &["analyze", "--format=human", "/p"][..],
            // The scope axis is package|workspace|file; `aim` is the
            // invalid-value role.
            &["analyze", "--scope", "aim", "/p"][..],
            &["analyze", "--scope=aim", "/p"][..],
            &["analyze", "--metrics-json"][..],
            &["check", "--metrics-json"][..],
            &["query"][..],
            &["query", "symbol"][..],
            &["query", "references"][..],
            &["query", "callers"][..],
            &["query", "bogus"][..],
            // `explain` parses bare since the face landed (74e3258a): the
            // symbol slot is optional and the run tier rejects the
            // no-selector form — the invalid-value role this line pins is
            // the retired `machine` spelling of the format axis, and the
            // required changed-file positional of `impact` (483a6642).
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

    /// The verb table's hidden faces and visible aliases, straight off the
    /// clap command tree: `check`/`index`/`assists`/`daemon` are hidden
    /// (protocol and compat faces), the primary names stay visible, and
    /// `compatibility`/`resolve-at`/`completion` ride as aliases.
    #[test]
    fn hidden_verb_table_is_declared() {
        use clap::CommandFactory as _;
        let command = Cli::command();
        let subcommand = |name: &str| {
            command
                .find_subcommand(name)
                .unwrap_or_else(|| panic!("`{name}` is declared"))
                .clone()
        };
        for hidden in ["check", "index", "assists", "daemon"] {
            assert!(subcommand(hidden).is_hide_set(), "`{hidden}` stays hidden");
        }
        for visible in [
            "analyze", "packages", "session", "events", "mcp", "query", "compat", "resolve",
            "complete",
        ] {
            assert!(
                !subcommand(visible).is_hide_set(),
                "`{visible}` stays visible"
            );
        }
        assert!(
            subcommand("compat")
                .get_aliases()
                .any(|alias| alias == "compatibility"),
            "`compatibility` rides as an alias"
        );
        assert!(
            subcommand("resolve")
                .get_aliases()
                .any(|alias| alias == "resolve-at")
        );
        assert!(
            subcommand("complete")
                .get_aliases()
                .any(|alias| alias == "completion")
        );
    }

    /// The format axis: the default is the constant `lon` on every
    /// face that carries the flag — no terminal sniffing, no piped flip.
    #[test]
    fn format_defaults_are_constant_lon() {
        let analyze = |args: &[&str]| match parse_args_of(args) {
            Ok(Some(Command::Analyze(args))) => args,
            _ => panic!("expected analyze command"),
        };
        assert!(
            matches!(analyze(&["analyze", "/p"]).format, AnalyzeFormat::Lon),
            "analyze defaults to lon"
        );
        let symbol = |args: &[&str]| match parse_args_of(args) {
            Ok(Some(Command::Query(QueryArgs {
                face: QueryFace::Symbol(args),
            }))) => args,
            _ => panic!("expected query symbol"),
        };
        assert!(
            matches!(symbol(&["query", "symbol", "X"]).format, OutputFormat::Lon),
            "query defaults to lon"
        );
        let explain = match parse_args_of(&["explain", "Dog"])
            .expect("parse")
            .expect("subcommand")
        {
            Command::Explain(args) => args,
            _ => panic!("expected explain"),
        };
        assert!(matches!(explain.format, OutputFormat::Lon));
    }

    /// The lon document's diags rows' `file` column, in order (the tests'
    /// reader for the analyze faces).
    fn diag_files(out: &[u8]) -> Vec<String> {
        let text = std::str::from_utf8(out).expect("UTF-8");
        let document: serde_json::Value = lon::from_str(text.trim_end()).expect("one lon document");
        document["diags"]
            .as_array()
            .expect("diags")
            .iter()
            .map(|row| row["file"].as_str().expect("file column").to_owned())
            .collect()
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
            engine: Some(EngineArg::Dartq),
            format: CheckFormat::Lon,
            metrics_json: None,
            fatal_warnings: false,
            no_fatal_warnings: false,
            fatal_infos: false,
            cold: true,
            no_queue: false,
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
            engine: Some(EngineArg::Dartq),
            format: CheckFormat::Lon,
            metrics_json: None,
            fatal_warnings: false,
            no_fatal_warnings: false,
            fatal_infos: false,
            cold: true,
            no_queue: false,
            dartdev_globals: DartdevGlobals::default(),
            paths: vec![root.clone()],
        };
        // broken.dart has one scanner/parsed syntax error → exit 3
        assert_eq!(check(&args), exit::ERRORS);

        let clean_args = CheckArgs {
            paths: vec![root.join("lib/clean.dart")],
            format: CheckFormat::Lon,
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

    /// The check document's locator root is the target the run was pointed
    /// at (`analyze_targets`): a directory target spells every row under
    /// itself (`lib/broken.dart`), and a bare file target anchors at the
    /// file's parent — never at a `common_root` the file set alone would
    /// derive one level deeper.
    #[test]
    fn check_document_root_is_the_target_directory() {
        let root = std::env::temp_dir().join(format!("dq_cli_check_root_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("lib")).expect("mkdir");
        fs::write(root.join("lib/broken.dart"), "void f() { x }\n").expect("write");
        let canonical = root.canonicalize().expect("root");

        let args = CheckArgs {
            engine: Some(EngineArg::Dartq),
            paths: vec![root.clone()],
            format: CheckFormat::Lon,
            metrics_json: None,
            fatal_warnings: false,
            no_fatal_warnings: false,
            fatal_infos: false,
            cold: true,
            no_queue: false,
            dartdev_globals: DartdevGlobals::default(),
        };
        let mut out: Vec<u8> = Vec::new();
        assert_eq!(check_run(&args, &mut out), exit::ERRORS);
        let doc = read_lon(&out);
        assert_eq!(
            doc["root"],
            canonical.display().to_string(),
            "a directory target is the analysis root"
        );
        assert_eq!(
            doc["diags"][0]["file"], "lib/broken.dart",
            "rows spell under the target directory"
        );

        // The bare file target: the root anchors at the file's parent.
        let file_args = CheckArgs {
            paths: vec![root.join("lib/broken.dart")],
            ..args
        };
        let mut out: Vec<u8> = Vec::new();
        assert_eq!(check_run(&file_args, &mut out), exit::ERRORS);
        let doc = read_lon(&out);
        assert_eq!(
            doc["root"],
            canonical.join("lib").display().to_string(),
            "a bare file target anchors at its parent"
        );
        assert_eq!(doc["diags"][0]["file"], "broken.dart");

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
            engine: Some(EngineArg::Dartq),
            format: CheckFormat::Lon,
            metrics_json: Some(metrics_path.clone()),
            fatal_warnings: false,
            no_fatal_warnings: false,
            fatal_infos: false,
            cold: true,
            no_queue: false,
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
        assert!(metrics["sdk_ms"].is_u64());
        assert!(metrics["assembly_ms"].is_u64());
        assert!(metrics["analyze_ms"].is_u64());
        assert!(metrics["lints_ms"].is_u64());
        assert!(metrics["render_ms"].is_u64());
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
            engine: Some(EngineArg::Dartq),
            format: CheckFormat::Lon,
            metrics_json: None,
            fatal_warnings: false,
            no_fatal_warnings: false,
            fatal_infos: false,
            cold: true,
            no_queue: false,
            dartdev_globals: DartdevGlobals::default(),
            paths: vec![root.join("notes.txt")],
        };
        assert_eq!(check(&args), exit::CLEAN);

        fs::remove_dir_all(&root).ok();
    }

    // -----------------------------------------------------------------------
    // The backend seam: the official analyzer by default, --engine beside it
    // -----------------------------------------------------------------------

    fn analyze_face(args: &[&str]) -> AnalyzeArgs {
        match parse_args_of(args) {
            Ok(Some(Command::Analyze(args))) => args,
            _ => panic!("expected analyze command"),
        }
    }

    /// The engine selection pins: a bare `--engine` is `dartq` (the 0.1.12
    /// spelling), both values parse, an unknown value is the usage tier, and
    /// the flag beats `analyzer.engine` in `dartq.yaml`, which beats the
    /// `official` default.
    #[test]
    fn engine_flag_values_and_config_precedence() {
        use dartq_commands::config::Engine;
        let flag = |argv: &[&str]| analyze_face(argv).engine;
        assert_eq!(flag(&["analyze", "/p"]), None);
        assert_eq!(flag(&["analyze", "--engine", "/p"]), Some(EngineArg::Dartq));
        assert_eq!(
            flag(&["analyze", "--engine", "dartq", "/p"]),
            Some(EngineArg::Dartq)
        );
        assert_eq!(
            flag(&["analyze", "--engine=official", "/p"]),
            Some(EngineArg::Official)
        );
        assert_eq!(
            flag(&["analyze", "--engine", "official", "/p"]),
            Some(EngineArg::Official)
        );
        assert_eq!(
            analyze_face(&["analyze", "--engine", "/p"]).targets,
            [PathBuf::from("/p")]
        );
        assert_eq!(
            check_args(&["check", "--engine", "/p"]).engine,
            Some(EngineArg::Dartq)
        );
        let owned = |argv: &[&str]| argv.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>();
        assert_eq!(
            run(&owned(&["analyze", "--engine=rust", "/p"])),
            exit::USAGE
        );

        assert_eq!(Backend::of(None, None), Backend::Official);
        assert_eq!(Backend::of(None, Some(Engine::Dartq)), Backend::Engine);
        assert_eq!(Backend::of(None, Some(Engine::Official)), Backend::Official);
        assert_eq!(
            Backend::of(Some(EngineArg::Official), Some(Engine::Dartq)),
            Backend::Official
        );
        assert_eq!(
            Backend::of(Some(EngineArg::Dartq), Some(Engine::Official)),
            Backend::Engine
        );
    }

    /// `--cold` requires `--engine dartq` and `--no-resident` requires the
    /// official engine: each is the usage tier on the other, by name.
    #[test]
    fn engine_specific_flags_are_usage_errors_on_the_other_engine() {
        let owned = |argv: &[&str]| argv.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>();
        assert_eq!(
            run(&owned(&["analyze", "--engine=official", "--cold", "/p"])),
            exit::USAGE
        );
        assert!(engine_only_message("--cold").contains("--engine dartq"));
        assert_eq!(
            run(&owned(&[
                "analyze",
                "--engine=dartq",
                "--no-resident",
                "/p"
            ])),
            exit::USAGE
        );
        assert!(official_only_message("--no-resident").contains("--engine official"));
    }

    /// The flags the resident server cannot honor send the run per-run.
    #[test]
    fn resident_requests_skip_flags_the_server_cannot_honor() {
        let ask = |argv: &[&str]| analyze_face(argv).resident_request();
        let plain =
            ask(&["analyze", "--no-fatal-warnings", "--fatal-infos", "/p"]).expect("resident");
        assert!(!plain.fatal_warnings && plain.fatal_infos);
        for argv in [
            &["analyze", "--no-resident", "/p"][..],
            &["analyze", "--sdk-path", "/sdk", "/p"][..],
            &["analyze", "--cache", "/c", "/p"][..],
            &["analyze", "--enable-experiment=x", "/p"][..],
            &["analyze", "--no-use-aot-snapshot", "/p"][..],
            &["analyze", "--no-plugins", "/p"][..],
        ] {
            assert!(ask(argv).is_none(), "{argv:?}");
        }
    }

    /// The #4116 call site: help and version lines gain `--no-queue`.
    #[test]
    fn help_and_version_test_lines_bypass_the_queue() {
        let line = |args: &[&str]| test::TestArgs {
            args: args.iter().map(|a| (*a).to_owned()).collect(),
        };
        assert_eq!(
            queue_bypass(line(&["--help"])).args,
            ["--help", "--no-queue"]
        );
        assert_eq!(
            queue_bypass(line(&["test/a_test.dart"])).args,
            ["test/a_test.dart"]
        );
    }

    /// The one dispatch point: each backend runs alone, and a flag of the
    /// other engine is the usage tier.
    #[test]
    fn dispatch_routes_one_backend_and_refuses_engine_flags_on_the_official_one() {
        const OFFICIAL: u8 = 11;
        const ENGINE: u8 = 22;
        let route = |backend, flag, official_flag| {
            dispatch(backend, flag, official_flag, || OFFICIAL, || ENGINE)
        };
        assert_eq!(route(Backend::Official, None, None), OFFICIAL);
        assert_eq!(
            route(Backend::Official, None, Some("--no-resident")),
            OFFICIAL
        );
        assert_eq!(route(Backend::Engine, None, None), ENGINE);
        assert_eq!(route(Backend::Engine, Some("--cold"), None), ENGINE);
        assert_eq!(route(Backend::Official, Some("--cold"), None), exit::USAGE);
        assert_eq!(
            route(Backend::Engine, None, Some("--no-resident")),
            exit::USAGE
        );
    }

    /// Every engine-only flag on both faces: the classification names it,
    /// and the real entry answers the usage tier before anything resolves
    /// or spawns.
    #[test]
    fn engine_only_flags_are_usage_errors_by_name_without_engine() {
        let refused = |argv: &[&str], flag: &str| {
            let owned: Vec<String> = argv.iter().map(|arg| (*arg).to_owned()).collect();
            assert_eq!(run(&owned), exit::USAGE, "{argv:?}");
            let message = engine_only_message(flag);
            assert!(
                message.contains(flag) && message.contains("--engine dartq"),
                "{message}"
            );
        };
        for (flag, argv) in [
            ("--fast", &["analyze", "--fast", "/p"][..]),
            (
                "--metrics-json",
                &["analyze", "--metrics-json", "m", "/p"][..],
            ),
            ("--overlay", &["analyze", "--overlay", "a=b", "/p"][..]),
            (
                "--memory-limit",
                &["analyze", "--memory-limit", "64", "/p"][..],
            ),
            ("--memory-report", &["analyze", "--memory-report", "/p"][..]),
            (
                "--trace-events",
                &["analyze", "--trace-events", "t", "/p"][..],
            ),
            (
                "--cache-size-limit",
                &["analyze", "--cache-size-limit", "1MiB", "/p"][..],
            ),
            ("--cold", &["analyze", "--cold", "/p"][..]),
        ] {
            assert_eq!(analyze_face(argv).engine_only_flag(), Some(flag));
            refused(argv, flag);
        }
        for (flag, argv) in [
            (
                "--metrics-json",
                &["check", "--metrics-json", "m", "/p"][..],
            ),
            ("--cold", &["check", "--cold", "/p"][..]),
        ] {
            assert_eq!(check_args(argv).engine_only_flag(), Some(flag));
            refused(argv, flag);
        }
        assert_eq!(analyze_face(&["analyze", "/p"]).engine_only_flag(), None);
        assert_eq!(check_args(&["check", "/p"]).engine_only_flag(), None);
    }

    /// The forward class reaches the child with the targets last; the
    /// consumed class never does.
    #[test]
    fn official_args_forward_the_flag_table() {
        let args = analyze_face(&[
            "analyze",
            // Consumed.
            "--format=json",
            "--memory",
            "--diagnostics",
            "--enable-analytics",
            "--disable-analytics",
            // Forwarded.
            "--sdk",
            "/sdk",
            "--packages",
            "/config.json",
            "--cache",
            "/cache",
            "--enable-experiment=a,b",
            "--no-plugins",
            "--no-use-aot-snapshot",
            "--no-fatal-warnings",
            "--fatal-infos",
            "--verbose",
            "--suppress-analytics",
            "--disable-telemetry",
            "--no-analytics",
            "/pkg",
            "lib/a.dart",
        ]);
        assert_eq!(
            args.official_args(),
            [
                "--no-fatal-warnings",
                "--fatal-infos",
                "--verbose",
                "--suppress-analytics",
                "--disable-telemetry",
                "--no-analytics",
                "--sdk-path",
                "/sdk",
                "--packages",
                "/config.json",
                "--cache",
                "/cache",
                "--enable-experiment=a",
                "--enable-experiment=b",
                "--no-plugins",
                "--no-use-aot-snapshot",
                "/pkg",
                "lib/a.dart",
            ]
        );
        assert_eq!(analyze_face(&["analyze"]).official_args(), ["."]);
    }

    /// `check` forwards exactly what `analyze` forwards for the flags they
    /// share.
    #[test]
    fn check_forwards_what_analyze_forwards() {
        let shared = [
            "--format=json",
            "--no-fatal-warnings",
            "--fatal-infos",
            "--suppress-analytics",
            "--diagnostics",
            "/a",
            "/b",
        ];
        let argv = |verb| std::iter::once(verb).chain(shared).collect::<Vec<_>>();
        let forwarded = check_args(&argv("check")).official_args();
        assert_eq!(
            forwarded,
            [
                "--no-fatal-warnings",
                "--fatal-infos",
                "--suppress-analytics",
                "/a",
                "/b"
            ]
        );
        assert_eq!(forwarded, analyze_face(&argv("analyze")).official_args());
    }

    const INFO_ROW: &str = "INFO|LINT|AVOID_PRINT|{file}|1|1|3|m\n";
    const WARNING_ROW: &str = "WARNING|STATIC_WARNING|DEAD_CODE|{file}|1|1|3|m\n";
    const ERROR_ROW: &str = "ERROR|COMPILE_TIME_ERROR|UNDEFINED_IDENTIFIER|{file}|2|5|1|m\n";

    /// A canned official run over one source file (`{file}` in `rows` is its
    /// path), rooted at the file's directory. The rows are spelled in the
    /// machine grammar for brevity and reach the run as the `--format json`
    /// envelope the child prints; other lines pass through. No child runs.
    fn canned_run(rows: &str, code: Option<i32>, stderr: &str) -> (tempfile::TempDir, OfficialRun) {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().canonicalize().expect("canonical root");
        let file = root.join("a.dart");
        fs::write(&file, "var a;\nvar b;\n").expect("write");
        let mut parsed = Vec::new();
        let mut stdout = String::new();
        for line in rows.replace("{file}", &file.display().to_string()).lines() {
            let fields: Vec<&str> = line.split('|').collect();
            let [severity, kind, code, path, row, column, length, message] = fields[..] else {
                stdout.push_str(line);
                stdout.push('\n');
                continue;
            };
            let num = |text: &str| text.parse::<u32>().expect("number");
            let (row, column, length) = (num(row), num(column), num(length));
            let offset = (row - 1) * 7 + column - 1;
            parsed.push(dartq_analyzer_host::Row {
                severity: severity.into(),
                kind: kind.into(),
                location: dartq_analyzer_host::report::Location {
                    file: path.into(),
                    offset,
                    length,
                    start_line: Some(row),
                    start_column: Some(column),
                    end_line: Some(row),
                    end_column: Some(column + length),
                },
                message: message.into(),
                correction: None,
                code: code.to_ascii_lowercase(),
                url: None,
                context_messages: Vec::new(),
            });
        }
        let mut envelope = Vec::new();
        dartq_analyzer_host::report::write_json(&mut envelope, &parsed).expect("write");
        stdout.push_str(&String::from_utf8(envelope).expect("UTF-8"));
        let run = OfficialRun::parse(
            stdout.as_bytes(),
            stderr.as_bytes().to_vec(),
            code,
            Some(root),
        );
        (dir, run)
    }

    /// One run through the printer: exit code, stdout, stderr.
    fn printed(run: &OfficialRun, format: AnalyzeFormat, memory: bool) -> (u8, String, String) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = official_finish(run, format, memory, &mut out, &mut err);
        (
            code,
            String::from_utf8(out).expect("UTF-8"),
            String::from_utf8(err).expect("UTF-8"),
        )
    }

    /// Official rows print through the engine path's writers on all three
    /// faces, with the same root projection and a clean stderr.
    #[test]
    fn official_rows_print_through_the_shared_writers() {
        let (dir, run) = canned_run(ERROR_ROW, Some(3), "");
        let root = dir.path().canonicalize().expect("canonical root");

        let (code, lon, err) = printed(&run, AnalyzeFormat::Lon, false);
        assert_eq!((code, err.as_str()), (exit::ERRORS, ""));
        let document: serde_json::Value = lon::from_str(lon.trim_end()).expect("one document");
        assert_eq!(document["root"], root.display().to_string());
        assert_eq!(
            document["diags"][0],
            serde_json::json!({
                "code": "undefined_identifier",
                "sev": "error",
                "file": "a.dart",
                "line": 2,
                "col": 5,
                "len": 1,
                "msg": "m",
            })
        );

        let (_, text, _) = printed(&run, AnalyzeFormat::Text, false);
        assert_eq!(
            text,
            format!(
                "root: {}\na.dart\n  error undefined_identifier 2:5 m\n1 issue found.\n",
                root.display()
            )
        );

        let (_, json, _) = printed(&run, AnalyzeFormat::Json, false);
        let envelope: serde_json::Value = serde_json::from_str(&json).expect("one envelope");
        assert_eq!(
            envelope["diagnostics"][0]["location"]["range"]["start"],
            serde_json::json!({"offset": 11, "line": 2, "column": 5})
        );
        assert!(envelope.get("memory").is_none(), "--memory unset");

        let (_, json, _) = printed(&run, AnalyzeFormat::Json, true);
        let envelope: serde_json::Value = serde_json::from_str(&json).expect("one envelope");
        assert_eq!(envelope.get("memory").is_some(), cfg!(unix), "{json}");
    }

    /// The clean run is each face's own clean shape.
    #[test]
    fn a_clean_official_run_prints_the_clean_faces() {
        let (_dir, run) = canned_run("", Some(0), "");
        let clean = |format| printed(&run, format, false);
        let empty = String::new();
        assert_eq!(
            clean(AnalyzeFormat::Lon),
            (exit::CLEAN, "{v:1 diags:[]}\n".to_owned(), empty.clone())
        );
        assert_eq!(
            clean(AnalyzeFormat::Json),
            (
                exit::CLEAN,
                "{\"version\":1,\"diagnostics\":[]}\n".to_owned(),
                empty.clone()
            )
        );
        assert_eq!(
            clean(AnalyzeFormat::Text),
            (exit::CLEAN, "No issues found!\n".to_owned(), empty)
        );
    }

    /// The official code is the engine ladder's answer over the same rows
    /// and flags, and it prints nothing on stderr (the primary path, not a
    /// fallback).
    #[test]
    fn official_tiers_match_the_engine_ladder() {
        for (rows, official, fatal_warnings, fatal_infos) in [
            ("", exit::CLEAN, true, false),
            (INFO_ROW, exit::CLEAN, true, false),
            (INFO_ROW, exit::FATAL_INFOS, true, true),
            (WARNING_ROW, exit::FATAL_WARNINGS, true, false),
            (WARNING_ROW, exit::CLEAN, false, false),
            (ERROR_ROW, exit::ERRORS, true, false),
            (ERROR_ROW, exit::ERRORS, false, true),
        ] {
            let (_dir, run) = canned_run(rows, Some(i32::from(official)), "");
            assert_eq!(
                severity_exit(&run.diagnostics, fatal_warnings, fatal_infos),
                official,
                "the ladder agrees on {rows:?}"
            );
            let (code, _, err) = printed(&run, AnalyzeFormat::Lon, false);
            assert_eq!(code, official);
            assert_eq!(err, "", "stderr stays clean for CI");
        }
    }

    /// A code the rows do not explain still passes through, with the
    /// official stderr verbatim; past the severity tiers stdout stays empty
    /// unless the child delivered rows.
    #[test]
    fn official_failures_pass_through_with_stderr_verbatim() {
        for official in [4, 64, 254, 255] {
            let (_dir, run) = canned_run("", Some(official), "boom\nstack\n");
            let (code, out, err) = printed(&run, AnalyzeFormat::Json, false);
            assert_eq!(i32::from(code), official);
            assert_eq!(out, "", "a failure never prints a document");
            assert_eq!(err, "boom\nstack\n");
        }

        let (_dir, run) = canned_run("", Some(2), "fatal\n");
        let (code, _, err) = printed(&run, AnalyzeFormat::Lon, false);
        assert_eq!(code, exit::FATAL_WARNINGS);
        assert_eq!(err, "fatal\n");

        let (_dir, run) = canned_run(ERROR_ROW, Some(4), "crash\n");
        let (code, out, _) = printed(&run, AnalyzeFormat::Json, false);
        assert_eq!(code, exit::INTERNAL_ERROR);
        assert!(out.contains("undefined_identifier"), "rows are not lost");

        let (_dir, run) = canned_run("", None, "");
        let (code, out, err) = printed(&run, AnalyzeFormat::Lon, false);
        assert_eq!(code, exit::INTERNAL_ERROR, "a signal ended the child");
        assert_eq!(out, "");
        assert!(err.contains("dart analyze"), "{err}");
    }

    /// Exit 3 without an ERROR row is upstream's machine format omitting
    /// `pubspec.yaml` and `analysis_options.yaml` errors: the code stands
    /// and one note explains it; a row that explains the code needs none.
    #[test]
    fn errors_the_machine_format_omits_keep_their_tier_with_a_note() {
        let (_dir, run) = canned_run(WARNING_ROW, Some(3), "");
        let (code, out, err) = printed(&run, AnalyzeFormat::Json, false);
        assert_eq!(code, exit::ERRORS, "the official code outranks the rows");
        assert!(out.contains("dead_code"), "{out}");
        assert!(err.contains("pubspec.yaml"), "{err}");
    }

    /// A stdout line that is no row rides stderr verbatim as one advisory;
    /// the rows around it still print and no line is lost.
    #[test]
    fn an_unparseable_line_is_one_stderr_advisory() {
        let rows = format!("Please report this at dartbug.com.\n{INFO_ROW}");
        let (_dir, run) = canned_run(&rows, Some(0), "");
        let (code, out, err) = printed(&run, AnalyzeFormat::Json, false);
        assert_eq!(code, exit::CLEAN);
        assert_eq!(
            err,
            "note: unparsed dart analyze output: Please report this at dartbug.com.\n"
        );
        assert_eq!(run.diagnostics.len() + run.unparsed.len(), 2);
        let envelope: serde_json::Value = serde_json::from_str(&out).expect("one envelope");
        assert_eq!(envelope["diagnostics"].as_array().expect("rows").len(), 1);
    }

    /// The official path's analysis root: the workspace root its targets
    /// share, none across workspaces.
    #[test]
    fn analysis_root_is_the_one_workspace_of_the_targets() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().canonicalize().expect("canonical root");
        let package = root.join("pkg");
        let other = root.join("other");
        fs::create_dir_all(package.join("lib")).expect("mkdir");
        fs::create_dir_all(package.join("test")).expect("mkdir");
        fs::create_dir_all(&other).expect("mkdir");
        fs::write(package.join("pubspec.yaml"), "name: pkg\n").expect("write");
        fs::write(package.join("lib/a.dart"), "").expect("write");

        let resolved = |paths: &[PathBuf]| {
            analysis_root(&resolve_targets(paths).expect("targets"), &mut RootMemo::default())
        };
        assert_eq!(
            resolved(&[package.join("lib/a.dart"), package.join("test")]),
            Some(package.clone())
        );
        assert_eq!(resolved(&[package, other]), None);
    }

    /// The resident request's roots: a file or directory aim inside a pub
    /// workspace member analyzes that member's package (`--scope package`,
    /// the default), `--scope workspace` keeps the workspace root, and aims
    /// in different packages share one request over both package roots — a
    /// workspace-root aim never drags a member's file back to the whole
    /// workspace.
    #[test]
    fn scope_roots_resolve_each_aims_package_context() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().canonicalize().expect("canonical root");
        let outer = tempfile::tempdir().expect("tempdir");
        // Outside the workspace root: no pubspec anywhere on its walk.
        let bare = outer.path().canonicalize().expect("canonical").join("bare");
        let a = root.join("packages/a");
        let b = root.join("packages/b");
        let standalone = root.join("standalone");
        for package in [&a, &b, &standalone] {
            fs::create_dir_all(package.join("lib")).expect("mkdir");
            fs::write(package.join("lib/x.dart"), "").expect("write");
        }
        fs::create_dir_all(&bare).expect("mkdir");
        fs::write(
            root.join("pubspec.yaml"),
            "name: root\nenvironment:\n  sdk: ^3.5.0\nworkspace:\n  - packages/a\n  - packages/b\n",
        )
        .expect("workspace root");
        for (package, name) in [(&a, "a"), (&b, "b")] {
            fs::write(
                package.join("pubspec.yaml"),
                format!(
                    "name: {name}\nenvironment:\n  sdk: ^3.5.0\nresolution: workspace\n"
                ),
            )
            .expect("member pubspec");
        }
        fs::write(standalone.join("pubspec.yaml"), "name: standalone\n").expect("pubspec");

        let resolved = |paths: &[PathBuf], scope| {
            scope_roots(
                &resolve_targets(paths).expect("targets"),
                scope,
                &mut RootMemo::default(),
            )
        };
        // A member's file and a member directory both resolve to the member.
        for aim in [a.join("lib/x.dart"), a.clone()] {
            assert_eq!(
                resolved(&[aim.clone()], AnalyzeScope::Package),
                vec![a.clone()],
                "{aim:?} analyzes its package"
            );
            assert_eq!(
                resolved(&[aim], AnalyzeScope::Workspace),
                vec![root.clone()],
                "the workspace posture keeps the workspace root"
            );
        }
        // The workspace root aim is its own package context.
        assert_eq!(
            resolved(&[root.clone()], AnalyzeScope::Package),
            vec![root.clone()]
        );
        // A standalone project: both scopes are its own pubspec directory.
        assert_eq!(
            resolved(&[standalone.clone()], AnalyzeScope::Package),
            vec![standalone.clone()]
        );
        assert_eq!(
            resolved(&[standalone.clone()], AnalyzeScope::Workspace),
            vec![standalone.clone()]
        );
        // No pubspec on the walk: the aim directory itself, never the cwd.
        assert_eq!(resolved(&[bare.clone()], AnalyzeScope::Package), vec![bare]);
        // The file tier: the aims themselves are the roots — a file aim is
        // its own root (never its package), a directory aim is the
        // directory, and duplicate spellings of one aim dedupe while two
        // files keep two roots (a file set keys its own resident).
        let x = a.join("lib/x.dart");
        assert_eq!(resolved(&[x.clone()], AnalyzeScope::File), vec![x.clone()]);
        assert_eq!(
            resolved(&[x.clone(), a.clone()], AnalyzeScope::File),
            vec![a.clone(), x.clone()],
            "a file aim beside its own directory keeps both roots"
        );
        assert_eq!(
            resolved(&[b.clone()], AnalyzeScope::File),
            vec![b.clone()],
            "a directory aim is its own file-tier root"
        );
        let y = b.join("lib/x.dart");
        assert_eq!(
            resolved(&[y.clone(), x.clone()], AnalyzeScope::File),
            vec![x, y],
            "two files aim at two roots, sorted"
        );
        assert_eq!(
            resolved(&[root.clone()], AnalyzeScope::File),
            vec![root.clone()]
        );
        // Two packages aimed at once: one request over both package roots,
        // deduped; the workspace posture folds them onto the shared root.
        assert_eq!(
            resolved(
                &[a.join("lib/x.dart"), b.join("lib/x.dart")],
                AnalyzeScope::Package
            ),
            vec![a, b.clone()]
        );
        assert_eq!(
            resolved(&[b.clone(), root.clone()], AnalyzeScope::Package),
            vec![root.clone(), b.clone()],
            "a member aim next to a workspace aim keeps both roots"
        );
        assert_eq!(
            resolved(&[b, root.clone()], AnalyzeScope::Workspace),
            vec![root]
        );
    }

    /// The default scope resolves per shape (see [`resolve_scope`]): aims
    /// — files or directories — whose package context is their workspace
    /// root — the single-pubspec repo, a workspace root's own package, a
    /// no-pubspec bare aim — take the file tier, where the package tier
    /// would root at the whole repo and gain nothing (a directory aim
    /// then analyzes its own tree, the universe `dart analyze <dir>`
    /// opens). A pub workspace member's aim (its package tier is
    /// genuinely narrower) keeps the package tier, alone or mixed with
    /// whole-repo aims. An explicit `--scope` passes verbatim and never
    /// re-resolves.
    #[test]
    fn resolve_scope_takes_the_file_tier_for_whole_repo_aims() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().canonicalize().expect("canonical root");
        let outer = tempfile::tempdir().expect("tempdir");
        let bare = outer.path().canonicalize().expect("canonical").join("bare");
        let a = root.join("packages/a");
        let standalone = root.join("standalone");
        for package in [&a, &standalone] {
            fs::create_dir_all(package.join("lib")).expect("mkdir");
            fs::write(package.join("lib/x.dart"), "").expect("write");
        }
        fs::create_dir_all(root.join("tool")).expect("mkdir");
        fs::write(root.join("tool/main.dart"), "").expect("write");
        fs::create_dir_all(bare.join("lib")).expect("mkdir");
        fs::write(bare.join("lib/x.dart"), "").expect("write");
        fs::write(
            root.join("pubspec.yaml"),
            "name: root\nenvironment:\n  sdk: ^3.5.0\nworkspace:\n  - packages/a\n",
        )
        .expect("workspace root");
        fs::write(
            a.join("pubspec.yaml"),
            "name: a\nenvironment:\n  sdk: ^3.5.0\nresolution: workspace\n",
        )
        .expect("member pubspec");
        fs::write(standalone.join("pubspec.yaml"), "name: standalone\n").expect("pubspec");

        let resolved = |paths: &[PathBuf]| resolve_targets(paths).expect("targets");
        // The single-pubspec shape: the file tier.
        assert_eq!(
            resolve_scope(None, &resolved(&[standalone.join("lib/x.dart")]), &mut RootMemo::default()),
            AnalyzeScope::File,
            "a single-pubspec file aim: package == workspace == the repo"
        );
        // The workspace root's own package is that shape too: its package
        // tier roots at the whole workspace.
        assert_eq!(
            resolve_scope(None, &resolved(&[root.join("tool/main.dart")]), &mut RootMemo::default()),
            AnalyzeScope::File,
            "a workspace root's own file aim: the package tier is the whole repo"
        );
        // A no-pubspec bare file: the package tier floors at the parent
        // directory while the file tier is the oracle's own posture.
        assert_eq!(
            resolve_scope(None, &resolved(&[bare.join("lib/x.dart")]), &mut RootMemo::default()),
            AnalyzeScope::File
        );
        // A member's file keeps the package tier — it is genuinely
        // narrower than the workspace.
        assert_eq!(
            resolve_scope(None, &resolved(&[a.join("lib/x.dart")]), &mut RootMemo::default()),
            AnalyzeScope::Package,
            "a member file aim keeps its narrower package tier"
        );
        // A single-pubspec directory aim takes the aims tier too — the
        // package tier would pay the whole repo for one module tree (the
        // aim directory is what `dart analyze <dir>` passes verbatim).
        assert_eq!(
            resolve_scope(None, &resolved(&[standalone.clone()]), &mut RootMemo::default()),
            AnalyzeScope::File,
            "a single-pubspec directory aim analyzes its own tree"
        );
        // A nested module dir inside it: the same shape one level down.
        assert_eq!(
            resolve_scope(None, &resolved(&[standalone.join("lib")]), &mut RootMemo::default()),
            AnalyzeScope::File,
            "a nested module dir takes the aims tier"
        );
        // A mixed file + dir aim set over the same whole-repo shape: the
        // oracle passes both verbatim, so the closure covers both.
        assert_eq!(
            resolve_scope(
                None,
                &resolved(&[standalone.join("lib/x.dart"), standalone.clone()]),
                &mut RootMemo::default(),
            ),
            AnalyzeScope::File,
            "a mixed file + dir aim set over one whole-repo shape fires together"
        );
        // A member directory keeps the package tier just the same — the
        // member package is genuinely narrower than the workspace.
        assert_eq!(
            resolve_scope(None, &resolved(&[a.clone()]), &mut RootMemo::default()),
            AnalyzeScope::Package,
            "a member directory aim keeps its narrower package tier"
        );
        // A no-pubspec bare directory aim: both tiers floor at the aim
        // itself — firing is the oracle's own posture and changes nothing.
        assert_eq!(
            resolve_scope(None, &resolved(&[bare.join("lib")]), &mut RootMemo::default()),
            AnalyzeScope::File
        );
        // The workspace root aimed as a directory: firing is harmless —
        // both tiers root at the identical tree (held end-to-end by the
        // root-aim byte gates).
        assert_eq!(
            resolve_scope(None, &resolved(&[root.clone()]), &mut RootMemo::default()),
            AnalyzeScope::File,
            "the workspace root aim fires on the identical-tree shape"
        );
        assert_eq!(
            resolve_scope(
                None,
                &resolved(&[standalone.join("lib/x.dart"), a.join("lib/x.dart")]),
                &mut RootMemo::default(),
            ),
            AnalyzeScope::Package,
            "one member aim beside a whole-repo file aim keeps the package tier"
        );
        // An explicit scope passes verbatim on the firing shape.
        for scope in [AnalyzeScope::Package, AnalyzeScope::Workspace, AnalyzeScope::File] {
            assert_eq!(
                resolve_scope(
                Some(scope),
                &resolved(&[standalone.join("lib/x.dart")]),
                &mut RootMemo::default(),
            ),
                scope,
                "an explicit --scope never re-resolves"
            );
        }
    }

    /// The warm-standby passthrough (see [`warm_posture`]): the resolved
    /// `analyzer.pool` knobs of the governing `dartq.yaml` become the
    /// spec's `warm` / `warm_ttl_secs` — `warm_servers: true` declares
    /// warm on, `false` carries the spec's not-declared `0` (the off arm
    /// is the `DARTQ_WARM_SERVERS` env), and a config-less or malformed
    /// file carries the same not-declared default (the daemon's own
    /// default, off since 0.1.23).
    #[test]
    fn warm_posture_carries_the_yaml_knobs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let at = |yaml: &str| {
            fs::write(dir.path().join("dartq.yaml"), yaml).expect("write");
            let config = dartq_commands::config::discover(dir.path())
                .ok()
                .flatten();
            warm_posture(config.as_ref())
        };
        assert_eq!(
            at("version: 1\nanalyzer:\n  pool:\n    enabled: true\n    warm_servers: true\n    warm_ttl_secs: 120\n"),
            (1, 120),
            "warm on with the declared TTL"
        );
        assert_eq!(
            at("version: 1\nanalyzer:\n  pool:\n    enabled: true\n    warm_servers: false\n    warm_ttl_secs: 900\n"),
            (0, 900),
            "the off arm carries not-declared — the env is the off switch"
        );
        assert_eq!(
            at("version: 1\nanalyzer:\n  pool:\n    enabled: true\n"),
            (0, 900),
            "the pool section's own defaults: not-declared warm, 900 s"
        );
        assert_eq!(
            at("not: [a, config\n"),
            (0, 900),
            "a malformed dartq.yaml fails open to the default posture"
        );
        fs::remove_file(dir.path().join("dartq.yaml")).expect("remove");
    }

    // -----------------------------------------------------------------------
    // dartq analyze
    // -----------------------------------------------------------------------

    /// The analyze command face: the constant `lon` default over the cwd
    /// positional, flag spellings (`--format=lon|json|text`, `--sdk-path`
    /// with the hidden `--sdk` alias, both fatal-warnings spellings).
    /// Malformed spellings are covered by `usage_errors`.
    #[test]
    fn analyze_args_parse() {
        let parsed = parse_args_of(&["analyze", "/pkg"])
            .expect("parse")
            .expect("subcommand");
        let Command::Analyze(args) = parsed else {
            panic!("expected analyze command")
        };
        assert!(
            matches!(args.format, AnalyzeFormat::Lon),
            "lon is the constant default"
        );
        assert_eq!(args.engine, None, "the official backend is the default");
        assert!(args.metrics_json.is_none() && args.sdk_path.is_none());
        assert_eq!(
            args.scope, None,
            "the scope is unset by default — the shape resolves it at run time"
        );
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
            "--format=text",
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
        assert!(matches!(args.format, AnalyzeFormat::Text));
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

        // The hidden compat alias: `--sdk`.
        let parsed = parse_args_of(&["analyze", "--sdk", "/sdk", "/pkg"])
            .expect("parse")
            .expect("subcommand");
        let Command::Analyze(args) = parsed else {
            panic!("expected analyze command")
        };
        assert_eq!(args.sdk_path, Some(PathBuf::from("/sdk")));

        // The analysis-scope posture: `workspace` is the whole-workspace
        // escape hatch, both spellings parse.
        for spelling in [
            &["analyze", "--scope", "workspace", "/pkg"][..],
            &["analyze", "--scope=workspace", "/pkg"][..],
        ] {
            let parsed = parse_args_of(spelling)
                .expect("parse")
                .expect("subcommand");
            let Command::Analyze(args) = parsed else {
                panic!("expected analyze command")
            };
            assert_eq!(args.scope, Some(AnalyzeScope::Workspace), "{spelling:?}");
            assert!(
                args.resident_request().is_some_and(|ask| ask.scope
                    == Some(AnalyzeScope::Workspace)),
                "--scope plumbs into the resident request"
            );
        }

        // The file tier: both spellings parse and plumb the same way, and
        // the `dir` alias lands on the same aims-verbatim variant.
        for spelling in [
            &["analyze", "--scope", "file", "/pkg/lib/a.dart"][..],
            &["analyze", "--scope=file", "/pkg/lib/a.dart"][..],
            &["analyze", "--scope", "dir", "/pkg/lib"][..],
        ] {
            let parsed = parse_args_of(spelling)
                .expect("parse")
                .expect("subcommand");
            let Command::Analyze(args) = parsed else {
                panic!("expected analyze command")
            };
            assert_eq!(args.scope, Some(AnalyzeScope::File), "{spelling:?}");
            assert!(
                args.resident_request()
                    .is_some_and(|ask| ask.scope == Some(AnalyzeScope::File)),
                "--scope file plumbs into the resident request"
            );
        }

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

    /// `cache` carries the two maintenance faces and needs one of them;
    /// `bcache` stays the per-face spelling beside it.
    #[test]
    fn cache_parses_its_stats_and_gc_faces() {
        assert!(matches!(
            parse_args_of(&["cache", "stats"]),
            Ok(Some(Command::Cache(CacheArgs {
                cmd: CacheFace::Stats
            })))
        ));
        assert!(matches!(
            parse_args_of(&["cache", "gc"]),
            Ok(Some(Command::Cache(CacheArgs { cmd: CacheFace::Gc })))
        ));
        assert!(parse_args_of(&["cache"]).is_err());
        assert!(matches!(
            parse_args_of(&["bcache", "stats"]),
            Ok(Some(Command::Bcache(BcacheArgs {
                cmd: BcacheFace::Stats
            })))
        ));
    }

    /// `shim` carries the opt-in interception pair and needs one of
    /// them — `dartq init` no longer installs anything. `flutter` is the
    /// default tool; `dart` selects the analyze-shim face.
    #[test]
    fn shim_parses_its_install_and_remove_faces() {
        assert!(matches!(
            parse_args_of(&["shim", "install"]),
            Ok(Some(Command::Shim(ShimArgs {
                cmd: ShimFace::Install {
                    tool: ShimTool::Flutter
                }
            })))
        ));
        assert!(matches!(
            parse_args_of(&["shim", "remove"]),
            Ok(Some(Command::Shim(ShimArgs {
                cmd: ShimFace::Remove {
                    tool: ShimTool::Flutter
                }
            })))
        ));
        assert!(matches!(
            parse_args_of(&["shim", "install", "dart"]),
            Ok(Some(Command::Shim(ShimArgs {
                cmd: ShimFace::Install {
                    tool: ShimTool::Dart
                }
            })))
        ));
        assert!(matches!(
            parse_args_of(&["shim", "remove", "dart"]),
            Ok(Some(Command::Shim(ShimArgs {
                cmd: ShimFace::Remove {
                    tool: ShimTool::Dart
                }
            })))
        ));
        assert!(parse_args_of(&["shim"]).is_err());
    }

    /// The explicit flutter face dispatches on the first raw token and
    /// passes everything after it through verbatim — flutter's own
    /// flags included — while any other head stays with the usual CLI
    /// (a bare `dartq flutter` forwards an empty argv to flutter).
    #[test]
    fn flutter_face_argv_dispatches_on_the_first_token() {
        let argv = |args: &[&str]| -> Vec<std::ffi::OsString> {
            args.iter().map(std::ffi::OsString::from).collect()
        };
        let full = argv(&["flutter", "build", "apk", "--release"]);
        let forwarded = flutter_face_argv(&full).expect("the flutter token selects the face");
        assert_eq!(
            forwarded,
            argv(&["build", "apk", "--release"]),
            "every trailing argument rides along verbatim"
        );
        let bare_args = argv(&["flutter"]);
        let bare = flutter_face_argv(&bare_args).expect("bare selects the face");
        assert!(
            bare.is_empty(),
            "bare `dartq flutter` forwards an empty argv"
        );
        assert!(flutter_face_argv(&argv(&["analyze", "flutter"])).is_none());
        assert!(flutter_face_argv(&argv(&["fluttertest"])).is_none());
        assert!(flutter_face_argv(&[]).is_none());
    }

    /// The dart shim face dispatches on the first raw token: `analyze`
    /// re-enters the CLI parse (the argv after `dart` IS the analyze
    /// line), every other dart subcommand passes through to the real SDK
    /// dart — the passthrough itself is exercised in dartq-toolchain's
    /// own tests; here the routing decision is pinned.
    #[test]
    fn dart_shim_face_dispatches_analyze_and_passes_the_rest() {
        let argv = |args: &[&str]| -> Vec<std::ffi::OsString> {
            args.iter().map(std::ffi::OsString::from).collect()
        };
        // `dart analyze …` selects the analyze face: the tokens after
        // `analyze` are the analyze argv verbatim.
        let analyze_argv = argv(&["analyze", "--format", "json", "pkg/"]);
        assert_eq!(
            analyze_argv.first().map(|t| t.as_os_str()),
            Some(std::ffi::OsStr::new("analyze")),
            "the first token is the analyze subcommand"
        );
        // Anything else is passthrough territory — `dart run`, `dart
        // test`, `dart --version` must never reach the analyze parse.
        for passthrough in [
            argv(&["run", "bin/main.dart"]),
            argv(&["test"]),
            argv(&["--version"]),
            argv(&["pub", "get"]),
        ] {
            assert_ne!(
                passthrough.first().map(|t| t.as_os_str()),
                Some(std::ffi::OsStr::new("analyze")),
                "{:?} must not route to analyze",
                passthrough
            );
        }
    }

    /// A job slot is a ledger lease labelled with its worktree for the
    /// slot's life; dropping it returns the slot and records the peak.
    #[cfg(unix)]
    #[test]
    fn a_job_slot_holds_a_ledger_slot_until_dropped() {
        use dartq_toolchain::ledger::{self, Face};
        if isolated("tests::a_job_slot_holds_a_ledger_slot_until_dropped") {
            return;
        }
        let dir = tempfile::tempdir().expect("worktree");
        let key = format!("analyze:{}", dir.path().display());
        let label = dir.path().display().to_string();
        let held = || {
            ledger::leases()
                .into_iter()
                .filter(|(face, held, _)| *face == Face::Analyze && *held == label)
                .count()
        };
        let slot = job_slot(Job::Analyze, &key, dir.path(), 90, 0).expect("admitted");
        assert_eq!(held(), usize::from(slot.is_some()));
        drop(slot);
        assert_eq!(held(), 0);
        assert!(ledger::peak(&key).is_some(), "the peak is recorded");
    }

    /// Set in the child [`isolated`] starts: the test runs its body there.
    const ISOLATED_ENV: &str = "DARTQ_TEST_ISOLATED";

    /// Whether this test process exercises the machine-wide job gate: only
    /// inside an [`isolated`] child, which has a private ledger. Everywhere
    /// else in this binary the in-process gate entry points run ungated, so
    /// parallel tests never queue behind each other or behind the user's
    /// real dartq jobs, and never release the user's idle residents.
    pub(crate) fn gate_under_test() -> bool {
        std::env::var_os(ISOLATED_ENV).is_some()
    }

    /// Runs test `name` of this binary alone in a fresh child process and
    /// panics with its output unless it passed. Process-global state is
    /// then the test's own: the environment, which it may set, and the
    /// ledger's record of the slots this process holds, which turns a
    /// second slot request into a memory-only booking whenever any other
    /// in-process test holds a slot. The child starts with a private
    /// ledger (a fresh directory and room to spare), no inherited job
    /// token or cap override, and no `DART_SDK`. `true` in the parent (the
    /// caller returns); `false` in the child (the caller runs its body).
    #[cfg(unix)]
    pub(crate) fn isolated(name: &str) -> bool {
        isolated_stderr(name).is_some()
    }

    /// [`isolated`], answering the passed child's stderr in the parent
    /// (`None` in the child).
    #[cfg(unix)]
    fn isolated_stderr(name: &str) -> Option<String> {
        use dartq_toolchain::ledger;
        if std::env::var_os(ISOLATED_ENV).is_some() {
            return None;
        }
        let ledger_dir = tempfile::tempdir().expect("ledger dir");
        let output = std::process::Command::new(std::env::current_exe().expect("test binary"))
            .args([name, "--exact", "--test-threads", "1", "--nocapture"])
            .env(ISOLATED_ENV, "1")
            .env(ledger::DIR_ENV, ledger_dir.path())
            .env(ledger::AVAILABLE_ENV, (1u64 << 40).to_string())
            .env_remove(ledger::TOKEN_ENV)
            .env_remove(ledger::CAP_ENV)
            .env_remove("DART_SDK")
            .output()
            .expect("run the isolated test");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && stdout.contains("1 passed"),
            "isolated test {name} failed ({}):\n{stdout}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        Some(String::from_utf8_lossy(&output.stderr).into_owned())
    }

    /// Names the report file of [`gate_holder_probe`].
    #[cfg(unix)]
    const HOLDER_ENV: &str = "DARTQ_TEST_GATE_HOLDER";

    /// Another dartq job holding one slot, in a process of its own: a
    /// no-op unless [`HOLDER_ENV`] names its report. It takes one sized
    /// slot under the governing cap, writes the token it passes its
    /// children to the report, and holds the slot until the report's
    /// `.release` sibling appears.
    #[cfg(unix)]
    #[test]
    fn gate_holder_probe() {
        use dartq_toolchain::ledger::{self, Face, Request};
        let Some(report) = std::env::var_os(HOLDER_ENV).map(PathBuf::from) else {
            return;
        };
        let request = Request {
            face: Face::Analyze,
            label: "holder".into(),
            bytes: 1 << 20,
            pid: Some(std::process::id()),
            release: None,
            slot: true,
            cap: ledger::cap(0),
            sized: true,
            peak_key: None,
            source_bytes: None,
        };
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let lease = ledger::acquire(request, 1 << 20, 90, deadline, |_| {})
            .expect("admitted")
            .expect("a ledger");
        let mut child = std::process::Command::new("true");
        ledger::inherit(&mut child);
        let token = child
            .get_envs()
            .find(|(key, _)| *key == ledger::TOKEN_ENV)
            .and_then(|(_, value)| value)
            .expect("the holder's token")
            .to_owned();
        let staged = report.with_extension("tmp");
        fs::write(&staged, token.as_encoded_bytes()).expect("report");
        fs::rename(&staged, &report).expect("report");
        let release = report.with_extension("release");
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        while !release.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        drop(lease);
    }

    /// A live [`gate_holder_probe`] process and the token it passes on.
    #[cfg(unix)]
    struct Holder {
        child: std::process::Child,
        release: PathBuf,
        token: String,
    }

    #[cfg(unix)]
    impl Holder {
        fn start(dir: &Path) -> Self {
            let report = dir.join("holder.out");
            let child = std::process::Command::new(std::env::current_exe().expect("test binary"))
                .args([
                    "tests::gate_holder_probe",
                    "--exact",
                    "--test-threads",
                    "1",
                    "--nocapture",
                ])
                .env(HOLDER_ENV, &report)
                .env_remove(ISOLATED_ENV)
                .env_remove(dartq_toolchain::ledger::TOKEN_ENV)
                .stdout(std::process::Stdio::null())
                .spawn()
                .expect("start the holder");
            let deadline = std::time::Instant::now() + Duration::from_secs(20);
            while !report.exists() {
                assert!(
                    std::time::Instant::now() < deadline,
                    "the holder never reported"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
            Self {
                child,
                release: report.with_extension("release"),
                token: fs::read_to_string(&report).expect("token"),
            }
        }

        /// Releases the slot after `delay`, from another thread.
        fn release_after(&self, delay: Duration) {
            let release = self.release.clone();
            std::thread::spawn(move || {
                std::thread::sleep(delay);
                let _ = fs::write(release, "");
            });
        }

        fn finish(mut self) {
            let _ = fs::write(&self.release, "");
            assert!(self.child.wait().expect("the holder exits").success());
        }
    }

    /// The fixture the cold engine faces answer over.
    #[cfg(unix)]
    fn engine_gate_fixture(tag: &str) -> PathBuf {
        analyze_fixture(
            tag,
            &[(
                "lib/a.dart",
                "class Foo {\n  void meth() {}\n}\nvoid foo() {}\n",
            )],
        )
    }

    #[cfg(unix)]
    fn cold_symbol_query(root: &Path) -> u8 {
        let args = SymbolArgs {
            name: "foo".into(),
            dir: root.to_path_buf(),
            format: OutputFormat::Lon,
            verbosity: Verbosity::Compact,
            sdk: Some(root.join("sdk")),
            fuzzy: false,
        };
        query_symbol_run(&args, &mut Vec::new())
    }

    #[cfg(unix)]
    const SLOT_WAIT_NOTE: &str = "dartq: waiting for a job slot";

    /// Under cap 1 with another live job holding the only slot, a cold
    /// `query symbol` waits with the note and runs once the slot is free.
    #[cfg(unix)]
    #[test]
    fn a_cold_query_waits_for_the_only_slot() {
        let name = "tests::a_cold_query_waits_for_the_only_slot";
        if let Some(stderr) = isolated_stderr(name) {
            assert!(stderr.contains(SLOT_WAIT_NOTE), "{stderr}");
            assert!(stderr.contains("dartq: admitted"), "{stderr}");
            return;
        }
        let root = engine_gate_fixture("gate-query-waits");
        // SAFETY: this child process runs this test alone.
        unsafe { std::env::set_var(dartq_toolchain::ledger::CAP_ENV, "1") };
        let holder = Holder::start(&root);
        let delay = Duration::from_secs(2);
        let started = std::time::Instant::now();
        holder.release_after(delay);
        assert_eq!(cold_symbol_query(&root), exit::CLEAN);
        assert!(started.elapsed() >= delay, "ran beside the holder");
        holder.finish();
        let _ = fs::remove_dir_all(&root);
    }

    /// Under cap 1, a cold verb carrying a live holder's token (a child
    /// of that job) joins its slot instead of waiting behind it.
    #[cfg(unix)]
    #[test]
    fn a_cold_query_inside_a_holders_slot_joins_it() {
        let name = "tests::a_cold_query_inside_a_holders_slot_joins_it";
        if let Some(stderr) = isolated_stderr(name) {
            assert!(!stderr.contains("dartq: waiting"), "{stderr}");
            return;
        }
        let root = engine_gate_fixture("gate-query-joins");
        let holder = Holder::start(&root);
        // SAFETY: this child process runs this test alone.
        unsafe {
            std::env::set_var(dartq_toolchain::ledger::CAP_ENV, "1");
            std::env::set_var(dartq_toolchain::ledger::TOKEN_ENV, &holder.token);
        }
        // The fallback release keeps a wrongful wait from hanging the run.
        let fallback = Duration::from_secs(30);
        holder.release_after(fallback);
        let started = std::time::Instant::now();
        assert_eq!(cold_symbol_query(&root), exit::CLEAN);
        assert!(started.elapsed() < fallback, "waited behind its own holder");
        holder.finish();
        let _ = fs::remove_dir_all(&root);
    }

    /// With no cap, a cold verb whose `engine:<root>` peak is recorded is
    /// sized and runs beside another sized job at once.
    #[cfg(unix)]
    #[test]
    fn a_sized_cold_query_runs_beside_a_sized_holder() {
        use dartq_toolchain::ledger::{self, Face};
        let name = "tests::a_sized_cold_query_runs_beside_a_sized_holder";
        if let Some(stderr) = isolated_stderr(name) {
            assert!(!stderr.contains("dartq: waiting"), "{stderr}");
            return;
        }
        let root = engine_gate_fixture("gate-query-sized");
        let key = format!("engine:{}", engine_root(&root).display());
        ledger::record_peak(&key, Face::Engine, 64 << 20);
        assert!(ledger::peak(&key).is_some(), "the peak is recorded");
        let holder = Holder::start(&root);
        let fallback = Duration::from_secs(30);
        holder.release_after(fallback);
        let started = std::time::Instant::now();
        assert_eq!(cold_symbol_query(&root), exit::CLEAN);
        assert!(started.elapsed() < fallback, "waited beside a sized holder");
        holder.finish();
        let _ = fs::remove_dir_all(&root);
    }

    /// The gate covers the explain face too: under cap 1 a cold explain
    /// waits for the only slot.
    #[cfg(unix)]
    #[test]
    fn a_cold_explain_waits_for_the_only_slot() {
        let name = "tests::a_cold_explain_waits_for_the_only_slot";
        if let Some(stderr) = isolated_stderr(name) {
            assert!(stderr.contains(SLOT_WAIT_NOTE), "{stderr}");
            return;
        }
        let root = engine_gate_fixture("gate-explain-waits");
        // SAFETY: this child process runs this test alone.
        unsafe { std::env::set_var(dartq_toolchain::ledger::CAP_ENV, "1") };
        let holder = Holder::start(&root);
        let delay = Duration::from_secs(2);
        let started = std::time::Instant::now();
        holder.release_after(delay);
        let args = ExplainArgs {
            symbol: Some("Foo".into()),
            diagnostic: None,
            file: None,
            dir: root.clone(),
            format: OutputFormat::Lon,
            sdk: Some(root.join("sdk")),
            verbosity: Verbosity::Compact,
        };
        assert_eq!(explain_run(&args, &mut Vec::new()), exit::CLEAN);
        assert!(started.elapsed() >= delay, "ran beside the holder");
        holder.finish();
        let _ = fs::remove_dir_all(&root);
    }

    /// `memory.max_percent` reaches the analyze face from `dartq.yaml`,
    /// 90 when unset or unreadable.
    #[test]
    fn memory_max_percent_reads_the_config() {
        let dir = tempfile::tempdir().expect("tempdir");
        let percent = |dir: &Path| {
            let config = dartq_commands::config::discover(dir).ok().flatten();
            memory_max_percent(config.as_ref())
        };
        fs::write(dir.path().join("dartq.yaml"), "version: 1\n").expect("dartq.yaml");
        assert_eq!(percent(dir.path()), 90);
        fs::write(
            dir.path().join("dartq.yaml"),
            "version: 1\nmemory:\n  max_percent: 60\n",
        )
        .expect("dartq.yaml");
        assert_eq!(percent(dir.path()), 60);
        fs::write(
            dir.path().join("dartq.yaml"),
            "version: 1\nmemory:\n  max_percent: 0\n",
        )
        .expect("dartq.yaml");
        assert_eq!(
            percent(dir.path()),
            90,
            "a config error falls back"
        );
    }

    /// The official settings resolve from `dartq.yaml`'s `analyzer.official`
    /// section: unbounded / resident / 15 minutes unset, a set budget in
    /// GiB.
    #[test]
    fn official_settings_read_the_config() {
        let dir = tempfile::tempdir().expect("tempdir");
        let at = |dir: &Path| {
            let config = dartq_commands::config::discover(dir);
            official_settings(&config)
        };
        let settings = at(dir.path());
        assert_eq!(
            (
                settings.memory_budget_bytes,
                settings.resident,
                settings.idle_minutes
            ),
            (0, true, 15)
        );
        fs::write(
            dir.path().join("dartq.yaml"),
            "version: 1\nanalyzer:\n  official:\n    memory_budget_gb: 20\n    resident: false\n",
        )
        .expect("dartq.yaml");
        let settings = at(dir.path());
        assert_eq!(
            (settings.memory_budget_bytes, settings.resident),
            (20 << 30, false)
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
    pub(crate) fn analyze_fixture(tag: &str, libraries: &[(&str, &str)]) -> PathBuf {
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

    pub(crate) fn analyze_args(
        root: &Path,
        format: AnalyzeFormat,
        metrics_json: Option<PathBuf>,
    ) -> AnalyzeArgs {
        AnalyzeArgs {
            engine: Some(EngineArg::Dartq),
            no_resident: false,
            scope: None,
            stats: false,
            resident_serve: None,
            targets: vec![root.to_path_buf()],
            format,
            fast: false,
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
            cold: true,
            no_queue: false,
            memory_limit: None,
            memory_report: false,
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
    /// the lon document on stdout, error-tier exit code, metrics JSON at
    /// exit.
    ///
    /// Hand trace against the serial oracle probe (dart 3.13.1): `void f()
    /// { x }` puts `x` at UTF-16 offset 11 → line 1, column 12, length 1 on
    /// BOTH rows — the syntax row ("Expected to find ';'.") and the semantic
    /// row ("Undefined name 'x'.") share the token's span and order by the
    /// message tie-break.
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
            &analyze_args(&root, AnalyzeFormat::Lon, Some(metrics_path.clone())),
            &mut out,
        );

        let text = String::from_utf8(out).expect("stdout is UTF-8");
        let document: serde_json::Value = lon::from_str(text.trim_end()).expect("one lon document");
        assert_eq!(document["v"], 1, "the vocabulary envelope key");
        assert_eq!(
            document["root"],
            root.canonicalize().expect("root").display().to_string(),
            "rows exist: the envelope carries the analysis root exactly once"
        );
        let diags = document["diags"].as_array().expect("diags rows");
        assert_eq!(diags.len(), 2, "the fixture's two oracle rows: {text}");
        assert_eq!(
            diags[0],
            serde_json::json!({
                "code": "expected_token", "sev": "error",
                "file": "lib/broken.dart",
                "line": 1, "col": 12, "len": 1,
                "msg": "Expected to find ';'.",
            }),
            "the flat vocabulary row, located file, message tie-break first"
        );
        assert_eq!(diags[1]["code"], "undefined_identifier");
        assert_eq!(diags[1]["msg"], "Undefined name 'x'.");
        assert!(
            diags.iter().all(|row| row.get("doc").is_none()),
            "no doc URL on this face — `code` is the key, `explain` returns the URL: {document}"
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

    /// The v1 gate ladder: each posture flag
    /// declines with its own trace reason, the clean default ask maps the
    /// face tag, and the aim set canonicalizes (order and duplicates fold
    /// — two spellings of one coverage are one key).
    #[test]
    fn answer_envelope_shape_gates_and_maps_faces() {
        let root = PathBuf::from("/w");
        let targets = |paths: &[(&str, bool)]| {
            paths
                .iter()
                .map(|&(path, file)| {
                    if file {
                        TargetKind::File(PathBuf::from(path))
                    } else {
                        TargetKind::Dir(PathBuf::from(path))
                    }
                })
                .collect::<Vec<_>>()
        };
        let no_aims: Vec<TargetKind> = Vec::new();

        // The clean ask: lon face, aims sorted (Dir before File by leg) and
        // deduped.
        let shape = answer_envelope_shape(
            &analyze_args(&root, AnalyzeFormat::Lon, None),
            &targets(&[("/w/lib", false), ("/w/a.dart", true), ("/w/lib", false)]),
            1,
            &Default::default(),
        )
        .expect("the default posture asks");
        assert_eq!(shape.face, AnswerFace::AnalyzeLon);
        assert_eq!(
            shape.aims,
            vec![
                AnswerAim::Dir(PathBuf::from("/w/lib")),
                AnswerAim::File(PathBuf::from("/w/a.dart")),
            ],
            "sorted, deduped"
        );

        // Each posture flag declines with its own reason.
        let declines = |args: &AnalyzeArgs, reason: &str| {
            assert_eq!(
                answer_envelope_shape(args, &no_aims, 1, &Default::default())
                    .expect_err("gated"),
                reason
            );
        };
        let mut args = analyze_args(&root, AnalyzeFormat::Lon, None);
        args.packages = Some(PathBuf::from("/w/.dart_tool/package_config.json"));
        declines(&args, "gated-packages");
        let mut args = analyze_args(&root, AnalyzeFormat::Lon, None);
        args.enable_experiments = vec!["records".to_owned()];
        declines(&args, "gated-experiments");
        let mut args = analyze_args(&root, AnalyzeFormat::Lon, None);
        args.overlays = vec!["lib/a.dart=-".to_owned()];
        declines(&args, "gated-overlays");
        let mut args = analyze_args(&root, AnalyzeFormat::Lon, None);
        args.memory_limit = Some(512);
        declines(&args, "gated-memory-limit");
        let mut args = analyze_args(&root, AnalyzeFormat::Lon, None);
        args.trace_events = Some(PathBuf::from("/w/trace.jsonl"));
        declines(&args, "gated-trace-events");
        let mut args = analyze_args(&root, AnalyzeFormat::Lon, None);
        args.metrics_json = Some(PathBuf::from("/w/metrics.json"));
        declines(&args, "gated-metrics-json");
        let mut args = analyze_args(&root, AnalyzeFormat::Lon, None);
        args.memory_report = true;
        declines(&args, "gated-memory-report");
        assert_eq!(
            answer_envelope_shape(
                &analyze_args(&root, AnalyzeFormat::Lon, None),
                &no_aims,
                2,
                &Default::default()
            )
            .expect_err("multi-root"),
            "gated-multi-engine"
        );

        // The face mapping.
        let face_of = |format, memory| {
            let mut args = analyze_args(&root, AnalyzeFormat::Lon, None);
            args.format = format;
            args.memory = memory;
            answer_envelope_shape(&args, &no_aims, 1, &Default::default())
                .expect("asks")
                .face
        };
        assert_eq!(face_of(AnalyzeFormat::Text, false), AnswerFace::AnalyzeText);
        assert_eq!(face_of(AnalyzeFormat::Json, false), AnswerFace::AnalyzeJson);
        assert_eq!(
            face_of(AnalyzeFormat::Json, true),
            AnswerFace::AnalyzeJsonMemory
        );
    }

    /// The serve face (#4243): the recorded stdout bytes write verbatim
    /// with the recorded severity flags' exit tier, and the json+memory
    /// face re-splices the `memory` member over the recorded pre-splice
    /// envelope — this process's own peak RSS, the resident-routed face's
    /// splice posture.
    #[test]
    fn answer_envelope_serve_writes_bytes_and_tier() {
        let root = PathBuf::from("/w");
        let tier_of = |has_error: bool,
                       has_warning: bool,
                       has_info: bool,
                       fatal_warnings: bool,
                       fatal_infos: bool| {
            let mut args = analyze_args(&root, AnalyzeFormat::Lon, None);
            args.no_fatal_warnings = !fatal_warnings;
            args.fatal_infos = fatal_infos;
            let summary = AnswerSummary {
                has_error,
                has_warning,
                has_info,
                ..AnswerSummary::default()
            };
            let mut out: Vec<u8> = Vec::new();
            let code = answer_envelope_serve(
                &args,
                AnswerFace::AnalyzeLon,
                b"lon-bytes",
                &summary,
                &[],
                &mut out,
            );
            assert_eq!(out, b"lon-bytes", "verbatim serve");
            code
        };
        assert_eq!(tier_of(true, true, true, true, true), exit::ERRORS);
        assert_eq!(
            tier_of(false, true, false, true, false),
            exit::FATAL_WARNINGS
        );
        assert_eq!(tier_of(false, false, true, true, true), exit::FATAL_INFOS);
        assert_eq!(tier_of(false, false, false, true, true), exit::CLEAN);

        // The json+memory splice: the envelope ends `}\n`; a host with an
        // RSS reading gains the member before the closing brace, a host
        // without keeps the envelope (the oracle's null-usage gate).
        let mut args = analyze_args(&root, AnalyzeFormat::Json, None);
        args.memory = true;
        let envelope = "{\"v\":1,\"diagnostics\":[]}\n";
        let mut out: Vec<u8> = Vec::new();
        answer_envelope_serve(
            &args,
            AnswerFace::AnalyzeJsonMemory,
            envelope.as_bytes(),
            &AnswerSummary::default(),
            &[],
            &mut out,
        );
        let served = String::from_utf8(out).expect("utf-8");
        if max_rss_bytes().is_some() {
            assert!(
                served.starts_with("{\"v\":1,\"diagnostics\":[]")
                    && served.contains("\"memory\":")
                    && served.ends_with("}\n"),
                "the member splices inside the envelope: {served}"
            );
        } else {
            assert_eq!(served, envelope);
        }
    }

    /// End to end over a real temp workspace (#4243): the cold run renders
    /// and persists the answered-turn record (the sidecar appears under
    /// the default cache root's answers dir), the warm re-ask over an
    /// unchanged tree serves byte-identical stdout at the same exit tier —
    /// proven a serve, not a deterministic re-render, by a permission flip
    /// that moves no stamp field yet makes the file's rows unrenderable —
    /// and source edits move stamps: the next ask re-renders, and with the
    /// errors gone the tier clears.
    #[test]
    fn analyze_warm_reask_serves_the_persisted_answer() {
        let root = analyze_fixture(
            "envelope",
            &[
                ("lib/broken.dart", "void f() { x }\n"),
                // Both files contribute rows: the permission-flip leg needs
                // the flipped file's diagnostics inside the served bytes.
                ("lib/clean.dart", "int dup = 1;\nint dup = 2;\n"),
            ],
        );
        let args = analyze_args(&root, AnalyzeFormat::Lon, None);
        let targets = resolve_targets(&args.targets).expect("targets");

        let answers_dir = dartq_engine::store::default_root()
            .expect("cache root")
            .join("answers");
        let before = fs::read_dir(&answers_dir).map(|entries| entries.count()).unwrap_or(0);

        let mut out1: Vec<u8> = Vec::new();
        let code1 = analyze_targets(&args, &targets, &mut out1, &mut RootMemo::default());
        assert_eq!(code1, exit::ERRORS, "the fixture's error tier");
        let after =
            fs::read_dir(&answers_dir).map(|entries| entries.count()).unwrap_or(0);
        assert!(
            after > before,
            "the cold run persisted an answer record under {answers_dir:?}"
        );

        // The hit proof: a permission flip moves no stamp field (dev, ino,
        // mtime, size all stand), so the warm re-ask must HIT — but a
        // re-render could never reproduce run 1's bytes, whose rows quote
        // the now-unreadable file. Byte equality here proves the serve
        // bypassed the analysis phases, not just that renders are
        // deterministic.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let unreadable = root.join("lib/clean.dart");
            fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o000))
                .expect("chmod");
        }
        let mut out2: Vec<u8> = Vec::new();
        let code2 = analyze_targets(&args, &targets, &mut out2, &mut RootMemo::default());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(root.join("lib/clean.dart"), fs::Permissions::from_mode(0o644))
                .expect("restore chmod");
        }
        assert_eq!(out2, out1, "the warm re-ask serves byte-identical stdout");
        assert_eq!(code2, code1, "the same exit tier");

        // A source edit moves a stamp: the next ask re-renders. Fixing
        // both files clears the error tier.
        fs::write(root.join("lib/broken.dart"), "void f() { }\n").expect("fix");
        fs::write(root.join("lib/clean.dart"), "var ok = 1;\n").expect("fix");
        let mut out3: Vec<u8> = Vec::new();
        let code3 = analyze_targets(&args, &targets, &mut out3, &mut RootMemo::default());
        assert_eq!(code3, exit::CLEAN, "the fixed tree's tier");
        assert_ne!(out3, out1, "the re-render reflects the edit");

        fs::remove_dir_all(&root).ok();
    }

    /// The `--metrics-json` phase split (#2628): every phase key is always
    /// present, the phases are non-overlapping sub-intervals of the total
    /// window (`sum <= total_ms`), and the analyze face's real phases
    /// (sdk + assembly + engine pass) are non-zero over a temp workspace.
    #[test]
    fn analyze_metrics_json_attributes_every_phase() {
        let root = analyze_fixture(
            "phases",
            &[
                ("lib/broken.dart", "void f() { x }\n"),
                ("lib/clean.dart", "var ok = 1;\n"),
            ],
        );
        let metrics_path = root.join("metrics.json");
        let mut out: Vec<u8> = Vec::new();
        let code = analyze_run(
            &analyze_args(&root, AnalyzeFormat::Lon, Some(metrics_path.clone())),
            &mut out,
        );
        assert_eq!(code, exit::ERRORS);

        let metrics: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&metrics_path).expect("metrics at exit"))
                .expect("one JSON object");
        for key in [
            "discovery_ms",
            "sdk_ms",
            "assembly_ms",
            "analyze_ms",
            "lints_ms",
            "render_ms",
            "total_ms",
        ] {
            assert!(
                metrics[key].is_u64(),
                "phase `{key}` always present: {metrics}"
            );
        }
        let phase = |key: &str| metrics[key].as_u64().expect("u64 phase");
        let attributed: u64 = [
            "discovery_ms",
            "sdk_ms",
            "assembly_ms",
            "analyze_ms",
            "lints_ms",
            "render_ms",
        ]
        .into_iter()
        .map(phase)
        .sum();
        assert!(
            attributed <= phase("total_ms"),
            "non-overlapping sub-intervals of the total window: {metrics}"
        );
        assert!(
            phase("sdk_ms") + phase("assembly_ms") + phase("analyze_ms") > 0,
            "the analyze face measures its real phases: {metrics}"
        );

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

    /// The degrade notice's stdout invariant (#1311): a skewed SDK
    /// whitelist surfaces the notice on stderr only — the lon document and
    /// the JSON envelope (the data faces on stdout) carry no trace of it,
    /// and the exit tier ignores the telemetry. The once-per-skew /
    /// zero-when-clean notice contract itself is pinned at the shared core
    /// (`AnalyzeRun.degrade_notices`, dartq-commands' analyze tests).
    #[test]
    fn analyze_degrade_notice_stays_off_the_data_faces() {
        let root = analyze_fixture("degrade", &[("lib/ok.dart", "var ok = 1;\n")]);
        fs::write(
            root.join("sdk/lib/_internal/allowed_experiments.json"),
            r#"{"version": 1, "experimentSets": {"sdkExperiments": ["future-flag"]}, "sdk": {"default": {"experimentSet": "sdkExperiments"}}, "packages": {"fixture": {"experimentSet": "sdkExperiments"}}}"#,
        )
        .expect("skewed whitelist");

        let mut out: Vec<u8> = Vec::new();
        assert_eq!(
            analyze_run(&analyze_args(&root, AnalyzeFormat::Lon, None), &mut out),
            exit::CLEAN,
            "the telemetry never joins the exit tier"
        );
        let lon = String::from_utf8(out).expect("UTF-8");
        assert!(
            !lon.contains("note: SDK") && !lon.contains("ExperimentDegrade"),
            "the lon face stays notice-free: {lon}"
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

    /// Clean set → exit 0 with the empty lon document; the text clean edge
    /// carries no per-file heading (nothing to group) and ends in the oracle
    /// tail — over the broken fixture both oracle rows appear under the one
    /// file heading (oracle display order, message tie-break within the
    /// shared token span) and the tail counts both.
    #[test]
    fn analyze_clean_and_text_faces() {
        let root = analyze_fixture("faces", &[("lib/clean.dart", "var ok = 1;\n")]);

        let mut out: Vec<u8> = Vec::new();
        let code = analyze_run(&analyze_args(&root, AnalyzeFormat::Lon, None), &mut out);
        assert_eq!(code, exit::CLEAN);
        assert_eq!(
            String::from_utf8(out).expect("UTF-8"),
            "{v:1 diags:[]}\n",
            "the empty set is the empty document"
        );

        let mut out: Vec<u8> = Vec::new();
        let code = analyze_run(&analyze_args(&root, AnalyzeFormat::Text, None), &mut out);
        assert_eq!(code, exit::CLEAN);
        // The whole oracle clean shape in one pin: no `lib/clean.dart`
        // heading (the oracle prints none on a clean set), no
        // `Analyzing ...` preamble from this face, and the exact tail
        // spelling (`dart analyze`, 3.13.1).
        let text = String::from_utf8(out).expect("UTF-8");
        assert_eq!(text, "No issues found!\n", "oracle clean human output");

        fs::remove_dir_all(&root).ok();

        // The broken twin: the root line leads once, both oracle rows
        // under the located file heading.
        let broken_root =
            analyze_fixture("faces-broken", &[("lib/broken.dart", "void f() { x }\n")]);
        let mut out: Vec<u8> = Vec::new();
        let code = analyze_run(
            &analyze_args(&broken_root, AnalyzeFormat::Text, None),
            &mut out,
        );
        assert_eq!(code, exit::ERRORS);
        let text = String::from_utf8(out).expect("UTF-8");
        let root_line = format!(
            "root: {}",
            broken_root.canonicalize().expect("root").display()
        );
        assert_eq!(
            text.lines().next(),
            Some(root_line.as_str()),
            "the located headings' root leads once: {text}"
        );
        assert!(
            text.contains("\nlib/broken.dart\n"),
            "the located file heading: {text}"
        );
        assert!(
            text.contains("error expected_token 1:12 Expected to find ';'.")
                && text.contains("error undefined_identifier 1:12 Undefined name 'x'."),
            "both rows under the heading in `sev code line:col msg` grammar: {text}"
        );
        assert!(
            text.contains("2 issues found."),
            "the tail counts both rows: {text}"
        );

        fs::remove_dir_all(&broken_root).ok();
    }

    /// The stale-config advisory face (#732): a stale fixture surfaces
    /// exactly ONE advisory — the data faces keep it off stdout (the lon
    /// document is diagnostics only, the `Info • code • message` line rides
    /// stderr), the text face leads stdout with the same human line — and
    /// the advisory never joins the per-file set, the metrics count, or the
    /// exit tier. The fresh side of the contract is pinned by the byte-exact
    /// faces above (`analyze_clean_and_text_faces` — no advisory row, no
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

        // The assembly surfaces the advisory at all (the fixture's own
        // pin — the run below only proves the faces' purity).
        let root = analyze_fixture("stale-machine", &[("lib/broken.dart", "void f() { x }\n")]);
        stale_config(&root);
        let (_engine, _dir, advisory) = open_engine_with_overrides(
            &root,
            Some(root.join("sdk")).as_deref(),
            &AnalyzeOverrides::default(),
        )
        .expect("assembly surfaces the advisory");
        assert!(
            advisory.is_some(),
            "the stale fixture assembles an advisory"
        );

        // The lon face: stdout is the diags document alone — the advisory
        // has no row in it (it rides stderr as the Info line).
        let mut out: Vec<u8> = Vec::new();
        let code = analyze_run(&analyze_args(&root, AnalyzeFormat::Lon, None), &mut out);
        let text = String::from_utf8(out).expect("stdout is UTF-8");
        let document: serde_json::Value = lon::from_str(text.trim_end()).expect("one lon document");
        assert_eq!(
            document["diags"].as_array().expect("diags").len(),
            2,
            "the oracle rows only: {text}"
        );
        assert!(
            !text.contains("package_config_out_of_date"),
            "the advisory never joins the lon document: {text}"
        );
        assert_eq!(code, exit::ERRORS, "the advisory never moves the exit tier");

        // Text format: the advisory line leads, the issues tail counts
        // rows only.
        let root = analyze_fixture("stale-human", &[("lib/broken.dart", "void f() { x }\n")]);
        stale_config(&root);
        let mut out: Vec<u8> = Vec::new();
        let code = analyze_run(&analyze_args(&root, AnalyzeFormat::Text, None), &mut out);
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

        let missing_dir = analyze_args(&root.join("nope"), AnalyzeFormat::Lon, None);
        assert_eq!(analyze_run(&missing_dir, &mut Vec::new()), exit::USAGE);

        let no_sdk = AnalyzeArgs {
            sdk_path: Some(root.join("no-such-sdk")),
            ..analyze_args(&root, AnalyzeFormat::Lon, None)
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
            ..analyze_args(&root, AnalyzeFormat::Lon, None)
        };
        let mut out: Vec<u8> = Vec::new();
        let code = analyze_run(&args, &mut out);
        assert_eq!(code, exit::ERRORS, "any ERROR → 3");
        let files = diag_files(&out);
        for name in ["broken_a.dart", "broken_b.dart", "broken_t.dart"] {
            assert!(
                files.iter().any(|file| file.contains(name)),
                "the union covers {name}: {files:?}"
            );
        }

        // A lone file target emits that file's rows only — the siblings
        // stay analyzed (resolution context) but never emit (the oracle's
        // file context root).
        let args = AnalyzeArgs {
            targets: vec![test_file.clone()],
            ..analyze_args(&root, AnalyzeFormat::Lon, None)
        };
        let mut out: Vec<u8> = Vec::new();
        let code = analyze_run(&args, &mut out);
        assert_eq!(code, exit::ERRORS);
        let files = diag_files(&out);
        assert!(
            !files.is_empty() && files.iter().all(|file| file.contains("broken_t.dart")),
            "the file target emits its rows alone: {files:?}"
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
        // gate): the lon document never carries the memory key.
        let args = AnalyzeArgs {
            memory: true,
            ..analyze_args(&root, AnalyzeFormat::Lon, None)
        };
        let mut out: Vec<u8> = Vec::new();
        assert_eq!(analyze_run(&args, &mut out), exit::ERRORS);
        let lon = String::from_utf8(out).expect("UTF-8");
        assert!(
            !lon.contains("memory:"),
            "the lon face never carries the memory key: {lon}"
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
            ..analyze_args(&root_a, AnalyzeFormat::Lon, None)
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
    /// rides stderr (the lon document keeps the oracle rows alone), and
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

        let mut out: Vec<u8> = Vec::new();
        let code = analyze_run(&analyze_args(&root, AnalyzeFormat::Lon, None), &mut out);
        let text = std::str::from_utf8(&out)
            .expect("stdout is UTF-8")
            .to_owned();
        assert!(
            !text.contains("DARTQ/"),
            "the advisory never joins the lon stdout: {text}"
        );
        let files = diag_files(&out);
        assert!(
            files.iter().any(|file| file == "lib/main.dart"),
            "the package: import is unresolved, the oracle's degraded face, located under the no-config floor's root: {files:?}"
        );
        assert_eq!(code, exit::ERRORS);

        // The assembly surfaces the advisory (the fixture's own pin — the
        // run above only proves the document's purity).
        let (_engine, _dir, advisory) = open_engine_with_overrides(
            &root,
            Some(root.join("sdk")).as_deref(),
            &AnalyzeOverrides::default(),
        )
        .expect("assembly surfaces the advisory");
        assert!(
            advisory.is_some(),
            "the missing-config fixture assembles an advisory"
        );

        fs::remove_dir_all(&root).ok();
    }

    /// A bare `.dart` file target — no `pubspec.yaml`, no
    /// `package_config.json`, the workspace walk's `NoRoot` floor at the
    /// file's own directory — still analyzes in the no-package context: the
    /// oracle's `dart analyze <file>` face emits the file's own semantic
    /// rows (SDK types resolve, `package:` URIs would degrade to
    /// `URI_DOES_NOT_EXIST`) rather than an empty stream. Pin the lon
    /// document's non-empty row set against regression to a zero-row run.
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
            ..analyze_args(&root, AnalyzeFormat::Lon, None)
        };
        let mut out: Vec<u8> = Vec::new();
        let code = analyze_run(&args, &mut out);
        assert_eq!(code, exit::ERRORS, "the file's own ERROR rows → 3");
        let text = String::from_utf8(out).expect("UTF-8");
        let document: serde_json::Value = lon::from_str(text.trim_end()).expect("one lon document");
        let rows = document["diags"].as_array().expect("diags");
        assert!(
            !rows.is_empty()
                && rows
                    .iter()
                    .any(|row| row["code"] == "invalid_assignment" && row["file"] == "bad.dart"),
            "the bare file's own diagnostic emits, located under its no-pubspec floor root, never a zero-row run: {text}"
        );
        assert!(
            rows.iter()
                .all(|row| row["file"].as_str().is_some_and(|f| f.contains("bad.dart"))),
            "no sibling or workspace noise joins the file target's set: {text}"
        );

        fs::remove_dir_all(&root).ok();
    }

    /// The analyze face's crash tier: a panic anywhere in the engine pass
    /// exits at the internal-error tier — the armed hook's deterministic
    /// death, pinned end-to-end by
    /// [`crash_tier_exits_at_internal_error_on_worker_panic`] (in-process
    /// here it would kill the harness). The tier's remaining arms: a run
    /// `Err` stays the internal tier, and a clean pass passes through with
    /// no exit override.
    #[test]
    fn analyze_crash_tier_maps_error_to_internal_exit() {
        use dartq_commands::analyze::AnalyzeError;

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
    /// broken fixture (exit 0, the empty lon document), an overlay carrying
    /// an error reports it against the analyzed path in the lon row shape
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
            ..analyze_args(&root, AnalyzeFormat::Lon, None)
        };
        assert_eq!(analyze_run(&fixed, &mut Vec::new()), exit::CLEAN);

        // The overlay never touches disk: the on-disk file stays broken.
        assert_eq!(
            fs::read_to_string(&target).expect("target intact"),
            "void f() { x }\n"
        );

        // Broken overlay: the errors surface against the analyzed path in
        // the lon row shape (engine-canonical → oracle display order;
        // serial oracle probe on `void g() { y }`: `y` at UTF-16 offset 11
        // → line 1, column 12, length 1 on BOTH rows — the syntax row
        // ("Expected to find ';'.") and the semantic row ("Undefined
        // name 'y'.") share the token's span and order by the message
        // tie-break).
        let mut out: Vec<u8> = Vec::new();
        let broken_overlay = AnalyzeArgs {
            overlays: vec![format!("{}={}", target.display(), broken.display())],
            ..analyze_args(&root, AnalyzeFormat::Lon, None)
        };
        assert_eq!(analyze_run(&broken_overlay, &mut out), exit::ERRORS);
        assert_eq!(
            diag_files(&out),
            vec!["lib/broken.dart".to_owned(), "lib/broken.dart".to_owned()],
            "the overlay path pins the mirrored oracle sort (both rows on the analyzed path, located under the run root, message tie-break order)"
        );

        // Relative overlay spec resolves against the package dir.
        let relative = AnalyzeArgs {
            overlays: vec![format!("lib/broken.dart={}", clean.display())],
            ..analyze_args(&root, AnalyzeFormat::Lon, None)
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
            ..analyze_args(&root, AnalyzeFormat::Lon, None)
        };
        assert_eq!(analyze_run(&introduced, &mut out), exit::ERRORS);
        assert_eq!(
            diag_files(&out),
            vec![
                "lib/broken.dart".to_owned(),
                "lib/broken.dart".to_owned(),
                "lib/new.dart".to_owned(),
                "lib/new.dart".to_owned(),
            ],
            "the introduced file's rows anchor at the canonical dir-joined path, located under the run root"
        );

        // Missing local file → usage tier.
        let missing = AnalyzeArgs {
            overlays: vec![format!(
                "{}={}",
                target.display(),
                root.join("nope").display()
            )],
            ..analyze_args(&root, AnalyzeFormat::Lon, None)
        };
        assert_eq!(analyze_run(&missing, &mut Vec::new()), exit::USAGE);

        // Malformed spec → usage tier.
        let malformed = AnalyzeArgs {
            overlays: vec!["no-equals-sign".to_owned()],
            ..analyze_args(&root, AnalyzeFormat::Lon, None)
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
        assert!(
            matches!(symbol.format, OutputFormat::Lon),
            "lon is the constant default"
        );
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
        assert!(matches!(symbol.format, OutputFormat::Json));
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

    fn symbol_args(root: &Path, format: OutputFormat, name: &str) -> SymbolArgs {
        SymbolArgs {
            name: name.to_string(),
            dir: root.to_path_buf(),
            format,
            verbosity: Verbosity::Compact,
            sdk: Some(root.join("sdk")),
            fuzzy: false,
        }
    }

    /// Decodes one lon line (the tests' reader).
    fn read_lon(out: &[u8]) -> serde_json::Value {
        lon::from_str(std::str::from_utf8(out).expect("UTF-8").trim_end())
            .expect("the lon line decodes")
    }

    /// End to end over a real temp workspace: the class plus its cross-file
    /// uses (two bare, one import-prefixed) in the lon vocabulary answer
    /// (`sym`/`decls`/`refs`, the rows' `col`/`len`/`off`/`target`), a
    /// member hit with its container name, the json face (the `qualified`
    /// column: omitted on bare rows, `p.Foo` on the prefixed row), the
    /// empty-result edge (exit 0, the empty arrays), and the usage tier.
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
        // `Foo? f` types a reference; `Foo()` is an implicit-creation
        // invocation; both target a.dart unqualified, use sites in b.dart;
        // c.dart's `p.Foo()` arrives through the armed `p` prefix.
        let mut out: Vec<u8> = Vec::new();
        let code = query_symbol_run(&symbol_args(&root, OutputFormat::Lon, "Foo"), &mut out);
        assert_eq!(code, exit::CLEAN);
        let answer = read_lon(&out);
        assert_eq!(answer["v"], 1);
        assert_eq!(answer["sym"], "Foo", "the queried name rides sym");
        assert_eq!(
            answer["root"],
            root.canonicalize().expect("root").display().to_string(),
            "the envelope carries the analysis root once"
        );
        assert!(
            answer.get("name").is_none(),
            "the long spelling drops at the envelope"
        );
        let decls = answer["decls"].as_array().expect("decls");
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0]["kind"], "class");
        assert_eq!(decls[0]["name"], "Foo");
        assert_eq!(
            decls[0]["file"], "lib/a.dart",
            "the row labels are the located spellings (root-relative)"
        );
        assert_eq!(decls[0]["line"], 1);
        assert_eq!(decls[0]["col"], 7, "column → col");
        assert_eq!(decls[0]["len"], 3, "length → len");
        let refs = answer["refs"].as_array().expect("refs");
        assert_eq!(refs.len(), 3);
        assert_eq!(refs[0]["kind"], "reference");
        assert_eq!(refs[0]["target"], "lib/a.dart", "target_file → target");
        assert_eq!(refs[0]["file"], "lib/b.dart");
        assert_eq!(refs[0]["off"], 33, "offset → off");
        assert_eq!(refs[1]["kind"], "invoked");
        assert_eq!(refs[1]["off"], 42);
        assert_eq!(refs[2]["kind"], "invoked");
        assert_eq!(refs[2]["off"], 43);
        assert_eq!(refs[2]["qualified"], "p.Foo");

        // A member hit: the container name surfaces, and the `sig` column
        // spells the element face (the rendered member signature); no uses
        // → no refs rows, still exit 0.
        let mut out: Vec<u8> = Vec::new();
        let code = query_symbol_run(&symbol_args(&root, OutputFormat::Lon, "meth"), &mut out);
        assert_eq!(code, exit::CLEAN);
        let answer = read_lon(&out);
        let decls = answer["decls"].as_array().expect("decls");
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0]["container"], "Foo");
        assert_eq!(decls[0]["sig"], "void meth()", "signature → sig");
        assert_eq!(
            answer["refs"].as_array().expect("refs").len(),
            0,
            "no uses → no rows"
        );

        // JSON face: one object, the same short-vocabulary fields the lon
        // face prints (the file labels are the located spellings, the
        // envelope's `root` riding once).
        let mut out: Vec<u8> = Vec::new();
        let code = query_symbol_run(&symbol_args(&root, OutputFormat::Json, "Foo"), &mut out);
        assert_eq!(code, exit::CLEAN);
        let json: serde_json::Value =
            serde_json::from_str(&String::from_utf8(out).expect("UTF-8")).expect("one object");
        assert_eq!(json["sym"], "Foo");
        assert_eq!(json["v"], 1);
        assert_eq!(
            json["root"],
            root.canonicalize().expect("root").display().to_string()
        );
        assert_eq!(json["decls"][0]["kind"], "class");
        assert_eq!(json["decls"][0]["file"], "lib/a.dart");
        assert_eq!(json["decls"][0]["col"], 7);
        assert_eq!(json["refs"].as_array().expect("array").len(), 3);
        assert_eq!(json["refs"][0]["kind"], "reference");
        assert_eq!(json["refs"][0]["target"], "lib/a.dart");
        assert_eq!(json["refs"][0]["file"], "lib/b.dart");
        assert!(json["refs"][0].get("qualified").is_none());
        assert_eq!(json["refs"][0]["off"], 33);
        assert_eq!(json["refs"][2]["qualified"], "p.Foo");

        // The minimal tier on the lon face: the locators stay (the `root`
        // envelope among them), the convenience fields drop (container/sig
        // on decls; the json face trims its own way, pinned by the frozen
        // contract).
        let mut out: Vec<u8> = Vec::new();
        let args = SymbolArgs {
            verbosity: Verbosity::Minimal,
            ..symbol_args(&root, OutputFormat::Lon, "Foo")
        };
        assert_eq!(query_symbol_run(&args, &mut out), exit::CLEAN);
        let answer = read_lon(&out);
        let decl = &answer["decls"][0];
        assert_eq!(decl["file"], "lib/a.dart", "the locator stays");
        assert_eq!(
            answer["root"],
            root.canonicalize().expect("root").display().to_string(),
            "the root rides at every tier"
        );
        assert!(decl.get("container").is_none(), "the convenience drops");
        assert!(decl.get("sig").is_none());
        assert!(
            answer["refs"]
                .as_array()
                .expect("array")
                .iter()
                .all(|row| row.get("qualified").is_none() && row.get("file").is_none()),
            "minimal reference rows keep target/kind/off/len only"
        );

        // Zero results: exit 0, the empty arrays print.
        let mut out: Vec<u8> = Vec::new();
        let code = query_symbol_run(&symbol_args(&root, OutputFormat::Lon, "Nope"), &mut out);
        assert_eq!(code, exit::CLEAN);
        let answer = read_lon(&out);
        assert_eq!(answer["sym"], "Nope");
        assert_eq!(answer["decls"], serde_json::json!([]));

        // Usage tier: missing directory.
        let missing = symbol_args(&root.join("nope"), OutputFormat::Lon, "Foo");
        assert_eq!(query_symbol_run(&missing, &mut Vec::new()), exit::USAGE);

        fs::remove_dir_all(&root).ok();
    }

    /// The references/callers faces end to end over the symbol test's
    /// fixture: `references` serves the symbol answer's rows
    /// value-for-value; `callers` keeps only the `invoked` rows (the
    /// `Foo()` constructor site at UTF-16 offset 41, the `foo()` call at
    /// 50) while the declaration rows stay for target identity; a
    /// declaration with no invocations prints its decls row and no refs
    /// rows; the json envelope is the frozen v1 shape.
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

        // references == symbol rows (the index's reference surface is
        // name-keyed and already kinded).
        let mut symbol_out: Vec<u8> = Vec::new();
        assert_eq!(
            query_symbol_run(
                &symbol_args(&root, OutputFormat::Lon, "Foo"),
                &mut symbol_out
            ),
            exit::CLEAN
        );
        let mut references_out: Vec<u8> = Vec::new();
        assert_eq!(
            query_references_run(
                &symbol_args(&root, OutputFormat::Lon, "Foo"),
                &mut references_out
            ),
            exit::CLEAN
        );
        assert_eq!(
            read_lon(&symbol_out),
            read_lon(&references_out),
            "the references spelling serves the symbol rows"
        );

        // callers Foo: the constructor site (the `var f` local carries no
        // reference site).
        let mut out: Vec<u8> = Vec::new();
        assert_eq!(
            query_callers_run(&symbol_args(&root, OutputFormat::Lon, "Foo"), &mut out),
            exit::CLEAN
        );
        let answer = read_lon(&out);
        assert_eq!(
            answer["decls"].as_array().expect("decls").len(),
            1,
            "the declaration rows stay for target identity"
        );
        let refs = answer["refs"].as_array().expect("refs");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0]["kind"], "invoked");
        assert_eq!(refs[0]["off"], 41);
        assert_eq!(refs[0]["target"], "lib/a.dart");

        // callers foo: the one call site. The top-level function carries
        // its own signature face.
        let mut out: Vec<u8> = Vec::new();
        assert_eq!(
            query_callers_run(&symbol_args(&root, OutputFormat::Lon, "foo"), &mut out),
            exit::CLEAN
        );
        let answer = read_lon(&out);
        let decls = answer["decls"].as_array().expect("decls");
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0]["kind"], "function");
        assert_eq!(decls[0]["sig"], "void foo()", "the element-face signature");
        let refs = answer["refs"].as_array().expect("refs");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0]["off"], 50);

        // callers meth: declared, never invoked — decls row, no refs rows.
        let mut out: Vec<u8> = Vec::new();
        assert_eq!(
            query_callers_run(&symbol_args(&root, OutputFormat::Lon, "meth"), &mut out),
            exit::CLEAN
        );
        let answer = read_lon(&out);
        assert_eq!(answer["decls"].as_array().expect("decls").len(), 1);
        assert_eq!(answer["refs"].as_array().expect("refs").len(), 0);

        // Callers json: the same envelope with the filtered references.
        let mut out: Vec<u8> = Vec::new();
        assert_eq!(
            query_callers_run(&symbol_args(&root, OutputFormat::Json, "Foo"), &mut out),
            exit::CLEAN
        );
        let json: serde_json::Value =
            serde_json::from_str(&String::from_utf8(out).expect("UTF-8")).expect("one object");
        assert_eq!(json["v"], 1);
        assert_eq!(json["sym"], "Foo");
        assert_eq!(json["decls"].as_array().expect("array").len(), 1);
        let refs = json["refs"].as_array().expect("array");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0]["kind"], "invoked");
        assert_eq!(refs[0]["off"], 41);
        assert_eq!(refs[0]["target"], "lib/a.dart");

        // Usage tier: missing directory.
        let missing = symbol_args(&root.join("nope"), OutputFormat::Lon, "Foo");
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
        assert!(matches!(args.format, OutputFormat::Lon), "lon default");
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
        assert!(matches!(args.format, OutputFormat::Json));

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
            format: OutputFormat::Json,
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
        // The answer's file columns spell through the one file locator:
        // every file inside the analysis root (the lib and test trees
        // alike) is root-relative, and the envelope carries the root once.
        let canonical = root.canonicalize().expect("root");
        let animal = "lib/animal.dart";
        let dog = "lib/dog.dart";
        let main = "lib/main.dart";
        let dog_test = "test/dog_test.dart";

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
        assert_eq!(json["v"], 1);
        assert_eq!(json["root"], canonical.display().to_string());
        assert_eq!(json["sym"], "Dog");
        assert_eq!(
            json["declaration"],
            serde_json::json!({
                "file": dog,
                "name": "Dog",
                "kind": "class",
                "off": explain_offset(EXPLAIN_DOG, "Dog"),
                "len": 3,
            })
        );
        assert_eq!(
            json["bases"],
            serde_json::json!([{ "name": "Animal", "file": animal, "clause": "extends" }])
        );
        assert_eq!(
            json["methods"],
            serde_json::json!([{
                "name": "bark",
                "kind": "method",
                "off": explain_offset(EXPLAIN_DOG, "bark"),
                "len": 4,
            }])
        );
        let refs = json["refs"].as_array().expect("refs");
        assert_eq!(refs.len(), 2, "the main use plus the test use");
        // Row order keys on the located label: `lib/` sorts before
        // `test/`, the same posture the impact face pins over this fixture
        // shape.
        assert_eq!(refs[0]["file"], main);
        assert_eq!(refs[0]["kind"], "invoked");
        assert_eq!(refs[1]["file"], dog_test);
        assert_eq!(
            json["deps"],
            serde_json::json!([{ "path": main }, { "path": dog_test }])
        );
        assert_eq!(json["tests"], serde_json::json!([{ "path": dog_test }]));

        // Minimal: the declaration locators stay, the arrays swap for counts.
        let mut out: Vec<u8> = Vec::new();
        assert_eq!(
            explain_run(&explain_args(&root, Verbosity::Minimal, "Dog"), &mut out),
            exit::CLEAN
        );
        let json: serde_json::Value =
            serde_json::from_str(&String::from_utf8(out).expect("UTF-8")).expect("one object");
        assert_eq!(json["declaration"]["name"], "Dog", "the red line stays");
        assert!(json.get("refs").is_none(), "arrays drop");
        assert_eq!(json["counts"]["refs"], 2);
        assert_eq!(json["counts"]["tests"], 1);

        // Unknown symbol: the empty seam report, printed as-is at exit 0.
        let mut out: Vec<u8> = Vec::new();
        assert_eq!(
            explain_run(&explain_args(&root, Verbosity::Compact, "Nope"), &mut out),
            exit::CLEAN
        );
        let json: serde_json::Value =
            serde_json::from_str(&String::from_utf8(out).expect("UTF-8")).expect("one object");
        assert_eq!(json["sym"], "Nope");
        assert!(json.get("declaration").is_none());
        assert_eq!(json["refs"], serde_json::json!([]));

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
            format: OutputFormat::Json,
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
        assert_eq!(json["v"], 1);
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
    /// defaults to the daemon's standing posture (the 2048 MiB budget, the
    /// L3 store at the analyze face's default cap, no sink) that
    /// `SessionRuntime::default` plus the shell's arming carries.
    #[test]
    fn daemon_runtime_flags_parse() {
        let parsed = parse_args_of(&["daemon", "/pkg"])
            .expect("parse")
            .expect("subcommand");
        let Command::Daemon(args) = parsed else {
            panic!("expected daemon command")
        };
        assert_eq!(args.memory_limit, None, "the standing budget by default");
        assert_eq!(
            args.cache_size_limit, None,
            "the store keeps its default cap"
        );
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

    /// Writes the `sky_engine`-shaped package: `lib/_embedder.yaml` mapping
    /// `dart:core` and `dart:ui` inside the package, plus the mapped files.
    fn write_sky_engine(root: &Path) {
        fs::create_dir_all(root.join("sky_engine/lib/ui")).expect("package dirs");
        fs::write(
            root.join("sky_engine/lib/_embedder.yaml"),
            "embedded_libs:\n  \"dart:core\": \"embedder_core.dart\"\n  \"dart:ui\": \"ui/ui.dart\"\n",
        )
        .expect("embedder yaml");
        fs::write(
            root.join("sky_engine/lib/embedder_core.dart"),
            "library dart.core;",
        )
        .expect("embedder core");
        fs::write(root.join("sky_engine/lib/ui/ui.dart"), "library dart.ui;").expect("embedder ui");
    }

    /// The fixture's package config plus one `sky_engine` entry (an
    /// absolute `file://` rootUri — a real Flutter config's sky_engine
    /// spelling; `root` is the canonical root so the resolved paths spell
    /// exactly).
    fn rewrite_config_with_sky_engine(root: &Path) {
        fs::write(
            root.join(".dart_tool/package_config.json"),
            format!(
                r#"{{"configVersion": 2, "packages": [{{"name": "fixture", "rootUri": "../", "packageUri": "lib/", "languageVersion": "3.5"}}, {{"name": "sky_engine", "rootUri": "file://{}", "packageUri": "lib/", "languageVersion": "3.5"}}], "generator": "pub", "generatorVersion": "3.13.1"}}"#,
                root.join("sky_engine").display(),
            ),
        )
        .expect("config");
    }

    /// The context-axis snapshot rides the base resolution's embedder
    /// table (#2633): a `sky_engine` entry makes every declared context's
    /// SDK reload resolve `dart:ui` through the embedder and `dart:core`
    /// onto the embedder's file (full replacement); a plain resolution
    /// keeps the dart-sdk table for the same contexts.
    #[test]
    fn daemon_context_snapshot_rides_the_embedder_table() {
        let root = analyze_fixture("daemon_embedder", &[])
            .canonicalize()
            .expect("root");
        write_sky_engine(&root);
        rewrite_config_with_sky_engine(&root);
        write_daemon_context_config(&root);
        let (base, _, _) =
            engine_config_with_advisory(&root, Some(&root.join("sdk"))).expect("assembly");
        let extras = assemble_contexts(&base, &[]).expect("the declared contexts load");
        assert_eq!(extras.len(), 2, "plain and web");
        for (_, engine) in &extras {
            let ui = engine
                .sdk
                .flat_library_to_path("ui", "")
                .expect("dart:ui resolves through the embedder");
            assert_eq!(
                ui,
                root.join("sky_engine")
                    .join("lib")
                    .join("ui")
                    .join("ui.dart")
            );
            assert_eq!(
                engine.sdk.flat_library_to_path("core", ""),
                Some(
                    root.join("sky_engine")
                        .join("lib")
                        .join("embedder_core.dart")
                ),
                "full replacement: core comes from the embedder file"
            );
        }
        fs::remove_dir_all(&root).ok();

        // The plain resolution: the same contexts keep the dart-sdk table.
        let plain = analyze_fixture("daemon_embedder_plain", &[]);
        write_daemon_context_config(&plain);
        let plain = plain.canonicalize().expect("root");
        let (base, _, _) =
            engine_config_with_advisory(&plain, Some(&plain.join("sdk"))).expect("assembly");
        let extras = assemble_contexts(&base, &[]).expect("the declared contexts load");
        assert_eq!(extras.len(), 2);
        for (_, engine) in &extras {
            assert_eq!(engine.sdk.flat_library_to_path("ui", ""), None);
            assert_eq!(
                engine.sdk.flat_library_to_path("core", ""),
                Some(plain.join("sdk").join("lib").join("core").join("core.dart")),
                "the plain dart-sdk core file backs dart:core"
            );
        }
        fs::remove_dir_all(&plain).ok();
    }

    /// The crash tier's deterministic panic death: a panic inside the
    /// armed pass exits the process at the internal-error tier even when it
    /// fires on a thread whose unwind could not reach `catch_unwind` before
    /// a sibling job's own (possibly unbounded) work finished — the hang
    /// class the hook exists to kill. The armed hook also restores the
    /// previous hook after a clean pass, and a nested tier inside an armed
    /// pass neither deadlocks nor re-arms.
    ///
    /// The panic itself runs in a self-spawned child: the armed hook's
    /// `process::exit` would kill the test harness if it fired in-process.
    /// `DARTQ_CRASH_TIER_SELFTEST` selects the child path, which panics on
    /// a spawned thread — the path `catch_unwind` alone could never save.
    #[test]
    fn crash_tier_exits_at_internal_error_on_worker_panic() {
        if std::env::var_os("DARTQ_CRASH_TIER_SELFTEST").is_some() {
            crash_tier(|| {
                std::thread::spawn(|| panic!("crash-tier selftest"));
                // A sibling job that never completes: without the armed
                // hook the join would hold this pass open forever — the
                // post-panic spin the tier exists to kill.
                loop {
                    std::thread::park();
                }
            })
            .ok();
            return;
        }
        let current = std::env::current_exe().expect("test binary");
        let output = std::process::Command::new(current)
            .args([
                "--exact",
                "tests::crash_tier_exits_at_internal_error_on_worker_panic",
                "--nocapture",
            ])
            .env("DARTQ_CRASH_TIER_SELFTEST", "1")
            .output()
            .expect("selftest child runs");
        assert_eq!(
            output.status.code(),
            Some(i32::from(exit::INTERNAL_ERROR)),
            "the armed hook exits the process at the internal tier; stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("crash-tier selftest"),
            "the default hook's panic message still prints"
        );
    }

    /// A nested `crash_tier` inside an armed ancestor neither deadlocks
    /// nor re-arms: the outer arming covers the subtree, the inner tier
    /// degrades to a plain `catch_unwind` (the `ARMED` flag's whole job).
    /// A panic-free nested run returns `Ok`, and the previous hook is
    /// restored when the outer tier disarms.
    #[test]
    fn nested_crash_tier_runs_under_the_ancestor_arming() {
        let got = crash_tier(|| crash_tier(|| 7));
        assert!(matches!(got, Ok(Ok(7))));
    }
}

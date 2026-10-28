//! The `dartq mcp` shell: the MCP stdio server — one line of setup for
//! AI hosts, every tool mirroring a CLI verb.
//!
//! # Convergence
//!
//! The server binds to the worktree's resident session (the same
//! identity triple the CLI's `session` face manages): engine-backed
//! tools proxy the daemon protocol over the session socket — one engine
//! per worktree, shared by CLI one-shots, the events stream, and every
//! MCP client. When no session can live here (a read-only tree), the
//! server still runs: engine tools answer `isError` with the
//! degradation reason, and the local faces (`dartq_events`, the
//! workspace-status resource) keep working.
//!
//! # Wire
//!
//! Newline-delimited JSON-RPC 2.0 on stdin/stdout (the MCP stdio
//! transport): `initialize`/`initialized`, `ping`, `tools/list`,
//! `tools/call`, `resources/list`, `resources/read`. Tool arguments
//! pass through to the daemon request's own parameters — the daemon
//! owns the schemas; each tool's description states when to use it and
//! when not. Long operations bracket the call with MCP progress
//! notifications when the request carries a progress token.
//!
//! Tool calls open one socket connection per call (the resident server
//! serves one client at a time; short-lived connections keep CLI
//! one-shots from queueing behind a resident MCP host).
//!
//! The daemon-call proxy ([`daemon_call`]) is the binary's shared
//! session transport: the `debug` verb group proxies its
//! `dartq/debug/*` requests through the same machinery — one engine,
//! one protocol, two faces.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::Args;
use serde_json::{Value, json};

use dartq_session::{EventLog, Layout, Progress};

use crate::exit;

/// `dartq mcp` — serve the MCP stdio face.
#[derive(Args)]
pub struct McpArgs {
    /// The worktree whose session the tools converge on; defaults to the
    /// current directory.
    #[arg(default_value = ".")]
    pub dir: PathBuf,
    /// Dart SDK root; defaults to `$DART_SDK`.
    #[arg(long)]
    pub sdk: Option<PathBuf>,
}

/// One tool parameter: its type face, whether it is required, and its
/// description. The tool table below is the single source of every
/// tool's parameters — `tools/list` renders it as the inputSchema and
/// `tools/call` validates against it (#2657: the schema that advertises
/// the contract is the same table that guards the dispatch).
struct Param {
    name: &'static str,
    kind: ParamKind,
    required: bool,
    description: &'static str,
}

/// A parameter's JSON-Schema type face. `Path` renders as `string` (the
/// description carries the resolution rule — relative paths resolve
/// against the server's working directory, the workspace root as the
/// fallback).
#[derive(Clone, Copy)]
enum ParamKind {
    Str,
    Path,
    Num,
    Bool,
    /// A closed string vocabulary.
    Enum(&'static [&'static str]),
    /// An array of strings.
    Strings,
    /// An array of paths.
    Paths,
    /// A nested object with its own field table.
    Object(&'static [Param]),
}

/// An optional parameter.
const fn opt(name: &'static str, kind: ParamKind, description: &'static str) -> Param {
    Param {
        name,
        kind,
        required: false,
        description,
    }
}

/// A required parameter.
const fn req(name: &'static str, kind: ParamKind, description: &'static str) -> Param {
    Param {
        name,
        kind,
        required: true,
        description,
    }
}

/// The shared vocabularies (the daemon faces' own closed sets).
const VERBOSITY: ParamKind = ParamKind::Enum(&["minimal", "compact", "full"]);
const QUERY_KINDS: ParamKind = ParamKind::Enum(&[
    "symbol",
    "references",
    "callers",
    "implementations",
    "dependencies",
]);
const FIX_ROLES: ParamKind =
    ParamKind::Enum(&["primary", "ignore_line", "ignore_file", "suppress"]);

/// The breakpoint-location object (`dartq_debug_breakpoint_add`): a
/// source line (`script` + `line`, optional `column`) or a function entry
/// (`function_id`) — exactly one of the two forms.
const BREAKPOINT_LOCATION: ParamKind = ParamKind::Object(&[
    opt(
        "script",
        ParamKind::Str,
        "The script URI or path of the line breakpoint.",
    ),
    opt(
        "line",
        ParamKind::Num,
        "The 1-based line of the breakpoint.",
    ),
    opt("column", ParamKind::Num, "The optional 1-based column."),
    opt(
        "function_id",
        ParamKind::Str,
        "A function-entry breakpoint instead of a line (mutually exclusive with script/line).",
    ),
]);

/// The one tool family the server serves.
struct Tool {
    name: &'static str,
    description: &'static str,
    kind: ToolKind,
    params: &'static [Param],
}

enum ToolKind {
    /// A daemon request over the session socket; the tool's arguments
    /// are the request's parameters, and the answer's canonical LON line
    /// is the tool text.
    Daemon(&'static str),
    /// The `dartq/fix` daemon face: the answer envelope's `files`
    /// documents, one canonical LON line per file — the CLI face's own
    /// stdout lines, joined by `\n` (#2657).
    Fix,
    /// The rename face over a fresh in-process engine (a rare operation;
    /// a cold pass per call). The tool text is the CLI face's own
    /// document (`rename::to_json`).
    Rename,
    /// The session's event log / progress snapshot, read locally: one
    /// canonical LON line per row — the CLI `events` verb's own stdout
    /// lines, joined by `\n`.
    Events,
}

const TOOLS: &[Tool] = &[
    Tool {
        name: "dartq_analyze",
        description: "Full-engine diagnostics (scan, parse, resolution, semantic/flow/lint) \
for the target, rendered exactly like the CLI's `dartq analyze` lon document \
(root key, root-relative files). Use for: 'what is wrong with this code', \
pre-commit checks, whole-file review context. Not for: symbol lookups \
(dartq_query), why a diagnostic fires (dartq_explain), or blast-radius \
questions (dartq_impact).",
        kind: ToolKind::Daemon("dartq/analyze"),
        params: &[
            opt(
                "target",
                ParamKind::Path,
                "The analysis aim: a directory or file. Relative paths resolve against the \
                 server's working directory (the workspace root as the fallback); the default \
                 is the server's working directory itself — the CLI's positional default.",
            ),
            opt(
                "verbosity",
                VERBOSITY,
                "The answer tier; compact by default.",
            ),
            opt(
                "agent_id",
                ParamKind::Str,
                "Analyze this agent's overlay view instead of the base view.",
            ),
            opt(
                "context",
                ParamKind::Str,
                "A declared dartq.yaml context tag to analyze.",
            ),
        ],
    },
    Tool {
        name: "dartq_query",
        description: "Symbol-index lookups: declarations and resolved references by name. Use \
for: 'where is X declared/used', call sites, implementations, dependencies. Not for: \
diagnostics (dartq_analyze) or one-cursor symbol resolution (no CLI mirror — the daemon \
resolveAt).",
        kind: ToolKind::Daemon("dartq/query"),
        params: &[
            req("name", ParamKind::Str, "The symbol name to look up."),
            opt(
                "kind",
                QUERY_KINDS,
                "The answered row kind; symbol by default.",
            ),
            opt(
                "verbosity",
                VERBOSITY,
                "The answer tier; compact by default.",
            ),
            opt(
                "agent_id",
                ParamKind::Str,
                "Analyze this agent's overlay view instead of the base view.",
            ),
            opt(
                "context",
                ParamKind::Str,
                "A declared dartq.yaml context tag to analyze.",
            ),
            opt(
                "since",
                ParamKind::Num,
                "The session's delta cursor: keep only rows whose file changed after it.",
            ),
        ],
    },
    Tool {
        name: "dartq_explain",
        description: "The one-stop report for one symbol: declaration, shape, references, \
related diagnostics. Use for: 'tell me everything about this API'. Not for: raw reference \
rows (dartq_query is cheaper) or diagnostics alone (dartq_analyze).",
        kind: ToolKind::Daemon("dartq/explain"),
        params: &[
            req(
                "symbol",
                ParamKind::Str,
                "The symbol name the report is for.",
            ),
            opt(
                "verbosity",
                VERBOSITY,
                "The answer tier; compact by default.",
            ),
            opt(
                "since",
                ParamKind::Num,
                "The session's delta cursor: keep only rows whose file changed after it.",
            ),
            opt(
                "sdk",
                ParamKind::Path,
                "The SDK root the report's engine assembles against; defaults to the \
                 session's own SDK root.",
            ),
        ],
    },
    Tool {
        name: "dartq_impact",
        description: "What a change to one file would affect: dependent files, signatures, \
tests, environments. Use before: edits whose blast radius is unclear. Not for: current \
diagnostics (dartq_analyze) — this answers the counterfactual. Answers exactly the CLI's \
`dartq impact` report, environment verdicts included.",
        kind: ToolKind::Daemon("dartq/impact"),
        params: &[
            req(
                "path",
                ParamKind::Path,
                "The changed .dart file to assess. Relative paths resolve against the \
                 server's working directory (the workspace root as the fallback).",
            ),
            opt(
                "symbols",
                ParamKind::Strings,
                "The CLI's repeatable --symbol narrowing, one spelling per item: a bare \
                 `name` (every same-named declaration), `Container.name` (that \
                 container's face), or `path/to/file.dart:line` (the declaration at \
                 that line). Absent is the file-level posture; a spelling that \
                 resolves to nothing is the face's usage error.",
            ),
            opt(
                "verbosity",
                VERBOSITY,
                "Parses for grammar parity; the report is tier-invariant.",
            ),
            opt(
                "agent_id",
                ParamKind::Str,
                "Analyze this agent's overlay view instead of the base view.",
            ),
            opt(
                "context",
                ParamKind::Str,
                "A declared dartq.yaml context tag to analyze.",
            ),
            opt(
                "since",
                ParamKind::Num,
                "The session's delta cursor: keep only rows whose file changed after it.",
            ),
        ],
    },
    Tool {
        name: "dartq_fix",
        description: "Anchored fix proposals/patches. Use for: applying lint/quick fixes. \
Not for: rename edits (dartq_rename) or plugin assists. The tool text is the CLI \
`dartq fix --dry-run` stdout itself: one LON patch document per file, located paths.",
        kind: ToolKind::Fix,
        params: &[
            req(
                "paths",
                ParamKind::Paths,
                "The target directories/files. Relative paths resolve against the server's \
                 working directory (the workspace root as the fallback); the CLI's own \
                 spelling is kept, so the documents match the CLI run's bytes.",
            ),
            opt(
                "role",
                FIX_ROLES,
                "The candidate kind the answer serves: primary (default, one code change \
                 per diagnostic), ignore_line, ignore_file, or suppress.",
            ),
            opt(
                "verbosity",
                VERBOSITY,
                "The answer tier; compact by default.",
            ),
        ],
    },
    Tool {
        name: "dartq_rename",
        description: "Rename one symbol across the workspace; returns the machine edit set \
(dry-run semantics — the caller applies edits) as the CLI's own rename document (v, \
target, edit rows). Use for: identifier renames. Not for: fix-class rewrites \
(dartq_fix). Runs a cold analysis pass — expect it slower than the session-resident \
tools.",
        kind: ToolKind::Rename,
        params: &[
            req(
                "target",
                ParamKind::Str,
                "The rename anchor: 'PATH:LINE:COL' (1-based; the column counts UTF-16 \
                 code units) or 'PATH:OFFSET' (0-based UTF-16 code-unit offset).",
            ),
            req(
                "new_name",
                ParamKind::Str,
                "The new identifier: a legal Dart identifier, not a reserved word.",
            ),
        ],
    },
    Tool {
        name: "dartq_test",
        description: "Run `dart test` through the session (cached, impacted-selectable). Use \
for: verifying a change. Not for: reading results of a run already made \
(dartq_test_events) or diagnostics (dartq_analyze).",
        kind: ToolKind::Daemon("dartq/test/run"),
        params: &[
            opt(
                "args",
                ParamKind::Strings,
                "The dart test arguments (test paths, --name filters, ...).",
            ),
            opt(
                "cwd",
                ParamKind::Path,
                "The run's working directory; defaults to the session's workspace root.",
            ),
        ],
    },
    Tool {
        name: "dartq_test_status",
        description: "The session's test runs: in-flight and finished. Use after dartq_test \
to poll. Not for: starting a run.",
        kind: ToolKind::Daemon("dartq/test/status"),
        params: &[opt(
            "run_id",
            ParamKind::Num,
            "One run's status; absent lists every run.",
        )],
    },
    Tool {
        name: "dartq_test_events",
        description: "Cursor-paged events of one test run. Use for: streaming a run's \
results. Not for: live session/analysis progress (dartq_events).",
        kind: ToolKind::Daemon("dartq/test/events"),
        params: &[
            req("run_id", ParamKind::Num, "The run whose events are paged."),
            opt(
                "cursor",
                ParamKind::Num,
                "The event cursor to continue after; 0 by default.",
            ),
            opt(
                "limit",
                ParamKind::Num,
                "The page width; the whole remainder by default.",
            ),
        ],
    },
    Tool {
        name: "dartq_test_cancel",
        description: "Cancel one in-flight test run. Use for: aborting a dartq_test that \noutlived its value. Not for: reading results (dartq_test_events).",
        kind: ToolKind::Daemon("dartq/test/cancel"),
        params: &[req("run_id", ParamKind::Num, "The run to cancel.")],
    },
    Tool {
        name: "dartq_events",
        description: "The session's unified event stream or its cold-analysis progress \
(files scanned, current package) — one LON row per line, the CLI `dartq events` \
stdout itself. Use for: 'is it still working' during a long first analysis, and \
lifecycle reads. Not for: test-run result events (dartq_test_events).",
        kind: ToolKind::Events,
        params: &[
            opt(
                "handle",
                ParamKind::Enum(&["session", "progress"]),
                "The stream to read; session by default.",
            ),
            opt(
                "since",
                ParamKind::Num,
                "The session handle's timestamp floor in milliseconds — the same unit the \
                 rows' ts field carries (default: no floor).",
            ),
            opt(
                "limit",
                ParamKind::Num,
                "At most this many rows, keeping the newest; 100 by default.",
            ),
        ],
    },
    Tool {
        name: "dartq_debug_launch",
        description: "Launch a debug session for a Dart script or a Flutter app (Flutter is \
the primary target). Use for: interactive debugging — launch pauses at start, so arm \
breakpoints via dartq_debug_breakpoint_add, then dartq_debug_resume (the standard \
takeover loop). Not for: analyzing code or running tests.",
        kind: ToolKind::Daemon("dartq/debug/launch"),
        params: &[
            req(
                "script",
                ParamKind::Path,
                "The entry script (bin/main.dart, or the Flutter app's main).",
            ),
            opt(
                "kind",
                ParamKind::Enum(&["dart", "flutter"]),
                "The launch kind; dart by default.",
            ),
            opt(
                "args",
                ParamKind::Strings,
                "Arguments passed to the program.",
            ),
            opt("vm_flags", ParamKind::Strings, "VM flags for the Dart VM."),
            opt(
                "pause_on_start",
                ParamKind::Bool,
                "Pause at start (the default) so breakpoints arm first.",
            ),
            opt("cwd", ParamKind::Path, "The program's working directory."),
            opt(
                "device",
                ParamKind::Str,
                "Flutter: the device id (`flutter run -d`).",
            ),
            opt("flavor", ParamKind::Str, "Flutter: the build flavor."),
            opt(
                "dart_defines",
                ParamKind::Strings,
                "Flutter: --dart-define entries, KEY=VALUE strings.",
            ),
            opt(
                "flutter_args",
                ParamKind::Strings,
                "Flutter: extra flutter run arguments passed through.",
            ),
        ],
    },
    Tool {
        name: "dartq_debug_attach",
        description: "Attach a debug session to a running VM service ws URI — including one \
owned by a live `flutter run` (dartq speaks only the service protocol; the tool's own \
console keeps working). Use for: debugging an already-started program. Not for: starting \
fresh (dartq_debug_launch).",
        kind: ToolKind::Daemon("dartq/debug/attach"),
        params: &[req(
            "ws_uri",
            ParamKind::Str,
            "The VM service web-socket URI to attach to.",
        )],
    },
    Tool {
        name: "dartq_debug_detach",
        description: "Detach one debug session; the program keeps running and the answer \
carries the ws URI for re-attaching later. Use for: ending debugging without stopping \
the app. Not for: terminating it (dartq_debug_stop).",
        kind: ToolKind::Daemon("dartq/debug/detach"),
        params: &[opt(
            "session_id",
            ParamKind::Num,
            "The session to act on; the sole live session by default.",
        )],
    },
    Tool {
        name: "dartq_debug_stop",
        description: "Stop one debug session and its launched program (a Flutter app is shut \
down through the tool's own cleanup ladder — no orphaned app). Use for: ending a launched \
session for good. Not for: keeping it running (dartq_debug_detach).",
        kind: ToolKind::Daemon("dartq/debug/stop"),
        params: &[opt(
            "session_id",
            ParamKind::Num,
            "The session to act on; the sole live session by default.",
        )],
    },
    Tool {
        name: "dartq_debug_status",
        description: "The session's debug sessions and their stopped states. Use for: \norientation after a stop event. Not for: frame detail (dartq_debug_snapshot).",
        kind: ToolKind::Daemon("dartq/debug/status"),
        params: &[opt(
            "session_id",
            ParamKind::Num,
            "One session's status; absent lists every session.",
        )],
    },
    Tool {
        name: "dartq_debug_breakpoint_add",
        description: "Add a breakpoint to one debug session. Use for: stopping at a \nline or function entry. Not for: exception behavior (dartq_debug_exception_pause).",
        kind: ToolKind::Daemon("dartq/debug/breakpointAdd"),
        params: &[
            opt(
                "session_id",
                ParamKind::Num,
                "The session to act on; the sole live session by default.",
            ),
            opt(
                "isolate",
                ParamKind::Str,
                "The isolate; the paused one by default.",
            ),
            req(
                "location",
                BREAKPOINT_LOCATION,
                "The breakpoint target: a source line (script + line, optional column) or a function entry (function_id).",
            ),
            opt(
                "condition",
                ParamKind::Str,
                "A condition expression; the breakpoint hits when true.",
            ),
        ],
    },
    Tool {
        name: "dartq_debug_breakpoint_remove",
        description: "Remove one breakpoint. Use for: clearing a stop you set. Not for: \nresuming (dartq_debug_resume).",
        kind: ToolKind::Daemon("dartq/debug/breakpointRemove"),
        params: &[
            opt(
                "session_id",
                ParamKind::Num,
                "The session to act on; the sole live session by default.",
            ),
            opt(
                "isolate",
                ParamKind::Str,
                "The isolate; the paused one by default.",
            ),
            req(
                "breakpoint_id",
                ParamKind::Str,
                "The breakpoint to remove (the id the add answer carried).",
            ),
        ],
    },
    Tool {
        name: "dartq_debug_exception_pause",
        description: "Set a session's exception pause mode (none/uncaught/all). Use for: \nbreaking on throws. Not for: line stops (dartq_debug_breakpoint_add).",
        kind: ToolKind::Daemon("dartq/debug/exceptionPause"),
        params: &[
            opt(
                "session_id",
                ParamKind::Num,
                "The session to act on; the sole live session by default.",
            ),
            opt(
                "isolate",
                ParamKind::Str,
                "The isolate; the paused one by default.",
            ),
            req(
                "mode",
                ParamKind::Enum(&["none", "uncaught", "all"]),
                "The exception pause mode.",
            ),
        ],
    },
    Tool {
        name: "dartq_debug_resume",
        description: "Resume a paused session. Use for: continuing after any stop. Not for: \nstepping (dartq_debug_step).",
        kind: ToolKind::Daemon("dartq/debug/resume"),
        params: &[
            opt(
                "session_id",
                ParamKind::Num,
                "The session to act on; the sole live session by default.",
            ),
            opt(
                "isolate",
                ParamKind::Str,
                "The isolate; the paused one by default.",
            ),
        ],
    },
    Tool {
        name: "dartq_debug_pause",
        description: "Pause a running session. Use for: interrupting to inspect state. \nNot for: stepping (that rides dartq_debug_step after the pause).",
        kind: ToolKind::Daemon("dartq/debug/pause"),
        params: &[
            opt(
                "session_id",
                ParamKind::Num,
                "The session to act on; the sole live session by default.",
            ),
            opt("isolate", ParamKind::Str, "The isolate to pause."),
        ],
    },
    Tool {
        name: "dartq_debug_step",
        description: "Step (into/over/out) a paused session. Use for: advancing by one \nframe's worth. Not for: free-running (dartq_debug_resume).",
        kind: ToolKind::Daemon("dartq/debug/step"),
        params: &[
            opt(
                "session_id",
                ParamKind::Num,
                "The session to act on; the sole live session by default.",
            ),
            opt(
                "isolate",
                ParamKind::Str,
                "The isolate; the paused one by default.",
            ),
            req(
                "option",
                ParamKind::Enum(&["into", "over", "out"]),
                "The step direction.",
            ),
        ],
    },
    Tool {
        name: "dartq_debug_snapshot",
        description: "A stopped frame snapshot: variables, scopes. Use for: inspecting a \npaused program. Not for: its output stream (dartq_debug_output).",
        kind: ToolKind::Daemon("dartq/debug/snapshot"),
        params: &[
            opt(
                "session_id",
                ParamKind::Num,
                "The session to act on; the sole live session by default.",
            ),
            req(
                "isolate",
                ParamKind::Str,
                "The isolate whose stopped frame is snapshotted.",
            ),
        ],
    },
    Tool {
        name: "dartq_debug_reload",
        description: "Hot reload (reloadSources) — on a Flutter session this is the tool's \
own orchestrated reload (source push + reassemble), so edits on disk take effect. A broken \
edit answers the compile error as data; the session stays usable. Use for: the edit-reload \
half of the loop. Not for: state resets (dartq_debug_restart).",
        kind: ToolKind::Daemon("dartq/debug/reload"),
        params: &[
            opt(
                "session_id",
                ParamKind::Num,
                "The session to act on; the sole live session by default.",
            ),
            opt(
                "isolate",
                ParamKind::Str,
                "The isolate; the paused one by default.",
            ),
            opt(
                "force",
                ParamKind::Bool,
                "Force the reload past a dirty-state refusal.",
            ),
        ],
    },
    Tool {
        name: "dartq_debug_restart",
        description: "Hot restart (Flutter): state resets like the tool's `R`, isolates are \
rebuilt, and the session re-arms its breakpoints on the new isolate (the answer lists them). \
Use for: a clean slate after edits too large for reload. Not for: keeping state \
(dartq_debug_reload).",
        kind: ToolKind::Daemon("dartq/debug/restart"),
        params: &[opt(
            "session_id",
            ParamKind::Num,
            "The session to act on; the sole live session by default.",
        )],
    },
    Tool {
        name: "dartq_debug_evaluate",
        description: "Evaluate one Dart expression in a paused frame (frame_index) or the \
isolate's root library (no frame_index). Use for: inspecting state the snapshot only names, \
verifying a fix from inside the app. Not for: the one-call stopped bundle \
(dartq_debug_snapshot).",
        kind: ToolKind::Daemon("dartq/debug/evaluate"),
        params: &[
            opt(
                "session_id",
                ParamKind::Num,
                "The session to act on; the sole live session by default.",
            ),
            opt(
                "isolate",
                ParamKind::Str,
                "The isolate; the paused one by default.",
            ),
            req(
                "expression",
                ParamKind::Str,
                "The Dart expression to evaluate.",
            ),
            opt(
                "frame_index",
                ParamKind::Num,
                "The paused frame to evaluate in; the root library without it.",
            ),
        ],
    },
    Tool {
        name: "dartq_debug_events",
        description: "Cursor-paged debugger events of one session. Use for: replaying \nwhat the debugger did. Not for: program stdout (dartq_debug_output).",
        kind: ToolKind::Daemon("dartq/debug/events"),
        params: &[
            opt(
                "session_id",
                ParamKind::Num,
                "The session to act on; the sole live session by default.",
            ),
            opt(
                "cursor",
                ParamKind::Num,
                "The event cursor to continue after; 0 by default.",
            ),
            opt("limit", ParamKind::Num, "The page width; 64 by default."),
        ],
    },
    Tool {
        name: "dartq_debug_output",
        description: "Cursor-paged program output of one session. Use for: reading what \nthe program printed. Not for: debugger lifecycle (dartq_debug_events).",
        kind: ToolKind::Daemon("dartq/debug/output"),
        params: &[
            opt(
                "session_id",
                ParamKind::Num,
                "The session to act on; the sole live session by default.",
            ),
            req(
                "stream",
                ParamKind::Enum(&["stdout", "stderr"]),
                "The output stream to read.",
            ),
            opt(
                "cursor",
                ParamKind::Num,
                "The byte cursor to continue after; 0 by default.",
            ),
            opt(
                "max_bytes",
                ParamKind::Num,
                "The page's byte budget; 4096 by default.",
            ),
        ],
    },
];

/// The workspace-status resource's URI.
const STATUS_URI: &str = "dartq://workspace-status";

/// One server's bound context: the worktree, its layout, and whether the
/// resident session is attached (the degradation reason when not).
pub(crate) struct Mcp {
    pub(crate) root: PathBuf,
    pub(crate) layout: Layout,
    pub(crate) session: Result<(), String>,
}

impl Mcp {
    /// Elects or attaches the worktree's resident session — the one
    /// binding every session-converging face shares (this server and
    /// the `debug` verb group's proxy). `session` carries the
    /// standalone degradation reason when the tree cannot host one.
    pub(crate) fn attach(root: PathBuf, sdk: Option<&Path>) -> Self {
        let session = match crate::session::resolved_sdk_root(sdk) {
            Ok(sdk_root) => {
                match dartq_session::ensure(&root, &sdk_root, &crate::session::spawn_server) {
                    dartq_session::Ensured::Attached(_) => Ok(()),
                    dartq_session::Ensured::Standalone(reason) => Err(reason),
                }
            }
            Err(error) => Err(error),
        };
        Self {
            layout: Layout::new(&root),
            root,
            session,
        }
    }
}

pub fn run_entry(args: &McpArgs) -> u8 {
    let root = match std::fs::canonicalize(&args.dir) {
        Ok(root) => root,
        Err(error) => {
            eprintln!("error: cannot resolve {}: {error}", args.dir.display());
            return exit::USAGE;
        }
    };
    let mcp = Mcp::attach(root, args.sdk.as_deref());
    let stdin = io::stdin();
    let stdout = io::stdout();
    let code = serve(BufReader::new(stdin.lock()), stdout.lock(), &mcp);
    match code {
        Ok(code) => code,
        Err(error) => {
            eprintln!("dartq: internal error: mcp transport failed: {error}");
            exit::SOFTWARE
        }
    }
}

/// The stdio loop: one JSON-RPC message per line until EOF.
fn serve<R: Read, W: Write>(input: R, mut output: W, mcp: &Mcp) -> io::Result<u8> {
    let mut input = BufReader::new(input);
    let mut line = String::new();
    loop {
        line.clear();
        if input.read_line(&mut line)? == 0 {
            return Ok(exit::CLEAN);
        }
        let Ok(message) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        let method = message["method"].as_str().unwrap_or_default().to_owned();
        let id = message.get("id").cloned();
        let params = message.get("params").cloned().unwrap_or_else(|| json!({}));
        // Notifications carry no answer.
        let Some(id) = id else {
            continue;
        };
        let result = match method.as_str() {
            "initialize" => Ok(json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {
                    "tools": {"listChanged": false},
                    "resources": {"subscribe": false, "listChanged": false},
                },
                "serverInfo": {"name": "dartq", "version": env!("CARGO_PKG_VERSION")},
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({"tools": TOOLS.iter().map(tool_shape).collect::<Vec<_>>()})),
            "tools/call" => call_tool(&params, mcp, &mut output),
            "resources/list" => Ok(json!({"resources": [
                {"uri": STATUS_URI, "name": "workspace status",
                 "mimeType": "application/json",
                 "description": "The worktree's packages, session identity, and \
                  cold-analysis progress."},
            ]})),
            "resources/read" => read_resource(&params, mcp),
            other => Err(json!({
                "code": -32601,
                "message": format!("method not found: {other}"),
            })),
        };
        let answer = match result {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err(error) => json!({"jsonrpc": "2.0", "id": id, "error": error}),
        };
        writeln!(output, "{answer}")?;
        output.flush()?;
    }
}

/// The MCP tool shape: the real input schema, generated from the same
/// parameter table the dispatch validates against (#2657) — a closed
/// object (`additionalProperties: false`) whose `properties` carry each
/// parameter's type/enum/description and whose `required` names the
/// mandatory ones.
fn tool_shape(tool: &Tool) -> Value {
    let properties: serde_json::Map<String, Value> = tool
        .params
        .iter()
        .map(|param| (param.name.to_owned(), param_schema(param)))
        .collect();
    let required: Vec<Value> = tool
        .params
        .iter()
        .filter(|param| param.required)
        .map(|param| Value::String(param.name.to_owned()))
        .collect();
    json!({
        "name": tool.name,
        "description": tool.description,
        "inputSchema": {
            "type": "object",
            "additionalProperties": false,
            "properties": properties,
            "required": required,
        },
    })
}

/// One parameter's schema fragment.
fn param_schema(param: &Param) -> Value {
    let mut schema = match &param.kind {
        ParamKind::Str | ParamKind::Path => json!({"type": "string"}),
        ParamKind::Num => json!({"type": "integer"}),
        ParamKind::Bool => json!({"type": "boolean"}),
        ParamKind::Enum(values) => json!({"type": "string", "enum": values}),
        ParamKind::Strings => json!({"type": "array", "items": {"type": "string"}}),
        ParamKind::Paths => json!({"type": "array", "items": {"type": "string"}}),
        ParamKind::Object(fields) => tool_shape(&Tool {
            name: "",
            description: "",
            kind: ToolKind::Daemon(""),
            params: fields,
        })["inputSchema"]
            .clone(),
    };
    schema["description"] = Value::String(param.description.to_owned());
    schema
}

/// Validates one call's arguments against the tool's table: every key
/// known, every required key present, every value of its advertised type
/// — `Err` is the INVALID_PARAMS message, naming the parameter.
fn validate_arguments(tool: &Tool, args: &Value) -> Result<(), String> {
    let Some(object) = args.as_object() else {
        return Err("arguments must be an object".to_owned());
    };
    for key in object.keys() {
        if !tool.params.iter().any(|param| param.name == key) {
            return Err(format!("unknown parameter `{key}` for {}", tool.name));
        }
    }
    for param in tool.params {
        let Some(value) = object.get(param.name) else {
            if param.required {
                return Err(format!(
                    "missing required parameter `{}` for {}",
                    param.name, tool.name
                ));
            }
            continue;
        };
        check_value(param, value)
            .map_err(|reason| format!("parameter `{}` {reason}", param.name))?;
    }
    Ok(())
}

/// One value against its parameter's type face.
fn check_value(param: &Param, value: &Value) -> Result<(), String> {
    match &param.kind {
        ParamKind::Str | ParamKind::Path => value
            .as_str()
            .map_or_else(|| Err("must be a string".into()), |_| Ok(())),
        ParamKind::Num => {
            let integer = value
                .as_number()
                .is_some_and(|number| number.is_u64() || number.is_i64());
            integer
                .then_some(())
                .ok_or_else(|| "must be an integer".to_owned())
        }
        ParamKind::Bool => value
            .as_bool()
            .map_or_else(|| Err("must be a boolean".into()), |_| Ok(())),
        ParamKind::Enum(values) => match value.as_str() {
            Some(text) if values.contains(&text) => Ok(()),
            Some(text) => Err(format!(
                "must be one of {} (got `{text}`)",
                values.join(", ")
            )),
            None => Err("must be a string".into()),
        },
        ParamKind::Strings | ParamKind::Paths => value.as_array().map_or_else(
            || Err("must be an array".into()),
            |items| {
                items
                    .iter()
                    .all(|item| item.is_string())
                    .then_some(())
                    .ok_or_else(|| "must be an array of strings".to_owned())
            },
        ),
        ParamKind::Object(fields) => {
            let Some(object) = value.as_object() else {
                return Err("must be an object".to_owned());
            };
            for key in object.keys() {
                if !fields.iter().any(|field| field.name == key) {
                    return Err(format!("must not carry the unknown key `{key}`"));
                }
            }
            for field in fields.iter() {
                if let Some(inner) = object.get(field.name) {
                    check_value(field, inner)
                        .map_err(|reason| format!("key `{}` {reason}", field.name))?;
                }
            }
            Ok(())
        }
    }
}

fn call_tool(params: &Value, mcp: &Mcp, output: &mut impl Write) -> Result<Value, Value> {
    let name = params["name"].as_str().unwrap_or_default();
    let Some(tool) = TOOLS.iter().find(|tool| tool.name == name) else {
        return Err(json!({"code": -32602, "message": format!("unknown tool: {name}")}));
    };
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    if let Err(message) = validate_arguments(tool, &arguments) {
        return Err(json!({"code": -32602, "message": message}));
    }
    // Progress bookends for the long faces, when the caller sent a token.
    let token = params["_meta"]["progressToken"].clone();
    fn notify(output: &mut impl Write, token: &Value, progress: u64) {
        if token.is_null() {
            return;
        }
        let _ = writeln!(
            output,
            "{{\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\",\"params\":{{\"progressToken\":{token},\"progress\":{progress},\"total\":1}}}}"
        );
        let _ = output.flush();
    }
    notify(output, &token, 0);
    // The content text is the face's CLI stdout itself (#2657): the
    // canonical LON line of the answer's value — or, for the
    // multi-document faces (fix, events), the same lines the CLI prints,
    // joined by `\n`.
    let outcome = match tool.kind {
        ToolKind::Daemon(method) => {
            daemon_call(method, &arguments, mcp).map(|value| crate::output::lon_line(&value))
        }
        ToolKind::Fix => daemon_call("dartq/fix", &arguments, mcp).map(fix_text),
        ToolKind::Rename => rename_call(&arguments, mcp).map_err(ProxyError::Transport),
        ToolKind::Events => events_call(&arguments, mcp).map_err(ProxyError::Transport),
    }
    .map_err(|error| match error {
        // The wire error object verbatim — the daemon owns the shape.
        ProxyError::Daemon(code, message) => json!({"code": code, "message": message}).to_string(),
        ProxyError::Transport(reason) => reason,
    });
    notify(output, &token, 1);
    Ok(match outcome {
        Ok(text) => json!({
            "content": [{"type": "text", "text": text}],
            "isError": false,
        }),
        Err(reason) => json!({
            "content": [{"type": "text", "text": reason}],
            "isError": true,
        }),
    })
}

/// The fix face's tool text: the answer envelope's `files` documents,
/// each its own canonical LON line — the CLI `dartq fix --dry-run`
/// stdout (the plugin-error rows the CLI prints on stderr stay off the
/// data face here too).
fn fix_text(answer: Value) -> String {
    answer["files"]
        .as_array()
        .map_or_else(Vec::new, |files| {
            files
                .iter()
                .map(|document| crate::output::lon_line(document))
                .collect()
        })
        .join("\n")
}

/// The daemon-proxy failure face and the transport bound live in
/// `resident` — the binary's one session transport (the routed one-shot
/// faces of #2627 proxy through the same machinery).
pub(crate) use crate::resident::CALL_TIMEOUT as DAEMON_CALL_TIMEOUT;
pub(crate) use crate::resident::ProxyError;

/// One daemon request over the session socket, bounded by
/// [`DAEMON_CALL_TIMEOUT`].
pub(crate) fn daemon_call(method: &str, params: &Value, mcp: &Mcp) -> Result<Value, ProxyError> {
    daemon_call_bounded(method, params, mcp, DAEMON_CALL_TIMEOUT)
}

pub(crate) fn daemon_call_bounded(
    method: &str,
    params: &Value,
    mcp: &Mcp,
    timeout: Duration,
) -> Result<Value, ProxyError> {
    if let Err(reason) = &mcp.session {
        return Err(ProxyError::Transport(format!(
            "no resident session ({reason}) — the engine faces need one; \
             start it with `dartq session start` in a writable checkout"
        )));
    }
    crate::resident::session_call(&mcp.layout.socket, method, params, timeout)
}

/// The rename face over a cold in-process engine (the rare tool). The
/// tool text is the CLI face's own document — [`dartq_commands::rename::to_json`],
/// the one producer both faces print (#2657): `v`, the new name, the
/// resolved target, and the edit rows (canonical file spellings,
/// path-then-offset order). The anchor's path resolves against the
/// server's working directory and canonicalizes, the CLI face's own
/// steps.
fn rename_call(params: &Value, mcp: &Mcp) -> Result<String, String> {
    let target = params["target"]
        .as_str()
        .ok_or("arguments: {target: 'PATH:LINE:COL or PATH:OFFSET', new_name}")?;
    let new_name = params["new_name"]
        .as_str()
        .ok_or("arguments: {target: 'PATH:LINE:COL or PATH:OFFSET', new_name}")?;
    let (path, anchor) = parse_target(target)?;
    // Relative anchor paths resolve against the server's working
    // directory (the CLI's positional base), then canonicalize — the
    // engine interns canonical spellings.
    let path = if path.is_absolute() {
        path
    } else {
        mcp.root.join(path)
    };
    let path = path
        .canonicalize()
        .map_err(|_| format!("Directory or file doesn't exist: {target}"))?;
    let (mut engine, _dir) = dartq_commands::workspace::open_engine(
        &mcp.root,
        std::env::var_os("DART_SDK").map(PathBuf::from).as_deref(),
    )
    .map_err(|error| error.to_string())?;
    let run = dartq_commands::rename::rename(&mut engine, &path, anchor, new_name)
        .map_err(|error| error.to_string())?;
    Ok(crate::output::lon_line(&dartq_commands::rename::to_json(
        &run, new_name,
    )))
}

/// The rename anchor spec: the CLI face's grammar — a trailing
/// `:LINE:COL` pair (1-based; UTF-16 code units), a single trailing
/// `:OFFSET` (0-based UTF-16 code units), anything else malformed. Split from the
/// right so a Windows-style drive path keeps its colon.
fn parse_target(spec: &str) -> Result<(PathBuf, dartq_commands::rename::Anchor), String> {
    let malformed = || format!("expected PATH:LINE:COL or PATH:OFFSET, got `{spec}`");
    let Some((path, last)) = spec.rsplit_once(':') else {
        return Err(malformed());
    };
    if let Some((head, column_text)) = path.rsplit_once(':')
        && !head.is_empty()
        && let (Ok(line), Ok(column)) = (column_text.parse::<u32>(), last.parse::<u32>())
    {
        return Ok((
            PathBuf::from(head),
            dartq_commands::rename::Anchor::LineCol { line, column },
        ));
    }
    if let Ok(offset) = last.parse::<u64>() {
        if path.is_empty() {
            return Err(malformed());
        }
        return Ok((
            PathBuf::from(path),
            dartq_commands::rename::Anchor::Utf16Offset(offset),
        ));
    }
    Err(malformed())
}

/// The events read, local to the worktree. The tool text is the CLI
/// verb's own stdout (#2657): the session handle answers one canonical
/// LON line per row — newest first (#2622), `since` a millisecond floor
/// on the rows' own `ts` unit, `limit` keeping the newest rows — and the
/// progress handle the CLI's own progress row document.
fn events_call(params: &Value, mcp: &Mcp) -> Result<String, String> {
    match params["handle"].as_str().unwrap_or("session") {
        "session" => {
            let log = EventLog::new(&mcp.layout);
            let since_ms = params["since"].as_u64().map_or(0, u128::from);
            let limit = params["limit"].as_u64().unwrap_or(100).min(1000) as usize;
            Ok(log
                .read_newest(None, since_ms, limit)
                .into_iter()
                .map(|(_, value)| crate::output::lon_line(&value))
                .collect::<Vec<_>>()
                .join("\n"))
        }
        "progress" => {
            let progress = Progress::from_trace(&mcp.layout.dir.join("trace.jsonl"));
            Ok(crate::output::lon_line(&crate::events::progress_row(
                &progress,
            )))
        }
        other => Err(format!("unknown handle `{other}` (session | progress)")),
    }
}

fn read_resource(params: &Value, mcp: &Mcp) -> Result<Value, Value> {
    match params["uri"].as_str() {
        Some(STATUS_URI) => {
            let inventory = dartq_commands::packages::inventory(&mcp.root)
                .map_err(|error| json!({"code": -32603, "message": error.to_string()}))?;
            let packages: Vec<Value> = inventory
                .report
                .packages
                .iter()
                .map(|entry| {
                    json!({
                        "dir": entry.dir.display().to_string(),
                        "name": entry.name,
                        "kind": entry.kind.as_str(),
                        "sdk": entry.sdk,
                        "resolved": entry.resolved,
                    })
                })
                .collect();
            let progress = Progress::from_trace(&mcp.layout.dir.join("trace.jsonl"));
            Ok(json!({
                "contents": [{
                    "uri": STATUS_URI,
                    "mimeType": "application/json",
                    "text": json!({
                        "root": mcp.root.display().to_string(),
                        "packages": packages,
                        "session": mcp.session.is_ok(),
                        "progress": {
                            "files_scanned": progress.files_scanned,
                            "current_package": progress.current_package,
                        },
                    })
                    .to_string(),
                }],
            }))
        }
        _ => Err(json!({"code": -32602, "message": "unknown resource"})),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::os::unix::net::UnixStream;

    use crate::resident::{ReadError, read_frame};

    /// The serve fixture's twin (the session tests'): a package with a
    /// stub SDK.
    fn fixture() -> PathBuf {
        crate::session::tests::test_fixture("mcp")
    }

    /// Drives the stdio loop over in-memory pipes; returns the written
    /// lines (responses and notifications in order).
    fn drive(mcp: &Mcp, requests: &[Value]) -> Vec<Value> {
        let mut input = String::new();
        for request in requests {
            input.push_str(&request.to_string());
            input.push('\n');
        }
        let mut output = Vec::new();
        let code = serve(Cursor::new(input.into_bytes()), &mut output, mcp).expect("serve");
        assert_eq!(code, exit::CLEAN);
        String::from_utf8(output)
            .expect("utf-8")
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }

    #[test]
    fn mcp_serves_the_session_over_stdio() {
        // In-process serve loop: collateral of the session module's real
        // signal test (its SIGTERM is process-wide and would tear this
        // serve down mid-drive) — same serialization lock.
        let _serial = crate::session::tests::SIGNAL_SERIALIZATION
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let root = fixture();
        let serve_dir = root.clone();
        let serve_sdk = root.join("sdk");
        let server = std::thread::spawn(move || {
            crate::session::serve_loop_for_tests(&serve_dir, &serve_sdk)
        });
        let layout = Layout::new(&root);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !dartq_session::socket_open(&layout) {
            assert!(
                std::time::Instant::now() < deadline,
                "the serve loop never listened"
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let mcp = Mcp {
            root: root.clone(),
            layout: layout.clone(),
            session: Ok(()),
        };
        let answers = drive(
            &mcp,
            &[
                json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}),
                json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
                json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call",
                       "params": {"name": "dartq_analyze", "arguments": {}}}),
                json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call",
                       "params": {"name": "dartq_events", "arguments": {"handle": "progress"}}}),
                json!({"jsonrpc": "2.0", "id": 5, "method": "resources/read",
                       "params": {"uri": STATUS_URI}}),
                json!({"jsonrpc": "2.0", "id": 6, "method": "tools/call",
                       "params": {"name": "dartq_nope", "arguments": {}}}),
                json!({"jsonrpc": "2.0", "id": 7, "method": "ping"}),
            ],
        );
        let answer = |id: u64| {
            answers
                .iter()
                .find(|value| value["id"].as_u64() == Some(id))
                .unwrap_or_else(|| panic!("answer {id}: {answers:?}"))
                .clone()
        };
        assert_eq!(answer(1)["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(answer(1)["result"]["serverInfo"]["name"], "dartq");
        let tools = answer(2)["result"]["tools"]
            .as_array()
            .cloned()
            .expect("tools");
        assert!(tools.len() >= 20, "{} tools", tools.len());
        assert!(
            tools.iter().all(|tool| {
                tool["description"]
                    .as_str()
                    .is_some_and(|text| text.contains("Use") && text.contains("Not for"))
            }),
            "every tool's description states when to use it and when not"
        );
        let analyze = answer(3)["result"].clone();
        assert!(analyze["isError"].as_bool() == Some(false), "{analyze}");
        assert!(
            analyze["content"][0]["text"]
                .as_str()
                .unwrap_or_default()
                .contains("diags"),
            "the daemon answer rides the MCP content in canonical LON (diagnostics → diags)"
        );
        let progress = answer(4)["result"]["content"][0]["text"].to_string();
        assert!(progress.contains("files_scanned"), "{progress}");
        assert!(
            progress.contains("type:progress"),
            "the CLI verb's own row: {progress}"
        );
        let status = answer(5)["result"]["contents"][0]["text"].to_string();
        assert!(
            status.contains("fx"),
            "the workspace status names the package: {status}"
        );
        assert!(status.contains("packages"), "{status}");
        assert!(answer(6)["error"]["code"].as_i64() == Some(-32602));
        assert!(answer(7)["result"].as_object().is_some(), "ping answers");

        // A degraded tree (no session): the engine tools answer isError
        // with the reason; the local faces keep working.
        let degraded = Mcp {
            root: root.clone(),
            layout,
            session: Err("read-only checkout".into()),
        };
        let answers = drive(
            &degraded,
            &[
                json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                       "params": {"name": "dartq_analyze", "arguments": {}}}),
                json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
                       "params": {"name": "dartq_events", "arguments": {"handle": "progress"}}}),
            ],
        );
        assert!(answers[0]["result"]["isError"].as_bool() == Some(true));
        assert!(
            answers[0]["result"]["content"][0]["text"]
                .as_str()
                .unwrap_or_default()
                .contains("no resident session")
        );
        assert!(
            answers[1]["result"]["isError"].as_bool() == Some(false),
            "local faces survive"
        );

        // Drain the server thread.
        std::thread::sleep(std::time::Duration::from_secs(2));
        let _ = server.join();
        std::fs::remove_dir_all(&root).ok();
    }

    /// The tool table behind `tools/list`: names are unique, every daemon
    /// tool maps to a `dartq/*` method, and the served shape carries the
    /// real closed schema generated from the same table the dispatch
    /// validates against (#2657).
    #[test]
    fn tool_table_is_well_formed() {
        let mut names: Vec<&str> = TOOLS.iter().map(|tool| tool.name).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(names.len(), before, "tool names are unique");
        assert_eq!(TOOLS.len(), 28, "the tool family is exactly 28");
        for tool in TOOLS {
            if let ToolKind::Daemon(method) = tool.kind {
                assert!(
                    method.starts_with("dartq/"),
                    "{} maps to a daemon method",
                    tool.name
                );
            }
            let shape = tool_shape(tool);
            assert_eq!(shape["name"], tool.name);
            assert_eq!(shape["description"], tool.description);
            assert_eq!(shape["inputSchema"]["type"], "object");
            assert_eq!(
                shape["inputSchema"]["additionalProperties"], false,
                "{}: the schema is closed",
                tool.name
            );
            // Every advertised property renders, and `required` names
            // exactly the required ones.
            let properties = shape["inputSchema"]["properties"]
                .as_object()
                .expect("properties");
            assert_eq!(
                properties.len(),
                tool.params.len(),
                "{}: every parameter renders",
                tool.name
            );
            let mut required: Vec<&str> = tool
                .params
                .iter()
                .filter(|param| param.required)
                .map(|param| param.name)
                .collect();
            required.sort_unstable();
            let mut served: Vec<&str> = shape["inputSchema"]["required"]
                .as_array()
                .expect("required")
                .iter()
                .map(|value| value.as_str().expect("name"))
                .collect();
            served.sort_unstable();
            assert_eq!(required, served, "{}: required names match", tool.name);
            for param in tool.params {
                assert!(
                    param.description.len() > 8,
                    "{}.{}: the description says what it is",
                    tool.name,
                    param.name
                );
            }
        }
        // The ruling's flagship schemas: the closed vocabularies.
        let query = tool_shape(
            TOOLS
                .iter()
                .find(|tool| tool.name == "dartq_query")
                .expect("query"),
        );
        assert_eq!(
            query["inputSchema"]["properties"]["kind"]["enum"],
            json!([
                "symbol",
                "references",
                "callers",
                "implementations",
                "dependencies"
            ])
        );
        let fix = tool_shape(
            TOOLS
                .iter()
                .find(|tool| tool.name == "dartq_fix")
                .expect("fix"),
        );
        assert_eq!(
            fix["inputSchema"]["properties"]["role"]["enum"],
            json!(["primary", "ignore_line", "ignore_file", "suppress"])
        );
        assert_eq!(
            fix["inputSchema"]["required"],
            json!(["paths"]),
            "fix requires its paths"
        );
        let evaluate = tool_shape(
            TOOLS
                .iter()
                .find(|tool| tool.name == "dartq_debug_evaluate")
                .expect("evaluate"),
        );
        assert!(
            evaluate["inputSchema"]["properties"]
                .as_object()
                .expect("properties")
                .contains_key("frame_index"),
            "the evaluate schema spells frame_index"
        );
        // The impact face carries the CLI's repeatable --symbol narrowing
        // (#2663): a string array, optional — absent is the file-level
        // posture — spelled `symbols` like the daemon face's own list
        // parameters (sdk_roots, paths, args).
        let impact = tool_shape(
            TOOLS
                .iter()
                .find(|tool| tool.name == "dartq_impact")
                .expect("impact"),
        );
        assert_eq!(
            impact["inputSchema"]["properties"]["symbols"]["type"], "array",
            "the narrowing rides as the CLI's repeatable-flag list"
        );
        assert_eq!(
            impact["inputSchema"]["properties"]["symbols"]["items"]["type"],
            "string"
        );
        assert!(
            !impact["inputSchema"]["required"]
                .as_array()
                .expect("required")
                .iter()
                .any(|value| value.as_str() == Some("symbols")),
            "symbols is optional — absent is the file-level posture"
        );
    }

    /// `tools/call` validates against the table: an unknown parameter,
    /// a missing required one, and a wrong type each answer INVALID_PARAMS
    /// naming the parameter — and a legal call still passes through.
    #[test]
    fn unknown_and_malformed_parameters_answer_invalid_params() {
        let root = std::env::temp_dir().join(format!("dqs-mcp-args-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("dir");
        let mcp = Mcp {
            layout: Layout::new(&root),
            root: root.clone(),
            session: Err("standalone".into()),
        };
        let mut output = Vec::new();
        let call = |output: &mut Vec<u8>, name: &str, arguments: Value| {
            serve(
                Cursor::new(
                    json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                           "params": {"name": name, "arguments": arguments}})
                    .to_string()
                    .into_bytes(),
                ),
                &mut *output,
                &mcp,
            )
            .expect("serve");
            let lines: Vec<Value> = String::from_utf8(std::mem::take(output))
                .expect("utf-8")
                .lines()
                .filter_map(|line| serde_json::from_str(line).ok())
                .collect();
            lines[0].clone()
        };
        let unknown = call(&mut output, "dartq_analyze", json!({"bogus": 1}));
        assert_eq!(unknown["error"]["code"], -32602, "{unknown}");
        assert!(
            unknown["error"]["message"]
                .as_str()
                .unwrap_or_default()
                .contains("`bogus`"),
            "the message names the parameter: {unknown}"
        );
        let missing = call(&mut output, "dartq_query", json!({}));
        assert_eq!(missing["error"]["code"], -32602);
        assert!(
            missing["error"]["message"]
                .as_str()
                .unwrap_or_default()
                .contains("`name`"),
            "the message names the missing parameter: {missing}"
        );
        let typed = call(&mut output, "dartq_query", json!({"name": 7}));
        assert_eq!(typed["error"]["code"], -32602);
        assert!(
            typed["error"]["message"]
                .as_str()
                .unwrap_or_default()
                .contains("`name`"),
            "the message names the mistyped parameter: {typed}"
        );
        let spelled = call(
            &mut output,
            "dartq_fix",
            json!({"paths": ["lib"], "role": "nope"}),
        );
        assert_eq!(spelled["error"]["code"], -32602);
        assert!(
            spelled["error"]["message"]
                .as_str()
                .unwrap_or_default()
                .contains("`role`"),
            "the message names the enum parameter: {spelled}"
        );
        // A legal call reaches the transport tier (no session here — the
        // degradation answer, not a usage error).
        let legal = call(
            &mut output,
            "dartq_query",
            json!({"name": "foo", "kind": "callers"}),
        );
        assert!(
            legal["result"].is_object(),
            "the validated call proceeds: {legal}"
        );
        assert!(
            legal["result"]["isError"].as_bool() == Some(true),
            "no session: {legal}"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// The impact face's `symbols` narrowing reaches the daemon request
    /// verbatim — every spelling, order kept, the CLI's repeatable
    /// `--symbol` list (#2663) — and the daemon's usage answer for an
    /// unknown spelling (its InvalidParams tier carrying the face's own
    /// `no such symbol` wording) rides the MCP face as the tool-result
    /// `isError` text, not the MCP layer's INVALID_PARAMS rejection that
    /// answered before the parameter joined the table.
    #[test]
    fn impact_symbols_ride_the_daemon_request_and_usage_errors_flatten() {
        let root = std::env::temp_dir().join(format!("dqs-mcp-symbols-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("dir");
        let layout = Layout::new(&root);
        std::fs::create_dir_all(&layout.dir).expect("session dir");
        let listener = std::os::unix::net::UnixListener::bind(&layout.socket).expect("bind");
        // The daemon's own answer for an unknown spelling: the impact
        // face maps the usage error to its InvalidParams tier, message
        // and all — the stub speaks that frame back.
        let usage = json!({
            "jsonrpc": "2.0", "id": 1,
            "error": {"code": -32602, "message": "no such symbol in lib/a.dart: NoSuch"},
        });
        let (seen, request) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let frame = match read_frame(&mut stream) {
                Ok(frame) => frame,
                Err(_) => return,
            };
            let body = usage.to_string();
            let _ = write!(stream, "Content-Length: {}\r\n\r\n{body}", body.len());
            let _ = stream.flush();
            let _ = seen.send(frame);
        });
        let mcp = Mcp {
            layout,
            root: root.clone(),
            session: Ok(()),
        };
        let mut output = Vec::new();
        serve(
            Cursor::new(
                json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": {"name": "dartq_impact", "arguments": {
                    "path": "lib/a.dart",
                    "symbols": ["TodoAppService.editDraft", "TodoList", "lib/a.dart:12"],
                }}})
                .to_string()
                .into_bytes(),
            ),
            &mut output,
            &mcp,
        )
        .expect("serve");
        let frame = request
            .recv_timeout(Duration::from_secs(5))
            .expect("the daemon saw the call");
        assert_eq!(frame["method"], "dartq/impact");
        assert_eq!(frame["params"]["path"], "lib/a.dart");
        assert_eq!(
            frame["params"]["symbols"],
            json!(["TodoAppService.editDraft", "TodoList", "lib/a.dart:12"]),
            "every spelling rides the request verbatim, order kept"
        );
        let answer: Value = String::from_utf8(output)
            .expect("utf-8")
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .next()
            .expect("one answer line");
        assert!(
            answer["result"].is_object(),
            "the daemon's usage answer is a tool result, not the MCP layer's \
             rejection: {answer}"
        );
        assert_eq!(answer["result"]["isError"], true);
        let text = answer["result"]["content"][0]["text"]
            .as_str()
            .expect("content text");
        assert!(
            text.contains("no such symbol"),
            "the daemon's usage message rides the isError text: {text}"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// The wire's noise discipline: a line that is not JSON and a
    /// notification (no id) answer nothing; the next real request still
    /// gets its answer, and EOF ends the loop cleanly.
    #[test]
    fn malformed_lines_and_notifications_answer_nothing() {
        let root = std::env::temp_dir().join(format!("dqs-mcp-noise-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("dir");
        let mcp = Mcp {
            layout: Layout::new(&root),
            root: root.clone(),
            session: Err("standalone".into()),
        };
        let input = concat!(
            "this line is not json\n",
            "\n",
            "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":9,\"method\":\"ping\"}\n",
        );
        let mut output = Vec::new();
        let code = serve(Cursor::new(input.as_bytes().to_vec()), &mut output, &mcp).expect("serve");
        assert_eq!(code, exit::CLEAN);
        let lines: Vec<Value> = String::from_utf8(output)
            .expect("utf-8")
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
        assert_eq!(lines.len(), 1, "only the ping answered: {lines:?}");
        assert_eq!(lines[0]["id"], 9);
        assert!(lines[0]["result"].as_object().is_some());
        std::fs::remove_dir_all(&root).ok();
    }

    /// Unknown methods and unknown resources answer the protocol's usage
    /// errors (-32601, -32602) with the offending name in the message.
    #[test]
    fn unknown_method_and_resource_answer_protocol_errors() {
        let root = std::env::temp_dir().join(format!("dqs-mcp-unknown-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("dir");
        let mcp = Mcp {
            layout: Layout::new(&root),
            root: root.clone(),
            session: Err("standalone".into()),
        };
        let answers = drive(
            &mcp,
            &[
                json!({"jsonrpc": "2.0", "id": 1, "method": "workspace/symbol", "params": {}}),
                json!({"jsonrpc": "2.0", "id": 2, "method": "resources/read",
                       "params": {"uri": "dartq://nope"}}),
                json!({"jsonrpc": "2.0", "id": 3, "method": "resources/list"}),
            ],
        );
        let answer = |id: u64| {
            answers
                .iter()
                .find(|value| value["id"].as_u64() == Some(id))
                .unwrap_or_else(|| panic!("answer {id}: {answers:?}"))
                .clone()
        };
        assert_eq!(answer(1)["error"]["code"], -32601);
        assert!(
            answer(1)["error"]["message"]
                .as_str()
                .unwrap_or_default()
                .contains("workspace/symbol"),
            "the message names the method"
        );
        assert_eq!(
            answer(2)["error"]["code"],
            -32602,
            "an unknown resource is a usage error"
        );
        let resources = answer(3)["result"]["resources"]
            .as_array()
            .cloned()
            .expect("resources");
        assert_eq!(resources[0]["uri"], STATUS_URI);
        std::fs::remove_dir_all(&root).ok();
    }

    /// A tools/call carrying `_meta.progressToken` is bracketed by MCP
    /// progress notifications (0 before the work, 1 after), in output
    /// order ahead of the answer; without a token no notification rides
    /// the stream.
    #[test]
    fn progress_token_brackets_a_tool_call() {
        let root = std::env::temp_dir().join(format!("dqs-mcp-progress-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("dir");
        let mcp = Mcp {
            layout: Layout::new(&root),
            root: root.clone(),
            session: Err("read-only checkout".into()),
        };
        let mut input = String::new();
        input.push_str(
            &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                    "params": {"name": "dartq_analyze", "arguments": {},
                               "_meta": {"progressToken": "tok-7"}}})
            .to_string(),
        );
        input.push('\n');
        input.push_str(
            &json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
                    "params": {"name": "dartq_events", "arguments": {"handle": "progress"}}})
            .to_string(),
        );
        input.push('\n');
        let mut output = Vec::new();
        serve(Cursor::new(input.into_bytes()), &mut output, &mcp).expect("serve");
        let lines: Vec<Value> = String::from_utf8(output)
            .expect("utf-8")
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
        let progress: Vec<&Value> = lines
            .iter()
            .filter(|value| value["method"].as_str() == Some("notifications/progress"))
            .collect();
        assert_eq!(
            progress.len(),
            2,
            "exactly the token-carrying call brackets: {lines:?}"
        );
        for (index, value) in progress.iter().enumerate() {
            assert_eq!(value["params"]["progressToken"], "tok-7");
            assert_eq!(
                value["params"]["progress"], index as u64,
                "0 then 1, in order"
            );
            assert_eq!(value["params"]["total"], 1);
        }
        // The notifications precede their call's answer.
        let answer_position = lines
            .iter()
            .position(|value| value["id"].as_u64() == Some(1))
            .expect("answer 1");
        assert!(
            lines[..answer_position]
                .iter()
                .any(|value| value["method"].as_str() == Some("notifications/progress")),
            "the bracket opens before the answer rides the stream"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// One daemon frame over a socketpair: headers parse case-insensitively,
    /// the body must be JSON, and a frame without Content-Length is the
    /// usage error. A peer that accepts but never answers is the wedge
    /// signal: the bounded read answers `Wedged`, not a hang (#2587).
    #[test]
    fn read_frame_parses_headers_and_rejects_missing_length() {
        let (mut mine, mut theirs) = UnixStream::pair().expect("socketpair");
        let body = r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#;
        write!(
            mine,
            "Content-Type: application/json\r\ncontent-length: {}\r\n\r\n{}",
            body.len(),
            body
        )
        .expect("write frame");
        mine.flush().expect("flush");
        let answer = read_frame(&mut theirs).expect("frame");
        assert_eq!(answer["result"]["ok"], true);
        write!(mine, "X-Other: 1\r\n\r\nno length").expect("write headerless");
        mine.flush().expect("flush");
        assert!(matches!(
            read_frame(&mut theirs),
            Err(ReadError::Malformed(message)) if message == "missing Content-Length"
        ));

        // The wedge: a peer holding the socket open without writing.
        // The read deadline turns the would-be hang into `Wedged`.
        let (silent, mut reader) = UnixStream::pair().expect("socketpair");
        std::mem::forget(silent); // held open, never written
        reader
            .set_read_timeout(Some(Duration::from_millis(50)))
            .expect("timeout");
        assert!(matches!(read_frame(&mut reader), Err(ReadError::Wedged)));
    }

    /// A daemon proxy against an engine that never answers fails within
    /// the caller's bound with the wedge message (#2587): the MCP tool
    /// answers `isError`, it never hangs.
    #[test]
    fn daemon_call_answers_iserror_within_bound_against_a_silent_engine() {
        let root = std::env::temp_dir().join(format!("dqs-mcp-wedge-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("dir");
        let layout = Layout::new(&root);
        std::fs::create_dir_all(&layout.dir).expect("session dir");
        let listener = std::os::unix::net::UnixListener::bind(&layout.socket).expect("bind");
        // The silent engine: accepts the connection, never answers.
        std::thread::spawn(move || {
            if let Ok((stream, _)) = listener.accept() {
                std::mem::forget(stream);
            }
        });
        let mcp = Mcp {
            layout,
            root: root.clone(),
            session: Ok(()),
        };
        let started = std::time::Instant::now();
        let error = match daemon_call_bounded(
            "dartq/test/status",
            &json!({}),
            &mcp,
            Duration::from_millis(300),
        )
        .expect_err("the silent engine must answer the bound error")
        {
            ProxyError::Transport(reason) => reason,
            other => panic!("the wedge is a transport failure: {other:?}"),
        };
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "bounded: {error}"
        );
        assert!(
            error.contains("did not answer dartq/test/status"),
            "the error names the method: {error}"
        );
        assert!(error.contains("wedged"), "the error says wedge: {error}");
        std::fs::remove_dir_all(&root).ok();
    }

    /// The rename target grammar — the CLI face's own: a trailing
    /// `:LINE:COL` pair, a single trailing `:OFFSET`, a Windows-style
    /// drive path keeping its colon (split from the right); every other
    /// shape is the usage error.
    #[test]
    fn parse_target_splits_from_the_right() {
        use dartq_commands::rename::Anchor;
        let ok = |spec: &str| parse_target(spec).expect("target");
        assert_eq!(
            ok("lib/a.dart:3:7"),
            (
                PathBuf::from("lib/a.dart"),
                Anchor::LineCol { line: 3, column: 7 }
            )
        );
        assert_eq!(
            ok("lib/a.dart:58"),
            (PathBuf::from("lib/a.dart"), Anchor::Utf16Offset(58))
        );
        assert_eq!(
            ok("/w/a.dart:1:2"),
            (
                PathBuf::from("/w/a.dart"),
                Anchor::LineCol { line: 1, column: 2 }
            )
        );
        assert_eq!(
            ok("C:\\a.dart:1:2"),
            (
                PathBuf::from("C:\\a.dart"),
                Anchor::LineCol { line: 1, column: 2 }
            )
        );
        // The offset form: a single trailing number is a 0-based UTF-16
        // code-unit offset (the CLI face's own grammar).
        assert_eq!(
            ok("a.dart:58"),
            (PathBuf::from("a.dart"), Anchor::Utf16Offset(58))
        );
        for bad in ["a.dart", "a.dart:x", ":58", ""] {
            assert!(parse_target(bad).is_err(), "`{bad}` must be refused");
        }
        // `we:ird/a.dart:2` is the offset form over a colon-bearing path —
        // accepted, exactly like the CLI face's grammar.
        assert_eq!(
            ok("we:ird/a.dart:2"),
            (PathBuf::from("we:ird/a.dart"), Anchor::Utf16Offset(2))
        );
    }

    /// The events tool is a local face: without a session it still reads
    /// the event log (newest first, millisecond floor on the rows' own
    /// `ts` unit, limit keeping the newest rows) and the progress
    /// snapshot, and an unknown handle is the usage error. The session
    /// handle's text is the CLI verb's own stdout: one canonical LON
    /// line per row, joined by `\n` — never a wrapper array, so no row
    /// count can flip the encoding (#2657).
    #[test]
    fn events_tool_reads_local_faces_without_a_session() {
        let root = std::env::temp_dir().join(format!("dqs-mcp-events-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("dir");
        let layout = Layout::new(&root);
        let log = EventLog::new(&layout);
        log.append("session", "serving", json!({})).expect("append");
        log.append("session", "client_attached", json!({"pid": 7}))
            .expect("append");
        let mcp = Mcp {
            layout,
            root: root.clone(),
            session: Err("read-only checkout".into()),
        };
        let now_ms = u64::try_from(dartq_session::SessionState::now_ms()).expect("unix ms");

        let events = events_call(&json!({}), &mcp).expect("default handle is session");
        let lines: Vec<&str> = events.split('\n').collect();
        assert_eq!(lines.len(), 2, "one line per row: {events}");
        assert!(
            lines.iter().all(|line| line.starts_with('{')),
            "every row is its own object document, no wrapper array: {events}"
        );
        let first: Value = lon::from_str(lines[0]).expect("lon row");
        assert_eq!(
            first["event"], "client_attached",
            "newest first: the last append leads the page"
        );
        let second: Value = lon::from_str(lines[1]).expect("lon row");
        assert_eq!(second["event"], "serving");

        // The millisecond floor (#2657): the same unit the rows' ts carry.
        let floored = events_call(&json!({"since": now_ms - 60_000}), &mcp).expect("floor read");
        assert_eq!(floored.split('\n').count(), 2);
        let future = events_call(&json!({"since": now_ms + 60_000}), &mcp).expect("future floor");
        assert!(future.is_empty(), "a future floor reads nothing: {future}");
        // A seconds-scale floor would read nothing under the old contract
        // — the rows' ts (≈1.7e12) sit far above it — so this also pins
        // that the parameter is milliseconds, not seconds.
        assert!(
            !events_call(&json!({"since": now_ms / 1000}), &mcp)
                .expect("seconds-scale floor")
                .is_empty(),
            "since is milliseconds: a seconds-scale number floors nothing"
        );
        let limited = events_call(&json!({"limit": 1}), &mcp).expect("limit read");
        let page: Vec<&str> = limited.split('\n').collect();
        assert_eq!(page.len(), 1);
        let newest: Value = lon::from_str(page[0]).expect("lon row");
        assert_eq!(
            newest["event"], "client_attached",
            "the limit keeps the newest row"
        );

        let progress = events_call(&json!({"handle": "progress"}), &mcp).expect("progress");
        assert_eq!(
            progress,
            "{type:progress files_scanned:0 from_cache:0 current:null current_package:null cursor:0}",
            "no trace in this tree — the CLI verb's own row document"
        );

        let error = events_call(&json!({"handle": "bogus"}), &mcp).expect_err("unknown handle");
        assert!(error.contains("unknown handle"), "{error}");
        std::fs::remove_dir_all(&root).ok();
    }

    /// The events page's shape is a function of the data alone (#2657):
    /// a 40-row page — well past the 31/32 boundary a wrapper array once
    /// flipped at — answers 40 standalone row documents, heterogeneous
    /// `data` shapes included; no row count can change the encoding.
    #[test]
    fn a_40_row_events_page_keeps_one_document_per_row() {
        let root = std::env::temp_dir().join(format!("dqs-mcp-events-40-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("dir");
        let layout = Layout::new(&root);
        let log = EventLog::new(&layout);
        for index in 0..40 {
            // Heterogeneous data shapes on purpose: a uniform table is
            // what a wrapper array would need, and mixed shapes are what
            // flipped it before.
            let data = if index % 2 == 0 {
                json!({"pid": index})
            } else {
                json!({"run": index, "label": format!("run-{index}")})
            };
            log.append("session", "test_event", data).expect("append");
        }
        let mcp = Mcp {
            layout,
            root: root.clone(),
            session: Err("standalone".into()),
        };
        let text = events_call(&json!({"limit": 40}), &mcp).expect("page");
        let lines: Vec<&str> = text.split('\n').collect();
        assert_eq!(lines.len(), 40, "every row answers: {text}");
        assert!(
            lines
                .iter()
                .all(|line| line.starts_with('{') && line.ends_with('}')),
            "40 object documents, no record table, no wrapper: {text}"
        );
        let newest: Value = lon::from_str(lines[0]).expect("lon row");
        assert_eq!(newest["seq"], 40, "newest first: {newest}");
        std::fs::remove_dir_all(&root).ok();
    }
}

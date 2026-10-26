//! The `dartq debug` shell: the debug face's CLI verb group — a thin
//! proxy to the worktree's resident session over the same daemon-call
//! transport the MCP server uses ([`crate::mcp`]). One engine, one
//! protocol: every subcommand maps to exactly one `dartq/debug/*`
//! daemon method with the daemon's own parameter spellings (`--wsUri`,
//! `--sessionId`, `--dartDefines`, …), so the CLI and MCP faces are
//! interchangeable — whatever one can drive, the other can too.
//!
//! Output contract: one JSON line per call on stdout — the daemon's
//! answer verbatim (the debug face is a machine channel, like `session`
//! and `events`). Exit tiers: 0 the answer rode stdout; 1 the daemon
//! answered a domain error (one `{"error":{"code","message"}}` line on
//! stdout — the debug family's `DARTQ/DEBUG/*` machine-code prefix
//! rides the message); 64 usage (clap or an unresolvable worktree);
//! 4 the proxy never got an answer (no resident session, transport,
//! wedge).

use std::path::{Path, PathBuf};

use clap::{Args, Subcommand, ValueEnum};
use serde_json::{Value, json};

use crate::exit;
use crate::mcp::{Mcp, ProxyError, daemon_call};

/// `dartq debug` — drive debug sessions on the resident engine.
#[derive(Args)]
pub struct DebugArgs {
    #[command(subcommand)]
    command: DebugCommand,
}

#[derive(Subcommand)]
enum DebugCommand {
    /// Launch a debug session for a Dart script or a Flutter app (the
    /// primary target). The app pauses at start — the takeover window:
    /// arm breakpoints with `breakpoint add`, then `resume`. Not for:
    /// debugging an already-running program (`attach`).
    Launch(LaunchArgs),
    /// Attach a debug session to a running VM service ws URI —
    /// including one owned by a live `flutter run` (dartq speaks only
    /// the service protocol; the tool's own console keeps working).
    /// Not for: starting fresh (`launch`).
    Attach(AttachArgs),
    /// Detach one debug session; the program keeps running and the
    /// answer carries the ws URI for re-attaching later. Not for:
    /// terminating it (`stop`).
    Detach(SessionIdArgs),
    /// Stop one debug session and its launched program (a Flutter app
    /// is shut down through the tool's own cleanup ladder — no
    /// orphaned app). Not for: keeping it running (`detach`).
    Stop(SessionIdArgs),
    /// The worktree's debug sessions and their stopped states —
    /// orientation after a stop event. With `--sessionId`, one
    /// session's detail including its breakpoints.
    Status(StatusArgs),
    /// A stopped frame snapshot of one isolate: why it stopped, where,
    /// the stack summary, and the top frame's variables with
    /// lazy-expansion handles. Not for: the program's output (`output`).
    Snapshot(SnapshotArgs),
    /// Evaluate one Dart expression in a paused frame (`--frameIndex`)
    /// or the isolate's root library. Use for: inspecting state the
    /// snapshot only names; verifying a fix from inside the app.
    Evaluate(EvaluateArgs),
    /// Step a paused session: into / over / out. Not for: free-running
    /// (`resume`).
    Step(StepArgs),
    /// Interrupt a running session to inspect state. Not for: stepping
    /// (that rides `step` after the pause) or resuming (`resume`).
    Pause(FlowArgs),
    /// Resume a paused session. Use for: continuing after any stop.
    /// Not for: stepping (`step`).
    Resume(FlowArgs),
    /// Add or remove breakpoints.
    Breakpoint(BreakpointArgs),
    /// Set a session's exception pause mode: none / uncaught / all.
    /// Use for: breaking on throws. Not for: line stops
    /// (`breakpoint add`).
    #[command(name = "exception-pause")]
    ExceptionPause(ExceptionPauseArgs),
    /// Hot reload (`reloadSources`): on a Flutter session this is the
    /// tool's own orchestrated reload (source push + reassemble), so
    /// edits on disk take effect. A broken edit answers the compile
    /// error as data; the session stays usable. Not for: state resets
    /// (`restart`).
    Reload(ReloadArgs),
    /// Hot restart (Flutter): state resets like the tool's `R`,
    /// isolates are rebuilt, and the session re-arms its breakpoints on
    /// the new isolate (the answer lists them). Not for: keeping state
    /// (`reload`).
    Restart(SessionIdArgs),
    /// Cursor-paged debugger events of one session. Use for: replaying
    /// what the debugger did. Not for: program stdout (`output`).
    Events(EventsArgs),
    /// Cursor-paged program output of one session. Use for: reading
    /// what the program printed. Not for: debugger lifecycle (`events`).
    Output(OutputArgs),
}

/// The worktree every subcommand binds to — flattened into each.
#[derive(Args)]
struct WorktreeArgs {
    /// The worktree whose resident session drives the debug face;
    /// defaults to the current directory.
    #[arg(long, default_value = ".")]
    dir: PathBuf,
    /// Dart SDK root; defaults to `$DART_SDK`.
    #[arg(long)]
    sdk: Option<PathBuf>,
}

#[derive(Args)]
struct LaunchArgs {
    /// The entrypoint to run: a Dart script (`dart <script>`) or a
    /// Flutter target (`flutter run -t <script>`); relative resolves
    /// against the worktree root.
    script: PathBuf,
    /// Which toolchain launches.
    #[arg(long, value_enum, default_value_t = LaunchKindArg::Dart)]
    kind: LaunchKindArg,
    /// Arguments passed to the program after the script; everything
    /// after `--` rides along verbatim.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 0..)]
    args: Vec<String>,
    /// VM flags ahead of the script (Dart launches only — a Flutter
    /// launch rejects them; pass tool flags through `--flutterArgs`).
    #[arg(long = "vmFlags", value_name = "FLAG", allow_hyphen_values = true)]
    vm_flags: Vec<String>,
    /// Release the start pause: the program runs immediately. The
    /// default pauses at start — the takeover window for arming
    /// breakpoints. `--pauseOnStart` restores it.
    #[arg(long = "no-pauseOnStart", overrides_with = "pause_on_start")]
    no_pause_on_start: bool,
    /// The positive spelling — see `no_pause_on_start`.
    #[arg(long = "pauseOnStart", hide = true, overrides_with = "no_pause_on_start")]
    pause_on_start: bool,
    /// Working directory for the launched program; relative resolves
    /// against the worktree root.
    #[arg(long)]
    cwd: Option<PathBuf>,
    /// Flutter: the device id (`flutter run -d`).
    #[arg(long, value_name = "ID")]
    device: Option<String>,
    /// Flutter: the build flavor (`flutter run --flavor`).
    #[arg(long)]
    flavor: Option<String>,
    /// Flutter: `--dart-define` entries, `KEY=VALUE`; repeatable.
    #[arg(long = "dartDefines", value_name = "KEY=VALUE")]
    dart_defines: Vec<String>,
    /// Flutter: extra `flutter run` arguments passed through verbatim;
    /// repeatable.
    #[arg(long = "flutterArgs", value_name = "FLAG", allow_hyphen_values = true)]
    flutter_args: Vec<String>,
    #[command(flatten)]
    worktree: WorktreeArgs,
}

#[derive(Args)]
struct AttachArgs {
    /// The VM service WebSocket URI to attach to.
    #[arg(long = "wsUri", value_name = "WS_URI")]
    ws_uri: String,
    #[command(flatten)]
    worktree: WorktreeArgs,
}

/// The `--sessionId`-only faces (`detach` / `stop` / `restart`).
#[derive(Args)]
struct SessionIdArgs {
    /// The debug session id (`launch`/`attach`'s answer, or `status`).
    #[arg(long = "sessionId")]
    session_id: u32,
    #[command(flatten)]
    worktree: WorktreeArgs,
}

#[derive(Args)]
struct StatusArgs {
    /// One session's detail (with its breakpoints); absent lists every
    /// debug session of the worktree's resident engine.
    #[arg(long = "sessionId")]
    session_id: Option<u32>,
    #[command(flatten)]
    worktree: WorktreeArgs,
}

#[derive(Args)]
struct SnapshotArgs {
    #[arg(long = "sessionId")]
    session_id: u32,
    /// The isolate to read (`launch`'s `isolateIds`, or `status`).
    #[arg(long)]
    isolate: String,
    #[command(flatten)]
    worktree: WorktreeArgs,
}

#[derive(Args)]
struct EvaluateArgs {
    #[arg(long = "sessionId")]
    session_id: u32,
    /// The Dart expression to evaluate.
    expression: String,
    /// Evaluate in this frame of the paused isolate; absent evaluates
    /// in the isolate's root library.
    #[arg(long = "frameIndex")]
    frame_index: Option<u32>,
    #[arg(long)]
    isolate: Option<String>,
    #[command(flatten)]
    worktree: WorktreeArgs,
}

#[derive(Args)]
struct StepArgs {
    #[arg(long = "sessionId")]
    session_id: u32,
    /// How to step.
    option: StepOptionArg,
    #[arg(long)]
    isolate: Option<String>,
    #[command(flatten)]
    worktree: WorktreeArgs,
}

/// The `pause` / `resume` faces: one session, optionally one isolate.
#[derive(Args)]
struct FlowArgs {
    #[arg(long = "sessionId")]
    session_id: u32,
    #[arg(long)]
    isolate: Option<String>,
    #[command(flatten)]
    worktree: WorktreeArgs,
}

#[derive(Args)]
struct BreakpointArgs {
    #[command(subcommand)]
    command: BreakpointCommand,
}

#[derive(Subcommand)]
enum BreakpointCommand {
    /// Arm a line breakpoint (`--script` with `--line`, optional
    /// `--column`) or a function-entry breakpoint (`--functionId`);
    /// `--condition` makes it conditional. Not for: exception stops
    /// (`exception-pause`).
    Add(BreakpointAddArgs),
    /// Remove one breakpoint by id (`breakpoint add`'s answer, or
    /// `status`). Not for: resuming (`resume`).
    #[command(name = "rm")]
    Remove(BreakpointRemoveArgs),
}

#[derive(Args)]
struct BreakpointAddArgs {
    #[arg(long = "sessionId")]
    session_id: u32,
    /// The script URI of the line to break on
    /// (`package:app/main.dart`); pair with `--line`.
    #[arg(long, requires = "line")]
    script: Option<String>,
    /// The 1-based line to break on; pair with `--script`.
    #[arg(long, requires = "script")]
    line: Option<u32>,
    /// Column narrowing within the line.
    #[arg(long, requires = "line")]
    column: Option<u32>,
    /// A function entry to break on (`functions/42`) — the alternative
    /// to the `--script`/`--line` pair.
    #[arg(long = "functionId", value_name = "ID", conflicts_with_all = ["script", "line"])]
    function_id: Option<String>,
    /// A Dart condition; the breakpoint fires only when it evaluates
    /// true.
    #[arg(long)]
    condition: Option<String>,
    #[arg(long)]
    isolate: Option<String>,
    #[command(flatten)]
    worktree: WorktreeArgs,
}

#[derive(Args)]
struct BreakpointRemoveArgs {
    #[arg(long = "sessionId")]
    session_id: u32,
    /// The breakpoint id to remove.
    #[arg(long = "breakpointId", value_name = "ID")]
    breakpoint_id: String,
    #[arg(long)]
    isolate: Option<String>,
    #[command(flatten)]
    worktree: WorktreeArgs,
}

#[derive(Args)]
struct ExceptionPauseArgs {
    #[arg(long = "sessionId")]
    session_id: u32,
    /// When to pause on exceptions.
    mode: PauseModeArg,
    #[arg(long)]
    isolate: Option<String>,
    #[command(flatten)]
    worktree: WorktreeArgs,
}

#[derive(Args)]
struct ReloadArgs {
    #[arg(long = "sessionId")]
    session_id: u32,
    #[arg(long)]
    isolate: Option<String>,
    /// Force the reload past the VM's viability check.
    #[arg(long)]
    force: bool,
    #[command(flatten)]
    worktree: WorktreeArgs,
}

#[derive(Args)]
struct EventsArgs {
    #[arg(long = "sessionId")]
    session_id: u32,
    /// The cursor: events after this position (the last page's
    /// `nextCursor`); absent reads from the head.
    #[arg(long)]
    since: Option<u64>,
    /// At most this many events per page.
    #[arg(long, default_value_t = 64)]
    limit: usize,
    #[command(flatten)]
    worktree: WorktreeArgs,
}

#[derive(Args)]
struct OutputArgs {
    #[arg(long = "sessionId")]
    session_id: u32,
    /// Which stream to read.
    #[arg(long, value_enum, default_value_t = StreamArg::Stdout)]
    stream: StreamArg,
    /// The cursor: output after this byte position (the last page's
    /// `nextCursor`); absent reads from the head.
    #[arg(long)]
    since: Option<u64>,
    /// At most this many bytes per page.
    #[arg(long = "maxBytes", default_value_t = 4096)]
    max_bytes: usize,
    #[command(flatten)]
    worktree: WorktreeArgs,
}

// The enum-spelled selectors — the daemon's wire words, exactly.

/// Which toolchain a launch drives.
#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum LaunchKindArg {
    /// `dart <script>`.
    Dart,
    /// `flutter run -t <script>` — the primary target platform.
    Flutter,
}

impl LaunchKindArg {
    fn wire(self) -> &'static str {
        match self {
            Self::Dart => "dart",
            Self::Flutter => "flutter",
        }
    }
}

/// How to step.
#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum StepOptionArg {
    /// Into the callee.
    Into,
    /// Over the callee.
    Over,
    /// Out of the current frame.
    Out,
}

impl StepOptionArg {
    fn wire(self) -> &'static str {
        match self {
            Self::Into => "into",
            Self::Over => "over",
            Self::Out => "out",
        }
    }
}

/// When to pause on exceptions.
#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum PauseModeArg {
    #[value(name = "none")]
    None,
    /// Pause only on uncaught throws.
    #[value(name = "uncaught")]
    Uncaught,
    /// Pause on every throw.
    #[value(name = "all")]
    All,
}

impl PauseModeArg {
    fn wire(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Uncaught => "uncaught",
            Self::All => "all",
        }
    }
}

/// Which program stream to read.
#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum StreamArg {
    /// The program's stdout.
    Stdout,
    /// The program's stderr.
    Stderr,
}

impl StreamArg {
    fn wire(self) -> &'static str {
        match self {
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
        }
    }
}

// ---------------------------------------------------------------------------
// The proxy mapping
// ---------------------------------------------------------------------------

/// One parsed subcommand's proxy target: the worktree the session binds
/// to, plus the daemon method and its parameters.
struct Request<'a> {
    dir: &'a Path,
    sdk: Option<&'a Path>,
    method: &'static str,
    params: Value,
}

impl DebugArgs {
    /// The proxy mapping — every subcommand to its `dartq/debug/*`
    /// method and the daemon's own parameter spellings. Pure: no I/O,
    /// no session, unit-testable in isolation.
    fn request(&self) -> Request<'_> {
        let (worktree, method, params) = match &self.command {
            DebugCommand::Launch(args) => {
                let mut params = json!({
                    "script": &args.script,
                    "kind": args.kind.wire(),
                    "args": &args.args,
                    "vmFlags": &args.vm_flags,
                    "dartDefines": &args.dart_defines,
                    "flutterArgs": &args.flutter_args,
                });
                // The daemon's default is pause-on-start; only the
                // explicit opt-out rides the wire.
                if args.no_pause_on_start {
                    params["pauseOnStart"] = json!(false);
                }
                if let Some(cwd) = &args.cwd {
                    params["cwd"] = json!(cwd);
                }
                if let Some(device) = &args.device {
                    params["device"] = json!(device);
                }
                if let Some(flavor) = &args.flavor {
                    params["flavor"] = json!(flavor);
                }
                (&args.worktree, "dartq/debug/launch", params)
            }
            DebugCommand::Attach(args) => {
                (&args.worktree, "dartq/debug/attach", json!({"wsUri": &args.ws_uri}))
            }
            DebugCommand::Detach(args) => (
                &args.worktree,
                "dartq/debug/detach",
                json!({"sessionId": args.session_id}),
            ),
            DebugCommand::Stop(args) => {
                (&args.worktree, "dartq/debug/stop", json!({"sessionId": args.session_id}))
            }
            DebugCommand::Status(args) => {
                let mut params = json!({});
                if let Some(id) = args.session_id {
                    params["sessionId"] = json!(id);
                }
                (&args.worktree, "dartq/debug/status", params)
            }
            DebugCommand::Snapshot(args) => (
                &args.worktree,
                "dartq/debug/snapshot",
                json!({"sessionId": args.session_id, "isolate": &args.isolate}),
            ),
            DebugCommand::Evaluate(args) => {
                let mut params = json!({
                    "sessionId": args.session_id,
                    "expression": &args.expression,
                });
                if let Some(frame) = args.frame_index {
                    params["frameIndex"] = json!(frame);
                }
                if let Some(isolate) = &args.isolate {
                    params["isolate"] = json!(isolate);
                }
                (&args.worktree, "dartq/debug/evaluate", params)
            }
            DebugCommand::Step(args) => {
                let mut params = json!({
                    "sessionId": args.session_id,
                    "option": args.option.wire(),
                });
                if let Some(isolate) = &args.isolate {
                    params["isolate"] = json!(isolate);
                }
                (&args.worktree, "dartq/debug/step", params)
            }
            DebugCommand::Pause(args) => {
                let mut params = json!({"sessionId": args.session_id});
                if let Some(isolate) = &args.isolate {
                    params["isolate"] = json!(isolate);
                }
                (&args.worktree, "dartq/debug/pause", params)
            }
            DebugCommand::Resume(args) => {
                let mut params = json!({"sessionId": args.session_id});
                if let Some(isolate) = &args.isolate {
                    params["isolate"] = json!(isolate);
                }
                (&args.worktree, "dartq/debug/resume", params)
            }
            DebugCommand::Breakpoint(breakpoint) => match &breakpoint.command {
                BreakpointCommand::Add(args) => {
                    let mut location = json!({});
                    if let (Some(script), line) = (&args.script, args.line) {
                        location["script"] = json!(script);
                        location["line"] = json!(line);
                        if let Some(column) = args.column {
                            location["column"] = json!(column);
                        }
                    } else if let Some(function) = &args.function_id {
                        location["functionId"] = json!(function);
                    }
                    let mut params = json!({
                        "sessionId": args.session_id,
                        "location": location,
                    });
                    if let Some(condition) = &args.condition {
                        params["condition"] = json!(condition);
                    }
                    if let Some(isolate) = &args.isolate {
                        params["isolate"] = json!(isolate);
                    }
                    (&args.worktree, "dartq/debug/breakpointAdd", params)
                }
                BreakpointCommand::Remove(args) => {
                    let mut params = json!({
                        "sessionId": args.session_id,
                        "breakpointId": &args.breakpoint_id,
                    });
                    if let Some(isolate) = &args.isolate {
                        params["isolate"] = json!(isolate);
                    }
                    (&args.worktree, "dartq/debug/breakpointRemove", params)
                }
            },
            DebugCommand::ExceptionPause(args) => {
                let mut params = json!({
                    "sessionId": args.session_id,
                    "mode": args.mode.wire(),
                });
                if let Some(isolate) = &args.isolate {
                    params["isolate"] = json!(isolate);
                }
                (&args.worktree, "dartq/debug/exceptionPause", params)
            }
            DebugCommand::Reload(args) => {
                let mut params = json!({"sessionId": args.session_id});
                if let Some(isolate) = &args.isolate {
                    params["isolate"] = json!(isolate);
                }
                if args.force {
                    params["force"] = json!(true);
                }
                (&args.worktree, "dartq/debug/reload", params)
            }
            DebugCommand::Restart(args) => {
                (&args.worktree, "dartq/debug/restart", json!({"sessionId": args.session_id}))
            }
            DebugCommand::Events(args) => {
                let mut params = json!({
                    "sessionId": args.session_id,
                    "limit": args.limit,
                });
                if let Some(since) = args.since {
                    params["cursor"] = json!(since);
                }
                (&args.worktree, "dartq/debug/events", params)
            }
            DebugCommand::Output(args) => {
                let mut params = json!({
                    "sessionId": args.session_id,
                    "stream": args.stream.wire(),
                    "maxBytes": args.max_bytes,
                });
                if let Some(since) = args.since {
                    params["cursor"] = json!(since);
                }
                (&args.worktree, "dartq/debug/output", params)
            }
        };
        Request {
            dir: &worktree.dir,
            sdk: worktree.sdk.as_deref(),
            method,
            params,
        }
    }
}

// ---------------------------------------------------------------------------
// The entry
// ---------------------------------------------------------------------------

pub fn run_entry(args: &DebugArgs) -> u8 {
    let request = args.request();
    let root = match std::fs::canonicalize(request.dir) {
        Ok(root) => root,
        Err(error) => {
            eprintln!("error: cannot resolve {}: {error}", request.dir.display());
            return exit::USAGE;
        }
    };
    let mcp = Mcp::attach(root, request.sdk);
    answer_tier(daemon_call(request.method, &request.params, &mcp))
}

/// Prints one proxy outcome per the output contract and maps it to its
/// exit tier.
fn answer_tier(outcome: Result<Value, ProxyError>) -> u8 {
    match outcome {
        Ok(answer) => {
            println!("{answer}");
            exit::CLEAN
        }
        Err(ProxyError::Daemon(code, message)) => {
            println!("{}", json!({"error": {"code": code, "message": message}}));
            exit::DEBUG_DOMAIN
        }
        Err(ProxyError::Transport(reason)) => {
            eprintln!("dartq: debug: {reason}");
            exit::INTERNAL_ERROR
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// `derive(Args)` brings no `try_parse_from`; parse through the
    /// augmented command the parent `Cli` would splice in.
    fn parse(args: &[&str]) -> Result<DebugArgs, clap::Error> {
        let command = <DebugArgs as clap::Args>::augment_args(clap::Command::new("debug"));
        let matches =
            command.try_get_matches_from(std::iter::once("debug").chain(args.iter().copied()))?;
        <DebugArgs as clap::FromArgMatches>::from_arg_matches(&matches)
    }

    /// The proxy mapping, table-driven: every subcommand parses and maps
    /// to exactly its daemon method with the daemon's own parameter
    /// spellings — the CLI face's whole contract in one place.
    #[test]
    fn every_subcommand_maps_to_its_daemon_request() {
        let cases: Vec<(&[&str], &str, Value)> = vec![
            (
                &["launch", "bin/main.dart"],
                "dartq/debug/launch",
                json!({
                    "script": "bin/main.dart", "kind": "dart",
                    "args": [], "vmFlags": [], "dartDefines": [], "flutterArgs": [],
                }),
            ),
            (
                &[
                    "launch", "--kind", "flutter", "--device", "macos", "--flavor", "dev",
                    "--dartDefines", "A=1", "--dartDefines", "B=2", "--flutterArgs",
                    "--no-devtools", "--vmFlags", "--enable-asserts", "--cwd", "bin",
                    "--no-pauseOnStart", "lib/main.dart",
                ],
                "dartq/debug/launch",
                json!({
                    "script": "lib/main.dart", "kind": "flutter",
                    "args": [], "vmFlags": ["--enable-asserts"],
                    "dartDefines": ["A=1", "B=2"], "flutterArgs": ["--no-devtools"],
                    "pauseOnStart": false, "cwd": "bin", "device": "macos", "flavor": "dev",
                }),
            ),
            (
                &["launch", "bin/main.dart", "--", "--fast", "-x"],
                "dartq/debug/launch",
                json!({
                    "script": "bin/main.dart", "kind": "dart",
                    "args": ["--fast", "-x"], "vmFlags": [], "dartDefines": [],
                    "flutterArgs": [],
                }),
            ),
            (
                &["attach", "--wsUri", "ws://127.0.0.1:8181/ws"],
                "dartq/debug/attach",
                json!({"wsUri": "ws://127.0.0.1:8181/ws"}),
            ),
            (&["detach", "--sessionId", "3"], "dartq/debug/detach", json!({"sessionId": 3})),
            (&["stop", "--sessionId", "3"], "dartq/debug/stop", json!({"sessionId": 3})),
            (&["status"], "dartq/debug/status", json!({})),
            (&["status", "--sessionId", "2"], "dartq/debug/status", json!({"sessionId": 2})),
            (
                &["snapshot", "--sessionId", "1", "--isolate", "isolates/7"],
                "dartq/debug/snapshot",
                json!({"sessionId": 1, "isolate": "isolates/7"}),
            ),
            (
                &[
                    "evaluate", "--sessionId", "1", "--frameIndex", "0", "--isolate",
                    "isolates/7", "count + 1",
                ],
                "dartq/debug/evaluate",
                json!({"sessionId": 1, "expression": "count + 1", "frameIndex": 0, "isolate": "isolates/7"}),
            ),
            (
                &["step", "--sessionId", "1", "over"],
                "dartq/debug/step",
                json!({"sessionId": 1, "option": "over"}),
            ),
            (
                &["pause", "--sessionId", "1", "--isolate", "isolates/7"],
                "dartq/debug/pause",
                json!({"sessionId": 1, "isolate": "isolates/7"}),
            ),
            (
                &["resume", "--sessionId", "1"],
                "dartq/debug/resume",
                json!({"sessionId": 1}),
            ),
            (
                &[
                    "breakpoint", "add", "--sessionId", "1", "--script",
                    "package:app/main.dart", "--line", "12", "--column", "5",
                    "--condition", "x > 3",
                ],
                "dartq/debug/breakpointAdd",
                json!({
                    "sessionId": 1,
                    "location": {"script": "package:app/main.dart", "line": 12, "column": 5},
                    "condition": "x > 3",
                }),
            ),
            (
                &["breakpoint", "add", "--sessionId", "1", "--functionId", "functions/42"],
                "dartq/debug/breakpointAdd",
                json!({"sessionId": 1, "location": {"functionId": "functions/42"}}),
            ),
            (
                &["breakpoint", "rm", "--sessionId", "1", "--breakpointId", "breakpoints/3"],
                "dartq/debug/breakpointRemove",
                json!({"sessionId": 1, "breakpointId": "breakpoints/3"}),
            ),
            (
                &["exception-pause", "--sessionId", "1", "uncaught"],
                "dartq/debug/exceptionPause",
                json!({"sessionId": 1, "mode": "uncaught"}),
            ),
            (
                &["reload", "--sessionId", "1", "--force"],
                "dartq/debug/reload",
                json!({"sessionId": 1, "force": true}),
            ),
            (&["restart", "--sessionId", "1"], "dartq/debug/restart", json!({"sessionId": 1})),
            (
                &["events", "--sessionId", "1", "--since", "41", "--limit", "8"],
                "dartq/debug/events",
                json!({"sessionId": 1, "limit": 8, "cursor": 41}),
            ),
            (
                &["events", "--sessionId", "1"],
                "dartq/debug/events",
                json!({"sessionId": 1, "limit": 64}),
            ),
            (
                &[
                    "output", "--sessionId", "1", "--stream", "stderr", "--since", "9",
                    "--maxBytes", "512",
                ],
                "dartq/debug/output",
                json!({"sessionId": 1, "stream": "stderr", "maxBytes": 512, "cursor": 9}),
            ),
            (
                &["output", "--sessionId", "1"],
                "dartq/debug/output",
                json!({"sessionId": 1, "stream": "stdout", "maxBytes": 4096}),
            ),
        ];
        for (argv, method, params) in cases {
            let parsed = parse(argv).unwrap_or_else(|error| panic!("parse {argv:?}: {error}"));
            let request = parsed.request();
            assert_eq!(request.method, method, "{argv:?}");
            assert_eq!(request.params, params, "{argv:?}");
            assert_eq!(request.dir, Path::new("."), "the worktree defaults to the cwd: {argv:?}");
        }
        // An explicit worktree rides every subcommand (spot checks) —
        // including after a launch's trailing-arg positional, the
        // shape every e2e incantation uses.
        for argv in [
            &["launch", "bin/main.dart", "--dir", "/w", "--sdk", "/s"][..],
            &["status", "--dir", "/w"][..],
            &["output", "--sessionId", "1", "--dir", "/w"][..],
        ] {
            let parsed = parse(argv).expect("parse");
            let request = parsed.request();
            assert_eq!(request.dir, Path::new("/w"), "{argv:?}");
            assert_eq!(request.sdk, argv[0].eq("launch").then_some(Path::new("/s")), "{argv:?}");
        }
        let launch = parse(&["launch", "bin/main.dart", "--dir", "/w"]).expect("parse");
        match &launch.command {
            DebugCommand::Launch(args) => {
                assert!(args.args.is_empty(), "flags after the script stay flags: {:?}", args.args);
            }
            _ => panic!("expected launch"),
        }
    }

    /// The drift gate: one parse per subcommand emits exactly the daemon
    /// family it serves — no method invented, none missing, no typo.
    #[test]
    fn the_emitted_family_is_exactly_the_daemons_debug_requests() {
        let argvs: [&[&str]; 17] = [
            &["launch", "bin/main.dart"],
            &["attach", "--wsUri", "ws://x"],
            &["detach", "--sessionId", "1"],
            &["stop", "--sessionId", "1"],
            &["status"],
            &["snapshot", "--sessionId", "1", "--isolate", "isolates/1"],
            &["evaluate", "--sessionId", "1", "1 + 1"],
            &["step", "--sessionId", "1", "into"],
            &["pause", "--sessionId", "1"],
            &["resume", "--sessionId", "1"],
            &["breakpoint", "add", "--sessionId", "1", "--script", "s.dart", "--line", "1"],
            &["breakpoint", "rm", "--sessionId", "1", "--breakpointId", "b/1"],
            &["exception-pause", "--sessionId", "1", "all"],
            &["reload", "--sessionId", "1"],
            &["restart", "--sessionId", "1"],
            &["events", "--sessionId", "1"],
            &["output", "--sessionId", "1"],
        ];
        let mut emitted: Vec<&str> =
            argvs.iter().map(|argv| parse(argv).expect("parse").request().method).collect();
        emitted.sort_unstable();
        let mut served = dartq_daemon::DEBUG_REQUESTS.to_vec();
        served.sort_unstable();
        assert_eq!(emitted, served, "the CLI family mirrors the daemon family 1:1");
    }

    /// The grammar's refusal set: required selectors, the mutually
    /// exclusive breakpoint shapes, and the enum-spelled words.
    #[test]
    fn malformed_requests_are_usage_errors() {
        for argv in [
            &["launch"][..],
            &["attach"][..],
            &["detach"][..],
            &["snapshot", "--sessionId", "1"][..],
            &["evaluate", "--sessionId", "1"][..],
            &["step", "--sessionId", "1"][..],
            &["step", "--sessionId", "1", "sideways"][..],
            &["exception-pause", "--sessionId", "1", "sometimes"][..],
            &["output", "--sessionId", "1", "--stream", "stdin"][..],
            &["breakpoint", "add", "--sessionId", "1", "--script", "s.dart"][..],
            &["breakpoint", "add", "--sessionId", "1", "--line", "5"][..],
            &["breakpoint", "add", "--sessionId", "1", "--column", "3"][..],
            &["breakpoint", "add", "--sessionId", "1", "--functionId", "f/1", "--line", "5"][..],
            &["breakpoint", "rm", "--sessionId", "1"][..],
            &["bogus"][..],
        ] {
            assert!(parse(argv).is_err(), "`{argv:?}` must be the usage tier");
        }
    }

    /// The output contract's tier mapping: answers ride stdout at 0, a
    /// daemon domain error is data at 1, a transport failure is internal
    /// at 4.
    #[test]
    fn answer_tier_maps_outcomes_to_exit_codes() {
        assert_eq!(answer_tier(Ok(json!({"ok": true}))), exit::CLEAN);
        assert_eq!(
            answer_tier(Err(ProxyError::Daemon(
                -32001,
                "DARTQ/DEBUG/NO_SUCH_SESSION: session 1 is not live".into()
            ))),
            exit::DEBUG_DOMAIN
        );
        assert_eq!(
            answer_tier(Err(ProxyError::Transport("no resident session".into()))),
            exit::INTERNAL_ERROR
        );
    }

    /// The full path against a really-served session stays at the
    /// process level (the e2e suite's debug leg drives `session start`
    /// and the real detached server): the in-process serve loop's 1 s
    /// idle reaper races a second `ensure` under parallel test load —
    /// and a take-over would spawn a stray detached server mid-test.
    /// Here the tiers beyond the daemon answer (the usage tier) stay
    /// unit-covered through the same `run_entry` path.
    #[test]
    fn run_entry_answers_the_usage_tier_for_an_unresolvable_worktree() {
        let gone = std::env::temp_dir().join(format!("dqs-debug-gone-{}", std::process::id()));
        let dir = gone.display().to_string();
        assert_eq!(
            run_entry(&parse(&["status", "--dir", &dir]).expect("parse")),
            exit::USAGE
        );
    }
}

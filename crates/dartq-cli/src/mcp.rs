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

use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use clap::Args;
use serde_json::{Value, json};

use dartq_diagnostics::FileLayoutProvider;

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

/// The one tool family the server serves.
struct Tool {
    name: &'static str,
    description: &'static str,
    kind: ToolKind,
}

enum ToolKind {
    /// A daemon request over the session socket; the tool's arguments
    /// are the request's parameters.
    Daemon(&'static str),
    /// The rename face over a fresh in-process engine (a rare operation;
    /// a cold pass per call).
    Rename,
    /// The session's event log / progress snapshot, read locally.
    Events,
}

const TOOLS: &[Tool] = &[
    Tool {
        name: "dartq_analyze",
        description: "Full-engine diagnostics for the workspace (scan, parse, resolution, \
semantic/flow/lint). Use for: 'what is wrong with this code', pre-commit checks, whole-file \
review context. Not for: symbol lookups (dartq_query), why a diagnostic fires (dartq_explain), \
or blast-radius questions (dartq_impact). Arguments: the daemon analyze parameters \
(verbosity, context, agent).",
        kind: ToolKind::Daemon("dartq/analyze"),
    },
    Tool {
        name: "dartq_query",
        description: "Symbol-index lookups: declarations and resolved references by name. Use \
for: 'where is X declared/used', call sites, implementations, dependencies. Not for: \
diagnostics (dartq_analyze) or one-cursor symbol resolution (no CLI mirror — the daemon \
resolveAt). Arguments: {name, kind (symbol|references|callers|implementations|dependencies), \
verbosity}.",
        kind: ToolKind::Daemon("dartq/query"),
    },
    Tool {
        name: "dartq_explain",
        description: "The one-stop report for one symbol: declaration, shape, references, \
related diagnostics. Use for: 'tell me everything about this API'. Not for: raw reference \
rows (dartq_query is cheaper) or diagnostics alone (dartq_analyze).",
        kind: ToolKind::Daemon("dartq/explain"),
    },
    Tool {
        name: "dartq_impact",
        description: "What a change to one file would affect: dependent files, signatures, \
tests. Use before: edits whose blast radius is unclear. Not for: current diagnostics \
(dartq_analyze) — this answers the counterfactual.",
        kind: ToolKind::Daemon("dartq/impact"),
    },
    Tool {
        name: "dartq_fix",
        description: "Anchored fix proposals/patches for the analyzed workspace. Use for: \
applying lint/quick fixes. Not for: rename edits (dartq_rename) or plugin assists.",
        kind: ToolKind::Daemon("dartq/fix"),
    },
    Tool {
        name: "dartq_rename",
        description: "Rename one symbol across the workspace; returns the machine edit set \
(dry-run semantics — the caller applies edits). Use for: identifier renames. Not for: \
fix-class rewrites (dartq_fix). Arguments: {target: 'PATH:LINE:COL', newName}. Runs a cold \
analysis pass — expect it slower than the session-resident tools.",
        kind: ToolKind::Rename,
    },
    Tool {
        name: "dartq_test",
        description: "Run `dart test` through the session (cached, impacted-selectable). Use \
for: verifying a change. Not for: reading results of a run already made \
(dartq_test_events) or diagnostics (dartq_analyze).",
        kind: ToolKind::Daemon("dartq/test/run"),
    },
    Tool {
        name: "dartq_test_status",
        description: "The session's test runs: in-flight and finished. Use after dartq_test \
to poll. Not for: starting a run.",
        kind: ToolKind::Daemon("dartq/test/status"),
    },
    Tool {
        name: "dartq_test_events",
        description: "Cursor-paged events of one test run. Use for: streaming a run's \
results. Not for: live session/analysis progress (dartq_events).",
        kind: ToolKind::Daemon("dartq/test/events"),
    },
    Tool {
        name: "dartq_test_cancel",
        description: "Cancel one in-flight test run. Use for: aborting a dartq_test that \noutlived its value. Not for: reading results (dartq_test_events).",
        kind: ToolKind::Daemon("dartq/test/cancel"),
    },
    Tool {
        name: "dartq_events",
        description: "The session's unified event stream or its cold-analysis progress \
(files scanned, current package). Use for: 'is it still working' during a long first \
analysis, and lifecycle reads. Not for: test-run result events \
(dartq_test_events). Arguments: {handle: 'session'|'progress', since, limit}.",
        kind: ToolKind::Events,
    },
    Tool {
        name: "dartq_debug_launch",
        description: "Launch a debug session for a Dart script or a Flutter app (Flutter is \
the primary target; kind: dart|flutter, plus device/flavor/dartDefines for flutter). Use for: \
interactive debugging — launch pauses at start, so arm breakpoints via \
dartq_debug_breakpoint_add, then dartq_debug_resume (the standard takeover loop). Not for: \
analyzing code or running tests.",
        kind: ToolKind::Daemon("dartq/debug/launch"),
    },
    Tool {
        name: "dartq_debug_attach",
        description: "Attach a debug session to a running VM service ws URI — including one \
owned by a live `flutter run` (dartq speaks only the service protocol; the tool's own \
console keeps working). Use for: debugging an already-started program. Not for: starting \
fresh (dartq_debug_launch).",
        kind: ToolKind::Daemon("dartq/debug/attach"),
    },
    Tool {
        name: "dartq_debug_detach",
        description: "Detach one debug session; the program keeps running and the answer \
carries the ws URI for re-attaching later. Use for: ending debugging without stopping the \
app. Not for: terminating it (dartq_debug_stop).",
        kind: ToolKind::Daemon("dartq/debug/detach"),
    },
    Tool {
        name: "dartq_debug_stop",
        description: "Stop one debug session and its launched program (a Flutter app is shut \
down through the tool's own cleanup ladder — no orphaned app). Use for: ending a launched \
session for good. Not for: keeping it running (dartq_debug_detach).",
        kind: ToolKind::Daemon("dartq/debug/stop"),
    },
    Tool {
        name: "dartq_debug_status",
        description: "The session's debug sessions and their stopped states. Use for: \norientation after a stop event. Not for: frame detail (dartq_debug_snapshot).",
        kind: ToolKind::Daemon("dartq/debug/status"),
    },
    Tool {
        name: "dartq_debug_breakpoint_add",
        description: "Add a line breakpoint to one debug session. Use for: stopping at a \nline. Not for: exception behavior (dartq_debug_exception_pause).",
        kind: ToolKind::Daemon("dartq/debug/breakpointAdd"),
    },
    Tool {
        name: "dartq_debug_breakpoint_remove",
        description: "Remove one breakpoint. Use for: clearing a stop you set. Not for: \nresuming (dartq_debug_resume).",
        kind: ToolKind::Daemon("dartq/debug/breakpointRemove"),
    },
    Tool {
        name: "dartq_debug_exception_pause",
        description: "Set a session's exception pause mode (never/uncaught/all). Use for: \nbreaking on throws. Not for: line stops (dartq_debug_breakpoint_add).",
        kind: ToolKind::Daemon("dartq/debug/exceptionPause"),
    },
    Tool {
        name: "dartq_debug_resume",
        description: "Resume a paused session. Use for: continuing after any stop. Not for: \nstepping (dartq_debug_step).",
        kind: ToolKind::Daemon("dartq/debug/resume"),
    },
    Tool {
        name: "dartq_debug_pause",
        description: "Pause a running session. Use for: interrupting to inspect state. \nNot for: stepping (that rides dartq_debug_step after the pause).",
        kind: ToolKind::Daemon("dartq/debug/pause"),
    },
    Tool {
        name: "dartq_debug_step",
        description: "Step (into/over/out) a paused session. Use for: advancing by one \nframe's worth. Not for: free-running (dartq_debug_resume).",
        kind: ToolKind::Daemon("dartq/debug/step"),
    },
    Tool {
        name: "dartq_debug_snapshot",
        description: "A stopped frame snapshot: variables, scopes. Use for: inspecting a \npaused program. Not for: its output stream (dartq_debug_output).",
        kind: ToolKind::Daemon("dartq/debug/snapshot"),
    },
    Tool {
        name: "dartq_debug_reload",
        description: "Hot reload (reloadSources) — on a Flutter session this is the tool's \
own orchestrated reload (source push + reassemble), so edits on disk take effect. A broken \
edit answers the compile error as data; the session stays usable. Use for: the edit-reload \
half of the loop. Not for: state resets (dartq_debug_restart).",
        kind: ToolKind::Daemon("dartq/debug/reload"),
    },
    Tool {
        name: "dartq_debug_restart",
        description: "Hot restart (Flutter): state resets like the tool's `R`, isolates are \
rebuilt, and the session re-arms its breakpoints on the new isolate (the answer lists them). \
Use for: a clean slate after edits too large for reload. Not for: keeping state \
(dartq_debug_reload).",
        kind: ToolKind::Daemon("dartq/debug/restart"),
    },
    Tool {
        name: "dartq_debug_evaluate",
        description: "Evaluate one Dart expression in a paused frame (frameIndex) or the \
isolate's root library (no frameIndex). Use for: inspecting state the snapshot only names, \
verifying a fix from inside the app. Not for: the one-call stopped bundle \
(dartq_debug_snapshot).",
        kind: ToolKind::Daemon("dartq/debug/evaluate"),
    },
    Tool {
        name: "dartq_debug_events",
        description: "Cursor-paged debugger events of one session. Use for: replaying \nwhat the debugger did. Not for: program stdout (dartq_debug_output).",
        kind: ToolKind::Daemon("dartq/debug/events"),
    },
    Tool {
        name: "dartq_debug_output",
        description: "Cursor-paged program output of one session. Use for: reading what \nthe program printed. Not for: debugger lifecycle (dartq_debug_events).",
        kind: ToolKind::Daemon("dartq/debug/output"),
    },
];

/// The workspace-status resource's URI.
const STATUS_URI: &str = "dartq://workspace-status";

/// One server's bound context: the worktree, its layout, and whether the
/// resident session is attached (the degradation reason when not).
struct Mcp {
    root: PathBuf,
    layout: Layout,
    session: Result<(), String>,
}

pub fn run_entry(args: &McpArgs) -> u8 {
    let root = match std::fs::canonicalize(&args.dir) {
        Ok(root) => root,
        Err(error) => {
            eprintln!("error: cannot resolve {}: {error}", args.dir.display());
            return exit::USAGE;
        }
    };
    let sdk = crate::session::resolved_sdk_root(args.sdk.as_deref());
    let session = match sdk {
        Ok(sdk_root) => match dartq_session::ensure(&root, &sdk_root, &crate::session::spawn_server) {
            dartq_session::Ensured::Attached(_) => Ok(()),
            dartq_session::Ensured::Standalone(reason) => Err(reason),
        },
        Err(error) => Err(error),
    };
    let mcp = Mcp { layout: Layout::new(&root), root, session };
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

/// The MCP tool shape: the permissive object schema (the daemon owns the
/// parameter contracts; each description documents them).
fn tool_shape(tool: &Tool) -> Value {
    json!({
        "name": tool.name,
        "description": tool.description,
        "inputSchema": {"type": "object", "additionalProperties": true, "properties": {}},
    })
}

fn call_tool(params: &Value, mcp: &Mcp, output: &mut impl Write) -> Result<Value, Value> {
    let name = params["name"].as_str().unwrap_or_default();
    let Some(tool) = TOOLS.iter().find(|tool| tool.name == name) else {
        return Err(json!({"code": -32602, "message": format!("unknown tool: {name}")}));
    };
    let arguments = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
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
    let outcome = match tool.kind {
        ToolKind::Daemon(method) => daemon_call(method, &arguments, mcp),
        ToolKind::Rename => rename_call(&arguments, mcp),
        ToolKind::Events => events_call(&arguments, mcp),
    };
    notify(output, &token, 1);
    Ok(match outcome {
        Ok(value) => json!({
            "content": [{"type": "text", "text": value.to_string()}],
            "isError": false,
        }),
        Err(reason) => json!({
            "content": [{"type": "text", "text": reason}],
            "isError": true,
        }),
    })
}

/// One daemon request over the session socket.
fn daemon_call(method: &str, params: &Value, mcp: &Mcp) -> Result<Value, String> {
    if let Err(reason) = &mcp.session {
        return Err(format!(
            "no resident session ({reason}) — the engine tools need one; \
             start it with `dartq session start` in a writable checkout"
        ));
    }
    let mut stream = UnixStream::connect(&mcp.layout.socket).map_err(|error| error.to_string())?;
    let body = serde_json::to_string(&json!({
        "jsonrpc": "2.0", "id": 1, "method": method, "params": params,
    }))
    .map_err(|error| error.to_string())?;
    write!(stream, "Content-Length: {}\r\n\r\n{body}", body.len()).map_err(|e| e.to_string())?;
    stream.flush().map_err(|error| error.to_string())?;
    let answer = read_frame(&mut stream)?;
    if let Some(error) = answer.get("error") {
        return Err(error.to_string());
    }
    Ok(answer.get("result").cloned().unwrap_or_else(|| json!({})))
}

/// Reads one Content-Length framed JSON message.
fn read_frame(stream: &mut UnixStream) -> Result<Value, String> {
    let mut header = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        stream.read_exact(&mut byte).map_err(|error| error.to_string())?;
        header.push(byte[0]);
        if header.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let header = String::from_utf8_lossy(&header);
    let length: usize = header
        .lines()
        .find_map(|line| line.split_once(':').and_then(|(name, value)| {
            (name.trim().eq_ignore_ascii_case("content-length"))
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        }))
        .ok_or_else(|| "missing Content-Length".to_owned())?;
    let mut body = vec![0u8; length];
    stream.read_exact(&mut body).map_err(|error| error.to_string())?;
    serde_json::from_slice(&body).map_err(|error| error.to_string())
}

/// The rename face over a cold in-process engine (the rare tool).
fn rename_call(params: &Value, _mcp: &Mcp) -> Result<Value, String> {
    let target = params["target"].as_str().ok_or("arguments: {target: 'PATH:LINE:COL', newName}")?;
    let new_name = params["newName"].as_str().ok_or("arguments: {target: 'PATH:LINE:COL', newName}")?;
    let (path, line, column) = parse_target(target)?;
    let (mut engine, _dir) = dartq_commands::workspace::open_engine(
        path.parent().unwrap_or(Path::new(".")),
        std::env::var_os("DART_SDK").map(PathBuf::from).as_deref(),
    )
    .map_err(|error| error.to_string())?;
    let run = dartq_commands::rename::rename(
        &mut engine,
        &path,
        dartq_commands::rename::Anchor::LineCol { line, column },
        new_name,
    )
    .map_err(|error| error.to_string())?;
    let layout = &run.layout;
    let edits: Vec<Value> = run
        .edits
        .iter()
        .map(|edit| {
            json!({
                "file": layout.file_path(edit.file),
                "range": {"start": edit.range.offset, "length": edit.range.length},
            })
        })
        .collect();
    Ok(json!({"newName": new_name, "edits": edits}))
}

fn parse_target(spec: &str) -> Result<(PathBuf, u32, u32), String> {
    let mut parts = spec.rsplitn(3, ':');
    let column = parts.next().and_then(|c| c.parse().ok()).ok_or("target PATH:LINE:COL")?;
    let line = parts.next().and_then(|l| l.parse().ok()).ok_or("target PATH:LINE:COL")?;
    let path = parts.next().filter(|p| !p.is_empty()).ok_or("target PATH:LINE:COL")?;
    Ok((PathBuf::from(path), line, column))
}

/// The events read, local to the worktree.
fn events_call(params: &Value, mcp: &Mcp) -> Result<Value, String> {
    match params["handle"].as_str().unwrap_or("session") {
        "session" => {
            let log = EventLog::new(&mcp.layout);
            let since = params["since"].as_u64().unwrap_or_default();
            let limit = params["limit"].as_u64().unwrap_or(100).min(1000) as usize;
            Ok(json!({"events": log.read(None, since, limit)
                .into_iter()
                .map(|(_, value)| value)
                .collect::<Vec<_>>()}))
        }
        "progress" => {
            let progress = Progress::from_trace(&mcp.layout.dir.join("trace.jsonl"));
            Ok(json!({
                "files_scanned": progress.files_scanned,
                "from_cache": progress.from_cache,
                "current": progress.current,
                "current_package": progress.current_package,
            }))
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
        let root = fixture();
        let serve_dir = root.clone();
        let serve_sdk = root.join("sdk");
        let server = std::thread::spawn(move || {
            crate::session::serve_loop_for_tests(&serve_dir, &serve_sdk)
        });
        let layout = Layout::new(&root);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !dartq_session::socket_open(&layout) {
            assert!(std::time::Instant::now() < deadline, "the serve loop never listened");
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let mcp = Mcp { root: root.clone(), layout: layout.clone(), session: Ok(()) };
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
        let tools = answer(2)["result"]["tools"].as_array().cloned().expect("tools");
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
            analyze["content"][0]["text"].as_str().unwrap_or_default().contains("diagnostics"),
            "the daemon answer rides the MCP content"
        );
        let progress = answer(4)["result"]["content"][0]["text"].to_string();
        assert!(progress.contains("files_scanned"), "{progress}");
        let status = answer(5)["result"]["contents"][0]["text"].to_string();
        assert!(status.contains("fx"), "the workspace status names the package: {status}");
        assert!(status.contains("packages"), "{status}");
        assert!(answer(6)["error"]["code"].as_i64() == Some(-32602));
        assert!(answer(7)["result"].as_object().is_some(), "ping answers");

        // A degraded tree (no session): the engine tools answer isError
        // with the reason; the local faces keep working.
        let degraded = Mcp { root: root.clone(), layout, session: Err("read-only checkout".into()) };
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
        assert!(answers[0]["result"]["content"][0]["text"].as_str().unwrap_or_default().contains("no resident session"));
        assert!(answers[1]["result"]["isError"].as_bool() == Some(false), "local faces survive");

        // Drain the server thread.
        std::thread::sleep(std::time::Duration::from_secs(2));
        let _ = server.join();
        std::fs::remove_dir_all(&root).ok();
    }

    /// The tool table behind `tools/list`: names are unique, every daemon
    /// tool maps to a `dartq/*` method, and the served shape carries the
    /// permissive object schema (the daemon owns the parameter contracts).
    #[test]
    fn tool_table_is_well_formed() {
        let mut names: Vec<&str> = TOOLS.iter().map(|tool| tool.name).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(names.len(), before, "tool names are unique");
        assert!(TOOLS.len() >= 20, "{} tools", TOOLS.len());
        for tool in TOOLS {
            if let ToolKind::Daemon(method) = tool.kind {
                assert!(method.starts_with("dartq/"), "{} maps to a daemon method", tool.name);
            }
            let shape = tool_shape(tool);
            assert_eq!(shape["name"], tool.name);
            assert_eq!(shape["description"], tool.description);
            assert_eq!(shape["inputSchema"]["type"], "object");
            assert_eq!(shape["inputSchema"]["additionalProperties"], true);
        }
    }

    /// The wire's noise discipline: a line that is not JSON and a
    /// notification (no id) answer nothing; the next real request still
    /// gets its answer, and EOF ends the loop cleanly.
    #[test]
    fn malformed_lines_and_notifications_answer_nothing() {
        let root = std::env::temp_dir().join(format!("dqs-mcp-noise-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("dir");
        let mcp = Mcp { layout: Layout::new(&root), root: root.clone(), session: Err("standalone".into()) };
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
        let mcp = Mcp { layout: Layout::new(&root), root: root.clone(), session: Err("standalone".into()) };
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
            answer(1)["error"]["message"].as_str().unwrap_or_default().contains("workspace/symbol"),
            "the message names the method"
        );
        assert_eq!(answer(2)["error"]["code"], -32602, "an unknown resource is a usage error");
        let resources = answer(3)["result"]["resources"].as_array().cloned().expect("resources");
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
        let mcp = Mcp { layout: Layout::new(&root), root: root.clone(), session: Err("read-only checkout".into()) };
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
        assert_eq!(progress.len(), 2, "exactly the token-carrying call brackets: {lines:?}");
        for (index, value) in progress.iter().enumerate() {
            assert_eq!(value["params"]["progressToken"], "tok-7");
            assert_eq!(value["params"]["progress"], index as u64, "0 then 1, in order");
            assert_eq!(value["params"]["total"], 1);
        }
        // The notifications precede their call's answer.
        let answer_position = lines
            .iter()
            .position(|value| value["id"].as_u64() == Some(1))
            .expect("answer 1");
        assert!(
            lines[..answer_position].iter().any(|value| value["method"].as_str() == Some("notifications/progress")),
            "the bracket opens before the answer rides the stream"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// One daemon frame over a socketpair: headers parse case-insensitively,
    /// the body must be JSON, and a frame without Content-Length is the
    /// usage error string.
    #[test]
    fn read_frame_parses_headers_and_rejects_missing_length() {
        let (mut mine, mut theirs) = UnixStream::pair().expect("socketpair");
        let body = r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#;
        write!(mine, "Content-Type: application/json\r\ncontent-length: {}\r\n\r\n{}", body.len(), body)
            .expect("write frame");
        mine.flush().expect("flush");
        let answer = read_frame(&mut theirs).expect("frame");
        assert_eq!(answer["result"]["ok"], true);
        write!(mine, "X-Other: 1\r\n\r\nno length").expect("write headerless");
        mine.flush().expect("flush");
        assert_eq!(read_frame(&mut theirs), Err("missing Content-Length".to_owned()));
    }

    /// The rename target grammar splits from the right, so a Windows-style
    /// drive path keeps its colon; every other shape is the usage error.
    #[test]
    fn parse_target_splits_from_the_right() {
        let ok = |spec: &str| parse_target(spec).expect("target");
        assert_eq!(ok("lib/a.dart:3:7"), (PathBuf::from("lib/a.dart"), 3, 7));
        assert_eq!(ok("/w/a.dart:1:2"), (PathBuf::from("/w/a.dart"), 1, 2));
        assert_eq!(ok("C:\\a.dart:1:2"), (PathBuf::from("C:\\a.dart"), 1, 2));
        for bad in ["a.dart", "a.dart:1", "a.dart:x:2", ":1:2", ""] {
            assert!(parse_target(bad).is_err(), "`{bad}` must be refused");
        }
    }

    /// The events tool is a local face: without a session it still reads
    /// the event log (handle filtering, since/limit) and the progress
    /// snapshot, and an unknown handle is the usage error.
    #[test]
    fn events_tool_reads_local_faces_without_a_session() {
        let root = std::env::temp_dir().join(format!("dqs-mcp-events-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("dir");
        let layout = Layout::new(&root);
        let log = EventLog::new(&layout);
        log.append("session", "serving", json!({})).expect("append");
        log.append("session", "client_attached", json!({})).expect("append");
        let mcp = Mcp { layout, root: root.clone(), session: Err("read-only checkout".into()) };

        let events = events_call(&json!({}), &mcp).expect("default handle is session");
        assert_eq!(events["events"].as_array().expect("events").len(), 2);
        let tail = events_call(&json!({"since": 1, "limit": 10}), &mcp).expect("cursor read");
        assert_eq!(tail["events"].as_array().expect("events").len(), 1);
        let limited = events_call(&json!({"limit": 1}), &mcp).expect("limit read");
        assert_eq!(limited["events"].as_array().expect("events").len(), 1);

        let progress = events_call(&json!({"handle": "progress"}), &mcp).expect("progress");
        assert_eq!(progress["files_scanned"], 0, "no trace in this tree");

        let error = events_call(&json!({"handle": "bogus"}), &mcp).expect_err("unknown handle");
        assert!(error.contains("unknown handle"), "{error}");
        std::fs::remove_dir_all(&root).ok();
    }
}

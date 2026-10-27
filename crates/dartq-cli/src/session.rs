//! The `dartq session` shell: the resident session layer's management
//! face — `start` (elect or attach), `status` (the per-user registry,
//! grouped by checkout), `stop`, and the hidden `serve` the detached
//! server process runs. The identity, election, registry, and lifecycle
//! rules live in the `dartq-session` crate; the engine serving is the
//! daemon protocol (`dartq-daemon::run`) over the session socket, one
//! client at a time.

use std::io;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use clap::{Args, Subcommand};
use serde_json::{Value, json};

use dartq_session::{
    Ensured, EventLog, HEARTBEAT, IDLE_TIMEOUT, Layout, Record, Registry, SessionState,
    SessionTriple, ensure, pid_alive, terminate,
};

use crate::OutputFormat;
use crate::exit;
use crate::output::lon_line;

/// `dartq session` — manage the resident analysis session.
#[derive(Args)]
pub struct SessionArgs {
    #[command(subcommand)]
    command: SessionCommand,
}

#[derive(Subcommand)]
enum SessionCommand {
    /// Bring up (or find) the resident session for a worktree and print
    /// its identity. An unwritable or foreign tree degrades to
    /// standalone — printed, never an error: the caller proceeds
    /// in-process.
    Start(StartArgs),
    /// List the user's live sessions, grouped by checkout.
    Status(StatusArgs),
    /// Stop the session of a worktree (or every one with `--all`). A
    /// write operation: `--format text` falls back to the lon record.
    Stop(StopArgs),
    /// The resident serve loop — the detached process `start` spawns.
    /// Hidden: not a human verb.
    #[command(hide = true)]
    Serve(ServeArgs),
}

#[derive(Args)]
struct StartArgs {
    /// The worktree; defaults to the current directory.
    #[arg(default_value = ".")]
    dir: PathBuf,
    /// Dart SDK root; defaults to `$DART_SDK`.
    #[arg(long)]
    sdk: Option<PathBuf>,
    /// Output format: `lon` (the default), `json`, or `text` (the
    /// identity lines).
    #[arg(long, value_enum, default_value_t = OutputFormat::Lon)]
    format: OutputFormat,
}

#[derive(Args)]
struct StatusArgs {
    /// Output format: `lon` (the default), `json`, or `text` (the
    /// aligned listing).
    #[arg(long, value_enum, default_value_t = OutputFormat::Lon)]
    format: OutputFormat,
}

#[derive(Args)]
struct StopArgs {
    /// The worktree; defaults to the current directory.
    #[arg(default_value = ".")]
    dir: PathBuf,
    /// Stop every live session in the registry.
    #[arg(long)]
    all: bool,
    /// Output format: a write operation — `text` falls back to the lon
    /// record; `lon` the default, `json` the frozen row.
    #[arg(long, value_enum, default_value_t = OutputFormat::Lon)]
    format: OutputFormat,
}

#[derive(Args)]
struct ServeArgs {
    /// The worktree the session serves (the resolution root).
    #[arg(long)]
    dir: PathBuf,
    /// The resolved SDK root (part of the session triple).
    #[arg(long)]
    sdk: PathBuf,
}

pub fn run_entry(args: &SessionArgs) -> u8 {
    match &args.command {
        SessionCommand::Start(start) => start_session(start),
        SessionCommand::Status(status) => session_status(status),
        SessionCommand::Stop(stop) => stop_session(stop),
        SessionCommand::Serve(serve) => serve_loop(serve, IDLE_TIMEOUT),
    }
}

/// The SDK root a session keys on: the flag, else `$DART_SDK` — the
/// engine assembly's own resolution order, surfaced here because the
/// triple needs the concrete root.
pub(crate) fn resolved_sdk_root(sdk: Option<&Path>) -> Result<PathBuf, String> {
    sdk.map(Path::to_path_buf).or_else(|| std::env::var_os("DART_SDK").map(PathBuf::from)).ok_or_else(
        || "no Dart SDK root (pass --sdk PATH or set DART_SDK)".to_owned(),
    )
}

/// The detached serve spawn: the current binary, `session serve`, quiet
/// stdio — the child outlives the starter by design. Its own process
/// group detaches it from the starter's fate (#2587): a starter — or
/// the whole shell task it ran in — being killed by group must never
/// reap the resident engine (the same posture as dartq-debug's flutter
/// children).
pub(crate) fn spawn_server(layout: &Layout, triple: &SessionTriple) -> io::Result<()> {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("dartq"));
    let mut command = Command::new(exe);
    command
        .arg("session")
        .arg("serve")
        .arg("--dir")
        .arg(&triple.root)
        .arg("--sdk")
        .arg(&triple.sdk_root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    command
        .spawn()
        .map(|child| {
            // Reaped by init when the serve loop exits; the starter does
            // not wait.
            drop(child);
            let _ = layout;
        })
}

/// One row through the format axis. The rows are already vocabulary
/// spellings; `text` renders the read faces' identity lines (the write
/// faces pass `None` and their rows ride the lon spelling — a write's
/// output is a record, not prose).
fn print_row(format: OutputFormat, row: &Value, text: Option<String>) {
    match format {
        OutputFormat::Lon => println!("{}", lon_line(row)),
        OutputFormat::Json => println!("{row}"),
        OutputFormat::Text => match text {
            Some(line) => println!("{line}"),
            None => println!("{}", lon_line(row)),
        },
    }
}

/// The text identity line of a `session` row.
fn session_text(row: &Value) -> String {
    format!(
        "session pid {} root {} (age {}s{})",
        row["pid"],
        row["root"].as_str().unwrap_or_default(),
        row.get("age_s").and_then(Value::as_u64).unwrap_or(0),
        if row.get("stale_binary").and_then(Value::as_bool).unwrap_or(false) {
            ", stale binary"
        } else {
            ""
        },
    )
}

fn start_session(args: &StartArgs) -> u8 {
    let root = match std::fs::canonicalize(&args.dir) {
        Ok(root) => root,
        Err(error) => {
            eprintln!("error: cannot resolve {}: {error}", args.dir.display());
            return exit::USAGE;
        }
    };
    let sdk_root = match resolved_sdk_root(args.sdk.as_deref()) {
        Ok(sdk) => sdk,
        Err(message) => {
            eprintln!("error: {message}");
            return exit::USAGE;
        }
    };
    // Lifecycle upkeep rides the start: upgrade orphans die, the cap
    // holds, then the election runs.
    let registry = Registry::open();
    let triple_probe = SessionTriple::compute(&root, &sdk_root);
    for swept in registry.sweep_upgrades(&triple_probe.binary_hash) {
        let row = json!({"type": "swept", "root": swept.display().to_string()});
        print_row(
            args.format,
            &row,
            Some(format!("swept stale session at {}", swept.display())),
        );
    }
    for evicted in registry.evict_past_cap() {
        let row = json!({"type": "evicted", "root": evicted.display().to_string()});
        print_row(
            args.format,
            &row,
            Some(format!("evicted capped session at {}", evicted.display())),
        );
    }
    match ensure(&root, &sdk_root, &spawn_server) {
        Ensured::Attached(state) => {
            print_state(args.format, &state, true);
            exit::CLEAN
        }
        Ensured::Standalone(reason) => {
            let row = json!({"type": "standalone", "reason": reason});
            print_row(args.format, &row, Some(format!("standalone: {reason}")));
            exit::CLEAN
        }
    }
}

fn session_status(args: &StatusArgs) -> u8 {
    let registry = Registry::open();
    let binary = SessionTriple::compute(Path::new("."), Path::new(".")).binary_hash;
    // Grouped by checkout: same-root rows adjacent (the most recent
    // heartbeat first inside a group).
    let mut live = registry.list();
    live.sort_by(|a, b| {
        a.state
            .triple
            .root
            .cmp(&b.state.triple.root)
            .then(b.state.heartbeat_ms.cmp(&a.state.heartbeat_ms))
    });
    // The cwd worktree's cold-analysis progress, when its session has a
    // trace: the anti-hung signal rides status too (`dartq events
    // progress` is the polling face).
    if let Ok(root) = std::fs::canonicalize(".") {
        let layout = Layout::new(&root);
        let progress = dartq_session::Progress::from_trace(&layout.dir.join("trace.jsonl"));
        if progress.files_scanned > 0 {
            let row = json!({
                "type": "progress",
                "root": root.display().to_string(),
                "files_scanned": progress.files_scanned,
                "current_package": progress.current_package,
            });
            let package = progress
                .current_package
                .as_deref()
                .unwrap_or("analyzing");
            print_row(
                args.format,
                &row,
                Some(format!(
                    "progress: {} files scanned ({package})",
                    progress.files_scanned
                )),
            );
        }
    }
    for record in live {
        let age_s = SessionState::now_ms().saturating_sub(record.state.heartbeat_ms) / 1000;
        let row = json!({
            "type": "session",
            "root": record.state.triple.root.display().to_string(),
            "pid": record.state.pid,
            "sdk": record.state.triple.sdk_root.display().to_string(),
            "binary": record.state.triple.binary_hash,
            "age_s": age_s,
            "stale_binary": record.state.triple.binary_hash != binary,
        });
        print_row(args.format, &row, Some(session_text(&row)));
    }
    exit::CLEAN
}

fn stop_session(args: &StopArgs) -> u8 {
    let registry = Registry::open();
    if args.all {
        let mut stopped = 0;
        for record in registry.list() {
            if stop_one(&registry, &record) {
                stopped += 1;
            }
        }
        // A write operation: no prose face — text rides the lon record.
        let row = json!({"type": "stopped", "count": stopped});
        print_row(args.format, &row, None);
        return exit::CLEAN;
    }
    let root = match std::fs::canonicalize(&args.dir) {
        Ok(root) => root,
        Err(error) => {
            eprintln!("error: cannot resolve {}: {error}", args.dir.display());
            return exit::USAGE;
        }
    };
    let layout = Layout::new(&root);
    if let Some(state) = SessionState::read(&layout) {
        terminate(state.pid);
        wait_gone(state.pid);
    }
    registry.remove(&root);
    let _ = std::fs::remove_file(&layout.socket);
    let _ = std::fs::remove_file(&layout.state);
    let row = json!({"type": "stopped", "root": root.display().to_string()});
    print_row(args.format, &row, None);
    exit::CLEAN
}

fn stop_one(registry: &Registry, record: &Record) -> bool {
    terminate(record.state.pid);
    let gone = wait_gone(record.state.pid);
    let layout = Layout::new(&record.state.triple.root);
    let _ = std::fs::remove_file(&layout.socket);
    let _ = std::fs::remove_file(&layout.state);
    registry.remove(&record.state.triple.root);
    gone
}

/// Waits up to 5 s for the pid to leave (reaped or gone); `true` when it
/// did. A zombie answers `kill(pid, 0)` — the parent here is init (the
/// serve process was detached), so termination shows up promptly.
fn wait_gone(pid: u32) -> bool {
    for _ in 0..100 {
        if !pid_alive(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

fn print_state(format: OutputFormat, state: &SessionState, attached: bool) {
    let row = json!({
        "type": "session",
        "attached": attached,
        "root": state.triple.root.display().to_string(),
        "pid": state.pid,
        "sdk": state.triple.sdk_root.display().to_string(),
        "binary": state.triple.binary_hash,
        "socket": Layout::new(&state.triple.root).socket.display().to_string(),
    });
    let text = if attached {
        Some(format!(
            "attached session pid {} at {}",
            state.pid,
            Layout::new(&state.triple.root).socket.display()
        ))
    } else {
        Some("session detached".to_owned())
    };
    print_row(format, &row, text);
}

// ---------------------------------------------------------------------------
// The serve loop
// ---------------------------------------------------------------------------

/// The serve loop's wake granularity: the accept tick, the beater's
/// sleep slices, and the signal poll all ride it. The beater must not
/// sleep a whole [`HEARTBEAT`] blind — the shutdown ladder joins it,
/// and an idle daemon's exit has to land in milliseconds
/// ([`dartq_session::REAP_GRACE`] covers only the debug-detach ladder).
const TICK: Duration = Duration::from_millis(250);

/// The resident loop: bind the session socket, announce the state, and
/// serve daemon-protocol clients one at a time — the engine (the
/// session) persists across clients; each connection is a full
/// `dartq-daemon` serve until the client disconnects. Between
/// connections the loop wakes on a 250 ms tick to heartbeat the state
/// and to reap itself after [`IDLE_TIMEOUT`] with no client and no
/// held debug session (#2619: an attached debug session freezes the
/// idle clock; SIGTERM/SIGINT and every exit path take the
/// graceful-shutdown ladder that detaches — resumes — every held
/// session first; no self-pipe: the tick replaces it — the Windows
/// posture, unix-native). A signal landing while a client is held is
/// polled at the same granularity inside the client loop (#2632), so
/// the ladder runs without waiting for that client to disconnect.
fn serve_loop(args: &ServeArgs, idle_timeout: Duration) -> u8 {
    let root = match std::fs::canonicalize(&args.dir) {
        Ok(root) => root,
        Err(error) => {
            eprintln!("dartq: session serve: cannot resolve {}: {error}", args.dir.display());
            return exit::SOFTWARE;
        }
    };
    let layout = Layout::new(&root);
    if let Err(reason) = layout.create() {
        eprintln!("dartq: session serve: {reason}");
        return exit::SOFTWARE;
    }
    let (config, _dir, advisory) =
        match crate::engine_config_with_advisory(&root, Some(&args.sdk)) {
            Ok(faces) => faces,
            Err(error) => {
                eprintln!("dartq: session serve: {error}");
                cleanup(&layout, &root);
                return exit::USAGE;
            }
        };
    // The engine trace — the cold-analysis progress substrate
    // (`dartq events progress` reads it; the file truncates per serve, so
    // each resident generation's sweep starts at zero).
    let trace = layout.dir.join("trace.jsonl");
    let mut runtime = dartq_daemon::SessionRuntime::default();
    match dartq_engine::event::TraceSink::create(&trace) {
        Ok(sink) => runtime.trace = Some(sink),
        Err(error) => eprintln!("dartq: session serve: trace sink disabled: {error}"),
    }
    let mut session = match dartq_daemon::Session::with_runtime(
        config,
        Vec::new(),
        None,
        advisory,
        runtime,
    ) {
        Ok(session) => session,
        Err(error) => {
            eprintln!("dartq: internal error: session unusable: {error}");
            cleanup(&layout, &root);
            return exit::SOFTWARE;
        }
    };
    let triple = SessionTriple::compute(&root, &args.sdk);
    let events = EventLog::new(&layout);
    let _ = events.append(
        "session",
        "serving",
        serde_json::json!({"pid": std::process::id()}),
    );
    let listener = match bind_takeover(&layout) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("dartq: session serve: binding {}: {error}", layout.socket.display());
            cleanup(&layout, &root);
            return exit::SOFTWARE;
        }
    };
    if listener.set_nonblocking(true).is_err() {
        eprintln!("dartq: session serve: socket not pollable");
        cleanup(&layout, &root);
        return exit::SOFTWARE;
    }
    let registry = Registry::open();
    // #2632: the heartbeat advertises the held debug-session count (the
    // reaper's SIGTERM grace scales by it); the counter's shareable
    // handle feeds the beat thread, which cannot borrow the session.
    let held_sessions = session.debug_sessions_counter();
    heartbeat(&layout, &registry, &triple, held_sessions.load(std::sync::atomic::Ordering::Relaxed));

    // #2591: the heartbeat owns its own thread. Beating between accepts
    // tied liveness to client service — a long client connection (a
    // minutes-long cold analysis inside one drain) stopped the beat for
    // its whole duration (a 233 s heartbeat age observed live), leaving
    // `session status` reporting a stale engine. The beat thread is
    // independent of the accept loop; it exits when the flag drops at
    // reap/exit, before cleanup strips the state.
    let beating = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let beat_layout = layout.clone();
    let beat_triple = triple.clone();
    let beat_registry = Registry::open();
    let beat_flag = std::sync::Arc::clone(&beating);
    let beat_held = std::sync::Arc::clone(&held_sessions);
    let beater = std::thread::Builder::new()
        .name("dartq-heartbeat".into())
        .spawn(move || {
            while beat_flag.load(std::sync::atomic::Ordering::Relaxed) {
                // The beat window in TICK slices, not one HEARTBEAT
                // sleep: the ladder's join must not wait out a full
                // window — exit latency stays at the tick granularity.
                let wake_at = Instant::now() + HEARTBEAT;
                while Instant::now() < wake_at
                    && beat_flag.load(std::sync::atomic::Ordering::Relaxed)
                {
                    std::thread::sleep(TICK);
                }
                if beat_flag.load(std::sync::atomic::Ordering::Relaxed) {
                    heartbeat(
                        &beat_layout,
                        &beat_registry,
                        &beat_triple,
                        beat_held.load(std::sync::atomic::Ordering::Relaxed),
                    );
                }
            }
        })
        .expect("the heartbeat thread spawns");

    let mut last_client = Instant::now();
    // #2619: the exit signals land in a flag this loop's tick polls —
    // SIGTERM (the election reap's grace window, `session stop`) and
    // SIGINT both take the graceful ladder below instead of killing a
    // daemon that holds paused debug sessions. Registration failure
    // degrades to the default disposition (an immediate death, the
    // pre-#2619 behavior), never a failed serve.
    let signalled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let signals_ready = {
        use signal_hook::consts::{SIGINT, SIGTERM};
        signal_hook::flag::register(SIGTERM, std::sync::Arc::clone(&signalled))
            .and_then(|_| signal_hook::flag::register(SIGINT, std::sync::Arc::clone(&signalled)))
            .is_ok()
    };
    if !signals_ready {
        eprintln!("dartq: session serve: signal handling unavailable; exit signals kill in place");
    }
    loop {
        if signalled.load(std::sync::atomic::Ordering::Relaxed) {
            return serve_shutdown(
                &mut session,
                &events,
                &layout,
                &root,
                &beating,
                beater,
                "signal",
                exit::CLEAN,
            );
        }
        match listener.accept() {
            Ok((stream, _)) => {
                last_client = Instant::now();
                let _ = stream.set_nonblocking(false);
                let _ = events.append("session", "client_attached", serde_json::json!({}));
                serve_client(stream, &mut session, &signalled);
                let _ = events.append("session", "client_detached", serde_json::json!({}));
                heartbeat(
                    &layout,
                    &registry,
                    &triple,
                    held_sessions.load(std::sync::atomic::Ordering::Relaxed),
                );
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => {
                eprintln!("dartq: session serve: accept failed: {error}");
                return serve_shutdown(
                    &mut session,
                    &events,
                    &layout,
                    &root,
                    &beating,
                    beater,
                    "accept_failed",
                    exit::SOFTWARE,
                );
            }
        }
        if idle_elapsed(last_client, Instant::now(), session.debug_sessions_len()) > idle_timeout {
            return serve_shutdown(
                &mut session,
                &events,
                &layout,
                &root,
                &beating,
                beater,
                "idle",
                exit::CLEAN,
            );
        }
        std::thread::sleep(TICK);
    }
}

/// The idle clock (#2619): client silence, frozen at zero while any
/// debug session is attached — a held debug session is never idle (a
/// paused app must not outlive its daemon by reaper fiat), and on
/// detach a full fresh idle window runs.
fn idle_elapsed(last_client: Instant, now: Instant, debug_sessions: usize) -> Duration {
    if debug_sessions > 0 {
        Duration::ZERO
    } else {
        now.saturating_duration_since(last_client)
    }
}

/// Every serve-loop exit path takes this ladder (#2619): first detach
/// (resume) every held debug session through the worker — bounded per
/// session by dartq-debug's release budget, inside the election reap's
/// grace — then append the `shutdown` event naming what was released,
/// stop the beat, clean the layout, and exit. A daemon must never die
/// leaving a paused app frozen behind it.
fn serve_shutdown(
    session: &mut dartq_daemon::Session,
    events: &EventLog,
    layout: &Layout,
    root: &Path,
    beating: &std::sync::Arc<std::sync::atomic::AtomicBool>,
    beater: std::thread::JoinHandle<()>,
    reason: &str,
    code: u8,
) -> u8 {
    let released = session.debug_shutdown().unwrap_or(0);
    let _ = events.append(
        "session",
        "shutdown",
        serde_json::json!({"reason": reason, "sessions_released": released}),
    );
    beating.store(false, std::sync::atomic::Ordering::Relaxed);
    let _ = beater.join();
    cleanup(layout, root);
    code
}

/// One client connection: the daemon protocol until the peer closes —
/// or until the exit signal fires under it (#2632: the client loop
/// polls the same flag the accept tick does, at the same 250 ms
/// granularity, so a signal landing while a long-lived client — the
/// MCP stdio proxy — is held finishes the in-flight turn, answers or
/// cleanly closes, and hands the loop back for the shutdown ladder;
/// without this the reaper's SIGKILL would land first and every held
/// debug session died un-resumed).
fn serve_client(
    stream: UnixStream,
    session: &mut dartq_daemon::Session,
    signalled: &std::sync::atomic::AtomicBool,
) {
    let Ok(read) = stream.try_clone() else {
        return;
    };
    // The read half is the clone; the original stream stays the write
    // half. run owns both and returns at the client's disconnect or the
    // exit signal.
    let _ = dartq_daemon::run_with_signal(read, stream, session, None, Some(signalled));
}

/// Binds the socket, unlinking a leftover file once (a crashed server's
/// residue — the election normally cleans it, this is the belt).
fn bind_takeover(layout: &Layout) -> io::Result<UnixListener> {
    UnixListener::bind(&layout.socket).or_else(|first| {
        let _ = std::fs::remove_file(&layout.socket);
        UnixListener::bind(&layout.socket).map_err(|_| first)
    })
}

/// The liveness rewrite: state.json plus the registry record. `held`
/// is the live debug-session count (#2632) — the reaper reading either
/// copy scales its SIGTERM grace by it instead of SIGKILLing a daemon
/// mid-detach.
fn heartbeat(layout: &Layout, registry: &Registry, triple: &SessionTriple, held: usize) {
    let state = SessionState {
        pid: std::process::id(),
        heartbeat_ms: SessionState::now_ms(),
        triple: triple.clone(),
        debug_sessions: held,
    };
    if state.write(layout).is_ok() {
        let _ = registry.upsert(&Record { id: triple.identity(), state });
    }
}

/// The shutdown residue: socket, state, registry record.
fn cleanup(layout: &Layout, root: &Path) {
    let _ = std::fs::remove_file(&layout.socket);
    let _ = std::fs::remove_file(&layout.state);
    let _ = std::fs::remove_file(layout.dir.join("session.sock.point"));
    Registry::open().remove(root);
}

/// The sibling tests' serve entry: the loop with the short idle.
#[cfg(all(test, unix))]
pub(crate) fn serve_loop_for_tests(dir: &Path, sdk: &Path) -> u8 {
    serve_loop(
        &ServeArgs { dir: dir.to_path_buf(), sdk: sdk.to_path_buf() },
        Duration::from_secs(1),
    )
}

#[cfg(all(test, unix))]
pub(crate) mod tests {
    use super::*;
    use std::io::{Read, Write};

    /// Serializes every test that runs a serve loop in-process: #2632's
    /// pin fires a real SIGTERM, which is process-wide — it sets the
    /// flag of every registered serve loop, not just its own, so any
    /// concurrently running serve test dies as collateral (its loop
    /// takes the shutdown ladder and cleanup strips the state/socket
    /// out from under its assertions). Shared with the mcp module's
    /// serve test via `serve_loop_for_tests`.
    pub(crate) static SIGNAL_SERIALIZATION: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// A short-rooted fixture (unix sockets cap at ~104 path bytes): a
    /// package with a stub SDK — the shape the daemon session assembles
    /// against. Shared with the mcp module's tests.
    pub(crate) fn test_fixture(label: &str) -> PathBuf {
        let root = PathBuf::from("/tmp").join(format!("dqs-serve-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let write = |rel: &str, body: &str| {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
            std::fs::write(path, body).expect("write");
        };
        write("pubspec.yaml", "name: fx\nenvironment:\n  sdk: ^3.5.0\n");
        write(
            ".dart_tool/package_config.json",
            r#"{"configVersion":2,"packages":[{"name":"fx","rootUri":"../","packageUri":"lib/","languageVersion":"3.5"}],"generator":"pub"}"#,
        );
        write("sdk/version", "3.13.1\n");
        write(
            "sdk/lib/libraries.json",
            r#"{"vm":{"libraries":{"core":{"uri":"core/core.dart"}}}}"#,
        );
        write(
            "sdk/lib/_internal/allowed_experiments.json",
            r#"{"version":1,"experimentSets":{"sdkExperiments":[]},"sdk":{"default":{"experimentSet":"sdkExperiments"}},"packages":{}}"#,
        );
        write("sdk/lib/core/core.dart", "library dart.core;\n");
        write("lib/a.dart", "int a() => 1;\n");
        root
    }

    /// One framed request over the socket; returns the first response
    /// body.
    fn roundtrip(socket: &Path, request: &str) -> String {
        let mut stream = UnixStream::connect(socket).expect("connect");
        let body = request.as_bytes();
        write!(
            stream,
            "Content-Length: {}\r\n\r\n",
            body.len()
        )
        .expect("write frame");
        stream.write_all(body).expect("write body");
        stream.flush().expect("flush");
        let mut text = String::new();
        let mut header = String::new();
        let mut byte = [0u8; 1];
        // Read until the blank line, then the body.
        loop {
            stream.read_exact(&mut byte).expect("header byte");
            header.push(byte[0] as char);
            if header.ends_with("\r\n\r\n") {
                break;
            }
        }
        let length: usize = header
            .lines()
            .find_map(|line| line.split_once(':').and_then(|(name, value)| {
                (name.trim().eq_ignore_ascii_case("content-length"))
                    .then(|| value.trim().parse().expect("length"))
            }))
            .expect("content-length");
        let mut body = vec![0u8; length];
        stream.read_exact(&mut body).expect("body");
        text.push_str(&String::from_utf8(body).expect("utf-8"));
        text
    }

    /// The events faces after a served client: the session log carries
    /// the lifecycle records cursor-paginated, and the progress handle
    /// reads the engine trace the serve loop armed.
    #[test]
    fn events_faces_read_the_served_session() {
        // In-process serve loop: collateral of the real signal test (see
        // SIGNAL_SERIALIZATION) — a stray SIGTERM would tear the socket
        // and state down mid-roundtrip.
        let _serial = SIGNAL_SERIALIZATION.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let root = test_fixture("events");
        let serve_dir = root.clone();
        let serve_sdk = root.join("sdk");
        let server = std::thread::spawn(move || {
            serve_loop(&ServeArgs { dir: serve_dir, sdk: serve_sdk }, Duration::from_secs(1))
        });
        let layout = Layout::new(&root);
        let deadline = Instant::now() + Duration::from_secs(20);
        while !dartq_session::socket_open(&layout) {
            assert!(Instant::now() < deadline, "the serve loop never listened");
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = roundtrip(
            &layout.socket,
            r#"{"jsonrpc":"2.0","id":1,"method":"dartq/analyze","params":{}}"#,
        );
        // The cold-analysis progress: the sweep analyzed at least the
        // fixture's one library, and names its package.
        let progress = dartq_session::Progress::from_trace(&layout.dir.join("trace.jsonl"));
        assert!(progress.files_scanned >= 1, "the trace captured the sweep: {progress:?}");
        // The package signal: the fixture's root dir carries `lib/`.
        let package_root = root.file_name().and_then(|n| n.to_str()).expect("root name");
        assert_eq!(progress.current_package.as_deref(), Some(package_root));

        // The event log: serving → client_attached → client_detached,
        // cursor-paginated.
        let log = dartq_session::EventLog::new(&layout);
        let first = log.read(Some("session"), 0, 1);
        assert_eq!(first.len(), 1);
        let seq = first[0].0;
        let rest = log.read(Some("session"), seq, 10);
        let names: Vec<&str> = rest
            .iter()
            .map(|(_, value)| value["event"].as_str().unwrap_or_default())
            .collect();
        assert!(names.contains(&"client_attached"), "{names:?}");
        assert!(names.contains(&"client_detached"), "{names:?}");

        let code = server.join().expect("server thread");
        assert_eq!(code, exit::CLEAN);
        std::fs::remove_dir_all(&root).ok();
    }

    /// The resident loop end to end: a client speaks the daemon protocol
    /// over the session socket, the engine persists, and the idle reaper
    /// exits cleanly once the last client is gone.
    #[test]
    fn serve_loop_serves_a_client_then_reaps_itself() {
        // This pin asserts the shutdown's reason; a stray signal from the
        // parallel signal test would rewrite it (see SIGNAL_SERIALIZATION).
        let _serial = SIGNAL_SERIALIZATION.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let root = test_fixture("loop");
        let serve_dir = root.clone();
        let serve_sdk = root.join("sdk");
        let server = std::thread::spawn(move || {
            serve_loop(&ServeArgs { dir: serve_dir, sdk: serve_sdk }, Duration::from_secs(1))
        });
        let layout = Layout::new(&root);
        let deadline = Instant::now() + Duration::from_secs(20);
        while !dartq_session::socket_open(&layout) {
            assert!(Instant::now() < deadline, "the serve loop never listened");
            std::thread::sleep(Duration::from_millis(50));
        }
        let answer = roundtrip(
            &layout.socket,
            r#"{"jsonrpc":"2.0","id":1,"method":"dartq/analyze","params":{}}"#,
        );
        assert!(answer.contains("\"id\":1"), "answered: {answer}");
        assert!(answer.contains("diags"), "the analyze envelope: {answer}");
        // The state file names the server and its triple.
        let state = SessionState::read(&layout).expect("state");
        assert!(pid_alive(state.pid));
        assert_eq!(state.triple.root, root.canonicalize().expect("canon"));
        // Idle past the timeout: the loop cleans up and exits clean.
        let code = server.join().expect("server thread");
        assert_eq!(code, exit::CLEAN);
        assert!(!layout.socket.exists(), "the socket was removed at reap");
        assert!(!layout.state.exists(), "the state was removed at reap");
        // #2619: the reap took the graceful ladder — the shutdown event
        // names the reason and what it released (nothing held here).
        let shutdown = EventLog::new(&layout)
            .read(Some("session"), 0, usize::MAX)
            .into_iter()
            .find(|(_, value)| value["event"].as_str() == Some("shutdown"))
            .map(|(_, value)| value)
            .expect("the shutdown event landed");
        assert_eq!(shutdown["data"]["reason"], "idle");
        assert_eq!(shutdown["data"]["sessions_released"], 0);
        std::fs::remove_dir_all(&root).ok();
    }

    /// #2619: the idle clock — client silence, frozen at zero while any
    /// debug session is attached. A held session is never idle however
    /// long the silence, and on detach a full fresh idle window runs.
    #[test]
    fn the_idle_clock_freezes_while_debug_sessions_are_attached() {
        let start = Instant::now();
        let silent_for = |secs: u64, sessions: usize| {
            idle_elapsed(start, start + Duration::from_secs(secs), sessions)
        };
        assert_eq!(silent_for(0, 0), Duration::ZERO);
        assert_eq!(
            silent_for(16 * 60, 0),
            Duration::from_secs(16 * 60),
            "no session: plain silence counts"
        );
        assert_eq!(
            silent_for(16 * 60, 1),
            Duration::ZERO,
            "one held session freezes the clock past the timeout"
        );
        assert_eq!(
            silent_for(16 * 60, 4),
            Duration::ZERO,
            "several held sessions are never idle either"
        );
    }

    /// #2632: the exit signal under a held client. A quiet long-lived
    /// connection (the MCP stdio proxy's shape) used to pin the loop
    /// inside `serve_client` until the client disconnected — a SIGTERM
    /// landed unobserved, the reaper's SIGKILL escalation arrived first,
    /// and every held debug session died un-resumed. With the client
    /// loop polling the flag, the signal finishes the turn, the client
    /// sees a clean end, and the serve loop takes its shutdown ladder
    /// naming the signal.
    #[test]
    fn the_exit_signal_unblocks_a_held_client_into_the_shutdown_ladder() {
        // The real SIGTERM this pin delivers sets every registered
        // serve-loop flag in the process; the serial lock keeps the other
        // reason-asserting serve pins out of its window.
        let _serial = SIGNAL_SERIALIZATION.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let root = test_fixture("signal-held");
        let serve_dir = root.clone();
        let serve_sdk = root.join("sdk");
        let server = std::thread::spawn(move || {
            serve_loop(&ServeArgs { dir: serve_dir, sdk: serve_sdk }, Duration::from_secs(1))
        });
        let layout = Layout::new(&root);
        let deadline = Instant::now() + Duration::from_secs(20);
        while !dartq_session::socket_open(&layout) {
            assert!(Instant::now() < deadline, "the serve loop never listened");
            std::thread::sleep(Duration::from_millis(50));
        }
        // Hold a client open, quiet — the exact shape that used to pin
        // the loop. The idle timeout (1 s here) must NOT fire while it
        // is held; only the signal may end the serve. The hold stays
        // short so the post-signal return to the loop top (≤ one 250 ms
        // tick + one 250 ms sleep) still beats the idle window.
        let held = UnixStream::connect(&layout.socket).expect("client connects");
        std::thread::sleep(Duration::from_millis(100));
        // The exit signal lands while the client is held. In-process,
        // the serve loop's own flag registration turns the real SIGTERM
        // into the flag set — no test hook needed.
        // SAFETY: SIGTERM to this test process reaches only the serve
        // loop's registered flag (its disposition replaced the default
        // when the loop started); nothing else in the harness arms one.
        unsafe {
            libc::kill(std::process::id() as libc::pid_t, libc::SIGTERM);
        }
        let joined = Instant::now() + Duration::from_secs(10);
        while !server.is_finished() && Instant::now() < joined {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            server.is_finished(),
            "the signal ended the held-client serve without waiting for the client"
        );
        drop(held);
        let code = server.join().expect("server thread");
        assert_eq!(code, exit::CLEAN);
        // The ladder ran for the signal's reason, not the idle reaper's.
        let shutdown = EventLog::new(&layout)
            .read(Some("session"), 0, usize::MAX)
            .into_iter()
            .find(|(_, value)| value["event"].as_str() == Some("shutdown"))
            .map(|(_, value)| value)
            .expect("the shutdown event landed");
        assert_eq!(shutdown["data"]["reason"], "signal");
        assert_eq!(shutdown["data"]["sessions_released"], 0);
        assert!(!layout.socket.exists(), "the socket was removed at shutdown");
        std::fs::remove_dir_all(&root).ok();
    }

    /// #2591: the heartbeat keeps beating while a client connection is
    /// being served — a held-open client used to stop the beat for its
    /// whole duration (a cold analysis inside one drain aged the
    /// heartbeat by minutes). The beat thread is independent of the
    /// accept loop, so the state stays fresh under a held client.
    #[test]
    fn the_heartbeat_stays_fresh_while_a_client_is_held_open() {
        // In-process serve loop: collateral of the real signal test (see
        // SIGNAL_SERIALIZATION) — a stray SIGTERM would run this loop's
        // shutdown ladder and strip the state file mid-hold (the observed
        // full-suite failure: "state while held").
        let _serial = SIGNAL_SERIALIZATION.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let root = test_fixture("beat-held");
        let serve_dir = root.clone();
        let serve_sdk = root.join("sdk");
        let server = std::thread::spawn(move || {
            serve_loop(&ServeArgs { dir: serve_dir, sdk: serve_sdk }, Duration::from_secs(1))
        });
        let layout = Layout::new(&root);
        let deadline = Instant::now() + Duration::from_secs(20);
        while !dartq_session::socket_open(&layout) {
            assert!(Instant::now() < deadline, "the serve loop never listened");
            std::thread::sleep(Duration::from_millis(50));
        }
        // Hold a client open and pin the beat under it, deterministically:
        // first learn the heartbeat current at hold entry (the serve
        // thread's initial write lands between bind and the first accept,
        // so it may postdate the connect), then wait — bounded, polling
        // the observable state write rather than sleeping a fixed window
        // a loaded host can stretch — for a strictly newer beat while the
        // client stays held. That newer beat can only come from the beat
        // thread, which must therefore be independent of client service.
        let held = UnixStream::connect(&layout.socket).expect("client connects");
        let entry_hb = loop {
            match SessionState::read(&layout) {
                Some(state) => break state.heartbeat_ms,
                None => {
                    assert!(Instant::now() < deadline, "the initial heartbeat never landed");
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        };
        let beat_by = Instant::now() + 2 * HEARTBEAT;
        loop {
            let beat = SessionState::read(&layout).map(|state| state.heartbeat_ms);
            if beat.is_some_and(|heartbeat_ms| heartbeat_ms > entry_hb) {
                break;
            }
            assert!(
                Instant::now() < beat_by,
                "no heartbeat newer than hold entry landed while the client was held: {beat:?}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        drop(held);
        let _ = server.join();
        std::fs::remove_dir_all(&root).ok();
    }

    /// #2592: a checkout whose worktree socket path exceeds the kernel's
    /// `sun_path` budget still gets a listening session — the socket
    /// relocates to the per-user short directory (pointer file left in
    /// the session dir), and the protocol round-trips over it.
    #[test]
    fn a_long_checkout_path_still_listens_on_the_relocated_socket() {
        // In-process serve loop: collateral of the real signal test (see
        // SIGNAL_SERIALIZATION).
        let _serial = SIGNAL_SERIALIZATION.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        // A root deep enough that `<root>/.dart_tool/dartq/session.sock`
        // blows the 104-byte sun_path: one long directory name under /tmp.
        let segment = "w".repeat(72);
        let root = PathBuf::from("/tmp").join(format!("dqs-long-{}-{segment}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("lib")).expect("dir");
        std::fs::create_dir_all(root.join(".dart_tool")).expect("dot dir");
        std::fs::write(root.join("pubspec.yaml"), "name: fx\nenvironment:\n  sdk: ^3.5.0\n").expect("pubspec");
        std::fs::write(
            root.join(".dart_tool/package_config.json"),
            r#"{"configVersion":2,"packages":[{"name":"fx","rootUri":"../","packageUri":"lib/","languageVersion":"3.5"}],"generator":"pub"}"#,
        )
        .expect("package config");
        let sdk = root.join("sdk");
        for (rel, body) in [
            ("version", "3.13.1\n"),
            ("lib/libraries.json", r#"{"vm":{"libraries":{"core":{"uri":"core/core.dart"}}}}"#),
            (
                "lib/_internal/allowed_experiments.json",
                r#"{"version":1,"experimentSets":{"sdkExperiments":[]},"sdk":{"default":{"experimentSet":"sdkExperiments"}},"packages":{}}"#,
            ),
            ("lib/core/core.dart", "library dart.core;\n"),
        ] {
            let path = sdk.join(rel);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("sdk dir");
            std::fs::write(path, body).expect("sdk file");
        }
        std::fs::write(root.join("lib/a.dart"), "int a() => 1;\n").expect("source");

        let layout = Layout::new(&root);
        let local = layout.dir.join("session.sock");
        assert!(
            local.as_os_str().len() > 104,
            "the fixture's local socket path must exceed sun_path: {}",
            local.display()
        );
        assert_ne!(layout.socket, local, "the socket relocated");
        assert!(
            layout.socket.as_os_str().len() <= 104,
            "the relocated path fits: {}",
            layout.socket.display()
        );

        let serve_dir = root.clone();
        let serve_sdk = sdk.clone();
        let server = std::thread::spawn(move || {
            serve_loop(&ServeArgs { dir: serve_dir, sdk: serve_sdk }, Duration::from_secs(1))
        });
        let deadline = Instant::now() + Duration::from_secs(20);
        while !dartq_session::socket_open(&layout) {
            assert!(Instant::now() < deadline, "the long-path session never listened");
            std::thread::sleep(Duration::from_millis(50));
        }
        // The pointer file names the real socket; the protocol answers
        // over the relocated path.
        let pointer = std::fs::read_to_string(layout.dir.join("session.sock.point"))
            .expect("the pointer file exists");
        assert_eq!(
            pointer.as_bytes(),
            layout.socket.as_os_str().as_encoded_bytes(),
            "the pointer names the socket"
        );
        let answer = roundtrip(
            &layout.socket,
            r#"{"jsonrpc":"2.0","id":1,"method":"dartq/test/status","params":{}}"#,
        );
        assert!(answer.contains("\"id\":1"), "answered over the relocated socket: {answer}");
        assert!(!local.exists(), "no socket was attempted at the long path");

        let _ = server.join();
        assert!(!layout.socket.exists(), "the reap removed the relocated socket");
        std::fs::remove_dir_all(&root).ok();
        let _ = std::fs::remove_file(&layout.socket);
    }
    /// group. A starter task killed by group (the CI/shell shape — the
    /// live incident's second finding) must leave the engine serving:
    /// the socket answers and the state's pid stays alive.
    #[test]
    fn the_engine_survives_its_starters_process_group_death() {
        use std::os::unix::process::CommandExt as _;

        let root = test_fixture("pg-detach");
        // The real CLI binary: unit tests execute from
        // `deps/dartq-<hash>`, the linked binary sits two levels up
        // (`cargo test` builds the bin target alongside).
        let dartq = std::env::current_exe()
            .expect("current exe")
            .parent()
            .and_then(Path::parent)
            .map(|dir| dir.join("dartq"))
            .filter(|path| path.is_file())
            .unwrap_or_else(|| panic!("no linked dartq binary next to the test"));
        // A shell task in its own process group starts the session and
        // then lingers — the kill below lands on the whole group while
        // it is alive, exactly like a harness reaping its task.
        let script = format!(
            "'{}' session start '{}' --sdk '{}' && sleep 30",
            dartq.display(),
            root.display(),
            root.join("sdk").display()
        );
        let mut starter = Command::new("zsh");
        starter
            .arg("-c")
            .arg(&script)
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // A private session registry: the real `session start` sweeps
        // foreign-binary records, and this suite's in-process serve
        // tests register the test process itself — a shared registry
        // would have the starter SIGTERM this very test binary.
        starter.env("XDG_CACHE_HOME", root.join("cache"));
        let mut starter = starter.spawn().expect("starter task spawns");
        let layout = Layout::new(&root);
        let deadline = Instant::now() + Duration::from_secs(30);
        while !dartq_session::socket_open(&layout) {
            assert!(Instant::now() < deadline, "the engine never listened");
            std::thread::sleep(Duration::from_millis(50));
        }
        let state = SessionState::read(&layout).expect("state after start");

        // The group kill: SIGKILL to the starter's whole process group.
        // The engine's own group (spawn_server's detach) must survive it.
        unsafe {
            libc::kill(-(starter.id() as libc::pid_t), libc::SIGKILL);
        }
        let _ = starter.wait();

        assert!(
            pid_alive(state.pid),
            "the engine survived its starter's process-group death"
        );
        let answer = roundtrip(
            &layout.socket,
            r#"{"jsonrpc":"2.0","id":1,"method":"dartq/test/status","params":{}}"#,
        );
        assert!(answer.contains("\"id\":1"), "the engine still answers: {answer}");
        assert!(answer.contains("runs"), "the status envelope: {answer}");

        // Teardown: stop the engine and clear the tree.
        dartq_session::terminate(state.pid);
        let deadline = Instant::now() + Duration::from_secs(10);
        while pid_alive(state.pid) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        std::fs::remove_dir_all(&root).ok();
    }

    /// The subverb grammar: `start` defaults to the cwd with no SDK flag,
    /// `stop` takes `--all`, `serve` requires both `--dir` and `--sdk`
    /// (it is the machine-facing spawn, not a human verb), and an unknown
    /// subverb is the clap usage error.
    #[test]
    fn session_subverbs_parse() {
        // `derive(Args)` brings no `try_parse_from`; parse through the
        // augmented command the parent `Cli` would splice in.
        let parse = |args: &[&str]| -> Result<SessionArgs, clap::Error> {
            let command = <SessionArgs as clap::Args>::augment_args(clap::Command::new("session"));
            let matches = command
                .try_get_matches_from(std::iter::once("session").chain(args.iter().copied()))?;
            <SessionArgs as clap::FromArgMatches>::from_arg_matches(&matches)
        };
        let start = parse(&["start"]).expect("parse");
        match &start.command {
            SessionCommand::Start(args) => {
                assert_eq!(args.dir, PathBuf::from("."));
                assert_eq!(args.sdk, None);
            }
            _ => panic!("expected start"),
        }
        let start = parse(&["start", "/w", "--sdk", "/s"]).expect("parse");
        match &start.command {
            SessionCommand::Start(args) => {
                assert_eq!(args.dir, PathBuf::from("/w"));
                assert_eq!(args.sdk, Some(PathBuf::from("/s")));
            }
            _ => panic!("expected start"),
        }
        assert!(matches!(
            parse(&["status"]).expect("parse").command,
            SessionCommand::Status(_)
        ));
        let stop = parse(&["stop"]).expect("parse");
        match &stop.command {
            SessionCommand::Stop(args) => {
                assert_eq!(args.dir, PathBuf::from("."));
                assert!(!args.all);
            }
            _ => panic!("expected stop"),
        }
        let stop = parse(&["stop", "--all", "/w"]).expect("parse");
        match &stop.command {
            SessionCommand::Stop(args) => {
                assert!(args.all);
                assert_eq!(args.dir, PathBuf::from("/w"));
            }
            _ => panic!("expected stop"),
        }
        let serve = parse(&["serve", "--dir", "/w", "--sdk", "/s"]).expect("parse");
        assert!(
            matches!(&serve.command, SessionCommand::Serve(args) if args.dir == PathBuf::from("/w") && args.sdk == PathBuf::from("/s"))
        );
        assert!(parse(&["serve"]).is_err(), "serve requires its flags");
        assert!(parse(&["bogus"]).is_err());
    }

    /// The SDK root order: the flag wins, `$DART_SDK` answers when the
    /// flag is absent, and neither is the navigation error.
    #[test]
    fn resolved_sdk_root_flag_then_env_then_error() {
        let flag = resolved_sdk_root(Some(Path::new("/flag")));
        assert_eq!(flag, Ok(PathBuf::from("/flag")));
        let saved = std::env::var_os("DART_SDK");
        unsafe { std::env::set_var("DART_SDK", "/env/sdk"); }
        assert_eq!(resolved_sdk_root(None), Ok(PathBuf::from("/env/sdk")));
        assert_eq!(
            resolved_sdk_root(Some(Path::new("/flag"))),
            Ok(PathBuf::from("/flag")),
            "the flag still wins over the environment"
        );
        unsafe { std::env::remove_var("DART_SDK"); }
        let error = resolved_sdk_root(None).expect_err("no sdk source");
        assert!(error.contains("DART_SDK"), "{error}");
        if let Some(saved) = saved {
            unsafe { std::env::set_var("DART_SDK", saved); }
        }
    }

    /// One `stop` of a live record: the process dies, the socket, the
    /// state file, and the registry entry all go, and the caller learns
    /// it left.
    #[test]
    fn stop_one_terminates_and_cleans_every_artifact() {
        let home = std::env::temp_dir().join(format!("dqs-stop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).expect("dir");
        let registry = Registry::at(&home.join("sessions"));
        let root = home.join("w");
        let layout = Layout::new(&root);
        layout.create().expect("layout");
        let triple = SessionTriple::compute(&root, &root);
        let listener = UnixListener::bind(&layout.socket).expect("bind");
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if stream.is_err() {
                    break;
                }
            }
        });
        let child = std::process::Command::new("sleep").arg("30").spawn().expect("sleep child");
        let state = SessionState {
            pid: child.id(),
            heartbeat_ms: SessionState::now_ms(),
            triple: triple.clone(),
            debug_sessions: 0,
        };
        state.write(&layout).expect("state");
        let record = Record { id: triple.identity(), state };
        registry.upsert(&record).expect("upsert");
        assert!(layout.socket.exists() && layout.state.exists());
        // In production the serve process's parent is init (detached), so
        // termination reaps promptly; in the harness the test process is
        // the parent, and a zombie still answers kill(pid, 0) — the
        // boolean is therefore not asserted here, the delivery is.
        let _ = stop_one(&registry, &record);
        let mut child = child;
        let exited = child.try_wait().expect("waitable").expect("the SIGTERM reached the server");
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt as _;
            assert_eq!(exited.signal(), Some(libc::SIGTERM), "{exited:?}");
        }
        assert!(!layout.socket.exists(), "socket removed");
        assert!(!layout.state.exists(), "state removed");
        assert!(registry.list().is_empty(), "the registry entry dropped");
        // A dead pid answers gone immediately.
        assert!(wait_gone(u32::MAX - 1));
        std::fs::remove_dir_all(&home).ok();
    }
}

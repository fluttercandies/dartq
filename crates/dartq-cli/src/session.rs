//! The `dartq session` shell: the resident session layer's management
//! face — `start` (elect or attach), `status` (the per-user registry,
//! grouped by checkout), `stop`, and the hidden `serve` the detached
//! server process runs. The identity, election, registry, and lifecycle
//! rules live in the `dartq-session` crate; the engine serving is the
//! daemon protocol (`dartq-daemon::run`) over the session socket, one
//! client at a time.

use std::io;
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;
#[cfg(unix)]
use std::time::Instant;

use clap::{Args, Subcommand};
use serde_json::{Value, json};

use dartq_session::{
    Ensured, Layout, Record, Registry, SessionState, SessionTriple, ensure, pid_alive, terminate,
};
#[cfg(unix)]
use dartq_session::{EventLog, HEARTBEAT, IDLE_TIMEOUT};

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
    /// Hidden: not a human verb. Unix-only: the serve binds the session's
    /// Unix socket and rides the unix-only memory ledger (`ensure` only
    /// ever spawns it on unix — on other hosts a session is always
    /// standalone and this verb cannot be reached).
    #[cfg(unix)]
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

#[cfg(unix)]
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
        #[cfg(unix)]
        SessionCommand::Serve(serve) => serve_loop(serve, IDLE_TIMEOUT),
    }
}

/// The pure flag/env step of [`sdk_root_for`]: the flag, else
/// `$DART_SDK` — the engine assembly's own resolution order, surfaced
/// here because the triple needs the concrete root. Its error is the
/// navigation text `sdk_root_for` answers when no live session fills in
/// either, so it names the session way too.
fn resolved_sdk_root(sdk: Option<&Path>) -> Result<PathBuf, String> {
    sdk.map(Path::to_path_buf)
        .or_else(|| std::env::var_os("DART_SDK").map(PathBuf::from))
        .ok_or_else(|| {
            "no Dart SDK root (pass --sdk PATH, set DART_SDK, or start a session with \
             `dartq session start --sdk PATH`)"
                .to_owned()
        })
}

/// The SDK root a face that attaches to `root`'s session keys on: the
/// flag, else `$DART_SDK`, else the live resident session's own triple
/// (the state file is keyed by root alone, so a live session names the
/// one SDK to attach under — `session start` without a flag attaches to
/// it, and the debug/MCP/answer faces inherit it silently). No live
/// session answers the navigation error, which names all three ways.
pub(crate) fn sdk_root_for(root: &Path, sdk: Option<&Path>) -> Result<PathBuf, String> {
    match resolved_sdk_root(sdk) {
        Ok(sdk_root) => Ok(sdk_root),
        Err(navigation) => dartq_session::resident_sdk_root(root).ok_or(navigation),
    }
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
        // The daemon outlives the starter's job slot and takes its own.
        dartq_toolchain::ledger::detach(&mut command);
    }
    command.spawn().map(|child| {
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
        if row
            .get("stale_binary")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
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
    let sdk_root = match sdk_root_for(&root, args.sdk.as_deref()) {
        Ok(sdk) => sdk,
        Err(message) => {
            eprintln!("error: {message}");
            return exit::USAGE;
        }
    };
    // Lifecycle upkeep rides the start: upgrade orphans die, the cap
    // holds, dead-daemon socket files go (#2661 — a SIGKILLed reap
    // leaves them behind and nothing else ever claims them), then the
    // election runs.
    let registry = Registry::open();
    let triple_probe = SessionTriple::compute(&root, &sdk_root);
    for swept in registry.sweep_upgrades(&triple_probe.binary_stamp) {
        let row = json!({"type": "swept", "root": swept.display().to_string()});
        print_row(
            args.format,
            &row,
            Some(format!("swept stale session at {}", swept.display())),
        );
    }
    let _ = registry.sweep_dead_sockets();
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
    let binary = SessionTriple::compute(Path::new("."), Path::new(".")).binary_stamp;
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
            let package = progress.current_package.as_deref().unwrap_or("analyzing");
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
            "binary": record.state.triple.binary_stamp,
            "age_s": age_s,
            "stale_binary": record.state.triple.binary_stamp != binary,
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
        "binary": state.triple.binary_stamp,
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
#[cfg(unix)]
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
#[cfg(unix)]
fn serve_loop(args: &ServeArgs, idle_timeout: Duration) -> u8 {
    let root = match std::fs::canonicalize(&args.dir) {
        Ok(root) => root,
        Err(error) => {
            eprintln!(
                "dartq: session serve: cannot resolve {}: {error}",
                args.dir.display()
            );
            return exit::SOFTWARE;
        }
    };
    let layout = Layout::new(&root);
    if let Err(reason) = layout.create() {
        eprintln!("dartq: session serve: {reason}");
        return exit::SOFTWARE;
    }
    let (config, _dir, advisory) = match crate::engine_config_with_advisory(&root, Some(&args.sdk))
    {
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
    let mut session =
        match dartq_daemon::Session::with_runtime(config, Vec::new(), None, advisory, runtime) {
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
            eprintln!(
                "dartq: session serve: binding {}: {error}",
                layout.socket.display()
            );
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
    // The last-answer clock rides the same way: the pressure eviction's
    // LRU sort reads `used_ms` out of every live state's registry copy.
    let held_sessions = session.debug_sessions_counter();
    let last_used = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(
        u64::try_from(SessionState::now_ms()).unwrap_or(u64::MAX),
    ));
    heartbeat(
        &layout,
        &registry,
        &triple,
        held_sessions.load(std::sync::atomic::Ordering::Relaxed),
        u128::from(last_used.load(std::sync::atomic::Ordering::Relaxed)),
    );

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
    let beat_used = std::sync::Arc::clone(&last_used);
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
                        u128::from(beat_used.load(std::sync::atomic::Ordering::Relaxed)),
                    );
                }
            }
        })
        .expect("the heartbeat thread spawns");

    let mut last_client = Instant::now();
    // Consecutive accept failures (#2675): a transient accept error (a
    // descriptor or connection-table blip under host load) must never end
    // the daemon by itself — the shutdown ladder it used to take strands
    // every held debug session behind a socket nothing answers, which a
    // client observes as a hung transport followed by a fresh daemon that
    // knows none of its sessions. The loop retries one tick at a time;
    // only a listener that cannot recover for a whole window (40 ticks ≈
    // 10 s) ends the serve through the ladder.
    let mut accept_failures: u32 = 0;
    // #2619: the exit signals land in a flag this loop's tick polls —
    // SIGTERM (the election reap's grace window, `session stop`) and
    // SIGINT both take the graceful ladder below instead of killing a
    // daemon that holds paused debug sessions. Registration failure
    // degrades to the default disposition (an immediate death, the
    // pre-#2619 behavior), never a failed serve.
    let signalled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    // #2729: the session's analyses ride a token bound to the SAME
    // cell — a mid-turn SIGTERM stops the in-flight engine run at its
    // next checkpoint (~the token's stride) instead of waiting out the
    // turn for the reaper's grace. No watcher thread: the signal write
    // is already asynchronous to this loop.
    session.bind_quit_flag(std::sync::Arc::clone(&signalled));
    let signals_ready = {
        use signal_hook::consts::{SIGINT, SIGTERM};
        signal_hook::flag::register(SIGTERM, std::sync::Arc::clone(&signalled))
            .and_then(|_| signal_hook::flag::register(SIGINT, std::sync::Arc::clone(&signalled)))
            .is_ok()
    };
    if !signals_ready {
        eprintln!("dartq: session serve: signal handling unavailable; exit signals kill in place");
    }
    // The machine-wide job gate: the daemon holds a job slot and books its
    // last recorded peak for its life. It is a resident the gate may
    // release (SIGTERM, the graceful ladder) only while its busy lock is
    // free: no client is being served and no debug session is held. While
    // it hosts a debug session it gives the slot up (debugging is never
    // restricted) and keeps the memory booked.
    let peak_key = format!("session:{}", root.display());
    let max_percent = crate::memory_max_percent(&root);
    let mut busy = Busy::open(&layout.dir.join("busy.lock"));
    let mut pressure = dartq_toolchain::ledger::Pressure::default();
    let mut lease = match lease_session(&peak_key, &root, &layout, &signalled) {
        Ok(lease) => lease,
        Err(message) => {
            eprintln!("dartq: session serve: {message}");
            stop_accepting(listener, &layout);
            return serve_shutdown(
                &mut session,
                &events,
                &layout,
                &root,
                &beating,
                beater,
                "gate",
                exit::SOFTWARE,
            );
        }
    };
    let _peak = PeakOnExit(peak_key.clone());
    loop {
        if signalled.load(std::sync::atomic::Ordering::Relaxed) {
            stop_accepting(listener, &layout);
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
                accept_failures = 0;
                last_client = Instant::now();
                let _ = stream.set_nonblocking(false);
                let _ = events.append("session", "client_attached", serde_json::json!({}));
                busy.hold(true);
                serve_client(stream, &mut session, &signalled, &mut || {
                    if let Some(lease) = lease.as_mut() {
                        size_after_load(lease, &peak_key, &trace);
                    }
                });
                busy.hold(hosting(&session) > 0);
                if let Some(lease) = lease.as_ref() {
                    lease.touch();
                }
                let _ = events.append("session", "client_detached", serde_json::json!({}));
                last_used.store(
                    u64::try_from(SessionState::now_ms()).unwrap_or(u64::MAX),
                    std::sync::atomic::Ordering::Relaxed,
                );
                heartbeat(
                    &layout,
                    &registry,
                    &triple,
                    held_sessions.load(std::sync::atomic::Ordering::Relaxed),
                    u128::from(last_used.load(std::sync::atomic::Ordering::Relaxed)),
                );
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                busy.hold(hosting(&session) > 0);
            }
            Err(error) => {
                accept_failures += 1;
                if accept_failures == 1 || accept_failures % 20 == 0 {
                    eprintln!(
                        "dartq: session serve: accept failed ({accept_failures} consecutive): {error}"
                    );
                }
                if accept_failures >= 40 {
                    eprintln!("dartq: session serve: the listener cannot recover; shutting down");
                    stop_accepting(listener, &layout);
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
        }
        if !settle_slot(
            lease.as_mut(),
            session.debug_sessions_len(),
            session.test_runs_live(),
        ) {
            eprintln!(
                "dartq: session serve: the last debug session ended and no job slot is free; \
                 shutting down"
            );
            stop_accepting(listener, &layout);
            return serve_shutdown(
                &mut session,
                &events,
                &layout,
                &root,
                &beating,
                beater,
                "no_slot",
                exit::CLEAN,
            );
        }
        // Memory pressure evicts an idle peer: while this daemon itself
        // is idle (no client, nothing hosted) the shared ledger monitor
        // samples host memory on its own cadence; consecutive lows under
        // the ledger's reserve line release the least-recently-used idle
        // ledger holder — a resident analyzer or a peer session, busy
        // lock free, quiet past its unused window — and the machine-wide
        // cooldown paces evictions so one pressure episode cannot strip
        // the fleet in a burst. The monitor complements the job gate:
        // the gate releases idle residents only when an admission needs
        // the memory; the monitor gives it back before anyone has to ask.
        if hosting(&session) == 0 && !busy.held {
            if let Some(victim) = pressure.tick(std::process::id(), max_percent, true) {
                let _ = events.append(
                    "session",
                    "pressure_evict",
                    serde_json::json!({ "victim": victim }),
                );
            }
        } else {
            pressure.reset();
        }
        if idle_elapsed(last_client, Instant::now(), hosting(&session)) > idle_timeout {
            stop_accepting(listener, &layout);
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

/// The shared pressure monitor lives in `dartq_toolchain::ledger`
/// (`Pressure::tick`): one policy for the session daemon and every
/// analyzer-host daemon — cadence, consecutive-low arm, the ledger-wide
/// cooldown, and the LRU-idle releasable victim choice over the ledger's
/// own holder records (the session's busy lock covers "serving or
/// hosting", so a debug mount is never evictable there).

/// The session daemon's busy lock: held while a client is served or a
/// debug session is held, so the memory ledger never releases a daemon
/// mid-request or with a paused app behind it. A lock file that cannot be
/// opened holds nothing (the daemon is then never seen idle either: the
/// ledger releases a resident only when it can take the lock itself).
#[cfg(unix)]
struct Busy {
    file: Option<std::fs::File>,
    held: bool,
}

#[cfg(unix)]
impl Busy {
    fn open(path: &Path) -> Self {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)
            .ok();
        Self { file, held: false }
    }

    fn hold(&mut self, held: bool) {
        let Some(file) = &self.file else {
            return;
        };
        if held == self.held {
            return;
        }
        let changed = if held { file.lock() } else { file.unlock() };
        if changed.is_ok() {
            self.held = held;
        }
    }
}

/// Records this daemon's peak RSS under its key when the serve ends, for
/// the next start's lease.
#[cfg(unix)]
struct PeakOnExit(String);

#[cfg(unix)]
impl Drop for PeakOnExit {
    fn drop(&mut self) {
        dartq_toolchain::ledger::record_peak(
            &self.0,
            dartq_toolchain::ledger::Face::Session,
            dartq_toolchain::proc::self_peak_rss(),
        );
    }
}

/// What the daemon hosts beyond its clients: debug sessions and running
/// `dartq/test/run` children. While it hosts anything it is busy (never
/// released by the gate) and never idle.
#[cfg(unix)]
fn hosting(session: &dartq_daemon::Session) -> usize {
    session.debug_sessions_len() + session.test_runs_live()
}

/// Keeps the daemon's job slot in step with what it hosts: given up while
/// it hosts a debug session, retaken without waiting when the last one
/// ends. While a test run lives the slot stays as it is: the run either
/// sits in the daemon's slot (and leaving it would let the run escape the
/// count cap) or, spawned while the slot was given up, holds its own (and
/// a failed retake would end the daemon under it). `false` when the slot
/// cannot be retaken (every slot is taken): the daemon then exits rather
/// than run outside the cap.
#[cfg(unix)]
fn settle_slot(
    lease: Option<&mut dartq_toolchain::ledger::Lease>,
    debug_sessions: usize,
    test_runs: usize,
) -> bool {
    let Some(lease) = lease else {
        return true;
    };
    if test_runs > 0 {
        return true;
    }
    if debug_sessions > 0 {
        lease.leave_slot();
        return true;
    }
    // A gate that cannot be read keeps the daemon serving, as at start.
    lease.retake_slot().unwrap_or(true)
}

/// Books the daemon's measured peak in its lease once its first analysis
/// has run (the engine trace names an analyzed library), checked after
/// every served turn while the client is still attached: an unsized
/// daemon then leaves the first-run slot at once, and the daemon of
/// another worktree waiting there borrows the figure. Once sized, the
/// check is a field read; the exit records the final peak
/// ([`PeakOnExit`]).
#[cfg(unix)]
fn size_after_load(lease: &mut dartq_toolchain::ledger::Lease, peak_key: &str, trace: &Path) {
    if lease.sized() || dartq_session::Progress::from_trace(trace).files_scanned == 0 {
        return;
    }
    lease.record_peak(peak_key, dartq_toolchain::proc::self_peak_rss());
}

/// How long a session daemon waits at the job gate before it gives up.
#[cfg(unix)]
const SESSION_MEMORY_WAIT: Duration = Duration::from_secs(30 * 60);

/// The session daemon's lease: its recorded peak (else the largest live
/// session daemon's, else the ledger default) over its own tree, analyzer
/// plugin children included (a debug app holds its own lease and is not
/// counted twice). A ledger that cannot be used serves unleased; the error
/// is a wait that timed out. An exit signal ends the wait.
#[cfg(unix)]
fn lease_session(
    peak_key: &str,
    root: &Path,
    layout: &Layout,
    signalled: &std::sync::atomic::AtomicBool,
) -> Result<Option<dartq_toolchain::ledger::Lease>, String> {
    use dartq_toolchain::ledger::{self, Face, Releasable, Request};
    #[cfg(test)]
    if !crate::tests::gate_under_test() {
        return Ok(None);
    }
    let (bytes, sized) = ledger::sizing(peak_key, Face::Session);
    let request = Request {
        face: Face::Session,
        label: root.display().to_string(),
        bytes,
        pid: Some(std::process::id()),
        release: Some(Releasable {
            pid: std::process::id(),
            busy: layout.dir.join("busy.lock"),
        }),
        slot: true,
        cap: crate::job_cap(root),
        sized,
        peak_key: Some(peak_key.to_owned()),
        source_bytes: None,
    };
    let deadline = Instant::now() + SESSION_MEMORY_WAIT;
    let max_percent = crate::memory_max_percent(root);
    match ledger::acquire_or_stop(request, bytes, max_percent, deadline, signalled, |note| {
        eprintln!("{note}");
    }) {
        Ok(lease) => Ok(lease),
        Err(error @ (ledger::Error::Timeout { .. } | ledger::Error::Stopped)) => {
            Err(error.to_string())
        }
        Err(ledger::Error::Io(_)) => Ok(None),
    }
}

/// Ends the accept face at ladder entry (#2675): the socket path unlinks
/// and the listener drops BEFORE the — possibly seconds-long — detach
/// ladder runs, so a client connecting during the ladder is refused at
/// once and re-elects cleanly instead of hanging against a doomed
/// daemon's backlog, and a takeover during the ladder can never bind a
/// socket the dying daemon's cleanup would later strip.
#[cfg(unix)]
fn stop_accepting(listener: UnixListener, layout: &Layout) {
    let _ = std::fs::remove_file(&layout.socket);
    drop(listener);
}

/// The idle clock (#2619): client silence, frozen at zero while the
/// daemon hosts anything ([`hosting`]: a debug session, a test run) — a held debug session is never idle (a
/// paused app must not outlive its daemon by reaper fiat), and on
/// detach a full fresh idle window runs.
#[cfg(unix)]
fn idle_elapsed(last_client: Instant, now: Instant, hosted: usize) -> Duration {
    if hosted > 0 {
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
#[cfg(unix)]
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
    // #2729: every ladder entry kills the session's analysis token —
    // the signal arms already share the cell (idempotent here), and
    // the flag-less exits (idle reap, unrecoverable listener) leave
    // no live analyzer behind before the detach walk and cleanup run.
    session.cancel_analysis();
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
#[cfg(unix)]
fn serve_client(
    stream: UnixStream,
    session: &mut dartq_daemon::Session,
    signalled: &std::sync::atomic::AtomicBool,
    on_turn: &mut dyn FnMut(),
) {
    let Ok(read) = stream.try_clone() else {
        return;
    };
    // The read half is the clone; the original stream stays the write
    // half. run owns both and returns at the client's disconnect or the
    // exit signal.
    let _ = dartq_daemon::run_with_signal(read, stream, session, None, Some(signalled), on_turn);
}

/// Binds the socket, unlinking a leftover file once (a crashed server's
/// residue — the election normally cleans it, this is the belt).
#[cfg(unix)]
fn bind_takeover(layout: &Layout) -> io::Result<UnixListener> {
    UnixListener::bind(&layout.socket).or_else(|first| {
        let _ = std::fs::remove_file(&layout.socket);
        UnixListener::bind(&layout.socket).map_err(|_| first)
    })
}

/// The liveness rewrite: state.json plus the registry record. `held`
/// is the live debug-session count (#2632) — the reaper reading either
/// copy scales its SIGTERM grace by it instead of SIGKILLing a daemon
/// mid-detach. `used_ms` is the last client answer's wall time — the
/// memory-pressure eviction's LRU fact.
#[cfg(unix)]
fn heartbeat(
    layout: &Layout,
    registry: &Registry,
    triple: &SessionTriple,
    held: usize,
    used_ms: u128,
) {
    let state = SessionState {
        pid: std::process::id(),
        heartbeat_ms: SessionState::now_ms(),
        triple: triple.clone(),
        debug_sessions: held,
        used_ms,
    };
    if state.write(layout).is_ok() {
        let _ = registry.upsert(&Record {
            id: triple.identity(),
            state,
        });
    }
}

/// The shutdown residue: socket, state, registry record.
#[cfg(unix)]
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
        &ServeArgs {
            dir: dir.to_path_buf(),
            sdk: sdk.to_path_buf(),
        },
        Duration::from_secs(1),
    )
}

/// [`serve_loop_for_tests`] with a caller-chosen idle window — the
/// resident-routing tests run a cold in-process pass beside the serve
/// loop and must not race the 1 s self-reap.
#[cfg(all(test, unix))]
pub(crate) fn serve_loop_for_tests_with(dir: &Path, sdk: &Path, idle: Duration) -> u8 {
    serve_loop(
        &ServeArgs {
            dir: dir.to_path_buf(),
            sdk: sdk.to_path_buf(),
        },
        idle,
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

    /// The session daemon's busy lock is held exactly while it serves or
    /// holds a debug session: the memory ledger, which may release an
    /// idle daemon, can take the lock only when it is free.
    #[test]
    fn the_busy_lock_marks_a_session_daemon_unreleasable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("busy.lock");
        let mut busy = Busy::open(&path);
        let free = || {
            std::fs::File::open(&path)
                .expect("busy lock")
                .try_lock()
                .is_ok()
        };
        assert!(free(), "an idle daemon is releasable");
        busy.hold(true);
        assert!(!free(), "a serving daemon is not");
        busy.hold(true);
        assert!(!free(), "holding twice is still held");
        busy.hold(false);
        assert!(
            free(),
            "idle again once the client and debug sessions are gone"
        );
    }

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
        exchange(&mut UnixStream::connect(socket).expect("connect"), request)
    }

    /// One framed request over a held connection; returns the first
    /// response body.
    fn exchange(stream: &mut UnixStream, request: &str) -> String {
        let body = request.as_bytes();
        write!(stream, "Content-Length: {}\r\n\r\n", body.len()).expect("write frame");
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
            .find_map(|line| {
                line.split_once(':').and_then(|(name, value)| {
                    (name.trim().eq_ignore_ascii_case("content-length"))
                        .then(|| value.trim().parse().expect("length"))
                })
            })
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
        let _serial = SIGNAL_SERIALIZATION
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let root = test_fixture("events");
        let serve_dir = root.clone();
        let serve_sdk = root.join("sdk");
        let server = std::thread::spawn(move || {
            serve_loop(
                &ServeArgs {
                    dir: serve_dir,
                    sdk: serve_sdk,
                },
                Duration::from_secs(1),
            )
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
        assert!(
            progress.files_scanned >= 1,
            "the trace captured the sweep: {progress:?}"
        );
        // The package signal: the fixture's root dir carries `lib/`.
        let package_root = root
            .file_name()
            .and_then(|n| n.to_str())
            .expect("root name");
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
        let _serial = SIGNAL_SERIALIZATION
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let root = test_fixture("loop");
        // #2661: an earlier daemon's orphan manifest sits in the trail
        // — this daemon holds nothing, so its shutdown must release
        // zero and leave the trail untouched.
        let orphan_dir = Layout::new(&root).dir.join("debug");
        std::fs::create_dir_all(&orphan_dir).expect("orphan dir");
        std::fs::write(
            orphan_dir.join("9.json"),
            r#"{"ws_uri":"ws://earlier","kind":"attach","since":1700000000}"#,
        )
        .expect("orphan manifest");
        let serve_dir = root.clone();
        let serve_sdk = root.join("sdk");
        let server = std::thread::spawn(move || {
            serve_loop(
                &ServeArgs {
                    dir: serve_dir,
                    sdk: serve_sdk,
                },
                Duration::from_secs(1),
            )
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
        // The orphan trail survived the release-zero shutdown verbatim:
        // sessions_released counts only this daemon's own detachments,
        // and a daemon holding nothing rewrites no manifest.
        let manifest = std::fs::read_to_string(orphan_dir.join("9.json")).expect("manifest");
        assert!(
            !manifest.contains("released"),
            "an unheld orphan manifest is never rewritten: {manifest}"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// #2619: the idle clock — client silence, frozen at zero while any
    /// debug session is attached. A held session is never idle however
    /// long the silence, and on detach a full fresh idle window runs.
    /// Debugging is never restricted: a daemon hosting a debug session
    /// gives its job slot up; when the last one ends with every slot
    /// taken, it cannot retake one and the serve loop exits.
    #[cfg(unix)]
    #[test]
    fn a_daemon_whose_last_debug_session_ends_with_no_free_slot_exits() {
        use dartq_toolchain::ledger::{self, Face, Request};
        if crate::tests::isolated(
            "session::tests::a_daemon_whose_last_debug_session_ends_with_no_free_slot_exits",
        ) {
            return;
        }
        let request = |face, label: &str| Request {
            face,
            label: label.into(),
            bytes: 1 << 20,
            pid: None,
            release: None,
            slot: true,
            cap: 1,
            sized: true,
            peak_key: None,
            source_bytes: None,
        };
        let deadline = || Instant::now() + Duration::from_secs(5);
        let mut daemon =
            ledger::acquire(request(Face::Session, "a"), 1 << 20, 90, deadline(), |_| {})
                .expect("admitted")
                .expect("a ledger");
        assert!(
            settle_slot(Some(&mut daemon), 1, 0),
            "debugging keeps it serving"
        );
        assert!(!daemon.holds_slot(), "the slot is given up while debugging");
        // A test run is admitted at once beside the live debug session.
        let started = Instant::now();
        let test = ledger::acquire(request(Face::Test, "b"), 1 << 20, 90, deadline(), |_| {})
            .expect("admitted")
            .expect("a ledger");
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "admitted at once"
        );
        assert!(test.holds_slot(), "the test run takes the only slot");
        assert!(
            settle_slot(Some(&mut daemon), 0, 1),
            "a live test run keeps the daemon serving without a retake"
        );
        assert!(!daemon.holds_slot(), "the slot stays the test run's");
        assert!(
            !settle_slot(Some(&mut daemon), 0, 0),
            "no free slot: the daemon exits"
        );
        drop(test);
        assert!(
            settle_slot(Some(&mut daemon), 0, 0),
            "a free slot is retaken"
        );
        assert!(daemon.holds_slot());
    }

    /// The stand-in for the `dartq test` child a `dartq/test/run` spawns:
    /// a no-op unless [`PROBE_ENV`] names its report file. It makes the
    /// slot request a nested `dartq test` with no recorded peak makes (a
    /// first run), writes the outcome (`joined`, `slot`, or the error) to
    /// the report, and lives until the report's `.release` sibling
    /// appears.
    #[test]
    fn test_run_gate_probe() {
        use dartq_toolchain::ledger::{self, Face, Request};
        let Some(report) = std::env::var_os(PROBE_ENV).map(PathBuf::from) else {
            return;
        };
        let request = Request {
            face: Face::Test,
            label: "probe".into(),
            bytes: 1 << 20,
            pid: None,
            release: None,
            slot: true,
            cap: ledger::cap(0),
            sized: false,
            peak_key: None,
            source_bytes: None,
        };
        let deadline = Instant::now() + Duration::from_secs(3);
        let outcome = match ledger::acquire(request, 1 << 20, 90, deadline, |_| {}) {
            Ok(None) => "joined".to_owned(),
            Ok(Some(lease)) if lease.holds_slot() => "slot".to_owned(),
            Ok(Some(_)) => "memory".to_owned(),
            Err(error) => error.to_string(),
        };
        std::fs::write(&report, outcome).expect("report");
        let release = report.with_extension("release");
        let deadline = Instant::now() + Duration::from_secs(60);
        while !release.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    const PROBE_ENV: &str = "DARTQ_TEST_GATE_PROBE";

    /// Under cap 1 the session daemon holds the only job slot, and a
    /// `dartq/test/run` child joins it: it is admitted at once with no
    /// slot of its own (it never waits on its own parent). While the run
    /// lives the daemon stays busy (the gate never releases it) and never
    /// idles out, even with no client attached; once it ends the daemon
    /// is releasable and reaps itself.
    #[test]
    fn a_daemon_test_run_joins_its_slot_and_keeps_it_busy() {
        use std::os::unix::fs::PermissionsExt as _;
        if crate::tests::isolated(
            "session::tests::a_daemon_test_run_joins_its_slot_and_keeps_it_busy",
        ) {
            return;
        }
        let root = test_fixture("testrun");
        let report = root.join("probe.out");
        let release = report.with_extension("release");
        let probe = root.join("dartq-probe");
        let binary = std::env::current_exe().expect("test binary");
        std::fs::write(
            &probe,
            format!(
                "#!/bin/sh\nexec '{}' session::tests::test_run_gate_probe --exact --nocapture\n",
                binary.display()
            ),
        )
        .expect("probe script");
        std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o755))
            .expect("probe mode");
        // SAFETY: this child process runs this test alone.
        unsafe {
            std::env::set_var(ledger_cap_env(), "1");
            std::env::set_var("DARTQ_BIN", &probe);
            std::env::set_var(PROBE_ENV, &report);
        }
        let (dir, sdk) = (root.clone(), root.join("sdk"));
        let server = std::thread::spawn(move || {
            serve_loop_for_tests_with(&dir, &sdk, Duration::from_secs(1))
        });
        let layout = Layout::new(&root);
        let deadline = Instant::now() + Duration::from_secs(20);
        while !dartq_session::socket_open(&layout) {
            assert!(Instant::now() < deadline, "the serve loop never listened");
            std::thread::sleep(Duration::from_millis(50));
        }
        let started = Instant::now();
        let answer = roundtrip(
            &layout.socket,
            r#"{"jsonrpc":"2.0","id":1,"method":"dartq/test/run","params":{"args":[]}}"#,
        );
        assert!(answer.contains("run_id"), "the run started: {answer}");
        while !report.exists() {
            assert!(
                started.elapsed() < Duration::from_secs(20),
                "the probe never reported"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(std::fs::read_to_string(&report).expect("report"), "joined");
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "admitted without waiting for a slot"
        );
        // No client is attached, yet the daemon is busy and outlives its
        // idle window while the run lives.
        std::thread::sleep(Duration::from_secs(2));
        let busy = std::fs::File::open(layout.dir.join("busy.lock")).expect("busy lock");
        assert!(
            matches!(busy.try_lock(), Err(std::fs::TryLockError::WouldBlock)),
            "a live test run keeps the daemon unreleasable"
        );
        assert!(
            dartq_session::socket_open(&layout),
            "a live test run is never idle"
        );
        std::fs::write(&release, "").expect("release the probe");
        assert_eq!(server.join().expect("server thread"), exit::CLEAN);
        let _ = std::fs::remove_dir_all(&root);
    }

    fn ledger_cap_env() -> &'static str {
        dartq_toolchain::ledger::CAP_ENV
    }

    /// An unsized session daemon (no recorded peak, no peer to borrow
    /// from) holds the first-run slot only until its first analysis: while
    /// the long-lived client that triggered it stays attached (the MCP
    /// proxy's shape), its lease record is sized with its measured peak,
    /// the peak is recorded, and another worktree's unsized first run is
    /// admitted at once.
    #[test]
    fn a_session_daemon_is_sized_after_its_first_load() {
        use dartq_toolchain::ledger::{self, Face};
        if crate::tests::isolated("session::tests::a_session_daemon_is_sized_after_its_first_load")
        {
            return;
        }
        let _serial = SIGNAL_SERIALIZATION
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let root = test_fixture("sized");
        let (dir, sdk) = (root.clone(), root.join("sdk"));
        let server = std::thread::spawn(move || {
            serve_loop_for_tests_with(&dir, &sdk, Duration::from_secs(3))
        });
        let layout = Layout::new(&root);
        let deadline = Instant::now() + Duration::from_secs(20);
        while !dartq_session::socket_open(&layout) {
            assert!(Instant::now() < deadline, "the serve loop never listened");
            std::thread::sleep(Duration::from_millis(50));
        }
        let key = format!("session:{}", root.canonicalize().expect("canon").display());
        assert!(
            !ledger::sizing(&key, Face::Session).1,
            "an unsized first run"
        );
        let mut client = UnixStream::connect(&layout.socket).expect("connect");
        let answer = exchange(
            &mut client,
            r#"{"jsonrpc":"2.0","id":1,"method":"dartq/analyze","params":{}}"#,
        );
        assert!(answer.contains("\"id\":1"), "answered: {answer}");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let (bytes, sized) = ledger::sizing("session:another-worktree", Face::Session);
            if sized {
                assert!(bytes > 0, "the borrowed figure is the measured peak");
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the daemon's lease never turned sized"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(
            ledger::peak(&key).is_some(),
            "the peak is recorded once loaded"
        );
        // Still attached: another worktree's first run (a test run, a face
        // with no peak anywhere, in a process of its own) takes a slot at
        // once.
        let report = root.join("probe.out");
        let mut probe = std::process::Command::new(std::env::current_exe().expect("test binary"))
            .args([
                "session::tests::test_run_gate_probe",
                "--exact",
                "--nocapture",
            ])
            .env(PROBE_ENV, &report)
            .spawn()
            .expect("the probe");
        let deadline = Instant::now() + Duration::from_secs(20);
        while !report.exists() {
            assert!(Instant::now() < deadline, "the probe never reported");
            std::thread::sleep(Duration::from_millis(20));
        }
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(std::fs::read_to_string(&report).expect("report"), "slot");
        std::fs::write(report.with_extension("release"), "").expect("release the probe");
        assert!(probe.wait().expect("probe").success());
        drop(client);
        assert_eq!(server.join().expect("server thread"), exit::CLEAN);
        let _ = std::fs::remove_dir_all(&root);
    }

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
        let _serial = SIGNAL_SERIALIZATION
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let root = test_fixture("signal-held");
        let serve_dir = root.clone();
        let serve_sdk = root.join("sdk");
        let server = std::thread::spawn(move || {
            serve_loop(
                &ServeArgs {
                    dir: serve_dir,
                    sdk: serve_sdk,
                },
                Duration::from_secs(1),
            )
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
        assert!(
            !layout.socket.exists(),
            "the socket was removed at shutdown"
        );
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
        let _serial = SIGNAL_SERIALIZATION
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let root = test_fixture("beat-held");
        let serve_dir = root.clone();
        let serve_sdk = root.join("sdk");
        let server = std::thread::spawn(move || {
            serve_loop(
                &ServeArgs {
                    dir: serve_dir,
                    sdk: serve_sdk,
                },
                Duration::from_secs(1),
            )
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
                    assert!(
                        Instant::now() < deadline,
                        "the initial heartbeat never landed"
                    );
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
        let _serial = SIGNAL_SERIALIZATION
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // A root deep enough that `<root>/.dart_tool/dartq/session.sock`
        // blows the 104-byte sun_path: one long directory name under /tmp.
        let segment = "w".repeat(72);
        let root = PathBuf::from("/tmp").join(format!("dqs-long-{}-{segment}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("lib")).expect("dir");
        std::fs::create_dir_all(root.join(".dart_tool")).expect("dot dir");
        std::fs::write(
            root.join("pubspec.yaml"),
            "name: fx\nenvironment:\n  sdk: ^3.5.0\n",
        )
        .expect("pubspec");
        std::fs::write(
            root.join(".dart_tool/package_config.json"),
            r#"{"configVersion":2,"packages":[{"name":"fx","rootUri":"../","packageUri":"lib/","languageVersion":"3.5"}],"generator":"pub"}"#,
        )
        .expect("package config");
        let sdk = root.join("sdk");
        for (rel, body) in [
            ("version", "3.13.1\n"),
            (
                "lib/libraries.json",
                r#"{"vm":{"libraries":{"core":{"uri":"core/core.dart"}}}}"#,
            ),
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
            serve_loop(
                &ServeArgs {
                    dir: serve_dir,
                    sdk: serve_sdk,
                },
                Duration::from_secs(1),
            )
        });
        let deadline = Instant::now() + Duration::from_secs(20);
        while !dartq_session::socket_open(&layout) {
            assert!(
                Instant::now() < deadline,
                "the long-path session never listened"
            );
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
        assert!(
            answer.contains("\"id\":1"),
            "answered over the relocated socket: {answer}"
        );
        assert!(!local.exists(), "no socket was attempted at the long path");

        let _ = server.join();
        assert!(
            !layout.socket.exists(),
            "the reap removed the relocated socket"
        );
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
        assert!(
            answer.contains("\"id\":1"),
            "the engine still answers: {answer}"
        );
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

    /// #4105: the SDK root order a face that attaches resolves by — the
    /// flag wins, `$DART_SDK` next, then the live resident session's own
    /// triple, and only then the navigation error, which names all three
    /// ways. It sets `$DART_SDK`, so it runs alone in its own process
    /// ([`crate::tests::isolated`]), which starts with the variable unset.
    #[test]
    fn sdk_root_order_flag_then_env_then_resident_then_error() {
        if crate::tests::isolated(
            "session::tests::sdk_root_order_flag_then_env_then_resident_then_error",
        ) {
            return;
        }
        // A live session fixture: a listening socket plus the state file
        // naming this process with a fresh heartbeat — the same technique
        // the serve tests build placeholders with.
        let home = PathBuf::from("/tmp").join(format!("dqs-sdkorder-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let root = home.join("w");
        std::fs::create_dir_all(&root).expect("root dir");
        let sdk = root.join("sdk");
        std::fs::create_dir_all(&sdk).expect("sdk dir");
        let layout = Layout::new(&root);
        layout.create().expect("layout");
        let triple = SessionTriple::compute(&root, &sdk);
        let listener = UnixListener::bind(&layout.socket).expect("bind");
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if stream.is_err() {
                    break;
                }
            }
        });
        SessionState {
            pid: std::process::id(),
            heartbeat_ms: SessionState::now_ms(),
            triple: triple.clone(),
            debug_sessions: 0,
            used_ms: 0,
        }
        .write(&layout)
        .expect("state");

        // No flag, no env: the live session's SDK.
        assert_eq!(
            sdk_root_for(&root, None),
            Ok(sdk.canonicalize().expect("canonical sdk")),
            "the live session answers its own sdk root"
        );
        // The env outranks the resident session; the flag outranks both.
        // SAFETY: the test runs alone in its own process (`isolated`).
        unsafe {
            std::env::set_var("DART_SDK", "/env/sdk");
        }
        assert_eq!(sdk_root_for(&root, None), Ok(PathBuf::from("/env/sdk")));
        assert_eq!(
            sdk_root_for(&root, Some(Path::new("/flag"))),
            Ok(PathBuf::from("/flag")),
            "the flag still wins over the environment and the session"
        );
        // SAFETY: as above.
        unsafe {
            std::env::remove_var("DART_SDK");
        }

        // No flag, no env, no live session: the navigation error names
        // all three ways.
        let bare = home.join("bare");
        std::fs::create_dir_all(&bare).expect("bare dir");
        let error = sdk_root_for(&bare, None).expect_err("no sdk source");
        assert_eq!(
            error,
            "no Dart SDK root (pass --sdk PATH, set DART_SDK, or start a session with \
             `dartq session start --sdk PATH`)"
        );

        std::fs::remove_dir_all(&home).ok();
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
        let child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("sleep child");
        let state = SessionState {
            pid: child.id(),
            heartbeat_ms: SessionState::now_ms(),
            triple: triple.clone(),
            debug_sessions: 0,
            used_ms: 0,
        };
        state.write(&layout).expect("state");
        let record = Record {
            id: triple.identity(),
            state,
        };
        registry.upsert(&record).expect("upsert");
        assert!(layout.socket.exists() && layout.state.exists());
        // In production the serve process's parent is init (detached), so
        // termination reaps promptly; in the harness the test process is
        // the parent, and a zombie still answers kill(pid, 0) — the
        // boolean is therefore not asserted here, the delivery is.
        let _ = stop_one(&registry, &record);
        let mut child = child;
        let exited = child
            .try_wait()
            .expect("waitable")
            .expect("the SIGTERM reached the server");
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

    /// #2661: every serve-loop exit path funnels through `cleanup` —
    /// idle, signal, and accept_failed alike — so its unlink is the
    /// one fact that keeps a daemon from leaving its socket behind.
    #[test]
    fn cleanup_unlinks_the_socket_state_and_pointer() {
        let home = std::env::temp_dir().join(format!("dqs-cleanup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let root = home.join("w");
        let layout = Layout::new(&root);
        layout.create().expect("layout");
        std::fs::write(&layout.socket, b"").expect("socket file");
        std::fs::write(layout.dir.join("session.sock.point"), b"/relocated").expect("pointer");
        SessionState {
            pid: std::process::id(),
            heartbeat_ms: SessionState::now_ms(),
            triple: SessionTriple::compute(&root, &root),
            debug_sessions: 0,
            used_ms: 0,
        }
        .write(&layout)
        .expect("state");
        assert!(layout.socket.exists() && layout.state.exists());
        cleanup(&layout, &root);
        assert!(!layout.socket.exists(), "the socket was unlinked");
        assert!(!layout.state.exists(), "the state was removed");
        assert!(
            !layout.dir.join("session.sock.point").exists(),
            "the relocation pointer was removed"
        );
        std::fs::remove_dir_all(&home).ok();
    }
}

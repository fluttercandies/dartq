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

use dartq_session::{
    Ensured, EventLog, HEARTBEAT, IDLE_TIMEOUT, Layout, Record, Registry, SessionState,
    SessionTriple, ensure, pid_alive, terminate,
};

use crate::exit;

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
    Status,
    /// Stop the session of a worktree (or every one with `--all`).
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
}

#[derive(Args)]
struct StopArgs {
    /// The worktree; defaults to the current directory.
    #[arg(default_value = ".")]
    dir: PathBuf,
    /// Stop every live session in the registry.
    #[arg(long)]
    all: bool,
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
        SessionCommand::Status => status(),
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
/// stdio — the child outlives the starter by design.
pub(crate) fn spawn_server(layout: &Layout, triple: &SessionTriple) -> io::Result<()> {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("dartq"));
    Command::new(exe)
        .arg("session")
        .arg("serve")
        .arg("--dir")
        .arg(&triple.root)
        .arg("--sdk")
        .arg(&triple.sdk_root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|child| {
            // Reaped by init when the serve loop exits; the starter does
            // not wait.
            drop(child);
            let _ = layout;
        })
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
        println!("{{\"type\":\"swept\",\"root\":{}}}", json_path(&swept));
    }
    for evicted in registry.evict_past_cap() {
        println!("{{\"type\":\"evicted\",\"root\":{}}}", json_path(&evicted));
    }
    match ensure(&root, &sdk_root, &spawn_server) {
        Ensured::Attached(state) => {
            print_state(&state, true);
            exit::CLEAN
        }
        Ensured::Standalone(reason) => {
            println!("{{\"type\":\"standalone\",\"reason\":{}}}", serde_json::to_string(&reason).unwrap_or_default());
            exit::CLEAN
        }
    }
}

fn status() -> u8 {
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
            let package = serde_json::to_string(&progress.current_package)
                .unwrap_or_else(|_| "null".into());
            println!(
                "{{\"type\":\"progress\",\"root\":{},\"files_scanned\":{},\"current_package\":{}}}",
                json_path(&root),
                progress.files_scanned,
                package,
            );
        }
    }
    for record in live {
        let age = SessionState::now_ms().saturating_sub(record.state.heartbeat_ms) / 1000;
        println!(
            "{{\"type\":\"session\",\"root\":{},\"pid\":{},\"sdk\":{},\"binary\":{},\"heartbeat_age_s\":{},\"stale_binary\":{}}}",
            json_path(&record.state.triple.root),
            record.state.pid,
            json_path(&record.state.triple.sdk_root),
            json_path(Path::new(&record.state.triple.binary_hash)),
            age,
            record.state.triple.binary_hash != binary,
        );
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
        println!("{{\"type\":\"stopped\",\"count\":{stopped}}}");
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
    println!("{{\"type\":\"stopped\",\"root\":{}}}", json_path(&root));
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

fn json_path(path: &Path) -> String {
    serde_json::to_string(&path.display().to_string()).unwrap_or_default()
}

fn print_state(state: &SessionState, attached: bool) {
    println!(
        "{{\"type\":\"session\",\"attached\":{},\"root\":{},\"pid\":{},\"sdk\":{},\"binary\":{},\"socket\":{}}}",
        attached,
        json_path(&state.triple.root),
        state.pid,
        json_path(&state.triple.sdk_root),
        json_path(Path::new(&state.triple.binary_hash)),
        json_path(&Layout::new(&state.triple.root).socket),
    );
}

// ---------------------------------------------------------------------------
// The serve loop
// ---------------------------------------------------------------------------

/// The resident loop: bind the session socket, announce the state, and
/// serve daemon-protocol clients one at a time — the engine (the
/// session) persists across clients; each connection is a full
/// `dartq-daemon` serve until the client disconnects. Between
/// connections the loop wakes on a 250 ms tick to heartbeat the state
/// and to reap itself after [`IDLE_TIMEOUT`] with no client (no
/// self-pipe: the tick replaces it — the Windows posture, unix-native).
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
    heartbeat(&layout, &registry, &triple);

    let mut last_client = Instant::now();
    let mut next_beat = Instant::now() + HEARTBEAT;
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                last_client = Instant::now();
                let _ = stream.set_nonblocking(false);
                let _ = events.append("session", "client_attached", serde_json::json!({}));
                serve_client(stream, &mut session);
                let _ = events.append("session", "client_detached", serde_json::json!({}));
                heartbeat(&layout, &registry, &triple);
                next_beat = Instant::now() + HEARTBEAT;
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => {
                eprintln!("dartq: session serve: accept failed: {error}");
                cleanup(&layout, &root);
                return exit::SOFTWARE;
            }
        }
        let now = Instant::now();
        if now >= next_beat {
            heartbeat(&layout, &registry, &triple);
            next_beat = now + HEARTBEAT;
        }
        if now.duration_since(last_client) > idle_timeout {
            let _ = events.append("session", "reaped", serde_json::json!({"idle_s": idle_timeout.as_secs()}));
            cleanup(&layout, &root);
            return exit::CLEAN;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// One client connection: the daemon protocol until the peer closes.
fn serve_client(stream: UnixStream, session: &mut dartq_daemon::Session) {
    let Ok(read) = stream.try_clone() else {
        return;
    };
    // The read half is the clone; the original stream stays the write
    // half. run owns both and returns at the client's disconnect.
    let _ = dartq_daemon::run(read, stream, session, None);
}

/// Binds the socket, unlinking a leftover file once (a crashed server's
/// residue — the election normally cleans it, this is the belt).
fn bind_takeover(layout: &Layout) -> io::Result<UnixListener> {
    UnixListener::bind(&layout.socket).or_else(|first| {
        let _ = std::fs::remove_file(&layout.socket);
        UnixListener::bind(&layout.socket).map_err(|_| first)
    })
}

/// The liveness rewrite: state.json plus the registry record.
fn heartbeat(layout: &Layout, registry: &Registry, triple: &SessionTriple) {
    let state = SessionState {
        pid: std::process::id(),
        heartbeat_ms: SessionState::now_ms(),
        triple: triple.clone(),
    };
    if state.write(layout).is_ok() {
        let _ = registry.upsert(&Record { id: triple.identity(), state });
    }
}

/// The shutdown residue: socket, state, registry record.
fn cleanup(layout: &Layout, root: &Path) {
    let _ = std::fs::remove_file(&layout.socket);
    let _ = std::fs::remove_file(&layout.state);
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
        assert!(answer.contains("diagnostics"), "the analyze envelope: {answer}");
        // The state file names the server and its triple.
        let state = SessionState::read(&layout).expect("state");
        assert!(pid_alive(state.pid));
        assert_eq!(state.triple.root, root.canonicalize().expect("canon"));
        // Idle past the timeout: the loop cleans up and exits clean.
        let code = server.join().expect("server thread");
        assert_eq!(code, exit::CLEAN);
        assert!(!layout.socket.exists(), "the socket was removed at reap");
        assert!(!layout.state.exists(), "the state was removed at reap");
        std::fs::remove_dir_all(&root).ok();
    }
}

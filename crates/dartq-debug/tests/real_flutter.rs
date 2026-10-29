//! Live integration tests against a real `flutter run` process.
//!
//! Skipped by default: running them requires a real `flutter`
//! executable (set `DARTQ_FLUTTER_BIN`, put it on `PATH`, or have
//! `~/fvm/default/bin/flutter`), a macOS desktop target with its
//! toolchain, and spawns real apps. Run explicitly:
//!
//! ```text
//! cargo test -p dartq-debug --test real_flutter -- --ignored
//! ```
//!
//! The scenarios share one project scaffold (generated once from the
//! checked-in fixture under `tests/fixtures/flutter_app` by `flutter
//! create`, so the platform runner always matches the installed
//! Flutter) and serialize on a global mutex — one app instance at a
//! time, matching the single-instance debug discipline. The first
//! scenario pays the cold debug build (minutes); later ones reuse it.
//!
//! Together they walk the complete agent loop against Flutter:
//! paused-on-start takeover, breakpoints in widget/build code, frame
//! evaluation and object-graph expansion, hot reload (including a
//! broken edit that must fail as data while the session stays
//! usable), hot restart with breakpoint re-arm and state reset,
//! attach coexisting with the tool's own clients, detach without
//! killing, mid-session app death surfacing a clean terminal state,
//! and clean teardown from running and paused states.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use dartq_debug::{
    BreakpointLocationSpec, BreakpointSpec, DebugSession, Error, LaunchConfig, LaunchKind,
    SessionConfig, StoppedReason,
};

/// How long one scenario may run end to end (generous: the first cold
/// build of the macOS debug app can take minutes).
const SCENARIO_TIMEOUT: Duration = Duration::from_secs(600);

/// Finds the flutter executable: `DARTQ_FLUTTER_BIN`, then `PATH`,
/// then this host's fvm default. Panics with instructions when none
/// exists — these are opt-in tests, a missing toolchain is a skip
/// condition, not a failure of the crate.
fn flutter_bin() -> PathBuf {
    if let Some(explicit) = std::env::var_os("DARTQ_FLUTTER_BIN") {
        let path = PathBuf::from(explicit);
        assert!(path.is_file(), "DARTQ_FLUTTER_BIN={:?} is not a file", path);
        return path;
    }
    if let Some(paths) = std::env::var_os("PATH")
        && let Some(found) = std::env::split_paths(&paths)
            .map(|dir| dir.join("flutter"))
            .find(|candidate| candidate.is_file())
    {
        return found;
    }
    let fvm = fvm_default();
    if fvm.is_file() {
        return fvm;
    }
    panic!(
        "no flutter executable found: set DARTQ_FLUTTER_BIN or add flutter to PATH \
         (this host keeps it at ~/fvm/default/bin/flutter)"
    );
}

/// `~/fvm/default/bin/flutter` — the fvm-managed default.
fn fvm_default() -> PathBuf {
    std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join("fvm/default/bin/flutter"))
        .unwrap_or_default()
}

/// The one-off scenario lock: one flutter command, one app instance at
/// a time.
fn scenario_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// The shared project scaffold (created once per test process).
fn fixture() -> &'static Path {
    static FIXTURE: OnceLock<PathBuf> = OnceLock::new();
    FIXTURE.get_or_init(|| prepare_fixture())
}

/// Generates the project scaffold and lays the checked-in fixture
/// over the template. `flutter create --no-pub` keeps the template's
/// hosted dependencies (cupertino_icons, flutter_lints) out of the
/// resolution: the fixture's pubspec is SDK-only, so the whole suite
/// runs offline once the Flutter SDK cache is present.
fn prepare_fixture() -> PathBuf {
    let started = Instant::now();
    let root = std::env::temp_dir().join(format!("dartq-debug-flutter-e2e-{}", std::process::id()));
    let bin = flutter_bin();
    // SAFETY: this runs exactly once, inside `OnceLock::get_or_init`;
    // every test in this binary reaches any environment access only
    // after this initialization has completed (they all start at the
    // scenario lock / this fixture).
    unsafe { std::env::set_var("DARTQ_FLUTTER_BIN", &bin) };

    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/flutter_app");
    if !root.join("lib/main.dart").is_file() {
        run(
            Command::new(&bin)
                .args([
                    "create",
                    "--platforms=macos",
                    "--project-name",
                    "flutter_debug_fixture",
                    "--no-pub",
                ])
                .arg(&root),
            "flutter create",
        );
        std::fs::copy(source.join("pubspec.yaml"), root.join("pubspec.yaml"))
            .expect("copy fixture pubspec");
        std::fs::copy(source.join("lib/main.dart"), root.join("lib/main.dart"))
            .expect("copy fixture main.dart");
        let test_dir = root.join("test");
        if test_dir.is_dir() {
            std::fs::remove_dir_all(&test_dir).expect("remove template test dir");
        }
    }
    run(
        Command::new(&bin).arg("pub").arg("get").current_dir(&root),
        "flutter pub get",
    );
    eprintln!(
        "fixture ready at {} in {:.1}s",
        root.display(),
        started.elapsed().as_secs_f32()
    );
    root
}

/// Runs a preparation command to completion, panicking with its output
/// on failure.
fn run(command: &mut Command, what: &str) {
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("{what}: {error}"));
    assert!(
        output.status.success(),
        "{what} failed ({}):\n{}{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

/// The standard launch configuration for the scenarios.
fn launch_config(pause_on_start: bool) -> LaunchConfig {
    let root = fixture().to_path_buf();
    LaunchConfig {
        kind: LaunchKind::Flutter,
        device: Some("macos".into()),
        dart_defines: vec![("PROBE_TAG".into(), "e2e".into())],
        flutter_args: vec!["--no-devtools".into()],
        pause_on_start,
        launch_timeout: SCENARIO_TIMEOUT,
        cwd: Some(root.clone()),
        ..LaunchConfig::flutter(root.join("lib/main.dart"))
    }
}

/// The session settings for the scenarios (tighter than the defaults
/// so a wedged engine fails the test instead of hanging it).
fn settings() -> SessionConfig {
    SessionConfig {
        pause_timeout: Duration::from_secs(30),
        rpc_timeout: Duration::from_secs(30),
        ..SessionConfig::default()
    }
}

/// The app source as currently on disk.
fn main_source() -> String {
    std::fs::read_to_string(fixture().join("lib/main.dart")).expect("read fixture main.dart")
}

/// 1-based line number of the (first) line containing `needle` —
/// breakpoint targets are located by content so fixture edits cannot
/// shift them silently.
fn line_of(needle: &str) -> u32 {
    main_source()
        .lines()
        .position(|line| line.contains(needle))
        .map(|index| index as u32 + 1)
        .unwrap_or_else(|| panic!("fixture has no line containing {needle:?}"))
}

/// Replaces the (single) occurrence of `from` with `to` in the app
/// source — the edit phase of the reload scenarios.
fn edit_main(from: &str, to: &str) {
    let source = main_source();
    let count = source.matches(from).count();
    assert_eq!(count, 1, "fixture must contain {from:?} exactly once");
    std::fs::write(
        fixture().join("lib/main.dart"),
        source.replacen(from, to, 1),
    )
    .expect("write fixture main.dart");
}

/// Normalizes an editable fixture line to `to`, whichever variant is
/// on disk — scenario order is arbitrary, and a failed scenario may
/// leave an edit behind. Most-specific variants come first.
fn normalize_main(variants: &[&str], to: &str) {
    let source = main_source();
    for from in variants {
        let count = source.matches(from).count();
        match count {
            0 => continue,
            1 => {
                if *from != to {
                    std::fs::write(
                        fixture().join("lib/main.dart"),
                        source.replacen(from, to, 1),
                    )
                    .expect("write fixture main.dart");
                }
                return;
            }
            more => panic!("fixture contains {from:?} {more} times"),
        }
    }
    assert!(
        source.contains(to),
        "no known variant of the editable line; expected it to read {to:?}"
    );
}

/// The fixture's editable lines, normalized to their canonical text by
/// every scenario before use.
fn normalize_fixture() {
    normalize_main(
        &[
            "const String reloadTag = 'v4';",
            "const String reloadTag = 'v3';",
            "const String reloadTag = 'v2';",
            "const String reloadTag = 'v1';",
        ],
        "const String reloadTag = 'v1';",
    );
    normalize_main(
        &[
            "int probeEcho(int value) => value * 2 oops;",
            "int probeEcho(int value) => value * 3;",
            "int probeEcho(int value) => value * 2;",
        ],
        "int probeEcho(int value) => value * 2;",
    );
}

/// Waits until the session's captured stdout contains `needle`,
/// returning everything captured so far.
fn wait_for_output(session: &mut DebugSession, needle: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut cursor = 0_u64;
    let mut seen = Vec::new();
    loop {
        session.poll_events().expect("poll for output");
        let batch = session.stdout_since(cursor, usize::MAX);
        cursor = batch.next_cursor;
        seen.extend_from_slice(&batch.bytes);
        let text = String::from_utf8_lossy(&seen);
        if text.contains(needle) {
            return text.into_owned();
        }
        assert!(
            Instant::now() < deadline,
            "output {needle:?} never arrived; captured so far:\n{text}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Arms a line breakpoint on the fixture line containing `needle` and
/// returns its handle. The script URI is the VM's own spelling (from
/// `getScripts`): a Flutter kernel app presents its entrypoint as
/// `package:flutter_debug_fixture/main.dart`, not as a local `file://`
/// path, and an unmatched URI never resolves.
fn arm_line_breakpoint(session: &mut DebugSession, needle: &str) -> dartq_debug::BreakpointHandle {
    let script_uri = session
        .scripts(None)
        .ok()
        .and_then(|scripts| {
            scripts
                .into_iter()
                .find(|script| {
                    script.uri.as_deref().is_some_and(|uri| {
                        uri.contains("flutter_debug_fixture") && uri.ends_with("main.dart")
                    })
                })
                .and_then(|script| script.uri)
        })
        .unwrap_or_else(|| "package:flutter_debug_fixture/main.dart".into());
    session
        .add_breakpoint(BreakpointSpec {
            isolate: None,
            location: BreakpointLocationSpec::Line {
                script_uri,
                line: line_of(needle),
                column: None,
            },
            condition: None,
        })
        .unwrap_or_else(|error| panic!("add breakpoint on {needle:?}: {error}"))
}

/// `wait_for_pause` on the session's main isolate (borrow-friendly).
fn wait_for_pause_main(
    session: &mut DebugSession,
) -> dartq_debug::Result<dartq_debug::StoppedSnapshot> {
    let main = session.main_isolate().expect("main isolate").to_owned();
    session.wait_for_pause(&main)
}

/// PIDs of processes whose command line references the fixture root
/// (the built app and the tool's dart processes) — the orphan check
/// for teardown.
fn lingering_fixture_processes() -> Vec<u32> {
    let output = Command::new("pgrep")
        .arg("-f")
        .arg(fixture().display().to_string())
        .output()
        .expect("pgrep");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.trim().parse().ok())
        .collect()
}

/// Waits up to `timeout` for every fixture process to be gone.
fn assert_no_orphans(timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        let lingering = lingering_fixture_processes();
        if lingering.is_empty() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "fixture processes survived teardown: {lingering:?}"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// A `flutter run` spawned by the test itself, torn down by Drop even
/// on a panicking scenario — group-killed and reaped so no app, tool,
/// or DDS process survives the test.
struct OwnedRun {
    child: Option<Child>,
}

impl OwnedRun {
    /// Waits for the process to exit on its own within `grace`.
    fn wait_exit(&mut self, grace: Duration) {
        let deadline = Instant::now() + grace;
        while let Some(child) = self.child.as_mut() {
            match child.try_wait().expect("flutter run liveness") {
                Some(_) => {
                    self.child = None;
                    return;
                }
                None => {}
            }
            assert!(Instant::now() < deadline, "flutter run ignored teardown");
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// The graceful teardown: SIGTERM to the whole tree (the tool's
    /// cleanup handler stops the app), then the Drop guard as the
    /// backstop.
    #[cfg(unix)]
    fn term(&mut self) {
        if let Some(child) = self.child.as_ref() {
            unsafe { libc::kill(-(child.id() as i32), libc::SIGTERM) };
        }
        self.wait_exit(Duration::from_secs(60));
    }
}

impl Drop for OwnedRun {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            #[cfg(unix)]
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Spawns `flutter run` under the test's own management (the attach
/// scenario), drains both pipes on reader threads, and returns once
/// the "A Dart VM Service on … is available at:" banner carried a URI.
fn spawn_flutter_run_paused() -> (OwnedRun, String, Arc<Mutex<Vec<u8>>>) {
    let mut command = Command::new(flutter_bin());
    command
        .args([
            "run",
            "-d",
            "macos",
            "-t",
            "lib/main.dart",
            "--no-devtools",
            "--start-paused",
        ])
        .current_dir(fixture())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn().expect("spawn flutter run");
    let captured = Arc::new(Mutex::new(Vec::new()));
    let drain = |pipe: Box<dyn std::io::Read + Send>| {
        let sink = Arc::clone(&captured);
        std::thread::spawn(move || {
            for line in BufReader::new(pipe).lines().map_while(Result::ok) {
                let mut bytes = sink.lock().expect("capture lock");
                bytes.extend_from_slice(line.as_bytes());
                bytes.push(b'\n');
            }
        });
    };
    if let Some(stdout) = child.stdout.take() {
        drain(Box::new(stdout));
    }
    if let Some(stderr) = child.stderr.take() {
        drain(Box::new(stderr));
    }
    let deadline = Instant::now() + SCENARIO_TIMEOUT;
    let mut run = OwnedRun { child: Some(child) };
    loop {
        let banner = {
            let bytes = captured.lock().expect("capture lock");
            String::from_utf8_lossy(&bytes).to_string()
        };
        if let Some(uri) = banner.lines().find_map(|line| extract_service_uri(line)) {
            return (run, uri, captured);
        }
        if let Some(child) = run.child.as_mut() {
            match child.try_wait().expect("flutter run liveness") {
                Some(status) => panic!("flutter run died before the banner ({status}):\n{banner}"),
                None => {}
            }
        }
        assert!(
            Instant::now() < deadline,
            "no VM service banner from flutter run; captured:\n{banner}"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// The service URI of a `A Dart VM Service on <device> is available
/// at: <uri>` line.
fn extract_service_uri(line: &str) -> Option<String> {
    let rest = line.strip_prefix("A Dart VM Service on ")?;
    let separator = rest.rfind(" is available at: ")?;
    Some(
        rest[separator + " is available at: ".len()..]
            .trim()
            .to_owned(),
    )
}

/// Converts the banner http URI to its WebSocket form.
fn banner_to_ws(uri: &str) -> String {
    uri.trim_end_matches('/').replacen("http", "ws", 1) + "/ws"
}

/// The paused-on-start takeover: launch, arm breakpoints while the app
/// holds at start, then release it into the breakpoints — the standard
/// agent loop — plus frame evaluation and object-graph expansion from
/// a widget build frame.
#[test]
#[ignore = "requires a real flutter toolchain and macOS desktop device"]
fn launch_takeover_and_build_frame_inspection() {
    let _guard = scenario_lock().lock().expect("scenario lock");
    normalize_fixture();
    let mut session = DebugSession::launch_with(launch_config(true), settings())
        .unwrap_or_else(|error| panic!("flutter launch failed: {error}"));

    assert!(!session.ws_uri().is_empty(), "ws uri recorded");
    assert!(
        session.flutter_tool_attached(),
        "flutter tool behind the service"
    );
    let main = session.main_isolate().expect("main isolate").to_owned();
    // The tool wraps the service in DDS; the VM (and its system
    // isolates) are reachable alongside the app's user isolate.
    assert!(
        session.isolates().any(|isolate| isolate.is_system_isolate),
        "system isolates tracked"
    );

    // Paused-on-start takeover: the app holds until this session
    // releases it.
    let start = session.wait_for_pause(&main).expect("start pause");
    assert_eq!(start.reason, StoppedReason::Start);

    // Arm while held, then run into it.
    let handle = arm_line_breakpoint(&mut session, "final label =");
    let hit = session
        .step_and_package(None, None)
        .expect("resume into breakpoint");
    match &hit.reason {
        StoppedReason::Breakpoint { breakpoints } => assert!(
            breakpoints.iter().any(|id| id == &handle.id),
            "expected hit on {}, got {breakpoints:?}",
            handle.id
        ),
        other => panic!("expected breakpoint hit, got {other:?}"),
    }
    assert!(
        hit.location
            .script_uri
            .as_deref()
            .is_some_and(|uri| uri.ends_with("main.dart")),
        "stopped in the app source: {:?}",
        hit.location.script_uri
    );

    // State inspection from a widget build frame: `this` is the State
    // object; its fields expand lazily.
    let this = session
        .evaluate_in_frame(None, 0, "this")
        .expect("evaluate this in build frame");
    let state_handle = match this {
        dartq_debug::VmObject::Instance(instance) => instance.id,
        other => panic!("expected the State instance, got {other:?}"),
    };
    let fields = session.expand(None, &state_handle).expect("expand state");
    assert!(
        fields.iter().any(|row| row.name == "ticks"),
        "state fields expand: {fields:?}"
    );
    let ticks = session
        .evaluate_in_frame(None, 0, "this.ticks")
        .expect("evaluate this.ticks");
    match ticks {
        dartq_debug::VmObject::Instance(instance) => {
            assert_eq!(instance.value_as_string.as_deref(), Some("0"));
        }
        other => panic!("expected Int, got {other:?}"),
    }

    // One step lands somewhere sane and still paused.
    let stepped = session
        .step_and_package(None, Some(dartq_debug::StepOption::Over))
        .expect("step over in build");
    assert_eq!(stepped.reason, StoppedReason::Step);

    // Disarm before letting the app run: the ticker rebuilds this
    // build every second, and a live breakpoint here would pause the
    // app before the awaited output could ever print.
    session
        .remove_breakpoint(None, &handle.id)
        .expect("remove breakpoint");
    session.resume(None).expect("final resume");
    // The dart-define rode the launch; the app's startup marker and
    // the ticker prove the running app's output stream.
    let output = wait_for_output(&mut session, "probe:tag=e2e");
    assert!(output.contains("build:title=probe-v1"), "in {output}");
    wait_for_output(&mut session, "tick:1");

    // Clean shutdown from the running state leaves nothing behind.
    session.terminate().expect("terminate from running");
    drop(session);
    assert_no_orphans(Duration::from_secs(30));
}

/// Hot reload follows the tool's orchestration (DevFS push +
/// reassemble), a broken edit fails as data while the session stays
/// alive and usable, and breakpoints outside the build path survive
/// reloads.
#[test]
#[ignore = "requires a real flutter toolchain and macOS desktop device"]
fn hot_reload_failure_recovery_and_breakpoint_survival() {
    let _guard = scenario_lock().lock().expect("scenario lock");
    normalize_fixture();
    let mut session = DebugSession::launch_with(launch_config(false), settings())
        .unwrap_or_else(|error| panic!("flutter launch failed: {error}"));
    wait_for_output(&mut session, "probe:tag=e2e");

    // A source edit the tool must push: only the tool-orchestrated
    // reload carries it, and only the reassemble rebuild runs
    // ProbeApp.build again.
    edit_main(
        "const String reloadTag = 'v1';",
        "const String reloadTag = 'v2';",
    );
    let reloaded = session.reload_sources(None, false).expect("reload v2");
    assert!(reloaded, "reload v2 reported success");
    wait_for_output(&mut session, "build:title=probe-v2");

    // A reload storm: back-to-back reloads without waiting between
    // them — the tool serializes them and the app stays usable.
    session.poll_events().expect("cursor before burst");
    let burst_cursor = session.stdout_since(0, usize::MAX).next_cursor;
    for (from, to) in [
        (
            "const String reloadTag = 'v2';",
            "const String reloadTag = 'v3';",
        ),
        (
            "const String reloadTag = 'v3';",
            "const String reloadTag = 'v4';",
        ),
    ] {
        edit_main(from, to);
        let reloaded = session.reload_sources(None, false).expect("burst reload");
        assert!(reloaded, "burst reload {to} reported success");
    }
    wait_for_output_from(&mut session, burst_cursor, "build:title=probe-v4");

    // Let the reload settle before arming: a breakpoint added into the
    // IsolateReload window can be left unresolved forever (the VM
    // matches pending URI breakpoints at script load, and a reloaded
    // script does not re-trigger that matching). One ticker line means
    // the post-reload code is running.
    session.poll_events().expect("cursor after burst");
    let settle_cursor = session.stdout_since(0, usize::MAX).next_cursor;
    wait_for_output_from(&mut session, settle_cursor, "tick:");

    // A breakpoint outside the build path, hit by the ticker.
    let handle = arm_line_breakpoint(&mut session, "int probeEcho(int value)");
    let hit = match wait_for_pause_main(&mut session) {
        Ok(hit) => hit,
        Err(error) => {
            let bps = session
                .breakpoints()
                .map(|bp| format!("{}:resolved={}", bp.id, bp.resolved))
                .collect::<Vec<_>>()
                .join(",");
            panic!("echo hit failed: {error}; breakpoints {bps}");
        }
    };
    match &hit.reason {
        StoppedReason::Breakpoint { breakpoints } => assert!(breakpoints.contains(&handle.id)),
        other => panic!("expected echo breakpoint hit, got {other:?}"),
    }
    session.resume(None).expect("resume from echo hit");

    // A broken edit must fail as data, not kill the session.
    edit_main(
        "int probeEcho(int value) => value * 2;",
        "int probeEcho(int value) => value * 2 oops;",
    );
    let broken = session
        .reload_sources(None, false)
        .expect_err("broken reload");
    assert!(
        matches!(broken, Error::Rpc { .. }),
        "compile failure surfaces as an RPC error, got {broken:?}"
    );

    // The session survived: fix the edit, reload, and the same
    // breakpoint still hits — hot reload keeps debug state.
    edit_main(
        "int probeEcho(int value) => value * 2 oops;",
        "int probeEcho(int value) => value * 3;",
    );
    let fixed = session
        .reload_sources(None, false)
        .expect("reload after fix");
    assert!(fixed, "reload after fix reported success");
    let hit = wait_for_pause_main(&mut session).expect("echo re-hit");
    match &hit.reason {
        StoppedReason::Breakpoint { breakpoints } => assert!(
            breakpoints.contains(&handle.id),
            "breakpoint survived the reloads: {breakpoints:?}"
        ),
        other => panic!("expected the same breakpoint to re-hit, got {other:?}"),
    }

    session.remove_breakpoint(None, &handle.id).expect("remove");
    session.resume(None).expect("final resume");
    session.terminate().expect("terminate");
    drop(session);
    assert_no_orphans(Duration::from_secs(30));
}

/// Hot restart resets app state the way the tool's own `R` does, and
/// the session re-arms its breakpoints against the post-restart
/// isolate (the tool removes the old ones itself; re-arming is the
/// client's job by design).
#[test]
#[ignore = "requires a real flutter toolchain and macOS desktop device"]
fn hot_restart_re_arms_breakpoints_and_resets_state() {
    let _guard = scenario_lock().lock().expect("scenario lock");
    normalize_fixture();
    let mut session = DebugSession::launch_with(launch_config(true), settings())
        .unwrap_or_else(|error| panic!("flutter launch failed: {error}"));
    let main = session.main_isolate().expect("main isolate").to_owned();
    let start = session.wait_for_pause(&main).expect("start pause");
    assert_eq!(start.reason, StoppedReason::Start);

    // Arm while held and run into it once — proof the pre-restart
    // breakpoint works from a launch takeover.
    let pre = arm_line_breakpoint(&mut session, "final label =");
    let hit = session
        .step_and_package(None, None)
        .expect("resume into the pre-restart hit");
    match &hit.reason {
        StoppedReason::Breakpoint { breakpoints } => assert!(breakpoints.contains(&pre.id)),
        other => panic!("expected the pre-restart breakpoint hit, got {other:?}"),
    }

    let outcome = session.hot_restart().expect("hot restart");
    assert_eq!(
        outcome.re_armed.len(),
        1,
        "the breakpoint re-armed: {outcome:?}"
    );
    assert!(outcome.dropped.is_empty(), "nothing dropped: {outcome:?}");
    assert_eq!(
        session.breakpoints().count(),
        1,
        "the session tracks the re-arm"
    );
    assert!(
        session.main_isolate().is_some(),
        "a post-restart main isolate exists"
    );

    // The re-armed breakpoint hits again (a ticker rebuild — the very
    // first frame after restart races the re-arm by design).
    let hit = wait_for_pause_main(&mut session).expect("re-armed hit");
    match &hit.reason {
        StoppedReason::Breakpoint { breakpoints } => assert!(
            outcome
                .re_armed
                .iter()
                .any(|re| breakpoints.contains(&re.id)),
            "a re-armed breakpoint hit: {breakpoints:?}"
        ),
        other => panic!("expected a re-armed breakpoint hit, got {other:?}"),
    }

    // State reset: the ticker started over. Read only output captured
    // after the restart returned. The re-armed breakpoint re-hits on
    // the NEXT build, so one tick's window is the assertion.
    session.poll_events().expect("poll");
    let cursor = session.stdout_since(0, usize::MAX).next_cursor;
    session.resume(None).expect("resume after restart hit");
    let post_restart = wait_for_output_from(&mut session, cursor, "tick:1");
    assert!(
        post_restart.contains("home-build:ticks 0"),
        "in {post_restart}"
    );

    // Clean shutdown from the paused state (whichever pause the app is
    // in when we ask — the re-armed breakpoint pauses it every build).
    session.pause(None).expect("pause for shutdown");
    let paused = wait_for_pause_main(&mut session).expect("paused for shutdown");
    assert!(matches!(
        paused.reason,
        StoppedReason::Breakpoint { .. } | StoppedReason::Interrupted
    ));
    session.terminate().expect("terminate from paused");
    drop(session);
    assert_no_orphans(Duration::from_secs(30));
}

/// Waits until output captured strictly after `cursor` contains
/// `needle` (the hot-restart scenario must not match pre-restart
/// lines).
fn wait_for_output_from(session: &mut DebugSession, mut cursor: u64, needle: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut seen = Vec::new();
    loop {
        session.poll_events().expect("poll for output");
        let batch = session.stdout_since(cursor, usize::MAX);
        cursor = batch.next_cursor;
        seen.extend_from_slice(&batch.bytes);
        let text = String::from_utf8_lossy(&seen);
        if text.contains(needle) {
            return text.into_owned();
        }
        assert!(
            Instant::now() < deadline,
            "output {needle:?} (after cursor) never arrived; captured:\n{text}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Attaching to a `flutter run` owned by another process works over
/// the WebSocket alone (the tool's stdin stays the owner's), takes
/// over a paused-on-start app, and detaching leaves the app running.
#[test]
#[ignore = "requires a real flutter toolchain and macOS desktop device"]
#[cfg(unix)]
fn attach_coexists_and_detaches_without_killing() {
    let _guard = scenario_lock().lock().expect("scenario lock");
    normalize_fixture();
    let (mut run, uri, _captured) = spawn_flutter_run_paused();
    let ws = banner_to_ws(&uri);
    let mut session = DebugSession::attach_with(&ws, settings())
        .unwrap_or_else(|error| panic!("attach to flutter run: {error}"));

    assert!(session.flutter_tool_attached(), "the tool is a co-client");
    let main = session.main_isolate().expect("main isolate").to_owned();
    let start = session
        .wait_for_pause(&main)
        .expect("start pause over attach");
    assert_eq!(start.reason, StoppedReason::Start);

    // A second concurrent dartq session on the same app: DDS holds
    // both, and either can act.
    let mut second =
        DebugSession::attach_with(&ws, settings()).expect("second concurrent session attaches");
    assert!(
        second.main_isolate().is_some(),
        "the co-session sees isolates"
    );
    assert!(
        second.flutter_tool_attached(),
        "the co-session sees the tool"
    );
    drop(second);

    // Full takeover through the attach path: arm while held, release,
    // hit in build code.
    let handle = arm_line_breakpoint(&mut session, "final label =");
    let hit = session
        .step_and_package(None, None)
        .expect("resume into breakpoint over attach");
    match &hit.reason {
        StoppedReason::Breakpoint { breakpoints } => assert!(breakpoints.contains(&handle.id)),
        other => panic!("expected breakpoint hit over attach, got {other:?}"),
    }
    let label = session
        .evaluate_in_frame(None, 0, "this.ticks")
        .expect("evaluate while the tool co-exists");
    assert!(matches!(label, dartq_debug::VmObject::Instance(_)));
    session.resume(None).expect("resume");

    // Detach: the connection closes, the app (and its owner) keep
    // running.
    session.detach();
    drop(session);
    std::thread::sleep(Duration::from_secs(3));
    assert!(
        run.child.is_some(),
        "detached flutter run must keep running (the Drop guard has not reaped it)"
    );

    // Teardown of the externally-owned run.
    run.term();
    drop(run);
    assert_no_orphans(Duration::from_secs(30));
}

/// An app that dies mid-session (the in-app equivalent of a crash or
/// device loss) surfaces a clean terminal state — a typed error, not
/// a hang — and the session tears down without orphans.
#[test]
#[ignore = "requires a real flutter toolchain and macOS desktop device"]
fn app_exit_mid_session_surfaces_clean_terminal_state() {
    let _guard = scenario_lock().lock().expect("scenario lock");
    normalize_fixture();
    let mut session = DebugSession::launch_with(launch_config(true), settings())
        .unwrap_or_else(|error| panic!("flutter launch failed: {error}"));
    let main = session.main_isolate().expect("main isolate").to_owned();
    let start = session.wait_for_pause(&main).expect("start pause");
    assert_eq!(start.reason, StoppedReason::Start);
    session.resume(None).expect("release the app");
    wait_for_output(&mut session, "tick:1");

    // Pause, arm the in-app exit hook, release it: the app exits a
    // moment later, mid-session.
    session.pause(None).expect("pause");
    session.wait_for_pause(&main).expect("paused");
    session
        .evaluate_in_root_lib(None, "scheduleExit(77)")
        .expect("arm the exit hook");
    session.resume(None).expect("release into the exit");

    // The wait for a pause ends in a terminal state — the process or
    // isolate dying, or the exit pause — bounded, never a hang.
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        match session.wait_for_pause(&main) {
            Err(Error::ProcessExited(_)) | Err(Error::IsolateExited(_)) => break,
            Err(Error::Timeout(_)) if Instant::now() >= deadline => {
                panic!("mid-session exit never surfaced as a terminal state")
            }
            Ok(snapshot) if snapshot.reason == StoppedReason::Exit => break,
            other => {
                let text = format!("{other:?}");
                assert!(
                    Instant::now() < deadline,
                    "expected a terminal state, got stuck on {text}"
                );
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }

    // Telemetry keeps draining; terminate is a clean no-op-plus-reap.
    session.poll_events().expect("poll after death");
    session.terminate().expect("terminate after death");
    drop(session);
    assert_no_orphans(Duration::from_secs(30));
}

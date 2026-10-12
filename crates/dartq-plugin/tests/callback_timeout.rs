//! Callback-budget enforcement (PRD §14.3 CPU row): a `request/fixes` that
//! overruns the manifest's `callback_ms` kills the child and consumes one
//! restart at the error site; repeated overruns exhaust the budget into the
//! terminal [`RuntimeError::Crashed`], which [`crash_diagnostic`] turns into
//! the `dartq/plugin_crashed` diagnostic the output layer merges.

use std::path::{Path, PathBuf};
use std::time::Instant;

use dartq_plugin::{
    Manifest, PluginRuntime, RuntimeError, SpawnSpec, StdioPolicy, crash_diagnostic,
};
use dartq_source::FileTable;

mod support;

use support::dart_available;

fn host_package_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../dartq-plugin-host")
}

#[test]
#[ignore = "spawns real dart processes; run in the serial window via plugin-live (DARTQ_M4_LIVE=1)"]
fn dart_plugin_over_callback_budget_is_killed_and_restarted() {
    if !dart_available() {
        println!("skipping: no `dart` on PATH, the live fixture cannot run");
        return;
    }
    let manifest = Manifest::parse(
        r#"{"name": "slow", "version": "0.1.0", "entrypoint": "bin/slow_plugin.dart",
            "capabilities": ["lint", "fix"],
            "timeouts": {"callback_ms": 500, "request_ms": 120000}}"#,
    )
    .expect("manifest parses");
    let mut runtime = PluginRuntime::spawn(SpawnSpec {
        program: "dart".to_owned(),
        args: vec!["bin/slow_plugin.dart".to_owned()],
        current_dir: host_package_dir(),
        manifest,
        stderr: StdioPolicy::Inherit,
        serve_queries: false,
    })
    .expect("dart host spawns and handshakes");

    let started = Instant::now();
    let outcome = runtime.request_fixes("lib/main.dart", 10, 2);
    let error = outcome.expect_err("a stalled callback must hit the kill path");
    assert!(
        matches!(&error, RuntimeError::CallbackTimeout { ms } if *ms == 500),
        "the callback budget surfaces, got {error:?}"
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(90),
        "the kill must not wait out the plugin's full stall"
    );

    // The kill consumed one restart and left a fresh, handshakeable child.
    let stats = runtime.stats();
    assert_eq!(
        stats.restarts, 1,
        "the timeout restart is observable: {stats:?}"
    );
    assert!(runtime.negotiated().is_some());

    // The failure converts to the dartq/ timeout diagnostic.
    let mut files = FileTable::new();
    let anchor = files.intern(Path::new("lib/main.dart"));
    let diagnostic = crash_diagnostic("slow", &error, anchor);
    assert_eq!(diagnostic.code.name, "dartq/plugin_timeout");
    assert!(diagnostic.message.contains("slow"));

    // The fresh generation answers shutdown; nothing leaks.
    runtime
        .shutdown()
        .expect("graceful shutdown of the fresh child");
}

/// A cheap `sh` child that answers the handshake and stalls every later
/// request, so the restart budget can be burned end-to-end without paying
/// for a Dart VM per generation.
#[cfg(unix)]
#[test]
#[ignore = "spawns real dart processes; run in the serial window via plugin-live (DARTQ_M4_LIVE=1)"]
fn repeated_callback_overruns_exhaust_the_budget_into_a_terminal_crash() {
    let script = concat!(
        r#"read line; "#,
        r#"printf '%s\n' '{"schema_version":1,"id":1,"method":"initialize","params":{"protocol_version":1,"capabilities":["fix"]}}'; "#,
        // The stall redirects its (inherited) stdout: an orphaned sleep must
        // not hold the pipe's write end, or the killed generation's reader
        // waits out the full sleep before it sees EOF.
        r#"while read line; do sleep 5 >/dev/null; printf '%s\n' '{"schema_version":1,"id":2,"method":"request/fixes","params":{"fixes":[]}}'; done"#,
    );
    let manifest = Manifest::parse(
        r#"{"name": "staller", "version": "0.1.0", "entrypoint": "bin/nothing",
            "timeouts": {"callback_ms": 200, "request_ms": 5000, "request_retries": 2}}"#,
    )
    .expect("manifest parses");
    let mut runtime = PluginRuntime::spawn(SpawnSpec {
        program: "sh".to_owned(),
        args: vec!["-c".to_owned(), script.to_owned()],
        current_dir: Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf(),
        manifest,
        stderr: StdioPolicy::Null,
        serve_queries: false,
    })
    .expect("the sh child handshakes");

    // Each overrun kills one generation and consumes one restart; the third
    // request finds the budget spent and fails terminally.
    let first = runtime.request_fixes("lib/main.dart", 0, 0);
    assert!(
        matches!(&first, Err(RuntimeError::CallbackTimeout { .. })),
        "{first:?}"
    );
    let second = runtime.request_fixes("lib/main.dart", 0, 0);
    assert!(
        matches!(&second, Err(RuntimeError::CallbackTimeout { .. })),
        "{second:?}"
    );
    let third = runtime.request_fixes("lib/main.dart", 0, 0);
    let terminal = third.expect_err("budget spent");
    assert!(
        matches!(&terminal, RuntimeError::Crashed { reason } if reason.contains("restart budget")),
        "terminal failure names the exhausted budget: {terminal:?}"
    );
    assert_eq!(runtime.stats().restarts, 2);

    let mut files = FileTable::new();
    let anchor = files.intern(Path::new("lib/main.dart"));
    let diagnostic = crash_diagnostic("staller", &terminal, anchor);
    assert_eq!(diagnostic.code.name, "dartq/plugin_crashed");
    assert!(diagnostic.message.contains("staller"));
}

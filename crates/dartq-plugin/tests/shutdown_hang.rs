//! Shutdown teardown bound: the reply budget bounds the whole teardown, not
//! just the protocol exchange. A plugin that answers `shutdown` and then
//! hangs in a cleanup handler never closes its stdout — the reader thread
//! stays parked on it — so the pre-fix teardown (join the reader, then reap
//! the child) waited out the hang and wedged the caller forever. The
//! reply-ack path now kills the child before joining: the session is
//! already Terminated, nothing of value is lost.
//!
//! Unix-gated: the child is an `sh` one-liner, deliberately not a Dart
//! process — startup speed and determinism are the point (same posture as
//! `crash_restart.rs`).

use std::path::Path;
use std::time::{Duration, Instant};

use dartq_plugin::{Manifest, PluginRuntime, SpawnSpec, StdioPolicy};

/// Answers the handshake, then on the next frame answers `shutdown` and
/// hangs in a (simulated) cleanup handler. The sleep's stdout is redirected:
/// an orphaned `sleep` must not hold the pipe's write end after the kill, or
/// the killed generation's reader waits out the full sleep before it sees
/// EOF (same discipline as `write_timeout.rs`).
const SCRIPT: &str = concat!(
    "read line; ",
    r#"printf '%s\n' '{"schema_version":1,"id":1,"method":"initialize","params":{"protocol_version":1,"capabilities":[]}}'; "#,
    "while read line; do ",
    r#"printf '%s\n' '{"schema_version":1,"id":2,"method":"shutdown","params":{}}'; "#,
    "sleep 60 >/dev/null; ",
    "done",
);

#[cfg(unix)]
#[test]
fn shutdown_kills_a_plugin_that_hangs_after_answering() {
    let manifest = Manifest::parse(
        r#"{"name": "hang", "version": "0.1.0", "entrypoint": "bin/nothing",
            "timeouts": {"request_ms": 10000}}"#,
    )
    .expect("manifest parses");
    let runtime = PluginRuntime::spawn(SpawnSpec {
        program: "sh".to_owned(),
        args: vec!["-c".to_owned(), SCRIPT.to_owned()],
        current_dir: Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf(),
        manifest,
        stderr: StdioPolicy::Null,
        serve_queries: false,
    })
    .expect("the sh child handshakes");

    let started = Instant::now();
    let outcome = runtime.shutdown();
    let elapsed = started.elapsed();
    assert!(
        matches!(outcome, Ok(())),
        "the answered shutdown succeeds, got {outcome:?}"
    );
    // The reply arrives in milliseconds; the bound leaves CI slack while
    // still failing the pre-fix teardown, which waited out the full 60 s
    // cleanup hang (or blocked forever on a never-exiting plugin).
    assert!(
        elapsed < Duration::from_secs(10),
        "the teardown must not wait out the plugin's cleanup hang, took {elapsed:?}"
    );
}

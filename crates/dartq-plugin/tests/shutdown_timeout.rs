//! Shutdown timeout: `shutdown` awaits its reply under
//! the request budget; a plugin that never answers is killed at the budget
//! and the consuming call surfaces [`RuntimeError::ShutdownTimeout`] with the
//! enforced budget. The kill consumes no restart — the runtime ends with the
//! session — and never waits out the silent child. (The answered-reply face
//! with a post-reply hang lives in `shutdown_hang.rs`.)
//!
//! Unix-gated: the child is an `sh` one-liner (same posture as
//! `shutdown_hang.rs`).
#![cfg(unix)]

use std::path::Path;
use std::time::{Duration, Instant};

use dartq_plugin::{Manifest, PluginRuntime, RuntimeError, SpawnSpec, StdioPolicy};

#[test]
fn shutdown_without_a_reply_hits_the_budget_and_kills() {
    // Answers the handshake, then never reads again: nothing answers the
    // shutdown request. The sleep's stdout is redirected so an orphaned
    // `sleep` could never hold the pipe's write end (same discipline as
    // `write_timeout.rs`).
    let script = concat!(
        "read line; ",
        r#"printf '%s\n' '{"schema_version":1,"id":1,"method":"initialize","params":{"protocol_version":1,"capabilities":[]}}'; "#,
        "sleep 30 >/dev/null",
    );
    let manifest = Manifest::parse(
        r#"{"name": "unanswered", "version": "0.1.0", "entrypoint": "bin/nothing",
            "timeouts": {"request_ms": 400}}"#,
    )
    .expect("manifest parses");
    let runtime = PluginRuntime::spawn(SpawnSpec {
        program: "sh".to_owned(),
        args: vec!["-c".to_owned(), script.to_owned()],
        current_dir: Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf(),
        manifest,
        stderr: StdioPolicy::Null,
        serve_queries: false,
    })
    .expect("the sh child handshakes");

    // Before the call: the kill is enforcement, not a crash — no restart is
    // budgeted for it, and only the handshake frame ever crossed.
    let stats = runtime.stats();
    assert_eq!(
        stats.restarts, 0,
        "the shutdown kill is not a restart: {stats:?}"
    );
    assert_eq!(stats.frames_out, 1, "only initialize crossed: {stats:?}");

    let started = Instant::now();
    let outcome = runtime.shutdown();
    assert!(
        matches!(&outcome, Err(RuntimeError::ShutdownTimeout { ms }) if *ms == 400),
        "the typed timeout with the enforced budget surfaces, got {outcome:?}"
    );
    // The reply budget is 400 ms; the bound fails any wait-out of the 30 s
    // child.
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "the kill must land at the budget, not wait out the silent child"
    );
}

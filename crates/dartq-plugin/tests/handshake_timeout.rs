//! Handshake timeout: a child that never answers
//! `initialize` is killed at the request budget, and every relaunch attempt
//! inside the manifest's retry budget hits the same silence. What `spawn`
//! surfaces at the end is the typed [`RuntimeError::HandshakeTimeout`] with
//! the enforced budget — the failure never renames into a crash, and no
//! attempt waits out the silent child.
//!
//! Unix-gated: the child is an `sh` one-liner, deliberately not a Dart
//! process (same posture as `crash_restart.rs`).
#![cfg(unix)]

use std::path::Path;
use std::time::{Duration, Instant};

use dartq_plugin::{Manifest, PluginRuntime, RuntimeError, SpawnSpec, StdioPolicy};

#[test]
fn silent_child_burns_the_handshake_budget_into_the_typed_timeout() {
    // Reads nothing, writes nothing, stays alive far past every budget: each
    // attempt must be killed at its deadline, never waited out.
    let manifest = Manifest::parse(
        r#"{"name": "silent", "version": "0.1.0", "entrypoint": "bin/nothing",
            "timeouts": {"request_ms": 500, "request_retries": 2}}"#,
    )
    .expect("manifest parses");
    let started = Instant::now();
    let outcome = PluginRuntime::spawn(SpawnSpec {
        program: "sh".to_owned(),
        args: vec!["-c".to_owned(), "sleep 30".to_owned()],
        current_dir: Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf(),
        manifest,
        stderr: StdioPolicy::Null,
        serve_queries: false,
    });
    let error = outcome.expect_err("a silent child cannot complete the handshake");
    assert!(
        matches!(&error, RuntimeError::HandshakeTimeout { ms, .. } if *ms == 500),
        "the typed timeout with the enforced budget surfaces, got {error:?}"
    );
    // Three attempts (initial + 2 retries) cost 3 x 500 ms of budget; the
    // bound fails any wait-out of the 30 s child (one would already blow it).
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "every silent attempt must be killed at the budget, not waited out"
    );
}

//! wait_diagnostics on an already-dead generation: the eager restart gate
//! runs BEFORE the park, so a child that died since the last interaction
//! yields a typed outcome immediately — the terminal crash when the restart
//! budget is spent — instead of parking out the caller's whole timeout on a
//! corpse. (A death DURING the park is the documented opposite: the park runs
//! to the deadline and the final alive gate restarts there.)
//!
//! Unix-gated: the child is an `sh` one-liner (same posture as
//! `crash_restart.rs`).
#![cfg(unix)]

use std::path::Path;
use std::thread::sleep;
use std::time::{Duration, Instant};

use dartq_plugin::{Manifest, PluginRuntime, RuntimeError, SpawnSpec, StdioPolicy};

#[test]
fn wait_on_a_dead_generation_with_a_spent_budget_is_typed_and_early() {
    // Handshakes, then exits: the reader EOFs, the death slot records the
    // close, and with a zero restart budget the death is terminal.
    let script = concat!(
        "read line; ",
        r#"printf '%s\n' '{"schema_version":1,"id":1,"method":"initialize","params":{"protocol_version":1,"capabilities":[]}}'"#,
    );
    let manifest = Manifest::parse(
        r#"{"name": "gone", "version": "0.1.0", "entrypoint": "bin/nothing",
            "timeouts": {"request_ms": 5000, "request_retries": 0}}"#,
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
    sleep(Duration::from_millis(400)); // the child exits; the death lands

    // A 10 s drain budget on a corpse must not park: the death gate errors
    // out before the wait begins.
    let started = Instant::now();
    let outcome = runtime.wait_output(Duration::from_secs(10));
    let error = outcome.expect_err("a dead generation with a spent budget cannot be waited on");
    assert!(
        matches!(&error, RuntimeError::Crashed { reason }
            if reason.contains("closed the connection")),
        "the death slot's reason surfaces through the wait gate: {error:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "the gate precedes the park, took {:?}",
        started.elapsed()
    );
}

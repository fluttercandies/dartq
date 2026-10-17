//! Analyze clock close→reopen: a demand
//! satisfied by produced diagnostics closes the clock, and the NEXT fed batch
//! opens a fresh, still-enforced demand. If the close were sticky, a plugin
//! that answers exactly one cycle and then stalls would be exempt from the
//! callback budget forever; the monitor must fire on the reopened demand
//! through the ordinary crash path.
//!
//! (The clock's storage contract — open-once, no refresh on re-feed, closed
//! on demand — is pinned as a unit test inside `runtime.rs`; this suite pins
//! the same machine end-to-end, across the restart the enforcement triggers.)
//!
//! Unix-gated: the child is an `sh` one-liner (same posture as
//! `analyze_timeout.rs`).
#![cfg(unix)]

use std::path::Path;
use std::time::{Duration, Instant};

use dartq_diagnostics::{
    CodeInfo, Diagnostic, DiagnosticType, Severity as EngineSeverity, SourceRange,
};
use dartq_plugin::{CyclePush, Manifest, PluginRuntime, RuntimeError, SpawnSpec, StdioPolicy};
use dartq_source::{FileId, FileTable};

/// The same shape the daemon feeds on the analyze path.
fn engine_row(file: FileId, message: &'static str) -> Diagnostic {
    Diagnostic::new(
        EngineSeverity::Warning,
        CodeInfo::new("test/row", "TEST_ROW", DiagnosticType::Tool, false),
        message,
        file,
        SourceRange {
            offset: 0,
            length: 2,
        },
        None,
    )
}

#[test]
fn a_satisfied_demand_reopens_and_stays_enforced() {
    // Answers the handshake, produces exactly one diagnostic for the first
    // fed batch (closing the demand's clock), then drains silently forever.
    // Every relaunch reruns the same script, so the relaunched generation
    // answers its first demand too and stalls on the next — the reopen and
    // the enforcement are pinned twice over.
    let script = concat!(
        "read line; ",
        r#"printf '%s\n' '{"schema_version":1,"id":1,"method":"initialize","params":{"protocol_version":1,"capabilities":["lint"]}}'; "#,
        "read line; ",
        r#"printf '%s\n' '{"schema_version":1,"id":null,"method":"notification/diagnostic","params":{"file":"lib/main.dart","severity":"warning","code":"once/only","message":"burst one","offset":0,"length":0}}'; "#,
        "cat >/dev/null",
    );
    let manifest = Manifest::parse(
        r#"{"name": "once", "version": "0.1.0", "entrypoint": "bin/nothing",
            "capabilities": ["lint"],
            "timeouts": {"callback_ms": 300, "request_ms": 5000, "request_retries": 1}}"#,
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

    let mut files = FileTable::new();
    let anchor = files.intern(Path::new("lib/main.dart"));
    let batch = [engine_row(anchor, "first demand")];

    // Cycle 1: the batch crosses, the diagnostic closes the clock, and the
    // burst drains as the satisfied demand it is.
    runtime
        .feed(&batch, &files, &CyclePush::default())
        .expect("the first batch crosses");
    let drained = runtime
        .wait_output(Duration::from_secs(5))
        .expect("the answered cycle drains");
    assert_eq!(
        drained.diagnostics.len(),
        1,
        "the clock-closing diagnostic arrived: {:?}",
        drained.diagnostics
    );
    assert_eq!(drained.diagnostics[0].message, "burst one");

    // Cycle 2: the re-opened demand meets silence — the monitor must fire at
    // the callback budget through the ordinary crash path. A sticky (never
    // reopened) clock would keep this loop running green forever.
    let started = Instant::now();
    let mut outcome = Ok(());
    for _ in 0..100 {
        outcome = runtime
            .feed(&batch, &files, &CyclePush::default())
            .and_then(|()| runtime.wait_output(Duration::from_millis(50)).map(|_| ()));
        if outcome.is_err() {
            break;
        }
    }
    let terminal = outcome.expect_err("the stalled second demand must die terminally");
    assert!(
        matches!(&terminal, RuntimeError::Crashed { reason }
            if reason.contains("analyze callback budget")),
        "the reopened demand is enforced: {terminal:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(30),
        "the kill rides the monitor tick, not the caller's loop"
    );
    assert_eq!(
        runtime.stats().restarts,
        1,
        "the second demand's overrun consumed the budget: {:?}",
        runtime.stats()
    );

    // Teardown is the Drop kill: the terminal generation is already dead and
    // no fresh child exists to answer shutdown.
    drop(runtime);
}

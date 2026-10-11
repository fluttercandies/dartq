//! Death-slot attribution across a natural (EOF) child death: a generation
//! that dies after a fed batch — before producing diagnostics — records
//! "closed the connection" and closes the analyze clock with it. A monitor
//! tick landing between the EOF and that close used to leave the arm firing
//! on the corpse, overwriting the death slot; idling past `callback_ms` after
//! the death then made the next interaction's `Crashed` blame the analyze
//! budget instead of the real cause. The reason must stay the truth: the
//! connection closed.
//!
//! (The enforcement itself — the budget arm against a live, silent child —
//! is pinned by `analyze_clock_reopen.rs`; this suite pins that a dead
//! generation is not enforcement material.)
//!
//! Unix-gated: the child is an `sh` one-liner (same posture as
//! `analyze_clock_reopen.rs`).
#![cfg(unix)]

use std::path::Path;
use std::time::Duration;

use dartq_diagnostics::{
    CodeInfo, Diagnostic, DiagnosticType, Severity as EngineSeverity, SourceRange,
};
use dartq_plugin::{Manifest, PluginRuntime, RuntimeError, SpawnSpec, StdioPolicy};
use dartq_source::{FileId, FileTable};

/// The same shape the daemon feeds on the analyze path.
fn engine_row(file: FileId, message: &str) -> Diagnostic {
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
fn a_natural_death_keeps_its_reason_past_the_callback_budget() {
    // Answers the handshake, consumes the fed batch, then falls off the end:
    // a natural death with the analyze clock still open and nothing produced
    // — the exact corpse the budget arm used to fire on.
    let script = concat!(
        "read line; ",
        r#"printf '%s\n' '{"schema_version":1,"id":1,"method":"initialize","params":{"protocol_version":1,"capabilities":["lint"]}}'; "#,
        "read line",
    );
    let manifest = Manifest::parse(
        r#"{"name": "eof", "version": "0.1.0", "entrypoint": "bin/nothing",
            "capabilities": ["lint"],
            "timeouts": {"callback_ms": 200, "request_ms": 5000, "request_retries": 0}}"#,
    )
    .expect("manifest parses");
    let mut runtime = PluginRuntime::spawn(SpawnSpec {
        program: "sh".to_owned(),
        args: vec!["-c".to_owned(), script.to_owned()],
        current_dir: Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf(),
        manifest,
        stderr: StdioPolicy::Null,
    })
    .expect("the sh child handshakes");

    let mut files = FileTable::new();
    let anchor = files.intern(Path::new("lib/main.dart"));
    let batch = [engine_row(anchor, "the fatal batch")];

    runtime
        .feed(&batch, &files)
        .expect("the batch crosses before the death");

    // Idle past the callback budget (+ several monitor ticks): with the clock
    // left open on the corpse, the budget arm fired in this window and
    // overwrote the death slot. The clock closes with the reader's EOF, so
    // the arm has nothing to fire on.
    std::thread::sleep(Duration::from_millis(700));

    // The next interaction is terminal (retries: 0) and must name the real
    // cause, not the analyze budget.
    let terminal = runtime
        .feed(&batch, &files)
        .expect_err("a spent restart budget surfaces the death");
    assert!(
        matches!(&terminal, RuntimeError::Crashed { reason }
            if reason.contains("closed the connection")),
        "the natural death keeps its own attribution: {terminal:?}"
    );
    assert!(
        matches!(&terminal, RuntimeError::Crashed { reason }
            if !reason.contains("analyze callback budget")),
        "the budget arm must not fire on a dead generation: {terminal:?}"
    );
    assert_eq!(
        runtime.stats().restarts,
        0,
        "the corpse cost no restart: {:?}",
        runtime.stats()
    );

    // Teardown is the Drop kill: the terminal generation is already dead and
    // no fresh child exists to answer shutdown.
    drop(runtime);
}

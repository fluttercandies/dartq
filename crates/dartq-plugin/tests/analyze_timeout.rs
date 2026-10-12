//! Analyze-demand CPU enforcement (PRD §14.3 CPU row, analyze path): a
//! plugin that consumes every fed batch but never produces
//! `notification/diagnostic` keeps the demand's clock open; the monitor
//! kills it at the manifest's `callback_ms`, the death record names the
//! budget, and the restart consumption rides the ordinary crash path.
#![cfg(unix)]

use std::path::Path;
use std::time::{Duration, Instant};

use dartq_diagnostics::{
    CodeInfo, Diagnostic, DiagnosticType, Severity as EngineSeverity, SourceRange,
};
use dartq_plugin::{
    CyclePush, Manifest, PluginRuntime, RuntimeError, SpawnSpec, StdioPolicy, crash_diagnostic,
};
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

/// A cheap `sh` child that answers the handshake, then drains stdin forever
/// without ever sending `notification/diagnostic`: the demand's clock opens
/// at the first fed batch and never closes, so the monitor — not the
/// caller's loop — is what kills the generation.
#[test]
fn analyze_demand_without_diagnostics_is_killed_at_the_callback_budget() {
    let script = concat!(
        r#"read line; "#,
        r#"printf '%s\n' '{"schema_version":1,"id":1,"method":"initialize","params":{"protocol_version":1,"capabilities":["lint"]}}'; "#,
        r#"cat >/dev/null"#,
    );
    let manifest = Manifest::parse(
        r#"{"name": "silence", "version": "0.1.0", "entrypoint": "bin/nothing",
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
    let row = [engine_row(
        anchor,
        "context for a plugin that never answers",
    )];

    // The daemon's cycle shape: feed, wait a short drain budget, repeat. The
    // demand stays open across cycles (a re-feed must not refresh the
    // clock), so the waits keep returning empty until the monitor fires.
    let started = Instant::now();
    let mut outcome = Ok(());
    for _ in 0..100 {
        outcome = runtime
            .feed(&row, &files, &CyclePush::default())
            .and_then(|()| runtime.wait_output(Duration::from_millis(50)).map(|_| ()));
        if outcome.is_err() {
            break;
        }
    }
    let terminal = outcome.expect_err("the never-producing demand must die terminally");
    assert!(
        matches!(&terminal, RuntimeError::Crashed { reason }
            if reason.contains("analyze callback budget")),
        "the death record names the analyze budget: {terminal:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(30),
        "the kill rides the monitor tick, not the caller's loop"
    );
    // One overrun consumed the whole budget of one (initial generation plus
    // one eager restart); the counters survive the terminal crash.
    assert_eq!(
        runtime.stats().restarts,
        1,
        "the overrun's restart consumption is observable"
    );

    // The failure is the ordinary crash face for the output layer.
    let diagnostic = crash_diagnostic("silence", &terminal, anchor);
    assert_eq!(diagnostic.code.name, "dartq/plugin_crashed");
    assert!(diagnostic.message.contains("silence"));

    // Teardown is the Drop kill: the terminal generation is already dead and
    // no fresh child exists to answer shutdown.
    drop(runtime);
}

//! Crash isolation: child death is counted and absorbed within the restart
//! budget, then surfaces as a terminal [`RuntimeError::Crashed`] — the
//! runtime never loops forever on a deterministically broken plugin, and the
//! engine's own state never follows the plugin down (PRD §14.3).
//!
//! Unix-gated: the children are `sh` one-liners, deliberately not Dart
//! processes — startup speed and determinism are the point.

use std::path::Path;
use std::time::Duration;

use dartq_diagnostics::{Diagnostic, Severity, SourceRange};
use dartq_plugin::{Manifest, PluginRuntime, RuntimeError, SpawnSpec, StdioPolicy};
use dartq_source::FileTable;

fn spec(script: &str, retries: u32) -> SpawnSpec {
    let manifest = Manifest::parse(&format!(
        r#"{{"name": "dies", "version": "0.1.0", "entrypoint": "bin/nothing",
            "timeouts": {{"request_ms": 5000, "request_retries": {retries}}}}}"#
    ))
    .expect("manifest parses");
    SpawnSpec {
        program: "sh".to_owned(),
        args: vec!["-c".to_owned(), script.to_owned()],
        current_dir: Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf(),
        manifest,
        stderr: StdioPolicy::Null,
    }
}

fn one_diagnostic(files: &mut FileTable) -> Vec<Diagnostic> {
    let file = files.intern(Path::new("lib/main.dart"));
    vec![Diagnostic::new(
        Severity::Info,
        dartq_diagnostics::URI_DOES_NOT_EXIST,
        "irrelevant",
        file,
        SourceRange {
            offset: 0,
            length: 0,
        },
        None,
    )]
}

#[cfg(unix)]
#[test]
fn startup_death_burns_the_budget_and_reports_crashed() {
    let outcome = PluginRuntime::spawn(spec("exit 1", 2))
        .expect_err("a child that dies at startup cannot handshake");
    assert!(
        matches!(outcome, RuntimeError::Crashed { .. }),
        "terminal failure is Crashed, got {outcome:?}"
    );
}

#[cfg(unix)]
#[test]
fn mid_session_death_restarts_within_budget_then_reports_crashed() {
    // A well-behaved child: answers the handshake, lives 300 ms, dies.
    // Every generation does, so feeds keep hitting fresh restarts until the
    // budget (2) is spent.
    let script = concat!(
        r#"read line; "#,
        r#"printf '%s\n' '{"schema_version":1,"id":1,"method":"initialize","params":{"protocol_version":1,"capabilities":[]}}'; "#,
        "sleep 0.3",
    );
    let mut runtime = PluginRuntime::spawn(spec(script, 2)).expect("the sh child handshakes");

    let mut files = FileTable::new();
    let engine = one_diagnostic(&mut files);

    // One death absorbed per sleep: each generation lives 300 ms, so the
    // waits guarantee the next feed observes a dead child and restarts.
    runtime
        .feed(&engine, &files)
        .expect("push on the first generation");
    std::thread::sleep(Duration::from_millis(400));
    runtime
        .feed(&engine, &files)
        .expect("push after the first restart");
    std::thread::sleep(Duration::from_millis(400));
    runtime
        .feed(&engine, &files)
        .expect("push after the second restart");
    std::thread::sleep(Duration::from_millis(400));

    // Budget spent: the next interaction is terminal, never an endless loop.
    let outcome = runtime.feed(&engine, &files);
    assert!(
        matches!(&outcome, Err(RuntimeError::Crashed { .. })),
        "post-budget failure is Crashed, got {outcome:?}"
    );
    let stats = runtime.stats();
    assert_eq!(stats.restarts, 2, "every child death is counted: {stats:?}");

    // Shutdown of a dead runtime reports the crash; it never restarts.
    let outcome = runtime.shutdown();
    assert!(matches!(outcome, Err(RuntimeError::Crashed { .. })));
}

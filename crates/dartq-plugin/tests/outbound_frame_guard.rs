//! Outbound frame guard (PRD §14.3, the write-side twin of the inbound
//! decoder gate): a batch whose frame would exceed [`MAX_FRAME_SIZE`] is
//! rejected at the handle with the same typed protocol error before any byte
//! reaches the child. The child stays alive, no restart is consumed, and the
//! drop is counted. Without the guard the same batch kills the child's
//! decoder, relaunches into the identical batch, and dies again — a
//! deterministic restart loop that only ends in a budget-exhausted crash
//! blaming the plugin.

use std::path::{Path, PathBuf};

use dartq_diagnostics::{Diagnostic, Severity as EngineSeverity, SourceRange, URI_DOES_NOT_EXIST};
use dartq_plugin::{
    Manifest, PluginRuntime, RuntimeError, SpawnSpec, StdioPolicy, MAX_FRAME_SIZE, ProtocolError,
};
use dartq_source::{FileId, FileTable};

mod support;

use support::dart_available;

fn host_package_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../dartq-plugin-host")
}

fn warning(message: &str, file: FileId) -> Diagnostic {
    Diagnostic::new(
        EngineSeverity::Warning,
        URI_DOES_NOT_EXIST,
        message.to_owned(),
        file,
        SourceRange {
            offset: 0,
            length: 0,
        },
        None,
    )
}

/// A Dart host that handshakes, answers each engine batch with a plugin
/// finding, and exits on the stdin EOF — the live control that proves the
/// rejected batch left a working plugin behind (the crate's echo fixture,
/// shared with `e2e_round_trip.rs`).
#[test]
fn dart_host_survives_an_over_limit_outbound_batch() {
    if !dart_available() {
        println!("skipping: no `dart` on PATH, the live fixture cannot run");
        return;
    }
    let manifest = Manifest::parse(
        r#"{"name": "echo", "version": "0.1.0", "entrypoint": "bin/echo_plugin.dart",
            "capabilities": ["lint"],
            "timeouts": {"callback_ms": 60000, "request_ms": 10000}}"#,
    )
    .expect("manifest parses");
    let runtime = PluginRuntime::spawn(SpawnSpec {
        program: "dart".to_owned(),
        args: vec!["bin/echo_plugin.dart".to_owned()],
        current_dir: host_package_dir(),
        manifest,
        stderr: StdioPolicy::Inherit,
    })
    .expect("dart host spawns and handshakes");
    run_guard_scenarios(runtime);
}

/// A cheap `sh` child with the same contract: handshake, drain silently,
/// answer `shutdown` only.
#[cfg(unix)]
#[test]
fn sh_host_survives_an_over_limit_outbound_batch() {
    let script = concat!(
        r#"read line; "#,
        r#"printf '%s\n' '{"schema_version":1,"id":1,"method":"initialize","params":{"protocol_version":1,"capabilities":["lint"]}}'; "#,
        // Drain notifications silently; the shutdown request is the only
        // line that gets an answer (its reply id is the session's second).
        r#"while read line; do case "$line" in *'"method":"shutdown"'*) printf '%s\n' '{"schema_version":1,"id":2,"method":"shutdown","params":{}}';; esac; done"#,
    );
    let manifest = Manifest::parse(
        r#"{"name": "drain", "version": "0.1.0", "entrypoint": "bin/nothing",
            "capabilities": ["lint"],
            "timeouts": {"callback_ms": 60000, "request_ms": 10000}}"#,
    )
    .expect("manifest parses");
    let runtime = PluginRuntime::spawn(SpawnSpec {
        program: "sh".to_owned(),
        args: vec!["-c".to_owned(), script.to_owned()],
        current_dir: Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf(),
        manifest,
        stderr: StdioPolicy::Null,
    })
    .expect("the sh child handshakes");
    run_guard_scenarios(runtime);
}

/// The guard contract, run against any live host: the over-limit batch is a
/// typed rejection, not a crash; the child never sees it and no restart is
/// burned; a following small batch crosses the wire; the same generation
/// retires gracefully. Consumes the runtime — `shutdown` is the final
/// exchange.
fn run_guard_scenarios(mut runtime: PluginRuntime) {
    let mut files = FileTable::new();
    let anchor = files.intern(&std::env::temp_dir().join("dartq_outbound_guard_test.dart"));

    // One diagnostic whose JSON frame (content plus terminator) exceeds the
    // 16 MiB wire bound — exactly the batch a naive write would loop on.
    let oversized = [warning(&"a".repeat(MAX_FRAME_SIZE), anchor)];
    let error = runtime
        .feed(&oversized, &files)
        .expect_err("an over-limit outbound batch must be rejected");
    assert!(
        matches!(
            &error,
            RuntimeError::Wire(ProtocolError::TooLarge { limit }) if *limit == MAX_FRAME_SIZE
        ),
        "the rejection is the typed protocol violation, not a write or crash error: {error:?}"
    );

    // Nothing crossed the wire and nobody died: the handshake frame is the
    // only outbound frame, the drop is counted, and the restart loop the
    // guard exists to break never started.
    let stats = runtime.stats();
    assert_eq!(stats.frames_out, 1, "only initialize went out: {stats:?}");
    assert_eq!(stats.frames_dropped, 1, "the drop is counted: {stats:?}");
    assert_eq!(stats.restarts, 0, "no child was killed: {stats:?}");

    // The same runtime keeps working: a small batch crosses the wire.
    let small = [warning("fits", anchor)];
    runtime
        .feed(&small, &files)
        .expect("a batch under the limit still crosses");
    assert_eq!(runtime.stats().frames_out, 2);

    // ...and the same, still-live generation retires gracefully. On the
    // Dart host this also pins the shutdown-window tolerance: the echo
    // plugin answers the small batch with a finding that legitimately
    // crosses the shutdown request on the wire (the host had not seen the
    // request when it wrote it) — a crash here means in-flight diagnostics
    // are being rejected as violations again.
    runtime
        .shutdown()
        .expect("the child survived the rejected batch");
}

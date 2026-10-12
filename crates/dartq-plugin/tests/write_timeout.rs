//! Write-side deadline (the writer-thread protocol): a child that stops
//! draining its input pipe cannot block the runtime forever — once the 64
//! KiB pipe fills, the stdin write blocks the writer thread, the request
//! deadline fires, and the stall is treated as a protocol violation: kill
//! path, death recorded, crash surfaced.
//!
//! Unix-gated: the child is an `sh` one-liner that reads exactly one line
//! (the handshake) and then stops reading.

use std::path::Path;

use dartq_diagnostics::{Diagnostic, Severity, SourceRange, URI_DOES_NOT_EXIST};
use dartq_plugin::{CyclePush, Manifest, PluginRuntime, RuntimeError, SpawnSpec, StdioPolicy};
use dartq_source::FileTable;

#[cfg(unix)]
#[test]
fn stdin_write_into_a_stopped_reader_hits_the_deadline_and_kills() {
    let script = concat!(
        r#"read line; "#,
        r#"printf '%s\n' '{"schema_version":1,"id":1,"method":"initialize","params":{"protocol_version":1,"capabilities":[]}}'; "#,
        // The stall redirects its (inherited) stdout: an orphaned sleep must
        // not hold the pipe's write end, or the killed generation's reader
        // waits out the full sleep before it sees EOF.
        "sleep 30 >/dev/null",
    );
    // request_ms doubles as the write deadline: 200 ms after the pipe fills,
    // the stall is a protocol violation.
    let manifest = Manifest::parse(
        r#"{"name": "undrained", "version": "0.1.0", "entrypoint": "bin/nothing",
            "timeouts": {"request_ms": 200}}"#,
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

    // A batch well past the 64 KiB pipe buffer: the write must stall, hit
    // the deadline, and surface as the crash it is — never block forever.
    let mut files = FileTable::new();
    let file = files.intern(Path::new("lib/main.dart"));
    let engine: Vec<Diagnostic> = (0..1_000)
        .map(|i| {
            Diagnostic::new(
                Severity::Info,
                URI_DOES_NOT_EXIST,
                format!("Target of URI doesn't exist: 'missing_{i}.dart'."),
                file,
                SourceRange {
                    offset: 0,
                    length: 0,
                },
                None,
            )
        })
        .collect();
    let outcome = runtime.feed(&engine, &files, &CyclePush::default());
    let error = outcome.expect_err("a non-draining child must hit the write deadline");
    assert!(
        matches!(&error, RuntimeError::Crashed { reason } if reason.contains("input pipe")),
        "the write stall surfaces as a crash naming the pipe: {error:?}"
    );

    // Only the handshake write acked; the stalled frame never did, and the
    // kill restarts lazily (no restart has been paid yet).
    let stats = runtime.stats();
    assert_eq!(
        stats.frames_out, 1,
        "the stalled write never acked: {stats:?}"
    );
    assert_eq!(stats.restarts, 0);

    // The next interaction restarts within the budget: one small frame feeds
    // cleanly through a fresh generation's handshake. (No shutdown here —
    // the regenerated script sleeps instead of answering; Drop reaps it.)
    let single = vec![engine[0].clone()];
    runtime
        .feed(&single, &files, &CyclePush::default())
        .expect("a fresh generation feeds");
    assert_eq!(runtime.stats().restarts, 1);
}

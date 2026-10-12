//! Memory-budget enforcement (PRD §14.3 memory row): a child whose RSS
//! sample exceeds the manifest's `memory_budget_mb` is killed by the
//! monitor thread, the death record names the budget, and the breach rides
//! the ordinary crash path — one restart absorbed, the next interaction
//! terminal, and [`crash_diagnostic`] turns it into the
//! `dartq/plugin_crashed` diagnostic the output layer merges.
#![cfg(unix)]

use std::path::Path;
use std::time::{Duration, Instant};

use dartq_diagnostics::{Diagnostic, Severity, SourceRange, URI_DOES_NOT_EXIST};
use dartq_plugin::{
    CyclePush, Manifest, PluginRuntime, RuntimeError, SpawnSpec, StdioPolicy, crash_diagnostic,
};
use dartq_source::FileTable;

/// Answers the handshake, then balloons past any small budget: the command
/// substitution holds a ~133 MB base64 string in the shell's own RSS
/// (100 MB of NUL bytes encode to 133 MB of text), then the shell stays
/// alive so the monitor — not an exit — ends the generation. The trailing
/// sleep redirects its stdout (an orphan must not hold the pipe's write
/// end, or the killed generation's reader waits out the full sleep before
/// it sees EOF — same discipline as `write_timeout.rs`), and the
/// background `cat` drains stdin so the test's feeds never trip the write
/// deadline: the only armable breach in this suite is memory.
const SCRIPT: &str = concat!(
    "read line; ",
    r#"printf '%s\n' '{"schema_version":1,"id":1,"method":"initialize","params":{"protocol_version":1,"capabilities":[]}}'; "#,
    "cat >/dev/null & ",
    "v=$(head -c 100000000 /dev/zero | base64); ",
    "sleep 30 >/dev/null",
);

#[test]
fn rss_over_the_memory_budget_is_killed_and_reported_as_the_memory_row() {
    // callback_ms is deliberately out of reach (the kill lands within a few
    // seconds of the first allocation): the only row this test can arm is
    // memory, so the death record's wording is pinned deterministically.
    let manifest = Manifest::parse(
        r#"{"name": "balloon", "version": "0.1.0", "entrypoint": "bin/nothing",
            "timeouts": {"callback_ms": 30000, "request_ms": 5000, "request_retries": 1},
            "memory_budget_mb": 32}"#,
    )
    .expect("manifest parses");
    let mut runtime = PluginRuntime::spawn(SpawnSpec {
        program: "sh".to_owned(),
        args: vec!["-c".to_owned(), SCRIPT.to_owned()],
        current_dir: Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf(),
        manifest,
        stderr: StdioPolicy::Null,
        serve_queries: false,
    })
    .expect("the sh child handshakes");
    let memory_budget_bytes: u64 = 32 * 1024 * 1024;

    let mut files = FileTable::new();
    let anchor = files.intern(Path::new("lib/main.dart"));
    let row = [Diagnostic::new(
        Severity::Info,
        URI_DOES_NOT_EXIST,
        "context for a child whose only breach is memory",
        anchor,
        SourceRange {
            offset: 0,
            length: 0,
        },
        None,
    )];

    // The daemon's cycle shape: feed small batches until enforcement lands.
    // Every feed succeeds while some generation is alive; the over-budget
    // kill absorbs the manifest's one restart, and the next interaction
    // finds the budget spent and surfaces the terminal crash.
    let started = Instant::now();
    let mut outcome = Ok(());
    for _ in 0..400 {
        outcome = runtime.feed(&row, &files, &CyclePush::default());
        if outcome.is_err() {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    let terminal =
        outcome.expect_err("an over-budget child must die terminally inside the feed loop");
    assert!(
        started.elapsed() < Duration::from_secs(30),
        "the kill rides the monitor tick, not the caller's loop"
    );
    assert!(
        matches!(&terminal, RuntimeError::Crashed { reason }
            if reason.contains("memory budget")),
        "the death record names the memory row: {terminal:?}"
    );

    // The §14.3 memory counters: the monitor recorded the over-budget
    // sample as the peak before killing, and the kill consumed exactly the
    // one manifest restart.
    let stats = runtime.stats();
    assert!(
        stats.peak_rss_bytes > memory_budget_bytes,
        "the over-budget sample is the recorded peak: {stats:?}"
    );
    assert_eq!(stats.restarts, 1, "one absorbed restart, then terminal");

    // The ordinary crash face for the output layer: one tool diagnostic,
    // naming the plugin and the memory row.
    let diagnostic = crash_diagnostic("balloon", &terminal, anchor);
    assert_eq!(diagnostic.code.name, "dartq/plugin_crashed");
    assert_eq!(diagnostic.severity, Severity::Error);
    assert!(
        diagnostic.message.contains("balloon") && diagnostic.message.contains("memory budget"),
        "message names the plugin and the memory row: {}",
        diagnostic.message
    );

    // Teardown is the Drop kill: the terminal generation is already dead
    // and no fresh child exists to answer shutdown.
    drop(runtime);
}

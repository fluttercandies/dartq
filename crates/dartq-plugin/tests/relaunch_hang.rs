//! Relaunch teardown bound: a descendant that inherits the pipe's write end
//! keeps it open past the kill, so the killed generation's reader never sees
//! EOF and the pre-fix relaunch path (`ensure_alive` → `teardown_generation`)
//! joined that reader for as long as the descendant lived — one budgeted
//! enforcement kill (RSS, callback, analyze, write-stall, or a natural death
//! the analyze budget converts into one) and every later interaction hung
//! past any budget (§14.3 isolation breach). Two bounds close the wedge: the
//! child is spawned as its own Unix process group and every kill signals the
//! group (descendants die with it, the pipe closes), and the teardown's
//! joins are bounded — a reader that still has not seen EOF is detached and
//! the relaunch proceeds.
//!
//! The fixture mirrors `shutdown_hang.rs`: an `sh` one-liner, deliberately
//! not a Dart process. Unlike that fixture's `sleep 60 >/dev/null` (whose
//! redirect keeps an orphan from holding the pipe), here the sleep's
//! INHERITED stdout is the wedge under test.

use std::path::Path;
use std::time::{Duration, Instant};

use dartq_diagnostics::{Diagnostic, Severity, SourceRange, URI_DOES_NOT_EXIST};
use dartq_plugin::{CyclePush, Manifest, PluginRuntime, SpawnSpec, StdioPolicy};
use dartq_source::FileTable;

/// Answers the handshake, then on the next frame spawns `sleep 60` with the
/// inherited stdout (no redirect — the survivor holding the write end IS the
/// wedge) and exits. The reader sees no EOF until the sleep exits, so the
/// generation looks alive until the analyze callback budget fires and the
/// monitor kills it.
const SCRIPT: &str = concat!(
    "read line; ",
    r#"printf '%s\n' '{"schema_version":1,"id":1,"method":"initialize","params":{"protocol_version":1,"capabilities":[]}}'; "#,
    "while read line; do sleep 60 & exit; done",
);

#[cfg(unix)]
#[test]
fn relaunch_survives_a_descendant_holding_the_pipe() {
    let manifest = Manifest::parse(
        r#"{"name": "wedge", "version": "0.1.0", "entrypoint": "bin/nothing",
            "timeouts": {"callback_ms": 500, "request_ms": 5000, "request_retries": 2}}"#,
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

    // The feed opens the analyze clock; the script spawns the inheriting
    // sleep and exits. Nothing will ever be produced, so after the 500 ms
    // callback budget the monitor kills the generation — on Unix the whole
    // group, sleep included, which closes the pipe and EOFs the reader.
    let mut files = FileTable::new();
    let file = files.intern(Path::new("lib/main.dart"));
    let batch = [Diagnostic::new(
        Severity::Warning,
        URI_DOES_NOT_EXIST,
        "relaunch wedge fixture",
        file,
        SourceRange {
            offset: 0,
            length: 0,
        },
        None,
    )];
    runtime
        .feed(&batch, &files, &CyclePush::default())
        .expect("the feed lands");

    // The death is only observable as "no diagnostics ever arrive": the wait
    // parks out its budget, and its final alive gate performs the teardown
    // plus relaunch inside this call — the wedge site. Pre-fix, the teardown
    // joined the reader parked on the sleep-held pipe and the call ran ~60 s
    // (until the sleep's own EOF); the bound below fails that, while leaving
    // CI slack over the 5 s park a healthy relaunch needs.
    let started = Instant::now();
    let drained = runtime
        .wait_output(Duration::from_secs(5))
        .expect("the enforced death relaunches within budget");
    let elapsed = started.elapsed();
    assert!(
        drained.diagnostics.is_empty(),
        "the dead generation produced nothing"
    );
    assert!(
        elapsed < Duration::from_secs(15),
        "the relaunch must not wait out the descendant holding the pipe, took {elapsed:?}"
    );
    assert_eq!(
        runtime.stats().restarts,
        1,
        "the enforcement kill consumed exactly one restart"
    );
    assert!(
        runtime.negotiated().is_some(),
        "the relaunched generation completed its handshake"
    );
    // Dropping here kills the fresh generation's process group: no orphaned
    // sleep outlives the test.
}

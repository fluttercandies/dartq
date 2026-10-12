//! End-to-end engine-wiring round trip against the real Dart host:
//! core pushes engine diagnostics (`notification/engine_diagnostics`), the
//! echo plugin echoes one back (`notification/diagnostic`), the runtime
//! surfaces it for the output merge, and the §14.3 isolation counters show
//! the traffic (PRD §14 conformance; module docs of `dartq_plugin`).
//!
//! Runs `dart` (on `PATH`) against the echo plugin in the sibling
//! `dartq-plugin-host` package — the one place a Dart process is part of a
//! Rust test, because the contract under test is the process boundary.

use std::path::{Path, PathBuf};
use std::time::Duration;

use dartq_diagnostics::{Diagnostic, Severity, SourceRange, URI_DOES_NOT_EXIST};
use dartq_plugin::{
    CapabilitySet, CyclePush, Manifest, PluginRuntime, Severity as WireSeverity, SpawnSpec,
    StdioPolicy,
};
use dartq_source::FileTable;

mod support;

use support::dart_available;

/// Generous handshake budget: a cold `dart` JIT start must fit inside one
/// request timeout (PRD §14.3 default is 10 s; CI cold caches get slack).
const REQUEST_MS: u64 = 120_000;

fn host_package_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../dartq-plugin-host")
}

fn spawn_echo() -> PluginRuntime {
    let manifest = Manifest::parse(&format!(
        r#"{{"name": "echo", "version": "0.1.0", "entrypoint": "bin/echo_plugin.dart",
            "capabilities": ["lint"], "timeouts": {{"request_ms": {REQUEST_MS}}}}}"#
    ))
    .expect("manifest parses");
    PluginRuntime::spawn(SpawnSpec {
        program: "dart".to_owned(),
        args: vec!["bin/echo_plugin.dart".to_owned()],
        current_dir: host_package_dir(),
        manifest,
        stderr: StdioPolicy::Inherit,
        serve_queries: false,
    })
    .expect("dart host spawns and handshakes")
}

#[test]
#[ignore = "spawns real dart processes; run in the serial window via plugin-live (DARTQ_M4_LIVE=1)"]
fn engine_diagnostics_round_trip_through_the_dart_host() {
    if !dart_available() {
        println!("skipping: no `dart` on PATH, the live fixture cannot run");
        return;
    }
    let mut runtime = spawn_echo();
    assert_eq!(
        runtime.negotiated().map(CapabilitySet::names),
        Some(vec!["lint"]),
        "capability intersection takes the plugin's set"
    );

    // One engine diagnostic, mapped through the engine's file table.
    let mut files = FileTable::new();
    let file = files.intern(Path::new("lib/main.dart"));
    let engine = Diagnostic::new(
        Severity::Warning,
        URI_DOES_NOT_EXIST,
        "Target of URI doesn't exist: 'missing.dart'.",
        file,
        SourceRange {
            offset: 12,
            length: 16,
        },
        None,
    );

    runtime
        .feed(&[engine], &files, &CyclePush::default())
        .expect("engine feed");
    let echoed = runtime
        .wait_output(Duration::from_mins(1))
        .expect("plugin diagnostics arrive")
        .diagnostics;
    assert_eq!(echoed.len(), 1, "the echo plugin emits exactly one frame");
    let diagnostic = &echoed[0];
    // The table normalizes paths (relative spellings absolutize), so the
    // wire carries — and the echo returns — the canonical form.
    assert_eq!(
        diagnostic.file,
        files.path(file).expect("interned").to_string_lossy()
    );
    assert_eq!(diagnostic.severity, WireSeverity::Warning);
    assert_eq!(diagnostic.code, "echo/roundtrip");
    assert_eq!(
        diagnostic.message,
        "Target of URI doesn't exist: 'missing.dart'."
    );
    assert_eq!(diagnostic.offset, 12);
    assert_eq!(diagnostic.length, 16);

    // §14.3 counters: the wire traffic of the round trip is all observable.
    let stats = runtime.stats();
    assert_eq!(stats.restarts, 0);
    assert_eq!(stats.frames_dropped, 0);
    assert_eq!(
        stats.frames_in, 2,
        "initialize reply + the echoed diagnostic"
    );
    assert_eq!(stats.frames_out, 2, "initialize + the engine push");
    assert!(stats.bytes_in > 0 && stats.bytes_out > 0);
    assert!(
        stats.peak_rss_bytes > 0,
        "child RSS must be sampled on {}",
        std::env::consts::OS
    );

    runtime.shutdown().expect("graceful shutdown");
}

#[test]
#[ignore = "spawns real dart processes; run in the serial window via plugin-live (DARTQ_M4_LIVE=1)"]
fn empty_feed_sends_no_frame() {
    if !dart_available() {
        println!("skipping: no `dart` on PATH, the live fixture cannot run");
        return;
    }
    let mut runtime = spawn_echo();
    let before = runtime.stats();
    runtime
        .feed(&[], &FileTable::new(), &CyclePush::default())
        .expect("empty feed is a no-op");
    let after = runtime.stats();
    assert_eq!(after.frames_out, before.frames_out);
    runtime.shutdown().expect("graceful shutdown");
}

//! Live plugin E2E (M4-T9): a real `dart` process — the `bin/live_plugin`
//! fixture in the sibling `dartq-plugin-host` package, built on that
//! package's `Plugin` lifecycle and `servePlugin` integration — driven
//! through the runtime's contract: `dart run` launch, handshake + capability
//! negotiation, an engine push giving the request its fix context, a
//! `request/fixes` reply computed by the registered producer, graceful
//! shutdown in budget, and mid-session crash isolation reported as the
//! `dartq/plugin_crashed` diagnostic (PRD §14).
//!
//! Distinct from `e2e_round_trip.rs` (echo over the raw callback API) and
//! `plugin_discovery.rs` (manifest discovery): this exercises the
//! pub-integrated `dart run` launch path and the host package's
//! `Plugin`/`PluginRegistry` API with real producer execution. A missing
//! `dart` skips the suite (CI without the SDK must not go red); children
//! are reaped by the runtime's own shutdown paths — the tests never kill or
//! wait on them directly.

use std::path::{Path, PathBuf};

use dartq_diagnostics::{
    CodeInfo, Diagnostic, DiagnosticType, Severity, SourceRange, URI_DOES_NOT_EXIST,
};
use dartq_plugin::{
    CapabilitySet, Manifest, PluginRuntime, RuntimeError, SpawnSpec, StdioPolicy, crash_diagnostic,
};
use dartq_source::FileTable;

mod support;

use support::dart_available;

/// The fixture's registered rule (`bin/live_plugin.dart`); the engine rows
/// fed below carry this code so the request has a fix context.
const LIVE_RULE: CodeInfo = CodeInfo::new("live/fixed", "LIVE_FIXED", DiagnosticType::Lint, false);

/// Generous request budget: a cold `dart run` JIT start must fit inside one
/// (PRD §14.3 default is 10 s; CI cold caches get the same slack as
/// `e2e_round_trip`). The tests never shrink the runtime's budgets — every
/// wait is a manifest budget the runtime itself enforces.
const REQUEST_MS: u64 = 120_000;

fn host_package_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../dartq-plugin-host")
}

/// Spawns the fixture: `dart run bin/live_plugin.dart [args...]` inside the
/// host package directory.
fn spawn_live(args: &[&str]) -> PluginRuntime {
    let manifest = Manifest::parse(&format!(
        r#"{{"name": "live", "version": "0.1.0", "entrypoint": "bin/live_plugin.dart",
            "capabilities": ["lint", "fix"],
            "timeouts": {{"callback_ms": 30000, "request_ms": {REQUEST_MS}}}}}"#
    ))
    .expect("manifest parses");
    PluginRuntime::spawn(SpawnSpec {
        program: "dart".to_owned(),
        args: std::iter::once("run".to_owned())
            .chain(args.iter().map(ToString::to_string))
            .collect(),
        current_dir: host_package_dir(),
        manifest,
        stderr: StdioPolicy::Inherit,
    })
    .expect("dart run spawns the live fixture and completes the handshake")
}

#[test]
#[ignore = "spawns real dart processes; run in the serial window via plugin-live (DARTQ_M4_LIVE=1)"]
fn live_plugin_round_trips_fixes_and_shuts_down_in_budget() {
    if !dart_available() {
        println!("skipping: no `dart` on PATH, the live fixture cannot run");
        return;
    }
    let mut runtime = spawn_live(&["bin/live_plugin.dart"]);
    assert_eq!(
        runtime.negotiated().map(CapabilitySet::names),
        Some(vec!["lint", "fix"]),
        "the negotiated set is the fixture's declared capabilities"
    );

    // The fix context: one engine diagnostic carrying the fixture's
    // registered rule code (span 10..14) plus one with a different,
    // unregistered code right on the requested range — only the registered
    // rule's producer may answer.
    let mut files = FileTable::new();
    let file = files.intern(Path::new("lib/main.dart"));
    let rows = [
        Diagnostic::new(
            Severity::Warning,
            LIVE_RULE,
            "the live rule fired",
            file,
            SourceRange {
                offset: 10,
                length: 4,
            },
            None,
        ),
        Diagnostic::new(
            Severity::Warning,
            URI_DOES_NOT_EXIST,
            "Target of URI doesn't exist: 'missing.dart'.",
            file,
            SourceRange {
                offset: 12,
                length: 2,
            },
            None,
        ),
    ];
    runtime.feed(&rows, &files).expect("engine feed");

    // The fixes round trip: the request names 12..14, inside the live rule's
    // span. The reply is the registered producer's compute output — one edit
    // replacing the *diagnostic's* span (10..14), not the requested range.
    let path = files
        .path(file)
        .expect("interned")
        .to_string_lossy()
        .into_owned();
    let reply = runtime
        .request_fixes(&path, 12, 2)
        .expect("the fixture answers within the callback budget");
    assert_eq!(
        reply.fixes.len(),
        1,
        "only the registered rule's producer answers"
    );
    assert_eq!(reply.fixes[0].priority, 1);
    assert_eq!(reply.fixes[0].message, "live fix");
    assert_eq!(reply.fixes[0].edits.len(), 1);
    assert_eq!(reply.fixes[0].edits[0].offset, 10);
    assert_eq!(reply.fixes[0].edits[0].length, 4);
    assert_eq!(reply.fixes[0].edits[0].replacement, "fixed");

    // §14.3 counters over the live wire: handshake + engine push + fixes
    // request out, their replies in (the fixture emits no diagnostics of
    // its own); no restarts, no frame-guard drops; the child's RSS is
    // sampled.
    let stats = runtime.stats();
    assert_eq!(stats.restarts, 0);
    assert_eq!(stats.frames_dropped, 0);
    assert_eq!(
        stats.frames_out, 3,
        "initialize + engine push + fixes request"
    );
    assert_eq!(stats.frames_in, 2, "initialize reply + fixes reply");
    assert!(stats.bytes_in > 0 && stats.bytes_out > 0);
    assert!(
        stats.peak_rss_bytes > 0,
        "child RSS must be sampled on {}",
        std::env::consts::OS
    );

    // `Ok` means the shutdown reply beat the manifest's request budget; the
    // runtime reaps the child behind it.
    runtime
        .shutdown()
        .expect("graceful shutdown within the request budget");
}

#[test]
#[ignore = "spawns real dart processes; run in the serial window via plugin-live (DARTQ_M4_LIVE=1)"]
fn live_plugin_mid_session_death_isolated_as_plugin_crashed() {
    if !dart_available() {
        println!("skipping: no `dart` on PATH, the live fixture cannot run");
        return;
    }
    // `crash` mode: the fixture answers the handshake, then dies inside the
    // producer's `compute` on the first work request — a mid-session death,
    // not a startup failure, and proof the host executes registered
    // producers (the death happens in the producer, not the serve loop).
    // The request is written into a live, reading process, so the EOF the
    // runtime observes is deterministically the fixture's exit.
    let mut runtime = spawn_live(&["bin/live_plugin.dart", "crash"]);

    // The matching engine row gives the request its fix context; without it
    // no producer would run and the fixture would answer empty instead of
    // dying.
    let mut files = FileTable::new();
    let file = files.intern(Path::new("lib/main.dart"));
    let rows = [Diagnostic::new(
        Severity::Warning,
        LIVE_RULE,
        "the live rule fired",
        file,
        SourceRange {
            offset: 10,
            length: 4,
        },
        None,
    )];
    runtime.feed(&rows, &files).expect("engine feed");

    let path = files
        .path(file)
        .expect("interned")
        .to_string_lossy()
        .into_owned();
    let outcome = runtime.request_fixes(&path, 12, 2);
    let error = outcome.expect_err("a fixture that dies mid-session cannot answer");
    assert!(
        matches!(&error, RuntimeError::Crashed { reason }
            if reason.contains("closed the connection")),
        "the death is reported with its cause, got {error:?}"
    );
    assert_eq!(
        runtime.stats().restarts,
        0,
        "a death discovered mid-request absorbs no restart budget"
    );

    // Documented death reporting: the failure converts to the one tool
    // diagnostic the output layer merges, naming the plugin and the cause.
    let diagnostic = crash_diagnostic("live", &error, file);
    assert_eq!(diagnostic.code.name, "dartq/plugin_crashed");
    assert_eq!(diagnostic.severity, Severity::Error);
    assert!(
        diagnostic.message.contains("live") && diagnostic.message.contains("closed the connection"),
        "message names the plugin and the cause: {}",
        diagnostic.message
    );

    // Shutdown of the dead runtime reports the same crash and reaps the
    // child; it never restarts.
    let outcome = runtime.shutdown();
    assert!(
        matches!(outcome, Err(RuntimeError::Crashed { .. })),
        "shutdown after death surfaces the crash, got {outcome:?}"
    );
}

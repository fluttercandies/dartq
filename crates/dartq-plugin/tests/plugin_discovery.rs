//! Discovery end-to-end (PRD §14.2): the `<workspace>/plugins/` convention
//! finds a fixture manifest (and skips a broken one with a structured
//! error), the discovered plugin spawns as a real Dart host, the
//! handshake + engine-diagnostics push + `request/fixes` round trip run
//! against it, and the §14.3 counters move coherently.
//!
//! Like `e2e_round_trip.rs`, this is the one place a Dart process is part of
//! a Rust test: the contract under test is the process boundary.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use dartq_diagnostics::{Diagnostic, Severity, SourceRange, URI_DOES_NOT_EXIST};
use dartq_plugin::{
    CapabilitySet, ManifestError, PluginRuntime, Severity as WireSeverity, SpawnSpec, discover,
};
use dartq_source::FileTable;

mod support;

use support::dart_available;

/// Generous handshake/write budget: a cold `dart` JIT start must fit inside
/// one request timeout (PRD §14.3 default is 10 s; CI cold caches get slack).
const REQUEST_MS: u64 = 120_000;

fn host_package_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../dartq-plugin-host")
}

/// A throwaway workspace whose `plugins/` directory holds the fixer fixture
/// (valid), a broken manifest, and a manifest-less non-plugin directory.
struct Workspace(PathBuf);

impl Workspace {
    /// A per-test root: the tag keeps parallel tests off each other's
    /// workspaces.
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "dartq-plugin-discovery-e2e-{tag}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("plugins")).expect("create workspace");
        Self(root)
    }

    fn write_fixture(&self, name: &str, manifest: &str) {
        let dir = self.0.join("plugins").join(name);
        fs::create_dir_all(&dir).expect("create plugin dir");
        fs::write(dir.join("dartq_plugin.json"), manifest).expect("write manifest");
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn fixture_manifest() -> String {
    let entrypoint = host_package_dir().join("bin/fix_plugin.dart");
    format!(
        r#"{{"name": "fixer", "version": "0.1.0", "entrypoint": "{}",
            "capabilities": ["lint", "fix"],
            "timeouts": {{"callback_ms": 30000, "request_ms": {REQUEST_MS}}}}}"#,
        entrypoint.display()
    )
}

#[test]
fn discovery_spawns_the_fixture_and_round_trips_fixes() {
    if !dart_available() {
        println!("skipping: no `dart` on PATH, the live fixture cannot run");
        return;
    }
    let workspace = Workspace::new("roundtrip");
    workspace.write_fixture("fixer", &fixture_manifest());
    // A broken manifest and a manifest-less directory: neither may hide the
    // valid plugin, and only the former may surface as a structured skip.
    workspace.write_fixture(
        "broken",
        r#"{"name": "broken", "version": "0.1.0", "entrypoint": "x.dart", "memory_budget_mb": 0}"#,
    );
    fs::create_dir_all(workspace.path().join("plugins").join("notes")).expect("non-plugin dir");

    let report = discover(workspace.path(), &[]);
    assert_eq!(report.plugins.len(), 1, "only the valid fixture spawns");
    assert_eq!(report.skipped.len(), 1);
    assert_eq!(
        report.skipped[0].error,
        ManifestError::InvalidField("memory_budget_mb", "must be a positive integer".to_owned(),)
    );

    let discovered = &report.plugins[0];
    assert_eq!(discovered.manifest.name, "fixer");
    let mut runtime = PluginRuntime::spawn(SpawnSpec::dart(
        &discovered.dir,
        discovered.manifest.clone(),
    ))
    .expect("the discovered fixture spawns and handshakes");
    assert_eq!(
        runtime.negotiated().map(CapabilitySet::names),
        Some(vec!["lint", "fix"]),
        "the intersection carries the plugin's fix capability"
    );

    // Push the engine's diagnostics; the fixture echoes the first back.
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
    runtime.feed(&[engine], &files).expect("engine feed");
    let echoed = runtime
        .wait_diagnostics(Duration::from_mins(1))
        .expect("plugin diagnostics arrive");
    assert_eq!(echoed.len(), 1);
    assert_eq!(echoed[0].code, "fixer/roundtrip");
    assert_eq!(echoed[0].severity, WireSeverity::Warning);
    assert_eq!(echoed[0].offset, 12);

    // The fixes round trip: one prioritized change anchored at the request.
    let reply = runtime
        .request_fixes(
            &files.path(file).expect("interned").to_string_lossy(),
            10,
            2,
        )
        .expect("fixes reply arrives within the callback budget");
    assert_eq!(reply.fixes.len(), 1);
    assert_eq!(reply.fixes[0].priority, 1);
    assert_eq!(reply.fixes[0].message, "insert const");
    assert_eq!(reply.fixes[0].edits.len(), 1);
    assert_eq!(reply.fixes[0].edits[0].offset, 10);
    assert_eq!(reply.fixes[0].edits[0].length, 2);
    assert_eq!(reply.fixes[0].edits[0].replacement, "const ");

    // §14.3 counters: initialize + engine push + fixes request out, their
    // three replies in, zero restarts or guard drops.
    let stats = runtime.stats();
    assert_eq!(stats.restarts, 0);
    assert_eq!(stats.frames_dropped, 0);
    assert_eq!(
        stats.frames_out, 3,
        "initialize, engine push, fixes request"
    );
    assert_eq!(stats.frames_in, 3, "their replies");
    assert!(stats.bytes_in > 0 && stats.bytes_out > 0);
    assert!(stats.peak_rss_bytes > 0, "child RSS must be sampled");

    runtime.shutdown().expect("graceful shutdown");
}

#[test]
fn overrides_load_directories_and_report_missing_paths() {
    let workspace = Workspace::new("overrides");
    workspace.write_fixture("fixer", &fixture_manifest());
    let plugin_dir = workspace.path().join("plugins").join("fixer");

    // A directory override and its manifest file resolve to the same plugin;
    // neither the convention scan nor a double override duplicates it.
    let report = discover(
        workspace.path(),
        &[plugin_dir.clone(), plugin_dir.join("dartq_plugin.json")],
    );
    assert_eq!(report.plugins.len(), 1);
    assert!(report.skipped.is_empty());

    // An override the caller named explicitly must report, not skip silently;
    // the convention scan still finds the fixture on its own.
    let missing = workspace.path().join("plugins").join("nowhere");
    let report = discover(workspace.path(), &[missing]);
    assert_eq!(report.plugins.len(), 1);
    assert_eq!(report.skipped.len(), 1);
    assert!(report.skipped[0].path.to_string_lossy().contains("nowhere"));
}

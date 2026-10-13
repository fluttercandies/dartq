//! Live compatibility-sample E2E (M4-T9 + T5): the `samples/compat` plugin
//! of the sibling `dartq-plugin-host` package — an official
//! `analysis_server_plugin`-shaped plugin (see that sample's `README.md` for
//! the porting guide) — driven end to end through the runtime's contract:
//! manifest discovery, `dart <entrypoint>` launch (the exact daemon path),
//! the rule census in the handshake, the per-cycle config push
//! (`notification/plugin_config`), the analyzed set riding the engine batch,
//! the registered rule executing host-side over the overlay text, the
//! severity override re-leveling, the daemon-side merge contract, the fixes
//! face with a UTF-16 edit applied to real text, and the crash mode surfaced
//! as the `dartq/plugin_crashed` diagnostic (PRD §14). A further row drives
//! the query mode (`query` launch argument + `serve_queries: true`): the
//! runtime serves the producer's reverse `request/query` round trips and the
//! reply's changes state the resolution facts. The
//! disabled-by-default face is pinned at the in-process level
//! (`test/compat_sample_test.dart`); the live cycle always configures.
//!
//! Four more rows drive the real ecosystem plugin overlays
//! (`samples/{signals,jaspr,riverpod,pyramid}` — thin packages that run the
//! actual upstream plugin objects from the pinned `.third/aswp-samples`
//! clones over the host loop): the #644 compat matrix. Each row asserts the
//! registration census (the observable proof the upstream `register` ran),
//! the negotiated capabilities, and the fixes face where the row exercises
//! it. The clones are maintainer-provisioned and not part of git, and the
//! overlays' `.third` path deps additionally require `dart pub get` to have
//! resolved them, so a row whose overlay directory is absent or unresolved
//! skips at runtime with a clear message.
//!
//! Distinct from `live_host.rs` (the `bin/live_plugin` fixture): this drives
//! on-disk samples with their real `dartq_plugin.json` manifests. All tests
//! spawn a real `dart` process: `#[ignore]`d here so the flag-gated
//! plugin-live serial face owns them — run with
//! `cargo test -p dartq-plugin -- --ignored` (DARTQ_M4_LIVE=1).
//! A missing `dart` skips at runtime (CI without the SDK must not go red);
//! children are reaped by the runtime's own shutdown paths.

use rustc_hash::FxHashSet;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use dartq_diagnostics::{Diagnostic, Severity, SourceRange, URI_DOES_NOT_EXIST};
use dartq_plugin::session::SessionError;
use dartq_plugin::{
    Capability, CapabilitySet, CycleConfig, CyclePush, Manifest, OverlayEntry, PendingQuery,
    PluginRuntime, QueryKind, RuleKind, RuntimeError, Severity as WireSeverity, SpawnSpec,
    StdioPolicy, crash_diagnostic, discover,
};
use serde_json::{Map, Value, json};
use dartq_source::{FileTable, LineIndex, utf16_offset_to_byte};

mod support;

use support::dart_available;

/// The on-disk sample: a real plugin directory (manifest + entrypoint).
fn sample_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../dartq-plugin-host/samples/compat")
}

/// The content the engine holds for `lib/a.dart` (the test plays the
/// engine's resident-text role). The emoji is a surrogate pair, so UTF-16
/// offsets past it differ from byte offsets — the position exchange's proof.
const A_DART: &str = "import '\u{1F600}.dart';\nvoid main() {}\n";

/// The open-document text pushed as the cycle's overlay: [A_DART] plus a
/// boolean literal — the rule fires at UTF-16 offset 38, length 4 (past the
/// emoji, where UTF-16 and byte offsets diverge).
const FIXTURE: &str = "import '\u{1F600}.dart';\nvoid main() { print(true); }\n";

/// UTF-16 offset of the fixture's `true` literal.
const TRUE_OFFSET: u64 = 38;

#[test]
#[ignore = "spawns real dart processes; run in the serial window via plugin-live (DARTQ_M4_LIVE=1)"]
// One linear end-to-end scenario (discovery → spawn → feed → merge → fixes
// → teardown) whose value is the uninterrupted sequence; carving it into
// helpers would thread six Arcs of context for no reader benefit.
#[expect(clippy::too_many_lines, reason = "see above")]
fn compat_sample_diagnostics_merge_and_fixes_apply_over_a_live_process() {
    if !dart_available() {
        println!("skipping: no `dart` on PATH, the live fixture cannot run");
        return;
    }
    // Discovery through the real manifest: an explicit override loads the
    // sample directory (the convention scan is empty here).
    let report = discover(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join(".."),
        &[sample_dir()],
    );
    assert!(
        report.skipped.is_empty(),
        "the sample manifest must load cleanly: {:?}",
        report.skipped
    );
    assert_eq!(report.plugins.len(), 1);
    let discovered = &report.plugins[0];
    assert_eq!(discovered.manifest.name, "compat");
    assert_eq!(discovered.manifest.entrypoint, "bin/compat_plugin.dart");

    let mut runtime = PluginRuntime::spawn(SpawnSpec::dart(
        &discovered.dir,
        discovered.manifest.clone(),
    ))
    .expect("the sample spawns via `dart <entrypoint>` and completes the handshake");
    assert_eq!(
        runtime.negotiated().map(CapabilitySet::names),
        Some(vec!["lint", "fix"]),
        "the negotiated set is the manifest's capability list"
    );

    // The rule census rode the initialize reply: the registered lint rule
    // with its code and declared severity — the enablement resolution's
    // input.
    let census = runtime.rule_census();
    assert_eq!(census.len(), 1);
    assert_eq!(census[0].name, "boolean_literal_review");
    assert_eq!(census[0].kind, RuleKind::Lint);
    assert_eq!(census[0].codes.len(), 1);
    assert_eq!(census[0].codes[0].name, "boolean_literal");
    assert_eq!(census[0].codes[0].severity, WireSeverity::Warning);

    // The file set: two .dart files (resident text held by the test) and one
    // non-Dart file the engine row proves flows untouched.
    let mut files = FileTable::new();
    let a = files.intern(Path::new("lib/a.dart"));
    let b = files.intern(Path::new("lib/b.dart"));
    let notes = files.intern(Path::new("notes.txt"));
    let a_path = files
        .path(a)
        .expect("interned")
        .to_string_lossy()
        .into_owned();
    let b_path = files
        .path(b)
        .expect("interned")
        .to_string_lossy()
        .into_owned();

    // Engine diagnostics: offsets are UTF-16; the first row spans the
    // emoji's surrogate pair.
    let engine = vec![
        Diagnostic::new(
            Severity::Error,
            URI_DOES_NOT_EXIST,
            "Target of URI doesn't exist: 'x.dart'.",
            a,
            SourceRange {
                offset: 8,
                length: 2,
            },
            None,
        ),
        Diagnostic::new(
            Severity::Error,
            URI_DOES_NOT_EXIST,
            "Target of URI doesn't exist: 'y.dart'.",
            a,
            SourceRange {
                offset: 10,
                length: 4,
            },
            None,
        ),
        Diagnostic::new(
            Severity::Warning,
            URI_DOES_NOT_EXIST,
            "Target of URI doesn't exist: 'z.dart'.",
            b,
            SourceRange {
                offset: 0,
                length: 1,
            },
            None,
        ),
        Diagnostic::new(
            Severity::Warning,
            URI_DOES_NOT_EXIST,
            "non-Dart files are not analyzed by the rule",
            notes,
            SourceRange {
                offset: 0,
                length: 4,
            },
            None,
        ),
    ];

    // Cycle 1: the config enables the rule, the analyzed set carries both
    // .dart files, and the overlay carries the unsaved text the host parses
    // (the disk never sees the boolean literal). The severity override is
    // cycle 2's face; cycle 1 proves the declared severity stands.
    let base_config = |overrides: BTreeMap<String, WireSeverity>| CyclePush {
        analyzed: vec![a_path.clone(), b_path.clone()],
        config: CycleConfig {
            enabled_rules: vec!["boolean_literal_review".to_owned()],
            severity_overrides: overrides,
            overlays: vec![OverlayEntry {
                file: a_path.clone(),
                content: Some(FIXTURE.to_owned()),
            }],
        },
    };
    runtime
        .feed(&engine, &files, &base_config(BTreeMap::new()))
        .expect("engine feed");
    // `Duration::from_minutes` is not stable on the pinned toolchain, so
    // seconds remain the only expressible unit for this budget.
    #[expect(clippy::duration_suboptimal_units, reason = "see above")]
    let findings = runtime
        .wait_output(Duration::from_secs(60))
        .expect("plugin findings arrive within the drain budget")
        .diagnostics;
    assert_eq!(findings.len(), 1, "the enabled rule reports the literal");
    assert_eq!(findings[0].file, a_path);
    assert_eq!(findings[0].code, "boolean_literal");
    assert_eq!(findings[0].severity, WireSeverity::Warning);
    assert_eq!(
        findings[0].message,
        "Boolean literals need a compat review."
    );
    assert_eq!(
        findings[0].offset, TRUE_OFFSET,
        "UTF-16 offset past the emoji carried through"
    );
    assert_eq!(findings[0].length, 4);

    // Cycle 2: a severity override re-levels the row (the config params
    // changed, so the runtime pushes the new config instead of deduping).
    let mut overrides = BTreeMap::new();
    overrides.insert("boolean_literal".to_owned(), WireSeverity::Error);
    runtime
        .feed(&engine, &files, &base_config(overrides))
        .expect("engine feed");
    #[expect(clippy::duration_suboptimal_units, reason = "see above")]
    let findings = runtime
        .wait_output(Duration::from_secs(60))
        .expect("plugin findings arrive within the drain budget")
        .diagnostics;
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].severity, WireSeverity::Error, "overridden");
    assert_eq!(findings[0].code, "boolean_literal");
    assert_eq!(findings[0].offset, TRUE_OFFSET);

    // The daemon-side merge contract (crates/dartq-daemon/src/plugins.rs,
    // `merge` + `to_lsp`): engine rows first in order, then plugin rows
    // offset-sorted within the plugin, deduped by
    // (file, range.start.line, range.start.character, code, source).
    let notes_path = files
        .path(notes)
        .expect("interned")
        .to_string_lossy()
        .into_owned();
    let engine_rows: Vec<MergedRow> = engine
        .iter()
        .map(|d| {
            let (path, text) = if d.file.index() == a.index() {
                (a_path.as_str(), A_DART)
            } else if d.file.index() == b.index() {
                (b_path.as_str(), B_DART)
            } else {
                (notes_path.as_str(), NOTES_TXT)
            };
            let (line, character) = lsp_position(text, d.range.offset);
            MergedRow {
                file: path.to_owned(),
                line,
                character,
                code: d.code.name.to_owned(),
                source: "engine".to_owned(),
                offset: u64::from(d.range.offset),
            }
        })
        .collect();
    let plugin_rows: Vec<MergedRow> = findings
        .iter()
        .map(|row| {
            // The rule ran on the overlay text, so its positions live in
            // FIXTURE's coordinate space.
            let (line, character) =
                lsp_position(FIXTURE, u32::try_from(row.offset).expect("in-range"));
            MergedRow {
                file: row.file.clone(),
                line,
                character,
                code: row.code.clone(),
                source: "compat".to_owned(),
                offset: row.offset,
            }
        })
        .collect();
    let merged = merge(engine_rows, plugin_rows);
    assert_eq!(
        merged.len(),
        5,
        "engine + plugin rows, nothing deduped away"
    );
    assert_eq!(merged[0].offset, 8, "engine rows first, in order");
    assert_eq!(merged[3].offset, 0, "the notes.txt engine row is intact");
    // The plugin row sorts last (offset 38).
    assert_eq!(
        (merged[4].file.as_str(), merged[4].offset),
        (a_path.as_str(), TRUE_OFFSET)
    );

    // The fixes face: the rule fix anchored at the plugin's own finding
    // (the fix context is the engine batch together with the rule's
    // emissions), then the message-only suggestion.
    let reply = runtime
        .request_fixes(&a_path, TRUE_OFFSET, 4)
        .expect("fixes reply arrives within the callback budget");
    assert_eq!(reply.fixes.len(), 2);
    assert_eq!(reply.fixes[0].priority, 1);
    assert_eq!(
        reply.fixes[0].message,
        "Replace literal with a compat marker"
    );
    assert_eq!(reply.fixes[0].edits.len(), 1);
    let edit = &reply.fixes[0].edits[0];
    assert_eq!(edit.offset, TRUE_OFFSET);
    assert_eq!(edit.length, 4);
    assert_eq!(edit.replacement, "/* compat: fixed */");
    assert_eq!(reply.fixes[1].priority, 50);
    assert_eq!(reply.fixes[1].message, "Review this location");
    assert!(reply.fixes[1].edits.is_empty());

    // Applying the edit proves the UTF-16 exchange: offsets 38..42 sit past
    // the emoji, where UTF-16 and byte offsets diverge.
    let start = utf16_offset_to_byte(FIXTURE, TRUE_OFFSET as u32);
    let end = utf16_offset_to_byte(FIXTURE, TRUE_OFFSET as u32 + 4);
    let mut fixed = String::with_capacity(FIXTURE.len());
    fixed.push_str(&FIXTURE[..start]);
    fixed.push_str(&edit.replacement);
    fixed.push_str(&FIXTURE[end..]);
    assert_eq!(
        fixed, "import '\u{1F600}.dart';\nvoid main() { print(/* compat: fixed */); }\n",
        "the fix applied at the UTF-16 range the plugin reported"
    );

    let stats = runtime.stats();
    assert_eq!(stats.restarts, 0);
    assert!(stats.peak_rss_bytes > 0, "child RSS must be sampled");
    runtime.shutdown().expect("graceful shutdown");
}

#[test]
#[ignore = "spawns real dart processes; run in the serial window via plugin-live (DARTQ_M4_LIVE=1)"]
fn compat_sample_crash_mode_surfaces_as_plugin_crashed() {
    if !dart_available() {
        println!("skipping: no `dart` on PATH, the live fixture cannot run");
        return;
    }
    // Crash mode: the sample answers the handshake, then dies on the first
    // work request before replying — the intentional-crash mode for crash
    // surfacing.
    let manifest =
        Manifest::from_path(&sample_dir().join("dartq_plugin.json")).expect("manifest parses");
    let mut runtime = PluginRuntime::spawn(SpawnSpec {
        program: "dart".to_owned(),
        args: vec![manifest.entrypoint.clone(), "crash".to_owned()],
        current_dir: sample_dir(),
        manifest,
        stderr: StdioPolicy::Inherit,
        serve_queries: false,
    })
    .expect("the crash-mode sample spawns and handshakes");

    let error = runtime
        .request_fixes("lib/a.dart", 0, 0)
        .expect_err("a sample that dies mid-request cannot answer");
    assert!(
        matches!(&error, RuntimeError::Crashed { reason }
            if reason.contains("closed the connection")),
        "the death is reported with its cause, got {error:?}"
    );
    assert_eq!(runtime.stats().restarts, 0);

    let anchor = {
        let mut files = FileTable::new();
        files.intern(Path::new("lib/a.dart"))
    };
    let diagnostic = crash_diagnostic("compat", &error, anchor);
    assert_eq!(diagnostic.code.name, "dartq/plugin_crashed");
    assert_eq!(diagnostic.severity, Severity::Error);
    assert!(
        diagnostic.message.contains("compat")
            && diagnostic.message.contains("closed the connection"),
        "message names the plugin and the cause: {}",
        diagnostic.message
    );

    assert!(matches!(
        runtime.shutdown(),
        Err(RuntimeError::Crashed { .. })
    ));
}

/// The query-mode fixture: the Dart probe's own table
/// (`bin/query_probe.dart`), all-ASCII so UTF-16 units are plain. Line 1
/// carries the rule's `true` finding (offset 20); `print` sits at 14
/// (length 5); `unused` — an unreferenced declaration — at 34 (length 6);
/// line 1's end at 29 is the miss position.
const QUERY_FIXTURE: &str = "void main() { print(true); }\nvoid unused() {}\n";

/// The fixture table's `(offset, length)` pairs into [QUERY_FIXTURE].
const QUERY_PRINT: (u64, u64) = (14, 5);
const QUERY_UNUSED: (u64, u64) = (34, 6);
/// Line 1's end: nothing resolves there (the legal empty answer).
const QUERY_MISS: (u64, u64) = (29, 4);

#[test]
#[ignore = "spawns real dart processes; run in the serial window via plugin-live (DARTQ_M4_LIVE=1)"]
// One linear scenario (spawn → negotiate → cycle → the four producer faces
// → teardown) walking the query-driven producer's paths in order; helpers
// would only thread the runtime and the fixture table through them.
#[expect(clippy::too_many_lines, reason = "see above")]
fn compat_sample_query_mode_serves_reverse_queries_over_a_live_process() {
    if !dart_available() {
        println!("skipping: no `dart` on PATH, the live fixture cannot run");
        return;
    }
    // Query mode = the crash-mode row's shape: the `query` launch argument
    // arms the sample's query-driven producer, and `serve_queries` makes
    // this session offer the capability at the handshake — a session without
    // a query server never advertises it, so the negotiated set honestly
    // represents what can be served.
    let manifest =
        Manifest::from_path(&sample_dir().join("dartq_plugin.json")).expect("manifest parses");
    let mut runtime = PluginRuntime::spawn(SpawnSpec {
        program: "dart".to_owned(),
        args: vec![manifest.entrypoint.clone(), "query".to_owned()],
        current_dir: sample_dir(),
        manifest,
        stderr: StdioPolicy::Inherit,
        serve_queries: true,
    })
    .expect("the query-mode sample spawns and handshakes");
    assert_eq!(
        runtime.negotiated().map(CapabilitySet::names),
        Some(vec!["lint", "fix", "query"]),
        "the launch argument advertises query and the serving session mirrors it"
    );

    // The rule census is unchanged in query mode: the extra producer is an
    // assist, not a rule.
    let census = runtime.rule_census();
    assert_eq!(census.len(), 1);
    assert_eq!(census[0].name, "boolean_literal_review");

    let mut files = FileTable::new();
    let a = files.intern(Path::new("lib/a.dart"));
    let a_path = files
        .path(a)
        .expect("interned")
        .to_string_lossy()
        .into_owned();

    // The rule face over the overlay is intact — one finding at the literal,
    // exactly as without the `query` argument.
    runtime
        .feed(
            &[],
            &files,
            &CyclePush {
                analyzed: vec![a_path.clone()],
                config: CycleConfig {
                    enabled_rules: vec!["boolean_literal_review".to_owned()],
                    severity_overrides: BTreeMap::new(),
                    overlays: vec![OverlayEntry {
                        file: a_path.clone(),
                        content: Some(QUERY_FIXTURE.to_owned()),
                    }],
                },
            },
        )
        .expect("engine feed");
    #[expect(clippy::duration_suboptimal_units, reason = "see above")]
    let findings = runtime
        .wait_output(Duration::from_secs(60))
        .expect("plugin findings arrive within the drain budget")
        .diagnostics;
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].code, "boolean_literal");
    assert_eq!(
        findings[0].offset, 20,
        "the rule face is unchanged in query mode"
    );

    // The fixture index the Dart probe serves (bin/query_probe.dart), played
    // by the test through the runtime's serving seam: `element_at` resolves
    // the two declarations (and declines the unknown file on the error
    // channel), `references` knows two resolved uses of `print` across two
    // files, and every other position gets the legal empty answer.
    let print_element = json!({
        "file": "lib/a.dart",
        "container": null,
        "kind": "function",
        "name": "print",
        "line": 1,
        "column": QUERY_PRINT.0 + 1,
        "length": QUERY_PRINT.1,
    });
    let unused_element = json!({
        "file": "lib/a.dart",
        "container": null,
        "kind": "function",
        "name": "unused",
        "line": 2,
        // Line 1 plus its newline ends at offset 29, so the 1-based column
        // of the name span is 34 - 29 + 1.
        "column": 6,
        "length": QUERY_UNUSED.1,
    });
    let print_references = json!([
        {
            "target_file": "lib/a.dart",
            "kind": "invoked",
            "file": "lib/a.dart",
            "offset": QUERY_PRINT.0,
            "length": QUERY_PRINT.1,
            "qualified": null,
        },
        {
            "target_file": "lib/a.dart",
            "kind": "invoked",
            "file": "lib/b.dart",
            "offset": 4,
            "length": QUERY_PRINT.1,
            "qualified": "p.print",
        },
    ]);
    let mut serve = |query: &PendingQuery| -> Result<Map<String, Value>, String> {
        let request = &query.request;
        match request.kind {
            QueryKind::ElementAt => {
                let file = request.file.as_deref().unwrap_or_default();
                if file == "lib/gone.dart" {
                    return Err(format!("unknown file `{file}`"));
                }
                let element = match (file, request.offset) {
                    ("lib/a.dart", Some(offset)) if offset == QUERY_PRINT.0 => {
                        Some(&print_element)
                    }
                    ("lib/a.dart", Some(offset)) if offset == QUERY_UNUSED.0 => {
                        Some(&unused_element)
                    }
                    _ => None,
                };
                let elements = element.map(|row| json!([row])).unwrap_or_else(|| json!([]));
                Ok(query_reply("elements", elements))
            }
            QueryKind::References => {
                let rows = if request.name.as_deref() == Some("print") {
                    print_references.clone()
                } else {
                    json!([])
                };
                Ok(query_reply("references", rows))
            }
            // The producer only asks these two kinds; anything else is a
            // decline, like the probe's table.
            other => Err(format!("unsupported query kind `{other:?}`")),
        }
    };

    // A referenced name: the producer round trips `element_at` +
    // `references` and answers with a concrete edit embedding the facts.
    let reply = runtime
        .request_fixes_serving(&a_path, QUERY_PRINT.0, QUERY_PRINT.1, &mut serve)
        .expect("fixes reply arrives within the callback budget");
    assert_eq!(reply.fixes.len(), 2, "the review plus the suggestion");
    assert_eq!(
        reply.fixes[0].priority, 10,
        "the sample's queryReviewPriority sorts first"
    );
    assert_eq!(
        reply.fixes[0].message,
        "compat query: `print` has 2 reference(s) across 2 file(s)"
    );
    assert_eq!(reply.fixes[0].edits.len(), 1);
    let edit = &reply.fixes[0].edits[0];
    assert_eq!(
        (edit.offset, edit.length),
        QUERY_PRINT,
        "the requested UTF-16 range echoed"
    );
    assert_eq!(edit.replacement, "print /* compat: 2 refs */");
    assert!(reply.fixes[1].edits.is_empty(), "the suggestion is message-only");

    // An unreferenced declaration: the removal-candidate decision,
    // message-only (a safe removal needs more than the name span).
    let reply = runtime
        .request_fixes_serving(&a_path, QUERY_UNUSED.0, QUERY_UNUSED.1, &mut serve)
        .expect("fixes reply arrives within the callback budget");
    assert_eq!(
        reply.fixes[0].message,
        "compat query: `unused` has no resolved references; a removal candidate"
    );
    assert!(reply.fixes[0].edits.is_empty());

    // A miss: empty elements is a legal answer, not a failure.
    let reply = runtime
        .request_fixes_serving(&a_path, QUERY_MISS.0, QUERY_MISS.1, &mut serve)
        .expect("fixes reply arrives within the callback budget");
    assert_eq!(
        reply.fixes[0].message,
        "compat query: nothing resolved at lib/a.dart:29"
    );

    // An unknown file rides the error channel into a QueryError and degrades
    // to a message-only change — the session stays up (the suggestion still
    // rides the same reply).
    let reply = runtime
        .request_fixes_serving("lib/gone.dart", 0, 0, &mut serve)
        .expect("fixes reply arrives within the callback budget");
    assert_eq!(
        reply.fixes[0].message,
        "compat query failed: unknown file `lib/gone.dart`"
    );
    assert_eq!(reply.fixes.len(), 2, "the session survived the failed query");

    assert_eq!(runtime.stats().restarts, 0);
    runtime.shutdown().expect("graceful shutdown");
}

// The #644 compat matrix: the four real ecosystem plugin overlays. One
// driver runs each row through the full lifecycle (discovery → spawn →
// handshake → the upstream `register` inside the child → diagnostics batch →
// census → fixes face → close); the rows are expectation tables, not code.

/// The fixes face one row exercises: one change per registered generator,
/// the first change's message pinning the first registration, and the marker
/// the edit writes at the requested UTF-16 range.
struct FixesFace {
    /// Changes in the reply (fix generators first, then assists).
    count: usize,
    /// The first change's message (registration order is wire order).
    first_message: &'static str,
    /// The replacement text the first edit writes.
    replacement: &'static str,
}

/// One overlay's expectations, pinning its #644 matrix row to the pinned
/// `.third` clone revision. If a pin moves, re-count the upstream
/// `register(PluginRegistry)` calls (or `getLintRules`/`getAssists` for the
/// custom_lint-shaped signals) and update the census/counts here — a census
/// mismatch is the drift detector.
struct OverlayRow {
    /// Overlay directory (and manifest name) under
    /// `dartq-plugin-host/samples/`.
    name: &'static str,
    /// The manifest's entrypoint.
    entrypoint: &'static str,
    /// Negotiated capabilities (manifest list, protocol order).
    negotiated: &'static [&'static str],
    /// `(lint, warning, fix, assist)` registration census.
    census: (u64, u64, u64, u64),
    /// The fixes face, when the row negotiates `fix`; `None` proves the
    /// capability gate on a lint-only row.
    fixes: Option<FixesFace>,
}

#[test]
#[ignore = "spawns real dart processes; run in the serial window via plugin-live (DARTQ_M4_LIVE=1)"]
fn signals_overlay_minimal_custom_lint_plugin_registers_and_applies_utf16_edits() {
    // Minimal deps (custom_lint, no analyzer registry); its five producers
    // are `DartAssist` subclasses served through `request/fixes`.
    drive_overlay(&OverlayRow {
        name: "signals",
        entrypoint: "bin/signals_plugin.dart",
        negotiated: &["lint", "fix"],
        census: (5, 0, 0, 5),
        fixes: Some(FixesFace {
            count: 5,
            first_message: "signals assist 1",
            replacement: "/* dartq:signals */",
        }),
    });
}

#[test]
#[ignore = "spawns real dart processes; run in the serial window via plugin-live (DARTQ_M4_LIVE=1)"]
fn jaspr_overlay_all_three_capabilities_with_assist_round_trip() {
    // 3 lint + 2 warning rules (census only: no host mirror), 4 fix
    // generators + 14 assists delegated to the host registry.
    drive_overlay(&OverlayRow {
        name: "jaspr",
        entrypoint: "bin/jaspr_plugin.dart",
        negotiated: &["lint", "fix", "assist"],
        census: (3, 2, 4, 14),
        fixes: Some(FixesFace {
            count: 18,
            first_message: "jaspr fix LintCode.sort_children_last",
            replacement: "/* dartq:jaspr */",
        }),
    });
}

#[test]
#[ignore = "spawns real dart processes; run in the serial window via plugin-live (DARTQ_M4_LIVE=1)"]
fn riverpod_overlay_bulk_registrations_over_a_live_process() {
    // The bulk row: 16 warning rules (census only) + 6 fix generators + 12
    // assists, all registered inside the child before the handshake.
    drive_overlay(&OverlayRow {
        name: "riverpod",
        entrypoint: "bin/riverpod_plugin.dart",
        negotiated: &["lint", "fix", "assist"],
        census: (0, 16, 6, 12),
        fixes: Some(FixesFace {
            count: 18,
            first_message: "riverpod fix LintCode.async_value_nullable_pattern",
            replacement: "/* dartq:riverpod */",
        }),
    });
}

#[test]
#[ignore = "spawns real dart processes; run in the serial window via plugin-live (DARTQ_M4_LIVE=1)"]
fn pyramid_overlay_lint_only() {
    // The lint-only row: the bulk of the upstream registration face (37
    // rules + 23 fix generators + 3 assists) with `request/fixes` gated off
    // by the manifest's `[lint]` capability.
    drive_overlay(&OverlayRow {
        name: "pyramid",
        entrypoint: "bin/pyramid_plugin.dart",
        negotiated: &["lint"],
        census: (37, 0, 23, 3),
        fixes: None,
    });
}

/// The on-disk overlay directory for one ecosystem sample.
fn overlay_dir(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../dartq-plugin-host/samples")
        .join(name)
}

/// Early-skips an overlay row when it cannot run: the `.third` ecosystem
/// clones are maintainer-provisioned (absent on fresh clones), `dart pub get`
/// must have resolved the overlay's `.third` path deps (proof: a generated
/// `package_config.json`), or no `dart` is on PATH. Returns the overlay
/// directory when ready.
fn overlay_ready(name: &str) -> Option<PathBuf> {
    let dir = overlay_dir(name);
    if !dir.join("dartq_plugin.json").exists() {
        eprintln!(
            "skipping: {} is absent; the ecosystem sample clones under .third/ \
             are maintainer-provisioned and not part of git",
            dir.display()
        );
        return None;
    }
    if !dir.join(".dart_tool/package_config.json").exists() {
        eprintln!(
            "skipping: {} is unresolved; run `dart pub get` in it first (its \
             .third path deps are maintainer-provisioned and not part of git)",
            dir.display()
        );
        return None;
    }
    if !dart_available() {
        eprintln!("skipping: no `dart` on PATH, the live fixture cannot run");
        return None;
    }
    Some(dir)
}

/// Parses a census message (`census lint=L warning=W fix=F assist=A`) into
/// `(lint, warning, fix, assist)`.
fn parse_census(message: &str) -> (u64, u64, u64, u64) {
    let mut counts = [0u64; 4];
    let body = message
        .strip_prefix("census ")
        .unwrap_or_else(|| panic!("census message, got: {message}"));
    for pair in body.split_whitespace() {
        let (key, value) = pair
            .split_once('=')
            .unwrap_or_else(|| panic!("census `key=value` pairs, got: {pair}"));
        let slot = match key {
            "lint" => 0,
            "warning" => 1,
            "fix" => 2,
            "assist" => 3,
            other => panic!("unknown census key `{other}`"),
        };
        counts[slot] = value
            .parse()
            .unwrap_or_else(|_| panic!("census count for `{key}`, got: {value}"));
    }
    (counts[0], counts[1], counts[2], counts[3])
}

/// Drives one overlay row through the full live lifecycle: manifest
/// discovery, `dart <entrypoint>` spawn (the child runs the real upstream
/// `register` before answering `initialize`), capability negotiation, one
/// engine diagnostics batch answered with the registration census, the
/// fixes face (or its capability gate), and graceful shutdown.
// One linear end-to-end scenario per row; carving it up would thread the
// runtime through helpers for no reader benefit (the compat test above
// keeps the same posture).
#[expect(clippy::too_many_lines, reason = "see above")]
fn drive_overlay(row: &OverlayRow) {
    let Some(dir) = overlay_ready(row.name) else {
        return;
    };

    // Discovery through the real manifest: an explicit override loads the
    // overlay directory (the convention scan is empty here).
    let report = discover(&Path::new(env!("CARGO_MANIFEST_DIR")).join(".."), &[dir]);
    assert!(
        report.skipped.is_empty(),
        "the overlay manifest must load cleanly: {:?}",
        report.skipped
    );
    assert_eq!(report.plugins.len(), 1);
    let discovered = &report.plugins[0];
    assert_eq!(discovered.manifest.name, row.name);
    assert_eq!(discovered.manifest.entrypoint, row.entrypoint);

    let mut runtime = PluginRuntime::spawn(SpawnSpec::dart(
        &discovered.dir,
        discovered.manifest.clone(),
    ))
    .expect("the overlay spawns via `dart <entrypoint>` and completes the handshake");
    assert_eq!(
        runtime.negotiated().map(CapabilitySet::names),
        Some(row.negotiated.to_vec()),
        "the negotiated set is the manifest's capability list"
    );

    // The engine's batch: one .dart row whose UTF-16 range spans the emoji's
    // surrogate pair (the position exchange's proof, as in the compat test).
    let mut files = FileTable::new();
    let a = files.intern(Path::new("lib/a.dart"));
    let a_path = files
        .path(a)
        .expect("interned")
        .to_string_lossy()
        .into_owned();
    let engine = vec![Diagnostic::new(
        Severity::Error,
        URI_DOES_NOT_EXIST,
        "Target of URI doesn't exist: 'x.dart'.",
        a,
        SourceRange {
            offset: 8,
            length: 2,
        },
        None,
    )];
    runtime
        .feed(&engine, &files, &CyclePush::default())
        .expect("engine feed");
    // `Duration::from_minutes` is not stable on the pinned toolchain, so
    // seconds remain the only expressible unit for this budget.
    #[expect(clippy::duration_suboptimal_units, reason = "see above")]
    let findings = runtime
        .wait_output(Duration::from_secs(60))
        .expect("overlay findings arrive within the drain budget")
        .diagnostics;

    // The census: the observable proof the upstream registration ran.
    assert_eq!(findings.len(), 1, "one census row per batch");
    assert_eq!(findings[0].file, a_path);
    assert_eq!(findings[0].code, format!("{}/census", row.name));
    assert_eq!(findings[0].severity, WireSeverity::Info);
    assert_eq!(
        parse_census(&findings[0].message),
        row.census,
        "registration census pins the clone revision"
    );

    match &row.fixes {
        Some(face) => {
            let reply = runtime
                .request_fixes(&a_path, 8, 2)
                .expect("fixes reply arrives within the callback budget");
            assert_eq!(
                reply.fixes.len(),
                face.count,
                "one change per registered generator"
            );
            assert_eq!(reply.fixes[0].message, face.first_message);
            let edit = &reply.fixes[0].edits[0];
            assert_eq!(
                (edit.offset, edit.length),
                (8, 2),
                "the requested UTF-16 range echoed"
            );
            assert_eq!(edit.replacement, face.replacement);
            // Applying the edit proves the UTF-16 exchange: offsets 8..10
            // span the emoji's surrogate pair, where UTF-16 and byte offsets
            // diverge.
            let start = utf16_offset_to_byte(A_DART, 8);
            let end = utf16_offset_to_byte(A_DART, 10);
            let mut fixed = String::with_capacity(A_DART.len());
            fixed.push_str(&A_DART[..start]);
            fixed.push_str(&edit.replacement);
            fixed.push_str(&A_DART[end..]);
            assert_eq!(
                fixed,
                format!("import '{}.dart';\nvoid main() {{}}\n", face.replacement),
                "the fix applied at the UTF-16 range the plugin echoed"
            );
        }
        None => {
            // Lint-only row: `request/fixes` is gated on the negotiated
            // `fix` capability and must never reach the plugin.
            let error = runtime
                .request_fixes(&a_path, 0, 0)
                .expect_err("the fix capability is not negotiated");
            assert!(
                matches!(
                    error,
                    RuntimeError::Protocol(SessionError::CapabilityNotNegotiated(Capability::Fix))
                ),
                "the gate is the capability check, got {error:?}"
            );
        }
    }

    runtime.shutdown().expect("graceful shutdown");
}

/// The content the engine holds for the other files of the set (the test
/// plays the engine's resident-text role in the merge assertion).
const B_DART: &str = "var x = 0;\n";
const NOTES_TXT: &str = "notes\n";

/// One query reply's params object: the answer rows under the schema's
/// envelope key (`elements` for `element_at`, `references` for the
/// name-keyed face).
fn query_reply(key: &str, rows: Value) -> Map<String, Value> {
    Map::from_iter([(key.to_owned(), rows)])
}

/// 0-based `(line, character)` for a UTF-16 offset — the daemon's `to_lsp`
/// position mapping through the line index.
fn lsp_position(text: &str, offset: u32) -> (u32, u32) {
    let index = LineIndex::new(text);
    let (line, column) = index.line_col_utf16(text, offset);
    (line - 1, column - 1)
}

/// One merged output row, the test-local shape of the daemon's
/// `(FileId, lsp::Diagnostic)` pair reduced to its merge-relevant fields.
struct MergedRow {
    file: String,
    line: u32,
    character: u32,
    code: String,
    source: String,
    offset: u64,
}

/// The daemon's `merge` (crates/dartq-daemon/src/plugins.rs), mirrored so
/// this suite can assert the sample's findings satisfy the output contract
/// without pulling the daemon crate into the plugin crate's test graph:
/// engine rows first in order, then plugin rows offset-sorted (stable), a
/// repeated `(file, range.start, code, source)` key dropped.
fn merge(engine: Vec<MergedRow>, mut findings: Vec<MergedRow>) -> Vec<MergedRow> {
    let mut merged = engine;
    let mut seen: FxHashSet<(String, u32, u32, String, String)> = merged
        .iter()
        .map(|row| {
            (
                row.file.clone(),
                row.line,
                row.character,
                row.code.clone(),
                row.source.clone(),
            )
        })
        .collect();
    findings.sort_by_key(|row| row.offset);
    for row in findings {
        let key = (
            row.file.clone(),
            row.line,
            row.character,
            row.code.clone(),
            row.source.clone(),
        );
        if seen.insert(key) {
            merged.push(row);
        }
    }
    merged
}

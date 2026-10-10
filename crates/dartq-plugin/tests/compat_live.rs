//! Live compatibility-sample E2E (M4-T9): the `samples/compat` plugin of the
//! sibling `dartq-plugin-host` package — an official `analysis_server_plugin`-
//! shaped plugin (see that sample's `README.md` for the porting guide) —
//! driven end to end through the runtime's contract: manifest discovery,
//! `dart <entrypoint>` launch (the exact daemon path), the diagnostics flow
//! over a file set, the daemon-side merge contract, the fixes face with a
//! UTF-16 edit applied to real text, and the crash mode surfaced as the
//! `dartq/plugin_crashed` diagnostic (PRD §14).
//!
//! Distinct from `live_host.rs` (the `bin/live_plugin` fixture): this drives
//! the on-disk sample with its real `dartq_plugin.json` manifest. Both tests
//! spawn a real `dart` process: `#[ignore]`d here so the serial verification
//! phase owns them — run with
//! `cargo test -p dartq-plugin --test compat_live -- --ignored`.
//! A missing `dart` skips at runtime (CI without the SDK must not go red);
//! children are reaped by the runtime's own shutdown paths.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use dartq_diagnostics::{Diagnostic, Severity, SourceRange, URI_DOES_NOT_EXIST};
use dartq_plugin::{
    CapabilitySet, Manifest, PluginRuntime, RuntimeError, Severity as WireSeverity, SpawnSpec,
    StdioPolicy, crash_diagnostic, discover,
};
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

#[test]
#[ignore = "spawns a live `dart` process; serial verification phase: cargo test -p dartq-plugin --test compat_live -- --ignored"]
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

    // The file set: two .dart files (resident text held by the test) and one
    // non-Dart file the rule must skip.
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
    runtime.feed(&engine, &files).expect("engine feed");
    let findings = runtime
        .wait_diagnostics(Duration::from_secs(60))
        .expect("plugin findings arrive within the drain budget");
    assert_eq!(findings.len(), 3, "the .txt row is not analyzed");
    assert_eq!(findings[0].file, a_path);
    assert_eq!(findings[0].code, "compat/companion");
    assert_eq!(findings[0].severity, WireSeverity::Warning);
    assert_eq!(findings[0].message, "companion for `uri_does_not_exist`");
    assert_eq!(
        findings[0].offset, 8,
        "UTF-16 surrogate-pair range carried through"
    );
    assert_eq!(findings[0].length, 2);
    assert_eq!(findings[1].offset, 10);
    assert_eq!(findings[1].length, 4);
    assert_eq!(findings[2].file, b_path);
    assert_eq!(findings[2].offset, 0);

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
            let text = if row.file == a_path { A_DART } else { B_DART };
            let offset = u32::try_from(row.offset).expect("in-range UTF-16 offset");
            let (line, character) = lsp_position(text, offset);
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
        7,
        "engine + plugin rows, nothing deduped away"
    );
    assert_eq!(merged[0].offset, 8, "engine rows first, in order");
    assert_eq!(merged[3].offset, 0, "the notes.txt engine row is intact");
    // Plugin rows sort by offset across the file set (the daemon's sort key
    // is the offset alone): b.dart@0, a.dart@8, a.dart@10.
    assert_eq!(
        (merged[4].file.as_str(), merged[4].offset),
        (b_path.as_str(), 0)
    );
    assert_eq!(
        (merged[5].file.as_str(), merged[5].offset),
        (a_path.as_str(), 8)
    );
    assert_eq!(
        (merged[6].file.as_str(), merged[6].offset),
        (a_path.as_str(), 10)
    );

    // The fixes face: the range fix anchored at the requested UTF-16 range,
    // then the message-only suggestion.
    let reply = runtime
        .request_fixes(&a_path, 10, 4)
        .expect("fixes reply arrives within the callback budget");
    assert_eq!(reply.fixes.len(), 2);
    assert_eq!(reply.fixes[0].priority, 1);
    assert_eq!(reply.fixes[0].message, "Replace range with a compat marker");
    assert_eq!(reply.fixes[0].edits.len(), 1);
    let edit = &reply.fixes[0].edits[0];
    assert_eq!(edit.offset, 10);
    assert_eq!(edit.length, 4);
    assert_eq!(edit.replacement, "/* compat: fixed */");
    assert_eq!(reply.fixes[1].priority, 50);
    assert_eq!(reply.fixes[1].message, "Review this location");
    assert!(reply.fixes[1].edits.is_empty());

    // Applying the edit proves the UTF-16 exchange: offsets 10..4 sit past
    // the emoji, where UTF-16 and byte offsets diverge.
    let start = utf16_offset_to_byte(A_DART, 10);
    let end = utf16_offset_to_byte(A_DART, 14);
    let mut fixed = String::with_capacity(A_DART.len());
    fixed.push_str(&A_DART[..start]);
    fixed.push_str(&edit.replacement);
    fixed.push_str(&A_DART[end..]);
    assert_eq!(
        fixed, "import '\u{1F600}/* compat: fixed */t';\nvoid main() {}\n",
        "the fix applied at the UTF-16 range the plugin echoed"
    );

    let stats = runtime.stats();
    assert_eq!(stats.restarts, 0);
    assert!(stats.peak_rss_bytes > 0, "child RSS must be sampled");
    runtime.shutdown().expect("graceful shutdown");
}

#[test]
#[ignore = "spawns a live `dart` process; serial verification phase: cargo test -p dartq-plugin --test compat_live -- --ignored"]
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

/// The content the engine holds for the other files of the set (the test
/// plays the engine's resident-text role in the merge assertion).
const B_DART: &str = "var x = 0;\n";
const NOTES_TXT: &str = "notes\n";

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
    let mut seen: HashSet<(String, u32, u32, String, String)> = merged
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

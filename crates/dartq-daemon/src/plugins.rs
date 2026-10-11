//! Plugin lifecycle orchestration for the daemon (PRD §14): discovery,
//! eager spawn, the per-cycle engine-diagnostics feed, the crash→diagnostic
//! bridge, and the deterministic merge of plugin findings into the LSP
//! output. The thin daemon-side layer `dartq-plugin` deliberately leaves to
//! its output consumer.
//!
//! # Representation contract (the merge's type-level boundary)
//!
//! Plugin *findings* convert directly to [`lsp::Diagnostic`] with the
//! plugin's manifest name as the LSP `source` tag. They never pass through
//! the engine [`Diagnostic`] — its `CodeInfo` requires `&'static` code
//! names, and plugin codes are free-form strings. Only
//! [`crash_diagnostic`] output (already a legal engine `Diagnostic`) rides
//! the engine diagnostics vec; the session's convert-once pass then carries
//! it into publishes and answers.
//!
//! # Blocking posture (synchronous-core policy)
//!
//! Every face here blocks the calling thread — the main loop's thread — by
//! design: [`PluginSet::launch`] spends up to the manifest's
//! `request_ms × (retries + 1)` per plugin on the spawn handshake (the
//! manifest defaults 10 s × 4, PRD §14.3); [`PluginSet::cycle`] spends at
//! most one [`DRAIN_BUDGET`] per plugin per cycle waiting for findings.
//! Findings that arrive after a cycle's budget publish on the next cycle.
//!
//! # Known-accepted postures
//!
//! - `feed(&[])` sends nothing: there is no "diagnostics cleared" signal to
//!   a plugin (the transport has none). Accepted for this milestone.
//! - The restart budget never resets (PRD semantics): the runtime relaunches
//!   a dead child lazily within the manifest's `request_retries`, and a
//!   spent budget is terminal for the session.
//! - A crash row recorded while the session has no anchor file is withheld,
//!   not dropped: it publishes on the first cycle that has one.
//! - A plugin row whose offsets fall outside the resident file (the classic
//!   stale row surviving a shrink edit) clamps to the file's UTF-16 end in
//!   [`to_lsp`]: the row publishes degraded, it never panics the mapping.

use std::path::Path;
use std::time::Duration;

use dartq_diagnostics::{Diagnostic, FileId};
use dartq_plugin::{
    PluginDiagnostic, PluginRuntime, RuntimeError, Severity as PluginSeverity, SpawnSpec,
    crash_diagnostic, discover,
};
use dartq_source::{FileTable, LineIndex, utf16_len_of};
use rustc_hash::FxHashSet;

use crate::lsp;

/// Per-cycle wait for plugin findings (the bounded drain): long enough for a
/// warm plugin's push, short enough to keep the main loop responsive. Rows
/// arriving later publish on the next cycle.
const DRAIN_BUDGET: Duration = Duration::from_millis(50);

/// Every plugin discovered under the workspace's `plugins/` convention, in
/// discovery order, each with its live runtime or its terminal failure.
///
/// Dropping the set without [`PluginSet::shutdown`] kills the child
/// processes (the runtime's `Drop`) without the graceful handshake — the
/// session end must call [`PluginSet::shutdown`]. `Default` is the empty
/// set: the session takes the set at teardown (`mem::take`), and an
/// all-skipped discovery leaves the empty default in place.
#[derive(Default)]
pub struct PluginSet {
    plugins: Vec<PluginSlot>,
}

/// One plugin's session state. `runtime: None` marks the plugin dead for
/// this session: the first surfaced runtime error means the runtime already
/// spent its restart budget reaching it, and further interaction would burn
/// the manifest's request budget on every cycle for a plugin that cannot
/// come back.
struct PluginSlot {
    /// The manifest name — the LSP `source` tag and the crash-message
    /// subject.
    name: String,
    runtime: Option<PluginRuntime>,
    /// A recorded failure awaiting an anchor file to publish against
    /// (recorded when the failure surfaced before any anchor existed).
    pending_crash: Option<Box<RuntimeError>>,
    /// At most one crash row per plugin per session: repeats are suppressed.
    crash_emitted: bool,
}

impl PluginSet {
    /// Discovers plugins under `<workspace_root>/plugins/` (the session's
    /// `config_dir.parent()` convention; no overrides this milestone) and
    /// spawns each hit eagerly, completing the handshake before returning.
    ///
    /// Spawn failures never fail the session: they are logged here and
    /// surface as their `dartq/plugin_crashed` row on the first cycle with
    /// an anchor file. Skipped manifests (malformed, missing entrypoint,
    /// duplicate name) are logged and ignored.
    #[must_use]
    pub fn launch(workspace_root: &Path) -> Self {
        let report = discover(workspace_root, &[]);
        for issue in &report.skipped {
            eprintln!(
                "dartq-daemon: skipping plugin manifest `{}`: {}",
                issue.path.display(),
                issue.error
            );
        }
        let plugins = report
            .plugins
            .into_iter()
            .map(|plugin| {
                let name = plugin.manifest.name.clone();
                match PluginRuntime::spawn(SpawnSpec::dart(&plugin.dir, plugin.manifest)) {
                    Ok(runtime) => PluginSlot {
                        name,
                        runtime: Some(runtime),
                        pending_crash: None,
                        crash_emitted: false,
                    },
                    Err(error) => {
                        eprintln!("dartq-daemon: plugin `{name}` failed to launch: {error}");
                        PluginSlot {
                            name,
                            runtime: None,
                            pending_crash: Some(Box::new(error)),
                            crash_emitted: false,
                        }
                    }
                }
            })
            .collect();
        Self { plugins }
    }

    /// True when no plugin was discovered — the per-cycle hook skips its
    /// work entirely (and the session skips the drain latency with it).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    /// A set carrying one already-dead plugin: `runtime: None` with a
    /// recorded failure awaiting an anchor, exactly what `launch` produces
    /// when a spawn fails. Test seam for the session-side weave — reaching
    /// the same state through the launch path needs a real child process
    /// (and a manifest whose entrypoint exists; missing-entrypoint
    /// manifests are *skipped*, never spawned-failed, so a session gate
    /// cannot conjure a crash from them deterministically). The crash-row
    /// generation itself is pinned by this module's tests; the session's
    /// gate pins the weave: anchor resolution, convert-once inclusion,
    /// merge order, once-per-session suppression.
    #[cfg(test)]
    pub(crate) fn with_dead_plugin(name: &str, reason: &str) -> Self {
        Self {
            plugins: vec![PluginSlot {
                name: name.to_owned(),
                runtime: None,
                pending_crash: Some(Box::new(RuntimeError::Crashed {
                    reason: reason.to_owned(),
                })),
                crash_emitted: false,
            }],
        }
    }

    /// One analyze cycle against the plugins: feeds the engine's merged
    /// diagnostic set (the lints included by the caller), waits
    /// [`DRAIN_BUDGET`] per live plugin for findings, and returns both.
    ///
    /// `anchor` is the crash anchor, resolved by the caller as the session
    /// file table's first entry — `None` when the table is empty, in which
    /// case crash rows are withheld until a cycle that has one. Crash rows
    /// are engine `Diagnostic`s the caller extends into its engine vec
    /// before the convert-once pass; at most one is emitted per plugin per
    /// session.
    ///
    /// Findings are grouped per plugin in discovery order with their wire
    /// rows intact; the caller resolves them onto resident files and merges
    /// ([`resolve_findings`] + [`merge`]).
    #[must_use]
    pub fn cycle(
        &mut self,
        engine: &[Diagnostic],
        files: &FileTable,
        anchor: Option<FileId>,
    ) -> Cycle {
        let mut cycle = Cycle {
            findings: Vec::new(),
            crashes: Vec::new(),
        };
        for slot in &mut self.plugins {
            // A crash recorded before any anchor existed publishes here.
            if let (Some(error), Some(anchor)) = (&slot.pending_crash, anchor) {
                cycle
                    .crashes
                    .push(crash_diagnostic(&slot.name, error, anchor));
                slot.pending_crash = None;
                slot.crash_emitted = true;
            }
            let Some(runtime) = slot.runtime.as_mut() else {
                continue;
            };
            let outcome = runtime
                .feed(engine, files)
                .and_then(|()| runtime.wait_diagnostics(DRAIN_BUDGET));
            match outcome {
                Ok(rows) if !rows.is_empty() => {
                    cycle.findings.push(PluginBatch {
                        name: slot.name.clone(),
                        rows,
                    });
                }
                Ok(_) => {}
                Err(error) => {
                    // First surfaced error ends the plugin's session (the
                    // isolation contract: the runtime has already spent its
                    // budget; the engine turn stays alive).
                    slot.runtime = None;
                    if slot.crash_emitted {
                        continue;
                    }
                    if let Some(anchor) = anchor {
                        cycle
                            .crashes
                            .push(crash_diagnostic(&slot.name, &error, anchor));
                        slot.crash_emitted = true;
                    } else {
                        slot.pending_crash = Some(Box::new(error));
                    }
                }
            }
        }
        cycle
    }

    /// The graceful session-end teardown: `shutdown` per live runtime (the
    /// graceful handshake; `Drop` alone is kill-only). Errors are logged,
    /// never propagated — teardown quality does not affect the session's
    /// outcome. Each live plugin emits one structured §14.3 stats line (the
    /// evidence round's per-plugin numbers); the snapshot is read before the
    /// handshake consumes the runtime, and a dead slot's counters died with
    /// its dropped runtime — its crash row already recorded the death.
    pub fn shutdown(self) {
        for slot in self.plugins {
            if let Some(runtime) = slot.runtime {
                let name = &slot.name;
                let stats = runtime.stats();
                if let Err(error) = runtime.shutdown() {
                    eprintln!("dartq-daemon: plugin `{name}` shutdown failed: {error}");
                }
                eprintln!(
                    "dartq-daemon: plugin stats `{name}`: peak_rss_bytes={} restarts={} \
                     frames_in={} frames_out={}",
                    stats.peak_rss_bytes, stats.restarts, stats.frames_in, stats.frames_out,
                );
            }
        }
    }
}

/// One cycle's plugin output.
pub struct Cycle {
    /// Findings per emitting plugin, in discovery order; empty batches are
    /// omitted.
    pub findings: Vec<PluginBatch>,
    /// Crash rows for the caller's engine vec (at most one per plugin per
    /// session).
    pub crashes: Vec<Diagnostic>,
}

/// One plugin's drained findings, in wire arrival order.
pub struct PluginBatch {
    /// The manifest name (the LSP `source` tag for these rows).
    pub name: String,
    pub rows: Vec<PluginDiagnostic>,
}

/// One plugin finding resolved onto a resident file, ready for [`merge`].
pub struct PluginFinding {
    pub file: FileId,
    /// Emitting plugin's discovery-order index (the merge's grouping key).
    pub plugin: usize,
    /// The wire row's UTF-16 start offset (the within-plugin sort key).
    pub offset: u64,
    pub diagnostic: lsp::Diagnostic,
}

/// Resolves drained plugin rows onto the files they name and converts them
/// to the LSP shape. `resident` answers a `FileId` with its line index and
/// text (the session's engine faces); rows on files the engine does not
/// know are skipped — the same posture the transport's own feed takes for
/// unknown `FileId`s.
///
/// Offsets are UTF-16 end to end: the wire rows already carry UTF-16 units
/// and `line_col_utf16` consumes the same, so this is a position mapping,
/// never a unit conversion.
#[must_use]
pub fn resolve_findings<'a>(
    batches: &[PluginBatch],
    files: &FileTable,
    mut resident: impl FnMut(FileId) -> Option<(&'a LineIndex, &'a str)>,
) -> Vec<PluginFinding> {
    let mut findings = Vec::new();
    for (plugin, batch) in batches.iter().enumerate() {
        for row in &batch.rows {
            let Some(file) = files.try_file_id(Path::new(&row.file)) else {
                continue;
            };
            let Some((index, text)) = resident(file) else {
                continue;
            };
            findings.push(PluginFinding {
                file,
                plugin,
                offset: row.offset,
                diagnostic: to_lsp(&batch.name, row, index, text),
            });
        }
    }
    findings
}

/// Converts one wire row to the LSP shape: severity mapped to the LSP
/// 1/2/3 scale, UTF-16 offsets mapped through the line index to zero-based
/// positions, the plugin's manifest name as the `source` tag.
///
/// Plugin offsets are untrusted (the §14.3 isolation posture): a stale row
/// can predate an edit that shrank the file. Both bounds clamp to the
/// text's UTF-16 length before mapping — the file-end degradation
/// [`wire_offset`] starts — so a malformed row degrades to zero-width at
/// the end instead of tripping [`LineIndex::line_col_utf16`]'s
/// engine-internal assert. The monotone clamp keeps `start <= end`.
fn to_lsp(plugin: &str, row: &PluginDiagnostic, index: &LineIndex, text: &str) -> lsp::Diagnostic {
    let total = utf16_len_of(text);
    let start = wire_offset(row.offset).min(total);
    let end = wire_offset(row.offset.saturating_add(row.length)).min(total);
    let (start_line, start_column) = index.line_col_utf16(text, start);
    let (end_line, end_column) = index.line_col_utf16(text, end);
    let severity = match row.severity {
        PluginSeverity::Error => 1,
        PluginSeverity::Warning => 2,
        PluginSeverity::Info => 3,
    };
    lsp::Diagnostic {
        range: lsp::Range {
            start: lsp::Position {
                line: start_line - 1,
                character: start_column - 1,
            },
            end: lsp::Position {
                line: end_line - 1,
                character: end_column - 1,
            },
        },
        severity,
        code: row.code.clone(),
        source: plugin.to_owned(),
        message: row.message.clone(),
    }
}

/// Clamps a wire offset into the engine's u32 UTF-16 range; the resident
/// text's UTF-16-length bound is applied on top in [`to_lsp`].
fn wire_offset(offset: u64) -> u32 {
    u32::try_from(offset).unwrap_or(u32::MAX)
}

/// The dedup key of the merge: file, range start, code, and source.
type DedupKey = (u32, u32, u32, String, String);

fn dedup_key(file: FileId, diagnostic: &lsp::Diagnostic) -> DedupKey {
    (
        file.index(),
        diagnostic.range.start.line,
        diagnostic.range.start.character,
        diagnostic.code.clone(),
        diagnostic.source.clone(),
    )
}

/// Merges the cycle's engine rows (as `(file, converted)` pairs, in their
/// existing order) with the resolved plugin findings into one deterministic
/// sequence: engine rows first in the given order, then plugin rows grouped
/// by plugin discovery order, offset-sorted within each plugin (the stable
/// sort keeps wire arrival order for equal offsets). A row whose
/// `(file, range.start, code, source)` key already appeared is dropped —
/// a plugin re-emitting per cycle cannot churn publishes.
#[must_use]
pub fn merge(
    engine_rows: impl IntoIterator<Item = (FileId, lsp::Diagnostic)>,
    findings: Vec<PluginFinding>,
) -> Vec<(FileId, lsp::Diagnostic)> {
    let mut merged: Vec<(FileId, lsp::Diagnostic)> = engine_rows.into_iter().collect();
    let mut seen: FxHashSet<DedupKey> = merged
        .iter()
        .map(|(file, diagnostic)| dedup_key(*file, diagnostic))
        .collect();
    let mut findings = findings;
    findings.sort_by_key(|finding| (finding.plugin, finding.offset));
    for finding in findings {
        if seen.insert(dedup_key(finding.file, &finding.diagnostic)) {
            merged.push((finding.file, finding.diagnostic));
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

    use dartq_diagnostics::{
        CodeInfo, DARTQ_PLUGIN_CRASHED, DiagnosticType, Severity as EngineSeverity, SourceRange,
    };

    use super::*;

    /// A throwaway workspace tree, removed on drop (a panicked test never
    /// leaks its fixtures into the shared temp dir).
    struct TempWorkspace(PathBuf);

    impl TempWorkspace {
        fn new(tag: &str) -> Self {
            static SEQ: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "dartq-daemon-plugins-{tag}-{}-{}",
                std::process::id(),
                SEQ.fetch_add(1, Relaxed),
            ));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join("plugins")).expect("create workspace");
            Self(root)
        }

        fn write_plugin(&self, name: &str, files: &[(&str, &str)]) {
            let dir = self.0.join("plugins").join(name);
            std::fs::create_dir_all(&dir).expect("create plugin dir");
            for (file, content) in files {
                std::fs::write(dir.join(file), content).expect("write plugin file");
            }
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempWorkspace {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn lsp_row(code: &str, source: &str, line: u32, character: u32) -> lsp::Diagnostic {
        lsp::Diagnostic {
            range: lsp::Range {
                start: lsp::Position { line, character },
                end: lsp::Position { line, character },
            },
            severity: 2,
            code: code.to_owned(),
            source: source.to_owned(),
            message: "m".to_owned(),
        }
    }

    fn finding(
        file: FileId,
        plugin: usize,
        offset: u64,
        code: &str,
        source: &str,
    ) -> PluginFinding {
        PluginFinding {
            file,
            plugin,
            offset,
            // The start mirrors the offset so distinct offsets carry
            // distinct dedup keys, as real converted rows would.
            diagnostic: lsp_row(code, source, 0, offset as u32),
        }
    }

    fn wire_row(file: &str, code: &str, offset: u64, length: u64) -> PluginDiagnostic {
        PluginDiagnostic {
            file: file.to_owned(),
            severity: PluginSeverity::Warning,
            code: code.to_owned(),
            message: "watch out".to_owned(),
            offset,
            length,
        }
    }

    fn engine_row(file: FileId, message: &str) -> Diagnostic {
        Diagnostic::new(
            EngineSeverity::Warning,
            CodeInfo::new("test/row", "TEST_ROW", DiagnosticType::Tool, false),
            message,
            file,
            SourceRange {
                offset: 0,
                length: 2,
            },
            None,
        )
    }

    #[test]
    fn merge_orders_engine_first_then_plugin_groups_by_discovery_order() {
        let mut files = FileTable::new();
        let a = files.intern(Path::new("a.dart"));
        let b = files.intern(Path::new("b.dart"));

        let engine_rows = [
            (a, lsp_row("e/one", "dartq", 0, 0)),
            (a, lsp_row("e/two", "dartq", 1, 0)),
        ];
        let findings = vec![
            finding(b, 1, 1, "p1/a", "p1"),
            finding(a, 0, 5, "p0/b", "p0"),
            finding(a, 0, 2, "p0/a", "p0"),
        ];

        let merged = merge(engine_rows, findings);
        let codes: Vec<&str> = merged.iter().map(|(_, d)| d.code.as_str()).collect();
        assert_eq!(codes, ["e/one", "e/two", "p0/a", "p0/b", "p1/a"]);
        assert_eq!(merged[4].0, b, "the plugin-1 row keeps its own file");
    }

    #[test]
    fn merge_dedups_on_file_start_code_and_source() {
        let mut files = FileTable::new();
        let a = files.intern(Path::new("a.dart"));

        let engine_rows = [(a, lsp_row("shared/code", "dartq", 0, 0))];
        let findings = vec![
            // Different source than the engine row: kept.
            finding(a, 0, 0, "shared/code", "echo"),
            // Same (file, start, code, source) as the row above: dropped.
            finding(a, 0, 0, "shared/code", "echo"),
            // Same code and source, different start: kept.
            finding(a, 0, 3, "shared/code", "echo"),
        ];

        let merged = merge(engine_rows, findings);
        let keys: Vec<(&str, &str)> = merged
            .iter()
            .map(|(_, d)| (d.code.as_str(), d.source.as_str()))
            .collect();
        assert_eq!(
            keys,
            [
                ("shared/code", "dartq"),
                ("shared/code", "echo"),
                ("shared/code", "echo")
            ]
        );
        let starts: Vec<(u32, u32)> = merged[1..]
            .iter()
            .map(|(_, d)| (d.range.start.line, d.range.start.character))
            .collect();
        assert_eq!(starts, [(0, 0), (0, 3)], "offset-sorted within the plugin");
    }

    #[test]
    fn resolve_findings_maps_positions_and_skips_unresolvable_files() {
        let mut files = FileTable::new();
        let a = files.intern(Path::new("a.dart"));
        let resident_never = files.intern(Path::new("b.dart"));

        let index = LineIndex::new("ab\ncd\ne");
        let text = "ab\ncd\ne";
        let resident = |file: FileId| (file == a).then_some((&index, text));

        let batches = [PluginBatch {
            name: "echo".to_owned(),
            rows: vec![
                wire_row("a.dart", "echo/roundtrip", 3, 2),
                wire_row("ghost.dart", "echo/ghost", 0, 0),
                wire_row("b.dart", "echo/not_resident", 0, 0),
            ],
        }];

        let findings = resolve_findings(&batches, &files, resident);
        assert_eq!(findings.len(), 1, "unknown and non-resident files skip");
        let row = &findings[0];
        assert_eq!(row.file, a);
        assert_eq!(row.plugin, 0);
        assert_eq!(row.offset, 3);
        // "ab\ncd\ne": offset 3 starts line 2 col 1; the row's end offset 5
        // (the newline terminating line 2) answers line 2 col 3 — one-based
        // line_col answers, LSP zero-based positions out.
        assert_eq!(
            row.diagnostic.range.start,
            lsp::Position {
                line: 1,
                character: 0
            }
        );
        assert_eq!(
            row.diagnostic.range.end,
            lsp::Position {
                line: 1,
                character: 2
            }
        );
        assert_eq!(row.diagnostic.severity, 2);
        assert_eq!(row.diagnostic.code, "echo/roundtrip");
        assert_eq!(row.diagnostic.source, "echo");
        assert_eq!(row.diagnostic.message, "watch out");
        assert_eq!(resident_never.index(), 1, "the interned id is exercised");
    }

    #[test]
    fn resolve_findings_clamps_out_of_range_offset_to_file_end() {
        let mut files = FileTable::new();
        let a = files.intern(Path::new("a.dart"));

        let index = LineIndex::new("ab\ncd\ne");
        let text = "ab\ncd\ne";
        let resident = |file: FileId| (file == a).then_some((&index, text));

        // The classic stale row: offsets taken before an edit shrank the
        // file far below them. The mapping must degrade, not panic.
        let batches = [PluginBatch {
            name: "echo".to_owned(),
            rows: vec![wire_row("a.dart", "echo/stale", 10_000_000, 4)],
        }];

        let findings = resolve_findings(&batches, &files, resident);
        assert_eq!(findings.len(), 1, "the row publishes degraded");
        // Both bounds clamp to the file's UTF-16 end (7 units): zero-width
        // at EOF ("e" is line 3, so the 1-based (3, 2) answers LSP (2, 1)).
        let eof = lsp::Position {
            line: 2,
            character: 1,
        };
        assert_eq!(findings[0].diagnostic.range.start, eof);
        assert_eq!(findings[0].diagnostic.range.end, eof);
    }

    #[test]
    fn resolve_findings_clamps_bounds_crossing_the_file_end() {
        let mut files = FileTable::new();
        let a = files.intern(Path::new("a.dart"));

        let index = LineIndex::new("ab\ncd\ne");
        let text = "ab\ncd\ne";
        let resident = |file: FileId| (file == a).then_some((&index, text));

        // Start in range with the length running past EOF, and start beyond
        // EOF with the end further still — the clamp keeps `start <= end`
        // in both shapes.
        let batches = [PluginBatch {
            name: "echo".to_owned(),
            rows: vec![
                wire_row("a.dart", "echo/long_tail", 5, 1_000_000),
                wire_row("a.dart", "echo/past_end", 100, 200),
            ],
        }];

        let findings = resolve_findings(&batches, &files, resident);
        assert_eq!(findings.len(), 2);
        let eof = lsp::Position {
            line: 2,
            character: 1,
        };
        let range_of = |code: &str| {
            &findings
                .iter()
                .find(|f| f.diagnostic.code == code)
                .expect("row present")
                .diagnostic
                .range
        };
        // Offset 5 survives intact (the newline ending line 2 → LSP (1, 2));
        // the out-of-range end clamps to EOF.
        assert_eq!(
            range_of("echo/long_tail").start,
            lsp::Position {
                line: 1,
                character: 2
            }
        );
        assert_eq!(range_of("echo/long_tail").end, eof);
        // Both bounds past EOF: zero-width at the end.
        assert_eq!(range_of("echo/past_end").start, eof);
        assert_eq!(range_of("echo/past_end").end, eof);
    }

    fn dead_set(reason: &str) -> PluginSet {
        PluginSet {
            plugins: vec![PluginSlot {
                name: "echo".to_owned(),
                runtime: None,
                pending_crash: Some(Box::new(RuntimeError::Crashed {
                    reason: reason.to_owned(),
                })),
                crash_emitted: false,
            }],
        }
    }

    #[test]
    fn crash_row_emitted_once_per_plugin_per_session() {
        let mut files = FileTable::new();
        let a = files.intern(Path::new("a.dart"));
        let mut set = dead_set("restart budget of 3 exhausted");

        let first = set.cycle(&[], &files, Some(a));
        assert_eq!(first.crashes.len(), 1);
        assert!(first.findings.is_empty());
        let crash = &first.crashes[0];
        assert_eq!(crash.code.name, DARTQ_PLUGIN_CRASHED.name);
        assert_eq!(crash.severity, EngineSeverity::Error);
        assert_eq!(
            crash.range,
            SourceRange {
                offset: 0,
                length: 0
            }
        );
        assert_eq!(crash.file, a);
        assert!(crash.message.contains("plugin `echo`"));

        let second = set.cycle(&[], &files, Some(a));
        assert!(second.crashes.is_empty(), "suppressed after the first");
    }

    #[test]
    fn crash_row_held_until_an_anchor_exists() {
        let mut files = FileTable::new();
        let a = files.intern(Path::new("a.dart"));
        let mut set = dead_set("plugin process closed the connection");

        let anchored = set.cycle(&[], &files, None);
        assert!(anchored.crashes.is_empty(), "nothing to anchor against");

        let with_anchor = set.cycle(&[], &files, Some(a));
        assert_eq!(with_anchor.crashes.len(), 1, "the held row publishes once");

        let again = set.cycle(&[], &files, Some(a));
        assert!(again.crashes.is_empty(), "still once per session");
    }

    #[test]
    fn launch_skips_broken_manifests_and_empty_workspaces() {
        let workspace = TempWorkspace::new("broken");
        workspace.write_plugin("bad_json", &[("dartq_plugin.json", "{not json")]);
        workspace.write_plugin(
            "no_script",
            &[(
                "dartq_plugin.json",
                r#"{"name": "no_script", "version": "0.1.0", "entrypoint": "missing.dart"}"#,
            )],
        );
        // A subdirectory without a manifest is not a plugin.
        std::fs::create_dir_all(workspace.path().join("plugins").join("notes"))
            .expect("create notes dir");

        let set = PluginSet::launch(workspace.path());
        assert!(set.is_empty(), "every manifest was skipped");

        let empty = TempWorkspace::new("empty");
        std::fs::remove_dir(empty.path().join("plugins")).expect("drop plugins dir");
        assert!(PluginSet::launch(empty.path()).is_empty());
    }

    /// Serial phase: spawns a real `dart` plugin process (the host package's
    /// echo fixture) and exercises launch → feed → drain → shutdown end to
    /// end. Skipped in the ordinary suite — lanes cannot execute dart.
    #[test]
    #[ignore = "serial phase: spawns a real dart plugin process"]
    fn live_launch_cycle_and_shutdown_with_the_echo_fixture() {
        let dart_ok = std::process::Command::new("dart")
            .arg("--version")
            .output()
            .is_ok();
        assert!(dart_ok, "dart is not on PATH");

        let script =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../dartq-plugin-host/bin/echo_plugin.dart");
        let workspace = TempWorkspace::new("live");
        let manifest = format!(
            r#"{{"name": "echo", "version": "0.1.0", "entrypoint": "{}",
                "capabilities": ["lint"],
                "timeouts": {{"callback_ms": 30000, "request_ms": 120000, "request_retries": 3}}}}"#,
            script.display(),
        );
        workspace.write_plugin("echo", &[("dartq_plugin.json", &manifest)]);

        let mut set = PluginSet::launch(workspace.path());
        assert!(!set.is_empty(), "the echo fixture discovered and launched");

        let mut files = FileTable::new();
        let a = files.intern(&workspace.path().join("lib").join("a.dart"));
        let engine_rows = [engine_row(a, "hello")];

        // A cold `dart` boot rides the first cycle's feed (the handshake);
        // the echo reply lands within a later cycle's bounded drain.
        let mut batch = None;
        for _ in 0..200 {
            let cycle = set.cycle(&engine_rows, &files, Some(a));
            assert!(cycle.crashes.is_empty(), "the echo fixture must not crash");
            if let Some(found) = cycle.findings.into_iter().next() {
                batch = Some(found);
                break;
            }
        }
        let batch = batch.expect("echo replied within the cycle budget");
        assert_eq!(batch.name, "echo");
        assert_eq!(batch.rows.len(), 1);
        let row = &batch.rows[0];
        assert_eq!(row.code, "echo/roundtrip");
        assert_eq!(
            row.file,
            files.path(a).unwrap_or(Path::new("")).to_string_lossy()
        );

        set.shutdown();
    }
}

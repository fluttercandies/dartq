//! Engine events (engine-api-spec §1.4, incremental-l1-spec §4.2) and the
//! `--trace-events` JSONL face (PRD §17.4).
//!
//! Delivered synchronously through the `&mut dyn FnMut(EngineEvent)`
//! callbacks of [`apply_pending`](crate::Engine::apply_pending) and
//! [`analyze`](crate::Engine::analyze). Events fire only when work happened:
//! an empty pending queue emits nothing, and a queue whose every entry hits
//! the ContentId gate emits `OverlayApplied` plus per-file `ContentIdHit`
//! `FileRevalidated`s and an empty `CascadeInvalidated`.
//!
//! # The trace face (§17.4)
//!
//! [`TraceSink`] records one JSON object per event, field order fixed by
//! the record structs (CI sampling compares captures byte-wise), flushing
//! per event — events are rare boundary observations, and a killed CI run
//! must not lose its tail. Writes are best-effort: a failed write is
//! remembered by the writer (later writes no-op) and never fails the
//! analysis it observes. The taxonomy maps as: `OverlayApplied`,
//! `CascadeInvalidated`, `MemoryDegrade` verbatim; `FileRevalidated`'s
//! `ast_cache_hit` is the §17.4 `AstCacheHit` observable;
//! `LibraryAnalyzed.from_cache` is the §19.3 Cache Hit Rate seed (aggregated
//! by [`cache_hit_rate`]). The remaining taxonomy names (`StoreHit`,
//! `SnapshotReuse`, `SignatureUnchanged`, `ContextBuilt`,
//! `SchedulerQueueDepth`, `PluginLatency`) land as variants with their
//! emitters once the M1 engine produces their data. Wave W1 of the L3 plan
//! (g485) adds one additive trace-only row, `CycleRebuilt` — the
//! condensation rebuild at the `structure_dirty` gate; deliberately no
//! `EngineEvent` variant, so the event goldens stay byte-stable.

use std::io::Write as _;
use std::path::Path;
use std::time::Duration;

use dartq_source::FileId;
use serde::Serialize;

use crate::state::LibraryId;

/// In-process observation of apply/analyze work (§4.2 M1 subset).
#[derive(Debug)]
pub enum EngineEvent {
    /// Apply batch boundary: the pending queue that was just drained, split
    /// by gate outcome. Fired once per non-empty queue, before the per-file
    /// and cascade events of the same apply.
    OverlayApplied {
        /// Files whose ContentId differed from residency (work follows).
        changed: Vec<FileId>,
        /// Resident files removed by this apply.
        removed: Vec<FileId>,
    },
    /// Per-file gate outcome. `ContentIdHit` = unchanged bytes, zero work.
    /// `Parsed` = revalidated; `ast_cache_hit` = the (content, feature set)
    /// key was already computed — hit the facts cache or an identical
    /// sibling earlier in the same batch — so no re-scan/re-parse ran.
    FileRevalidated {
        /// The revalidated file.
        file: FileId,
        /// Which gate branch fired.
        gate: RevalidateGate,
    },
    /// Per-library recompute outcome; `from_cache` is the Cache Hit Rate
    /// seed (PRD §19.3). `duration` is an additive delta against
    /// incremental-l1-spec §4.2's draft payload — it implements the
    /// benchmark harness's per-library duration hook (§3 of
    /// engine-api-spec) inside the event the API already has.
    LibraryAnalyzed {
        /// The recomputed (or cache-served) library.
        library: LibraryId,
        /// `true` when the result came from the per-library cache.
        from_cache: bool,
        /// Wall time of the recompute; zero for cache hits.
        duration: Duration,
    },
    /// Post-closure dirty set, once per non-empty apply (possibly empty —
    /// the ContentId-gate no-op case emits the empty set on purpose, so a
    /// zero-cascade step is assertable).
    CascadeInvalidated {
        /// Every library marked dirty by this apply, gate-to-closure.
        libraries: Vec<LibraryId>,
    },
    /// A memory-budget tier fired (PRD §17.1's ladder; §17.4's
    /// `MemoryDegrade`). `tier` is the machine key (`lazy_parse` |
    /// `semantic_slices` | `ast_cold`); `file` anchors the event the same
    /// way the paired `dartq/memory_degraded` diagnostic anchors. Fires at
    /// most once per tier per engine — the ladder is sticky and one-way.
    MemoryDegrade {
        /// The machine key of the §17.1 tier that fired.
        tier: &'static str,
        /// The armed budget limit, in MiB.
        limit_mib: u64,
        /// The anchor file (a dirty library's defining file, else any live
        /// library's).
        file: FileId,
    },
}

/// The per-file revalidation branch of [`EngineEvent::FileRevalidated`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevalidateGate {
    /// Bytes identical to residency: zero work (official
    /// `file_tracker.dart:161` `nothing` branch).
    ContentIdHit,
    /// Content changed and was revalidated; `ast_cache_hit` says whether
    /// the parse was skipped (facts cache, or an identical-content sibling
    /// computed earlier in the same batch).
    Parsed { ast_cache_hit: bool },
}

/// The `--trace-events` sink (PRD §17.4): appends one JSON object per event
/// to `path`, field order fixed by the record structs below. Armed through
/// [`crate::Engine::set_trace_sink`]; an unarmed engine constructs nothing
/// and touches no event path.
pub struct TraceSink {
    writer: std::io::BufWriter<std::fs::File>,
}

impl TraceSink {
    /// Creates (truncating) the JSONL file at `path`.
    ///
    /// # Errors
    ///
    /// Any [`std::io::Error`] from opening the file — the caller decides
    /// how to surface it (the CLI exits `USAGE`).
    pub fn create(path: &Path) -> std::io::Result<Self> {
        Ok(Self {
            writer: std::io::BufWriter::new(std::fs::File::create(path)?),
        })
    }

    /// Writes one record line. Best-effort: a failed write leaves the
    /// writer in its error state (later writes no-op) and never propagates.
    fn write_line<T: Serialize>(&mut self, record: &T) {
        let _ = serde_json::to_writer(&mut self.writer, record);
        let _ = self.writer.write_all(b"\n");
        let _ = self.writer.flush();
    }

    /// Records [`EngineEvent::OverlayApplied`] (counts — per-file identity
    /// follows in the `FileRevalidated` lines of the same apply).
    pub(crate) fn overlay_applied(&mut self, changed: usize, removed: usize) {
        self.write_line(&OverlayAppliedTrace {
            event: "OverlayApplied",
            changed,
            removed,
        });
    }

    /// Records [`EngineEvent::FileRevalidated`]; the gate resolves to its
    /// machine form, `ast_cache_hit` present only for the parsed branch.
    pub(crate) fn file_revalidated(&mut self, uri: &str, gate: RevalidateGate) {
        let (gate, ast_cache_hit) = match gate {
            RevalidateGate::ContentIdHit => ("content_id_hit", None),
            RevalidateGate::Parsed { ast_cache_hit } => ("parsed", Some(ast_cache_hit)),
        };
        self.write_line(&FileRevalidatedTrace {
            event: "FileRevalidated",
            uri,
            gate,
            ast_cache_hit,
        });
    }

    /// Records [`EngineEvent::LibraryAnalyzed`] — the §19.3 Cache Hit Rate
    /// seed line.
    pub(crate) fn library_analyzed(&mut self, uri: &str, from_cache: bool, duration_us: u64) {
        self.write_line(&LibraryAnalyzedTrace {
            event: "LibraryAnalyzed",
            uri,
            from_cache,
            duration_us,
        });
    }

    /// Records [`EngineEvent::CascadeInvalidated`] (defining-file paths in
    /// closure order).
    pub(crate) fn cascade_invalidated(&mut self, libraries: &[String]) {
        self.write_line(&CascadeInvalidatedTrace {
            event: "CascadeInvalidated",
            libraries,
        });
    }

    /// Records [`EngineEvent::MemoryDegrade`] (§17.4 taxonomy, verbatim).
    pub(crate) fn memory_degrade(&mut self, tier: &'static str, limit_mib: u64, uri: &str) {
        self.write_line(&MemoryDegradeTrace {
            event: "MemoryDegrade",
            tier,
            limit_mib,
            uri,
        });
    }

    /// Records the condensation rebuild (g485 W1): the cycle count after the
    /// wholesale re-derivation at the `structure_dirty` gate. Trace-only —
    /// deliberately no `EngineEvent` variant (g485 D3), so the event goldens
    /// stay byte-stable.
    pub(crate) fn cycle_rebuilt(&mut self, cycles: usize) {
        self.write_line(&CycleRebuiltTrace {
            event: "CycleRebuilt",
            cycles,
        });
    }
}

/// Wall time as additive microseconds (integer — float durations would not
/// be byte-stable across platforms).
pub(crate) fn duration_us(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

/// PRD §19.3's Cache Hit Rate, aggregated over a `--trace-events` capture:
/// cache-served [`EngineEvent::LibraryAnalyzed`] lines over all of them.
/// `None` when the capture holds no library lines (nothing to rate).
pub fn cache_hit_rate(jsonl: &str) -> Option<f64> {
    let (hits, total) = jsonl
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter_map(|value| {
            let hit = value.get("from_cache")?.as_bool()?;
            value
                .get("event")?
                .as_str()?
                .eq("LibraryAnalyzed")
                .then_some(hit)
        })
        .fold((0u64, 0u64), |(hits, total), hit| {
            (hits + u64::from(hit), total + 1)
        });
    (total > 0).then(move || hits as f64 / total as f64)
}

/// Trace records — serde serializes struct fields in declaration order, so
/// each struct below IS the wire contract (§17.4: CI samples these bytes).
#[derive(Serialize)]
struct OverlayAppliedTrace {
    event: &'static str,
    changed: usize,
    removed: usize,
}

#[derive(Serialize)]
struct FileRevalidatedTrace<'a> {
    event: &'static str,
    uri: &'a str,
    gate: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    ast_cache_hit: Option<bool>,
}

#[derive(Serialize)]
struct LibraryAnalyzedTrace<'a> {
    event: &'static str,
    uri: &'a str,
    from_cache: bool,
    duration_us: u64,
}

#[derive(Serialize)]
struct CascadeInvalidatedTrace<'a> {
    event: &'static str,
    libraries: &'a [String],
}

#[derive(Serialize)]
struct MemoryDegradeTrace<'a> {
    event: &'static str,
    tier: &'static str,
    limit_mib: u64,
    uri: &'a str,
}

#[derive(Serialize)]
struct CycleRebuiltTrace {
    event: &'static str,
    cycles: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh sink over a throwaway JSONL path.
    fn temp_sink(tag: &str) -> (TraceSink, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!(
            "dartq-trace-pin-{tag}-{}.jsonl",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        (TraceSink::create(&path).expect("sink opens"), path)
    }

    /// The wire contract per event kind: exact bytes, stable field order
    /// (§17.4 — CI samples captures byte-wise). `content_id_hit` omits
    /// `ast_cache_hit`; the parsed branch carries it.
    #[test]
    fn jsonl_shape_pins_every_event_kind() {
        let (mut sink, path) = temp_sink("shape");
        sink.overlay_applied(2, 1);
        sink.file_revalidated("/w/lib/a.dart", RevalidateGate::ContentIdHit);
        sink.file_revalidated(
            "/w/lib/b.dart",
            RevalidateGate::Parsed {
                ast_cache_hit: true,
            },
        );
        sink.library_analyzed("/w/lib/a.dart", false, 1500);
        sink.cascade_invalidated(&["/w/lib/a.dart".into(), "/w/lib/b.dart".into()]);
        sink.memory_degrade("ast_cold", 2048, "/w/lib/a.dart");
        sink.cycle_rebuilt(2);
        drop(sink);
        let capture = std::fs::read_to_string(&path).expect("capture readable");
        assert_eq!(
            capture,
            concat!(
                "{\"event\":\"OverlayApplied\",\"changed\":2,\"removed\":1}\n",
                "{\"event\":\"FileRevalidated\",\"uri\":\"/w/lib/a.dart\",\"gate\":\"content_id_hit\"}\n",
                "{\"event\":\"FileRevalidated\",\"uri\":\"/w/lib/b.dart\",\"gate\":\"parsed\",\"ast_cache_hit\":true}\n",
                "{\"event\":\"LibraryAnalyzed\",\"uri\":\"/w/lib/a.dart\",\"from_cache\":false,\"duration_us\":1500}\n",
                "{\"event\":\"CascadeInvalidated\",\"libraries\":[\"/w/lib/a.dart\",\"/w/lib/b.dart\"]}\n",
                "{\"event\":\"MemoryDegrade\",\"tier\":\"ast_cold\",\"limit_mib\":2048,\"uri\":\"/w/lib/a.dart\"}\n",
                "{\"event\":\"CycleRebuilt\",\"cycles\":2}\n",
            )
        );
        let _ = std::fs::remove_file(&path);
    }

    /// §19.3's Cache Hit Rate: cache-served library lines over all
    /// well-formed library lines; other event kinds, malformed lines, and
    /// library lines without a `from_cache` field don't count; a capture
    /// without library lines rates `None`.
    #[test]
    fn cache_hit_rate_aggregates_the_seed_lines() {
        let capture = concat!(
            "{\"event\":\"OverlayApplied\",\"changed\":1,\"removed\":0}\n",
            "not json at all\n",
            "{\"event\":\"LibraryAnalyzed\",\"uri\":\"a\",\"from_cache\":false,\"duration_us\":10}\n",
            "{\"event\":\"LibraryAnalyzed\",\"uri\":\"b\",\"from_cache\":true,\"duration_us\":0}\n",
            "{\"event\":\"LibraryAnalyzed\",\"uri\":\"c\",\"from_cache\":true,\"duration_us\":0}\n",
            "{\"event\":\"LibraryAnalyzed\",\"uri\":\"d\"}\n",
            "{\"event\":\"FileRevalidated\",\"uri\":\"a\",\"gate\":\"parsed\",\"ast_cache_hit\":true}\n",
        );
        let rate = cache_hit_rate(capture).expect("library lines exist");
        assert!((rate - 2.0 / 3.0).abs() < 1e-12, "2 of 3: {rate}");
        assert_eq!(
            cache_hit_rate("{\"event\":\"OverlayApplied\",\"changed\":0,\"removed\":0}\n"),
            None,
            "no library lines, no rate"
        );
        assert_eq!(cache_hit_rate(""), None);
    }
}

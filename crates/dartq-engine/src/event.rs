//! Engine events and the
//! `--trace-events` JSONL face.
//!
//! Delivered synchronously through the `&mut dyn FnMut(EngineEvent)`
//! callbacks of [`apply_pending`](crate::Engine::apply_pending) and
//! [`analyze`](crate::Engine::analyze). Events fire only when work happened:
//! an empty pending queue emits nothing, and a queue whose every entry hits
//! the `ContentId` gate emits `OverlayApplied` plus per-file `ContentIdHit`
//! `FileRevalidated`s and an empty `CascadeInvalidated`.
//!
//! # The trace face
//!
//! [`TraceSink`] records one JSON object per event, field order fixed by
//! the record structs (CI sampling compares captures byte-wise), flushing
//! per event — events are rare boundary observations, and a killed CI run
//! must not lose its tail. Writes are best-effort: a failed write is
//! remembered by the writer (later writes no-op) and never fails the
//! analysis it observes. The taxonomy maps as: `OverlayApplied`,
//! `CascadeInvalidated`, `MemoryDegrade` verbatim; `FileRevalidated`'s
//! `ast_cache_hit` is the `AstCacheHit` observable;
//! `LibraryAnalyzed.from_cache` is the Cache Hit Rate seed (aggregated
//! by [`cache_hit_rate`]). The remaining taxonomy names (`StoreHit`,
//! `SnapshotReuse`, `SignatureUnchanged`, `ContextBuilt`,
//! `SchedulerQueueDepth`, `PluginLatency`) land as variants with their
//! emitters once the M1 engine produces their data. Waves W1/W2 of the L3
//! plan (g485) add two additive trace-only rows, `CycleRebuilt` — the
//! condensation rebuild at the `structure_dirty` gate — and `CycleReused` —
//! the libraries whose cycle fingerprint stood at the post-closure reuse
//! gate; deliberately no `EngineEvent` variants, so the event goldens stay
//! byte-stable. `ExperimentDegrade` (#1304) is additive on both faces: a
//! version-skewed SDK whitelist degraded to known-only grants, the
//! observable signal for the lenient posture #1293 introduced.
//! `ResidencyDegrade` (stability lane #1459) joins the additive
//! trace-only rows: a residency-pairing miss a derived pass salvaged
//! over, one line per (file, face) pair per engine.

use std::io::Write as _;
use std::path::Path;
use std::time::Duration;

use dartq_source::FileId;
use serde::Serialize;

use crate::state::LibraryId;

/// In-process observation of apply/analyze work.
#[derive(Debug)]
pub enum EngineEvent {
    /// Apply batch boundary: the pending queue that was just drained, split
    /// by gate outcome. Fired once per non-empty queue, before the per-file
    /// and cascade events of the same apply.
    OverlayApplied {
        /// Files whose `ContentId` differed from residency (work follows).
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
    /// seed. `duration` is an additive delta against the draft payload —
    /// it implements the benchmark harness's per-library duration hook
    /// inside the event the API already has.
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
    /// A memory-budget tier fired (ladder; the
    /// `MemoryDegrade`). `tier` is the machine key (`lazy_parse` |
    /// `semantic_slices` | `ast_cold`); `file` anchors the event the same
    /// way the paired `dartq/memory_degraded` diagnostic anchors. Fires at
    /// most once per tier per engine — the ladder is sticky and one-way.
    MemoryDegrade {
        /// The machine key of the tier that fired.
        tier: &'static str,
        /// The armed budget limit, in MiB.
        limit_mib: u64,
        /// The anchor file (a dirty library's defining file, else any live
        /// library's).
        file: FileId,
    },
    /// A version-skewed SDK whitelist: the analyzed SDK's
    /// `allowed_experiments.json` named an experiment the vendored feature
    /// registry does not know, so the grant degraded to the known-only set
    /// instead of aborting (#1293) — this event is that degradation's
    /// observable signal (#1304). Fires at most once per unknown experiment
    /// name per engine, carried by the apply whose lanes first derived the
    /// degraded grant; a name already reported never re-fires, whatever
    /// else re-derives it (the memo bounds derivations per owner and
    /// version, the report set bounds across them).
    ExperimentDegrade {
        /// The SDK version whose whitelist named the unknown experiments
        /// (the SDK `version` file's trimmed content).
        sdk_version: String,
        /// The unknown experiment names, in whitelist order.
        unknown: Vec<String>,
        /// The granted fallback posture — [`KNOWN_ONLY_GRANT`]: an unknown
        /// grant cannot be honored and is skipped; every known one applies.
        granted: &'static str,
    },
    /// A content set was dropped because the text exceeds the engine's
    /// 32-bit span capacity ([`dartq_source::MAX_SOURCE_BYTES`]) — the
    /// file is rejected and everything else keeps analyzing. Fires at
    /// either gate: the apply funnel (a pending set over capacity never
    /// reaches residency — the file keeps its previous content, or stays
    /// absent if never resident) and the manager overlay (the change is
    /// skipped before the shared store ever holds the bytes). Warning-tier
    /// telemetry: the faces render it as a stderr notice, never a
    /// diagnostic row.
    SourceRejected {
        /// The rejected file's path (possibly never interned — the overlay
        /// face rejects before interning).
        path: std::path::PathBuf,
        /// The content's byte size.
        len: u64,
    },
}

/// The machine key of the granted fallback posture
/// [`EngineEvent::ExperimentDegrade`] reports: the known-only set.
pub const KNOWN_ONLY_GRANT: &str = "known_only";

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

/// The `--trace-events` sink: appends one JSON object per event
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

    /// Records [`EngineEvent::LibraryAnalyzed`] — the Cache Hit Rate
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

    /// Records [`EngineEvent::MemoryDegrade`].
    pub(crate) fn memory_degrade(&mut self, tier: &'static str, limit_mib: u64, uri: &str) {
        self.write_line(&MemoryDegradeTrace {
            event: "MemoryDegrade",
            tier,
            limit_mib,
            uri,
        });
    }

    /// Records [`EngineEvent::ExperimentDegrade`] (#1304): the version-skew
    /// degrade, additive to the taxonomy.
    pub(crate) fn experiment_degrade(&mut self, sdk_version: &str, unknown: &[String]) {
        self.write_line(&ExperimentDegradeTrace {
            event: "ExperimentDegrade",
            sdk_version,
            unknown,
            granted: KNOWN_ONLY_GRANT,
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

    /// Records the fingerprint-keyed reuse (g485 W2): the library count
    /// whose recorded cycle fingerprint stood at the post-closure gate,
    /// keeping the recorded run instead of recomputing it. Trace-only — no
    /// `EngineEvent` variant (g485 D3), the event goldens stay byte-stable.
    pub(crate) fn cycle_reused(&mut self, libraries: usize) {
        self.write_line(&CycleReusedTrace {
            event: "CycleReused",
            libraries,
        });
    }

    /// Records the residency-pairing degrade (stability lane #1459): a
    /// derived pass salvaged over a live member whose capture, residency,
    /// store, or UTF-8 pairing missed, dropping the unit from its product.
    /// Trace-only — no `EngineEvent` variant (the g485 D3 posture), the
    /// event goldens stay byte-stable.
    pub(crate) fn residency_degrade(&mut self, uri: &str, face: &str) {
        self.write_line(&ResidencyDegradeTrace {
            event: "ResidencyDegrade",
            uri,
            face,
        });
    }
}

/// Wall time as additive microseconds (integer — float durations would not
/// be byte-stable across platforms).
pub(crate) fn duration_us(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

/// The Cache Hit Rate, aggregated over a `--trace-events` capture:
/// cache-served [`EngineEvent::LibraryAnalyzed`] lines over all of them.
/// `None` when the capture holds no library lines (nothing to rate).
#[must_use]
#[expect(clippy::cast_precision_loss)] // span arithmetic narrows under the scanner u32 offset cap
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
/// each struct below IS the wire contract.
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
struct ExperimentDegradeTrace<'a> {
    event: &'static str,
    sdk_version: &'a str,
    unknown: &'a [String],
    granted: &'static str,
}

#[derive(Serialize)]
struct CycleRebuiltTrace {
    event: &'static str,
    cycles: usize,
}

#[derive(Serialize)]
struct CycleReusedTrace {
    event: &'static str,
    libraries: usize,
}

#[derive(Serialize)]
struct ResidencyDegradeTrace<'a> {
    event: &'static str,
    uri: &'a str,
    face: &'a str,
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
    ///. `content_id_hit` omits
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
        sink.experiment_degrade("3.10.0", &["future_flag".to_owned()]);
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
                "{\"event\":\"ExperimentDegrade\",\"sdk_version\":\"3.10.0\",\"unknown\":[\"future_flag\"],\"granted\":\"known_only\"}\n",
                "{\"event\":\"CycleRebuilt\",\"cycles\":2}\n",
            )
        );
        let _ = std::fs::remove_file(&path);
    }

    /// The W2 reuse row's wire contract (g485 W2): exact bytes, stable
    /// field order, same additive-row discipline as `CycleRebuilt` — pinned
    /// separately so the W1 capture pin above stays byte-frozen. The gate
    /// emits the row only when reuse fired, so the zero shape never occurs
    /// and is not pinned.
    #[test]
    fn cycle_reused_row_is_pinned() {
        let (mut sink, path) = temp_sink("reused");
        sink.cycle_reused(3);
        drop(sink);
        let capture = std::fs::read_to_string(&path).expect("capture readable");
        assert_eq!(capture, "{\"event\":\"CycleReused\",\"libraries\":3}\n");
        let _ = std::fs::remove_file(&path);
    }

    /// The residency-pairing degrade row's wire contract (stability lane
    /// #1459): exact bytes, stable field order, same additive-row
    /// discipline as `CycleRebuilt`/`CycleReused` — the passes emit the
    /// row only when a pairing invariant broke, so the healthy-event
    /// captures above stay byte-frozen.
    #[test]
    fn residency_degrade_row_is_pinned() {
        let (mut sink, path) = temp_sink("residency-degrade");
        sink.residency_degrade("/w/lib/a.dart", "capture");
        drop(sink);
        let capture = std::fs::read_to_string(&path).expect("capture readable");
        assert_eq!(
            capture,
            "{\"event\":\"ResidencyDegrade\",\"uri\":\"/w/lib/a.dart\",\"face\":\"capture\"}\n"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// the Cache Hit Rate: cache-served library lines over all
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

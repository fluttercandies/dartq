//! Engine events (engine-api-spec §1.4, incremental-l1-spec §4.2).
//!
//! Delivered synchronously through the `&mut dyn FnMut(EngineEvent)`
//! callbacks of [`apply_pending`](crate::Engine::apply_pending) and
//! [`analyze`](crate::Engine::analyze). Events fire only when work happened:
//! an empty pending queue emits nothing, and a queue whose every entry hits
//! the ContentId gate emits `OverlayApplied` plus per-file `ContentIdHit`
//! `FileRevalidated`s and an empty `CascadeInvalidated`.

use std::time::Duration;

use dartq_source::FileId;

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

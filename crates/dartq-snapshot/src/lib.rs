//! Persistent snapshot substrate for dartiq's M3 cold-start path (PRD §12,
//! §17.2 L2).
//!
//! # The accelerator invariant
//!
//! A snapshot is never a correctness dependency. Every load-time failure —
//! missing generation, header mismatch, truncated or corrupt entry, broken
//! remap tables — resolves to a *miss* (an absent generation or a skipped
//! entry) and never to an error. Load is an infallible API; only saving can
//! report failure, and a caller is always free to ignore it.
//!
//! # Layout
//!
//! ```text
//! <root>/snapshots/<generation id>/header.json    generation header (last write)
//! <root>/snapshots/<generation id>/tables.json    StringId/FileId remap slices
//! <root>/snapshots/<generation id>/entries/<entry id>.entry
//! ```
//!
//! A *generation* is one [`GenerationHeader`] + its entries. The directory
//! name is derived from the header itself, so a header mismatch is detected
//! before any large read. Entries are per-file facts keyed by [`EntryKey`]
//! (content hash + feature fingerprint + semantic version, PRD §5.6); each
//! entry file is a blake3-128 checksum line followed by the payload JSON.
//! Every file is written to a sibling temporary file and moved into place
//! with [`std::fs::rename`], so a reader never observes a torn write.
//!
//! # Format
//!
//! JSON v1 — final form, with [`SNAPSHOT_FORMAT_VERSION`] in the header
//! should a future container (e.g. bincode) ever replace it. The engine's
//! semantic version is *supplied by the caller*: this crate deliberately has
//! no dependency on `dartq-engine`, so the wiring layer passes the current
//! `EngineSemanticVersion` when it builds the header and entry keys.

mod entry;
mod header;
mod remap;
mod store;
#[cfg(test)]
mod tests;

pub use entry::{Entry, EntryKey};
pub use header::{GenerationHeader, SNAPSHOT_FORMAT_VERSION};
pub use remap::{RemapTables, SnapshotFileId, SnapshotStringId};
pub use store::{DEFAULT_CAPACITY_BYTES, SaveError, Snapshot, SnapshotConfig, SnapshotStore};

/// blake3 truncated to 128 bits — the house content-id width (§10.5).
pub(crate) fn blake3_128(bytes: &[u8]) -> [u8; 16] {
    let digest = blake3::hash(bytes);
    let mut id = [0u8; 16];
    id.copy_from_slice(&digest.as_bytes()[..16]);
    id
}

//! Persistent snapshot substrate for dartq's M3 cold-start path (PRD §12,
//! §17.2 L2).
//!
//! # The accelerator invariant
//!
//! A snapshot is never a correctness dependency. Every load-time failure —
//! missing generation, header mismatch, truncated or corrupt entry, broken
//! remap tables, an entry set that does not match its manifest — resolves
//! to a *miss* (an absent generation, never a partial one) and never to an
//! error. Load is an infallible API; only saving can report failure, and a
//! caller is always free to ignore it.
//!
//! # Layout
//!
//! ```text
//! <root>/snapshots/<generation id>/header.json    generation header (last write)
//! <root>/snapshots/<generation id>/tables.json    remap slices + entry-id manifest (one atomic unit)
//! <root>/snapshots/<generation id>/recency        load-recency marker (touched once per load)
//! <root>/snapshots/<generation id>/entries/<entry id>.entry
//! ```
//!
//! A *generation* is one [`GenerationHeader`] + its entries. The directory
//! name is derived from the header itself, so a header mismatch is detected
//! before any large read. Entries are per-file facts keyed by [`EntryKey`]
//! (content hash + feature fingerprint + semantic version, PRD §5.6); each
//! entry file is a blake3-128 checksum line followed by the payload JSON.
//! `tables.json` carries the remap slices together with the entry-id
//! manifest — the exact set of entry files the generation claims — and
//! [`SnapshotStore::load`] accepts the generation only when the entries
//! directory matches that set exactly, so a torn or raced save can never
//! load as a mixture of two saves. Every file is written to a sibling
//! temporary file and moved into place with [`std::fs::rename`], so a
//! reader never observes a torn write.
//!
//! # The recovery contract
//!
//! What a crash (or a failed save) may leave on disk, and what happens:
//!
//! - **Guaranteed loadable**: the last generation whose `save` completed
//!   every rename. Every returned entry passed its blake3-128 checksum and
//!   JSON decode, and the entry files on disk are exactly the set the
//!   manifest claims. A re-save that failed mid-way keeps the generation
//!   loadable when its entry set is a subset of the current claim — the
//!   rewritten entry files carry the new payloads (same key, same id) —
//!   while a failed save whose set adds keys leaves extra files that demote
//!   the generation to a miss.
//! - **Guaranteed lost**: any save whose renames did not all complete —
//!   including the mixed middle of a torn re-save, which the manifest gate
//!   demotes to a miss. There is no fsync anywhere; after power loss even
//!   the last fully-renamed generation may vanish entirely (directory
//!   entries are not durable). Every such outcome is a miss, never an
//!   error: the accelerator invariant.
//! - **Never guaranteed**: that a re-save is atomic (it can tear; the torn
//!   state is a miss, not a mixture), that the on-disk entry set is
//!   complete (an incomplete set is a miss, not a smaller `Some`), or that
//!   entries align with the last save inside a capacity-throttle window
//!   (residue simply leaves the generation undecodable).
//!
//! Consumption model for wiring layers: `load` is a **per-key hint cache**.
//! A returned entry is trustworthy — its key pins the content hash, the
//! feature fingerprint, and the semantic version; an absent key means
//! recompute. Never treat `Some(snapshot)` as an authoritative fact set,
//! and never treat `entries.len()` as meaningful beyond this load.
//!
//! # The save-shape contract
//!
//! Each `save` claims the *complete* entry set of the generation: the
//! manifest written with the tables names exactly the entries passed in.
//! Entry keys are a deterministic function of their content triple, so
//! re-saves whose entry set only grows stay coherent, while a re-save with
//! any other shape leaves the abandoned files of earlier saves in place
//! (there is no stale sweep; they die with the generation under LRU) and
//! the manifest gate then demotes the whole generation — an availability
//! loss (one reanalysis), never a correctness one.
//!
//! # Format
//!
//! JSON — final form, with [`SNAPSHOT_FORMAT_VERSION`] in the header
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

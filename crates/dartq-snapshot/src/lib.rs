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
//! <root>/snapshots/<generation id>/tables.json    remap slices + entry-id manifest (written first)
//! <root>/snapshots/<generation id>/recency        load-recency marker (touched once per load)
//! <root>/snapshots/<generation id>/entries/<entry id>.entry
//! ```
//!
//! A *generation* is one [`GenerationHeader`] + its entries. The directory
//! name is derived from the header itself, so a header mismatch is detected
//! before any large read. Entries are per-file facts keyed by [`EntryKey`]
//! (content hash + feature fingerprint + semantic version, PRD §5.6); each
//! entry file is a blake3-128 checksum, the blake3-128 *stamp* of the
//! generation's remap-slices serialization, then the payload JSON.
//! `tables.json` carries the remap slices together with the entry-id
//! manifest — the exact set of entry files the generation claims. The
//! manifest pairs with the slices in one atomic write but sits outside the
//! stamp (#422): an entry-set delta leaves the stamp, and every unchanged
//! entry frame, byte-identical. [`SnapshotStore::save`] writes
//! the tables first (the stamp source must exist before the entries embed
//! it), then the stamped entries — a frame already on disk byte-identically
//! is left in place, not rewritten — then sweeps every on-disk entry outside
//! the claim, then the header; [`SnapshotStore::load`] accepts the
//! generation only when the stamped, decodable entries equal that claim
//! exactly, so a torn or raced save can never load as a mixture of two
//! saves. Every file is written to a sibling temporary file and moved into
//! place with [`std::fs::rename`], so a reader never observes a torn write.
//!
//! # The recovery contract
//!
//! What a crash (or a failed save) may leave on disk, and what happens:
//!
//! - **Guaranteed loadable**: the last generation whose `save` completed
//!   every rename. Every returned entry passed its blake3-128 checksum,
//!   its stamp matches the generation's remap slices, its payload key
//!   agrees with its filename, and the stamped entry files on disk are
//!   exactly the set the manifest claims. A re-save that failed mid-way
//!   keeps the generation loadable when the interrupted save had not yet
//!   replaced the tables (the old stamped entries still pair with them);
//!   once the new tables have landed, the old entries no longer pair with
//!   the new save group — stale stamp when the slices changed, manifest
//!   mismatch when only the entry set did — and the generation is a miss
//!   until the save is retried. Every such outcome is an availability
//!   loss (one reanalysis), never a correctness one.
//! - **Guaranteed lost**: any save whose renames did not all complete —
//!   including the mixed middle of a torn re-save, which the stamp gate
//!   (and the manifest gate behind it) demotes to a miss. There is no
//!   fsync anywhere; after power loss even the last fully-renamed
//!   generation may vanish entirely (directory entries are not durable).
//!   Every such outcome is a miss, never an error: the accelerator
//!   invariant.
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
//! manifest written with the tables names exactly the entries passed in,
//! and the save-end sweep removes every on-disk entry outside the claim,
//! so the generation is coherent in any set shape — growing, shrinking, or
//! changed. The daemon flush obligation (#377) stands: flush writes the
//! full current entry set, never a partial subset. A save that fails
//! mid-way may leave residue (old-stamped entries, a partial sweep); the
//! stamp and manifest gates demote that to a miss, never a mixture, and
//! the next full-set save sweeps it away.
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

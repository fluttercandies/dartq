//! dartq's content-addressed source layer.
//!
//! Everything shareable in dartq is named by content, not by path: a file
//! path is merely a [`Locator`] hint, while identity comes
//! from [`ContentId`], the blake3-128 digest of the bytes. This crate is the
//! foundation of that model:
//!
//! - [`ContentId`] / [`content_hash`] — the universal content name.
//! - [`SourceStore`] / [`MemorySourceStore`] — content-addressed byte
//!   storage; identical bytes occupy one slot. [`MmapSourceStore`] is the
//!   file-backed CAS layout: canonical text rides read-only file
//!   mappings, deduplicated by content.
//! - [`SourceText`] — canonical source text with its [`ContentId`] baked
//!   in, held owned or as a mapping lease; the zero-copy hand-off between
//!   the loaders, the overlay, and the store.
//! - [`FileTable`] / [`FileId`] — normalized workspace paths interned to
//!   small integers, the `file` field of diagnostics.
//! - [`SourceRange`], [`LineIndex`], and the UTF-16 conversion helpers —
//!   the single source of truth for the engine's external position
//!   convention (UTF-16 code units, matching the official Dart analyzer
//!   and LSP).
//! - [`Locator`] — sparse path-to-(mtime, [`ContentId`]) map backing
//!   change-detection prefiltering.
//!
//! The crate depends only on `blake3`; the `serde` implementations are
//! behind an opt-in `serde` feature.

mod content_id;
mod file_table;
mod locator;
mod source_range;
mod store;
mod string_table;
mod text;

pub use content_id::{ContentId, ParseContentIdError, content_hash};
pub use file_table::{FileId, FileTable, normalize_lexical, normalize_path, normalize_paths};
pub use locator::Locator;
pub use source_range::{
    LineIndex, SourceRange, byte_offset_to_utf16, utf16_len_of, utf16_offset_to_byte,
};
pub use store::{
    ContentPool, MappedContentPool, MemorySourceStore, MmapSourceStore, PooledMappedSourceStore,
    PooledSourceStore, SourceStore,
};
pub use string_table::{StringId, StringTable};
pub use text::{MAX_SOURCE_BYTES, MMAP_MIN_BYTES, OpenError, SourceText, open_mapped, open_source};

/// Saturating `usize` → `u32` conversion for pool-slot and length caps
/// (the stability audit's pool-cap class). Pool-backed ids across the
/// stack address entries by `u32` ([`FileId`], [`StringId`], semantic
/// intern-pool slots, flow state slots); a pool cannot grow past
/// `u32::MAX` entries before RSS exhaustion (every entry costs at least
/// one heap allocation), so saturation replaces a provably unreachable
/// panic with a defined value. This is defense in depth, not a reachable
/// path: call sites are structurally bounded far below the cap.
///
/// Defined here because `dartq-source` is the common base dependency of
/// the parser / flow / semantic / lints stack — one definition, one
/// saturation contract everywhere.
#[must_use]
#[inline]
pub fn saturate_slot_u32(len: usize) -> u32 {
    u32::try_from(len).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::saturate_slot_u32;

    /// The pool-cap saturation contract: pass-through below the cap,
    /// identity at the cap, saturation past it.
    #[test]
    fn slot_saturation_passes_lengths_through_and_saturates_past_u32_max() {
        assert_eq!(saturate_slot_u32(0), 0);
        assert_eq!(saturate_slot_u32(1234), 1234);
        assert_eq!(saturate_slot_u32(u32::MAX as usize), u32::MAX);
        assert_eq!(saturate_slot_u32(u32::MAX as usize + 1), u32::MAX);
        assert_eq!(saturate_slot_u32(usize::MAX), u32::MAX);
    }
}

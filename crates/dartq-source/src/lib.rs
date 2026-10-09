//! dartq's content-addressed source layer (PRD §5.2, §10.4).
//!
//! Everything shareable in dartq is named by content, not by path: a file
//! path is merely a [`Locator`](locator::Locator) hint, while identity comes
//! from [`ContentId`], the blake3-128 digest of the bytes. This crate is the
//! foundation of that model:
//!
//! - [`ContentId`] / [`content_hash`] — the universal content name.
//! - [`SourceStore`] / [`MemorySourceStore`] — content-addressed byte
//!   storage; identical bytes occupy one slot.
//! - [`FileTable`] / [`FileId`] — normalized workspace paths interned to
//!   small integers, the `file` field of diagnostics (PRD §10.4).
//! - [`SourceRange`], [`LineIndex`], and the UTF-16 conversion helpers —
//!   the single source of truth for the engine's external position
//!   convention (UTF-16 code units, matching the official Dart analyzer
//!   and LSP).
//! - [`Locator`] — sparse path-to-(mtime, [`ContentId`]) map backing
//!   change-detection prefiltering (PRD §16.7).
//!
//! The crate depends only on `blake3`; the `serde` implementations are
//! behind an opt-in `serde` feature.

mod content_id;
mod file_table;
mod locator;
mod source_range;
mod store;
mod string_table;

pub use content_id::{ContentId, ParseContentIdError, content_hash};
pub use file_table::{FileId, FileTable, normalize_lexical};
pub use locator::Locator;
pub use source_range::{
    LineIndex, SourceRange, byte_offset_to_utf16, utf16_len_of, utf16_offset_to_byte,
};
pub use store::{MemorySourceStore, SourceStore};
pub use string_table::{StringId, StringTable};

//! Canonical source text: content-hashed at construction, borrowable as
//! `&str` with no per-access cost.
//!
//! A [`SourceText`] is the engine's unit of source: the canonical form is
//! one leading UTF-8 BOM stripped, invalid UTF-8 lossily decoded (the
//! oracle read-layer posture — see `SourceStore::load_file`). The bytes are
//! held either owned (`Arc<str>`) or as a read-only file mapping
//! (`Arc<MappedRange>`); both variants are cheap-clone leases, so the
//! pending overlay, the apply batch, and the content store share one
//! allocation by refcount instead of copying text between seams.
//!
//! The [`ContentId`] is computed once at construction — every later
//! consumer (store insert, apply's content gate, snapshot entry keys)
//! reads the carried id instead of re-hashing. UTF-8 validation likewise
//! happens exactly once, at construction (SIMD-accelerated via
//! `simdutf8`); accessors assume it.
//!
//! # Mapping lifetime contract
//!
//! A mapped [`SourceText`] aliases the file's pages for as long as any
//! lease (or store slot) holds it. The file must not be truncated in place
//! while mapped — that would fault (`SIGBUS`) on later access, the same
//! posture as every read-only mmap consumer of live files (the upstream
//! rust-analyzer VFS loader, SQLite's read-only mmap mode). Workspace
//! mutations reach the engine through the overlay API, which replaces the
//! text wholesale and retires the mapping by refcount; disk edits are
//! picked up by re-load. In-place truncation of an analyzed file mid-
//! session is outside the contract, as it already is for every mmap-based
//! tool serving source from the page cache.

use std::fmt;
use std::fs::File;
use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

use memmap2::Mmap;

use crate::content_id::{ContentId, content_hash};

/// The largest source the engine accepts, in bytes.
///
/// Positions in the engine's universe are 32-bit ([`SourceRange`],
/// [`crate::LineIndex`] line starts, UTF-16 offsets), and the binding
/// constraint is not the raw offsets — those stay `<= len` — but the
/// derived counts: a text of `len` bytes holds at most `len + 1` lines
/// (every byte a lone terminator), so line starts and 1-based line
/// numbers stay inside [`u32`] exactly while `len + 1 <= u32::MAX`. The
/// capacity is therefore one byte under the raw 4 GiB word, and a file of
/// exactly [`u32::MAX`] bytes is rejected too. The intake gates
/// ([`MappedRange::open`] at load, the engine's apply funnel at
/// residency) enforce this before any position is derived, so a resident
/// text never breaches it.
pub const MAX_SOURCE_BYTES: usize = u32::MAX as usize - 1;

/// Why [`open_mapped`] refused a file.
#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    /// The open/metadata/map I/O error, surfaced as-is.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// The file exceeds [`MAX_SOURCE_BYTES`] — the engine's 32-bit span
    /// capacity. `len` is the file's byte size.
    #[error("source is {len} bytes, over the {MAX_SOURCE_BYTES}-byte analysis capacity")]
    TooLarge {
        /// The file's byte size.
        len: u64,
    },
}

/// Canonical source text with its content id baked in. `Clone` is a
/// refcount bump on both variants, never a text copy.
#[derive(Clone)]
pub struct SourceText {
    id: ContentId,
    inner: TextInner,
}

#[derive(Clone, Debug)]
pub(crate) enum TextInner {
    /// Owned canonical text (the overlay face, and the fallback for BOM'd
    /// or invalid-UTF-8 files).
    Owned(Arc<str>),
    /// A read-only mapping of the file; the text is `map[start..end]`
    /// (the BOM-stripped range).
    Mapped(Arc<MappedRange>),
}

/// A read-only file mapping carrying validated UTF-8 in `map[start..end]`.
#[derive(Debug)]
pub(crate) struct MappedRange {
    map: Mmap,
    range: Range<usize>,
}

impl SourceText {
    /// Wraps already-canonical `text` (the overlay face's contract: no
    /// second decode, a leading BOM stays content). Hashes once.
    #[must_use]
    pub fn owned(text: String) -> Self {
        let id = content_hash(text.as_bytes());
        Self {
            id,
            inner: TextInner::Owned(Arc::from(text)),
        }
    }

    /// Canonicalizes raw file bytes into engine text: exactly one leading
    /// UTF-8 BOM stripped, invalid sequences lossily decoded to U+FFFD.
    /// The common case — valid UTF-8, no BOM — borrows `bytes` directly
    /// into the owned slot with a single copy (the bytes arrive borrowed,
    /// so something must own the text).
    #[must_use]
    pub fn canonical(bytes: &[u8]) -> Self {
        let stripped = strip_one_bom(bytes);
        match simdutf8::basic::from_utf8(stripped) {
            Ok(text) => Self {
                id: content_hash(stripped),
                inner: TextInner::Owned(Arc::from(text)),
            },
            // The oracle-parity fallback: the lossy decode of the stripped
            // bytes is the text (rare — an undecodable file contributes
            // zero diagnostics under the analyzer's read layer).
            Err(_) => Self::owned(String::from_utf8_lossy(stripped).into_owned()),
        }
    }

    /// The content id — computed once at construction.
    #[must_use]
    pub const fn content_id(&self) -> ContentId {
        self.id
    }

    /// The canonical text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match &self.inner {
            TextInner::Owned(text) => text,
            TextInner::Mapped(mapped) => mapped.as_str(),
        }
    }

    /// The canonical text's bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        self.as_str().as_bytes()
    }

    /// Byte length of the canonical text.
    #[must_use]
    pub fn len(&self) -> usize {
        self.as_bytes().len()
    }

    /// Whether the canonical text is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether the text rides a file mapping (exposed for the store's
    /// zero-copy slot sharing and the memory tests).
    #[must_use]
    pub fn is_mapped(&self) -> bool {
        matches!(self.inner, TextInner::Mapped(_))
    }

    pub(crate) fn inner(&self) -> &TextInner {
        &self.inner
    }
}

impl fmt::Debug for SourceText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SourceText")
            .field("id", &self.id)
            .field("len", &self.len())
            .field("mapped", &self.is_mapped())
            .finish()
    }
}

impl MappedRange {
    /// Maps `path` read-only and builds the canonical [`SourceText`] from
    /// its bytes: BOM stripped into the mapped range, UTF-8 validated once
    /// here. Files whose bytes are not valid UTF-8 fall back to the owned
    /// canonical form (rare); an empty file cannot be mapped and takes the
    /// owned path directly.
    ///
    /// Files over [`MAX_SOURCE_BYTES`] are rejected on the metadata stat,
    /// before any mapping or read: the span gate keeps every engine
    /// offset u32, and refusing here skips the mapping and its validation
    /// pass for a file that can never enter residency. A file grown past
    /// this stat between the check and the mapping lands on the engine's
    /// apply-time gate — the intake funnel's backstop.
    pub(crate) fn open(path: &Path) -> Result<SourceText, OpenError> {
        let file = File::open(path)?;
        let len = file.metadata()?.len();
        if len > MAX_SOURCE_BYTES as u64 {
            return Err(OpenError::TooLarge { len });
        }
        if len == 0 {
            // mmap rejects zero-length mappings; empty text needs none.
            return Ok(SourceText::owned(String::new()));
        }
        // Safety: a read-only mapping of a file we hold open; the mapping
        // lifetime contract on `SourceText` rules out in-place truncation
        // while any lease is alive.
        let map = unsafe { Mmap::map(&file)? };
        let stripped = strip_one_bom(&map);
        match simdutf8::basic::from_utf8(stripped) {
            Ok(text) => {
                let start = stripped.as_ptr() as usize - map.as_ptr() as usize;
                let range = start..start + text.len();
                let id = content_hash(text.as_bytes());
                Ok(SourceText {
                    id,
                    inner: TextInner::Mapped(Arc::new(MappedRange { map, range })),
                })
            }
            Err(_) => Ok(SourceText::canonical(stripped)),
        }
    }

    pub(crate) fn as_str(&self) -> &str {
        let bytes = &self.map[self.range.clone()];
        debug_assert!(std::str::from_utf8(bytes).is_ok(), "validated at open");
        // Safety: `open` validated exactly this byte range as UTF-8, and
        // the range is immutable for the mapping's lifetime.
        unsafe { std::str::from_utf8_unchecked(bytes) }
    }
}

/// Strips exactly one leading UTF-8 BOM (oracle read-layer parity: all
/// spans are relative to the stripped text). A second BOM is content.
pub(crate) fn strip_one_bom(bytes: &[u8]) -> &[u8] {
    bytes
        .strip_prefix([0xEF, 0xBB, 0xBF].as_slice())
        .unwrap_or(bytes)
}

/// Loads the canonical text of the file at `path` as a mapping lease: one
/// leading BOM stripped into the mapped range, UTF-8 validated once here
/// (SIMD via `simdutf8`), content-hashed once. Files whose bytes are not
/// valid UTF-8 fall back to the owned canonical form (rare); an empty file
/// cannot be mapped and takes the owned path directly. Registering the
/// text with a store is a separate [`SourceStore::put_text`] call — the
/// split is what lets the workspace walk take mappings in parallel and
/// insert them serially in load order.
///
/// The returned lease aliases the file's pages; see the mapping lifetime
/// contract on [`SourceText`].
///
/// # Errors
///
/// The open/metadata/map I/O errors, surfaced as-is, and the
/// [`OpenError::TooLarge`] rejection when the file exceeds
/// [`MAX_SOURCE_BYTES`] (the caller reports the rejection at the file's
/// own position and moves on — the rest of the load batch is unaffected).
#[must_use]
pub fn open_mapped(path: &Path) -> Result<SourceText, OpenError> {
    MappedRange::open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_strips_exactly_one_bom() {
        let text = SourceText::canonical("\u{FEFF}var x;".as_bytes());
        assert_eq!(text.as_str(), "var x;");

        // A second BOM is content, not a second strip.
        let double = SourceText::canonical("\u{FEFF}\u{FEFF}x".as_bytes());
        assert_eq!(double.as_str(), "\u{FEFF}x");

        let bare = SourceText::canonical(b"var x;");
        assert_eq!(bare.as_str(), "var x;");

        // A BOM-only file strips to empty.
        let only = SourceText::canonical("\u{FEFF}".as_bytes());
        assert!(only.as_str().is_empty());
    }

    #[test]
    fn canonical_lossy_decodes_invalid_utf8() {
        // FF FE in code position: U+FFFD replacement, matching the
        // analyzer's lossy text face.
        let text = SourceText::canonical(b"a\xffb");
        assert_eq!(text.as_str(), "a\u{FFFD}b");
        assert!(!text.is_mapped());
    }

    #[test]
    fn owned_keeps_a_leading_bom_as_content() {
        // The overlay face hands over pre-canonicalized text: no second
        // strip, no second decode.
        let text = SourceText::owned("\u{FEFF}var x;".to_string());
        assert_eq!(text.as_str(), "\u{FEFF}var x;");
    }

    #[test]
    fn content_id_matches_the_house_hash_of_the_text() {
        let text = SourceText::canonical(b"void main() {}");
        assert_eq!(text.content_id(), content_hash(b"void main() {}"));
        // A BOM'd file canonicalizes to the stripped text, so its identity
        // is the stripped bytes' hash — the same id as the BOM-free file.
        let bomd = SourceText::canonical("\u{FEFF}void main() {}".as_bytes());
        assert_eq!(
            bomd.content_id(),
            text.content_id(),
            "a BOM'd file's identity is its stripped text's"
        );
    }

    #[test]
    fn clone_shares_allocation_not_bytes() {
        let text = SourceText::owned("shared".to_string());
        let lease = text.clone();
        assert_eq!(lease.as_str(), "shared");
        assert!(matches!(lease.inner, TextInner::Owned(_)));
        assert_eq!(lease.len(), 6);
        assert!(!lease.is_empty());
    }

    /// The span-capacity gate (#1543): a file over [`MAX_SOURCE_BYTES`] is
    /// rejected on the metadata stat, before any mapping. The fixture is a
    /// sparse file — `set_len` sizes it without writing blocks, so the test
    /// costs no disk and no read pass; what the gate checks is exactly the
    /// metadata length the fixture sets.
    #[test]
    fn open_rejects_files_over_the_span_capacity() {
        let path =
            std::env::temp_dir().join(format!("dartq-source-cap-{}.dart", std::process::id()));
        let file = File::create(&path).expect("create sparse fixture");
        file.set_len(MAX_SOURCE_BYTES as u64 + 1)
            .expect("size fixture");
        drop(file);

        match open_mapped(&path) {
            Err(OpenError::TooLarge { len }) => {
                assert_eq!(len, MAX_SOURCE_BYTES as u64 + 1);
            }
            other => panic!("expected TooLarge, got {other:?}"),
        }
        std::fs::remove_file(&path).expect("remove fixture");
    }

    /// The capacity value is the u32 line-count bound, not the raw word: a
    /// text of `len` bytes holds at most `len + 1` lines (every byte a lone
    /// terminator), so line starts and 1-based line numbers stay inside u32
    /// exactly while `len <= u32::MAX - 1`. Pins the one-byte headroom that
    /// keeps the whole `LineIndex` count family invariant-true behind the
    /// intake gates.
    #[test]
    fn capacity_is_the_u32_line_count_bound() {
        assert_eq!(MAX_SOURCE_BYTES, u32::MAX as usize - 1);
    }

    /// The rejection's display names both sizes: the file's and the capacity's
    /// — the notice faces render it verbatim for the user.
    #[test]
    fn too_large_error_names_both_sizes() {
        let line = OpenError::TooLarge {
            len: MAX_SOURCE_BYTES as u64 + 1,
        }
        .to_string();
        assert!(
            line.contains(&MAX_SOURCE_BYTES.to_string()),
            "names the capacity: {line}"
        );
        assert!(
            line.contains(&(MAX_SOURCE_BYTES as u64 + 1).to_string()),
            "names the file size: {line}"
        );
    }
}

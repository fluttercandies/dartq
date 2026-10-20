//! The M1 symbol index: the
//! unlinked declaration surface, the per-library reference surface, and the
//! `query symbol` faces.
//!
//! # Faces
//!
//! * **Unlinked declaration index** ([`FileIndex`], one per file): every
//!   declaration name the parse surface carries — kind, name-token span,
//!   and the enclosing container's row as the container chain anchor.
//!   Parse-only, like the official Layer A name sets; a file's rows are
//!   rebuilt whole on re-parse.
//! * **Per-library reference index** ([`LibraryIndex`]): every identifier
//!   use that resolves through the local-file scope and the #117
//!   [`Namespaces`](dartq_resolution::Namespaces) (a [`RefRow`] naming its
//!   target declaration), every use that does not (a [`NameRow`] in the
//!   unresolved-names table — the recovery surface), and the direct
//!   supertype name edges (clause-typed: `extends`/`with`/`implements`/`on`)
//!   resolved to the same target shape.
//! * **Query faces** ([`SymbolIndex`]): declarations by name across the
//!   workspace and references of a declaration — the `query symbol` CLI
//!   pair (#123), answered through the name-keyed postings directories
//!   (the `postings` module) instead of workspace scans.
//! * **Test graph** ([`TestGraph`]): the inverted `test_of` edges from test
//!   files to the libraries they test — the `AffectedTests` join
//!   of the impact query, in the same CSR posture over the caller's library
//!   id space.
//!
//! # Identity and the migration seams
//!
//! dartq-index mints no ids of its own: rows are flat `Vec`s of
//! borrowed ids ([`FileId`], [`StringId`]), and a declaration's identity is
//! its `(file, name)` pair — the same identity #117's namespace entries
//! use. Two seams are kept open on purpose:
//!
//! * **`ElementId`**: when dartq-semantic mints elements, [`DeclRow`] gains
//!   an element column and [`RefRow`]'s target pair becomes (or gains) the
//!   `ElementId`; the row shapes do not change otherwise.
//! * **Unresolved-name byproducts** (#120): the `undefined_*` diagnostics
//!   resolve identifiers as a side effect of their checks; their miss rows
//!   belong in this crate's names table, not in a parallel path. The seam
//!   is [`LibraryIndex::record_unresolved`] — appended rows are served
//!   through the same [`LibraryIndex::unresolved`] probe as built-in rows.
//!
//! # Memory and update law
//!
//! Sized by input and nothing else: declaration rows by the file's
//! declaration surface, reference rows by its identifier uses; build-time
//! maps are dropped at `finish`. There are no caches. Update is whole-unit
//! replacement (the official index has no partial update — a
//! dirty file replaces its [`FileIndex`], a dirty library rebuilds its
//! [`LibraryIndex`] contribution whole at the `LibraryAnalyzed` point of
//! the L1 pipeline); the Engine owns dirty scheduling, the index never
//! invalidates itself. The postings directories carry no rows
//! of their own — only name → unit listings (the `postings` module
//! documents the per-entry bytes) — so an update's directory cost is the
//! replaced unit's own name set, re-listed. The exported
//! [`Postings`] serves the engine boundary, which mounts sibling
//! directories over its own dense-id arenas and applies the same
//! whole-unit law at its capture/surface mutation points — one law, no
//! second index.

use std::collections::BTreeMap;

use dartq_resolution::LibraryId;
use dartq_source::{FileId, StringId};

mod capture;
mod library;
mod postings;
mod test_graph;

pub use capture::{
    Capturer, DeclKind, DeclRow, FileCapture, FileIndex, SuperClause, SuperEdge, capture,
};
pub use library::{
    LibraryBuilder, LibraryIndex, NameRow, RefKind, RefRow, SubtypeEdge, TargetRef,
};
pub use postings::{Postings, distinct_decl_names};
pub use test_graph::TestGraph;

/// One declaration hit of a workspace-wide by-name query.
#[derive(Clone, Copy, Debug)]
pub struct SymbolDecl<'a> {
    /// The declaring file.
    pub file: FileId,
    /// The declaration row (name, kind, span, container chain).
    pub decl: &'a DeclRow,
}

/// The workspace index: per-file declaration surfaces plus per-library
/// reference surfaces, with the two `query symbol` faces over them.
///
/// Maps are `BTreeMap`s on the ids so iteration — and therefore query
/// output — is deterministic by id order (merges must be
/// deterministic). The postings directories carry the name → unit
/// listings the faces answer through; their unit lists stay ascending, so
/// query order is id order there too (the `postings` module). Sized by the analyzed
/// set; nothing grows between inserts.
#[derive(Debug, Default)]
pub struct SymbolIndex {
    files: BTreeMap<FileId, FileIndex>,
    libraries: BTreeMap<LibraryId, LibraryIndex>,
    decl_dir: postings::Postings<FileId>,
    ref_dir: postings::Postings<LibraryId>,
}

impl SymbolIndex {
    /// Stores (or replaces, on re-parse) the declaration surface of `file`.
    ///
    /// The postings directories follow the whole-unit law: a replaced
    /// surface is unlisted from its old names before the new one lists.
    pub fn insert_file(&mut self, file: FileId, index: FileIndex) {
        if let Some(old) = self.files.insert(file, index) {
            for name in postings::distinct_decl_names(&old) {
                self.decl_dir.unlist(name, file);
            }
        }
        let fresh = self.files.get(&file).expect("just stored");
        for name in postings::distinct_decl_names(fresh) {
            self.decl_dir.list(name, file);
        }
    }

    /// Stores (or replaces, on re-analysis) the reference surface of `lib`.
    pub fn insert_library(&mut self, lib: LibraryId, index: LibraryIndex) {
        if let Some(old) = self.libraries.insert(lib, index) {
            for name in old.distinct_reference_names() {
                self.ref_dir.unlist(name, lib);
            }
        }
        let fresh = self.libraries.get(&lib).expect("just stored");
        for name in fresh.distinct_reference_names() {
            self.ref_dir.list(name, lib);
        }
    }

    /// Drops `file`'s declaration surface — the file left the analyzed set
    /// (deleted, or evicted by the Engine); returns the removed surface.
    pub fn remove_file(&mut self, file: FileId) -> Option<FileIndex> {
        let old = self.files.remove(&file)?;
        for name in postings::distinct_decl_names(&old) {
            self.decl_dir.unlist(name, file);
        }
        Some(old)
    }

    /// Drops `lib`'s reference surface; returns the removed surface.
    pub fn remove_library(&mut self, lib: LibraryId) -> Option<LibraryIndex> {
        let old = self.libraries.remove(&lib)?;
        for name in old.distinct_reference_names() {
            self.ref_dir.unlist(name, lib);
        }
        Some(old)
    }

    /// The declaration surface of `file`, if analyzed.
    #[must_use]
    pub fn file(&self, file: FileId) -> Option<&FileIndex> {
        self.files.get(&file)
    }

    /// The reference surface of `lib`, if analyzed.
    #[must_use]
    pub fn library(&self, lib: LibraryId) -> Option<&LibraryIndex> {
        self.libraries.get(&lib)
    }

    /// Every indexed file with its surface, ascending by [`FileId`].
    pub fn files(&self) -> impl Iterator<Item = (FileId, &FileIndex)> + '_ {
        self.files.iter().map(|(&file, index)| (file, index))
    }

    /// Every indexed library with its surface, ascending by [`LibraryId`].
    pub fn libraries(&self) -> impl Iterator<Item = (LibraryId, &LibraryIndex)> + '_ {
        self.libraries.iter().map(|(&lib, index)| (lib, index))
    }

    /// `query symbol` by name: every declaration row called `name` across
    /// the workspace, ascending by file.
    ///
    /// The postings directory names the declaring files in one hash probe;
    /// each file's rows come from its own equal-name run — so the cost is
    /// the answer's size, not the workspace's.
    pub fn symbols(&self, name: StringId) -> impl Iterator<Item = SymbolDecl<'_>> + '_ {
        let files: &[FileId] = self.decl_dir.units(name);
        files
            .iter()
            .copied()
            .filter_map(move |file| {
                let rows = self.files.get(&file)?.by_name(name)?;
                Some(rows.iter().map(move |decl| SymbolDecl { file, decl }))
            })
            .flatten()
    }

    /// Reverse query: every recorded reference to the declaration
    /// `(file, name)`, ascending by library.
    ///
    /// The postings directory names the referencing libraries in one hash
    /// probe; each library's rows come from its own target run.
    pub fn references(&self, file: FileId, name: StringId) -> impl Iterator<Item = &RefRow> + '_ {
        let libraries: &[LibraryId] = self.ref_dir.units(name);
        libraries
            .iter()
            .copied()
            .filter_map(move |lib| self.libraries.get(&lib)?.references_of(file, name))
            .flatten()
    }

    /// The postings directories' memory in bytes: hash slots plus unit
    /// lists at capacity, both faces — the bench's total-bytes input (the
    /// rows themselves live once, in the per-unit surfaces).
    #[must_use]
    pub fn postings_bytes(&self) -> usize {
        self.decl_dir.bytes() + self.ref_dir.bytes()
    }
}

/// A count or row index as `u32` — the column width every flat array here
/// uses.
///
/// # Panics
///
/// Panics when the value exceeds [`u32::MAX`]: the tables are sized by
/// input, so a four-billion-row table is a program bug to surface, not
/// truncate (the crate-wide "surface, never clamp" contract).
pub(crate) fn narrow(count: usize) -> u32 {
    u32::try_from(count).expect("count fits u32")
}

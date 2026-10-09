//! The M1 symbol index (index-design-spec §8-§10; PRD §10.3, §20.3): the
//! unlinked declaration surface, the per-library reference surface, the
//! `query symbol` faces, and CSR-friendly graph exports.
//!
//! # Faces (spec §8 keep list)
//!
//! * **Unlinked declaration index** ([`FileIndex`], one per file): every
//!   declaration name the parse surface carries — kind, name-token span,
//!   and the enclosing container's row as the container chain anchor.
//!   Parse-only, like the official Layer A name sets; a file's rows are
//!   rebuilt whole on re-parse.
//! * **Per-library reference index** ([`LibraryIndex`]): every identifier
//!   use that resolves through the local-file scope and the #117
//!   [`Namespaces`](dartiq_resolution::Namespaces) (a [`RefRow`] naming its
//!   target declaration), every use that does not (a [`NameRow`] in the
//!   unresolved-names table — the §8 recovery surface), and the direct
//!   supertype name edges resolved to the same target shape.
//! * **Query faces** ([`SymbolIndex`]): declarations by name across the
//!   workspace and references of a declaration — the `query symbol` CLI
//!   pair (#123).
//! * **CSR exports** ([`csr`]): the import graph and the reference graph as
//!   flat `&[u32]` offsets/edges arrays (the GPU posture directive — future
//!   compute backends read views, never restructure the store).
//!
//! # Identity and the migration seams
//!
//! dartiq-index mints no ids of its own (spec §9): rows are flat `Vec`s of
//! borrowed ids ([`FileId`], [`StringId`]), and a declaration's identity is
//! its `(file, name)` pair — the same identity #117's namespace entries
//! use. Two seams are kept open on purpose:
//!
//! * **`ElementId`**: when dartiq-semantic mints elements, [`DeclRow`] gains
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
//! replacement (spec §6/§8: the official index has no partial update — a
//! dirty file replaces its [`FileIndex`], a dirty library rebuilds its
//! [`LibraryIndex`] contribution whole at the `LibraryAnalyzed` point of
//! the L1 pipeline); the Engine owns dirty scheduling, the index never
//! invalidates itself (spec §10).

use std::collections::BTreeMap;

use dartiq_resolution::LibraryId;
use dartiq_source::{FileId, StringId};

mod capture;
mod csr;
mod library;

pub use capture::{DeclKind, DeclRow, FileCapture, FileIndex, SuperEdge, capture};
pub use csr::{ImportCsr, ReferenceCsr};
pub use library::{LibraryBuilder, LibraryIndex, NameRow, RefKind, RefRow, TargetRef};

/// One declaration hit of a workspace-wide by-name query.
#[derive(Clone, Copy, Debug)]
pub struct SymbolDecl<'a> {
    /// The declaring file.
    pub file: FileId,
    /// The declaration row (name, kind, span, container chain).
    pub decl: &'a DeclRow,
}

/// A `(file, name)` declaration key — the node id payload of the reference
/// CSR export and the identity every row shares.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DeclKey {
    /// The declaring file.
    pub file: FileId,
    /// The declared (lookup-spelling) name.
    pub name: StringId,
}

/// The workspace index: per-file declaration surfaces plus per-library
/// reference surfaces, with the two `query symbol` faces over them.
///
/// Maps are `BTreeMap`s on the ids so iteration — and therefore query
/// output — is deterministic by id order (spec §12.4 requires merges to be
/// deterministic). Sized by the analyzed set; nothing grows between
/// inserts.
#[derive(Debug, Default)]
pub struct SymbolIndex {
    files: BTreeMap<FileId, FileIndex>,
    libraries: BTreeMap<LibraryId, LibraryIndex>,
}

impl SymbolIndex {
    /// Stores (or replaces, on re-parse) the declaration surface of `file`.
    pub fn insert_file(&mut self, file: FileId, index: FileIndex) {
        self.files.insert(file, index);
    }

    /// Stores (or replaces, on re-analysis) the reference surface of `lib`.
    pub fn insert_library(&mut self, lib: LibraryId, index: LibraryIndex) {
        self.libraries.insert(lib, index);
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
    pub fn symbols(&self, name: StringId) -> impl Iterator<Item = SymbolDecl<'_>> + '_ {
        self.files
            .iter()
            .filter_map(move |(&file, index)| {
                index
                    .by_name(name)
                    .map(move |rows| rows.iter().map(move |decl| (file, decl)))
            })
            .flatten()
            .map(|(file, decl)| SymbolDecl { file, decl })
    }

    /// Reverse query: every recorded reference to the declaration
    /// `(file, name)`, ascending by library.
    pub fn references(&self, file: FileId, name: StringId) -> impl Iterator<Item = &RefRow> + '_ {
        self.libraries
            .values()
            .filter_map(move |index| index.references_of(file, name))
            .flatten()
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

//! URI, import, export, part, and conditional import resolution with
//! namespace computation.
//!
//! The crate carries the URI layer (M1 ticket #65) and the namespace/graph
//! layer (M1 ticket #117):
//!
//! - [`uri`] — directive URI parsing/validation under Dart `Uri.parse`
//!   rules, relative composition against the container's canonical URI,
//!   scheme dispatch (`dart:`/`package:`/`file:`), interning to
//!   [`dartq_source::FileId`], and the [`ResolvedTarget`] projection.
//! - [`conditional`] — conditional-URI selection for configurable
//!   import/export directives (PRD §9.4): first-match-wins selection over
//!   the declared-variables map with the oracle's primary-URI fallback.
//! - [`library_graph`] — interned library nodes keyed by canonical URI,
//!   import/export edges, and the eager export-scope fixpoint pinned to the
//!   analyzer's worklist behavior (locals beat re-exports, conflicting
//!   re-exports resolve first-come-wins).
//! - [`namespace`] — import-namespace and prefix-table construction with
//!   build-time combinator filtering, and the `Found`/`Ambiguous`/`NotFound`
//!   lookup face the `undefined_*` diagnostics consume.
//! - [`codes`] — the frozen §4.2 resolution diagnostic code table.
//!
//! The part/part-of family (`part_of_*`, `duplicate_part`) and its file-role
//! graph remain ticket #119.

pub mod codes;
pub mod conditional;
pub mod library_graph;
pub mod namespace;
pub mod uri;

pub use codes::{ResolutionCode, ResolutionCodeInfo, UNDEFINED_HIDDEN_NAME, UNDEFINED_SHOWN_NAME};
pub use conditional::{Configuration, SelectedUri, resolve_conditional};
pub use library_graph::{
    CombinatorClause, Combinators, ExportEdge, ExportEntry, ImportEdge, LibraryGraph,
    LibraryGraphBuilder, LibraryId, LocalLookup, UndefinedCombinatorName,
};
pub use namespace::{Lookup, NameEntry, NamespaceEntry, Namespaces};
pub use uri::{
    DirectiveKind, PackageTable, RawUri, ResolvedTarget, ResolvedUri, UnresolvedKind,
    UriResolution, UriResolver, parse_directive_uri,
};

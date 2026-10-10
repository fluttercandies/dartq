//! The `query symbol` face (PRD §20.3; index-design-spec §8–§10): the
//! workspace symbol index answered from the engine's maintained index
//! state — the per-file captures (declaration surface + raw sites) and the
//! per-library reference surfaces analyze keeps current at the
//! `LibraryAnalyzed` point.
//!
//! # Freshness contract
//!
//! A query is a pure read of the state the latest [`Engine::analyze`]
//! produced: callers run analyze first (the read-after-write), and a query
//! never applies the overlay, re-analyzes, or otherwise mutates derived
//! state — the only side effect is interning the queried name. Querying
//! before the first analyze answers an empty set. Reference surfaces track
//! the completed-library partition of the latest analyze exactly (a
//! cancelled analyze leaves the not-yet-recomputed libraries on their
//! previous surfaces, the same partition their cached diagnostic results
//! keep).
//!
//! # Cost shape
//!
//! The answer assembles in O(rows of the answer): declaration rows come
//! straight from the maintained captures (ascending file — the deterministic
//! order `SymbolIndex` prescribed), reference rows from the maintained
//! surfaces (ascending library). No scan, no parse, no namespace rebuild —
//! the whole-workspace recapture this face used to run per query died with
//! the maintained index (the audit's query-path multiplication).

use dartq_index::{DeclRow, RefRow};
use dartq_source::{FileId, SourceRange};

use crate::state::EngineState;

pub use dartq_index::{DeclKind, RefKind};

/// The `query symbol` answer (PRD §20.3): declaration rows plus resolved
/// reference rows. Rows borrow the engine's interned spellings — lookups
/// run on interned ids and resolve to `&str` only at this boundary; nothing
/// clones per row.
#[derive(Debug, Clone)]
pub struct SymbolAnswer<'a> {
    /// Every declaration named by the query, ascending by file.
    pub decls: Vec<DeclHit<'a>>,
    /// Every resolved reference to those declarations, grouped by target
    /// file (ascending), offset-ascending within a target.
    pub refs: Vec<RefHit>,
}

/// One declaration hit of [`SymbolAnswer`].
#[derive(Clone, Copy, Debug)]
pub struct DeclHit<'a> {
    /// The declaring file.
    pub file: FileId,
    /// The declaration family.
    pub kind: DeclKind,
    /// The declared name (setters under their `name=` lookup spelling).
    pub name: &'a str,
    /// The enclosing container's name; [`None`] for top-level rows and
    /// unnamed-extension members.
    pub container: Option<&'a str>,
    /// The name token's span (UTF-16, the engine's external convention).
    pub range: SourceRange,
}

/// One resolved reference hit of [`SymbolAnswer`].
#[derive(Clone, Copy, Debug)]
pub struct RefHit {
    /// The referenced declaration's file.
    pub target_file: FileId,
    /// The use site's file — the library member whose capture recorded the
    /// row: a library spans its defining unit plus parts, so the target
    /// pair alone cannot locate the use (dartq-index `RefRow::file`).
    pub file: FileId,
    /// The relation kind of the use.
    pub kind: RefKind,
    /// The use is explicitly qualified (`p.x`) and cannot be shadowed by a
    /// local declaration.
    pub qualified: bool,
    /// The use site's span (UTF-16).
    pub range: SourceRange,
}

/// Answers a by-name symbol query over the maintained index state (module
/// docs for the freshness and cost contracts).
pub(crate) fn symbols<'a>(state: &'a EngineState, name: &str) -> SymbolAnswer<'a> {
    // The queried name interns once; every lookup below runs on the id.
    let id = state.strings.intern(name);

    // Declaration rows from the maintained captures, ascending by file
    // (`BTreeMap` order — the deterministic order the per-query
    // `SymbolIndex` build produced). Equal-name rows of one file are
    // contiguous, so a last-wins check deduplicates the reference probes.
    let mut decls: Vec<DeclHit<'a>> = Vec::new();
    let mut targets: Vec<FileId> = Vec::new();
    for (&file, capture) in &state.captures {
        let Some(rows) = capture.index.by_name(id) else {
            continue;
        };
        for decl in rows {
            let container = (decl.container != DeclRow::NONE).then(|| {
                state
                    .strings
                    .resolve(capture.index.row(decl.container).name)
            });
            decls.push(DeclHit {
                file,
                kind: decl.kind,
                name: state.strings.resolve(decl.name),
                container,
                range: decl.range,
            });
        }
        if targets.last() != Some(&file) {
            targets.push(file);
        }
    }

    // Reference rows from the maintained surfaces, ascending by library —
    // the same order the per-query build's `SymbolIndex::references` served.
    let refs: Vec<RefHit> = targets
        .iter()
        .flat_map(|&file| {
            state
                .library_surfaces
                .values()
                .filter_map(move |surface| surface.references_of(file, id))
                .flatten()
        })
        .map(|row: &RefRow| RefHit {
            target_file: row.target_file,
            file: row.file,
            kind: row.kind,
            qualified: row.qualified,
            range: row.range,
        })
        .collect();
    SymbolAnswer { decls, refs }
}

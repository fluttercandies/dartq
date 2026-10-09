//! The `query symbol` face (PRD §20.3; index-design-spec §8–§10): the
//! workspace symbol index answered through the dartq-index query faces
//! (#201/#122) over the current analyzed state.
//!
//! # Freshness contract
//!
//! A query is a pure read of the state the latest [`Engine::analyze`]
//! produced: callers run analyze first (the read-after-write), and a query
//! never applies the overlay, re-analyzes, or otherwise mutates derived
//! state — the only side effect is interning the queried name. Querying
//! before the first analyze (no resolution stage yet) answers an empty set.
//!
//! # Build shape
//!
//! The index is built fresh per query, per library, on the batch fan-out
//! (the analyze recompute's posture): member files are captured with the
//! apply-phase per-file derivation (owning-package language version, the
//! shared `l1::scan_unit` `@dart` override settlement), resolved through
//! the analyze-built graph namespaces, and folded into [`SymbolIndex`]
//! faces. Captures drop
//! inside each fan-out job, so peak residency is one library's parse plus
//! the row-sized finished surfaces. Incremental index maintenance in the L1
//! cascade is deliberately out of scope here; this face is independent of
//! it and stays correct against whatever analyzed state exists.

use dartq_index::{
    DeclRow, FileCapture, FileIndex, LibraryBuilder, LibraryIndex, SymbolDecl, SymbolIndex, capture,
};
use dartq_language::DEFAULT_LANGUAGE_VERSION;
use dartq_parser::ParserFeatures;
use dartq_resolution::Namespaces;
use dartq_source::{FileId, SourceRange};
use rayon::prelude::*;

use crate::pipeline;
use crate::state::{EngineState, LibraryId};

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
    /// The relation kind of the use.
    pub kind: RefKind,
    /// The use is explicitly qualified (`p.x`) and cannot be shadowed by a
    /// local declaration.
    pub qualified: bool,
    /// The use site's span (UTF-16). The index row shape carries no
    /// use-file column (dartq-index `RefRow`), so the span is the only
    /// position signal.
    pub range: SourceRange,
}

/// One fan-out job's output: the graph-library key of the finished
/// reference surface plus the declaration surfaces to register.
type BuiltLibrary = (
    dartq_resolution::LibraryId,
    LibraryIndex,
    Vec<(FileId, FileIndex)>,
);

/// Answers a by-name symbol query over the analyzed state (module docs for
/// the freshness and build contracts).
pub(crate) fn symbols<'a>(
    state: &'a EngineState,
    resolved: Option<&pipeline::ResolvedGraph>,
    name: &str,
) -> SymbolAnswer<'a> {
    let Some(resolved) = resolved else {
        return SymbolAnswer {
            decls: Vec::new(),
            refs: Vec::new(),
        };
    };
    // The queried name interns once; every lookup below runs on the id.
    let id = state.strings.intern(name);
    let namespaces = Namespaces::build(&resolved.graph);

    // Per-library build on the batch fan-out: each job captures its member
    // files, resolves their sites through the shared namespaces, and drops
    // the captures before returning the row-sized surfaces.
    let jobs: Vec<(LibraryId, dartq_resolution::LibraryId)> = state
        .libraries
        .live()
        .filter_map(|library| {
            resolved
                .graph_ids
                .get(&library)
                .copied()
                .map(|graph_library| (library, graph_library))
        })
        .collect();
    let built: Vec<BuiltLibrary> = jobs
        .par_iter()
        .map(|&(library, graph_library)| {
            let info = state.libraries.info(library).expect("live library");
            let captures: Vec<FileCapture> = crate::pipeline::member_files(info)
                .par_iter()
                .map(|&file| capture_file(state, file))
                .collect();
            let mut builder = LibraryBuilder::new(graph_library, &resolved.graph, &namespaces);
            for member in &captures {
                builder.add_file(member);
            }
            let files = captured_files(captures);
            (graph_library, builder.finish(), files)
        })
        .collect();

    let mut index = SymbolIndex::default();
    for (library, surface, files) in built {
        for (file, file_index) in files {
            index.insert_file(file, file_index);
        }
        index.insert_library(library, surface);
    }

    // Materialize the answer: ids resolve to borrowed spellings, rows stay
    // `Copy` — no per-row allocation.
    let mut decls: Vec<DeclHit<'a>> = Vec::new();
    let mut targets: Vec<FileId> = Vec::new();
    for SymbolDecl { file, decl } in index.symbols(id) {
        let container = (decl.container != DeclRow::NONE).then(|| {
            state.strings.resolve(
                index
                    .file(file)
                    .expect("indexed file")
                    .row(decl.container)
                    .name,
            )
        });
        decls.push(DeclHit {
            file,
            kind: decl.kind,
            name: state.strings.resolve(decl.name),
            container,
            range: decl.range,
        });
        // Equal-name rows of one file are contiguous in the iteration, so a
        // last-wins check deduplicates the reference probes below.
        if targets.last() != Some(&file) {
            targets.push(file);
        }
    }
    let refs: Vec<RefHit> = targets
        .iter()
        .flat_map(|&file| index.references(file, id))
        .map(|row| RefHit {
            target_file: row.target_file,
            kind: row.kind,
            qualified: row.qualified,
            range: row.range,
        })
        .collect();
    SymbolAnswer { decls, refs }
}

/// Splits the captures into their persistable halves (file id + declaration
/// surface); the raw sites died with the library build above.
fn captured_files(captures: Vec<FileCapture>) -> Vec<(FileId, FileIndex)> {
    captures
        .into_iter()
        .map(|captured| (captured.file, captured.index))
        .collect()
}

/// Captures one resident file's declaration and reference surface: the
/// apply-phase per-file derivation (owning-package language version, the
/// shared `l1::scan_unit` `@dart` override settlement — an above-latest
/// override drops here too, and its warning is not baked, the capture face
/// produces no diagnostics) feeding the dartq-index capture pass. The parse
/// is local to this call; only interned ids leave.
fn capture_file(state: &EngineState, file: FileId) -> FileCapture {
    let facts = &state.residency[&file];
    let bytes = state.store.get(facts.content).expect("resident content");
    let text = std::str::from_utf8(bytes).expect("engine content is UTF-8");
    // The apply-phase version derivation (l1::language_version_for): the
    // owning package's version, or the default when the file belongs to no
    // package.
    let version = state
        .packages
        .containing(state.files.path(file).expect("interned path"))
        .and_then(|identity| identity.language_version)
        .map_or(
            DEFAULT_LANGUAGE_VERSION,
            dartq_pub::LanguageVersion::into_inner,
        );
    let (feature_set, scanned, _) = crate::l1::scan_unit(text, version, file);
    capture(
        file,
        text,
        &scanned.tokens,
        ParserFeatures::from_feature_set(feature_set),
        &state.strings,
    )
}

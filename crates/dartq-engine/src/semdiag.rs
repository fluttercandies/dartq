//! The engine's default [`SemanticDiagnoser`]: the #120/#121 plug point
//! filled with `dartq_semantic::diagnose`, so the `undefined_*` /
//! `wrong_number_of_*` families reach [`Engine::analyze`](crate::Engine::analyze)
//! without the caller assembling an adapter (#236).
//!
//! Per member file the fold reads the persisted scan face
//! ([`LibraryView::scanned`], keyed by the facts key the apply recompute
//! filled) — the owning-package language version and the settled `@dart`
//! rescan resolve to a cache hit, never a re-scan; a cache miss (a face
//! evicted above the cap) re-derives the identical face locally. The fold
//! then diagnoses the unit against the analyze-built graph namespaces.
//! The token tables themselves persist in the bounded scanned cache: one
//! keystroke on a part-heavy library parses its members' faces, it does not
//! re-scan them.
//!
//! # The element index scope
//!
//! Each fold indexes exactly its own library's member element models —
//! sized like the parse it feeds and alive only for the call. The models
//! move by `Arc`: the facts hold them as `Arc<FileElements>` (#308) and
//! #286's index face stores them shared, so the fold is pure refcount
//! bumps — zero deep clones on the member walk, however large the library
//! (the perf charter: the incremental path runs through here per
//! keystroke). Consequence, contract-consistent with the M1 under-report
//! posture: a superclass or container that resolves to another library's
//! file has no index entry, so member walks through it suppress instead
//! of reporting — the same verdict the diagnoser reaches for any chain it
//! cannot see through. The #286 face makes widening the index to every
//! resident file a refcount-bump change whenever a ticket buys the oracle
//! alignment; the fold stays per-library until then.
//!
//! `dartq_semantic::diagnose` re-folds the collector's capture-time
//! duplicate log into its report front; the bake in
//! [`l1::duplicate_diagnostics`] is the single source, so the fold drops the
//! re-issued `duplicate_definition`s and passes everything else through.
//! The report's §8 names rows ([`dartq_semantic::FileReport::names`]) stay
//! unmounted until #122 moves the names table behind dartq-index.

use std::sync::Arc;

use dartq_diagnostics::Diagnostic;
use dartq_parser::ParserFeatures;
use dartq_semantic::ElementIndex;
use dartq_source::FileId;
use rustc_hash::FxHashMap;

use crate::l1;
use crate::state::UnitFacts;
use crate::{LibraryView, SemanticDiagnoser};

/// The `duplicate_definition` code name — the identity both the bake and the
/// re-fold share.
fn duplicate_definition_name() -> &'static str {
    dartq_semantic::codes::SemanticCode::DuplicateDefinition
        .code_info()
        .name
}

/// The fold's element index: each member file's model enters as an `Arc`
/// clone of the facts' share — a refcount bump, never a deep clone (#308,
/// consuming #286's shared index face).
fn build_index(files: &[FileId], facts: &FxHashMap<FileId, Arc<UnitFacts>>) -> ElementIndex {
    let mut index = ElementIndex::default();
    for &file in files {
        index.insert(file, Arc::clone(&facts[&file].elements));
    }
    index
}

/// The default semantic diagnoser over [`LibraryView`] (stateless; the
/// analyze fan-out shares one instance across workers).
pub struct Diagnoser;

impl SemanticDiagnoser for Diagnoser {
    fn diagnose_library(&self, library: LibraryView<'_>, out: &mut Vec<Diagnostic>) {
        // The library's own element models, indexed by refcount bump (member
        // files only — see the module docs for the scope contract).
        let index = build_index(library.files, library.facts);

        let duplicate = duplicate_definition_name();
        for &file in library.files {
            let facts = &library.facts[&file];
            let bytes = library.store.get(facts.content).expect("resident content");
            let text = std::str::from_utf8(bytes).expect("engine content is UTF-8");
            // The persisted scan face; a miss (eviction above the cache cap)
            // re-derives the identical face — the scan the apply recompute
            // would have run.
            let local;
            let (feature_set, tokens) = if let Some(face) = library.scanned.get(&file) {
                (face.features, &face.tokens)
            } else {
                let version = l1::language_version_for(library.packages, library.file_table, file);
                let (feature_set, face, _) = l1::scan_unit(text, version, file);
                local = face;
                (feature_set, &local.tokens)
            };
            let report = dartq_semantic::diagnose(
                text,
                tokens,
                ParserFeatures::from_feature_set(feature_set),
                library.strings,
                file,
                library.graph_library,
                library.graph,
                library.namespaces,
                &index,
            );
            out.extend(
                report
                    .diagnostics
                    .into_iter()
                    .filter(|diagnostic| diagnostic.code.name != duplicate),
            );
        }
    }
}

/// The default diagnoser as the [`crate::EngineConfig::semdiag`] face.
#[must_use]
pub fn diagnoser() -> Arc<dyn SemanticDiagnoser> {
    Arc::new(Diagnoser)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dartq_source::{FileTable, LineIndex, StringTable, content_hash};
    use std::path::Path;

    /// The fold must index the facts' element models by shared pointer:
    /// the index entry and the facts' `Arc` are the same allocation, held
    /// by exactly two refcounts (facts + index). A deep clone would fail
    /// the pointer identity — the observable that pins #308's zero-clone
    /// member walk.
    #[test]
    fn fold_indexes_share_the_facts_models() {
        let mut files = FileTable::default();
        let a = files.intern(Path::new("/t/a.dart"));
        let b = files.intern(Path::new("/t/b.dart"));
        let strings = StringTable::new();
        let facts_for = || {
            Arc::new(UnitFacts {
                content: content_hash(b""),
                api_signature: dartq_semantic::FileElements::default()
                    .api_signature_with_directives(&strings, b""),
                directives: Vec::new(),
                elements: Arc::new(dartq_semantic::FileElements::default()),
                diagnostics: Vec::new(),
                line_index: LineIndex::new(""),
            })
        };
        let facts = FxHashMap::from_iter([(a, facts_for()), (b, facts_for())]);

        let index = build_index(&[a, b], &facts);

        for file in [a, b] {
            let shared = &facts[&file].elements;
            assert!(std::ptr::eq(
                index.get(file).expect("indexed"),
                shared.as_ref()
            ));
            assert_eq!(Arc::strong_count(shared), 2);
        }
    }
}

//! The engine's default [`SemanticDiagnoser`]: the #120/#121 plug point
//! filled with `dartq_semantic::diagnose`, so the `undefined_*` /
//! `wrong_number_of_*` families reach [`Engine::analyze`](crate::Engine::analyze)
//! without the caller assembling an adapter (#236).
//!
//! Per member file the fold re-derives the apply-phase per-file inputs —
//! the owning-package language version, the settled `@dart` rescan
//! ([`l1::scan_unit`]), and the token table — then diagnoses the unit
//! against the analyze-built graph namespaces. The capture is local to the
//! call (the #210 per-query posture): nothing persists between libraries,
//! so peak cost is one library's parse.
//!
//! # The element index scope
//!
//! Each fold indexes exactly its own library's member element models —
//! sized like the parse it feeds, alive only for the call, and never a
//! workspace-wide clone (the perf charter: the incremental path runs
//! through here per keystroke). Consequence, contract-consistent with the
//! M1 under-report posture: a superclass or container that resolves to
//! another library's file has no index entry, so member walks through it
//! suppress instead of reporting — the same verdict the diagnoser reaches
//! for any chain it cannot see through. Restoring cross-library member
//! coverage needs an `Arc`-sharing element-index face in dartq-semantic
//! (its `ElementIndex` takes ownership); flagged for the semantic lane.
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

use crate::l1;
use crate::{LibraryView, SemanticDiagnoser};

/// The `duplicate_definition` code name — the identity both the bake and the
/// re-fold share.
fn duplicate_definition_name() -> &'static str {
    dartq_semantic::codes::SemanticCode::DuplicateDefinition
        .code_info()
        .name
}

/// The default semantic diagnoser over [`LibraryView`] (stateless; the
/// analyze fan-out shares one instance across workers).
pub struct Diagnoser;

impl SemanticDiagnoser for Diagnoser {
    fn diagnose_library(&self, library: LibraryView<'_>, out: &mut Vec<Diagnostic>) {
        // The library's own element models, cloned once per fold (member
        // files only — see the module docs for the scope contract).
        let mut index = ElementIndex::default();
        for &file in library.files {
            index.insert(file, library.facts[&file].elements.clone());
        }

        let duplicate = duplicate_definition_name();
        for &file in library.files {
            let facts = &library.facts[&file];
            let bytes = library.store.get(facts.content).expect("resident content");
            let text = std::str::from_utf8(bytes).expect("engine content is UTF-8");
            let version = l1::language_version_for(library.packages, library.file_table, file);
            let (feature_set, scanned, _) = l1::scan_unit(text, version, file);
            let report = dartq_semantic::diagnose(
                text,
                &scanned.tokens,
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

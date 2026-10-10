//! The impact face's core (PRD §15.4): what a change to one file would
//! affect, over the analyzed workspace.
//!
//! The query assembles the impact substrate from the engine's public graph
//! faces and the dartq-index capture:
//!
//! * the **import closure** reads the engine's library import CSR (node =
//!   engine [`dartq_engine::LibraryId`] index);
//! * the **test graph** joins the CSR import edges with the
//!   defining-file basenames (dartq-index's derivation: imports primary,
//!   the `*_test.dart` convention fallback);
//! * the **file snapshot** folds the seed library's member files — blake3
//!   over the member hashes, the library-face declaration names from the
//!   dartq-index capture pass.
//!
//! One-shot posture: a CLI run holds no baseline, so the seed enters the
//! query as a **hypothetical** API-level change ([`LibraryChange::hypothetical`])
//! — the report answers "what depends on this file's API surface". The
//! daemon passes real before/after diffs through the same query.
//!
//! [`LibraryChange::hypothetical`]: dartq_ai::impact::LibraryChange::hypothetical

use std::path::{Path, PathBuf};

use dartq_ai::impact::{self as query, FileSnapshot, LibraryChange, content_hash};
use dartq_engine::{Engine, LibraryId};
use dartq_index::{TestGraph, capture};
use dartq_language::{DEFAULT_LANGUAGE_VERSION, FeatureSet};
use dartq_parser::ParserFeatures;
use dartq_scanner::{ScanConfig, scan};
use dartq_source::{FileId, StringId, StringTable};
use thiserror::Error;

use crate::analyze::AnalyzeError;

/// The scan configuration of every non-version-gated capture face (the
/// workspace default: triple shift and augmentations on).
const SCAN_CONFIG: ScanConfig = ScanConfig {
    enable_triple_shift: true,
    enable_augmentations: true,
};

/// Why an impact run aborts: target problems are the CLI's usage tier,
/// prelude failures the internal tier.
#[derive(Debug, Error)]
pub enum ImpactError {
    /// The target path does not exist (the message names the path as given).
    #[error("Directory or file doesn't exist: {0}")]
    Missing(PathBuf),
    /// The canonical target is not an analyzed `.dart` file of the
    /// workspace.
    #[error("not an analyzed .dart file: {0}")]
    NotAnalyzed(PathBuf),
    /// The discover+analyze prelude failed.
    #[error(transparent)]
    Step(#[from] AnalyzeError),
}

/// Runs the impact face over an assembled engine: the discover+analyze
/// prelude, then the report for `target`'s owning library as a hypothetical
/// API-level change.
///
/// # Errors
///
/// [`ImpactError`] on a missing or unanalyzed target, discovery failure, or
/// cancellation.
pub fn impact(engine: &mut Engine, target: &Path) -> Result<query::ImpactReport, ImpactError> {
    crate::analyze::prelude(engine).map_err(ImpactError::from)?;
    impact_after_analysis(engine, target)
}

/// The report over an already-analyzed engine — the daemon path, whose read
/// task has just run the analysis pass.
///
/// # Errors
///
/// [`ImpactError::Missing`] / [`ImpactError::NotAnalyzed`] on a bad target.
pub fn impact_after_analysis(
    engine: &Engine,
    target: &Path,
) -> Result<query::ImpactReport, ImpactError> {
    // The seed: the target file's owning library.
    let canonical = target.canonicalize().map_err(|_| ImpactError::Missing(target.to_path_buf()))?;
    let Some(seed_file) = engine.file_table().try_file_id(&canonical) else {
        return Err(ImpactError::NotAnalyzed(canonical));
    };
    let Some(seed_library) = engine.library_of(seed_file) else {
        return Err(ImpactError::NotAnalyzed(canonical));
    };

    // Library identities and the seed's member list.
    let csr = engine.library_import_graph();
    let count = csr.nodes();
    let libraries = engine.libraries();
    let seed_library_index = seed_library.index() as usize;
    let members = libraries
        .iter()
        .find(|(library, _)| library.index() as usize == seed_library_index)
        .map(|(_, files)| files.clone())
        .expect("live seed library");
    let idents = library_identities(engine, &libraries);

    let strings = StringTable::new();
    let current = seed_snapshot(engine, &members, &strings);

    // The substrate over the engine's faces, then the query.
    let neighbors = |library: u32| -> &[u32] {
        let start = csr.offsets()[library as usize] as usize;
        let end = csr.offsets()[library as usize + 1] as usize;
        &csr.edges()[start..end]
    };
    let tests = TestGraph::build(count, neighbors, |library| {
        idents[library as usize].as_deref()
    });
    let change = LibraryChange::hypothetical(seed_library.index(), current);
    Ok(query::impact(
        count,
        neighbors,
        |library| -> &str { idents[library as usize].as_deref().expect("live library") },
        &tests,
        &strings,
        &[change],
    ))
}

/// The stable label of one library, keyed by the engine library id (`None`
/// on a dead slot). The label is the defining file's path; the test graph
/// reads the same string's directory and basename for the `*_test.dart`
/// convention (dartq-index's sibling rule).
pub(crate) type IdentTable = Vec<Option<String>>;

/// The stable label of every live library, indexed by the engine library id
/// (dead slots hold `None`).
pub(crate) fn library_identities(engine: &Engine, libraries: &[(LibraryId, Vec<FileId>)]) -> IdentTable {
    let mut idents: IdentTable = vec![None; libraries.len()];
    for (library, files) in libraries {
        let Some(path) = files
            .first()
            .and_then(|&file| engine.file_table().path(file))
        else {
            continue;
        };
        idents[library.index() as usize] = Some(path.display().to_string());
    }
    idents
}

/// The folded surface of one library: blake3 over the member hashes, the
/// library-face declaration names from the capture pass (parse-surface
/// capture proceeds through scan diagnostics — the under-report bias; this
/// face produces no diagnostics of its own).
fn seed_snapshot(engine: &Engine, members: &[FileId], strings: &StringTable) -> FileSnapshot {
    let mut names: Vec<StringId> = Vec::new();
    let mut folded = Vec::with_capacity(members.len() * 32);
    for &file in members {
        let bytes = engine.content(file).expect("resident member");
        folded.extend_from_slice(&content_hash(bytes));
        let text = std::str::from_utf8(bytes).expect("engine content is UTF-8");
        let scanned = scan(text, SCAN_CONFIG, file);
        let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
            DEFAULT_LANGUAGE_VERSION,
        ));
        let captured = capture(file, text, &scanned.tokens, features, strings);
        names.extend(
            captured
                .index
                .decls()
                .iter()
                .filter(|row| row.is_library_face())
                .map(|row| row.name),
        );
    }
    names.sort_unstable();
    names.dedup();
    FileSnapshot {
        content_hash: content_hash(&folded),
        top_level_names: names.into_boxed_slice(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::{open_engine, test_fixture::workspace};

    /// The naming-fallback precision (ai-audit X1) end-to-end over the real
    /// engine: two same-basename `a.dart` in different directories plus a
    /// sibling `test/a_test.dart` with no imports. The sibling seed sees
    /// its test; the homonym seed gains nothing — the old workspace-wide
    /// basename fan-out would have listed the test for both.
    #[test]
    fn test_fallback_resolves_the_sibling_only() {
        let root = workspace(
            "impact_sibling",
            &[
                ("lib/a.dart", "class A {}\n"),
                ("lib/other/a.dart", "class B {}\n"),
                ("test/a.dart", "class C {}\n"),
                ("test/a_test.dart", "void main() {}\n"),
            ],
        );
        let (mut engine, _) = open_engine(&root, Some(&root.join("sdk"))).expect("engine");

        let sibling = root.join("test/a.dart");
        let report = impact(&mut engine, &sibling).expect("sibling impact");
        let tests: Vec<&str> = report
            .affected_tests
            .iter()
            .map(|row| row.path.as_str())
            .collect();
        assert_eq!(tests.len(), 1, "the sibling test, and nothing else");
        assert!(tests[0].ends_with("test/a_test.dart"));

        let homonym = root.join("lib/other/a.dart");
        let report = impact(&mut engine, &homonym).expect("homonym impact");
        assert!(
            report.affected_tests.is_empty(),
            "no test edge reaches a same-basename homonym"
        );

        std::fs::remove_dir_all(&root).ok();
    }
}

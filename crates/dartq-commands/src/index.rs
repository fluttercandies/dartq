//! The `dartq index` face (PRD §15.1): build the dartq-index symbol index
//! over the analyzed workspace and report what it built.
//!
//! # Build shape
//!
//! The index assembly (per-library capture + resolution through the
//! namespace tables, folded into `dartq_index::SymbolIndex`) lives behind
//! the engine's `pub(crate)` query face, which answers by-name only. This
//! face rides the engine's one public per-library window — the
//! [`dartq_engine::SemanticDiagnoser`] hook, whose [`dartq_engine::LibraryView`]
//! carries exactly the query face's inputs (member files, resolved graph,
//! namespaces, intern table): the collector re-runs the capture fan-out at
//! diagnose time and tallies the finished surfaces. When the engine grows a
//! first-class index face (build-once + row totals), this collector is the
//! seam to delete.
//!
//! The collector replaces the default semantic diagnoser on this face's
//! engine: the face reports counts, not diagnostics, so the semantic fold
//! would be whole-workspace work on discarded output. The scan settlement
//! is `crate::check::scan_file_at` — the same `@dart` override contract the
//! engine's capture applies; the capture face produces no diagnostics, so
//! the above-latest warning rides back dropped.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use dartq_engine::{Engine, LibraryView, SemanticDiagnoser};
use dartq_language::FeatureSet;
use dartq_parser::ParserFeatures;
use serde_json::json;
use thiserror::Error;

use crate::analyze::{AnalyzeError, prelude};
use crate::check::scan_file_at;
use crate::workspace::{WorkspaceError, engine_config};

/// Why an index run aborts: the workspace assembly (usage tier) or the
/// analyze prelude (internal tier).
#[derive(Debug, Error)]
pub enum IndexError {
    /// The workspace faces are unusable (the CLI's usage tier).
    #[error("{0}")]
    Workspace(#[from] WorkspaceError),
    /// The engine context could not bind (the CLI's usage tier).
    #[error("engine context unusable: {0}")]
    Engine(String),
    /// Discovery failure or cancellation (the CLI's internal tier).
    #[error("{0}")]
    Analyze(#[from] AnalyzeError),
}

/// What one index run built: the analyzed workspace's row totals.
///
/// `symbols` counts declaration rows (every `FileIndex` entry);
/// `references` counts resolved reference rows (`LibraryIndex::refs` — the
/// rows `query symbol`'s reference surface serves). The unresolved-names
/// table is the recovery surface, not the reference surface, and is not
/// counted; supertype edges are a separate table and not counted.
#[derive(Debug, Default, Clone, Copy)]
pub struct IndexRun {
    /// Member files captured into the index.
    pub files: usize,
    /// Libraries whose reference surfaces were built.
    pub libraries: usize,
    /// Declaration rows indexed.
    pub symbols: usize,
    /// Resolved reference rows indexed.
    pub references: usize,
}

impl IndexRun {
    /// The JSON face: one object — `schema_version` plus the row totals
    /// (snake_case, the metrics-JSON convention). The index domain has no
    /// dartq-schema document yet; the constant is the shared v1.
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        json!({
            "schema_version": crate::SCHEMA_VERSION,
            "files": self.files,
            "libraries": self.libraries,
            "symbols": self.symbols,
            "references": self.references,
        })
    }
}

/// One index run over the workspace at `dir`: assemble the engine (the
/// collector diagnoser installed), run the discover+analyze prelude (the
/// read-after-write the index reflects — the collector harvests during the
/// analyze fan-out), and return the tallied totals.
///
/// `full` is the §15.1 grammar's `--full`: force a rebuild instead of
/// reusing incremental state. The engine has no resident index yet — every
/// run builds fresh — so both modes build full; the flag pins the grammar
/// until engine-side incremental index maintenance lands.
///
/// # Errors
///
/// [`IndexError`] on workspace-assembly, engine-bind, discovery, or
/// cancellation failure.
pub fn run(dir: &Path, sdk: Option<&Path>, full: bool) -> Result<IndexRun, IndexError> {
    // The flag has no behavioral counterpart yet (module docs); bound it so
    // the parameter stays honest when incremental reuse arrives.
    let _ = full;
    let (mut config, _dir) = engine_config(dir, sdk)?;
    let collector = Arc::new(IndexCollector::default());
    config.semdiag = Some(collector.clone() as Arc<dyn SemanticDiagnoser>);
    let mut engine = Engine::new(config)
        .map_err(|error| IndexError::Engine(error.to_string()))?;
    prelude(&mut engine)?;
    Ok(IndexRun {
        files: collector.files.load(Ordering::Relaxed),
        libraries: collector.libraries.load(Ordering::Relaxed),
        symbols: collector.symbols.load(Ordering::Relaxed),
        references: collector.references.load(Ordering::Relaxed),
    })
}

/// Harvests the per-library index surfaces the analyze fan-out offers
/// through [`LibraryView`], tallying row totals. The counters are atomic:
/// `diagnose_library` runs on the batch fan-out workers.
#[derive(Default)]
struct IndexCollector {
    files: AtomicUsize,
    libraries: AtomicUsize,
    symbols: AtomicUsize,
    references: AtomicUsize,
}

impl SemanticDiagnoser for IndexCollector {
    fn diagnose_library(&self, view: LibraryView<'_>, _out: &mut Vec<dartq_diagnostics::Diagnostic>) {
        self.libraries.fetch_add(1, Ordering::Relaxed);
        self.files.fetch_add(view.files.len(), Ordering::Relaxed);

        let mut builder = dartq_index::LibraryBuilder::new(
            view.graph_library,
            view.graph,
            view.namespaces,
        );
        for &file in view.files {
            let facts = view.facts.get(&file).expect("analyzed member");
            let bytes = view
                .store
                .get(facts.content())
                .expect("resident content");
            let text = std::str::from_utf8(bytes).expect("engine content is UTF-8");
            // The apply-phase version derivation (the engine capture's): the
            // owning package's version, or the default when the file belongs
            // to no package.
            let version = view
                .packages
                .containing(view.file_table.path(file).expect("interned path"))
                .and_then(|identity| identity.language_version)
                .map_or(
                    dartq_language::DEFAULT_LANGUAGE_VERSION,
                    dartq_pub::LanguageVersion::into_inner,
                );
            let (settled, scanned, _) = scan_file_at(text, version, file);
            let captured = dartq_index::capture(
                file,
                text,
                &scanned.tokens,
                ParserFeatures::from_feature_set(FeatureSet::for_language_version(settled)),
                view.strings,
            );
            self.symbols.fetch_add(captured.index.len(), Ordering::Relaxed);
            builder.add_file(&captured);
        }
        let surface = builder.finish();
        self.references.fetch_add(surface.refs().len(), Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::test_fixture::workspace;

    /// Three libraries over one imported declaration surface (the same
    /// fixture the query-face tests pin): a.dart declares 3 rows, b.dart and
    /// c.dart construct `Foo` cross-file and b.dart calls `top`. Hand-traced
    /// totals: files 3, libraries 3, symbols 3+1+1, references 2 (b.dart) +
    /// 1 (c.dart).
    #[test]
    fn counts_pin_the_fixture_totals() {
        let root = workspace(
            "index",
            &[
                ("lib/a.dart", "class Foo {\n  void meth() {}\n}\nvoid top() {}\n"),
                ("lib/b.dart", "import 'a.dart';\nvoid main() {\n  var f = Foo();\n  top();\n}\n"),
                ("lib/c.dart", "import 'a.dart';\nvoid other() {\n  var f2 = Foo();\n}\n"),
            ],
        );
        let run = run(&root, Some(&root.join("sdk")), true).expect("index run");
        assert_eq!(run.files, 3);
        assert_eq!(run.libraries, 3);
        assert_eq!(run.symbols, 5, "3 decl rows in a.dart, 1 each in b/c");
        assert_eq!(run.references, 3, "2 resolved uses in b.dart, 1 in c.dart");

        let json = run.to_json();
        assert_eq!(json["schema_version"], crate::SCHEMA_VERSION);
        assert_eq!(json["files"], 3);
        assert_eq!(json["libraries"], 3);
        assert_eq!(json["symbols"], 5);
        assert_eq!(json["references"], 3);

        std::fs::remove_dir_all(&root).ok();
    }

    /// A workspace with no Dart files still reports a valid empty index.
    #[test]
    fn empty_workspace_counts_are_zero() {
        let root = workspace("index_empty", &[]);
        let run = run(&root, Some(&root.join("sdk")), false).expect("index run");
        assert_eq!(
            (run.files, run.libraries, run.symbols, run.references),
            (0, 0, 0, 0)
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// An unusable workspace (no package config) is the usage tier.
    #[test]
    fn missing_package_config_is_a_workspace_error() {
        let root = workspace("index_broken", &[]);
        std::fs::remove_file(root.join(".dart_tool/package_config.json")).ok();
        match run(&root, Some(&root.join("sdk")), false) {
            Err(IndexError::Workspace(_)) => {}
            other => panic!("expected workspace error, got {other:?}"),
        }
        std::fs::remove_dir_all(&root).ok();
    }
}

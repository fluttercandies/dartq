//! The `dartq index` face (PRD §15.1): report the dartq-index symbol index
//! the analyzed workspace maintains.
//!
//! # Build shape
//!
//! The index is the engine's maintained state, not a face-side rebuild:
//! analyze recaptures every changed file into the per-file captures and
//! rebuilds each live library's reference surface at its `LibraryAnalyzed`
//! point (index-design-spec §6/§8) — the same state `query_symbols`
//! serves. This face tallies it through the engine's read hooks
//! ([`dartq_engine::Engine::file_capture`] /
//! [`dartq_engine::Engine::library_surfaces`]): no scan, no parse, no
//! second capture pass. The semantic fold plays no role in the counts, so
//! the face skips it (`semdiag = None`); the collector this face retired
//! skipped it the same way.

use std::path::Path;

use dartq_engine::Engine;
use serde_json::json;
use thiserror::Error;

use crate::analyze::{AnalyzeError, prelude};
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

/// One index run over the workspace at `dir`: assemble the engine, run the
/// discover+analyze prelude (the read-after-write the counts reflect), and
/// tally the maintained index state's row totals.
///
/// `full` is the §15.1 grammar's `--full`: force a rebuild instead of
/// reusing incremental state. One CLI run assembles a fresh engine and
/// analyzes the whole workspace, so the tallies are a full build every
/// time; the flag pins the grammar.
///
/// # Panics
///
/// Panics on an engine-contract violation, surfaced never clamped: a live
/// library member without a capture is a program bug.
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
    // The counts read the maintained index state; the semantic fold's
    // diagnostics are discarded on this face, so the fold is skipped.
    config.semdiag = None;
    let mut engine = Engine::new(config).map_err(|error| IndexError::Engine(error.to_string()))?;
    prelude(&mut engine)?;

    // The row totals off the maintained state: member files and their
    // declaration rows (the captures), the live libraries and their
    // resolved reference rows (the surfaces).
    let libraries = engine.libraries();
    let mut files = 0;
    let mut symbols = 0;
    for (_, members) in &libraries {
        files += members.len();
        for &file in members {
            symbols += engine
                .file_capture(file)
                .expect("analyzed member")
                .index
                .len();
        }
    }
    let mut built = 0;
    let mut references = 0;
    for (_, surface) in engine.library_surfaces() {
        built += 1;
        references += surface.reference_count();
    }
    Ok(IndexRun {
        files,
        libraries: built,
        symbols,
        references,
    })
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
                (
                    "lib/a.dart",
                    "class Foo {\n  void meth() {}\n}\nvoid top() {}\n",
                ),
                (
                    "lib/b.dart",
                    "import 'a.dart';\nvoid main() {\n  var f = Foo();\n  top();\n}\n",
                ),
                (
                    "lib/c.dart",
                    "import 'a.dart';\nvoid other() {\n  var f2 = Foo();\n}\n",
                ),
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

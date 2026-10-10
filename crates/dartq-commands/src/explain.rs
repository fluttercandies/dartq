//! The explain face's core (PRD §15.3): the AI's one-stop symbol report —
//! the commands wiring of the [`dartq_ai::explain`] assembler.
//!
//! The report answers from the engine's maintained index state after the
//! one discover+analyze prelude — no second analysis pass runs. The
//! declaration hits read the per-file captures
//! ([`dartq_engine::Engine::file_capture`]), the reference and supertype
//! rows the per-library reference surfaces
//! ([`dartq_engine::Engine::library_surfaces`]) — the same maintained
//! state `query_symbols` serves, at raw-row fidelity so the assembler's
//! container anchors resolve in place. The rows' `StringId`s are native
//! to the engine's intern table, which the answer resolves through. The
//! transient per-answer maps (hit grouping, supertype projection) are
//! symbol-scoped and dropped at the end of the call.
//!
//! The reverse import graph and the test graph build once per call from
//! the engine's import CSR — the same per-query posture `impact` keeps
//! over the same CSR. The semantic fold plays no role in the report, so
//! the face skips it (`semdiag = None`); the collectors this face retired
//! skipped it the same way.
//!
//! The JSON answer is the assembler's serialized report routed through
//! `dartq-schema`'s `validate(payload, Domain::Explain)` where the payload
//! is born; a violation is a producer bug and stops the run (surfaced,
//! never clamped).

use std::collections::BTreeMap;
use std::path::Path;

use dartq_ai::explain::{self as query, SymbolDeclHit};
use dartq_ai::rev_import_graph::RevImportGraph;
use dartq_engine::Engine;
use dartq_index::{RefRow, TargetRef, TestGraph};
use dartq_source::FileId;
use serde_json::Value;
use thiserror::Error;

use crate::Verbosity;
use crate::analyze::{AnalyzeError, prelude};
use crate::impact::library_identities;
use crate::workspace::{WorkspaceError, engine_config};

/// Why an explain run aborts: workspace problems are the CLI's usage tier,
/// the prelude the internal tier.
#[derive(Debug, Error)]
pub enum ExplainError {
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

/// Runs the explain face over the workspace at `dir`: assemble the engine,
/// run the discover+analyze prelude (the read-after-write the report
/// reflects), and answer for `symbol` from the maintained index state as
/// schema-valid JSON.
///
/// # Errors
///
/// [`ExplainError`] on workspace-assembly, engine-bind, discovery, or
/// cancellation failure.
pub fn explain(
    dir: &Path,
    sdk: Option<&Path>,
    symbol: &str,
    verbosity: Verbosity,
) -> Result<Value, ExplainError> {
    let (mut config, _dir) = engine_config(dir, sdk)?;
    // The report reads the maintained index state; the semantic fold's
    // diagnostics are discarded on this face, so the fold is skipped.
    config.semdiag = None;
    let mut engine =
        Engine::new(config).map_err(|error| ExplainError::Engine(error.to_string()))?;
    prelude(&mut engine)?;
    Ok(explain_answer(&engine, symbol, verbosity))
}

/// The report over an already-analyzed engine: the adapter over the
/// maintained index state (transient, symbol-scoped — dropped at the end
/// of the call) feeding the assembler.
///
/// # Panics
///
/// Panics on engine-contract violations, surfaced never clamped: a live
/// library member without a capture, or an answer file id missing from
/// the engine's file table, is a program bug.
fn explain_answer(engine: &Engine, symbol: &str, verbosity: Verbosity) -> Value {
    // The queried name interns into the engine's table once — the rows'
    // native table (the documented query posture: the only side effect).
    let sym = engine.strings().intern(symbol);

    // Declaration hits from the maintained captures: live libraries ×
    // members, the equal-name run per declaring file. The rows borrow the
    // captures — nothing copies.
    let libraries = engine.libraries();
    let mut hits: Vec<SymbolDeclHit<'_>> = Vec::new();
    let mut hit_files: BTreeMap<u32, Vec<FileId>> = BTreeMap::new();
    for (library, members) in &libraries {
        let library = library.index();
        for &file in members {
            let capture = engine.file_capture(file).expect("analyzed member");
            if let Some(rows) = capture.index.by_name(sym) {
                hits.extend(
                    rows.iter().map(|decl| SymbolDeclHit { library, file, decl }),
                );
                hit_files.entry(library).or_default().push(file);
            }
        }
    }

    // The reference rows naming the symbol and the hit files' resolved
    // supertype targets — one pass over the maintained surfaces (the
    // by-name union the assembler filters to the primary declaration).
    let mut refs: Vec<RefRow> = Vec::new();
    let mut supers: BTreeMap<FileId, Vec<TargetRef>> = BTreeMap::new();
    for (library, surface) in engine.library_surfaces() {
        refs.extend(
            surface
                .refs()
                .iter()
                .filter(|row| row.target_name == sym)
                .copied(),
        );
        if let Some(files) = hit_files.get(&library.index()) {
            for &file in files {
                let targets = surface
                    .supertypes_of(file, sym)
                    .map_or(Vec::new(), |edges| {
                        edges.iter().map(|edge| edge.supertype).collect()
                    });
                supers.insert(file, targets);
            }
        }
    }

    // The graph faces over the engine's import CSR — the substrate
    // `impact` reads (one build each per call).
    let csr = engine.library_import_graph();
    let count = csr.nodes();
    let idents = library_identities(engine, &libraries);
    let neighbors = |library: u32| -> &[u32] {
        let start = csr.offsets()[library as usize] as usize;
        let end = csr.offsets()[library as usize + 1] as usize;
        &csr.edges()[start..end]
    };
    let importers = RevImportGraph::build(count, &neighbors);
    let tests = TestGraph::build(count, &neighbors, |library| {
        idents[library as usize].as_deref()
    });

    // Stable path text for every file id the answer carries (internal ids
    // never leave the engine boundary).
    let file_table = engine.file_table();
    let mut ids: Vec<FileId> = Vec::new();
    ids.extend(hits.iter().map(|hit| hit.file));
    ids.extend(refs.iter().map(|row| row.file));
    ids.extend(supers.values().flat_map(|targets| {
        targets.iter().filter_map(|target| match target {
            TargetRef::Decl(file, _) => Some(*file),
            TargetRef::Name(_) => None,
        })
    }));
    ids.sort_unstable();
    ids.dedup();
    let file_labels: Vec<String> = ids
        .iter()
        .map(|&file| {
            file_table
                .path(file)
                .expect("answer file")
                .display()
                .to_string()
        })
        .collect();

    let report = query::explain(
        symbol,
        verbosity.into(),
        &hits,
        &refs,
        |file| {
            engine
                .file_capture(file)
                .map_or(&[][..], |capture| capture.index.decls())
        },
        |file, _| supers.get(&file).map_or(&[][..], |targets| targets.as_slice()),
        &importers,
        |library| idents[library as usize].as_deref().expect("live library"),
        |file| {
            let index = ids.binary_search(&file).expect("answer file label");
            &file_labels[index]
        },
        &tests,
        engine.strings(),
    );
    // One serialization into the answer `Value`, then one serialization of
    // that Value to the validator's &str face (`dartq-schema::validate` is
    // string-typed) — the report is converted exactly twice, never
    // re-parsed.
    let value = serde_json::to_value(&report).expect("explain report serializes");
    let json = serde_json::to_string(&value).expect("explain report serializes");
    dartq_schema::validate(&json, dartq_schema::Domain::Explain).unwrap_or_else(|violation| {
        panic!("explain report violates the published schema: {violation}")
    });
    value
}

/// The commands [`Verbosity`] onto the assembler's: same three tiers, same
/// default — the spelling mapping is the whole job.
impl From<Verbosity> for query::Verbosity {
    fn from(verbosity: Verbosity) -> Self {
        match verbosity {
            Verbosity::Minimal => query::Verbosity::Minimal,
            Verbosity::Compact => query::Verbosity::Compact,
            Verbosity::Full => query::Verbosity::Full,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::json;

    use super::*;
    use crate::workspace::test_fixture::workspace;

    /// One import chain plus its test: `animal.dart` declares `Animal`
    /// (member `breathe`), `dog.dart` declares `Dog extends Animal`
    /// (member `bark`), `main.dart` imports `dog.dart` and constructs
    /// `Dog`, and `test/dog_test.dart` imports it by package URI.
    const ANIMAL: &str = "class Animal {\n  void breathe() {}\n}\n";
    const DOG: &str = "import 'animal.dart';\nclass Dog extends Animal {\n  void bark() {}\n}\n";
    const MAIN: &str = "import 'dog.dart';\nvoid main() {\n  var d = Dog();\n  d.bark();\n}\n";
    const TEST: &str = "import 'package:fixture/dog.dart';\nvoid main() {\n  var d = Dog();\n}\n";

    fn fixture(tag: &str) -> PathBuf {
        workspace(
            tag,
            &[
                ("lib/animal.dart", ANIMAL),
                ("lib/dog.dart", DOG),
                ("lib/main.dart", MAIN),
                ("test/dog_test.dart", TEST),
            ],
        )
    }

    /// The name token's start offset in `source` (ASCII sources, so byte
    /// offset = UTF-16 offset).
    fn offset(source: &str, needle: &str) -> u32 {
        source
            .find(needle)
            .unwrap_or_else(|| panic!("{needle} in source")) as u32
    }

    /// End-to-end over the real engine + workspace fixture: the report
    /// validates against `Domain::Explain` and every section the substrate
    /// can reach carries its row — the declaration, the superclass, the
    /// member, both use sites (each naming its use file), the importing
    /// libraries, and the test library.
    #[test]
    fn explain_report_pins_the_sections_and_validates() {
        let root = fixture("explain");
        let json = explain(&root, Some(&root.join("sdk")), "Dog", Verbosity::Compact)
            .expect("explain run");
        let root = root.canonicalize().expect("root");
        let animal = root.join("lib/animal.dart").display().to_string();
        let dog = root.join("lib/dog.dart").display().to_string();
        let main = root.join("lib/main.dart").display().to_string();
        let dog_test = root.join("test/dog_test.dart").display().to_string();

        // The entry already routes through the published validator; the
        // assertion pins that the answer is born schema-valid.
        let text = serde_json::to_string(&json).expect("serialize");
        dartq_schema::validate(&text, dartq_schema::Domain::Explain).expect("schema-valid");

        assert_eq!(json["schema_version"], 1);
        assert_eq!(json["symbol"], "Dog");
        assert_eq!(
            json["declaration"],
            json!({
                "file": dog,
                "name": "Dog",
                "kind": "class",
                "range": [offset(DOG, "Dog"), offset(DOG, "Dog") + 3],
            })
        );
        assert_eq!(
            json["inheritance"],
            json!([{"name": "Animal", "file": animal}])
        );
        assert_eq!(json["interfaces"], json!([]), "the documented seam");
        assert_eq!(json["constructors"], json!([]), "the documented seam");
        assert_eq!(
            json["methods"],
            json!([{"name": "bark", "kind": "method", "range": [offset(DOG, "bark"), offset(DOG, "bark") + 4]}])
        );
        assert_eq!(
            json["references"],
            json!([
                {"file": main, "kind": "invoked", "range": [offset(MAIN, "Dog"), offset(MAIN, "Dog") + 3], "qualified": false},
                {"file": dog_test, "kind": "invoked", "range": [offset(TEST, "Dog"), offset(TEST, "Dog") + 3], "qualified": false},
            ]),
            "each row names its use site's file, path order"
        );
        assert_eq!(
            json["dependencies"],
            json!([{"path": main}, {"path": dog_test}]),
            "the libraries that import dog.dart"
        );
        assert_eq!(json["affected_tests"], json!([{"path": dog_test}]));

        std::fs::remove_dir_all(&root).ok();
    }

    /// `minimal` keeps the declaration locators and replaces every array
    /// with the `counts` object.
    #[test]
    fn minimal_truncates_to_declaration_and_counts() {
        let root = fixture("explain_minimal");
        let json = explain(&root, Some(&root.join("sdk")), "Dog", Verbosity::Minimal)
            .expect("explain run");
        let root = root.canonicalize().expect("root");

        assert_eq!(json["symbol"], "Dog");
        assert_eq!(json["declaration"]["name"], "Dog", "the red line stays");
        assert!(json["declaration"].get("container").is_none());
        for section in [
            "inheritance",
            "interfaces",
            "constructors",
            "methods",
            "references",
            "dependencies",
            "affected_tests",
        ] {
            assert!(json.get(section).is_none(), "{section} is dropped");
        }
        assert_eq!(
            json["counts"],
            json!({
                "inheritance": 1,
                "interfaces": 0,
                "constructors": 0,
                "methods": 1,
                "references": 2,
                "dependencies": 2,
                "affected_tests": 1,
            })
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// A name that declares nowhere answers the seam report: no
    /// declaration, every section empty (never absent at compact).
    #[test]
    fn unknown_symbol_answers_the_empty_seam_report() {
        let root = fixture("explain_unknown");
        let json = explain(&root, Some(&root.join("sdk")), "Nope", Verbosity::Compact)
            .expect("explain run");
        let root = root.canonicalize().expect("root");

        assert_eq!(json["schema_version"], 1);
        assert_eq!(json["symbol"], "Nope");
        assert!(json.get("declaration").is_none());
        assert_eq!(json["inheritance"], json!([]));
        assert_eq!(json["methods"], json!([]));
        assert_eq!(json["references"], json!([]));
        assert_eq!(json["dependencies"], json!([]));
        assert_eq!(json["affected_tests"], json!([]));
        assert!(
            json.get("counts").is_none(),
            "counts are a minimal-tier field"
        );

        std::fs::remove_dir_all(&root).ok();
    }
}

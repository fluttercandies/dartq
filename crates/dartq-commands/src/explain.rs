//! The explain face's core (PRD §15.3): the AI's one-stop symbol report —
//! the commands wiring of the [`dartq_ai::explain`] assembler.
//!
//! The report joins index surfaces the engine keeps behind its `pub(crate)`
//! index query (see [`crate::index`]'s module docs), so this face rides the
//! same one public per-library window — the
//! [`dartq_engine::SemanticDiagnoser`] hook — as the index face does. The
//! [`ExplainCollector`] harvests exactly what the queried symbol needs
//! while the analyze fan-out runs:
//!
//! * the declaring files' full declaration surfaces (containers and member
//!   anchors resolve through them), remapped into the harvest's own intern
//!   table — the engine's table is not exported;
//! * the library's resolved reference rows naming the symbol (the
//!   assembler filters them to the primary declaration's file);
//! * the resolved supertype targets of `(file, symbol)`, mapped from the
//!   built library index's [`dartq_index::SubtypeEdge`]s onto
//!   [`dartq_index::TargetRef`]s.
//!
//! Harvesting is symbol-filtered: files that do not declare the name
//! contribute nothing. The reverse import graph and the test graph build
//! once per call from the engine's import CSR — the same per-query
//! posture `impact_after_analysis` keeps over the same CSR. When the
//! engine grows a first-class index face, both this harvest and that
//! per-call build are the seams to delete.
//!
//! The JSON answer is the assembler's serialized report routed through
//! `dartq-schema`'s `validate(payload, Domain::Explain)` where the payload
//! is born; a violation is a producer bug and stops the run (surfaced,
//! never clamped).

use std::path::Path;
use std::sync::{Arc, Mutex};

use dartq_ai::explain::{self as query, SymbolDeclHit};
use dartq_ai::rev_import_graph::RevImportGraph;
use dartq_diagnostics::Diagnostic;
use dartq_engine::{Engine, LibraryView, SemanticDiagnoser};
use dartq_index::{DeclRow, LibraryBuilder, RefRow, TargetRef, TestGraph, capture};
use dartq_language::{DEFAULT_LANGUAGE_VERSION, FeatureSet};
use dartq_parser::ParserFeatures;
use dartq_source::{FileId, StringTable};
use serde_json::Value;
use thiserror::Error;

use crate::Verbosity;
use crate::analyze::{AnalyzeError, prelude};
use crate::check::scan_file_at;
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

/// Runs the explain face over the workspace at `dir`: assemble the engine
/// (the [`ExplainCollector`] installed), run the discover+analyze prelude
/// (the harvest rides the analyze fan-out), and return the report for
/// `symbol` as schema-valid JSON.
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
    let collector = Arc::new(ExplainCollector::new(symbol.to_owned()));
    let (mut config, _dir) = engine_config(dir, sdk)?;
    config.semdiag = Some(collector.clone() as Arc<dyn SemanticDiagnoser>);
    let mut engine = Engine::new(config).map_err(|error| ExplainError::Engine(error.to_string()))?;
    prelude(&mut engine)?;
    Ok(explain_after_analysis(&engine, &collector, verbosity))
}

/// The report over an already-analyzed engine — the daemon path, which
/// installs [`ExplainCollector::new`] on its engine config at assembly and
/// holds the arc. `collector` must be the one the engine was built with:
/// an unharvested collector is indistinguishable from an unknown symbol.
#[must_use]
pub fn explain_after_analysis(
    engine: &Engine,
    collector: &ExplainCollector,
    verbosity: Verbosity,
) -> Value {
    let harvest = collector.harvest.lock().expect("harvest lock poisoned");
    let sym = harvest.strings.intern(&collector.symbol);

    // The graph faces over the engine's import CSR — the substrate
    // `impact` reads (one build each per call).
    let csr = engine.library_import_graph();
    let count = csr.nodes();
    let idents = library_identities(engine, &engine.libraries());
    let neighbors = |library: u32| -> &[u32] {
        let start = csr.offsets()[library as usize] as usize;
        let end = csr.offsets()[library as usize + 1] as usize;
        &csr.edges()[start..end]
    };
    let importers = RevImportGraph::build(count, &neighbors);
    let tests = TestGraph::build(count, &neighbors, |library| {
        idents[library as usize].as_deref()
    });

    // Stable path text for every file id the harvest carries (internal ids
    // never leave the engine boundary).
    let file_table = engine.file_table();
    let mut ids: Vec<FileId> = Vec::new();
    ids.extend(harvest.surfaces.iter().map(|(_, file, _)| *file));
    ids.extend(harvest.refs.iter().map(|row| row.file));
    ids.extend(harvest.supers.iter().flat_map(|(_, targets)| {
        targets
            .iter()
            .filter_map(|target| match target {
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
                .expect("harvested file")
                .display()
                .to_string()
        })
        .collect();

    let hits: Vec<SymbolDeclHit<'_>> = harvest
        .surfaces
        .iter()
        .flat_map(|(library, file, surface)| {
            surface.iter().filter(|row| row.name == sym).map(move |row| {
                SymbolDeclHit {
                    library: *library,
                    file: *file,
                    decl: row,
                }
            })
        })
        .collect();
    let report = query::explain(
        &collector.symbol,
        verbosity.into(),
        &hits,
        &harvest.refs,
        |file| {
            harvest
                .surfaces
                .iter()
                .find(|(_, f, _)| *f == file)
                .map_or(&[], |(_, _, surface)| surface.as_slice())
        },
        |file, _| {
            harvest
                .supers
                .iter()
                .find(|(f, _)| *f == file)
                .map_or(&[], |(_, targets)| targets.as_slice())
        },
        &importers,
        |library| idents[library as usize].as_deref().expect("live library"),
        |file| {
            let index = ids.binary_search(&file).expect("harvested file label");
            &file_labels[index]
        },
        &tests,
        &harvest.strings,
    );
    let json = serde_json::to_string(&report).expect("explain report serializes");
    dartq_schema::validate(&json, dartq_schema::Domain::Explain).unwrap_or_else(|violation| {
        panic!("explain report violates the published schema: {violation}")
    });
    serde_json::from_str(&json).expect("validated JSON re-parses")
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

/// Harvests the queried symbol's index surfaces through the analyze
/// fan-out's one public window ([`LibraryView`]). The harvest data is the
/// assembler's input faces, symbol-filtered at capture time.
pub struct ExplainCollector {
    symbol: String,
    harvest: Mutex<Harvest>,
}

impl ExplainCollector {
    /// The collector for one `explain` query over `symbol`.
    #[must_use]
    pub fn new(symbol: String) -> Self {
        Self {
            symbol,
            harvest: Mutex::default(),
        }
    }
}

/// What one harvest holds. `StringId`s are remapped into `strings` (the
/// engine's intern table is not exported); `FileId`s stay engine-native —
/// the report's labels resolve through the engine's file table.
#[derive(Default)]
struct Harvest {
    strings: StringTable,
    /// Full declaration surfaces of the files that declare the symbol, as
    /// `(engine library id, file, rows)` — full, so the rows' container
    /// anchors index validly.
    surfaces: Vec<(u32, FileId, Vec<DeclRow>)>,
    /// Resolved reference rows naming the symbol (the by-name union).
    refs: Vec<RefRow>,
    /// The resolved supertype targets of `(file, symbol)` per hit file.
    supers: Vec<(FileId, Vec<TargetRef>)>,
}

impl SemanticDiagnoser for ExplainCollector {
    fn diagnose_library(&self, view: LibraryView<'_>, _out: &mut Vec<Diagnostic>) {
        let queried = view.strings.intern(&self.symbol);
        let mut harvest = self.harvest.lock().expect("harvest lock poisoned");
        let local = harvest.strings.intern(&self.symbol);
        let library = view.library.index() as u32;
        let mut builder = LibraryBuilder::new(view.graph_library, view.graph, view.namespaces);
        let mut hit_files: Vec<FileId> = Vec::new();
        for &file in view.files {
            let facts = view.facts.get(&file).expect("analyzed member");
            let bytes = view.store.get(facts.content()).expect("resident content");
            let text = std::str::from_utf8(bytes).expect("engine content is UTF-8");
            // The apply-phase version derivation (the engine capture's): the
            // owning package's version, or the default when the file belongs
            // to no package.
            let version = view
                .packages
                .containing(view.file_table.path(file).expect("interned path"))
                .and_then(|identity| identity.language_version)
                .map_or(
                    DEFAULT_LANGUAGE_VERSION,
                    dartq_pub::LanguageVersion::into_inner,
                );
            let (settled, scanned, _) = scan_file_at(text, version, file);
            let captured = capture(
                file,
                text,
                &scanned.tokens,
                ParserFeatures::from_feature_set(FeatureSet::for_language_version(settled)),
                view.strings,
            );
            if captured.index.by_name(queried).is_some() {
                hit_files.push(file);
                let surface = captured
                    .index
                    .decls()
                    .iter()
                    .map(|row| DeclRow {
                        name: harvest.strings.intern(view.strings.resolve(row.name)),
                        ..*row
                    })
                    .collect();
                harvest.surfaces.push((library, file, surface));
            }
            builder.add_file(&captured);
        }
        let surface = builder.finish();
        harvest
            .refs
            .extend(surface.refs().iter().filter(|row| row.target_name == queried).map(
                |row| RefRow {
                    target_name: local,
                    ..*row
                },
            ));
        for file in hit_files {
            let targets = surface
                .supertypes_of(file, queried)
                .unwrap_or_default()
                .iter()
                .map(|edge| match edge.supertype {
                    TargetRef::Decl(target_file, name) => TargetRef::Decl(
                        target_file,
                        harvest.strings.intern(view.strings.resolve(name)),
                    ),
                    TargetRef::Name(name) => {
                        TargetRef::Name(harvest.strings.intern(view.strings.resolve(name)))
                    }
                })
                .collect();
            harvest.supers.push((file, targets));
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
        source.find(needle).unwrap_or_else(|| panic!("{needle} in source")) as u32
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
        assert!(json.get("counts").is_none(), "counts are a minimal-tier field");

        std::fs::remove_dir_all(&root).ok();
    }
}

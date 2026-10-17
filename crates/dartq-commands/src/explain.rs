//! The explain face's core: the AI's one-stop symbol report —
//! the commands wiring of the [`dartq_ai::explain`] assembler.
//!
//! The report answers from the engine's maintained index state after the
//! one discover+analyze prelude — no second analysis pass runs. The
//! declaration hits read the per-file captures
//! ([`dartq_engine::Engine::file_capture`]), the reference and supertype
//! rows the per-library reference surfaces
//! ([`dartq_engine::Engine::library_surfaces`]) — the same maintained
//! state `query_symbols` serves, at raw-row fidelity so the assembler's
//! container anchors resolve in place; the constructor rows and the
//! signature faces read the resident element facts
//! ([`dartq_engine::Engine::file_facts`], computed by the same apply
//! phase as the captures). The rows' `StringId`s are native
//! to the engine's intern table, which the answer resolves through. The
//! transient per-answer maps (hit grouping, supertype edges, constructor
//! render) are symbol-scoped and dropped at the end of the call.
//!
//! The reverse import graph and the test graph build once per call from
//! the engine's import CSR — the same per-query posture `impact` keeps
//! over the same CSR. The semantic fold plays no role in the report, so
//! the face skips it (`semdiag = None`); the collectors this face retired
//! skipped it the same way.
//!
//! The JSON answer is the assembler's serialized report routed through
//! `dartq-schema`'s `validate_at_tier(payload, Domain::Explain, tier)`
//! where the payload is born — the producing tier rides the gate, so a
//! wrong-tier section (the minimal-only `counts` at compact, a section
//! array at minimal) is a producer bug surfaced as the face's
//! internal-tier error — never clamped, never a dead process.

use std::collections::BTreeMap;
use std::io;
use std::path::Path;

use dartq_ai::explain::{self as query, ConstructorFact, SymbolDeclHit};
use dartq_ai::rev_import_graph::RevImportGraph;
use dartq_engine::Engine;
use dartq_index::{RefRow, SuperClause, TargetRef};
use dartq_source::{FileId, StringId};
use serde_json::Value;
use thiserror::Error;

use crate::Verbosity;
use crate::analyze::{AnalyzeError, prelude};
use crate::impact::impact_substrate;
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
    /// Discovery failure or cancellation, plus the report's own internal
    /// tier: a born report failing its published schema rides this variant
    /// (a producer bug — internal tier, never a usage error).
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
/// [`ExplainError`] on workspace-assembly, engine-bind, discovery,
/// cancellation failure, or a born report failing its published schema
/// (the internal tier — the producer-bug face).
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
    explain_answer(&engine, symbol, verbosity)
}

/// The report over an already-analyzed engine: the adapter over the
/// maintained index state (transient, symbol-scoped — dropped at the end
/// of the call) feeding the assembler.
///
/// # Errors
///
/// A born report failing its published schema is the internal-tier
/// [`ExplainError`] — the violation surfaces as the error line (the
/// daemon's `Internal` answer, the CLI's exit 4), never a clamped report
/// and never a dead process.
///
/// # Panics
///
/// Panics on engine-contract violations, surfaced never clamped: a live
/// library member without a capture, or an answer file id missing from
/// the engine's file table, is a program bug.
fn explain_answer(
    engine: &Engine,
    symbol: &str,
    verbosity: Verbosity,
) -> Result<Value, ExplainError> {
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
                hits.extend(rows.iter().map(|decl| SymbolDeclHit {
                    library,
                    file,
                    decl,
                }));
                hit_files.entry(library).or_default().push(file);
            }
        }
    }

    // The reference rows naming the symbol, the hit files' resolved
    // supertype edges (clause intact — the interfaces/inheritance split
    // reads it), and the hit files' constructor facts (rendered here from
    // the resident element face; the assembler stays semantic-free) — one
    // pass over the maintained state, all transient maps symbol-scoped.
    let mut refs: Vec<RefRow> = Vec::new();
    let mut supers: BTreeMap<FileId, Vec<(TargetRef, SuperClause)>> = BTreeMap::new();
    let mut ctors: BTreeMap<FileId, Vec<ConstructorFact>> = BTreeMap::new();
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
                supers.insert(
                    file,
                    surface
                        .supertypes_of(file, sym)
                        .map_or(Vec::new(), |edges| {
                            edges
                                .iter()
                                .map(|edge| (edge.supertype, edge.clause))
                                .collect()
                        }),
                );
                let facts = constructor_facts(engine, file, sym);
                if !facts.is_empty() {
                    ctors.insert(file, facts);
                }
            }
        }
    }

    // The graph faces over the engine's import CSR — the shared substrate
    // `impact` reads (one build each per call, #558).
    let substrate = impact_substrate(engine, &libraries);
    let count = substrate.node_count();
    let importers = RevImportGraph::build(count, |library| substrate.neighbors(library));

    // Stable reference labels for every file id the answer carries — the
    // package-form projection (the shared [`crate::pkg_label::pkg_label`]
    // rule; internal ids never leave the engine boundary).
    let file_table = engine.file_table();
    let mut ids: Vec<FileId> = Vec::new();
    ids.extend(hits.iter().map(|hit| hit.file));
    ids.extend(refs.iter().map(|row| row.file));
    ids.extend(supers.values().flat_map(|edges| {
        edges.iter().filter_map(|(target, _)| match target {
            TargetRef::Decl(file, _) => Some(*file),
            TargetRef::Name(_) => None,
        })
    }));
    ids.sort_unstable();
    ids.dedup();
    // Projected once per answer file here (the attribution walk amortizes
    // over the package table's per-directory memo); non-UTF-8 paths label
    // lossily — the JSON string columns cannot carry them raw either way.
    let packages = engine.packages();
    let file_labels: Vec<String> = ids
        .iter()
        .map(|&file| {
            crate::pkg_label::pkg_label(packages, file_table.path(file).expect("answer file"))
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
        |file, _| supers.get(&file).map_or(&[][..], |edges| edges.as_slice()),
        |file, _| ctors.get(&file).map_or(&[][..], |rows| rows.as_slice()),
        &importers,
        |library| substrate.label(library),
        |file| {
            let index = ids.binary_search(&file).expect("answer file label");
            file_labels[index].as_str()
        },
        substrate.tests(),
        engine.strings(),
    );
    // One serialization into the answer `Value`, then one serialization of
    // that Value to the validator's &str face (`dartq-schema::validate` is
    // string-typed) — the report is converted exactly twice, never
    // re-parsed. Both conversions are total for this payload (plain
    // derived data: strings, numbers, options, vectors), so the two
    // expects are true invariants; the validator is the real gate.
    let value = serde_json::to_value(&report).expect("explain report serializes");
    let json = serde_json::to_string(&value).expect("explain report serializes");
    // The published-schema gate, tier-exact: the producing tier threads
    // through, so a wrong-tier section (the minimal-only `counts`
    // elsewhere, a section array at minimal) is a violation, not a born
    // report. A violation is a producer bug and rides the internal tier
    // (the daemon answers `SessionError::Internal`, the CLI exits 4
    // through its catch-all arm) — user-visible and recoverable, never a
    // clamped report and never a dead process. The violation text carries
    // the failure; the variant's discovery-side display prefix is the cost
    // of keeping `ExplainError`'s variant set stable for the daemon's
    // exhaustive tier mapping.
    let tier = match verbosity {
        Verbosity::Minimal => dartq_schema::Verbosity::Minimal,
        Verbosity::Compact => dartq_schema::Verbosity::Compact,
        Verbosity::Full => dartq_schema::Verbosity::Full,
    };
    dartq_schema::validate_at_tier(&json, dartq_schema::Domain::Explain, tier).map_err(
        |violation| {
            ExplainError::Analyze(AnalyzeError::Discovery(io::Error::other(format!(
                "explain report violates the published schema: {violation}"
            ))))
        },
    )?;
    Ok(value)
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

/// The container `sym`'s constructor facts rendered from the resident
/// element face: named selectors under `A.named`, the unnamed face under
/// the container's own name (the implicit default — `has_unnamed_-
/// constructor` with no captured formals — renders the empty list). The
/// capture mints no constructor rows, so this face is where the section's
/// rows are born; a non-container declaration answers none.
///
/// # Panics
///
/// Panics when the file is resident without element facts — an internal
/// invariant breach (captures and facts are computed by the same apply
/// phase; impact's resident-facts join leans on the same posture).
fn constructor_facts(engine: &Engine, file: FileId, sym: StringId) -> Vec<ConstructorFact> {
    let elements = &engine
        .file_facts(file)
        .expect("resident member facts")
        .elements;
    let Some(container) = elements.container(sym) else {
        return Vec::new();
    };
    let strings = engine.strings();
    let args = elements.type_args();
    let name = strings.resolve(sym);
    let mut rows = Vec::with_capacity(
        container.constructors.len() + usize::from(container.has_unnamed_constructor),
    );
    for selector_id in &container.constructors {
        let selector = strings.resolve(*selector_id);
        rows.push(ConstructorFact {
            name: format!("{name}.{selector}"),
            signature: format!(
                "{name}.{selector}{}",
                crate::query::render_formals(
                    container.ctor_formals.get(selector_id).map(Box::as_ref),
                    strings,
                    args
                )
            ),
        });
    }
    if container.has_unnamed_constructor {
        rows.push(ConstructorFact {
            name: name.to_owned(),
            signature: format!(
                "{name}{}",
                crate::query::render_formals(
                    container.ctor_formals.get(&sym).map(Box::as_ref),
                    strings,
                    args
                )
            ),
        });
    }
    rows
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::json;

    use super::*;
    use crate::workspace::open_engine;
    use crate::workspace::test_fixture::workspace;

    /// One import chain plus its test: `animal.dart` declares `Animal`
    /// (member `breathe`), the `Loyal` mixin, and the `Pet` interface;
    /// `dog.dart` declares `Dog extends Animal with Loyal implements Pet`
    /// (member `bark`, an unnamed and a named constructor), `main.dart`
    /// imports `dog.dart` and constructs `Dog`, and `test/dog_test.dart`
    /// imports it by package URI.
    const ANIMAL: &str =
        "class Animal {\n  void breathe() {}\n}\nmixin Loyal {}\nabstract class Pet {}\n";
    const DOG: &str = "import 'animal.dart';\nclass Dog extends Animal with Loyal implements Pet {\n  void bark() {}\n  Dog();\n  Dog.named();\n}\n";
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

    /// The retired-slot twin on the explain face (#558): removing a file
    /// retires its library, the arena's slot count rises above the live
    /// count, and the substrate's label table answers anyway — the dead
    /// slot is flood-unreachable (the `Substrate::label` proof). Under the
    /// retired live-count sizing this panicked in `TestGraph::build`.
    #[test]
    fn explain_answers_after_a_library_retires() {
        let root = workspace(
            "explain_retired",
            &[
                ("lib/animal.dart", ANIMAL),
                ("lib/dog.dart", DOG),
                (
                    "test/animal_test.dart",
                    "import 'package:fixture/animal.dart';\nvoid main() {}\n",
                ),
            ],
        );
        let (mut engine, _) = open_engine(&root, Some(&root.join("sdk"))).expect("engine");
        prelude(&mut engine).expect("analyze");

        // Retire dog's library; animal's and the test's stay live. The
        // disk file must leave FIRST — the prelude drives residency from
        // discovery, and a still-present file is re-settled (the pending
        // overlay is keyed by file, so a discovery Set replaces the Remove
        // and the library never retires — the #558 fixture defect).
        std::fs::remove_file(root.join("lib/dog.dart")).expect("delete dog.dart");
        engine
            .remove_file(&root.join("lib/dog.dart"))
            .expect("remove");
        prelude(&mut engine).expect("re-analyze");
        // The state guard: the retirement must actually have happened —
        // live libraries below the arena's slot count is the exact shape
        // the substrate's arena sizing exists for (a silent no-op
        // retirement would fail the dependency assertion vacuously).
        assert_eq!(
            engine.libraries().len(),
            2,
            "animal and the test library stay live"
        );
        assert_eq!(
            engine.library_import_graph().nodes(),
            3,
            "dog's arena slot stays retired-but-counted"
        );

        let json = explain_answer(&engine, "Animal", Verbosity::Compact).expect("explain answer");
        assert_eq!(json["symbol"], "Animal");
        // The dependency rows enumerate the live importers only: dog's
        // library retired, the test library's row survives — the dead
        // slot's name (and its row) are gone, never a mislabel.
        let root = root.canonicalize().expect("canonical root");
        let animal_test = root.join("test/animal_test.dart").display().to_string();
        assert_eq!(
            json["dependencies"],
            json!([{ "path": animal_test }]),
            "the retired importer's row is gone, the live one intact"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// The name token's start offset in `source` (ASCII sources, so byte
    /// offset = UTF-16 offset).
    #[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
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
        // Lib files project to the stable package-form references; the
        // test tree file keeps its canonical path (no `package:` URI
        // addresses it — the documented fallback).
        let root = root.canonicalize().expect("root");
        let animal = "package:fixture/animal.dart".to_owned();
        let dog = "package:fixture/dog.dart".to_owned();
        let main = "package:fixture/main.dart".to_owned();
        let dog_test = root.join("test/dog_test.dart").display().to_string();

        // The entry routes through the published validator at the producing
        // tier; the assertion pins that the answer is born schema-valid.
        let text = serde_json::to_string(&json).expect("serialize");
        dartq_schema::validate_at_tier(
            &text,
            dartq_schema::Domain::Explain,
            dartq_schema::Verbosity::Compact,
        )
        .expect("schema-valid at the producing tier");

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
            json!([{"name": "Animal", "file": animal, "clause": "extends"}])
        );
        assert_eq!(
            json["interfaces"],
            json!([
                {"name": "Loyal", "file": animal, "clause": "with"},
                {"name": "Pet", "file": animal, "clause": "implements"},
            ]),
            "the with/implements edges, each naming its clause"
        );
        assert_eq!(
            json["constructors"],
            json!([
                {"name": "Dog", "kind": "constructor", "signature": "Dog()"},
                {"name": "Dog.named", "kind": "constructor", "signature": "Dog.named()"},
            ]),
            "the unnamed face under the container's own name, the selector under A.named"
        );
        assert_eq!(
            json["methods"],
            json!([{"name": "bark", "kind": "method", "range": [offset(DOG, "bark"), offset(DOG, "bark") + 4]}])
        );
        assert_eq!(
            json["references"],
            json!([
                {"file": dog_test, "kind": "invoked", "range": [offset(TEST, "Dog"), offset(TEST, "Dog") + 3], "qualified": false},
                {"file": main, "kind": "invoked", "range": [offset(MAIN, "Dog"), offset(MAIN, "Dog") + 3], "qualified": false},
            ]),
            "each row names its use site's file, label-projection order (`/` sorts before `p` in `package:` labels)"
        );
        assert_eq!(
            json["dependencies"],
            json!([{"path": dog_test}, {"path": main}]),
            "the libraries that import dog.dart, label-projection order"
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
                "interfaces": 2,
                "constructors": 2,
                "methods": 1,
                "references": 2,
                "dependencies": 2,
                "affected_tests": 1,
            })
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// The birth gate is tier-exact, mutation-probed: a compact answer is
    /// born without `counts`, and the gate it passes through —
    /// `validate_at_tier` with the producing tier — rejects the
    /// minimal-only section a producer bug would inject (the validator
    /// tests' idiom: build the payload, expect the violation, name it).
    #[test]
    fn the_birth_gate_rejects_a_wrong_tier_section() {
        let root = fixture("explain_gate");
        let json = explain(&root, Some(&root.join("sdk")), "Dog", Verbosity::Compact)
            .expect("explain run");
        let mut mutated = json;
        assert!(
            mutated.get("counts").is_none(),
            "compact carries no counts section"
        );
        mutated["counts"] = json!({
            "inheritance": 1,
            "interfaces": 2,
            "constructors": 2,
            "methods": 1,
            "references": 2,
            "dependencies": 2,
            "affected_tests": 1,
        });
        let text = serde_json::to_string(&mutated).expect("serialize");
        let violation = dartq_schema::validate_at_tier(
            &text,
            dartq_schema::Domain::Explain,
            dartq_schema::Verbosity::Compact,
        )
        .expect_err("counts at compact violate the producing tier");
        assert!(
            violation.path.contains("counts"),
            "the minimal-only section is named: {violation}"
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
        assert_eq!(json["interfaces"], json!([]));
        assert_eq!(json["constructors"], json!([]));
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

    /// The constructor rows carry the captured formal face: required
    /// positional bare, the named group in braces with the `required`
    /// keyword, the implicit-default face rendered as the empty list.
    #[test]
    fn constructor_rows_carry_the_formal_face() {
        const CAT: &str =
            "class Cat {\n  Cat(String name, {int? lives});\n  Cat.kitten() : this('mew');\n}\n";
        let root = workspace("explain_ctor_formals", &[("lib/cat.dart", CAT)]);
        let json = explain(&root, Some(&root.join("sdk")), "Cat", Verbosity::Compact)
            .expect("explain run");
        assert_eq!(
            json["constructors"],
            json!([
                {"name": "Cat", "kind": "constructor", "signature": "Cat(String name, {int? lives})"},
                {"name": "Cat.kitten", "kind": "constructor", "signature": "Cat.kitten()"},
            ]),
            "the unnamed face's captured formals, the redirect's empty list"
        );

        std::fs::remove_dir_all(&root).ok();
    }
}

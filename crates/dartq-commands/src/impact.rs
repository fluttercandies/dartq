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
//!   over the member hashes, the blake3-128 fold of the member API
//!   signatures (dartq-ai's [`dartq_ai::impact::fold_api_signatures`]), the
//!   library-face declaration names. Per member the persisted-facts hint
//!   decides the read face: a snapshot hit is `materialize`d through the
//!   #341 load faces; a genuine miss reads the engine's resident facts
//!   through the #341 accessor face ([`dartq_engine::Engine::file_facts`])
//!   — the analysis prelude has already computed them, so no member is
//!   ever re-scanned (the retired scan+capture seed also carried a
//!   language-version drift: it re-fixed the default version where the
//!   engine settles each file's own).
//!
//! One-shot posture: a CLI run holds no baseline, so the seed enters the
//! query as a **hypothetical** API-level change ([`LibraryChange::hypothetical`])
//! — the report answers "what depends on this file's API surface". The
//! daemon passes real before/after diffs through the same query.
//!
//! [`LibraryChange::hypothetical`]: dartq_ai::impact::LibraryChange::hypothetical

use std::path::{Path, PathBuf};

use dartq_ai::impact::{self as query, FileSnapshot, LibraryChange, content_hash};
use dartq_diagnostics::CodeRegistry;
use dartq_engine::state::persist::PersistedFacts;
use dartq_engine::{CsrGraph, Engine, LibraryId, UnitFacts};
use dartq_index::TestGraph;
use dartq_semantic::MicroKind;
use dartq_source::{FileId, FileTable, StringId, StringTable};
use thiserror::Error;

use crate::analyze::AnalyzeError;

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
/// task has just run the analysis pass. Hint-less: every member seeds. A
/// caller holding a snapshot generation threads it through
/// [`impact_after_analysis_with_persisted_facts`].
///
/// # Errors
///
/// [`ImpactError::Missing`] / [`ImpactError::NotAnalyzed`] on a bad target.
pub fn impact_after_analysis(
    engine: &Engine,
    target: &Path,
) -> Result<query::ImpactReport, ImpactError> {
    impact_after_analysis_with_persisted_facts(engine, target, &unpersisted)
}

/// The persisted-facts hint the query consults before seeding (the #341
/// load faces): called once per member file with the file's resident
/// source, it answers `Some` when the snapshot generation carries the
/// file's facts under its current content key and `None` on a genuine miss
/// — first run, generation mismatch, tables-stamp mismatch, eviction. The
/// accelerator invariant: a miss is a recompute (the seed arm), never an
/// error.
pub type PersistedFactsHint<'a> = &'a dyn Fn(&str) -> Option<PersistedFacts>;

/// The hint-less posture.
fn unpersisted(_: &str) -> Option<PersistedFacts> {
    None
}

/// [`impact_after_analysis`] over a persisted-facts hint: members the
/// snapshot carries surface through [`PersistedFacts::materialize`], the
/// rest seed exactly as before — the outputs are byte-identical either way
/// for the same content (the impact equivalence test).
///
/// # Errors
///
/// [`ImpactError::Missing`] / [`ImpactError::NotAnalyzed`] on a bad target.
pub fn impact_after_analysis_with_persisted_facts(
    engine: &Engine,
    target: &Path,
    persisted: PersistedFactsHint<'_>,
) -> Result<query::ImpactReport, ImpactError> {
    // The seed: the target file's owning library.
    let canonical = target
        .canonicalize()
        .map_err(|_| ImpactError::Missing(target.to_path_buf()))?;
    let Some(seed_file) = engine.file_table().try_file_id(&canonical) else {
        return Err(ImpactError::NotAnalyzed(canonical));
    };
    let Some(seed_library) = engine.library_of(seed_file) else {
        return Err(ImpactError::NotAnalyzed(canonical));
    };

    // Library identities and the seed's member list: one `libraries` fetch
    // feeds both the member lookup and the substrate's labels. Borrowed,
    // not cloned: the list only feeds the immutable surface fold below,
    // and `libraries` outlives every use in this function.
    let libraries = engine.libraries();
    let seed_library_index = seed_library.index() as usize;
    let members = libraries
        .iter()
        .find(|(library, _)| library.index() as usize == seed_library_index)
        .map(|(_, files)| files.as_slice())
        .expect("live seed library");

    // One intern table for the whole answer: the engine's own — resident
    // facts' `StringId`s are native to it, and materializing a payload
    // re-interns into it idempotently (Mutex-guarded `intern`, the
    // documented query posture: the only side effect is interning).
    let strings = engine.strings();
    let mut restored = FileTable::new();
    let current = library_surface(engine, members, strings, persisted, &mut restored);

    // The substrate over the engine's faces, then the query.
    let substrate = impact_substrate(engine, &libraries);
    let change = LibraryChange::hypothetical(seed_library.index(), current);
    Ok(query::impact(
        substrate.node_count(),
        |library| substrate.neighbors(library),
        |library| substrate.label(library),
        &substrate.tests,
        strings,
        &[change],
    ))
}

/// The graph substrate both graph faces read, built once per call (#558):
/// the engine's import CSR projection, the arena-spaced identity labels,
/// and the test graph joined over them.
pub(crate) struct Substrate<'a> {
    csr: CsrGraph,
    /// The defining-file label of every arena slot, `None` on a dead slot —
    /// sized by [`CsrGraph::nodes`] (the arena's slot count), never by the
    /// live-library count: the consumers iterate the CSR's node space,
    /// which includes retired slots, so a live-count-sized table indexes
    /// out of bounds on the first retirement (#558 P1).
    idents: Vec<Option<&'a str>>,
    tests: TestGraph,
}

/// Assembles the substrate over `engine`'s faces and the already-fetched
/// `libraries` (the faces pass the same fetch they need for their own
/// member lookups — no second `Engine::libraries` build).
///
/// The labels borrow the engine's file table (`&str`, no per-library
/// allocation); a path that is not UTF-8 labels empty — it cannot ride the
/// JSON faces' string columns anyway, and the retired lossy spelling's
/// replacement characters surfaced no better.
pub(crate) fn impact_substrate<'a>(
    engine: &'a Engine,
    libraries: &[(LibraryId, Vec<FileId>)],
) -> Substrate<'a> {
    let csr = engine.library_import_graph();
    let count = csr.nodes();
    let mut idents: Vec<Option<&'a str>> = vec![None; count];
    for (library, files) in libraries {
        if let Some(label) = files
            .first()
            .and_then(|&file| engine.file_table().path(file))
            .and_then(std::path::Path::to_str)
        {
            idents[library.index() as usize] = Some(label);
        }
    }
    let neighbors = |library: u32| -> &[u32] {
        let start = csr.offsets()[library as usize] as usize;
        let end = csr.offsets()[library as usize + 1] as usize;
        &csr.edges()[start..end]
    };
    // Dead slots carry no identity and gain no test edges — the convention
    // fallback reads the identity path, and `None` skips the slot.
    let tests = TestGraph::build(count, neighbors, |library| idents[library as usize]);
    Substrate { csr, idents, tests }
}

impl<'a> Substrate<'a> {
    /// The node count: the engine library arena's slot count, dead slots
    /// included (the CSR's node space).
    #[must_use]
    pub(crate) fn node_count(&self) -> usize {
        self.csr.nodes()
    }

    /// The resolved import targets of `library` (library ids, not slots).
    #[must_use]
    pub(crate) fn neighbors(&self, library: u32) -> &[u32] {
        let start = self.csr.offsets()[library as usize] as usize;
        let end = self.csr.offsets()[library as usize + 1] as usize;
        &self.csr.edges()[start..end]
    }

    /// The defining-file label of `library`.
    ///
    /// Dead slots never reach here — the label consumers are flood-joined:
    /// a retired slot projects no edges, so it appears in no importer row
    /// (`RevImportGraph` inverts `neighbors`, and nothing lists a slot with
    /// an empty row as an importer), builds no test edge (`TestGraph::build`
    /// skips identity-less slots), and holds no hit (hits enumerate the live
    /// libraries). A label demand on a dead slot is a caller bug and
    /// surfaces as one.
    #[must_use]
    pub(crate) fn label(&self, library: u32) -> &'a str {
        self.idents[library as usize].expect("live library")
    }

    /// The joined test graph (the `AffectedTests` join's read side).
    #[must_use]
    pub(crate) fn tests(&self) -> &TestGraph {
        &self.tests
    }
}

/// The folded surface of one library over its member files: blake3 over the
/// member content hashes, the blake3-128 fold of the member API signatures,
/// the library-face declaration names. Per member the persisted hint decides
/// the read face: a hit `materialize`s the payload through the #341 load
/// faces; a miss reads the engine's resident facts through the #341
/// accessor face — analysis has already computed them, so nothing is
/// re-derived (parse-surface diagnostics play no role: this face produces
/// no diagnostics of its own).
///
/// The equivalence the fold stands on: both arms answer for the same
/// resident bytes — the content identity is the same blake3 over the same
/// bytes, the face sets coincide (the materialized element model and the
/// engine's residency are equal for equal content), and the signature
/// columns are the same baked digest — so a generation whose entries match
/// the workspace folds into the seed's exact snapshot. The `restored` table
/// takes the payload's file references (directive targets, diagnostic
/// files); the surface face reads none of them, but `materialize` mints ids
/// through it by contract.
fn library_surface(
    engine: &Engine,
    members: &[FileId],
    strings: &StringTable,
    persisted: PersistedFactsHint<'_>,
    restored: &mut FileTable,
) -> FileSnapshot {
    /// The library-face names of one unit's element model: every named
    /// container plus every non-type-parameter top-level declaration,
    /// setters under their `name=` spelling — the capture filter's
    /// mirror, applying the same `MicroKind::TypeParam` exclusion the
    /// engine's library graph does.
    fn face_names(facts: &UnitFacts, names: &mut Vec<StringId>) {
        names.extend(facts.elements.containers().map(|(name, _)| name));
        names.extend(
            facts
                .elements
                .declarations()
                .filter(|(_, decl)| decl.kind != MicroKind::TypeParam)
                .map(|(name, _)| name),
        );
    }

    let mut names: Vec<StringId> = Vec::new();
    let mut folded = Vec::with_capacity(members.len() * 32);
    let mut signatures = Vec::with_capacity(members.len());
    // The builtin diagnostic-code registry, built once per load (the #428
    // contract): `materialize` consults it per restored diagnostic, and a
    // per-member rebuild would reconstruct the ~270-entry map for every
    // snapshot hit.
    let codes = CodeRegistry::builtin();
    for &file in members {
        let bytes = engine.content(file).expect("resident member");
        // The content identity rides the query contract's 32-byte blake3
        // over the resident bytes — the same digest either arm answers for;
        // the payload's 16-byte column is the entry key's domain, not the
        // fold's.
        folded.extend_from_slice(&content_hash(bytes));
        let text = std::str::from_utf8(bytes).expect("engine content is UTF-8");
        let signature = if let Some(payload) = persisted(text) {
            // The signature column off the payload bytes before the
            // materializing move (`materialize` consumes the payload).
            let signature = payload.api_signature;
            let facts =
                payload.materialize(text, strings, &codes, &mut |path| restored.intern(path));
            face_names(&facts, &mut names);
            signature
        } else {
            // The engine's residency is the authority here: the
            // analysis prelude computed these facts, so a snapshot miss
            // costs one map read — no rescan, no re-intern (the names'
            // `StringId`s are native to the query's table — the
            // engine's own), and no language-version drift (the file's
            // settled feature set rode the analysis).
            let facts = engine.file_facts(file).expect("resident member facts");
            face_names(facts, &mut names);
            *facts.api_signature().as_bytes()
        };
        signatures.push(signature);
    }
    names.sort_unstable();
    names.dedup();
    FileSnapshot {
        content_hash: content_hash(&folded),
        api_signature: query::fold_api_signatures(&signatures),
        top_level_names: names.into_boxed_slice(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::{open_engine, test_fixture::workspace};
    use dartq_index::{DeclKind, capture};
    use dartq_language::{DEFAULT_LANGUAGE_VERSION, FeatureSet};
    use dartq_parser::ParserFeatures;
    use dartq_scanner::{ScanConfig, scan};
    use rustc_hash::FxHashMap;
    use serde_json::json;
    use std::fs;

    /// A retired library slot is invisible to the report (#558): removing a
    /// file retires its library at the next apply, so the arena's slot
    /// count rises above the live count — the substrate's label table is
    /// arena-sized, and the dead slot holds no edges, no test identity, and
    /// no hit. Under the retired live-count sizing this test panicked
    /// (index out of bounds in `TestGraph::build`, the first consumer that
    /// walks the full node space).
    #[test]
    fn retired_library_slots_are_invisible_to_the_report() {
        let root = workspace(
            "impact_retired",
            &[
                ("lib/a.dart", "class A {}\n"),
                ("lib/b.dart", "import 'a.dart';\nclass B {}\n"),
                ("test/a_test.dart", "import 'a.dart';\nvoid main() {}\n"),
            ],
        );
        let (mut engine, _) = open_engine(&root, Some(&root.join("sdk"))).expect("engine");
        let target = root.join("lib/a.dart");
        let baseline = impact(&mut engine, &target).expect("seeded impact");
        assert!(
            baseline
                .affected
                .iter()
                .any(|row| row.path.ends_with("b.dart")),
            "b imports a: {:?}",
            baseline.affected
        );

        // The removal retires b's library when `impact`'s prelude applies:
        // the disk file must leave FIRST — the prelude drives residency
        // from discovery, and a still-present file is re-settled (the
        // pending overlay is keyed by file, so a discovery Set replaces
        // the Remove and the library never retires — the #558 fixture
        // defect). With the file gone, the pending Remove survives the
        // apply.
        std::fs::remove_file(root.join("lib/b.dart")).expect("delete b.dart");
        engine
            .remove_file(&root.join("lib/b.dart"))
            .expect("remove");
        let after = impact(&mut engine, &target).expect("post-retirement impact");
        // The state guard: the retirement must actually have happened —
        // live libraries below the arena's slot count is the exact shape
        // the substrate's arena sizing exists for. Without this, a silent
        // no-op retirement passes the row assertions vacuously.
        assert_eq!(
            engine.libraries().len(),
            2,
            "a and the test library stay live"
        );
        assert_eq!(
            engine.library_import_graph().nodes(),
            3,
            "b's arena slot stays retired-but-counted"
        );
        assert!(
            !after
                .affected
                .iter()
                .any(|row| row.path.ends_with("b.dart")),
            "the retired library is gone: {:?}",
            after.affected
        );
        assert!(
            after
                .affected_tests
                .iter()
                .any(|row| row.path.ends_with("a_test.dart")),
            "the live test edge survives: {:?}",
            after.affected_tests
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// The scan configuration of every non-version-gated capture face (the
    /// workspace default: triple shift and augmentations on) — the payload
    /// projection's capture pass.
    const SCAN_CONFIG: ScanConfig = ScanConfig {
        enable_triple_shift: true,
        enable_augmentations: true,
    };

    /// Projects one file's facts onto the persisted payload the saver
    /// writes. The engine's resident facts are crate-private, so the
    /// projection re-derives the face rows with the same capture pass
    /// analysis uses: every library-face row becomes an element column
    /// entry (containers vs top-level declarations, names on the per-entry
    /// string slice, kinds in the persisted spellings) — the shape the load
    /// arm's derivation consumes. The signature column is a fixture
    /// simplification (all-zero): it rides the payload but never reaches
    /// the report, whose verdicts key on the name sets.
    fn persisted_facts(text: &str, file: FileId, strings: &StringTable) -> PersistedFacts {
        let scanned = scan(text, SCAN_CONFIG, file);
        let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
            DEFAULT_LANGUAGE_VERSION,
        ));
        let captured = capture(file, text, &scanned.tokens, features, strings);
        let mut slice: Vec<String> = Vec::new();
        let mut slots: FxHashMap<String, u32> = FxHashMap::default();
        let top_decl = |kind: &str| {
            json!({
                "kind": kind,
                "type_params": 0,
                "var_ty": "Unknown",
                "return_ty": "Unknown",
                "formals": null,
            })
        };
        let container = |kind: &str| {
            json!({
                "kind": kind,
                "superclass": null,
                "members": [],
                "type_params": 0,
                "sealed": false,
                "is_abstract": false,
                "with_clause": [],
                "implements_clause": [],
                "on_clause": [],
                "header_residue": [],
                "primary_formals": [],
                "enum_constants": [],
                "constructors": [],
                "has_unnamed_constructor": true,
            })
        };
        let mut decls: Vec<serde_json::Value> = Vec::new();
        let mut containers: Vec<serde_json::Value> = Vec::new();
        let mut order: Vec<u32> = Vec::new();
        for row in captured.index.decls() {
            if !row.is_library_face() {
                continue;
            }
            let name = strings.resolve(row.name).to_owned();
            let at = match slots.get(&name) {
                Some(&at) => at,
                None => {
                    slots.insert(name.clone(), slice.len() as u32);
                    slice.push(name);
                    (slice.len() - 1) as u32
                }
            };
            match row.kind {
                DeclKind::Class => {
                    order.push(at);
                    containers.push(json!([at, container("Class")]));
                }
                DeclKind::Enum => {
                    order.push(at);
                    containers.push(json!([at, container("Enum")]));
                }
                DeclKind::Mixin => {
                    order.push(at);
                    containers.push(json!([at, container("Mixin")]));
                }
                DeclKind::Extension => {
                    order.push(at);
                    containers.push(json!([at, container("Extension")]));
                }
                DeclKind::ExtensionType => {
                    order.push(at);
                    containers.push(json!([at, container("ExtensionType")]));
                }
                DeclKind::TypeAlias => decls.push(json!([at, top_decl("TypeAlias")])),
                DeclKind::TopLevelFn => decls.push(json!([at, top_decl("TopLevelFn")])),
                DeclKind::TopLevelVar => decls.push(json!([at, top_decl("TopLevelVar")])),
                // Members and type parameters are never library faces; the
                // filter above already skipped them.
                _ => {}
            }
        }
        serde_json::from_value(json!({
            "api_signature": vec![0u8; 16],
            "directives": [],
            "elements": {
                "strings": slice,
                "decls": decls,
                "containers": containers,
                "container_order": order,
                "extension_faces": [],
                "duplicates": [],
                "arity_violations": [],
            },
            "diagnostics": [],
            "files": [],
        }))
        .expect("saver-shape payload decodes")
    }

    /// The equivalence contract of the persisted surface: entries carried
    /// by the snapshot answer for their files through `materialize`, and
    /// the report over the loaded facts is byte-identical to the seeded
    /// baseline for the same content. The fixture spans every face kind
    /// plus a generic top-level function, whose type parameter must stay
    /// out of both derivations, and a part file folded into the seed
    /// library's surface.
    #[test]
    fn loaded_facts_reproduce_the_seeded_report() {
        let root = workspace(
            "impact_persisted",
            &[
                (
                    "lib/main.dart",
                    "part 'extra.dart';\n\n class Greeter { String hello() => 'hi'; }\n\
                     enum Color { red, green }\n mixin Runner {}\n\
                     extension GreeterRun on Greeter { void wave() {} }\n\
                     extension type Meters(int value) {}\n typedef Name = String;\n\
                     int counter = 0;\n String get greeting => 'hi';\n\
                     set greeting(String value) {}\n void greet<T>(T thing) {}\n",
                ),
                (
                    "lib/extra.dart",
                    "part of 'main.dart';\n\n void helper(int x) {}\n double ratio = 1.0;\n",
                ),
                (
                    "lib/other.dart",
                    "import 'main.dart';\n\n void use() { Greeter().hello(); }\n",
                ),
                ("test/main_test.dart", "void main() {}\n"),
            ],
        );
        let (mut engine, _) = open_engine(&root, Some(&root.join("sdk"))).expect("engine");
        let target = root.join("lib/main.dart");
        let seeded = impact(&mut engine, &target).expect("seeded impact");

        // The save side: one payload per workspace file, keyed by the exact
        // content the store's entry key pins.
        let strings = StringTable::new();
        let mut persisted: FxHashMap<String, PersistedFacts> = FxHashMap::default();
        for relative in [
            "lib/main.dart",
            "lib/extra.dart",
            "lib/other.dart",
            "test/main_test.dart",
        ] {
            let path = root.join(relative);
            let text = fs::read_to_string(&path).expect("fixture file");
            let file = engine.file_table().try_file_id(&path).expect("discovered");
            persisted.insert(text.clone(), persisted_facts(&text, file, &strings));
        }

        let loaded = impact_after_analysis_with_persisted_facts(&engine, &target, &|text: &str| {
            persisted.get(text).cloned()
        })
        .expect("loaded impact");

        // The seed members (main.dart and its part) both hit; the report
        // bytes are the seeded baseline's.
        assert_eq!(
            serde_json::to_string(&loaded).expect("serialize"),
            serde_json::to_string(&seeded).expect("serialize"),
            "the loaded report is the seeded report",
        );
        // The comparison is not vacuous: the hypothetical change lists the
        // library's face names, and the type parameter is in neither
        // derivation's set.
        let names: Vec<&str> = loaded
            .changed
            .iter()
            .map(|row| row.symbol.as_str())
            .collect();
        assert!(
            names.iter().any(|row| row.ends_with("|Greeter")),
            "face rows: {names:?}"
        );
        assert!(
            names.iter().any(|row| row.ends_with("|greeting=")),
            "the setter's lookup spelling rides: {names:?}"
        );
        assert!(
            names.iter().any(|row| row.ends_with("|helper")),
            "the part file folds into the surface: {names:?}"
        );
        assert!(
            !names.iter().any(|row| row.ends_with("|T")),
            "the type parameter is not a library face: {names:?}"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// The fallback tier: a member the snapshot does not carry seeds exactly
    /// as before. With no hint at all — and with entries present for
    /// non-member files only — the report stays byte-identical to the
    /// seed: the per-entry decision consults the seed library's members
    /// alone, and a miss is a recompute, never an error.
    #[test]
    fn absent_entries_fall_back_to_seeding_per_member() {
        let root = workspace(
            "impact_absent",
            &[
                (
                    "lib/main.dart",
                    "part 'extra.dart';\n\n class Greeter { String hello() => 'hi'; }\n\
                     void greet<T>(T thing) {}\n",
                ),
                (
                    "lib/extra.dart",
                    "part of 'main.dart';\n\n void helper(int x) {}\n",
                ),
                (
                    "lib/other.dart",
                    "import 'main.dart';\n\n void use() { Greeter().hello(); }\n",
                ),
                ("test/main_test.dart", "void main() {}\n"),
            ],
        );
        let (mut engine, _) = open_engine(&root, Some(&root.join("sdk"))).expect("engine");
        let target = root.join("lib/main.dart");
        let seeded = impact(&mut engine, &target).expect("seeded impact");

        let hintless = impact_after_analysis(&engine, &target).expect("hintless impact");
        assert_eq!(
            serde_json::to_string(&hintless).expect("serialize"),
            serde_json::to_string(&seeded).expect("serialize"),
            "the hint-less posture is the seed",
        );

        // Entries exist — but only for files outside the seed library.
        let strings = StringTable::new();
        let mut persisted: FxHashMap<String, PersistedFacts> = FxHashMap::default();
        for relative in ["lib/other.dart", "test/main_test.dart"] {
            let path = root.join(relative);
            let text = fs::read_to_string(&path).expect("fixture file");
            let file = engine.file_table().try_file_id(&path).expect("discovered");
            persisted.insert(text.clone(), persisted_facts(&text, file, &strings));
        }
        let loaded = impact_after_analysis_with_persisted_facts(&engine, &target, &|text: &str| {
            persisted.get(text).cloned()
        })
        .expect("partially loaded impact");
        assert_eq!(
            serde_json::to_string(&loaded).expect("serialize"),
            serde_json::to_string(&seeded).expect("serialize"),
            "missing seed members seed; foreign entries never perturb",
        );

        std::fs::remove_dir_all(&root).ok();
    }

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

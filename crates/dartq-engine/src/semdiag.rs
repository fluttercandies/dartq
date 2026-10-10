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
//! Each fold indexes every resident file's element model — the whole
//! [`LibraryView::facts`] map, widened from member-files-only by #452 —
//! and the index is alive only for the call. The models move by `Arc`:
//! the facts hold them as `Arc<FileElements>` (#308) and #286's index
//! face stores them shared, so the fold is pure refcount bumps — zero
//! deep clones, however large the residency (the perf charter: the
//! incremental path runs through here per keystroke). The index is
//! rebuilt per fold over residency rather than shared across the fan-out:
//! nothing persists into engine state and nothing is cached per library,
//! so the widening costs nothing when idle. Reach effect: a superclass or
//! container in another resident library now resolves through the
//! namespace hop, so member walks answer honestly instead of
//! suppressing — an unresolvable hop still suppresses (`ChainBroken`),
//! ambiguous imports still suppress, and no emission site's logic
//! changes with this widening.
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

/// The fold's element index: every resident file's model enters as an
/// `Arc` clone of the facts' share — a refcount bump, never a deep clone
/// (#308, consuming #286's shared index face). The whole
/// [`LibraryView::facts`] map is indexed (#452), so member walks resolve
/// through other resident libraries instead of suppressing at the first
/// cross-file hop.
fn build_index(facts: &FxHashMap<FileId, Arc<UnitFacts>>) -> ElementIndex {
    let mut index = ElementIndex::default();
    for (&file, unit) in facts {
        index.insert(file, Arc::clone(&unit.elements));
    }
    index
}

/// The default semantic diagnoser over [`LibraryView`] (stateless; the
/// analyze fan-out shares one instance across workers).
pub struct Diagnoser;

impl SemanticDiagnoser for Diagnoser {
    fn diagnose_library(&self, library: LibraryView<'_>, out: &mut Vec<Diagnostic>) {
        // Every resident file's element models, indexed by refcount bump
        // (full residency — see the module docs for the scope contract).
        let index = build_index(library.facts);

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
    use dartq_package::PackageTable;
    use dartq_pub::{LockFile, PackageConfig, Pubspec, build_graph};
    use dartq_sdk::SdkSnapshot;
    use dartq_source::{FileTable, LineIndex, StringTable, content_hash};
    use std::path::Path;
    use std::sync::atomic::{AtomicU32, Ordering};

    use crate::{CancellationToken, Engine, EngineConfig, memory_store};

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

        let index = build_index(&facts);

        for file in [a, b] {
            let shared = &facts[&file].elements;
            assert!(std::ptr::eq(
                index.get(file).expect("indexed"),
                shared.as_ref()
            ));
            assert_eq!(Arc::strong_count(shared), 2);
        }
    }

    /// Analyzes a two-library single-package workspace through the real
    /// engine (the l1/persist unit-test fixture shape): synthetic SDK, the
    /// real pub/package faces, the default diagnoser on. `main` imports
    /// `other` (a sibling library, not a part), so `main`'s member walk
    /// must consult `other` through the widened residency-wide index.
    /// Returns main's file id plus the merged analyze diagnostics.
    fn analyze_two_libraries(main: &str, other: &str) -> (FileId, Vec<Diagnostic>) {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let root = std::env::temp_dir().join(format!(
            "dartq-engine-semdiag-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        let write = |relative: &str, content: &str| {
            let path = root.join(relative);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
            std::fs::write(path, content).expect("file");
        };
        write(
            "pubspec.yaml",
            "name: t\nversion: 0.1.0\nenvironment:\n  sdk: ^3.5.0\n",
        );
        write(
            "pubspec.lock",
            "packages: {}\nsdks:\n  dart: \">=3.5.0 <4.0.0\"\n",
        );
        write(
            ".dart_tool/package_config.json",
            r#"{"configVersion": 2, "packages": [{"name": "t", "rootUri": "../", "languageVersion": "3.5"}], "generator": "pub"}"#,
        );
        write("sdk/version", "3.13.1\n");
        write(
            "sdk/lib/libraries.json",
            r#"{"vm": {"libraries": {"core": {"uri": "core/core.dart"}}}}"#,
        );
        write(
            "sdk/lib/_internal/allowed_experiments.json",
            r#"{"version": 1, "experimentSets": {"sdkExperiments": []}, "sdk": {"default": {"experimentSet": "sdkExperiments"}}, "packages": {}}"#,
        );
        // Canonicalize before the pub faces resolve it (the #287 spelling
        // universe — macOS /var vs /private/var).
        let root = root.canonicalize().expect("fixture root resolves");
        let config_dir = root.join(".dart_tool");
        let config = Arc::new(
            PackageConfig::parse(
                &std::fs::read_to_string(config_dir.join("package_config.json")).expect("config"),
            )
            .expect("valid config"),
        );
        let lock = LockFile::parse("packages: {}\nsdks:\n  dart: \">=3.5.0 <4.0.0\"\n")
            .expect("valid lock");
        let root_pubspec = Pubspec::parse("name: t\nversion: 0.1.0\nenvironment:\n  sdk: ^3.5.0\n")
            .expect("valid pubspec");
        let graph = build_graph(
            &config,
            &lock,
            &root_pubspec,
            &std::collections::BTreeMap::new(),
            None,
            &config_dir,
        )
        .expect("graph");
        let packages = PackageTable::build(&config, &graph, &config_dir).expect("table");

        let mut engine = Engine::new(EngineConfig {
            store: memory_store(),
            sdk: SdkSnapshot::load(&root.join("sdk"), None).expect("synthetic SDK loads"),
            packages,
            pub_config: Arc::clone(&config),
            config_dir,
            semdiag: Some(diagnoser()),
            target: crate::DEFAULT_TARGET.to_owned(),
            declared_variables: rustc_hash::FxHashMap::default(),
        })
        .expect("engine");
        let main_file = engine.set_content(&root.join("lib/main.dart"), main.as_bytes());
        engine.set_content(&root.join("lib/other.dart"), other.as_bytes());
        let diagnostics = engine
            .analyze(&[], &mut |_| {}, &CancellationToken::default())
            .expect("analyze completes");
        (main_file, diagnostics)
    }

    /// The widened index end-to-end: the superclass lives in another
    /// library of the residency (`main.dart` imports `other.dart`), so the
    /// pre-widening member-files-only index had no entry for it and the
    /// override check suppressed. Through the full-residency index the
    /// hop resolves and `invalid_override` fires, attributed to the
    /// importing file — the M1 under-report posture change #452 buys,
    /// pinned through the real engine fan-out.
    #[test]
    fn cross_library_superclass_emits_end_to_end() {
        let (main_file, diagnostics) = analyze_two_libraries(
            "import 'other.dart';\nclass B extends A { void f() {} }\n",
            "class A { void f(int x) {} }\n",
        );
        let overrides: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.code.name == "invalid_override")
            .collect();
        assert_eq!(overrides.len(), 1, "{overrides:?}");
        assert_eq!(overrides[0].file, main_file);
        assert_eq!(
            overrides[0].message,
            "'B.f' ('void Function()') isn't a valid override of 'A.f' ('void Function(int)')."
        );
    }

    /// The residue contract under the widened index (5d0d1ae): `extends A, B`
    /// latches only `A` as the superclass face; the overflow name `B` is
    /// `header_residue` and is never consulted — even though the widened
    /// index makes `B` resolvable (it lives in the imported library, so
    /// pre-widening it was invisible and the pin was vacuous). The parser's
    /// own `multiple_extends_clauses` syntax row proves the overflow path
    /// actually fired.
    #[test]
    fn extends_overflow_residue_is_never_a_super_face_across_libraries() {
        let (main_file, diagnostics) = analyze_two_libraries(
            "import 'other.dart';\nclass A {}\nclass C extends A, B { void f() {} }\n",
            "class B { void f(int x) {} }\n",
        );
        assert!(
            !diagnostics
                .iter()
                .any(|d| d.code.name == "invalid_override"),
            "{:?}",
            diagnostics.iter().map(|d| &d.code.name).collect::<Vec<_>>()
        );
        assert!(
            diagnostics
                .iter()
                .any(|d| d.file == main_file && d.code.name == "multiple_extends_clauses"),
            "the parser must flag the overflow for the pin to be live"
        );
    }
}

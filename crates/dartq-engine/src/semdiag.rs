//! The engine's default [`SemanticDiagnoser`]: the #120/#121 plug point
//! filled with `dartq_semantic::diagnose_with` (the library's
//! `analyzer.language.strict-*` modes resolved from its analysis options,
//! all-off without any), so the `undefined_*` /
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
//! Each analyze indexes every resident file's element model — the whole
//! [`LibraryView::facts`] map, widened from member-files-only by #452 —
//! once, before the fan-out, and every per-library fold shares it by
//! reference through [`LibraryView::index`] (#775). The models move by
//! `Arc`: the facts hold them as `Arc<FileElements>` (#308) and #286's
//! index face stores them shared, so the build is pure refcount bumps —
//! zero deep clones, however large the residency (the perf charter: the
//! incremental path runs through here per keystroke). The index is built
//! once per analyze over residency rather than per fold: a cold start has
//! every library dirty, and a per-fold rebuild would re-index the whole
//! residency once per library — O(libraries × residency) hash inserts plus
//! a transient residency-sized map per worker. The index content is a pure
//! function of the residency facts (file-keyed entries, by-file lookups),
//! so sharing one build changes no fold's observable behavior. Reach
//! effect: a superclass or
//! container in another resident library now resolves through the
//! namespace hop, so member walks answer honestly instead of
//! suppressing — an unresolvable hop still suppresses (`ChainBroken`),
//! ambiguous imports still suppress, and no emission site's logic
//! changes with this widening.
//!
//! `dartq_semantic::diagnose_with` re-folds the collector's capture-time
//! duplicate log into its report front; the bake in
//! `l1::duplicate_diagnostics` is the single source, so the fold drops the
//! re-issued `duplicate_definition`s and passes everything else through.
//! The report's names rows ([`dartq_semantic::FileReport::names`]) stay
//! unmounted until #122 moves the names table behind dartq-index.
//!
//! # The member-fold memo (#1384)
//!
//! The fold is memoized per member: a member whose identity inputs stood —
//! its own content id, the graph generation, the owning library's cycle
//! fingerprint, the strict modes, and the posture epoch — is served
//! byte-identical from the engine state's semdiag memo, skipping the text
//! decode, the scan-face lookup, and the parse+resolve fold; only members
//! whose faces moved re-fold. The inputs are exactly
//! what `diagnose_unit` reads beyond the unit's own face, and the fold is
//! deliberately NOT treated as monotone (a dependency edit can remove a
//! row as easily as add one), so no dirty-front pruning decides validity —
//! the key proves freshness outright or the fold reruns. The cycle
//! fingerprint is the load-bearing component: it folds every member face
//! and every transitive dependency face, which is precisely the face set
//! the fold observes through the namespaces + element index, and it is the
//! same trust the L2 result-reuse gate already runs on (g485 W2). The
//! posture epoch covers the one input no fingerprint folds — a `// @dart`
//! header edit moves the `class-modifiers` posture under a standing
//! signature. The memo is consumed only where the fold runs (inside the
//! analyze, past the apply), so unlike the reachability memo it carries no
//! applied/stale window: a stale entry just misses its key.

#[cfg(test)]
use std::cell::Cell;
use std::sync::Arc;
use std::sync::Mutex;
#[cfg(test)]
use std::sync::atomic::Ordering;

use dartq_diagnostics::Diagnostic;
use dartq_language::Feature;
use dartq_parser::ParserFeatures;
use dartq_resolution::LibraryClassModifiers;
use dartq_semantic::{
    ElementIndex, ExportUseFace, ImportUseFace, LibraryUsages, StrictModes, UnitUseFace,
};
use dartq_source::{ContentId, FileId, LineIndex, SourceRange, StringId, StringTable};
use rayon::prelude::*;
use rustc_hash::FxHashMap;
use std::sync::PoisonError;

use crate::l1;
use crate::pipeline::ResolvedGraph;
use crate::state::{EngineState, FactsKey, ScannedFace, UnitFacts};
use crate::{LibraryView, SemanticDiagnoser};

// Test-only build counter for the #775 sharing pin: counting invocations
// discriminates one-build-per-analyze from a per-fold rebuild where address
// comparison cannot (a sequential fold that drops its index before the next
// fold can reuse the address and false-pass pointer identity). Thread-local
// because `build_index` runs on the analyze-calling thread while sibling
// tests' builds land on their own threads — no cross-test pollution.
#[cfg(test)]
thread_local! {
    static BUILD_INDEX_CALLS: Cell<usize> = const { Cell::new(0) };
}

/// The `duplicate_definition` code name — the identity both the bake and the
/// re-fold share.
fn duplicate_definition_name() -> &'static str {
    dartq_semantic::codes::SemanticCode::DuplicateDefinition
        .code_info()
        .name
}

/// The `unused_element` code name — the one family the library-wide close
/// re-decides (see [`MemberFold::unused`]).
fn unused_element_name() -> &'static str {
    dartq_semantic::codes::SemanticCode::UnusedElement
        .code_info()
        .name
}

/// The declaration name an `unused_element` row's verdict tested: the
/// written spelling at the row's anchor — the declaration-name token the
/// per-file pass anchored on (a named constructor's rewritten selector, a
/// setter's bare written name) — interned against the shared table so the
/// release test reads the same [`StringId`] the unit use faces recorded.
/// The nameless recovery anchor resolves to whatever text stands there or
/// to nothing; neither is ever a recorded use name, so such a row releases
/// unchanged.
fn staged_decl_name(
    text: &str,
    index: &LineIndex,
    range: SourceRange,
    strings: &StringTable,
) -> StringId {
    let start = index.utf16_to_byte(text, range.offset);
    let end = index.utf16_to_byte(text, range.end());
    strings.intern(text.get(start as usize..end as usize).unwrap_or_default())
}

/// The analyze's element index: every resident file's model enters as an
/// `Arc` clone of the facts' share — a refcount bump, never a deep clone
/// (#308, consuming #286's shared index face). Built once per analyze over
/// the whole [`LibraryView::facts`] map (#452), before the fan-out, and
/// shared by every fold through [`LibraryView::index`] (#775). The content
/// depends only on the residency facts — file-keyed entries, by-file
/// lookups — never on the map's iteration order.
pub(crate) fn build_index(facts: &FxHashMap<FileId, Arc<UnitFacts>>) -> ElementIndex {
    #[cfg(test)]
    BUILD_INDEX_CALLS.with(|calls| calls.set(calls.get() + 1));
    let mut index = ElementIndex::default();
    for (&file, unit) in facts {
        index.insert(file, Arc::clone(&unit.elements));
    }
    index
}

/// The per-library `class-modifiers` posture (#1294): every live engine
/// library's defining file's settled version (the `@dart` override applied)
/// resolved through the feature registry (`class-modifiers`, `enabledIn
/// 3.0.0`). Built once per analyze beside the element index and shared by
/// reference through the whole fan-out; the graph's synthetic `dart:` face
/// nodes have no engine-library mapping (`ResolvedGraph::graph_ids`) and
/// stay absent — [`LibraryClassModifiers`] answers them the modern posture,
/// which is what the platform libraries carry upstream.
pub(crate) fn build_class_modifiers(
    state: &mut EngineState,
    resolved: &ResolvedGraph,
) -> LibraryClassModifiers {
    // The defining files snapshot first: the posture walk needs `&mut`
    // (the cache fill) and `iter_infos` borrows `state` immutably for the
    // whole loop.
    let defining_files: Vec<(dartq_resolution::LibraryId, FileId)> = state
        .libraries
        .iter_infos()
        .filter_map(|(library, info)| {
            resolved
                .graph_ids
                .get(&library)
                .map(|&graph_library| (graph_library, info.defining))
        })
        .collect();
    // The per-file postures are independent reads (a cached face, else a
    // local scan), so they fan out; the cache fills and degrade rows then
    // land serially in the defining-file order — the exact sequence the
    // serial walk produced, so the bounded scanned cache ends identical.
    let resolved: Vec<Posture> = {
        let state = &*state;
        defining_files
            .par_iter()
            .map(|&(_, defining)| class_modifiers_of(state, defining))
            .collect()
    };
    let mut posture = LibraryClassModifiers::default();
    for (&(graph_library, defining), resolved) in defining_files.iter().zip(resolved) {
        let enabled = match resolved {
            Posture::Cached(enabled) => enabled,
            Posture::Scanned(key, face) => {
                let enabled = face.features.is_enabled(Feature::ClassModifiers);
                state.cache_scanned(key, face);
                enabled
            }
            Posture::Degraded(face) => {
                state.residency_degrades.record(&[(defining, face)]);
                true
            }
        };
        posture.insert(graph_library, enabled);
    }
    posture
}

/// `Feature::ClassModifiers` at `file`'s settled version: the persisted
/// scan face when cached, else a local re-derivation the caller persists
/// under the facts key — the same miss shape the fold's
/// own member loop and the apply-phase recompute follow, so a warm-start or
/// evicted face is derived exactly once per analyze and later consumers
/// hit.
fn class_modifiers_of(state: &EngineState, file: FileId) -> Posture {
    if let Some(face) = state.settled_face(file) {
        return Posture::Cached(face.features.is_enabled(Feature::ClassModifiers));
    }
    let content = state.residency[&file].content;
    let key = l1::facts_key(&state.grants(), file, content);
    // The resident content backs the facts (the residency contract) and is
    // lossy-decoded UTF-8 (the store contract) — a pairing miss skips the
    // fold and records a bounded degrade row: the posture answers the
    // modern face (what an absent map entry means) and the next apply
    // re-captures the file.
    let Some(bytes) = state.store.get(content) else {
        return Posture::Degraded("store");
    };
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Posture::Degraded("utf8");
    };
    let version = l1::language_version_for(&state.packages, &state.files, file);
    let (feature_set, settled, scanned, _) = l1::scan_unit(text, version, file, &state.grants());
    Posture::Scanned(
        key,
        ScannedFace {
            tokens: scanned.tokens,
            features: feature_set,
            version: settled,
        },
    )
}

/// One defining file's posture answer, resolved off-thread and applied in
/// order: a cached face's flag, a fresh scan face to persist, or a pairing
/// miss's degrade row (the modern posture).
enum Posture {
    Cached(bool),
    Scanned(FactsKey, ScannedFace),
    Degraded(&'static str),
}

/// The default semantic diagnoser over [`LibraryView`] (stateless; the
/// analyze fan-out shares one instance across workers).
pub struct Diagnoser;

/// The per-library identity inputs of the member fold (#1384): everything
/// `dartq_semantic::diagnose_unit` reads beyond the unit's own face. Two
/// members fold identically iff these stood alongside their content ids —
/// the graph generation pins the graph and its namespaces, the cycle
/// fingerprint pins every member and transitive dependency element face
/// the resolution walk can reach (the same trust the L2 result-reuse gate
/// runs on), the strict modes pin the options face, and the posture epoch
/// pins the settled feature faces the `class-modifiers` posture derives
/// from (an `// @dart` header edit moves those under a standing
/// signature — no per-library fingerprint folds them).
#[derive(Clone, Copy)]
pub(crate) struct FoldInputs {
    pub(crate) generation: u32,
    pub(crate) fingerprint: dartq_semantic::ApiSignature,
    pub(crate) strict: StrictModes,
    pub(crate) posture_epoch: u32,
}

/// One member's memoized fold (#1384): the identity the product was folded
/// under and the product itself — the duplicate-filtered diagnostic rows
/// (the bake in `l1::duplicate_diagnostics` is the single source, and the
/// filter is a pure function of the report) split into the rows that report
/// outright and the `unused_element` candidates, plus the unit use face the
/// substrate closes per library. `names` stays unmounted exactly as the
/// live fold leaves it (#122). The rows are refcounted once here (#1501):
/// the per-library results hold handles to these very rows, so a fold's
/// bytes exist once across the memo, its library's run, and the merged
/// output's clone.
///
/// The `unused_element` split is the part-file verdict face (#2156): the
/// per-file pass fires against its own unit's use set, but the oracle runs
/// the warning pass against the *library-merged* `UsedLocalElements`
/// (library_analyzer.dart:305-315) — a use in any file of the library
/// spares a declaration in every other file. The candidate rows ride the
/// fold with the declaration name their verdict tested, and the library
/// close releases only those the merged set still shows unused.
pub(crate) struct MemberFold {
    inputs: FoldInputs,
    content: ContentId,
    diagnostics: Arc<[Arc<Diagnostic>]>,
    unused: Arc<[(Arc<Diagnostic>, StringId)]>,
    face: Arc<UnitUseFace>,
}

impl MemberFold {
    /// Whether this product answers a fold of `content` under `inputs`:
    /// exact field equality — the fold is not monotone (a dependency edit
    /// removes rows as readily as it adds them), so freshness is proven,
    /// never inferred from a dirty front.
    fn serves(&self, content: ContentId, inputs: &FoldInputs) -> bool {
        self.content == content
            && self.inputs.generation == inputs.generation
            && self.inputs.fingerprint == inputs.fingerprint
            && self.inputs.strict == inputs.strict
            && self.inputs.posture_epoch == inputs.posture_epoch
    }
}

/// The fold map plus the calling library's identity inputs, handed to the
/// diagnoser through [`LibraryView`]. Copyable — one probe/insert pair per
/// member, the guard covers hash operations only (the grants-memo
/// precedent), and the fold itself runs outside it.
#[derive(Clone, Copy)]
pub(crate) struct MemoHandle<'a> {
    pub(crate) memo: &'a Mutex<crate::state::SemdiagMemo>,
    pub(crate) inputs: FoldInputs,
}

impl MemoHandle<'_> {
    /// The member's standing product, when its identity stood.
    fn hit(&self, file: FileId, content: ContentId) -> Option<Arc<MemberFold>> {
        let memo = self.memo.lock().unwrap_or_else(PoisonError::into_inner);
        let fold = memo
            .folds
            .get(&file)
            .filter(|fold| fold.serves(content, &self.inputs))?;
        #[cfg(test)]
        memo.serves.fetch_add(1, Ordering::Relaxed);
        Some(Arc::clone(fold))
    }

    /// Stores the freshly folded product under the member's file.
    fn insert(&self, file: FileId, fold: MemberFold) {
        self.memo
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .folds
            .insert(file, Arc::new(fold));
    }
}

/// Clones a unit use face — the memo serves faces into the substrate by
/// value and `UnitUseFace` is not `Clone` upstream. Kept here because only
/// the memo needs it; a new `UnitUseFace` field breaks this compile-time,
/// so the clone cannot silently drop one.
fn clone_use_face(face: &UnitUseFace) -> UnitUseFace {
    UnitUseFace {
        used: face.used.clone(),
        reads: face.reads.clone(),
        annotations: face.annotations.clone(),
        imports: face
            .imports
            .iter()
            .map(|import| ImportUseFace {
                prefix: import.prefix,
                shown: import.shown.clone(),
            })
            .collect(),
        exports: face
            .exports
            .iter()
            .map(|export| ExportUseFace {
                shown: export.shown.clone(),
            })
            .collect(),
    }
}

impl SemanticDiagnoser for Diagnoser {
    fn diagnose_library(&self, library: LibraryView<'_>, out: &mut Vec<Arc<Diagnostic>>) {
        let duplicate = duplicate_definition_name();
        let unused = unused_element_name();
        // The M3 U1 use-face substrate (#767): every fold emits its unit's
        // face alongside the report, and the library's faces close into the
        // per-library consumed set once the member loop ends — the
        // gather-per-file/merge-per-library pair
        // (library_analyzer.dart:305-315), live in every full analysis.
        // The consumed set decides the `unused_element` verdicts at the
        // close below (#2156): the per-file pass fired them against their
        // own unit's set, and the merged set is what spares a declaration
        // used from a sibling file of the same library (part files).
        let mut usages = LibraryUsages::default();
        // The per-library staging of the family: candidate rows with the
        // declaration names their verdicts tested. Populated by both fold
        // paths (fresh and memo-served — the split is part of the memoized
        // product), released after the close.
        let mut staged: Vec<(Arc<Diagnostic>, StringId)> = Vec::new();
        for &file in library.files {
            let facts = &library.facts[&file];
            // The member-level memo (#1384): a member whose identity inputs
            // stood is served byte-identical — the text decode, the
            // scan-face lookup, and the parse+resolve fold all skip. The
            // use face re-enters the substrate by clone, so the consumed
            // set closes exactly as a re-fold would have closed it.
            if let Some(fold) = library.memo.hit(file, facts.content) {
                out.extend(fold.diagnostics.iter().cloned());
                staged.extend(
                    fold.unused
                        .iter()
                        .map(|(row, name)| (Arc::clone(row), *name)),
                );
                usages.add_unit(library.graph_library, clone_use_face(&fold.face));
                continue;
            }
            // The member's facts' content is resident in the store (the
            // residency contract) and lossy-decoded UTF-8 (the store
            // contract): a pairing miss skips the unit's fold — no
            // diagnostics, no use face — and records a bounded degrade
            // row; the next apply re-captures the file.
            let Some(bytes) = library.store.get(facts.content) else {
                library.degrades.record(&[(file, "store")]);
                continue;
            };
            let Ok(text) = std::str::from_utf8(bytes) else {
                library.degrades.record(&[(file, "utf8")]);
                continue;
            };
            // The persisted scan face; a miss (eviction above the cache cap)
            // re-derives the identical face — the scan the apply recompute
            // would have run.
            let local;
            let (feature_set, tokens) = if let Some(face) = library.scanned.get(&file) {
                (face.features, &face.tokens)
            } else {
                let version = l1::language_version_for(library.packages, library.file_table, file);
                let grants = l1::Grants {
                    tables: library.grants,
                    experiments: &library.sdk.platform_metadata,
                    packages: library.packages,
                    files: library.file_table,
                };
                let (feature_set, _, face, _) = l1::scan_unit(text, version, file, &grants);
                local = face;
                (feature_set, &local.tokens)
            };
            let (report, face) = dartq_semantic::diagnose_unit(
                text,
                tokens,
                ParserFeatures::from_feature_set(feature_set),
                library.strings,
                file,
                library.graph_library,
                library.graph,
                library.namespaces,
                // The analyze-shared element index (#775): every resident
                // file's model, built once per analyze (see the module docs
                // for the scope contract).
                library.index,
                // The library's `analyzer.language.strict-*` face (#588
                // item 4): resolved from the governing analysis options at
                // the view build, all-off without any.
                library.strict,
                // The per-library `class-modifiers` posture (#1294): the
                // analyzed set's defining files' settled versions resolved
                // through the feature registry, once per analyze (see
                // `build_class_modifiers`).
                library.class_modifiers,
                // The field-promotion grant (#1417): the unit's own settled
                // feature set — the `@dart` override applied (`scanned`'s
                // persisted face or the re-derivation above) — so a
                // pre-3.2 library analyzes with promotion off.
                feature_set.is_enabled(Feature::InferenceUpdate2),
            );
            // The bake in `l1::duplicate_diagnostics` is the single source,
            // so the fold drops the re-issued rows; the filtered set is
            // what the memo stores. The `unused_element` candidates split
            // off here (#2156) — the per-file verdict stays the fire
            // decision, the library close below owns the release — and
            // each candidate rides the declaration name its verdict
            // tested. Each row takes its refcount once here (#1501): the
            // run and the merged output hold handles or a single served
            // clone, never fresh bytes.
            let mut live: Vec<Arc<Diagnostic>> = Vec::new();
            let mut candidates: Vec<(Arc<Diagnostic>, StringId)> = Vec::new();
            for diagnostic in report.diagnostics {
                if diagnostic.code.name == duplicate {
                    continue;
                }
                let row = Arc::new(diagnostic);
                if row.code.name == unused {
                    let name = staged_decl_name(
                        text,
                        &facts.line_index,
                        row.range,
                        library.strings,
                    );
                    candidates.push((row, name));
                } else {
                    live.push(row);
                }
            }
            out.extend(live.iter().cloned());
            // The memo owns the rows; the staging holds refcount bumps
            // released at the close below.
            staged.extend(
                candidates
                    .iter()
                    .map(|(row, name)| (Arc::clone(row), *name)),
            );
            usages.add_unit(library.graph_library, clone_use_face(&face));
            library.memo.insert(
                file,
                MemberFold {
                    inputs: library.memo.inputs,
                    content: facts.content,
                    diagnostics: live.into(),
                    unused: candidates.into(),
                    face: Arc::new(face),
                },
            );
        }
        // The library close (#2156): the merged consumed set is what the
        // oracle's warning pass runs against, so the staged candidates
        // release exactly when the library shows them still unused — a use
        // in any member file spares the declaration (the part-file shapes),
        // and a genuinely unused private stays reported. Uses from *other*
        // libraries never enter the set (the close folds this library's
        // faces only), so a same-named private elsewhere cannot spare.
        let consumed = usages.close_library_usage(library.graph_library);
        out.extend(
            staged
                .into_iter()
                .filter_map(|(row, name)| (!consumed.used(name)).then_some(row)),
        );
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
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

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
                doc_imports: Vec::new(),
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

    /// Analyzes a three-library single-package workspace through the real
    /// engine (the l1/persist unit-test fixture shape): synthetic SDK, the
    /// real pub/package faces, `semdiag` wired as the diagnoser. `main`
    /// imports `other` (a sibling library, not a part), so `main`'s member
    /// walk must consult `other` through the residency-wide shared index;
    /// `old` is a third sibling for the cross-library posture tests (its
    /// content carries its own `// @dart` header when one is needed).
    /// Returns main's file id plus the merged analyze diagnostics.
    fn analyze_libraries(
        semdiag: Arc<dyn SemanticDiagnoser>,
        main: &str,
        other: &str,
        old: &str,
    ) -> (FileId, Vec<Diagnostic>) {
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
            semdiag: Some(semdiag),
            target: crate::DEFAULT_TARGET.to_owned(),
            declared_variables: rustc_hash::FxHashMap::default(),
            experiments: Vec::new(),
        })
        .expect("engine");
        let main_file = engine.set_content(&root.join("lib/main.dart"), main.as_bytes());
        engine.set_content(&root.join("lib/other.dart"), other.as_bytes());
        engine.set_content(&root.join("lib/old.dart"), old.as_bytes());
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
        let (main_file, diagnostics) = analyze_libraries(
            diagnoser(),
            "import 'other.dart';\nclass B extends A { void f() {} }\n",
            "class A { void f(int x) {} }\n",
            "class O {}\n",
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
        let (main_file, diagnostics) = analyze_libraries(
            diagnoser(),
            "import 'other.dart';\nclass A {}\nclass C extends A, B { void f() {} }\n",
            "class B { void f(int x) {} }\n",
            "class O {}\n",
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

    /// Counts the folds the fan-out runs, so the sharing pin can assert the
    /// fan-out was live (both libraries folded) against the build count.
    #[derive(Debug)]
    struct FoldProbe(AtomicUsize);

    impl SemanticDiagnoser for FoldProbe {
        fn diagnose_library(&self, _library: LibraryView<'_>, _out: &mut Vec<Arc<Diagnostic>>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// The #775 sharing pin: one `build_index` per analyze, shared by every
    /// per-library fold. Counted, not address-compared — a sequential fold
    /// that drops its index before the next fold can get the address reused
    /// and false-pass pointer equality. A cold two-library analyze must fold
    /// both libraries (the pin is live) and build the index exactly once; a
    /// per-fold rebuild would bump the build count to one per fold.
    /// Behavioral, driven through the real engine — a behavioral pin over an
    /// API-shape assertion because the failure the ticket kills is a runtime
    /// cost shape (rebuild per fold), which only a runtime observation pins.
    #[test]
    fn one_index_shared_across_the_analyze_fan_out() {
        let probe = Arc::new(FoldProbe(AtomicUsize::new(0)));
        BUILD_INDEX_CALLS.with(|calls| calls.set(0));
        let (_, diagnostics) = analyze_libraries(
            Arc::clone(&probe) as Arc<dyn SemanticDiagnoser>,
            "import 'other.dart';\nclass B extends A {}\n",
            "class A {}\n",
            "class O {}\n",
        );
        let folds = probe.0.load(Ordering::Relaxed);
        assert!(
            folds >= 2,
            "both libraries must fold for the pin to be live: {folds} / {diagnostics:?}"
        );
        let builds = BUILD_INDEX_CALLS.with(Cell::get);
        assert_eq!(
            builds, 1,
            "one index build per analyze, not one per fold: {builds} builds for {folds} folds"
        );
    }

    /// The pre-3.0 posture pin (probe P2, guard half): a `// @dart=2.19`
    /// use site does NOT blanket-suppress the outside-library family — the
    /// platform exemption needs an SDK *supertype* library, and a modern
    /// user library's `final class` is never exempt (`_mayIgnoreClassModifiers`,
    /// error_verifier.dart:8471-8472). Guards the posture gate against an
    /// over-broad fix; the oracle reports the same row.
    #[test]
    fn pre_feature_use_site_still_reports_user_final_supers() {
        let (main_file, diagnostics) = analyze_libraries(
            diagnoser(),
            "// @dart=2.19\nimport 'other.dart';\nclass Z extends F {}\n",
            "final class F {}\n",
            "class O {}\n",
        );
        let rows: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.file == main_file && d.code.name == "invalid_use_of_type_outside_library")
            .collect();
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(
            rows[0].message,
            "The class 'F' can't be extended outside of its library because it's a final class."
        );
    }

    /// The posture gate's observable face (#1294): the implements walk's
    /// indirect-supertype skip (error_verifier.dart:5641-5648) reads the
    /// *named* declaration's library posture — a `// @dart=2.19` naming
    /// library (no class modifiers) lets the walk reach the transitive
    /// `final` supertype in the modern third library, so
    /// `final_class_implemented_outside_of_library` fires at the modern use
    /// site. Under the retired hardcoded modern posture the walk skipped
    /// and the row was absent; the oracle reports it.
    #[test]
    fn pre_feature_naming_library_lets_the_implements_walk_reach_final() {
        let (main_file, diagnostics) = analyze_libraries(
            diagnoser(),
            "import 'old.dart';\nclass X implements F {}\n",
            "final class G {}\n",
            "// @dart=2.19\nimport 'other.dart';\nclass F extends G {}\n",
        );
        let rows: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.file == main_file && d.code.name == "invalid_use_of_type_outside_library")
            .collect();
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(
            rows[0].message,
            "The class 'G' can't be implemented outside of its library because it's a final class."
        );
    }

    /// A throwaway single-package workspace for the memo pin: the
    /// synthetic-SDK fixture shape, sources mounted through `set_content`
    /// by the caller (so one root backs the incremental engine and every
    /// per-step wholesale oracle). Returns the canonicalized root.
    fn memo_workspace() -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let root = std::env::temp_dir().join(format!(
            "dartq-engine-semdiag-memo-{}-{}",
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
        root.canonicalize().expect("fixture root resolves")
    }

    /// A fresh default-diagnoser engine over [`memo_workspace`]'s faces —
    /// the wholesale oracle the pin rebuilds per step.
    fn memo_engine(root: &Path) -> Engine {
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
        Engine::new(EngineConfig {
            store: memory_store(),
            sdk: SdkSnapshot::load(&root.join("sdk"), None).expect("synthetic SDK loads"),
            packages: PackageTable::build(&config, &graph, &config_dir).expect("table"),
            pub_config: Arc::clone(&config),
            config_dir,
            semdiag: Some(diagnoser()),
            target: crate::DEFAULT_TARGET.to_owned(),
            declared_variables: rustc_hash::FxHashMap::default(),
            experiments: Vec::new(),
        })
        .expect("engine")
    }

    /// The pin's byte-identity face: every diagnostic field projected
    /// across engines, the file spelled by its path (`FileId`s are
    /// per-engine internings).
    fn canonical_rows(
        engine: &Engine,
        rows: &[Diagnostic],
    ) -> Vec<(String, String, String, String, String, Option<String>)> {
        rows.iter()
            .map(|row| {
                (
                    engine
                        .file_table()
                        .path(row.file)
                        .map(|path| path.display().to_string())
                        .unwrap_or_default(),
                    row.code.name.to_string(),
                    format!("{:?}", row.severity),
                    row.message.to_string(),
                    format!("{:?}", row.range),
                    row.correction.as_deref().map(str::to_owned),
                )
            })
            .collect()
    }

    /// The wholesale check: a fresh cold engine over the same current
    /// contents must produce byte-identical rows to the memo-served
    /// incremental engine (the pre-memo eager fold is a cold engine's
    /// every-member recompute).
    fn assert_matches_wholesale(
        root: &Path,
        engine: &Engine,
        current: &[(String, String)],
        served: &[Diagnostic],
    ) {
        let mut oracle = memo_engine(root);
        for (relative, content) in current {
            oracle.set_content(&root.join(relative), content.as_bytes());
        }
        let expected = oracle
            .analyze(&[], &mut |_| {}, &CancellationToken::default())
            .expect("oracle analyze");
        assert_eq!(
            canonical_rows(engine, served),
            canonical_rows(&oracle, &expected),
            "memo-served output diverged from a wholesale recompute over {current:?}",
        );
    }

    /// The memo serves this session (the pin's liveness evidence).
    fn memo_serves(engine: &Engine) -> usize {
        engine
            .state
            .semdiag
            .lock()
            .expect("semdiag memo mutex")
            .serves
            .load(Ordering::Relaxed)
    }

    /// The salvage pin (#1384), the `salvaged_map_matches_wholesale_over_edits`
    /// shape: the steps sweep the memo's decision paths — the standing
    /// member salvaged beside its re-folded library (step 1), the
    /// generation-forked wholesale rerun (steps 2-4) — and the served output
    /// stays byte-identical to a cold wholesale recompute over the same
    /// contents. Step 5 is the one where keying on (content, graph
    /// generation) alone would be wrong: a dependency's MEMBER-face move
    /// rebuilds neither the graph nor any member's content, but it moves
    /// the owning library's cycle fingerprint and `p1`'s `undefined_method`
    /// row must clear through a fresh fold — the non-monotone direction.
    #[test]
    fn memo_served_folds_match_wholesale_over_edits() {
        let root = memo_workspace();
        let mut engine = memo_engine(&root);
        let main_base = "import 'other.dart';\npart 'p1.dart';\n\nvoid top() {\n  ghost();\n}\n\nclass UsesOther {\n  final Other o;\n  UsesOther(this.o);\n}\n";
        let p1_base = "part of 'main.dart';\n\nint g1(int x) => x;\n";
        let other_base = "class Other {\n  void m() {}\n}\n";
        // Current contents in canonical mount order — the oracle mounts
        // exactly this list, so the two engines see identical workspaces.
        let mut current: Vec<(String, String)> = vec![
            ("lib/main.dart".into(), main_base.into()),
            ("lib/p1.dart".into(), p1_base.into()),
            ("lib/other.dart".into(), other_base.into()),
        ];
        for (relative, content) in &current {
            engine.set_content(&root.join(relative), content.as_bytes());
        }
        let analyze = |engine: &mut Engine| {
            engine
                .analyze(&[], &mut |_| {}, &CancellationToken::default())
                .expect("analyze")
        };

        // 0 — the cold build: every fold fresh, and the pin is live (the
        // undefined `ghost` reports at main).
        let out = analyze(&mut engine);
        assert!(
            out.iter().any(|row| row.code.name == "undefined_function"),
            "{out:?}"
        );
        assert_eq!(memo_serves(&engine), 0, "a cold build serves nothing");
        assert_matches_wholesale(&root, &engine, &current, &out);

        // 1 — comment churn on the defining file: the library recomputes
        // (its content identity moved), the standing part's fold is served
        // byte-identical — the no-op-fold salvage the ticket names. The
        // churn appends after every token: signature v6's member
        // name-token anchor (#1867, the cross-file context-row face) hashes
        // member spans, so a byte-shifting insertion upstream of
        // `UsesOther`'s members (e.g. before `void top`) moves the anchor
        // face, forks the cycle fingerprint, and the part legitimately
        // re-folds — the adjudicated trade that the byte-consistency gate
        // outranks the comment-churn cascade saving. A trailing comment
        // moves no anchor, so the fingerprint — and with it the part's
        // memo key — stands.
        current[0].1 = format!("{main_base}// churn\n");
        engine.set_content(&root.join("lib/main.dart"), current[0].1.as_bytes());
        let out = analyze(&mut engine);
        assert_eq!(
            memo_serves(&engine),
            1,
            "exactly the standing part is salvaged"
        );
        assert_matches_wholesale(&root, &engine, &current, &out);

        // 2 — member edit: p1's bytes move, its fold reruns and reports the
        // unresolved `n` (`Other.n()` is an invocation, the static face's
        // `undefined_method` shape). The edit adds a top-level declaration,
        // so the structural face moves and the graph generation forks every
        // memo key — nothing is salvaged, the same wholesale posture step 3
        // takes.
        current[1].1 = format!("{p1_base}\nint h2() => Other.n();\n");
        engine.set_content(&root.join("lib/p1.dart"), current[1].1.as_bytes());
        let out = analyze(&mut engine);
        assert!(
            out.iter().any(|row| row.code.name == "undefined_method"),
            "{out:?}"
        );
        assert_eq!(
            memo_serves(&engine),
            1,
            "the generation fork salvages nothing — the tally holds step 1's \
             standing-part serve"
        );
        assert_matches_wholesale(&root, &engine, &current, &out);

        // 3 — member add: the membership move is structural, the graph
        // generation forks every key, and the fold reruns wholesale.
        current[0].1 = current[0]
            .1
            .replace("part 'p1.dart';", "part 'p1.dart';\npart 'p2.dart';");
        current.push((
            "lib/p2.dart".into(),
            "part of 'main.dart';\n\nvoid fromP2() {}\n".into(),
        ));
        engine.set_content(&root.join("lib/main.dart"), current[0].1.as_bytes());
        engine.set_content(&root.join("lib/p2.dart"), current[3].1.as_bytes());
        let out = analyze(&mut engine);
        assert_eq!(
            memo_serves(&engine),
            1,
            "a structural move salvages nothing"
        );
        assert_matches_wholesale(&root, &engine, &current, &out);

        // 4 — member remove: p2 leaves with its part line, same wholesale
        // posture.
        current[0].1 = current[0].1.replace("\npart 'p2.dart';", "");
        current.pop();
        engine.set_content(&root.join("lib/main.dart"), current[0].1.as_bytes());
        engine
            .remove_file(&root.join("lib/p2.dart"))
            .expect("remove");
        let out = analyze(&mut engine);
        assert_eq!(memo_serves(&engine), 1);
        assert_matches_wholesale(&root, &engine, &current, &out);

        // 5 — the decisive non-monotone case: a dependency's member-face
        // move (`n` comes into existence) rebuilds neither the graph nor
        // any member's content, but it moves main's library cycle
        // fingerprint, so p1's stale `undefined_method` row must clear
        // through a fresh fold. Keying on (content, generation) alone
        // would serve the stale row here.
        let generation_before = engine.state.graph_generation;
        current[2].1 = "class Other {\n  void m() {}\n  void n() {}\n}\n".into();
        engine.set_content(&root.join("lib/other.dart"), current[2].1.as_bytes());
        let out = analyze(&mut engine);
        assert_eq!(
            engine.state.graph_generation, generation_before,
            "a member-face move must not rebuild the graph — the fingerprint is what invalidates"
        );
        assert!(
            !out.iter().any(|row| row.code.name == "undefined_method"),
            "the dependency's new member resolves through the fresh fold: {out:?}"
        );
        assert_eq!(
            memo_serves(&engine),
            1,
            "the fingerprint move forks every key — nothing stale is served"
        );
        assert_matches_wholesale(&root, &engine, &current, &out);
    }

    /// The `unused_element` rows as (file path, message) — the message
    /// carries the declared name the verdict anchored.
    fn unused_rows(engine: &Engine, rows: &[Diagnostic]) -> Vec<(String, String)> {
        rows.iter()
            .filter(|row| row.code.name == unused_element_name())
            .map(|row| {
                (
                    engine
                        .file_table()
                        .path(row.file)
                        .map(|path| path.display().to_string())
                        .unwrap_or_default(),
                    row.message.to_string(),
                )
            })
            .collect()
    }

    /// The part-file unused-element close (#2156): the per-file pass fires
    /// `unused_element` against its own unit's use set, but the oracle runs
    /// the warning pass against the library-merged `UsedLocalElements`
    /// (library_analyzer.dart:305-315) — a use in any member file spares a
    /// declaration in every other file. The app corpus's family-B shape (a
    /// part-declared private class used only in the host) stays silent, the
    /// mirror (a host-declared private used only in the part) likewise,
    /// same-file sparing is untouched, and a genuinely unused part private
    /// still reports. Another library's use of a same-named private never
    /// spares — the close folds this library's faces only. A trailing host
    /// comment then salvages the part's fold (#1384) and the memo-served
    /// staging releases byte-identically to the wholesale recompute.
    #[test]
    fn part_file_unused_element_closes_library_wide() {
        let root = memo_workspace();
        let mut engine = memo_engine(&root);
        let main_base = "part 'p.dart';\n\nclass _Host {}\n\nvoid main() {\n  _Tile();\n  _tileFn();\n}\n";
        let p_base = "part of 'main.dart';\n\nclass _Tile {\n  const _Tile();\n}\n\nvoid _tileFn() {}\n\nclass _Orphan {}\n\nvoid _makeHost() {\n  _Host();\n}\n\nvoid _useSelf() {\n  _SelfUsed();\n}\n\nclass _SelfUsed {}\n\nclass _Shadow {}\n";
        let other_base =
            "class _Shadow {\n  void m() {}\n}\n\nvoid useOwn() {\n  _Shadow().m();\n}\n";
        let mut current: Vec<(String, String)> = vec![
            ("lib/main.dart".into(), main_base.into()),
            ("lib/p.dart".into(), p_base.into()),
            ("lib/other.dart".into(), other_base.into()),
        ];
        for (relative, content) in &current {
            engine.set_content(&root.join(relative), content.as_bytes());
        }
        let out = engine
            .analyze(&[], &mut |_| {}, &CancellationToken::default())
            .expect("analyze");
        assert_matches_wholesale(&root, &engine, &current, &out);
        let unused = unused_rows(&engine, &out);
        // The part file: only the genuinely unused privates report, in
        // source order (`_Tile`/`_tileFn` are used from the host,
        // `_SelfUsed` from its own file — all spared by the merged set).
        assert_eq!(
            unused,
            vec![
                (
                    root.join("lib/p.dart").display().to_string(),
                    "The declaration '_Orphan' isn't referenced.".into()
                ),
                (
                    root.join("lib/p.dart").display().to_string(),
                    "The declaration '_makeHost' isn't referenced.".into()
                ),
                (
                    root.join("lib/p.dart").display().to_string(),
                    "The declaration '_useSelf' isn't referenced.".into()
                ),
                (
                    root.join("lib/p.dart").display().to_string(),
                    "The declaration '_Shadow' isn't referenced.".into()
                ),
            ],
            "the family-B shapes must be spared, the orphans kept: {unused:?}"
        );

        // A trailing host comment churns only the host's bytes: the part's
        // fold is memo-served, and its staged rows release identically —
        // the merged set re-decides the same verdicts.
        current[0].1 = format!("{main_base}// churn\n");
        engine.set_content(&root.join("lib/main.dart"), current[0].1.as_bytes());
        let out = engine
            .analyze(&[], &mut |_| {}, &CancellationToken::default())
            .expect("analyze");
        assert_eq!(
            memo_serves(&engine),
            1,
            "the standing part is salvaged and its staging re-released"
        );
        assert_matches_wholesale(&root, &engine, &current, &out);
        assert_eq!(unused_rows(&engine, &out), unused, "served == fresh");
    }
}

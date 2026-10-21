//! The #2423 pins: the extension-pool family's two name-level separation
//! faces, plus the nested-index local-read credit.
//!
//! Law F (the analyzer re-triage) triaged the corpus's
//! `AMBIGUOUS_EXTENSION_MEMBER_ACCESS` rows as a pool double-registration;
//! the probes below falsified that — the faces register once, and the
//! false ambiguity is the specificity step: the name level compared only
//! the `on` type's HEAD, so sibling faces whose instantiated `on` types
//! differ in arguments (`List<ManifestNode>` vs `List<ManifestNode?>`,
//! `Map<K, FieldItem>` vs `Map<K, ManifestItem>`) stayed mutually
//! unordered and an unambiguous access read as ambiguous. The oracle's
//! `_isMoreSpecific` compares the instantiated `on` types
//! (`extension_member_resolver.dart:467-491`); the fix reads their shape
//! argument-wise at the same name level.
//!
//! The `UNUSED_LOCAL_VARIABLE` half is the assignment-target walk: an
//! unfolded index target (`t[0][c] = v`) keeps its index arguments as
//! plain read rows, and the walk stripped the nearest one's local credit
//! — the official walk only skips the target's own element
//! (`_useAssignmentTargetElement`), the index subexpression is a
//! normal read.
//!
//! Bounds-carrying generic faces (`extension _L<T extends A> on List<T>`,
//! the collection package's `IterableComparableExtension`) stay mutually
//! unordered here on purpose: the face model carries no type-parameter
//! bounds, so the oracle's instantiate-to-bounds tail is out of reach and
//! those corpus rows ride a separate dispatch.
//!
//! Oracle provenance: each expectation was probed with `dart analyze`
//! (Dart 3.13.1 via fvm 3.47.1) over the /tmp/probe2423 fixtures during
//! this ticket — lib/b.dart (genuine ambiguity fires), lib/g.dart
//! (nullable-argument domination resolves), lib/d.dart (subtype-argument
//! domination resolves, the dominated face's member reports),
//! lib/r2.dart + lib/r3.dart (nested-index read credit).

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::LibraryGraphBuilder;
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Diagnoses `source` as the sole file of a library importing nothing but
/// the seeded dart:core stub — the `ticket_2422_tests` rig mirrored (the
/// shapes below read `List`/`Map`/`String`/`int`/`bool`, which the
/// import-free 2318 rig cannot name).
fn report(source: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let core_file = files.intern(Path::new("<dart:core>"));
    let scanned = dartq_scanner::scan(
        source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        file,
    );
    assert!(
        scanned.diagnostics.is_empty(),
        "scanner diagnostics: {source:?}"
    );
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let elements = collect(source, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    let mut index = ElementIndex::default();
    index.insert(file, elements.clone());
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, file);
    }
    builder.seed_implicit_core(
        lib,
        core_file,
        &[
            "Object", "String", "int", "bool", "double", "num", "List", "Map", "void",
        ],
    );
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let fold = fold_unit(
        source,
        &scanned.tokens,
        features,
        &strings,
        file,
        lib,
        &graph,
        &namespaces,
        &index,
        StrictModes::default(),
        &LibraryClassModifiers::default(),
        set.is_enabled(dartq_language::Feature::InferenceUpdate2),
    );
    let mut report = fold.report;
    report.diagnostics = merge_source_ordered(fold.parse_diagnostics, report.diagnostics);
    report
}

fn code_names(report: &FileReport) -> Vec<&'static str> {
    report
        .diagnostics
        .iter()
        .map(|d| d.code.name)
        .collect::<Vec<_>>()
}

/// The diagnostics' code-plus-message view — the row identities a failing
/// assert needs (which local, which face).
fn code_views(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .map(|d| format!("{}: {}", d.code.name, d.message))
        .collect()
}

/// A non-nullable `on` argument dominates its nullable twin: the access
/// resolves through the non-nullable face and neither member reports
/// (the oracle resolves `xs.match` on `List<ManifestNode>` the same way).
#[test]
fn nullable_argument_face_dominates_its_twin() {
    let source = "class ManifestNode {}\n\
                  extension NodesExt on List<ManifestNode> {\n\
                    bool match(List<ManifestNode> other) => true;\n\
                  }\n\
                  extension NullableNodesExt on List<ManifestNode?> {\n\
                    bool match(List<ManifestNode?> other) => false;\n\
                  }\n\
                  bool use(List<ManifestNode> xs) => xs.match(xs);\n";
    assert!(
        code_names(&report(source)).is_empty(),
        "{:?}",
        code_names(&report(source))
    );
}

/// A subtype `on` argument dominates its supertype twin: the access
/// resolves through the exact face, whose member is spared, while the
/// dominated face's member — never resolved — reports
/// (the oracle: `FieldItem`-map access resolves, the `ManifestItem`-map
/// face's private member stays unused).
#[test]
fn subtype_argument_face_dominates_its_twin() {
    let source = "class LookupName {}\n\
                  class ManifestItem {}\n\
                  class FieldItem extends ManifestItem {}\n\
                  class BinaryWriter {}\n\
                  extension _ItemMapExt on Map<LookupName, ManifestItem> {\n\
                    void write(BinaryWriter w) {}\n\
                  }\n\
                  extension _FieldMapExt on Map<LookupName, FieldItem> {\n\
                    void write(BinaryWriter w) {}\n\
                  }\n\
                  void use(Map<LookupName, FieldItem> m, BinaryWriter w) {\n\
                    m.write(w);\n\
                  }\n";
    assert_eq!(
        code_names(&report(source)),
        ["unused_element"],
        "{:?}",
        code_names(&report(source))
    );
}

/// The genuine ambiguity is untouched: two faces with equal `on` types
/// stay mutually unordered and the access reports
/// (the /tmp/probe2350b control — the harness must keep observing it).
#[test]
fn genuine_ambiguity_still_reports() {
    let source = "extension B1 on Map<String, int> {\n\
                    int get probe => 1;\n\
                  }\n\
                  extension B2 on Map<String, int> {\n\
                    int get probe => 2;\n\
                  }\n\
                  int use(Map<String, int> m) => m.probe;\n";
    assert_eq!(
        code_names(&report(source)),
        ["ambiguous_extension_member_access"],
        "{:?}",
        code_names(&report(source))
    );
}

/// A nested index-assignment target's index argument is a read: the
/// local keeps its credit (`table[0][mj] = v` — the official walk skips
/// only the target's own element).
#[test]
fn nested_index_target_keeps_the_index_read() {
    let source = "class Fm {\n\
                    int matchesLayerOffset = 0;\n\
                    int minScore = 0;\n\
                    String pattern = '';\n\
                    List<List<int>> table = [];\n\
                    int computeScore() {\n\
                      for (var j = 0; j <= pattern.length; j++) {\n\
                        var mj = matchesLayerOffset + j;\n\
                        table[0][j] = j == 0 ? 0 : minScore << 1;\n\
                        table[0][mj] = minScore << 1;\n\
                      }\n\
                      return 0;\n\
                    }\n\
                  }\n";
    assert!(
        code_names(&report(source)).is_empty(),
        "{:?}",
        code_views(&report(source))
    );
}

/// The folded single-index shape is untouched: the index argument drains
/// into the fold, the write lands on the receiver's operator row, and the
/// local's own read keeps its credit.
#[test]
fn folded_index_target_keeps_the_index_read() {
    let source = "int go(List<int> l, int m) {\n\
                    var c = m;\n\
                    l[c] = 1;\n\
                    return c;\n\
                  }\n";
    assert!(
        code_names(&report(source)).is_empty(),
        "{:?}",
        code_names(&report(source))
    );
}

/// The compound-operator twin: a nested index argument of `t[0][c] += v`
/// is a read of `c` all the same.
#[test]
fn nested_index_compound_keeps_the_index_read() {
    let source = "class T6 {\n\
                    List<int> operator [](int i) => const [];\n\
                    void operator []=(int i, int v) {}\n\
                  }\n\
                  int go(T6 t, int m) {\n\
                    var c = m;\n\
                    t[0][c] += 1;\n\
                    return c;\n\
                  }\n";
    assert!(
        code_names(&report(source)).is_empty(),
        "{:?}",
        code_names(&report(source))
    );
}

/// The write-only rule itself is untouched: a plain assignment target
/// that is never read still reports.
#[test]
fn write_only_target_still_reports() {
    let source = "void go() {\n\
                    var x = 0;\n\
                    x = 1;\n\
                  }\n";
    assert_eq!(
        code_names(&report(source)),
        ["unused_local_variable"],
        "{:?}",
        code_names(&report(source))
    );
}

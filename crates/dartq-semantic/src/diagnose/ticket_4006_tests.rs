//! #4006 — the names-only SDK face carried no declaration-kind fact, so
//! `implements Comparator<int>` (a typedef-to-function-type face — the
//! oracle emits `implements_non_class`) was indistinguishable from
//! `implements Future<int>` (a class face — silent per #4001). The layer
//! contract this pin asserts: a stub entry whose recorded
//! [`FaceKind`] is `Class` answers clause-legal, one whose kind is
//! `NonClass` emits the clause code, and one whose kind is `Mixin`
//! answers per clause (`implements`/`with`/`on` legal, `extends` emits).
//!
//! The rig mounts `dart:core` the way the engine's SDK node build does —
//! `face_unit` + `declare_face` with the generated table's terminal kinds —
//! so the unindexed-declaring-file arm reads [`LibraryGraph::face_kind`]
//! exactly as production does. `Never` rides `NonClass` (the front-end
//! bottom type, not a class); the remaining stub heads ride `Class`, the
//! posture every pre-kind rig kept.

use super::test_rig::{CORE_PRELUDE, code_names, rows};
use super::{FileReport, StrictModes, fold_unit_with_ops, merge_source_ordered};
use crate::collector::collect;
use crate::library::ElementIndex;
use crate::model::{FileElements, MicroKind};
use crate::resolved_ops::{ClauseTrees, LibraryTypes, ResolvedTypeOps};
use dartq_language::{Feature, FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{
    ClassModifiers, Combinators, FaceKind, ImportEdge, LibraryClassModifiers, LibraryGraphBuilder,
    Namespaces,
};
use dartq_scanner::ScanConfig;
use dartq_source::{FileTable, StringTable};
use std::path::Path;

/// The names-only `dart:core` stub heads the fixtures name, each with the
/// terminal kind the generated `SDK_LIBRARY_FACES` table records:
/// `Future` is a class (re-exported `dart:async` class), `Comparator` a
/// typedef-to-function-type, `Never` the synthesized bottom type,
/// `PlainMixin` a bare mixin (the per-clause arm), and the rest classes —
/// the same shape `dart:core`'s real face carries.
const STUB_FACES: &[(&str, FaceKind)] = &[
    ("Object", FaceKind::Class),
    ("Never", FaceKind::NonClass),
    ("Null", FaceKind::NonClass),
    ("Function", FaceKind::Class),
    ("Future", FaceKind::Class),
    ("FutureOr", FaceKind::Class),
    ("Comparator", FaceKind::NonClass),
    ("PlainMixin", FaceKind::Mixin),
];

/// Diagnoses `source` (prelude ahead, like [`super::test_rig::report_ops`])
/// against a `dart:core` stub whose entries carry [`FaceKind`] facts.
fn report_face_kinds(source: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let source = format!("{CORE_PRELUDE}{source}");
    let scanned = dartq_scanner::scan(
        &source,
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
    let elements = collect(&source, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    let core_lib = builder.intern("dart:core");
    let core_file = files.intern(Path::new("<dart:core>"));
    builder.face_unit(core_lib, core_file);
    for (name, kind) in STUB_FACES {
        builder.declare_face(core_lib, strings.intern(name), core_file, *kind);
    }
    builder.import(
        lib,
        ImportEdge {
            target: Some(core_lib),
            prefix: None,
            deferred: false,
            non_library: false,
            combinators: Combinators::unfiltered(),
        },
    );
    let mut index = ElementIndex::default();
    index.insert(file, elements.clone());
    // The production posture: the synthetic face file IS in the element
    // index carrying an empty model — `index.get(owner)` answers `Some`,
    // so the clause check must discriminate on `graph.is_face_file`, not
    // on the index miss alone. An empty `FileElements` reproduces that.
    index.insert(core_file, FileElements::default());
    builder.unit(lib, file);
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        if matches!(
            decl.kind,
            MicroKind::Class | MicroKind::Enum | MicroKind::Mixin
        ) {
            builder.declare_class(lib, name, file, ClassModifiers::default());
        } else {
            builder.declare(lib, name, file);
        }
    }
    let mut clauses = ClauseTrees::empty();
    crate::capture_clause_trees(
        file,
        &source,
        &scanned.tokens,
        features,
        &strings,
        &mut clauses,
    );
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let types =
        LibraryTypes::build_with_clauses(&graph, &namespaces, &index, &strings, lib, &clauses);
    let ops = ResolvedTypeOps::new(&types, &strings, file);
    let fold = fold_unit_with_ops(
        &source,
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
        set.is_enabled(Feature::InferenceUpdate2),
        Some(&ops),
    );
    let mut report = fold.report;
    report.diagnostics = merge_source_ordered(fold.parse_diagnostics, report.diagnostics);
    report
}

/// `implements Comparator<int>` — the typedef-to-function-type face —
/// reports `implements_non_class` oracle-verbatim, while the class face
/// `implements Future<int>` stays silent on the same stub. Exactly one
/// row proves the split: zero would be the pre-#4006 silence, two would
/// mean the class face regressed into a false emit.
#[test]
fn typedef_face_implements_reports_and_class_face_stays_silent() {
    let report = report_face_kinds(
        r#"
class SubFuture implements Future<int> {
  SubFuture();
}

class SubComparator implements Comparator<int> {
  SubComparator();
}
"#,
    );
    let hits: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "implements_non_class")
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "exactly the Comparator clause may report implements_non_class, got {:?}",
        code_names(&report)
    );
}

/// The `Never` bottom type is `NonClass` upstream — `implements Never`
/// reports the clause code, never clause-legal.
#[test]
fn never_face_implements_reports() {
    let report = report_face_kinds(
        r#"
class SubNever implements Never {
  SubNever();
}
"#,
    );
    let names = code_names(&report);
    assert!(
        names.contains(&"implements_non_class"),
        "implements Never must report implements_non_class, got {names:?}"
    );
}

/// A mixin face answers per clause: `implements`/`with` accept it,
/// `extends` reports `extends_non_class` — the `MixinElement` arm of the
/// upstream `type.element` switch.
#[test]
fn mixin_face_answers_per_clause() {
    let report = report_face_kinds(
        r#"
class UsesMixin implements PlainMixin {
  UsesMixin();
}

class ExtendsMixin extends PlainMixin {
  ExtendsMixin();
}
"#,
    );
    let names = code_names(&report);
    assert!(
        names.contains(&"extends_non_class"),
        "extends over a mixin face must report extends_non_class, got {names:?}"
    );
    assert!(
        !names.contains(&"implements_non_class"),
        "implements over a mixin face stays legal, got {names:?}"
    );
}

/// Silence guard for the failure face: the report's rows name the clause
/// code and its span, so a regression to the pre-#4006 posture (silent on
/// `Comparator`) fails loudly here.
#[test]
fn comparator_clause_row_names_the_code() {
    let report = report_face_kinds(
        r#"
class SubComparator implements Comparator<int> {
  SubComparator();
}
"#,
    );
    let rows = rows(&report);
    assert!(
        rows.iter()
            .any(|row| row.starts_with("implements_non_class")),
        "expected an implements_non_class row, got {rows:?}"
    );
}

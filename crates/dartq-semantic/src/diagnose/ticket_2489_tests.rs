//! The #2489 pins: `unnecessary_type_check` reads the operand's resolved
//! face, not only the name-level lattice — the census's webview
//! `webview_android_controller.dart:1588-1589` oracle-only rows.
//!
//! - The `FutureOr<S>` operand head: `rv is Future<S>` over a
//!   `FutureOr<S>` operand is constant-true through rule 6's legs
//!   (`S <: Future<S>` fails the value leg upstream, but the 0c
//!   `InvalidType` vacuity answers it when `S` resolves nowhere — and the
//!   pure-type face `FutureOr<int> is Future<int>` folds through the
//!   declared-edge legs). The name-level lattice cannot pose a union
//!   head question, so the verdict re-asks through
//!   `Engine::resolved_subtype`.
//! - The `InvalidType` operand/checked gates: `type_check_verdict`
//!   orders `Invalid` operand → informative and `Invalid` checked →
//!   informative before every arm, so an undefined operand type or an
//!   `is Missing` spelling reports nothing here (the checked side's own
//!   `type_test_with_undefined_name` carries the report — probe
//!   `/tmp/probe2477/rig/p12_undef_arg_matrix.dart` cases b/e).
//! - The keep-honest faces: a well-typed operand (`int is
//!   Future<Missing>`) keeps both branches — rule 12's interface walk
//!   answers false — and a `dynamic` operand stays live the same way
//!   (probe p12 cases a/c).
//!
//! The harness is the #2477 one: the sole file of a library folded with
//! the resolved `TypeOps` mounted, diagnostics read off the joined
//! report.

use super::*;
use crate::collector::collect;
use crate::resolved_ops::{LibraryTypes, ResolvedTypeOps};
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::{LibraryClassModifiers, LibraryGraphBuilder};
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

fn scan(source: &str, file: FileId) -> (dartq_scanner::ScanResult, ParserFeatures, FeatureSet) {
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
    (scanned, features, set)
}

/// Diagnoses `source` as the sole file of a library with the resolved
/// `TypeOps` face mounted — the engine path the is-check folds decide
/// through.
fn report_ops(source: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let (scanned, features, set) = scan(source, file);
    let elements = collect(source, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    builder.unit(lib, file);
    let mut index = ElementIndex::default();
    index.insert(file, elements.clone());
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, file);
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let types = LibraryTypes::build(&graph, &namespaces, &index, &strings, lib);
    let ops = ResolvedTypeOps::new(&types, &strings, file);
    let fold = fold_unit_with_ops(
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
        Some(&ops),
    );
    let mut report = fold.report;
    report.diagnostics = merge_source_ordered(fold.parse_diagnostics, report.diagnostics);
    report
}

fn code_names(report: &FileReport) -> Vec<&'static str> {
    report.diagnostics.iter().map(|d| d.code.name).collect()
}

fn rows(report: &FileReport, code: &str) -> Vec<(u32, u32)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| (d.range.offset, d.range.length))
        .collect()
}

// —— the G6(ii) verdict half: `unnecessary_type_check` on the resolved face —

/// The census shape (probe `p3_is_future_undef_arg.dart`): `FutureOr<
/// UndefinedNav> rv; rv is Future<UndefinedNav>` folds constant-true —
/// the undefined written argument is `InvalidType` in that position, so
/// rule 6's value leg `Invalid <: Future<Invalid>` answers vacuously and
/// the future leg answers reflexively. `unnecessary_type_check` anchors
/// at the `is` expression span.
#[test]
fn future_or_undef_operand_is_future_folds_unnecessary() {
    let source = concat!(
        "class Future<T> {}\n",
        "class FutureOr<T> {}\n",
        "void h(FutureOr<UndefinedNav> rv) {\n",
        "  if (rv is Future<UndefinedNav>) {\n",
        "    print(rv);\n",
        "  } else {\n",
        "    print('no');\n",
        "  }\n",
        "}\n",
    );
    let report = report_ops(source);
    let is_token = source.find("rv is").map(|i| i as u32).unwrap();
    let checks = rows(&report, "unnecessary_type_check");
    assert!(
        checks.iter().any(|&(offset, _)| offset == is_token),
        "the FutureOr operand's is-test folds constant-true: {:?}",
        report.diagnostics
    );
}

/// The pure-type face: `Future<int> is FutureOr<int>` folds through
/// rule 10's future leg (`Future<int> <: Future<int>` answers
/// reflexively) — the union target admits the operand without the
/// value leg ever deciding.
#[test]
fn future_operand_is_future_or_folds_unnecessary() {
    let source = concat!(
        "class Future<T> {}\n",
        "class FutureOr<T> {}\n",
        "void h(Future<int> rv) {\n",
        "  if (rv is FutureOr<int>) {\n",
        "    print(rv);\n",
        "  }\n",
        "}\n",
    );
    let report = report_ops(source);
    let is_token = source.find("rv is").map(|i| i as u32).unwrap();
    let checks = rows(&report, "unnecessary_type_check");
    assert!(
        checks.iter().any(|&(offset, _)| offset == is_token),
        "the Future operand against FutureOr folds constant-true: {:?}",
        report.diagnostics
    );
}

/// Rule 6 is a conjunction: `FutureOr<int> is int` needs `int <: int`
/// AND `Future<int> <: int` — the second leg is false, so the check is
/// informative and neither row reports.
#[test]
fn future_or_operand_is_plain_head_stays_live() {
    let source = concat!(
        "class Future<T> {}\n",
        "class FutureOr<T> {}\n",
        "void h(FutureOr<int> rv) {\n",
        "  if (rv is int) {\n",
        "    print('a');\n",
        "  } else {\n",
        "    print('b');\n",
        "  }\n",
        "}\n",
    );
    let report = report_ops(source);
    assert!(
        rows(&report, "unnecessary_type_check").is_empty()
            && rows(&report, "dead_code").is_empty(),
        "the conjunctive miss keeps both arms live: {:?}",
        report.diagnostics
    );
}

// —— the `InvalidType` gates ————————————————————————————————————————

/// The undefined `FutureOr` head itself: `FutureOr` resolves nowhere
/// (no `dart:async` import, no declaration), so the operand is
/// `InvalidType` and the verdict is informative — no
/// `unnecessary_type_check` (probe p12 case b: the oracle reports only
/// `dead_code`, which the flow fold owns).
#[test]
fn undefined_future_or_head_keeps_the_verdict_silent() {
    let source = concat!(
        "class Future<T> {}\n",
        "void h(FutureOr<Missing> rv) {\n",
        "  if (rv is Future<Missing>) {\n",
        "    print('a');\n",
        "  } else {\n",
        "    print('b');\n",
        "  }\n",
        "}\n",
    );
    let report = report_ops(source);
    assert!(
        rows(&report, "unnecessary_type_check").is_empty(),
        "the Invalid operand is informative, not vacuous: {:?}",
        report.diagnostics
    );
}

/// An `is Missing` target: the checked side's own
/// `type_test_with_undefined_name` reports the spelling and the verdict
/// is informative — `unnecessary_type_check` never fires on an
/// `InvalidType` checked type (probe p12 case e).
#[test]
fn undefined_checked_head_keeps_the_verdict_silent() {
    let source = concat!(
        "class FutureOr<T> {}\n",
        "void h(FutureOr<Missing> rv) {\n",
        "  if (rv is Missing) {\n",
        "    print('a');\n",
        "  } else {\n",
        "    print('b');\n",
        "  }\n",
        "}\n",
    );
    let report = report_ops(source);
    assert!(
        rows(&report, "unnecessary_type_check").is_empty(),
        "the Invalid checked side is informative: {:?}",
        report.diagnostics
    );
}

/// An operand whose declared type resolves nowhere: `Missing rv` is the
/// `InvalidType` operand — informative before every arm, even the
/// vacuous `Invalid <: int` the 0c rule would answer.
#[test]
fn undefined_operand_type_keeps_the_verdict_silent() {
    let source = concat!(
        "void h(Missing rv) {\n",
        "  if (rv is int) {\n",
        "    print('a');\n",
        "  } else {\n",
        "    print('b');\n",
        "  }\n",
        "}\n",
    );
    let report = report_ops(source);
    assert!(
        rows(&report, "unnecessary_type_check").is_empty(),
        "the Invalid operand is informative: {:?}",
        report.diagnostics
    );
}

// —— keep-honest: the real relation still answers ————————————————————

/// `int is Future<Missing>`: the undefined argument is `InvalidType`
/// inside the checked type's own argument list — 0c fires only at the
/// top level, so rule 12's interface walk answers false and both
/// branches stay live (probe p12 case a).
#[test]
fn well_typed_operand_against_future_undef_stays_live() {
    let source = concat!(
        "class Future<T> {}\n",
        "void h(int x) {\n",
        "  if (x is Future<Missing>) {\n",
        "    print('a');\n",
        "  } else {\n",
        "    print('b');\n",
        "  }\n",
        "}\n",
    );
    let report = report_ops(source);
    assert!(
        rows(&report, "unnecessary_type_check").is_empty()
            && rows(&report, "dead_code").is_empty(),
        "the interface miss keeps both arms live: {:?}",
        report.diagnostics
    );
}

/// `dynamic is Future<Missing>`: Left-Top answers through `Object?`,
/// which is no `Future` — the check stays a real test (probe p12 case
/// c).
#[test]
fn dynamic_operand_against_future_undef_stays_live() {
    let source = concat!(
        "class Future<T> {}\n",
        "void h(dynamic x) {\n",
        "  if (x is Future<Missing>) {\n",
        "    print('a');\n",
        "  } else {\n",
        "    print('b');\n",
        "  }\n",
        "}\n",
    );
    let report = report_ops(source);
    assert!(
        rows(&report, "unnecessary_type_check").is_empty()
            && rows(&report, "dead_code").is_empty(),
        "the dynamic operand's test stays live: {:?}",
        report.diagnostics
    );
}

/// A `Null`-typed operand against `FutureOr<Null>`: rule 10's value leg
/// `Null <: Null` folds the check constant-true — the right-`FutureOr`
/// union admits it.
#[test]
fn null_operand_is_future_or_null_folds_unnecessary() {
    let source = concat!(
        "class FutureOr<T> {}\n",
        "void h(Null rv) {\n",
        "  if (rv is FutureOr<Null>) {\n",
        "    print('a');\n",
        "  }\n",
        "}\n",
    );
    let report = report_ops(source);
    let is_token = source.find("rv is").map(|i| i as u32).unwrap();
    let checks = rows(&report, "unnecessary_type_check");
    assert!(
        checks.iter().any(|&(offset, _)| offset == is_token),
        "the Null operand against FutureOr<Null> folds: {:?}",
        report.diagnostics
    );
}

/// `is!` with a constant-true pair reports the complement's row: the
/// same `FutureOr<UndefinedNav>` operand spells `is! Future<
/// UndefinedNav>` and the check is always false.
#[test]
fn future_or_undef_operand_is_not_future_folds_unnecessary_false() {
    let source = concat!(
        "class Future<T> {}\n",
        "class FutureOr<T> {}\n",
        "void h(FutureOr<UndefinedNav> rv) {\n",
        "  if (rv is! Future<UndefinedNav>) {\n",
        "    print('a');\n",
        "  } else {\n",
        "    print('b');\n",
        "  }\n",
        "}\n",
    );
    let report = report_ops(source);
    let is_token = source.find("rv is").map(|i| i as u32).unwrap();
    let checks = rows(&report, "unnecessary_type_check");
    assert!(
        checks.iter().any(|&(offset, _)| offset == is_token),
        "the is! face folds to the false row: {:?}",
        report.diagnostics
    );
}

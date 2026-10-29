//! The #2449 pins: receiver static types for top-level-getter references
//! and compound receiver chains (census F1, 240 o-only ERROR rows).
//!
//! A top-level getter is captured as a `MicroKind::TopLevelFn` with its
//! declared type on `return_ty` and no formal table, so its push-time
//! `BareInfo::Plain` carries `Unknown`. Every downstream member and
//! null-aware check read that information-less face and silently skipped,
//! while the oracle resolves the accessor element and reads its return
//! type before the member lookup (upstream Dart SDK source,
//! pkg/analyzer/lib/src/dart/resolver/property_element_resolver.dart —
//! the getter's declared return is the read's static type).
//!
//! Oracle provenance: /tmp/probe2449/{p5_getter,p4_shapes,p6_fn} (Dart
//! 3.13.1). `p5_getter` reports undefined_method on `g2.missing2()` and
//! `g2?.missing4()`, unchecked_use_of_nullable_value on `g1.missing1()`,
//! and invalid_null_aware_operator on the non-nullable `g2?.missing4()`;
//! `p4_shapes` reports on the top-level getter and the
//! `Holder().field?.m()` compound; `p6_fn` shows a top-level *function*
//! read does not borrow the callee's return type (it types as
//! `Function`), which is why the getter gate keys on the absent formal
//! list.

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::LibraryGraphBuilder;
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Diagnoses `source` as the sole file of a library with no imports —
/// the single-file rig (`ticket_2245_tests` carries the same shape).
fn report(source: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
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

fn count_code(report: &FileReport, name: &str) -> usize {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == name)
        .count()
}

fn codes(report: &FileReport) -> Vec<&str> {
    report.diagnostics.iter().map(|d| d.code.name).collect()
}

/// The plain-dot face on a non-nullable getter receiver: the getter's
/// declared return types the member lookup, so the miss reports
/// `undefined_method` on `Plain` (oracle `p5_getter` row at `g2.missing2()`).
#[test]
fn non_nullable_getter_receiver_reports_undefined_method() {
    let source = "\
class Plain {
  void real() {}
}

Plain get g2 => Plain();

void f() {
  g2.missing2();
}
";
    let report = report(source);
    assert_eq!(
        count_code(&report, "undefined_method"),
        1,
        "{:?}",
        codes(&report)
    );
}

/// The null-aware face over a nullable getter receiver: the receiver's
/// static type is `Plain?`, so the missing member reports AND the
/// unconditional-access face fires — the oracle's `g1?.missing3()` row.
#[test]
fn nullable_getter_receiver_reports_undefined_method() {
    let source = "\
class Plain {
  void real() {}
}

Plain? get g1 => null;

void f() {
  g1?.missing3();
}
";
    let report = report(source);
    assert_eq!(
        count_code(&report, "undefined_method"),
        1,
        "{:?}",
        codes(&report)
    );
    assert_eq!(
        count_code(&report, "invalid_null_aware_operator"),
        0,
        "a nullable getter keeps `?.` necessary: {:?}",
        codes(&report)
    );
}

/// The plain-dot face on a nullable getter receiver: the receiver types as
/// `Plain?`, so the oracle's `unchecked_use_of_nullable_value` reports
/// alongside the member miss (oracle `p5_getter` row at `g1.missing1()`).
#[test]
fn nullable_getter_plain_dot_reports_unchecked_use() {
    let source = "\
class Plain {
  void real() {}
}

Plain? get g1 => null;

void f() {
  g1.missing1();
}
";
    let report = report(source);
    assert_eq!(
        count_code(&report, "unchecked_use_of_nullable_value"),
        1,
        "{:?}",
        codes(&report)
    );
}

/// The unnecessary-null-aware face over a strictly non-nullable getter
/// receiver: the declared `Plain` makes `?.` unnecessary (oracle
/// `p5_getter` row at `g2?.missing4()`).
#[test]
fn non_nullable_getter_null_aware_reports_unnecessary() {
    let source = "\
class Plain {
  void real() {}
}

Plain get g2 => Plain();

void f() {
  g2?.missing4();
}
";
    let report = report(source);
    assert_eq!(
        count_code(&report, "invalid_null_aware_operator"),
        1,
        "{:?}",
        codes(&report)
    );
}

/// The compound receiver chain: `Holder().field?.m()` — the creation
/// receiver names `Holder`, the field read answers its declared `Plain?`,
/// and the missing member reports on `Plain` (oracle `p4_shapes` row at
/// `Holder().field?.m5()`).
#[test]
fn creation_then_field_null_aware_chain_reports_undefined_method() {
    let source = "\
class Plain {
  void real() {}
}

class Holder {
  Plain? field = null;
}

void f() {
  Holder().field?.m5();
}
";
    let report = report(source);
    assert_eq!(
        count_code(&report, "undefined_method"),
        1,
        "{:?}",
        codes(&report)
    );
}

/// The keep-silent guard: a resolvable member on a typed getter receiver
/// stays silent — the getter face types the receiver, and `real()` exists
/// on `Plain`, so no new d-only row appears (oracle `p5_getter` silent on
/// `g2.toString()`).
#[test]
fn resolvable_member_on_getter_receiver_stays_silent() {
    let source = "\
class Plain {
  void real() {}
}

Plain get g2 => Plain();
Plain? get g1 => null;

void f() {
  g2.real();
  g2.toString();
  g1?.real();
}
";
    let report = report(source);
    assert_eq!(
        count_code(&report, "undefined_method"),
        0,
        "{:?}",
        codes(&report)
    );
    assert_eq!(
        count_code(&report, "invalid_null_aware_operator"),
        0,
        "{:?}",
        codes(&report)
    );
}

/// The property-read face over a nullable getter receiver: the receiver's
/// static type is `Plain?`, so an unguarded property read reports
/// `unchecked_use_of_nullable_value` the same way an invocation does
/// (oracle `p5_getter` row at `print(g1.real)`).
#[test]
fn nullable_getter_property_read_reports_unchecked_use() {
    let source = "\
class Plain {
  void real() {}
}

Plain? get g1 => null;

void f() {
  print(g1.real);
}
";
    let report = report(source);
    assert_eq!(
        count_code(&report, "unchecked_use_of_nullable_value"),
        1,
        "{:?}",
        codes(&report)
    );
}

/// The function-read guard: a top-level *function* referenced as a value
/// is a tear-off whose static type is the signature the name-level model
/// does not render — it must not borrow the callee's return type
/// (`fn.missingA()` must not report on `void`; oracle `p6_fn` types it as
/// `Function`, a face this layer keeps under-reporting).
#[test]
fn top_level_function_read_does_not_borrow_return_type() {
    let source = "\
class Plain {
  void real() {}
}

void fn() {}

void test() {
  fn.missingA();
}
";
    let report = report(source);
    assert_eq!(
        count_code(&report, "undefined_method"),
        0,
        "a function tear-off keeps the unknown face: {:?}",
        codes(&report)
    );
}

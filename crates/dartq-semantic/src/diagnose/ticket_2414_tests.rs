//! The #2414 pins (U1/R1): the `?.` short-circuit carries its result
//! nullability onto every node the operator guards — `a?.b` types `B?`,
//! and `a?.b.c` types `C?` even though `.c` resolves its member on `b`'s
//! own written type (upstream Dart SDK source, pkg/analyzer's
//! ShortCircuiting resolution). The row keeps the member's declared face;
//! the member-offset latch (`null_aware_members`) marks the chain nodes
//! whose EXPRESSION result joins the `?`, and every result-consumer —
//! assignment right side, declaration initializer, `return`, argument
//! capture, `var` inference — re-arms it there.
//!
//! The receiver check is untouched: `a?.b.c` on `B` stays silent while
//! `a?.b.c` on `B?` reports `unchecked_use_of_nullable_value` — the `?`
//! propagates to the result, never into the continuation's receiver.
//!
//! Oracle provenance: every fixture was probed against `dart analyze`
//! (Dart 3.13.1, fvm default) on the same sources — the `/tmp/probe2414`
//! family. Member-call results (`a?.m()`) stay out: the model keeps no
//! declared type on an invoked member row, a pre-existing under-report
//! boundary that predates this family.

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::LibraryGraphBuilder;
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Diagnoses `source` as the sole file of an import-free library — the
/// single-file rig mirrored from the sibling ticket suites.
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
            "Object", "String", "int", "bool", "double", "num", "List", "Map",
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

/// The defect face (U1): `int x = a?.b` — the initializer's static type is
/// `int?`, not `int`, so the declaration reports `invalid_assignment`
/// (probed: 3.13.1 oracle reports exactly that code at the initializer).
#[test]
fn null_aware_initializer_reports_invalid_assignment() {
    let source = "class A { int b = 1; }\n\
                  void sink(int x) {}\n\
                  void f(A? a) {\n\
                  \x20 int x = a?.b;\n\
                  \x20 sink(x);\n\
                  }\n";
    assert_eq!(code_names(&report(source)), ["invalid_assignment"]);
}

/// The plain-assignment twin: `x = a?.b` stages the `int?` result against
/// the slot's `int`.
#[test]
fn null_aware_assignment_rhs_reports_invalid_assignment() {
    let source = "class A { int b = 1; }\n\
                  void sink(int x) {}\n\
                  void f(A? a) {\n\
                  \x20 int x = 0;\n\
                  \x20 x = a?.b;\n\
                  \x20 sink(x);\n\
                  }\n";
    assert_eq!(code_names(&report(source)), ["invalid_assignment"]);
}

/// The return face: `return a?.b` reads the `int?` result against the
/// declared `int` return (probed: oracle reports
/// `return_of_invalid_type_from_method` — the host name rides the code).
#[test]
fn null_aware_return_reports_return_of_invalid_type() {
    let source = "class A { int b = 1; }\n\
                  int f(A? a) {\n\
                  \x20 return a?.b;\n\
                  }\n";
    let names = code_names(&report(source));
    assert_eq!(names.len(), 1, "{names:?}");
    assert!(names[0].starts_with("return_of_invalid_type"), "{names:?}");
}

/// The argument face (U1): `g(a?.b)` offers `int?` where the formal reads
/// `int` — `argument_type_not_assignable`.
#[test]
fn null_aware_argument_reports_not_assignable() {
    let source = "class A { int b = 1; }\n\
                  void g(int x) {}\n\
                  void f(A? a) {\n\
                  \x20 g(a?.b);\n\
                  }\n";
    assert_eq!(
        code_names(&report(source)),
        ["argument_type_not_assignable"]
    );
}

/// The chain face (U1's propagation half): `a?.b.c` types the whole
/// expression `int?` — the `?` the `?.` could not join propagates through
/// the guarded continuation, so `g(a?.b.c)` reports (probed: oracle's
/// `argument_type_not_assignable` on the same source).
#[test]
fn null_aware_chain_result_reports_not_assignable() {
    let source = "class B { int c = 0; }\n\
                  class A { B get b => B(); }\n\
                  void g(int x) {}\n\
                  void f(A? a) {\n\
                  \x20 g(a?.b.c);\n\
                  }\n";
    assert_eq!(
        code_names(&report(source)),
        ["argument_type_not_assignable"]
    );
}

/// The chain-into-initializer twin: `int x = a?.b.c` asks `int?` against
/// `int` the same way.
#[test]
fn null_aware_chain_initializer_reports_invalid_assignment() {
    let source = "class B { int c = 0; }\n\
                  class A { B get b => B(); }\n\
                  void sink(int x) {}\n\
                  void f(A? a) {\n\
                  \x20 int x = a?.b.c;\n\
                  \x20 sink(x);\n\
                  }\n";
    assert_eq!(code_names(&report(source)), ["invalid_assignment"]);
}

/// The receiver-side control (the latch must not leak INTO the
/// continuation's receiver check): `a?.b.c` on `B?` reports
/// `unchecked_use_of_nullable_value` at `.c` — the `?.` guards `a`, never
/// `b`'s own nullable result (probed: the oracle reports exactly one row).
#[test]
fn null_aware_chain_on_nullable_member_reports_unchecked_use() {
    let source = "class B { int c = 0; }\n\
                  class A { B? get b => null; }\n\
                  void sink(Object? x) {}\n\
                  void f(A? a) {\n\
                  \x20 var v = a?.b.c;\n\
                  \x20 sink(v);\n\
                  }\n";
    assert_eq!(
        code_names(&report(source)),
        ["unchecked_use_of_nullable_value"]
    );
}

/// The double-`?.` control: `a?.b?.c` unwraps each receiver — no
/// unchecked-dereference face at all (probed: oracle silent).
#[test]
fn double_null_aware_chain_stays_silent() {
    let source = "class B { int c = 0; }\n\
                  class A { B? get b => null; }\n\
                  void sink(Object? x) {}\n\
                  void f(A? a) {\n\
                  \x20 var v = a?.b?.c;\n\
                  \x20 sink(v);\n\
                  }\n";
    assert_eq!(code_names(&report(source)), Vec::<&str>::new());
}

/// The R1 exemplar: `x?.m == true` — the equality asks no nullability
/// question and the result feeds a `bool` return cleanly (probed: the
/// webview_flutter_android shape is silent on the oracle).
#[test]
fn null_aware_member_equality_stays_silent() {
    let source = "class X { bool get m => true; }\n\
                  bool f(X? x) {\n\
                  \x20 return x?.m == true;\n\
                  }\n";
    assert_eq!(code_names(&report(source)), Vec::<&str>::new());
}

/// The `!` contrast control: `g(a!.b)` unwraps the receiver — the `int`
/// result assigns cleanly (probed: oracle silent; the `?` re-arm must
/// never reach a non-null-aware member).
#[test]
fn bang_receiver_result_stays_non_nullable() {
    let source = "class A { int b = 1; }\n\
                  void g(int x) {}\n\
                  void f(A? a) {\n\
                  \x20 g(a!.b);\n\
                  }\n";
    assert_eq!(code_names(&report(source)), Vec::<&str>::new());
}

/// The compatible-target control: `int? x = a?.b` — the nullable result
/// fits the nullable slot (probed: oracle silent; the re-arm must not
/// over-report on a legal face).
#[test]
fn null_aware_result_into_nullable_slot_stays_silent() {
    let source = "class A { int b = 1; }\n\
                  void sink(int? x) {}\n\
                  void f(A? a) {\n\
                  \x20 int? x = a?.b;\n\
                  \x20 sink(x);\n\
                  }\n";
    assert_eq!(code_names(&report(source)), Vec::<&str>::new());
}

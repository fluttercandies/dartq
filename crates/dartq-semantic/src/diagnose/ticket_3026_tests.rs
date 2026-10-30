//! The #3026 pins: the generic-enum exhaustiveness filter
//! (`EnumStaticType._createEnumElements`, upstream Dart SDK source,
//! pkg/_fe_analyzer_shared/lib/src/exhaustiveness/types/enum.dart:99-134)
//! — a constant populates the enum's closed subtype set only when its
//! declared instantiation is a subtype of the scrutinee's
//! overapproximation, so `E.b` (`E<String>`) is no value of `E<num>` and
//! an `E.a || E.c` case set is already exhaustive — plus the
//! `isAlwaysExhaustive` `FutureOr<T>` and type-parameter-bound arms
//! (type_system.dart:869-879): `FutureOr<S>` of a sealed `S` and
//! `T extends St` over the enum `St` must split completely.

use crate::collector::collect;
use crate::diagnose::{FileReport, StrictModes, diagnose_with};
use crate::library::ElementIndex;
use crate::model::MicroKind;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{LibraryClassModifiers, LibraryGraphBuilder, Namespaces};
use dartq_scanner::ScanConfig;
use dartq_source::{FileTable, StringTable};
use std::path::Path;

/// The one-library rig: `source` is the sole analyzed file of
/// `package:t`, no imports (the #2432 convention).
fn unit_report(source: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
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
    let elements = collect(source, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, file);
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let mut index = ElementIndex::default();
    index.insert(file, elements);
    diagnose_with(
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
    )
}

fn count_code(report: &FileReport, name: &str) -> usize {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == name)
        .count()
}

// —— the generic-enum constant filter ————————————————————————————————

/// `E<num>` over `enum E<T> { a<int>, b<String>, c<double> }`: `E.b`
/// can never inhabit the scrutinee, so the `E.a`/`E.c` case set is
/// already exhaustive — oracle 3.13.x reports no
/// `non_exhaustive_switch_expression` (co19
/// `exhaustiveness_enum_A01_t01`).
#[test]
fn generic_enum_num_scrutinee_excludes_mismatched_constant() {
    let source = "\
enum E<T> { a<int>(1), b<String>('x'), c<double>(2.0); const E(Object _); }
f(E<num> e) {
  return switch (e) {
    E.a => 1,
    E.c => 3,
  };
}
";
    let report = unit_report(source);
    assert_eq!(
        count_code(&report, "non_exhaustive_switch_expression"),
        0,
        "{:?}",
        report.diagnostics
    );
}

/// The complementary gap: `E.a` alone over `E<num>` leaves the
/// `E.c`-shaped population uncovered — `non_exhaustive_switch_expression`
/// reports with `E.c` as the witness, never `E.b` (the filtered
/// constant can never be a witness either, co19
/// `exhaustiveness_enum_A02_t01` test4 arm).
#[test]
fn generic_enum_missing_constant_reports_the_populating_one() {
    let source = "\
enum E<T> { a<int>(1), b<String>('x'), c<double>(2.0); const E(Object _); }
f(E<num> e) {
  return switch (e) {
    E.a => 1,
  };
}
";
    let report = unit_report(source);
    let rows: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "non_exhaustive_switch_expression")
        .collect();
    assert_eq!(rows.len(), 1, "rows: {rows:?}");
    assert!(
        rows[0].message.contains("doesn't match the pattern 'E.c'"),
        "{rows:?}"
    );
}

/// `E<T>` with the function's own `T extends num` overapproximates to
/// `E<num>`: the same `E.a`/`E.c` set covers it — oracle stays silent
/// (co19 `exhaustiveness_enum_A01_t02`, the bound-substitution half of
/// the filter).
#[test]
fn generic_enum_param_scrutinee_overapproximates_to_the_bound() {
    let source = "\
enum E<T> { a<int>(1), b<String>('x'), c<double>(2.0); const E(Object _); }
f<T extends num>(E<T> e) {
  return switch (e) {
    E.a => 1,
    E.c => 3,
  };
}
";
    let report = unit_report(source);
    assert_eq!(
        count_code(&report, "non_exhaustive_switch_expression"),
        0,
        "{:?}",
        report.diagnostics
    );
}

/// Every-constant coverage of a generic enum stays exhaustive exactly
/// like before — the filter only removes constants the scrutinee
/// cannot hold (regression pin: `E<dynamic>` holds them all, so a
/// missing `E.b` still reports).
#[test]
fn generic_enum_dynamic_scrutinee_keeps_every_constant() {
    let source = "\
enum E<T> { a<int>(1), b<String>('x'); const E(Object _); }
f(E<dynamic> e) {
  return switch (e) {
    E.a => 1,
  };
}
";
    let report = unit_report(source);
    let rows: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "non_exhaustive_switch_expression")
        .collect();
    assert_eq!(rows.len(), 1, "rows: {rows:?}");
    assert!(
        rows[0].message.contains("doesn't match the pattern 'E.b'"),
        "{rows:?}"
    );
}

// —— `isAlwaysExhaustive`: `FutureOr<T>` and the parameter bound ————-

/// `FutureOr<S>` of a sealed `S` is always-exhaustive
/// (type_system.dart:869-871): a statement switch over it must split
/// completely — a missing `B` case reports
/// `non_exhaustive_switch_statement` where the unmodelled arm used to
/// stay silent.
#[test]
fn future_or_of_sealed_is_always_exhaustive() {
    let source = "\
import 'dart:async';
sealed class S {}
class A extends S {}
class B extends S {}
f(FutureOr<S> x) {
  switch (x) {
    case A _:
      break;
  }
}
";
    let report = unit_report(source);
    let rows: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "non_exhaustive_switch_statement")
        .collect();
    assert_eq!(rows.len(), 1, "rows: {rows:?}");
}

/// `FutureOr<int>` is NOT always-exhaustive (`int` is open): a
/// statement switch over it keeps the pre-arm silence — the
/// `FutureOr` arm delegates to the base, never closes on its own.
#[test]
fn future_or_of_open_type_stays_silent() {
    let source = "\
import 'dart:async';
f(FutureOr<int> x) {
  switch (x) {
    case int _:
      break;
  }
}
";
    let report = unit_report(source);
    assert_eq!(
        count_code(&report, "non_exhaustive_switch_statement"),
        0,
        "{:?}",
        report.diagnostics
    );
}

/// `T extends St` over the enum `St` is always-exhaustive
/// (type_system.dart:873-879, the declared-bound arm): a statement
/// switch missing `St.b` reports — the parameter delegates to its
/// bound, which the sealed arm closes.
#[test]
fn type_param_delegates_to_its_enum_bound() {
    let source = "\
enum St { a, b }
f<T extends St>(T t) {
  switch (t) {
    case St.a:
      break;
  }
}
";
    let report = unit_report(source);
    let rows: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "non_exhaustive_switch_statement")
        .collect();
    assert_eq!(rows.len(), 1, "rows: {rows:?}");
    assert!(
        rows[0].message.contains("doesn't match the pattern 'St.b'"),
        "{rows:?}"
    );
}

/// An unbounded parameter (`f<T>(T t)`) is never always-exhaustive —
/// the bound-less arm answers `false`, keeping the pre-#3026 silence.
#[test]
fn unbounded_type_param_stays_silent() {
    let source = "\
f<T>(T t) {
  switch (t) {
    case int _:
      break;
  }
}
";
    let report = unit_report(source);
    assert_eq!(
        count_code(&report, "non_exhaustive_switch_statement"),
        0,
        "{:?}",
        report.diagnostics
    );
}

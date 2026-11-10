//! The #3007 pins: the extension-type erasure face of the name-level
//! exhaustiveness world (`getExtensionTypeErasure`, upstream Dart SDK
//! source, pkg/_fe_analyzer_shared/lib/src/exhaustiveness/shared.dart:195
//! — every `StaticType` the walk builds erases the extension type to its
//! representation first). An `extension type ET(E _)` scrutinee answers
//! through `E`: the covering gap reports with `E`'s unmatched constants
//! as the witness, an exhaustive case set stays silent, and a
//! representation-enum constant may overlap the matched value (no
//! `constant_pattern_never_matches_value_type`). The second face is the
//! non-nullable-local definite-assignment gate — a declared-non-nullable
//! local read before any assignment reports
//! `not_assigned_potentially_non_nullable_local_variable`, while a
//! nullable or initialized declaration stays silent (oracle 3.13.x:
//! `int x;` reports at the first read, `int? x;` and `int x = f();`
//! never do).

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

// —— the extension-type erasure face ————————————————————————————————

/// `ET1` (a bare extension type, no `implements`) erases to its enum
/// representation for the covering verdict: an `E.a`-only case set
/// reports `non_exhaustive_switch_expression` with the repr's remaining
/// constants as the witness — never `ET1()` (oracle 3.13.x, the
/// `getExtensionTypeErasure` entry inside `getStaticType`).
#[test]
fn extension_type_scrutinee_reports_the_repr_witness() {
    let source = "\
enum E { a, b, c }
extension type ET(E _) {}
f(ET e) {
  return switch (e) {
    E.a => 1,
    E.c => 3,
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
        "witness names the representation type's constant: {rows:?}"
    );
}

/// The same switch over the enum directly keeps its verbatim verdict —
/// the erasure never touches a non-extension-type scrutinee.
#[test]
fn plain_enum_scrutinee_unchanged() {
    let source = "\
enum E { a, b }
f(E e) {
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

/// An exhaustive case set over the erased representation stays silent:
/// `ET` over `E` with every constant covered reports no covering row —
/// and the enum constants may overlap the matched value, so no
/// `constant_pattern_never_matches_value_type` either (the co19
/// `exhaustiveness_enum_A01` shape, oracle 3.13.x: clean).
#[test]
fn exhaustive_extension_type_scrutinee_stays_silent() {
    let source = "\
enum E { a, b }
extension type ET(E _) {}
f(ET e) {
  return switch (e) {
    E.a => 1,
    E.b => 2,
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
    assert_eq!(
        count_code(&report, "constant_pattern_never_matches_value_type"),
        0,
        "{:?}",
        report.diagnostics
    );
}

/// `ET2 implements E` is the always-exhaustive face for STATEMENT
/// switches too: the erased representation is an enum, so a missing
/// constant reports `non_exhaustive_switch_statement` — the
/// `isAlwaysExhaustive` extension-type arm (type_system.dart:862-868).
#[test]
fn statement_switch_over_implements_extension_type_is_always_exhaustive() {
    let source = "\
enum E { a, b }
extension type ET(E _) implements E {}
f(ET e) {
  switch (e) {
    case E.a:
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
        rows[0].message.contains("doesn't match the pattern 'E.b'"),
        "{rows:?}"
    );
}

/// A non-enum representation keeps the ordinary open verdict: `ET(int _)`
/// reports the repr head's bare witness (`int()`), not `ET()` —
/// `getExtensionTypeErasure` again, over a primitive representation.
#[test]
fn extension_type_over_primitive_reports_repr_head() {
    let source = "\
extension type ET(int _) {}
f(ET e) {
  return switch (e) {
    0 => 1,
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
        rows[0]
            .message
            .contains("doesn't match the pattern 'int()'"),
        "{rows:?}"
    );
}

/// The `!` postfix wrap keeps its `Null` half whole and intersects only
/// the non-null half: `E.a!` over `E?` covers `E.a|Null`, so the
/// reported gap is the remaining constants — never `null` and never the
/// covered constant (`createNullAssertSpace`, shared.dart:790-797 — the
/// wrap UNIONs the narrowed space with the full `Null` space, never
/// intersects the `Null` leaf away).
#[test]
fn enum_constant_null_assert_wrap_reports_the_remaining_constants() {
    let source = "\
enum E { a, b }
f(E? e) {
  return switch (e) {
    E.a! => 1,
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

/// A `p!` whose subpattern covers the whole non-null face reports
/// nothing: `E _!` over `E?` wraps `E|Null` — the full `E?` space.
#[test]
fn covering_null_assert_wrap_stays_silent() {
    let source = "\
enum E { a, b }
f(E? e) {
  return switch (e) {
    E _! => 1,
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

// —— the definite-assignment face ——————————————————————————————————

/// `int x;` then a read reports
/// `not_assigned_potentially_non_nullable_local_variable` at the read —
/// the non-late, non-final `!isAssigned` arm of upstream
/// `_checkReadOfNotAssignedLocalVariable` (type_analyzer.dart:4990-5030).
#[test]
fn non_nullable_local_read_before_assignment_reports() {
    let source = "\
f() {
  int x;
  print(x);
}
";
    let report = unit_report(source);
    assert_eq!(
        count_code(
            &report,
            "not_assigned_potentially_non_nullable_local_variable"
        ),
        1,
        "{:?}",
        report.diagnostics
    );
}

/// `int? x;` reads silent — a nullable declared type is never the
/// `isPotentiallyNonNullable` arm (the same gate, nullable face).
#[test]
fn nullable_local_read_before_assignment_silent() {
    let source = "\
f() {
  int? x;
  print(x);
}
";
    let report = unit_report(source);
    assert_eq!(
        count_code(
            &report,
            "not_assigned_potentially_non_nullable_local_variable"
        ),
        0,
        "{:?}",
        report.diagnostics
    );
}

/// `int x = f();` reads silent — the initializer is the assignment.
#[test]
fn initialized_local_silent() {
    let source = "\
int g() => 0;
f() {
  int x = g();
  print(x);
}
";
    let report = unit_report(source);
    assert_eq!(
        count_code(
            &report,
            "not_assigned_potentially_non_nullable_local_variable"
        ),
        0,
        "{:?}",
        report.diagnostics
    );
}

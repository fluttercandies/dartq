//! The #2445 pins: the constant-pattern verdict over an unsolved generic
//! call's residual type-parameter scrutinee.
//!
//! `reader.readEnum(AstNodeTag.values)` — whose `<T extends Enum>` lives
//! at the declaring executable, outside the diagnosing function's
//! parameter tracks — leaves the scrutinee spelled `T` (`is_scope_param`
//! answers false at the use, so the in-scope bound substitution never
//! stands in). The ordinary relation then read the unresolvable head as
//! provably disjoint and emitted `constant_pattern_never_matches_value_type`
//! rows the oracle never reports. The official gate (`_canBeEqual`,
//! upstream Dart SDK source,
//! pkg/analyzer/lib/src/dart/constant/constant_verifier.dart:563) decides
//! a `TypeParameterType` matched face through the parameter's
//! element-resolved bound and falls to the unsupported `return true` —
//! no warning — when no bound is resolvable at the check; the same
//! fall-through covers an `InvalidType` (undefined-name) scrutinee. The
//! engine mirror: a matched head the world cannot resolve as a declared
//! face (`is_known_head` — the same opaque-head notion the
//! mutual-exclusion relation's right arm uses) answers "possible",
//! while a bound visible in scope (the function's own and the enclosing
//! container's parameter) still substitutes and decides, and a declared
//! unrelated scrutinee still reports.
//!
//! Oracle provenance: /tmp/probe2445 — `dart analyze` 3.13.1: p1 (the
//! unsolved call) silent, p2 (function-scope bound) and p4 (container-
//! scope bound) silent on the enum constant and reporting on `case 1`,
//! p7 (`String` scrutinee) reporting both rows.

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
/// `package:t`, no imports.
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

/// The unsolved call: the method's `T` is out of the diagnosing scope, so
/// the scrutinee spells a parameter the check cannot bound — the verdict
/// is unprovable and every face stays silent (oracle p1: no
/// never-match row, no dead body).
#[test]
fn unsolved_call_scrutinee_stays_silent() {
    let source = "\
class List<E> {
  E get first => throw 0;
}
enum AstNodeTag { a, b, c }
class R {
  T readEnum<T extends Enum>(List<T> values) => values.first;
}
Object? f(R reader) {
  var tag = reader.readEnum(AstNodeTag.values);
  switch (tag) {
    case AstNodeTag.a:
      return 1;
    case AstNodeTag.b:
      return 2;
    case AstNodeTag.c:
      return 3;
  }
}
";
    let report = unit_report(source);
    assert_eq!(
        count_code(&report, "constant_pattern_never_matches_value_type"),
        0,
        "{:?}",
        report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect::<Vec<_>>()
    );
    assert_eq!(count_code(&report, "dead_code"), 0);
}

/// The over-suppression guard: a declared unrelated scrutinee still
/// reports the never-match row (oracle p7 reports over `String`).
#[test]
fn declared_scrutinee_still_reports_never_match() {
    let source = "\
enum AstNodeTag { a, b }
class Unrelated {}
Object? f() {
  Unrelated u = Unrelated();
  switch (u) {
    case AstNodeTag.a:
      return 1;
  }
  return null;
}
";
    let report = unit_report(source);
    assert_eq!(
        count_code(&report, "constant_pattern_never_matches_value_type"),
        1,
        "{:?}",
        report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect::<Vec<_>>()
    );
}

/// The function-scope bound face: `T extends AstNodeTag` on the
/// diagnosing function itself substitutes the bound — the enum constant
/// can equal it, the int constant provably cannot (oracle p2: one row,
/// anchored at `case 1`).
#[test]
fn function_scope_bound_still_decides() {
    let source = "\
enum AstNodeTag { a, b }
Object? g<T extends AstNodeTag>(T value) {
  switch (value) {
    case AstNodeTag.a:
      return 1;
    case 1:
      return 2;
  }
  return null;
}
";
    let report = unit_report(source);
    assert_eq!(
        count_code(&report, "constant_pattern_never_matches_value_type"),
        1,
        "{:?}",
        report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect::<Vec<_>>()
    );
}

/// The container-scope bound face: the enclosing class's parameter bound
/// reaches the method body the same way (oracle p4: one row, anchored at
/// `case 1`).
#[test]
fn container_scope_bound_still_decides() {
    let source = "\
enum AstNodeTag { a, b }
class C<T extends AstNodeTag> {
  Object? m(T value) {
    switch (value) {
      case AstNodeTag.a:
        return 1;
      case 1:
        return 2;
    }
    return null;
  }
}
";
    let report = unit_report(source);
    assert_eq!(
        count_code(&report, "constant_pattern_never_matches_value_type"),
        1,
        "{:?}",
        report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect::<Vec<_>>()
    );
}

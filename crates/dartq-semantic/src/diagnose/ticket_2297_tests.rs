//! #2297 pins: the implicit-`Object` edge `TypeSystem::can_be_equal`
//! asks through `isSubtypeOf` (subtype.dart:304-340 — every class type
//! without a written `extends` clause carries the `Object` superclass,
//! the primitives `bool`/`String` included), and the conservative arm
//! of the switch-end `isExhaustive` verdict.
//!
//! The `TypeParameterTypeImpl` bound-recursion rides the recorder's
//! `boundaries` table. Since #2358 the production `DiagnoseListener`
//! forwards the `begin/end_type_variable(s)` (and record-type) events
//! that populate it, so a type-parameter scrutinee resolves through its
//! *declared* bound under the real pipeline (`type_system.dart:868-877`):
//! `T extends St` over the enum `St` splits completely and the full
//! switch is exhaustive. The conservative arm survives exactly where the
//! official's is conservative too: the implicit `Object?` bound of an
//! unbounded parameter never exhausts, and a circular bound chain
//! exhausts the walk's recursion budget into `false`.

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

/// `(offset, name)` of each row, in report order — presence/absence
/// anchors below (the residual faces still report beside the pinned
/// rows, so no exact-list assertions).
fn rows(report: &FileReport) -> Vec<(u32, &'static str)> {
    report
        .diagnostics
        .iter()
        .map(|d| (d.range.offset, d.code.name))
        .collect()
}

fn has(report: &FileReport, name: &str) -> bool {
    report.diagnostics.iter().any(|d| d.code.name == name)
}

// —— the type-parameter bound arm ————————————————————————————————————

/// A type-parameter scrutinee resolves through its declared bound: the
/// forwarded `begin/end_type_variable` events populate `Flow::boundaries`
/// (#2358), and the switch-end verdict recurses into `T extends St`'s
/// bound (`type_system.dart:868-877`) — over the enum `St` the space
/// splits completely, so the full-coverage switch is exhaustive and the
/// body end is unreachable (oracle silent: no `body_might` row).
#[test]
fn type_param_scrutinee_exhaustive_through_its_declared_bound() {
    let source = "\
class int {}
enum St { a, b }
int f<T extends St>(T v) {
  switch (v) {
    case St.a:
      return 1;
    case St.b:
      return 2;
  }
}
";
    let report = unit_report(source);
    assert!(
        !has(&report, "body_might_complete_normally"),
        "{:?}",
        rows(&report)
    );
}

/// #2358 completeness pin: the same bound-sourced exhaustiveness must
/// also silence the switch's own row — `T extends St` with full coverage
/// reports neither `non_exhaustive_switch_statement` nor a reachable
/// body end.
#[test]
fn type_param_scrutinee_full_coverage_is_not_non_exhaustive() {
    let source = "\
class int {}
enum St { a, b }
int f<T extends St>(T v) {
  switch (v) {
    case St.a:
      return 1;
    case St.b:
      return 2;
  }
}
";
    let report = unit_report(source);
    assert!(
        !has(&report, "non_exhaustive_switch_statement"),
        "{:?}",
        rows(&report)
    );
    assert!(
        !has(&report, "body_might_complete_normally"),
        "{:?}",
        rows(&report)
    );
}

/// Contrast — the conservative faces survive the forwarding: an
/// unbounded `T` over the same enum carries the implicit `Object?` bound
/// (subtype.dart:313), which never exhausts — the post-switch unmatched
/// path stays live and the declared `int` return reports
/// `body_might_complete_normally`.
#[test]
fn unbounded_type_param_over_enum_stays_conservative() {
    let source = "\
class int {}
enum St { a, b }
int f<T>(T v) {
  switch (v) {
    case St.a:
      return 1;
    case St.b:
      return 2;
  }
}
";
    let report = unit_report(source);
    assert!(
        has(&report, "body_might_complete_normally"),
        "{:?}",
        rows(&report)
    );
}

/// Contrast — a circular bound chain (`T extends U, U extends T`, which
/// the oracle rejects as `type_parameter_supertype_of_its_bound`) must
/// cycle into the walk's recursion budget and answer the conservative
/// `false`: the body end stays live.
#[test]
fn circular_bound_chain_stays_conservative() {
    let source = "\
class int {}
int f<T extends U, U extends T>(T v) {
  switch (v) {
    case 1:
      return 1;
  }
}
";
    let report = unit_report(source);
    assert!(
        has(&report, "body_might_complete_normally"),
        "{:?}",
        rows(&report)
    );
}

/// The conservative arm: an unbounded `T` carries the implicit
/// `Object?` bound (subtype.dart:313), which is not always exhaustive
/// — the trailing statement stays live and the declared `int` return
/// still reports `body_might_complete_normally` (oracle
/// implicit_bound2: silent — coverage is complete there; this cell
/// pins the boundary's own answer through the missing-default shape).
#[test]
fn unbounded_type_param_keeps_the_unmatched_path_live() {
    let source = "\
class int {}
int f<T>(T v) {
  switch (v) {
    case 1:
      return 1;
  }
}
";
    let report = unit_report(source);
    assert!(
        has(&report, "body_might_complete_normally"),
        "{:?}",
        rows(&report)
    );
}

// —— the `can_be_equal` implicit-`Object` edge ————————————————————————

/// `Object` vs a `bool` constant: `bool <: Object` through the
/// implicit superclass edge — no
/// `constant_pattern_never_matches_value_type` (oracle obj_true:
/// silent). The name-level world answers the same for the String and
/// enum-constant faces.
#[test]
fn object_scrutinee_accepts_the_core_constant_faces() {
    let source = "\
class int {}
void f(Object o) {
  switch (o) {
    case true:
      break;
  }
}
";
    let report = unit_report(source);
    assert!(
        !has(&report, "constant_pattern_never_matches_value_type"),
        "{:?}",
        rows(&report)
    );

    let source = "\
enum St { a, b }
void f(Object o) {
  switch (o) {
    case St.a:
      break;
  }
}
";
    let report = unit_report(source);
    assert!(
        !has(&report, "constant_pattern_never_matches_value_type"),
        "{:?}",
        rows(&report)
    );
}

/// `bool` vs an enum constant: `St` and `bool` share no subtype edge,
/// so the constant can never equal the matched value (oracle
/// bool_enum: `constant_pattern_never_matches_value_type` at the
/// constant — the `named_sub` implicit-`Object` edge serves
/// `can_be_equal`'s positive arm, not a fabricated `St <: bool`). The
/// stub-less rig also reports `bool`'s `undefined_class` and the dead
/// case body that the never-matching matched type implies; the row
/// under pin is the constant's.
#[test]
fn bool_scrutinee_rejects_the_enum_constant() {
    let source = "\
enum St { a, b }
void f(bool b) {
  switch (b) {
    case St.a:
      break;
  }
}
";
    let report = unit_report(source);
    assert!(
        has(&report, "constant_pattern_never_matches_value_type"),
        "{:?}",
        rows(&report)
    );
}

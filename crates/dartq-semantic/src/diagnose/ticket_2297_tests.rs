//! #2297 pins: the implicit-`Object` edge `TypeSystem::can_be_equal`
//! asks through `isSubtypeOf` (subtype.dart:304-340 — every class type
//! without a written `extends` clause carries the `Object` superclass,
//! the primitives `bool`/`String` included), and the conservative arm
//! of the switch-end `isExhaustive` verdict.
//!
//! The `TypeParameterTypeImpl` bound-recursion itself is pinned at the
//! flow level (`dartq_flow::record::tests`), because that arm's bound
//! source is the recorder's `boundaries` table — and the production
//! `DiagnoseListener` (a separate `ParserListener` that wraps `Flow`)
//! forwards `handle_type`/`end_formal_parameter` but not the
//! `begin/end_type_variable`/`end_top_level_declaration` events that
//! populate it. Under this rig `Flow::boundaries` is empty, so a
//! type-parameter scrutinee answers the conservative `false` (its
//! post-switch unmatched path stays live) — the correct under-report
//! until the listener forwards the type-variable events
//! (`type_system.dart:868-877`). The cells below therefore pin the
//! faces that hold at this level: the `can_be_equal` implicit-`Object`
//! edge (independent of the bound source) and the unbounded-parameter
//! conservative arm.

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

/// A type-parameter scrutinee under this rig carries no bound source:
/// `Flow::boundaries` is populated by the `begin/end_type_variable`
/// events the production `DiagnoseListener` does not forward to `flow`,
/// so `exhaustive_within` answers the conservative `false` — the
/// post-switch unmatched path stays live and the body end is reachable.
/// This pins the conservative under-report as the current contract (the
/// exhaustive arm is pinned at the flow level, where `Flow` is the
/// listener and the table is populated). When the type-variable events
/// forward, this cell inverts to the oracle answer (`body_might` absent
/// for `T extends St` over an enum).
#[test]
fn type_param_scrutinee_conservative_without_a_bound_source() {
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

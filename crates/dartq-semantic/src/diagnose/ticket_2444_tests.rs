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
        // The same derivation the engine's fold call site makes from
        // the settled set (the `inference-update-2` release gate).
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

fn at(source: &str, needle: &str) -> u32 {
    u32::try_from(
        source
            .find(needle)
            .unwrap_or_else(|| panic!("{needle:?} not in {source:?}")),
    )
    .expect("offset fits u32")
}

/// Exactly one row of `name`, anchored at `offset`.
fn one_at(report: &FileReport, name: &str, offset: u32, source: &str) {
    let rows: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == name)
        .collect();
    assert_eq!(rows.len(), 1, "{name}: {rows:?}");
    assert_eq!(rows[0].range.offset, offset, "in {source:?}");
}

// —— the body-death cut: nullability-disjoint only ————————————————————

/// A genuinely value-disjoint constant never kills its case body: the
/// never-match warning reports at the constant and the body stays
/// reachable (oracle 3.13.1, probe2444 pa — the warning alone, zero
/// `dead_code`; the flow's `_equalityCheck` falls to
/// `_NoEqualityInformation` for a bool-vs-int cross).
#[test]
fn bool_constant_over_int_scrutinee_keeps_body_live() {
    let source = "\
int f(int x) {
  switch (x) {
    case true:
      return 1;
    case 1:
      return 2;
  }
  return 0;
}
";
    let report = unit_report(source);
    one_at(
        &report,
        "constant_pattern_never_matches_value_type",
        at(source, "true"),
        source,
    );
    assert_eq!(
        count_code(&report, "dead_code"),
        0,
        "{:?}",
        report.diagnostics
    );
}

/// Another enum's constant over an enum scrutinee is the same face: the
/// warning reports at the foreign constant and the bodies stay live (oracle
/// 3.13.1, probe2444 ph — the warning alone, zero `dead_code`). The row is
/// asserted by count only: the oracle anchors the full qualified constant
/// (`A.a`, `5|10|3` in the probe) while the parse-time face anchors the
/// inner identifier — a pre-existing wiring offset outside this ticket's
/// face (the release gate compares `(severity, code, file)` triples).
#[test]
fn other_enum_constant_over_enum_scrutinee_keeps_body_live() {
    let source = "\
enum A { a, b }
enum B { x }
void f(B e) {
  switch (e) {
    case A.a:
      print('a');
    case B.x:
      print('b');
  }
}
";
    let report = unit_report(source);
    assert_eq!(
        count_code(&report, "constant_pattern_never_matches_value_type"),
        1,
        "{:?}",
        report.diagnostics
    );
    assert_eq!(
        count_code(&report, "dead_code"),
        0,
        "{:?}",
        report.diagnostics
    );
}

/// The union face needs ALL leaves null: a mixed `null || true` or-pattern
/// over an `int` scrutinee keeps its body live through the non-null leaf
/// (oracle 3.13.1, probe2444 pg — the two never-match warnings, zero
/// `dead_code`).
#[test]
fn mixed_null_or_bool_union_keeps_body_live() {
    let source = "\
void f() {
  int x = 0;
  switch (x) {
    case null || true:
      print('a');
    case 1:
      print('b');
  }
}
";
    let report = unit_report(source);
    let never: Vec<u32> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "constant_pattern_never_matches_value_type")
        .map(|d| d.range.offset)
        .collect();
    assert_eq!(
        never,
        vec![at(source, "null"), at(source, "true")],
        "{:?}",
        report.diagnostics
    );
    assert_eq!(
        count_code(&report, "dead_code"),
        0,
        "{:?}",
        report.diagnostics
    );
}

/// The Null-scrutinee face keeps dying: a non-null constant against a
/// `Null`-typed scrutinee is nullability-disjoint (`_typesAreDisjointDueToNullability`
/// with the `Null` operand on the matched side), so only that case's body
/// dies — a `null` leaf against the same scrutinee is the known-TRUE
/// equality and stays live (oracle 3.13.1, probe2444 pf — the warning at
/// `true` plus one `dead_code` over `return 1`, none over `return 2`).
#[test]
fn constant_over_null_scrutinee_dies_but_null_leaf_stays_live() {
    let source = "\
int f() {
  Null x = null;
  switch (x) {
    case true:
      return 1;
    case null:
      return 2;
  }
}
";
    let report = unit_report(source);
    one_at(
        &report,
        "constant_pattern_never_matches_value_type",
        at(source, "true"),
        source,
    );
    assert_eq!(
        count_code(&report, "dead_code"),
        1,
        "{:?}",
        report.diagnostics
    );
    one_at(&report, "dead_code", at(source, "return 1"), source);
}

// —— the world arithmetic behind the Null faces ———————————————————————

/// The [`crate::switch_tracker::NameWorld::can_be_subtype_of`] Null arms
/// the two dead faces ride (type_system.dart:160-169): `Null` inhabits a
/// non-nullable slot never (the `case null` face) but inhabits the `Null`
/// type itself (the known-true equality — `case null` over `Null` stays
/// live), while a non-null constant inhabits a `Null` slot never (the
/// `case true` over `Null` face).
#[test]
fn world_null_arms_decide_the_two_dead_faces() {
    use crate::switch_tracker::{CORE_SUPERTYPES, NameTy, NameWorld};
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let index = ElementIndex::default();
    let world = NameWorld {
        index: &index,
        file,
        strings: &strings,
        scope: None,
        resolved: std::cell::RefCell::default(),
        bool_id: strings.intern("bool"),
        object_id: strings.intern("Object"),
        enum_id: strings.intern("Enum"),
        primitive_ids: crate::member_face::FINAL_PRIMITIVES.map(|p| strings.intern(p)),
        core_supertypes: CORE_SUPERTYPES.map(|(b, p)| (strings.intern(b), strings.intern(p))),
        unknowns: std::cell::Cell::new(0),
    };
    let int = NameTy::Named(strings.intern("int"), false);
    let null_ty = NameTy::Null;
    // `case null` over `int`: the null leaf cannot inhabit the slot.
    assert!(!world.can_be_subtype_of(&NameTy::Null, &int));
    // `case null` over `Null`: the known-true equality — live.
    assert!(world.can_be_subtype_of(&NameTy::Null, &null_ty));
    // `case true` over `Null`: the bool value cannot inhabit the slot.
    assert!(!world.can_be_subtype_of(&NameTy::BoolValue(true), &null_ty));
}

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

// —— the bound-aware null-aware faces ————————————————————————————————

/// The corpus witness (`lib/src/ast.dart` `_becomeParentOf1<T extends
/// AstNodeImpl?>`): a parameter read whose written bound is nullable
/// keeps `?.` legal — the bound-aware strictly face answers
/// nullable-possible and the row stays silent (probed 3.13.1).
#[test]
fn nullable_bound_param_keeps_null_aware_legal() {
    let source = "\
class AstNodeImpl {
  AstNodeImpl? _parent;
  AstNodeImpl? _becomeParentOf1<T extends AstNodeImpl?>(T child) {
    child?._parent = this;
    return child;
  }
}
";
    let report = unit_report(source);
    assert_eq!(count_code(&report, "invalid_null_aware_operator"), 0);
}

/// An unbounded parameter's implicit bound is `Object?` — nullable-
/// possible — so `?.` over its reads stays legal too (probed 3.13.1).
#[test]
fn unbounded_param_keeps_null_aware_legal() {
    let source = "\
class C {
  int _parent = 0;
  void f<T>(T child) {
    child?._parent;
  }
}
";
    let report = unit_report(source);
    assert_eq!(count_code(&report, "invalid_null_aware_operator"), 0);
}

/// The container's own parameter carries the same bound reach: a method
/// body reading `t?.m` over `class C<T extends C?>` stays silent (the
/// container-capture arm answers when the executable declares no window
/// of its own).
#[test]
fn container_param_bound_reaches_the_member_gate() {
    let source = "\
class C {
  int _parent = 0;
  void m<T extends C?>(T t) {
    t?._parent;
  }
}
";
    let report = unit_report(source);
    assert_eq!(count_code(&report, "invalid_null_aware_operator"), 0);
}

/// A non-nullable bound keeps the strictly face: `?.` over the bound's
/// read reports at the operator (probed 3.13.1).
#[test]
fn non_null_bound_param_still_reports_null_aware() {
    let source = "\
class C {
  int _parent = 0;
  void f<T extends C>(T child) {
    child?._parent;
  }
}
";
    let report = unit_report(source);
    one_at(
        &report,
        "invalid_null_aware_operator",
        at(source, "?."),
        source,
    );
}

/// The index spelling rides the same bound-aware face: `?[` over a
/// non-nullable-bound parameter reports, over a nullable-bound one
/// stays silent (probed 3.13.1).
#[test]
fn index_null_aware_decides_through_the_bound() {
    let source = "\
class C {
  int operator [](int i) => i;
}
void f<T extends C>(T c) {
  c?[0];
}
void g<T extends C?>(T c) {
  c?[0];
}
";
    let report = unit_report(source);
    one_at(
        &report,
        "invalid_null_aware_operator",
        at(source, "?["),
        source,
    );
}

/// `!` over a nullable-bound parameter stays live; over a non-nullable
/// bound it is dead (probed 3.13.1).
#[test]
fn null_assert_decides_through_the_bound() {
    let nullable = "\
class C {
  int _parent = 0;
  void f<T extends C?>(T child) {
    child!._parent;
  }
}
";
    assert_eq!(
        count_code(&unit_report(nullable), "unnecessary_non_null_assertion"),
        0
    );
    let non_null = "\
class C {
  int _parent = 0;
  void f<T extends C>(T child) {
    child!._parent;
  }
}
";
    let report = unit_report(non_null);
    one_at(
        &report,
        "unnecessary_non_null_assertion",
        at(non_null, "!"),
        non_null,
    );
}

// —— the bound-aware null-comparison face ————————————————————————————

/// `t == null` over a non-nullable-bound parameter is constant-false
/// (the bound-aware strictly face reports); the nullable bound suppresses
/// (probed 3.13.1).
#[test]
fn null_comparison_decides_through_the_bound() {
    let non_null = "void f<T extends int>(T t) { if (t == null) {} }";
    let report = unit_report(non_null);
    one_at(
        &report,
        "unnecessary_null_comparison",
        at(non_null, "=="),
        non_null,
    );
    let nullable = "void f<T extends int?>(T t) { if (t == null) {} }";
    assert_eq!(
        count_code(&unit_report(nullable), "unnecessary_null_comparison"),
        0
    );
}

// —— the null-check-pattern face —————————————————————————————————————

/// `var x?` over a non-nullable-bound parameter is dead; over a nullable
/// bound it stays live (probed 3.13.1).
#[test]
fn null_check_pattern_decides_through_the_bound() {
    let non_null = "\
void f<T extends int>(T t) {
  switch (t) {
    case var x?:
      break;
  }
}
";
    let report = unit_report(non_null);
    // The oracle anchors the row at the `?` token (`probe_w` 4:15, the
    // question of `var x?`), not the binding's name.
    one_at(
        &report,
        "unnecessary_null_check_pattern",
        at(non_null, "x?") + 1,
        non_null,
    );
    let nullable = "\
void f<T extends int?>(T t) {
  switch (t) {
    case var x?:
      break;
  }
}
";
    assert_eq!(
        count_code(&unit_report(nullable), "unnecessary_null_check_pattern"),
        0
    );
}

// —— the never-match gates ———————————————————————————————————————————

/// A constant outside the bound's family can never equal a matched
/// parameter value: the bound stands in for the comparison (`int` vs
/// `T extends Color`) and the row reports at the constant (probed
/// 3.13.1).
#[test]
fn constant_outside_param_bound_reports_never_match() {
    let source = "\
enum Color { red, green }
void f<T extends Color>(T x) {
  switch (x) {
    case 1:
      break;
  }
}
";
    let report = unit_report(source);
    one_at(
        &report,
        "constant_pattern_never_matches_value_type",
        at(source, "1"),
        source,
    );
}

/// A constant inside the bound's family can equal it — `Color.red` vs
/// `T extends Color` stays silent (probed 3.13.1).
#[test]
fn constant_within_param_bound_stays_possible() {
    let source = "\
enum Color { red, green }
void f<T extends Color>(T x) {
  switch (x) {
    case Color.red:
      break;
  }
}
";
    assert_eq!(
        count_code(
            &unit_report(source),
            "constant_pattern_never_matches_value_type"
        ),
        0
    );
}

/// An unbounded parameter can equal anything (`Object?` bound): the
/// never-match row cannot arm (probed 3.13.1).
#[test]
fn unbounded_param_scrutinee_cannot_prove_disjoint() {
    let source = "\
void f<T>(T x) {
  switch (x) {
    case 1:
      break;
  }
}
";
    assert_eq!(
        count_code(
            &unit_report(source),
            "constant_pattern_never_matches_value_type"
        ),
        0
    );
}

/// A bound that itself mentions a parameter (`T extends S`) proves no
/// disjointness (the official's `hasTypeParameterReference` gate; probed
/// 3.13.1).
#[test]
fn param_mentioning_bound_cannot_prove_disjoint() {
    let source = "\
void f<S, T extends S>(T x) {
  switch (x) {
    case 1:
      break;
  }
}
";
    assert_eq!(
        count_code(
            &unit_report(source),
            "constant_pattern_never_matches_value_type"
        ),
        0
    );
}

/// The `null` constant against a non-nullable-bound parameter: `Null`
/// never equals the bound's family, the row reports (probed 3.13.1).
#[test]
fn null_constant_against_param_bound_reports() {
    let source = "\
enum Color { red }
void f<T extends Color>(T x) {
  switch (x) {
    case null:
      break;
  }
}
";
    let report = unit_report(source);
    one_at(
        &report,
        "constant_pattern_never_matches_value_type",
        at(source, "null"),
        source,
    );
}

/// The reader-family shape: a parameter the call already solved to a
/// concrete type (`T := Color`) spells no parameter at the comparison —
/// the int-vs-enum disjoint verdict survives (#2402's substitution).
#[test]
fn solved_generic_call_return_still_reports_never_match() {
    let source = "\
enum Color { red, green }
Color readEnum<T extends Color>(T x) => x;
void f() {
  Color c = readEnum(Color.red);
  switch (c) {
    case 1:
      break;
  }
}
";
    let report = unit_report(source);
    one_at(
        &report,
        "constant_pattern_never_matches_value_type",
        at(source, "1"),
        source,
    );
}

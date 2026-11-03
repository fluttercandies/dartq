//! The #3690 pins — the `pattern_type_mismatch_in_irrefutable_context`
//! over-report faces (census-12 lane P: 59 missing / 268 extra).
//!
//! # Why the extra side dominated
//!
//! Upstream reports this code from four check sites, each guarded by
//! `irrefutableContext != null` plus its own silence gate (upstream Dart
//! SDK source, `pkg/_fe_analyzer_shared/lib/src/type_inference/type_analyzer.dart`
//! `analyzeDeclaredVariablePattern` / `analyzeAssignedVariablePattern` /
//! `analyzeObjectPattern` / `analyzeRecordPattern` / `analyzeListPattern` /
//! `analyzeMapPattern`). Two of those gates were missing here, and one
//! column fed the walk a fabricated type:
//!
//! 1. **`dynamic` is never a mismatch.** Every check site spells
//!    `matchedValueType is! SharedDynamicType` (and `is! SharedInvalidType`)
//!    before asking assignability. The walk now leaves a `dynamic` matched
//!    face silent instead of routing it through the lattice — which under
//!    `strict-casts` would have answered `false` and fabricated a row.
//! 2. **An unresolved object-pattern getter reads `dynamic`.** Upstream's
//!    `resolveObjectPatternPropertyGet` answers `typeProvider.dynamicType`
//!    when the getter does not resolve
//!    (`pkg/analyzer/lib/src/generated/resolver.dart:1644-1693`); this lane
//!    previously handed the child `Object?`, so `Square(areaAsList: [a])`
//!    against the undeclared-in-model `areaAsList` reported
//!    `Object? isn't assignable to List<Object?>`.
//! 3. **A `: pattern` field infers its name from the variable pattern.**
//!    `PatternFieldNameImpl.name` is `nameNode.name ?? pattern.variablePattern?.name`
//!    (upstream Dart SDK source, `pkg/analyzer/lib/src/dart/ast/ast.dart:25717`),
//!    and the object-pattern getter column resolves through that same name
//!    (`resolver.dart:1650-1651`). `Square(:areaAsInt)` therefore reads the
//!    `areaAsInt` column, not `dynamic`.
//! 4. **A mismatched record shape hands children `Object?` only per the
//!    official `_matchRecordTypeShape` gate** (`type_analyzer.dart:2935-2985`):
//!    every named field must exist on the matched record, the positional
//!    fields must pair one-for-one, and the record must carry no extra
//!    field. Duplicate named fields contribute one required slot, not two.
//!
//! Oracle provenance: `dart analyze` 3.13.7 rows in
//! `conformance/expected.tsv` — the co19 `LanguageFeatures/Patterns`
//! fixtures (`object_A01_t09`, `object_A02_t07/t08`, `record_A02_t02/t04`,
//! `record_A03_t01`, `pattern_assignment_A05_t01`, `matching_record_A01_t01`)
//! and the analyzer suite's
//! `PatternTypeMismatchInIrrefutableContextTest`.

use super::test_rig::{code_names, report_ops, rows};
use crate::diagnose::FileReport;

/// The `pattern_type_mismatch_in_irrefutable_context` rows as
/// `(offset, length, message)` triples, ascending.
fn mismatch_rows(report: &FileReport) -> Vec<(u32, u32, String)> {
    let mut rows: Vec<(u32, u32, String)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "pattern_type_mismatch_in_irrefutable_context")
        .map(|d| {
            (
                d.range.offset,
                d.range.end() - d.range.offset,
                d.message.to_string(),
            )
        })
        .collect();
    rows.sort_unstable();
    rows
}

/// `needle`'s absolute offset inside `fixture` as the rig analyzes it (the
/// prelude shifts every fixture-local offset).
fn at(fixture: &str, needle: &str) -> u32 {
    let hit = fixture
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not in fixture:\n{fixture}"));
    u32::try_from(super::test_rig::CORE_PRELUDE.len() + hit).expect("fixture offsets fit u32")
}

/// The `Shape`/`Square` fixture the co19 pattern helper declares
//   (`patterns_lib.dart`'s `Shape<T extends MetricUnits>` getter set).
const SHAPES: &str = "abstract class MetricUnits {}\n\
     class Meter extends MetricUnits {}\n\
     abstract class Shape<T extends MetricUnits> {\n\
     \x20 int get areaAsInt => 0;\n\
     \x20 double get areaAsDouble => 0.0;\n\
     \x20 List<num> get areaAsList => const <num>[];\n\
     }\n\
     class Square<T extends MetricUnits> extends Shape<T> {}\n\
     class Circle<T extends MetricUnits> extends Shape<T> {}\n";

// —— extra-side repairs: the refutable/silent faces ——

/// A `dynamic` scrutinee never reports: a declaration pattern's own check
/// site guards `matchedValueType is! SharedDynamicType`
/// (`analyzeDeclaredVariablePattern`), so a typed leaf over `dynamic`
/// stays silent even though `dynamic` is not a subtype of `int`.
///
/// Oracle (`matching_record_A01_t01`, `void test4(dynamic o) { var (int
/// v1, num _) = o; }`): the file's only row is the unused-variable warning,
/// no `pattern_type_mismatch_in_irrefutable_context`.
#[test]
fn dynamic_scrutinee_types_leaf_stays_silent() {
    let report = report_ops("void f(dynamic o) {\n  var (int v1, num _) = o;\n}\n");
    assert!(
        !code_names(&report)
            .contains(&"pattern_type_mismatch_in_irrefutable_context"),
        "a `dynamic` matched type is upstream's own silence gate: {:#?}",
        rows(&report)
    );
}

/// The record node over a `dynamic` scrutinee is silent too (the record
/// check site carries the same `is! SharedDynamicType` guard), and its
/// children are dispatched `dynamic` (`dispatchFields(operations.dynamicType)`).
///
/// Oracle (`matching_record_A01_t01`): silent.
#[test]
fn dynamic_scrutinee_record_node_stays_silent() {
    let report = report_ops("void f(dynamic o) {\n  var (a, int b) = o;\n}\n");
    assert_eq!(mismatch_rows(&report), vec![], "{:#?}", rows(&report));
}

/// An unresolvable object-pattern getter reads `dynamic`, not `Object?`
/// (`resolveObjectPatternPropertyGet` answers `typeProvider.dynamicType`
/// when the name does not resolve). The child list pattern therefore sees
/// `dynamic` and stays silent.
///
/// Oracle (`object_A01_t09`, `var Square(areaAsList: [a1]) = Square(2);`
/// with `areaAsList` a `List<num>` getter): silent — the getter column
/// resolves and `List<num>` fits `List<Object?>`. The model here declares
/// no such getter, so the column is `dynamic` (upstream's own fallback for
/// a missing name) and the row must still not appear.
#[test]
fn unresolved_getter_column_is_dynamic_not_object() {
    let fixture = "abstract class MetricUnits {}\n\
                   class Meter extends MetricUnits {}\n\
                   class Shape<T extends MetricUnits> {}\n\
                   class Square<T extends MetricUnits> extends Shape<T> {}\n\
                   void f() {\n\
                   \x20 var Square(areaAsList: [a1]) = Square<Meter>();\n\
                   }\n";
    let report = report_ops(fixture);
    assert!(
        !code_names(&report)
            .contains(&"pattern_type_mismatch_in_irrefutable_context"),
        "an unresolved getter column is `dynamic`, never `Object?`: {:#?}",
        rows(&report)
    );
}

/// A `: pattern` field infers its name from the variable pattern, so
/// `Square(:areaAsList)` reads the `areaAsList` column instead of falling
/// back to `dynamic` — and, being `List<num>`, reports nothing.
///
/// Oracle (`object_A02_t08`'s `: double areaAsDouble` shape, and the
/// `Square(:areaAsInt)` shapes of `execution_pattern_for_element_A01_t05`):
/// silent.
#[test]
fn shorthand_field_infers_the_variable_name() {
    let report = report_ops(&format!(
        "{SHAPES}void f() {{\n  var Square(:areaAsList) = Square<Meter>();\n}}\n"
    ));
    assert!(
        !code_names(&report)
            .contains(&"pattern_type_mismatch_in_irrefutable_context"),
        "`:name` infers `name`, so the getter column resolves: {:#?}",
        rows(&report)
    );
}

/// The same inference reaching a populated column: `Square(:areaAsInt)`
/// over `Square<Meter>` reads `int`, and a `: int` leaf accepts it.
///
/// Oracle (`object_A02_t09`, `var Square(:(int areaAsInt)) = Square(1);`
/// and the `:int areaAsInt` shapes): silent.
#[test]
fn shorthand_field_infers_the_name_for_a_typed_leaf() {
    let report = report_ops(&format!(
        "{SHAPES}void f() {{\n  var Square(:int areaAsInt) = Square<Meter>();\n}}\n"
    ));
    assert_eq!(mismatch_rows(&report), vec![], "{:#?}", rows(&report));
}

// —— extra-side repairs: the record-shape gate ——

/// A record pattern whose shape matches reads its fields' real types: a
/// `var (int a, String b) = (1, 's');` over a matching `(int, String)`
/// initializer reports nothing.
///
/// Oracle (`record_A02_t04`'s matched-shape block): silent.
#[test]
fn matching_record_shape_reads_the_real_slots() {
    let report = report_ops("void f() {\n  var (int a, String b) = (1, 's');\n}\n");
    assert_eq!(mismatch_rows(&report), vec![], "{:#?}", rows(&report));
}

/// A record pattern over a NON-record matched face hands every child
/// `Object?` (`dispatchFields(operations.objectQuestionType)`), and every
/// `Object?`-typed leaf then reports its own mismatch.
///
/// Oracle (`record_A02_t03`/`record_A02_t04`'s not-a-record shapes,
/// `test_recordPattern_notRecord`): one row per typed leaf.
#[test]
fn record_over_non_record_dispatches_object_question_to_children() {
    let fixture = "void f(int x) {\n  var (int a,) = x;\n}\n";
    let report = report_ops(fixture);
    let got = mismatch_rows(&report);
    assert_eq!(got.len(), 2, "{:#?}", rows(&report));
    assert_eq!(got[0].0, at(fixture, "(int a,)"), "{got:#?}");
    assert_eq!(
        got[0].2,
        "The matched value of type 'int' isn't assignable to the required type '(Object?,)'."
    );
    assert_eq!(
        got[1].2,
        "The matched value of type 'Object?' isn't assignable to the required type 'int'."
    );
}

/// A record pattern whose matched record carries a field the pattern omits
/// fails `_matchRecordTypeShape`'s completion leg (`namedCount !=
/// matchedType.namedTypes.length`), so its children read `Object?` — and a
/// typed leaf over it reports.
///
/// Oracle (`record_A03_t01`'s `var (n1: a1, b1, n1: c1)` family): the
/// children carry `Object?`.
#[test]
fn record_with_extra_matched_field_dispatches_object_question() {
    let fixture = "void f() {\n  var (int a,) = (1, 's');\n}\n";
    let report = report_ops(fixture);
    let got = mismatch_rows(&report);
    assert_eq!(got.len(), 2, "{:#?}", rows(&report));
    assert_eq!(
        got[1].2,
        "The matched value of type 'Object?' isn't assignable to the required type 'int'."
    );
}

/// A DUPLICATE named field contributes one required slot, not two
/// (`analyzeRecordPattern`'s `requiredTypeNamedTypes` skips a field whose
/// index carries a `duplicate_record_pattern_field` error), so the NODE
/// itself is silent — but `_matchRecordTypeShape` still counts every
/// named field, so the shape gate fails and each typed leaf reads
/// `Object?`.
///
/// Oracle (`record_A03_t01`, `var (n7: int a7, int b7, n7: int c7) =
/// (n7: 1, 2);`): the node carries no row; `Object? isn't assignable to
/// int` lands at each of the three leaves.
#[test]
fn duplicate_named_field_node_is_silent_and_leaves_read_object() {
    let fixture = "void f() {\n  var (n7: int a7, int b7, n7: int c7) = (n7: 1, 2);\n}\n";
    let report = report_ops(fixture);
    let got = mismatch_rows(&report);
    assert_eq!(got.len(), 3, "{:#?}", rows(&report));
    for (offset, _, message) in &got {
        assert_eq!(
            message,
            "The matched value of type 'Object?' isn't assignable to the required type 'int'."
        );
        assert_ne!(*offset, at(fixture, "(n7:"), "the node itself stays silent");
    }
}

/// A `:name` record field is NAMED, not positional: `(:n1, :n2)` requires
/// `(Object?, {Object? n2}? …)` — the shape that matches the record literal
/// `(n1: …, n2: …)` — so the node is silent.
///
/// Oracle (`record_A02_t02`, `var (:n1 as num) = (n1: 42);`): silent.
#[test]
fn shorthand_named_record_field_is_named_not_positional() {
    let report = report_ops("void f() {\n  var (:n1 as num) = (n1: 42);\n}\n");
    assert!(
        !code_names(&report)
            .contains(&"pattern_type_mismatch_in_irrefutable_context"),
        "`:n1` is a named field: {:#?}",
        rows(&report)
    );
}

// —— genuine errors (the miss side, pins with oracle-verbatim text) ——

/// `test_declaredVariablePattern_valueSupertype`: `num` is not assignable
/// to `int`, so `var (int a) = x;` over a `num` reports at the leaf.
///
/// Oracle (`PatternTypeMismatchInIrrefutableContextTest`): message verbatim.
#[test]
fn typed_leaf_over_a_wider_scrutinee_reports() {
    let fixture = "void f(num x) {\n  var (int a) = x;\n}\n";
    let report = report_ops(fixture);
    let got = mismatch_rows(&report);
    assert_eq!(got.len(), 1, "{:#?}", rows(&report));
    assert_eq!(got[0].0, at(fixture, "int a"), "{got:#?}");
    assert_eq!(got[0].1, 5);
    assert_eq!(
        got[0].2,
        "The matched value of type 'num' isn't assignable to the required type 'int'."
    );
}

/// `test_objectPattern_differentClass`: an object pattern's written head
/// over an unrelated matched class reports at the head's span with the
/// verbatim message.
///
/// Oracle (`PatternTypeMismatchInIrrefutableContextTest`).
#[test]
fn object_pattern_head_mismatch_reports() {
    let fixture = "void f(Object x) {\n  var String(length: a) = x;\n}\n";
    let report = report_ops(fixture);
    let got = mismatch_rows(&report);
    assert_eq!(got.len(), 1, "{:#?}", rows(&report));
    assert_eq!(
        got[0].2,
        "The matched value of type 'Object' isn't assignable to the required type 'String'."
    );
}

/// The getter column DOES report a genuine mismatch: `Square(:areaAsInt)`
/// read through `int` against a `double`-typed leaf reports the leaf.
///
/// Oracle (`object_A02_t08`'s typed-leaf-over-getter shape reports the
/// leaf's own span with the getter's type as matched).
#[test]
fn getter_column_mismatch_reports_the_leaf() {
    let fixture = format!(
        "{SHAPES}void f() {{\n  var Square(:double areaAsInt) = Square<Meter>();\n}}\n"
    );
    let report = report_ops(&fixture);
    let got = mismatch_rows(&report);
    assert_eq!(got.len(), 1, "{:#?}", rows(&report));
    assert_eq!(
        got[0].2,
        "The matched value of type 'int' isn't assignable to the required type 'double'."
    );
}

// —— negative controls ——

/// A refutable context (`switch` case, `if-case`) never checks the
/// irrefutable face, whatever the shapes: a mismatched typed leaf there is
/// legal and silent.
#[test]
fn refutable_contexts_stay_silent() {
    for src in [
        "void f(num x) {\n  switch (x) {\n    case (int a,):\n      break;\n  }\n}\n",
        "void f(num x) {\n  if (x case (int a,)) {}\n}\n",
        "void f(Object? x) {\n  switch (x) {\n    case String(:length: int n):\n      break;\n  }\n}\n",
    ] {
        let report = report_ops(src);
        assert!(
            !code_names(&report)
                .contains(&"pattern_type_mismatch_in_irrefutable_context"),
            "{src:?} rows: {:#?}",
            rows(&report)
        );
    }
}

/// A matching irrefutable pattern stays silent: an assignable leaf, a
/// `dynamic` leaf, and an inferred (untyped) binder all pass.
#[test]
fn agreeing_irrefutable_patterns_stay_silent() {
    for src in [
        "void f(int x) {\n  var (num a) = x;\n}\n",
        "void f(dynamic x) {\n  var (int a) = x;\n}\n",
        "void f() {\n  var (int a, String b) = (1, 's');\n}\n",
        "void f() {\n  var (a, b) = (1, 's');\n}\n",
    ] {
        let report = report_ops(src);
        assert_eq!(mismatch_rows(&report), vec![], "{src:?}: {:#?}", rows(&report));
    }
}

/// A source with no pattern at all is untouched — the guard for the walk
/// being reachable only from the three irrefutable context shapes.
#[test]
fn no_pattern_no_rows() {
    let report = report_ops("int f() {\n  final x = 1 + 2;\n  return x;\n}\n");
    assert_eq!(mismatch_rows(&report), vec![], "{:#?}", rows(&report));
}

// —— census-shape regressions (verbatim shapes from the 268-extra side) ——

/// `object_A01_t09` shape: `var Square(areaAsList: [num c1]) = Square(2);`
/// with the real `List<num> get areaAsList` getter declared — the getter
/// column resolves to `List<num>`, the list node requires
/// `List<Object?>`, and `List<num>` fits it. Nothing reports.
///
/// Oracle (`object_A01_t09`): silent.
#[test]
fn census_object_a01_t09_nonNullable_list_getter_is_silent() {
    let report = report_ops(&format!(
        "{SHAPES}void f() {{\n  var Square(areaAsList: [num c1]) = Square<Meter>();\n}}\n"
    ));
    assert_eq!(mismatch_rows(&report), vec![], "{:#?}", rows(&report));
}

/// `record_A02_t08`'s nested-record shape, well formed: a record pattern
/// whose matched record has the same shape reads every slot's real type,
/// so a nested record subpattern compares against its own real column
/// rather than `Object?`.
///
/// Oracle (`record_A02_t08` lines 43/53 — `var ((int a7,),) = ((-1,),);`):
/// silent.
#[test]
fn census_record_a02_t08_nested_record_shape_is_silent() {
    let report = report_ops("void f() {\n  var ((int a,),) = ((-1,),);\n}\n");
    assert_eq!(mismatch_rows(&report), vec![], "{:#?}", rows(&report));
    let report = report_ops("void f() {\n  var (n: (int x, int y)) = (n: (42, -1));\n}\n");
    assert_eq!(mismatch_rows(&report), vec![], "{:#?}", rows(&report));
}

/// `object_A02_t08` shape: `var Square(: double areaAsDouble) = Square(3);`
/// — the `:name` inference reads the `areaAsDouble` getter (`double`) and
/// the `double` leaf accepts it.
///
/// Oracle (`object_A02_t08`): silent.
#[test]
fn census_object_a02_t08_shorthand_typed_leaf_is_silent() {
    let report = report_ops(&format!(
        "{SHAPES}void f() {{\n  var Square(: double areaAsDouble) = Square<Meter>();\n}}\n"
    ));
    assert_eq!(mismatch_rows(&report), vec![], "{:#?}", rows(&report));
}

/// `pattern_assignment_A05_t01` shape: an explicit `name: pattern` record
/// field in an assignment keeps its named slot, so `(n1: af, _) = (n1: 1,
/// 2);` requires `(Object?, {Object? n1})` and the matched
/// `(int, {int n1})` fits it — the node is silent and only `af`'s own
/// `int` leaf is checked (and it agrees).
///
/// Oracle (`pattern_assignment_A05_t01` line 71): silent.
#[test]
fn census_pattern_assignment_a05_explicit_named_field_is_silent() {
    let report = report_ops("void f() {\n  int af = 0;\n  (n1: af, _) = (n1: 1, 2);\n}\n");
    assert_eq!(mismatch_rows(&report), vec![], "{:#?}", rows(&report));
}

/// The same shape with the name omitted (`:af`): the inferred name is
/// `af`, so the required record keeps one named slot.
///
/// Oracle (`pattern_assignment_A05_t01` line 76, `(:af, _) = (af: 1, 2);`):
/// silent.
#[test]
fn census_pattern_assignment_a05_shorthand_field_is_silent() {
    let report = report_ops("void f() {\n  int af = 0;\n  (:af, _) = (af: 1, 2);\n}\n");
    assert_eq!(mismatch_rows(&report), vec![], "{:#?}", rows(&report));
}

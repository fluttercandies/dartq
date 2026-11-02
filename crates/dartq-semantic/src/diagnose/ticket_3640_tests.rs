//! The #3640 pins — the S1-A irrefutable-walk faces: the shared
//! `irrefutable_walk_between` entry point, the three extra-family
//! verdict-hygiene fixes, and the refutable-shape emission.
//!
//! # The faces
//!
//! - **Refutable shapes** inside an irrefutable context report
//!   `refutable_pattern_in_irrefutable_context` at the node's own extent
//!   and silence the subtree (`analyzeLogicalOrPattern`,
//!   `analyzeRelationalPattern`, `analyzeNullCheckOrAssertPattern` —
//!   `_fe_analyzer_shared`'s `type_analyzer.dart`, each reporting first
//!   and then `makeRefutable()`).
//! - **Keyword leaves**: a leading `var`/`final` is the variable
//!   pattern's modifier, never a type head (`final int _` requires
//!   `int`; a bare `var x` infers and stays silent).
//! - **Undecidable verdicts**: a scope-parameter matched type answers
//!   `None`-silent — upstream `isSubtypeOf` is undecidable there, so the
//!   oracle reports nothing.
//! - **Rest columns**: a list pattern's rest element sees the LIST
//!   (`List<E>`), never the element (`analyzeListPattern`'s
//!   `subPatternMatchedType = requiredType`).
//!
//! Oracle provenance: `dart analyze` 3.13.7 — the analyzer suite's
//! `refutable_pattern_in_irrefutable_context_test.dart` (declaration
//! shapes `(_ || _)`, `(_?)`, `(> 0)`) and co19's
//! `LanguageFeatures/Patterns` fixtures for the typed-leaf and rest
//! columns.

use super::test_rig::{code_names, report_ops};
use crate::diagnose::FileReport;

/// The `refutable_pattern_in_irrefutable_context` rows as
/// `(offset, length)` pairs, ascending.
fn refutable_rows(report: &FileReport) -> Vec<(u32, u32)> {
    let mut rows: Vec<(u32, u32)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "refutable_pattern_in_irrefutable_context")
        .map(|d| (d.range.offset, d.range.end() - d.range.offset))
        .collect();
    rows.sort_unstable();
    rows
}

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

/// `needle`'s absolute offset inside `fixture` as the rig analyzes it
/// (the prelude shifts every fixture-local offset).
fn at(fixture: &str, needle: &str) -> u32 {
    let hit = fixture
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not in fixture:\n{fixture}"));
    u32::try_from(super::test_rig::CORE_PRELUDE.len() + hit).expect("fixture offsets fit u32")
}

/// `var (_ || _) = 0;` reports the logical-or pattern at its own extent
/// (`_ || _`) — the parser's constant face never sees this shape.
#[test]
fn logical_or_pattern_reports_at_its_extent() {
    let fixture = "void f() {\n  var (_ || _) = 0;\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        refutable_rows(&report),
        vec![(at(fixture, "_ || _"), 6)],
        "{:#?}",
        report.diagnostics
    );
    let row = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "refutable_pattern_in_irrefutable_context")
        .expect("row");
    assert_eq!(row.severity, dartq_diagnostics::Severity::Error);
    assert_eq!(
        row.message.as_str(),
        "Refutable patterns can't be used in an irrefutable context."
    );
    assert_eq!(
        row.correction.as_deref(),
        Some("Try using an if-case, a 'switch' statement, or a 'switch' expression instead.")
    );
}

/// `var (_?) = x;` reports the null-check pattern at `_?` and leaves the
/// inner wildcard silent (`makeRefutable()` after the report).
#[test]
fn null_check_pattern_reports_at_the_operator() {
    let fixture = "void f(int? x) {\n  var (_?) = x;\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        refutable_rows(&report),
        vec![(at(fixture, "_?) = x"), 2)],
        "{:#?}",
        report.diagnostics
    );
}

/// `var (> 0) = 0;` reports the relational pattern at `> 0`.
#[test]
fn relational_pattern_reports_at_its_extent() {
    let fixture = "void f() {\n  var (> 0) = 0;\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        refutable_rows(&report),
        vec![(at(fixture, "> 0) = 0"), 3)],
        "{:#?}",
        report.diagnostics
    );
}

/// A typed object head's `<…>` argument group is not a relational
/// operator: `Square<C>(…)` in an irrefutable context adds no refutable
/// row (and the head's own mismatch face is the ordinary one).
#[test]
fn typed_object_head_is_not_a_relational_pattern() {
    let fixture = "class Square<C> { const Square(int v); }\n\
                   void f() {\n  var Square<int>(:int v) = Square<int>(1);\n}\n";
    let report = report_ops(fixture);
    assert!(
        refutable_rows(&report).is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

/// `final int _` requires `int` — the `final` keyword is the variable
/// pattern's modifier, never a type head (`required type 'final'` was
/// the pre-#3640 extra).
#[test]
fn keyword_leaf_is_not_a_type_head() {
    let fixture = "void f() {\n  final (int a, _) = (1, 2);\n  var (b, _) = (1, 2);\n}\n";
    let report = report_ops(fixture);
    assert!(
        mismatch_rows(&report).is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

/// A scope-parameter matched type answers `None`-silent: upstream
/// `isSubtypeOf` cannot decide `T <: int`, so the oracle reports nothing
/// where the pre-#3640 name-level fallback fabricated a row.
#[test]
fn scope_param_matched_type_stays_silent() {
    let fixture = "void f<T>(T t) {\n  var (int a) = t;\n}\n";
    let report = report_ops(fixture);
    assert!(
        mismatch_rows(&report).is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

/// A list pattern's rest element sees the LIST column, not the element:
/// `[...int r]` over `<int>[]` asks `List<int> <: int` — a provable
/// mismatch — where the pre-#3640 element column answered `int <: int`
/// and stayed silent.
#[test]
fn rest_element_sees_the_list_column() {
    let fixture = "void f() {\n  var [...int r] = <int>[1];\n}\n";
    let report = report_ops(fixture);
    let rows = mismatch_rows(&report);
    assert_eq!(rows.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(rows[0].0, at(fixture, "int r"), "{rows:#?}");
    assert!(
        rows[0].2.contains("'List<int>'") && rows[0].2.contains("'int'"),
        "{rows:#?}"
    );
}

/// The declared-variable leaf's own span stays the written type through
/// the variable name, and a matching matched type stays silent.
#[test]
fn typed_leaf_matching_the_matched_type_is_silent() {
    let fixture = "void f() {\n  var (int a, String b) = (1, 'x');\n}\n";
    let report = report_ops(fixture);
    assert!(
        mismatch_rows(&report).is_empty(),
        "{:#?}",
        report.diagnostics
    );
    assert!(
        !code_names(&report)
            .iter()
            .any(|name| name.starts_with("refutable_pattern")),
        "{:#?}",
        report.diagnostics
    );
}

/// A record node over a narrower matched record reports at the record's
/// own extent, and each field's slot follows the matched record's shape
/// (`analyzeRecordPattern`'s positional walk).
#[test]
fn record_node_and_slot_columns_report() {
    let fixture = "void f() {\n  var (int a, int b) = ('x', 2);\n}\n";
    let report = report_ops(fixture);
    let rows = mismatch_rows(&report);
    assert_eq!(rows.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(rows[0].0, at(fixture, "int a"), "{rows:#?}");
}

// —— record / object field columns (co19 `record_A04_t01`, `object_A02_t07`
// —— shapes; the declaration context the walk owns) ——————————————————————

/// `var (:_) = (x1: 1);` — the record node requires `(Object?,)` (a
/// name-less field counts positionally, `analyzeRecordPattern`) and the
/// wildcard field adds nothing: wildcards carry no irrefutable check.
#[test]
fn record_node_with_wildcard_field_reports_only_the_node() {
    let fixture = "void f() {\n  var (:_) = (x1: 1);\n}\n";
    let report = report_ops(fixture);
    let rows = mismatch_rows(&report);
    assert_eq!(rows.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(rows[0].0, at(fixture, "(:_)"), "{rows:#?}");
    assert!(
        rows[0].2.contains("'({int x1})'") && rows[0].2.contains("'(Object?,)'"),
        "{rows:#?}"
    );
    assert!(refutable_rows(&report).is_empty(), "{:#?}", report.diagnostics);
}

/// `var (:int _) = (x3: 1);` — the record node reports, and the typed
/// wildcard field reports its own `Object?`-slot column at `int _`.
#[test]
fn record_typed_wildcard_field_reports_its_column() {
    let fixture = "void f() {\n  var (:int _) = (x3: 1);\n}\n";
    let report = report_ops(fixture);
    let rows = mismatch_rows(&report);
    assert_eq!(rows.len(), 2, "{:#?}", report.diagnostics);
    assert_eq!(rows[0].0, at(fixture, "(:int _)"), "{rows:#?}");
    assert_eq!(rows[1].0, at(fixture, "int _)"), "{rows:#?}");
    assert!(
        rows[1].2.contains("'Object?'") && rows[1].2.contains("'int'"),
        "{rows:#?}"
    );
}

/// `var (:_?) = (x4: 1);` — the null-check field is refutable inside the
/// irrefutable record context and reports at `_?`; `_!` stays silent.
#[test]
fn record_null_check_field_is_refutable_and_null_assert_is_not() {
    let fixture = "void f() {\n  var (:_?) = (x4: 1);\n  var (:_!) = (x7: 1);\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        refutable_rows(&report),
        vec![(at(fixture, "_?) = (x4"), 2)],
        "{:#?}",
        report.diagnostics
    );
}

/// `var (:_ as int) = (x10: 1);` — a cast tail re-dispatches the inner
/// wildcard with the cast target and adds no row of its own.
#[test]
fn record_cast_field_adds_no_row() {
    let fixture = "void f() {\n  var (:_ as int) = (x10: 1);\n}\n";
    let report = report_ops(fixture);
    let rows = mismatch_rows(&report);
    assert_eq!(rows.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(rows[0].0, at(fixture, "(:_ as int)"), "{rows:#?}");
}

/// `var Rectangle(: > 5) = Rectangle(1, 2);` — a relational field value is
/// refutable at its own extent, and the object node itself stays clean
/// (`Rectangle <: Rectangle`).
#[test]
fn object_relational_field_is_refutable() {
    let fixture = "class Rectangle { const Rectangle(int w, int h); }\n\
                   void f() {\n  var Rectangle(: > 5) = Rectangle(1, 2);\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        refutable_rows(&report),
        vec![(at(fixture, "> 5) = Rectangle"), 3)],
        "{:#?}",
        report.diagnostics
    );
    assert!(mismatch_rows(&report).is_empty(), "{:#?}", report.diagnostics);
}

/// A `||` object field value reports the whole logical-or node and
/// silences both operands.
#[test]
fn object_logical_or_field_is_refutable() {
    let fixture = "class Rectangle { const Rectangle(int w, int h); }\n\
                   void f() {\n  var Rectangle(: var a || var a) = Rectangle(1, 2);\n}\n";
    let report = report_ops(fixture);
    let rows = refutable_rows(&report);
    assert_eq!(rows.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(rows[0].0, at(fixture, "var a || var a"), "{rows:#?}");
    assert_eq!(rows[0].1, "var a || var a".len() as u32, "{rows:#?}");
}

// —— the S1-B wiring (#3666): the two listener call sites reaching
// —— `irrefutable_walk_pattern_assignment` / `irrefutable_walk_for_in_head`

/// A pattern assignment's left side walks the same irrefutable context:
/// `(a, b) = ('x', 2)` with `a: int` reports the assigned-variable leaf
/// `a` — `String <: int` fails against the variable's declared type.
#[test]
fn pattern_assignment_left_side_reports_the_mismatched_leaf() {
    let fixture = "void f() {\n  int a = 0;\n  int b = 0;\n  (a, b) = ('x', 2);\n}\n";
    let report = report_ops(fixture);
    let rows = mismatch_rows(&report);
    assert_eq!(rows.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(rows[0].0, at(fixture, "a, b) ="), "{rows:#?}");
    assert!(
        rows[0].2.contains("'String'") && rows[0].2.contains("'int'"),
        "{rows:#?}"
    );
}

/// A for-in pattern head walks the same irrefutable context with the
/// iterable's element type as the matched type: `var (int v) in
/// <String>[…]` asks `String <: int` at the typed leaf `int v`.
#[test]
fn for_in_pattern_head_reports_against_the_element_type() {
    let fixture = "void f() {\n  for (var (int v) in <String>['x']) {}\n}\n";
    let report = report_ops(fixture);
    let rows = mismatch_rows(&report);
    assert_eq!(rows.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(rows[0].0, at(fixture, "int v"), "{rows:#?}");
    assert_eq!(rows[0].1, 5, "{rows:#?}");
    assert!(
        rows[0].2.contains("'String'") && rows[0].2.contains("'int'"),
        "{rows:#?}"
    );
}

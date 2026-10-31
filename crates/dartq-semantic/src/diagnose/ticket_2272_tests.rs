//! The #2272 pins — the `?.`/`?..`/`?[` null-shorting RESULT-TYPE face
//! (the triple-burn U1 lane): upstream lifts a null-aware chain's static
//! type at `handleNullShortingFinished` (`ExpressionImpl.recordNullShortedType`,
//! upstream Dart SDK source,
//! pkg/analyzer/lib/src/generated/resolver.dart:1196-1200) — the `?.`
//! member result is `memberType` made nullable, the `?..` cascade result
//! is the TARGET's own type (`visitCascadeExpression`,
//! static_type_analyzer.dart:70-72 — `node.target.typeOrThrow`), the `?[`
//! result is the nullable element type, and every continuation the
//! operator guards inherits the lift (`a?.b.c` types `C?`,
//! `a?[0].b` types `B?`).
//!
//! The result type is observed through its downstream assignability
//! verdict: `B x = <nullable B>` reports `invalid_assignment` exactly
//! when the chain's resolved type carries the lift, and
//! `invalid_null_aware_operator` pins the receiver-check face (`a?[0]?.b`
//! reads the pre-lift element `E`, so the `?.` is unnecessary on a
//! non-null element and reports — upstream's
//! `_checkForUnnecessaryNullAware` decides on the continuation face, not
//! the shorted row face).

use crate::diagnose::test_rig::{assert_silent, code_names, report_ops, rows};

/// Every diagnostic's `(name, offset, length)` triple in source order.
fn triples(
    report: &crate::diagnose::FileReport,
) -> Vec<(&'static str, u32, u32)> {
    report
        .diagnostics
        .iter()
        .map(|d| (d.code.name, d.range.offset, d.range.length))
        .collect()
}

/// —— `?.` member access ————————————————————————————————————————————

/// `a?.b` over a nullable receiver resolves `B?` — the declaration
/// `B x = a?.b` reports `invalid_assignment` at `x`'s declarator (the
/// nullable read never assigns into a non-null slot, oracle 3.13.7).
#[test]
fn ticket_2272_question_dot_nullable_receiver_lifts_result() {
    let fixture = "class A { B b = B(); } class B {}\n\
                   void g(Object? o) {}\n\
                   void f(A? a) {\n  B x = a?.b;\n  g(x);\n}";
    let report = report_ops(fixture);
    let rows = triples(&report);
    assert_eq!(rows.len(), 1, "{:?}", code_names(&report));
    assert_eq!(rows[0].0, "invalid_assignment");
    let offset =
        crate::diagnose::test_rig::CORE_PRELUDE.len() + fixture.find("a?.b").unwrap();
    assert_eq!(rows[0].1, offset as u32, "{rows:?}");
}

/// `a?.b` over a NON-NULL receiver keeps the operator verdict — the
/// result still types `B?` upstream (the warning is about the operator,
/// not the type), so the same `invalid_assignment` fires beside
/// `invalid_null_aware_operator`.
#[test]
fn ticket_2272_question_dot_non_null_receiver_reports_and_lifts() {
    let fixture = "class A { B b = B(); } class B {}\n\
                   void g(Object? o) {}\n\
                   void f(A a) {\n  B x = a?.b;\n  g(x);\n}";
    let report = report_ops(fixture);
    let names = code_names(&report);
    assert!(
        names.contains(&"invalid_null_aware_operator"),
        "{names:?}"
    );
    assert!(names.contains(&"invalid_assignment"), "{names:?}");
}

/// `a?.b.c` — the chained continuation inherits the lift: the whole
/// node types `C?` (upstream's `ShortCircuiting` resolution lifts every
/// node the `?.` guards), so `C x = a?.b.c` reports.
#[test]
fn ticket_2272_question_dot_chained_continuation_lifts() {
    let fixture =
        "class A { B b = B(); } class B { C c = C(); } class C {}\n\
         void g(Object? o) {}\n\
         void f(A? a) {\n  C x = a?.b.c;\n  g(x);\n}";
    let report = report_ops(fixture);
    assert_eq!(
        code_names(&report),
        ["invalid_assignment"],
        "{:?}",
        rows(&report)
    );
}

/// `a?.m()` — an invoked member's result lifts the same way: `R?` for a
/// `R m()` member, so `R x = a?.m()` reports.
#[test]
fn ticket_2272_question_dot_invoked_member_lifts_result() {
    let fixture = "class A { R m() => throw 0; } class R {}\n\
                   void g(Object? o) {}\n\
                   void f(A? a) {\n  R x = a?.m();\n  g(x);\n}";
    let report = report_ops(fixture);
    assert_eq!(
        code_names(&report),
        ["invalid_assignment"],
        "{:?}",
        rows(&report)
    );
}

/// —— `?..` cascade ————————————————————————————————————————————————
///
/// `a?..m()` returns the cascade TARGET's type (`static_type_analyzer`'s
/// `visitCascadeExpression` — `node.target.typeOrThrow`): `A? a` cascaded
/// types `A?`, so `A x = a?..m()` reports `invalid_assignment` while
/// `A? y = a?..m()` stays silent.
#[test]
fn ticket_2272_question_dot_dot_returns_receiver_type() {
    let fixture = "class A { void m() {} }\n\
                   void g(Object? o) {}\n\
                   void f(A? a) {\n  A? ok = a?..m();\n  A x = a?..m();\n  g(ok);\n  g(x);\n}";
    let report = report_ops(fixture);
    let rows = triples(&report);
    assert_eq!(rows.len(), 1, "{:?}", code_names(&report));
    assert_eq!(rows[0].0, "invalid_assignment");
    let at = fixture.rfind("?..m()").unwrap() as u32;
    assert_eq!(
        rows[0].1,
        crate::diagnose::test_rig::CORE_PRELUDE.len() as u32 + at,
        "{rows:?}"
    );
}

/// `a?..m()` over a NON-NULL receiver: `?..` on `A` reports
/// `invalid_null_aware_operator` (`?..`/`..` lexemes), and the result
/// still types `A` — `A x = a?..m()` then binds silently.
#[test]
fn ticket_2272_question_dot_dot_non_null_receiver_reports() {
    let fixture = "class A { void m() {} }\n\
                   void g(Object? o) {}\n\
                   void f(A a) {\n  A x = a?..m();\n  g(x);\n}";
    let report = report_ops(fixture);
    assert_eq!(
        code_names(&report),
        ["invalid_null_aware_operator"],
        "{:?}",
        rows(&report)
    );
}

/// `a?..m()..n()` — the chain keeps the receiver type through every
/// section: `A?` cascaded stays `A?`, so `A x = a?..m()..n()` reports.
#[test]
fn ticket_2272_question_dot_dot_chained_sections_keep_receiver() {
    let fixture = "class A { void m() {} void n() {} }\n\
                   void g(Object? o) {}\n\
                   void f(A? a) {\n  A x = a?..m()..n();\n  g(x);\n}";
    let report = report_ops(fixture);
    assert_eq!(
        code_names(&report),
        ["invalid_assignment"],
        "{:?}",
        rows(&report)
    );
}

/// `x?..a?.b` — a `?.` INSIDE a cascade section spells the ACCESS face:
/// `a` resolves on the non-null cascade target, so the section's own
/// `?.` reports `invalid_null_aware_operator` with the `?.` lexeme, not
/// the `?..` replacement message.
#[test]
fn ticket_2272_question_dot_inside_cascade_spells_access() {
    let fixture = "class C { A a = A(); } class A {}\n\
                   void f(C? x) {\n  x?..a?.hashCode;\n}";
    let report = report_ops(fixture);
    let rows: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_null_aware_operator")
        .collect();
    assert_eq!(rows.len(), 1, "{:?}", code_names(&report));
    assert!(
        rows[0].message.contains("'?.'"),
        "access lexeme: {:?}",
        rows[0].message
    );
    assert!(
        !rows[0].message.contains("?.."),
        "not the cascade lexeme: {:?}",
        rows[0].message
    );
}

/// —— `?[` index ———————————————————————————————————————————————————
///
/// `a?[0]` is the nullable element type — `Map`'s `V?` `[]` stays `V?`,
/// `List<E>`'s `E` `[]` lifts to `E?`: `E x = l?[0]` reports.
#[test]
fn ticket_2272_question_bracket_nullable_element() {
    let fixture = "void g(Object? o) {}\n\
                   void f(List<int>? l) {\n  int x = l?[0];\n  g(x);\n}";
    let report = report_ops(fixture);
    assert_eq!(
        code_names(&report),
        ["invalid_assignment"],
        "{:?}",
        rows(&report)
    );
}

/// `l?[0]` over a NON-NULL `List<int>` — the `?[` is unnecessary and
/// reports `invalid_null_aware_operator` (the `?[`/`[` lexemes).
#[test]
fn ticket_2272_question_bracket_non_null_receiver_reports() {
    let fixture = "void g(Object? o) {}\n\
                   void f(List<int> l) {\n  int? x = l?[0];\n  g(x);\n}";
    let report = report_ops(fixture);
    assert_eq!(
        code_names(&report),
        ["invalid_null_aware_operator"],
        "{:?}",
        rows(&report)
    );
}

/// `l?[0].f` — the chained continuation through a `?[` result
/// inherits the lift (`l == null ? null : l[0].f` types `int?`):
/// `int x = l?[0].f` reports `invalid_assignment`. This is the
/// triple-burn face — the `?[` fold bakes `E?` into the result row and
/// the `.f` merge must read the pre-lift element type to decide the
/// continuation's own verdict and latch.
#[test]
fn ticket_2272_question_bracket_chained_member_lifts() {
    let fixture = "class S { int f = 0; }\n\
                   void g(Object? o) {}\n\
                   void f(List<S>? l) {\n  int x = l?[0].f;\n  g(x);\n}";
    let report = report_ops(fixture);
    assert_eq!(
        code_names(&report),
        ["invalid_assignment"],
        "{:?}",
        rows(&report)
    );
}

/// `l?[0]?.length` — the `?.` after a `?[` reads the PRE-LIFT
/// element type (`l[0]` is `String`, never `String?`), so the operator is
/// unnecessary on the non-null element and reports
/// `invalid_null_aware_operator` (upstream's continuation face, not the
/// `String?` the row stores).
#[test]
fn ticket_2272_question_bracket_then_question_dot_reports() {
    let fixture = "void g(Object? o) {}\n\
                   void f(List<String>? l) {\n  int? x = l?[0]?.length;\n  g(x);\n}";
    let report = report_ops(fixture);
    assert_eq!(
        code_names(&report),
        ["invalid_null_aware_operator"],
        "{:?}",
        rows(&report)
    );
}

/// `a?.l?[0].f` — the whole doubly-shorted chain types `int?` —
/// `int x = a?.l?[0].f` reports `invalid_assignment`.
#[test]
fn ticket_2272_question_dot_bracket_chain_lifts() {
    let fixture = "class S { int f = 0; }\n\
                   class A { List<S>? l = throw 0; }\n\
                   void g(Object? o) {}\n\
                   void f(A? a) {\n  int x = a?.l?[0].f;\n  g(x);\n}";
    let report = report_ops(fixture);
    assert_eq!(
        code_names(&report),
        ["invalid_assignment"],
        "{:?}",
        rows(&report)
    );
}

/// The `?[` write-read symmetry pin: a nullable element keeps the
/// continuation's own unchecked verdict — `List<String?>?`'s
/// `l?[0].length` resolves `.length` on `String?`, so an `unchecked_*`
/// verdict fires at the member (upstream: the `?[` guards the LIST,
/// never the element's own nullability).
#[test]
fn ticket_2272_question_bracket_nullable_element_keeps_unchecked() {
    let fixture = "void f(List<String?>? l) {\n  l?[0].length;\n}";
    let report = report_ops(fixture);
    let names = code_names(&report);
    assert!(
        names.iter().any(|n| n.starts_with("unchecked_")),
        "{names:?}"
    );
}

/// —— non-nullable control ——————————————————————————————————————————
///
/// `a.b` (plain access, no null-aware operator) binds `B` — `B x = a.b`
/// stays silent: the pins above observe the lift, this row proves the
/// fixture's own declared faces are clean.
#[test]
fn ticket_2272_plain_member_keeps_declared_type() {
    let fixture = "class A { B b = B(); } class B {}\n\
                   void g(Object? o) {}\n\
                   void f(A a) {\n  B x = a.b;\n  g(x);\n}";
    assert_silent(&report_ops(fixture));
}

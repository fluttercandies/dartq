//! The #3509 pins: a record literal's field `void`-use anchor is the FIELD
//! NODE's extent — byte-aligned with the oracle (dart analyze 3.13.7).
//!
//! Upstream `record_literal_resolver.dart:159` reports
//! `diag.useOfVoidResult.at(field)`, where `field` is the literal's field
//! node: a `RecordLiteralNamedField` for a named field (the span opens at
//! the label and closes at the value expression's end) and the value
//! expression itself for a positional field. The engine's field rows spell
//! only the operand's own token (a declared-method send keeps its
//! `methodName`), so the anchor widens to the value's whole expression
//! before the label is joined: `(one: v(),)` reports `one: v()` (length 8 —
//! not `one: v`) and `(v(), 1)` reports `v()` (length 3 — not `v`).
//!
//! Registered through `diagnose/tests.rs`'s `#[path]` block; the module
//! stays inside the `diagnose` subtree so `crate::diagnose::test_rig`
//! resolves.

use crate::diagnose::test_rig::{CORE_PRELUDE, report_ops, rows};

/// The `use_of_void_result` rows of `fixture` as `(offset, length)` in the
/// fixture's own coordinates, ascending.
fn void_spans(fixture: &str) -> Vec<(usize, usize)> {
    let shift = CORE_PRELUDE.len();
    let mut spans: Vec<(usize, usize)> = report_ops(fixture)
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "use_of_void_result")
        .map(|d| (d.range.offset as usize - shift, d.range.length as usize))
        .collect();
    spans.sort_unstable();
    spans
}

/// The `(offset, length)` of the LAST `needle` in `fixture`.
fn span_of(fixture: &str, needle: &str) -> (usize, usize) {
    (fixture.rfind(needle).expect("needle"), needle.len())
}

/// The single `void` row of `fixture`.
fn sole_void(fixture: &str) -> (usize, usize) {
    let spans = void_spans(fixture);
    assert_eq!(spans.len(), 1, "expected one void row: {spans:?}");
    spans[0]
}

/// A named field anchors on the whole `RecordLiteralNamedField`: label,
/// colon and the value's expression — `one: v()`, never the callee name.
#[test]
fn ticket_3509_named_field_anchors_whole_field() {
    let fixture = "void v() {}\nvoid f() {\n  var r = (one: v(),);\n}";
    let span = sole_void(fixture);
    assert_eq!(span, span_of(fixture, "one: v()"));
    assert_eq!(&fixture[span.0..span.0 + span.1], "one: v()");
}

/// A positional field anchors on its value expression — `v()` whole, not
/// the callee's name token.
#[test]
fn ticket_3509_positional_field_anchors_value_expression() {
    let fixture = "void v() {}\nvoid f() {\n  var r = (v(), 1);\n}";
    let span = sole_void(fixture);
    assert_eq!(span, (fixture.find("v(),").unwrap(), 3));
    assert_eq!(&fixture[span.0..span.0 + span.1], "v()");
}

/// The trailing-comma spelling of a single positional field is the same
/// node extent.
#[test]
fn ticket_3509_single_positional_field_anchors_value_expression() {
    let fixture = "void v() {}\nvoid f() {\n  var r = (v(),);\n}";
    assert_eq!(sole_void(fixture), (fixture.find("v(),").unwrap(), 3));
}

/// A positional field carrying a member access anchors the whole access
/// (`x.id`), the value expression the field node wraps.
#[test]
fn ticket_3509_positional_field_member_access_anchors_whole_access() {
    let fixture = "class E { void get id => throw 0; }\nvoid f(E x) {\n  var r = (x.id, 1);\n}";
    assert_eq!(sole_void(fixture), span_of(fixture, "x.id"));
}

/// The silent twins: a field whose value is not `void` consumes nothing.
#[test]
fn ticket_3509_non_void_fields_stay_silent() {
    for fixture in [
        "void f() {\n  var r = (one: 1,);\n}",
        "void f() {\n  var r = (1, 'x');\n}",
    ] {
        assert!(
            void_spans(fixture).is_empty(),
            "silent: {fixture:?} {:?}",
            rows(&report_ops(fixture))
        );
    }
}

/// A mixed literal reports only the `void` field, at that field's extent —
/// the non-`void` sibling stays silent and the rows keep their source
/// order.
#[test]
fn ticket_3509_mixed_fields_report_only_the_void_field() {
    let fixture = "void v() {}\nvoid f() {\n  var r = (one: 1, two: v(), three: 'x');\n}";
    assert_eq!(sole_void(fixture), span_of(fixture, "two: v()"));
}

/// The `const` spelling keeps the non-`void` field silent — the const
/// capture path stages no field row of its own.
#[test]
fn ticket_3509_const_record_literal_stays_silent() {
    let fixture = "void f() {\n  var r = const (one: 1, two: 2);\n}";
    assert!(
        void_spans(fixture).is_empty(),
        "{fixture:?} {:?}",
        rows(&report_ops(fixture))
    );
}

/// Family isolation: a binary operand's declared-method send keeps the
/// `methodName` anchor (`v`, length 1) — the operand use-site rule, not
/// the field-node rule.
#[test]
fn ticket_3509_binary_operand_keeps_callee_name_anchor() {
    let fixture = "void v() {}\nvoid g() {\n  v() && true;\n}";
    assert_eq!(sole_void(fixture), (fixture.find("v() &&").unwrap(), 1));
}

/// Family isolation: the ternary condition's send keeps the `methodName`
/// anchor as well.
#[test]
fn ticket_3509_condition_operand_keeps_callee_name_anchor() {
    let fixture = "void v() {}\nvoid g() {\n  v() ? 1 : 2;\n}";
    assert_eq!(sole_void(fixture), (fixture.find("v() ?").unwrap(), 1));
}

/// The report is a single row per `void` field: the widened anchor must not
/// duplicate beside the operand-level faces the same fixture invites.
#[test]
fn ticket_3509_field_reports_once() {
    let fixture = "void v() {}\nvoid f() {\n  var r = (one: v(), other: v());\n}";
    let mut expected = vec![span_of(fixture, "one: v()"), span_of(fixture, "other: v()")];
    expected.sort_unstable();
    assert_eq!(void_spans(fixture), expected);
}

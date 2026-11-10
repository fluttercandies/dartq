//! #3751 — `unchecked_use_of_nullable_value` function-variant anchor pins.
//!
//! The oracle anchors the function variant
//! (`uncheckedInvocationOfNullableValue`) on the whole callee expression —
//! receiver through selector — not the member token alone
//! (`function_expression_invocation_resolver.dart` reports on the
//! `FunctionExpressionInvocation`'s function node). The member-callee arm
//! used to anchor on the selector only, so `x.first()` reported at `first`
//! where the oracle reports at `x.first`.
//!
//! Oracle provenance: `~/.dartq-oracle/3.13.7/dart-sdk/bin/dart analyze` —
//! each pin asserts the oracle's exact (code, offset, length, message).

use super::test_rig::{CORE_PRELUDE, report_ops, rows};
use super::*;

/// The absolute fixture offset of `needle` (the rig's prelude included).
fn at(fixture: &str, needle: &str) -> usize {
    CORE_PRELUDE.len() + fixture.find(needle).unwrap()
}

/// One row of `name` anchored at `offset:len` carrying `message`.
#[track_caller]
fn assert_row(report: &FileReport, name: &str, offset: usize, len: u32, message: &str) {
    let hits: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| {
            d.code.name == name && d.range.offset as usize == offset && d.range.length == len
        })
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "expected one {name} @ {offset}:{len}; got {:#?}",
        rows(report)
    );
    assert_eq!(hits[0].message, message, "{:#?}", rows(report));
}

const UNCHECKED: &str = "unchecked_use_of_nullable_value";
const FUNCTION_TEXT: &str =
    "The function can't be unconditionally invoked because it can be 'null'.";
const METHOD_INDEX_TEXT: &str =
    "The method '[]' can't be unconditionally invoked because the receiver can be 'null'.";
const OPERATOR_PLUS_TEXT: &str =
    "The operator '+' can't be unconditionally invoked because the receiver can be 'null'.";

/// `x.h()` invokes the nullable function stored in the member `h` — the
/// row anchors on `x.h`, not `h`.
#[test]
fn member_callee_anchors_on_receiver_through_selector() {
    let src = "\nclass H {\n  int Function()? h;\n}\nvoid m(H x) {\n  x.h();\n}\n";
    let report = report_ops(src);
    assert_row(&report, UNCHECKED, at(src, "x.h"), 3, FUNCTION_TEXT);
}

/// `this.f()` anchors on `this.f`.
#[test]
fn this_member_callee_anchors_on_this_through_selector() {
    let src = "\nclass C {\n  int Function()? f;\n  void m() {\n    this.f();\n  }\n}\n";
    let report = report_ops(src);
    assert_row(&report, UNCHECKED, at(src, "this.f"), 6, FUNCTION_TEXT);
}

/// A bare nullable-function read invoked keeps the read's own span — the
/// receiver IS the callee, so the anchor does not change.
#[test]
fn bare_function_callee_keeps_read_span() {
    let src = "\nclass C {\n  int Function()? f;\n  void m() {\n    f();\n  }\n}\n";
    let report = report_ops(src);
    assert_row(&report, UNCHECKED, at(src, "f();"), 1, FUNCTION_TEXT);
}

// —— shape 2: `this` operator access inside `extension on A?` —————————
//
// `this` carries the `on` type's static face `A?` inside a nullable
// extension body, so `this[0]`/`this[0] = v`/`this + this` dereference a
// potentially-nullable receiver and report the shared unchecked code at the
// operator — the oracle's `isNullable` tail precedes the member lookup
// (type_property_resolver.dart), ahead of the `A.operator []`/`+`
// definedness the spelling would ask. The null-aware spellings
// (`this?[0]`, `this?[0] = v`) short the access instead and stay silent.
//
// Oracle provenance: probe fixture at `extension E on A?` — `this[0]` and
// `this[0] = A()` both report `The method '[]' …` at the opening bracket
// with length 1, `this + this` reports `The operator '+' …` at the token,
// and the `?[` forms report nothing.

/// `this[0]` inside `extension on A?` reports `'[]'` anchored at the
/// opening `[` with length 1.
#[test]
fn this_null_aware_index_on_nullable_extension_reports_at_bracket() {
    let src = "\nclass A { A operator [](int i) => this; }\n\
               extension E on A? {\n  void f() {\n    this[0];\n  }\n}\n";
    let report = report_ops(src);
    assert_row(&report, UNCHECKED, at(src, "[0];"), 1, METHOD_INDEX_TEXT);
}

/// `this?[0]` inside `extension on A?` shorts the access — silent.
#[test]
fn this_null_aware_index_question_mark_is_silent() {
    let src = "\nclass A { A operator [](int i) => this; }\n\
               extension E on A? {\n  void f() {\n    this?[0];\n  }\n}\n";
    let report = report_ops(src);
    let hits: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == UNCHECKED)
        .collect();
    assert!(
        hits.is_empty(),
        "expected no unchecked row: {:#?}",
        rows(&report)
    );
}

/// `this[0] = v` reports the BASE `'[]'` spelling at the opening `[` —
/// the element lookup's `[]=` question is never the dereference's text.
#[test]
fn this_index_write_reports_base_index_spelling() {
    let src = "\nclass A { A operator [](int i) => this; void operator []=(int i, A v) {} }\n\
               extension E on A? {\n  void f() {\n    this[0] = A();\n  }\n}\n";
    let report = report_ops(src);
    assert_row(&report, UNCHECKED, at(src, "[0] ="), 1, METHOD_INDEX_TEXT);
}

/// `this?[0] = v` shorts the write form too — silent.
#[test]
fn this_null_aware_index_write_question_mark_is_silent() {
    let src = "\nclass A { A operator [](int i) => this; void operator []=(int i, A v) {} }\n\
               extension E on A? {\n  void f() {\n    this?[0] = A();\n  }\n}\n";
    let report = report_ops(src);
    let hits: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == UNCHECKED)
        .collect();
    assert!(
        hits.is_empty(),
        "expected no unchecked row: {:#?}",
        rows(&report)
    );
}

/// `this + this` inside `extension on A?` reports the OPERATOR face at the
/// operator token — the BinaryExpression arm of the node-shape dispatch.
#[test]
fn this_binary_operator_reports_operator_face() {
    let src = "\nclass A { A operator +(A o) => this; }\n\
               extension E on A? {\n  void f() {\n    this + this;\n  }\n}\n";
    let report = report_ops(src);
    assert_row(
        &report,
        UNCHECKED,
        at(src, "+ this;"),
        1,
        OPERATOR_PLUS_TEXT,
    );
}

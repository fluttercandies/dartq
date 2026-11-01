//! The #3349 pins — the four `use_of_void_result` residuals #3316 left
//! open:
//!
//! 1. the DOUBLED row on a `&&`/`||`/`??` operand: the listener's
//!    condition faces and `fold_binary`/`end_if_null` both reported the
//!    same operand row, while the oracle answers one row per consumed node
//!    (upstream `checkForUseOfVoidResult`, generated/
//!    error_detection_helpers.dart:245-258, called from
//!    error_verifier.dart:470/:482 and bool_expression_verifier.dart:57);
//! 2. the MISSING for-in element row: `_checkForEachParts`'s
//!    `sequenceElementType is VoidType && variableType is! VoidType` arm
//!    (error_verifier.dart:4478-4484) anchored `.at(node.iterable)`;
//! 3. the collection literal's EXTRA row: the void use runs only when the
//!    enclosing element type is not `void` (literal_element_verifier.dart
//!    :95, the map arms :127/:166);
//! 4. the named record field's ANCHOR: the `RecordLiteralNamedField` node
//!    (`name: value`), not its value alone (record_literal_resolver.dart
//!    :159 `.at(field)`).
//!
//! Registered through `diagnose/tests.rs`'s `#[path]` block; the module
//! stays inside the `diagnose` subtree so `crate::diagnose::test_rig`
//! resolves.

use crate::diagnose::test_rig::{CORE_PRELUDE, report_ops};
use crate::diagnose::types::FileReport;

/// The `use_of_void_result` rows of `report` as `(offset, length)`, in the
/// fixture's own coordinates, ascending.
fn void_spans(report: &FileReport) -> Vec<(u32, u32)> {
    let shift = u32::try_from(CORE_PRELUDE.len()).expect("prelude fits u32");
    let mut spans: Vec<(u32, u32)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "use_of_void_result")
        .map(|d| (d.range.offset - shift, d.range.length))
        .collect();
    spans.sort_unstable();
    spans
}

/// The `(offset, length)` of the LAST `needle` in `fixture`.
fn span_of(fixture: &str, needle: &str) -> (u32, u32) {
    let at = u32::try_from(fixture.rfind(needle).expect("needle")).expect("offset fits u32");
    (at, u32::try_from(needle.len()).expect("len fits u32"))
}

/// The `(offset, 1)` of the LAST `needle` — the `.at(methodName)` anchor
/// `checkForUseOfVoidResult` gives an invoked operand (its row spans the
/// callee's name token).
fn callee_span(fixture: &str, needle: &str) -> (u32, u32) {
    let at = u32::try_from(fixture.rfind(needle).expect("needle")).expect("offset fits u32");
    (at, 1)
}

/// The single `void` row of `fixture`, asserting there is exactly one.
fn sole_void(fixture: &str) -> (u32, u32) {
    let spans = void_spans(&report_ops(fixture));
    assert_eq!(spans.len(), 1, "expected one void row: {spans:?}");
    spans[0]
}

/// Residual 1: a `void` operand of a lazy boolean operator reports once —
/// the fold's operand check owns the row, the condition face beside it does
/// not double it.
#[test]
fn ticket_3349_lazy_boolean_operand_reports_once() {
    for op in ["&&", "||"] {
        let fixture = format!("void v() {{}}\nvoid g() {{\n  v() {op} true;\n}}");
        assert_eq!(
            void_spans(&report_ops(&fixture)),
            vec![callee_span(&fixture, "v()")],
            "`v() {op} true`"
        );
    }
}

/// Residual 1, the `??` spelling: `end_if_null` and the fold both read the
/// left operand — one row.
#[test]
fn ticket_3349_if_null_operand_reports_once() {
    let fixture = "void v() {}\nvoid g() {\n  var a = v() ?? 1;\n}";
    assert_eq!(sole_void(fixture), callee_span(fixture, "v()"));
}

/// Residual 1's negative control: both operands `void`, one row each.
#[test]
fn ticket_3349_lazy_boolean_both_operands_report_once_each() {
    let fixture = "void v() {}\nvoid w() {}\nvoid g() {\n  v() && w();\n}";
    let mut expected = vec![callee_span(fixture, "v()"), callee_span(fixture, "w()")];
    expected.sort_unstable();
    assert_eq!(void_spans(&report_ops(fixture)), expected);
}

/// Residual 2: a `void` element iterated into a non-`void` loop variable
/// consumes the element's value — the oracle reports at the ITERABLE
/// (`.at(node.iterable)`).
#[test]
fn ticket_3349_for_in_void_element_reports_at_the_iterable() {
    let fixture = "void f(Iterable<void> xs) {\n  for (Object? o in xs) {\n  }\n}";
    // The iterable node is the bare read: its own `xs` span.
    let (offset, _) = span_of(fixture, "xs)");
    assert_eq!(void_spans(&report_ops(fixture)), vec![(offset, 2)]);
}

/// Residual 2's literal spelling `for (Object? o in <void>[])`: routed, not
/// silent by design. The written group's leaf never folds `void` onto
/// `Ty::Void` (`literal_arg_leaf` special-cases only `dynamic`/`Null`), so
/// the literal keeps an opaque row and the element face has nothing to
/// read. The emitter (`Engine::for_in_void_element`) already answers the
/// annotated iterable above; this arm turns green once the literal's
/// written argument spells `void`.
#[test]
#[ignore = "routed: literal_arg_leaf must spell `void` (assignability.rs, #3312's file)"]
fn ticket_3349_for_in_void_literal_group_element() {
    let fixture = "void f() {\n  for (Object? o in <void>[]) {}\n}";
    assert_eq!(sole_void(fixture), span_of(fixture, "<void>[]"));
}

/// Residual 2's negative controls: a `var` head infers the element itself
/// (a `void` head — the oracle stays silent), and a `void` head discards
/// into `void` by declaration.
#[test]
fn ticket_3349_for_in_void_head_stays_silent() {
    for fixture in [
        "void f(Iterable<void> xs) {\n  for (var o in xs) {\n  }\n}",
        "void f(Iterable<void> xs) {\n  for (void o in xs) {\n  }\n}",
    ] {
        assert!(
            void_spans(&report_ops(fixture)).is_empty(),
            "silent: {fixture:?}"
        );
    }
}

/// Residual 3: the element-type gate. A written non-`void` element slot
/// consumes the leaf's value; a `void` slot (and an untyped literal, whose
/// inferred element IS the `void` leaf) discards it.
#[test]
fn ticket_3349_collection_literal_element_gate() {
    let typed = "void v() {}\nvoid f() {\n  var x = <int>[v()];\n}";
    assert_eq!(sole_void(typed), callee_span(typed, "v()"));
    for fixture in [
        "void v() {}\nvoid f() {\n  var x = <void>[v()];\n}",
        "void v() {}\nvoid f() {\n  var x = [v()];\n}",
    ] {
        assert!(
            void_spans(&report_ops(fixture)).is_empty(),
            "silent: {fixture:?}"
        );
    }
}

/// Residual 4: a named record field anchors on the whole
/// `RecordLiteralNamedField` (`one: v` — the value's row stops at the
/// callee name), a positional field on its expression.
#[test]
fn ticket_3349_named_record_field_anchor() {
    let named = "void v() {}\nvoid f() {\n  var r = (one: v(),);\n}";
    let (offset, _) = span_of(named, "one");
    assert_eq!(void_spans(&report_ops(named)), vec![(offset, 6)], "`one: v`");

    let positional = "void v() {}\nvoid f() {\n  var r = (v(), 1);\n}";
    assert_eq!(sole_void(positional), callee_span(positional, "v()"));
}

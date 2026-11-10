//! #3579 pins — fabricated `undefined_operator` where the oracle resolves
//! the name through a completed send's RETURN type.
//!
//! # The defect
//!
//! An operator row's staged `Receiver::Typed` carries the receiver
//! expression's declared face, which for a call result is the CALLEE's own
//! function type: `f()[i]` classified its `[]` receiver as `int
//! Function()` instead of the send's result `int`, and
//! `maybeIndexable<R, W>(x)?[i] ??= v` classified as `Indexable<R, W>?
//! Function(R)` instead of `Indexable<R, W>?`. Every operator spelling
//! the function face genuinely lacks then reported `undefined_operator`
//! against a receiver the oracle never resolves against — the census
//! cluster's largest slice (`syntax_test`'s `gb0()[0]`, the
//! `inference_update_3` `?[`/`??=` matrix, the `Function` comparisons).
//!
//! # The wiring
//!
//! `Engine::check_operator_expr_in` now swaps the receiver face for the
//! send's result when the operand row's own span opens on a completed
//! send (`call_ends` keys every callee offset; `mark_operator` flips the
//! operand row in place, so `row.range.offset` IS the callee token — the
//! same gate `invoked_receiver_face` keeps). `callable_result_of` answers
//! the non-generic return; a generic callee grounds the definedness
//! question on its DECLARED return's container (the written signature's
//! `Indexable<R, W>?`, which the extension application's own parameter
//! walk instantiates), matching the oracle's resolution over the
//! unsubstituted signature.

use crate::diagnose::test_rig::{assert_silent, code_names, report_ops, rows};

/// The bare `Function` callee's result is `dynamic` — the oracle resolves
/// `[]` on `dynamic` and reports nothing.
#[test]
fn t3579_function_call_result_index_silent() {
    // dart analyze on `void test(Function f) { f()[0]; }` reports nothing.
    let report = report_ops("void test(Function f) { f()[0]; }");
    assert_silent(&report);
}

/// The generic callee's `?[`/`??=` pair resolves through its declared
/// return's extension face — no `undefined_operator` is fabricated on the
/// call-result receiver.
///
/// The extension here is UNNAMED, so its members are library-private and
/// the oracle does report `unused_element` on `[]`: the compound write
/// `?[0] ??=` credits `[]=` only. `dart analyze` on this exact fixture
/// emits `UNUSED_ELEMENT` at 2:49 ("The declaration '[]' isn't
/// referenced.") plus the `dead_code` / `dead_null_aware_expression` pair
/// on the `??=` operand. So the pin asserts the absence of the fabricated
/// operator row rather than silence.
#[test]
fn t3579_generic_callee_extension_index_silent() {
    let report = report_ops(
        "class Indexable<R, W> { final R v; Indexable(this.v); } \
         extension<R, W> on Indexable<R, W> { R operator [](int i) => v; void operator []=(int i, W w) {} } \
         Indexable<R, W>? maybeIndexable<R, W>(R v) => Indexable(v); \
         void test() { maybeIndexable<int, int>(1)?[0] ??= 3; }",
    );
    let codes = code_names(&report);
    assert!(
        !codes.contains(&"undefined_operator"),
        "the call-result receiver must not fabricate an operator row; got {codes:?}"
    );
}

/// The binary operator on a call result resolves on the return type —
/// `f() + 1` with `f : int Function()` resolves `+` on `int`.
#[test]
fn t3579_binary_op_on_call_result_silent() {
    let report = report_ops("void test(int Function() f) { f() + 1; }");
    assert_silent(&report);
}

/// A genuinely missing spelling still reports — and names the call's
/// RETURN type, never the callee's function type. `f()[0]` with
/// `f : int Function()` resolves `[]` on `int`, which declares none.
#[test]
fn t3579_call_result_index_miss_names_return_type() {
    // dart analyze on `void test(int Function() f) { f()[0]; }` reports
    // "The operator '[]' isn't defined for the type 'int'."
    let report = report_ops("void test(int Function() f) { f()[0]; }");
    let codes = code_names(&report);
    assert_eq!(
        codes,
        vec!["undefined_operator"],
        "expected exactly the return-type miss, got {:?}",
        rows(&report)
    );
    let message = &report.diagnostics[0].message;
    assert!(
        message.contains("int") && !message.contains("Function"),
        "the miss must name the call's return type, got {message:?}"
    );
}

/// A plain function-typed value receiver (no send) keeps its own face —
/// `f < g` over tear-offs reports `undefined_operator` on the function
/// type, matching the oracle's `instanceReceiverType` answer.
#[test]
fn t3579_tear_off_operand_still_reports() {
    let report = report_ops("void test(String Function(int) f, String Function(int) g) { f < g; }");
    let codes = code_names(&report);
    assert_eq!(
        codes,
        vec!["undefined_operator"],
        "a function-typed operand keeps its own face, got {:?}",
        rows(&report)
    );
    assert!(
        report.diagnostics[0].message.contains("Function"),
        "expected the function-type display, got {:?}",
        report.diagnostics[0].message
    );
}

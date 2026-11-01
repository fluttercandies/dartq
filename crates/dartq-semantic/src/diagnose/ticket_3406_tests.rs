//! #3406 pins — the `undefined_*` cluster C2: a receiver that IS a call
//! result must be modelled by the invocation's RETURN type, never by the
//! callee's own function type.
//!
//! # The defect
//!
//! Upstream types a completed send by the invoked signature's return: the
//! receiver-type computation in
//! `pkg/analyzer/lib/src/dart/resolver/method_invocation_resolver.dart`
//! reads the receiver node's `staticType`, which for `f(x)` is
//! `staticType.returnType`, so `f(x).m` resolves `m` on `f`'s return type.
//! The `FunctionType` receiver arm there (`:1197`) is reachable only when
//! the receiver really IS a function value (`.call`, or the
//! `undefinedMethodOnFunctionType` row of `:223-228` over a function-type
//! TYPE LITERAL).
//!
//! dartq's `recv_expression_ty` re-derived a receiver span through its
//! value faces alone (`rebound_read_face`, `read_type_at`, the
//! flow-blind initializer face, …). For `f(x).m` that span is the CALLEE
//! identifier, so the face came back as `f`'s declared type — a
//! `FunctionType` for every function-typed formal, local, field or
//! tear-off — and `check_shape_receiver` then ran the member verdict on
//! the function face, reporting `undefined_method`/`undefined_getter`
//! against a `Function` receiver the oracle never resolves against
//! (475 method/operator rows + 13 anonymous-method rows in the census).
//!
//! # The wiring
//!
//! `Engine::recv_expression_ty`'s value faces now pass through
//! `invoked_receiver_face`, which swaps a completed send's callee face
//! for the call's result (`callable_result_of`) — gated on `call_ends`,
//! the exact ledger of completed sends. The chain leg is exempt (an
//! invoked node's answer already IS the send's result), and the
//! declaration-shape fallback answers the signature's return instead of
//! dropping the face.

use crate::diagnose::test_rig::{assert_silent, code_names, report_ops, rows};

/// The member verdict on a call result runs on the RETURN type: a
/// defined member of `Snip` resolves, so nothing reports.
#[test]
fn t3406_member_on_call_result_silent() {
    let report = report_ops(
        "class Snip { void run() {} } \
         void test(Snip Function(int) make) { make(1).run(); }",
    );
    assert_silent(&report);
}

/// The cascade form (`f(x)..m()`): every section resolves against the
/// cascade TARGET's static type, which for a call result is its return.
#[test]
fn t3406_cascade_on_call_result_silent() {
    let report = report_ops(
        "class Snip { void run() {} } \
         void test(Snip Function(int) make) { make(1)..run(); }",
    );
    assert_silent(&report);
}

/// The generic call result (`f<A>(x).m`): the result face is undecidable
/// without inference, so the access keeps the under-report — the pinned
/// property is that no `Function`-receiver row is invented.
#[test]
fn t3406_generic_call_result_member_silent() {
    let report = report_ops(
        "class Snip { void run() {} } \
         void test(Snip Function<T>(T) make) { make<int>(1).run(); }",
    );
    assert_silent(&report);
}

/// The local-closure spelling of the same receiver (`var g = (int x) =>
/// Snip()`), whose face the parse-time initializer derivation holds.
#[test]
fn t3406_local_closure_call_result_silent() {
    let report = report_ops(
        "class Snip { void run() {} } \
         void test() { var g = (int x) => Snip(); g(1).run(); }",
    );
    assert_silent(&report);
}

/// The verdict runs on the RETURN type, so a genuine miss there still
/// reports — and names the return type, never `Function`.
#[test]
fn t3406_member_miss_reports_on_return_type() {
    let report = report_ops(
        "class Snip { void run() {} } \
         void test(Snip Function(int) make) { make(1).gone(); }",
    );
    let codes = code_names(&report);
    assert_eq!(
        codes,
        vec!["undefined_method"],
        "expected exactly the return-type miss, got {:?}",
        rows(&report)
    );
    let message = &report.diagnostics[0].message;
    assert!(
        message.contains("Snip") && !message.contains("Function"),
        "the miss must name the call's return type, got {message:?}"
    );
}

/// Negative pin: a plain function-typed VALUE receiver (no call) keeps
/// the function face — a genuine function-type member miss still reports.
#[test]
fn t3406_tear_off_receiver_still_reports() {
    let report = report_ops("void test(int Function(int) f) { f.gone; }");
    let codes = code_names(&report);
    assert_eq!(
        codes,
        vec!["undefined_getter"],
        "a function-typed value receiver keeps its own face, got {:?}",
        rows(&report)
    );
    assert!(
        report.diagnostics[0].message.contains("Function"),
        "expected the function-type display, got {:?}",
        report.diagnostics[0].message
    );
}

/// Negative pin: a call whose RESULT is a function type keeps that result
/// as the receiver — the send is unwrapped once, never twice, so the row
/// never names the CALLEE's signature (`int Function() Function(int)`);
/// at worst the member face stays undecided and the access is silent.
#[test]
fn t3406_call_result_function_type_is_never_the_callee() {
    let report = report_ops(
        "void test(int Function() Function(int) make) { make(1).gone; }",
    );
    assert!(
        code_names(&report)
            .iter()
            .all(|code| *code == "undefined_getter"),
        "expected at most the result-type getter miss, got {:?}",
        rows(&report)
    );
    for diagnostic in &report.diagnostics {
        assert!(
            !diagnostic.message.contains("Function(int)"),
            "the receiver must never be the callee's own signature, got {:?}",
            diagnostic.message
        );
    }
}

/// The declared-callee spelling (`Snip make(int x) => …`): its declaration
/// shape is a signature too, so the receiver still reads the send's RETURN
/// type and a genuine miss on it reports.
#[test]
fn t3406_declared_callee_result_miss_reports_on_return_type() {
    let report = report_ops(
        "class Snip { void run() {} } \
         Snip make(int x) => Snip(); \
         void test() { make(1).gone(); }",
    );
    let codes = code_names(&report);
    assert_eq!(
        codes,
        vec!["undefined_method"],
        "expected exactly the return-type miss, got {:?}",
        rows(&report)
    );
    assert!(
        report.diagnostics[0].message.contains("Snip"),
        "the miss must name the call's return type, got {:?}",
        report.diagnostics[0].message
    );
}

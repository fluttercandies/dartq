//! The #3062 pins — the closure-argument signature walk: the downward
//! return face runs BEFORE the function-subtype ask, and the arity and
//! parameter slots decide the row.
//!
//! Upstream decides a closure-literal argument through the same
//! per-argument `correspondingParameter` ask as any other argument
//! (pkg/analyzer/lib/src/generated/resolver.dart:523
//! `checkForArgumentTypesNotAssignableInList`, the loop at :528-535;
//! error_detection_helpers.dart:67 reads `argument.correspondingParameter`),
//! and the pair decides by `_isFunctionSubtypeOf`
//! (pkg/analyzer/lib/src/dart/element/subtype.dart:338 — return covariance
//! :352-354, then the positional walk). The oracle's own pin
//! (`test_functionType`, argument_type_not_assignable_test.dart:336-347)
//! proves a literal NEVER acquires the context's parameters: `a.n(() => 0)`
//! over `n(void f(int i))` reports `void Function()` — the written formals
//! (none) with the context return — so every arity mismatch between the
//! literal's written formals and the formal's signature is a decided
//! rejection, never an inference question.
//!
//! The captured stub's `void` return is never an inference: the formal's
//! return is the downward context the body re-infers under
//! (`inferLocalFunction`'s cap — the `S = R` posture the capture's Value
//! row already carries), so the walk re-types the stub up front and ONE
//! `fn_sig_subtype` ask decides on the real faces.

use super::test_rig::report_ops;
use super::*;

fn codes(folded: &FileReport) -> Vec<&str> {
    folded
        .diagnostics
        .iter()
        .map(|diag| diag.code.name)
        .collect()
}

fn atna_messages(folded: &FileReport) -> Vec<String> {
    folded
        .diagnostics
        .iter()
        .filter(|diag| diag.code.name == "argument_type_not_assignable")
        .map(|diag| diag.message.to_string())
        .collect()
}

/// `g(() {})` against `void Function(dynamic)` — the literal writes zero
/// formals, the formal requires one: the arity walk decides the rejection
/// (the `test_functionType` mechanism, spelled with a `dynamic` formal —
/// a zero-formal literal never acquires the context's parameter).
#[test]
fn closure_arity_deficit_reports() {
    let folded = report_ops(
        "void g(void Function(dynamic) f) {}\n\
         void k() {\n\
           g(() {});\n\
         }\n",
    );
    assert_eq!(
        codes(&folded),
        vec!["argument_type_not_assignable"],
        "zero-formal literal against a one-required formal reports: {:#?}",
        folded.diagnostics
    );
    let messages = atna_messages(&folded);
    assert!(
        messages[0].contains("'void Function()' can't be assigned")
            && messages[0].contains("'void Function(dynamic)'"),
        "the message renders the literal's written formals and the formal: {messages:?}"
    );
}

/// The surplus twin: `g((x) {})` against `void Function()` — the untyped
/// formal finds no schema entry and stays `dynamic`
/// (`closure_ctx_formal_ty`'s `unwrap_or(Dynamic)`), so the literal's
/// required-1 signature rejects against the required-0 formal.
#[test]
fn closure_arity_surplus_reports() {
    let folded = report_ops(
        "void g(void Function() f) {}\n\
         void k() {\n\
           g((x) {});\n\
         }\n",
    );
    assert_eq!(
        codes(&folded),
        vec!["argument_type_not_assignable"],
        "one-required literal against a zero-formal formal reports: {:#?}",
        folded.diagnostics
    );
}

/// The context-typed pair stays silent: `g((x) {})` against
/// `void Function(dynamic)` — the untyped formal infers the schema's
/// `dynamic` (`_inferFormalParameters`), the re-typed signature matches,
/// and no arity face fires.
#[test]
fn closure_untyped_formal_context_silences() {
    let folded = report_ops(
        "void g(void Function(dynamic) f) {}\n\
         void k() {\n\
           g((x) {});\n\
         }\n",
    );
    assert_eq!(
        codes(&folded),
        Vec::<&str>::new(),
        "the schema-infers `dynamic` formal matches: silent"
    );
}

/// `h(() => "s")` against `int Function()` — the argument row is silent
/// (the literal re-infers the context return, the cap posture) and the
/// failure stays the closure-return row's, exactly the oracle's
/// split (`return_of_invalid_type_from_closure`, never an argument row).
#[test]
fn closure_body_type_mismatch_never_becomes_an_argument_row() {
    let folded = report_ops(
        "void h(int Function() cb) {}\n\
         void k() {\n\
           h(() => 's');\n\
         }\n",
    );
    assert_eq!(
        codes(&folded),
        vec!["return_of_invalid_type_from_closure"],
        "the body's row owns the failure; no ATNA: {:#?}",
        folded.diagnostics
    );
}

/// `h(() => 5)` against `num Function()` — the context return re-types
/// the stub through a NON-void formal return (the pre-typed ask, the
/// rendering class the old two-ask dance only reached after a first
/// `Some(false)`): the literal satisfies the formal and stays silent.
#[test]
fn closure_context_return_satisfies_nonvoid_formal() {
    let folded = report_ops(
        "void h(num Function() cb) {}\n\
         void k() {\n\
           h(() => 5);\n\
         }\n",
    );
    assert_eq!(
        codes(&folded),
        Vec::<&str>::new(),
        "`int` re-typed under the `num` context assigns: silent"
    );
}

/// The tear-off twin (the oracle's `test_invocation_functionTypes_optional`
/// message): `acceptFunOptBool(funBool)` — a required-`bool` tear-off
/// rejects against the `[bool b]` formal because the required groups
/// disagree (`subtype.dart`'s positional walk).
#[test]
fn tear_off_required_group_mismatch_reports() {
    let folded = report_ops(
        "void acceptFunOptBool(void funNumOptBool([bool b])) {}\n\
         void funBool(bool b) {}\n\
         void main() {\n\
           acceptFunOptBool(funBool);\n\
         }\n",
    );
    let messages = atna_messages(&folded);
    assert!(
        messages.len() == 1
            && messages[0].contains("'void Function(bool)' can't be assigned")
            && messages[0].contains("'void Function([bool])'"),
        "the required-group face reports the tear-off pair: {messages:?}"
    );
}

//! #3956 pins — the leading-dot shorthand CALLEE face
//! (`const_eval_faces.rs` `classify_call`).
//!
//! Two rows were routed here from AR's #3921 report; both rode the same
//! blanket deferral: every shorthand callee took the `CallClass::ConstCtor`
//! branch, so nothing was checked at all.
//!
//! * **The method-vs-constructor split.** Upstream resolves the shorthand
//!   against the CONTEXT type and probes the member set FIRST
//!   (`method_invocation_resolver.dart`
//!   `_resolveReceiverTypeLiteralForDotShorthand`: `_resolveElement`'s
//!   getter/method lookup, the static executable taking the
//!   method-invocation face, and only a miss falling through to
//!   `getNamedConstructor`'s rewrite to a
//!   `DotShorthandConstructorInvocation`). [`crate::diagnose::Engine`]'s
//!   `shorthand_callee` mirrors that order over the model's member walk and
//!   the constructor tables, so `const A a = .method();` over a static
//!   method now reports `const_eval_method_invocation`.
//!
//! * **The shorthand creation's initializer walk.** `const A a = .new();`
//!   over a const constructor whose `assert` fails owes
//!   `const_eval_throws_exception`; the creation used to stage no
//!   `ConstCreationEntry` and the walk keyed the callee alone, which the
//!   shorthand spells as the SELECTOR. `ctor_init_key` re-derives the head
//!   from the declaration's context type, so the walk reaches the stash.
//!
//! The under-report direction is pinned explicitly: a shorthand whose
//! context type the model cannot resolve keeps the creation face and stays
//! silent.

use super::test_rig::report_ops;
use crate::FileReport;

/// The rows of one code, in source order.
fn rows_of(report: &FileReport, code: &str) -> Vec<String> {
    let mut at: Vec<(u32, String)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| (d.range.offset, d.message.to_string()))
        .collect();
    at.sort_by_key(|(offset, _)| *offset);
    at.into_iter().map(|(_, text)| text).collect()
}

/// Every code in the report, sorted.
fn codes(report: &FileReport) -> Vec<String> {
    let mut out: Vec<String> = report
        .diagnostics
        .iter()
        .map(|d| d.code.name.to_string())
        .collect();
    out.sort();
    out
}

/// `const A a = .method();` over a STATIC method of the context type: the
/// shorthand is a `DotShorthandInvocation` upstream, so the call is the
/// oracle's `const_eval_method_invocation` fault. Probed oracle row:
/// `CONST_EVAL_METHOD_INVOCATION` at the whole shorthand (`4:13`, length 9).
#[test]
fn shorthand_static_method_call_reports_method_invocation() {
    let report = report_ops(
        "class A {\n\
         \x20 static A method() => A();\n\
         }\n\
         const A a = .method();\n",
    );
    assert_eq!(
        rows_of(&report, "const_eval_method_invocation"),
        ["Methods can't be invoked in constant expressions."]
    );
}

/// The same split on a STATIC FIELD holding a function: `_resolveElement`
/// answers the getter, the invocation goes through the
/// `FunctionExpressionInvocation` rewrite — no `const_eval_method_invocation`
/// (probed oracle: only `const_initialized_with_non_constant_value` at the
/// shorthand). Pins that the discriminator does not overshoot into the
/// method arm for a non-executable member.
#[test]
fn shorthand_static_function_field_is_not_the_method_face() {
    let report = report_ops(
        "typedef F = A Function();\n\
         class A {\n\
         \x20 const A();\n\
         \x20 static const F k = _f;\n\
         }\n\
         A _f() => A();\n\
         const A a = .k();\n",
    );
    assert!(
        !codes(&report)
            .iter()
            .any(|c| c == "const_eval_method_invocation"),
        "{:?}",
        codes(&report)
    );
}

/// `const A a = .new();` over a const constructor whose `assert` fails:
/// the shorthand creation reaches the initializer walk through
/// `ctor_init_key`'s context-type head, so the fault reports at the
/// creation. Probed oracle row: `CONST_EVAL_THROWS_EXCEPTION` at `4:13`,
/// length 6 — identical to the plain `const A a = A();` row.
#[test]
fn shorthand_constructor_assert_fault_reports() {
    let report = report_ops(
        "class A {\n\
         \x20 const A() : assert(false);\n\
         }\n\
         const A a = .new();\n",
    );
    assert_eq!(
        rows_of(&report, "const_eval_throws_exception"),
        ["Evaluation of this constant expression throws an exception."]
    );
}

/// The named-constructor shorthand takes the same walk: `.named()` over a
/// const `A.named` with a failing assert. Probed oracle row:
/// `CONST_EVAL_THROWS_EXCEPTION` at `4:13`, length 9.
#[test]
fn shorthand_named_constructor_assert_fault_reports() {
    let report = report_ops(
        "class A {\n\
         \x20 const A.named() : assert(false);\n\
         }\n\
         const A a = .named();\n",
    );
    assert_eq!(
        rows_of(&report, "const_eval_throws_exception"),
        ["Evaluation of this constant expression throws an exception."]
    );
}

/// The control: the PLAIN creation form still reports — the walk's key
/// change must not regress the spelling it already answered.
#[test]
fn plain_constructor_assert_fault_still_reports() {
    let report = report_ops(
        "class A {\n\
         \x20 const A() : assert(false);\n\
         }\n\
         const A a = A();\n",
    );
    assert_eq!(
        rows_of(&report, "const_eval_throws_exception"),
        ["Evaluation of this constant expression throws an exception."]
    );
}

/// The reverse control: a passing shorthand creation stays silent (the
/// discriminator must not manufacture a fault for the valid case).
#[test]
fn valid_shorthand_constructor_is_silent() {
    let report = report_ops(
        "class A {\n\
         \x20 const A();\n\
         }\n\
         const A a = .new();\n",
    );
    assert!(
        !codes(&report).iter().any(|c| c.starts_with("const_eval")),
        "{:?}",
        codes(&report)
    );
}

/// The under-report direction, pinned: a context type this pass cannot
/// index (an SDK head with no in-file container) keeps the creation face,
/// so a method shorthand over it stays silent rather than inventing a
/// fault. `Duration` carries no in-file decl, so `shorthand_ctx_ty`
/// resolves but the member walk cannot answer — the defer arm.
#[test]
fn unresolvable_context_shorthand_stays_silent() {
    let report = report_ops("const Duration d = .new(seconds: 1);\n");
    assert!(
        !codes(&report).iter().any(|c| c.starts_with("const_eval")),
        "{:?}",
        codes(&report)
    );
}

/// An INSTANCE member is not the shorthand's answer upstream (there is no
/// receiver): the oracle reports `dot_shorthand_undefined_member` and the
/// initializer's `const_initialized_with_non_constant_value`, never
/// `const_eval_method_invocation`. Probed oracle rows for `const A a =
/// .m();` over `A m() => A();`: `CONST_INITIALIZED_WITH_NON_CONSTANT_VALUE`
/// at `4:13` plus `DOT_SHORTHAND_UNDEFINED_MEMBER` at `4:14`.
#[test]
fn shorthand_instance_method_is_not_the_method_face() {
    let report = report_ops(
        "class A {\n\
         \x20 A m() => A();\n\
         }\n\
         const A a = .m();\n",
    );
    assert!(
        !codes(&report)
            .iter()
            .any(|c| c == "const_eval_method_invocation"),
        "{:?}",
        codes(&report)
    );
    assert_eq!(
        rows_of(&report, "dot_shorthand_undefined_member"),
        ["The static method or constructor 'm' isn't defined for the context type 'A'."]
    );
}

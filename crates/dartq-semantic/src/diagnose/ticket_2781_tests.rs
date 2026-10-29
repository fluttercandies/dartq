//! The #2781 pins: the `invocation_of_non_function_expression` emission
//! face's callable exemptions against the family's conformance clusters
//! (target/conformance report, wave-2 W4). The resolver's decision order
//! (upstream Dart SDK source,
//! pkg/analyzer/lib/src/dart/resolver/function_expression_invocation_resolver.dart:55-145)
//! answers from the callee's PROMOTED static type: a function type is
//! callable (:67-79), any other type resolves `.call` through the
//! interface AND the accessible extensions (:92-100), a type parameter
//! resolves through its bound first (`resolveToBound`, :66), and a
//! record reports in both its arms (:103-121). Flow promotion of a
//! captured parameter rides the official write-capture gate only
//! (pkg/_fe_analyzer_shared/lib/src/flow_analysis/flow_analysis.dart:3172-3175
//! — `tryPromoteForTypeCheck` bails on `writeCaptured`; a variable merely
//! READ by a closure stays promotable, the co19 `Utils/async_utils.dart`
//! `error is Function(Object)` shape).

use super::test_rig::report_ops;
use super::*;

fn invocation_rows(report: &FileReport) -> Vec<&'static str> {
    report
        .diagnostics
        .iter()
        .filter(|d| {
            d.code.name == "invocation_of_non_function_expression"
                || d.code.name == "invocation_of_non_function"
        })
        .map(|d| d.code.name)
        .collect()
}

/// The co19 `Utils/async_utils.dart:179` shape (the family's dominant
/// cluster, 3,135 conformance rows share this one site): a parameter
/// captured-and-READ by a closure, tested `is Function(Object)` INSIDE
/// the closure, then invoked. The oracle promotes (the closure never
/// writes the variable — the official gate is write-capture only,
/// `tryPromoteForTypeCheck`'s `info.writeCaptured` bail,
/// pkg/_fe_analyzer_shared/lib/src/flow_analysis/flow_analysis.dart:3172-3175,
/// with `captured` = written-in-closure per
/// pkg/_fe_analyzer_shared/lib/src/type_inference/assigned_variables.dart:119-126)
/// and stays silent. dartq's flow layer records no promotion for the
/// function-form `is` test here — the deferred row re-decides on the
/// DECLARED face and reports. The missing fact lives in the
/// promotion/type-capture face (dartq-flow's `is`-test promotion and the
/// function-form spelling reduction), NOT in this invocation face — this
/// pin is the cluster's repro and flips green the moment the capture
/// carries the promotion.
///
/// #2783 evidence (flow pin
/// `dartq_flow::tests::closure_read_captured_parameter_promotes_through_the_is_test`,
/// green): the flow promotion IS recorded — the invoked callee's
/// `FlowFact::ReadType` row carries the promoted `Base::Function` chain
/// head. The blind face is this crate's consumer chain: the staged
/// translation degrades the signature-less function head to
/// `Ty::Unknown` (`staged_promotion_typed`, diagnose/types.rs, the #2174
/// substitution — the semantic `Ty::Function` needs an interned
/// `FnShapeId` the flow row does not carry), and the deferred invocation
/// face answers non-callable on Unknown
/// (`apply_deferred_invocation_faces`, diagnose/row_resolution.rs — its
/// `Ty::Function => callable` arm is unreachable for flow-staged rows).
/// The consumer fix (spell or mark the function-form promotion as
/// callable at the deferred face) belongs to the semantic lane and flips
/// this pin. Fixture fidelity note: the co19 original declares
/// `Object error` (Utils/async_utils.dart:169) — the oracle's promotion
/// step is valid only from a supertype of `dynamic Function(Object)`;
/// when the consumer fix lands, keep this fixture oracle-faithful (the
/// `Err` spelling admits no promotion step and the oracle itself
/// reports there).
#[ignore = "#2783: flow records the promotion (ReadType carries Base::Function — flow pin green); the semantic consumer chain is blind to it: staged_promotion_typed degrades the signature-less function head to Ty::Unknown (#2174) and apply_deferred_invocation_faces answers non-callable on Unknown. Fix the consumer face (semantic lane) to flip this pin; keep the fixture oracle-faithful (co19 declares Object error, not Err)."]
#[test]
fn closure_read_capture_promotes_for_the_invocation() {
    let report = report_ops(
        r#"
class Err {}
void run(void Function() action) {
  action();
}
void f(Err error) {
  run(() {
    if (error is Function(Object)) {
      error(0);
    }
  });
}
"#,
    );
    assert!(
        invocation_rows(&report).is_empty(),
        "{:?}",
        report.diagnostics
    );
}

/// The control: the same parameter invoked WITHOUT the type test keeps
/// the report — the exemption is the promotion, not the capture.
#[test]
fn untested_non_function_parameter_still_reports() {
    let report = report_ops(
        r#"
void f(int error) {
  error(0);
}
"#,
    );
    let rows = invocation_rows(&report);
    assert_eq!(rows, ["invocation_of_non_function_expression"], "{:?}", report.diagnostics);
}

/// The oracle `FunctionExpressionInvocationTest/
/// test_error_invocationOfNonFunction_OK_functionTypeTypeParameter` row:
/// a callee typed by a type parameter whose bound is a function-form
/// alias is callable — the resolver's `resolveToBound` step answers
/// `receiverType is FunctionTypeImpl` first. Silent at the field read
/// (the Value row) and at any bare spelling alike.
#[test]
fn type_param_function_bound_callee_is_callable() {
    let report = report_ops(
        r#"
typedef MyFunction = double Function(int _);
class C<T extends MyFunction> {
  T foo;
  C(this.foo);
  void main() {
    foo(0);
  }
}
"#,
    );
    assert!(
        invocation_rows(&report).is_empty(),
        "{:?}",
        report.diagnostics
    );
}

/// The oracle `FunctionExpressionInvocationTest/
/// test_expression_interfaceType_nullable_hasCall` row: an extension's
/// `call` method over a NULLABLE `on` type applies to the nullable
/// receiver — the type-property resolve's extension leg answers before
/// the report arms. Silent.
#[test]
fn nullable_extension_call_member_is_callable() {
    let report = report_ops(
        r#"
void f(int? a) {
  a();
}
extension N on int? {
  int call() => 0;
}
"#,
    );
    assert!(
        invocation_rows(&report).is_empty(),
        "{:?}",
        report.diagnostics
    );
}

/// The oracle `ExtensionMethodsInternalReferenceTest/test_instance_call`
/// row: `this(2)` inside an extension whose `on` type declares `call` —
/// the extension itself is the `.call` resolution the receiver climb
/// asks. Silent.
#[test]
fn this_invocation_finds_the_enclosing_extension_call() {
    let report = report_ops(
        r#"
class C {}
extension E on C {
  int call(int x) => 0;
  int m() => this(2);
}
"#,
    );
    assert!(
        invocation_rows(&report).is_empty(),
        "{:?}",
        report.diagnostics
    );
}

/// The oracle `FunctionExpressionInvocationTest/
/// test_expression_recordType_noCall` row: a record-typed callee reports
/// at the callee — the `recordField`/`needsGetterError` arms both report
/// (function_expression_invocation_resolver.dart:103-121).
#[test]
fn record_typed_callee_reports() {
    let report = report_ops(
        r#"
void f() {
  (int, String) r = (1, 'a');
  r();
}
"#,
    );
    assert_eq!(
        invocation_rows(&report),
        ["invocation_of_non_function_expression"],
        "{:?}",
        report.diagnostics
    );
}

/// A non-callable interface callee with no extension `call` in scope
/// still reports — the exemptions must not mute the base face.
#[test]
fn plain_interface_callee_without_call_still_reports() {
    let report = report_ops(
        r#"
class C {}
void f(C c) {
  c();
}
"#,
    );
    assert_eq!(
        invocation_rows(&report),
        ["invocation_of_non_function_expression"],
        "{:?}",
        report.diagnostics
    );
}

//! The #3841 pins: the nullable-operator operand pass's three
//! oracle-verified faces, each probed against `dart analyze` 3.13.7 (the
//! pinned oracle at `~/.dartq-oracle/3.13.7/dart-sdk/bin/dart`).
//!
//! - **InvalidType operands stay silent.** Upstream's
//!   `isInvalidBounded` short-circuit (type_property_resolver.dart:87-94)
//!   runs BEFORE the `isNullable` tail, so an operand whose static type
//!   is `InvalidType` never reaches the potentially-nullable consult:
//!   `x + 1` on an undefined `x` reports only `undefined_identifier`
//!   (no `unchecked_use_of_nullable_value`), `x + bar(7)` on a
//!   parameter declared `X?` over an unresolvable `X` stays silent, a
//!   flow-staged `String?` promotion of an undefined `x` inside
//!   `x is String? ? (x + 1) : z` is an InvalidType read upstream, and a
//!   member tail `x.y + 1` whose receiver resolved to nothing types the
//!   whole read `InvalidType`.
//! - **A `void` prefix operand reports.** `isPotentiallyNullable(void)`
//!   is true upstream, so `-x`/`~x` on a `void`-typed local or parameter
//!   emits the unchecked row at the operator beside `use_of_void_result`
//!   at the operand — the resolved lane's bare-read arm never reaches the
//!   nullable tail there, so this pass owns it.
//! - **The extension gate answers through the parameter's bound.**
//!   `applicableTo` subtypes `T` via `bound(T)`, so `t + t` inside
//!   `A<T extends C?>` resolves `extension on C?`'s `operator +` and the
//!   oracle reports nothing.

use crate::diagnose::test_rig::report_ops;
use crate::diagnose::types::FileReport;

/// Every `unchecked_use_of_nullable_value` row's rendered message, in
/// report order.
fn unchecked_messages(report: &FileReport) -> Vec<&str> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unchecked_use_of_nullable_value")
        .map(|d| d.message.as_str())
        .collect()
}

/// Every `unchecked_use_of_nullable_value` row's range offset, in report
/// order.
fn unchecked_offsets(report: &FileReport) -> Vec<u32> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unchecked_use_of_nullable_value")
        .map(|d| d.range.offset)
        .collect()
}

/// An undefined bare operand is the oracle's `InvalidType` — the
/// `isInvalidBounded` short-circuit clears the nullable consult before it
/// runs, so `x + 1` and `-x` report only `undefined_identifier`.
///
/// Probed 3.13.7: `UNDEFINED_IDENTIFIER` on each `x`, zero
/// `UNCHECKED_USE_OF_NULLABLE_VALUE` rows.
#[test]
fn ticket_3841_undefined_bare_operand_reports_no_unchecked_row() {
    let fixture = "void f() {\n\
         \x20 x + 1;\n\
         \x20 -x;\n\
         }\n";
    let report = report_ops(fixture);
    assert!(
        unchecked_messages(&report).is_empty(),
        "{:?}",
        crate::diagnose::test_rig::rows(&report)
    );
}

/// The staged-promotion variant of the same face: `x is String?` writes a
/// `String?` promotion the oracle never builds — upstream `x` is
/// `InvalidType`, and the `is`-tested branch's `x + 1` operand inherits
/// that face (InvalidType is not promoted).
///
/// Probed 3.13.7 (the `parser/generated` `test_is_nullable*` family):
/// `UNDEFINED_IDENTIFIER` on each `x`, `dead_code` on `z`, zero
/// `UNCHECKED_USE_OF_NULLABLE_VALUE` rows.
#[test]
fn ticket_3841_is_tested_undefined_operand_stays_invalid() {
    let fixture = "void f() {\n\
         \x20 x is String? ? (x + 1) : z;\n\
         }\n";
    let report = report_ops(fixture);
    assert!(
        unchecked_messages(&report).is_empty(),
        "{:?}",
        crate::diagnose::test_rig::rows(&report)
    );
}

/// A parameter declared `X?` over an unresolvable `X` is InvalidType —
/// the `TypeUse` verdict's `Missing` head — so `x + bar(7)` reports the
/// `undefined_*` rows alone (the `NNBDParserTest::test_assignment_complex`
/// shape).
///
/// Probed 3.13.7: `undefined_class` on `D`/`X`, `undefined_function` on
/// `bar`, zero `UNCHECKED_USE_OF_NULLABLE_VALUE` rows.
#[test]
fn ticket_3841_invalid_declared_operand_type_stays_silent() {
    let fixture = "D? foo(X? x) {\n\
         \x20 X? x1;\n\
         \x20 X? x2 = x + bar(7);\n\
         }\n";
    let report = report_ops(fixture);
    assert!(
        unchecked_messages(&report).is_empty(),
        "{:?}",
        crate::diagnose::test_rig::rows(&report)
    );
}

/// The positive twin: a parameter declared `X?` over a RESOLVED `X` is a
/// genuine nullable operand — `x + bar(7)` reports the unchecked row at
/// `+` exactly once.
///
/// Probed 3.13.7: one `UNCHECKED_USE_OF_NULLABLE_VALUE` at `+` —
/// `The operator '+' can't be unconditionally invoked because the
/// receiver can be 'null'.`
#[test]
fn ticket_3841_defined_nullable_operand_still_reports() {
    let fixture = "class D {}\n\
         class X {}\n\
         D? foo(X? x) {\n\
         \x20 X? x2 = x + bar(7);\n\
         }\n";
    let report = report_ops(fixture);
    assert_eq!(
        unchecked_messages(&report),
        vec![
            "The operator '+' can't be unconditionally invoked because the receiver can be 'null'."
        ],
        "{:?}",
        crate::diagnose::test_rig::rows(&report)
    );
}

/// The member-tail InvalidType face: `x.y + 1` where `x` resolves to
/// nothing types the read `InvalidType` upstream (the resolver never
/// runs the member lookup on the InvalidType receiver).
///
/// Probed 3.13.7: `undefined_identifier` on `x`, zero
/// `UNCHECKED_USE_OF_NULLABLE_VALUE` rows.
#[test]
fn ticket_3841_member_tail_on_invalid_receiver_stays_silent() {
    let fixture = "void f() {\n\
         \x20 x.y + 1;\n\
         }\n";
    let report = report_ops(fixture);
    assert!(
        unchecked_messages(&report).is_empty(),
        "{:?}",
        crate::diagnose::test_rig::rows(&report)
    );
}

/// `void` is potentially nullable upstream
/// (`isPotentiallyNullable`'s `VoidType` arm), so `-x` on a
/// `void`-declared parameter reports the unchecked `unary-` row at `-`
/// beside `use_of_void_result` at `x` — the resolved bare-read arm never
/// reaches the nullable tail on a `void` operand, so this pass owns the
/// row.
///
/// Probed 3.13.7 (`void_type_usage_test`'s `param_negation` family):
/// `UNCHECKED_USE_OF_NULLABLE_VALUE` at `-` (`The method 'unary-' can't
/// be unconditionally invoked because the receiver can be 'null'.`) plus
/// `USE_OF_VOID_RESULT` at `x`.
#[test]
fn ticket_3841_void_prefix_minus_reports_unary_unchecked() {
    let fixture = "void f(void x) {\n\
         \x20 -x;\n\
         }\n";
    let report = report_ops(fixture);
    assert_eq!(
        unchecked_messages(&report),
        vec![
            "The method 'unary-' can't be unconditionally invoked because the receiver can be 'null'."
        ],
        "{:?}",
        crate::diagnose::test_rig::rows(&report)
    );
}

/// The `~` twin on a `void` local — the same potentially-nullable tail,
/// the same operator anchor.
///
/// Probed 3.13.7: `UNCHECKED_USE_OF_NULLABLE_VALUE` at `~` (`The method
/// '~' can't be unconditionally invoked because the receiver can be
/// 'null'.`) plus `USE_OF_VOID_RESULT` at `x`.
#[test]
fn ticket_3841_void_prefix_tilde_reports_unchecked() {
    let fixture = "void f() {\n\
         \x20 void x;\n\
         \x20 ~x;\n\
         }\n";
    let report = report_ops(fixture);
    assert_eq!(
        unchecked_messages(&report),
        vec!["The method '~' can't be unconditionally invoked because the receiver can be 'null'."],
        "{:?}",
        crate::diagnose::test_rig::rows(&report)
    );
}

/// The binary `void` operand stays with the resolved lane — its
/// `check_operator_expr_in` `Void` arm already reports the unchecked row
/// at `+`; this pass must not double it (one row total, exactly the
/// resolved lane's anchor).
///
/// Probed 3.13.7: `UNCHECKED_USE_OF_NULLABLE_VALUE` at `+` plus
/// `USE_OF_VOID_RESULT` at `x` — one unchecked row total.
#[test]
fn ticket_3841_void_binary_operand_reports_exactly_once() {
    let fixture = "void f(void x) {\n\
         \x20 x + 1;\n\
         }\n";
    let report = report_ops(fixture);
    assert_eq!(
        unchecked_messages(&report),
        vec![
            "The operator '+' can't be unconditionally invoked because the receiver can be 'null'."
        ],
        "{:?}",
        crate::diagnose::test_rig::rows(&report)
    );
}

/// The extension application's parameter-bound receiver: upstream
/// `applicableTo` subtypes `T` through `bound(T)`, so `t + t` inside
/// `A<T extends C?>` resolves `extension on C?`'s `operator +` and the
/// oracle reports nothing (the `static_errors_A01_t1x` family — the same
/// shape against a class, a mixin class, an enum, and an extension
/// type).
///
/// Probed 3.13.7: zero diagnostics.
#[test]
fn ticket_3841_parameter_bound_satisfies_nullable_on_clause() {
    let fixture = "class C {}\n\
         extension on C? {\n\
         \x20 C? operator +(C? other) => other;\n\
         }\n\
         class A<T extends C?> {\n\
         \x20 T t;\n\
         \x20 A(this.t);\n\
         \x20 test() {\n\
         \x20   t + t;\n\
         \x20 }\n\
         }\n\
         main() {\n\
         \x20 C? c = null;\n\
         \x20 c + c;\n\
         \x20 A(null).test();\n\
         }\n";
    let report = report_ops(fixture);
    assert!(
        unchecked_messages(&report).is_empty(),
        "{:?}",
        crate::diagnose::test_rig::rows(&report)
    );
}

/// The bound chain's InvalidType face: `T extends X?` over an
/// unresolvable `X` is invalid-bounded upstream
/// (`isInvalidBounded`'s `TypeParameterTypeImpl` arm), so `t + t` keeps
/// the under-report posture.
///
/// Probed 3.13.7: `undefined_class` on `X`, zero
/// `UNCHECKED_USE_OF_NULLABLE_VALUE` rows.
#[test]
fn ticket_3841_invalid_bounded_parameter_operand_stays_silent() {
    let fixture = "class A<T extends X?> {\n\
         \x20 T t;\n\
         \x20 A(this.t);\n\
         \x20 test() {\n\
         \x20   t + t;\n\
         \x20 }\n\
         }\n";
    let report = report_ops(fixture);
    assert!(
        unchecked_messages(&report).is_empty(),
        "{:?}",
        crate::diagnose::test_rig::rows(&report)
    );
}

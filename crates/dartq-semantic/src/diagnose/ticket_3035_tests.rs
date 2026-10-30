//! The #3035 pins: the argument-arity family — the count faces
//! (`not_enough_positional_arguments`, `extra_positional_arguments` /
//! `extra_positional_arguments_could_be_named`, `missing_required_argument`)
//! and the type-argument arity faces
//! (`wrong_number_of_type_arguments`/`_element`/`_constructor`)
//! across the invocation shapes the argument-group transport carries no
//! callee row for. Upstream `resolveArgumentsToParameters`
//! (resolver.dart:4978-5009) runs the deficit/surplus verdicts
//! unconditionally once a parameter element resolved; the required-named
//! walk is `_checkRequired` (required_parameters_verifier.dart:134-218).

use super::test_rig::report_ops;
use crate::diagnose::FileReport;

fn count_code(report: &FileReport, name: &str) -> usize {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == name)
        .count()
}

// —— super-constructor invocation (`: super(...)`) ——————————————————

/// `D(super.x) : super(1)` — the written arguments plus the positional
/// `super.x` formals cover `B(int x, int y)`: no deficit
/// (resolver.dart:4930-4940/`verifySuperFormalParameters` feed
/// `positionalArgumentCount`; the gate at :4978 is unconditional).
#[test]
fn super_formals_cover_deficit() {
    let source = "\
class B { B(int x, int y); }
class D extends B { D(super.x) : super(1); }
";
    let report = report_ops(source);
    assert_eq!(
        count_code(&report, "not_enough_positional_arguments"),
        0,
        "{:?}",
        report.diagnostics
    );
}

/// `D(super.x) : super(1, 2, 3)` against `B(int x, int y)` — 1 written +
/// 1 super formal + 2 surplus = 4 against capacity 2:
/// `extra_positional_arguments` reports (resolver.dart:4993-5009).
#[test]
fn super_invocation_surplus_reports() {
    let source = "\
class B { B(int x, int y); }
class D extends B { D(super.x) : super(1, 2, 3); }
";
    let report = report_ops(source);
    assert_eq!(
        count_code(&report, "extra_positional_arguments"),
        1,
        "{:?}",
        report.diagnostics
    );
}

/// The deficit gate is unconditional (`resolveArgumentsToParameters`
/// :4978 fires `positionalArgumentCount < requiredParameterCount` even
/// when the enclosing constructor carries super formals):
/// `D(super.x) : super()` against `B(int x, int y)` — 0 written + 1
/// positional super formal = 1 < 2, so `not_enough_positional_arguments`
/// reports once.
#[test]
fn super_invocation_deficit_with_super_formals() {
    let source = "\
class B { B(int x, int y); }
class D extends B { D(super.x) : super(); }
";
    let report = report_ops(source);
    assert_eq!(
        count_code(&report, "not_enough_positional_arguments"),
        1,
        "{:?}",
        report.diagnostics
    );
}

/// A `required` named super-constructor formal no argument and no named
/// `super.x` covers reports `missing_required_argument`
/// (required_parameters_verifier.dart:122-132 `:134-218` — the
/// `super` arm's `_check` runs the named coverage with
/// `_containsNamedExpression`'s second walk crediting named super
/// formals).
#[test]
fn super_invocation_missing_required_named() {
    let source = "\
class B { B({required int y}); }
class D extends B { D() : super(); }
";
    let report = report_ops(source);
    assert_eq!(
        count_code(&report, "missing_required_argument"),
        1,
        "{:?}",
        report.diagnostics
    );
}

/// A named `super.y` formal stands in for the same-named argument
/// (`_containsNamedExpression`'s second walk,
/// required_parameters_verifier.dart:197-218) — `D(super.y) : super()`
/// against `B({required int y})` reports no
/// `missing_required_argument`.
#[test]
fn named_super_formal_covers_required_named() {
    let source = "\
class B { B({required int y}); }
class D extends B { D({required super.y}) : super(); }
";
    let report = report_ops(source);
    assert_eq!(
        count_code(&report, "missing_required_argument"),
        0,
        "{:?}",
        report.diagnostics
    );
}

// —— redirecting constructor (`this(...)`/`this.named(...)`) ————————

/// `A() : this.named(1)` against `A.named(int x, int y)` — 1 written
/// against 2 required: `not_enough_positional_arguments`
/// (`visitRedirectingConstructorInvocation` → `resolveInvocation`,
/// resolver.dart:4057-4085).
#[test]
fn redirecting_invocation_deficit() {
    let source = "\
class A {
  A.named(int x, int y);
  A() : this.named(1);
}
";
    let report = report_ops(source);
    assert_eq!(
        count_code(&report, "not_enough_positional_arguments"),
        1,
        "{:?}",
        report.diagnostics
    );
}

/// `A() : this.named(1, 2, 3)` against `A.named(int x)` — surplus
/// reports `extra_positional_arguments` (same pass).
#[test]
fn redirecting_invocation_surplus() {
    let source = "\
class A {
  A.named(int x);
  A() : this.named(1, 2, 3);
}
";
    let report = report_ops(source);
    assert_eq!(
        count_code(&report, "extra_positional_arguments"),
        1,
        "{:?}",
        report.diagnostics
    );
}

/// `A() : this.named()` against `A.named({required int y})` —
/// `missing_required_argument` reports
/// (required_parameters_verifier.dart:110-118, the redirecting arm).
#[test]
fn redirecting_invocation_missing_required_named() {
    let source = "\
class A {
  A.named({required int y});
  A() : this.named();
}
";
    let report = report_ops(source);
    assert_eq!(
        count_code(&report, "missing_required_argument"),
        1,
        "{:?}",
        report.diagnostics
    );
}

/// `A() : this.named(1, b: 2)` against `A.named(int x)` — `b:` names no
/// formal: `undefined_named_parameter` at the label (the same
/// `resolveArgumentsToParameters` pass).
#[test]
fn redirecting_invocation_undefined_named() {
    let source = "\
class A {
  A.named(int x);
  A() : this.named(1, b: 2);
}
";
    let report = report_ops(source);
    assert_eq!(
        count_code(&report, "undefined_named_parameter"),
        1,
        "{:?}",
        report.diagnostics
    );
}

// —— enum-constant argument list (`v(...)`) —————————————————————————

/// `enum E { v(0) }` whose only constructor is the synthetic unnamed
/// face — `ctor_formals` keyed on `E` is absent, the element's own
/// `formalParameters` is the empty list the unnamed `const E()` leaves:
/// the written argument is surplus,
/// `extra_positional_arguments` reports
/// (`visitEnumConstantDeclaration` → the constant's resolved
/// constructor's formals).
#[test]
fn enum_const_no_ctor_surplus() {
    let source = "\
enum E { v(0) }
";
    let report = report_ops(source);
    assert_eq!(
        count_code(&report, "extra_positional_arguments"),
        1,
        "{:?}",
        report.diagnostics
    );
}

/// `enum E { v(); const E({required int x}); }` — the required named
/// formal no `x:` argument covers reports `missing_required_argument`
/// at the constant's name (required_parameters_verifier.dart:60-66 —
/// `errorEntity: node.name`).
#[test]
fn enum_const_missing_required_named() {
    let source = "\
enum E { v(); const E({required int x}); }
";
    let report = report_ops(source);
    assert_eq!(
        count_code(&report, "missing_required_argument"),
        1,
        "{:?}",
        report.diagnostics
    );
}

// —— implicit creation's container arity (`C<int>(...)`) ————————————

/// `var c = Map<int>()` — the implicit-creation head is a
/// `Bare(Container)` shape: its declared arity (2) against the written
/// 1 reports `wrong_number_of_type_arguments` (upstream
/// `resolveTypeArguments` / `_checkTypeArgumentCount` on the
/// `ConstructorName`'s `type` — the class element's own arity).
#[test]
fn implicit_creation_class_arity() {
    let source = "\
f() {
  var c = Map<int>();
}
";
    let report = report_ops(source);
    assert_eq!(
        count_code(&report, "wrong_number_of_type_arguments"),
        1,
        "{:?}",
        report.diagnostics
    );
}

/// `var c = C<int, int>()` over `class C<T> {}` — 2 written against
/// 1 declared reports the same face at the head.
#[test]
fn implicit_creation_class_surplus_type_args() {
    let source = "\
class C<T> {}
f() {
  var c = C<int, int>();
}
";
    let report = report_ops(source);
    assert_eq!(
        count_code(&report, "wrong_number_of_type_arguments"),
        1,
        "{:?}",
        report.diagnostics
    );
}

// —— tear-off / method type-argument arity (`foo<int>`) ——————————————

/// `f() { g<int, int>; } void g<T>() {}` — a top-level function's
/// tear-off with surplus type arguments reports
/// `wrong_number_of_type_arguments_element` (`type_instantiation_target`'s
/// FunctionElement arm).
#[test]
fn top_level_fn_tearoff_type_arity() {
    let source = "\
void g<T>() {}
f() {
  g<int, int>;
}
";
    let report = report_ops(source);
    assert_eq!(
        count_code(&report, "wrong_number_of_type_arguments_element"),
        1,
        "{:?}",
        report.diagnostics
    );
}

// —— annotation (`@A(...)`) ——————————————————————————————————————————

/// `@A()` over `class A { const A({required int x}); }` — the metadata
/// invocation's `ctor_formals` resolves, the required named formal no
/// `x:` covers reports `missing_required_argument` (upstream
/// `visitAnnotation` runs `_check` with `errorEntity` on the
/// constructor identifier).
#[test]
fn annotation_missing_required_named() {
    let source = "\
class A { const A({required int x}); }
@A()
f() {}
";
    let report = report_ops(source);
    assert_eq!(
        count_code(&report, "missing_required_argument"),
        1,
        "{:?}",
        report.diagnostics
    );
}

// —— explicit-keyword unnamed creation (`const A()` / `new A()`) ————

/// `f() { const A(1); } class A { const A(int x, int y); }` — the
/// `unnamed_head` latch's resolved class supplies the unnamed
/// `ctor_formals` face: 1 written against 2 required reports
/// `not_enough_positional_arguments`
/// (`visitInstanceCreationExpression` → `resolveArgumentsToParameters`).
#[test]
fn const_creation_deficit() {
    let source = "\
class A { const A(int x, int y); }
f() {
  const A(1);
}
";
    let report = report_ops(source);
    assert_eq!(
        count_code(&report, "not_enough_positional_arguments"),
        1,
        "{:?}",
        report.diagnostics
    );
}

/// `f() { new A(1, 2, 3); } class A { A(int x); }` — the same unnamed
/// face's surplus reports `extra_positional_arguments`.
#[test]
fn new_creation_surplus() {
    let source = "\
class A { A(int x); }
f() {
  new A(1, 2, 3);
}
";
    let report = report_ops(source);
    assert_eq!(
        count_code(&report, "extra_positional_arguments"),
        1,
        "{:?}",
        report.diagnostics
    );
}

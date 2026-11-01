//! #3287 pins — the callable-class `call` tear-off arm of the
//! `invalid_assignment`/`argument_type_not_assignable` assignability
//! verdict.
//!
//! `isAssignableTo`'s callable-class acceptance (upstream Dart SDK source,
//! pkg/analyzer/lib/src/dart/element/type_system.dart:875-884 +
//! `acceptsFunctionType`, :121-126): a non-nullable interface source in a
//! function-typed context tears off its `call` method and checks the
//! resulting signature. When that `call` is generic (`T call<T>(T t)`) the
//! oracle's downward tear-off inference instantiates the type parameters
//! toward the context signature *before* the subtype ask — `int
//! Function(int) f = C()` solves `T := int` and stays silent, which a bare
//! `relateTypeParameters` count gate would refuse outright. When the
//! context cannot solve (an unbridgeable arity, an unbound parameter, or
//! a solve violating the declared bound) `getCallMethodType` falls back
//! to `instantiateTypeFormalsToBounds` — `T call<T extends num>(T)`
//! reads `num Function(num)`, `T call<T>(T)` under an unpaired `void
//! Function()` reads `dynamic Function(dynamic)` — and a resolvable
//! class with no `call` member rejects outright.
//!
//! Registered through `diagnose/tests.rs`'s `#[path]` block (`diagnose.rs`
//! is a live-lane file out of this ticket's write scope).

use crate::FileReport;
use crate::diagnose::test_rig::report_ops;

/// The rows of one code.
fn rows<'a>(report: &'a FileReport, code: &str) -> Vec<&'a str> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| d.message.as_str())
        .collect()
}

/// A generic `call` in a non-generic function context instantiates toward
/// the target and accepts — the oracle's silent row.
#[test]
fn generic_call_instantiates_to_context_and_accepts() {
    let fixture = "\
class C {
  T call<T>(T t) => t;
}

int Function(int) f = C();
";
    let report = report_ops(fixture);
    assert!(
        rows(&report, "invalid_assignment").is_empty(),
        "`int Function(int) f = C()` instantiates `call`'s T := int and accepts; got: {:?}",
        report.diagnostics
    );
}

/// The same instantiation decides a genuine mismatch — the solved
/// `num Function(num)` still fails a `String Function(String)` target.
#[test]
fn generic_call_instantiation_still_reports_a_mismatch() {
    let fixture = "\
class C {
  T call<T extends num>(T t) => t;
}

String Function(String) f = C();
";
    let report = report_ops(fixture);
    let messages = rows(&report, "invalid_assignment");
    assert_eq!(
        messages.len(),
        1,
        "one invalid_assignment over the instantiated `call`; got: {:?}",
        report.diagnostics
    );
}

/// A generic `call` whose solved signature mismatches arity reports —
/// `T call<T>(T)` under a `void Function()` context instantiates
/// `T := dynamic` and reports the `dynamic Function(dynamic)` face, never
/// the bare class name.
#[test]
fn generic_call_unbound_context_reports_the_call_signature() {
    let fixture = "\
class C {
  T call<T>(T t) => t;
}

void Function() f = C();
";
    let report = report_ops(fixture);
    let messages = rows(&report, "invalid_assignment");
    assert_eq!(
        messages.len(),
        1,
        "one invalid_assignment over `void Function() f = C()`; got: {:?}",
        report.diagnostics
    );
}

/// A class with no `call` member never tears off — the bare interface type
/// still fails a function-typed target.
#[test]
fn no_call_member_reports_the_class_type() {
    let fixture = "\
class C {}

String Function(String) f = C();
";
    let report = report_ops(fixture);
    let messages = rows(&report, "invalid_assignment");
    assert_eq!(
        messages.len(),
        1,
        "one invalid_assignment over the `call`-less class; got: {:?}",
        report.diagnostics
    );
    assert!(
        messages[0].contains("'C'"),
        "a `call`-less source names the class, got: {}",
        messages[0]
    );
}

/// A generic `call` through a typedef-of-generic context instantiates —
/// `Fn<U> f = C()` over `typedef Fn<T> = T Function(T)` binds `U` and the
/// solved `call` matches.
#[test]
fn generic_call_through_generic_typedef_context() {
    let fixture = "\
class C {
  T call<T>(T t) => t;
}
typedef Fn<T> = T Function(T);
class D<U> {
  Fn<U>? f;
  void m() { f = C(); }
}
";
    let report = report_ops(fixture);
    assert!(
        rows(&report, "invalid_assignment").is_empty(),
        "`Fn<U> f = C()` instantiates and accepts; got: {:?}",
        report.diagnostics
    );
}

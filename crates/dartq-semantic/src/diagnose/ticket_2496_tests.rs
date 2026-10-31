//! The #2496 pins: the `FutureOr`-aware receiver nullability plus the
//! `FutureOr` member face — `FutureOr<T>`'s nullability follows its
//! argument's own (`FutureOr<int>`/`var` stay NON-nullable, while
//! `FutureOr<int?>`/`FutureOr<dynamic>`/`FutureOr<void>`/an unresolved
//! argument read nullable — the upstream `_isNonNullable` `FutureOr` arm,
//! `pkg/analyzer/lib/src/dart/element/type_system.dart`), and a member
//! send over a `FutureOr<T>` receiver resolves `defined` through the
//! `Future` interface's member set (`then`/`catchError`/`whenComplete`/
//! `timeout`/`asStream`).
//!
//! Half 1 joins `future_or_potentially_nullable` onto the dead-chain
//! receiver gates' `is_nullable_ty` (`?[i]`/`?..` reads through a
//! nullable `FutureOr` exactly like a written `?` face). Half 2 adds the
//! `FutureOr` row to [`crate::member_face::CORE_TYPE_MEMBERS`] mirroring
//! `Future`'s own member row — the union typedef's element face resolves
//! to the Future's interface at the name level.

use crate::diagnose::test_rig::{code_names, report_ops_bare, rows};

/// Asserts a diagnostic `code` sits in the report.
fn assert_code(report: &super::FileReport, code: &str) {
    assert!(
        code_names(report).iter().any(|&c| c == code),
        "expected {code}: {:?}",
        rows(report)
    );
}

/// Asserts `code` is absent.
fn assert_no_code(report: &super::FileReport, code: &str) {
    assert!(
        !code_names(report).iter().any(|&c| c == code),
        "unexpected {code}: {:?}",
        rows(report)
    );
}

// A member send over a `FutureOr<T>` receiver resolves the selector
// through the `Future` member set — `x.then(...)` is `defined`, never
// `undefined_method`. The `Future`/`FutureOr`/`int`/`Object` declarations
// ride the fixture so `FutureOr<int>` resolves its typedef head.
const FUTURE_LIB: &str = "class Object {} class int {}\n\
     abstract class Future<T> {\n\
     \x20 Future<S> then<S>(S f(T t)) => throw 0;\n\
     \x20 Future<T> catchError(Function onError) => throw 0;\n\
     \x20 Future<T> whenComplete(void Function() action) => throw 0;\n\
     \x20 Future<T> timeout(Duration timeLimit) => throw 0;\n\
     \x20 Stream<T> asStream() => throw 0;\n\
     }\n\
     class Stream<T> {}\n\
     class Duration {}\n\
     typedef FutureOr<T> = Future<T>;\n";

/// `x.then(...)` over `FutureOr<int>` resolves `defined` — the selector
/// answers `Future.then`, never `undefined_method`.
#[test]
fn future_or_then_resolves_defined() {
    let source = "void f(FutureOr<int> x) {\n  x.then((v) => v);\n}";
    let report = report_ops_bare(&format!("{FUTURE_LIB}{source}"));
    assert_no_code(&report, "undefined_method");
}

/// `x.catchError(...)` over `FutureOr<int>` — the `catchError` selector is
/// in the `Future` member set, so `defined` (not `undefined_method`).
#[test]
fn future_or_catch_error_resolves_defined() {
    let source = "void f(FutureOr<int> x) {\n  x.catchError((e) => 0);\n}";
    let report = report_ops_bare(&format!("{FUTURE_LIB}{source}"));
    assert_no_code(&report, "undefined_method");
}

/// `x.whenComplete(...)` + `x.timeout(...)` + `x.asStream()` — the whole
/// Future member row is reachable through the `FutureOr` union face.
#[test]
fn future_or_member_set_resolves_defined() {
    let source = "void f(FutureOr<int> x) {\n\
                    \x20 x.whenComplete(() {});\n\
                    \x20 x.timeout(Duration());\n\
                    \x20 var s = x.asStream();\n\
                    }";
    let report = report_ops_bare(&format!("{FUTURE_LIB}{source}"));
    assert_no_code(&report, "undefined_method");
    assert_no_code(&report, "undefined_getter");
}

/// A non-Future selector over `FutureOr<int>` still reports
/// `undefined_method` — the member set is `Future`'s, not everything's.
#[test]
fn future_or_unknown_selector_still_reports() {
    let source = "void f(FutureOr<int> x) {\n  x.noSuchFutureMember();\n}";
    let report = report_ops_bare(&format!("{FUTURE_LIB}{source}"));
    assert_code(&report, "undefined_method");
}

/// `FutureOr<int>` receiver stays NON-nullable: a `then` call over it
/// reports no `unchecked_use_of_nullable_value` — the argument's
/// non-nullable face keeps the receiver verdict non-nullable (the
/// keep-honest direction: `FutureOr<int>` is not `int?`-shaped).
#[test]
fn future_or_non_nullable_arg_stays_non_nullable() {
    let source = "void f(FutureOr<int> x) {\n  x.then((v) => v);\n}";
    let report = report_ops_bare(&format!("{FUTURE_LIB}{source}"));
    assert_no_code(&report, "unchecked_use_of_nullable_value");
    assert_no_code(&report, "undefined_method");
}

/// `x ??= 1` on `FutureOr<int>` — a non-nullable target's `??=` write
/// reports the dead coalesce face (the right operand is never evaluated),
/// matching the declared-non-null verdict.
#[test]
fn future_or_int_assignment_stays_non_nullable() {
    let source = "void f(FutureOr<int> x) {\n  x ??= x.then((v) => v);\n}";
    let report = report_ops_bare(&format!("{FUTURE_LIB}{source}"));
    assert_code(&report, "dead_null_aware_expression");
}

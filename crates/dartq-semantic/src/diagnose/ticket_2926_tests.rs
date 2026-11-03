//! The #2926 pins: the two #2919 residuals closed at push time, plus the
//! rider's fourth `instantiated_head_ty` adoption.
//!
//! 1. F2 — the bare sends inside `extension on A?`: a bare `foo()` /
//!    `foo` / `foo = 0` resolves the selector through the extension's
//!    `on` face while the enclosing implicit `this` stays nullable, so
//!    the oracle's `type_property_resolver` nullable tail reports the
//!    shared unchecked code at the selector (upstream Dart SDK source,
//!    `pkg/analyzer/lib/src/dart/resolver/type_property_resolver.dart`).
//!    The variant follows the parentNode shape: `foo()` and the
//!    assignment-shaped `foo = 0` (an `AssignmentExpression`, which
//!    implements `MethodReferenceExpression`) report Method; the bare
//!    read `foo` reports Property. The push-time latch
//!    (`ext_nullable_member_rows`, set in `push_bare` where the frame
//!    still knows the enclosing nullable-`on` extension) feeds the
//!    emit-before-record_miss posture #2919 established.
//! 2. The bare write's second job: `foo = 0` probes the `foo=` setter
//!    spelling the read-side `resolve_bare` never tried, so pass 2's
//!    `undefined_identifier` stands down on `ext_write_target_rows` —
//!    nullability-independent (a `set foo` on `A` inside `extension on A`
//!    is silent either way).
//! 3. F4 — the parenthesized type-parameter receiver: `(t).foo` where
//!    `T` has no bound reads `Object?` in the oracle (a boundless
//!    parameter's erased bound), so `foo` reports the unchecked property
//!    row. `exec_param_bound` now falls back to a container-owned window
//!    (`class C<T>`'s `(T, None)`) when no executable window declares
//!    the parameter.
//! 4. Rider — `fold_alias_ty`'s terminal bare head adopts
//!    `instantiated_head_ty` (the fourth I2B fill point #2925 listed):
//!    `typedef H = A` over `class A<X extends num>` resolves `H`-typed
//!    receivers to `A<num>`, so `h.missing` quotes `A<num>` like the
//!    oracle (`Instantiate-to-bound/class_typedef_l1_t03` family).
//!
//! Oracle provenance: the #2926 census rows —
//! `UncheckedUseOfNullableValueInsideExtensionTest/test_methodInvocation_
//! nullable#0` (`foo()` at 8:5), `test_propertyAccess_getter_nullable#0`,
//! `test_propertyAccess_setter_nullable#0` (`foo = 0` at 10:5 Method;
//! `this.foo = 0` at 11:10 Property), `test_propertyAccess_setter_
//! nonNullable#0` (silent), `PropertyAccessResolutionTest/test_
//! targetTypeParameter_noBound#0` (`(t).foo` at 4:9 Property), and the
//! co19 `Instantiate-to-bound` `class_typedef_l1` family — `dart
//! analyze`, Dart 3.13.

use super::test_rig::{CORE_PRELUDE, report_ops};
use crate::diagnose::FileReport;

/// The rows of `report` carrying `code`, sorted by offset —
/// `(offset, length, message)`.
fn rows_named(report: &FileReport, code: &str) -> Vec<(u32, u32, String)> {
    let mut rows: Vec<(u32, u32, String)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| (d.range.offset, d.range.length, d.message.to_string()))
        .collect();
    rows.sort();
    rows
}

/// The `unchecked_use_of_nullable_value` rows of `report`.
fn unchecked_rows(report: &FileReport) -> Vec<(u32, u32, String)> {
    rows_named(report, "unchecked_use_of_nullable_value")
}

/// Byte offset of `needle` inside the fixture source, shifted by the
/// rig's core prelude.
fn at(source: &str, needle: &str) -> u32 {
    (CORE_PRELUDE.len() + source.find(needle).expect("needle in fixture")) as u32
}

// —— F2 shape 1: the bare method invocation ——

/// `extension E on A?` with `foo` on `A`: the bare `foo()` reports the
/// method variant at the selector — the `on`-face member send on the
/// nullable implicit `this`. Census `test_methodInvocation_nullable#0`
/// (oracle row at 8:5, Method). `bar()` — E's own member — and the
/// `this?.foo()` null-aware send stand down.
#[test]
fn bare_method_invocation_nullable_on_reports_method() {
    let source = r#"
class A {
  void foo() {}
}

extension E on A? {
  void bar() {}
  void baz() {
    foo();
    this?.foo();
    bar();
  }
}
"#;
    let report = report_ops(source);
    let rows = unchecked_rows(&report);
    assert_eq!(
        rows.len(),
        1,
        "only the bare foo() reports: {rows:?} {:?}",
        report.diagnostics
    );
    let (offset, length, message) = &rows[0];
    assert_eq!((*offset, *length), (at(source, "foo();"), 3), "{rows:?}");
    assert!(
        message.contains("The method 'foo' can't be unconditionally invoked"),
        "the method variant: {rows:?}"
    );
}

// —— F2 shape 2: the bare getter read ——

/// The bare read `foo` (a `set foo`/`get foo` pair on `A`) reports the
/// property variant — the resolver's `propertyErrorEntity` is the
/// SimpleIdentifier itself, not an invocation-shaped parent. Census
/// `test_propertyAccess_getter_nullable#0` (oracle row Property).
#[test]
fn bare_getter_read_nullable_on_reports_property() {
    let source = r#"
class A {
  int get foo => 0;
}

extension E on A? {
  void baz() {
    foo;
  }
}
"#;
    let report = report_ops(source);
    let rows = unchecked_rows(&report);
    assert_eq!(
        rows.len(),
        1,
        "the bare foo read reports: {rows:?} {:?}",
        report.diagnostics
    );
    let (offset, length, message) = &rows[0];
    assert_eq!((*offset, *length), (at(source, "foo;"), 3), "{rows:?}");
    assert!(
        message.contains("The property 'foo' can't be unconditionally accessed"),
        "the property variant: {rows:?}"
    );
}

// —— F2 shape 3: the bare setter write ——

/// `foo = 0` reports the METHOD variant — upstream's
/// `AssignmentExpression implements MethodReferenceExpression`, so the
/// `parentNode` dispatch lands the Method code even though the send is
/// a write (the `this.foo = 0` twin reports Property — its
/// `propertyErrorEntity` rides the `PropertyAccess`). Census
/// `test_propertyAccess_setter_nullable#0` (oracle rows at 10:5 Method,
/// 11:10 Property). `bar = 0` — E's own setter — stays silent.
#[test]
fn bare_setter_write_nullable_on_reports_method() {
    let source = r#"
class A {
  set foo(int _) {}
}

extension E on A? {
  set bar(int _) {}

  void baz() {
    foo = 0;
    this?.foo = 0;
    bar = 0;
  }
}
"#;
    let report = report_ops(source);
    let rows = unchecked_rows(&report);
    assert_eq!(
        rows.len(),
        1,
        "only the bare foo = 0 reports: {rows:?} {:?}",
        report.diagnostics
    );
    let (offset, length, message) = &rows[0];
    assert_eq!((*offset, *length), (at(source, "foo = 0;"), 3), "{rows:?}");
    assert!(
        message.contains("The method 'foo' can't be unconditionally invoked"),
        "the assignment-shaped send reports the method variant: {rows:?}"
    );
}

// —— F2 shape 4: the write-side resolution ——

/// The `foo = 0` write resolves the `foo=` setter spelling: no
/// `undefined_identifier` rides the unchecked row. The spelling probe
/// is nullability-independent — `extension on A` with `set foo` on `A`
/// is equally silent (census `test_propertyAccess_setter_nonNullable#0`,
/// oracle `-`).
#[test]
fn bare_setter_write_reports_no_undefined_identifier() {
    let nullable = report_ops(
        r#"
class A {
  set foo(int _) {}
}

extension E on A? {
  void baz() {
    foo = 0;
  }
}
"#,
    );
    assert!(
        rows_named(&nullable, "undefined_identifier").is_empty(),
        "the foo= spelling resolved: {:?}",
        nullable.diagnostics
    );

    let non_nullable = report_ops(
        r#"
class A {
  set foo(int _) {}
}

extension E on A {
  void baz() {
    foo = 0;
  }
}
"#,
    );
    assert!(
        non_nullable.diagnostics.is_empty(),
        "the non-nullable on stays fully silent: {:?}",
        non_nullable.diagnostics
    );
}

/// The own-setter twin: `bar = 0` where `set bar` lives on `E` itself
/// resolves lexically (the ExtensionMemberScope wins over `this`), so
/// the nullable `on` never fires unchecked for it — and the lexical
/// resolution suppresses `undefined_identifier` outright.
#[test]
fn own_extension_setter_stays_silent_under_nullable_on() {
    let report = report_ops(
        r#"
class A {}

extension E on A? {
  set bar(int _) {}

  void baz() {
    bar = 0;
  }
}
"#,
    );
    assert!(
        report.diagnostics.is_empty(),
        "bar resolves through E's own member scope: {:?}",
        report.diagnostics
    );
}

// —— F4: the parenthesized type-parameter receiver ——

/// `(t).foo` where `T` has no bound (`class C<T> { f(T t) }`): the
/// oracle erases a boundless parameter to `Object?`, so `foo` reports
/// the unchecked property row — never `undefined_getter` (`foo` is a
/// member-miss on the `Object?` face, not an undefined name). Census
/// `PropertyAccessResolutionTest/test_targetTypeParameter_noBound#0`
/// (oracle row at 4:9 Property).
#[test]
fn parenthesized_unbound_param_receiver_reports_unchecked() {
    let source = r#"
class C<T> {
  void f(T t) {
    (t).foo;
  }
}
"#;
    let report = report_ops(source);
    let rows = unchecked_rows(&report);
    assert_eq!(
        rows.len(),
        1,
        "the parenthesized (t).foo reports: {rows:?} {:?}",
        report.diagnostics
    );
    let (offset, length, message) = &rows[0];
    assert_eq!((*offset, *length), (at(source, "foo;"), 3), "{rows:?}");
    assert!(
        message.contains("The property 'foo' can't be unconditionally accessed"),
        "the property variant: {rows:?}"
    );
    assert!(
        rows_named(&report, "undefined_getter").is_empty(),
        "a member-miss, not an undefined name: {:?}",
        report.diagnostics
    );
}

// —— rider: the fourth I2B fill point ——

/// `fold_alias_ty`'s terminal bare head still decides the VERDICT
/// through the instantiated expansion (`typedef H = A` over `class
/// A<X extends num>` — `H`'s `h` is not assignable to `A<int>` because
/// the fold's `A<num>` face fails the arg check), but the MESSAGE
/// spells the written alias use `'H'` — the oracle's
/// `preferTypeAlias` face keeps the use's spelling wherever the
/// resolved type carries the `alias` reference (`#3685`'s bind
/// posture). Oracle row (`dart analyze` 3.13.7, `~/.dartq-oracle/
/// 3.13.7`, the fixture verbatim): `error - main.dart:4:14 - A value
/// of type 'H' can't be assigned to a variable of type 'A<int>'. -
/// invalid_assignment`. The earlier `A<num>` expectation pinned the
/// pre-#3685 expansion-spelled message.
#[test]
fn bare_alias_head_instantiates_through_fold() {
    let source = r#"
class A<X extends num> {}
typedef H = A;
void f(H h) {
  A<int> x = h;
}
"#;
    let report = report_ops(source);
    let rows = rows_named(&report, "invalid_assignment");
    assert_eq!(rows.len(), 1, "one invalid_assignment row: {rows:?}");
    assert!(
        rows[0].2.contains("'H'"),
        "the value face spells the written alias use `H`: {rows:?}"
    );
}

//! The #3836 pins — the `undefined_method` receiver text of the
//! container-static invoke face (`container_member_verdict`).
//!
//! Census `undefined_method` (freshest report: 49 missing / 171 extra).
//! Six same-span pairs were message-only: both sides reported
//! `undefined_method` at the same `(file, line:col, length)`, and only the
//! receiver text differed. Every one of them rode the static class face
//! and carried the declaration's type parameters, while the oracle names
//! the bare interface element.
//!
//! Upstream (Dart SDK 3.13.7) splits the trio's texts:
//!   - `_reportUndefinedMethodOrNew(receiver, methodName)` reports
//!     `diag.undefinedMethod` with `typeName: receiver.displayName` — the
//!     interface ELEMENT's name, never a parameterised spelling
//!     (`pkg/analyzer/lib/src/dart/resolver/method_invocation_resolver.dart:418-446`).
//!   - The getter/setter twin (`:1180`) names `receiverType.element.name`
//!     — bare as well — while the STATIC property face
//!     (`lookup_failure_reporter.dart`'s `interface.thisType`) keeps the
//!     declaration's own parameter spellings.
//!
//! Pins, oracle 3.13.7:
//!   - `C.gone()` over `class C<T>` reports `undefined_method` naming `C`
//!     (`C<T>` before the fix; the same row as the census's
//!     MethodInvocationResolutionTest test_error_undefinedMethod_hasTarget_class_typeParameter
//!     and type_variable/conflict2_test pairs).
//!   - `A.gone()` over `typedef A = List<int>` names `List` — the aliased
//!     interface element, not the alias and not `List<int>`
//!     (UndefinedMethodTest test_typeAlias_interfaceType).
//!   - `C<int> c; c.gone()` (the instance face) names `C` — both faces
//!     agree.
//!   - `C.goneStatic` KEEPS `C<T>`: the read face's `thisType` text is the
//!     positive direction pinned beside the invoke fix.

use super::test_rig::{code_names, report_ops, rows};

/// `C.gone()` over `class C<T>` names the bare element `C`.
#[test]
fn static_invoke_miss_names_bare_generic_element() {
    let report = report_ops(
        r#"
class C<T> {
  static int s = 1;
}

void f() {
  C.gone();
}
"#,
    );
    assert_eq!(
        code_names(&report),
        ["undefined_method"],
        "{:#?}",
        rows(&report)
    );
    let row = rows(&report).remove(0);
    assert!(
        row.contains("The method 'gone' isn't defined for the type 'C'."),
        "expected the bare element name, got {row}"
    );
}

/// A typedef head naming a generic interface reports through the aliased
/// element — `List`, not `List<int>` and not the alias.
#[test]
fn static_invoke_miss_over_interface_alias_names_aliased_element() {
    let report = report_ops(
        r#"
typedef A = List<int>;

void f() {
  A.gone();
}
"#,
    );
    assert_eq!(
        code_names(&report),
        ["undefined_method"],
        "{:#?}",
        rows(&report)
    );
    let row = rows(&report).remove(0);
    assert!(
        row.contains("The method 'gone' isn't defined for the type 'List'."),
        "expected the aliased element name, got {row}"
    );
}

/// The instance face names the bare element too — the two faces must not
/// drift apart (`c.gone()` over `C<int>` reports `C`).
#[test]
fn instance_invoke_miss_names_bare_generic_element() {
    let report = report_ops(
        r#"
class C<T> {}

void f() {
  C<int> c = C<int>();
  c.gone();
}
"#,
    );
    let names = code_names(&report);
    assert!(names.contains(&"undefined_method"), "{:#?}", rows(&report));
    let row = rows(&report)
        .into_iter()
        .find(|row| row.contains("undefined_method"))
        .expect("undefined_method row");
    assert!(
        row.contains("The method 'gone' isn't defined for the type 'C'."),
        "expected the bare element name, got {row}"
    );
}

/// The positive direction on the same edit: the STATIC property read keeps
/// the declaration's `thisType`, parameters included (`C<T>`, probed
/// 3.13.7 — `undefined_getter` is the `interface.thisType` face, a
/// different text from the invoke face's element name).
#[test]
fn static_getter_miss_keeps_parameterised_this_type() {
    let report = report_ops(
        r#"
class C<T> {
  static int s = 1;
}

void f() {
  C.goneStatic;
}
"#,
    );
    assert_eq!(
        code_names(&report),
        ["undefined_getter"],
        "{:#?}",
        rows(&report)
    );
    let row = rows(&report).remove(0);
    assert!(
        row.contains("The getter 'goneStatic' isn't defined for the type 'C<T>'."),
        "expected the declaration's thisType text, got {row}"
    );
}

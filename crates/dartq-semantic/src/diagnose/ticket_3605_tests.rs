//! The #3605 pins — the static-extensions tail of the container-static
//! access face (`container_member_verdict`'s `resolveStaticExtension`
//! arm) and the generic `thisType` miss text.
//!
//! Census `undefined_getter` (freshest report: 135 missing / 290 extra).
//! The extra side's biggest cluster was the `A.foo` shape over
//! `extension E on A { static ... }`: the oracle's
//! `resolveStaticExtension` binds a unique static member of the
//! on-declaration's extensions (`type_property_resolver.dart:245`),
//! which the row face never consulted — every such access fabricated
//! `undefined_getter` (ExtensionMethodsExternalReferenceTest's 11
//! `test_static_*_success_*` cases).
//!
//! Pins, oracle 3.13.7:
//!   - `A.foo` over `extension E on A { static int get foo => 0; }`
//!     resolves — silent (class/mixin/enum/extension-type alike).
//!   - `A.foo` over `extension E on A<String> { static ... }` still
//!     resolves: the on-declaration is the element, the written
//!     arguments never enter the match.
//!   - `C.a` over `static int a = 1;` reports `undefined_getter`: the
//!     induced getter+setter pair is already two entries — the oracle's
//!     ambiguity, which still reports the access-intent miss.
//!   - `FutureOr.foo` reports `undefined_getter` naming `FutureOr<T>`:
//!     `on FutureOr` has no on-declaration (the union's element is
//!     excluded upstream), and the static-face text is `thisType` — a
//!     generic declaration keeps its own parameter spellings
//!     (`List.gone` names `List<E>`).
//!   - `C.m()` over `static void m()` still reports `undefined_method`:
//!     the invocation face does not ride the static-extension tail.

use super::test_rig::{assert_silent, code_names, report_ops, rows};

// `A.foo` over a unique static extension getter on the accessed
// declaration resolves — the oracle binds `E::foo`, no row. Probed
// 3.13.7: silent.
#[test]
fn static_extension_getter_on_class_resolves() {
    let report = report_ops(
        r#"
class A {}

extension E on A {
  static int get foo => 0;
}

void f() {
  A.foo;
}
"#,
    );
    assert!(
        !code_names(&report).contains(&"undefined_getter"),
        "fabricated undefined_getter: {:#?}",
        rows(&report)
    );
}

// The generic `on` head matches by ELEMENT, not written arguments —
// `extension E on A<String>` still declares `A`'s static members
// (probed 3.13.7: `A.foo` silent).
#[test]
fn static_extension_getter_on_generic_class_resolves() {
    let report = report_ops(
        r#"
class A<X> {}

extension E on A<String> {
  static int get foo => 0;
}

void f() {
  A.foo;
}
"#,
    );
    assert!(
        !code_names(&report).contains(&"undefined_getter"),
        "fabricated undefined_getter: {:#?}",
        rows(&report)
    );
}

// The typedef on-declaration unfolds to the interface element both
// ways: `extension E on T` over `typedef T = A` answers `A.foo`, and
// `extension E on A` answers `T.foo` (probed 3.13.7: both silent).
#[test]
fn static_extension_getter_via_typedef_resolves() {
    let report = report_ops(
        r#"
class A {}

typedef T = A;

extension E on T {
  static int get foo => 0;
}

void f() {
  A.foo;
  T.foo;
}
"#,
    );
    assert!(
        !code_names(&report).contains(&"undefined_getter"),
        "fabricated undefined_getter: {:#?}",
        rows(&report)
    );
}

// A lone static setter answers the write — and a compound write's BOTH
// faces (the single member clears `_needsGetterError` and
// `_needsSetterError` alike, `resolveStaticExtension` — `C.s += 1`
// stays fully silent, probed 3.13.7).
#[test]
fn static_extension_setter_on_class_resolves() {
    let report = report_ops(
        r#"
class A {}

extension E on A {
  static set s(int v) {}
}

void f() {
  A.s = 1;
  A.s += 1;
}
"#,
    );
    assert_silent(&report);
}

// A mutable static field's induced getter+setter is two entries — the
// oracle's ambiguity still reports the access-intent miss (probed
// 3.13.7: `undefined_getter` on `C.a`, `undefined_setter` on `C.a = 2`).
#[test]
fn static_extension_field_stays_ambiguous() {
    let report = report_ops(
        r#"
class A {}

extension E on A {
  static int a = 1;
}

void f() {
  A.a;
  A.a = 2;
}
"#,
    );
    let codes = code_names(&report);
    assert!(
        codes.contains(&"undefined_getter"),
        "expected ambiguous-field undefined_getter: {:#?}",
        rows(&report)
    );
    assert!(
        codes.contains(&"undefined_setter"),
        "expected ambiguous-field undefined_setter: {:#?}",
        rows(&report)
    );
}

// The miss-side pin: a name the on-declaration's extensions do not
// carry still reports `undefined_getter` on the accessed type.
#[test]
fn static_extension_miss_still_reports() {
    let report = report_ops(
        r#"
class A {}

extension E on A {
  static int get foo => 0;
}

void f() {
  A.gone;
}
"#,
    );
    let codes = code_names(&report);
    assert_eq!(codes, ["undefined_getter"]);
    assert!(
        rows(&report)
            .iter()
            .any(|row| row.contains("'gone' isn't defined for the type 'A'")),
        "expected miss naming A: {:#?}",
        rows(&report)
    );
}

// The generic `thisType` text: `C.gone` over `class C<T>` names `C<T>`,
// the declaration's own parameter spellings — the element's
// `interface.thisType`, never the bare head (probed 3.13.7).
#[test]
fn generic_container_miss_names_this_type() {
    let report = report_ops(
        r#"
class C<K, V> {}

void f() {
  C.gone;
}
"#,
    );
    assert!(
        rows(&report)
            .iter()
            .any(|row| row.contains("'gone' isn't defined for the type 'C<K, V>'")),
        "expected thisType text C<K, V>: {:#?}",
        rows(&report)
    );
}

// The invocation face does NOT ride the static-extension tail: `C.m()`
// over `extension E on C { static void m() {} }` reports
// `undefined_method` naming the type (probed 3.13.7).
#[test]
fn static_extension_invoke_still_reports() {
    let report = report_ops(
        r#"
class C {}

extension E on C {
  static void m() {}
}

void f() {
  C.m();
}
"#,
    );
    let codes = code_names(&report);
    assert_eq!(codes, ["undefined_method"]);
    assert!(
        rows(&report)
            .iter()
            .any(|row| row.contains("'m' isn't defined for the type 'C'")),
        "expected undefined_method naming C: {:#?}",
        rows(&report)
    );
}

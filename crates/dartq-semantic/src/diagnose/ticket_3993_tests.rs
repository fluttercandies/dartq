//! #3993 pins — `concrete_class_with_abstract_member` consulted only the
//! supertype-chain member map; `argument_type_not_assignable` let a
//! signature interned on `InvalidType` reject through its arity gates.
//!
//! Face A — `interface_has_concrete` now mirrors upstream's `implemented`
//! map (`inheritance_manager3.dart` `getImplemented`): the container's own
//! concrete member, the supertype chain's concrete member, the
//! field-induced accessor pair (`name=` answers through a non-`final`
//! field's `name` entry), the implicit `Object`/`Enum` faces every
//! interface carries, the non-`Object` `noSuchMethod` forwarders
//! (`inheritance_manager3.dart:696-708` — a concrete `noSuchMethod` fills
//! every unimplemented name), and the complete augmentation fragments of
//! the member (an `augment` body makes the merged element concrete even
//! though the primary fragment's `is_abstract` bit never folds).
//!
//! Face B — `fn_sig_subtype` abstains ahead of its structural gates when
//! either signature carries an `InvalidType` face: the interned shape is
//! derived from an unresolved face, so its arity answers cannot be
//! trusted (upstream `isSubtypeOf`'s InvalidType-suppresses rule,
//! `subtype.dart:45-47`). The gate lives in `assignability.rs` under the
//! #3869 lane's scope; this file pins only the negative side.

use super::test_rig::{report_ops, report_ops_augment_intro_experiments};
use crate::FileReport;

/// The row count of one code.
fn count_code(report: &FileReport, code: &str) -> usize {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .count()
}

/// `class A { bool operator ==(other); const A(); }` — the `==` face is
/// filled by `Object`'s concrete `==`, which every interface carries
/// (`element.supertype` answers `Object` for a clause-less header). The
/// oracle reports nothing; the member-map BFS bottomed out before the
/// implicit `Object` hop and invented the extra.
#[test]
fn object_equals_fills_abstract_equals() {
    let report = report_ops("class A { bool operator ==(other); const A(); }\n");
    assert_eq!(
        count_code(&report, "concrete_class_with_abstract_member"),
        0,
        "{:#?}",
        report.diagnostics
    );
}

/// `class A { void foo(); noSuchMethod(_) {} }` — a concrete
/// `noSuchMethod` not declared on `Object` synthesizes a forwarder for
/// every unimplemented interface name (`inheritance_manager3.dart`
/// `_implemented[name] ??= forwarder`), so `foo` counts as implemented
/// and the oracle stays silent.
#[test]
fn nosuchmethod_forwarder_fills_abstract_member() {
    let report = report_ops("class A { void foo(); noSuchMethod(_) {} }\n");
    assert_eq!(
        count_code(&report, "concrete_class_with_abstract_member"),
        0,
        "{:#?}",
        report.diagnostics
    );
}

/// `Object`'s own `noSuchMethod` does NOT count (the upstream boundary
/// `_isDeclaredInObject`): `class A { void foo(); }` keeps its
/// `concrete_class_with_abstract_member` row.
#[test]
fn object_nosuchmethod_does_not_fill() {
    let report = report_ops("class A { void foo(); }\n");
    assert_eq!(
        count_code(&report, "concrete_class_with_abstract_member"),
        1,
        "{:#?}",
        report.diagnostics
    );
}

/// `class B { int foo = 0; } class A extends B { void set foo(int v); }` —
/// the concrete field `foo` induces the `foo=` setter on `B`'s interface,
/// so `A`'s abstract `set foo` is implemented. The member map keys the
/// field under `foo`, so a bare `foo=` lookup misses the pair.
#[test]
fn inherited_field_induces_setter() {
    let report =
        report_ops("class B { int foo = 0; } class A extends B { void set foo(int v); }\n");
    assert_eq!(
        count_code(&report, "concrete_class_with_abstract_member"),
        0,
        "{:#?}",
        report.diagnostics
    );
}

/// The augmentation-chain arm: `augment class A { augment void foo() {}
/// }` completes the abstract `foo` fragment — the merged element is
/// concrete upstream, even though the stored member entry keeps the
/// introducing fragment's `is_abstract` bit.
#[test]
fn augment_body_completes_abstract_member() {
    let report = report_ops_augment_intro_experiments(
        "class A { void foo(); }\n",
        "augment class A { augment void foo() {} }\n",
        &[dartq_language::Feature::Augmentations],
    );
    assert_eq!(
        count_code(&report, "concrete_class_with_abstract_member"),
        0,
        "{:#?}",
        report.diagnostics
    );
}

/// The field-augmentation arm: `augment int foo = 0;` induces the `foo=`
/// setter (not `final`), completing `A`'s abstract `set foo`.
#[test]
fn field_augment_induces_setter() {
    let report = report_ops_augment_intro_experiments(
        "class A { void set foo(int v); }\n",
        "augment class A { augment int foo = 0; }\n",
        &[dartq_language::Feature::Augmentations],
    );
    assert_eq!(
        count_code(&report, "concrete_class_with_abstract_member"),
        0,
        "{:#?}",
        report.diagnostics
    );
}

/// The Face-B abstention gate lives in `assignability.rs`
/// (`fn_sig_subtype`'s `sig_has_unknown` head guard) — the
/// `argfx(void f(x))`/`topx(x)` shape stays silent once an
/// `InvalidType`-bearing signature abstains ahead of the arity gates.
/// The deciding pin for that file rides the #3869 lane's scope; this
/// file only pins the negative side — a decided shape must keep its row.
/// `void f(int i) {}` against `void g(String s) {}` keeps its
/// `argument_type_not_assignable` row (the contravariant `int`/`String`
/// param ask decides, no `InvalidType` involved).
#[test]
fn decided_fn_mismatch_still_reports() {
    let report = report_ops("void f(int i) {} void g(String s) {} main() { f(g); }\n");
    assert_eq!(
        count_code(&report, "argument_type_not_assignable"),
        1,
        "{:#?}",
        report.diagnostics
    );
}

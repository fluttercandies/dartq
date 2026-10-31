//! #3083 — the `undefined_*` residual clusters D–H.
//!
//! Two faces land here; every expectation was probed against oracle
//! `dart analyze` 3.13.7:
//!
//! - Object-pattern field getters (`resolveObjectPatternPropertyGet`,
//!   pkg/analyzer/lib/src/dart/analysis; `type_analyzer.dart`). The head's
//!   static type folds a written alias away for the MEMBER lookup while the
//!   row keeps the written head spelling ("The getter 'foo' isn't defined
//!   for the type 'Ali'."), and every interface-bearing head answers: an
//!   extension type's own members, a mixin's `on` chain. A function
//!   right-hand side owns no field column at all, so every name misses
//!   (`typedef Fn = void Function()` reports the alias spelling).
//! - Primary-constructor formal scope
//!   (pkg/analyzer/lib/src/dart/resolver/scope_context.dart:406-461): the
//!   formals live in `ConstructorInitializerScope` — the non-late INSTANCE
//!   field initializers — and `PrimaryParameterScope` — the `this : ...`
//!   body — alone. A static or `late` field initializer reads
//!   `undefined_identifier`, while a plain instance field initializer
//!   resolves.

use super::*;
use crate::diagnose::test_rig::report_ops;

fn messages(report: &FileReport, code: &str) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| d.message.to_string())
        .collect()
}

/// An extension type head resolves the field on the extension type's own
/// member face; a clean miss names the head (oracle: `ET(:var foo)` over
/// `extension type ET(int it) {}` → "for the type 'ET'").
#[test]
fn object_pattern_extension_type_head_reports() {
    let report = report_ops(
        "extension type ET(int it) {}\n\
         void f(Object o) { if (o case ET(:var foo)) {} }\n",
    );
    let getters = messages(&report, "undefined_getter");
    assert_eq!(
        getters,
        ["The getter 'foo' isn't defined for the type 'ET'."],
        "{getters:?}"
    );
}

/// A mixin head reads its own members and its `on` chain; a clean miss
/// names the mixin (oracle: `M(:var foo)` over `mixin M {}` → 'M').
#[test]
fn object_pattern_mixin_head_reports() {
    let report = report_ops(
        "mixin M {}\n\
         void f(Object o) { if (o case M(:var foo)) {} }\n",
    );
    let getters = messages(&report, "undefined_getter");
    assert_eq!(
        getters,
        ["The getter 'foo' isn't defined for the type 'M'."],
        "{getters:?}"
    );
}

/// A function-type alias head has no field column: every name misses at the
/// written alias spelling (oracle: `Fn(:var foo)` over
/// `typedef Fn = void Function();` → 'Fn').
#[test]
fn object_pattern_function_alias_head_reports() {
    let report = report_ops(
        "typedef Fn = void Function();\n\
         void f(Object o) { if (o case Fn(:var foo)) {} }\n",
    );
    let getters = messages(&report, "undefined_getter");
    assert_eq!(
        getters,
        ["The getter 'foo' isn't defined for the type 'Fn'."],
        "{getters:?}"
    );
}

/// An alias of a class folds for the member lookup but the row keeps the
/// ALIAS spelling (oracle: `Ali(:var foo)` over `typedef Ali = A;` → 'Ali',
/// never 'A').
#[test]
fn object_pattern_class_alias_head_keeps_alias_spelling() {
    let report = report_ops(
        "class A { int bar = 0; }\n\
         typedef Ali = A;\n\
         void f(Object o) { if (o case Ali(:var foo)) {} }\n",
    );
    let getters = messages(&report, "undefined_getter");
    assert_eq!(
        getters,
        ["The getter 'foo' isn't defined for the type 'Ali'."],
        "{getters:?}"
    );
}

/// A declared field resolves through the head's member face — no row.
#[test]
fn object_pattern_resolved_field_is_silent() {
    let report = report_ops(
        "class A { int bar = 0; }\n\
         void f(Object o) { if (o case A(:var bar)) {} }\n",
    );
    let getters = messages(&report, "undefined_getter");
    assert!(getters.is_empty(), "{getters:?}");
}

/// A static field initializer is outside the primary constructor's scopes:
/// the formal does not resolve there (oracle:
/// `class A(int foo) { static var s = foo; }` → "Undefined name 'foo'.").
#[test]
fn primary_ctor_formal_hidden_from_static_field_initializer() {
    let report = report_ops("class A(int foo) { static var s = foo; }\n");
    let names = messages(&report, "undefined_identifier");
    assert_eq!(names, ["Undefined name 'foo'."], "{names:?}");
}

/// The same shape with the `late` modifier: a `late` instance field
/// initializer is outside `ConstructorInitializerScope` too.
#[test]
fn primary_ctor_formal_hidden_from_late_field_initializer() {
    let report = report_ops("class A(int foo) { late var l = foo; }\n");
    let names = messages(&report, "undefined_identifier");
    assert_eq!(names, ["Undefined name 'foo'."], "{names:?}");
}

/// A plain instance field initializer IS the primary constructor's
/// initializer scope — the formal resolves and stays row-free.
#[test]
fn primary_ctor_formal_visible_from_instance_field_initializer() {
    let report = report_ops("class A(int foo) { var i = foo; }\n");
    let names = messages(&report, "undefined_identifier");
    assert!(names.is_empty(), "{names:?}");
}

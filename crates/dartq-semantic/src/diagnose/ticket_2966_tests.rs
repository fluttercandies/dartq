//! The #2966 pins (UNDEF-2): the member verdict on receivers whose
//! static type is a SHAPE, not a container — a `Ty::Function` (a
//! `var`-inferred closure, a `int Function()` formal, a method
//! tear-off), a `Ty::Record`, and a typedef-to-shape head — plus the
//! `call` carve-out on the bare `Function` head. Upstream resolves
//! these through `type_property_resolver.dart`'s non-interface arms
//! (:202-234): `call` and a record's own fields resolve first, the
//! extension pool answers next, the `Object` interface closes the
//! lookup, and a clean miss reports the access-intent trio —
//! `undefined_getter`/`_setter` naming the receiver's own type display
//! (property_element_resolver.dart:619-625's `targetType`), a record
//! miss `undefined_method` naming `<unknown>` and a function miss
//! `Function` (method_invocation_resolver.dart:1190-1199).
//!
//! Oracle provenance: `dart analyze` 3.13.1 — every expectation below
//! was probed against the real binary before the fix.

use super::test_rig::{assert_silent, code_names, report_ops, rows};

/// `f.bar` on a `var`-inferred closure reports the getter face against
/// the receiver's own shape display.
#[test]
fn var_fn_receiver_read_reports_undefined_getter() {
    let report = report_ops("void f() { var g = () => 0; g.bar; }\n");
    assert_eq!(
        code_names(&report),
        vec!["undefined_getter"],
        "{:?}",
        rows(&report)
    );
}

/// `f.bar()` on the same receiver reports the method face against the
/// constant `Function` display (the invocation resolver's
/// `receiverClassName` for a `FunctionType`).
#[test]
fn var_fn_receiver_invoke_reports_undefined_method() {
    let report = report_ops("void f() { var g = () => 0; g.bar(); }\n");
    assert_eq!(
        code_names(&report),
        vec!["undefined_method"],
        "{:?}",
        rows(&report)
    );
}

/// `f.bar = 1` reports the setter face.
#[test]
fn var_fn_receiver_write_reports_undefined_setter() {
    let report = report_ops("void f() { var g = () => 0; g.bar = 1; }\n");
    assert_eq!(
        code_names(&report),
        vec!["undefined_setter"],
        "{:?}",
        rows(&report)
    );
}

/// `call` resolves the receiver's own signature — the
/// `callFunctionType` resolution (`type_property_resolver.dart:202-209`).
#[test]
fn var_fn_receiver_call_resolves() {
    let report = report_ops("void f() { var g = () => 0; g.call; g.call(); }\n");
    assert_silent(&report);
}

/// `Object`'s own members resolve on a function receiver — the
/// interface walk's last leg answers them.
#[test]
fn var_fn_receiver_object_members_resolve() {
    let report = report_ops("void f() { var g = () => 0; g.toString; g.hashCode; g == g; }\n");
    assert_silent(&report);
}

/// A `int Function()` formal carries the same face.
#[test]
fn fn_typed_formal_receiver_reports() {
    let report = report_ops("void f(int Function() p) { p.bar; }\n");
    assert_eq!(
        code_names(&report),
        vec!["undefined_getter"],
        "{:?}",
        rows(&report)
    );
}

/// A nullable function receiver reports the shared unchecked code —
/// `isNullable`'s tail never reaches the `undefined_*` family.
#[test]
fn nullable_fn_receiver_reports_unchecked() {
    let report = report_ops("void f(int Function()? p) { p.bar; }\n");
    assert!(
        code_names(&report).contains(&"unchecked_use_of_nullable_value"),
        "{report:?}"
    );
}

/// `?.` unwraps the nullable receiver first — the miss still reports
/// the getter face.
#[test]
fn null_aware_fn_receiver_reports_undefined_getter() {
    let report = report_ops("void f(int Function()? p) { p?.bar; }\n");
    assert_eq!(
        code_names(&report),
        vec!["undefined_getter"],
        "{:?}",
        rows(&report)
    );
}

/// A method tear-off's member access reads its own `void Function()`
/// face.
#[test]
fn method_tear_off_receiver_reports() {
    let report = report_ops("class C { void m() { m.bar; } }\n");
    assert_eq!(
        code_names(&report),
        vec!["undefined_getter"],
        "{:?}",
        rows(&report)
    );
}

/// `typedef F = void Function();` — the alias folds to the function
/// shape; the miss names the written alias head.
#[test]
fn typedef_fn_receiver_reports_undefined_getter() {
    let report = report_ops(
        "typedef F = void Function();\n\
         void f(F fn) { fn.bar; }\n",
    );
    assert_eq!(
        code_names(&report),
        vec!["undefined_getter"],
        "{:?}",
        rows(&report)
    );
}

/// The typedef head's `call` resolves the same way — the folded shape
/// carries its own signature.
#[test]
fn typedef_fn_receiver_call_resolves() {
    let report = report_ops(
        "typedef F = void Function();\n\
         void f(F fn) { fn.call; fn.call(); }\n",
    );
    assert_silent(&report);
}

/// `f.call` on a bare `Function` receiver resolves — the
/// `isDartCoreFunction` arm (`type_property_resolver.dart:194-198`).
/// Before the fix the named walk reported `undefined_getter`/
/// `undefined_method` for a member the `Function` interface never
/// declared.
#[test]
fn named_function_call_resolves() {
    let report = report_ops("void f(Function g) { g.call; g.call(); }\n");
    assert_silent(&report);
}

/// The named `Function` face still reports ordinary misses — the
/// carve-out covers `call` alone.
#[test]
fn named_function_member_miss_reports() {
    let report = report_ops("void f(Function g) { g.bar; g.bar(); }\n");
    assert_eq!(
        code_names(&report),
        vec!["undefined_getter", "undefined_method"],
        "{:?}",
        rows(&report)
    );
}

/// `r.foo` on a record receiver reports the getter face against the
/// record display.
#[test]
fn record_receiver_read_reports_undefined_getter() {
    let report = report_ops("void f() { var r = (1, \"a\"); r.foo; }\n");
    assert_eq!(
        code_names(&report),
        vec!["undefined_getter"],
        "{:?}",
        rows(&report)
    );
}

/// `r.foo()` on a record reports the method face against `<unknown>`
/// (the invocation resolver's `receiverClassName` default).
#[test]
fn record_receiver_invoke_reports_undefined_method() {
    let report = report_ops("void f() { var r = (1, \"a\"); r.foo(); }\n");
    assert_eq!(
        code_names(&report),
        vec!["undefined_method"],
        "{:?}",
        rows(&report)
    );
}

/// `r.foo = 1` on a record reports the setter face.
#[test]
fn record_receiver_write_reports_undefined_setter() {
    let report = report_ops("void f() { var r = (1, \"a\"); r.foo = 1; }\n");
    assert_eq!(
        code_names(&report),
        vec!["undefined_setter"],
        "{:?}",
        rows(&report)
    );
}

/// A record's own fields resolve (`fieldByName`) — positional `$n`
/// and named slots stay silent.
#[test]
fn record_own_fields_resolve() {
    let report = report_ops("void f() { var r = (1, s: \"a\"); r.\x241; r.s; }\n");
    assert_silent(&report);
}

/// An extension `on` the exact function shape resolves the member —
/// the application's first leg.
#[test]
fn extension_on_fn_shape_resolves() {
    let report = report_ops(
        "extension E on int Function() { int get bar => 0; }\n\
         void f() { var g = () => 0; g.bar; }\n",
    );
    assert_silent(&report);
}

/// An extension `on Function` applies to every function shape — the
/// named-supertypes leg of the application.
#[test]
fn extension_on_named_function_resolves() {
    let report = report_ops(
        "extension E on Function { int get bar => 0; }\n\
         void f() { var g = () => 0; g.bar; }\n",
    );
    assert_silent(&report);
}

/// An extension `on Object` applies to a non-nullable function
/// receiver; a nullable one never reaches it — `int Function()?` is
/// not assignable to `Object`.
#[test]
fn extension_on_object_splits_on_nullability() {
    let report = report_ops(
        "extension E on Object { int get bar => 0; }\n\
         void f(int Function()? p) { p.bar; }\n",
    );
    assert!(
        code_names(&report).contains(&"unchecked_use_of_nullable_value"),
        "{report:?}"
    );
    let report = report_ops(
        "extension E on Object { int get bar => 0; }\n\
         void f() { var g = () => 0; g.bar; }\n",
    );
    assert_silent(&report);
}

/// An extension `on Object?` accepts the nullable receiver — the
/// access resolves, no unchecked row.
#[test]
fn extension_on_nullable_object_accepts_nullable_receiver() {
    let report = report_ops(
        "extension E on Object? { int get bar => 0; }\n\
         void f(int Function()? p) { p.bar; }\n",
    );
    assert_silent(&report);
}

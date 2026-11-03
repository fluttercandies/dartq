//! The #3429 pins (EXTRA-row suppression on shape/object-pattern
//! receivers): `resolveObjectPatternPropertyGet`
//! (resolver.dart:1644-1693) is an ORDINARY property consult — the same
//! `type_property_resolver.dart` machinery as `a.foo` — so a
//! function-typed pattern head resolves `call` on its own signature, the
//! extension pool answers next, and the `Object` interface closes the
//! lookup; a clean miss alone spells `undefined_getter` on the alias
//! spelling. The nullable arm (:103-185) never reaches the `undefined_*`
//! family — `Object` members and applicable `on`-nullable extensions
//! resolve silently and a genuine miss reports the unchecked code at the
//! TYPE token, a span the pending getter triple does not carry (the
//! verdict stands down — the established under-report direction, and the
//! nullable-head face already spells the unchecked row).
//!
//! The member face rides the same consult through
//! `function_extension_face`: `applicableTo`'s subtype ask
//! (applicable_extensions.dart:251-270) — a concrete `on` shape decides
//! by covariance/contravariance over the signature (`on void Function()`
//! accepts an `int Function()` literal), and an `on` face spelling the
//! extension's own parameters binds each bare own-param leaf off the
//! receiver's aligned slot (`extension E<T> on T Function(T)`).
//!
//! Oracle provenance: `dart analyze` 3.13.7 — the expectations mirror
//! `ObjectPatternResolutionTest`
//! (test_typedef_functionType_notGeneric_hasName_{extensionGetter,
//! hashCode,unresolved}, test_typedef_functionType_generic_
//! withTypeArguments_hasName_extensionGetter, test_typeAlias_nullable)
//! and the member verdict's shape arms (upstream
//! type_property_resolver.dart:186-234).

use super::test_rig::{assert_silent, code_names, report_ops, rows};

/// `Fn(hashCode: var y)` over `typedef Fn = void Function()` resolves
/// `dart:core::Object::hashCode` — the interface lookup's last leg
/// (upstream test_typedef_functionType_notGeneric_hasName_hashCode).
#[test]
fn pattern_fn_head_object_member_silent() {
    let report = report_ops(
        "typedef Fn = void Function();\n\
         void f(Object? x, Object? sink) {\n\
           if (x case Fn(hashCode: var y)) { sink = y; }\n\
         }\n",
    );
    assert_silent(&report);
}

/// `Fn(foo: var y)` over the same head is the oracle's
/// `undefined_getter` "for the type 'Fn'" — the alias spelling at the
/// field name (upstream
/// test_typedef_functionType_notGeneric_hasName_unresolved).
#[test]
fn pattern_fn_head_genuine_miss_reports() {
    let report = report_ops(
        "typedef Fn = void Function();\n\
         void f(Object? x, Object? sink) {\n\
           if (x case Fn(foo: var y)) { sink = y; }\n\
         }\n",
    );
    assert_eq!(
        code_names(&report),
        vec!["undefined_getter"],
        "{:?}",
        rows(&report)
    );
}

/// An extension getter on the exact folded shape resolves the field
/// (upstream test_typedef_functionType_notGeneric_hasName_
/// extensionGetter).
#[test]
fn pattern_fn_head_extension_getter_silent() {
    let report = report_ops(
        "typedef Fn = void Function();\n\
         extension E on void Function() { int get foo => 0; }\n\
         void f(Object? x, Object? sink) {\n\
           if (x case Fn(foo: var y)) { sink = y; }\n\
         }\n",
    );
    assert_silent(&report);
}

/// A generic alias's instantiated fold still reads the `on` shape
/// (upstream test_typedef_functionType_generic_withTypeArguments_
/// hasName_extensionGetter — `Fn<int>` folds to `int Function()`).
#[test]
fn pattern_fn_head_generic_alias_extension_silent() {
    let report = report_ops(
        "typedef Fn<T> = T Function();\n\
         extension E on int Function() { int get foo => 0; }\n\
         void f(Object? x, Object? sink) {\n\
           if (x case Fn<int>(foo: var y)) { sink = y; }\n\
         }\n",
    );
    assert_silent(&report);
}

/// `call` resolves a non-nullable function head's own signature
/// (type_property_resolver.dart:202-209).
#[test]
fn pattern_fn_head_call_silent() {
    let report = report_ops(
        "typedef Fn = void Function();\n\
         void f(Object? x, Object? sink) {\n\
           if (x case Fn(call: var y)) { sink = y; }\n\
         }\n",
    );
    assert_silent(&report);
}

/// A nullable folded FUNCTION head never spells `undefined_getter` —
/// upstream's `isNullable` branch reports the unchecked family at the
/// TYPE token (upstream test_typeAlias_nullable's
/// `uncheckedPropertyAccessOfNullableValue`), a span the pending triple
/// does not carry, so the verdict stands down on a genuine miss while
/// `Object` members still resolve.
#[test]
fn pattern_nullable_fn_head_stands_down() {
    let report = report_ops(
        "typedef Fn = void Function()?;\n\
         void f(Object? x, Object? sink) {\n\
           if (x case Fn(foo: var y)) { sink = y; }\n\
           if (x case Fn(hashCode: var z)) { sink = z; }\n\
         }\n",
    );
    assert_silent(&report);
}

/// A nullable NAMED head (`typedef B = C?`) stands down the same way —
/// the `undefined_getter` row is suppressed while the nullable-head
/// face's `unchecked_use_of_nullable_value` row (the oracle's
/// `uncheckedPropertyAccessOfNullableValue`) stands.
#[test]
fn pattern_nullable_named_head_stands_down() {
    let report = report_ops(
        "class C {}\n\
         typedef B = C?;\n\
         void f(Object? x, Object? sink) {\n\
           if (x case B(foo: var y)) { sink = y; }\n\
         }\n",
    );
    let names = code_names(&report);
    assert_eq!(names.len(), 1, "{:?}", rows(&report));
    assert!(
        names[0].starts_with("unchecked_use_of_nullable_value"),
        "{:?}",
        rows(&report)
    );
}

/// `dynamic`'s own shape stays conservative (the established silence —
/// `isDynamicBounded` resolves through `Object` recovery only).
#[test]
fn pattern_dynamic_head_stays_silent() {
    let report = report_ops(
        "typedef A = dynamic;\n\
         void f(Object? x, Object? sink) {\n\
           if (x case A(foo: var y)) { sink = y; }\n\
         }\n",
    );
    assert_silent(&report);
}

/// The member face's `applicableTo` subtype ask: `on void Function()`
/// accepts the `int Function()` closure literal (return covariance —
/// the exact-shape equality the consult replaced missed it).
#[test]
fn fn_literal_receiver_extension_on_super_shape_silent() {
    let report = report_ops(
        "extension E on void Function() { int get bar => 0; }\n\
         void f() { var g = () => 0; g.bar; }\n",
    );
    assert_silent(&report);
}

/// A genuine miss on the non-nullable shape still reports — the
/// suppression must not stand down the `undefined_*` family.
#[test]
fn fn_literal_receiver_genuine_miss_reports() {
    let report = report_ops("void f() { var g = () => 0; g.bar; }\n");
    assert_eq!(
        code_names(&report),
        vec!["undefined_getter"],
        "{:?}",
        rows(&report)
    );
}

/// A nullable shape receiver's miss is the unchecked row, never
/// `undefined_getter` (`isNullable` precedes the family).
#[test]
fn nullable_fn_receiver_miss_reports_unchecked() {
    let report = report_ops("void f(void Function()? g) { g.bar; }\n");
    let names = code_names(&report);
    assert_eq!(names.len(), 1, "{:?}", rows(&report));
    assert!(
        names[0].starts_with("unchecked_use_of_nullable_value"),
        "{:?}",
        rows(&report)
    );
}

/// A `!= null`-guarded read promotes the receiver to its non-null
/// function shape — `g.bar` resolves no member on `void Function()`
/// and reports `undefined_getter`, exactly the oracle's verdict
/// (`dart analyze` 3.13.7, `~/.dartq-oracle/3.13.7`, the fixture
/// verbatim: `error - main.dart:1:49 - The getter 'bar' isn't defined
/// for the type 'void Function()'. - undefined_getter`). The earlier
/// `unchecked_use_of_nullable_value` expectation pinned the
/// pre-promotion posture (the flow's `promoted` face never spelled the
/// function type), which the wave's promoted-receiver read fixed.
#[test]
fn guarded_fn_receiver_miss_reports_undefined_getter() {
    let report =
        report_ops("void f(void Function()? g) { if (g != null) { g.bar; } }\n");
    let names = code_names(&report);
    assert_eq!(names.len(), 1, "{:?}", rows(&report));
    assert!(
        names[0].starts_with("undefined_getter"),
        "{:?}",
        rows(&report)
    );
}

/// A `?.`-unwrapped receiver consults the `on` face at the promoted
/// non-null shape — `g?.bar` over `void Function()?` resolves `on void
/// Function()`'s member (upstream's `applicableTo` runs against the
/// promoted type, never `T?`).
#[test]
fn null_aware_fn_receiver_extension_resolves() {
    let report = report_ops(
        "extension E on void Function() { int get bar => 0; }\n\
         void f(void Function()? g) { g?.bar; }\n",
    );
    assert_silent(&report);
}

/// `extension E<T> on T Function(T)`: each bare own-param leaf binds
/// the receiver's aligned slot — `T := int` over `int Function(int)`
/// applies the face (upstream `constrainArgument` +
/// `tryChooseFinalTypes`).
#[test]
fn param_on_function_face_binds_receiver_silent() {
    let report = report_ops(
        "extension E<T> on T Function(T) { int get foo => 0; }\n\
         void f(int Function(int) g) { g.foo; }\n",
    );
    assert_silent(&report);
}

/// The same face with an unsatisfiable-but-solvable interval: `T`
/// collects `>= int` (covariant return) and `<= num` (contravariant
/// formal) over `int Function(num)` — `T := int` solves and the face
/// applies (upstream `constrainArgument`'s interval solution).
#[test]
fn param_on_function_face_interval_binds_silent() {
    let report = report_ops(
        "extension E<T> on T Function(T) { int get foo => 0; }\n\
         void f(int Function(num) g) { g.foo; }\n",
    );
    assert_silent(&report);
}

/// An unsatisfiable binding interval (`T >= int` against `T <= String`)
/// leaves the face inapplicable — the genuine miss reports.
#[test]
fn param_on_function_face_inconsistent_reports() {
    let report = report_ops(
        "extension E<T> on T Function(T) { int get foo => 0; }\n\
         void f(int Function(String) g) { g.foo; }\n",
    );
    assert_eq!(
        code_names(&report),
        vec!["undefined_getter"],
        "{:?}",
        rows(&report)
    );
}

/// A bound the binding violates keeps the face inapplicable —
/// `tryChooseFinalTypes` rejects `T := int` under `T extends String`.
#[test]
fn param_on_function_face_bound_violation_reports() {
    let report = report_ops(
        "extension E<T extends String> on T Function(T) { int get foo => 0; }\n\
         void f(int Function(int) g) { g.foo; }\n",
    );
    assert_eq!(
        code_names(&report),
        vec!["undefined_getter"],
        "{:?}",
        rows(&report)
    );
}

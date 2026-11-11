//! #3881 pins — two `scopes_bodies.rs` faces.
//!
//! (1) A bare generic alias head (`Foo f` over
//! `typedef Foo<S> = S Function<T>(T x)`) resolves to the
//! instantiate-to-bounds instantiation (`Foo<dynamic>` = `dynamic
//! Function<T>(T)` — `instantiateToBounds`, type_system.dart:719-731), so
//! the closure literal opens under the substituted context: no
//! `body_might_complete_normally 'S'` row (the raw right-hand side's
//! parameter return) and no `invalid_assignment 'S Function(T)'` against
//! the declared `Foo<dynamic>` face.
//!
//! (2) The closure-solver's disagreeing-sibling merge certifies the shared
//! supertype head under ONE instantiation on both sides — the official
//! interface LUB's supertype-set intersection (`_computeLeastUpperBound`,
//! least_upper_bound.dart:162-177): `List<int>`/`Set<int>` both view
//! `Iterable<E>` with `E := int`, so the bound is `Iterable<int>`, never
//! the `Object` under-report; a shared head whose two views differ stays
//! `Object`, as do unrelated heads.
//!
//! Oracle provenance: `~/.dartq-oracle/3.13.7/dart-sdk/bin/dart analyze
//! --format=machine` — the bare-typedef fixture is silent (the census
//! case `test_genericTypeAlias_fieldAndReturnType_typeParameters_noArguments`
//! carries both rows as our extras); `BODY_MIGHT … 'Iterable<int>'` +
//! `UNDEFINED_METHOD … 'Iterable'`; `BODY_MIGHT … 'I<int>'` +
//! `UNDEFINED_METHOD … 'I'`; both controls answer `'Object'`.

use super::test_rig::{report_ops, rows};
use crate::diagnose::FileReport;

fn undefined_methods(report: &FileReport) -> Vec<&str> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "undefined_method")
        .map(|d| d.message.as_str())
        .collect()
}

fn body_mights(report: &FileReport) -> Vec<&str> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name.starts_with("body_might"))
        .map(|d| d.message.as_str())
        .collect()
}

fn invalid_assignments(report: &FileReport) -> Vec<&str> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_assignment")
        .map(|d| d.message.as_str())
        .collect()
}

/// (1) The bare generic typedef's closure context instantiates to bounds:
/// `Foo f = <T>(T x) {}` opens the literal under `dynamic Function<T>(T)`,
/// so the falling-off block reports no `body_might` (its context return is
/// `dynamic`) and the value assigns to the declared `Foo<dynamic>` face
/// with no `invalid_assignment`. Before the fix the raw right-hand side
/// (`S Function(T)`) was the context — the census case carries both rows
/// as our extras.
#[test]
fn bare_generic_typedef_closure_opens_under_i2b_context() {
    let report = report_ops(
        "typedef Foo<S> = S Function<T>(T x);\n\
         void main() {\n\
         Foo f = <T>(T x) {};\n\
         }\n",
    );
    assert!(body_mights(&report).is_empty(), "{:#?}", rows(&report));
    assert!(
        invalid_assignments(&report).is_empty(),
        "{:#?}",
        rows(&report)
    );
}

/// (2a) Distinct arg-carrying heads sharing a core supertype: `List<int>`
/// and `Set<int>` both view `Iterable<E>` with `E := int`, so the solved
/// sibling bound is `Iterable<int>` — the member miss reads `Iterable` and
/// the closure's context return face is `Iterable<int>`, never `Object`.
#[test]
fn closure_lub_list_set_join_iterable_int() {
    let report = report_ops(
        "S g<S>(S base, S other, S Function(S a) f) => f(base);\n\
         void main() {\n\
         g(<int>[], <int>{}, (a) { a.noSuchEver(); });\n\
         }\n",
    );
    let hits = undefined_methods(&report);
    assert_eq!(hits.len(), 1, "{:#?}", rows(&report));
    assert!(
        hits[0].contains("isn't defined for the type 'Iterable"),
        "{:#?}",
        rows(&report)
    );
    let mights = body_mights(&report);
    assert!(
        mights.iter().any(|m| m.contains("'Iterable<int>'")),
        "{:#?}",
        rows(&report)
    );
}

/// (2b) Distinct in-file heads sharing one generic interface under the
/// SAME instantiation: `C<int>`/`D<int>` both view `I<X>` with `X := int`,
/// so the bound is `I<int>` — never `Object`.
#[test]
fn closure_lub_shared_interface_same_instantiation() {
    let report = report_ops(
        "class I<X> {}\n\
         class C<X> implements I<X> {}\n\
         class D<X> implements I<X> {}\n\
         S g<S>(S base, S other, S Function(S a) f) => f(base);\n\
         void main() {\n\
         g(C<int>(), D<int>(), (a) { a.noSuchEver(); });\n\
         }\n",
    );
    let hits = undefined_methods(&report);
    assert_eq!(hits.len(), 1, "{:#?}", rows(&report));
    assert!(
        hits[0].contains("isn't defined for the type 'I"),
        "{:#?}",
        rows(&report)
    );
    let mights = body_mights(&report);
    assert!(
        mights.iter().any(|m| m.contains("'I<int>'")),
        "{:#?}",
        rows(&report)
    );
}

/// (2-control 1) A shared interface whose two views differ (`C<int>` /
/// `D<String>` over `I<X>`) is not in the instantiated intersection — the
/// `Object` terminal stands, the oracle's answer.
#[test]
fn closure_lub_divergent_instantiations_stay_object() {
    let report = report_ops(
        "class I<X> {}\n\
         class C<X> implements I<X> {}\n\
         class D<X> implements I<X> {}\n\
         S g<S>(S base, S other, S Function(S a) f) => f(base);\n\
         void main() {\n\
         g(C<int>(), D<String>(), (a) { a.noSuchEver(); });\n\
         }\n",
    );
    let hits = undefined_methods(&report);
    assert_eq!(hits.len(), 1, "{:#?}", rows(&report));
    assert!(
        hits[0].contains("isn't defined for the type 'Object'."),
        "{:#?}",
        rows(&report)
    );
}

/// (2-control 2) Unrelated arg-carrying heads share only `Object` — the
/// terminal arm the merge keeps (the #3872 bare-shape pin, arg-carrying
/// twin).
#[test]
fn closure_lub_unrelated_arg_heads_stay_object() {
    let report = report_ops(
        "class C<X> {}\n\
         class D<X> {}\n\
         S g<S>(S base, S other, S Function(S a) f) => f(base);\n\
         void main() {\n\
         g(C<int>(), D<int>(), (a) { a.noSuchEver(); });\n\
         }\n",
    );
    let hits = undefined_methods(&report);
    assert_eq!(hits.len(), 1, "{:#?}", rows(&report));
    assert!(
        hits[0].contains("isn't defined for the type 'Object'."),
        "{:#?}",
        rows(&report)
    );
}

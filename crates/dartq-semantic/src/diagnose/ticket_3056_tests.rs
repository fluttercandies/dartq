//! The #3056 pins — the type-parameter value read's static type and the
//! `S <: T` assignability verdict.
//!
//! A bare `T` in expression position is a TypeLiteral: it reifies the
//! parameter as a `Type` object, never the parameter's own type (upstream
//! Dart SDK source, pkg/analyzer/lib/src/dart/resolver/
//! simple_identifier_resolver.dart:268-269 — the `TypeParameterElement`
//! arm answers `_typeProvider.typeType`). `return T` therefore asks `Type`
//! against the declared return, and `Type <: T` is decidable-false the
//! moment the target spells a type parameter (subtype.dart's Left Variable
//! arm admits only a param source or the bound chain — an interface source
//! never subtypes a bare parameter). Together these land the
//! `return T` / `int <: T` / `S <: T` family. The rig mounts `dart:core`
//! so `Type` resolves as the real reified object head.

use super::test_rig::report_ops;
use super::*;

fn codes(folded: &FileReport) -> Vec<&str> {
    folded
        .diagnostics
        .iter()
        .map(|diag| diag.code.name)
        .collect()
}

/// `return T` reads the parameter's `Type` reification, not the parameter —
/// `Type <: T` is a definite rejection, so the family head reports.
#[test]
fn return_type_param_literal_reports() {
    let folded = report_ops("class C<T> { T f() { return T; } }\n");
    assert_eq!(
        codes(&folded),
        vec!["return_of_invalid_type"],
        "`return T` asks Type <: T — the oracle's definite rejection"
    );
}

/// A param-TYPED value (`x: T`) returns silently — `T <: T` is the
/// reflexivity arm, not the interface-source rejection the literal takes.
/// This is the negative control: the param-target arm must not over-fire on
/// a source that is itself the parameter.
#[test]
fn return_param_typed_value_is_clean() {
    let folded = report_ops("class D<T> { T g(T x) { return x; } }\n");
    assert_eq!(
        codes(&folded),
        Vec::<&str>::new(),
        "`return x` where `x: T` subtypes the declared `T`"
    );
}

/// A concrete source into a param target: `int <: T` is `false` upstream —
/// no interface type subtypes a bare parameter (its bound is `Object?`, but
/// subtype.dart's Left Variable arm decides over the param's own identity,
/// not the bound). The arm lands the second half of the family.
#[test]
fn return_concrete_into_param_target_reports() {
    let folded = report_ops("class E<T> { T h() { return 3; } }\n");
    assert_eq!(
        codes(&folded),
        vec!["return_of_invalid_type"],
        "`return 3` asks int <: T — a definite rejection"
    );
}

/// The different-parameter source: `S <: T` is undecidable on the name-level
/// lattice (the bound chain is the blind spot), so it stays the under-report
/// `None` — the arm declines a param source rather than guess the chain.
#[test]
fn return_different_param_is_undecidable() {
    let folded = report_ops("class F<S, T> { T k(S s) { return s; } }\n");
    assert_eq!(
        codes(&folded),
        Vec::<&str>::new(),
        "`return s` where `s: S` into `T` — the bound chain is undecidable"
    );
}

/// `null` into a param target already reports through `assignable_from`'s
/// Left-Null arm — unchanged by the param-target arm (it short-circuits
/// before the tail). Pin the posture so the arm's placement stays behind it.
#[test]
fn return_null_into_param_target_reports() {
    let folded = report_ops("class N<T> { T m() { return null; } }\n");
    assert_eq!(
        codes(&folded),
        vec!["return_of_invalid_type"],
        "`return null` into non-nullable `T` stays a rejection"
    );
}

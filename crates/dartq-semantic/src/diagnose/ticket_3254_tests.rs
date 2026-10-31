//! #3254 pins — the generic-method bound arm of `override_broken` (recon
//! C3): position unification + bound EQUALITY.
//!
//! Upstream correspondence (`pkg/analyzer`):
//! - `subtype.dart:86-96` `_isFunctionSubtypeOf` asks
//!   `relateTypeParameters` first and answers a plain `false` when it
//!   cannot relate the two signatures — so an unrelatable pair is an
//!   `invalid_override` whatever the parameter/result faces say;
//! - `type_system.dart:1570-1624` `relateTypeParameters`: the two
//!   signatures must declare the same parameter count, both sides are
//!   instantiated onto ONE shared fresh parameter set (unification is by
//!   POSITION — the two declarations' spellings are unrelated), and every
//!   bound pair must be `isEqualTo` (a mutual subtype question, not a
//!   direction) after that substitution; an absent bound reads `dynamic`.
//!
//! The corpus faces these pins mirror:
//! `features/_core/generic_methods/language/generic_methods/overriding_test_01`
//! (`String fun<T extends X>(T)` over `String fun<T extends Y>(T)` — the
//! widened bound is still a mismatch), `language/mixin/method_override_test`
//! (`T Function<T extends int>(dynamic)` over `T Function<T extends num>(dynamic)`
//! and the unbounded-over-bounded row), and
//! `InferredTypeTest.test_genericMethods_doNotInferInvalidOverrideOfGenericMethod`
//! / `test_genericMethods_handleOverrideOfNonGenericWithGeneric` (the
//! count gate in both directions).
//!
//! Registered through `diagnose/tests.rs`'s `#[path]` block: `diagnose.rs`
//! is a live-lane file out of this ticket's write scope, and `#[path]`
//! keeps the module inside the `diagnose` subtree so
//! `crate::diagnose::test_rig` resolves.

use crate::diagnose::FileReport;
use crate::diagnose::test_rig::{report_ops_bare, rows};

/// The `invalid_override` rows of one report, verbatim.
fn override_rows(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_override")
        .map(|d| d.message.to_string())
        .collect()
}

/// The bound pair decides the verdict, not the faces: `T extends int` is
/// not equal to `T extends num` in either direction (upstream
/// `type_system.dart:1611` `isEqualTo`, so a NARROWER bound breaks the
/// override exactly like a wider one — `overriding_test_01`'s
/// `String fun<T extends X>(T)` over `String fun<T extends Y>(T)`).
#[test]
fn narrowed_bound_overrides_broken() {
    let report = report_ops_bare(
        "\
class C { String m<T extends num>(T t) => ''; }
class D extends C { String m<T extends int>(T t) => ''; }
",
    );
    let found = override_rows(&report);
    assert_eq!(
        found.len(),
        1,
        "exactly one invalid_override row: {}",
        rows(&report).join("\n")
    );
    assert!(
        found[0].contains("'D.m' ('String Function<T extends int>(T)')"),
        "the declared slot renders the narrowed bound: {}",
        found[0]
    );
    assert!(
        found[0].contains("'C.m' ('String Function<T extends num>(T)')"),
        "the overridden slot renders the inherited bound: {}",
        found[0]
    );
}

/// The same bound spelled under different parameter names is the same
/// bound: upstream unifies by position, so `R extends num` over
/// `T extends num` compares `num` against `num` and stays silent.
#[test]
fn renamed_parameters_with_equal_bounds_silent() {
    let report = report_ops_bare(
        "\
class C { String m<T extends num>(T t) => ''; }
class D extends C { String m<R extends num>(R t) => ''; }
",
    );
    let found = override_rows(&report);
    assert!(
        found.is_empty(),
        "an identical bound under a renamed parameter is valid: {}",
        rows(&report).join("\n")
    );
}

/// An absent bound reads `dynamic` (upstream `bound1 ??= dynamic`), which
/// no concrete bound equals — the mixin corpus's
/// `T Function<T>(dynamic)` over `T Function<T extends num>(dynamic)` row.
#[test]
fn unbounded_over_bounded_broken() {
    let report = report_ops_bare(
        "\
class C { String m<T extends num>(T t) => ''; }
class D extends C { String m<T>(T t) => ''; }
",
    );
    let found = override_rows(&report);
    assert_eq!(
        found.len(),
        1,
        "exactly one invalid_override row: {}",
        rows(&report).join("\n")
    );
    assert!(
        found[0].contains("'D.m' ('String Function<T>(T)')"),
        "the unbounded declared slot stays unwritten: {}",
        found[0]
    );
    assert!(
        found[0].contains("'C.m' ('String Function<T extends num>(T)')"),
        "the overridden slot renders its bound: {}",
        found[0]
    );
}

/// The count gate (`relateTypeParameters`'s length check): two parameters
/// over one never relate, so the member is broken before any face is
/// compared — both directions of the `InferredTypeTest` generic-method
/// cohort.
#[test]
fn type_parameter_count_mismatch_broken_both_directions() {
    let declared_more = report_ops_bare(
        "\
class C { String m<T>(T t) => ''; }
class D extends C { String m<T, S>(T t, S s) => ''; }
",
    );
    let found = override_rows(&declared_more);
    assert_eq!(
        found.len(),
        1,
        "exactly one invalid_override row: {}",
        rows(&declared_more).join("\n")
    );
    assert!(
        found[0].contains("'D.m' ('String Function<T, S>(T, S)')")
            && found[0].contains("'C.m' ('String Function<T>(T)')"),
        "the two-parameter declaration is not a valid override: {}",
        found[0]
    );

    // The non-generic override of a generic method. The count gate fires
    // ahead of every face, so the ROW is unconditional; the declared slot's
    // text is NOT pinned here — `effective_declared` currently infers the
    // untyped member from the generic inherited signature (`'T Function(T)'`)
    // where the oracle refuses that inference and prints
    // `'dynamic Function(dynamic)'`
    // (`test_genericMethods_doNotInferInvalidOverrideOfGenericMethod` in
    // `conformance/expected.tsv`). That divergence lives in the
    // instance-member-inference face, not in this bound/count arm, and is
    // recorded as its own residual.
    let declared_none = report_ops_bare(
        "\
class C { T m<T>(T x) => x; }
class D extends C { m(x) => x; }
",
    );
    let found = override_rows(&declared_none);
    assert_eq!(
        found.len(),
        1,
        "exactly one invalid_override row: {}",
        rows(&declared_none).join("\n")
    );
    assert!(
        found[0].contains("'C.m' ('T Function<T>(T)')"),
        "the overridden slot renders the generic signature: {}",
        found[0]
    );
}

/// A bound may spell another of the method's own parameters
/// (`S extends T`). Under position unification the declared `B extends A`
/// and the inherited `S extends T` denote the same pair, so the override
/// is valid even though no leaf spelling survives the rename.
#[test]
fn parameter_referencing_bounds_unify_silent() {
    let report = report_ops_bare(
        "\
class C { void m<T, S extends T>(T a, S b) {} }
class D extends C { void m<A, B extends A>(A a, B b) {} }
",
    );
    let found = override_rows(&report);
    assert!(
        found.is_empty(),
        "position-unified parameter bounds are equal: {}",
        rows(&report).join("\n")
    );
}

/// The parameter-headed bound against a concrete one: `B extends A`
/// unifies to a bound on the first parameter (`dynamic` — `A` is
/// unbounded), which no `num` equals in either direction. The renamed
/// bound leaf must resolve to the unified parameter (not read as an
/// unresolvable head, whose InvalidType leniency would answer
/// "compatible" in both directions and swallow the row).
#[test]
fn parameter_bound_against_concrete_bound_broken() {
    let report = report_ops_bare(
        "\
class C { void m<T, S extends num>(T a, S b) {} }
class D extends C { void m<A, B extends A>(A a, B b) {} }
",
    );
    let found = override_rows(&report);
    assert_eq!(
        found.len(),
        1,
        "exactly one invalid_override row: {}",
        rows(&report).join("\n")
    );
    assert!(
        found[0].contains("'D.m' ('void Function<A, B extends A>(A, B)')"),
        "the declared slot renders the renamed bound: {}",
        found[0]
    );
    assert!(
        found[0].contains("'C.m' ('void Function<T, S extends num>(T, S)')"),
        "the overridden slot renders its concrete bound: {}",
        found[0]
    );
}

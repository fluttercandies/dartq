//! #4004 — the two foreign-red faces' layer guards, pinned at the oracle
//! rows (both regressions bisected by the lane's runtime probes, never
//! rebuilt):
//!
//! 1. The generic-signature quantifier-leaf slots (`relateTypeParameters`
//!    after its bounds equality): both signatures instantiate with one
//!    shared fresh parameter set (upstream Dart SDK source,
//!    `pkg/analyzer/lib/src/dart/element/type_system.dart`:1593-1610), so
//!    a bare leaf spelling one of the target's own parameters is that
//!    variable — `subtype.dart` decides a variable target false for every
//!    non-reflexive concrete source (the both-interface leg at :253
//!    requires an interface target; the ask falls to the unconditional
//!    `return false` at :293), and a variable source by its implicit
//!    `Object?` bound. The name-level slot ask used to abstain on the
//!    unresolvable leaf, silencing the whole verdict
//!    (`generic_fn_mismatched_formal_reports`'s red).
//! 2. A function-typed FORMAL's own value read (`void g(x)`): the
//!    parameter's captured signature (#2937 3c's written-signature claim)
//!    is its value face, but the tear-off recovery's shadow gate refuses
//!    every frame-local name, so `return g;` read `Unknown` and the
//!    return matrix never asked (`untyped_inner_formal_return_of_invalid_
//!    type`'s red). The value-READ bare row answers the claim table's
//!    signature; a shadowing plain local claims nothing and keeps the
//!    unknown face.

use super::test_rig::{report_ops, rows};

// —— the quantifier-leaf slot face ——

/// The bisected regression itself: `String foo<T>(T x)` into
/// `String Function<S>(int)` — the target's concrete formal against the
/// shared variable leaf decides false (oracle 3.13.7:
/// `INVALID_ASSIGNMENT` at 3|31, "A value of type
/// 'String Function<T>(T)' can't be assigned to a variable of type
/// 'String Function<S>(int)'.").
#[test]
fn quantifier_leaf_formal_mismatch_reports() {
    let report = report_ops(
        "String foo<T>(T x) => 'x';\n\
         void test1() {\n\
           String Function<S>(int) y = foo;\n\
           print(y);\n\
         }\n",
    );
    let names: Vec<String> = rows(&report);
    assert!(
        names.iter().any(|r| r.contains("invalid_assignment")),
        "expected invalid_assignment, got {names:?}"
    );
}

/// The named-formal twin: the leaf sits in the target's named section —
/// the same shared-variable verdict (oracle 3.13.7:
/// `INVALID_ASSIGNMENT` at 2|33).
#[test]
fn quantifier_leaf_named_formal_mismatch_reports() {
    let report = report_ops(
        "void t(void Function<T>({T x}) f) {\n\
           void Function<S>({int x}) u = f;\n\
         }\n",
    );
    let names: Vec<String> = rows(&report);
    assert!(
        names.iter().any(|r| r.contains("invalid_assignment")),
        "expected invalid_assignment, got {names:?}"
    );
}

/// The bounds-equality gate still owns its own family: equal written
/// bounds accept, unequal written bounds reject before any slot ask
/// (oracle 3.13.7: the num/num pair silent, the num/int pair
/// `INVALID_ASSIGNMENT` at 2|38).
#[test]
fn quantifier_bounds_equality_still_decides() {
    let report = report_ops(
        "void t(void Function<T extends num>() f) {\n\
           void Function<S extends int>() x = f;\n\
         }\n",
    );
    let names: Vec<String> = rows(&report);
    assert!(
        names.iter().any(|r| r.contains("invalid_assignment")),
        "expected invalid_assignment, got {names:?}"
    );
    let quiet = report_ops(
        "void t(void Function<T extends num>() f) {\n\
           void Function<S extends num>() x = f;\n\
         }\n",
    );
    assert!(
        !rows(&quiet)
            .iter()
            .any(|r| r.contains("invalid_assignment")),
        "bounds-equal pair must not report: {:?}",
        rows(&quiet)
    );
}

// —— the function-typed formal's value-read face ——

/// The bisected regression itself: `return g;` over a `void g(x)`
/// formal — the formal's captured signature (`void Function(dynamic)`,
/// the unwritten inner formal's `dynamic` default) refuses the
/// `int Function(int)` host (oracle 3.13.7: `RETURN_OF_INVALID_TYPE`
/// at 1|41).
#[test]
fn typed_formal_value_read_reports_on_return() {
    let report = report_ops(
        "int Function(int) f(void g(x)) { return g; }\n\
         void main() {}\n",
    );
    let names: Vec<String> = rows(&report);
    assert!(
        names.iter().any(|r| r.contains("return_of_invalid_type")),
        "expected return_of_invalid_type, got {names:?}"
    );
}

/// The typed inner formal keeps its own face through the same read —
/// `void Function(int)` (oracle 3.13.7: `RETURN_OF_INVALID_TYPE`
/// at 1|45).
#[test]
fn untyped_formal_value_read_reports_typed_inner() {
    let report = report_ops(
        "int Function(int) f(void g(int x)) { return g; }\n\
         void main() {}\n",
    );
    let names: Vec<String> = rows(&report);
    assert!(
        names
            .iter()
            .any(|r| r.contains("return_of_invalid_type") && r.contains("void Function(int)")),
        "expected return_of_invalid_type over 'void Function(int)', got {names:?}"
    );
}

/// The shadow posture survives the stash recovery: a plain `int g`
/// local claims nothing, so `f(g)` answers the ordinary nominal miss —
/// `int` against `void Function(dynamic)` (oracle 3.13.7:
/// `ARGUMENT_TYPE_NOT_ASSIGNABLE` at 2|28). The claim table answering a
/// shadowing local would silence this row.
#[test]
fn shadowing_plain_local_keeps_the_nominal_miss() {
    let report = report_ops(
        "void f(void g(x)) {}\n\
         void main() { int g = 3; f(g); }\n",
    );
    let names: Vec<String> = rows(&report);
    assert!(
        names
            .iter()
            .any(|r| r.contains("argument_type_not_assignable")),
        "expected argument_type_not_assignable, got {names:?}"
    );
}

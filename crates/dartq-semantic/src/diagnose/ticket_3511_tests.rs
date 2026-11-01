//! #3511 — the required-positional count is part of the function-type
//! identity.
//!
//! `FnShape` carried only the merged positional column, so
//! `int Function(int)` and `int Function([int])` interned to one
//! `FnShapeId` and the required/optional split had to be patched in from a
//! side table keyed by that shared id. Upstream keeps the split in the
//! type itself: `FunctionTypeImpl` equality compares each formal's
//! positional/optional kind (pkg/analyzer `_equalParameters`), and
//! `_isFunctionSubtypeOf` answers `false` for a required source formal
//! against an optional target formal, so the two spellings are distinct
//! types and never subtype-equal.
//!
//! Oracle rows below were captured against `dart analyze --format machine`
//! 3.13.7 (the two fixtures are the `function_subtype` repro pairs).
//!
//! Residual (not pinned here): the `is` TARGET built by the typedef-alias
//! fold still carries the all-required count, because `fn_sig_ty` interns
//! through the count-blind `intern_function_ty` — `req is TOpt` over
//! `void req(int i)` vs `typedef TOpt = void Function([int])` therefore
//! proves constant-true where the oracle stays silent. That face lives in
//! `engine_core.rs` (`fn_sig_ty`/`fn_face_sig`), outside this ticket's
//! file set.

use super::test_rig::report_ops;
use super::*;

fn type_check_rows(report: &FileReport) -> usize {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unnecessary_type_check")
        .count()
}

fn all_codes(report: &FileReport) -> Vec<&'static str> {
    report.diagnostics.iter().map(|d| d.code.name).collect()
}

// —— The identity contract, pinned at the pool ——————————————————————————

/// The split joins the intern key: two spellings that differ only in the
/// required-positional count are distinct shapes, while the same spelling
/// (and the split-blind [`ArgPool::function`] entry, whose positional
/// column reads all-required) stays one slot. This is the one equal-pair
/// control the change must not break.
#[test]
fn required_split_is_identity_and_equal_spellings_stay_interned() {
    let mut pool = crate::model::ArgPool::default();
    let required = pool.function_required(0, 1, Ty::Void, &[Ty::Dynamic], &[], false);
    let required_twin = pool.function_required(0, 1, Ty::Void, &[Ty::Dynamic], &[], false);
    let optional = pool.function_required(0, 0, Ty::Void, &[Ty::Dynamic], &[], false);
    let split_blind = pool.function(0, Ty::Void, &[Ty::Dynamic], &[], false);

    assert_eq!(
        required, required_twin,
        "one spelling must keep interning to one shape"
    );
    assert_eq!(
        split_blind, required,
        "the split-blind entry is the all-required reading"
    );
    assert_ne!(
        required, optional,
        "`void Function(int)` and `void Function([int])` are distinct types"
    );
}

// —— The oracle polarity pair (optparam.dart) ————————————————————————————

/// `opt is TOpt` where `opt: void Function([int])` — the identical
/// spelling, so the two faces share one `FnShapeId` and the verdict is
/// constant-true (oracle: one `unnecessary_type_check` row).
#[test]
fn optional_operand_against_identical_optional_target_reports_true() {
    let report = report_ops(
        "typedef TOpt = void Function([int]);\n\
         void opt([int i]) {}\n\
         void probe() { print(opt is TOpt); }",
    );
    assert_eq!(
        type_check_rows(&report),
        1,
        "the same optional spelling must prove equal: {:?}",
        all_codes(&report)
    );
}

// —— The oracle report on arity alone (bmcn.dart face) ————————————————————

/// `foo is Boz` where `foo: int Function(bool, [String])` and
/// `Boz = int Function(bool)`: one required source positional against one
/// required target, the optional tail unconstrained — subtype, so the
/// check is constant-true (oracle: one row). Under the merged shape the
/// split read as "all required" and the row was missed.
#[test]
fn optional_tail_operand_against_required_target_reports_true() {
    let report = report_ops(
        "typedef int Boz(bool a);\n\
         int foo(bool a, [String b = '']) => -1;\n\
         void probe() { print(foo is Boz); }",
    );
    assert_eq!(
        type_check_rows(&report),
        1,
        "Function(bool, [String]) is a Function(bool): {:?}",
        all_codes(&report)
    );
}

/// `foo is Baz` where `Baz = int Function({bool a})` and `foo` has no named
/// formal: a REAL subtype-negative pair, so nothing is reported. This is
/// the row the shared `FnShapeId` used to fabricate.
#[test]
fn named_mismatch_against_optional_tail_operand_stays_silent() {
    let report = report_ops(
        "typedef int Baz(bool a, {String b});\n\
         int foo(bool a, [String b = '']) => -1;\n\
         void probe() { print(foo is Baz); }",
    );
    assert!(
        type_check_rows(&report) == 0,
        "the named set differs, so the check is informative: {:?}",
        all_codes(&report)
    );
}

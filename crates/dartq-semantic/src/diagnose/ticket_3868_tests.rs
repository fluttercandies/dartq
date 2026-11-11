//! #3868 pins — the pattern/const never-matches verdicts over the
//! `Ty::Function`/`Ty::Record` matched faces, and the covering verdicts
//! over the remaining names-only core stub heads.
//!
//! `NameTy::from_semantic` keys a `Ty::Function` by a sentinel unknown
//! and a `Ty::Record` by its interned shape; neither answers through
//! `head_name`, so the `canBeSubtypeOf` relation's head-pair gate
//! (`type_system.dart:191`) collapsed every function/record pair to the
//! conservative `true` — a `void Function()` matched never reported
//! `pattern_never_matches_value_type` against `int`, a record against a
//! mismatched record, and neither reported
//! `constant_pattern_never_matches_value_type` against a non-`Null`
//! constant. The verdict arms the oracle fires (type_system.dart:
//! 161-171's FunctionType faces, :269-320's RecordType faces, and
//! constant_verifier.dart:570-580's `_canBeEqual` value arms) now run
//! before `head_name` returns `None`.
//!
//! The covering face's `name_resolvable` gate suppresses every head it
//! cannot decide — #3728 added `Object`; `Function`, `Record`, `Type`,
//! `Symbol`, `Future` answer the same way (an opaque leaf plus the
//! `Head()` witness — oracle 3.13.7: every `Head?` switch reports the
//! `null` gap), so the gate names them all.
//!
//! Oracle rows: dart 3.13.7 (`dart analyze --format=machine`) over the
//! fixtures below — verbatim messages pinned where they are the verdict.

use super::test_rig::{code_names, report_ops};

/// The `non_exhaustive_switch_expression` messages of `report`, in source
/// order.
fn covering_rows(source: &str) -> Vec<String> {
    report_ops(source)
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "non_exhaustive_switch_expression")
        .map(|d| d.message.to_string())
        .collect()
}

/// The `pattern_never_matches_value_type` messages of `source`'s report,
/// in source order.
fn never_rows(source: &str) -> Vec<String> {
    report_ops(source)
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "pattern_never_matches_value_type")
        .map(|d| d.message.to_string())
        .collect()
}

/// The `constant_pattern_never_matches_value_type` messages of `source`'s
/// report, in source order.
fn constant_never_rows(source: &str) -> Vec<String> {
    report_ops(source)
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "constant_pattern_never_matches_value_type")
        .map(|d| d.message.to_string())
        .collect()
}

// —— the names-only stub heads: `Head _` over `Head?` reports the null gap —

/// `Function _` covers the non-null half of `Function?` — `null` reports.
/// The pre-fix gate read the unindexed `Function` head as unresolvable
/// and suppressed the emit outright.
#[test]
fn function_stub_scrutinee_reports_the_null_gap() {
    let rows = covering_rows("int f(Function? o) => switch (o) { Function _ => 1 };\n");
    assert_eq!(
        rows,
        [
            "The type 'Function?' isn't exhaustively matched by the switch cases since it doesn't match the pattern 'null'."
        ]
    );
}

/// `Record` is `abstract final` (record.dart:82) — zero declared
/// subtypes, so the leaf-plus-witness answer is the whole space.
#[test]
fn record_stub_scrutinee_reports_the_null_gap() {
    let rows = covering_rows("int f(Record? o) => switch (o) { Record _ => 1 };\n");
    assert_eq!(
        rows,
        [
            "The type 'Record?' isn't exhaustively matched by the switch cases since it doesn't match the pattern 'null'."
        ]
    );
}

/// `Type` is `abstract interface` (type.dart:21): open, but every
/// subtype still inhabits the `[Type, Null]` split the head alone spells.
#[test]
fn type_stub_scrutinee_reports_the_null_gap() {
    let rows = covering_rows("int f(Type? o) => switch (o) { Type _ => 1 };\n");
    assert_eq!(
        rows,
        [
            "The type 'Type?' isn't exhaustively matched by the switch cases since it doesn't match the pattern 'null'."
        ]
    );
}

/// `Symbol` is `abstract class` (symbol.dart:8) — the same decidability
/// shape as `Type`/`Future`: opaque leaf, `Symbol()` witness.
#[test]
fn symbol_stub_scrutinee_reports_the_null_gap() {
    let rows = covering_rows("int f(Symbol? o) => switch (o) { Symbol _ => 1 };\n");
    assert_eq!(
        rows,
        [
            "The type 'Symbol?' isn't exhaustively matched by the switch cases since it doesn't match the pattern 'null'."
        ]
    );
}

/// `Future<int>?` — the `Future` head is `abstract interface`
/// (future.dart:229) like `Type`; its argument rides the display, never
/// the decidability.
#[test]
fn future_stub_scrutinee_reports_the_null_gap() {
    let rows = covering_rows("int f(Future<int>? o) => switch (o) { Future<int> _ => 1 };\n");
    assert_eq!(
        rows,
        [
            "The type 'Future<int>?' isn't exhaustively matched by the switch cases since it doesn't match the pattern 'null'."
        ]
    );
}

// —— exhaustive controls over the same heads stay silent ————————————————

/// `Function _` plus a catch-all covers `Function?` — nothing reports.
#[test]
fn function_stub_scrutinee_with_default_stays_silent() {
    let rows = covering_rows("int f(Function? o) => switch (o) { Function _ => 1, _ => 2 };\n");
    assert!(rows.is_empty(), "{rows:?}");
}

/// `Future<int>? _` covers the whole `Future<int>?` space — silent.
#[test]
fn future_stub_scrutinee_nullable_case_stays_silent() {
    let rows = covering_rows("int f(Future<int>? o) => switch (o) { Future<int>? _ => 1 };\n");
    assert!(rows.is_empty(), "{rows:?}");
}

// —— `pattern_never_matches_value_type` over function/record faces ——————

/// `void Function()` matched vs `int` required: the FunctionType-left
/// arm (type_system.dart:163-165) — `int` is a known head that is
/// neither `Function` nor `Object`.
#[test]
fn function_matched_against_int_required_reports() {
    let rows =
        never_rows("int f(void Function() fn) { if (fn case int _) { return 1; } return 0; }\n");
    assert_eq!(
        rows,
        ["The matched value type 'void Function()' can never match the required type 'int'."]
    );
}

/// `(int,)` matched vs `String` required: the RecordType-left arm
/// (type_system.dart:273-275) — `String` is a known head outside
/// `Object`/`Record`.
#[test]
fn record_matched_against_string_required_reports() {
    let rows = never_rows("int f((int,) r) { if (r case String _) { return 1; } return 0; }\n");
    assert_eq!(
        rows,
        ["The matched value type '(int,)' can never match the required type 'String'."]
    );
}

/// `(int, int)` matched vs `(int,)` required: the record-pair arm's
/// positional-arity clause (type_system.dart:288-289).
#[test]
fn record_matched_against_narrower_record_reports() {
    let rows = never_rows("int f((int, int) r) { if (r case (int,) _) { return 1; } return 0; }\n");
    assert_eq!(
        rows,
        ["The matched value type '(int, int)' can never match the required type '(int,)'."]
    );
}

/// `(int,)` matched vs `({int a})` required: the record-pair arm's
/// named-set clause (type_system.dart:303-310) — zero named against one
/// positional rejects before any field ask.
#[test]
fn record_matched_against_named_record_reports() {
    let rows = never_rows("int f((int,) r) { if (r case ({int a}) _) { return 1; } return 0; }\n");
    assert_eq!(
        rows,
        ["The matched value type '(int,)' can never match the required type '({int a})'."]
    );
}

/// The same-shape control: `(int,)` matched vs `(int,)` required is the
/// oracle-silent pair — identical shapes overlap (every field the same).
#[test]
fn record_matched_against_same_record_stays_silent() {
    let rows = never_rows("int f((int,) r) { if (r case (int,) _) { return 1; } return 0; }\n");
    assert!(rows.is_empty(), "{rows:?}");
}

/// `void Function()` matched vs `Function` required: `isDartCoreFunction`
/// is the arm's admit head (type_system.dart:164) — silent.
#[test]
fn function_matched_against_function_head_stays_silent() {
    let rows = never_rows(
        "int f(void Function() fn) { if (fn case Function _) { return 1; } return 0; }\n",
    );
    assert!(rows.is_empty(), "{rows:?}");
}

/// `void Function()` matched vs `Object` required: `isDartCoreObject`
/// likewise (type_system.dart:164) — silent.
#[test]
fn function_matched_against_object_head_stays_silent() {
    let rows =
        never_rows("int f(void Function() fn) { if (fn case Object _) { return 1; } return 0; }\n");
    assert!(rows.is_empty(), "{rows:?}");
}

// —— `constant_pattern_never_matches_value_type` over the same faces ————

/// `case null` over `void Function()`: the `_canBeEqual` FunctionType
/// value arm admits `Null` only when the value is nullable
/// (constant_verifier.dart:570-572) — the non-nullable function matched
/// never equals `null`.
#[test]
fn null_constant_over_function_matched_reports() {
    let rows =
        constant_never_rows("void f(void Function() fn) { switch (fn) { case null: break; } }\n");
    assert_eq!(
        rows,
        [
            "The matched value type 'void Function()' can never be equal to this constant of type 'Null'."
        ]
    );
}

/// `case 1` over `void Function()`: a non-`Null` constant against a
/// function value is provably never equal (constant_verifier.dart:573).
#[test]
fn int_constant_over_function_matched_reports() {
    let rows =
        constant_never_rows("void f(void Function() fn) { switch (fn) { case 1: break; } }\n");
    assert_eq!(
        rows,
        [
            "The matched value type 'void Function()' can never be equal to this constant of type 'int'."
        ]
    );
}

/// `case 1` over `(int, int)`: the RecordType value arm answers the
/// same `false` for non-`Null` constants (constant_verifier.dart:575-578).
#[test]
fn int_constant_over_record_matched_reports() {
    let rows = constant_never_rows("void f((int, int) r) { switch (r) { case 1: break; } }\n");
    assert_eq!(
        rows,
        ["The matched value type '(int, int)' can never be equal to this constant of type 'int'."]
    );
}

/// `case null` over non-nullable `(int, int)`: `Null` is admitted only
/// against a nullable value (constant_verifier.dart:576-578).
#[test]
fn null_constant_over_record_matched_reports() {
    let rows = constant_never_rows("void f((int, int) r) { switch (r) { case null: break; } }\n");
    assert_eq!(
        rows,
        ["The matched value type '(int, int)' can never be equal to this constant of type 'Null'."]
    );
}

/// `case null` over a NULLABLE record matched: `Null` can equal
/// `(int, int)?`'s `null` half — the oracle-silent shape the arm must
/// not newly fire over.
#[test]
fn null_constant_over_nullable_record_stays_silent() {
    let rows = constant_never_rows(
        "void f((int, int)? r) { switch (r) { case null: break; default: break; } }\n",
    );
    assert!(rows.is_empty(), "{rows:?}");
}

// —— the suppression the fix must NOT undo ———————————————————————————————

/// A scrutinee head that resolves NOWHERE still suppresses the covering
/// emit (the #2336/#3722 preserved gate — `Missing` is no declaration,
/// not the stub shape the five heads carry).
#[test]
fn unresolvable_scrutinee_still_suppresses() {
    let report = report_ops("int f(Missing m) => switch (m) { Function _ => 1, _ => 2 };\n");
    assert!(
        !code_names(&report)
            .iter()
            .any(|name| *name == "non_exhaustive_switch_expression"),
        "{:?}",
        code_names(&report)
    );
}

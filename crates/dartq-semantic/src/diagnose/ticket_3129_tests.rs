//! #3129 — `unnecessary_type_check` verdict pins.
//!
//! The oracle's `_checkAllTypeChecks` is one question — after the
//! `InvalidType`/`dynamic`/`Null` gates, the row fires exactly when
//! `isSubtypeOf(leftType, rightType)` (upstream Dart SDK source,
//! pkg/analyzer/lib/src/error/best_practices_verifier.dart:985-1038);
//! `is!` reports the complementary 'false' row for the same verdict.
//! The under-report roots this ticket closes:
//!
//! * **Nested generic signatures** — the interned `Ty::Function` shapes
//!   carry `type_params_named` (collector `function_from_tree`,
//!   `ArgPool::function_generic`), but every re-decision leg read the
//!   count-only `type_params` through `function_shape_sig`, so a
//!   `Function<S>(S)` vs `Function<S>(T)` nested pair never unified.
//!   `signature_leg_subtype` (engine_core.rs) recurses the positional
//!   unification and `strict_subtype`'s Function arm consults it before
//!   the slot walk — `relateTypeParameters`,
//!   type_system.dart:1581-1635 ahead of subtype.dart:338-350.
//! * **Fresh-variable legs** — upstream's shared fresh set gives the
//!   `to` parameters their own identity: a `to`-param leaf on a leg's
//!   left answers through its bound (subtype.dart:484-493), on the
//!   right no concrete face subtypes it. `zn is Yn`'s inner `T` proves
//!   `T <: num` through `extends num`.
//! * **Declaration bounds on the tear-off** — `operand_tear_off_ty`
//!   carried the generic group's names without their written bounds, so
//!   `xn`'s `T extends num` could not satisfy `relateTypeParameters`'s
//!   bound-equality leg; the decl table's `type_param_bounds` rides the
//!   interned shape now.
//!
//! Oracle rows captured 2026-10-31 against `dart analyze --format
//! machine` 3.13.7 (`/tmp/utc/cases`).

use super::test_rig::report_ops;
use super::*;

fn utc_rows(report: &FileReport) -> Vec<&Diagnostic> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unnecessary_type_check")
        .collect()
}

fn all_codes(report: &FileReport) -> Vec<&'static str> {
    report.diagnostics.iter().map(|d| d.code.name).collect()
}

// —— Sync matrix ————————————————————————————————————————————————————————

/// `int i; i is int` — the reflexive row (`sub == sup`,
/// subtype.dart:66-68): oracle `3:3 len 8` in the bare fixture (the
/// `i is int` expression whole).
#[test]
fn declared_same_type_reports_true() {
    let report = report_ops("void f() { int i = 0; i is int; }");
    let rows = utc_rows(&report);
    assert_eq!(rows.len(), 1, "{:?}", all_codes(&report));
    assert!(rows[0].message.contains("always 'true'"));
}

/// `i is String` — a sibling interface pair is informative
/// (`subtype.dart`'s interface walk has no edge, and the verdict has no
/// subtype-negative arm — best_practices_verifier.dart:1032-1037).
#[test]
fn sibling_type_target_stays_silent() {
    let report = report_ops("void f() { int i = 0; i is String; }");
    assert!(utc_rows(&report).is_empty(), "{:?}", all_codes(&report));
}

/// `i is! int` — the complementary 'false' row for the constant-true
/// verdict (best_practices_verifier.dart:992-999's `notOperator` arm).
#[test]
fn negated_same_type_reports_false() {
    let report = report_ops("void f() { int i = 0; i is! int; }");
    let rows = utc_rows(&report);
    assert_eq!(rows.len(), 1, "{:?}", all_codes(&report));
    assert!(rows[0].message.contains("always 'false'"));
}

/// `d is int` over `dynamic` — Left Top answers only the top faces
/// (subtype.dart:226-231): the check is a real runtime question.
#[test]
fn dynamic_operand_stays_silent() {
    let report = report_ops("void f() { dynamic d = 0; d is int; }");
    assert!(utc_rows(&report).is_empty(), "{:?}", all_codes(&report));
}

/// `o is int` over `Object?` — Left Nullable needs `Object <: int` AND
/// `Null <: int` (subtype.dart:346-351): informative.
#[test]
fn nullable_object_operand_stays_silent() {
    let report = report_ops("void f() { Object? o = 0; o is int; }");
    assert!(utc_rows(&report).is_empty(), "{:?}", all_codes(&report));
}

/// `x is num` over `T extends num` — the bare in-scope parameter answers
/// through its bound (subtype.dart:484-493's Left Variable Bound arm).
#[test]
fn bound_parameter_operand_reports_true() {
    let report = report_ops("T id<T extends num>(T x) { x is num; return x; }");
    let rows = utc_rows(&report);
    assert_eq!(rows.len(), 1, "{:?}", all_codes(&report));
    assert!(rows[0].message.contains("always 'true'"));
}

/// `i is int?` — Right Nullable strips (`S <: T?` iff `S <: T`,
/// subtype.dart:452-478): `int <: int` decides.
#[test]
fn nullable_target_reports_true() {
    let report = report_ops("void f() { int i = 0; i is int?; }");
    let rows = utc_rows(&report);
    assert_eq!(rows.len(), 1, "{:?}", all_codes(&report));
}

/// `i is! Never` — no `int <: Never` (Right Bottom falls through every
/// arm to `false`, subtype.dart:74-77): the `is!` is informative.
#[test]
fn never_target_negated_stays_silent() {
    let report = report_ops("void f() { int i = 0; i is! Never; }");
    assert!(utc_rows(&report).is_empty(), "{:?}", all_codes(&report));
}

/// `l is List` — the bare checked head materializes `List<dynamic>`;
/// `int <: dynamic` proves the pair (the same-head bare-args walk).
#[test]
fn bare_container_target_reports_true() {
    let report = report_ops("void f() { List<int> l = []; l is List; }");
    let rows = utc_rows(&report);
    assert_eq!(rows.len(), 1, "{:?}", all_codes(&report));
}

/// `l is List<String>` — the covariant argument leg `int <: String`
/// refuses: informative.
#[test]
fn misfit_generic_argument_stays_silent() {
    let report = report_ops("void f() { List<int> l = []; l is List<String>; }");
    assert!(utc_rows(&report).is_empty(), "{:?}", all_codes(&report));
}

/// `n is int` / `n is! int` over `num` — the declared-lattice edge runs
/// downward only: a `num` could be `double`, so both polarities are
/// informative (oracle-verified silent pair).
#[test]
fn supertype_source_stays_silent_both_polarities() {
    let report = report_ops("void f() { num n = 0; n is int; n is! int; }");
    assert!(utc_rows(&report).is_empty(), "{:?}", all_codes(&report));
}

/// `null is int` — Left Null fits only the tops and nullable spellings
/// (subtype.dart:319-344): the literal's check is informative.
#[test]
fn null_literal_operand_stays_silent() {
    let report = report_ops("void f() { null is int; }");
    assert!(utc_rows(&report).is_empty(), "{:?}", all_codes(&report));
}

/// `g() is void` — oracle reports the `unnecessary_type_check` TRUE row
/// beside its own `expected_type_name`/`use_of_void_result` parse-face
/// rows (the `void` target is a parse error but still resolves to the
/// `void` TYPE for the `isSubtypeOf` ask). The resolution pass arms no
/// `void` face here — the `is` target's `handle_void_keyword` never
/// reaches the armed slot under recovery, so `checked` lands as the
/// recovery `Named{;}` head and the verdict suppresses — a residual of
/// `resolved_ops`'s `tested_head_undefined`/`emit_is_type_miss` gap, not
/// the subtype walk.

/// `new C() is C` — the `new` keyword joins the operand's extent
/// (`.at(node)` = the whole IsExpression,
/// best_practices_verifier.dart:997): the row starts at `new`, never at
/// the created head.
#[test]
fn new_creation_operand_anchors_at_keyword() {
    let source = "class C {}\nvoid f() { new C() is C; }";
    let report = report_ops(source);
    let rows = utc_rows(&report);
    assert_eq!(rows.len(), 1, "{:?}", all_codes(&report));
    assert_eq!(
        rows[0].range.length, 12,
        "`new C() is C` is 12 columns: {:?}",
        rows[0].range
    );
}

/// `q is int` after `q != null` — the promoted read re-decides through
/// the deferred walk (`deferred_type_checks`, the #3005 arm): the
/// promoted `int` source proves the check (oracle-verified row).
#[test]
fn promoted_read_redecides_through_deferred_walk() {
    let report =
        report_ops("void f() { int? q = 0; if (q != null) { q is int; } }");
    let rows = utc_rows(&report);
    assert_eq!(rows.len(), 1, "{:?}", all_codes(&report));
    assert!(rows[0].message.contains("always 'true'"));
}

// —— Function-shape faces ———————————————————————————————————————————————

/// `f is int Function(int)` over a function-typed local — the
/// non-generic signature pair decides covariantly (subtype.dart's
/// function rule via `fn_sig_subtype`).
#[test]
fn function_typed_local_reports_true() {
    let report = report_ops(
        "void f() { int Function(int) g = (x) => x; g is int Function(int); }",
    );
    let rows = utc_rows(&report);
    assert_eq!(rows.len(), 1, "{:?}", all_codes(&report));
}

/// `f is String Function(int)` — the return-covariance leg `int <:
/// String` refuses: informative (oracle-verified silent).
#[test]
fn function_return_misfit_stays_silent() {
    let report = report_ops(
        "void f() { int Function(int) g = (x) => x; g is String Function(int); }",
    );
    assert!(utc_rows(&report).is_empty(), "{:?}", all_codes(&report));
}

/// `g is IntFn` — the typedef's aliased function shape folds before the
/// verdict (`fold_alias_ty`): the identical signature proves.
#[test]
fn typedef_function_target_reports_true() {
    let report = report_ops(
        "typedef IntFn = int Function(int);\n\
         void f() { int Function(int) g = (x) => x; g is IntFn; }",
    );
    let rows = utc_rows(&report);
    assert_eq!(rows.len(), 1, "{:?}", all_codes(&report));
}

/// `c.m is int Function(int)` — a non-generic method read carries its
/// declared signature as the tear-off face (`member_tear_off_ty`,
/// `PropertyAccessImpl.staticType` — oracle-verified TRUE row).
#[test]
fn method_tear_off_operand_reports_true() {
    let report = report_ops(
        "class C { int m(int x) => x; }\nvoid f() { var c = C(); c.m is int Function(int); }",
    );
    let rows = utc_rows(&report);
    assert_eq!(rows.len(), 1, "{:?}", all_codes(&report));
}

// —— Generic signature unification (the 2620-fail family) ———————————————

/// `fn is Fn` — the nested unification: the typedef's outer `<T>`
/// unifies with the tear-off's, and the inner `Function<S>(S)` pair
/// matches leaf-for-leaf under the shared fresh set
/// (type_system.dart:1581-1635). THE under-report pin: without
/// `signature_leg_subtype`'s recursion the inner generic pair answered
/// `None` through `function_shape_sig`'s count-only rebuild.
#[test]
fn nested_generic_signature_identical_reports_true() {
    let report = report_ops(
        "typedef Fn = void Function<S>(S val) Function<T>(T val);\n\
         void Function<S>(S) fn<T>(T val) => <R>(R val) {};\n\
         void f() { fn is Fn; }",
    );
    let rows = utc_rows(&report);
    assert_eq!(
        rows.len(),
        1,
        "fn is Fn proves under the shared fresh set: {:?}",
        all_codes(&report)
    );
    assert!(rows[0].message.contains("always 'true'"));
}

/// `fn is Gn` — the inner signatures differ in the `T`-vs-`S` formal:
/// upstream's fresh variables make the pair informative — the oracle is
/// silent, and `Some(false)`/undecidable both keep that silence.
#[test]
fn nested_generic_signature_misfit_stays_silent() {
    let report = report_ops(
        "typedef Fn = void Function<S>(S val) Function<T>(T val);\n\
         typedef Gn = void Function<S>(T val) Function<T>(T val);\n\
         void Function<S>(S) fn<T>(T val) => <R>(R val) {};\n\
         void Function<S>(T) gn<T>(T val) => <R>(T val) {};\n\
         void f() { fn is Gn; gn is Fn; }",
    );
    assert!(
        utc_rows(&report).is_empty(),
        "the transposed nested formals stay informative: {:?}",
        all_codes(&report)
    );
}

/// `xn is Xn` — the bounded group: `T extends num` on both sides must
/// satisfy `relateTypeParameters`'s bound-equality leg. The tear-off's
/// declared bound rides `exec_bounds`; the typedef side's rides the
/// `AliasRhs::Fn` capture's `type_params` — a NON-nested function-RHS
/// registers it (the nested spelling drops out of `index.aliases` on the
/// capture's stray-root arm — a residual, not modeled here).
#[test]
fn bounded_generic_signature_reports_true() {
    let report = report_ops(
        "typedef Xn = T Function<T extends num>(T val);\n\
         T xn<T extends num>(T val) => val;\n\
         void f() { xn is Xn; }",
    );
    let rows = utc_rows(&report);
    assert_eq!(
        rows.len(),
        1,
        "xn is Xn proves with the declared bounds: {:?}",
        all_codes(&report)
    );
}

/// `xn is Yn` — the formal's `num` spelling is no fresh variable: the
/// leg's `T <: num` ask answers through the shared fresh set's bound
/// (subtype.dart:484-493), so `T Function<T extends num>(num)` is
/// informative where `T Function<T extends num>(T)` proves — oracle-
/// verified silent.
#[test]
fn bounded_signature_concrete_formal_stays_silent() {
    let report = report_ops(
        "typedef Yn = T Function<T extends num>(num val);\n\
         T xn<T extends num>(T val) => val;\n\
         void f() { xn is Yn; }",
    );
    assert!(
        utc_rows(&report).is_empty(),
        "xn is Yn stays informative: {:?}",
        all_codes(&report)
    );
}

/// `zn is Xn` AND `zn is Yn` — `zn`'s formal spells the concrete `num`
/// where the typedef's parameter `T` sits: on `zn is Xn` the leg's LEFT
/// leaf is the fresh `T`, answering `num <: num` through its `extends
/// num` bound (subtype.dart:484-493); on `zn is Yn` the formals coincide
/// leaf-for-leaf — oracle reports TRUE on both.
#[test]
fn concrete_formal_satisfies_either_bound() {
    let report = report_ops(
        "typedef Xn = T Function<T extends num>(T val);\n\
         typedef Yn = T Function<T extends num>(num val);\n\
         T zn<T extends num>(num val) => val;\n\
         void f() { zn is Xn; zn is Yn; }",
    );
    let rows = utc_rows(&report);
    assert_eq!(
        rows.len(),
        2,
        "zn proves against both typedefs: {:?}",
        all_codes(&report)
    );
    assert!(rows.iter().all(|d| d.message.contains("always 'true'")));
}

// —— Never / dynamic / Null faces ———————————————————————————————————————

/// `n is int` over `Never` — Left Bottom (`Never <: T1`,
/// subtype.dart:163-165): the check is constant-true.
#[test]
fn never_operand_reports_true() {
    let report = report_ops("void f() { Never n = throw 0; n is int; }");
    let rows = utc_rows(&report);
    assert_eq!(rows.len(), 1, "{:?}", all_codes(&report));
}

/// `o is dynamic` — the `is dynamic` gate reports unconditionally
/// (best_practices_verifier.dart:1006-1010).
#[test]
fn dynamic_target_reports_true() {
    let report = report_ops("void f() { Object o = 0; o is dynamic; }");
    let rows = utc_rows(&report);
    assert_eq!(rows.len(), 1, "{:?}", all_codes(&report));
}

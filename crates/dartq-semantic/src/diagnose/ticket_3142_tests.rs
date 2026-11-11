//! #3142 — `unnecessary_type_check` residual faces after #3129.
//!
//! The verdict is `_checkAllTypeChecks`'s single `isSubtypeOf(leftType,
//! rightType)` question (upstream Dart SDK source,
//! pkg/analyzer/lib/src/error/best_practices_verifier.dart:985-1038), with
//! the function pair answered by `_isFunctionSubtypeOf`
//! (subtype.dart:86-171). Four under/over-report roots this ticket closes:
//!
//! * **Qualified (`p.T`/`core.`) heads** — the captured alias RHS and the
//!   written target spell `core.List` where the operand's declared face
//!   spells the same declaration bare (`List`). `canonical_dotted_ty`'s
//!   same-element rewrite now runs on both faces inside
//!   `check_type_check` — without it `f2 is F2<int>` (whose typedef RHS
//!   spells `core.List<core.int>`) read a different head than `f2`'s
//!   declared `core.List` return and stayed silent.
//! * **Generic member tear-off operands** — `member_tear_off_ty` and the
//!   enclosing-container arm of `operand_tear_off_ty` gated
//!   `entry.type_params == 0`, so every `m*<A>`/`m*<B extends core.int>`
//!   method read answered `Unknown` and the verdict never reached the
//!   subtype ask. The generic face now carries `type_params_named`
//!   (+ bounds), which `generic_signature_subtype` unifies against the
//!   generic alias or written signature (`m4 is F4<int>`,
//!   `m4 is List<Function> Function<A>(A x)` — `relateTypeParameters`,
//!   type_system.dart:1581-1635).
//! * **Raw generic aliases** — `x is F0` over `typedef F0<T> = …`
//!   survived the first fold as the instantiate-to-bounds head
//!   `F0<dynamic>`; a second `fold_alias_ty` pass reads that arg-carrying
//!   head through `fold_parameterized_alias`, substituting the bounds into
//!   the captured signature.
//! * **Required-vs-optional positionals** — the canonical `FnShapeId`
//!   merges required and optional positionals (`Function(int)` and
//!   `Function([int])` are one id), so the arity gate
//!   `_isFunctionSubtypeOf` runs (subtype.dart:107-113) could not read the
//!   split. `fn_shape_required` records each written signature's required
//!   count at its own `end_function_type`; `fn_face_sig`/`operand_fn_sig`
//!   rebuild `FnVarSig`s with honest counts and route the non-generic
//!   function pair through `fn_sig_subtype` instead of the pooled
//!   `sub == sup` reflexivity — `f5 is F5<int>` and the optional-formal
//!   families decide on the real arity now.
//!
//! Oracle rows captured 2026-10-31 against `dart analyze --format
//! machine` 3.13.7. The `core.`-spelled fixtures run under
//! `report_ops_core_prefixed` — the corpus's `import 'dart:core' as core`
//! wiring, where `core.List` and `List` name one declaration (the plain
//! rig never binds the prefix the fixture writes, and its prelude lands
//! ahead of the import directive).

use super::test_rig::{report_ops, report_ops_core_prefixed};
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

// —— Qualified heads (the `core.`/`p.` canonicalization faces) ————————————

/// `f2 is F2<int>` — the typedef RHS spells `core.List<core.int>` while
/// `f2`'s declared return spells the same `List` element: without the
/// canonical rewrite the folded target and the operand face read different
/// heads and the always-true verdict stayed silent (oracle-verified row).
#[test]
fn core_spelled_alias_rhs_reports_true() {
    let report = report_ops_core_prefixed(
        "import 'dart:core' as core;\n\
         typedef F2<T> = core.List<core.int> Function(int, [List<T> x]);\n\
         core.List<core.int> f2(int x0, [List<int> x = const []]) => throw 'u';\n\
         void g() { f2 is F2<int>; }",
    );
    let rows = utc_rows(&report);
    assert_eq!(rows.len(), 1, "{:?}", all_codes(&report));
    assert!(rows[0].message.contains("always 'true'"));
}

/// `f2 is List<int> Function(int, [List<int> x])` — the plain-`List`
/// written target against the `core.List`-declared operand: the same
/// element under both spellings (oracle-verified TRUE).
#[test]
fn plain_target_against_core_operand_reports_true() {
    let report = report_ops_core_prefixed(
        "import 'dart:core' as core;\n\
         core.List<core.int> f2(int x0, [List<int> x = const []]) => throw 'u';\n\
         void g() { f2 is List<int> Function(int, [List<int> x]); }",
    );
    let rows = utc_rows(&report);
    assert_eq!(rows.len(), 1, "{:?}", all_codes(&report));
}

/// `f2 is core.List<core.int> Function(int, [List<int> x])` — the
/// qualified spelling on the written target side too (oracle-verified
/// TRUE; the #3129 baseline).
#[test]
fn core_target_against_core_operand_reports_true() {
    let report = report_ops_core_prefixed(
        "import 'dart:core' as core;\n\
         core.List<core.int> f2(int x0, [List<int> x = const []]) => throw 'u';\n\
         void g() { f2 is core.List<core.int> Function(int, [List<int> x]); }",
    );
    let rows = utc_rows(&report);
    assert_eq!(rows.len(), 1, "{:?}", all_codes(&report));
}

// —— Generic member tear-off operands ————————————————————————————————————

/// `u.m4 is F4<int>` — a generic instance method read through a typed
/// receiver carries its own `<A>` group on the signature; the generic
/// alias's matching `<A>` unifies (oracle-verified TRUE).
#[test]
fn generic_method_alias_target_reports_true() {
    let report = report_ops(
        "typedef F4<T> = List<Function> Function<A>(A x);\n\
         class U<T> { List<Function> m4<A>(A x) => throw 'u'; }\n\
         void g(U<int> u) { u.m4 is F4<int>; }",
    );
    let rows = utc_rows(&report);
    assert_eq!(rows.len(), 1, "{:?}", all_codes(&report));
}

/// `u.m4 is List<Function> Function<A>(A x)` — the written generic
/// signature against the same generic method read (oracle-verified TRUE).
#[test]
fn generic_method_written_signature_reports_true() {
    let report = report_ops(
        "class U<T> { List<Function> m4<A>(A x) => throw 'u'; }\n\
         void g(U<int> u) { u.m4 is List<Function> Function<A>(A x); }",
    );
    let rows = utc_rows(&report);
    assert_eq!(rows.len(), 1, "{:?}", all_codes(&report));
}

/// `m5 is F5<int>` — a function-returning generic method's tear-off
/// (`int Function([Function x]) Function<B extends core.int>()`) unifies
/// against the alias's identical nested signature with matching bounds
/// (oracle-verified TRUE; the enclosing-container bare read).
#[test]
fn fn_returning_generic_method_alias_reports_true() {
    let report = report_ops_core_prefixed(
        "import 'dart:core' as core;\n\
         typedef F5<T> = int Function([Function x]) Function<B extends core.int>();\n\
         class U<T> { int Function([Function x]) m5<B extends core.int>() => throw 'u'; }\n\
         void g(U<int> u) { u.m5 is F5<int>; }",
    );
    let rows = utc_rows(&report);
    assert_eq!(rows.len(), 1, "{:?}", all_codes(&report));
}

/// `m5 is int Function([Function x]) Function<B extends core.int>()` —
/// the written nested signature against the same read (oracle-verified
/// TRUE).
#[test]
fn fn_returning_generic_method_written_sig_reports_true() {
    let report = report_ops_core_prefixed(
        "import 'dart:core' as core;\n\
         class U<T> { int Function([Function x]) m5<B extends core.int>() => throw 'u'; }\n\
         void g(U<int> u) { u.m5 is int Function([Function x]) Function<B extends core.int>(); }",
    );
    let rows = utc_rows(&report);
    assert_eq!(rows.len(), 1, "{:?}", all_codes(&report));
}

/// A generic top-level function tear-off already unified (#3129's
/// `tear_off_sig` face); the qualified `u.m*` path is the gap this ticket
/// closes — `u.m22 is F22<int>` over `Function<A>() Function<B extends
/// int>()` (oracle-verified TRUE).
#[test]
fn generic_fn_returning_fn_member_reports_true() {
    let report = report_ops(
        "typedef F22<T> = Function<A>() Function<B extends int>();\n\
         class U<T> { Function<A>() m22<B extends int>() => throw 'u'; }\n\
         void g(U<int> u) { u.m22 is F22<int>; }",
    );
    let rows = utc_rows(&report);
    assert_eq!(rows.len(), 1, "{:?}", all_codes(&report));
}

// —— Raw generic aliases —————————————————————————————————————————————————

/// `x is F0` — a bare parameterized alias target instantiates to bounds
/// (`F0<dynamic>`) and folds into the captured signature: `x`'s declared
/// `F0<int>` proves the always-true pair (oracle-verified row).
#[test]
fn raw_generic_alias_target_reports_true() {
    let report = report_ops(
        "typedef F0<T> = int Function(int y, [Function x]);\n\
         void g(F0<int> x) { x is F0; }",
    );
    let rows = utc_rows(&report);
    assert_eq!(rows.len(), 1, "{:?}", all_codes(&report));
}

// —— Required-vs-optional positionals ————————————————————————————————————

/// `f5 is F5<int>` — a top-level function-returning-function tear-off
/// whose optional formal `[Function x]` the pooled shape once read as
/// required: the honest split proves the identical signature
/// (oracle-verified TRUE).
#[test]
fn optional_formal_alias_reports_true() {
    let report = report_ops_core_prefixed(
        "import 'dart:core' as core;\n\
         typedef F5<T> = int Function([Function x]) Function<B extends core.int>();\n\
         int Function([Function x]) f5<B extends core.int>() => throw 'u';\n\
         void g() { f5 is F5<int>; }",
    );
    let rows = utc_rows(&report);
    assert_eq!(rows.len(), 1, "{:?}", all_codes(&report));
}

/// `g is int Function(int)` where `g: int Function([int])` — the
/// optional operand accepts every call the required target accepts (its
/// required count 0 is under the target's 1, totals equal), so
/// `_isFunctionSubtypeOf`'s arity gate (subtype.dart:107-113) answers
/// true — constant-true (oracle-verified).
#[test]
fn optional_operand_against_required_target_reports_true() {
    let report =
        report_ops("void h() { int Function([int]) g = ([x]) => 0; g is int Function(int); }");
    let rows = utc_rows(&report);
    assert_eq!(rows.len(), 1, "{:?}", all_codes(&report));
}

/// `g is int Function([int])` where `g: int Function(int)` — the
/// operand's one required positional exceeds the target's zero
/// (`from.required > to.required`, subtype.dart:107-113): callers may
/// invoke the target with no argument, which the source rejects, so the
/// check is informative and the oracle stays silent.
#[test]
fn required_operand_against_optional_target_stays_silent() {
    let report =
        report_ops("void h() { int Function(int) g = (x) => x; g is int Function([int]); }");
    assert!(
        utc_rows(&report).is_empty(),
        "Function(int) is not a Function([int]): {:?}",
        all_codes(&report)
    );
}

/// `r is List<int> Function([int x])` where `r: List<int> Function(int)`
/// — the same-`FnShapeId` collision the `last_fn_seal`/`fn_shape_required`
/// pair answers on the verdict path: the operand's declaration sealed the
/// map first (required 1) and the `is` target's own close rides
/// `last_fn_seal` (required 0), so `fn_sig_subtype`'s arity gate answers
/// `false` where the pooled `sub == sup` once read one shape
/// (oracle-silent).
#[test]
fn same_shape_required_operand_against_optional_target_stays_silent() {
    let report = report_ops(
        "void h() { List<int> Function(int) r = (x) => x; r is List<int> Function([int x]); }",
    );
    assert!(
        utc_rows(&report).is_empty(),
        "required operand cannot cover an optional target: {:?}",
        all_codes(&report)
    );
}

/// `r is List<int> Function(int)` where `r: List<int> Function([int])` —
/// the collision's other polarity: the operand's map answer is 0, the
/// target's seal is 1, and `from.required <= to.required` plus the equal
/// positional leaves prove the always-true pair (oracle-verified).
#[test]
fn same_shape_optional_operand_against_required_target_reports_true() {
    let report = report_ops(
        "void h() { List<int> Function([int]) r = ([x]) => x; r is List<int> Function(int); }",
    );
    let rows = utc_rows(&report);
    assert_eq!(rows.len(), 1, "{:?}", all_codes(&report));
}

// The residual bound — two spelling sites of the SAME interned shape
// feeding non-verdict subtype asks (a nested signature leg inside
// `strict_subtype`) — shares the map's first-write answer; closing it
// needs the model's required column (the FnShape split), out of scope.

// —— Regression: #3129 faces stay green ———————————————————————————————————

/// `f0 is F0<int>` — the non-generic alias pair still proves
/// (oracle-verified TRUE).
#[test]
fn non_generic_alias_still_reports_true() {
    let report = report_ops(
        "typedef F0<T> = int Function(int y, [Function x]);\n\
         int f0(int y, [Function? x]) => 0;\n\
         void g() { f0 is F0<int>; }",
    );
    let rows = utc_rows(&report);
    assert_eq!(rows.len(), 1, "{:?}", all_codes(&report));
}

/// `i is String` — a sibling interface pair is still informative
/// (regression: the canonical/fold passes must not over-prove a real
/// subtype-negative pair).
#[test]
fn sibling_type_target_still_silent() {
    let report = report_ops("void f() { int i = 0; i is String; }");
    assert!(utc_rows(&report).is_empty(), "{:?}", all_codes(&report));
}

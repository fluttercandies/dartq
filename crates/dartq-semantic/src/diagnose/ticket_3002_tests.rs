//! The #3002 `unnecessary_type_check` pins: the `is`/`is!` verdict
//! (`_checkAllTypeChecks`, upstream Dart SDK source,
//! pkg/analyzer/lib/src/error/best_practices_verifier.dart:877-930 —
//! `isSubtypeOf(leftType, rightType)` constant-true reports, the `is!`
//! polarity mapping to the 'false' row; every undecidable pair is
//! silent). The rows cover the always-true `is`, the `is!` false
//! polarity, the undecidable and `dynamic` silences, the promoted
//! operand's verdict, and the function-typed operand faces (a
//! declaration read's tear-off static type — `PropertyAccessImpl`/
//! `FunctionReference`'s static type).

use super::test_rig::report_ops;

/// `unnecessary_type_check` rows in source order.
fn verdicts(report: &super::FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unnecessary_type_check")
        .map(|d| d.message.to_string())
        .collect()
}

/// The always-true `is`: the operand's static type subtypes the checked
/// face (`isSubtypeOf` answers true), the plain polarity reads the
/// 'true' message.
#[test]
fn always_true_is_reports_true_row() {
    let fixture = concat!(
        "void t() {\n",
        "  int i = 0;\n",
        "  if (i is int) {}\n",
        "  if (i is num) {}\n",
        "  if (i is Object) {}\n",
        "}\n",
    );
    let report = report_ops(fixture);
    assert_eq!(
        verdicts(&report),
        [
            "Unnecessary type check; the result is always 'true'.",
            "Unnecessary type check; the result is always 'true'.",
            "Unnecessary type check; the result is always 'true'.",
        ],
        "{:?}",
        report.diagnostics
    );
}

/// The `is!` polarity over a provable subtype: the check always FAILS —
/// the oracle's 'false' row.
#[test]
fn always_succeeding_is_not_reports_false_row() {
    let fixture = concat!(
        "void t() {\n",
        "  int i = 0;\n",
        "  if (i is! int) {}\n",
        "  if (i is! num) {}\n",
        "}\n",
    );
    let report = report_ops(fixture);
    assert_eq!(
        verdicts(&report),
        [
            "Unnecessary type check; the result is always 'false'.",
            "Unnecessary type check; the result is always 'false'.",
        ],
        "{:?}",
        report.diagnostics
    );
}

/// Undecidable pairs stay silent: `Object`/`num` operand against an
/// unrelated or narrower checked face is a real runtime question — the
/// upstream walk reports nothing (`isSubtypeOf` answers false, the
/// verifier's `report()` never runs — there is no 'always fails' arm).
/// The `n is Object` row DOES report upstream (`num <: Object` proves on
/// the declared face alone — the Right-Object tail of `isSubtypeOf`),
/// and `q is int` proves through the initializer's non-null promotion;
/// both stand as the oracle's 'true' rows beside the silent trio.
#[test]
fn undecidable_pairs_stay_silent() {
    let fixture = concat!(
        "void t() {\n",
        "  Object o = 0;\n",
        "  num n = 0;\n",
        "  if (o is int) {}\n",
        "  if (n is Object) {}\n",
        "  if (o is! int) {}\n",
        "  if (o is num) {}\n",
        "}\n",
    );
    let report = report_ops(fixture);
    assert_eq!(
        verdicts(&report),
        ["Unnecessary type check; the result is always 'true'."],
        "only `num <: Object` proves on the declared faces: {:?}",
        report.diagnostics
    );
}

/// The initializer's non-null promotion (`int? q = 0`) is the flow's
/// `PromotedRead` face: `q`'s read inside `q is int` carries the promoted
/// `int` — a `isSubtypeOf(int, int)` constant-true upstream. The pass-1
/// verdict runs before the flow replay; #3005's deferred-verdict pass
/// (`Engine::deferred_type_checks`) re-decides the site against the
/// materialized read rows and emits the oracle's row.
#[test]
fn initializer_promoted_operand_reports_true() {
    let fixture = concat!(
        "void t() {\n",
        "  int? q = 0;\n",
        "  if (q is int) {}\n",
        "}\n",
    );
    let report = report_ops(fixture);
    assert_eq!(
        verdicts(&report),
        ["Unnecessary type check; the result is always 'true'."],
        "the promoted operand decides: {:?}",
        report.diagnostics
    );
}

/// A `dynamic` operand decides nothing either way: `d is T` and
/// `d is! T` are real checks (`dynamic`'s Left-Top lattice answers no
/// subtype question below the top faces).
#[test]
fn dynamic_operand_stays_silent() {
    let fixture = concat!(
        "void t() {\n",
        "  dynamic d = 0;\n",
        "  if (d is int) {}\n",
        "  if (d is! int) {}\n",
        "  if (d is String) {}\n",
        "}\n",
    );
    let report = report_ops(fixture);
    assert!(
        verdicts(&report).is_empty(),
        "dynamic operand is a runtime question: {:?}",
        report.diagnostics
    );
}

/// The promoted operand reads its promoted static type: inside
/// `if (o is String)`, `o`'s face is `String` — a re-check of the same
/// head is constant-true, a `is!` of it constant-false, and an
/// unrelated target stays a runtime question.
///
/// Residual (#3002 → #3005): `check_type_check` runs in pass 1, before
/// the flow replay stages the `PromotedRead`/`ReadType` facts, so the
/// operand's promoted face was not yet answerable. #3005's deferred
/// re-decision (`Engine::deferred_type_checks`, run after
/// `resolve_rows` beside the `filter_staged` teardown) replays the
/// verdict against the materialized read rows; the assertions pin the
/// oracle's truth.
#[test]
fn promoted_operand_reads_promoted_ty() {
    let fixture = concat!(
        "void t() {\n",
        "  Object o = 's';\n",
        "  if (o is String) {\n",
        "    if (o is String) {}\n",
        "    if (o is! String) {}\n",
        "    if (o is int) {}\n",
        "  }\n",
        "}\n",
    );
    let report = report_ops(fixture);
    assert_eq!(
        verdicts(&report),
        [
            "Unnecessary type check; the result is always 'true'.",
            "Unnecessary type check; the result is always 'false'.",
        ],
        "{:?}",
        report.diagnostics
    );
}

/// A top-level function read as an `is` operand is the tear-off — its
/// static type is the declared signature as a function type
/// (`FunctionReference`/`PropertyAccess`'s static type). `fn <: Function`
/// and `fn <: Object` both prove constant-true; `fn <: int` decides
/// nothing upstream (an interface target against a function operand is
/// no subtype, so the verifier stays silent).
#[test]
fn function_name_operand_tear_off_verdicts() {
    let fixture = concat!(
        "int fi(int x) => x;\n",
        "void t() {\n",
        "  if (fi is Function) {}\n",
        "  if (fi is Object) {}\n",
        "  if (fi is int Function(int)) {}\n",
        "  if (fi is! Function) {}\n",
        "  if (fi is int) {}\n",
        "}\n",
    );
    let report = report_ops(fixture);
    assert_eq!(
        verdicts(&report),
        [
            "Unnecessary type check; the result is always 'true'.",
            "Unnecessary type check; the result is always 'true'.",
            "Unnecessary type check; the result is always 'true'.",
            "Unnecessary type check; the result is always 'false'.",
        ],
        "{:?}",
        report.diagnostics
    );
}

/// A function-typed LOCAL (`var v = fi`) carries the same faces: the
/// inferred variable's `is Function`/`is Object`/signature pairs are
/// constant-true, the unrelated `is int` is silent.
#[test]
fn function_typed_local_verdicts() {
    let fixture = concat!(
        "int fi(int x) => x;\n",
        "void t() {\n",
        "  var v = fi;\n",
        "  if (v is Function) {}\n",
        "  if (v is Object) {}\n",
        "  if (v is int Function(int)) {}\n",
        "  if (v is int) {}\n",
        "}\n",
    );
    let report = report_ops(fixture);
    assert_eq!(
        verdicts(&report),
        [
            "Unnecessary type check; the result is always 'true'.",
            "Unnecessary type check; the result is always 'true'.",
            "Unnecessary type check; the result is always 'true'.",
        ],
        "{:?}",
        report.diagnostics
    );
}

/// A collection literal operand carries its inferred face: `{1, 2}` is
/// `Set<int>` — `is Set` proves through the raw all-dynamic head
/// (`Set<int> <: Set<dynamic>`), `is List` stays a runtime question.
#[test]
fn collection_literal_operand_verdicts() {
    let fixture = concat!(
        "void t() {\n",
        "  if (<int>{1, 2} is Set) {}\n",
        "  if (<int>{1, 2} is List) {}\n",
        "  if ([1, 2] is List) {}\n",
        "}\n",
    );
    let report = report_ops(fixture);
    assert_eq!(
        verdicts(&report),
        [
            "Unnecessary type check; the result is always 'true'.",
            "Unnecessary type check; the result is always 'true'.",
        ],
        "{:?}",
        report.diagnostics
    );
}

/// `is dynamic`/`is! dynamic` are the constant pair: every non-invalid
/// operand answers the top face (the `rightType is DynamicType` arm —
/// best_practices_verifier.dart:898-902).
#[test]
fn dynamic_target_pair() {
    let fixture = concat!(
        "void t() {\n",
        "  int i = 0;\n",
        "  if (i is dynamic) {}\n",
        "  if (i is! dynamic) {}\n",
        "}\n",
    );
    let report = report_ops(fixture);
    assert_eq!(
        verdicts(&report),
        [
            "Unnecessary type check; the result is always 'true'.",
            "Unnecessary type check; the result is always 'false'.",
        ],
        "{:?}",
        report.diagnostics
    );
}

/// The `null` literal operand: `Null <: T` decides — `null is Null` and
/// `null is Object?` prove, `null is int` is silent.
#[test]
fn null_literal_operand_verdicts() {
    let fixture = concat!(
        "void t() {\n",
        "  if (null is Null) {}\n",
        "  if (null is int) {}\n",
        "}\n",
    );
    let report = report_ops(fixture);
    assert_eq!(
        verdicts(&report),
        ["Unnecessary type check; the result is always 'true'."],
        "{:?}",
        report.diagnostics
    );
}

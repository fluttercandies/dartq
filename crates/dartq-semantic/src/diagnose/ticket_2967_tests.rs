//! The #2967 pins: the `assignable(src, dst)` verdict's residual shape
//! families — the Right-Bottom (`Never` target) refusal, the Right
//! Nullable `S1?` decomposition (`int? x = "s"` reports instead of the
//! blanket top-type pass), the bare function-name tear-off read at the
//! initializer/assignment/return positions, and the return window's
//! send-result resolution (`int f() { return g(); }` asks the send's own
//! static type).
//!
//! Upstream premises: `subtype.dart`'s rule order (Right Nullable
//! :224-246, Left Nullable :389-392, Left Bottom :74-77, the trailing
//! `false` a `Never` target falls to), `type_system.dart:860-891`
//! (`isAssignableTo`'s `dynamic`-downcast relaxation), and the element
//! face of a function-name read (its static type is the declaration's
//! `FunctionType`, formals and written return included).

use super::async_return_tests::{code_names, report};
use super::super::types::FileReport;

/// The three codes this ticket owns; the pins assert over these alone so
/// incidental warnings (`unused_local_variable` on pin locals) never blur
/// the verdict.
fn assign_codes(report: &FileReport) -> Vec<&'static str> {
    code_names(report)
        .into_iter()
        .filter(|code| {
            matches!(
                *code,
                "invalid_assignment" | "argument_type_not_assignable" | "return_of_invalid_type"
            )
        })
        .collect()
}

/// Right Bottom: `Never x = 1;` reports — no subtyping rule holds into
/// `Never` but reflexivity (subtype.dart's trailing `false`).
#[test]
fn never_target_reports() {
    let report = report("class Never {}\nNever e = 1;");
    assert_eq!(assign_codes(&report), ["invalid_assignment"]);
}

/// A `null` initializer against the `Never` target reports the same way
/// (Left Null's only true targets are `Null` and `S?`).
#[test]
fn never_target_null_reports() {
    let report = report("class Never {}\nNever f = null;");
    assert_eq!(assign_codes(&report), ["invalid_assignment"]);
}

/// Left Bottom survives the `?` target: a `Never`-typed read assigns into
/// `int?` (`Never <: T1` outright, subtype.dart:74-77) — the declared
/// walk must not overturn the proof with an empty supertype closure.
#[test]
fn never_source_into_nullable_stays_clean() {
    let report = report("class Never {}\nNever nv = throw 1;\nint? h = nv;");
    assert!(assign_codes(&report).is_empty(), "{:?}", code_names(&report));
}

/// A `Never`-typed read is clean against its own target too.
#[test]
fn never_source_into_never_stays_clean() {
    let report = report("class Never {}\nNever nv = throw 1;\nNever g = nv;");
    assert!(assign_codes(&report).is_empty(), "{:?}", code_names(&report));
}

/// Right Nullable (subtype.dart:224-246): `int? x = "s"` reports — the
/// `?` suffix is not a top type.
#[test]
fn nullable_target_mismatch_reports() {
    let report = report("class int {}\nclass String {}\nint? a = \"s\";");
    assert_eq!(assign_codes(&report), ["invalid_assignment"]);
}

/// The same refusal through a typed variable source.
#[test]
fn nullable_target_var_source_reports() {
    let report = report("class int {}\nclass String {}\nString s = \"x\";\nint? a = s;");
    assert_eq!(assign_codes(&report), ["invalid_assignment"]);
}

/// Right Nullable's `Null` leg: `int? d = null` stays clean.
#[test]
fn nullable_target_null_stays_clean() {
    let report = report("class int {}\nint? d = null;");
    assert!(assign_codes(&report).is_empty(), "{:?}", code_names(&report));
}

/// Left Nullable ahead of Right Nullable: `int? <: int?` decomposes on
/// the source side and stays clean.
#[test]
fn nullable_into_same_nullable_stays_clean() {
    let report = report("class int {}\nint one = 1;\nint? d = one;\nint? e = d;");
    assert!(assign_codes(&report).is_empty(), "{:?}", code_names(&report));
}

/// The bare function-name initializer IS the tear-off: `int x = nf;`
/// asks `num Function()` against `int` and reports (the oracle's element
/// face for the read).
#[test]
fn tear_off_into_non_function_reports() {
    let report = report("class int {}\nclass num {}\nnum nf() => 1;\nint x = nf;");
    assert_eq!(assign_codes(&report), ["invalid_assignment"]);
    assert!(
        report.diagnostics[0].message.contains("num Function()"),
        "{:?}",
        report.diagnostics[0].message
    );
}

/// Return covariance through the tear-off: `int Function() p = nf`
/// (num return) reports.
#[test]
fn tear_off_return_covariance_reports() {
    let report = report("class int {}\nclass num {}\nnum nf() => 1;\nint Function() p = nf;");
    assert_eq!(assign_codes(&report), ["invalid_assignment"]);
}

/// Parameter contravariance through the tear-off: an `int`-accepting
/// function does not stand where a `num`-accepting one is required
/// (`subtype.dart:127` — `gParameter <: fParameter`).
#[test]
fn tear_off_param_contravariance_reports() {
    let report = report(
        "class int {}\nclass num {}\nint iin(int p) => p;\nint Function(num) p2 = iin;\nnum Function(num) p3 = iin;",
    );
    assert_eq!(
        assign_codes(&report),
        ["invalid_assignment", "invalid_assignment"]
    );
}

/// The matching tear-off stays clean (same signature both sides).
#[test]
fn tear_off_matching_stays_clean() {
    let report = report("void vf() {}\nvoid Function() q = vf;");
    assert!(assign_codes(&report).is_empty(), "{:?}", code_names(&report));
}

/// The nullable function target runs the same signature rule through the
/// base leg: `int Function()? q = nf` (num return) reports.
#[test]
fn tear_off_into_nullable_function_reports() {
    let report = report("class int {}\nclass num {}\nnum nf() => 1;\nint Function()? q = nf;");
    assert_eq!(assign_codes(&report), ["invalid_assignment"]);
}

/// The tear-off at the return position: `int f() { return nf; }` asks
/// the signature against the declared host.
#[test]
fn tear_off_return_position_reports() {
    let report = report("class int {}\nclass num {}\nnum nf() => 1;\nint f() { return nf; }");
    assert_eq!(assign_codes(&report), ["return_of_invalid_type"]);
}

/// A declared local function's tear-off answers through the frame
/// capture (#2852) at the initializer position too.
#[test]
fn local_fn_tear_off_reports() {
    let report = report(
        "class int {}\nclass num {}\nvoid main() {\n  num lx() => 1;\n  int y = lx;\n}",
    );
    assert_eq!(assign_codes(&report), ["invalid_assignment"]);
}

/// The return window reads the send's own static type
/// (`expression_row_ty`'s mirror): `int f() { return g(); }` asks
/// `String` against `int`.
#[test]
fn return_send_result_reports() {
    let report = report(
        "class int {}\nclass String {}\nString g() => \"s\";\nint f1() { return g(); }",
    );
    assert_eq!(assign_codes(&report), ["return_of_invalid_type"]);
}

/// The arrow body runs the same resolution.
#[test]
fn arrow_send_result_reports() {
    let report = report("class int {}\nclass String {}\nString g() => \"s\";\nint f3() => g();");
    assert_eq!(assign_codes(&report), ["return_of_invalid_type"]);
}

/// A member send's result reads the method's declared return.
#[test]
fn member_send_result_reports() {
    let report = report(
        "class int {}\nclass String {}\nclass C { String m() => \"m\"; }\nC c = C();\nint f() { return c.m(); }",
    );
    assert_eq!(assign_codes(&report), ["return_of_invalid_type"]);
}

/// A `Future<int>`-typed expression against an `int` host reports at the
/// return position (the declared-subtype negative runs for every host,
/// rtv.dart's single `isAssignableTo` ask).
#[test]
fn return_future_into_int_reports() {
    let report = report(
        "class int {}\nclass Future<T> { Future.value(T v); }\nFuture<int> fi = Future.value(1);\nint f1() { return fi; }",
    );
    assert_eq!(assign_codes(&report), ["return_of_invalid_type"]);
}

/// The async matrix's both-legs report: `Future<int> f() async { return
/// fs; }` (a `Future<String>` source) flattens to `String` against the
/// declared `int` and reports (rtv.dart:274-289).
#[test]
fn async_return_future_source_reports() {
    let report = report(
        "class int {}\nclass String {}\nclass Future<T> { Future.value(T v); }\nFuture<String> fs = Future.value(\"s\");\nFuture<int> f() async { return fs; }",
    );
    assert_eq!(assign_codes(&report), ["return_of_invalid_type"]);
}

/// The legal async value return stays clean (`Future<int> f() async {
/// return 1; }` — the flattened ask accepts).
#[test]
fn async_return_value_stays_clean() {
    let report = report(
        "class int {}\nclass Future<T> { Future.value(T v); }\nFuture<int> f() async { return 1; }",
    );
    assert!(assign_codes(&report).is_empty(), "{:?}", code_names(&report));
}

/// The argument position's tear-off face keeps its own pin: passing the
/// tear-off where `int` is expected reports (the #2842 capture's face,
/// unchanged by the verdict wiring).
#[test]
fn tear_off_argument_into_non_function_reports() {
    let report = report(
        "class int {}\nclass num {}\nnum nf() => 1;\nvoid h(int n) {}\nvoid main() { h(nf); }",
    );
    assert_eq!(assign_codes(&report), ["argument_type_not_assignable"]);
}

/// The if-null assignment asks the right side against the slot's nullable
/// write type (upstream `_checkForInvalidAssignment(node.writeType!)` runs
/// for `??=` the same as `=`, assignment_expression_resolver.dart:361-365):
/// `String? s; s ??= 42;` reports `int` into `String?` — the `?` suffix is
/// not a top type.
#[test]
fn if_null_assign_into_nullable_reports() {
    let report = report(
        "class int {}\nclass String {}\nvoid main() {\n  String? s;\n  s ??= 42;\n}",
    );
    assert_eq!(assign_codes(&report), ["invalid_assignment"]);
}

/// The if-null twin with the operands swapped: `int? i2; i2 ??= "x";`
/// reports `String` into `int?`.
#[test]
fn if_null_assign_string_into_nullable_int_reports() {
    let report = report(
        "class int {}\nclass String {}\nvoid main() {\n  int? i2;\n  i2 ??= \"x\";\n}",
    );
    assert_eq!(assign_codes(&report), ["invalid_assignment"]);
}

/// The matching if-null write stays clean (`String? s; s ??= "v";` — the
/// base leg accepts `String`).
#[test]
fn if_null_assign_matching_stays_clean() {
    let report = report(
        "class String {}\nvoid main() {\n  String? s;\n  s ??= \"v\";\n}",
    );
    assert!(assign_codes(&report).is_empty(), "{:?}", code_names(&report));
}

/// The compound operator's own argument row
/// (`checkForArgumentTypeNotAssignableForArgument(rightHandSide)`,
/// assignment_expression_resolver.dart:368-372): `String t; t += 1;` asks
/// the right side against `String.+`'s `String` parameter and reports ATNA
/// alone — the `String` result writes back clean.
#[test]
fn compound_atna_string_plus_int_reports() {
    let report = report(
        "class int {}\nclass String {}\nvoid main() {\n  String t = \"a\";\n  t += 1;\n}",
    );
    assert_eq!(assign_codes(&report), ["argument_type_not_assignable"]);
    let atna = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "argument_type_not_assignable")
        .expect("ATNA row present");
    assert!(
        atna.message.contains("parameter type 'String'"),
        "{:?}",
        atna.message
    );
}

/// `int i; i += "x";` reports both rows: ATNA for `String` against
/// `int.+`'s inherited `num` parameter, and the write-back refusal of the
/// `num` result into `int`.
#[test]
fn compound_atna_beside_writeback_reports() {
    let report = report(
        "class int {}\nclass num {}\nclass String {}\nvoid main() {\n  int i = 1;\n  i += \"x\";\n}",
    );
    let mut codes = assign_codes(&report);
    codes.sort_unstable();
    assert_eq!(
        codes,
        ["argument_type_not_assignable", "invalid_assignment"]
    );
}

/// The clean compound stays clean on both faces (`int i; i += 2;` — `2`
/// fits `num`, the refined `int` result fits the slot).
#[test]
fn compound_matching_stays_clean() {
    let report = report(
        "class int {}\nclass num {}\nvoid main() {\n  int i = 1;\n  i += 2;\n}",
    );
    assert!(assign_codes(&report).is_empty(), "{:?}", code_names(&report));
}

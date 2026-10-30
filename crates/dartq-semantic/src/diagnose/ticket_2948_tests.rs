//! The #2948 pins: the `is`/`is!` operand's static type when the operand is
//! a parenthesized group or a compound receiver — the oracle's
//! `visitIsExpression` reads `leftNode.typeOrThrow` over the WHOLE
//! `(e).m()`/`(e)` operand (`_checkAllTypeChecks`,
//! best_practices_verifier.dart:987-1041), and a
//! `ParenthesizedExpression`'s static type is the inner expression's own
//! (`ParenthesizedExpressionImpl.staticType`). Until this fix the
//! parenthesized group's row carried only the inner row's push-time `ty`,
//! so a `TypeLiteral` (`(C)`), an unsolved creation (`(C())`), or a
//! completed send `(f())` folded to an Unknown receiver — the
//! `.member` merge classified `Receiver::Unknown`, the send's declared
//! return never resolved, and `check_type_check` suppressed the verdict.

use crate::diagnose::test_rig::{RigImport, report_ops, report_ops_imports};
use crate::diagnose::types::FileReport;

/// The `unnecessary_type_check` rows of `report`, as `(offset, length)`.
fn type_check_rows(report: &FileReport) -> Vec<(u32, u32)> {
    let mut found: Vec<(u32, u32)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unnecessary_type_check")
        .map(|d| (d.range.offset, d.range.length))
        .collect();
    found.sort_unstable();
    found
}

/// —— the parenthesized type literal's `Type` receiver ——

/// `(C).toString() is String` (the `class/literal_test_*` census family —
/// 20+ rows): the group's inner `TypeLiteral` types `Type`, so `.toString`
/// resolves through `Object`'s universal face and `String <: String`
/// folds constant-true.
#[test]
fn type_literal_call_reports() {
    let report = report_ops(
        "class C {}\n\
         \x20void main() {\n\
         \x20  if ((C).toString() is String) {}\n\
         \x20}",
    );
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "rows: {:#?}",
        report.diagnostics
    );
}

/// `(C)` as the is-check operand itself: `Type is Type` folds — the
/// group carries the inner literal's static type to the verdict.
#[test]
fn type_literal_group_is_type_reports() {
    let report = report_ops(
        "class C {}\n\
         \x20void main() {\n\
         \x20  if ((C) is Type) {}\n\
         \x20}",
    );
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "rows: {:#?}",
        report.diagnostics
    );
}

/// —— the parenthesized creation's container ——

/// `(C()).toString() is String` (the brief's reproduction): the
/// creation's group types `C`, the universal `toString` face answers
/// `String`, and the check folds.
#[test]
fn creation_group_call_reports() {
    let report = report_ops(
        "class C {}\n\
         \x20void main() {\n\
         \x20  if ((C()).toString() is String) {}\n\
         \x20}",
    );
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "rows: {:#?}",
        report.diagnostics
    );
}

/// `(C())` as the is-check operand: `C is C` folds through the group's
/// carried creation type.
#[test]
fn creation_group_is_own_type_reports() {
    let report = report_ops(
        "class C {}\n\
         \x20void main() {\n\
         \x20  if ((C()) is C) {}\n\
         \x20}",
    );
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "rows: {:#?}",
        report.diagnostics
    );
}

/// —— the parenthesized send's declared return ——

/// `(f()) is int` over an `int` callee: the group's inner send types its
/// declared return exactly like the unparenthesized face.
#[test]
fn paren_send_result_reports() {
    let report = report_ops(
        "int f() => 1;\n\
         \x20void main() {\n\
         \x20  if ((f()) is int) {}\n\
         \x20}",
    );
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "rows: {:#?}",
        report.diagnostics
    );
}

/// `(f()).m() is T`: the send's declared return is the group receiver —
/// the member walk resolves `m` on it and the return folds.
#[test]
fn paren_send_member_chain_reports() {
    let report = report_ops(
        "class C { String run() => ''; }\n\
         \x20C f() => C();\n\
         \x20void main() {\n\
         \x20  if ((f()).run() is String) {}\n\
         \x20}",
    );
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "rows: {:#?}",
        report.diagnostics
    );
}

/// —— the is! polarity over a parenthesized receiver ——

/// `(x).m() is! T`: the constant-true shape reports on `is!` exactly as
/// upstream's `negated` polarity arm.
#[test]
fn paren_receiver_isnot_reports() {
    let report = report_ops(
        "class C { String run() => ''; }\n\
         \x20void f(C x) {\n\
         \x20  if ((x).run() is! String) {}\n\
         \x20}",
    );
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "rows: {:#?}",
        report.diagnostics
    );
}

/// —— nested groups ——

/// `((e).m()).n() is T`: the inner group's member call is itself an
/// operand expression — its send result types the outer group's receiver.
#[test]
fn nested_group_member_reports() {
    let report = report_ops(
        "class C { C run() => C(); int n() => 0; }\n\
         \x20void f(C e) {\n\
         \x20  if (((e).run()).n() is int) {}\n\
         \x20}",
    );
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "rows: {:#?}",
        report.diagnostics
    );
}

/// —— regression controls ——

/// The simple operand's verdict is unchanged: a typed local folds as
/// before.
#[test]
fn simple_operand_unchanged() {
    let report = report_ops(
        "void f(int x) {\n\
         \x20  if (x is int) {}\n\
         \x20}",
    );
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "rows: {:#?}",
        report.diagnostics
    );
}

/// The unparenthesized compound send is unchanged (`f() is int` — the
/// #2854 face).
#[test]
fn bare_send_operand_unchanged() {
    let report = report_ops(
        "int f() => 1;\n\
         \x20void main() {\n\
         \x20  if (f() is int) {}\n\
         \x20}",
    );
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "rows: {:#?}",
        report.diagnostics
    );
}

/// A genuinely undecidable parenthesized operand stays silent:
/// `(miss).m() is T` names no container — the Unknown suppress stands.
#[test]
fn unresolvable_group_stays_silent() {
    let report = report_ops(
        "void f() {\n\
         \x20  if ((miss).m() is int) {}\n\
         \x20}",
    );
    assert!(
        type_check_rows(&report).is_empty(),
        "rows: {:#?}",
        report.diagnostics
    );
}

/// `(p.C).toString() is String`: the prefixed type literal's group
/// carries `Type` too — the `Member{Prefix}` arm of the operand-static
/// face answers it.
#[test]
fn prefixed_literal_call_reports() {
    let report = report_ops_imports(
        "void main() {\n\
         \x20  if ((p.C).toString() is String) {}\n\
         \x20}",
        &[RigImport {
            uri: "package:t/p.dart",
            prefix: Some("p"),
            source: "class C {}\n",
        }],
    );
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "rows: {:#?}",
        report.diagnostics
    );
}

/// The `is p.X` joined-name face is untouched (the fix-span prefix
/// substitution's contract): `p.T` as the checked type still decides
/// through the continuation row.
#[test]
fn prefixed_target_unchanged() {
    let report = report_ops_imports(
        "void f(T l) {\n\
         \x20  if (l is! p.T) {}\n\
         \x20}",
        &[
            RigImport {
                uri: "package:t/p.dart",
                prefix: Some("p"),
                source: "class T {}\n",
            },
            RigImport {
                uri: "package:t/p.dart",
                prefix: None,
                source: "class T {}\n",
            },
        ],
    );
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "rows: {:#?}",
        report.diagnostics
    );
}

/// —— the C4b bare-creation operand faces ——

/// `D() is D` (the unnamed-constructor send operand): the callee row's
/// `Bare(Container)` head carries no `ty`, and `callee_return_ty`'s
/// declaration table answers callables only — the listener re-types the
/// operand row to the created instance before `check_type_check`, the
/// same `InstanceCreationExpressionImpl.staticType` face upstream's
/// `leftNode.typeOrThrow` reads.
#[test]
fn bare_creation_operand_reports() {
    let report = report_ops(
        "class D {}\n\
         \x20void f() {\n\
         \x20  if (D() is D) {}\n\
         \x20}",
    );
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "rows: {:#?}",
        report.diagnostics
    );
}

/// `D(1) is D`: the argument row folds under the send, so the operand
/// row is still the `D` callee — the same re-type answers the verdict.
#[test]
fn bare_creation_with_args_reports() {
    let report = report_ops(
        "class D { D([int a = 0]); }\n\
         \x20void f() {\n\
         \x20  if (D(1) is D) {}\n\
         \x20}",
    );
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "rows: {:#?}",
        report.diagnostics
    );
}

/// `C is Type` (the bare `TypeLiteral` operand): the container head's
/// row reads `Type` — `TypeLiteralImpl.staticType` — where the verdict's
/// untyped faces answered nothing.
#[test]
fn bare_container_literal_reports() {
    let report = report_ops(
        "class C {}\n\
         \x20void f() {\n\
         \x20  if (C is Type) {}\n\
         \x20}",
    );
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "rows: {:#?}",
        report.diagnostics
    );
}

/// `p.D() is p.D`: the prefixed creation's `Member{Prefix}` callee —
/// the scoped lookup's kind separates the constructor from a prefixed
/// top-level call, which keeps `callee_return_ty`'s answer.
#[test]
fn prefixed_creation_operand_reports() {
    let report = report_ops_imports(
        "void f() {\n\
         \x20  if (p.D() is p.D) {}\n\
         \x20}",
        &[RigImport {
            uri: "package:t/p.dart",
            prefix: Some("p"),
            source: "class D {}\n",
        }],
    );
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "rows: {:#?}",
        report.diagnostics
    );
}

/// `p.C is Type`: the prefixed TypeLiteral — the non-invoked
/// `Member{Prefix}` container head reads `Type` exactly like the bare
/// `C` face.
#[test]
fn prefixed_container_literal_reports() {
    let report = report_ops_imports(
        "void f() {\n\
         \x20  if (p.C is Type) {}\n\
         \x20}",
        &[RigImport {
            uri: "package:t/p.dart",
            prefix: Some("p"),
            source: "class C {}\n",
        }],
    );
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "rows: {:#?}",
        report.diagnostics
    );
}

/// The extension-type creation `Ext(1)` folds the same `Created` row as a
/// class — `unnecessary_type_check` reports `Ext is Ext` (upstream's
/// operand `staticType` is the constructed `Ext`, not `Type`). A plain
/// `extension`'s `Ex(1)` is an invalid call upstream but still types the
/// operand `Ex`, so the verdict reports there too.
#[test]
fn extension_type_creation_operand_reports() {
    let report = report_ops(
        "extension type Ext(int i) {}\n\
         \x20void f() {\n\
         \x20  if (Ext(1) is Ext) {}\n\
         \x20}",
    );
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "rows: {:#?}",
        report.diagnostics
    );
}

/// The enum/mixin edge upstream reports (`E() is E`, `M() is M`): the
/// invalid-instantiation operand still carries the container type, so the
/// verdict folds — the `Created` row's `kind` is not a gate.
#[test]
fn enum_and_mixin_creation_operands_report() {
    let report = report_ops(
        "enum E { a, b }\n\
         \x20mixin M {}\n\
         \x20void f() {\n\
         \x20  if (E() is E) {}\n\
         \x20  if (M() is M) {}\n\
         \x20}",
    );
    assert_eq!(
        type_check_rows(&report).len(),
        2,
        "rows: {:#?}",
        report.diagnostics
    );
}

/// A plain `extension` is not instantiable — `Ex(1)` is an invalid call
/// upstream and its operand types `InvalidType`, so `Ex(1) is Ex` reports
/// nothing. `MicroKind::Extension` collapses `extension`/`extension type`
/// into one row kind, so the arm resolves the real container kind before
/// answering the created type.
#[test]
fn plain_extension_call_operand_stays_silent() {
    let report = report_ops(
        "extension Ex on int {}\n\
         \x20void f() {\n\
         \x20  if (Ex(1) is Ex) {}\n\
         \x20}",
    );
    assert!(
        type_check_rows(&report).is_empty(),
        "rows: {:#?}",
        report.diagnostics
    );
}

/// The over-emit guard: a callable send's declared return still decides
/// (`g() is int`), and a genuinely-undecidable operand (`miss()`)
/// suppresses the verdict — the re-type claims only the container heads
/// the oracle types as `Type`/the created instance.
#[test]
fn callable_and_unresolvable_operands_stay_honest() {
    let report = report_ops(
        "int g() => 1;\n\
         \x20void f() {\n\
         \x20  if (g() is int) {}\n\
         \x20  if (miss() is int) {}\n\
         \x20}",
    );
    assert_eq!(
        type_check_rows(&report).len(),
        1,
        "rows: {:#?}",
        report.diagnostics
    );
}

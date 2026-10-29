//! The #2832 method type-argument pins: a type argument that names a
//! local reports `non_type_as_type_argument` at the identifier AND
//! credits the local a read — the oracle's `visitNamedType` runs
//! `_useIdentifierElement` on the resolved element in every type
//! position (unused_local_elements_verifier.dart:243-246/:402-423), and
//! the resolver's decision order puts the argument face before the
//! LocalVariableElement / LocalFunctionElement arm
//! (`_isTypeInTypeArgumentList` guards `nonTypeAsTypeArgument`,
//! named_type_resolver.dart:659-691). The co19 shape is
//! `f(a<b, c>(d))` with `b`/`c` locals (ambiguities_A01_t01): both names
//! report, both declarations stay spared the `unused_local_variable`
//! row, and the `is`-operand exclusion (`_insideIsExpression`) keeps its
//! operand mentions uncounted while a true type argument (a builtin, a
//! typedef) never reports.

use super::test_rig::{CORE_PRELUDE, report_ops, rows};
use super::*;

/// The `non_type_as_type_argument` rows as
/// `message @offset:length`, source order.
fn nta_rows(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "non_type_as_type_argument")
        .map(|d| format!("{} @{}:{}", d.message, d.range.offset, d.range.length))
        .collect()
}

/// The `unused_local_variable` rows as `message @offset:length`, source
/// order.
fn uv_rows(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unused_local_variable")
        .map(|d| format!("{} @{}:{}", d.message, d.range.offset, d.range.length))
        .collect()
}

/// Whether the report carries no row of `code`.
fn silent(report: &FileReport, code: &str) -> bool {
    report.diagnostics.iter().all(|d| d.code.name != code)
}

/// The absolute fixture offset of `needle` (the rig's prelude included).
fn at(fixture: &str, needle: &str) -> usize {
    CORE_PRELUDE.len() + fixture.find(needle).unwrap()
}

/// `The name '<name>' isn't a type, so it can't be used as a type
/// argument.` at `offset` over `len`.
fn nta(name: &str, offset: usize, len: usize) -> String {
    format!(
        "The name '{name}' isn't a type, so it can't be used as a type argument. @{offset}:{len}"
    )
}

/// `The value of the local variable '<name>' isn't used.` at `offset`
/// over `len`.
fn uv(name: &str, offset: usize, len: usize) -> String {
    format!("The value of the local variable '{name}' isn't used. @{offset}:{len}")
}

/// The co19 ambiguities_A01_t01 shape: `f(a<b, c>(d))` with `a`–`d`
/// locals parses as one method type-argument group (the parser contract
/// gate_2804_sig_swallow pins), and the semantic tier answers both
/// faces — `non_type_as_type_argument` at `b` and at `c` (the oracle's
/// 58:7/61:7 rows), and the read credit that spares every local the
/// `unused_local_variable` row (`a` reads as the instantiation head,
/// `d` as the call argument, `b`/`c` through the type arguments). The
/// never-referenced `e` still reports — the credit never masks a
/// genuinely unused declaration.
#[test]
fn method_type_arguments_over_locals_report_and_credit_reads() {
    let fixture = "String f(a, [b]) => \"$a, $b\";\nmain() {\n  int a = 1;\n  int b = 2;\n  int c = 3;\n  int d = 4;\n  int e = 5;\n  f(a<b,\n      c>(d));\n}\n";
    let report = report_ops(fixture);
    let b = at(fixture, "a<b") + 2;
    let c = at(fixture, "c>(d))");
    assert_eq!(
        nta_rows(&report),
        [nta("b", b, 1), nta("c", c, 1)],
        "{:#?}",
        rows(&report)
    );
    let e = at(fixture, "int e") + 4;
    assert_eq!(uv_rows(&report), [uv("e", e, 1)], "{:#?}", rows(&report));
}

/// The ambiguities_A01_t02 shape: the same call with a generic function
/// head and typedef arguments is a clean instantiation — no argument
/// face, no undefined name, no unused rows (a true type argument is
/// silent through the whole usage cluster).
#[test]
fn true_type_arguments_stay_silent() {
    let fixture = "String f(a, [b]) => \"$a, $b\";\nString a2<T1, T2>(int x) => \"$x\";\ntypedef tb = int;\ntypedef tc = String;\nmain() {\n  int d = 4;\n  f(a2<tb, tc>(d));\n}\n";
    let report = report_ops(fixture);
    assert!(nta_rows(&report).is_empty(), "{:#?}", rows(&report));
    assert!(silent(&report, "undefined_class"), "{:#?}", rows(&report));
    assert!(uv_rows(&report).is_empty(), "{:#?}", rows(&report));
    assert!(silent(&report, "unused_element"), "{:#?}", rows(&report));
}

/// The annotation side of the same face: `List<b>` with a local `b`
/// reports `non_type_as_type_argument` at the argument (the group is a
/// TypeArgumentList whatever its use site) and counts the read, so only
/// the genuinely unread `x` reports.
#[test]
fn annotation_type_arguments_report_and_credit_the_local() {
    let fixture = "main() {\n  int b = 0;\n  List<b> x = [];\n}\n";
    let report = report_ops(fixture);
    let b = at(fixture, "List<b>") + 5;
    assert_eq!(nta_rows(&report), [nta("b", b, 1)], "{:#?}", rows(&report));
    let x = at(fixture, "List<b> x") + 8;
    assert_eq!(uv_rows(&report), [uv("x", x, 1)], "{:#?}", rows(&report));
}

/// The `is`-operand exclusion (`_insideIsExpression`): the argument
/// face still reports inside `1 is List<b>`, but the mention never
/// counts as a read — `b` keeps its `unused_local_variable` row beside
/// `t`'s (assigned, never read).
#[test]
fn is_operand_type_arguments_report_but_never_credit() {
    let fixture = "main() {\n  int b = 0;\n  var t = 1 is List<b>;\n}\n";
    let report = report_ops(fixture);
    let b_arg = at(fixture, "List<b>") + 5;
    assert_eq!(
        nta_rows(&report),
        [nta("b", b_arg, 1)],
        "{:#?}",
        rows(&report)
    );
    let b_decl = at(fixture, "int b") + 4;
    let t = at(fixture, "var t") + 4;
    assert_eq!(
        uv_rows(&report),
        [uv("b", b_decl, 1), uv("t", t, 1)],
        "{:#?}",
        rows(&report)
    );
}

/// The parameter twin: a type argument naming a formal reports through
/// the parameter arm and the read credit spares the declaration the
/// `unused_element_parameter` row (the type argument is the parameter's
/// only reference).
#[test]
fn parameter_type_arguments_report_and_credit_the_parameter() {
    let fixture =
        "void main() {\n  _m(3);\n}\nvoid _m(int p) {\n  void g<T1, T2>() {}\n  g<p, int>();\n}\n";
    let report = report_ops(fixture);
    let p = at(fixture, "g<p") + 2;
    assert_eq!(nta_rows(&report), [nta("p", p, 1)], "{:#?}", rows(&report));
    assert!(
        silent(&report, "unused_element_parameter"),
        "{:#?}",
        rows(&report)
    );
    assert!(uv_rows(&report).is_empty(), "{:#?}", rows(&report));
    assert!(silent(&report, "unused_element"), "{:#?}", rows(&report));
}

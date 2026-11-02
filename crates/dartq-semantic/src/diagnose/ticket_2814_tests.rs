//! The #2814 statement-form operand pins: the for-in iterable
//! (`uncheckedUseOfNullableValueAsIterator`) and the spread operand
//! (`uncheckedUseOfNullableValueInSpread`) faces derive their candidates
//! at the teardown join and re-decide against the flow's read-type rows —
//! an unguarded nullable iterable/operand reports (messages verbatim from
//! messages.yaml's shared-name family), a null-guard promotion silences,
//! the null-aware spread and the non-nullable negatives stay silent, and
//! the condition face (`…as a condition`) answers for a bare `bool?` read
//! and its guard.

use super::test_rig::{CORE_PRELUDE, report_ops, rows};
use super::*;

/// The absolute fixture offset of `needle` (the prelude the rig prepends
/// included).
fn at(fixture: &str, needle: &str) -> usize {
    CORE_PRELUDE.len() + fixture.find(needle).unwrap()
}

/// The `unchecked_use_of_nullable_value` rows as `offset:len message`.
fn unchecked(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unchecked_use_of_nullable_value")
        .map(|d| {
            format!(
                "{} @{}:{} {}",
                d.code.name, d.range.offset, d.range.length, d.message
            )
        })
        .collect()
}

/// Exactly one unchecked row at `offset:len` carrying `message`.
#[track_caller]
fn assert_unchecked(report: &FileReport, offset: usize, len: u32, message: &str) {
    assert_eq!(
        unchecked(report),
        vec![format!(
            "unchecked_use_of_nullable_value @{}:{} {}",
            offset, len, message
        )],
    );
}

#[test]
fn for_in_unguarded_reports_at_the_iterable() {
    let fixture = "f() {\n  List? x;\n  for (var y in x) {}\n}\n";
    let report = report_ops(fixture);
    let x = at(fixture, "x) {}");
    assert_unchecked(
        &report,
        x,
        1,
        "A nullable expression can't be used as an iterator in a for-in loop.",
    );
}

#[test]
fn for_in_guarded_by_null_check_stays_silent() {
    let fixture = "f() {\n  List? x;\n  if (x != null) {\n    for (var y in x) {}\n  }\n}\n";
    let report = report_ops(fixture);
    assert!(
        unchecked(&report).is_empty(),
        "guarded iterable reported: {:#?}",
        unchecked(&report)
    );
}

#[test]
fn for_in_pattern_iterable_reports() {
    let fixture = "f(List? l) {\n  for (var (a, b) in l) {}\n}\n";
    let report = report_ops(fixture);
    let l = at(fixture, "l) {}");
    assert_unchecked(
        &report,
        l,
        1,
        "A nullable expression can't be used as an iterator in a for-in loop.",
    );
}

#[test]
fn await_for_iterable_reports() {
    let fixture = "f(List? s) async {\n  await for (var x in s) {}\n}\n";
    let report = report_ops(fixture);
    let s = at(fixture, "s) {}");
    assert_unchecked(
        &report,
        s,
        1,
        "A nullable expression can't be used as an iterator in a for-in loop.",
    );
}

#[test]
fn for_in_member_chain_reports_at_the_chain() {
    // A member chain's iterable type is the chain's own, not the
    // receiver's: the operand-expression ask grounds the chain record at
    // the member token and reports at the whole iterable expression
    // (the official `.at(iterable)` span — #3632 retired the
    // single-reachable-read attribution).
    let fixture = "class C {\n  List? list;\n}\nf(C c) {\n  for (var x in c.list) {}\n}\n";
    let report = report_ops(fixture);
    let chain = at(fixture, "c.list");
    assert_unchecked(
        &report,
        chain,
        6,
        "A nullable expression can't be used as an iterator in a for-in loop.",
    );
}

#[test]
fn for_in_compound_iterable_reports_at_the_iterable() {
    // The if-null iterable's type is the arms' join; the operand's own
    // type grounds at its last read and reports over the whole
    // expression span.
    let fixture = "f() {\n  List? a, b;\n  for (var x in a ?? b) {}\n}\n";
    let report = report_ops(fixture);
    let compound = at(fixture, "a ?? b");
    assert_unchecked(
        &report,
        compound,
        6,
        "A nullable expression can't be used as an iterator in a for-in loop.",
    );
}

#[test]
fn for_in_spread_iterable_reports_the_spread_only() {
    // The list literal's own type is non-nullable — the iterable face
    // stays silent; the element read's nullable row answers through the
    // spread face.
    let fixture = "f() {\n  List? s;\n  for (var x in [...s]) {}\n}\n";
    let report = report_ops(fixture);
    let s = at(fixture, "s]) {}");
    assert_unchecked(
        &report,
        s,
        1,
        "A nullable expression can't be used in a spread.",
    );
}

#[test]
fn c_style_for_stays_silent() {
    let fixture = "f() {\n  for (var i = 0; i < 3; i++) {\n    print(i);\n  }\n}\n";
    let report = report_ops(fixture);
    assert!(
        unchecked(&report).is_empty(),
        "C-style for reported: {:#?}",
        unchecked(&report)
    );
}

#[test]
fn for_in_non_nullable_iterable_stays_silent() {
    let fixture = "f() {\n  List x;\n  for (var y in x) {\n    print(y);\n  }\n}\n";
    let report = report_ops(fixture);
    assert!(
        unchecked(&report).is_empty(),
        "non-nullable iterable reported: {:#?}",
        unchecked(&report)
    );
}

#[test]
fn spread_reports_at_the_operand() {
    let fixture = "f() {\n  List? s;\n  var y = [...s];\n  print(y);\n}\n";
    let report = report_ops(fixture);
    let s = at(fixture, "s];");
    assert_unchecked(
        &report,
        s,
        1,
        "A nullable expression can't be used in a spread.",
    );
}

#[test]
fn map_spread_reports_at_the_operand() {
    let fixture = "f() {\n  Map? m;\n  var y = {...m};\n  print(y);\n}\n";
    let report = report_ops(fixture);
    let m = at(fixture, "m};");
    assert_unchecked(
        &report,
        m,
        1,
        "A nullable expression can't be used in a spread.",
    );
}

#[test]
fn null_aware_spread_stays_silent() {
    let fixture = "f() {\n  List? s;\n  var y = [...?s];\n  print(y);\n}\n";
    let report = report_ops(fixture);
    assert!(
        unchecked(&report).is_empty(),
        "null-aware spread reported: {:#?}",
        unchecked(&report)
    );
}

#[test]
fn spread_non_nullable_operand_stays_silent() {
    let fixture = "f() {\n  List s;\n  var y = [...s];\n  print(y);\n}\n";
    let report = report_ops(fixture);
    assert!(
        unchecked(&report).is_empty(),
        "non-nullable spread reported: {:#?}",
        unchecked(&report)
    );
}

#[test]
fn rest_pattern_binding_stays_silent() {
    // A rest pattern's sub-pattern is a binding, not a read — no row
    // stands in the window and the spread face never arms.
    let fixture = "f(List? l) {\n  if (l case [1, ...rest]) {\n    print(rest);\n  }\n}\n";
    let report = report_ops(fixture);
    assert!(
        unchecked(&report).is_empty(),
        "rest pattern reported: {:#?}",
        unchecked(&report)
    );
}

#[test]
fn condition_bare_bool_nullable_read_reports() {
    let fixture = "f() {\n  bool? b;\n  if (b) {\n    b = null;\n  }\n}\n";
    let report = report_ops(fixture);
    let b = at(fixture, "b) {");
    assert_unchecked(
        &report,
        b,
        1,
        "A nullable expression can't be used as a condition.",
    );
}

#[test]
fn condition_guarded_read_stays_silent() {
    let fixture =
        "f() {\n  bool? b;\n  if (b != null) {\n    if (b) {\n      print(b);\n    }\n  }\n}\n";
    let report = report_ops(fixture);
    assert!(
        unchecked(&report).is_empty(),
        "guarded condition reported: {:#?}",
        unchecked(&report)
    );
}

#[test]
fn rows_helper_shows_the_family_shape() {
    // The family shares one code: the three statement-form texts and the
    // property-access text answer under the same name (a smoke pin over
    // the shared report shape).
    let fixture = "f() {\n  List? s;\n  var y = [...s];\n  print(y);\n}\n";
    let report = report_ops(fixture);
    assert!(
        rows(&report)
            .iter()
            .any(|r| r.contains("A nullable expression can't be used in a spread."))
    );
}

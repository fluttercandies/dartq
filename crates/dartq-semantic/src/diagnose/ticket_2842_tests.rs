//! The #2842 pins: the local-function tear-off operand face at an
//! `is`-check (`type_alias_test.dart`'s `plus is Fun` family — a frame
//! local, so the top-level and member tear-off faces refused it and the
//! operand resolved unknown, the verdict early-returns informative). The
//! capture harvested at the local function's own frame close answers the
//! declared signature as the operand's static type, and the strict
//! verdict chain folds the matching pairs. The negative pins: a
//! non-matching pair stays informative, a shadowing local variable's
//! read never takes the channel, and the top-level tear-off face stays
//! green over the shared `tear_off_fn_ty` conversion.

use super::test_rig::{CORE_PRELUDE, report_ops};
use super::*;

/// The `unnecessary_type_check` rows as the exact anchored source text,
/// source order.
fn ut_spans(report: &FileReport, full: &str) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unnecessary_type_check")
        .map(|d| {
            full[d.range.offset as usize..(d.range.offset + d.range.length) as usize].to_string()
        })
        .collect()
}

/// The corpus's own pairs (`type_alias_test.dart:109/110`, the `plus`
/// block): the local function's tear-off reads as the operand's static
/// type, and each matching `is` folds constant-true — one
/// `unnecessary_type_check` row per check, anchored at the is-expression.
#[test]
fn local_fn_tear_off_matching_pairs_fold() {
    let fixture = concat!(
        "typedef Fun(Never a, Never b);\n",
        "typedef int IntFun(Never a, Never b);\n",
        "void t() {\n",
        "  int plus(int a, [int b = 1]) {\n",
        "    return a + b;\n",
        "  }\n",
        "  plus is Fun;\n",
        "  plus is IntFun;\n",
        "}\n",
    );
    let report = report_ops(fixture);
    let full = format!("{CORE_PRELUDE}{fixture}");
    assert_eq!(
        ut_spans(&report, &full),
        ["plus is Fun", "plus is IntFun"],
        "{:?}",
        report.diagnostics
    );
}

/// A non-matching pair stays informative (`plus is CompareString` — the
/// formal column never proves): no constant verdict, the check remains a
/// runtime question.
#[test]
fn local_fn_tear_off_non_matching_stays_informative() {
    let fixture = concat!(
        "typedef int CmpStr(String a, String b, [bool swap]);\n",
        "void t() {\n",
        "  int plus(int a, [int b = 1]) {\n",
        "    return a + b;\n",
        "  }\n",
        "  plus is CmpStr;\n",
        "}\n",
    );
    let report = report_ops(fixture);
    assert!(
        report.diagnostics.is_empty(),
        "the mismatched pair must stay informative: {:?}",
        report.diagnostics
    );
}

/// The optional-formal boundary rides the capture: `swap`'s required
/// count is the two positionals before `[` (a required-everything
/// capture would refuse the pair on the leftover-required rule and drop
/// the row — `type_alias_test.dart:68/77`'s `is CompareString` face).
#[test]
fn local_fn_optional_formals_keep_the_required_boundary() {
    let fixture = concat!(
        "typedef int CmpStr(String a, String b, [bool swap]);\n",
        "void t() {\n",
        "  int rev(String a, String b, [bool s = false]) {\n",
        "    return 1;\n",
        "  }\n",
        "  rev is CmpStr;\n",
        "}\n",
    );
    let report = report_ops(fixture);
    let full = format!("{CORE_PRELUDE}{fixture}");
    assert_eq!(
        ut_spans(&report, &full),
        ["rev is CmpStr"],
        "{:?}",
        report.diagnostics
    );
}

/// A shadowing local VARIABLE's read never takes the channel: the inner
/// frame's `locals` entry answers before any outer capture (the
/// resolution precedence), so the shadowed check stays live while the
/// unshadowed read folds — the tear-off face and the variable face
/// coexist on one spelling.
#[test]
fn shadowed_local_variable_read_stays_out_of_the_channel() {
    let fixture = concat!(
        "typedef Fun(Never a, Never b);\n",
        "void t() {\n",
        "  int gi(int a, [int b = 1]) => a;\n",
        "  void u() {\n",
        "    int gi = 3;\n",
        "    if (gi is! Fun) throw 'bad';\n",
        "  }\n",
        "  u();\n",
        "  if (gi is! Fun) throw 'bad';\n",
        "}\n",
    );
    let report = report_ops(fixture);
    let full = format!("{CORE_PRELUDE}{fixture}");
    // Only the unshadowed tear-off read takes the channel: exactly one
    // verdict row, on the outer read (`is!` over a proven subtype — the
    // always-false row); the shadowed variable read stays informative.
    // (The dead-arm fold over a local-function spelling is the flow
    // layer's channel — the #2835 staging answers top-level spellings
    // only — so this pin asserts the verdict rows.)
    assert_eq!(
        ut_spans(&report, &full),
        ["gi is! Fun"],
        "{:?}",
        report.diagnostics
    );
}

/// The top-level tear-off face stays green over the shared conversion
/// (`#2835`'s written-signature pair — the arm this ticket refactored
/// into [`Engine::tear_off_fn_ty`]).
#[test]
fn top_level_tear_off_stays_green() {
    let fixture = concat!(
        "typedef fi(int i, );\n",
        "int topi(int i, ) => i;\n",
        "void t() {\n",
        "  if (topi is! fi) throw 'bad';\n",
        "}\n",
    );
    let report = report_ops(fixture);
    let full = format!("{CORE_PRELUDE}{fixture}");
    let dead: Vec<String> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "dead_code")
        .map(|d| {
            full[d.range.offset as usize..(d.range.offset + d.range.length) as usize].to_string()
        })
        .collect();
    assert_eq!(dead, ["throw 'bad';"], "{:?}", report.diagnostics);
}

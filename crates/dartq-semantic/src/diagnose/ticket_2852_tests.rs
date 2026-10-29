//! The #2852 pins: the dead `throw` arm over a frame-local function's
//! tear-off is-check (`if (gi is! Fun) throw 'bad';` — the #2835 channel
//! extended from top-level spellings to local ones). #2842's capture
//! already answers the verdict half engine-side (`unnecessary_type_check`
//! on the is-expression); the listener's staging branch + the adapter's
//! fold-time `local_fn_shapes` answer the flow half, so BOTH oracle rows
//! land: the verdict row stays and the throw arm reports `dead_code`
//! anchored at the statement. Non-matching pairs stay live, and a local
//! variable's read — plain or shadowing the function spelling — never
//! takes the channel.

use super::test_rig::{CORE_PRELUDE, report_ops};
use super::*;

/// The diagnostics named `code` as the exact anchored source text, source
/// order.
fn spans(report: &FileReport, full: &str, code: &str) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| {
            full[d.range.offset as usize..(d.range.offset + d.range.length) as usize].to_string()
        })
        .collect()
}

/// The ticket's own shape: the local function's tear-off operand folds the
/// `is!` constant-false against the matching alias, so the throw arm deads
/// (`throw 'bad';` at its statement) while the verdict row stays — both
/// oracle rows on one check.
#[test]
fn local_fn_dead_throw_arm_folds_beside_the_verdict_row() {
    let fixture = concat!(
        "typedef Fun(Never a, Never b);\n",
        "void t() {\n",
        "  int gi(int a, [int b = 1]) => a;\n",
        "  if (gi is! Fun) throw 'bad';\n",
        "}\n",
    );
    let report = report_ops(fixture);
    let full = format!("{CORE_PRELUDE}{fixture}");
    assert_eq!(
        spans(&report, &full, "dead_code"),
        ["throw 'bad';"],
        "{:?}",
        report.diagnostics
    );
    assert_eq!(
        spans(&report, &full, "unnecessary_type_check"),
        ["gi is! Fun"],
        "{:?}",
        report.diagnostics
    );
}

/// A non-matching pair stays informative: the positional-signature operand
/// against a named-only alias is a real runtime question — no constant
/// verdict and no dead arm.
#[test]
fn local_fn_non_matching_pair_keeps_both_arms_live() {
    let fixture = concat!(
        "typedef int CmpStr(String a, String b, [bool swap]);\n",
        "void t() {\n",
        "  int gi(int a, [int b = 1]) => a;\n",
        "  if (gi is! CmpStr) throw 'bad';\n",
        "}\n",
    );
    let report = report_ops(fixture);
    assert!(
        report.diagnostics.is_empty(),
        "the mismatched pair must stay live: {:?}",
        report.diagnostics
    );
}

/// A local variable's bare read never enters the function channel: the
/// operand rides the reference path on its own declared type, and the
/// is-check stays a runtime question whatever the tested alias folds to.
#[test]
fn local_variable_read_stays_out_of_the_channel() {
    let fixture = concat!(
        "typedef Fun(Never a, Never b);\n",
        "void t() {\n",
        "  int gi = 3;\n",
        "  if (gi is! Fun) throw 'bad';\n",
        "}\n",
    );
    let report = report_ops(fixture);
    assert!(
        report.diagnostics.is_empty(),
        "a local variable read must not take the tear-off face: {:?}",
        report.diagnostics
    );
}

/// A shadowing local variable splits one spelling's reads: the inner
/// frame's plain local answers before any outer capture (the resolution
/// precedence), so the shadowed check stays informative while the
/// unshadowed read folds BOTH rows — the tear-off face and the variable
/// face coexist on one name.
#[test]
fn shadowed_variable_read_stays_out_while_the_outer_read_folds() {
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
    // Only the unshadowed tear-off read takes the channel: its dead arm
    // and verdict row; the shadowed variable read stays informative.
    assert_eq!(
        spans(&report, &full, "dead_code"),
        ["throw 'bad';"],
        "{:?}",
        report.diagnostics
    );
    assert_eq!(
        spans(&report, &full, "unnecessary_type_check"),
        ["gi is! Fun"],
        "{:?}",
        report.diagnostics
    );
}

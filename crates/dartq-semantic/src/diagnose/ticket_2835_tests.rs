//! The #2835 pins: the dead `throw` arm over the conformance suite's
//! container-less bare-read is-check shape (`arg_param_trailing_comma`'s
//! `if (topx is! fx) throw 'bad';` family — the dead_code half of the 211
//! cases whose verdict half #2813 landed).
//!
//! The seam: the diagnose listener's implicit-`this` staging block gated
//! on `engine.container_context()` never answered a container-less bare
//! read, so the flow's unknown-read arm kept the operand unknown and the
//! `is_test_guaranteed` fold refused it — no constant verdict, no dead
//! branch. The else-branch stages a top-level function's OWN spelling at
//! the read's offset; the resolved snapshot's `fn_shapes` (the tear-off
//! signature flattened beside `typedef_shapes`) is what `map_name`
//! resolves the spelling to, and the existing is-test consumer folds the
//! pair honestly — matching pairs dead the throw arm, non-matching pairs
//! stay live.

use super::test_rig::{CORE_PRELUDE, report_ops};
use super::*;

/// The `dead_code` rows as the exact anchored source text, source order.
fn dead_spans(report: &FileReport, full: &str) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "dead_code")
        .map(|d| {
            full[d.range.offset as usize..(d.range.offset + d.range.length) as usize].to_string()
        })
        .collect()
}

/// The corpus's five trailing-comma pairs: each `if (topX is! fX) throw
/// 'bad';` folds constant-false, so the throw arm reports `dead_code`
/// anchored at the throw statement (the oracle's `161:20|22`-shaped row)
/// beside the #2813 verdict row.
#[test]
fn matching_pairs_dead_the_throw_arm() {
    let fixture = concat!(
        "typedef fx(x, );\n",
        "typedef fy([y, ]);\n",
        "typedef fxy(x, [y, ]);\n",
        "typedef fz({z, });\n",
        "typedef fxz(x, {z, });\n",
        "void topx(x, ) {}\n",
        "void topy([y, ]) {}\n",
        "void topxy(x, [y, ]) {}\n",
        "void topz({z, }) {}\n",
        "void topxz(x, {z, }) {}\n",
        "void testCalls() {\n",
        "  if (topx is! fx) throw 'bad';\n",
        "  if (topy is! fy) throw 'bad';\n",
        "  if (topxy is! fxy) throw 'bad';\n",
        "  if (topz is! fz) throw 'bad';\n",
        "  if (topxz is! fxz) throw 'bad';\n",
        "}\n",
    );
    let report = report_ops(fixture);
    let full = format!("{CORE_PRELUDE}{fixture}");
    assert_eq!(
        dead_spans(&report, &full),
        vec!["throw 'bad';".to_string(); 5],
        "{:?}",
        report.diagnostics
    );
    // The verdict half (already landed) rides the same pairs.
    let verdicts = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unnecessary_type_check")
        .count();
    assert_eq!(verdicts, 5, "{:?}", report.diagnostics);
}

/// A single pair, minimal: the fold rides the staged tear-off spelling
/// (`void Function(dynamic)` against the alias's synthesized
/// `dynamic Function(dynamic)` — return covariance over the tops).
#[test]
fn single_pair_dead_arm_with_exact_anchor() {
    let fixture = concat!(
        "typedef fx(x, );\n",
        "void topx(x, ) {}\n",
        "void t() {\n",
        "  if (topx is! fx) throw 'bad';\n",
        "}\n",
    );
    let report = report_ops(fixture);
    let full = format!("{CORE_PRELUDE}{fixture}");
    assert_eq!(dead_spans(&report, &full), ["throw 'bad';"]);
}

/// A written return keeps its own face: `int Function()` against the
/// alias's `int Function(int)` folds constant-false on the formal
/// column, the throw arm deads.
#[test]
fn written_signature_pair_folds() {
    let fixture = concat!(
        "typedef fi(int i, );\n",
        "int topi(int i, ) => i;\n",
        "void t() {\n",
        "  if (topi is! fi) throw 'bad';\n",
        "}\n",
    );
    let report = report_ops(fixture);
    let full = format!("{CORE_PRELUDE}{fixture}");
    assert_eq!(dead_spans(&report, &full), ["throw 'bad';"]);
}

/// A non-matching pair stays informative: the positional-signature
/// operand against a named-only alias is a real runtime question — no
/// constant verdict and no dead arm.
#[test]
fn non_matching_pair_keeps_both_arms_live() {
    let fixture = concat!(
        "typedef fz({z, });\n",
        "void topx(x, ) {}\n",
        "void t() {\n",
        "  if (topx is! fz) throw 'bad';\n",
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
        "typedef fx(x, );\n",
        "void t(Object v) {\n",
        "  if (v is! fx) throw 'bad';\n",
        "}\n",
    );
    let report = report_ops(fixture);
    assert!(
        report.diagnostics.is_empty(),
        "a local read must not take the tear-off face: {:?}",
        report.diagnostics
    );
}

/// A top-level GETTER's read is its return type, never a tear-off: the
/// staging gate refuses the formal-table-less declaration and the is-check
/// stays live (the operand face keeps the unknown arm's posture).
#[test]
fn top_level_getter_read_stays_live() {
    let fixture = concat!(
        "typedef fx(x, );\n",
        "int get topg => 0;\n",
        "void t() {\n",
        "  if (topg is! fx) throw 'bad';\n",
        "}\n",
    );
    let report = report_ops(fixture);
    assert!(
        report.diagnostics.is_empty(),
        "a getter read must not take the tear-off face: {:?}",
        report.diagnostics
    );
}

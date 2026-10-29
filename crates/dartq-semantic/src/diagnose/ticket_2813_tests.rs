//! The #2813 pins: the `unnecessary_type_check` verdict over the
//! conformance suite's dominant container-less bare-read is-check shape
//! (`arg_param_trailing_comma`'s `topx is! fx` family, 1,055 missing
//! entries over 211 cases at the #2833 recount).
//!
//! The seam the staging closes is the old-style function-typed alias:
//! `typedef fx(x, );` (no `=`) captured `var_ty = Ty::UNKNOWN` in the
//! collector, so the tested side never folded and the verdict's strict
//! ask read `void Function(dynamic) <: fx` as a Function-vs-Named pair —
//! always false, never constant. The collector now synthesizes the alias
//! right-hand face as a first-class function type (the written return
//! over the formal columns, omitted annotations reading `dynamic`, the
//! analyzer element's face), and the verdict normalizes both sides'
//! captured signatures through the official leaves
//! ([`Engine::official_fn_face`]) before the strict Function rule
//! (return covariance, formals contravariance) answers — the relation
//! rides real signatures inside the engine, never the flow lattice's
//! signature-less `Function` refusal (resolved_ops' `is_subtype_typed`
//! deliberately refuses signature-less pairs; the resolved snapshot's
//! `typedef_shapes` now carries the old-style alias's real shape, so the
//! flow's tested-side map resolves it there too).
//!
//! The shape's OTHER oracle row — the dead `throw` arm — rides the flow
//! fold (`isExpression_end`'s constant-verdict branch split), and the
//! flow operand for a container-less bare top-level function read stays
//! the unknown face: the bare-read adoption channel
//! (`Flow::stage_this_member_type`, the only `Ev::Ident` unknown-arm
//! adoption) is written solely by the diagnose listener's
//! container-gated implicit-`this` block, which a container-less read
//! never enters — a listener.rs seam this ticket does not touch (live
//! lane); the dead-arm half is reported, not pinned.

use super::test_rig::{CORE_PRELUDE, report_ops};
use super::*;

/// The `unnecessary_type_check` rows as `message @offset`, source order.
fn checks(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unnecessary_type_check")
        .map(|d| format!("{} @{}", d.message, d.range.offset))
        .collect()
}

/// The absolute fixture offset of `needle` (the rig's prelude included).
fn at(fixture: &str, needle: &str) -> usize {
    CORE_PRELUDE.len() + fixture.find(needle).unwrap()
}

/// The corpus's five trailing-comma pairs — every `topX` function against
/// its matching `fX` old-style alias — folds constant-false: the operand's
/// tear-off signature subtypes the alias's synthesized face, so each
/// `is!` reports `unnecessary_type_check` anchored at the is-expression
/// (the operand head through the written type, the oracle's
/// `161:7|11`-shaped span).
#[test]
fn matching_old_style_pairs_fold_constant_false() {
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
    let rows = checks(&report);
    let expected: Vec<String> = [
        "topx is! fx",
        "topy is! fy",
        "topxy is! fxy",
        "topz is! fz",
        "topxz is! fxz",
    ]
    .iter()
    .map(|needle| {
        format!(
            "Unnecessary type check; the result is always 'false'. @{}",
            at(fixture, needle)
        )
    })
    .collect();
    assert_eq!(rows, expected, "{:?}", report.diagnostics);
}

/// The positive polarity over the same pair folds constant-true (`is`
/// keeps its then arm; the row is the verdict alone).
#[test]
fn positive_polarity_folds_constant_true() {
    let fixture = "typedef fx(x, );\nvoid topx(x, ) {}\nvoid t() {\n  topx is fx;\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        checks(&report),
        vec![format!(
            "Unnecessary type check; the result is always 'true'. @{}",
            at(fixture, "topx is fx")
        )],
        "{:?}",
        report.diagnostics
    );
}

/// A non-matching pair stays informative: the positional-signature
/// operand against a named-only alias is a real runtime question (the
/// formals-contravariance walk answers false), so no constant row fires —
/// the channel carries the resolved relation, not a vacuous verdict.
#[test]
fn non_matching_pair_stays_informative() {
    let fixture = concat!(
        "typedef fz({z, });\n",
        "void topx(x, ) {}\n",
        "void t() {\n",
        "  topx is! fz;\n",
        "}\n",
    );
    let report = report_ops(fixture);
    assert!(
        checks(&report).is_empty(),
        "the mismatched pair must stay informative: {:?}",
        report.diagnostics
    );
}

/// A local variable's bare read never enters the function channel: the
/// operand is the local's own declared type, and the is-check stays a
/// runtime question whatever the tested alias folds to.
#[test]
fn local_variable_read_stays_out_of_the_channel() {
    let fixture = concat!(
        "typedef fx(x, );\n",
        "void t(Object v) {\n",
        "  v is! fx;\n",
        "}\n",
    );
    let report = report_ops(fixture);
    assert!(
        checks(&report).is_empty(),
        "a local read must not take the tear-off face: {:?}",
        report.diagnostics
    );
}

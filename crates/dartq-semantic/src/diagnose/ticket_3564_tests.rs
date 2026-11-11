//! #3564 — the typedef-alias `is` target carries its honest
//! required-positional count.
//!
//! `alias_function_ty`'s capture arm interned the folded alias face
//! through the split-blind all-required producer, so the shape's own
//! `required_positional` answered `positional.len()` — the count
//! `fn_face_sig`'s fallback reads. The arity gate
//! (`_isFunctionSubtypeOf`'s required rule, upstream Dart SDK source,
//! pkg/analyzer/lib/src/dart/element/subtype.dart:363-369) then read the
//! target as all-required and proved `req is TOpt` constant-true where
//! the oracle stays silent. The fold now interns through the honest
//! producer, so the two spellings land on distinct ids and the gate
//! decides on the real split.
//!
//! Oracle rows below were captured against `dart analyze --format
//! machine` 3.13.7: a required-positional source against an
//! optional-positional alias target is statically FALSE and the oracle
//! reports nothing; every required/identical polarity in the family
//! reports exactly one `unnecessary_type_check` ("always 'true'") row.

use super::test_rig::report_ops;
use super::*;

fn type_check_rows(report: &FileReport) -> usize {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unnecessary_type_check")
        .count()
}

fn all_codes(report: &FileReport) -> Vec<&'static str> {
    report.diagnostics.iter().map(|d| d.code.name).collect()
}

// —— The ticket fixture: required operand, optional-positional alias ————

/// `req is TOpt` where `req: void Function(int)` and
/// `TOpt = void Function(int, [int])`: the source lacks the target's
/// second positional formal, so the check is statically false and the
/// oracle stays silent. Under the all-required target face the arity
/// gates passed and the row was fabricated.
#[test]
fn required_operand_vs_optional_target_alias_stays_silent() {
    let report = report_ops(
        "typedef TOpt = void Function(int, [int]);\n\
         void req(int x) {}\n\
         void probe() { print(req is TOpt); }",
    );
    assert!(
        type_check_rows(&report) == 0,
        "a required source cannot be a subtype of a wider optional \
         target: {:?}",
        all_codes(&report)
    );
}

/// The #3511 residual verbatim: `req is TOpt` where
/// `TOpt = void Function([int])` is ALL-optional — the source-required
/// formal at the target's optional position answers false
/// (subtype.dart:363-369), so the oracle is silent.
#[test]
fn required_operand_vs_all_optional_alias_stays_silent() {
    let report = report_ops(
        "typedef TOpt = void Function([int]);\n\
         void req(int i) {}\n\
         void probe() { print(req is TOpt); }",
    );
    assert!(
        type_check_rows(&report) == 0,
        "source-required against target-optional is false: {:?}",
        all_codes(&report)
    );
}

// —— The true-direction controls the producer swap must not flip ————

/// `opt is TReq` where `opt: void Function([int])` and
/// `TReq = void Function(int)`: an optional source formal implements a
/// required target formal, so the check is constant-true (oracle: one
/// row). The honest target shape must keep this row.
#[test]
fn optional_operand_vs_required_alias_reports_true() {
    let report = report_ops(
        "typedef TReq = void Function(int);\n\
         void opt([int i]) {}\n\
         void probe() { print(opt is TReq); }",
    );
    assert_eq!(
        type_check_rows(&report),
        1,
        "an optional source implements a required target: {:?}",
        all_codes(&report)
    );
}

/// An operand DECLARED with the alias annotation carries the same
/// honest face as the target: `TOpt opt = ...; opt is TOpt` is the
/// identical spelling (oracle: one row). The alias-typed declaration's
/// face must not stay on the shared all-required id the repair just
/// left.
#[test]
fn alias_declared_operand_vs_identical_alias_reports_true() {
    let report = report_ops(
        "typedef TOpt = void Function([int]);\n\
         void probe(TOpt opt) { print(opt is TOpt); }",
    );
    assert_eq!(
        type_check_rows(&report),
        1,
        "the identical spelling through an alias-typed declaration is \
         constant-true: {:?}",
        all_codes(&report)
    );
}

/// The full four-polarity probe fixture, one row per true polarity:
/// `req is TOpt` (false, silent), `opt is TReq` (true), `opt is TOpt`
/// (identical spelling, true), `req is TReq` (identical spelling,
/// true) — the oracle reports exactly the three true rows.
#[test]
fn four_polarity_fixture_matches_oracle_row_count() {
    let report = report_ops(
        "typedef TOpt = void Function([int?]);\n\
         typedef TReq = void Function(int);\n\
         void req(int i) {}\n\
         void opt([int? i]) {}\n\
         void main() {\n\
         \x20 if (req is TOpt) print(1);\n\
         \x20 if (opt is TReq) print(2);\n\
         \x20 if (opt is TOpt) print(3);\n\
         \x20 if (req is TReq) print(4);\n\
         }",
    );
    assert_eq!(
        type_check_rows(&report),
        3,
        "the oracle reports the three true polarities only: {:?}",
        all_codes(&report)
    );
}
